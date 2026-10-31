pub use crate::keys::{Action, Binding, bindings, dispatch, validate_bindings};
use crate::{
    command::{self, Command},
    domain::*,
    input::Input,
    mime::ParsedMessage,
    storage::{MailStore, PAGE_SIZE},
    tree::{self, FolderKey, FolderPage, Node, RowKey, TreeData, TreeRow},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::mpsc;

pub const MAX_LOADED: usize = 2000;

pub fn attachment_matches(attachments: &[crate::mime::Attachment], query: &str) -> Vec<usize> {
    let mut matches: Vec<_> = attachments
        .iter()
        .enumerate()
        .filter_map(|(index, attachment)| {
            fuzzy_score(&attachment.name, query).map(|score| (score, index))
        })
        .collect();
    matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    matches.into_iter().map(|(_, index)| index).collect()
}

pub fn file_matches(entries: &[crate::files::FileEntry], query: &str) -> Vec<usize> {
    let mut matches: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| fuzzy_score(&entry.name, query).map(|score| (score, index)))
        .collect();
    matches.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| entries[b.1].directory.cmp(&entries[a.1].directory))
            .then_with(|| {
                entries[a.1]
                    .name
                    .to_lowercase()
                    .cmp(&entries[b.1].name.to_lowercase())
            })
    });
    matches.into_iter().map(|(_, index)| index).collect()
}

/// Subsequence matching with contiguous/boundary bonuses, like an fzf picker.
pub(crate) fn fuzzy_score(candidate: &str, query: &str) -> Option<usize> {
    let candidate = candidate.to_lowercase();
    let chars: Vec<_> = candidate.chars().collect();
    let mut score = 0;
    for word in query.to_lowercase().split_whitespace() {
        let mut next = 0;
        let mut previous = None;
        for needle in word.chars() {
            let position = chars
                .iter()
                .enumerate()
                .skip(next)
                .find(|(_, c)| **c == needle)?
                .0;
            score += 10
                + usize::from(previous.is_some_and(|p| p + 1 == position)) * 10
                + usize::from(position == 0 || chars[position - 1] == '/') * 5;
            next = position + 1;
            previous = Some(position);
        }
    }
    Some(score)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Messages,
    Calendar,
    Contacts,
    Tasks,
    Accounts,
    Detail,
    Pending,
    Status,
    Help,
    History,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PimRow {
    Collection {
        index: usize,
        count: usize,
        expanded: bool,
    },
    Entry(usize),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PimRowKey {
    Collection(String),
    Entry {
        collection: String,
        file: std::path::PathBuf,
    },
}
impl View {
    pub fn is_pim(self) -> bool {
        matches!(self, Self::Calendar | Self::Contacts | Self::Tasks)
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Messages => "Messages",
            Self::Calendar => "Calendar",
            Self::Contacts => "Contacts",
            Self::Tasks => "Tasks",
            Self::Accounts => "Accounts",
            Self::Detail => "Message",
            Self::Pending => "Outbox / pending",
            Self::Status => "Diagnostics",
            Self::Help => "Keyboard help",
            Self::History => "Command history",
        }
    }
}

#[derive(Clone, Debug)]
pub enum InputMode {
    Command,
    Filter,
    Move,
    Copy,
}
#[derive(Clone, Debug)]
pub enum Overlay {
    Assistant,
    Attachments {
        query: Input,
        row: usize,
        selected: HashSet<usize>,
    },
    SaveAttachments {
        picker: Box<Overlay>,
        directory: Input,
        error: Option<String>,
    },
    Find {
        query: Input,
        row: usize,
        messages: Vec<Message>,
        loading: bool,
        again: bool,
        account: Option<String>,
        error: Option<String>,
    },
    Move {
        ids: Vec<MessageId>,
        query: Input,
        row: usize,
    },
    CreateFolder {
        account: String,
        anchor: String,
        parent: bool,
        name: Input,
    },
    Compose {
        account: usize,
        field: usize,
        fields: [Input; 3],
        attachments: Vec<std::path::PathBuf>,
    },
    Files {
        composer: Box<Overlay>,
        path: std::path::PathBuf,
        entries: Vec<crate::files::FileEntry>,
        query: Input,
        row: usize,
        selected: HashSet<std::path::PathBuf>,
        loading: bool,
        error: Option<String>,
    },
    Help,
    Confirm {
        ids: Vec<MessageId>,
        confirm: bool,
    },
    Aliases,
    Quit,
    Url(String),
    TaskForm(Box<crate::task_form::TaskEditor>),
    PimForm {
        contact: bool,
        collection: usize,
        original: Option<usize>,
        fields: Vec<Input>,
        field: usize,
        error: Option<String>,
        book_menu: Option<usize>,
        contact_page: usize,
    },
    PimDelete {
        contact: bool,
        index: usize,
        confirm: bool,
    },
    PimDetails(String),
}

#[derive(Clone)]
struct Navigation {
    view: View,
    key: Option<RowKey>,
    row: usize,
    offset: usize,
    filter: String,
    account: Option<String>,
    messages: Vec<Message>,
    pages: HashMap<FolderKey, FolderPage>,
    expanded: HashSet<FolderKey>,
    collapsed_accounts: HashSet<String>,
    search: String,
    unread: bool,
    flagged: bool,
    body_scroll: usize,
    detail: Option<Message>,
    revision: u64,
    initialized: bool,
}

pub enum Request {
    Pim {
        generation: u64,
        calendars: Vec<crate::config::PimCollection>,
        books: Vec<crate::config::PimCollection>,
        tasks: Vec<crate::config::PimCollection>,
        change: Box<crate::pim::Change>,
        clock: Option<(chrono::DateTime<chrono::Utc>, Box<crate::pim::Snapshot>)>,
    },
    SaveAttachments {
        generation: u64,
        directory: std::path::PathBuf,
        attachments: Vec<crate::mime::Attachment>,
    },
    Pending {
        generation: u64,
    },
    Find {
        generation: u64,
        query: String,
        account: Option<String>,
    },
    Files {
        generation: u64,
        path: std::path::PathBuf,
    },
    CreateFolder {
        generation: u64,
        account: String,
        anchor: String,
        parent: Option<String>,
        name: String,
    },
    Compose {
        generation: u64,
        draft: Draft,
        send: bool,
    },
    Load {
        generation: u64,
        account: Option<String>,
        open: Option<Vec<FolderKey>>,
    },
    Folder {
        generation: u64,
        key: FolderKey,
        cursor: Option<Cursor>,
        append: bool,
        limit: usize,
    },
    Detail {
        generation: u64,
        id: MessageId,
    },
    Search {
        generation: u64,
        query: String,
        account: Option<String>,
    },
    Mutate {
        generation: u64,
        ids: Vec<MessageId>,
        mutation: Mutation,
    },
}
pub enum Event {
    Pim {
        generation: u64,
        result: Result<crate::pim::Snapshot, String>,
        write: bool,
    },
    SavedAttachments {
        generation: u64,
        result: Result<Vec<std::path::PathBuf>, String>,
    },
    Pending {
        generation: u64,
        result: Result<Vec<PendingMail>, String>,
    },
    Submitted {
        generation: u64,
        account: String,
        result: MutationResult,
        pending: Vec<PendingMail>,
    },
    Found {
        generation: u64,
        query: String,
        messages: Result<Vec<Message>, String>,
    },
    Files {
        generation: u64,
        path: std::path::PathBuf,
        entries: Result<Vec<crate::files::FileEntry>, String>,
    },
    Loaded {
        generation: u64,
        accounts: Vec<Account>,
        folders: Vec<Folder>,
        pages: Vec<(FolderKey, Result<Page, String>)>,
        pending: Vec<PendingMail>,
    },
    Folder {
        generation: u64,
        key: FolderKey,
        append: bool,
        page: Result<Page, String>,
    },
    Detail {
        generation: u64,
        parsed: ParsedMessage,
    },
    Search {
        generation: u64,
        messages: Vec<Message>,
        accounts: Vec<Account>,
        folders: Vec<Folder>,
    },
    Progress {
        generation: u64,
        count: u64,
    },
    Mutation {
        generation: u64,
        result: MutationResult,
    },
    Error {
        generation: u64,
        error: String,
    },
}
impl Event {
    fn generation(&self) -> u64 {
        match self {
            Self::Pim { generation, .. } => *generation,
            Self::SavedAttachments { generation, .. } => *generation,
            Self::Pending { generation, .. } | Self::Submitted { generation, .. } => *generation,
            Self::Found { generation, .. } => *generation,
            Self::Files { generation, .. } => *generation,
            Self::Loaded { generation, .. }
            | Self::Folder { generation, .. }
            | Self::Detail { generation, .. }
            | Self::Search { generation, .. }
            | Self::Progress { generation, .. }
            | Self::Mutation { generation, .. }
            | Self::Error { generation, .. } => *generation,
        }
    }
}

pub fn start_worker(
    store: Box<dyn MailStore>,
    current: Arc<AtomicU64>,
) -> (
    mpsc::Sender<Request>,
    mpsc::Receiver<Event>,
    tokio::task::JoinHandle<()>,
) {
    start_worker_with_index(store, current, None)
}
pub fn start_worker_with_index(
    mut store: Box<dyn MailStore>,
    current: Arc<AtomicU64>,
    index_path: Option<std::path::PathBuf>,
) -> (
    mpsc::Sender<Request>,
    mpsc::Receiver<Event>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, mut commands) = mpsc::channel::<Request>(8);
    let (events, rx) = mpsc::channel(16);
    let worker = tokio::task::spawn_blocking(move || {
        while let Some(command) = commands.blocking_recv() {
            let generation = match &command {
                Request::Pim { generation, .. } => *generation,
                Request::SaveAttachments { generation, .. } => *generation,
                Request::Pending { generation } => *generation,
                Request::Find { generation, .. } => *generation,
                Request::Files { generation, .. } => *generation,
                Request::Load { generation, .. }
                | Request::Folder { generation, .. }
                | Request::Detail { generation, .. }
                | Request::Search { generation, .. }
                | Request::CreateFolder { generation, .. }
                | Request::Compose { generation, .. }
                | Request::Mutate { generation, .. } => *generation,
            };
            if current.load(Ordering::Relaxed) != generation
                && !matches!(
                    &command,
                    Request::Mutate { .. }
                        | Request::Pim { .. }
                        | Request::CreateFolder { .. }
                        | Request::Compose { .. }
                        | Request::Pending { .. }
                        | Request::SaveAttachments { .. }
                )
            {
                continue;
            }
            let result: anyhow::Result<Event> = (|| match command {
                Request::Pim {
                    calendars,
                    books,
                    tasks,
                    change,
                    clock,
                    ..
                } => {
                    let write = !matches!(*change, crate::pim::Change::Load);
                    let result = if let Some((now, mut snapshot)) = clock {
                        crate::pim::refresh_occurrences(&mut snapshot, now);
                        Ok(*snapshot)
                    } else {
                        crate::pim::execute_with_tasks(&calendars, &books, &tasks, *change).map(
                            |mut snapshot| {
                                let contacts = (|| -> anyhow::Result<_> {
                                    let index = if let Some(path) = &index_path {
                                        crate::search::SearchIndex::reader(path)?
                                    } else {
                                        let index = crate::search::SearchIndex::new()?;
                                        index.fill_store(store.as_ref())?;
                                        index
                                    };
                                    index.contacts()
                                })();
                                match contacts {
                                    Ok(contacts) => snapshot.contacts.extend(contacts),
                                    Err(error) => {
                                        snapshot.warnings.push(format!("Mail contacts: {error:#}"))
                                    }
                                }
                                snapshot
                            },
                        )
                    };
                    Ok(Event::Pim {
                        generation,
                        result: result.map_err(|e| format!("{e:#}")),
                        write,
                    })
                }
                Request::SaveAttachments {
                    directory,
                    attachments,
                    ..
                } => Ok(Event::SavedAttachments {
                    generation,
                    result: crate::files::save_attachments(&directory, &attachments)
                        .map_err(|error| format!("{error:#}")),
                }),
                Request::Pending { .. } => Ok(Event::Pending {
                    generation,
                    result: store.pending().map_err(|error| format!("{error:#}")),
                }),
                Request::Find { query, account, .. } => {
                    let messages = (|| -> anyhow::Result<Vec<Message>> {
                        let index = if let Some(path) = &index_path {
                            crate::search::SearchIndex::reader(path)?
                        } else {
                            let index = crate::search::SearchIndex::new()?;
                            index.fill_store(store.as_ref())?;
                            index
                        };
                        index.find(&query, account.as_deref())
                    })()
                    .map_err(|error| format!("{error:#}"));
                    Ok(Event::Found {
                        generation,
                        query,
                        messages,
                    })
                }
                Request::Files { path, .. } => {
                    let entries = crate::files::browse(&path).map_err(|error| format!("{error:#}"));
                    Ok(Event::Files {
                        generation,
                        path,
                        entries,
                    })
                }
                Request::CreateFolder {
                    account,
                    anchor,
                    parent,
                    name,
                    ..
                } => Ok(Event::Mutation {
                    generation,
                    result: store.create_folder(&account, &anchor, parent.as_deref(), &name)?,
                }),
                Request::Compose { draft, send, .. } => {
                    let result = store.compose(&draft, send)?;
                    Ok(if send {
                        Event::Submitted {
                            generation,
                            account: draft.account,
                            result,
                            pending: store.pending().unwrap_or_default(),
                        }
                    } else {
                        Event::Mutation { generation, result }
                    })
                }
                Request::Load { account, open, .. } => {
                    let accounts = store.accounts();
                    let folders = store.folders()?;
                    let open = open.map(|keys| keys.into_iter().collect::<HashSet<_>>());
                    let mut pages = vec![];
                    let mut budget = MAX_LOADED;
                    for folder in &folders {
                        if current.load(Ordering::Relaxed) != generation {
                            anyhow::bail!("Request cancelled");
                        }
                        if account.as_ref().is_some_and(|a| a != &folder.account) {
                            continue;
                        }
                        let key = tree::folder_key(folder);
                        let wanted = open.as_ref().is_some_and(|keys| keys.contains(&key));
                        if !wanted || budget == 0 {
                            continue;
                        }
                        let page = store
                            .messages(&folder_scope(&key), None, budget.min(PAGE_SIZE))
                            .map_err(|e| format!("{e:#}"));
                        if let Ok(page) = &page {
                            budget -= page.messages.len();
                        }
                        pages.push((key, page));
                    }
                    Ok(Event::Loaded {
                        generation,
                        accounts,
                        folders,
                        pages,
                        pending: store.pending()?,
                    })
                }
                Request::Folder {
                    key,
                    cursor,
                    append,
                    limit,
                    ..
                } => {
                    let page = store
                        .messages(&folder_scope(&key), cursor.as_ref(), limit)
                        .map_err(|e| format!("{e:#}"));
                    Ok(Event::Folder {
                        generation,
                        key,
                        append,
                        page,
                    })
                }
                Request::Detail { id, .. } => Ok(Event::Detail {
                    generation,
                    parsed: crate::mime::parse(&store.raw(&id)?)?,
                }),
                Request::Search { query, account, .. } => Ok(Event::Search {
                    generation,
                    accounts: store.accounts(),
                    folders: store.folders()?,
                    messages: if let Some(path) = &index_path {
                        crate::search::SearchIndex::reader(path)?
                            .structured(&query, account.as_deref())?
                    } else {
                        crate::search::run(
                            store.as_ref(),
                            &query,
                            account.as_deref(),
                            generation,
                            &current,
                            |count| {
                                let _ = events.try_send(Event::Progress { generation, count });
                            },
                        )?
                    },
                }),
                Request::Mutate { ids, mutation, .. } => Ok(Event::Mutation {
                    generation,
                    result: store.mutate(&ids, &mutation)?,
                }),
            })();
            let event = result.unwrap_or_else(|e| Event::Error {
                generation,
                error: format!("{e:#}"),
            });
            // Drain already-confirmed writes even after the TUI closes its
            // event receiver. Stale reads are cancelled by the generation gate.
            let _ = events.blocking_send(event);
        }
    });
    (tx, rx, worker)
}

fn folder_scope(key: &FolderKey) -> Scope {
    Scope {
        account: Some(key.0.clone()),
        folder: Some(key.1.clone()),
        inbox_only: false,
    }
}
fn is_inbox(folder: &Folder) -> bool {
    folder.role.as_deref() == Some("inbox") || folder.name.eq_ignore_ascii_case("INBOX")
}

pub struct App {
    pub llm: crate::llm::Config,
    pub ai: crate::llm::Conversation,
    compose_reply: Option<ReplyHeaders>,
    pub demo: bool,
    pub view: View,
    pub row: usize,
    pub table_offset: usize,
    pub scope: Scope,
    pub accounts: Vec<Account>,
    pub calendar_collections: Vec<crate::config::PimCollection>,
    pub contact_books: Vec<crate::config::PimCollection>,
    pub calendar_events: Vec<crate::pim::Event>,
    pub contacts: Vec<crate::pim::Contact>,
    pub task_collections: Vec<crate::config::PimCollection>,
    pub tasks: Vec<crate::pim::Task>,
    pub expanded_calendars: HashSet<String>,
    pub expanded_contact_categories: HashSet<String>,
    pub expanded_task_lists: HashSet<String>,
    pub show_past_events: bool,
    pub show_completed_tasks: bool,
    upcoming_events: HashSet<std::path::PathBuf>,
    event_occurrences: HashMap<std::path::PathBuf, crate::pim::EventOccurrence>,
    pim_write_focus: Option<(View, String, String)>,
    pub sync_requests: HashSet<crate::pim_sync::Target>,
    pub mail_sync_requests: HashSet<String>,
    sync_enabled: HashSet<crate::pim_sync::Target>,
    pim_write_sync: Option<crate::pim_sync::Target>,
    pim_refresh_requested: bool,
    pub pim_loading: bool,
    pub pim_pending: bool,
    pub pim_warnings: Vec<String>,
    pim_generation: u64,
    pim_ai_request: Option<(u64, u64)>,
    pub folders: Vec<Folder>,
    pub messages: Vec<Message>,
    pub pending: Vec<PendingMail>,
    pub send_requests: HashSet<String>,
    pub send_errors: HashMap<String, String>,
    pub filter: String,
    pub search: String,
    pub selected: HashSet<MessageId>,
    pub parsed: Option<ParsedMessage>,
    pub detail: Option<Message>,
    pub input: Option<(InputMode, Input)>,
    pub overlay: Option<Overlay>,
    pub toast: String,
    pub theme: String,
    pub busy: bool,
    pub quit: bool,
    pub want_html: bool,
    pub html_remote: bool,
    html_on_load: bool,
    pub form_pending: bool,
    pub refresh_requested: bool,
    pub index_dirty: bool,
    pub index_busy: bool,
    quit_armed: Option<std::time::Instant>,
    pub body_scroll: usize,
    pub help_scroll: usize,
    pub show_header: bool,
    pub show_breadcrumbs: bool,
    pub wide: bool,
    pub page_height: usize,
    pub expanded: HashSet<FolderKey>,
    pub collapsed_accounts: HashSet<String>,
    pub pages: HashMap<FolderKey, FolderPage>,
    pub history: VecDeque<String>,
    stack: Vec<Navigation>,
    generation: u64,
    revision: u64,
    initialized: bool,
    root_loading: bool,
    requested_focus: Option<RowKey>,
    current: Arc<AtomicU64>,
    tx: mpsc::Sender<Request>,
    history_pos: usize,
    g_pending: bool,
    last_mark: Option<MessageId>,
    sort: String,
    descending: bool,
    unread: bool,
    flagged: bool,
}

impl App {
    pub fn new(
        demo: bool,
        theme: String,
        current: Arc<AtomicU64>,
        tx: mpsc::Sender<Request>,
    ) -> Self {
        Self {
            llm: crate::llm::Config::default(),
            ai: crate::llm::Conversation::default(),
            compose_reply: None,
            demo,
            view: View::Messages,
            row: 0,
            table_offset: 0,
            scope: Scope::default(),
            accounts: vec![],
            calendar_collections: vec![],
            contact_books: vec![],
            calendar_events: vec![],
            contacts: vec![],
            task_collections: vec![],
            tasks: vec![],
            expanded_calendars: HashSet::new(),
            expanded_contact_categories: HashSet::new(),
            expanded_task_lists: HashSet::new(),
            show_past_events: false,
            show_completed_tasks: false,
            upcoming_events: HashSet::new(),
            event_occurrences: HashMap::new(),
            pim_write_focus: None,
            sync_requests: HashSet::new(),
            mail_sync_requests: HashSet::new(),
            sync_enabled: HashSet::new(),
            pim_write_sync: None,
            pim_refresh_requested: false,
            pim_loading: false,
            pim_pending: false,
            pim_warnings: vec![],
            pim_generation: 0,
            pim_ai_request: None,
            folders: vec![],
            messages: vec![],
            pending: vec![],
            send_requests: HashSet::new(),
            send_errors: HashMap::new(),
            filter: String::new(),
            search: String::new(),
            selected: HashSet::new(),
            parsed: None,
            detail: None,
            input: None,
            overlay: None,
            toast: "Loading local mail tree…".into(),
            theme,
            busy: false,
            quit: false,
            want_html: false,
            html_remote: false,
            html_on_load: false,
            form_pending: false,
            refresh_requested: false,
            index_dirty: true,
            index_busy: false,
            quit_armed: None,
            body_scroll: 0,
            help_scroll: 0,
            show_header: true,
            show_breadcrumbs: true,
            wide: false,
            page_height: 12,
            expanded: HashSet::new(),
            collapsed_accounts: HashSet::new(),
            pages: HashMap::new(),
            history: VecDeque::new(),
            stack: vec![],
            generation: 0,
            revision: 0,
            initialized: false,
            root_loading: false,
            requested_focus: None,
            current,
            tx,
            history_pos: 0,
            g_pending: false,
            last_mark: None,
            sort: "date".into(),
            descending: true,
            unread: false,
            flagged: false,
        }
    }
    pub fn configure_pim(
        &mut self,
        calendar: crate::config::CalendarConfig,
        contacts: crate::config::ContactsConfig,
    ) {
        if contacts.enabled && !contacts.sync_command.is_empty() {
            self.sync_enabled.insert(crate::pim_sync::Target::Contacts);
        } else {
            self.sync_enabled.remove(&crate::pim_sync::Target::Contacts);
        }
        self.calendar_collections = if calendar.enabled {
            calendar.collections
        } else {
            vec![]
        };
        self.contact_books = if contacts.enabled {
            contacts.books
        } else {
            vec![]
        };
        self.reload_pim();
    }
    pub fn reload_pim(&mut self) {
        if self.pim_loading || self.pim_pending || self.overlay.is_some() {
            return;
        }
        self.pim_request(crate::pim::Change::Load);
    }
    pub fn configure_tasks_and_sync(
        &mut self,
        calendars: &crate::config::CalendarsConfig,
        tasks: &crate::config::CalendarsConfig,
    ) {
        self.task_collections = if tasks.enabled {
            tasks.collections.clone()
        } else {
            vec![]
        };
        for (target, config) in [
            (crate::pim_sync::Target::Calendars, calendars),
            (crate::pim_sync::Target::Tasks, tasks),
        ] {
            if config.sync_command.is_empty() {
                self.sync_enabled.remove(&target);
            } else {
                self.sync_enabled.insert(target);
            }
        }
        self.pim_refresh_requested = true;
        self.refresh_pim_if_idle();
    }
    pub fn refresh_pim_if_idle(&mut self) {
        if self.pim_refresh_requested
            && !self.pim_loading
            && !self.pim_pending
            && self.overlay.is_none()
        {
            self.pim_refresh_requested = !self.pim_request(crate::pim::Change::Load);
        }
    }
    pub fn pim_files_changed(&mut self) {
        self.pim_refresh_requested = true;
        self.refresh_pim_if_idle();
    }
    pub fn refresh_pim_clock(&mut self, now: chrono::DateTime<chrono::Utc>) {
        if self.pim_loading
            || self.pim_pending
            || self.pim_refresh_requested
            || self.overlay.is_some()
            || !self
                .event_occurrences
                .values()
                .any(|event| event.upcoming && event.ends_at <= now)
        {
            return;
        }
        let snapshot = crate::pim::Snapshot {
            events: self.calendar_events.clone(),
            contacts: self.contacts.clone(),
            tasks: self.tasks.clone(),
            warnings: self.pim_warnings.clone(),
            ..Default::default()
        };
        self.pim_request_with_clock(crate::pim::Change::Load, Some((now, Box::new(snapshot))));
    }
    pub fn sync_completed(&mut self, target: crate::pim_sync::Target, error: Option<String>) {
        self.pim_refresh_requested = true;
        self.notify(match error {
            Some(error) => format!(
                "Sync {} failed: {error} · local changes are saved",
                target.name()
            ),
            None => format!("Sync {} completed", target.name()),
        });
        self.refresh_pim_if_idle();
    }
    fn request_sync(&mut self) {
        if matches!(self.view, View::Messages | View::Detail | View::Accounts) {
            if self.demo {
                self.notify("Mail synchronization is unavailable in demo mode");
                return;
            }
            let accounts: Vec<_> = self
                .accounts
                .iter()
                .filter(|account| {
                    self.scope
                        .account
                        .as_ref()
                        .is_none_or(|scope| scope == &account.id)
                })
                .map(|account| account.id.clone())
                .collect();
            if accounts.is_empty() {
                self.notify("No mail accounts available to synchronize");
            } else {
                let count = accounts.len();
                self.mail_sync_requests.extend(accounts);
                self.notify(format!("Synchronizing mail for {count} account(s)…"));
            }
            return;
        }
        let target = match self.view {
            View::Calendar => crate::pim_sync::Target::Calendars,
            View::Tasks => crate::pim_sync::Target::Tasks,
            View::Contacts => crate::pim_sync::Target::Contacts,
            _ => {
                self.notify(":sync is available in Mail, Calendar, Contacts and Tasks");
                return;
            }
        };
        if !self.sync_enabled.contains(&target) {
            self.notify(format!("Configure [{}].sync_command first", target.name()));
        } else if self.pim_pending || self.pim_loading {
            self.notify("Wait for the PIM operation to finish");
        } else {
            self.sync_requests.insert(target);
            self.notify(format!("Synchronizing {}…", target.name()));
        }
    }
    fn pim_request(&mut self, change: crate::pim::Change) -> bool {
        self.pim_request_with_clock(change, None)
    }
    fn pim_request_with_clock(
        &mut self,
        change: crate::pim::Change,
        clock: Option<(chrono::DateTime<chrono::Utc>, Box<crate::pim::Snapshot>)>,
    ) -> bool {
        if self.pim_loading || self.pim_pending {
            self.notify("PIM operation in progress; retry shortly");
            return false;
        }
        let write = !matches!(change, crate::pim::Change::Load);
        let write_focus = match &change {
            crate::pim::Change::Event { entry, .. } => {
                Some((View::Calendar, entry.collection.clone(), entry.uid.clone()))
            }
            crate::pim::Change::Task { entry, .. } => {
                Some((View::Tasks, entry.collection.clone(), entry.uid.clone()))
            }
            crate::pim::Change::Contact { entry, .. } => {
                Some((View::Contacts, entry.book.clone(), entry.uid.clone()))
            }
            _ => None,
        };
        let target = match &change {
            crate::pim::Change::Event { .. } => Some(crate::pim_sync::Target::Calendars),
            crate::pim::Change::Task { .. } => Some(crate::pim_sync::Target::Tasks),
            crate::pim::Change::Contact { .. } => Some(crate::pim_sync::Target::Contacts),
            crate::pim::Change::Delete { source, .. }
                if source
                    .lines()
                    .any(|l| l.eq_ignore_ascii_case("BEGIN:VCARD")) =>
            {
                Some(crate::pim_sync::Target::Contacts)
            }
            crate::pim::Change::Delete { source, .. }
                if source
                    .lines()
                    .any(|l| l.eq_ignore_ascii_case("BEGIN:VTODO")) =>
            {
                Some(crate::pim_sync::Target::Tasks)
            }
            crate::pim::Change::Delete { source, .. }
                if source
                    .lines()
                    .any(|l| l.eq_ignore_ascii_case("BEGIN:VEVENT")) =>
            {
                Some(crate::pim_sync::Target::Calendars)
            }
            _ => None,
        };
        let generation = self.pim_generation + 1;
        let clock_only = clock.is_some();
        if self
            .tx
            .try_send(Request::Pim {
                generation,
                calendars: self.calendar_collections.clone(),
                books: self.contact_books.clone(),
                tasks: self.task_collections.clone(),
                change: Box::new(change),
                clock,
            })
            .is_err()
        {
            if !clock_only {
                self.notify("Background queue busy; retry");
            }
            return false;
        }
        self.pim_generation = generation;
        self.pim_write_sync = target;
        self.pim_write_focus = write_focus;
        self.pim_loading = !write;
        self.pim_pending = write;
        true
    }
    pub fn open_ai(&mut self, new: bool, global: bool) {
        if !self.llm.enabled {
            self.notify("Assistant disabled · configure [llm] in config.toml");
            return;
        }
        if new || !self.ai.initialized {
            let run = self.ai.run + 1;
            let anchor = if global {
                None
            } else if self.view == View::Detail {
                self.detail.clone()
            } else if self.view == View::Messages {
                self.focused().cloned()
            } else {
                None
            };
            let account = anchor
                .as_ref()
                .map(|m| m.id.account.clone())
                .or_else(|| self.scope.account.clone());
            self.ai = crate::llm::Conversation {
                initialized: true,
                account,
                anchor,
                run,
                status: "Ready".into(),
                ..Default::default()
            };
            self.ai.commands.push_back(crate::llm::Command::Cancel);
            if let Some(message) = self.ai.anchor.clone() {
                self.ai
                    .sources
                    .push(crate::llm::Source { number: 1, message });
            }
        }
        self.overlay = Some(Overlay::Assistant);
    }
    fn ai_command(&mut self, command: &str) {
        use crate::llm::{Command as C, Provider};
        if ![
            "", "new", "global", "login", "logout", "status", "stop", "models",
        ]
        .contains(&command)
        {
            self.notify("Usage: :ai [new|global|login|logout|status|stop|models]");
            return;
        }
        self.open_ai(command == "new" || command == "global", command == "global");
        if !self.llm.enabled {
            return;
        }
        match command {
            "stop" => self.ai_stop(),
            "login" | "logout" | "status" | "models" => {
                if self.llm.provider != Provider::Codex {
                    self.ai.status = if command == "models" {
                        "Model catalogue is available in Codex mode; API mode uses llm.model".into()
                    } else {
                        format!(
                            "{} · API key from {}",
                            self.llm.label(),
                            self.llm.auth.api_key_env
                        )
                    };
                    return;
                }
                self.ai.run += 1;
                self.ai.busy = true;
                self.ai.status = "Connecting to Codex…".into();
                self.ai.commands.push_back(match command {
                    "login" => C::Login(self.ai.run),
                    "logout" => C::Logout(self.ai.run),
                    "models" => C::Models(self.ai.run),
                    _ => C::Status(self.ai.run),
                });
            }
            _ => {}
        }
    }
    fn ai_stop(&mut self) {
        self.ai.run += 1;
        self.ai.busy = false;
        self.ai.status = "Stopped".into();
        self.ai.commands.push_back(crate::llm::Command::Cancel);
    }
    pub fn ai_event(&mut self, event: crate::llm::Event) {
        use crate::llm::Update;
        if event.run != self.ai.run {
            return;
        }
        match event.update {
            Update::Models(models) => {
                let list = models
                    .iter()
                    .map(|m| format!("• {m}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.ai.history.push(crate::llm::MessageText { role:"assistant".into(),content:format!("Codex model catalogue\n{list}\n\nConfigured model: {}\nUse an exact catalogue identifier in [llm].model, then restart MailTUI.\nThe catalogue is informational; the backend determines actual access.",self.llm.model) });
                self.ai.scroll = 0;
            }
            Update::Text(text) => {
                if self.ai.history.last().is_none_or(|m| m.role != "assistant") {
                    self.ai.history.push(crate::llm::MessageText {
                        role: "assistant".into(),
                        content: String::new(),
                    });
                }
                if let Some(m) = self.ai.history.last_mut()
                    && m.content.len() + text.len() <= 100_000
                {
                    m.content.push_str(&crate::mime::terminal_text(&text));
                }
            }
            Update::Status(status) => self.ai.status = crate::mime::terminal_text(&status),
            Update::Source(source) => {
                if !self
                    .ai
                    .sources
                    .iter()
                    .any(|s| s.message.id == source.message.id)
                {
                    self.ai.sources.push(source);
                }
            }
            Update::Proposal(proposal) => {
                let detail = match &proposal {
                    crate::llm::Proposal::Reply { draft } => {
                        format!("{}\n{}", proposal.summary(), draft.body)
                    }
                    _ => proposal.summary(),
                };
                self.ai.history.push(crate::llm::MessageText {
                    role: "assistant".into(),
                    content: crate::mime::terminal_text(&format!(
                        "Proposal {}: {detail}\n\n",
                        self.ai.proposals.len() + 1
                    )),
                });
                self.ai.proposals.push(Some(proposal));
            }
            Update::LoginUrl(url) => {
                self.ai.history.push(crate::llm::MessageText {
                    role: "assistant".into(),
                    content: format!(
                        "Complete Codex sign-in in your browser:\n{}",
                        crate::mime::terminal_text(&url)
                    ),
                });
                self.ai.status = "Waiting for Codex OAuth… · /stop cancels".into();
            }
            Update::Finished => {
                self.ai.busy = false;
                if self.ai.status.starts_with("Generating")
                    || self.ai.status.starts_with("Mail tool:")
                    || self.ai.status.starts_with("Local tool:")
                {
                    self.ai.status = "Done".into();
                }
            }
            Update::Error(error) => {
                self.ai.busy = false;
                self.ai.status = "Error · details in conversation".into();
                let content = crate::mime::terminal_text(&format!("Error: {error}"));
                if let Some(last) = self.ai.history.last_mut()
                    && last.role == "assistant"
                    && last.content.is_empty()
                {
                    last.content = content;
                } else {
                    self.ai.history.push(crate::llm::MessageText {
                        role: "assistant".into(),
                        content,
                    });
                }
                self.ai.scroll = 0;
            }
        }
    }
    fn ai_key(&mut self, key: KeyEvent) {
        use crate::llm::{Command as C, MessageText, Proposal, Turn};
        match key.code {
            KeyCode::Esc => {
                self.overlay = None;
                return;
            }
            KeyCode::PageUp => {
                self.ai.scroll = self.ai.scroll.saturating_add(self.page_height.max(1));
                return;
            }
            KeyCode::PageDown => {
                self.ai.scroll = self.ai.scroll.saturating_sub(self.page_height.max(1));
                return;
            }
            KeyCode::Enter => {}
            _ => {
                self.ai.input.key(key);
                return;
            }
        }
        let text = self.ai.input.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(command) = text.strip_prefix('/') {
            let (name, arg) = command.split_once(' ').unwrap_or((command, ""));
            match name {
                "sources" => {
                    let content = self
                        .ai
                        .sources
                        .iter()
                        .map(|s| {
                            format!(
                                "[{}] {} · {} · {} · {}",
                                s.number,
                                s.message.subject,
                                s.message.sender,
                                s.message.date,
                                s.message.id.account
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.ai.history.push(MessageText {
                        role: "assistant".into(),
                        content,
                    });
                }
                "new" | "global" | "login" | "logout" | "status" | "stop" | "models" => self.ai_command(name),
                "open" => {
                    if let Some(source) = arg
                        .parse::<usize>()
                        .ok()
                        .and_then(|n| self.ai.sources.iter().find(|s| s.number == n))
                        .cloned()
                    {
                        self.overlay = None;
                        self.navigate(View::Detail);
                        self.detail = Some(source.message.clone());
                        self.parsed = None;
                        self.send(Request::Detail {
                            generation: self.generation,
                            id: source.message.id,
                        });
                    } else {
                        self.ai.status = "Unknown source · /open N".into();
                    }
                }
                "apply" if !self.ai.busy => {
                    if let Some(index) = arg.parse::<usize>().ok().and_then(|n| n.checked_sub(1)) {
                        let proposal = self.ai.proposals.get(index).and_then(|p| p.clone());
                        match proposal {
                            Some(Proposal::Pim {change,..})=>{
                                let allowed=match change.as_ref() {
                                    crate::pim::Change::Event{collection,..}=>self.calendar_collections.iter().any(|c|c.id==collection.id&&c.path==collection.path),
                                    crate::pim::Change::Contact{book,..}=>self.contact_books.iter().any(|b|b.id==book.id&&b.path==book.path),
                                    crate::pim::Change::Delete{file,..}=>self.calendar_collections.iter().chain(&self.contact_books).any(|c|file.parent()==Some(c.path.as_path())),
                                    crate::pim::Change::Load | crate::pim::Change::Task { .. }=>false,
                                };
                                if !allowed {self.ai.status="Calendar/address book no longer enabled or configured".into();}
                                else if self.pim_request(*change) {
                                    self.pim_ai_request=Some((self.pim_generation,self.ai.run));
                                    self.ai.proposals[index]=None;
                                    self.ai.status="Local PIM operation requested · awaiting worker receipt".into();
                                } else {self.ai.status="PIM queue busy · retry /apply N".into();}
                            }
                            Some(Proposal::Reply { draft }) => {
                                if let Some(account) =
                                    self.accounts.iter().position(|a| a.id == draft.account)
                                {
                                    let mut fields =
                                        [Input::default(), Input::default(), Input::default()];
                                    for (input, text) in
                                        fields.iter_mut().zip([draft.to, draft.subject, draft.body])
                                    {
                                        input.set(text);
                                    }
                                    self.compose_reply = draft.reply;
                                    self.overlay = Some(Overlay::Compose {
                                        account,
                                        field: 3,
                                        fields,
                                        attachments: vec![],
                                    });
                                    self.ai.proposals[index] = None;
                                    self.ai.status = "Reply opened in composer · not sent".into();
                                } else {
                                    self.ai.status = "Account no longer available".into();
                                }
                            }
                            Some(Proposal::Move { ids, folder, .. }) => {
                                if self.folders.iter().any(|f| {
                                    f.id == folder && ids.iter().all(|id| id.account == f.account)
                                }) {
                                    let generation = self.generation();
                                    if self.send(Request::Mutate {
                                        generation,
                                        ids,
                                        mutation: Mutation::Move(folder),
                                    }) {
                                        self.ai.proposals[index] = None;
                                        self.ai.status =
                                            "Move requested · see operation status below".into();
                                    }
                                } else {
                                    self.ai.status = "Destination no longer available".into();
                                }
                            }
                            None => {
                                self.ai.status =
                                    "Unknown or already applied proposal · /apply N".into()
                            }
                        }
                    } else {
                        self.ai.status = "Usage: /apply N".into();
                    }
                }
                _ => self.ai.status =
                    "Commands: /new /global /stop /login /logout /status /models /sources /open N /apply N"
                        .into(),
            }
            self.ai.input = Input::default();
            return;
        }
        if self.ai.busy {
            self.ai.status = "Assistant is busy · /stop to interrupt".into();
            return;
        }
        if self
            .ai
            .history
            .iter()
            .map(|m| m.content.len())
            .sum::<usize>()
            + text.len()
            > 120_000
        {
            self.ai.status = "Conversation full · /new starts a new one".into();
            return;
        }
        self.ai.history.push(MessageText {
            role: "user".into(),
            content: text,
        });
        self.ai.run += 1;
        self.ai.busy = true;
        self.ai.scroll = 0;
        self.ai.status = "Connecting…".into();
        self.ai.commands.push_back(C::Turn(Box::new(Turn {
            run: self.ai.run,
            account: self.ai.account.clone(),
            anchor: self.ai.anchor.clone(),
            history: self.ai.history.clone(),
            sources: self.ai.sources.clone(),
        })));
        self.ai.history.push(MessageText {
            role: "assistant".into(),
            content: String::new(),
        });
        self.ai.input = Input::default();
    }
    fn generation(&mut self) -> u64 {
        self.html_on_load = false;
        self.generation += 1;
        self.current.store(self.generation, Ordering::Relaxed);
        for page in self.pages.values_mut() {
            page.loading = false;
            page.reserved = 0;
        }
        self.root_loading = false;
        self.generation
    }
    fn send(&mut self, request: Request) -> bool {
        if self.tx.try_send(request).is_ok() {
            self.busy = true;
            true
        } else {
            self.notify("Background queue is busy; retry your action");
            false
        }
    }
    pub fn notify(&mut self, text: impl Into<String>) {
        self.toast = crate::mime::terminal_text(&text.into());
    }
    pub fn account_name(&self) -> &str {
        self.scope.account.as_deref().unwrap_or("all")
    }
    pub fn searching(&self) -> bool {
        !self.search.is_empty()
    }
    pub fn sort_name(&self) -> &str {
        &self.sort
    }
    pub fn sort_descending(&self) -> bool {
        self.descending
    }

    pub fn visible(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        let mut rows: Vec<_> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                self.scope
                    .account
                    .as_ref()
                    .is_none_or(|a| a == &m.id.account)
                    && (!self.unread || !m.seen)
                    && (!self.flagged || m.flagged)
                    && m.visible_text().to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect();
        rows.sort_by(|a, b| {
            let (a, b) = (&self.messages[*a], &self.messages[*b]);
            let order = match self.sort.as_str() {
                "from" => a.sender.cmp(&b.sender),
                "subject" => a.subject.cmp(&b.subject),
                "size" => a.size.cmp(&b.size),
                "status" => (a.seen, !a.flagged).cmp(&(b.seen, !b.flagged)),
                _ => a.date.cmp(&b.date),
            }
            .then_with(|| a.id.cmp(&b.id));
            if self.descending {
                order.reverse()
            } else {
                order
            }
        });
        rows
    }
    pub fn tree_rows(&self) -> Vec<TreeRow> {
        TreeData {
            accounts: &self.accounts,
            folders: &self.folders,
            messages: &self.messages,
            visible_messages: &self.visible(),
            account: self.scope.account.as_deref(),
            expanded: &self.expanded,
            collapsed_accounts: &self.collapsed_accounts,
            pages: &self.pages,
            filter: &self.filter,
            searching: self.searching(),
            account_sort: (self.sort == "account").then_some(self.descending),
        }
        .rows()
    }
    pub fn row_key(&self, row: &TreeRow) -> RowKey {
        match row.node {
            Node::Account(i) => RowKey::Account(self.accounts[i].id.clone()),
            Node::Folder(i) => RowKey::Folder(tree::folder_key(&self.folders[i])),
            Node::Message(i) => RowKey::Message(self.messages[i].id.clone()),
            Node::More(i) => RowKey::More(tree::folder_key(&self.folders[i])),
            Node::Status(i) => RowKey::Status(tree::folder_key(&self.folders[i])),
        }
    }
    pub fn focused_key(&self) -> Option<RowKey> {
        (self.view == View::Messages)
            .then(|| self.tree_rows().get(self.row).map(|row| self.row_key(row)))
            .flatten()
    }
    pub fn focus_key(&mut self, key: &RowKey) -> bool {
        if let Some(row) = self
            .tree_rows()
            .iter()
            .position(|row| self.row_key(row) == *key)
        {
            self.row = row;
            true
        } else {
            false
        }
    }
    fn restore_focus(&mut self, key: Option<RowKey>) {
        if let Some(key) = key {
            if self.focus_key(&key) {
                return;
            }
            let parent = match key {
                RowKey::Message(id) => Some(RowKey::Folder((id.account, id.folder))),
                RowKey::More(key) | RowKey::Status(key) => Some(RowKey::Folder(key)),
                _ => None,
            };
            if parent.as_ref().is_some_and(|key| self.focus_key(key)) {
                return;
            }
        }
        self.row = self.row.min(self.count().saturating_sub(1));
    }
    pub fn focused(&self) -> Option<&Message> {
        if self.view == View::Detail {
            return self.detail.as_ref();
        }
        if self.view != View::Messages {
            return None;
        }
        match self.tree_rows().get(self.row)?.node {
            Node::Message(i) => self.messages.get(i),
            _ => None,
        }
    }
    pub fn focused_folder(&self) -> Option<FolderKey> {
        if self.view == View::Detail {
            return self
                .detail
                .as_ref()
                .map(|m| (m.id.account.clone(), m.id.folder.clone()));
        }
        match self.focused_key()? {
            RowKey::Folder(k) | RowKey::More(k) | RowKey::Status(k) => Some(k),
            RowKey::Message(id) => Some((id.account, id.folder)),
            _ => None,
        }
    }
    pub fn directory_name(&self) -> String {
        self.focused_folder()
            .map(|(_, f)| f)
            .unwrap_or_else(|| "all folders".into())
    }
    fn context_account(&self) -> Option<String> {
        self.focused_folder()
            .map(|key| key.0)
            .or_else(|| match self.focused_key() {
                Some(RowKey::Account(id)) => Some(id),
                _ => None,
            })
            .or_else(|| {
                if self.view == View::Accounts {
                    self.account_rows()
                        .get(self.row)
                        .and_then(|index| index.map(|i| self.accounts[i].id.clone()))
                } else {
                    None
                }
            })
            .or_else(|| self.scope.account.clone())
    }
    pub fn visible_message_indices(&self) -> Vec<usize> {
        self.tree_rows()
            .iter()
            .filter_map(|r| {
                if let Node::Message(i) = r.node {
                    Some(i)
                } else {
                    None
                }
            })
            .collect()
    }
    pub fn targets(&self) -> Vec<MessageId> {
        if self.view == View::Detail || self.selected.is_empty() {
            self.focused()
                .map(|m| vec![m.id.clone()])
                .unwrap_or_default()
        } else {
            let mut ids: Vec<_> = self.selected.iter().cloned().collect();
            ids.sort();
            ids
        }
    }
    pub fn account_rows(&self) -> Vec<Option<usize>> {
        let needle = self.filter.to_lowercase();
        let mut out = vec![];
        if "all accounts".contains(&needle) {
            out.push(None);
        }
        out.extend(
            self.accounts
                .iter()
                .enumerate()
                .filter(|(_, a)| {
                    format!("{} {} {}", a.id, a.label, a.email)
                        .to_lowercase()
                        .contains(&needle)
                })
                .map(|(i, _)| Some(i)),
        );
        out
    }
    pub fn pim_rows(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        if self.view == View::Calendar {
            let mut indices: Vec<_> = self
                .calendar_events
                .iter()
                .enumerate()
                .filter(|(_, e)| self.show_past_events || self.upcoming_events.contains(&e.file))
                .filter(|(_, e)| {
                    format!(
                        "{} {} {} {} {} {}",
                        e.collection,
                        e.collection_label,
                        e.summary,
                        self.calendar_event_dates(e).0,
                        e.location,
                        e.description
                    )
                    .to_lowercase()
                    .contains(&needle)
                })
                .map(|(i, _)| i)
                .collect();
            indices.sort_by(|a, b| {
                let a = &self.calendar_events[*a];
                let b = &self.calendar_events[*b];
                let a_occurrence = self.event_occurrences.get(&a.file);
                let b_occurrence = self.event_occurrences.get(&b.file);
                a_occurrence
                    .is_none()
                    .cmp(&b_occurrence.is_none())
                    .then_with(|| {
                        a_occurrence
                            .map(|o| o.starts_at)
                            .cmp(&b_occurrence.map(|o| o.starts_at))
                    })
                    .then_with(|| {
                        self.calendar_event_dates(a)
                            .0
                            .cmp(self.calendar_event_dates(b).0)
                    })
                    .then(a.summary.cmp(&b.summary))
                    .then(a.file.cmp(&b.file))
            });
            indices
        } else if self.view == View::Tasks {
            self.tasks
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    self.show_completed_tasks || !crate::pim::task_is_closed(&t.status)
                })
                .filter(|(_, t)| {
                    format!(
                        "{} {} {} {} {} {} {}",
                        t.collection,
                        t.collection_label,
                        t.summary,
                        t.due,
                        t.status,
                        crate::pim::task_status_label(&t.status),
                        t.description
                    )
                    .to_lowercase()
                    .contains(&needle)
                })
                .map(|(i, _)| i)
                .collect()
        } else {
            self.contacts
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    format!(
                        "{} {} {} {} {} {} {} {}",
                        c.book,
                        c.book_label,
                        c.name,
                        c.email,
                        c.phone,
                        c.organization,
                        if c.is_mail() { "Mail" } else { "CardDAV" },
                        c.addresses
                            .iter()
                            .map(|address| address.display())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                    .to_lowercase()
                    .contains(&needle)
                })
                .map(|(i, _)| i)
                .collect()
        }
    }
    pub fn calendar_event_dates<'a>(&'a self, event: &'a crate::pim::Event) -> (&'a str, &'a str) {
        self.event_occurrences
            .get(&event.file)
            .map(|occurrence| (occurrence.start.as_str(), occurrence.end.as_str()))
            .unwrap_or((&event.start, &event.end))
    }
    pub fn calendar_event_is_ongoing(
        &self,
        event: &crate::pim::Event,
        now: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        self.event_occurrences
            .get(&event.file)
            .is_some_and(|occurrence| occurrence.is_ongoing(now))
    }
    pub fn pim_collections(&self) -> &[crate::config::PimCollection] {
        if self.view == View::Contacts {
            crate::pim::contact_categories()
        } else if self.view == View::Tasks {
            &self.task_collections
        } else {
            &self.calendar_collections
        }
    }
    pub fn pim_tree_rows(&self) -> Vec<PimRow> {
        let entries = self.pim_rows();
        if !self.view.is_pim() {
            return vec![];
        }
        let mut grouped: HashMap<&str, Vec<usize>> = HashMap::new();
        for index in entries {
            let collection = if self.view == View::Contacts {
                self.contacts[index].category()
            } else if self.view == View::Tasks {
                &self.tasks[index].collection
            } else {
                &self.calendar_events[index].collection
            };
            grouped.entry(collection).or_default().push(index);
        }
        let expanded = if self.view == View::Contacts {
            &self.expanded_contact_categories
        } else if self.view == View::Tasks {
            &self.expanded_task_lists
        } else {
            &self.expanded_calendars
        };
        let needle = self.filter.to_lowercase();
        let mut rows = vec![];
        for (index, collection) in self.pim_collections().iter().enumerate() {
            let entries = grouped.remove(collection.id.as_str()).unwrap_or_default();
            if self.view == View::Calendar && entries.is_empty() {
                continue;
            }
            if !needle.is_empty()
                && entries.is_empty()
                && !format!("{} {}", collection.id, collection.label)
                    .to_lowercase()
                    .contains(&needle)
            {
                continue;
            }
            let open = expanded.contains(&collection.id) || !needle.is_empty();
            rows.push(PimRow::Collection {
                index,
                count: entries.len(),
                expanded: open,
            });
            if open {
                rows.extend(entries.into_iter().map(PimRow::Entry));
            }
        }
        rows
    }
    pub fn pim_focused_index(&self) -> Option<usize> {
        match self.pim_tree_rows().get(self.row)? {
            PimRow::Entry(index) => Some(*index),
            _ => None,
        }
    }
    pub fn pim_focused_key(&self) -> Option<PimRowKey> {
        match self.pim_tree_rows().get(self.row)? {
            PimRow::Collection { index, .. } => Some(PimRowKey::Collection(
                self.pim_collections()[*index].id.clone(),
            )),
            PimRow::Entry(index) => {
                let (collection, file) = match self.view {
                    View::Calendar => (
                        &self.calendar_events[*index].collection,
                        &self.calendar_events[*index].file,
                    ),
                    View::Tasks => (&self.tasks[*index].collection, &self.tasks[*index].file),
                    View::Contacts => {
                        let contact = &self.contacts[*index];
                        return Some(PimRowKey::Entry {
                            collection: contact.category().into(),
                            file: if contact.is_mail() {
                                contact.uid.clone().into()
                            } else {
                                contact.file.clone()
                            },
                        });
                    }
                    _ => return None,
                };
                Some(PimRowKey::Entry {
                    collection: collection.clone(),
                    file: file.clone(),
                })
            }
        }
    }
    pub fn focus_pim_key(&mut self, key: &PimRowKey) -> bool {
        let position = self
            .pim_tree_rows()
            .iter()
            .position(|row| match (row, key) {
                (PimRow::Collection { index, .. }, PimRowKey::Collection(id)) => {
                    &self.pim_collections()[*index].id == id
                }
                (PimRow::Entry(index), PimRowKey::Entry { file, .. }) => match self.view {
                    View::Calendar => &self.calendar_events[*index].file == file,
                    View::Tasks => &self.tasks[*index].file == file,
                    View::Contacts => {
                        let contact = &self.contacts[*index];
                        if contact.is_mail() {
                            std::path::Path::new(&contact.uid) == file
                        } else {
                            &contact.file == file
                        }
                    }
                    _ => false,
                },
                _ => false,
            });
        if let Some(position) = position {
            self.row = position;
            true
        } else {
            false
        }
    }
    fn restore_pim_focus(&mut self, key: Option<PimRowKey>) {
        if let Some(key) = key {
            if self.focus_pim_key(&key) {
                return;
            }
            if let PimRowKey::Entry { collection, .. } = key
                && self.focus_pim_key(&PimRowKey::Collection(collection))
            {
                return;
            }
        }
        self.row = self.row.min(self.count().saturating_sub(1));
    }
    fn toggle_pim_branch(&mut self) -> bool {
        let Some(PimRow::Collection { index, .. }) = self.pim_tree_rows().get(self.row).copied()
        else {
            return false;
        };
        let id = self.pim_collections()[index].id.clone();
        let expanded = if self.view == View::Contacts {
            &mut self.expanded_contact_categories
        } else if self.view == View::Tasks {
            &mut self.expanded_task_lists
        } else {
            &mut self.expanded_calendars
        };
        if !expanded.remove(&id) {
            expanded.insert(id.clone());
        }
        self.focus_pim_key(&PimRowKey::Collection(id));
        true
    }
    fn count(&self) -> usize {
        match self.view {
            View::Messages => self.tree_rows().len(),
            View::Calendar | View::Contacts | View::Tasks => self.pim_tree_rows().len(),
            View::Accounts => self.account_rows().len(),
            _ => 0,
        }
    }

    pub fn reload(&mut self, append: bool) {
        if append {
            self.load_more();
            return;
        }
        self.refresh_requested = false;
        if self.searching() && self.view == View::Messages {
            self.start_search(self.search.clone());
            return;
        }
        let focus = self.requested_focus.take().or_else(|| self.focused_key());
        let open = self.initialized.then(|| {
            self.folders
                .iter()
                .filter(|f| {
                    self.expanded.contains(&tree::folder_key(f))
                        && self.scope.account.as_ref().is_none_or(|a| a == &f.account)
                        && (self.scope.account.is_some()
                            || !self.collapsed_accounts.contains(&f.account))
                })
                .map(tree::folder_key)
                .collect()
        });
        let generation = self.generation();
        self.requested_focus = focus;
        self.root_loading = self.send(Request::Load {
            generation,
            account: self.scope.account.clone(),
            open,
        });
    }
    pub fn storage_changed(&mut self) {
        self.index_dirty = true;
        self.revision += 1;
        self.refresh_requested = true;
    }
    pub fn poll_pending(&self) {
        let _ = self.tx.try_send(Request::Pending {
            generation: self.generation,
        });
    }
    pub fn delivery_completed(&mut self, account: String, error: Option<String>) {
        if let Some(error) = error {
            self.send_errors.insert(account, error);
        } else {
            self.send_errors.remove(&account);
        }
        self.poll_pending();
        self.storage_changed();
    }
    pub fn mail_sync_completed(&mut self, account: String, error: Option<String>) {
        self.poll_pending();
        self.storage_changed();
        self.notify(match error {
            Some(error) => format!("Sync mail {account} failed: {error}"),
            None => format!("Sync mail {account} completed"),
        });
    }
    pub fn pending_label(&self) -> String {
        let failed = self
            .pending
            .iter()
            .filter(|mail| mail.error.is_some() || self.send_errors.contains_key(&mail.account))
            .count();
        let sending = self.pending.len() - failed;
        match (sending, failed) {
            (0, 0) => String::new(),
            (_, 0) => format!("Pending: {sending} sending"),
            (0, _) => format!("Pending: {failed} failed"),
            _ => format!("Pending: {sending} sending, {failed} failed"),
        }
    }
    pub fn index_changed(&mut self) {
        if self.view == View::Contacts {
            self.pim_files_changed();
        }
        if let Some(Overlay::Find { loading, again, .. }) = &mut self.overlay {
            if *loading {
                *again = true;
            } else {
                self.find_request();
            }
        } else if self.view == View::Messages && self.searching() {
            self.refresh_requested = true;
            self.refresh_if_idle();
        }
    }
    fn find_request(&mut self) {
        let Some(Overlay::Find {
            query,
            account,
            loading,
            error,
            ..
        }) = &mut self.overlay
        else {
            return;
        };
        if *loading {
            return;
        }
        *loading = true;
        *error = None;
        let request = Request::Find {
            generation: self.generation,
            query: query.text.clone(),
            account: account.clone(),
        };
        if !self.send(request)
            && let Some(Overlay::Find { loading, error, .. }) = &mut self.overlay
        {
            *loading = false;
            *error = Some("Background queue busy; retry".into());
        }
    }
    pub fn refresh_if_idle(&mut self) {
        if self.refresh_requested
            && matches!(self.view, View::Messages | View::Accounts)
            && !self.busy
            && self.overlay.is_none()
            && self.input.is_none()
        {
            self.reload(false);
        }
    }
    fn load_folder(&mut self, key: FolderKey) {
        if self.searching() || self.root_loading {
            return;
        }
        let held = self.pages.get(&key).cloned().unwrap_or_default();
        if held.loading || held.loaded && held.next.is_none() && held.error.is_none() {
            return;
        }
        let reserved: usize = self.pages.values().map(|p| p.reserved).sum();
        let limit = MAX_LOADED
            .saturating_sub(self.messages.len() + reserved)
            .min(PAGE_SIZE);
        if limit == 0 {
            self.notify("2,000 loaded messages reached; choose a narrower account scope or search");
            return;
        }
        let append = held.loaded;
        let request = Request::Folder {
            generation: self.generation,
            key: key.clone(),
            cursor: held.next.clone(),
            append,
            limit,
        };
        if self.send(request) {
            let page = self.pages.entry(key).or_default();
            page.loading = true;
            page.reserved = limit;
            page.error = None;
        }
    }
    fn load_more(&mut self) {
        if self.view != View::Messages {
            self.notify("Return to Messages to load folder pages");
            return;
        }
        if let Some(key) = self.focused_folder() {
            self.load_folder(key);
        } else {
            self.notify("Focus a folder or its ‘Load more’ row first");
        }
    }
    fn accept_page(&mut self, key: FolderKey, page: Result<Page, String>, append: bool) {
        match page {
            Ok(page) => {
                if !append {
                    self.messages.retain(|m| {
                        (m.id.account.as_str(), m.id.folder.as_str())
                            != (key.0.as_str(), key.1.as_str())
                    });
                }
                let mut ids: HashSet<_> = self.messages.iter().map(|m| m.id.clone()).collect();
                self.messages.extend(
                    page.messages
                        .into_iter()
                        .filter(|m| ids.insert(m.id.clone())),
                );
                self.pages.insert(
                    key,
                    FolderPage {
                        loaded: true,
                        next: page.next,
                        ..Default::default()
                    },
                );
            }
            Err(error) => {
                let page = self.pages.entry(key).or_default();
                page.loading = false;
                page.reserved = 0;
                page.error = Some(crate::mime::terminal_text(&error));
            }
        }
    }
    fn load_open_rows(&mut self) {
        let keys: Vec<_> = self
            .tree_rows()
            .iter()
            .filter_map(|r| {
                if let Node::Folder(i) = r.node {
                    let key = tree::folder_key(&self.folders[i]);
                    (self.expanded.contains(&key)
                        && self
                            .pages
                            .get(&key)
                            .is_none_or(|p| !p.loaded && !p.loading && p.error.is_none()))
                    .then_some(key)
                } else {
                    None
                }
            })
            .collect();
        for key in keys {
            self.load_folder(key);
        }
    }
    fn expand_ancestors(&mut self, key: &FolderKey) {
        let mut next = Some(key.1.clone());
        let mut visited = HashSet::new();
        while let Some(id) = next {
            if !visited.insert(id.clone()) {
                break;
            }
            self.expanded.insert((key.0.clone(), id.clone()));
            next = self
                .folders
                .iter()
                .find(|f| f.account == key.0 && f.id == id)
                .and_then(|f| f.parent.clone());
        }
        self.collapsed_accounts.remove(&key.0);
    }
    fn branch(&mut self, expand: bool, toggle: bool) {
        let rows = self.tree_rows();
        let Some(row) = rows.get(self.row) else {
            return;
        };
        let key = self.row_key(row);
        match row.node {
            Node::Account(i) => {
                let account = self.accounts[i].id.clone();
                let open = !self.collapsed_accounts.contains(&account);
                if toggle && open || !expand && open {
                    self.collapsed_accounts.insert(account);
                } else if expand || toggle {
                    self.collapsed_accounts.remove(&account);
                    self.load_open_rows();
                    if open
                        && !toggle
                        && rows.get(self.row + 1).is_some_and(|r| r.depth > row.depth)
                    {
                        self.row += 1;
                        return;
                    }
                } else {
                    self.parent_row();
                    return;
                }
            }
            Node::Folder(i) => {
                let folder = tree::folder_key(&self.folders[i]);
                let open = self.expanded.contains(&folder);
                if toggle && open || !expand && open {
                    self.expanded.remove(&folder);
                } else if expand || toggle {
                    self.expanded.insert(folder.clone());
                    let needs_load = self
                        .pages
                        .get(&folder)
                        .is_none_or(|p| !p.loaded || p.error.is_some());
                    if needs_load {
                        self.load_folder(folder);
                    } else if open
                        && !toggle
                        && rows.get(self.row + 1).is_some_and(|r| r.depth > row.depth)
                    {
                        self.row += 1;
                        return;
                    }
                } else {
                    self.parent_row();
                    return;
                }
            }
            Node::More(i) | Node::Status(i) if expand || toggle => {
                self.load_folder(tree::folder_key(&self.folders[i]));
            }
            _ => {
                if !expand {
                    self.parent_row();
                }
                return;
            }
        }
        self.restore_focus(Some(key));
    }
    fn parent_row(&mut self) {
        let rows = self.tree_rows();
        let Some(row) = rows.get(self.row) else {
            return;
        };
        if let Some(parent) = rows[..self.row].iter().rposition(|p| p.depth < row.depth) {
            self.row = parent;
        }
    }
    fn jump_branch(&mut self, forward: bool) {
        let rows = self.tree_rows();
        let branches: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r.node, Node::Account(_) | Node::Folder(_)))
            .map(|(i, _)| i)
            .collect();
        if let Some(next) = if forward {
            branches.iter().copied().find(|i| *i > self.row)
        } else {
            branches.iter().rev().copied().find(|i| *i < self.row)
        } {
            self.row = next;
        }
    }
    fn move_row(&mut self, amount: isize) {
        if matches!(
            self.view,
            View::Messages | View::Accounts | View::Calendar | View::Contacts | View::Tasks
        ) {
            self.row = self
                .row
                .saturating_add_signed(amount)
                .min(self.count().saturating_sub(1));
        } else {
            self.body_scroll = self.body_scroll.saturating_add_signed(amount).min(65_535);
        }
    }

    fn navigate(&mut self, view: View) {
        let key = self.focused_key();
        self.generation();
        self.busy = false;
        self.stack.push(Navigation {
            view: self.view,
            key,
            row: self.row,
            offset: self.table_offset,
            filter: self.filter.clone(),
            account: self.scope.account.clone(),
            messages: self.messages.clone(),
            pages: self.pages.clone(),
            expanded: self.expanded.clone(),
            collapsed_accounts: self.collapsed_accounts.clone(),
            search: self.search.clone(),
            unread: self.unread,
            flagged: self.flagged,
            body_scroll: self.body_scroll,
            detail: self.detail.clone(),
            revision: self.revision,
            initialized: self.initialized,
        });
        if self.stack.len() > 12 {
            self.stack.remove(0);
        }
        self.view = view;
        self.row = 0;
        self.table_offset = 0;
        self.filter.clear();
        self.body_scroll = 0;
    }
    fn back(&mut self) {
        if !self.filter.is_empty() {
            self.filter.clear();
            self.row = 0;
            return;
        }
        let Some(n) = self.stack.pop() else {
            self.request_quit();
            return;
        };
        let changed = n.account != self.scope.account;
        let stale = n.revision != self.revision || changed;
        self.generation();
        self.busy = false;
        self.view = n.view;
        self.row = n.row;
        self.table_offset = n.offset;
        self.filter = n.filter;
        self.messages = n.messages;
        self.pages = n.pages;
        self.expanded = n.expanded;
        self.collapsed_accounts = n.collapsed_accounts;
        self.search = n.search;
        self.unread = n.unread;
        self.flagged = n.flagged;
        self.body_scroll = n.body_scroll;
        self.detail = n.detail;
        self.initialized = n.initialized;
        if self.view == View::Detail && changed {
            self.view = View::Messages;
            self.detail = None;
        }
        if self.view == View::Detail {
            self.parsed = None;
            if let Some(id) = self.detail.as_ref().map(|m| m.id.clone()) {
                self.send(Request::Detail {
                    generation: self.generation,
                    id,
                });
            }
        } else if self.view == View::Messages {
            self.restore_focus(n.key.clone());
            if stale || !self.initialized {
                self.requested_focus = n.key;
                self.reload(false);
            }
        }
    }
    pub fn switch_account(&mut self, account: Option<String>) {
        if self.view.is_pim() {
            self.filter.clear();
        }
        self.scope.account = account;
        if self.view != View::Messages {
            self.view = View::Messages;
        }
        self.selected.clear();
        self.last_mark = None;
        self.row = 0;
        self.table_offset = 0;
        self.messages.clear();
        self.pages.clear();
        self.parsed = None;
        self.detail = None;
        self.reload(false);
    }
    fn select_account(&mut self) {
        if let Some(index) = self.account_rows().get(self.row) {
            let account = index.map(|i| self.accounts[i].id.clone());
            self.filter.clear();
            if !self.stack.is_empty() {
                self.back();
            }
            self.switch_account(account);
        }
    }
    fn show_folder(&mut self, key: FolderKey) {
        let was_searching = self.searching();
        if self.view != View::Messages || self.searching() {
            self.navigate(View::Messages);
            self.search.clear();
            if was_searching {
                self.messages.clear();
                self.pages.clear();
            }
        }
        self.filter.clear();
        self.expand_ancestors(&key);
        if !self.focus_key(&RowKey::Folder(key.clone())) {
            self.requested_focus = Some(RowKey::Folder(key.clone()));
        }
        if self.initialized {
            self.load_folder(key);
        } else {
            self.reload(false);
        }
    }
    fn start_search(&mut self, query: String) {
        let generation = self.generation();
        self.messages.clear();
        self.pages.clear();
        self.search = query.clone();
        self.send(Request::Search {
            generation,
            query,
            account: self.scope.account.clone(),
        });
    }
    fn mutate(&mut self, mutation: Mutation) {
        if !matches!(self.view, View::Messages | View::Detail) {
            return;
        }
        let ids = self.targets();
        if ids.is_empty() {
            self.notify("Select a message; account and folder rows are not emails");
            return;
        }
        let generation = self.generation();
        self.send(Request::Mutate {
            generation,
            ids,
            mutation,
        });
    }
    pub fn confirm_quit(&mut self, now: std::time::Instant) {
        if self.quit_armed.take().is_some_and(|armed| {
            now.saturating_duration_since(armed) <= std::time::Duration::from_secs(3)
        }) {
            self.quit = true;
        } else {
            self.quit_armed = Some(now);
        }
    }
    pub fn quit_hint(&self) -> Option<&'static str> {
        self.quit_armed
            .filter(|armed| armed.elapsed() <= std::time::Duration::from_secs(3))
            .map(|_| "Press Ctrl+C again within 3s to quit")
    }
    fn request_quit(&mut self) {
        if self.pending.is_empty() {
            self.quit = true;
        } else {
            self.overlay = Some(Overlay::Quit);
        }
    }

    fn pim_form(&mut self, original: Option<usize>) {
        if self.pim_loading || self.pim_pending {
            self.notify("Wait for the PIM operation to finish");
            return;
        }
        let contact = self.view == View::Contacts;
        if contact
            && original
                .and_then(|i| self.contacts.get(i))
                .is_some_and(|c| c.is_mail())
        {
            self.notify("Mail contacts are derived from the index and are read-only");
            return;
        }
        let focused_collection = self
            .pim_focused_key()
            .and_then(|key| {
                let id = match key {
                    PimRowKey::Collection(id) | PimRowKey::Entry { collection: id, .. } => id,
                };
                self.pim_collections().iter().position(|c| c.id == id)
            })
            .unwrap_or(0);
        if original.is_some_and(|i| {
            i >= if contact {
                self.contacts.len()
            } else if self.view == View::Tasks {
                self.tasks.len()
            } else {
                self.calendar_events.len()
            }
        }) {
            return;
        }
        if self.view == View::Tasks {
            if self.task_collections.is_empty() {
                self.notify("No task lists configured");
                return;
            }
            let task = original.and_then(|index| self.tasks.get(index));
            let collection = task
                .and_then(|task| {
                    self.task_collections
                        .iter()
                        .position(|c| c.id == task.collection)
                })
                .unwrap_or(focused_collection);
            self.overlay = Some(Overlay::TaskForm(Box::new(
                crate::task_form::TaskEditor::new(collection, original, task),
            )));
            return;
        }
        let (collection, values) = if contact {
            if self.contact_books.is_empty() {
                self.notify("No address books configured");
                return;
            }
            let item = original.and_then(|i| self.contacts.get(i));
            let collection = item
                .and_then(|c| self.contact_books.iter().position(|b| b.id == c.book))
                .unwrap_or(0);
            let mut values = item
                .map(|c| {
                    vec![
                        c.name.clone(),
                        c.email.clone(),
                        c.phone.clone(),
                        c.organization.clone(),
                    ]
                })
                .unwrap_or_else(|| vec![String::new(); 4]);
            for kind in 0..3 {
                let address = item
                    .map(|c| &c.addresses[kind])
                    .cloned()
                    .unwrap_or_default();
                values.extend([
                    address.street,
                    address.city,
                    address.region,
                    address.postal_code,
                    address.country,
                ]);
            }
            (collection, values)
        } else {
            if self.calendar_collections.is_empty() {
                self.notify("No calendars configured");
                return;
            }
            let item = original.and_then(|i| self.calendar_events.get(i));
            let collection = item
                .and_then(|e| {
                    self.calendar_collections
                        .iter()
                        .position(|c| c.id == e.collection)
                })
                .unwrap_or(focused_collection);
            let values = item
                .map(|e| {
                    vec![
                        e.summary.clone(),
                        e.start.clone(),
                        e.end.clone(),
                        e.location.clone(),
                        e.description.clone(),
                        e.recurrence.clone(),
                    ]
                })
                .unwrap_or_else(|| vec![String::new(); 6]);
            (collection, values)
        };
        self.overlay = Some(Overlay::PimForm {
            contact,
            collection,
            original,
            fields: values
                .into_iter()
                .map(|v| {
                    let mut i = Input::default();
                    i.set(v);
                    i
                })
                .collect(),
            field: if original.is_some() { 1 } else { 0 },
            error: None,
            book_menu: None,
            contact_page: 0,
        });
    }
    fn pim_save(
        &mut self,
        contact: bool,
        collection: usize,
        original: Option<usize>,
        fields: &[Input],
    ) -> anyhow::Result<()> {
        if contact {
            let book = self
                .contact_books
                .get(collection)
                .ok_or_else(|| anyhow::anyhow!("Address book unavailable"))?;
            let prior = original.and_then(|i| self.contacts.get(i));
            let item = crate::pim::Contact {
                book: book.id.clone(),
                book_label: book.label.clone(),
                uid: prior
                    .filter(|c| !c.uid.is_empty())
                    .map(|c| c.uid.clone())
                    .unwrap_or_else(|| format!("mailtui-{:016x}", rand::random::<u64>())),
                name: fields[0].text.clone(),
                email: fields[1].text.clone(),
                phone: fields[2].text.clone(),
                organization: fields[3].text.clone(),
                addresses: std::array::from_fn(|kind| {
                    let start = 4 + kind * 5;
                    crate::pim::PostalAddress {
                        street: fields[start].text.clone(),
                        city: fields[start + 1].text.clone(),
                        region: fields[start + 2].text.clone(),
                        postal_code: fields[start + 3].text.clone(),
                        country: fields[start + 4].text.clone(),
                        ..prior.map(|c| c.addresses[kind].clone()).unwrap_or_default()
                    }
                }),
                file: prior.map(|c| c.file.clone()).unwrap_or_default(),
                source: prior.map(|c| c.source.clone()).unwrap_or_default(),
            };
            if !self.pim_request(crate::pim::Change::Contact {
                book: book.clone(),
                entry: Box::new(item),
            }) {
                anyhow::bail!("Unable to queue operation; retry");
            }
        } else if self.view == View::Tasks {
            let collection = self
                .task_collections
                .get(collection)
                .ok_or_else(|| anyhow::anyhow!("Task list unavailable"))?;
            let prior = original.and_then(|i| self.tasks.get(i));
            let entry = crate::pim::Task {
                collection: collection.id.clone(),
                collection_label: collection.label.clone(),
                uid: prior
                    .map(|t| t.uid.clone())
                    .unwrap_or_else(|| format!("mailtui-{:016x}", rand::random::<u64>())),
                summary: fields[0].text.clone(),
                due: fields[1].text.clone(),
                status: if fields[2].text.trim().eq_ignore_ascii_case("to do") {
                    "NEEDS-ACTION".into()
                } else {
                    fields[2].text.trim().to_uppercase()
                },
                priority: fields[3].text.clone(),
                description: fields[4].text.clone(),
                start: fields[5].text.clone(),
                file: prior.map(|t| t.file.clone()).unwrap_or_default(),
                source: prior.map(|t| t.source.clone()).unwrap_or_default(),
            };
            let change = crate::pim::Change::Task {
                collection: collection.clone(),
                entry,
            };
            crate::pim::validate_change(&change)?;
            if !self.pim_request(change) {
                anyhow::bail!("Unable to queue operation; retry");
            }
        } else {
            let calendar = self
                .calendar_collections
                .get(collection)
                .ok_or_else(|| anyhow::anyhow!("Calendar unavailable"))?;
            let prior = original.and_then(|i| self.calendar_events.get(i));
            let item = crate::pim::Event {
                collection: calendar.id.clone(),
                collection_label: calendar.label.clone(),
                uid: prior
                    .map(|e| e.uid.clone())
                    .unwrap_or_else(|| format!("mailtui-{:016x}", rand::random::<u64>())),
                summary: fields[0].text.clone(),
                start: fields[1].text.clone(),
                end: fields[2].text.clone(),
                location: fields[3].text.clone(),
                description: fields[4].text.clone(),
                file: prior.map(|e| e.file.clone()).unwrap_or_default(),
                source: prior.map(|e| e.source.clone()).unwrap_or_default(),
                recurrence: fields[5].text.clone(),
                timezone: prior.map(|e| e.timezone.clone()).unwrap_or_default(),
            };
            if !self.pim_request(crate::pim::Change::Event {
                collection: calendar.clone(),
                entry: item,
            }) {
                anyhow::bail!("Unable to queue operation; retry");
            }
        }
        Ok(())
    }
    fn pim_delete(&mut self, contact: bool, index: usize) {
        let change = if self.view == View::Tasks {
            self.tasks.get(index).map(|t| crate::pim::Change::Delete {
                file: t.file.clone(),
                source: t.source.clone(),
            })
        } else if contact {
            self.contacts
                .get(index)
                .map(|c| crate::pim::Change::Delete {
                    file: c.file.clone(),
                    source: c.source.clone(),
                })
        } else {
            self.calendar_events
                .get(index)
                .map(|e| crate::pim::Change::Delete {
                    file: e.file.clone(),
                    source: e.source.clone(),
                })
        };
        if let Some(change) = change {
            self.pim_request(change);
        }
    }

    pub fn key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.confirm_quit(std::time::Instant::now());
            return;
        }
        if matches!(self.overlay, Some(Overlay::Assistant)) {
            self.ai_key(key);
            return;
        }
        if let Some(overlay) = self.overlay.clone() {
            if let Overlay::TaskForm(mut editor) = overlay {
                if self.pim_pending {
                    return;
                }
                match editor.key(key, self.task_collections.len()) {
                    crate::task_form::Action::Cancel => {
                        self.overlay = None;
                        return;
                    }
                    crate::task_form::Action::Save => {
                        if let Err(error) = self.pim_save(
                            false,
                            editor.collection,
                            editor.original,
                            &editor.inputs(),
                        ) {
                            editor.error = Some(format!("{error:#}"));
                        }
                    }
                    crate::task_form::Action::Continue => {}
                }
                self.overlay = Some(Overlay::TaskForm(editor));
                return;
            }
            if let Overlay::PimForm {
                contact,
                mut collection,
                original,
                mut fields,
                mut field,
                mut error,
                mut book_menu,
                mut contact_page,
            } = overlay
            {
                if self.pim_pending {
                    return;
                }
                if let Some(mut row) = book_menu {
                    let count = self.contact_books.len();
                    match key.code {
                        KeyCode::Esc => book_menu = None,
                        KeyCode::Up => row = row.saturating_sub(1),
                        KeyCode::Down => row = (row + 1).min(count.saturating_sub(1)),
                        KeyCode::Home => row = 0,
                        KeyCode::End => row = count.saturating_sub(1),
                        KeyCode::Enter => {
                            collection = row;
                            book_menu = None;
                        }
                        _ => {}
                    }
                    if book_menu.is_some() {
                        book_menu = Some(row);
                    }
                    self.overlay = Some(Overlay::PimForm {
                        contact,
                        collection,
                        original,
                        fields,
                        field,
                        error,
                        book_menu,
                        contact_page,
                    });
                    return;
                }
                let length = fields.len() + if contact { 3 } else { 1 };
                let page_start = if contact_page == 0 {
                    1
                } else {
                    5 + (contact_page - 1) * 5
                };
                let page_end = if contact_page == 0 { 4 } else { page_start + 4 };
                match key.code {
                    KeyCode::Esc => {
                        self.overlay = None;
                        return;
                    }
                    KeyCode::Tab if contact => {
                        field = if field == 0 {
                            page_start
                        } else if field == page_end {
                            length - 2
                        } else {
                            (field + 1) % length
                        };
                    }
                    KeyCode::BackTab if contact => {
                        field = if field == page_start {
                            0
                        } else if field == length - 2 {
                            page_end
                        } else {
                            field.checked_sub(1).unwrap_or(length - 1)
                        };
                    }
                    KeyCode::Tab => field = (field + 1) % length,
                    KeyCode::BackTab => field = field.checked_sub(1).unwrap_or(length - 1),
                    KeyCode::PageUp | KeyCode::PageDown if contact => {
                        contact_page = if key.code == KeyCode::PageDown {
                            (contact_page + 1) % 4
                        } else {
                            (contact_page + 3) % 4
                        };
                        field = if contact_page == 0 {
                            1
                        } else {
                            5 + (contact_page - 1) * 5
                        };
                    }
                    KeyCode::Enter
                        if contact
                            && matches!(field, 5 | 10 | 15)
                            && key.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        let input = &mut fields[field - 1];
                        if input.text.len() < 8192 {
                            input.text.insert(input.cursor, '\n');
                            input.cursor += 1;
                        }
                    }
                    KeyCode::Enter if contact && field == 0 => {
                        if original.is_none() {
                            book_menu = Some(collection);
                        } else {
                            field = page_start;
                        }
                    }
                    KeyCode::Enter if contact && field == length - 1 => {
                        self.overlay = None;
                        return;
                    }
                    KeyCode::Left | KeyCode::Right if contact && field > fields.len() => {
                        field = if field == length - 1 {
                            length - 2
                        } else {
                            length - 1
                        };
                    }
                    KeyCode::Left | KeyCode::Right if field == 0 && original.is_none() => {
                        let count = if contact {
                            self.contact_books.len()
                        } else if self.view == View::Tasks {
                            self.task_collections.len()
                        } else {
                            self.calendar_collections.len()
                        };
                        collection = if key.code == KeyCode::Right {
                            (collection + 1) % count
                        } else {
                            (collection + count - 1) % count
                        };
                    }
                    KeyCode::Enter
                        if field < fields.len() || (contact && field == fields.len()) =>
                    {
                        field = if contact && field == page_end {
                            length - 2
                        } else {
                            field + 1
                        };
                    }
                    KeyCode::Enter | KeyCode::Char('s')
                        if key.code == KeyCode::Enter
                            || key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        if let Err(reason) = self.pim_save(contact, collection, original, &fields) {
                            error = Some(format!("{reason:#}"));
                        }
                    }
                    _ if field > 0 && field <= fields.len() => {
                        fields[field - 1].key(key);
                        error = None;
                    }
                    _ => {}
                }
                if contact && field > 0 && field <= fields.len() {
                    contact_page = if field <= 4 { 0 } else { 1 + (field - 5) / 5 };
                }
                self.overlay = Some(Overlay::PimForm {
                    contact,
                    collection,
                    original,
                    fields,
                    field,
                    error,
                    book_menu,
                    contact_page,
                });
                return;
            }
            if let Overlay::PimDelete {
                contact,
                index,
                confirm,
            } = overlay
            {
                match key.code {
                    KeyCode::Esc => self.overlay = None,
                    KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                        self.overlay = Some(Overlay::PimDelete {
                            contact,
                            index,
                            confirm: !confirm,
                        });
                    }
                    KeyCode::Char('y' | 'Y') => {
                        self.overlay = Some(Overlay::PimDelete {
                            contact,
                            index,
                            confirm: true,
                        })
                    }
                    KeyCode::Char('n' | 'N') | KeyCode::Enter => self.overlay = None,
                    _ => {}
                }
                if matches!(key.code, KeyCode::Enter) && confirm {
                    self.pim_delete(contact, index);
                }
                return;
            }
            if matches!(
                overlay,
                Overlay::Find { .. }
                    | Overlay::Move { .. }
                    | Overlay::CreateFolder { .. }
                    | Overlay::Compose { .. }
                    | Overlay::Files { .. }
                    | Overlay::Attachments { .. }
                    | Overlay::SaveAttachments { .. }
            ) {
                self.form_key(overlay, key);
                return;
            }
            match (overlay, key.code) {
                (_, KeyCode::Esc | KeyCode::Char('q')) => self.overlay = None,
                (
                    Overlay::Confirm { ids, confirm },
                    KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right,
                ) => {
                    self.overlay = Some(Overlay::Confirm {
                        ids,
                        confirm: !confirm,
                    })
                }
                (Overlay::Confirm { confirm: false, .. }, KeyCode::Enter) => self.overlay = None,
                (Overlay::Confirm { ids, confirm: true }, KeyCode::Enter) => {
                    self.overlay = None;
                    let generation = self.generation();
                    self.send(Request::Mutate {
                        generation,
                        ids,
                        mutation: Mutation::Trash,
                    });
                }
                (
                    Overlay::Confirm { .. } | Overlay::Help | Overlay::PimDetails(_),
                    KeyCode::Down,
                ) => self.help_scroll = self.help_scroll.saturating_add(1).min(65_535),
                (Overlay::Confirm { .. } | Overlay::Help | Overlay::PimDetails(_), KeyCode::Up) => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                (
                    Overlay::Confirm { .. } | Overlay::Help | Overlay::PimDetails(_),
                    KeyCode::PageDown,
                ) => {
                    self.help_scroll = self
                        .help_scroll
                        .saturating_add(self.page_height.max(1))
                        .min(65_535)
                }
                (
                    Overlay::Confirm { .. } | Overlay::Help | Overlay::PimDetails(_),
                    KeyCode::PageUp,
                ) => self.help_scroll = self.help_scroll.saturating_sub(self.page_height.max(1)),
                (Overlay::Help, KeyCode::Char('/')) => {
                    self.overlay = None;
                    self.navigate(View::Help);
                    self.input = Some((InputMode::Filter, Input::default()));
                }
                (Overlay::Quit, KeyCode::Enter) => self.quit = true,
                _ => {}
            }
            return;
        }
        if self.input.is_some() {
            self.input_key(key);
            return;
        }
        if let Some(action) = dispatch(key) {
            self.action(action);
        }
    }
    pub fn action(&mut self, action: Action) {
        use Action::*;
        let previous_g = self.g_pending;
        self.g_pending = false;
        match action {
            Search | Filter if self.view.is_pim() => {
                let mut input = Input::default();
                input.set(self.filter.clone());
                self.input = Some((InputMode::Filter, input));
            }
            Open | Read if self.view.is_pim() => {
                if action == Open && self.toggle_pim_branch() {
                    return;
                }
                if let Some(index) = self.pim_focused_index() {
                    if self.view == View::Tasks {
                        self.pim_form(Some(index));
                        return;
                    }
                    let body = if self.view == View::Calendar {
                        let e = &self.calendar_events[index];
                        format!(
                            "{}\nCalendar: {}\nStart: {}\nEnd (exclusive): {}\nTimezone: {}\nLocation: {}\nRepeat rule: {}\n\n{}\n\nEsc close · e edits from the list",
                            e.summary,
                            e.collection_label,
                            crate::pim::display_date(&e.start),
                            crate::pim::display_date(&e.end),
                            if e.timezone.is_empty() {
                                "floating / UTC"
                            } else {
                                &e.timezone
                            },
                            e.location,
                            if e.recurrence.is_empty() {
                                "none"
                            } else {
                                &e.recurrence
                            },
                            e.description
                        )
                    } else {
                        let c = &self.contacts[index];
                        let addresses = c
                            .addresses
                            .iter()
                            .enumerate()
                            .filter(|(_, address)| !address.is_empty())
                            .map(|(kind, address)| {
                                format!(
                                    "\n{} address: {}",
                                    crate::pim::ADDRESS_LABELS[kind],
                                    address.display()
                                )
                            })
                            .collect::<String>();
                        format!(
                            "{}\nSource: {}\nEmail: {}\nPhone: {}\nOrganization: {}{}\n\n{}\n\nEsc close{}",
                            c.name,
                            c.book_label,
                            c.email,
                            c.phone,
                            c.organization,
                            addresses,
                            if c.is_mail() {
                                "Derived from indexed mail headers · read-only".into()
                            } else {
                                c.file.display().to_string()
                            },
                            if c.is_mail() {
                                ""
                            } else {
                                " · e edits from the list"
                            }
                        )
                    };
                    self.help_scroll = 0;
                    self.overlay = Some(Overlay::PimDetails(crate::mime::terminal_text(&body)));
                }
            }
            Assistant => self.open_ai(false, false),
            SaveAttachments if self.view == View::Detail => {
                if let Some(parsed) = &self.parsed {
                    if parsed.attachments.is_empty() {
                        self.notify("No attachments in this message");
                    } else {
                        self.overlay = Some(Overlay::Attachments {
                            query: Input::default(),
                            row: 0,
                            selected: HashSet::new(),
                        });
                    }
                } else {
                    self.notify("Wait for the message to load");
                }
            }
            Search => {
                self.overlay = Some(Overlay::Find {
                    query: Input::default(),
                    row: 0,
                    messages: vec![],
                    loading: false,
                    again: false,
                    account: self.scope.account.clone(),
                    error: None,
                });
                self.index_dirty = true;
                self.find_request();
            }
            ViewSlot(slot) => match slot {
                0 => self.switch_account(None),
                1 if !self.calendar_collections.is_empty() => {
                    if self.view != View::Calendar {
                        self.navigate(View::Calendar);
                    }
                    self.reload_pim();
                }
                2 => {
                    if self.view != View::Contacts {
                        self.navigate(View::Contacts);
                    }
                    self.reload_pim();
                }
                3 if !self.task_collections.is_empty() => {
                    if self.view != View::Tasks {
                        self.navigate(View::Tasks);
                    }
                    self.reload_pim();
                }
                1 => self.notify("Calendar view is not enabled/configured"),
                3 => self.notify("Tasks view is not enabled/configured"),
                _ => {}
            },
            Command => {
                self.history_pos = self.history.len();
                self.input = Some((InputMode::Command, Input::default()));
            }
            Filter if matches!(self.view, View::Messages | View::Accounts | View::Help) => {
                let mut input = Input::default();
                input.set(self.filter.clone());
                self.input = Some((InputMode::Filter, input));
            }
            Help => {
                self.help_scroll = 0;
                self.overlay = Some(Overlay::Help);
            }
            Back => self.back(),
            Quit => self.confirm_quit(std::time::Instant::now()),
            Next => self.move_row(1),
            Previous => self.move_row(-1),
            PageDown => self.move_row(self.page_height.max(1) as isize),
            PageUp => self.move_row(-(self.page_height.max(1) as isize)),
            HalfUp => self.move_row(-((self.page_height / 2).max(1) as isize)),
            FirstPrefix => {
                if previous_g {
                    self.row = 0;
                    self.body_scroll = 0;
                } else {
                    self.g_pending = true;
                }
            }
            Last => {
                self.row = self.count().saturating_sub(1);
                if self.view == View::Detail {
                    self.body_scroll = self
                        .parsed
                        .as_ref()
                        .map(|p| p.text.lines().count().saturating_sub(self.page_height))
                        .unwrap_or(0);
                }
            }
            NextBranch | PreviousBranch if self.view == View::Messages => {
                self.jump_branch(action == NextBranch)
            }
            CollapseAll if self.view == View::Messages => {
                self.expanded.clear();
                if self.scope.account.is_none() {
                    self.collapsed_accounts
                        .extend(self.accounts.iter().map(|a| a.id.clone()));
                }
                self.row = 0;
                self.table_offset = 0;
            }
            Expand | Collapse if self.view == View::Messages => {
                self.branch(action == Expand, false)
            }
            Open if self.view == View::Accounts => self.select_account(),
            Open | Read if self.view == View::Messages => {
                if let Some(message) = self.focused().cloned() {
                    self.navigate(View::Detail);
                    self.detail = Some(message.clone());
                    self.parsed = None;
                    self.send(Request::Detail {
                        generation: self.generation,
                        id: message.id,
                    });
                } else if action == Open {
                    self.branch(true, true);
                }
            }
            Select if self.view == View::Messages => {
                if let Some(id) = self.focused().map(|m| m.id.clone()) {
                    self.last_mark = Some(id.clone());
                    if !self.selected.remove(&id) {
                        self.selected.insert(id);
                    }
                }
            }
            SelectRange if self.view == View::Messages => {
                if let Some(end) = self.focused().map(|m| m.id.clone()) {
                    let visible = self.visible_message_indices();
                    let end_pos = visible
                        .iter()
                        .position(|i| self.messages[*i].id == end)
                        .unwrap();
                    let start = visible
                        .iter()
                        .position(|i| self.last_mark.as_ref() == Some(&self.messages[*i].id))
                        .unwrap_or(end_pos);
                    for i in &visible[start.min(end_pos)..=start.max(end_pos)] {
                        self.selected.insert(self.messages[*i].id.clone());
                    }
                    self.last_mark = Some(end);
                }
            }
            SelectAll if self.view == View::Messages => {
                for i in self.visible_message_indices() {
                    self.selected.insert(self.messages[i].id.clone());
                }
            }
            ClearSelection => {
                self.selected.clear();
                self.last_mark = None;
            }
            Delete if self.view.is_pim() => {
                if self.pim_loading || self.pim_pending {
                    self.notify("Wait for the PIM operation to finish");
                    return;
                }
                let contact = self.view == View::Contacts;
                if let Some(index) = self.pim_focused_index() {
                    if contact && self.contacts[index].is_mail() {
                        self.notify("Mail contacts are derived from the index and are read-only");
                        return;
                    }
                    self.overlay = Some(Overlay::PimDelete {
                        contact,
                        index,
                        confirm: false,
                    });
                }
            }
            Delete if matches!(self.view, View::Messages | View::Detail) => {
                let ids = self.targets();
                if ids.is_empty() {
                    self.notify(
                        "Select messages to delete; accounts and folders cannot be deleted here",
                    );
                } else {
                    self.help_scroll = 0;
                    self.overlay = Some(Overlay::Confirm {
                        ids,
                        confirm: false,
                    });
                }
            }
            Seen => self.mutate(Mutation::Seen(true)),
            ToggleSeen if matches!(self.view, View::Messages | View::Detail) => {
                let ids: HashSet<_> = self.targets().into_iter().collect();
                let seen = if self.view == View::Detail {
                    self.detail.as_ref().is_some_and(|m| !m.seen)
                } else {
                    self.messages.iter().any(|m| ids.contains(&m.id) && !m.seen)
                };
                self.mutate(Mutation::Seen(seen));
            }
            Unseen if self.view == View::Accounts => self.select_account(),
            Unseen => self.mutate(Mutation::Seen(false)),
            Flag => {
                if let Some(m) = self.focused() {
                    self.mutate(Mutation::Flagged(!m.flagged));
                }
            }
            Move if matches!(self.view, View::Messages | View::Detail) => {
                let ids = self.targets();
                if ids.is_empty() {
                    self.notify("Select a message first");
                } else if ids.iter().any(|id| id.account != ids[0].account) {
                    self.notify("Move messages from one account at a time");
                } else {
                    self.overlay = Some(Overlay::Move {
                        ids,
                        query: Input::default(),
                        row: 0,
                    });
                }
            }
            Copy if matches!(self.view, View::Messages | View::Detail) => {
                if self.targets().is_empty() {
                    self.notify("Select a message first");
                } else {
                    self.input = Some((InputMode::Copy, Input::default()));
                }
            }
            CreateFolder if self.view.is_pim() => self.pim_form(None),
            CreateFolder => {
                let account = self.context_account();
                let key = self.focused_folder().or_else(|| {
                    self.folders
                        .iter()
                        .find(|f| account.as_ref().is_none_or(|a| a == &f.account))
                        .map(tree::folder_key)
                });
                if let Some((account, anchor)) = key {
                    self.overlay = Some(Overlay::CreateFolder {
                        account,
                        anchor,
                        parent: false,
                        name: Input::default(),
                    });
                } else {
                    self.notify("Load an account with a folder first");
                }
            }
            Html | HtmlRemote if matches!(self.view, View::Detail | View::Messages) => {
                self.html_remote = action == HtmlRemote;
                if self.view == View::Messages {
                    if self.focused().is_some() {
                        self.action(Read);
                        self.html_on_load = true;
                    }
                } else if self.parsed.is_some() {
                    self.want_html = true;
                } else {
                    if !self.busy
                        && let Some(id) = self.detail.as_ref().map(|message| message.id.clone())
                    {
                        let generation = self.generation();
                        self.send(Request::Detail { generation, id });
                    }
                    self.html_on_load = true;
                }
            }
            Reload => self.execute("reload"),
            Sync => self.request_sync(),
            TogglePimHistory if matches!(self.view, View::Calendar | View::Tasks) => {
                let focus = self.pim_focused_key();
                if self.view == View::Calendar {
                    self.show_past_events = !self.show_past_events;
                } else {
                    self.show_completed_tasks = !self.show_completed_tasks;
                }
                self.restore_pim_focus(focus);
                self.table_offset = 0;
            }
            More if self.view == View::Messages => self.load_more(),
            Sort | SortSubject | SortDate | SortAccount | SortStatus => {
                let field = match action {
                    SortSubject => "subject",
                    SortDate => "date",
                    SortAccount => "account",
                    SortStatus => "status",
                    _ => match self.sort.as_str() {
                        "date" => "from",
                        "from" => "subject",
                        "subject" => "size",
                        _ => "date",
                    },
                };
                let focus = self.focused_key();
                self.descending = if self.sort == field {
                    !self.descending
                } else {
                    field == "date"
                };
                self.sort = field.into();
                self.restore_focus(focus);
            }
            ToggleHeader => self.show_header = !self.show_header,
            ToggleWide => self.wide = !self.wide,
            ToggleBreadcrumbs => self.show_breadcrumbs = !self.show_breadcrumbs,
            Aliases => self.overlay = Some(Overlay::Aliases),
            EditItem if self.view.is_pim() => {
                if let Some(index) = self.pim_focused_index() {
                    self.pim_form(Some(index));
                }
            }
            Compose => {
                if self.view.is_pim() {
                    self.pim_form(None);
                    return;
                }
                self.compose_reply = None;
                if self.accounts.is_empty() {
                    self.notify("No sending account configured");
                } else {
                    let active = self.context_account();
                    let account = self
                        .accounts
                        .iter()
                        .position(|a| Some(&a.id) == active.as_ref())
                        .unwrap_or(0);
                    self.overlay = Some(Overlay::Compose {
                        account,
                        field: 1,
                        fields: Default::default(),
                        attachments: vec![],
                    });
                }
            }
            Reply | ReplyAll | Forward => self.notify("Use c to compose a new message"),
            _ => {}
        }
    }

    pub fn move_destinations(&self, ids: &[MessageId], query: &str) -> Vec<FolderKey> {
        let Some(first) = ids.first() else {
            return vec![];
        };
        let mut folders: Vec<_> = self
            .folders
            .iter()
            .filter(|f| f.account == first.account)
            .filter_map(|f| {
                fuzzy_score(&format!("{} {}", f.name, f.id), query)
                    .map(|score| (score, tree::folder_key(f)))
            })
            .collect();
        folders.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        folders.into_iter().map(|(_, key)| key).collect()
    }
    pub fn browse_files(&mut self, path: std::path::PathBuf) {
        if let Some(Overlay::Files {
            path: current,
            entries,
            query,
            row,
            loading,
            error,
            ..
        }) = &mut self.overlay
        {
            *current = path.clone();
            entries.clear();
            *query = Input::default();
            *row = 0;
            *loading = true;
            *error = None;
        } else {
            return;
        }
        if !self.send(Request::Files {
            generation: self.generation,
            path,
        }) && let Some(Overlay::Files { loading, error, .. }) = &mut self.overlay
        {
            *loading = false;
            *error = Some("Background queue busy; retry".into());
        }
    }
    fn form_key(&mut self, mut overlay: Overlay, key: KeyEvent) {
        if self.form_pending {
            return;
        }
        if key.code == KeyCode::Esc {
            self.overlay = match overlay {
                Overlay::Files { composer, .. } => Some(*composer),
                Overlay::SaveAttachments { picker, .. } => Some(*picker),
                _ => None,
            };
            return;
        }
        if let Overlay::Attachments {
            query,
            row,
            selected,
        } = &mut overlay
        {
            let attachments = self
                .parsed
                .as_ref()
                .map(|parsed| parsed.attachments.as_slice())
                .unwrap_or_default();
            let matches = attachment_matches(attachments, &query.text);
            match key.code {
                KeyCode::Up => *row = row.saturating_sub(1),
                KeyCode::Down => *row = (*row + 1).min(matches.len().saturating_sub(1)),
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if matches.iter().all(|index| selected.contains(index)) {
                        for index in matches {
                            selected.remove(&index);
                        }
                    } else {
                        selected.extend(matches);
                    }
                }
                KeyCode::Tab | KeyCode::Char(' ') => {
                    if let Some(index) = matches.get(*row) {
                        if !selected.remove(index) {
                            selected.insert(*index);
                        }
                        *row = (*row + 1).min(matches.len().saturating_sub(1));
                    }
                }
                KeyCode::Enter => {
                    if selected.is_empty()
                        && let Some(index) = matches.get(*row)
                    {
                        selected.insert(*index);
                    }
                    if !selected.is_empty() {
                        let mut directory = Input::default();
                        directory.set(
                            crate::files::download_directory()
                                .to_string_lossy()
                                .into_owned(),
                        );
                        self.overlay = Some(Overlay::SaveAttachments {
                            picker: Box::new(overlay),
                            directory,
                            error: None,
                        });
                        return;
                    }
                }
                _ => {
                    query.key(key);
                    *row = 0;
                }
            }
            self.overlay = Some(overlay);
            return;
        }
        if let Overlay::SaveAttachments {
            picker,
            directory,
            error,
        } = &mut overlay
        {
            if key.code == KeyCode::Enter {
                match crate::files::destination(&directory.text) {
                    Ok(directory) => {
                        let Some(parsed) = &self.parsed else {
                            return;
                        };
                        let Overlay::Attachments { selected, .. } = picker.as_ref() else {
                            return;
                        };
                        let attachments = parsed
                            .attachments
                            .iter()
                            .enumerate()
                            .filter(|(index, _)| selected.contains(index))
                            .map(|(_, attachment)| attachment.clone())
                            .collect();
                        self.form_pending = self.send(Request::SaveAttachments {
                            generation: self.generation,
                            directory,
                            attachments,
                        });
                    }
                    Err(reason) => *error = Some(reason.to_string()),
                }
            } else {
                directory.key(key);
                *error = None;
            }
            self.overlay = Some(overlay);
            return;
        }
        if let Overlay::Find {
            query,
            row,
            messages,
            loading,
            ..
        } = &mut overlay
        {
            let previous = query.text.clone();
            match key.code {
                KeyCode::Up => *row = row.saturating_sub(1),
                KeyCode::Down => *row = (*row + 1).min(messages.len().saturating_sub(1)),
                KeyCode::Enter if !*loading => {
                    if let Some(message) = messages.get(*row).cloned() {
                        self.overlay = None;
                        self.navigate(View::Detail);
                        self.detail = Some(message.clone());
                        self.parsed = None;
                        self.send(Request::Detail {
                            generation: self.generation,
                            id: message.id,
                        });
                        return;
                    }
                }
                _ => {
                    query.key(key);
                    *row = 0;
                }
            }
            let changed = previous != query.text;
            self.overlay = Some(overlay);
            if changed {
                self.find_request();
            }
            return;
        }
        if key.code == KeyCode::Char('t')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && let Overlay::Compose { attachments, .. } = &overlay
        {
            let selected = attachments.iter().cloned().collect();
            let path = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| "/".into());
            self.overlay = Some(Overlay::Files {
                composer: Box::new(overlay),
                path: path.clone(),
                entries: vec![],
                query: Input::default(),
                row: 0,
                selected,
                loading: false,
                error: None,
            });
            self.browse_files(path);
            return;
        }
        if let Overlay::Files {
            composer,
            path,
            entries,
            query,
            row,
            selected,
            loading,
            ..
        } = &mut overlay
        {
            let matches = file_matches(entries, &query.text);
            let mut directory = None;
            let mut apply = false;
            match key.code {
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    selected.clear()
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    apply = true;
                }
                KeyCode::Up => *row = row.saturating_sub(1),
                KeyCode::Down => *row = (*row + 1).min(matches.len().saturating_sub(1)),
                KeyCode::Backspace if query.text.is_empty() => {
                    directory = path.parent().map(std::path::Path::to_path_buf)
                }
                KeyCode::Enter | KeyCode::Tab if !*loading => {
                    if let Some(entry) = matches.get(*row).map(|index| &entries[*index]) {
                        if entry.directory && key.code == KeyCode::Enter {
                            directory = Some(entry.path.clone());
                        } else if !entry.directory {
                            if key.code == KeyCode::Enter {
                                selected.insert(entry.path.clone());
                                apply = true;
                            } else if !selected.remove(&entry.path) {
                                selected.insert(entry.path.clone());
                            }
                        }
                        if key.code == KeyCode::Tab {
                            *row = (*row + 1).min(matches.len().saturating_sub(1));
                        }
                    }
                }
                _ => {
                    query.key(key);
                    *row = 0;
                }
            }
            if apply {
                let mut composer = (**composer).clone();
                if let Overlay::Compose { attachments, .. } = &mut composer {
                    *attachments = selected.iter().cloned().collect();
                    attachments.sort();
                }
                self.overlay = Some(composer);
                return;
            }
            self.overlay = Some(overlay);
            if let Some(path) = directory {
                self.browse_files(path);
            }
            return;
        }
        match &mut overlay {
            Overlay::Move { ids, query, row } => {
                let destinations = self.move_destinations(ids, &query.text);
                match key.code {
                    KeyCode::Up | KeyCode::BackTab => *row = row.saturating_sub(1),
                    KeyCode::Down | KeyCode::Tab => {
                        *row = (*row + 1).min(destinations.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some((_, folder)) = destinations.get(*row) {
                            let generation = self.generation();
                            if self.send(Request::Mutate {
                                generation,
                                ids: ids.clone(),
                                mutation: Mutation::Move(folder.clone()),
                            }) {
                                self.overlay = None;
                            }
                            return;
                        }
                    }
                    _ => {
                        query.key(key);
                        *row = 0;
                    }
                }
            }
            Overlay::CreateFolder {
                account,
                anchor,
                parent,
                name,
            } => match key.code {
                KeyCode::Tab | KeyCode::BackTab => *parent = !*parent,
                KeyCode::Enter => {
                    if name.text.trim().is_empty()
                        || name.text.chars().any(|c| c.is_control() || c == '/')
                    {
                        self.notify("Enter one directory name without slashes");
                    } else {
                        let generation = self.generation();
                        self.form_pending = self.send(Request::CreateFolder {
                            generation,
                            account: account.clone(),
                            anchor: anchor.clone(),
                            parent: parent.then(|| anchor.clone()),
                            name: name.text.trim().into(),
                        });
                    }
                }
                _ => name.key(key),
            },
            Overlay::Compose {
                account,
                field,
                fields,
                attachments,
            } => match key.code {
                KeyCode::Char('s' | 'o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let draft = Draft {
                        reply: self.compose_reply.clone(),
                        account: self.accounts[*account].id.clone(),
                        to: fields[0].text.clone(),
                        subject: fields[1].text.clone(),
                        body: fields[2].text.clone(),
                        attachments: attachments.clone(),
                    };
                    if let Err(error) = if key.code == KeyCode::Char('s') {
                        draft.recipients().map(|_| ())
                    } else {
                        Ok(())
                    } {
                        self.notify(error.to_string());
                    } else {
                        let generation = self.generation();
                        self.form_pending = self.send(Request::Compose {
                            generation,
                            draft,
                            send: key.code == KeyCode::Char('s'),
                        });
                    }
                }
                KeyCode::Tab => *field = (*field + 1) % 4,
                KeyCode::BackTab => *field = (*field + 3) % 4,
                KeyCode::Up if *field == 0 => *account = account.saturating_sub(1),
                KeyCode::Down if *field == 0 => {
                    *account = (*account + 1).min(self.accounts.len().saturating_sub(1))
                }
                KeyCode::Enter if *field == 3 => {
                    let body = &mut fields[2];
                    if body.text.len() < 65536 {
                        body.text.insert(body.cursor, '\n');
                        body.cursor += 1;
                    }
                }
                KeyCode::Enter => *field = (*field + 1) % 4,
                _ if *field > 0 => {
                    fields[*field - 1].key_with_limit(key, if *field == 3 { 65536 } else { 8192 })
                }
                _ => {}
            },
            _ => {}
        }
        self.overlay = Some(overlay);
    }

    fn input_key(&mut self, key: KeyEvent) {
        let (mode, mut input) = self.input.take().unwrap();
        if key.code == KeyCode::Esc {
            if matches!(mode, InputMode::Filter) {
                self.filter.clear();
                self.row = 0;
            }
            return;
        }
        if key.code == KeyCode::Enter {
            match mode {
                InputMode::Filter => self.filter = input.text,
                InputMode::Command => {
                    if !input.text.trim().is_empty() {
                        if self.history.len() == 100 {
                            self.history.pop_front();
                        }
                        self.history.push_back(input.text.clone());
                        self.execute(&input.text);
                    }
                }
                InputMode::Move => self.mutate(Mutation::Move(input.text)),
                InputMode::Copy => self.mutate(Mutation::Copy(input.text)),
            }
            return;
        }
        match (&mode, key.code) {
            (InputMode::Command, KeyCode::Up) => {
                self.history_pos = self.history_pos.saturating_sub(1);
                input.set(
                    self.history
                        .get(self.history_pos)
                        .cloned()
                        .unwrap_or_default(),
                );
            }
            (InputMode::Command, KeyCode::Down) => {
                self.history_pos = (self.history_pos + 1).min(self.history.len());
                input.set(
                    self.history
                        .get(self.history_pos)
                        .cloned()
                        .unwrap_or_default(),
                );
            }
            (InputMode::Command, KeyCode::Tab) => {
                if let Some(value) = self.completions(&input.text).first() {
                    input.set(value.clone());
                }
            }
            _ => input.key(key),
        }
        if matches!(mode, InputMode::Filter) {
            self.filter.clone_from(&input.text);
            self.row = 0;
        }
        self.input = Some((mode, input));
    }
    pub fn paste(&mut self, text: &str) {
        match &mut self.overlay {
            Some(Overlay::TaskForm(editor)) => {
                if !self.pim_pending {
                    editor.paste(text);
                }
            }
            Some(Overlay::PimForm {
                fields,
                field,
                contact,
                error,
                book_menu,
                ..
            }) if !self.pim_pending
                && *field > 0
                && *field <= fields.len()
                && book_menu.is_none() =>
            {
                if (!*contact && *field == 5) || (*contact && matches!(*field, 5 | 10 | 15)) {
                    let clean: String = crate::mime::terminal_text(text)
                        .chars()
                        .filter(|c| !c.is_control() || *c == '\n')
                        .collect();
                    let input = &mut fields[*field - 1];
                    if input.text.len() + clean.len() <= 65536 {
                        input.text.insert_str(input.cursor, &clean);
                        input.cursor += clean.len();
                    }
                } else {
                    fields[*field - 1].insert(text);
                }
                *error = None;
                return;
            }
            Some(Overlay::Assistant) => {
                self.ai.input.insert(text);
                return;
            }
            Some(Overlay::Attachments { query, row, .. }) => {
                query.insert(text);
                *row = 0;
                return;
            }
            Some(Overlay::SaveAttachments {
                directory, error, ..
            }) if !self.form_pending => {
                directory.insert(text);
                *error = None;
                return;
            }
            Some(Overlay::Find { query, row, .. }) => {
                query.insert(text);
                *row = 0;
                self.find_request();
                return;
            }
            Some(Overlay::Files { query, row, .. }) => {
                query.insert(text);
                *row = 0;
                return;
            }
            Some(Overlay::Move { query, row, .. }) => {
                query.insert(text);
                *row = 0;
                return;
            }
            Some(Overlay::CreateFolder { name, .. }) if !self.form_pending => {
                name.insert(text);
                return;
            }
            Some(Overlay::Compose { field, fields, .. }) if !self.form_pending && *field > 0 => {
                let input = &mut fields[*field - 1];
                if *field == 3 {
                    let clean: String = crate::mime::terminal_text(text)
                        .chars()
                        .filter(|c| !c.is_control() || *c == '\n')
                        .collect();
                    if input.text.len() + clean.len() <= 65536 {
                        input.text.insert_str(input.cursor, &clean);
                        input.cursor += clean.len();
                    }
                } else {
                    input.insert(text);
                }
                return;
            }
            Some(_) => return,
            None => {}
        }
        if let Some((mode, input)) = &mut self.input {
            input.insert(text);
            if matches!(mode, InputMode::Filter) {
                self.filter.clone_from(&input.text);
                self.row = 0;
            }
        }
    }
    pub fn completions(&self, prefix: &str) -> Vec<String> {
        if let Some(prefix) = prefix.strip_prefix("ai ") {
            return [
                "new", "global", "login", "logout", "status", "stop", "models",
            ]
            .into_iter()
            .filter(|s| s.starts_with(prefix))
            .map(|s| format!("ai {s}"))
            .collect();
        }
        for alias in ["account", "accounts", "ns"] {
            if let Some(name) = prefix.strip_prefix(&format!("{alias} ")) {
                let mut rows = vec![];
                if "all".starts_with(name) {
                    rows.push(format!("{alias} all"));
                }
                rows.extend(
                    self.accounts
                        .iter()
                        .filter(|a| a.id.starts_with(name))
                        .map(|a| format!("{alias} {}", shell_words::quote(&a.id))),
                );
                return rows;
            }
        }
        for alias in ["folder", "directory", "dir"] {
            if let Some(name) = prefix.strip_prefix(&format!("{alias} ")) {
                return self
                    .folders
                    .iter()
                    .filter(|f| {
                        Some(&f.account) == self.scope.account.as_ref() && f.id.starts_with(name)
                    })
                    .map(|f| format!("{alias} {}", shell_words::quote(&f.id)))
                    .collect();
            }
        }
        command::COMMANDS
            .iter()
            .filter(|s| s.starts_with(prefix))
            .map(|s| s.to_string())
            .collect()
    }
    pub fn execute(&mut self, text: &str) {
        let command = match command::parse(text) {
            Ok(c) => c,
            Err(e) => {
                self.notify(e.to_string());
                return;
            }
        };
        match command {
            Command::Ai(command) => self.ai_command(&command),
            Command::Quit => self.request_quit(),
            Command::More => self.load_more(),
            Command::Sync => self.request_sync(),
            Command::Reload => {
                if self.view.is_pim() {
                    self.reload_pim();
                    return;
                }
                self.revision += 1;
                if self.view == View::Detail {
                    let generation = self.generation();
                    self.parsed = None;
                    if let Some(id) = self.detail.as_ref().map(|m| m.id.clone()) {
                        self.send(Request::Detail { generation, id });
                    }
                } else {
                    self.reload(false);
                }
            }
            Command::Account(id) => {
                if id == "all" {
                    self.switch_account(None);
                } else if self.accounts.iter().any(|a| a.id == id) {
                    self.switch_account(Some(id));
                } else {
                    self.notify("Unknown account; use :ns");
                }
            }
            Command::Folder(path) => {
                if let Some(key) = self
                    .folders
                    .iter()
                    .find(|f| f.id == path && Some(&f.account) == self.scope.account.as_ref())
                    .map(tree::folder_key)
                {
                    self.show_folder(key);
                } else {
                    self.notify(
                        "Choose an account first, then an exact folder ID (:ns, :dir <path>)",
                    );
                }
            }
            Command::Search(query) => {
                if let Err(e) = crate::search::Query::parse(&query) {
                    self.notify(e.to_string());
                    return;
                }
                self.navigate(View::Messages);
                self.start_search(query);
            }
            Command::Sort(field, desc) => {
                let focus = self.focused_key();
                self.sort = field;
                self.descending = desc;
                self.restore_focus(focus);
            }
            Command::Theme(theme) => self.theme = theme,
            Command::View(name) => match name {
                "accounts" => {
                    self.navigate(View::Accounts);
                    self.row = self
                        .account_rows()
                        .iter()
                        .position(|i| {
                            i.map(|i| &self.accounts[i].id) == self.scope.account.as_ref()
                        })
                        .unwrap_or(0);
                }
                "folders" => {
                    if self.view != View::Messages {
                        self.navigate(View::Messages);
                    }
                    if let Some(key) = self.focused_folder() {
                        self.focus_key(&RowKey::Folder(key));
                    }
                    self.notify(
                        "Messages tree · Enter folds/unfolds · Tab jumps to next branch · h/H HTML",
                    );
                }
                "inbox" | "unified" => {
                    if self.view != View::Messages || self.searching() {
                        self.navigate(View::Messages);
                    }
                    self.search.clear();
                    self.filter.clear();
                    self.unread = false;
                    self.flagged = false;
                    if name == "unified" {
                        self.scope.account = None;
                        self.messages.clear();
                        self.pages.clear();
                    }
                    let inboxes: Vec<_> = self
                        .folders
                        .iter()
                        .filter(|f| {
                            is_inbox(f)
                                && self.scope.account.as_ref().is_none_or(|a| a == &f.account)
                        })
                        .map(tree::folder_key)
                        .collect();
                    for key in &inboxes {
                        self.expand_ancestors(key);
                    }
                    self.requested_focus = inboxes.first().cloned().map(RowKey::Folder);
                    self.reload(false);
                }
                "mail" => {
                    if self.view != View::Messages {
                        self.navigate(View::Messages);
                    }
                    if self.searching() {
                        self.search.clear();
                        self.filter.clear();
                        self.reload(false);
                    }
                }
                "unread" => {
                    self.unread = !self.unread;
                    self.row = 0;
                    self.notify(format!("Unread filter: {}", self.unread));
                }
                "flagged" => {
                    self.flagged = !self.flagged;
                    self.row = 0;
                    self.notify(format!("Flagged filter: {}", self.flagged));
                }
                "outbox" => self.navigate(View::Pending),
                "calendar" => self.action(Action::ViewSlot(1)),
                "tasks" => self.action(Action::ViewSlot(3)),
                "contacts" => self.action(Action::ViewSlot(2)),
                "compose" => self.action(Action::Compose),
                "mkdir" => self.action(Action::CreateFolder),
                "status" => self.navigate(View::Status),
                "help" => self.action(Action::Help),
                "history" => self.navigate(View::History),
                _ => {}
            },
        }
    }

    pub fn event(&mut self, event: Event) {
        if let Event::Pim {
            generation,
            result,
            write,
        } = event
        {
            if generation != self.pim_generation {
                return;
            }
            self.pim_loading = false;
            self.pim_pending = false;
            let sync_target = self.pim_write_sync.take();
            let write_focus = self.pim_write_focus.take();
            if write
                && self
                    .pim_ai_request
                    .take()
                    .is_some_and(|(request, run)| request == generation && run == self.ai.run)
            {
                let message=match &result {Ok(_)=>"Calendar/contact operation applied locally. The configured synchronization command, if any, will run next.".into(),Err(error)=>format!("Calendar/contact operation failed: {error}")};
                self.ai.status = if result.is_ok() {
                    "PIM operation applied locally".into()
                } else {
                    "PIM operation failed · see conversation".into()
                };
                self.ai.history.push(crate::llm::MessageText {
                    role: "assistant".into(),
                    content: crate::mime::terminal_text(&message),
                });
            }
            match result {
                Ok(snapshot) => {
                    let mut focus = self.pim_focused_key();
                    self.calendar_events = snapshot.events;
                    self.contacts = snapshot.contacts;
                    self.tasks = snapshot.tasks;
                    self.upcoming_events = snapshot.upcoming_events;
                    self.event_occurrences = snapshot.event_occurrences;
                    if let Some((view, mut collection, uid)) =
                        write_focus.filter(|(view, _, _)| *view == self.view)
                    {
                        let file = if view == View::Calendar {
                            self.expanded_calendars.insert(collection.clone());
                            self.calendar_events
                                .iter()
                                .find(|e| e.collection == collection && e.uid == uid)
                                .map(|e| e.file.clone())
                        } else if view == View::Contacts {
                            let file = self
                                .contacts
                                .iter()
                                .find(|c| c.book == collection && c.uid == uid && !c.is_mail())
                                .map(|c| c.file.clone());
                            collection = "carddav-contacts".into();
                            self.expanded_contact_categories.insert(collection.clone());
                            file
                        } else {
                            self.expanded_task_lists.insert(collection.clone());
                            self.tasks
                                .iter()
                                .find(|t| t.collection == collection && t.uid == uid)
                                .map(|t| t.file.clone())
                        };
                        focus = Some(match file {
                            Some(file) => PimRowKey::Entry { collection, file },
                            None => PimRowKey::Collection(collection),
                        });
                    }
                    if self.view.is_pim() {
                        self.restore_pim_focus(focus);
                    }
                    if write {
                        if matches!(
                            self.overlay,
                            Some(Overlay::PimForm { .. } | Overlay::TaskForm(_))
                        ) {
                            self.overlay = None;
                        }
                        if let Some(target) = sync_target.filter(|t| self.sync_enabled.contains(t))
                        {
                            self.sync_requests.insert(target);
                            self.notify(format!(
                                "Saved locally · synchronizing {}…",
                                target.name()
                            ));
                        } else {
                            self.notify("PIM entry saved/deleted locally");
                        }
                    }
                    if snapshot.warnings != self.pim_warnings && !snapshot.warnings.is_empty() {
                        self.notify(format!("PIM: {}", snapshot.warnings.join(" · ")));
                    }
                    self.pim_warnings = snapshot.warnings;
                }
                Err(reason) => {
                    if let Some(Overlay::PimForm { error, .. }) = &mut self.overlay {
                        *error = Some(reason.clone());
                    }
                    if let Some(Overlay::TaskForm(editor)) = &mut self.overlay {
                        editor.error = Some(reason.clone());
                    }
                    self.notify(format!("PIM: {reason}"));
                }
            }
            self.refresh_pim_if_idle();
            return;
        }
        if let Event::SavedAttachments { generation, result } = event {
            let current = generation == self.generation;
            if current {
                self.busy = false;
                self.form_pending = false;
            }
            match result {
                Ok(paths) => {
                    if current && matches!(self.overlay, Some(Overlay::SaveAttachments { .. })) {
                        self.overlay = None;
                    }
                    self.notify(format!(
                        "Saved {} attachment(s) → {}",
                        paths.len(),
                        paths
                            .first()
                            .and_then(|path| path.parent())
                            .map(|path| path.display().to_string())
                            .unwrap_or_default()
                    ));
                }
                Err(reason) => {
                    if current
                        && let Some(Overlay::SaveAttachments { error, .. }) = &mut self.overlay
                    {
                        *error = Some(reason);
                    } else {
                        self.notify(format!("Save attachments: {reason}"));
                    }
                }
            }
            return;
        }
        let event = match event {
            Event::Pending { result, .. } => {
                if let Ok(pending) = result {
                    self.pending = pending;
                    self.send_errors.retain(|account, _| {
                        self.pending.iter().any(|mail| &mail.account == account)
                    });
                }
                return;
            }
            Event::Submitted {
                generation,
                account,
                result,
                pending,
            } => {
                if matches!(&result, MutationResult::Queued(_)) {
                    for mail in pending {
                        if !self.pending.iter().any(|existing| {
                            existing.account == mail.account && existing.id == mail.id
                        }) {
                            self.pending.push(mail);
                        }
                    }
                    self.send_errors.remove(&account);
                    self.send_requests.insert(account);
                }
                Event::Mutation { generation, result }
            }
            event => event,
        };
        if event.generation() != self.generation {
            // A confirmed write still completes after navigation, but must not
            // close or cancel a newer form/request when its receipt arrives.
            if let Event::Mutation { result, .. } = event {
                match result {
                    MutationResult::AppliedLocal => {
                        self.storage_changed();
                        self.notify("Previous operation applied locally");
                    }
                    MutationResult::Queued(message) => {
                        self.storage_changed();
                        self.notify(message);
                    }
                    MutationResult::Unsupported(reason) => self.notify(reason),
                }
            }
            return;
        }
        let focus = self.focused_key();
        self.busy = false;
        match event {
            Event::Pending { .. }
            | Event::Submitted { .. }
            | Event::SavedAttachments { .. }
            | Event::Pim { .. } => {
                unreachable!("receipts handled above")
            }
            Event::Found {
                query: sent,
                messages,
                ..
            } => {
                let mut retry = false;
                if let Some(Overlay::Find {
                    query,
                    messages: shown,
                    loading,
                    again,
                    error,
                    row,
                    ..
                }) = &mut self.overlay
                {
                    *loading = false;
                    if query.text != sent || *again {
                        *again = false;
                        retry = true;
                    } else {
                        let selected = shown.get(*row).map(|message| message.id.clone());
                        match messages {
                            Ok(messages) => {
                                *shown = messages;
                                *row = selected
                                    .and_then(|id| {
                                        shown.iter().position(|message| message.id == id)
                                    })
                                    .unwrap_or(0);
                                *error = None;
                            }
                            Err(message) => {
                                shown.clear();
                                *error = Some(message);
                            }
                        }
                    }
                }
                if retry {
                    self.find_request();
                }
            }
            Event::Files { path, entries, .. } => {
                if let Some(Overlay::Files {
                    path: current,
                    entries: shown,
                    loading,
                    error,
                    ..
                }) = &mut self.overlay
                {
                    if *current == path {
                        *loading = false;
                        match entries {
                            Ok(entries) => *shown = entries,
                            Err(message) => *error = Some(message),
                        }
                    }
                    self.busy = *loading;
                }
            }
            Event::Loaded {
                accounts,
                folders,
                pages,
                pending,
                ..
            } => {
                let initial = !self.initialized;
                if initial {
                    self.collapsed_accounts
                        .extend(accounts.iter().map(|account| account.id.clone()));
                }
                self.accounts = accounts;
                self.folders = folders;
                self.pending = pending;
                self.messages.clear();
                self.pages.clear();
                self.root_loading = false;
                self.initialized = true;
                for (key, page) in pages {
                    if initial {
                        self.expand_ancestors(&key);
                    }
                    self.accept_page(key, page, false);
                }
                // Branches can be opened while the metadata refresh is in flight.
                // Their deferred first pages must start when that refresh completes.
                if self.view == View::Messages {
                    self.load_open_rows();
                }
                let requested = self.requested_focus.take().or(focus);
                self.restore_focus(requested);
                if self.demo {
                    self.notify("DEMO · Enter folds/unfolds accounts and folders · h/H HTML · Ctrl+C twice quits");
                } else if initial {
                    self.toast.clear();
                }
            }
            Event::Folder {
                key, append, page, ..
            } => {
                self.accept_page(key, page, append);
                self.restore_focus(focus);
                self.busy = self.pages.values().any(|p| p.loading);
            }
            Event::Detail { parsed, .. } => {
                self.parsed = Some(parsed);
                if self.html_on_load {
                    self.html_on_load = false;
                    self.want_html = true;
                }
            }
            Event::Search {
                messages,
                accounts,
                folders,
                ..
            } => {
                self.accounts = accounts;
                self.folders = folders;
                self.initialized = true;
                self.messages = messages;
                let keys: HashSet<_> = self
                    .messages
                    .iter()
                    .map(|m| (m.id.account.clone(), m.id.folder.clone()))
                    .collect();
                for key in keys {
                    self.expand_ancestors(&key);
                }
                self.row = 0;
                self.root_loading = false;
                self.notify("Search results in the Messages tree · up to 200 hits · Esc returns to browsing");
            }
            Event::Progress { count, .. } => {
                self.busy = true;
                self.notify(format!(
                    "Indexing local mail… {count} messages · Esc cancels"
                ));
            }
            Event::Mutation { result, .. } => match result {
                MutationResult::AppliedLocal | MutationResult::Queued(_) => {
                    self.index_dirty = true;
                    let notification = match result {
                        MutationResult::Queued(message) => Some(message),
                        _ => Some("Applied locally (demo)".into()),
                    };
                    if self.form_pending {
                        self.overlay = None;
                    }
                    self.form_pending = false;
                    self.revision += 1;
                    self.selected.clear();
                    self.last_mark = None;
                    if self.view == View::Detail {
                        self.back();
                    } else {
                        self.reload(false);
                    }
                    if let Some(message) = notification {
                        self.notify(message);
                    }
                }
                MutationResult::Unsupported(reason) => {
                    self.form_pending = false;
                    self.notify(reason);
                }
            },
            Event::Error { error, .. } => {
                self.form_pending = false;
                self.root_loading = false;
                self.notify(error);
            }
        }
    }
}

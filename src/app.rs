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
    Accounts,
    Detail,
    Pending,
    Status,
    Help,
    History,
}
impl View {
    pub fn name(self) -> &'static str {
        match self {
            Self::Messages => "Messages",
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
                        | Request::CreateFolder { .. }
                        | Request::Compose { .. }
                        | Request::Pending { .. }
                        | Request::SaveAttachments { .. }
                )
            {
                continue;
            }
            let result: anyhow::Result<Event> = (|| match command {
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
        if !["", "new", "global", "login", "logout", "status", "stop"].contains(&command) {
            self.notify("Usage: :ai [new|global|login|logout|status|stop]");
            return;
        }
        self.open_ai(command == "new" || command == "global", command == "global");
        if !self.llm.enabled {
            return;
        }
        match command {
            "stop" => self.ai_stop(),
            "login" | "logout" | "status" => {
                if self.llm.provider != Provider::Codex {
                    self.ai.status = format!(
                        "{} · API key from {}",
                        self.llm.label(),
                        self.llm.auth.api_key_env
                    );
                    return;
                }
                self.ai.run += 1;
                self.ai.busy = true;
                self.ai.status = "Connecting to Codex…".into();
                self.ai.commands.push_back(match command {
                    "login" => C::Login(self.ai.run),
                    "logout" => C::Logout(self.ai.run),
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
                {
                    self.ai.status = "Done".into();
                }
            }
            Update::Error(error) => {
                self.ai.busy = false;
                self.ai.status = crate::mime::terminal_text(&format!("Error: {error}"));
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
                "new" | "global" | "login" | "logout" | "status" | "stop" => self.ai_command(name),
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
                    "Commands: /new /global /stop /login /logout /status /sources /open N /apply N"
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
        if self.selected.is_empty() {
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
    fn count(&self) -> usize {
        match self.view {
            View::Messages => self.tree_rows().len(),
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
    pub fn pending_label(&self) -> String {
        let failed = self
            .pending
            .iter()
            .filter(|mail| mail.error.is_some() || self.send_errors.contains_key(&mail.account))
            .count();
        let sending = self.pending.len() - failed;
        match (sending, failed) {
            (0, 0) => "Pending: 0".into(),
            (_, 0) => format!("Pending: {sending} sending"),
            (0, _) => format!("Pending: {failed} failed"),
            _ => format!("Pending: {sending} sending, {failed} failed"),
        }
    }
    pub fn index_changed(&mut self) {
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
        if matches!(self.view, View::Messages | View::Accounts) {
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
                (Overlay::Confirm { .. } | Overlay::Help, KeyCode::Down) => {
                    self.help_scroll = self.help_scroll.saturating_add(1).min(65_535)
                }
                (Overlay::Confirm { .. } | Overlay::Help, KeyCode::Up) => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                (Overlay::Confirm { .. } | Overlay::Help, KeyCode::PageDown) => {
                    self.help_scroll = self
                        .help_scroll
                        .saturating_add(self.page_height.max(1))
                        .min(65_535)
                }
                (Overlay::Confirm { .. } | Overlay::Help, KeyCode::PageUp) => {
                    self.help_scroll = self.help_scroll.saturating_sub(self.page_height.max(1))
                }
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
            AccountSlot(slot) => {
                if slot == 0 {
                    self.switch_account(None);
                } else if let Some(id) = self.accounts.get(slot - 1).map(|a| a.id.clone()) {
                    self.switch_account(Some(id));
                } else {
                    self.notify(format!("No account assigned to <{slot}>; use :ns"));
                }
            }
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
            Compose => {
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
            return ["new", "global", "login", "logout", "status", "stop"]
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
            Command::Reload => {
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
            Event::Pending { .. } | Event::Submitted { .. } | Event::SavedAttachments { .. } => {
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

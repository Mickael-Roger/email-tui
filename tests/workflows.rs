use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mailtui::{
    app::{Action, App, Event, Overlay, View, start_worker},
    domain::*,
    storage::{DemoStore, MailStore},
    tree::{Node, RowKey},
};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

type Events = tokio::sync::mpsc::Receiver<Event>;
async fn settle(app: &mut App, events: &mut Events) {
    while app.busy {
        app.event(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
}
async fn fixture() -> (App, Events, Arc<AtomicU64>, tokio::task::JoinHandle<()>) {
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut rx, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "k9s-dark".into(), current.clone(), tx);
    app.reload(false);
    settle(&mut app, &mut rx).await;
    assert!(app.expanded.is_empty(), "Directories start folded");
    assert_eq!(app.collapsed_accounts.len(), 2, "Accounts start folded too");
    assert!(
        app.messages.is_empty(),
        "No metadata page is read before expansion"
    );
    app.execute("unified");
    settle(&mut app, &mut rx).await;
    app.focus_key(&RowKey::Account("work".into()));
    (app, rx, current, worker)
}
async fn finish(
    app: App,
    rx: Events,
    current: Arc<AtomicU64>,
    worker: tokio::task::JoinHandle<()>,
) {
    current.fetch_add(1, Ordering::Relaxed);
    drop(app);
    drop(rx);
    worker.await.unwrap();
}
fn key(app: &mut App, c: char) {
    app.key(KeyCode::Char(c).into());
}
fn ctrl(app: &mut App, c: char) {
    app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}
fn command(app: &mut App, s: &str) {
    key(app, ':');
    app.paste(s);
    app.key(KeyCode::Enter.into());
}
fn folder_key(account: &str, folder: &str) -> (String, String) {
    (account.into(), folder.into())
}
fn focus_folder(app: &mut App, account: &str, folder: &str) {
    assert!(
        app.focus_key(&RowKey::Folder(folder_key(account, folder))),
        "{account}/{folder} not visible"
    );
}
fn first_mail(app: &App, account: &str, folder: &str) -> MessageId {
    app.visible_message_indices()
        .iter()
        .map(|i| &app.messages[*i])
        .find(|m| m.id.account == account && m.id.folder == folder)
        .unwrap()
        .id
        .clone()
}
fn focus_mail(app: &mut App, id: &MessageId) {
    assert!(app.focus_key(&RowKey::Message(id.clone())));
}
async fn expand(app: &mut App, events: &mut Events, account: &str, folder: &str) {
    app.collapsed_accounts.remove(account);
    focus_folder(app, account, folder);
    if !app.expanded.contains(&folder_key(account, folder)) {
        app.action(Action::Expand);
    }
    settle(app, events).await;
}

#[tokio::test]
async fn toggle_read_status_handles_single_mixed_selection_and_open_message() {
    let (mut app, mut rx, current, worker) = fixture().await;
    let unread = app.messages.iter().find(|m| !m.seen).unwrap().id.clone();
    let read = app.messages.iter().find(|m| m.seen).unwrap().id.clone();
    focus_mail(&mut app, &unread);
    key(&mut app, 't');
    settle(&mut app, &mut rx).await;
    assert!(app.messages.iter().find(|m| m.id == unread).unwrap().seen);
    key(&mut app, 't');
    settle(&mut app, &mut rx).await;
    assert!(!app.messages.iter().find(|m| m.id == unread).unwrap().seen);

    app.selected.extend([unread.clone(), read.clone()]);
    focus_folder(&mut app, "work", "INBOX");
    key(&mut app, 't');
    settle(&mut app, &mut rx).await;
    for id in [&unread, &read] {
        assert!(app.messages.iter().find(|m| &m.id == id).unwrap().seen);
    }
    app.selected.extend([unread.clone(), read.clone()]);
    key(&mut app, 't');
    settle(&mut app, &mut rx).await;
    for id in [&unread, &read] {
        assert!(!app.messages.iter().find(|m| &m.id == id).unwrap().seen);
    }

    app.selected.insert(read.clone());
    focus_mail(&mut app, &unread);
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    assert_eq!(app.view, View::Detail);
    assert_eq!(app.targets(), vec![unread.clone()]);
    key(&mut app, 't');
    settle(&mut app, &mut rx).await;
    assert!(app.messages.iter().find(|m| m.id == unread).unwrap().seen);
    assert!(!app.messages.iter().find(|m| m.id == read).unwrap().seen);
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn one_list_nests_messages_below_their_folder_and_accounts_only_in_all_scope() {
    let (mut app, mut rx, current, worker) = fixture().await;
    assert_eq!(app.view, View::Messages);
    assert_eq!(app.messages.len(), 110);
    let rows = app.tree_rows();
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(r.node, Node::Account(_)))
            .count(),
        2
    );
    for (position, row) in rows.iter().enumerate() {
        if let Node::Message(i) = row.node {
            let parent = rows[..position]
                .iter()
                .rev()
                .find(|p| p.depth < row.depth)
                .unwrap();
            let Node::Folder(f) = parent.node else {
                panic!("Mail must have a folder parent");
            };
            assert_eq!(
                (&app.messages[i].id.account, &app.messages[i].id.folder),
                (&app.folders[f].account, &app.folders[f].id)
            );
            assert_eq!(row.depth, 2);
        }
    }
    command(&mut app, "ns work");
    settle(&mut app, &mut rx).await;
    assert_eq!(app.account_name(), "work");
    assert_eq!(app.messages.len(), 55);
    assert!(
        app.tree_rows()
            .iter()
            .all(|r| !matches!(r.node, Node::Account(_)))
    );
    assert!(
        app.tree_rows()
            .iter()
            .filter(|r| matches!(r.node, Node::Message(_)))
            .all(|r| r.depth == 1)
    );
    key(&mut app, '0');
    settle(&mut app, &mut rx).await;
    assert_eq!(
        app.tree_rows()
            .iter()
            .filter(|r| matches!(r.node, Node::Account(_)))
            .count(),
        2
    );
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn several_folders_unfold_independently_and_refold_without_losing_cached_mail() {
    let (mut app, mut rx, current, worker) = fixture().await;
    expand(&mut app, &mut rx, "work", "Projects").await;
    expand(&mut app, &mut rx, "work", "Projects/Rust").await;
    expand(&mut app, &mut rx, "work", "Projects/Rust/MailTUI").await;
    expand(&mut app, &mut rx, "work", "Archive").await;
    assert_eq!(app.messages.len(), 127);
    assert_eq!(app.view, View::Messages);
    assert_eq!(app.account_name(), "all");
    for path in ["INBOX", "Projects/Rust/MailTUI", "Archive"] {
        assert!(
            app.visible_message_indices().iter().any(
                |i| app.messages[*i].id.account == "work" && app.messages[*i].id.folder == path
            )
        );
    }
    focus_folder(&mut app, "work", "Projects");
    app.action(Action::Collapse);
    assert_eq!(app.messages.len(), 127);
    assert!(
        !app.visible_message_indices()
            .iter()
            .any(|i| app.messages[*i].id.folder == "Projects/Rust/MailTUI")
    );
    app.action(Action::Expand);
    assert!(!app.busy);
    assert!(
        app.visible_message_indices()
            .iter()
            .any(|i| app.messages[*i].id.folder == "Projects/Rust/MailTUI")
    );
    let id = first_mail(&app, "work", "Projects/Rust/MailTUI");
    focus_mail(&mut app, &id);
    app.action(Action::Collapse);
    assert_eq!(
        app.focused_key(),
        Some(RowKey::Folder(folder_key("work", "Projects/Rust/MailTUI")))
    );
    app.action(Action::Collapse);
    assert!(
        !app.expanded
            .contains(&folder_key("work", "Projects/Rust/MailTUI"))
    );
    expand(&mut app, &mut rx, "work", "Projects/Rust/MailTUI").await;
    focus_folder(&mut app, "work", "INBOX");
    app.action(Action::Collapse);
    focus_folder(&mut app, "personal", "INBOX");
    app.action(Action::Collapse);
    insta::assert_snapshot!("tree_multiple_open_folders", capture(&mut app, 120, 44));
    command(&mut app, "ns work");
    settle(&mut app, &mut rx).await;
    insta::assert_snapshot!("tree_single_account", capture(&mut app, 120, 40));
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn account_branches_fold_without_switching_namespace_and_arrows_move_rows() {
    let (mut app, rx, current, worker) = fixture().await;
    assert_eq!(app.focused_key(), Some(RowKey::Account("work".into())));
    app.key(KeyCode::Down.into());
    assert_eq!(
        app.focused_key(),
        Some(RowKey::Folder(folder_key("work", "INBOX")))
    );
    app.key(KeyCode::Up.into());
    assert_eq!(app.focused_key(), Some(RowKey::Account("work".into())));
    assert!(app.focus_key(&RowKey::Account("work".into())));
    app.key(KeyCode::Enter.into());
    assert_eq!(app.account_name(), "all");
    assert_eq!(app.visible_message_indices().len(), 55);
    assert!(
        app.visible_message_indices()
            .iter()
            .all(|i| app.messages[*i].id.account == "personal")
    );
    app.key(KeyCode::Enter.into());
    assert_eq!(app.visible_message_indices().len(), 110);
    assert!(!app.busy);
    app.action(Action::CollapseAll);
    assert_eq!(app.tree_rows().len(), 2);
    assert!(app.visible_message_indices().is_empty());
    insta::assert_snapshot!("tree_all_accounts_collapsed", capture(&mut app, 80, 24));
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn folder_aliases_and_search_stay_in_messages_with_ancestor_context() {
    let (mut app, mut rx, current, worker) = fixture().await;
    for alias in ["folders", "directories", "dirs", "dir"] {
        command(&mut app, alias);
        assert_eq!(app.view, View::Messages);
    }
    command(&mut app, "ns work");
    settle(&mut app, &mut rx).await;
    command(&mut app, "dir Projects/Rust/MailTUI");
    settle(&mut app, &mut rx).await;
    assert_eq!(app.view, View::Messages);
    assert_eq!(
        app.focused_key(),
        Some(RowKey::Folder(folder_key("work", "Projects/Rust/MailTUI")))
    );
    assert!(app.expanded.contains(&folder_key("work", "Projects")));
    let normal_key = app.focused_key();
    let normal_messages = app.messages.len();
    command(&mut app, "search kubernetes");
    settle(&mut app, &mut rx).await;
    assert_eq!(app.view, View::Messages);
    assert!(app.searching());
    assert!(!app.messages.is_empty());
    assert!(app.messages.iter().all(|m| m.id.account == "work"));
    assert_eq!(app.visible_message_indices().len(), app.messages.len());
    command(&mut app, "ns personal");
    settle(&mut app, &mut rx).await;
    assert_eq!(app.view, View::Messages);
    assert!(app.messages.iter().all(|m| m.id.account == "personal"));
    key(&mut app, '0');
    settle(&mut app, &mut rx).await;
    assert!(app.messages.iter().any(|m| m.id.account == "work"));
    assert!(app.messages.iter().any(|m| m.id.account == "personal"));
    app.key(KeyCode::Esc.into());
    settle(&mut app, &mut rx).await;
    assert!(!app.searching());
    assert_eq!(app.account_name(), "all");
    assert!(app.messages.len() >= normal_messages);
    assert!(normal_key.is_some());
    command(&mut app, "ns work");
    settle(&mut app, &mut rx).await;
    command(&mut app, "search account:personal kubernetes");
    settle(&mut app, &mut rx).await;
    assert!(app.messages.is_empty());
    assert!(app.tree_rows().is_empty());
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn filtering_retains_ancestors_and_bulk_delete_targets_only_marked_mail_even_when_folded() {
    let (mut app, mut rx, current, worker) = fixture().await;
    let work = first_mail(&app, "work", "INBOX");
    let personal = first_mail(&app, "personal", "INBOX");
    ctrl(&mut app, 'd');
    assert!(app.overlay.is_none()); // Initial account row is never an email.
    focus_mail(&mut app, &work);
    key(&mut app, ' ');
    focus_mail(&mut app, &personal);
    key(&mut app, ' ');
    let marked = app.selected.clone();
    app.action(Action::Filter);
    app.paste("incident");
    assert!(!app.visible_message_indices().is_empty());
    for i in app.visible_message_indices() {
        assert!(app.messages[i].subject.to_lowercase().contains("incident"));
    }
    assert!(
        app.tree_rows()
            .iter()
            .any(|r| matches!(r.node, Node::Folder(_)))
    );
    key(&mut app, 'x'); // Text editing must not trigger namespace/navigation actions.
    key(&mut app, '0');
    assert!(app.overlay.is_none());
    assert_eq!(app.account_name(), "all");
    app.key(KeyCode::Esc.into());
    assert!(app.filter.is_empty());
    assert_eq!(app.selected, marked);
    app.action(Action::CollapseAll);
    assert!(app.visible_message_indices().is_empty());
    assert_eq!(app.selected, marked);
    ctrl(&mut app, 'd');
    let Some(Overlay::Confirm { ids, confirm }) = &app.overlay else {
        panic!("No confirmation");
    };
    assert!(!confirm);
    assert_eq!(ids.iter().cloned().collect::<HashSet<_>>(), marked);
    insta::assert_snapshot!("delete_confirmation_cancel", capture(&mut app, 80, 24));
    app.key(KeyCode::Enter.into());
    assert!(app.overlay.is_none());
    assert_eq!(app.messages.len(), 110);
    ctrl(&mut app, 'd');
    app.key(KeyCode::Tab.into());
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Confirm { confirm: true, .. })
    ));
    insta::assert_snapshot!("delete_confirmation_accept", capture(&mut app, 80, 24));
    app.key(KeyCode::Esc.into());
    assert_eq!(app.selected, marked);
    ctrl(&mut app, 'd');
    app.key(KeyCode::Tab.into());
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    assert!(app.selected.is_empty());
    command(&mut app, "unified");
    settle(&mut app, &mut rx).await;
    assert_eq!(app.messages.len(), 108);
    assert!(app.messages.iter().all(|m| !marked.contains(&m.id)));
    expand(&mut app, &mut rx, "work", "Trash").await;
    assert!(
        app.messages
            .iter()
            .any(|m| m.id.account == "work" && m.id.folder == "Trash" && m.id.seq == work.seq)
    );
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn range_marks_skip_headers_and_detail_help_return_preserves_tree_position() {
    let (mut app, mut rx, current, worker) = fixture().await;
    let work_last = app
        .visible_message_indices()
        .iter()
        .map(|i| &app.messages[*i])
        .rfind(|m| m.id.account == "work")
        .unwrap()
        .id
        .clone();
    let personal = first_mail(&app, "personal", "INBOX");
    focus_mail(&mut app, &work_last);
    key(&mut app, ' ');
    focus_mail(&mut app, &personal);
    app.action(Action::SelectRange);
    assert_eq!(app.selected.len(), 2);
    assert!(app.selected.contains(&work_last));
    assert!(app.selected.contains(&personal));
    app.action(Action::ClearSelection);
    assert!(app.selected.is_empty());
    let position = app.row;
    let id = app.focused().unwrap().id.clone();
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    assert_eq!(app.view, View::Detail);
    assert!(app.parsed.is_some());
    app.key(KeyCode::Down.into());
    let scroll = app.body_scroll;
    key(&mut app, '?');
    app.key(KeyCode::Down.into());
    app.key(KeyCode::Esc.into());
    assert_eq!(app.body_scroll, scroll);
    app.key(KeyCode::Esc.into());
    assert_eq!(app.view, View::Messages);
    assert_eq!(app.row, position);
    assert_eq!(app.focused().unwrap().id, id);
    command(&mut app, "ns");
    assert_eq!(app.view, View::Accounts);
    assert_eq!(app.account_rows()[0], None);
    app.key(KeyCode::Down.into());
    key(&mut app, 'u');
    settle(&mut app, &mut rx).await;
    assert_eq!(app.view, View::Messages);
    assert_eq!(app.account_name(), "work");
    ctrl(&mut app, 'r');
    settle(&mut app, &mut rx).await;
    assert_eq!(app.account_name(), "work");
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn stale_events_cannot_replace_new_scope_and_search_can_cancel_initial_load() {
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut rx, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "no-color".into(), current.clone(), tx);
    app.reload(false);
    command(&mut app, "search incident");
    settle(&mut app, &mut rx).await;
    assert!(app.searching());
    assert!(!app.messages.is_empty());
    assert!(!app.tree_rows().is_empty());
    app.key(KeyCode::Esc.into());
    settle(&mut app, &mut rx).await;
    assert!(!app.searching());
    assert!(app.messages.is_empty()); // Returning restores the initially folded tree.
    command(&mut app, "ns personal");
    settle(&mut app, &mut rx).await;
    let ids: Vec<_> = app.messages.iter().map(|m| m.id.clone()).collect();
    app.event(Event::Error {
        generation: 0,
        error: "stale".into(),
    });
    app.event(Event::Folder {
        generation: 0,
        key: folder_key("work", "ghost"),
        append: false,
        page: Ok(Page {
            messages: vec![],
            total: 0,
            next: None,
        }),
    });
    assert_eq!(
        app.messages
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert!(!app.pages.contains_key(&folder_key("work", "ghost")));
    key(&mut app, ':');
    app.paste("unfinished");
    ctrl(&mut app, 'c');
    assert!(!app.quit);
    ctrl(&mut app, 'c');
    assert!(app.quit);
    finish(app, rx, current, worker).await;
}

#[test]
fn pages_keep_identical_ids_in_two_accounts_and_demo_mutations_scoped() {
    let mut store = DemoStore::new();
    let mut ids = HashSet::new();
    let mut cursor = None;
    loop {
        let page = store
            .messages(&Scope::default(), cursor.as_ref(), 7)
            .unwrap();
        assert_eq!(page.total, 144);
        for m in page.messages {
            assert!(ids.insert(m.id));
        }
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 144);
    let id = MessageId {
        account: "work".into(),
        folder: "INBOX".into(),
        seq: 1,
    };
    assert_eq!(
        store
            .mutate(std::slice::from_ref(&id), &Mutation::Seen(true))
            .unwrap(),
        MutationResult::AppliedLocal
    );
    let page = store
        .messages(
            &Scope {
                inbox_only: true,
                ..Default::default()
            },
            None,
            200,
        )
        .unwrap();
    assert!(page.messages.iter().find(|m| m.id == id).unwrap().seen);
    assert!(
        !page
            .messages
            .iter()
            .find(|m| m.id.account == "personal" && m.id.seq == 1)
            .unwrap()
            .seen
    );
    assert!(
        store
            .mutate(&[id], &Mutation::Move("not-a-folder".into()))
            .is_err()
    );
}

#[test]
fn active_keymap_has_no_conflicts_and_uses_numbers_for_resource_views() {
    mailtui::app::validate_bindings().unwrap();
    assert_eq!(
        mailtui::app::dispatch(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
        Some(Action::Delete)
    );
    assert_eq!(mailtui::app::dispatch(KeyCode::Char('d').into()), None);
    assert_eq!(
        mailtui::app::dispatch(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(
        mailtui::app::dispatch(KeyCode::Char('0').into()),
        Some(Action::ViewSlot(0))
    );
    assert_eq!(mailtui::app::dispatch(KeyCode::Tab.into()), None);
}

#[test]
fn ten_thousand_nested_folders_orphans_and_cycles_do_not_recurse_or_escape_folded_parent() {
    use mailtui::tree::TreeData;
    let accounts = vec![Account {
        id: "a".into(),
        label: "A".into(),
        email: "a@example.com".into(),
    }];
    let mut folders = vec![];
    let mut expanded = HashSet::new();
    for n in 0..10_000 {
        let id = n.to_string();
        expanded.insert(folder_key("a", &id));
        folders.push(Folder {
            account: "a".into(),
            id: id.clone(),
            name: id,
            parent: (n > 0).then(|| (n - 1).to_string()),
            role: None,
            total: 0,
            unread: 0,
        });
    }
    for (id, parent) in [
        ("orphan", "missing"),
        ("cycle1", "cycle2"),
        ("cycle2", "cycle1"),
    ] {
        expanded.insert(folder_key("a", id));
        folders.push(Folder {
            account: "a".into(),
            id: id.into(),
            name: id.into(),
            parent: Some(parent.into()),
            role: None,
            total: 0,
            unread: 0,
        });
    }
    let rows = |expanded: &HashSet<_>| {
        TreeData {
            accounts: &accounts,
            folders: &folders,
            messages: &[],
            visible_messages: &[],
            account: None,
            expanded,
            collapsed_accounts: &HashSet::new(),
            pages: &Default::default(),
            filter: "",
            searching: false,
            account_sort: None,
        }
        .rows()
    };
    assert_eq!(rows(&expanded).len(), 10_004);
    expanded.remove(&folder_key("a", "0"));
    assert_eq!(rows(&expanded).len(), 5);
}

struct PagedFixture {
    calls: Arc<AtomicUsize>,
    raw_reads: Arc<AtomicUsize>,
    inbox: usize,
}
impl MailStore for PagedFixture {
    fn accounts(&self) -> Vec<Account> {
        vec![Account {
            id: "large".into(),
            label: "Large".into(),
            email: "large@example.com".into(),
        }]
    }
    fn folders(&self) -> anyhow::Result<Vec<Folder>> {
        Ok([("INBOX", self.inbox), ("Archive", 450), ("Broken", 5)]
            .into_iter()
            .map(|(id, count)| Folder {
                account: "large".into(),
                id: id.into(),
                name: id.into(),
                parent: None,
                role: Some(if id == "INBOX" { "inbox" } else { "other" }.into()),
                total: count as u64,
                unread: count as u64,
            })
            .collect())
    }
    fn messages(
        &self,
        scope: &Scope,
        cursor: Option<&Cursor>,
        limit: usize,
    ) -> anyhow::Result<Page> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let folder = scope.folder.as_deref().unwrap();
        if folder == "Broken" {
            anyhow::bail!("fixture folder read failed");
        }
        let total = if folder == "INBOX" { self.inbox } else { 450 };
        let key = folder_key("large", folder);
        let before = cursor
            .and_then(|c| c.get(&key))
            .map(|(_, n)| *n)
            .unwrap_or(total as i64 + 1);
        let messages: Vec<_> = (1..before)
            .rev()
            .take(limit)
            .map(|seq| Message {
                id: MessageId {
                    account: "large".into(),
                    folder: folder.into(),
                    seq,
                },
                sender: "fixture@example.com".into(),
                subject: format!("Fixture {seq}"),
                date: "2026-10-18T09:14:00Z".into(),
                seen: false,
                flagged: false,
                attachment: false,
                size: 120,
            })
            .collect();
        let next = messages
            .last()
            .filter(|m| m.id.seq > 1)
            .map(|m| [(key, (m.date.clone(), m.id.seq))].into_iter().collect());
        Ok(Page {
            messages,
            total: total as u64,
            next,
        })
    }
    fn raw(&self, _: &MessageId) -> anyhow::Result<Vec<u8>> {
        self.raw_reads.fetch_add(1, Ordering::Relaxed);
        Ok(include_bytes!("fixtures/related.eml").to_vec())
    }
    fn pending(&self) -> anyhow::Result<Vec<mailtui::domain::PendingMail>> {
        Ok(vec![])
    }
    fn mutate(&mut self, _: &[MessageId], _: &Mutation) -> anyhow::Result<MutationResult> {
        Ok(MutationResult::Unsupported("fixture is read-only".into()))
    }
}

#[tokio::test]
async fn per_folder_pagination_is_lazy_deduplicated_and_preserves_focused_message() {
    let calls = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(AtomicUsize::new(0));
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut rx, worker) = start_worker(
        Box::new(PagedFixture {
            calls: calls.clone(),
            raw_reads: reads.clone(),
            inbox: 450,
        }),
        current.clone(),
    );
    let mut app = App::new(true, "k9s-dark".into(), current.clone(), tx);
    app.reload(false);
    settle(&mut app, &mut rx).await;
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    expand(&mut app, &mut rx, "large", "INBOX").await;
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(app.messages.len(), 200);
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    let id = first_mail(&app, "large", "INBOX");
    focus_folder(&mut app, "large", "Archive");
    app.action(Action::Expand);
    assert!(app.pages[&folder_key("large", "Archive")].loading);
    focus_mail(&mut app, &id);
    settle(&mut app, &mut rx).await;
    assert_eq!(app.focused().unwrap().id, id);
    assert_eq!(app.messages.len(), 400);
    for expected in [600, 650] {
        assert!(app.focus_key(&RowKey::More(folder_key("large", "INBOX"))));
        app.key(KeyCode::Enter.into());
        settle(&mut app, &mut rx).await;
        assert_eq!(app.messages.len(), expected);
    }
    assert!(app.pages[&folder_key("large", "INBOX")].next.is_none());
    assert_eq!(
        app.messages
            .iter()
            .map(|m| &m.id)
            .collect::<HashSet<_>>()
            .len(),
        650
    );
    assert_eq!(reads.load(Ordering::Relaxed), 0); // Unfolding never reads MIME.
    expand(&mut app, &mut rx, "large", "Broken").await;
    assert!(
        app.pages[&folder_key("large", "Broken")]
            .error
            .as_ref()
            .unwrap()
            .contains("fixture folder read failed")
    );
    assert!(
        app.tree_rows()
            .iter()
            .any(|r| matches!(r.node, Node::Status(_)))
    );
    assert_eq!(app.messages.len(), 650);
    assert!(!app.busy);
    focus_mail(&mut app, &id);
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    assert_eq!(reads.load(Ordering::Relaxed), 1);
    assert!(app.parsed.is_some());
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn loaded_metadata_budget_does_not_advance_a_cursor_past_unloaded_mail() {
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut rx, worker) = start_worker(
        Box::new(PagedFixture {
            calls: Arc::new(AtomicUsize::new(0)),
            raw_reads: Arc::new(AtomicUsize::new(0)),
            inbox: 2500,
        }),
        current.clone(),
    );
    let mut app = App::new(true, "no-color".into(), current.clone(), tx);
    app.reload(false);
    settle(&mut app, &mut rx).await;
    expand(&mut app, &mut rx, "large", "INBOX").await;
    for _ in 0..8 {
        focus_folder(&mut app, "large", "INBOX");
        command(&mut app, "more");
        settle(&mut app, &mut rx).await;
    }
    assert_eq!(app.messages.len(), 1800);
    focus_folder(&mut app, "large", "Archive");
    app.action(Action::Expand);
    assert_eq!(app.pages.values().map(|p| p.reserved).sum::<usize>(), 200);
    focus_folder(&mut app, "large", "Broken");
    app.action(Action::Expand); // The Archive request has already reserved the remaining budget.
    assert!(!app.pages.contains_key(&folder_key("large", "Broken")));
    settle(&mut app, &mut rx).await;
    assert_eq!(app.messages.len(), mailtui::app::MAX_LOADED);
    let cursor = app.pages[&folder_key("large", "INBOX")].next.clone();
    focus_folder(&mut app, "large", "INBOX");
    command(&mut app, "more");
    assert!(!app.busy);
    assert_eq!(app.pages[&folder_key("large", "INBOX")].next, cursor);
    expand(&mut app, &mut rx, "large", "Broken").await;
    assert_eq!(app.messages.len(), mailtui::app::MAX_LOADED);
    assert!(!app.pages.contains_key(&folder_key("large", "Broken")));
    assert!(app.toast.contains("2,000"));
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn responsive_golden_screens_are_one_tree_at_every_terminal_size() {
    use ratatui::style::Color;
    let (mut app, mut rx, current, worker) = fixture().await;
    for (w, h) in [(40, 12), (80, 24), (120, 40), (180, 50)] {
        insta::assert_snapshot!(format!("inbox_{w}x{h}"), capture(&mut app, w, h));
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    terminal
        .draw(|f| mailtui::tui::draw(f, &mut app, true))
        .unwrap(); // Legacy preview cannot create another pane.
    assert_eq!(terminal.backend().buffer()[(0, 0)].bg, Color::Black);
    assert_eq!(terminal.backend().buffer()[(1, 8)].bg, Color::Cyan);
    for command_name in ["accounts", "status", "outbox", "history"] {
        command(&mut app, command_name);
        insta::assert_snapshot!(format!("view_{command_name}"), capture(&mut app, 80, 24));
        app.key(KeyCode::Esc.into());
    }
    let id = first_mail(&app, "work", "INBOX");
    focus_mail(&mut app, &id);
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    insta::assert_snapshot!("view_detail", capture(&mut app, 80, 24));
    key(&mut app, '?');
    insta::assert_snapshot!("overlay_help", capture(&mut app, 80, 24));
    app.key(KeyCode::Esc.into());
    app.key(KeyCode::Esc.into());
    command(&mut app, "search from:alice@example.com kubernetes");
    settle(&mut app, &mut rx).await;
    insta::assert_snapshot!("view_search", capture(&mut app, 80, 24));
    command(&mut app, "sort invalid");
    insta::assert_snapshot!("error", capture(&mut app, 80, 24));
    app.action(Action::ToggleHeader);
    app.action(Action::ToggleBreadcrumbs);
    app.action(Action::ToggleWide);
    assert!(!app.show_header);
    assert!(!app.show_breadcrumbs);
    assert!(app.wide);
    assert!(capture(&mut app, 120, 40).contains("SIZE"));
    for (w, h) in [(1, 1), (19, 5), (30, 8), (40, 12), (80, 24)] {
        let _ = capture(&mut app, w, h);
    }
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn fuzzy_move_directory_creation_and_composer_are_functional() {
    let (mut app, mut rx, current, worker) = fixture().await;
    app.focus_key(&RowKey::Account("personal".into()));
    key(&mut app, 'c');
    assert!(
        matches!(&app.overlay, Some(Overlay::Compose { account, .. }) if app.accounts[*account].id == "personal")
    );
    app.key(KeyCode::Esc.into());
    key(&mut app, 'n');
    assert!(
        matches!(&app.overlay, Some(Overlay::CreateFolder { account, .. }) if account == "personal")
    );
    app.key(KeyCode::Esc.into());
    command(&mut app, "ns work");
    settle(&mut app, &mut rx).await;
    let id = first_mail(&app, "work", "INBOX");
    app.folders.push(Folder {
        account: "work".into(),
        id: "opaque/42".into(),
        name: "Vacances".into(),
        parent: None,
        role: None,
        total: 0,
        unread: 0,
    });
    assert_eq!(
        app.move_destinations(std::slice::from_ref(&id), "vcns"),
        vec![folder_key("work", "opaque/42")]
    );
    app.folders.pop();
    focus_mail(&mut app, &id);
    key(&mut app, ' ');
    let rendered = capture(&mut app, 120, 40);
    assert!(!rendered.contains("│ST "));
    assert!(rendered.contains(&app.focused().unwrap().date[..16].replace('T', " ")));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    terminal
        .draw(|frame| mailtui::tui::draw(frame, &mut app, false))
        .unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.bg == ratatui::style::Color::Yellow)
    );
    key(&mut app, 'm');
    app.paste("arv"); // Subsequence, not a substring of Archive.
    insta::assert_snapshot!("move_picker", capture(&mut app, 80, 24));
    assert_eq!(
        app.move_destinations(&[id], "arv"),
        vec![folder_key("work", "Archive")]
    );
    app.key(KeyCode::Enter.into());
    key(&mut app, 'c');
    app.paste("keep@example.com");
    settle(&mut app, &mut rx).await;
    assert!(
        matches!(&app.overlay, Some(Overlay::Compose { fields, .. }) if fields[0].text == "keep@example.com")
    );
    app.key(KeyCode::Esc.into());
    assert_eq!(
        app.folders
            .iter()
            .find(|f| f.account == "work" && f.id == "Archive")
            .unwrap()
            .total,
        9
    );
    key(&mut app, 'n');
    app.paste("Travel");
    insta::assert_snapshot!("create_directory", capture(&mut app, 80, 24));
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    assert!(
        app.folders
            .iter()
            .any(|f| f.account == "work" && f.id == "Travel" && f.parent.is_none())
    );
    key(&mut app, 'c');
    app.paste("alice@example.com, bob@example.com");
    app.key(KeyCode::Tab.into());
    app.paste("Bonjour · 日本語");
    app.key(KeyCode::Tab.into());
    app.paste("Première ligne\nDeuxième ligne");
    insta::assert_snapshot!("composer", capture(&mut app, 80, 24));
    ctrl(&mut app, 's');
    app.event(Event::Mutation {
        generation: 0,
        result: MutationResult::AppliedLocal,
    });
    assert!(app.form_pending && app.busy && matches!(app.overlay, Some(Overlay::Compose { .. })));
    settle(&mut app, &mut rx).await;
    assert!(app.overlay.is_none());
    expand(&mut app, &mut rx, "work", "Sent").await;
    let sent = first_mail(&app, "work", "Sent");
    focus_mail(&mut app, &sent);
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    assert_eq!(app.detail.as_ref().unwrap().subject, "Bonjour · 日本語");
    assert!(
        app.parsed
            .as_ref()
            .unwrap()
            .text
            .contains("Première ligne\nDeuxième ligne")
    );
    assert_eq!(
        app.parsed.as_ref().unwrap().recipients,
        "alice@example.com, bob@example.com"
    );
    app.key(KeyCode::Esc.into());
    key(&mut app, 'c');
    app.key(KeyCode::Tab.into());
    app.paste("Draft without recipients");
    ctrl(&mut app, 'o');
    settle(&mut app, &mut rx).await;
    assert!(
        app.folders
            .iter()
            .any(|f| f.account == "work" && f.id == "Drafts" && f.total == 1)
    );
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn quit_window_and_html_modes_are_explicit_and_branch_shortcuts_are_gone() {
    let (mut app, mut rx, current, worker) = fixture().await;
    for code in [
        KeyCode::Char('M'),
        KeyCode::Char('C'),
        KeyCode::Char('D'),
        KeyCode::Char('l'),
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Backspace,
    ] {
        assert!(mailtui::app::dispatch(code.into()).is_none());
    }
    let id = first_mail(&app, "work", "INBOX");
    focus_mail(&mut app, &id);
    key(&mut app, 'h');
    settle(&mut app, &mut rx).await;
    assert!(app.want_html && !app.html_remote);
    app.want_html = false;
    key(&mut app, 'H');
    assert!(app.want_html && app.html_remote);
    let now = std::time::Instant::now();
    app.confirm_quit(now);
    assert!(!app.quit);
    assert!(capture(&mut app, 80, 24).contains("Ctrl+C again"));
    app.confirm_quit(now + Duration::from_secs(4));
    assert!(!app.quit, "An expired confirmation must arm a new window");
    app.confirm_quit(now + Duration::from_secs(5));
    assert!(app.quit);
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn attachment_browser_filters_marks_across_directories_and_sends_original_bytes() {
    use mail_parser::MimeHeaders;
    let directory = tempfile::tempdir().unwrap();
    let nested = directory.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let pdf = directory.path().join("report.pdf");
    let binary = nested.join("données.bin");
    std::fs::write(&pdf, b"%PDF-1.7\nreport\n").unwrap();
    std::fs::write(&binary, [0, 1, 127, 255, 42]).unwrap();
    let (mut app, mut rx, current, worker) = fixture().await;
    key(&mut app, 'c');
    app.paste("alice@example.com");
    app.key(KeyCode::Tab.into());
    app.paste("Multiple attachments");
    app.key(KeyCode::Tab.into());
    app.paste("See attached.");
    ctrl(&mut app, 't');
    app.browse_files(directory.path().into());
    settle(&mut app, &mut rx).await;
    app.paste("rpt"); // report.pdf, non-contiguous fuzzy match.
    app.key(KeyCode::Enter.into());
    assert!(
        matches!(&app.overlay, Some(Overlay::Compose { attachments, fields, .. }) if attachments == &vec![pdf.clone()] && fields[2].text == "See attached.")
    );
    ctrl(&mut app, 't');
    app.browse_files(directory.path().into());
    settle(&mut app, &mut rx).await;
    app.paste("nst");
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    app.paste("dnbi");
    app.key(KeyCode::Enter.into());
    assert!(
        matches!(&app.overlay, Some(Overlay::Compose { attachments, .. }) if attachments.len() == 2)
    );
    ctrl(&mut app, 't');
    app.browse_files(nested.clone());
    settle(&mut app, &mut rx).await;
    app.paste("dnbi");
    app.key(KeyCode::Tab.into()); // Unmark, then mark it again with the fuzzy multi-select key.
    app.key(KeyCode::Tab.into());
    let original = app.overlay.clone();
    if let Some(Overlay::Files { path, .. }) = &mut app.overlay {
        *path = "[fixture]/nested".into();
    }
    insta::assert_snapshot!("attachment_picker", capture(&mut app, 100, 28));
    app.overlay = original;
    ctrl(&mut app, 's');
    let Some(Overlay::Compose {
        attachments,
        fields,
        ..
    }) = &app.overlay
    else {
        panic!("Composer was not restored");
    };
    assert_eq!(attachments.len(), 2);
    let draft = Draft {
        account: "work".into(),
        to: fields[0].text.clone(),
        subject: fields[1].text.clone(),
        body: fields[2].text.clone(),
        attachments: attachments.clone(),
        reply: None,
    };
    let raw = draft.mime("me@example.com").unwrap();
    let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
    let parts: std::collections::HashMap<_, _> = parsed
        .attachments()
        .map(|part| (part.attachment_name().unwrap(), part.contents()))
        .collect();
    assert_eq!(parts["report.pdf"], b"%PDF-1.7\nreport\n");
    assert_eq!(parts["données.bin"], &[0, 1, 127, 255, 42]);
    insta::assert_snapshot!("composer_attachments", capture(&mut app, 100, 28));
    // A missing attachment must preserve the entire composer for correction/retry.
    std::fs::remove_file(&binary).unwrap();
    ctrl(&mut app, 's');
    settle(&mut app, &mut rx).await;
    assert!(
        matches!(&app.overlay, Some(Overlay::Compose { attachments, .. }) if attachments.len() == 2)
    );
    assert!(app.toast.contains("Read attachment"));
    std::fs::write(&binary, [0, 1, 127, 255, 42]).unwrap();
    ctrl(&mut app, 's');
    settle(&mut app, &mut rx).await;
    expand(&mut app, &mut rx, "work", "Sent").await;
    let id = first_mail(&app, "work", "Sent");
    focus_mail(&mut app, &id);
    assert!(app.focused().unwrap().attachment);
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    assert_eq!(app.parsed.as_ref().unwrap().attachments.len(), 2);
    assert!(
        app.parsed
            .as_ref()
            .unwrap()
            .attachments
            .iter()
            .any(|attachment| attachment.name == "données.bin")
    );
    // Download just the pointed attachment after a fuzzy search.
    ctrl(&mut app, 's');
    app.paste("dnbi");
    insta::assert_snapshot!("download_attachments_picker", capture(&mut app, 100, 28));
    app.key(KeyCode::Enter.into());
    assert!(matches!(app.overlay, Some(Overlay::SaveAttachments { .. })));
    app.key(KeyCode::Esc.into());
    assert!(
        matches!(&app.overlay, Some(Overlay::Attachments { query, selected, .. }) if query.text == "dnbi" && selected.len() == 1)
    );
    app.key(KeyCode::Enter.into());
    let single = directory.path().join("single");
    if let Some(Overlay::SaveAttachments { directory, .. }) = &mut app.overlay {
        directory.set(single.to_string_lossy().into_owned());
    }
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    assert!(app.overlay.is_none());
    assert_eq!(
        std::fs::read(single.join("données.bin")).unwrap(),
        [0, 1, 127, 255, 42]
    );
    assert!(!single.join("report.pdf").exists());

    // Download all, preserve pre-existing files, and recover from a bad destination.
    ctrl(&mut app, 's');
    ctrl(&mut app, 'a');
    app.key(KeyCode::Enter.into());
    if let Some(Overlay::SaveAttachments { directory, .. }) = &mut app.overlay {
        directory.set(pdf.to_string_lossy().into_owned()); // a file, not a directory
    }
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    assert!(
        matches!(&app.overlay, Some(Overlay::SaveAttachments { error: Some(_), picker, .. }) if matches!(picker.as_ref(), Overlay::Attachments { selected, .. } if selected.len() == 2))
    );
    let downloads = directory.path().join("downloads");
    std::fs::create_dir(&downloads).unwrap();
    std::fs::write(downloads.join("report.pdf"), b"keep existing").unwrap();
    if let Some(Overlay::SaveAttachments {
        directory, error, ..
    }) = &mut app.overlay
    {
        directory.set("[fixture]/downloads".into());
        *error = None;
    }
    app.toast = "Saved 1 attachment(s) → [fixture]/single".into();
    insta::assert_snapshot!("download_destination", capture(&mut app, 100, 28));
    if let Some(Overlay::SaveAttachments { directory, .. }) = &mut app.overlay {
        directory.set(downloads.to_string_lossy().into_owned());
    }
    app.key(KeyCode::Enter.into());
    app.key(KeyCode::Esc.into()); // A confirmed write finishes before leaving its form.
    settle(&mut app, &mut rx).await;
    assert!(app.overlay.is_none());
    assert_eq!(
        std::fs::read(downloads.join("report.pdf")).unwrap(),
        b"keep existing"
    );
    assert_eq!(
        std::fs::read(downloads.join("report (1).pdf")).unwrap(),
        b"%PDF-1.7\nreport\n"
    );
    assert_eq!(
        std::fs::read(downloads.join("données.bin")).unwrap(),
        [0, 1, 127, 255, 42]
    );
    assert_eq!(app.view, View::Detail);
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn quitting_drains_all_confirmed_writes_even_after_the_event_receiver_closes() {
    struct CounterStore(Arc<AtomicUsize>);
    impl MailStore for CounterStore {
        fn accounts(&self) -> Vec<Account> {
            vec![]
        }
        fn folders(&self) -> anyhow::Result<Vec<Folder>> {
            Ok(vec![])
        }
        fn messages(&self, _: &Scope, _: Option<&Cursor>, _: usize) -> anyhow::Result<Page> {
            anyhow::bail!("unused")
        }
        fn raw(&self, _: &MessageId) -> anyhow::Result<Vec<u8>> {
            anyhow::bail!("unused")
        }
        fn pending(&self) -> anyhow::Result<Vec<mailtui::domain::PendingMail>> {
            Ok(vec![])
        }
        fn mutate(&mut self, _: &[MessageId], _: &Mutation) -> anyhow::Result<MutationResult> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(MutationResult::AppliedLocal)
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let current = Arc::new(AtomicU64::new(1)); // UI shutdown has invalidated generation zero.
    let (tx, events, worker) = start_worker(Box::new(CounterStore(calls.clone())), current);
    drop(events);
    for _ in 0..2 {
        tx.send(mailtui::app::Request::Mutate {
            generation: 0,
            ids: vec![],
            mutation: Mutation::Seen(true),
        })
        .await
        .unwrap();
    }
    drop(tx);
    worker.await.unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn a_queued_submission_receipt_survives_navigation_and_requests_a_local_update() {
    let (mut app, mut rx, current, worker) = fixture().await;
    app.demo = false;
    app.switch_account(Some("personal".into()));
    settle(&mut app, &mut rx).await;
    app.event(Event::Submitted {
        generation: 0,
        account: "work".into(),
        pending: vec![mailtui::domain::PendingMail {
            account: "work".into(),
            id: 42,
            error: None,
        }],
        result: MutationResult::Queued("Message queued".into()),
    });
    assert!(app.refresh_requested);
    assert!(app.index_dirty);
    assert_eq!(
        app.send_requests,
        ["work".to_string()].into_iter().collect()
    );
    assert_eq!(app.pending_label(), "Pending: 1 sending");
    app.delivery_completed("work".into(), Some("offline".into()));
    assert_eq!(app.pending_label(), "Pending: 1 failed");
    app.event(Event::Pending {
        generation: 0,
        result: Ok(vec![]),
    });
    assert_eq!(app.pending_label(), "Pending: 0");
    assert!(app.toast.contains("Message queued"));
    settle(&mut app, &mut rx).await;
    finish(app, rx, current, worker).await;
}

#[tokio::test]
async fn local_search_popup_coalesces_input_searches_bodies_and_opens_the_pointed_message() {
    let (mut app, mut rx, current, worker) = fixture().await;
    key(&mut app, '/');
    app.paste("kbrnts");
    settle(&mut app, &mut rx).await;
    assert!(matches!(&app.overlay, Some(Overlay::Find { messages, .. }) if !messages.is_empty()));
    insta::assert_snapshot!("local_search_picker", capture(&mut app, 100, 28));
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut rx).await;
    assert_eq!(app.view, View::Detail);
    assert!(app.detail.as_ref().unwrap().subject.contains("Kubernetes"));
    assert!(app.parsed.is_some());
    for code in [
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Char('j'),
        KeyCode::Char('k'),
        KeyCode::Char('g'),
        KeyCode::Char('G'),
        KeyCode::Char('*'),
        KeyCode::Char('d'),
    ] {
        assert!(mailtui::app::dispatch(code.into()).is_none());
    }
    for code in ['r', 'u', 'e', 'w', 'g', '\\', ' '] {
        assert!(
            mailtui::app::dispatch(KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL))
                .is_none()
        );
    }
    finish(app, rx, current, worker).await;
}

fn capture(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| mailtui::tui::draw(f, app, false))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let mut lines = vec![];
    for y in 0..h {
        let mut line = String::new();
        let mut x = 0;
        while x < w {
            let symbol = buffer[(x, y)].symbol();
            line.push_str(symbol);
            x += unicode_width::UnicodeWidthStr::width(symbol).max(1) as u16;
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

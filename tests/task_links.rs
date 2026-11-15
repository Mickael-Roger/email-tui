use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mailtui::{
    app::{App, Event, Overlay, View, start_worker_with_index},
    domain::{
        Account, Cursor, Folder, Message, MessageId, Mutation, MutationResult, Page, PendingMail,
        Scope,
    },
    search::SearchIndex,
    storage::MailStore,
    task_form::{Field, Popup, TaskEditor},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

const ORIGINAL: &[u8] = b"Message-ID: <original/42+tag?%#@example.test>\r\nSubject: Original\r\n\r\nOriginal email body";
const REPLY: &[u8] = b"Message-ID: <reply@example.test>\r\nReferences: <original/42+tag?%#@example.test>\r\nSubject: Reply\r\n\r\nReply body";

struct Store {
    original_available: Arc<AtomicBool>,
}
impl Store {
    fn original() -> Message {
        Message {
            id: MessageId {
                account: "work".into(),
                folder: "Archive/Moved".into(),
                seq: 1,
            },
            sender: "alice@example.test".into(),
            subject: "Original".into(),
            date: "2026-11-01".into(),
            seen: true,
            flagged: false,
            attachment: false,
            size: ORIGINAL.len() as u64,
        }
    }
    fn reply() -> Message {
        Message {
            id: MessageId {
                account: "personal".into(),
                folder: "INBOX".into(),
                seq: 2,
            },
            subject: "Reply".into(),
            date: "2026-11-15".into(),
            ..Self::original()
        }
    }
}
impl MailStore for Store {
    fn accounts(&self) -> Vec<Account> {
        vec![]
    }
    fn folders(&self) -> anyhow::Result<Vec<Folder>> {
        Ok(vec![])
    }
    fn messages(&self, _: &Scope, _: Option<&Cursor>, _: usize) -> anyhow::Result<Page> {
        Ok(Page {
            messages: vec![Self::original(), Self::reply()],
            total: 2,
            next: None,
        })
    }
    fn raw(&self, id: &MessageId) -> anyhow::Result<Vec<u8>> {
        if id == &Self::original().id && self.original_available.load(Ordering::Relaxed) {
            Ok(ORIGINAL.to_vec())
        } else if id == &Self::reply().id {
            Ok(REPLY.to_vec())
        } else {
            anyhow::bail!("Message unavailable")
        }
    }
    fn mutate(&mut self, _: &[MessageId], _: &Mutation) -> anyhow::Result<MutationResult> {
        anyhow::bail!("Unexpected mutation")
    }
    fn pending(&self) -> anyhow::Result<Vec<PendingMail>> {
        Ok(vec![])
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    app: App,
    events: tokio::sync::mpsc::Receiver<Event>,
    worker: tokio::task::JoinHandle<()>,
    available: Arc<AtomicBool>,
    link: String,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let available = Arc::new(AtomicBool::new(true));
        let store = Store {
            original_available: available.clone(),
        };
        let path = root.path().join("index.sqlite3");
        let index = SearchIndex::open(&path).unwrap();
        index.fill_store(&store).unwrap();
        // The newer reply is a candidate too: matching a thread token is insufficient.
        let candidates = index
            .referencing_message_id("original/42+tag?%#@example.test")
            .unwrap();
        assert_eq!(candidates[0].subject, "Reply");
        drop(index);
        let current = Arc::new(AtomicU64::new(0));
        let (tx, events, worker) =
            start_worker_with_index(Box::new(store), current.clone(), Some(path));
        let mut app = App::new(false, "no-color".into(), current, tx);
        app.view = View::Tasks;
        app.scope.account = Some("personal".into());
        let link = mailtui::mime::message_link(ORIGINAL).unwrap();
        let mut editor = TaskEditor::new(0, None, None);
        editor.title.set("Unsaved title".into());
        editor.description.set("Unsaved description".into());
        editor.due = "20261201".into();
        editor.comments.set(format!(
            "Original email\n{link}\n\nDeleted email\nmid:missing@example.test"
        ));
        editor.focus = Field::Comments;
        app.overlay = Some(Overlay::TaskForm(Box::new(editor)));
        Self {
            _root: root,
            app,
            events,
            worker,
            available,
            link,
        }
    }
    fn ctrl(&mut self, c: char) {
        self.app
            .key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }
    fn press(&mut self, key: KeyCode) {
        self.app.key(key.into());
    }
    fn editor(&self) -> &TaskEditor {
        let Some(Overlay::TaskForm(editor)) = &self.app.overlay else {
            panic!("Missing task form")
        };
        editor
    }
    async fn settle(&mut self) {
        while self.app.busy {
            let event = tokio::time::timeout(std::time::Duration::from_secs(5), self.events.recv())
                .await
                .unwrap()
                .unwrap();
            self.app.event(event);
        }
    }
    async fn finish(self) {
        drop(self.app);
        drop(self.events);
        self.worker.await.unwrap();
    }
}

#[tokio::test]
async fn task_links_open_exact_moved_email_across_accounts_and_return_to_the_unsaved_form() {
    let mut f = Fixture::new();
    f.ctrl('o');
    let Some(Popup::Links { links, row }) = &f.editor().popup else {
        panic!("Missing link selector")
    };
    assert_eq!(*row, 0);
    assert_eq!(links, &[f.link.clone(), "mid:missing@example.test".into()]);
    // Link selection is rendered safely at every supported terminal size.
    for (width, height) in [(1, 1), (20, 6), (40, 12), (100, 32)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| mailtui::tui::draw(frame, &mut f.app, false))
            .unwrap();
    }
    f.press(KeyCode::Down);
    f.press(KeyCode::Up);
    f.press(KeyCode::Enter);
    assert!(f.editor().opening_link);
    f.settle().await;
    assert_eq!(f.app.view, View::Detail);
    assert!(f.app.overlay.is_none());
    assert_eq!(f.app.detail.as_ref().unwrap().id, Store::original().id);
    assert!(
        f.app
            .parsed
            .as_ref()
            .unwrap()
            .text
            .contains("Original email body")
    );
    f.app.pim_files_changed();
    assert!(
        !f.app.pim_loading,
        "PIM refresh must remain deferred while the task draft is suspended"
    );
    f.press(KeyCode::Esc);
    assert_eq!(f.app.view, View::Tasks);
    assert_eq!(f.editor().title.text, "Unsaved title");
    assert_eq!(f.editor().description.text, "Unsaved description");
    assert_eq!(f.editor().due, "20261201");
    assert_eq!(f.editor().focus, Field::Comments);
    assert!(f.editor().comments.text.contains(&f.link));
    assert!(!f.editor().opening_link);
    assert!(!f.app.pim_pending);
    f.finish().await;
}

#[tokio::test]
async fn missing_and_malformed_links_keep_the_task_and_never_open_a_reply_instead() {
    let mut f = Fixture::new();
    f.available.store(false, Ordering::Relaxed);
    f.ctrl('o');
    f.press(KeyCode::Enter);
    f.settle().await;
    assert_eq!(f.app.view, View::Tasks);
    assert!(f.editor().error.as_ref().unwrap().contains("not found"));
    assert_eq!(f.editor().title.text, "Unsaved title");
    assert!(!f.editor().opening_link);
    if let Some(Overlay::TaskForm(editor)) = &mut f.app.overlay {
        editor.comments.set("mid:bad%XX@example.test".into());
    }
    f.ctrl('o');
    f.press(KeyCode::Enter);
    f.settle().await;
    assert!(
        f.editor()
            .error
            .as_ref()
            .unwrap()
            .contains("percent encoding")
    );
    assert_eq!(f.app.view, View::Tasks);
    f.finish().await;
}

#[tokio::test]
async fn cancelling_link_selection_or_loading_never_opens_a_late_email() {
    let mut f = Fixture::new();
    f.ctrl('o');
    f.press(KeyCode::Esc);
    assert!(f.editor().popup.is_none());
    assert!(!f.app.busy);
    if let Some(Overlay::TaskForm(editor)) = &mut f.app.overlay {
        editor.comments.set("Notes without email links".into());
    }
    f.ctrl('o');
    assert!(f.editor().error.as_ref().unwrap().contains("No mid:"));
    assert!(!f.app.busy);
    if let Some(Overlay::TaskForm(editor)) = &mut f.app.overlay {
        editor.comments.set(f.link.clone());
    }
    f.ctrl('o');
    f.press(KeyCode::Enter);
    f.press(KeyCode::Esc);
    assert!(f.app.overlay.is_none());
    f.settle().await;
    assert_eq!(f.app.view, View::Tasks);
    assert!(f.app.detail.is_none());
    assert!(f.app.overlay.is_none());
    f.finish().await;
}

#[test]
fn mid_link_decoding_is_strict_and_roundtrips_reserved_characters() {
    let link = mailtui::mime::message_link(ORIGINAL).unwrap();
    assert_eq!(
        mailtui::mime::message_link_id(&link).unwrap(),
        "original/42+tag?%#@example.test"
    );
    for link in [
        "mid:",
        "mid:bad%",
        "mid:bad%0",
        "mid:bad%GG",
        "mid:bad%FF",
        "mid:bad%0A",
        "https://example.test",
        "mid:message/part",
    ] {
        assert!(mailtui::mime::message_link_id(link).is_err(), "{link}");
    }
}

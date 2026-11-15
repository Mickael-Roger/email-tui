use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mailtui::{
    app::{App, Event, Overlay, View, start_worker},
    config::{CalendarsConfig, Config, ContactsConfig, PimCollection},
    pim,
    pim_sync::Target,
    storage::DemoStore,
};
use std::{
    fs,
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};

const TODO: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\nEND:VTIMEZONE\r\nBEGIN:VTODO\r\nUID:task@example.test\r\nSUMMARY:Planning\r\nDUE;TZID=Europe/Paris:20261120T090000\r\nSTATUS:NEEDS-ACTION\r\nRRULE:FREQ=WEEKLY\r\nX-CUSTOM:keep\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

fn config(root: &std::path::Path) -> Config {
    for name in [
        "tasks/home",
        "tasks/work",
        "calendars/work",
        "tasks/.hidden",
    ] {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    let path = root.join("config.toml");
    fs::write(&path, format!(
        "[calendars]\nenabled=true\npath={:?}\nsync_command=['calendar-sync','--all']\n[tasks]\nenabled=true\npath={:?}\nsync_command=['task-sync','--all']\nsync_timeout_seconds=30\n",
        root.join("calendars"), root.join("tasks")
    )).unwrap();
    Config::load(Some(path)).unwrap()
}

#[test]
fn task_and_sync_options_are_validated() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.tasks.sync_command = vec![String::new()];
    assert!(config.validate().is_err());
    config.tasks.sync_command.clear();
    config.tasks.sync_timeout_seconds = 0;
    assert!(config.validate().is_err());
    config.tasks.sync_timeout_seconds = 120;
    config.tasks.path = "relative/tasks".into();
    assert!(config.validate().is_err());
    config.tasks.enabled = false;
    assert!(config.validate().is_ok());
    config.calendars.sync_timeout_seconds = 0;
    assert!(config.validate().is_err());
}

#[test]
fn task_discovery_and_vtodo_edits_preserve_original_metadata_and_reject_stale_writes() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    assert_eq!(
        config
            .tasks
            .collections
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["home", "work"]
    );
    let list = &config.tasks.collections[0];
    let file = list.path.join("original-server-name.ics");
    fs::write(&file, TODO).unwrap();
    fs::write(list.path.join("event.ics"), "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:event\nSUMMARY:Meeting\nDTSTART:20261120\nEND:VEVENT\nEND:VCALENDAR").unwrap();
    let initial = pim::load_with_tasks(&config.tasks.collections, &[], &config.tasks.collections);
    assert_eq!(initial.events.len(), 1);
    assert_eq!(initial.tasks.len(), 1);
    assert!(initial.warnings.is_empty());
    let mut task = initial.tasks[0].clone();
    task.summary = "Préparer le projet".into();
    task.status = "COMPLETED".into();
    task.priority = "1".into();
    pim::write_task(list, &task).unwrap();
    let updated = fs::read_to_string(&file).unwrap();
    for property in [
        "X-CUSTOM:keep",
        "BEGIN:VTIMEZONE",
        "RRULE:FREQ=WEEKLY",
        "BEGIN:VALARM",
        "DUE;TZID=Europe/Paris:20261120T090000",
        "PERCENT-COMPLETE:100",
        "COMPLETED:",
        "STATUS:COMPLETED",
    ] {
        assert!(updated.contains(property), "Missing {property}");
    }
    assert!(pim::write_task(list, &task).is_err());
    let mut reopened = pim::load_with_tasks(&[], &[], &config.tasks.collections)
        .tasks
        .remove(0);
    reopened.status = "NEEDS-ACTION".into();
    pim::write_task(list, &reopened).unwrap();
    let updated = fs::read_to_string(&file).unwrap();
    assert!(!updated.contains("\r\nCOMPLETED:"));
    assert!(updated.contains("PERCENT-COMPLETE:0"));
    let latest = pim::load_with_tasks(&[], &[], &config.tasks.collections)
        .tasks
        .remove(0);
    let other = PimCollection {
        id: "outside".into(),
        label: "Outside".into(),
        path: root.path().join("outside"),
    };
    assert!(pim::write_task(&other, &latest).is_err());
    let change = pim::Change::Delete {
        file: latest.file,
        source: latest.source,
    };
    assert!(pim::execute(&[], &[], change.clone()).is_err());
    pim::execute_with_tasks(&[], &[], &config.tasks.collections, change).unwrap();
    assert!(!file.exists());
    assert!(list.path.join("event.ics").exists());
}

async fn settle(app: &mut App, events: &mut tokio::sync::mpsc::Receiver<Event>) {
    while app.pim_pending || app.pim_loading || app.busy {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        app.event(event);
    }
}
fn field(app: &mut App, index: usize, value: &str) {
    match &mut app.overlay {
        Some(Overlay::TaskForm(editor)) => match index {
            0 => editor.title.set(value.into()),
            1 => editor.due = value.into(),
            2 => editor.status = value.into(),
            3 => editor.priority = value.into(),
            4 => editor.description.set(value.into()),
            5 => editor.start = value.into(),
            _ => panic!("Invalid task field"),
        },
        Some(Overlay::PimForm { fields, .. }) => fields[index].set(value.into()),
        _ => panic!("Missing form"),
    }
}
fn save(app: &mut App) {
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
}

#[tokio::test]
async fn mail_selection_creates_one_task_with_links_in_comments_and_keeps_mail_context() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "no-color".into(), current, tx);
    app.configure_tasks_and_sync(&config.calendars, &config.tasks);
    app.reload(false);
    settle(&mut app, &mut events).await;

    // A bare account cannot become an email reference.
    app.key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert!(app.overlay.is_none());
    assert!(app.toast.contains("Select a message"));

    app.execute("unified");
    settle(&mut app, &mut events).await;
    let messages: Vec<_> = ["personal", "work"]
        .into_iter()
        .map(|account| {
            app.messages
                .iter()
                .find(|m| m.id.account == account && !m.attachment)
                .unwrap()
                .clone()
        })
        .collect();
    for message in &messages {
        app.selected.insert(message.id.clone());
    }
    app.expanded.clear();
    let row = app.row;
    let selected = app.selected.clone();
    app.key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    settle(&mut app, &mut events).await;
    let Some(Overlay::TaskForm(editor)) = &app.overlay else {
        panic!("Missing task editor")
    };
    assert!(editor.title.text.is_empty());
    assert!(editor.description.text.is_empty());
    let comments = editor.comments.text.clone();
    assert_eq!(comments.matches("mid:").count(), 2);
    for message in &messages {
        assert!(comments.contains(&message.subject));
        assert!(comments.contains(&format!(
            "mid:demo-{}-{}@example.com",
            message.id.seq, message.id.account
        )));
    }

    // The usual selectors and fields stay available while Mail remains active.
    app.key(KeyCode::Enter.into());
    app.key(KeyCode::Down.into());
    app.key(KeyCode::Enter.into());
    app.key(KeyCode::Tab.into());
    app.paste("Follow up on these emails");
    if let Some(Overlay::TaskForm(editor)) = &mut app.overlay {
        editor.focus = mailtui::task_form::Field::Description;
    }
    app.paste("Separate task description");
    app.key(KeyCode::Tab.into());
    app.key(KeyCode::Enter.into());
    app.paste("Notes: café, 日本語; follow up");
    save(&mut app);
    settle(&mut app, &mut events).await;
    assert!(app.overlay.is_none());
    assert_eq!(app.view, View::Messages);
    assert_eq!(app.row, row);
    assert_eq!(app.selected, selected);
    assert_eq!(app.tasks.len(), 1);
    let task = &app.tasks[0];
    assert_eq!(task.collection, "work");
    assert_eq!(task.summary, "Follow up on these emails");
    assert_eq!(task.description, "Separate task description");
    assert_eq!(
        task.comments,
        format!("{comments}\nNotes: café, 日本語; follow up")
    );
    assert!(fs::read_to_string(&task.file).unwrap().contains("COMMENT:"));
    assert!(app.sync_requests.contains(&Target::Tasks));

    app.key(KeyCode::Char('3').into());
    settle(&mut app, &mut events).await;
    app.expanded_task_lists.insert("work".into());
    assert!(app.focus_pim_key(&mailtui::app::PimRowKey::Entry {
        collection: "work".into(),
        file: app.tasks[0].file.clone(),
    }));
    app.key(KeyCode::Enter.into());
    let Some(Overlay::TaskForm(editor)) = &app.overlay else {
        panic!("Missing task editor")
    };
    assert_eq!(editor.comments.text, app.tasks[0].comments);
    drop(app);
    drop(events);
    worker.await.unwrap();
}

#[tokio::test]
async fn mail_task_uses_focused_or_open_email_and_cancel_never_writes() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "no-color".into(), current, tx);
    app.reload(false);
    settle(&mut app, &mut events).await;
    app.key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert!(app.toast.contains("No task lists"));
    app.configure_tasks_and_sync(&config.calendars, &config.tasks);
    settle(&mut app, &mut events).await;
    app.execute("unified");
    settle(&mut app, &mut events).await;
    let message = app.messages[0].clone();
    assert!(app.focus_key(&mailtui::tree::RowKey::Message(message.id.clone())));
    for reader in [false, true] {
        if reader {
            app.key(KeyCode::Enter.into());
            settle(&mut app, &mut events).await;
            // Marks from the list do not override the open email.
            app.selected.insert(app.messages[1].id.clone());
        }
        let view = app.view;
        app.key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        settle(&mut app, &mut events).await;
        let Some(Overlay::TaskForm(editor)) = &app.overlay else {
            panic!("Missing task editor")
        };
        assert_eq!(editor.title.text, message.subject);
        assert_eq!(editor.comments.text.matches("mid:").count(), 1);
        app.key(KeyCode::Esc.into());
        assert!(app.overlay.is_none());
        assert_eq!(app.view, view);
        assert!(app.tasks.is_empty());
        assert!(app.sync_requests.is_empty());
    }
    for list in &app.task_collections {
        assert_eq!(fs::read_dir(&list.path).unwrap().count(), 0);
    }
    app.key(KeyCode::Esc.into());
    app.selected.clear();
    // A missing MIME must fail the whole preparation, not produce partial links.
    app.messages[0].id.seq = i64::MAX;
    assert!(app.focus_key(&mailtui::tree::RowKey::Message(app.messages[0].id.clone())));
    app.key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    settle(&mut app, &mut events).await;
    assert!(app.overlay.is_none());
    assert!(!app.toast.is_empty());
    assert!(app.tasks.is_empty());
    drop(app);
    drop(events);
    worker.await.unwrap();
}

#[test]
fn task_comments_preserve_repeated_properties_and_edit_without_leaving_stale_links() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let list = &config.tasks.collections[0];
    let file = list.path.join("comments.ics");
    fs::write(
        &file,
        TODO.replace(
            "SUMMARY:Planning",
            "SUMMARY:Planning\r\nCOMMENT;LANGUAGE=en:First note\r\nCOMMENT:mid:second@example.test",
        )
        .replace("ACTION:DISPLAY", "ACTION:DISPLAY\r\nCOMMENT:Alarm comment"),
    )
    .unwrap();
    let mut task = pim::load_with_tasks(&[], &[], &config.tasks.collections)
        .tasks
        .remove(0);
    assert_eq!(task.comments, "First note\n\nmid:second@example.test");
    task.summary = "Renamed".into();
    pim::write_task(list, &task).unwrap();
    let raw = fs::read_to_string(&file).unwrap();
    assert!(raw.contains("COMMENT;LANGUAGE=en:First note"));
    assert!(raw.contains("COMMENT:mid:second@example.test"));
    let mut task = pim::load_with_tasks(&[], &[], &config.tasks.collections)
        .tasks
        .remove(0);
    task.comments = "Replacement\nmid:new@example.test".into();
    pim::write_task(list, &task).unwrap();
    let mut task = pim::load_with_tasks(&[], &[], &config.tasks.collections)
        .tasks
        .remove(0);
    assert_eq!(task.comments, "Replacement\nmid:new@example.test");
    let raw = fs::read_to_string(&file).unwrap();
    assert!(!raw.contains("mid:second"));
    assert!(raw.contains("COMMENT:Alarm comment"));
    task.comments.clear();
    pim::write_task(list, &task).unwrap();
    let task = pim::load_with_tasks(&[], &[], &config.tasks.collections)
        .tasks
        .remove(0);
    assert!(task.comments.is_empty());
    assert!(
        fs::read_to_string(file)
            .unwrap()
            .contains("COMMENT:Alarm comment")
    );
}

#[test]
fn message_links_encode_reserved_characters_and_require_a_message_id() {
    assert_eq!(
        mailtui::mime::message_link(b"Message-ID: <part/42+tag?%#@example.test>\r\n\r\nBody")
            .unwrap(),
        "mid:part%2F42%2Btag%3F%25%23@example.test"
    );
    assert!(
        mailtui::mime::message_link(b"Subject: No ID\r\n\r\nBody")
            .unwrap_err()
            .to_string()
            .contains("Message-ID")
    );
}

#[tokio::test]
async fn tasks_ui_and_calendar_writes_request_the_right_sync_and_defer_refresh_during_forms() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "no-color".into(), current, tx);
    app.configure_pim(config.calendar, ContactsConfig::default());
    app.configure_tasks_and_sync(&config.calendars, &config.tasks);
    settle(&mut app, &mut events).await;
    app.key(KeyCode::Char('3').into());
    settle(&mut app, &mut events).await;
    assert_eq!(app.view, View::Tasks);
    app.execute("sync");
    assert!(app.sync_requests.remove(&Target::Tasks));
    app.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert!(app.sync_requests.remove(&Target::Tasks));
    app.key(KeyCode::Char('c').into());
    app.key(KeyCode::Right.into());
    field(&mut app, 0, "New task");
    field(&mut app, 1, "20260230");
    save(&mut app);
    assert!(matches!(
        &app.overlay,
        Some(Overlay::TaskForm(editor)) if editor.error.is_some()
    ));
    assert!(app.sync_requests.is_empty());
    field(&mut app, 1, "20261121");
    save(&mut app);
    settle(&mut app, &mut events).await;
    assert!(app.overlay.is_none());
    assert_eq!(app.tasks.len(), 1);
    assert_eq!(app.tasks[0].collection, "work");
    assert_eq!(app.tasks[0].status, "NEEDS-ACTION");
    app.filter = "To do".into();
    assert_eq!(app.pim_rows().len(), 1);
    app.filter.clear();
    assert!(app.sync_requests.remove(&Target::Tasks));
    for (w, h) in [(40, 12), (80, 24), (120, 40)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| mailtui::tui::draw(f, &mut app, false))
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(rendered.contains("<3>"));
        assert!(rendered.contains("New task"));
        if w >= 45 {
            assert!(rendered.contains("To do"));
        }
        assert!(!rendered.contains("NEEDS-ACTION"));
        assert!(!rendered.contains("all day"));
        assert!(rendered.contains("<Ctrl+R>") || rendered.contains("<ctrl-r>"));
    }
    app.key(KeyCode::Enter.into());
    assert!(matches!(&app.overlay, Some(Overlay::TaskForm(editor))
        if editor.title.text == "New task" && editor.status_label() == "To do"
            && editor.due == "20261121" && editor.focus == mailtui::task_form::Field::Title));
    app.key(KeyCode::Esc.into());
    app.key(KeyCode::Char('e').into());
    assert!(
        matches!(&app.overlay, Some(Overlay::TaskForm(editor)) if editor.status_label() == "To do")
    );
    field(&mut app, 2, "completed");
    app.sync_completed(Target::Tasks, None);
    assert!(
        !app.pim_loading,
        "Sync refresh must not replace a form's source snapshot"
    );
    save(&mut app);
    settle(&mut app, &mut events).await;
    assert_eq!(app.tasks[0].status, "COMPLETED");
    assert!(app.sync_requests.remove(&Target::Tasks));
    assert!(
        app.pim_rows().is_empty(),
        "Completed tasks are hidden by default"
    );
    app.key(KeyCode::Char('v').into());
    assert_eq!(app.pim_rows().len(), 1);
    app.key(KeyCode::Down.into());
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    app.key(KeyCode::Char('y').into());
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut events).await;
    assert!(app.tasks.is_empty());
    assert!(app.sync_requests.remove(&Target::Tasks));

    app.key(KeyCode::Char('1').into());
    settle(&mut app, &mut events).await;
    app.key(KeyCode::Char('c').into());
    field(&mut app, 0, "Meeting");
    field(&mut app, 1, "20261122");
    save(&mut app);
    settle(&mut app, &mut events).await;
    assert!(app.sync_requests.remove(&Target::Calendars));
    app.execute("sync");
    assert!(app.sync_requests.remove(&Target::Calendars));
    app.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert!(app.sync_requests.remove(&Target::Calendars));
    app.sync_completed(Target::Calendars, Some("offline".into()));
    settle(&mut app, &mut events).await;
    assert_eq!(
        app.calendar_events.len(),
        1,
        "Sync failure must preserve local writes"
    );

    app.configure_tasks_and_sync(&CalendarsConfig::default(), &CalendarsConfig::default());
    settle(&mut app, &mut events).await;
    app.execute("sync");
    assert!(app.toast.contains("sync_command"));
    assert!(app.sync_requests.is_empty());
    drop(app);
    drop(events);
    worker.await.unwrap();
}

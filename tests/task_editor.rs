use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mailtui::{
    app::{App, Event, Overlay, start_worker},
    config::{CalendarsConfig, PimCollection},
    pim_sync::Target,
    storage::DemoStore,
    task_form::{DateAction, DateMode, DatePicker, Field, Popup, TaskEditor},
};
use std::{
    fs,
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};

type Events = tokio::sync::mpsc::Receiver<Event>;
struct Fixture {
    _root: tempfile::TempDir,
    app: App,
    events: Events,
    worker: tokio::task::JoinHandle<()>,
    file: std::path::PathBuf,
}
async fn settle(app: &mut App, events: &mut Events) {
    while app.pim_loading || app.pim_pending {
        app.event(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
}
async fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let collections: Vec<_> = ["Work", "Personal"]
        .into_iter()
        .map(|id| {
            let path = root.path().join(id);
            fs::create_dir(&path).unwrap();
            PimCollection {
                id: id.into(),
                label: id.into(),
                path,
            }
        })
        .collect();
    let file = collections[0].path.join("original-server-name.ics");
    fs::write(&file, "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\nEND:VTIMEZONE\r\nBEGIN:VTODO\r\nUID:original\r\nSUMMARY:Original task\r\nDTSTART;TZID=Europe/Paris:20261120T090000\r\nDUE;TZID=Europe/Paris:20261121T153045\r\nSTATUS:NEEDS-ACTION\r\nPRIORITY:4\r\nX-CUSTOM:preserve\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VTODO\r\nEND:VCALENDAR\r\n").unwrap();
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "no-color".into(), current, tx);
    app.configure_tasks_and_sync(
        &CalendarsConfig::default(),
        &CalendarsConfig {
            enabled: true,
            collections,
            sync_command: vec!["fixture-sync".into()],
            ..Default::default()
        },
    );
    settle(&mut app, &mut events).await;
    press(&mut app, KeyCode::Char('3'));
    settle(&mut app, &mut events).await;
    Fixture {
        _root: root,
        app,
        events,
        worker,
        file,
    }
}
async fn finish(fixture: Fixture) {
    drop(fixture.app);
    drop(fixture.events);
    fixture.worker.await.unwrap();
}
fn press(app: &mut App, code: KeyCode) {
    app.key(code.into());
}
fn editor(app: &App) -> &TaskEditor {
    let Some(Overlay::TaskForm(editor)) = &app.overlay else {
        panic!("Missing task editor");
    };
    editor
}
fn capture(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|frame| mailtui::tui::draw(frame, app, false))
        .unwrap();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn editing_uses_dropdowns_and_calendar_and_preserves_original_ics_metadata() {
    let mut fixture = fixture().await;
    let app = &mut fixture.app;
    press(app, KeyCode::Enter);
    press(app, KeyCode::Down);
    press(app, KeyCode::Char('e'));
    assert_eq!(editor(app).focus, Field::Title);
    insta::assert_snapshot!("task_editor_popup", capture(app, 100, 32));
    press(app, KeyCode::Home);
    for _ in 0..editor(app).title.text.chars().count() {
        press(app, KeyCode::Delete);
    }
    app.paste("Préparer le projet");
    press(app, KeyCode::Tab);
    press(app, KeyCode::Enter);
    insta::assert_snapshot!("task_status_selector", capture(app, 100, 32));
    press(app, KeyCode::Down);
    press(app, KeyCode::Enter);
    assert_eq!(editor(app).status, "IN-PROCESS");
    press(app, KeyCode::Tab);
    press(app, KeyCode::Enter);
    insta::assert_snapshot!("task_priority_selector", capture(app, 100, 32));
    press(app, KeyCode::Char('1'));
    assert_eq!(editor(app).priority, "1");
    press(app, KeyCode::Tab);
    press(app, KeyCode::Enter);
    insta::assert_snapshot!("task_date_selector", capture(app, 100, 32));
    press(app, KeyCode::Right);
    press(app, KeyCode::Esc);
    assert_eq!(editor(app).due, "20261121T153045");
    press(app, KeyCode::Enter);
    press(app, KeyCode::Right);
    press(app, KeyCode::Tab); // mode
    press(app, KeyCode::Tab);
    press(app, KeyCode::Up); // hour
    press(app, KeyCode::Tab);
    press(app, KeyCode::Down); // minute
    press(app, KeyCode::Tab);
    press(app, KeyCode::Enter); // use date
    assert_eq!(editor(app).due, "20261122T162945");
    press(app, KeyCode::Tab);
    press(app, KeyCode::Tab); // description
    app.paste("Notes\r\n日本語");
    press(app, KeyCode::Enter);
    app.paste("Dernière ligne");
    press(app, KeyCode::Tab);
    assert_eq!(editor(app).focus, Field::Save);
    press(app, KeyCode::Enter);
    assert!(app.pim_pending);
    press(app, KeyCode::Esc);
    assert!(matches!(app.overlay, Some(Overlay::TaskForm(_))));
    settle(app, &mut fixture.events).await;
    assert!(app.overlay.is_none());
    let task = &app.tasks[0];
    assert_eq!(task.summary, "Préparer le projet");
    assert_eq!(task.status, "IN-PROCESS");
    assert_eq!(task.priority, "1");
    assert_eq!(task.due, "20261122T162945");
    assert_eq!(task.start, "20261120T090000");
    assert_eq!(task.description, "Notes\n日本語\nDernière ligne");
    assert_eq!(task.file, fixture.file);
    let raw = fs::read_to_string(&fixture.file).unwrap();
    for property in [
        "UID:original",
        "DUE;TZID=Europe/Paris:20261122T162945",
        "BEGIN:VTIMEZONE",
        "BEGIN:VALARM",
        "X-CUSTOM:preserve",
    ] {
        assert!(raw.contains(property), "Missing {property}");
    }
    assert!(app.sync_requests.contains(&Target::Tasks));
    finish(fixture).await;
}

#[tokio::test]
async fn creation_selects_a_list_and_optional_dates_and_cancel_never_writes() {
    let mut fixture = fixture().await;
    let app = &mut fixture.app;
    press(app, KeyCode::Char('c'));
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(editor(app).error.is_some());
    assert!(!app.pim_pending);
    press(app, KeyCode::Enter);
    press(app, KeyCode::Down);
    press(app, KeyCode::Enter);
    assert_eq!(editor(app).collection, 1);
    press(app, KeyCode::Tab);
    app.paste("New task");
    press(app, KeyCode::Tab);
    press(app, KeyCode::Tab);
    press(app, KeyCode::Enter);
    for (w, h) in [(20, 6), (40, 12), (80, 24)] {
        capture(app, w, h);
    }
    press(app, KeyCode::Char('5'));
    press(app, KeyCode::Tab);
    press(app, KeyCode::Enter);
    press(app, KeyCode::Home);
    press(app, KeyCode::PageDown);
    let expected = match &editor(app).popup {
        Some(Popup::Date { picker, .. }) => picker.value(),
        _ => panic!("Missing date selector"),
    };
    for (w, h) in [(20, 6), (40, 12), (80, 24)] {
        capture(app, w, h);
    }
    press(app, KeyCode::Enter);
    assert_eq!(expected.len(), 8);
    press(app, KeyCode::Tab);
    press(app, KeyCode::Enter);
    press(app, KeyCode::Delete);
    assert!(editor(app).start.is_empty());
    for (w, h) in [(20, 6), (40, 12), (80, 24), (120, 40)] {
        capture(app, w, h);
    }
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(app, &mut fixture.events).await;
    let task = app
        .tasks
        .iter()
        .find(|task| task.summary == "New task")
        .unwrap();
    assert_eq!(task.collection, "Personal");
    assert_eq!(task.due, expected);
    assert_eq!(task.status, "NEEDS-ACTION");
    assert_eq!(task.priority, "5");
    assert!(task.start.is_empty());
    let count = fs::read_dir(&app.task_collections[1].path).unwrap().count();
    press(app, KeyCode::Char('c'));
    press(app, KeyCode::Tab);
    app.paste("Cancelled task");
    while editor(app).focus != Field::Cancel {
        press(app, KeyCode::Tab);
    }
    press(app, KeyCode::Left);
    assert_eq!(editor(app).focus, Field::Save);
    press(app, KeyCode::Right);
    assert_eq!(editor(app).focus, Field::Cancel);
    press(app, KeyCode::Enter);
    assert!(app.overlay.is_none());
    assert_eq!(
        fs::read_dir(&app.task_collections[1].path).unwrap().count(),
        count
    );
    press(app, KeyCode::Char('c'));
    press(app, KeyCode::Esc);
    assert!(app.overlay.is_none());
    finish(fixture).await;
}

#[test]
fn date_selector_handles_leap_days_month_ends_time_modes_and_clearing() {
    let mut date = DatePicker::new("20240228");
    date.key(KeyCode::Right.into());
    assert_eq!(date.value(), "20240229");
    date.key(KeyCode::Right.into());
    assert_eq!(date.value(), "20240301");
    let mut date = DatePicker::new("20260131");
    date.key(KeyCode::PageDown.into());
    assert_eq!(date.value(), "20260228");
    let mut time = DatePicker::new("20261121T153045Z");
    assert_eq!(time.value(), "20261121T153045Z");
    time.key(KeyCode::Tab.into());
    time.key(KeyCode::Left.into());
    assert_eq!(time.mode, DateMode::LocalTime);
    assert_eq!(time.value(), "20261121T153045");
    time.key(KeyCode::Left.into());
    assert_eq!(time.value(), "20261121");
    assert!(
        matches!(time.key(KeyCode::Delete.into()), DateAction::Apply(value) if value.is_empty())
    );
    assert!(matches!(time.key(KeyCode::Esc.into()), DateAction::Cancel));
}

#[tokio::test]
async fn closing_a_task_hides_it_immediately_and_history_allows_reopening_it() {
    for (status, choice) in [("CANCELLED", '4'), ("COMPLETED", '3')] {
        let mut fixture = fixture().await;
        let app = &mut fixture.app;
        press(app, KeyCode::Enter);
        press(app, KeyCode::Down);
        press(app, KeyCode::Char('e'));
        press(app, KeyCode::Tab);
        press(app, KeyCode::Enter);
        press(app, KeyCode::Char(choice));
        assert_eq!(editor(app).status, status);
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        settle(app, &mut fixture.events).await;
        assert!(app.overlay.is_none());
        assert_eq!(app.tasks[0].status, status);
        assert!(
            app.pim_rows().is_empty(),
            "{status} must disappear from current tasks immediately"
        );
        assert!(
            fs::read_to_string(&fixture.file)
                .unwrap()
                .contains(&format!("STATUS:{status}"))
        );
        press(app, KeyCode::Char('v'));
        assert_eq!(
            app.pim_rows().len(),
            1,
            "Closed tasks remain available in history"
        );
        press(app, KeyCode::Down);
        assert!(app.pim_focused_index().is_some());
        press(app, KeyCode::Char('e'));
        press(app, KeyCode::Tab);
        press(app, KeyCode::Enter);
        press(app, KeyCode::Char('1'));
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        settle(app, &mut fixture.events).await;
        assert_eq!(app.tasks[0].status, "NEEDS-ACTION");
        press(app, KeyCode::Char('v'));
        assert_eq!(
            app.pim_rows().len(),
            1,
            "Reopened tasks must return to the current list"
        );
        finish(fixture).await;
    }
}

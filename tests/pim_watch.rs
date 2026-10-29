use mailtui::{
    app::{App, Event, Overlay, View, start_worker},
    config::{CalendarConfig, CalendarsConfig, ContactsConfig, PimCollection},
    pim_watch::Changes,
    storage::DemoStore,
};
use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};

async fn changed(changes: &mut Changes) {
    tokio::time::timeout(Duration::from_secs(5), changes.events.recv())
        .await
        .expect("Missing filesystem notification")
        .expect("Watcher stopped")
        .unwrap();
}

#[tokio::test]
async fn native_notifications_cover_atomic_saves_deletion_and_directory_replacement_without_read_loops()
 {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("calendar");
    fs::create_dir(&directory).unwrap();
    let mut changes = Changes::start(vec![directory.clone()]).unwrap();
    fs::write(root.path().join("unrelated"), "ignore").unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(350), changes.events.recv())
            .await
            .is_err()
    );
    let file = directory.join("event.ics");
    fs::write(&file, "initial").unwrap();
    changed(&mut changes).await;
    assert_eq!(fs::read_to_string(&file).unwrap(), "initial");
    assert!(
        tokio::time::timeout(Duration::from_millis(350), changes.events.recv())
            .await
            .is_err(),
        "Reads must not trigger another reload"
    );
    let temporary = directory.join(".download.tmp");
    fs::write(&temporary, "updated").unwrap();
    fs::rename(temporary, &file).unwrap();
    changed(&mut changes).await;
    assert_eq!(fs::read_to_string(&file).unwrap(), "updated");
    fs::remove_file(&file).unwrap();
    changed(&mut changes).await;
    fs::remove_dir(&directory).unwrap();
    fs::create_dir(&directory).unwrap();
    changed(&mut changes).await;
    fs::write(&file, "after directory replacement").unwrap();
    changed(&mut changes).await;
    assert_eq!(
        fs::read_to_string(file).unwrap(),
        "after directory replacement"
    );
}

#[tokio::test]
async fn missing_collection_is_watched_when_its_parent_and_directory_are_created() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("new-parent/calendar");
    let mut changes = Changes::start(vec![directory.clone()]).unwrap();
    fs::create_dir_all(&directory).unwrap();
    changed(&mut changes).await;
    fs::write(directory.join("new.ics"), "created later").unwrap();
    changed(&mut changes).await;
}

fn collection(root: &Path, id: &str) -> PimCollection {
    let path = root.join(id);
    fs::create_dir(&path).unwrap();
    PimCollection {
        id: id.into(),
        label: id.into(),
        path,
    }
}
async fn settle(app: &mut App, events: &mut tokio::sync::mpsc::Receiver<Event>) {
    while app.pim_loading || app.pim_pending {
        app.event(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
}

#[tokio::test]
async fn external_pim_updates_are_deferred_during_a_form_and_reloaded_when_it_closes() {
    let root = tempfile::tempdir().unwrap();
    let calendar = collection(root.path(), "calendar");
    let tasks = collection(root.path(), "tasks");
    let contacts = collection(root.path(), "contacts");
    let event = calendar.path.join("event.ics");
    let todo = tasks.path.join("task.ics");
    let card = contacts.path.join("contact.vcf");
    fs::write(&event, "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:event\nSUMMARY:Before\nDTSTART:20991029\nEND:VEVENT\nEND:VCALENDAR").unwrap();
    fs::write(&todo, "BEGIN:VCALENDAR\nBEGIN:VTODO\nUID:task\nSUMMARY:Task\nSTATUS:NEEDS-ACTION\nEND:VTODO\nEND:VCALENDAR").unwrap();
    fs::write(
        &card,
        "BEGIN:VCARD\nVERSION:3.0\nUID:contact\nFN:Before\nEND:VCARD",
    )
    .unwrap();
    let mut changes = Changes::start(vec![
        calendar.path.clone(),
        tasks.path.clone(),
        contacts.path.clone(),
    ])
    .unwrap();
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "no-color".into(), current, tx);
    app.configure_pim(
        CalendarConfig {
            enabled: true,
            collections: vec![calendar],
        },
        ContactsConfig {
            enabled: true,
            books: vec![contacts],
        },
    );
    app.configure_tasks_and_sync(
        &CalendarsConfig::default(),
        &CalendarsConfig {
            enabled: true,
            collections: vec![tasks],
            ..Default::default()
        },
    );
    settle(&mut app, &mut events).await;
    app.view = View::Tasks;
    app.key(crossterm::event::KeyCode::Char('c').into());
    assert!(matches!(app.overlay, Some(Overlay::TaskForm(_))));
    let temporary = event.with_extension("tmp");
    fs::write(
        &temporary,
        fs::read_to_string(&event)
            .unwrap()
            .replace("Before", "After"),
    )
    .unwrap();
    fs::rename(temporary, &event).unwrap();
    fs::write(
        &todo,
        fs::read_to_string(&todo)
            .unwrap()
            .replace("NEEDS-ACTION", "COMPLETED"),
    )
    .unwrap();
    fs::write(
        &card,
        fs::read_to_string(&card)
            .unwrap()
            .replace("Before", "After"),
    )
    .unwrap();
    changed(&mut changes).await;
    app.pim_files_changed();
    assert!(!app.pim_loading);
    assert_eq!(app.tasks[0].status, "NEEDS-ACTION");
    assert_eq!(app.calendar_events[0].summary, "Before");
    app.key(crossterm::event::KeyCode::Esc.into());
    app.refresh_pim_if_idle();
    settle(&mut app, &mut events).await;
    assert_eq!(app.tasks[0].status, "COMPLETED");
    assert_eq!(app.calendar_events[0].summary, "After");
    assert_eq!(app.contacts[0].name, "After");
    drop(changes);
    drop(app);
    drop(events);
    worker.await.unwrap();
}

#[tokio::test]
async fn clock_refresh_uses_cached_ics_without_reading_the_directory() {
    let root = tempfile::tempdir().unwrap();
    let calendar = collection(root.path(), "calendar");
    let file = calendar.path.join("series.ics");
    let now = chrono::Utc::now();
    let stamp = |date: chrono::DateTime<chrono::Utc>| date.format("%Y%m%dT%H%M%SZ").to_string();
    let start = now + chrono::Duration::hours(1);
    let end = now + chrono::Duration::hours(2);
    fs::write(&file, format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:series\nSUMMARY:Series\nDTSTART:{}\nDTEND:{}\nRRULE:FREQ=DAILY;COUNT=2\nEND:VEVENT\nEND:VCALENDAR", stamp(start), stamp(end))).unwrap();
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "no-color".into(), current, tx);
    app.configure_pim(
        CalendarConfig {
            enabled: true,
            collections: vec![calendar],
        },
        ContactsConfig::default(),
    );
    settle(&mut app, &mut events).await;
    app.view = View::Calendar;
    app.refresh_pim_clock(now);
    assert!(!app.pim_loading, "No boundary crossed: no recalculation");
    fs::remove_file(&file).unwrap();
    app.refresh_pim_clock(now + chrono::Duration::hours(3));
    settle(&mut app, &mut events).await;
    assert_eq!(
        app.calendar_events.len(),
        1,
        "Clock updates must use cached documents, even if the file disappeared"
    );
    assert_eq!(
        app.calendar_event_dates(&app.calendar_events[0]).0,
        stamp(start + chrono::Duration::days(1))
    );
    app.refresh_pim_clock(now + chrono::Duration::days(3));
    settle(&mut app, &mut events).await;
    assert!(app.pim_rows().is_empty());
    assert_eq!(app.calendar_events.len(), 1);
    app.reload_pim();
    settle(&mut app, &mut events).await;
    assert!(
        app.calendar_events.is_empty(),
        "An actual file reload does notice the deleted file"
    );
    drop(app);
    drop(events);
    worker.await.unwrap();
}

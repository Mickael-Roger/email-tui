use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mailtui::{
    app::{App, Event, Overlay, View, start_worker},
    config::{CalendarConfig, ContactsConfig, PimCollection},
    pim,
    storage::DemoStore,
};
use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};

fn collection(root: &Path, id: &str) -> PimCollection {
    let path = root.join(id);
    fs::create_dir_all(&path).unwrap();
    PimCollection {
        id: id.into(),
        label: id.to_uppercase(),
        path,
    }
}
const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:opaque/uid@example.test\r\nDTSTART;TZID=Europe/Paris:20261021T090000\r\nDTEND;TZID=Europe/Paris:20261021T100000\r\nSUMMARY:Planning\r\nRRULE:FREQ=WEEKLY;BYDAY=WE\r\nEXDATE;TZID=Europe/Paris:20261028T090000\r\nX-PRESERVE:custom\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nDESCRIPTION:Alarm\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:opaque/uid@example.test\r\nRECURRENCE-ID;TZID=Europe/Paris:20261104T090000\r\nDTSTART;TZID=Europe/Paris:20261104T110000\r\nSUMMARY:Exception\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[test]
fn calendar_edits_keep_original_filename_timezones_alarms_and_recurrence_exceptions() {
    let root = tempfile::tempdir().unwrap();
    let a = collection(root.path(), "a");
    let b = collection(root.path(), "b");
    let original = a.path.join("server-selected-name.ics");
    fs::write(&original, ICS).unwrap();
    fs::write(b.path.join("same-uid.ics"), ICS).unwrap();
    fs::write(a.path.join("ignore.vcf"), "not a calendar").unwrap();
    fs::write(a.path.join("broken.ics"), "BEGIN:VCALENDAR\ninvalid").unwrap();
    let initial = pim::load(&[a.clone(), b.clone()], &[]);
    assert_eq!(initial.events.len(), 2);
    assert_eq!(initial.warnings.len(), 1);
    let mut entry = initial
        .events
        .into_iter()
        .find(|e| e.file == original)
        .unwrap();
    assert_eq!(entry.timezone, "Europe/Paris");
    assert_eq!(entry.description, "");
    entry.summary = "Réunion, architecture; détails".into();
    entry.description = "First line\nSecond line".into();
    pim::write_event(&a, &entry).unwrap();
    let raw = fs::read_to_string(&original).unwrap();
    for needle in [
        "BEGIN:VTIMEZONE",
        "DTSTART;TZID=Europe/Paris:",
        "BEGIN:VALARM",
        "RRULE:FREQ=WEEKLY;BYDAY=WE",
        "EXDATE;TZID=Europe/Paris:",
        "RECURRENCE-ID;TZID=Europe/Paris:",
        "SUMMARY:Exception",
        "X-PRESERVE:custom",
    ] {
        assert!(raw.contains(needle), "Lost {needle}");
    }
    assert_eq!(
        fs::read_to_string(b.path.join("same-uid.ics")).unwrap(),
        ICS
    );
    assert_eq!(fs::read_dir(&a.path).unwrap().count(), 3);
    let reloaded = pim::load(&[a], &[]);
    assert_eq!(reloaded.events[0].summary, entry.summary);
    assert_eq!(reloaded.events[0].description, entry.description);
    // A stale editor must not overwrite a synchronizer's update.
    assert!(pim::write_event(&b, &entry).is_err());
    assert!(
        pim::write_event(&collection(root.path(), "a"), &entry)
            .unwrap_err()
            .to_string()
            .contains("changed on disk")
    );
}

#[test]
fn vcard_edits_preserve_secondary_typed_properties_and_unicode_folding() {
    let root = tempfile::tempdir().unwrap();
    let book = collection(root.path(), "friends");
    let path = book.path.join("arbitrary.vcf");
    fs::write(&path,"BEGIN:VCARD\r\nVERSION:4.0\r\nUID:urn:uuid:abc\r\nFN:Élodie\r\nN:Martin;Élodie;;;\r\nEMAIL;TYPE=work:a@example.test\r\nEMAIL;TYPE=home:b@example.test\r\nTEL;TYPE=cell;VALUE=uri:tel:+33123456789\r\nORG:Team;Department\r\nNOTE:Unfolded\r\n  text\r\nX-CUSTOM:untouched\r\nEND:VCARD\r\n").unwrap();
    let mut contact = pim::load(&[], std::slice::from_ref(&book))
        .contacts
        .remove(0);
    assert_eq!(contact.email, "a@example.test");
    contact.name = "é中".repeat(60);
    contact.email = "new@example.test".into();
    pim::write_contact(&book, &contact).unwrap();
    let updated = fs::read_to_string(&path).unwrap();
    assert!(updated.lines().all(|line| line.len() <= 75));
    for needle in [
        "EMAIL;TYPE=work:new@example.test",
        "EMAIL;TYPE=home:b@example.test",
        "TEL;TYPE=cell;VALUE=uri:tel:+33123456789",
        "N:Martin;Élodie;;;",
        "NOTE:Unfolded text",
        "X-CUSTOM:untouched",
    ] {
        assert!(updated.contains(needle), "Lost {needle}");
    }
    assert_eq!(
        pim::load(&[], std::slice::from_ref(&book)).contacts[0].name,
        contact.name
    );
    let delete = pim::Change::Delete {
        file: path.clone(),
        source: contact.source,
    };
    assert!(pim::execute(&[], std::slice::from_ref(&book), delete).is_err());
    assert!(path.exists());
    let refreshed = pim::load(&[], std::slice::from_ref(&book))
        .contacts
        .remove(0);
    pim::execute(
        &[],
        &[book],
        pim::Change::Delete {
            file: path.clone(),
            source: refreshed.source,
        },
    )
    .unwrap();
    assert!(!path.exists());
}

async fn settle(app: &mut App, events: &mut tokio::sync::mpsc::Receiver<Event>) {
    while app.pim_loading || app.pim_pending || app.busy {
        app.event(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
}
fn key(app: &mut App, code: KeyCode) {
    app.key(code.into());
}
fn save(app: &mut App) {
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
}
fn text(app: &mut App, value: &str) {
    app.paste(value);
    key(app, KeyCode::Tab);
}
fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| mailtui::tui::draw(f, app, false))
        .unwrap();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn resource_views_create_edit_filter_reload_and_delete_in_selected_collections() {
    let root = tempfile::tempdir().unwrap();
    let work = collection(root.path(), "work");
    let home = collection(root.path(), "home");
    let team = collection(root.path(), "team");
    let friends = collection(root.path(), "friends");
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "no-color".into(), current, tx);
    key(&mut app, KeyCode::Char('1'));
    assert_eq!(app.view, View::Messages);
    key(&mut app, KeyCode::Char('2'));
    assert_eq!(app.view, View::Messages);
    assert!(!render(&mut app, 100, 30).contains("<1>"));
    app.configure_pim(
        CalendarConfig {
            enabled: true,
            collections: vec![work.clone(), home.clone()],
        },
        ContactsConfig {
            enabled: true,
            books: vec![team.clone(), friends.clone()],
        },
    );
    settle(&mut app, &mut events).await;
    key(&mut app, KeyCode::Char('1'));
    settle(&mut app, &mut events).await;
    assert_eq!(app.view, View::Calendar);
    key(&mut app, KeyCode::Char('c'));
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Tab);
    text(&mut app, "Holiday");
    text(&mut app, "20260230");
    text(&mut app, "20261023");
    text(&mut app, "Paris");
    text(&mut app, "Two lines\nof notes");
    text(&mut app, "FREQ=YEARLY;COUNT=2");
    save(&mut app);
    settle(&mut app, &mut events).await;
    assert!(matches!(
        &app.overlay,
        Some(Overlay::PimForm { error: Some(_), .. })
    ));
    assert!(app.calendar_events.is_empty());
    // Correct the invalid start without reconstructing the form.
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    for _ in 0..8 {
        key(&mut app, KeyCode::Delete);
    }
    app.paste("20261021");
    save(&mut app);
    settle(&mut app, &mut events).await;
    assert!(app.overlay.is_none());
    assert_eq!(app.calendar_events.len(), 1);
    assert_eq!(app.calendar_events[0].collection, "home");
    assert_eq!(app.calendar_events[0].description, "Two lines\nof notes");
    assert_eq!(fs::read_dir(&work.path).unwrap().count(), 0);
    let event_file = app.calendar_events[0].file.clone();
    let original_uid = app.calendar_events[0].uid.clone();
    key(&mut app, KeyCode::Char('e'));
    app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    for _ in 0..7 {
        key(&mut app, KeyCode::Delete);
    }
    app.paste("Vacation");
    save(&mut app);
    settle(&mut app, &mut events).await;
    assert_eq!(app.calendar_events[0].uid, original_uid);
    assert_eq!(app.calendar_events[0].file, event_file);
    assert_eq!(app.calendar_events[0].summary, "Vacation");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(&app.overlay,Some(Overlay::PimDetails(s)) if s.contains("FREQ=YEARLY")));
    key(&mut app, KeyCode::Esc);
    // External synchronization changes use the same reload as directory notifications.
    let source = fs::read_to_string(&event_file)
        .unwrap()
        .replace("SUMMARY:Vacation", "SUMMARY:Synced");
    fs::write(&event_file, source).unwrap();
    app.reload_pim();
    settle(&mut app, &mut events).await;
    assert_eq!(app.calendar_events[0].summary, "Synced");
    key(&mut app, KeyCode::Char('2'));
    settle(&mut app, &mut events).await;
    key(&mut app, KeyCode::Char('c'));
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Tab);
    text(&mut app, "Ada");
    text(&mut app, "ada@example.test");
    text(&mut app, "+33123");
    text(&mut app, "Analytical");
    save(&mut app);
    settle(&mut app, &mut events).await;
    assert_eq!(app.contacts.len(), 1);
    assert_eq!(app.contacts[0].book, "friends");
    assert_eq!(fs::read_dir(&team.path).unwrap().count(), 0);
    key(&mut app, KeyCode::Char('e'));
    app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    app.paste("Dr. ");
    save(&mut app);
    settle(&mut app, &mut events).await;
    assert_eq!(app.contacts[0].name, "Dr. Ada");
    if let Ok(python) = std::env::var("MAILTUI_PIM_PYTHON") {
        let output = std::process::Command::new(python)
            .arg("-c")
            .arg(
                r#"
from pathlib import Path
from datetime import date
from icalendar import Calendar
import sys, vobject
calendar = Calendar.from_ical(Path(sys.argv[1]).read_bytes())
event = calendar.walk('VEVENT')[0]
assert not event.errors, event.errors
assert event['DTSTART'].dt == date(2026, 10, 21)
assert event['DTEND'].dt == date(2026, 10, 23)
assert event['RRULE']['FREQ'] == ['YEARLY']
assert event['SUMMARY'] == 'Synced'
card = vobject.readOne(Path(sys.argv[2]).read_text())
assert card.fn.value == 'Dr. Ada'
assert card.email.value == 'ada@example.test'
assert card.validate()
"#,
            )
            .arg(&event_file)
            .arg(&app.contacts[0].file)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "External format validation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    key(&mut app, KeyCode::Char('/'));
    app.paste("missing");
    key(&mut app, KeyCode::Enter);
    assert!(app.pim_rows().is_empty());
    key(&mut app, KeyCode::Char('e'));
    assert!(app.overlay.is_none());
    key(&mut app, KeyCode::Esc);
    for view in [View::Calendar, View::Contacts] {
        app.view = view;
        app.row = app
            .pim_tree_rows()
            .iter()
            .position(|row| matches!(row, mailtui::app::PimRow::Entry(_)))
            .unwrap();
        for (w, h) in [(20, 6), (40, 12), (80, 24), (120, 40)] {
            render(&mut app, w, h);
            key(&mut app, KeyCode::Char('e'));
            render(&mut app, w, h);
            key(&mut app, KeyCode::Esc);
        }
    }
    let contact_file = app.contacts[0].file.clone();
    app.row = 0;
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    key(&mut app, KeyCode::Enter);
    assert!(contact_file.exists());
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    key(&mut app, KeyCode::Char('y'));
    key(&mut app, KeyCode::Enter);
    settle(&mut app, &mut events).await;
    assert!(!contact_file.exists());
    assert!(event_file.exists());
    key(&mut app, KeyCode::Char('0'));
    settle(&mut app, &mut events).await;
    assert_eq!(app.view, View::Messages);
    assert_eq!(app.scope.account, None);
    drop(app);
    drop(events);
    worker.await.unwrap();
}

#[test]
fn calendar_parent_discovers_collections_and_keeps_explicit_labels() {
    let root = tempfile::tempdir().unwrap();
    let calendars = root.path().join("calendars");
    let work = collection(&calendars, "work");
    let personal = collection(&calendars, "personal");
    let empty = collection(&calendars, "empty");
    collection(&calendars, ".hidden");
    fs::write(calendars.join("metadata"), "ignored").unwrap();
    fs::create_dir(personal.path.join("nested")).unwrap();
    fs::write(work.path.join("event.ics"), ICS).unwrap();
    fs::write(personal.path.join("event.ics"), ICS).unwrap();
    fs::write(personal.path.join("nested/ignored.ics"), ICS).unwrap();
    let path = root.path().join("config.toml");
    fs::write(
        &path,
        format!("[calendars]\nenabled = true\npath = {:?}\n", calendars),
    )
    .unwrap();
    let config = mailtui::config::Config::load(Some(path.clone())).unwrap();
    assert!(config.calendar.enabled);
    assert_eq!(
        config
            .calendar
            .collections
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["empty", "personal", "work"]
    );
    assert_eq!(config.calendar.collections[0].path, empty.path);
    assert_eq!(config.calendar.collections[1].label, "personal");
    let snapshot = pim::load(&config.calendar.collections, &[]);
    assert_eq!(snapshot.events.len(), 2);
    assert!(snapshot.warnings.is_empty());

    fs::write(&path, format!(
        "[calendars]\nenabled = true\npath = {:?}\n[[calendar.collections]]\nid = 'office'\nlabel = 'Travail'\npath = {:?}\n",
        calendars, work.path
    )).unwrap();
    let config = mailtui::config::Config::load(Some(path)).unwrap();
    assert_eq!(config.calendar.collections.len(), 3);
    assert_eq!(config.calendar.collections[0].id, "office");
    assert_eq!(config.calendar.collections[0].label, "Travail");
}

#[test]
fn calendar_parent_validates_paths_collisions_and_disabled_discovery() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    let load = |text: String| {
        fs::write(&path, text).unwrap();
        mailtui::config::Config::load(Some(path.clone()))
    };
    assert!(load("[calendars]\nenabled = true\npath = 'relative'".into()).is_err());
    let calendars = root.path().join("calendars");
    assert!(
        load(format!(
            "[calendars]\nenabled = true\npath = {:?}",
            calendars
        ))
        .is_err()
    );
    let disabled = load(format!(
        "[calendars]\nenabled = false\npath = {:?}",
        calendars
    ))
    .unwrap();
    assert!(!disabled.calendar.enabled);
    assert!(disabled.calendar.collections.is_empty());
    fs::create_dir(&calendars).unwrap();
    assert!(
        load(format!(
            "[calendars]\nenabled = true\npath = {:?}",
            calendars
        ))
        .is_err()
    );
    collection(&calendars, "work");
    let error = load(format!(
        "[calendars]\nenabled = true\npath = {:?}\n[[calendar.collections]]\nid = 'work'\nlabel = 'Other'\npath = {:?}",
        calendars, root.path().join("other")
    )).unwrap_err();
    assert!(error.to_string().contains("IDs unique"));
}

#[test]
fn optional_configuration_and_date_validation_are_strict() {
    use mailtui::config::Config;
    assert!(
        toml::from_str::<Config>("[calendar]\nenabled=true")
            .unwrap()
            .validate()
            .is_err()
    );
    assert!(
        toml::from_str::<Config>("[contacts]\nenabled=true")
            .unwrap()
            .validate()
            .is_err()
    );
    let config:Config=toml::from_str("[calendar]\nenabled=true\n[[calendar.collections]]\nid='work'\nlabel='Work'\npath='/tmp/calendar'\n[contacts]\nenabled=false").unwrap();
    assert!(config.validate().is_ok());
    let mut duplicate = config.clone();
    duplicate
        .calendar
        .collections
        .push(config.calendar.collections[0].clone());
    assert!(duplicate.validate().is_err());
    let mut relative = config.clone();
    relative.calendar.collections[0].path = "relative/path".into();
    assert!(relative.validate().is_err());
    let example: Config = toml::from_str(include_str!("../config.example.toml")).unwrap();
    assert!(example.validate().is_ok());
    for date in [
        "20261021",
        "20240229",
        "20261021T091500Z",
        "20261021T091500",
    ] {
        assert!(pim::valid_date(date));
    }
    for date in [
        "20260229",
        "20261301",
        "20260431",
        "20261021Z",
        "20261021T250000",
        "20261021\r\nX:bad",
    ] {
        assert!(!pim::valid_date(date));
    }
    assert!(pim::valid_date(&pim::utc_now()));
}

#[tokio::test]
async fn confirmed_pim_write_survives_mail_navigation_and_preserves_a_newer_composer() {
    let root = tempfile::tempdir().unwrap();
    let calendar = collection(root.path(), "calendar");
    let file = calendar.path.join("original.ics");
    fs::write(&file, ICS).unwrap();
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
    key(&mut app, KeyCode::Char('1'));
    settle(&mut app, &mut events).await;
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Down);
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    key(&mut app, KeyCode::Char('y'));
    key(&mut app, KeyCode::Enter);
    assert!(app.pim_pending);
    app.accounts = vec![mailtui::domain::Account {
        id: "work".into(),
        label: "Work".into(),
        email: "work@example.test".into(),
    }];
    key(&mut app, KeyCode::Char('0'));
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.overlay, Some(Overlay::Compose { .. })));
    settle(&mut app, &mut events).await;
    assert!(!file.exists());
    assert!(matches!(app.overlay, Some(Overlay::Compose { .. })));
    drop(app);
    drop(events);
    worker.await.unwrap();
}

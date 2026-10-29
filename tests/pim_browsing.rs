use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mailtui::{
    app::{App, Event, Overlay, PimRow, PimRowKey, View, start_worker},
    config::{CalendarConfig, CalendarsConfig, ContactsConfig, PimCollection},
    pim,
    storage::DemoStore,
};
use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};

fn event(start: &str, end: &str, rule: &str) -> pim::Event {
    pim::Event {
        collection: "work".into(),
        collection_label: "Work".into(),
        uid: "event".into(),
        summary: "Meeting".into(),
        start: start.into(),
        end: end.into(),
        recurrence: rule.into(),
        timezone: String::new(),
        description: String::new(),
        location: String::new(),
        file: "/calendar/event.ics".into(),
        source: String::new(),
    }
}

#[test]
fn recurring_rows_project_the_next_dates_and_keep_series_metadata_unchanged() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-29T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut daily = event("20261020T100000Z", "20261020T110000Z", "FREQ=DAILY");
    let next = pim::event_occurrence(&daily, now).unwrap();
    assert_eq!(
        (&*next.start, &*next.end),
        ("20261030T100000Z", "20261030T110000Z")
    );
    assert!(next.upcoming);
    assert_eq!(
        daily.start, "20261020T100000Z",
        "Projection must not edit the series master"
    );
    let ongoing = pim::event_occurrence(&daily, now - chrono::Duration::minutes(90)).unwrap();
    assert_eq!(ongoing.start, "20261029T100000Z");
    assert!(ongoing.is_ongoing(now - chrono::Duration::minutes(90)));
    assert!(!ongoing.is_ongoing(now));
    daily.source =
        "BEGIN:VEVENT\nEXDATE:20261030T100000Z\nRDATE:20261029T150000Z\nEND:VEVENT".into();
    let next = pim::event_occurrence(&daily, now).unwrap();
    assert_eq!(
        (&*next.start, &*next.end),
        ("20261029T150000Z", "20261029T160000Z")
    );
    daily.source = "BEGIN:VEVENT\nEXDATE:20261030T100000Z\nEND:VEVENT".into();
    assert_eq!(
        pim::event_occurrence(&daily, now).unwrap().start,
        "20261031T100000Z"
    );

    let mut zoned = event("20261020T090000", "20261020T100000", "FREQ=DAILY");
    zoned.timezone = "Europe/Paris".into();
    let next = pim::event_occurrence(&zoned, now).unwrap();
    assert_eq!(
        (&*next.start, &*next.end),
        ("20261030T090000", "20261030T100000")
    );
    assert_eq!(
        next.starts_at.to_rfc3339(),
        "2026-10-30T08:00:00+00:00",
        "The occurrence must use its own DST offset"
    );
    let all_day = event("20261020", "20261022", "FREQ=WEEKLY");
    let next = pim::event_occurrence(&all_day, now).unwrap();
    assert_eq!((&*next.start, &*next.end), ("20261103", "20261105"));
    let finished = event("20261020T100000Z", "20261020T110000Z", "FREQ=DAILY;COUNT=2");
    let last = pim::event_occurrence(&finished, now).unwrap();
    assert!(!last.upcoming);
    assert_eq!(
        (&*last.start, &*last.end),
        ("20261021T100000Z", "20261021T110000Z")
    );
}

#[test]
fn moved_and_cancelled_exceptions_replace_their_original_occurrence() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-29T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut daily = event("20261020T100000Z", "20261020T110000Z", "FREQ=DAILY");
    daily.source = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:event\nDTSTART:20261020T100000Z\nDTEND:20261020T110000Z\nRRULE:FREQ=DAILY\nEND:VEVENT\nBEGIN:VEVENT\nUID:event\nRECURRENCE-ID:20261030T100000Z\nDTSTART:20261030T150000Z\nDTEND:20261030T163000Z\nEND:VEVENT\nEND:VCALENDAR".into();
    let next = pim::event_occurrence(&daily, now).unwrap();
    assert_eq!(
        (&*next.start, &*next.end),
        ("20261030T150000Z", "20261030T163000Z")
    );
    daily.source = daily.source.replace(
        "DTSTART:20261030T150000Z",
        "STATUS:CANCELLED\nDTSTART:20261030T150000Z",
    );
    let next = pim::event_occurrence(&daily, now).unwrap();
    assert_eq!(
        (&*next.start, &*next.end),
        ("20261031T100000Z", "20261031T110000Z")
    );
}

#[test]
fn event_visibility_accounts_for_ongoing_all_day_timezones_and_finished_series() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-29T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let visible = |start, end, rule| pim::event_is_upcoming(&event(start, end, rule), now);
    assert!(!visible("20261028", "", ""));
    assert!(visible("20261029", "", ""));
    assert!(
        !visible("20261028", "20261029", ""),
        "All-day end is exclusive"
    );
    assert!(visible("20261028", "20261030", ""));
    assert!(!visible("20261029T100000Z", "20261029T110000Z", ""));
    assert!(visible("20261029T110000Z", "20261029T130000Z", ""));
    assert!(visible("20261030T100000Z", "", ""));
    assert!(!visible(
        "20261029T110000Z",
        "20261029T120000Z",
        "FREQ=DAILY;COUNT=1"
    ));
    assert!(visible(
        "20200101T100000Z",
        "20200101T110000Z",
        "FREQ=YEARLY"
    ));
    assert!(!visible(
        "20200101T100000Z",
        "20200101T110000Z",
        "FREQ=YEARLY;COUNT=2"
    ));
    assert!(!visible(
        "20200101T100000Z",
        "20200101T110000Z",
        "FREQ=YEARLY;UNTIL=20250101T100000Z"
    ));
    let mut zoned = event("20261029T123000", "20261029T133000", "");
    zoned.timezone = "Europe/Paris".into();
    assert!(pim::event_is_upcoming(&zoned, now));
    zoned.end = "20261029T130000".into();
    assert!(!pim::event_is_upcoming(&zoned, now));
    let mut duration = event("20261029T110000Z", "", "");
    duration.source = "BEGIN:VEVENT\nDURATION:PT2H\nEND:VEVENT".into();
    assert!(pim::event_is_upcoming(&duration, now));
    let mut excluded = event("20300101T100000Z", "20300101T110000Z", "FREQ=DAILY;COUNT=1");
    excluded.source = "BEGIN:VEVENT\nEXDATE:20300101T100000Z\nEND:VEVENT".into();
    assert!(!pim::event_is_upcoming(&excluded, now));
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
fn write_event(collection: &PimCollection, name: &str, start: &str) -> std::path::PathBuf {
    let path = collection.path.join(format!("{name}.ics"));
    fs::write(&path, format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:{name}\nSUMMARY:{name}\nDTSTART:{start}\nDESCRIPTION:Details of {name}\nEND:VEVENT\nEND:VCALENDAR\n")).unwrap();
    path
}
fn write_task(collection: &PimCollection, name: &str, status: &str) {
    fs::write(collection.path.join(format!("{name}.ics")), format!("BEGIN:VCALENDAR\nBEGIN:VTODO\nUID:{name}\nSUMMARY:{name}\nSTATUS:{status}\nEND:VTODO\nEND:VCALENDAR\n")).unwrap();
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
fn key(app: &mut App, code: KeyCode) {
    app.key(code.into());
}

fn rendered_buffer(app: &mut App) -> ratatui::buffer::Buffer {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| mailtui::tui::draw(frame, app, false))
        .unwrap();
    terminal.backend().buffer().clone()
}
fn screen_text(buffer: &ratatui::buffer::Buffer) -> String {
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}
fn text_cell<'a>(buffer: &'a ratatui::buffer::Buffer, text: &str) -> &'a ratatui::buffer::Cell {
    let chars: Vec<_> = text.chars().collect();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width.saturating_sub(chars.len() as u16) {
            if chars
                .iter()
                .enumerate()
                .all(|(i, c)| buffer[(x + i as u16, y)].symbol() == c.to_string())
            {
                return &buffer[(x, y)];
            }
        }
    }
    panic!("Missing rendered text: {text}");
}

#[tokio::test]
async fn pim_colors_distinguish_priority_and_running_events_without_location_column() {
    use ratatui::style::{Color, Modifier};
    let root = tempfile::tempdir().unwrap();
    let calendar = collection(root.path(), "Calendar");
    let list = collection(root.path(), "Tasks");
    let now = chrono::Utc::now();
    let stamp = |date: chrono::DateTime<chrono::Utc>| date.format("%Y%m%dT%H%M%SZ").to_string();
    let running = calendar.path.join("running.ics");
    fs::write(&running, format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:running\nSUMMARY:Running\nDTSTART:{}\nDTEND:{}\nLOCATION:Hidden room\nEND:VEVENT\nEND:VCALENDAR", stamp(now - chrono::Duration::minutes(30)), stamp(now + chrono::Duration::minutes(30)))).unwrap();
    write_event(&calendar, "Future", &stamp(now + chrono::Duration::days(1)));
    for (name, priority, status) in [
        ("Urgent", "1", "NEEDS-ACTION"),
        ("Medium", "5", "NEEDS-ACTION"),
        ("Low", "9", "NEEDS-ACTION"),
        ("Unspecified", "0", "NEEDS-ACTION"),
        ("Finished", "1", "COMPLETED"),
    ] {
        fs::write(list.path.join(format!("{name}.ics")), format!("BEGIN:VCALENDAR\nBEGIN:VTODO\nUID:{name}\nSUMMARY:{name}\nSTATUS:{status}\nPRIORITY:{priority}\nDUE:20261121\nEND:VTODO\nEND:VCALENDAR")).unwrap();
    }
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "k9s-dark".into(), current, tx);
    app.configure_pim(
        CalendarConfig {
            enabled: true,
            collections: vec![calendar.clone()],
        },
        ContactsConfig::default(),
    );
    app.configure_tasks_and_sync(
        &CalendarsConfig::default(),
        &CalendarsConfig {
            enabled: true,
            collections: vec![list.clone()],
            ..Default::default()
        },
    );
    settle(&mut app, &mut events).await;
    app.expanded_calendars.insert(calendar.id.clone());
    app.expanded_task_lists.insert(list.id.clone());
    app.show_completed_tasks = true;
    for theme in ["k9s-dark", "high-contrast", "no-color"] {
        app.theme = theme.into();
        app.view = View::Calendar;
        app.row = 0;
        let buffer = rendered_buffer(&mut app);
        let text = screen_text(&buffer);
        assert!(!text.contains("LOCATION"));
        assert!(!text.contains("Hidden room"));
        let running_cell = text_cell(&buffer, "Running");
        let future_cell = text_cell(&buffer, "Future");
        assert!(
            running_cell
                .modifier
                .contains(Modifier::UNDERLINED | Modifier::BOLD)
        );
        assert!(!future_cell.modifier.contains(Modifier::UNDERLINED));
        assert_eq!(
            running_cell.bg,
            match theme {
                "k9s-dark" => Color::Indexed(22),
                "high-contrast" => Color::Green,
                _ => Color::Reset,
            }
        );
        app.view = View::Tasks;
        let buffer = rendered_buffer(&mut app);
        let text = screen_text(&buffer);
        assert!(text.contains("To do"));
        assert!(!text.contains("NEEDS-ACTION"));
        assert!(text.contains("2026-11-21"));
        assert!(!text.contains("all day"));
        assert!(
            text_cell(&buffer, "Urgent")
                .modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            text_cell(&buffer, "Medium")
                .modifier
                .contains(Modifier::ITALIC)
        );
        assert!(text_cell(&buffer, "Low").modifier.contains(Modifier::DIM));
        let expected = match theme {
            "k9s-dark" => [
                Color::Red,
                Color::Yellow,
                Color::Green,
                Color::White,
                Color::DarkGray,
            ],
            "high-contrast" => [
                Color::LightRed,
                Color::Yellow,
                Color::LightGreen,
                Color::White,
                Color::White,
            ],
            _ => [Color::Reset; 5],
        };
        for (name, color) in ["Urgent", "Medium", "Low", "Unspecified", "Finished"]
            .iter()
            .zip(expected)
        {
            assert_eq!(text_cell(&buffer, name).fg, color, "{theme}: {name}");
        }
    }
    app.view = View::Calendar;
    assert!(app.focus_pim_key(&PimRowKey::Entry {
        collection: calendar.id,
        file: running
    }));
    key(&mut app, KeyCode::Enter);
    assert!(
        matches!(&app.overlay, Some(Overlay::PimDetails(text)) if text.contains("Location: Hidden room"))
    );
    drop(app);
    drop(events);
    worker.await.unwrap();
}

#[tokio::test]
async fn calendar_sorts_by_displayed_occurrences_and_only_shows_recurrence_in_details() {
    let root = tempfile::tempdir().unwrap();
    let calendar = collection(root.path(), "Work");
    let now = chrono::Utc::now();
    let stamp = |date: chrono::DateTime<chrono::Utc>| date.format("%Y%m%dT%H%M%SZ").to_string();
    let master = now - chrono::Duration::days(10);
    let next = now + chrono::Duration::days(4);
    let file = calendar.path.join("series.ics");
    fs::write(&file, format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:series\nSUMMARY:Recurring\nDTSTART:{}\nDTEND:{}\nRRULE:FREQ=DAILY;INTERVAL=14\nEND:VEVENT\nEND:VCALENDAR\n", stamp(master), stamp(master + chrono::Duration::hours(1)))).unwrap();
    write_event(
        &calendar,
        "Tomorrow",
        &stamp(now + chrono::Duration::days(1)),
    );
    write_event(&calendar, "Later", &stamp(now + chrono::Duration::days(5)));
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "no-color".into(), current, tx);
    app.configure_pim(
        CalendarConfig {
            enabled: true,
            collections: vec![calendar.clone()],
        },
        ContactsConfig::default(),
    );
    settle(&mut app, &mut events).await;
    app.view = View::Calendar;
    let summaries: Vec<_> = app
        .pim_rows()
        .iter()
        .map(|index| app.calendar_events[*index].summary.as_str())
        .collect();
    assert_eq!(summaries, ["Tomorrow", "Recurring", "Later"]);
    let series = app
        .calendar_events
        .iter()
        .find(|e| e.uid == "series")
        .unwrap();
    assert_eq!(series.start, stamp(master));
    assert_eq!(
        app.calendar_event_dates(series),
        (
            stamp(next).as_str(),
            stamp(next + chrono::Duration::hours(1)).as_str()
        )
    );
    key(&mut app, KeyCode::Enter);
    assert!(app.focus_pim_key(&PimRowKey::Entry {
        collection: calendar.id.clone(),
        file: file.clone()
    }));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 28)).unwrap();
    terminal
        .draw(|frame| mailtui::tui::draw(frame, &mut app, false))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("↻ Recurring"));
    assert!(screen.contains(&pim::display_date(&stamp(next))));
    assert!(screen.contains(&pim::display_date(&stamp(
        next + chrono::Duration::hours(1)
    ))));
    assert!(!screen.contains(&pim::display_date(&stamp(master))));
    assert!(!screen.contains("INTERVAL=14"));
    key(&mut app, KeyCode::Enter);
    assert!(
        matches!(&app.overlay, Some(Overlay::PimDetails(text)) if text.contains("FREQ=DAILY;INTERVAL=14"))
    );
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .contains(&format!("DTSTART:{}", stamp(master)))
    );
    key(&mut app, KeyCode::Esc);
    for file in [
        &file,
        &calendar.path.join("Tomorrow.ics"),
        &calendar.path.join("Later.ics"),
    ] {
        fs::remove_file(file).unwrap();
    }
    app.reload_pim();
    settle(&mut app, &mut events).await;
    assert!(
        app.pim_tree_rows().is_empty(),
        "Deleting the last event removes the calendar branch"
    );
    key(&mut app, KeyCode::Char('c'));
    assert!(
        matches!(app.overlay, Some(Overlay::PimForm { collection: 0, .. })),
        "An empty calendar remains a valid destination for new events"
    );
    drop(app);
    drop(events);
    worker.await.unwrap();
}

#[tokio::test]
async fn calendars_and_tasks_fold_independently_filter_history_and_preserve_focus_on_reload() {
    let root = tempfile::tempdir().unwrap();
    let a = collection(root.path(), "Calendar A");
    let b = collection(root.path(), "Calendar B");
    let empty = collection(root.path(), "Empty calendar");
    let past_only = collection(root.path(), "Past calendar");
    let task_a = collection(root.path(), "List A");
    let task_b = collection(root.path(), "List B");
    let future = write_event(&a, "Future A", "20991029T100000Z");
    let past = write_event(&a, "Past A", "20001029T100000Z");
    write_event(&b, "Future B", "20991029T100000Z");
    write_event(&past_only, "Only past", "20001029T100000Z");
    write_task(&task_a, "Active A", "NEEDS-ACTION");
    write_task(&task_a, "Done A", "COMPLETED");
    write_task(&task_b, "Active B", "IN-PROCESS");
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "no-color".into(), current, tx);
    app.configure_pim(
        CalendarConfig {
            enabled: true,
            collections: vec![a.clone(), b.clone(), empty.clone(), past_only.clone()],
        },
        ContactsConfig::default(),
    );
    app.configure_tasks_and_sync(
        &CalendarsConfig::default(),
        &CalendarsConfig {
            enabled: true,
            collections: vec![task_a.clone(), task_b.clone()],
            ..Default::default()
        },
    );
    settle(&mut app, &mut events).await;
    app.view = View::Calendar;
    assert_eq!(app.pim_rows().len(), 2);
    app.filter = empty.id.clone();
    assert!(
        app.pim_tree_rows().is_empty(),
        "An empty calendar stays hidden even when its name matches the filter"
    );
    app.filter.clear();
    assert!(matches!(
        app.pim_tree_rows().as_slice(),
        [
            PimRow::Collection {
                expanded: false,
                count: 1,
                ..
            },
            PimRow::Collection {
                expanded: false,
                count: 1,
                ..
            }
        ]
    ));
    key(&mut app, KeyCode::Char('e'));
    assert!(
        app.overlay.is_none(),
        "Collection rows are not editable events"
    );
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert!(
        app.overlay.is_none(),
        "Collection rows are not deletable events"
    );
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Down);
    let focused = app.pim_focused_key().unwrap();
    assert_eq!(
        focused,
        PimRowKey::Entry {
            collection: a.id.clone(),
            file: future.clone()
        }
    );
    key(&mut app, KeyCode::Enter);
    assert!(
        matches!(&app.overlay, Some(Overlay::PimDetails(text)) if text.contains("Details of Future A") && !text.contains(&future.to_string_lossy().to_string()) && !text.contains(".ics"))
    );
    key(&mut app, KeyCode::Esc);
    write_event(&a, "Earlier future", "20981029T100000Z");
    app.reload_pim();
    settle(&mut app, &mut events).await;
    assert_eq!(app.pim_focused_key(), Some(focused));
    assert!(app.focus_pim_key(&PimRowKey::Collection(b.id.clone())));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.expanded_calendars.len(), 2);
    assert!(app.focus_pim_key(&PimRowKey::Collection(a.id.clone())));
    key(&mut app, KeyCode::Enter);
    assert!(!app.expanded_calendars.contains(&a.id));
    assert!(app.expanded_calendars.contains(&b.id));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('v'));
    assert!(app.show_past_events);
    assert!(
        app.pim_tree_rows()
            .iter()
            .any(|row| matches!(row, PimRow::Collection { index: 3, .. })),
        "History restores calendars with past entries"
    );
    assert!(
        !app.pim_tree_rows()
            .iter()
            .any(|row| matches!(row, PimRow::Collection { index: 2, .. })),
        "Truly empty calendars remain hidden in history mode"
    );
    assert!(app.focus_pim_key(&PimRowKey::Entry {
        collection: a.id.clone(),
        file: past
    }));
    key(&mut app, KeyCode::Char('v'));
    assert_eq!(
        app.pim_focused_key(),
        Some(PimRowKey::Collection(a.id.clone()))
    );
    app.filter = "Future B".into();
    assert!(matches!(
        app.pim_tree_rows().as_slice(),
        [PimRow::Collection { index: 1, .. }, PimRow::Entry(_)]
    ));
    app.filter.clear();
    assert!(app.focus_pim_key(&PimRowKey::Collection(b.id.clone())));
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(
        app.overlay,
        Some(Overlay::PimForm { collection: 1, .. })
    ));
    key(&mut app, KeyCode::Esc);

    app.view = View::Tasks;
    app.row = 0;
    assert_eq!(app.pim_rows().len(), 2);
    assert_eq!(app.pim_tree_rows().len(), 2, "Task lists start folded too");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('v'));
    assert!(app.show_completed_tasks);
    assert!(!app.show_past_events, "History switches are independent");
    assert_eq!(app.pim_rows().len(), 3);
    key(&mut app, KeyCode::Char('v'));
    assert_eq!(app.pim_rows().len(), 2);
    for view in [View::Calendar, View::Tasks] {
        app.view = view;
        app.row = 0;
        for (w, h) in [(40, 12), (80, 24), (120, 40)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|f| mailtui::tui::draw(f, &mut app, false))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(!text.contains("Pending: 0"));
            assert!(text.contains("<v>"));
            assert!(text.contains(if view == View::Calendar {
                "Show past"
            } else {
                "Show closed"
            }));
        }
    }
    drop(app);
    drop(events);
    worker.await.unwrap();
}

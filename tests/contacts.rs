use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mailtui::{
    app::{App, Overlay, PimRow, View, start_worker_with_index},
    config::{CalendarConfig, Config, ContactsConfig},
    domain::{Message, MessageId},
    pim_sync::Target,
    search::SearchIndex,
    storage::DemoStore,
};
use std::{
    fs,
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};

fn indexed(index: &SearchIndex, seq: i64, sender: &str, recipients: &str) {
    index
        .insert(
            &Message {
                id: MessageId {
                    account: "personal".into(),
                    folder: "inbox".into(),
                    seq,
                },
                sender: sender.into(),
                subject: "Example".into(),
                date: format!("2026-10-{seq:02}"),
                seen: false,
                flagged: false,
                attachment: false,
                size: 0,
            },
            "body-only@example.test must not become a contact",
            recipients,
        )
        .unwrap();
}

#[test]
fn sqlite_contacts_decode_groups_deduplicate_and_follow_deleted_mail() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.sqlite3");
    let index = SearchIndex::open(&path).unwrap();
    indexed(
        &index,
        1,
        "ada@example.test",
        "Team: bob@example.test, carol@example.test;",
    );
    indexed(
        &index,
        2,
        "\"Lovelace, Ada\" <ADA@example.test>",
        "invalid, Bob <bob@example.test>",
    );
    indexed(&index, 3, "=?UTF-8?Q?Ren=C3=A9?= <rene@example.test>", "");
    let reader = SearchIndex::reader(&path).unwrap();
    let contacts = reader.contacts().unwrap();
    assert_eq!(contacts.len(), 4);
    assert!(contacts.iter().all(|c| c.is_mail()));
    assert_eq!(
        contacts
            .iter()
            .find(|c| c.uid == "ada@example.test")
            .unwrap()
            .name,
        "Lovelace, Ada"
    );
    assert_eq!(
        contacts
            .iter()
            .find(|c| c.uid == "rene@example.test")
            .unwrap()
            .name,
        "René"
    );
    assert_eq!(
        contacts
            .iter()
            .find(|c| c.uid == "bob@example.test")
            .unwrap()
            .name,
        "Bob"
    );
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("DELETE FROM mail", [])
        .unwrap();
    assert!(reader.contacts().unwrap().is_empty());
}

#[test]
fn contact_books_discover_and_validate_sync_configuration() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("friends")).unwrap();
    fs::create_dir(root.path().join(".hidden")).unwrap();
    let path = root.path().join("config.toml");
    fs::write(
        &path,
        format!(
            "[contacts]\nenabled=true\npath={:?}\nsync_command=['vdirsyncer','sync','contacts']\n",
            root.path()
        ),
    )
    .unwrap();
    let mut config = Config::load(Some(path)).unwrap();
    assert_eq!(config.contacts.books.len(), 1);
    assert_eq!(config.contacts.books[0].id, "friends");
    assert_eq!(
        config.contacts.sync_config().sync_command,
        ["vdirsyncer", "sync", "contacts"]
    );
    config.contacts.sync_timeout_seconds = 0;
    assert!(config.validate().is_err());
    config.contacts.sync_timeout_seconds = 120;
    config.contacts.sync_command = vec![" ".into()];
    assert!(config.validate().is_err());
}

async fn settle(app: &mut App, events: &mut tokio::sync::mpsc::Receiver<mailtui::app::Event>) {
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
async fn contacts_view_groups_sources_guards_mail_writes_and_synchronizes_carddav_edits() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.sqlite3");
    let index = SearchIndex::open(&path).unwrap();
    indexed(&index, 1, "Ada <ada@example.test>", "bob@example.test");
    let book = root.path().join("friends");
    fs::create_dir(&book).unwrap();
    let work_book = root.path().join("work");
    fs::create_dir(&work_book).unwrap();
    // A correspondent may also exist in an address book: the sources stay independent.
    fs::write(book.join("ada.vcf"), "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:ada\r\nFN:Ada\r\nEMAIL:ada@example.test\r\nEND:VCARD\r\n").unwrap();
    let contacts = ContactsConfig {
        enabled: true,
        books: vec![
            mailtui::config::PimCollection {
                id: "friends".into(),
                label: "Friends".into(),
                path: book,
            },
            mailtui::config::PimCollection {
                id: "work".into(),
                label: "Work contacts".into(),
                path: work_book,
            },
        ],
        sync_command: vec!["contact-sync".into()],
        ..Default::default()
    };
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) =
        start_worker_with_index(Box::new(DemoStore::new()), current.clone(), Some(path));
    let mut app = App::new(true, "no-color".into(), current, tx);
    app.configure_pim(CalendarConfig::default(), contacts);
    app.key(KeyCode::Char('2').into());
    settle(&mut app, &mut events).await;
    assert_eq!(app.view, View::Contacts);
    assert_eq!(app.contacts.len(), 3);
    assert_eq!(app.pim_tree_rows().len(), 2);
    assert!(matches!(
        app.pim_tree_rows()[0],
        PimRow::Collection {
            count: 2,
            expanded: false,
            ..
        }
    ));
    app.key(KeyCode::Enter.into());
    assert_eq!(app.pim_tree_rows().len(), 4);
    app.row = 3;
    app.key(KeyCode::Enter.into());
    assert_eq!(app.pim_tree_rows().len(), 5);
    app.row = 1;
    app.key(KeyCode::Char('e').into());
    assert!(app.overlay.is_none());
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert!(app.overlay.is_none());
    app.key(KeyCode::Enter.into());
    assert!(matches!(&app.overlay, Some(Overlay::PimDetails(body)) if body.contains("read-only")));
    app.key(KeyCode::Esc.into());
    app.row = 4;
    app.key(KeyCode::Char('e').into());
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app, &mut events).await;
    assert!(app.sync_requests.remove(&Target::Contacts));
    app.execute("sync");
    assert!(app.sync_requests.remove(&Target::Contacts));
    // A sync completion reloads changes downloaded by the external command.
    fs::write(root.path().join("friends/downloaded.vcf"), "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:new\r\nFN:Downloaded\r\nEMAIL:downloaded@example.test\r\nADR;TYPE=WORK:PO;Suite B;Server street;Berlin;Berlin;10115;Germany\r\nEND:VCARD\r\n").unwrap();
    app.sync_completed(Target::Contacts, None);
    settle(&mut app, &mut events).await;
    assert_eq!(app.contacts.len(), 4);
    let card_index = app.contacts.iter().position(|c| c.uid == "new").unwrap();
    app.row = app
        .pim_tree_rows()
        .iter()
        .position(|row| *row == PimRow::Entry(card_index))
        .unwrap();
    app.key(KeyCode::Char('e').into());
    assert!(
        matches!(&app.overlay, Some(Overlay::PimForm { fields, .. }) if fields[10].text == "Berlin")
    );
    app.key(KeyCode::PageDown.into());
    app.key(KeyCode::PageDown.into());
    app.key(KeyCode::Tab.into());
    app.key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
    app.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    // Replace the imported city while leaving the hidden PO box and extended address intact.
    app.paste("Hamburg");
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app, &mut events).await;
    assert!(app.sync_requests.remove(&Target::Contacts));
    assert_eq!(app.contacts[card_index].addresses[1].city, "Hamburg");
    assert_eq!(app.contacts[card_index].addresses[1].po_box, "PO");
    assert_eq!(app.contacts[card_index].addresses[1].extended, "Suite B");
    app.filter = "Hamburg".into();
    assert_eq!(app.pim_rows(), [card_index]);
    app.filter.clear();
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| mailtui::tui::draw(frame, &mut app, false))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let screen: String = (0..24)
        .flat_map(|y| (0..80).map(move |x| buffer[(x, y)].symbol()))
        .collect();
    assert!(screen.contains("Delete “Downloaded”?"));
    assert!(screen.contains("OK") && screen.contains("Cancel"));
    app.key(KeyCode::Enter.into());
    assert!(root.path().join("friends/downloaded.vcf").exists());
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    app.key(KeyCode::Tab.into());
    assert!(matches!(
        app.overlay,
        Some(Overlay::PimDelete { confirm: true, .. })
    ));
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut events).await;
    assert!(!root.path().join("friends/downloaded.vcf").exists());
    assert!(app.sync_requests.remove(&Target::Contacts));
    indexed(&index, 2, "New <new@example.test>", "");
    app.index_changed();
    settle(&mut app, &mut events).await;
    assert_eq!(app.contacts.len(), 4);
    app.key(KeyCode::Char('c').into());
    app.key(KeyCode::Enter.into());
    assert!(matches!(
        app.overlay,
        Some(Overlay::PimForm {
            book_menu: Some(0),
            ..
        })
    ));
    app.key(KeyCode::Down.into());
    app.key(KeyCode::Enter.into());
    assert!(matches!(
        app.overlay,
        Some(Overlay::PimForm {
            collection: 1,
            book_menu: None,
            ..
        })
    ));
    app.key(KeyCode::Tab.into());
    app.paste("Created contact");
    for (width, height) in [(20, 6), (40, 12), (80, 24), (100, 30)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| mailtui::tui::draw(frame, &mut app, false))
            .unwrap();
        if width == 100 {
            let buffer = terminal.backend().buffer();
            let screen = (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n");
            insta::assert_snapshot!("carddav_contact_editor", screen);
        }
    }
    // Populate each Zimbra-compatible address type before saving.
    for (kind, (street, city, region, postal, country)) in [
        (
            "12 rue des Lilas\nAppartement 4",
            "Paris",
            "Île-de-France",
            "75001",
            "France",
        ),
        (
            "25 avenue du Port",
            "Lyon",
            "Auvergne-Rhône-Alpes",
            "69002",
            "France",
        ),
        ("5 Main Street", "Montréal", "Québec", "H2X 1Y4", "Canada"),
    ]
    .into_iter()
    .enumerate()
    {
        app.key(KeyCode::PageDown.into());
        for value in [street, city, region, postal, country] {
            if let Some((first, second)) = value.split_once('\n') {
                app.paste(first);
                app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
                app.paste(second);
            } else {
                app.paste(value);
            }
            app.key(KeyCode::Tab.into());
        }
        // Return from Save to the last field before switching sections.
        app.key(KeyCode::BackTab.into());
        if kind == 0 {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 32)).unwrap();
            terminal
                .draw(|frame| mailtui::tui::draw(frame, &mut app, false))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let screen = (0..32)
                .map(|y| {
                    (0..100)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n");
            insta::assert_snapshot!("carddav_postal_address", screen);
        }
    }
    // Focus Save after the last address field.
    app.key(KeyCode::Tab.into());
    assert!(
        matches!(&app.overlay, Some(Overlay::PimForm { field, fields, .. }) if *field == fields.len() + 1)
    );
    // Buttons are focusable controls, not editable inputs.
    app.paste("ignored on Save");
    app.key(KeyCode::Enter.into());
    settle(&mut app, &mut events).await;
    assert!(app.sync_requests.remove(&Target::Contacts));
    assert_eq!(app.contacts.len(), 5);
    let created = app
        .contacts
        .iter()
        .find(|c| c.name == "Created contact")
        .unwrap();
    assert_eq!(
        created.addresses[0].street,
        "12 rue des Lilas\nAppartement 4"
    );
    assert_eq!(created.addresses[1].city, "Lyon");
    assert_eq!(created.addresses[2].postal_code, "H2X 1Y4");
    app.key(KeyCode::Enter.into());
    assert!(
        matches!(&app.overlay, Some(Overlay::PimDetails(body)) if body.contains("Personal address") && body.contains("Lyon") && body.contains("Canada"))
    );
    app.key(KeyCode::Esc.into());
    assert_eq!(
        app.contacts
            .iter()
            .find(|c| c.name == "Created contact")
            .unwrap()
            .book,
        "work"
    );
    app.key(KeyCode::Char('c').into());
    app.key(KeyCode::BackTab.into());
    app.paste("ignored on Cancel");
    app.key(KeyCode::Enter.into());
    assert!(app.overlay.is_none());
    assert_eq!(app.contacts.len(), 5);
    app.sync_completed(Target::Contacts, Some("server unavailable".into()));
    settle(&mut app, &mut events).await;
    assert!(app.contacts.iter().any(|c| c.name == "Created contact"));
    drop(app);
    drop(events);
    worker.await.unwrap();
}

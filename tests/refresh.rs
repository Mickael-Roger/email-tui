use io_pimdir::{
    change::PimdirWriteOp,
    client::{PimdirSourceStore, PimdirStore},
    object::PimdirObject,
    placement::{PimdirFlags, PimdirHandle, PimdirLevel, PimdirPlacement, PimdirStatus},
    summary::mail,
};
use mailtui::{
    app::{App, Event, start_worker},
    config::AccountConfig,
    storage::{Changes, PimdirStore as Adapter, change_tokens},
    tree::RowKey,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

fn arrive(owner: &mut PimdirSourceStore, number: i64, subject: &str) {
    let raw = format!(
        "From: sender@example.com\r\nTo: me@example.com\r\nSubject: {subject}\r\nMessage-ID: <{number}@example.com>\r\nDate: Sun, 18 Oct 2026 {:02}:14:00 +0000\r\n\r\nDownloaded message {number}.\r\n",
        9 + number
    );
    let hash = owner.hash(raw.as_bytes());
    let derived = mail::derive(raw.as_bytes());
    owner
        .write(vec![
            PimdirWriteOp::StoreObject {
                object: PimdirObject {
                    hash: hash.clone(),
                    size: raw.len(),
                },
                body: Some(raw.into_bytes()),
            },
            PimdirWriteOp::UpsertPlacement(PimdirPlacement {
                collection: "INBOX".into(),
                handle: PimdirHandle(number.to_string()),
                link_id: Some(derived.link_id),
                sort_key: derived.sort_key,
                summary: derived.summary,
                object: Some(hash),
                level: PimdirLevel::Full,
                flags: PimdirFlags::default(),
                status: PimdirStatus::Clean,
                base: None,
                conflict_revision: None,
                conflict_object: None,
                origin: None,
            }),
        ])
        .unwrap();
}

async fn settle(app: &mut App, events: &mut tokio::sync::mpsc::Receiver<Event>) {
    while app.busy {
        app.event(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
}

#[tokio::test]
async fn external_wal_downloads_refresh_each_account_without_restarting_or_losing_focus() {
    let work = tempfile::tempdir().unwrap();
    let personal = tempfile::tempdir().unwrap();
    let configs: Vec<_> = [("work", work.path()), ("personal", personal.path())]
        .into_iter()
        .map(|(id, path)| AccountConfig {
            neverest_account: None,
            id: id.into(),
            label: id.into(),
            email: "me@example.com".into(),
            store: path.into(),
        })
        .collect();
    let mut owners: Vec<_> = configs
        .iter()
        .map(|account| {
            let mut owner = PimdirStore::open(&account.store)
                .unwrap()
                .for_source("fixture");
            owner.ensure_collection("INBOX", "message/rfc822").unwrap();
            owner.set_collection_role("INBOX", Some("inbox")).unwrap();
            arrive(&mut owner, 1, "Already loaded");
            owner
        })
        .collect();
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) =
        start_worker(Box::new(Adapter::new(configs.clone())), current.clone());
    let mut app = App::new(false, "k9s-dark".into(), current.clone(), tx);
    app.reload(false);
    settle(&mut app, &mut events).await;
    assert_eq!(app.tree_rows().len(), 2);
    assert_eq!(app.collapsed_accounts.len(), 2);
    // Enter during an in-flight refresh must still load the newly opened folder.
    app.reload(false);
    app.focus_key(&RowKey::Account("work".into()));
    app.key(crossterm::event::KeyCode::Enter.into());
    app.focus_key(&RowKey::Folder(("work".into(), "INBOX".into())));
    app.key(crossterm::event::KeyCode::Enter.into());
    settle(&mut app, &mut events).await;
    assert_eq!(app.messages.len(), 1);
    app.focus_key(&RowKey::Folder(("work".into(), "INBOX".into())));
    app.key(crossterm::event::KeyCode::Enter.into());
    app.focus_key(&RowKey::Account("work".into()));
    app.key(crossterm::event::KeyCode::Enter.into());
    app.switch_account(Some("work".into()));
    settle(&mut app, &mut events).await;
    assert!(app.collapsed_accounts.contains("work")); // Account folds have no effect in single-account scope.
    assert!(app.focus_key(&RowKey::Folder(("work".into(), "INBOX".into()))));
    app.key(crossterm::event::KeyCode::Enter.into());
    settle(&mut app, &mut events).await;
    assert_eq!(app.messages.len(), 1);
    let focus = RowKey::Message(app.messages[0].id.clone());
    app.focus_key(&focus);
    let mut changes = Changes::watch(configs.clone());
    let before = tokio::time::timeout(Duration::from_secs(5), changes.events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before, change_tokens(&configs));
    arrive(&mut owners[0], 2, "New work mail downloaded by Neverest");
    arrive(
        &mut owners[0],
        3,
        "Another new work mail downloaded by Neverest",
    );
    let after = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let tokens = changes.events.recv().await.unwrap();
            if tokens != before {
                break tokens;
            }
        }
    })
    .await
    .unwrap();
    assert_ne!(before[0], after[0]);
    assert_eq!(before[1], after[1]);
    app.storage_changed();
    app.overlay = Some(mailtui::app::Overlay::Help);
    app.refresh_if_idle();
    assert!(app.refresh_requested && !app.busy);
    app.overlay = None;
    app.refresh_if_idle();
    settle(&mut app, &mut events).await;
    assert_eq!(app.messages.len(), 3);
    assert!(
        app.messages
            .iter()
            .any(|mail| mail.subject.starts_with("New work"))
    );
    assert_eq!(app.focused_key(), Some(focus));
    // A second account changes while hidden; its counts and its later-opened list must both update.
    arrive(
        &mut owners[1],
        2,
        "New personal mail downloaded by Neverest",
    );
    let second = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let tokens = changes.events.recv().await.unwrap();
            if tokens != after {
                break tokens;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(after[0], second[0]);
    assert_ne!(after[1], second[1]);
    app.storage_changed();
    app.refresh_if_idle();
    settle(&mut app, &mut events).await;
    assert_eq!(
        app.folders
            .iter()
            .find(|folder| folder.account == "personal")
            .unwrap()
            .total,
        2
    );
    app.switch_account(Some("personal".into()));
    settle(&mut app, &mut events).await;
    app.focus_key(&RowKey::Folder(("personal".into(), "INBOX".into())));
    app.key(crossterm::event::KeyCode::Enter.into());
    settle(&mut app, &mut events).await;
    assert!(
        app.messages
            .iter()
            .any(|mail| mail.subject.starts_with("New personal"))
    );
    current.fetch_add(1, Ordering::Relaxed);
    drop(app);
    drop(events);
    worker.await.unwrap();
}

#[test]
fn persistent_local_index_reopens_and_tracks_new_changed_deleted_and_late_hydrated_mail() {
    use std::io::Write;
    use std::sync::atomic::AtomicBool;
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store");
    let mut owner = PimdirStore::open(&store).unwrap().for_source("fixture");
    owner.ensure_collection("INBOX", "message/rfc822").unwrap();
    let config = AccountConfig {
        neverest_account: None,
        id: "work".into(),
        label: "Work".into(),
        email: "me@example.com".into(),
        store: store.clone(),
    };
    let stop = AtomicBool::new(false);
    arrive(&mut owner, 1, "Kubernetes planning");
    let path = directory.path().join("mailtui/search.sqlite3");
    let index = mailtui::search::SearchIndex::open(&path).unwrap();
    assert_eq!(
        index
            .refresh_native(std::slice::from_ref(&config), &stop)
            .unwrap(),
        1
    );
    assert_eq!(index.find("downloaded", Some("work")).unwrap().len(), 1); // body, not subject
    assert_eq!(index.find("kbrnts", Some("work")).unwrap().len(), 1); // fuzzy metadata
    assert!(
        index
            .find("downloaded", Some("personal"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        index
            .refresh_native(std::slice::from_ref(&config), &stop)
            .unwrap(),
        0
    );
    drop(index);
    assert_eq!(
        mailtui::search::SearchIndex::reader(&path)
            .unwrap()
            .find("download", None)
            .unwrap()
            .len(),
        1
    );
    let index = mailtui::search::SearchIndex::open(&path).unwrap();
    arrive(&mut owner, 1, "Renamed unicorn");
    index
        .refresh_native(std::slice::from_ref(&config), &stop)
        .unwrap();
    assert!(index.find("kubernetes", None).unwrap().is_empty());
    assert_eq!(index.find("unicorn", None).unwrap().len(), 1);
    arrive(&mut owner, 2, "Second unicorn");
    index
        .refresh_native(std::slice::from_ref(&config), &stop)
        .unwrap();
    assert_eq!(index.find("unicorn", None).unwrap().len(), 2);
    owner
        .ensure_collection("Archive", "message/rfc822")
        .unwrap();
    let moved = index
        .find("unicorn", None)
        .unwrap()
        .into_iter()
        .find(|message| message.subject == "Second unicorn")
        .unwrap();
    io_pimdir::client::producer::PimdirProducer::open(&store, "index-test")
        .unwrap()
        .enqueue(
            "INBOX",
            &io_pimdir::codec::PimdirAction::Move {
                seq: moved.id.seq,
                to: "Archive".into(),
            },
            None,
        )
        .unwrap();
    owner.drain().unwrap();
    index
        .refresh_native(std::slice::from_ref(&config), &stop)
        .unwrap();
    assert!(
        index
            .find("unicorn", None)
            .unwrap()
            .iter()
            .any(|message| message.subject == "Second unicorn" && message.id.folder == "Archive")
    );
    owner
        .write(vec![PimdirWriteOp::DropPlacement {
            collection: "INBOX".into(),
            handle: PimdirHandle("1".into()),
            reason: io_pimdir::change::PimdirDropReason::Deleted,
        }])
        .unwrap();
    index
        .refresh_native(std::slice::from_ref(&config), &stop)
        .unwrap();
    let hits = index.find("unicorn", None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].subject, "Second unicorn");
    // A known blob may arrive after its metadata, without changing the item cursor.
    arrive(&mut owner, 3, "Hydration demo");
    let reader = io_pimdir::client::reader::PimdirReader::open(&store).unwrap();
    let item = reader
        .list_summaries("INBOX", None, 100)
        .unwrap()
        .into_iter()
        .find(|item| item.seq != hits[0].id.seq)
        .unwrap();
    let hash = item.object.unwrap();
    let bytes = reader.blobs().get(&hash).unwrap().unwrap();
    std::fs::remove_file(reader.blobs().path(&hash)).unwrap();
    index
        .refresh_native(std::slice::from_ref(&config), &stop)
        .unwrap();
    let before = reader.change_cursor().unwrap();
    let mut writer = reader.blobs().writer().unwrap();
    writer.write_all(&bytes).unwrap();
    writer.commit(&hash).unwrap();
    assert_eq!(before, reader.change_cursor().unwrap());
    assert!(
        index
            .refresh_native(std::slice::from_ref(&config), &stop)
            .unwrap()
            > 0
    );
    assert!(
        index
            .find("downloaded", None)
            .unwrap()
            .iter()
            .any(|message| message.subject == "Hydration demo")
    );
    owner.delete_collection("INBOX").unwrap();
    index
        .refresh_native(std::slice::from_ref(&config), &stop)
        .unwrap();
    assert!(
        index
            .find("", None)
            .unwrap()
            .iter()
            .all(|message| message.id.folder == "Archive")
    );
    owner.delete_collection("Archive").unwrap();
    index.refresh_native(&[config], &stop).unwrap();
    assert!(index.find("", None).unwrap().is_empty());
}

#[tokio::test]
async fn external_arrivals_update_an_open_search_and_then_the_tree_without_manual_refresh() {
    use mailtui::{
        app::{Overlay, start_worker_with_index},
        index::Maintenance,
        search::SearchIndex,
    };
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store");
    let mut owner = PimdirStore::open(&store).unwrap().for_source("fixture");
    owner.ensure_collection("INBOX", "message/rfc822").unwrap();
    arrive(&mut owner, 1, "Already loaded");
    let configs = vec![AccountConfig {
        neverest_account: None,
        id: "work".into(),
        label: "Work".into(),
        email: "me@example.com".into(),
        store,
    }];
    let path = directory.path().join("search.sqlite3");
    SearchIndex::open(&path)
        .unwrap()
        .refresh_native(&configs, &std::sync::atomic::AtomicBool::new(false))
        .unwrap();
    let mut maintenance = Maintenance::start(path.clone(), configs.clone());
    let mut changes = Changes::watch(configs.clone());
    let before = tokio::time::timeout(Duration::from_secs(5), changes.events.recv())
        .await
        .unwrap()
        .unwrap();
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut events, worker) =
        start_worker_with_index(Box::new(Adapter::new(configs)), current.clone(), Some(path));
    let mut app = App::new(false, "k9s-dark".into(), current.clone(), tx);
    app.reload(false);
    settle(&mut app, &mut events).await;
    app.key(crossterm::event::KeyCode::Char('/').into());
    app.paste("carillon");
    settle(&mut app, &mut events).await;
    assert!(matches!(&app.overlay, Some(Overlay::Find { messages, .. }) if messages.is_empty()));

    // An external owner commits two arrivals while the user's query stays open.
    arrive(&mut owner, 2, "Carillon alpha");
    arrive(&mut owner, 3, "Carillon beta");
    tokio::time::timeout(Duration::from_secs(5), async {
        while changes.events.recv().await.unwrap() == before {}
    })
    .await
    .unwrap();
    app.storage_changed();
    app.refresh_if_idle();
    assert!(app.refresh_requested && !app.busy);
    assert!(maintenance.request());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), maintenance.events.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            >= 2
    );
    app.index_changed();
    settle(&mut app, &mut events).await;
    assert!(
        matches!(&app.overlay, Some(Overlay::Find { query, messages, loading: false, .. }) if query.text == "carillon" && messages.len() == 2)
    );

    // Closing the popup releases the deferred metadata refresh, without a reload key.
    app.key(crossterm::event::KeyCode::Esc.into());
    app.refresh_if_idle();
    settle(&mut app, &mut events).await;
    assert_eq!(app.folders[0].total, 3);
    assert!(app.collapsed_accounts.contains("work"));
    app.switch_account(Some("work".into()));
    settle(&mut app, &mut events).await;
    assert!(app.focus_key(&RowKey::Folder(("work".into(), "INBOX".into()))));
    app.key(crossterm::event::KeyCode::Enter.into());
    settle(&mut app, &mut events).await;
    assert_eq!(app.messages.len(), 3);
    assert!(
        app.messages
            .iter()
            .any(|message| message.subject == "Carillon alpha")
    );
    assert!(
        app.messages
            .iter()
            .any(|message| message.subject == "Carillon beta")
    );
    current.fetch_add(1, Ordering::Relaxed);
    drop(app);
    drop(events);
    worker.await.unwrap();
}

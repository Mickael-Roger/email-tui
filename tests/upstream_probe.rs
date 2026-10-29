use io_pimdir::{
    change::PimdirWriteOp,
    client::{PimdirStore, reader::PimdirReader},
    collection::PimdirCollectionId,
    object::PimdirObject,
    placement::{PimdirFlags, PimdirHandle, PimdirLevel, PimdirPlacement, PimdirStatus},
    summary::mail,
};

#[test]
fn deleting_mail_resolves_slash_and_dot_trash_folders_in_each_account() {
    use io_pimdir::capability::{MAIL, PimdirCapability, PimdirSupport};
    use mailtui::{
        config::AccountConfig,
        domain::{Mutation, Scope},
        storage::MailStore,
    };
    let root = tempfile::tempdir().unwrap();
    let mut configs = vec![];
    let mut stores = vec![];
    for (account, trash) in [("work", "Trash/"), ("personal", "INBOX.Trash")] {
        let path = root.path().join(account);
        let mut owner = PimdirStore::open(&path)
            .unwrap()
            .for_source("fixture")
            .for_account(account);
        for folder in ["INBOX", trash] {
            owner.ensure_collection(folder, "message/rfc822").unwrap();
            owner.set_collection_account(folder, Some(account)).unwrap();
        }
        owner.set_collection_role("INBOX", Some("inbox")).unwrap();
        owner
            .declare(
                "fixture",
                &MAIL
                    .iter()
                    .map(|name| PimdirCapability {
                        collection: None,
                        name: (*name).into(),
                        support: PimdirSupport::Full,
                        detail: None,
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let raw = b"From: sender@example.com\r\nTo: me@example.com\r\nSubject: Delete me\r\nMessage-ID: <delete@example.com>\r\n\r\nMessage.\r\n";
        let hash = owner.hash(raw);
        let derived = mail::derive(raw);
        owner
            .write(vec![
                PimdirWriteOp::StoreObject {
                    object: PimdirObject {
                        hash: hash.clone(),
                        size: raw.len(),
                    },
                    body: Some(raw.to_vec()),
                },
                PimdirWriteOp::UpsertPlacement(PimdirPlacement {
                    collection: "INBOX".into(),
                    handle: PimdirHandle("1".into()),
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
        configs.push(AccountConfig {
            id: account.into(),
            label: account.into(),
            email: "me@example.com".into(),
            store: path.clone(),
            neverest_account: None,
        });
        stores.push((path, owner, trash));
    }
    let mut adapter = mailtui::storage::PimdirStore::new(configs);
    let ids: Vec<_> = adapter
        .messages(
            &Scope {
                inbox_only: true,
                ..Default::default()
            },
            None,
            10,
        )
        .unwrap()
        .messages
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids.len(), 2);
    adapter.mutate(&ids, &Mutation::Trash).unwrap();
    for (path, mut owner, trash) in stores {
        let reader = PimdirReader::open(path).unwrap();
        let pending = reader.list_pending_actions().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(
            matches!(&pending[0].action, io_pimdir::codec::PimdirAction::Move { to, .. } if to.as_str() == trash)
        );
        owner.drain().unwrap();
        assert_eq!(reader.count_items("INBOX").unwrap(), 0);
        assert_eq!(reader.count_items(trash).unwrap(), 1);
    }
}

#[test]
fn real_pimdir_fixture_read_without_private_sql() {
    let dir = tempfile::tempdir().unwrap();
    let mut owner = PimdirStore::open(dir.path()).unwrap().for_source("fixture");
    owner.ensure_collection("INBOX", "message/rfc822").unwrap();
    let raw = b"From: alice@example.com\r\nTo: me@example.com\r\nSubject: Fixture\r\nMessage-ID: <fixture@example.com>\r\nDate: Sun, 18 Oct 2026 09:14:00 +0000\r\n\r\nHello offline.\r\n";
    let hash = owner.hash(raw);
    let derived = mail::derive(raw);
    owner
        .write(vec![
            PimdirWriteOp::StoreObject {
                object: PimdirObject {
                    hash: hash.clone(),
                    size: raw.len(),
                },
                body: Some(raw.to_vec()),
            },
            PimdirWriteOp::UpsertPlacement(PimdirPlacement {
                collection: PimdirCollectionId("INBOX".into()),
                handle: PimdirHandle("1".into()),
                link_id: Some(derived.link_id),
                sort_key: derived.sort_key,
                summary: derived.summary,
                object: Some(hash.clone()),
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
    // The lock-free reader must coexist with the owner, not acquire its role.
    let reader = PimdirReader::open(dir.path()).unwrap();
    assert_eq!(reader.list_collections().unwrap()[0].id, "INBOX");
    assert_eq!(reader.list_summaries("INBOX", None, 10).unwrap().len(), 1);
    assert_eq!(reader.blobs().get(&hash).unwrap().unwrap(), raw);
    assert_eq!(reader.count_items("INBOX").unwrap(), 1);
    // Exercise the application's concrete adapter, not just upstream handles.
    use mailtui::{
        config::AccountConfig,
        domain::{Mutation, MutationResult, Scope},
        storage::MailStore,
    };
    let mut adapter = mailtui::storage::PimdirStore::new(vec![AccountConfig {
        neverest_account: None,
        id: "fixture".into(),
        label: "Fixture".into(),
        email: "me@example.com".into(),
        store: dir.path().to_path_buf(),
    }]);
    assert_eq!(adapter.accounts()[0].id, "fixture");
    assert_eq!(adapter.folders().unwrap()[0].unread, 1);
    let page = adapter.messages(&Scope::default(), None, 10).unwrap();
    assert_eq!(page.messages[0].subject, "Fixture");
    assert_eq!(adapter.raw(&page.messages[0].id).unwrap(), raw);
    assert!(matches!(
        adapter
            .mutate(&[page.messages[0].id.clone()], &Mutation::Seen(true))
            .unwrap(),
        MutationResult::Queued(_)
    ));
    assert!(
        adapter
            .messages(&Scope::default(), None, 10)
            .unwrap()
            .messages[0]
            .seen
    );
    // Pending overlay is visible to the producer; the owner still has to drain it.
    assert!(adapter.pending().unwrap().is_empty()); // flag writes are not outgoing mail
    assert!(
        !reader
            .get_item("INBOX", page.messages[0].id.seq)
            .unwrap()
            .unwrap()
            .flags
            .contains("\\Seen")
    );
    owner.drain().unwrap();
    assert!(
        reader
            .get_item("INBOX", page.messages[0].id.seq)
            .unwrap()
            .unwrap()
            .flags
            .contains("\\Seen")
    );
    owner = owner.for_account("fixture");
    for (name, role) in [
        ("INBOX", "inbox"),
        ("Archive", "archive"),
        ("Drafts", "drafts"),
        ("Sent", "sent"),
    ] {
        owner.ensure_collection(name, "message/rfc822").unwrap();
        owner.set_collection_account(name, Some("fixture")).unwrap();
        owner.set_collection_role(name, Some(role)).unwrap();
    }
    use io_pimdir::capability::{MAIL, PimdirCapability, PimdirSupport};
    owner
        .declare(
            "fixture",
            &MAIL
                .iter()
                .map(|name| PimdirCapability {
                    collection: None,
                    name: (*name).into(),
                    support: PimdirSupport::Full,
                    detail: None,
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
    adapter
        .mutate(
            &[page.messages[0].id.clone()],
            &Mutation::Move("Archive".into()),
        )
        .unwrap();
    owner.drain().unwrap();
    assert_eq!(reader.count_items("Archive").unwrap(), 1);
    let attachment = dir.path().join("pièce jointe.pdf");
    std::fs::write(&attachment, b"%PDF-1.7\nfixture attachment\n").unwrap();
    let draft = mailtui::domain::Draft {
        account: "fixture".into(),
        to: "alice@example.com, bob@example.com".into(),
        subject: "Unicode · café 日本語".into(),
        body: "Line one\nLine two".into(),
        attachments: vec![attachment],
        reply: None,
    };
    adapter.compose(&draft, false).unwrap();
    owner.drain().unwrap();
    assert_eq!(reader.count_items("Drafts").unwrap(), 1);
    adapter.compose(&draft, true).unwrap();
    adapter
        .create_folder("fixture", "INBOX", Some("INBOX"), "Travel")
        .unwrap();
    let pending = reader.list_pending_actions().unwrap();
    assert_eq!(adapter.pending().unwrap().len(), 1); // submission only, not folder creation
    let submit = pending
        .iter()
        .find_map(|queued| match &queued.action {
            io_pimdir::codec::PimdirAction::Unknown { kind, payload, .. } if kind == "submit" => {
                Some(payload)
            }
            _ => None,
        })
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(submit).unwrap();
    assert_eq!(
        payload["rcpts"],
        serde_json::json!(["alice@example.com", "bob@example.com"])
    );
    assert_eq!(payload["source"], "fixture");
    let bytes = reader
        .blobs()
        .get(&io_pimdir::object::PimdirHash(
            payload["object"].as_str().unwrap().into(),
        ))
        .unwrap()
        .unwrap();
    let mail = mail_parser::MessageParser::default().parse(&bytes).unwrap();
    assert_eq!(mail.subject(), Some("Unicode · café 日本語"));
    assert!(mail.body_text(0).unwrap().contains("Line two"));
    use mail_parser::MimeHeaders;
    let attachments: Vec<_> = mail.attachments().collect();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].attachment_name(), Some("pièce jointe.pdf"));
    assert_eq!(attachments[0].contents(), b"%PDF-1.7\nfixture attachment\n");
    let create = pending
        .iter()
        .find_map(|queued| match &queued.action {
            io_pimdir::codec::PimdirAction::Unknown { kind, payload, .. }
                if kind == "collection-create" =>
            {
                Some(payload)
            }
            _ => None,
        })
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(create).unwrap();
    assert_eq!(payload["parent"], "INBOX");
    assert_eq!(payload["name"], "Travel");
}

use super::*;
struct NoMailboxAccess;
impl MailStore for NoMailboxAccess {
    fn accounts(&self) -> Vec<Account> {
        panic!("Search must use MailTUI's database")
    }
    fn folders(&self) -> Result<Vec<Folder>> {
        panic!("Search must use MailTUI's database")
    }
    fn messages(&self, _: &Scope, _: Option<&Cursor>, _: usize) -> Result<Page> {
        panic!("Search must not scan the mailbox")
    }
    fn raw(&self, _: &MessageId) -> Result<Vec<u8>> {
        panic!("Search must not read raw mail")
    }
    fn mutate(&mut self, _: &[MessageId], _: &Mutation) -> Result<MutationResult> {
        panic!("Search must not mutate")
    }
    fn pending(&self) -> Result<Vec<PendingMail>> {
        panic!("Search must use MailTUI's database")
    }
}
fn turn() -> Turn {
    Turn {
        run: 1,
        account: Some("work".into()),
        anchor: None,
        history: vec![],
        sources: vec![],
    }
}
#[test]
fn mail_search_uses_the_durable_mailtui_database_and_handles_quoted_subjects() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("search.sqlite3");
    let writer = crate::search::SearchIndex::open(&path).unwrap();
    writer
        .fill_store(&crate::storage::DemoStore::new())
        .unwrap();
    writer
        .db
        .execute(
            "UPDATE mail SET subject=?1,sender=?2,body=?3",
            ["Planning (Q4)", "O'Reilly", "l'offre OR local-needle"],
        )
        .unwrap();
    let (sender, _events) = mpsc::channel(32);
    let tools = Tools::new(
        Box::new(NoMailboxAccess),
        Some(path),
        false,
        super::super::pim_tools::PimTools::default(),
        &turn(),
        sender,
    );
    let mut state = tools.state.lock().unwrap();
    for args in [
        json!({"query":"subject:\"Planning (Q4)\" from:O'Reilly l'offre"}),
        json!({"query":"\"OR\"","mode":"structured"}),
        json!({"query":"Planning (Q4)","mode":"literal"}),
        json!({"query":"local-needle"}),
        json!({"query":""}),
        json!({"query":"","mode":"structured"}),
    ] {
        let (found, _) = state.execute("search_mail", &args).unwrap();
        assert_eq!(found["database"], "mailtui");
        assert!(
            !found["sources"].as_array().unwrap().is_empty(),
            "{args}: {found}"
        );
        assert!(
            found["sources"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["message"]["id"]["account"] == "work")
        );
    }
    let (outside, _) = state
        .execute(
            "search_mail",
            &json!({"query":"account:personal local-needle"}),
        )
        .unwrap();
    assert!(outside["sources"].as_array().unwrap().is_empty());
    assert!(
        state
            .execute(
                "search_mail",
                &json!({"query":"subject:Planning OR subject:Other"})
            )
            .is_err()
    );
    writer
        .db
        .execute("UPDATE mail SET subject=?1", ["L'offre (Q4) \"review\""])
        .unwrap();
    state.sources[0].message.subject = "Re: L'offre (Q4) \"review\"".into();
    let (related, _) = state
        .execute("find_related_mail", &json!({"source":1}))
        .unwrap();
    assert!(!related["sources"].as_array().unwrap().is_empty());
    // The cached read-only connection must see subsequent WAL changes.
    writer
        .db
        .execute("UPDATE mail SET body='replacement'", [])
        .unwrap();
    let (gone, _) = state
        .execute("search_mail", &json!({"query":"local-needle"}))
        .unwrap();
    assert!(gone["sources"].as_array().unwrap().is_empty());
}
#[tokio::test]
async fn missing_mailtui_database_returns_an_actionable_error_without_mailbox_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.sqlite3");
    let (sender, _events) = mpsc::channel(32);
    let mut tools = Tools::new(
        Box::new(NoMailboxAccess),
        Some(missing.clone()),
        false,
        super::super::pim_tools::PimTools::default(),
        &turn(),
        sender,
    );
    let error = tools
        .result("search_mail", json!({"query":"subject:test"}))
        .await;
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("MailTUI search database")
    );
    assert!(error["hint"].as_str().unwrap().contains("literal"));
    assert!(!missing.exists());
    let (sender, _events) = mpsc::channel(32);
    let mut tools = Tools::new(
        Box::new(NoMailboxAccess),
        None,
        false,
        super::super::pim_tools::PimTools::default(),
        &turn(),
        sender,
    );
    let error = tools.result("search_mail", json!({"query":"test"})).await;
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("no mailbox/store fallback")
    );
}

#[tokio::test]
async fn attached_uncached_mail_does_not_prevent_searching_mailtuis_database() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("search.sqlite3");
    let index = crate::search::SearchIndex::open(&path).unwrap();
    index.fill_store(&crate::storage::DemoStore::new()).unwrap();
    let mut turn = turn();
    let message = index.find("kubernetes", Some("work")).unwrap().remove(0);
    turn.anchor = Some(message.clone());
    turn.sources.push(Source { number: 1, message });
    let (sender, _events) = mpsc::channel(32);
    let mut tools = Tools::new(
        Box::new(crate::storage::PimdirStore::new(vec![])),
        Some(path),
        false,
        super::super::pim_tools::PimTools::default(),
        &turn,
        sender,
    );
    let context = tools.context(&turn).await.unwrap();
    assert!(context.contains("error"));
    let found = tools
        .result("search_mail", json!({"query":"kubernetes"}))
        .await;
    assert!(found.get("error").is_none());
    assert!(!found["sources"].as_array().unwrap().is_empty());
}

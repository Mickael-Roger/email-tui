use super::*;
fn proposal(tools: &PimTools, name: &str, args: Value) -> Change {
    let (result, proposal) = tools.execute(name, &args).unwrap();
    assert_eq!(result["applied"], false);
    let Some(Proposal::Pim { change, .. }) = proposal else {
        panic!("PIM proposal missing")
    };
    *change
}
#[test]
fn pim_tools_follow_enabled_collections_and_never_accept_filesystem_paths() {
    let mut tools = PimTools::default();
    assert!(tools.specs().is_empty());
    assert!(tools.execute("search_events", &json!({})).is_err());
    assert!(tools.execute("search_contacts", &json!({})).is_err());
    let dir = tempfile::tempdir().unwrap();
    tools.calendars.push(PimCollection {
        id: "work".into(),
        label: "Work".into(),
        path: dir.path().join("work"),
    });
    assert_eq!(tools.specs().len(), 6);
    assert!(
        tools
            .specs()
            .iter()
            .all(|s| !s["name"].as_str().unwrap().contains("contact"))
    );
    assert!(
        tools
            .execute("read_event", &json!({"event":"../../outside.ics"}))
            .is_err()
    );
    assert!(
        tools
            .execute(
                "propose_create_event",
                &json!({"calendar":"missing","summary":"Test","start":"20261021"})
            )
            .is_err()
    );
    assert!(
        tools
            .execute(
                "propose_create_event",
                &json!({"calendar":"work","summary":"Test","start":"20260230"})
            )
            .is_err()
    );
    assert!(!dir.path().join("work").exists());
}
#[test]
fn pim_proposals_cover_create_read_update_and_delete_without_implicit_writes() {
    let dir = tempfile::tempdir().unwrap();
    let calendars: Vec<_> = ["work", "home"]
        .map(|id| PimCollection {
            id: id.into(),
            label: id.into(),
            path: dir.path().join(id),
        })
        .into();
    let books: Vec<_> = ["team", "friends"]
        .map(|id| PimCollection {
            id: id.into(),
            label: id.into(),
            path: dir.path().join(id),
        })
        .into();
    let tools = PimTools {
        calendars: calendars.clone(),
        books: books.clone(),
    };
    assert_eq!(tools.specs().len(), 12);
    let change = proposal(
        &tools,
        "propose_create_event",
        json!({"calendar":"home","summary":"Planning","start":"20261021T090000Z","end":"20261021T100000Z","description":"Discuss\nRoadmap","recurrence":"FREQ=WEEKLY;BYDAY=WE"}),
    );
    assert!(!calendars[1].path.exists());
    pim::execute(&calendars, &books, change).unwrap();
    let change = proposal(
        &tools,
        "propose_create_contact",
        json!({"book":"friends","name":"Ada","email":"ada@example.test","organization":"Team"}),
    );
    assert!(!books[1].path.exists());
    pim::execute(&calendars, &books, change).unwrap();
    let (found, _) = tools
        .execute(
            "search_events",
            &json!({"query":"Planning","calendar":"home","limit":1}),
        )
        .unwrap();
    let event = found["items"][0]["event"].clone();
    assert!(event.is_string());
    assert!(
        tools
            .execute("search_events", &json!({"calendar":"work"}))
            .unwrap()
            .0["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let (read, _) = tools
        .execute("read_event", &json!({"event":event}))
        .unwrap();
    assert_eq!(read["description"], "Discuss\nRoadmap");
    assert!(read.get("file").is_none());
    assert!(read.get("source").is_none());
    let (found, _) = tools
        .execute("search_contacts", &json!({"query":"ada","book":"friends"}))
        .unwrap();
    let contact = found["items"][0]["contact"].clone();
    assert_eq!(
        tools
            .execute("read_contact", &json!({"contact":contact}))
            .unwrap()
            .0["name"],
        "Ada"
    );
    let update = proposal(
        &tools,
        "propose_update_event",
        json!({"event":event,"summary":"Architecture"}),
    );
    let before = pim::load(&calendars, &books).events.remove(0);
    assert_eq!(before.summary, "Planning");
    pim::execute(&calendars, &books, update).unwrap();
    let after = pim::load(&calendars, &books).events.remove(0);
    assert_eq!(after.summary, "Architecture");
    assert_eq!(after.file, before.file);
    assert_eq!(after.start, before.start);
    assert_eq!(after.recurrence, before.recurrence);
    let update = proposal(
        &tools,
        "propose_update_contact",
        json!({"contact":contact,"email":"new@example.test"}),
    );
    pim::execute(&calendars, &books, update).unwrap();
    assert_eq!(
        pim::load(&calendars, &books).contacts[0].organization,
        "Team"
    );
    let delete = proposal(&tools, "propose_delete_contact", json!({"contact":contact}));
    assert_eq!(pim::load(&calendars, &books).contacts.len(), 1);
    pim::execute(&calendars, &books, delete).unwrap();
    let delete = proposal(&tools, "propose_delete_event", json!({"event":event}));
    pim::execute(&calendars, &books, delete).unwrap();
    assert!(pim::load(&calendars, &books).events.is_empty());
    assert!(pim::load(&calendars, &books).contacts.is_empty());
    assert!(!calendars[0].path.exists());
    assert!(!books[0].path.exists());
}

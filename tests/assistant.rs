use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, Response},
    routing::post,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mailtui::{
    app::{App, Overlay, View, start_worker},
    domain::*,
    llm::{self, Api, AuthMethod, Command, Config, MessageText, Provider, Source, Turn, Update},
    storage::{DemoStore, MailStore},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::Duration,
};

fn config() -> Config {
    let mut config = Config {
        enabled: true,
        model: "test-model".into(),
        ..Default::default()
    };
    // An existing, non-secret variable avoids mutating process-global environment.
    config.auth.api_key_env = "PATH".into();
    config
}

#[tokio::test]
async fn calendar_contact_tools_work_on_chat_responses_and_codex_and_apply_through_the_worker() {
    use mailtui::config::{CalendarConfig, ContactsConfig, PimCollection};
    for transport in ["chat", "responses", "codex"] {
        let root = tempfile::tempdir().unwrap();
        let cal_path = root.path().join("cal");
        let book_path = root.path().join("book");
        let calendar = CalendarConfig {
            enabled: true,
            collections: vec![PimCollection {
                id: "cal".into(),
                label: "Calendar".into(),
                path: cal_path.clone(),
            }],
        };
        let contacts = ContactsConfig {
            enabled: true,
            books: vec![PimCollection {
                id: "book".into(),
                label: "Book".into(),
                path: book_path.clone(),
            }],
            ..Default::default()
        };
        let mut cfg = config();
        let server = if transport == "codex" {
            cfg.provider = Provider::Codex;
            cfg.auth.method = AuthMethod::CodexOauth;
            cfg.codex_home = Some(root.path().join("codex"));
            cfg.codex_command = vec![
                "python3".into(),
                format!(
                    "{}/tests/fixtures/codex_mock.py",
                    env!("CARGO_MANIFEST_DIR")
                ),
            ];
            None
        } else {
            let responses = transport == "responses";
            let requests = Arc::new(Mutex::new(0usize));
            let handler = move |State(step): State<Arc<Mutex<usize>>>, Json(body): Json<Value>| async move {
                let round = {
                    let mut step = step.lock().unwrap();
                    let current = *step;
                    *step += 1;
                    current
                };
                let tools = body["tools"].as_array().unwrap();
                for name in [
                    "list_calendars",
                    "list_address_books",
                    "search_events",
                    "search_contacts",
                    "read_event",
                    "read_contact",
                    "propose_create_event",
                    "propose_update_contact",
                    "propose_delete_event",
                ] {
                    assert!(
                        tools.iter().any(|t| if responses {
                            t["name"] == name
                        } else {
                            t["function"]["name"] == name
                        }),
                        "Missing {name}"
                    );
                }
                let calls = [
                    ("calendars", "list_calendars", json!({})),
                    ("books", "list_address_books", json!({})),
                    (
                        "event",
                        "propose_create_event",
                        json!({"calendar":"cal","summary":"LLM meeting","start":"20261201T090000Z","end":"20261201T100000Z"}),
                    ),
                    (
                        "contact",
                        "propose_create_contact",
                        json!({"book":"book","name":"LLM contact","email":"person@example.test"}),
                    ),
                ];
                let data = if round == 0 {
                    if responses {
                        json!({"type":"response.completed","response":{"output":calls.iter().map(|(id,name,args)|json!({"type":"function_call","id":id,"call_id":id,"name":name,"arguments":args.to_string()})).collect::<Vec<_>>()}})
                    } else {
                        json!({"choices":[{"delta":{"tool_calls":calls.iter().enumerate().map(|(i,(id,name,args))|json!({"index":i,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})).collect::<Vec<_>>()} ,"finish_reason":"tool_calls"}]})
                    }
                } else {
                    let messages = body[if responses { "input" } else { "messages" }]
                        .as_array()
                        .unwrap();
                    for (id, _, _) in &calls {
                        let message = messages
                            .iter()
                            .find(|m| {
                                if responses {
                                    m["type"] == "function_call_output" && m["call_id"] == *id
                                } else {
                                    m["role"] == "tool" && m["tool_call_id"] == *id
                                }
                            })
                            .unwrap();
                        let value: Value = serde_json::from_str(
                            message[if responses { "output" } else { "content" }]
                                .as_str()
                                .unwrap(),
                        )
                        .unwrap();
                        assert!(value.get("error").is_none(), "{value}");
                        if *id == "event" || *id == "contact" {
                            assert_eq!(value["applied"], false);
                        }
                    }
                    if responses {
                        json!({"type":"response.completed","response":{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Proposals ready"}]}]}})
                    } else {
                        json!({"choices":[{"delta":{"content":"Proposals ready"},"finish_reason":"stop"}]})
                    }
                };
                Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from(format!("data: {data}\n\ndata: [DONE]\n\n")))
                    .unwrap()
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            cfg.endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
            if responses {
                cfg.api = Api::Responses;
            }
            let router = Router::new()
                .route(
                    if responses {
                        "/v1/responses"
                    } else {
                        "/v1/chat/completions"
                    },
                    post(handler),
                )
                .with_state(requests);
            Some(tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            }))
        };
        let current = Arc::new(AtomicU64::new(0));
        let (tx, mut storage, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
        let mut app = App::new(true, "no-color".into(), current, tx);
        app.llm = cfg.clone();
        app.configure_pim(calendar.clone(), contacts.clone());
        while app.pim_loading {
            app.event(storage.recv().await.unwrap());
        }
        app.execute("ai");
        let run = app.ai.run;
        let mut assistant =
            llm::Assistant::new(cfg, vec![], true, None).with_pim(calendar, contacts);
        if transport == "codex" {
            assistant.request(Command::Login(100));
            assert!(matches!(
                collect(&mut assistant).await.last().unwrap().update,
                Update::Finished
            ));
        }
        assistant.request(Command::Turn(Box::new(turn(run))));
        for event in collect(&mut assistant).await {
            assert!(
                !matches!(event.update, Update::Error(_)),
                "{transport}: {event:?}"
            );
            app.ai_event(event);
        }
        assert_eq!(app.ai.proposals.len(), 2, "{transport}");
        assert!(!cal_path.exists());
        assert!(!book_path.exists());
        for n in 1..=2 {
            app.paste(&format!("/apply {n}"));
            app.key(KeyCode::Enter.into());
            assert!(app.pim_pending);
            while app.pim_pending {
                app.event(
                    tokio::time::timeout(Duration::from_secs(5), storage.recv())
                        .await
                        .unwrap()
                        .unwrap(),
                );
            }
            assert!(app.ai.proposals[n - 1].is_none());
            assert!(matches!(app.overlay, Some(Overlay::Assistant)));
        }
        assert_eq!(app.calendar_events[0].summary, "LLM meeting");
        assert_eq!(app.contacts[0].email, "person@example.test");
        assert!(app.ai.text().contains("operation applied locally"));
        app.paste("/apply 1");
        app.key(KeyCode::Enter.into());
        assert!(!app.pim_pending);
        drop(app);
        drop(storage);
        worker.await.unwrap();
        if let Some(server) = server {
            server.abort();
        }
    }
}
fn anchor() -> Message {
    DemoStore::new()
        .messages(
            &Scope {
                account: Some("work".into()),
                folder: Some("INBOX".into()),
                inbox_only: false,
            },
            None,
            20,
        )
        .unwrap()
        .messages
        .into_iter()
        .find(|m| m.id.seq == 2)
        .unwrap()
}
fn turn(run: u64) -> Turn {
    let message = anchor();
    Turn {
        run,
        account: Some("work".into()),
        anchor: Some(message.clone()),
        history: vec![MessageText {
            role: "user".into(),
            content: "Find earlier exchanges, prepare a reply and suggest a folder".into(),
        }],
        sources: vec![Source { number: 1, message }],
    }
}
async fn collect(assistant: &mut llm::Assistant) -> Vec<llm::Event> {
    let mut events = vec![];
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), assistant.events.recv())
            .await
            .unwrap()
            .unwrap();
        let done = matches!(event.update, Update::Finished | Update::Error(_));
        events.push(event);
        if done {
            return events;
        }
    }
}
type Requests = Arc<Mutex<Vec<Value>>>;
async fn server(responses: bool) -> (String, Requests, tokio::task::JoinHandle<()>) {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let handler = move |State(requests): State<Requests>,
                        headers: HeaderMap,
                        Json(body): Json<Value>| async move {
        assert_eq!(
            headers["authorization"],
            format!("Bearer {}", std::env::var("PATH").unwrap())
        );
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["stream"], true);
        let step = {
            let mut r = requests.lock().unwrap();
            let n = r.len();
            r.push(body.clone());
            n
        };
        let data = if responses {
            assert_eq!(body["store"], false);
            if step == 0 {
                json!({"type":"response.completed","response":{"output":[{"type":"function_call","id":"fc_1","call_id":"read","name":"read_mail","arguments":"{\"source\":1}"}]}})
            } else {
                assert!(
                    body["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|i| i["type"] == "function_call_output" && i["call_id"] == "read")
                );
                json!({"type":"response.completed","response":{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Historical context [1]"}]}]}})
            }
        } else {
            let calls = match step {
                0 => vec![
                    (
                        "outside",
                        "search_mail",
                        json!({"query":"account:personal"}),
                    ),
                    (
                        "search",
                        "search_mail",
                        json!({"query":"kubernetes","limit":3}),
                    ),
                ],
                1 => {
                    let outputs: Vec<_> = body["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|m| m["role"] == "tool")
                        .collect();
                    let outside: Value =
                        serde_json::from_str(outputs[0]["content"].as_str().unwrap()).unwrap();
                    assert!(outside["sources"].as_array().unwrap().is_empty());
                    let found: Value =
                        serde_json::from_str(outputs[1]["content"].as_str().unwrap()).unwrap();
                    assert!(
                        found["sources"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|s| s["message"]["id"]["account"] == "work")
                    );
                    vec![
                        ("read", "read_mail", json!({"source":1,"limit":5})),
                        (
                            "next",
                            "read_mail",
                            json!({"source":1,"offset":5,"limit":20}),
                        ),
                        ("thread", "read_thread", json!({"source":1})),
                        ("related", "find_related_mail", json!({"source":1})),
                        ("folders", "list_folders", json!({})),
                    ]
                }
                2 => {
                    let outputs = body["messages"].as_array().unwrap();
                    let output = |id: &str| -> Value {
                        serde_json::from_str(
                            outputs
                                .iter()
                                .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
                                .unwrap()["content"]
                                .as_str()
                                .unwrap(),
                        )
                        .unwrap()
                    };
                    assert_eq!(output("read")["body"], "Hello");
                    assert_eq!(output("read")["next_offset"], 5);
                    assert_eq!(output("next")["offset"], 5);
                    assert_eq!(output("thread")["messages"].as_array().unwrap().len(), 1);
                    vec![
                        (
                            "reply",
                            "propose_reply",
                            json!({"source":1,"body":"Bonjour,\nVoici notre réponse."}),
                        ),
                        (
                            "badmove",
                            "propose_move",
                            json!({"sources":[1],"folder":"missing","reason":"test"}),
                        ),
                        (
                            "move",
                            "propose_move",
                            json!({"sources":[1],"folder":"Archive","reason":"Handled incident"}),
                        ),
                    ]
                }
                _ => vec![],
            };
            if calls.is_empty() {
                json!({"choices":[{"delta":{"content":"Historical context [1]"},"finish_reason":"stop"}]})
            } else {
                json!({"choices":[{"delta":{"tool_calls":calls.into_iter().enumerate().map(|(i,(id,name,args))| json!({"index":i,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})).collect::<Vec<_>>()},"finish_reason":"tool_calls"}]})
            }
        };
        let wire = if responses && step > 0 {
            format!(
                "data: {}\n\ndata: {data}\n\n",
                json!({"type":"response.output_text.delta","delta":"Historical context [1]"})
            )
        } else {
            format!("data: {data}\n\ndata: [DONE]\n\n")
        };
        Response::builder()
            .header("content-type", "text/event-stream")
            .body(Body::from(wire))
            .unwrap()
    };
    let router = Router::new()
        .route(
            if responses {
                "/v1/responses"
            } else {
                "/v1/chat/completions"
            },
            post(handler),
        )
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, requests, task)
}

#[test]
fn toml_is_optional_and_enabled_configuration_is_validated() {
    let defaults: mailtui::config::Config = toml::from_str("").unwrap();
    assert!(!defaults.llm.enabled);
    defaults.validate().unwrap();
    let disabled: mailtui::config::Config =
        toml::from_str("[llm]\nenabled=false\nmodel=''\n").unwrap();
    disabled.validate().unwrap();
    let api: mailtui::config::Config = toml::from_str("[llm]\nenabled=true\nmodel='local-model'\nendpoint='http://localhost:8000/v1'\napi='responses'\n[llm.auth]\nmethod='api_key'\napi_key_env='TEST_LLM_KEY'\n").unwrap();
    api.validate().unwrap();
    let oauth: mailtui::config::Config = toml::from_str("[llm]\nenabled=true\nprovider='codex'\nmodel='codex-model'\n[llm.auth]\nmethod='codex_oauth'\n").unwrap();
    oauth.validate().unwrap();
    let mut invalid = config();
    invalid.model.clear();
    assert!(invalid.validate().is_err());
    let mut bad = config();
    bad.auth.method = AuthMethod::CodexOauth;
    assert!(bad.validate().is_err());
    bad.auth.method = AuthMethod::ApiKey;
    bad.endpoint = "https://secret@host/v1".into();
    assert!(bad.validate().is_err());
    assert!(mailtui::command::parse("ai new extra").is_err());
}

#[tokio::test]
async fn chat_tools_are_scoped_and_proposals_reenter_real_composer_and_mutations() {
    let (endpoint, requests, server) = server(false).await;
    let config = Config {
        endpoint,
        ..config()
    };
    let current = Arc::new(AtomicU64::new(0));
    let (tx, mut storage, worker) = start_worker(Box::new(DemoStore::new()), current.clone());
    let mut app = App::new(true, "k9s-dark".into(), current, tx);
    app.llm = config.clone();
    app.reload(false);
    while app.busy {
        app.event(storage.recv().await.unwrap());
    }
    app.view = View::Detail;
    app.detail = Some(anchor());
    app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert!(matches!(app.overlay, Some(Overlay::Assistant)));
    assert_eq!(app.ai.account.as_deref(), Some("work"));
    app.paste("Find earlier exchanges, prepare a reply and suggest a folder");
    app.key(KeyCode::Enter.into());
    let mut assistant = llm::Assistant::new(config, vec![], true, None);
    for command in app.ai.commands.drain(..) {
        assistant.request(command);
    }
    for event in collect(&mut assistant).await {
        assert!(!matches!(&event.update, Update::Error(_)), "{event:?}");
        app.ai_event(event);
    }
    assert_eq!(requests.lock().unwrap().len(), 4);
    assert_eq!(app.ai.proposals.len(), 2);
    assert!(
        app.ai
            .history
            .iter()
            .any(|m| m.content.contains("Historical context [1]"))
    );
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 28)).unwrap();
    terminal
        .draw(|f| mailtui::tui::draw(f, &mut app, false))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(screen.contains("Assistant"));
    assert!(screen.contains("/apply N"));
    app.paste("/apply 1");
    app.key(KeyCode::Enter.into());
    let Some(Overlay::Compose { fields, .. }) = &app.overlay else {
        panic!("Reply did not open composer");
    };
    assert_eq!(fields[0].text, "ops@example.com");
    assert!(fields[2].text.contains("réponse"));
    // Saving follows the existing worker path; the generated draft carries real reply headers.
    app.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    while app.busy {
        app.event(storage.recv().await.unwrap());
    }
    app.execute("ai");
    app.paste("/apply 2");
    app.key(KeyCode::Enter.into());
    while app.busy {
        app.event(storage.recv().await.unwrap());
    }
    assert!(app.ai.proposals[1].is_none());
    assert!(app.toast.contains("Applied locally"));
    app.key(KeyCode::Esc.into());
    app.detail = None;
    app.view = View::Messages;
    app.execute("ai");
    assert_eq!(
        app.ai.anchor.as_ref().unwrap().id.seq,
        2,
        "navigation must not rebind context"
    );
    let old = app.ai.run;
    app.paste("/new");
    app.key(KeyCode::Enter.into());
    app.ai_event(llm::Event {
        run: old,
        update: Update::Text("stale".into()),
    });
    assert!(app.ai.history.is_empty());
    drop(app);
    drop(storage);
    worker.await.unwrap();
    server.abort();
}

#[tokio::test]
async fn responses_stream_preserves_function_outputs_and_citations() {
    let (endpoint, requests, server) = server(true).await;
    let mut assistant = llm::Assistant::new(
        Config {
            endpoint,
            api: Api::Responses,
            ..config()
        },
        vec![],
        true,
        None,
    );
    assistant.request(Command::Turn(Box::new(turn(42))));
    let events = collect(&mut assistant).await;
    assert!(events.iter().all(|e| e.run == 42));
    assert!(
        events.iter().all(|e| !matches!(e.update, Update::Error(_))),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.update,Update::Text(t) if t.contains("[1]")))
    );
    assert_eq!(requests.lock().unwrap().len(), 2);
    server.abort();
}

#[tokio::test]
async fn disabled_assistant_and_input_ctrl_a_do_not_start_requests() {
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = App::new(true, "k9s-dark".into(), Arc::new(AtomicU64::new(0)), tx);
    app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert!(app.overlay.is_none());
    assert!(app.toast.contains("disabled"));
    assert!(app.ai.commands.is_empty());
    app.execute("ai login");
    assert!(app.ai.commands.is_empty());
    app.llm = config();
    app.key(KeyCode::Char(':').into());
    app.paste("status");
    app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert_eq!(app.input.as_ref().unwrap().1.cursor, 0);
    assert!(app.overlay.is_none());
    app.key(KeyCode::Esc.into());
    app.execute("ai global");
    app.paste("hello");
    app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert_eq!(app.ai.input.cursor, 0);
    app.ai.input.set("/stop".into());
    app.key(KeyCode::Enter.into());
    assert!(!app.ai.busy);
    assert!(matches!(app.ai.commands.back(), Some(Command::Cancel)));
}

#[tokio::test]
async fn missing_credentials_fail_lazily_and_cancellation_is_silent() {
    let mut cfg = config();
    cfg.auth.api_key_env = "MAILTUI_TEST_NONEXISTENT_CREDENTIAL_7A81".into();
    let mut assistant = llm::Assistant::new(cfg, vec![], true, None);
    assistant.request(Command::Turn(Box::new(turn(1))));
    let events = collect(&mut assistant).await;
    assert!(
        matches!(&events.last().unwrap().update,Update::Error(e) if e.contains("Set MAILTUI_TEST"))
    );
    assistant.request(Command::Turn(Box::new(turn(2))));
    assistant.request(Command::Cancel);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), assistant.events.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn codex_stdio_oauth_and_dynamic_tools_use_the_same_mail_layer() {
    let home = tempfile::tempdir().unwrap();
    let mut cfg = Config {
        provider: Provider::Codex,
        codex_home: Some(home.path().into()),
        codex_command: vec![
            "python3".into(),
            format!(
                "{}/tests/fixtures/codex_mock.py",
                env!("CARGO_MANIFEST_DIR")
            ),
        ],
        ..config()
    };
    cfg.auth.method = AuthMethod::CodexOauth;
    let mut assistant = llm::Assistant::new(cfg, vec![], true, None);
    assistant.request(Command::Login(1));
    let login = collect(&mut assistant).await;
    assert!(
        login
            .iter()
            .any(|e| matches!(e.update, Update::LoginUrl(_)))
    );
    assert!(
        matches!(login.last().unwrap().update, Update::Finished),
        "{login:?}"
    );
    assistant.request(Command::Turn(Box::new(turn(2))));
    let result = collect(&mut assistant).await;
    assert!(
        result.iter().all(|e| !matches!(e.update, Update::Error(_))),
        "{result:?}"
    );
    assert!(
        result
            .iter()
            .any(|e| matches!(&e.update,Update::Text(t) if t.contains("[1]")))
    );
    assistant.request(Command::Logout(3));
    assert!(matches!(
        collect(&mut assistant).await.last().unwrap().update,
        Update::Finished
    ));
    assistant.request(Command::Turn(Box::new(turn(4))));
    assert!(
        matches!(&collect(&mut assistant).await.last().unwrap().update,Update::Error(e) if e.contains("not connected"))
    );
}

#[tokio::test]
async fn codex_model_catalogue_is_paginated_and_backend_rejections_are_readable() {
    let home = tempfile::tempdir().unwrap();
    let mut cfg = Config {
        provider: Provider::Codex,
        model: "gpt-6-luna".into(),
        codex_home: Some(home.path().into()),
        codex_command: vec![
            "python3".into(),
            format!(
                "{}/tests/fixtures/codex_mock.py",
                env!("CARGO_MANIFEST_DIR")
            ),
        ],
        ..config()
    };
    cfg.auth.method = AuthMethod::CodexOauth;
    let mut assistant = llm::Assistant::new(cfg.clone(), vec![], true, None);
    assistant.request(Command::Login(1));
    assert!(matches!(
        collect(&mut assistant).await.last().unwrap().update,
        Update::Finished
    ));
    assistant.request(Command::Models(2));
    let catalogue = collect(&mut assistant).await;
    assert!(
        catalogue
            .iter()
            .all(|e| !matches!(e.update, Update::Error(_))),
        "{catalogue:?}"
    );
    assert!(catalogue.iter().any(|e| matches!(&e.update,Update::Models(models) if models == &vec!["gpt-6-luna".to_string(),"test-model".to_string()])));
    // A catalogue entry is not a guarantee of account/client entitlement. Do
    // not substitute a model or block inference based solely on the catalogue.
    assistant.request(Command::Turn(Box::new(turn(3))));
    let result = collect(&mut assistant).await;
    let Update::Error(error) = &result.last().unwrap().update else {
        panic!("Missing backend rejection: {result:?}");
    };
    assert!(error.contains("The 'gpt-6-luna' model is not supported"));
    assert!(error.contains(":ai models"));
    assert!(!error.contains("{\"type\""));
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = App::new(true, "k9s-dark".into(), Arc::new(AtomicU64::new(0)), tx);
    app.llm = cfg;
    app.execute("ai");
    let run = app.ai.run;
    app.ai_event(llm::Event {
        run,
        update: Update::Error(error.clone()),
    });
    assert!(app.ai.text().contains(error));
    app.paste("/models");
    app.key(KeyCode::Enter.into());
    assert!(matches!(app.ai.commands.back(), Some(Command::Models(_))));
    assert_eq!(app.completions("ai mod"), vec!["ai models"]);
}

#[test]
fn assistant_popup_preserves_mail_background_and_navigation_at_every_size() {
    fn render(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| mailtui::tui::draw(f, app, false))
            .unwrap();
        terminal.backend().buffer().clone()
    }
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = App::new(true, "k9s-dark".into(), Arc::new(AtomicU64::new(0)), tx);
    app.llm = config();
    let store = DemoStore::new();
    app.accounts = store.accounts();
    app.folders = store.folders().unwrap();
    app.detail = Some(anchor());
    app.parsed = Some(mailtui::mime::parse(&store.raw(&anchor().id).unwrap()).unwrap());
    for view in [View::Messages, View::Detail] {
        app.view = view;
        for (width, height) in [(100, 30), (80, 24), (40, 12), (20, 6)] {
            let before = render(&mut app, width, height);
            let row = app.row;
            app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
            let after = render(&mut app, width, height);
            let w = width * 80 / 100;
            let h = height * 80 / 100;
            let x0 = (width - w) / 2;
            let y0 = (height - h) / 2;
            assert_eq!(after[(x0, y0)].symbol(), "┌");
            assert_eq!(after[(x0 + w - 1, y0 + h - 1)].symbol(), "┘");
            for y in 0..height {
                for x in 0..width {
                    if x < x0 || x >= x0 + w || y < y0 || y >= y0 + h {
                        assert_eq!(
                            after[(x, y)],
                            before[(x, y)],
                            "Background changed at ({x},{y}) in {width}x{height}"
                        );
                    }
                }
            }
            app.key(KeyCode::Esc.into());
            assert_eq!(app.view, view);
            assert_eq!(app.row, row);
            assert_eq!(render(&mut app, width, height), before);
        }
    }
}

#[test]
fn generated_reply_keeps_thread_headers_and_rejects_header_injection() {
    let mut draft = Draft {
        to: "alice@example.com".into(),
        subject: "Re: topic".into(),
        body: "Reply".into(),
        reply: Some(ReplyHeaders {
            message_id: "parent@example.com".into(),
            references: vec!["root@example.com".into()],
        }),
        ..Default::default()
    };
    let raw = draft.mime("me@example.com").unwrap();
    let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
    assert_eq!(parsed.in_reply_to().as_text(), Some("parent@example.com"));
    assert_eq!(
        parsed.references().as_text_list().unwrap(),
        ["root@example.com", "parent@example.com"]
    );
    draft.reply.as_mut().unwrap().message_id = "bad\r\nBcc: someone@example.com".into();
    assert!(draft.mime("me@example.com").is_err());
}

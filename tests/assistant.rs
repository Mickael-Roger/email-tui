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

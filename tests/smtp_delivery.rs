//! Opt-in end-to-end test against the compatible real Neverest executable.
use io_pimdir::{
    capability::{MAIL, PimdirCapability, PimdirSupport},
    client::{PimdirStore, reader::PimdirReader},
};
use mailtui::{
    config::AccountConfig,
    domain::Draft,
    storage::{MailStore, PimdirStore as Adapter},
};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    time::{Duration, Instant},
};

#[tokio::test]
#[ignore = "Set MAILTUI_NEVEREST_BIN to a compatible Neverest built with IMAP/SMTP support"]
async fn real_neverest_delivers_multiple_attachments_to_an_smtp_sink() {
    let executable = std::env::var_os("MAILTUI_NEVEREST_BIN").expect("MAILTUI_NEVEREST_BIN");
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store");
    let mut owner = PimdirStore::open(&store).unwrap().for_account("wire-test");
    owner
        .ensure_collection("fixture/INBOX", "message/rfc822")
        .unwrap();
    owner
        .set_collection_account("fixture/INBOX", Some("wire-test"))
        .unwrap();
    owner
        .set_collection_role("fixture/INBOX", Some("inbox"))
        .unwrap();
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
    // A disposable Neverest sidecar, matching its documented namespaced layout.
    std::fs::write(store.join("neverest.json"), r#"{"layout":1}"#).unwrap();
    let first = directory.path().join("rapport été.pdf");
    let second = directory.path().join("binary.bin");
    std::fs::write(&first, b"%PDF-1.7\nSMTP attachment\n").unwrap();
    std::fs::write(&second, [0, 255, 1, 2, 42, 10]).unwrap();
    let account = AccountConfig {
        neverest_account: Some("wire-test".into()),
        id: "local".into(),
        label: "Local".into(),
        email: "me@example.com".into(),
        store: store.clone(),
    };
    let mut adapter = Adapter::new(vec![account.clone()]);
    adapter
        .compose(
            &Draft {
                account: "local".into(),
                to: "alice@example.com, bob@example.com".into(),
                subject: "SMTP integration · pièces jointes".into(),
                body: "Both original attachments must arrive.".into(),
                attachments: vec![first, second],
                reply: None,
            },
            true,
        )
        .unwrap();
    assert_eq!(
        PimdirReader::open(&store)
            .unwrap()
            .list_pending_actions()
            .unwrap()
            .len(),
        1
    );
    drop(owner);

    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (ack_tx, ack_rx) = std::sync::mpsc::channel();
    let sink = std::thread::spawn(move || -> std::io::Result<(Vec<String>, Vec<u8>)> {
        let mut ready_tx = Some(ready_tx);
        let deadline = Instant::now() + Duration::from_secs(12);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) => return Err(error),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.write_all(b"220 localhost SMTP sink\r\n")?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut commands = vec![];
        let mut data = vec![];
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let command = line.trim_end().to_string();
            let upper = command.to_ascii_uppercase();
            if upper.starts_with("EHLO") {
                stream.write_all(b"250-localhost\r\n250 8BITMIME\r\n")?;
            } else if upper.starts_with("MAIL FROM:") || upper.starts_with("RCPT TO:") {
                commands.push(command);
                stream.write_all(b"250 OK\r\n")?;
            } else if upper == "DATA" {
                stream.write_all(b"354 Send message\r\n")?;
                loop {
                    let mut line = vec![];
                    if reader.read_until(b'\n', &mut line)? == 0 {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }
                    if line == b".\r\n" || line == b".\n" {
                        break;
                    }
                    if line.starts_with(b"..") {
                        line.remove(0);
                    }
                    data.extend(line);
                }
                ready_tx.take().unwrap().send(()).unwrap();
                ack_rx
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(std::io::Error::other)?;
                stream.write_all(b"250 Accepted\r\n")?;
            } else if upper == "QUIT" {
                let _ = stream.write_all(b"221 Bye\r\n");
                break;
            } else {
                stream.write_all(b"250 OK\r\n")?;
            }
        }
        Ok((commands, data))
    });
    let config = directory.path().join("neverest.toml");
    std::fs::write(&config, format!("[accounts.aaa-default]\nstore.root = {}\n[accounts.aaa-default.sources.fixture]\nimap.server = \"imap://127.0.0.1:1\"\nimap.starttls = false\nsmtp.server = \"smtp://127.0.0.1:1\"\nsmtp.starttls = false\n[accounts.wire-test]\nstore.root = {}\n[accounts.wire-test.sources.fixture]\nimap.server = \"imap://127.0.0.1:1\"\nimap.starttls = false\nsmtp.server = \"smtp://127.0.0.1:{port}\"\nsmtp.starttls = false\n", serde_json::to_string(&directory.path().join("wrong-store").to_string_lossy()).unwrap(), serde_json::to_string(&store.to_string_lossy()).unwrap())).unwrap();
    let delivery = mailtui::config::Delivery {
        command: vec![executable.to_string_lossy().into_owned()],
        config: Some(config),
        timeout_seconds: 15,
    };
    let sending =
        tokio::spawn(async move { mailtui::delivery::deliver(&delivery, &account).await });
    tokio::time::timeout(Duration::from_secs(12), ready_rx)
        .await
        .unwrap()
        .unwrap();
    // DATA has arrived, but the server has not acknowledged it: keep Pending.
    assert_eq!(adapter.pending().unwrap().len(), 1);
    ack_tx.send(()).unwrap();
    sending.await.unwrap().unwrap();
    assert!(adapter.pending().unwrap().is_empty());
    let (commands, raw) = sink.join().unwrap().unwrap();
    assert!(
        commands
            .iter()
            .any(|command| command.starts_with("RCPT TO:<alice@example.com>"))
    );
    assert!(
        commands
            .iter()
            .any(|command| command.starts_with("RCPT TO:<bob@example.com>"))
    );
    use mail_parser::MimeHeaders;
    let message = mail_parser::MessageParser::default().parse(&raw).unwrap();
    let attachments: std::collections::HashMap<_, _> = message
        .attachments()
        .map(|part| (part.attachment_name().unwrap(), part.contents()))
        .collect();
    assert_eq!(attachments.len(), 2);
    assert_eq!(
        attachments["rapport été.pdf"],
        b"%PDF-1.7\nSMTP attachment\n"
    );
    assert_eq!(attachments["binary.bin"], &[0, 255, 1, 2, 42, 10]);
    assert!(
        PimdirReader::open(&store)
            .unwrap()
            .list_pending_actions()
            .unwrap()
            .is_empty()
    );
}

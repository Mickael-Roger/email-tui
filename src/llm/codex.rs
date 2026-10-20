//! Codex app-server stdio protocol. OAuth is owned/refreshed by Codex, never
//! treated as an API key for an arbitrary OpenAI-compatible endpoint.
use super::{tools::Tools, *};
use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::{collections::VecDeque, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
};

struct Rpc {
    child: Child,
    input: ChildStdin,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
    next: u64,
    pending: VecDeque<Value>,
    bytes: usize,
    _directory: tempfile::TempDir,
}
impl Rpc {
    async fn start(config: &Config) -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let home = config.codex_home.clone().unwrap_or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
                })
                .join("mailtui/codex")
        });
        if !home.exists() {
            std::fs::create_dir_all(&home)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        let mut command = tokio::process::Command::new(&config.codex_command[0]);
        command
            .args(&config.codex_command[1..])
            .args(["app-server"])
            .current_dir(directory.path())
            .env("CODEX_HOME", home)
            .env_remove("OPENAI_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .context("Cannot start Codex; install codex or set llm.codex_command")?;
        let input = child.stdin.take().context("Codex stdin unavailable")?;
        let lines =
            BufReader::new(child.stdout.take().context("Codex stdout unavailable")?).lines();
        let mut rpc = Self {
            child,
            input,
            lines,
            next: 0,
            pending: VecDeque::new(),
            bytes: 0,
            _directory: directory,
        };
        rpc.request("initialize",json!({"clientInfo":{"name":"mailtui","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
        rpc.write(json!({"method":"initialized"})).await?;
        Ok(rpc)
    }
    async fn write(&mut self, value: Value) -> Result<()> {
        let mut data = serde_json::to_vec(&value)?;
        data.push(b'\n');
        self.input.write_all(&data).await?;
        self.input.flush().await?;
        Ok(())
    }
    async fn read(&mut self) -> Result<Value> {
        let line = self
            .lines
            .next_line()
            .await?
            .context("Codex app-server stopped")?;
        self.bytes += line.len();
        ensure!(
            line.len() <= 2_000_000 && self.bytes <= 8_000_000,
            "Codex stream limit exceeded"
        );
        serde_json::from_str(&line).context("Invalid Codex JSON-RPC")
    }
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        self.write(json!({"id":id,"method":method,"params":params}))
            .await?;
        loop {
            let value = self.read().await?;
            if value["id"] == id && value.get("method").is_none() {
                if let Some(error) = value.get("error") {
                    bail!(
                        "Codex {method}: {}",
                        error["message"].as_str().unwrap_or("request rejected")
                    );
                }
                return Ok(value["result"].clone());
            }
            ensure!(
                self.pending.len() < 128,
                "Codex notification queue overflow"
            );
            self.pending.push_back(value);
        }
    }
    async fn next(&mut self) -> Result<Value> {
        if let Some(v) = self.pending.pop_front() {
            Ok(v)
        } else {
            self.read().await
        }
    }
}
impl Drop for Rpc {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

pub(super) async fn account(
    config: &Config,
    command: Command,
    run: u64,
    sender: &mpsc::Sender<Event>,
) -> Result<()> {
    ensure!(
        config.provider == Provider::Codex,
        "API mode uses the configured API-key environment variable; login/logout are Codex-only"
    );
    let mut rpc = Rpc::start(config).await?;
    let text = match command {
        Command::Login(_) => {
            let login = rpc
                .request("account/login/start", json!({"type":"chatgpt"}))
                .await?;
            let url = login["authUrl"]
                .as_str()
                .context("Codex did not provide a login URL")?;
            let parsed = reqwest::Url::parse(url)?;
            ensure!(
                ["https", "http"].contains(&parsed.scheme()),
                "Invalid Codex login URL"
            );
            sender
                .send(Event {
                    run,
                    update: Update::LoginUrl(url.into()),
                })
                .await?;
            loop {
                let value = rpc.next().await?;
                if value["method"] == "account/login/completed"
                    && value["params"]["loginId"] == login["loginId"]
                {
                    ensure!(
                        value["params"]["success"] == true,
                        "Codex login failed or was cancelled"
                    );
                    break;
                }
            }
            "Codex OAuth connected".to_string()
        }
        Command::Logout(_) => {
            rpc.request("account/logout", json!({})).await?;
            "Codex OAuth disconnected".into()
        }
        Command::Status(_) => {
            let result = rpc
                .request("account/read", json!({"refreshToken":false}))
                .await?;
            format!(
                "{} · account: {}",
                config.label(),
                result["account"]["type"]
                    .as_str()
                    .unwrap_or("not connected; use :ai login")
            )
        }
        _ => bail!("Invalid account operation"),
    };
    sender
        .send(Event {
            run,
            update: Update::Status(text),
        })
        .await?;
    Ok(())
}

pub(super) async fn turn(config: &Config, turn: &Turn, tools: &mut Tools) -> Result<()> {
    let mut rpc = Rpc::start(config).await?;
    let account = rpc
        .request("account/read", json!({"refreshToken":true}))
        .await?;
    ensure!(
        account["account"]["type"] == "chatgpt",
        "Codex OAuth is not connected; use :ai login"
    );
    let specs: Vec<_> = tools::specs().into_iter().map(|s| json!({"type":"function","name":s["name"],"description":s["description"],"inputSchema":s["parameters"]})).collect();
    let thread = rpc.request("thread/start",json!({
        "model":config.model,"modelProvider":"openai","approvalPolicy":"never","sandbox":"read-only","ephemeral":true,
        "baseInstructions":prompt(turn),"dynamicTools":specs,
        "config":{"project_doc_max_bytes":0,"features.shell_tool":false,"features.apply_patch_freeform":false,"features.multi_agent":false,"web_search":"disabled"}
    })).await?;
    let id = thread["thread"]["id"]
        .as_str()
        .context("Codex thread/start did not return a thread ID")?;
    let mut input = tools.context(turn).await?;
    input.push_str("\n\nConversation:\n");
    for m in &turn.history {
        input.push_str(&format!("{}: {}\n\n", m.role, m.content));
    }
    rpc.request(
        "turn/start",
        json!({"threadId":id,"input":[{"type":"text","text":input,"text_elements":[]}]}),
    )
    .await?;
    let mut streamed = std::collections::HashSet::new();
    let mut output_bytes = 0usize;
    let mut calls = 0usize;
    loop {
        let event = rpc.next().await?;
        match event["method"].as_str().unwrap_or_default() {
            "item/agentMessage/delta" => {
                let delta = event["params"]["delta"].as_str().unwrap_or_default();
                output_bytes += delta.len();
                ensure!(output_bytes <= 65536, "Codex answer too long");
                if let Some(id) = event["params"]["itemId"].as_str() {
                    streamed.insert(id.to_string());
                }
                tools.emit(Update::Text(delta.into())).await?;
            }
            "item/completed" => {
                let item = &event["params"]["item"];
                if item["type"] == "agentMessage"
                    && !streamed.contains(item["id"].as_str().unwrap_or_default())
                {
                    let text = item["text"].as_str().unwrap_or_default();
                    output_bytes += text.len();
                    ensure!(output_bytes <= 65536, "Codex answer too long");
                    tools.emit(Update::Text(text.into())).await?;
                }
            }
            "item/tool/call" => {
                calls += 1;
                ensure!(
                    calls <= config.max_tool_rounds * 4,
                    "Codex mail tool limit reached"
                );
                let name = event["params"]["tool"]
                    .as_str()
                    .context("Missing Codex tool name")?;
                let args = event["params"]["arguments"].clone();
                let args = if let Some(s) = args.as_str() {
                    serde_json::from_str(s)?
                } else {
                    args
                };
                let result = tools.result(name, args).await;
                rpc.write(json!({"id":event["id"],"result":{"contentItems":[{"type":"inputText","text":result.to_string()}],"success":result.get("error").is_none()}})).await?;
            }
            "turn/completed" => {
                ensure!(
                    event["params"]["turn"]["status"] == "completed",
                    "Codex turn failed or was interrupted"
                );
                return Ok(());
            }
            "error" => bail!(
                "Codex: {}",
                event["params"]["error"]["message"]
                    .as_str()
                    .unwrap_or("generation failed")
            ),
            _ if event.get("method").is_some() && event.get("id").is_some() => {
                rpc.write(json!({"id":event["id"],"error":{"code":-32601,"message":"Only MailTUI mail tools are available"}})).await?;
            }
            _ => {}
        }
    }
}

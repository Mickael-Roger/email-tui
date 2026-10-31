use crate::config::{AccountConfig, Delivery};
use anyhow::{Context, Result, bail};
use std::{
    collections::{HashMap, HashSet},
    process::Stdio,
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle};

pub struct Completed {
    pub account: String,
    pub error: Option<String>,
    pub full_sync: bool,
}

/// Explicit sends and manual sync share a per-account queue; no polling is scheduled.
pub struct Sender {
    config: Delivery,
    accounts: Vec<AccountConfig>,
    events: mpsc::Sender<Completed>,
    jobs: HashMap<String, JoinHandle<()>>,
    again: HashSet<String>,
    full_again: HashSet<String>,
}
impl Sender {
    pub fn new(
        config: Delivery,
        accounts: Vec<AccountConfig>,
        events: mpsc::Sender<Completed>,
    ) -> Self {
        Self {
            config,
            accounts,
            events,
            jobs: HashMap::new(),
            again: HashSet::new(),
            full_again: HashSet::new(),
        }
    }
    pub fn request(&mut self, account: &str) {
        self.start(account, false);
    }
    pub fn request_sync(&mut self, account: &str) {
        self.start(account, true);
    }
    fn start(&mut self, account: &str, full_sync: bool) {
        if self.jobs.contains_key(account) {
            if full_sync {
                self.full_again.insert(account.into());
            } else {
                self.again.insert(account.into());
            }
            return;
        }
        let Some(account) = self
            .accounts
            .iter()
            .find(|candidate| candidate.id == account)
            .cloned()
        else {
            return;
        };
        let config = self.config.clone();
        let events = self.events.clone();
        self.jobs.insert(
            account.id.clone(),
            tokio::spawn(async move {
                let error = run_neverest(&config, &account, full_sync)
                    .await
                    .err()
                    .map(|error| format!("{error:#}"));
                let _ = events
                    .send(Completed {
                        account: account.id,
                        error,
                        full_sync,
                    })
                    .await;
            }),
        );
    }
    pub fn completed(&mut self, account: &str) {
        self.jobs.remove(account);
        if self.full_again.remove(account) {
            self.request_sync(account);
        } else if self.again.remove(account) {
            self.request(account);
        }
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.abort();
        }
    }
}

pub async fn deliver(config: &Delivery, account: &AccountConfig) -> Result<()> {
    run_neverest(config, account, false).await
}
pub async fn synchronize(config: &Delivery, account: &AccountConfig) -> Result<()> {
    run_neverest(config, account, true).await
}
async fn run_neverest(config: &Delivery, account: &AccountConfig, full_sync: bool) -> Result<()> {
    let mut command = tokio::process::Command::new(&config.command[0]);
    command
        .args(&config.command[1..])
        .arg("sync")
        .arg("-a")
        .arg(account.neverest_account.as_deref().unwrap_or(&account.id));
    if let Some(path) = &config.config {
        command.arg("--config").arg(path);
    }
    // Explicit sends drain intents only; a manual full sync also fetches mail.
    if !full_sync {
        command.arg("--intents-only");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(
        Duration::from_secs(config.timeout_seconds),
        command.status(),
    )
    .await
    .with_context(|| {
        if full_sync {
            "Mail synchronization timed out"
        } else {
            "Sending timed out"
        }
    })?
    .with_context(|| {
        if full_sync {
            "Start Neverest for mail synchronization"
        } else {
            "Start Neverest for sending"
        }
    })?;
    if !status.success() {
        bail!(
            "{} needs attention (Neverest exit {})",
            if full_sync {
                "Mail synchronization"
            } else {
                "Sending"
            },
            status
                .code()
                .map_or_else(|| "signal".into(), |code| code.to_string())
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manual_full_sync_is_account_scoped_and_serialized_with_sends() {
        let root = tempfile::tempdir().unwrap();
        let log = root.path().join("sync.jsonl");
        let config = Delivery {
            command: vec!["python3".into(), "-c".into(),
                "import json,sys,time\nf=open(sys.argv[1],'a'); f.write(json.dumps(['start']+sys.argv[2:])+'\\n'); f.flush(); time.sleep(.1); f.write(json.dumps(['end']+sys.argv[2:])+'\\n'); f.close()".into(), log.to_string_lossy().into_owned()],
            config: Some(root.path().join("config with spaces")), timeout_seconds: 3,
        };
        let account = AccountConfig {
            id: "work".into(),
            label: "Work".into(),
            email: "x@example.test".into(),
            store: root.path().into(),
            neverest_account: Some("remote work".into()),
        };
        let (tx, mut rx) = mpsc::channel(8);
        let mut sender = Sender::new(config.clone(), vec![account.clone()], tx);
        sender.request("work");
        sender.request_sync("work");
        sender.request_sync("work");
        sender.request("work"); // A queued send must still run if the full sync fails.
        for full_sync in [false, true, false] {
            let event = tokio::time::timeout(Duration::from_secs(4), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(event.full_sync, full_sync);
            assert!(event.error.is_none());
            sender.completed(&event.account);
        }
        let records: Vec<Vec<String>> = std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            records.iter().map(|r| r[0].as_str()).collect::<Vec<_>>(),
            ["start", "end", "start", "end", "start", "end"]
        );
        assert_eq!(
            &records[0][1..],
            [
                "sync",
                "-a",
                "remote work",
                "--config",
                config.config.as_ref().unwrap().to_str().unwrap(),
                "--intents-only"
            ]
        );
        assert_eq!(
            &records[2][1..],
            [
                "sync",
                "-a",
                "remote work",
                "--config",
                config.config.as_ref().unwrap().to_str().unwrap()
            ]
        );
        let mut failure = config;
        failure.command = vec!["python3".into(), "-c".into(), "raise SystemExit(7)".into()];
        assert!(
            synchronize(&failure, &account)
                .await
                .unwrap_err()
                .to_string()
                .contains('7')
        );
        failure.command[2] = "import time; time.sleep(5)".into();
        failure.timeout_seconds = 1;
        assert!(
            synchronize(&failure, &account)
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
    }

    #[tokio::test]
    async fn sends_target_the_original_account_and_coalesce_only_that_accounts_followup() {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("delivery log.jsonl");
        let config_path = directory.path().join("neverest config.toml");
        let script = "import json,os,sys,time\na=sys.argv[2:]\nf=os.open(sys.argv[1],os.O_WRONLY|os.O_CREAT|os.O_APPEND,0o600)\nos.write(f,(json.dumps({'event':'start','args':a})+'\\n').encode())\ntime.sleep(.4)\nos.write(f,(json.dumps({'event':'end','args':a})+'\\n').encode())\nos.close(f)";
        let config = Delivery {
            command: vec![
                "python3".into(),
                "-c".into(),
                script.into(),
                log.to_string_lossy().into_owned(),
            ],
            config: Some(config_path.clone()),
            timeout_seconds: 5,
        };
        let accounts = ["work", "personal"]
            .into_iter()
            .map(|id| AccountConfig {
                id: id.into(),
                label: id.into(),
                email: "me@example.com".into(),
                store: directory.path().into(),
                neverest_account: (id == "work").then(|| "remote work".into()),
            })
            .collect();
        let (tx, mut events) = mpsc::channel(16);
        let mut sender = Sender::new(config, accounts, tx);
        sender.request("work");
        sender.request("work");
        sender.request("personal");
        for _ in 0..3 {
            let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(event.error.is_none());
            sender.completed(&event.account);
        }
        let records: Vec<serde_json::Value> = std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 6);
        assert_eq!(records[0]["event"], "start");
        assert_eq!(records[1]["event"], "start"); // distinct accounts overlap
        let starts: Vec<_> = records
            .iter()
            .filter(|record| record["event"] == "start")
            .collect();
        assert_eq!(
            starts
                .iter()
                .filter(|record| record["args"][2] == "remote work")
                .count(),
            2
        );
        assert_eq!(
            starts
                .iter()
                .filter(|record| record["args"][2] == "personal")
                .count(),
            1
        );
        for record in starts {
            let account = record["args"][2].as_str().unwrap();
            assert_eq!(
                record["args"],
                serde_json::json!([
                    "sync",
                    "-a",
                    account,
                    "--config",
                    config_path,
                    "--intents-only"
                ])
            );
        }
    }
}

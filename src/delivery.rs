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
}

/// Only explicit message submissions start a job. Never schedules mailbox polling.
pub struct Sender {
    config: Delivery,
    accounts: Vec<AccountConfig>,
    events: mpsc::Sender<Completed>,
    jobs: HashMap<String, JoinHandle<()>>,
    again: HashSet<String>,
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
        }
    }
    pub fn request(&mut self, account: &str) {
        if self.jobs.contains_key(account) {
            self.again.insert(account.into());
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
                let error = deliver(&config, &account)
                    .await
                    .err()
                    .map(|error| format!("{error:#}"));
                let _ = events
                    .send(Completed {
                        account: account.id,
                        error,
                    })
                    .await;
            }),
        );
    }
    pub fn completed(&mut self, account: &str) {
        self.jobs.remove(account);
        if self.again.remove(account) {
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
    let mut command = tokio::process::Command::new(&config.command[0]);
    command
        .args(&config.command[1..])
        .arg("sync")
        .arg("-a")
        .arg(account.neverest_account.as_deref().unwrap_or(&account.id));
    if let Some(path) = &config.config {
        command.arg("--config").arg(path);
    }
    // This is the frontend send operation: SMTP acknowledgement removes its intent.
    // Mailbox fetching and the Sent replica continue through Carillon.
    command
        .arg("--intents-only")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(
        Duration::from_secs(config.timeout_seconds),
        command.status(),
    )
    .await
    .context("Sending timed out")?
    .context("Start Neverest for sending")?;
    if !status.success() {
        bail!(
            "Sending needs attention (Neverest exit {})",
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

use crate::config::CalendarsConfig;
use anyhow::{Context, Result, bail};
use std::{
    collections::{HashMap, HashSet},
    process::Stdio,
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Calendars,
    Tasks,
    Contacts,
}
impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Self::Calendars => "calendars",
            Self::Tasks => "tasks",
            Self::Contacts => "contacts",
        }
    }
}
pub struct Completed {
    pub target: Target,
    pub error: Option<String>,
}
pub struct Synchronizer {
    configs: HashMap<Target, CalendarsConfig>,
    jobs: HashMap<Target, JoinHandle<()>>,
    again: HashSet<Target>,
    events: mpsc::Sender<Completed>,
}
impl Synchronizer {
    pub fn new(
        calendars: CalendarsConfig,
        tasks: CalendarsConfig,
        events: mpsc::Sender<Completed>,
    ) -> Self {
        Self {
            configs: [
                (Target::Calendars, calendars),
                (Target::Tasks, tasks),
                (Target::Contacts, CalendarsConfig::default()),
            ]
            .into(),
            jobs: HashMap::new(),
            again: HashSet::new(),
            events,
        }
    }
    pub fn request(&mut self, target: Target) {
        if self.jobs.contains_key(&target) {
            self.again.insert(target);
            return;
        }
        let config = self.configs[&target].clone();
        if config.sync_command.is_empty() {
            return;
        }
        let events = self.events.clone();
        self.jobs.insert(
            target,
            tokio::spawn(async move {
                let error = synchronize(&config).await.err().map(|e| format!("{e:#}"));
                let _ = events.send(Completed { target, error }).await;
            }),
        );
    }
    pub fn with_contacts(mut self, contacts: &crate::config::ContactsConfig) -> Self {
        self.configs
            .insert(Target::Contacts, contacts.sync_config());
        self
    }
    pub fn completed(&mut self, target: Target) {
        self.jobs.remove(&target);
        if self.again.remove(&target) {
            self.request(target);
        }
    }
}
impl Drop for Synchronizer {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.abort();
        }
    }
}
pub async fn synchronize(config: &CalendarsConfig) -> Result<()> {
    let executable = config
        .sync_command
        .first()
        .context("No sync_command configured")?;
    let mut command = tokio::process::Command::new(executable);
    command
        .args(&config.sync_command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(
        Duration::from_secs(config.sync_timeout_seconds),
        command.status(),
    )
    .await
    .context("Synchronization timed out")?
    .context("Start synchronization command")?;
    if !status.success() {
        bail!("Synchronization command exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn commands_keep_exact_arguments_and_coalesce_per_resource() {
        let root = tempfile::tempdir().unwrap();
        let log = root.path().join("sync log.jsonl");
        let config = |name: &str| {
            CalendarsConfig {
            sync_command: vec!["python3".into(), "-c".into(),
                "import json,sys,time\nwith open(sys.argv[1],'a') as f: f.write(json.dumps(sys.argv[2:])+'\\n')\ntime.sleep(.15)".into(),
                log.to_string_lossy().into_owned(), name.into(), "argument with spaces; $literal".into()],
            sync_timeout_seconds: 5,
            ..Default::default()
        }
        };
        let (tx, mut rx) = mpsc::channel(8);
        let mut sync = Synchronizer::new(config("calendar"), config("tasks"), tx).with_contacts(
            &crate::config::ContactsConfig {
                enabled: true,
                sync_command: config("contacts").sync_command,
                ..Default::default()
            },
        );
        sync.request(Target::Calendars);
        sync.request(Target::Calendars);
        sync.request(Target::Calendars);
        sync.request(Target::Tasks);
        sync.request(Target::Contacts);
        sync.request(Target::Contacts);
        sync.request(Target::Contacts);
        for _ in 0..5 {
            let completed = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(completed.error.is_none(), "{:?}", completed.error);
            sync.completed(completed.target);
        }
        assert!(sync.jobs.is_empty());
        let records: Vec<Vec<String>> = std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(records.iter().filter(|r| r[0] == "calendar").count(), 2);
        assert_eq!(records.iter().filter(|r| r[0] == "tasks").count(), 1);
        assert_eq!(records.iter().filter(|r| r[0] == "contacts").count(), 2);
        assert!(
            records
                .iter()
                .all(|r| r.len() == 2 && r[1] == "argument with spaces; $literal")
        );
    }

    #[tokio::test]
    async fn command_failures_and_timeout_are_reported() {
        let mut config = CalendarsConfig {
            sync_command: vec!["python3".into(), "-c".into(), "raise SystemExit(7)".into()],
            sync_timeout_seconds: 1,
            ..Default::default()
        };
        assert!(
            synchronize(&config)
                .await
                .unwrap_err()
                .to_string()
                .contains("7")
        );
        config.sync_command[2] = "import time; time.sleep(10)".into();
        assert!(
            synchronize(&config)
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        config.sync_command = vec!["/nonexistent/mailtui-sync".into()];
        assert!(
            synchronize(&config)
                .await
                .unwrap_err()
                .to_string()
                .contains("Start synchronization")
        );
    }
}

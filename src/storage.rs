use crate::{config::AccountConfig, domain::*};
use anyhow::{Context, Result, bail};
use io_pimdir::{
    client::producer::PimdirProducer, codec::PimdirAction, object::PimdirObject,
    placement::PimdirFlags,
};
use io_pimdir::{client::reader::PimdirReader, summary::PimdirSummary};
use std::{
    collections::HashMap,
    io::{Read, Write},
};

pub const MAX_MIME: usize = 16 * 1024 * 1024;
pub const PAGE_SIZE: usize = 200;

pub type ChangeTokens = Vec<(String, Option<(i64, i64)>)>;
/// Public change-feed stamps detect external Neverest writes, including WAL writes.
pub fn change_tokens(accounts: &[AccountConfig]) -> ChangeTokens {
    accounts
        .iter()
        .map(|account| {
            let cursor = PimdirReader::open(&account.store)
                .and_then(|reader| reader.change_cursor())
                .ok();
            (
                account.id.clone(),
                cursor.map(|cursor| (cursor.changed, cursor.purges)),
            )
        })
        .collect()
}

/// Poll public cursors off the UI thread, including changes committed in WAL.
pub struct Changes {
    pub events: tokio::sync::mpsc::Receiver<ChangeTokens>,
    task: tokio::task::JoinHandle<()>,
}
impl Changes {
    pub fn watch(accounts: Vec<AccountConfig>) -> Self {
        let (sender, events) = tokio::sync::mpsc::channel(1);
        let task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let accounts = accounts.clone();
                let Ok(tokens) =
                    tokio::task::spawn_blocking(move || change_tokens(&accounts)).await
                else {
                    break;
                };
                if sender.send(tokens).await.is_err() {
                    break;
                }
            }
        });
        Self { events, task }
    }
}
impl Drop for Changes {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub trait MailStore: Send {
    fn accounts(&self) -> Vec<Account>;
    fn folders(&self) -> Result<Vec<Folder>>;
    fn messages(&self, scope: &Scope, cursor: Option<&Cursor>, limit: usize) -> Result<Page>;
    fn raw(&self, id: &MessageId) -> Result<Vec<u8>>;
    fn mutate(&mut self, ids: &[MessageId], action: &Mutation) -> Result<MutationResult>;
    fn pending(&self) -> Result<Vec<PendingMail>>;
    fn create_folder(
        &mut self,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: &str,
    ) -> Result<MutationResult> {
        bail!("Folder creation is unavailable for this store")
    }
    fn compose(&mut self, _: &Draft, _: bool) -> Result<MutationResult> {
        bail!("Composition is unavailable for this store")
    }
    fn is_demo(&self) -> bool {
        false
    }
}

fn in_scope(folder: &Folder, scope: &Scope) -> bool {
    scope.account.as_ref().is_none_or(|a| a == &folder.account)
        && scope.folder.as_ref().is_none_or(|f| f == &folder.id)
        && (!scope.inbox_only
            || folder.role.as_deref() == Some("inbox")
            || folder.name.eq_ignore_ascii_case("INBOX"))
}

fn trash_folder<'a>(folders: &'a [Folder], account: &str) -> Option<&'a Folder> {
    folders
        .iter()
        .filter(|folder| folder.account == account)
        .filter_map(|folder| {
            let names = [folder.name.as_str(), folder.id.as_str()];
            let rank = if folder
                .role
                .as_deref()
                .is_some_and(|role| role.trim_start_matches('\\').eq_ignore_ascii_case("trash"))
            {
                0
            } else if names.iter().any(|name| {
                name.trim_end_matches(['/', '.'])
                    .eq_ignore_ascii_case("trash")
            }) {
                1
            } else if names.iter().any(|name| {
                name.trim_end_matches(['/', '.'])
                    .rsplit(['/', '.'])
                    .next()
                    .is_some_and(|leaf| leaf.eq_ignore_ascii_case("trash"))
            }) {
                2
            } else {
                return None;
            };
            Some((rank, folder))
        })
        .min_by(|(a_rank, a), (b_rank, b)| a_rank.cmp(b_rank).then(a.id.cmp(&b.id)))
        .map(|(_, folder)| folder)
}

fn merge_page(mut all: Vec<Message>, mut cursor: Cursor, total: u64, limit: usize) -> Page {
    all.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| a.id.account.cmp(&b.id.account))
            .then_with(|| a.id.folder.cmp(&b.id.folder))
            .then_with(|| b.id.seq.cmp(&a.id.seq))
    });
    let more = all.len() > limit;
    all.truncate(limit);
    for m in &all {
        cursor.insert(
            (m.id.account.clone(), m.id.folder.clone()),
            (m.date.clone(), m.id.seq),
        );
    }
    Page {
        messages: all,
        total,
        next: more.then_some(cursor),
    }
}

pub struct PimdirStore {
    accounts: Vec<AccountConfig>,
}
impl PimdirStore {
    pub fn new(accounts: Vec<AccountConfig>) -> Self {
        Self { accounts }
    }
    fn reader(&self, account: &str) -> Result<PimdirReader> {
        let config = self
            .accounts
            .iter()
            .find(|a| a.id == account)
            .context("Unknown account")?;
        PimdirReader::open(&config.store)
            .map(PimdirReader::with_pending)
            .with_context(|| format!("Cannot read pimdir store for {account}"))
    }
    fn producer(&self, account: &str) -> Result<PimdirProducer> {
        let config = self
            .accounts
            .iter()
            .find(|a| a.id == account)
            .context("Unknown account")?;
        Ok(PimdirProducer::open(&config.store, "mailtui")?)
    }
}
impl MailStore for PimdirStore {
    fn accounts(&self) -> Vec<Account> {
        self.accounts
            .iter()
            .map(|a| Account {
                id: a.id.clone(),
                label: a.label.clone(),
                email: a.email.clone(),
            })
            .collect()
    }
    fn folders(&self) -> Result<Vec<Folder>> {
        let mut out = vec![];
        for account in &self.accounts {
            let reader = self.reader(&account.id)?;
            let cols = reader.list_collections()?;
            let mail: Vec<_> = cols
                .into_iter()
                .filter(|c| c.kind.starts_with("message/rfc822"))
                .collect();
            let ids: Vec<_> = mail.iter().map(|c| c.id.as_str()).collect();
            let unread = reader.count_unread(&ids, None, None)?;
            for c in mail {
                out.push(Folder {
                    account: account.id.clone(),
                    total: reader.count_items(&c.id)?,
                    unread: unread.get(&c.id).copied().unwrap_or(0),
                    id: c.id,
                    name: c.name,
                    parent: c.parent,
                    role: c.role,
                });
            }
        }
        Ok(out)
    }
    fn messages(&self, scope: &Scope, cursor: Option<&Cursor>, limit: usize) -> Result<Page> {
        let mut out = vec![];
        let mut total = 0;
        for folder in self.folders()?.iter().filter(|f| in_scope(f, scope)) {
            let reader = self.reader(&folder.account)?;
            total += folder.total;
            let after = cursor.and_then(|c| c.get(&(folder.account.clone(), folder.id.clone())));
            for item in reader.list_summaries(
                &folder.id,
                after.map(|(s, n)| (s.as_str(), *n)),
                limit + 1,
            )? {
                let summary = match item.summary {
                    Some(PimdirSummary::Mail(m)) => m,
                    _ => Default::default(),
                };
                out.push(Message {
                    id: MessageId {
                        account: folder.account.clone(),
                        folder: folder.id.clone(),
                        seq: item.seq,
                    },
                    sender: summary.sender.unwrap_or_default(),
                    subject: summary.subject,
                    date: item.sort_key,
                    seen: item.flags.contains("\\Seen"),
                    flagged: item.flags.contains("\\Flagged"),
                    attachment: summary.attachment.unwrap_or(false),
                    size: summary.size.unwrap_or(0),
                });
            }
        }
        Ok(merge_page(
            out,
            cursor.cloned().unwrap_or_default(),
            total,
            limit,
        ))
    }
    fn raw(&self, id: &MessageId) -> Result<Vec<u8>> {
        let reader = self.reader(&id.account)?;
        let item = reader
            .get_item(&id.folder, id.seq)?
            .context("Stale message ID: reload the folder")?;
        let hash = item
            .object
            .context("Unavailable offline: body has not been hydrated by Neverest")?;
        let file = reader
            .blobs()
            .reader(&hash)?
            .context("Unavailable offline: missing MIME blob")?;
        let mut bytes = vec![];
        file.take((MAX_MIME + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_MIME {
            bail!("Message exceeds the 16 MiB reading limit");
        }
        Ok(bytes)
    }
    fn mutate(&mut self, ids: &[MessageId], mutation: &Mutation) -> Result<MutationResult> {
        let folders = self.folders()?;
        let mut actions = vec![];
        // Validate every target before starting the independent enqueue transactions.
        for id in ids {
            let reader = self.reader(&id.account)?;
            let item = reader
                .get_item(&id.folder, id.seq)?
                .context("Stale message; reload")?;
            let action = match mutation {
                Mutation::Seen(value) | Mutation::Flagged(value) => {
                    let mut flags = item
                        .flags
                        .known()
                        .cloned()
                        .context("Message flags unavailable; synchronize first")?;
                    let flag = if matches!(mutation, Mutation::Seen(_)) {
                        "\\Seen"
                    } else {
                        "\\Flagged"
                    };
                    if *value {
                        flags.insert(flag.into());
                    } else {
                        flags.remove(flag);
                    }
                    PimdirAction::SetFlags {
                        seq: id.seq,
                        flags: PimdirFlags::Known(flags),
                    }
                }
                Mutation::Move(to) | Mutation::Copy(to) => {
                    anyhow::ensure!(
                        folders
                            .iter()
                            .any(|f| f.account == id.account && f.id == *to),
                        "Destination does not exist in {}",
                        id.account
                    );
                    if matches!(mutation, Mutation::Move(_)) {
                        PimdirAction::Move {
                            seq: id.seq,
                            to: to.as_str().into(),
                        }
                    } else {
                        PimdirAction::Copy {
                            seq: id.seq,
                            to: to.as_str().into(),
                        }
                    }
                }
                Mutation::Trash => {
                    let to = trash_folder(&folders, &id.account)
                        .with_context(|| format!("No Trash folder in account {}", id.account))?;
                    PimdirAction::Move {
                        seq: id.seq,
                        to: to.id.as_str().into(),
                    }
                }
            };
            self.producer(&id.account)?.check(&id.folder, &action)?;
            actions.push((id, action));
        }
        for (queued, (id, action)) in actions.into_iter().enumerate() {
            self.producer(&id.account)?
                .enqueue(&id.folder, &action, None)
                .with_context(|| format!("{queued} actions already queued; enqueue failed"))?;
        }
        Ok(MutationResult::Queued(String::new()))
    }
    fn create_folder(
        &mut self,
        account: &str,
        anchor: &str,
        parent: Option<&str>,
        name: &str,
    ) -> Result<MutationResult> {
        self.producer(account)?
            .enqueue_collection_create(anchor, name, parent, None)?;
        Ok(MutationResult::Queued(String::new()))
    }
    fn compose(&mut self, draft: &Draft, send: bool) -> Result<MutationResult> {
        if send {
            draft.recipients()?;
        }
        let account = self
            .accounts
            .iter()
            .find(|a| a.id == draft.account)
            .context("Unknown sending account")?;
        let raw = draft.mime(&account.email)?;
        let folders = self.folders()?;
        let anchor = folders
            .iter()
            .find(|f| {
                f.account == draft.account
                    && if send {
                        f.role.as_deref() == Some("inbox")
                    } else {
                        f.role.as_deref() == Some("drafts")
                    }
            })
            .or_else(|| {
                send.then(|| folders.iter().find(|f| f.account == draft.account))
                    .flatten()
            })
            .context("No suitable folder (configure a Drafts folder to save drafts)")?;
        let mut producer = self.producer(&draft.account)?;
        let hash = producer.hash(&raw);
        let action = if send {
            let source =
                producer.performer(&anchor.id, io_pimdir::capability::MAIL_SUBMIT, None)?;
            let copy = folders
                .iter()
                .find(|f| f.account == draft.account && f.role.as_deref() == Some("sent"))
                .map(|f| f.id.clone())
                .filter(|_| {
                    producer
                        .performer(
                            &anchor.id,
                            io_pimdir::capability::MAIL_SUBMIT_COPY,
                            Some(&source),
                        )
                        .is_ok()
                });
            PimdirAction::Unknown { kind: "submit".into(), payload: serde_json::json!({"v":1,"from":account.email,"rcpts":draft.recipients()?,"subject":draft.subject,"source":source,"copy":copy,"object":hash.0}).to_string(), object_hash: Some(hash.clone()) }
        } else {
            PimdirAction::Add {
                link_id: None,
                flags: PimdirFlags::Known(
                    ["\\Draft".into(), "\\Seen".into()].into_iter().collect(),
                ),
                object: Some(hash.clone()),
            }
        };
        producer.check(&anchor.id, &action)?;
        let mut writer = producer.blobs().writer()?;
        writer.write_all(&raw)?;
        let size = writer.commit(&hash)?;
        producer.enqueue(
            &anchor.id,
            &action,
            Some(&PimdirObject {
                hash,
                size: size.try_into()?,
            }),
        )?;
        Ok(MutationResult::Queued(if send {
            String::new()
        } else {
            "Draft saved".into()
        }))
    }
    fn pending(&self) -> Result<Vec<PendingMail>> {
        let mut lines = vec![];
        for a in &self.accounts {
            let reader = self.reader(&a.id)?;
            for op in reader.list_pending_actions()? {
                if matches!(&op.action, PimdirAction::Unknown { kind, .. } if kind == "submit") {
                    lines.push(PendingMail {
                        account: a.id.clone(),
                        id: op.id,
                        error: None,
                    });
                }
            }
            for op in reader.parked_actions()? {
                if op.action == "submit" {
                    lines.push(PendingMail {
                        account: a.id.clone(),
                        id: op.id,
                        error: Some(op.error),
                    });
                }
            }
        }
        Ok(lines)
    }
}

pub struct DemoStore {
    accounts: Vec<Account>,
    folders: Vec<Folder>,
    messages: Vec<Message>,
    raw: HashMap<(String, i64), Vec<u8>>,
}
impl Default for DemoStore {
    fn default() -> Self {
        Self::new()
    }
}
impl DemoStore {
    pub fn new() -> Self {
        let accounts = vec![
            Account {
                id: "work".into(),
                label: "Work".into(),
                email: "you@company.example".into(),
            },
            Account {
                id: "personal".into(),
                label: "Personal".into(),
                email: "you@example.com".into(),
            },
        ];
        let mut folders = vec![];
        let mut messages = vec![];
        let mut raw = HashMap::new();
        let subjects = [
            "Release planning — Kubernetes 1.35",
            "Incident follow-up: networking",
            "Re: Review notes",
            "Weekly infrastructure report",
            "Bienvenue dans MailTUI ✨",
            "Dinner on Friday?",
            "Design review: local-first mail",
            "Your October statement",
            "日本語 · Unicode et café",
            "Maintenance window tonight",
        ];
        let senders = [
            "alice@example.com",
            "ops@example.com",
            "bob@example.com",
            "alerts@example.com",
        ];
        for a in &accounts {
            for (id, parent, role) in [
                ("INBOX", None, "inbox"),
                ("Projects", None, "other"),
                ("Projects/Rust", Some("Projects"), "other"),
                ("Projects/Rust/MailTUI", Some("Projects/Rust"), "other"),
                ("Sent", None, "sent"),
                ("Archive", None, "archive"),
                ("Trash", None, "trash"),
            ] {
                folders.push(Folder {
                    account: a.id.clone(),
                    id: id.into(),
                    name: id.rsplit('/').next().unwrap().into(),
                    parent: parent.map(str::to_string),
                    role: Some(role.into()),
                    total: 0,
                    unread: 0,
                });
            }
            for seq in 1..=72 {
                let ix = (seq as usize - 1) % subjects.len();
                let folder = if seq <= 55 {
                    "INBOX"
                } else if seq <= 64 {
                    "Projects/Rust/MailTUI"
                } else {
                    "Archive"
                };
                let subject = subjects[ix];
                let sender = senders[ix % senders.len()];
                let body = format!(
                    "Hello,\n\n{subject}\n\nThis message is available entirely offline.\nUse :accounts and :folders to explore your mail.\n\nIncident report: Kubernetes networking is healthy again.\nNo email will be sent from demo mode.\n\nDocumentation: https://pimalaya.org\n\nBest,\nAlice\n"
                );
                let mime = if ix == 0 {
                    include_bytes!("../tests/fixtures/related.eml").to_vec()
                } else {
                    format!("From: {sender}\r\nTo: {}\r\nSubject: {subject}\r\nMessage-ID: <demo-{seq}-{}@example.com>\r\nDate: Sun, 18 Oct 2026 09:14:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}", a.email, a.id).into_bytes()
                };
                raw.insert((a.id.clone(), seq), mime.clone());
                messages.push(Message {
                    id: MessageId {
                        account: a.id.clone(),
                        folder: folder.into(),
                        seq,
                    },
                    sender: sender.into(),
                    subject: subject.into(),
                    date: format!(
                        "2026-10-{:02}T{:02}:14:00Z",
                        18 - (seq / 24),
                        23 - (seq % 24)
                    ),
                    seen: seq % 3 == 0,
                    flagged: seq % 11 == 0,
                    attachment: ix == 0,
                    size: mime.len() as u64,
                });
            }
        }
        Self {
            accounts,
            folders,
            messages,
            raw,
        }
    }
}
impl MailStore for DemoStore {
    fn create_folder(
        &mut self,
        account: &str,
        _: &str,
        parent: Option<&str>,
        name: &str,
    ) -> Result<MutationResult> {
        anyhow::ensure!(
            !name.trim().is_empty() && !name.chars().any(|c| c.is_control() || c == '/'),
            "Enter a folder name without slashes"
        );
        anyhow::ensure!(
            self.accounts.iter().any(|a| a.id == account),
            "Unknown account"
        );
        anyhow::ensure!(
            parent.is_none_or(|id| self
                .folders
                .iter()
                .any(|f| f.account == account && f.id == id)),
            "Unknown parent folder"
        );
        let id = parent
            .map(|p| format!("{p}/{name}"))
            .unwrap_or_else(|| name.into());
        anyhow::ensure!(
            !self
                .folders
                .iter()
                .any(|f| f.account == account && f.id == id),
            "Folder already exists"
        );
        self.folders.push(Folder {
            account: account.into(),
            id,
            name: name.into(),
            parent: parent.map(str::to_string),
            role: None,
            total: 0,
            unread: 0,
        });
        Ok(MutationResult::AppliedLocal)
    }
    fn compose(&mut self, draft: &Draft, send: bool) -> Result<MutationResult> {
        let account = self
            .accounts
            .iter()
            .find(|a| a.id == draft.account)
            .context("Unknown account")?;
        if send {
            draft.recipients()?;
        }
        let raw = draft.mime(&account.email)?;
        let sender = account.email.clone();
        let folder = if send { "Sent" } else { "Drafts" };
        if !self
            .folders
            .iter()
            .any(|f| f.account == draft.account && f.id == folder)
        {
            self.create_folder(&draft.account, "INBOX", None, folder)?;
        }
        let seq = self.messages.iter().map(|m| m.id.seq).max().unwrap_or(0) + 1;
        let parsed = mail_parser::MessageParser::default()
            .parse(&raw)
            .context("Invalid composed MIME")?;
        let date = parsed.date().map(|d| d.to_rfc3339()).unwrap_or_default();
        self.messages.push(Message {
            id: MessageId {
                account: draft.account.clone(),
                folder: folder.into(),
                seq,
            },
            sender,
            subject: draft.subject.clone(),
            date,
            seen: true,
            flagged: false,
            attachment: !draft.attachments.is_empty(),
            size: raw.len() as u64,
        });
        self.raw.insert((draft.account.clone(), seq), raw);
        Ok(MutationResult::AppliedLocal)
    }
    fn is_demo(&self) -> bool {
        true
    }
    fn accounts(&self) -> Vec<Account> {
        self.accounts.clone()
    }
    fn folders(&self) -> Result<Vec<Folder>> {
        Ok(self
            .folders
            .iter()
            .cloned()
            .map(|mut f| {
                let msgs: Vec<_> = self
                    .messages
                    .iter()
                    .filter(|m| m.id.account == f.account && m.id.folder == f.id)
                    .collect();
                f.total = msgs.len() as u64;
                f.unread = msgs.iter().filter(|m| !m.seen).count() as u64;
                f
            })
            .collect())
    }
    fn messages(&self, scope: &Scope, cursor: Option<&Cursor>, limit: usize) -> Result<Page> {
        let folders = self.folders()?;
        let matches = |m: &&Message| {
            folders
                .iter()
                .any(|f| f.account == m.id.account && f.id == m.id.folder && in_scope(f, scope))
        };
        let total = self.messages.iter().filter(matches).count() as u64;
        let out = self
            .messages
            .iter()
            .filter(matches)
            .filter(|m| {
                cursor
                    .and_then(|c| c.get(&(m.id.account.clone(), m.id.folder.clone())))
                    .is_none_or(|(d, n)| (&m.date, m.id.seq) < (d, *n))
            })
            .cloned()
            .collect();
        Ok(merge_page(
            out,
            cursor.cloned().unwrap_or_default(),
            total,
            limit,
        ))
    }
    fn raw(&self, id: &MessageId) -> Result<Vec<u8>> {
        self.raw
            .get(&(id.account.clone(), id.seq))
            .cloned()
            .context("Stale demo message")
    }
    fn pending(&self) -> Result<Vec<PendingMail>> {
        Ok(vec![])
    }
    fn mutate(&mut self, ids: &[MessageId], action: &Mutation) -> Result<MutationResult> {
        for id in ids {
            if let Mutation::Move(to) | Mutation::Copy(to) = action
                && !self
                    .folders
                    .iter()
                    .any(|f| f.account == id.account && f.id == *to)
            {
                bail!("Destination folder does not exist in {}", id.account);
            }
            if !self.messages.iter().any(|m| &m.id == id) {
                bail!("Stale demo message");
            }
        }
        for id in ids {
            let index = self.messages.iter().position(|m| &m.id == id).unwrap();
            match action {
                Mutation::Seen(v) => self.messages[index].seen = *v,
                Mutation::Flagged(v) => self.messages[index].flagged = *v,
                Mutation::Move(to) => self.messages[index].id.folder.clone_from(to),
                Mutation::Trash => self.messages[index].id.folder = "Trash".into(),
                Mutation::Copy(to) => {
                    let mut m = self.messages[index].clone();
                    m.id.folder.clone_from(to);
                    if !self.messages.iter().any(|existing| existing.id == m.id) {
                        self.messages.push(m);
                    }
                }
            }
        }
        Ok(MutationResult::AppliedLocal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(account: &str, id: &str, name: &str, role: Option<&str>) -> Folder {
        Folder {
            account: account.into(),
            id: id.into(),
            name: name.into(),
            role: role.map(Into::into),
            parent: None,
            total: 0,
            unread: 0,
        }
    }

    #[test]
    fn trash_names_support_slash_dot_and_namespaced_ids_without_substring_matches() {
        for name in [
            "Trash",
            "Trash/",
            "INBOX.Trash",
            "INBOX/Trash",
            ".Trash/",
            "INBOX.tRaSh/",
            "prefix/INBOX.Trash/",
        ] {
            let folders = [folder("work", name, name, None)];
            assert_eq!(trash_folder(&folders, "work").unwrap().id, name);
        }
        let folders = [folder(
            "work",
            "namespace/INBOX.Trash/",
            "Localized name",
            None,
        )];
        assert_eq!(
            trash_folder(&folders, "work").unwrap().id,
            "namespace/INBOX.Trash/"
        );
        for name in [
            "NotTrash",
            "Trash-old",
            "Trash/Projects",
            "Trash.Children",
            "INBOX",
            "Archive",
        ] {
            assert!(
                trash_folder(&[folder("work", name, name, None)], "work").is_none(),
                "Unexpected Trash match: {name}"
            );
        }
    }

    #[test]
    fn declared_trash_wins_over_names_and_never_uses_another_accounts_folder() {
        let mut folders = vec![
            folder("personal", "Deleted", "Deleted", Some("trash")),
            folder("work", "INBOX.Trash", "INBOX.Trash", None),
            folder("work", "Trash/", "Trash/", None),
            folder("work", "Corbeille", "Corbeille", Some("\\Trash")),
        ];
        assert_eq!(trash_folder(&folders, "work").unwrap().id, "Corbeille");
        folders.reverse();
        assert_eq!(trash_folder(&folders, "work").unwrap().id, "Corbeille");
        folders.retain(|f| f.role.is_none());
        assert_eq!(trash_folder(&folders, "work").unwrap().id, "Trash/");
        assert!(trash_folder(&folders, "missing").is_none());
        assert!(
            trash_folder(
                &[folder("personal", "Trash", "Trash", Some("trash"))],
                "work"
            )
            .is_none()
        );
    }
}

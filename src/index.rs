//! MailTUI-owned durable full-text index. Never modifies the upstream store.
use crate::{
    config::AccountConfig,
    domain::*,
    search::{Query, SearchIndex},
    storage::{MAX_MIME, MailStore, PAGE_SIZE},
};
use anyhow::{Result, bail};
use io_pimdir::{
    client::reader::{PimdirItem, PimdirReader},
    summary::PimdirSummary,
};
use rusqlite::{OptionalExtension, params};
use std::{
    collections::HashSet,
    hash::{Hash, Hasher},
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub fn path(accounts: &[AccountConfig]) -> Result<PathBuf> {
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    let directory = root.join("mailtui");
    if !directory.exists() {
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let mut sources: Vec<_> = accounts.iter().collect();
    sources.sort_by(|a, b| a.id.cmp(&b.id));
    for account in sources {
        account.id.hash(&mut hash);
        account.store.hash(&mut hash);
    }
    Ok(directory.join(format!("search-{:016x}.sqlite3", hash.finish())))
}

impl SearchIndex {
    fn remove_key(&self, key: &str) -> Result<()> {
        let transaction = self.db.unchecked_transaction()?;
        self.remove_key_inner(key)?;
        transaction.commit()?;
        Ok(())
    }
    fn remove_key_inner(&self, key: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM thread_links WHERE key=?1", [key])?;
        self.db
            .execute("DELETE FROM thread_headers WHERE key=?1", [key])?;
        if let Some(row) = self
            .db
            .query_row("SELECT fts_rowid FROM entries WHERE key=?1", [key], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?
        {
            self.db.execute("DELETE FROM mail WHERE rowid=?1", [row])?;
            self.db.execute("DELETE FROM entries WHERE key=?1", [key])?;
        }
        self.db
            .execute("DELETE FROM missing_bodies WHERE key=?1", [key])?;
        Ok(())
    }
    fn record(
        &self,
        message: &Message,
        text: &str,
        recipients: &str,
        fingerprint: &str,
    ) -> Result<bool> {
        let key = serde_json::to_string(&message.id)?;
        let old: Option<String> = self
            .db
            .query_row(
                "SELECT fingerprint FROM entries WHERE key=?1",
                [&key],
                |row| row.get(0),
            )
            .optional()?;
        if old.as_deref() == Some(fingerprint) {
            return Ok(false);
        }
        let transaction = self.db.unchecked_transaction()?;
        self.remove_key_inner(&key)?;
        self.insert(message, text, recipients)?;
        let row = self.db.last_insert_rowid();
        self.db.execute(
            "INSERT INTO entries VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                key,
                row,
                serde_json::to_string(message)?,
                fingerprint,
                message.id.account,
                message.id.folder,
                message.date
            ],
        )?;
        transaction.commit()?;
        Ok(true)
    }
    pub fn cached_messages(&self, ids: Vec<MessageId>) -> Result<Vec<Message>> {
        let mut statement = self
            .db
            .prepare("SELECT metadata FROM entries WHERE key=?1")?;
        ids.into_iter()
            .filter_map(|id| {
                let key = match serde_json::to_string(&id) {
                    Ok(key) => key,
                    Err(error) => return Some(Err(error.into())),
                };
                match statement
                    .query_row([key], |row| row.get::<_, String>(0))
                    .optional()
                {
                    Ok(Some(value)) => Some(serde_json::from_str(&value).map_err(Into::into)),
                    Ok(None) => None,
                    Err(error) => Some(Err(error.into())),
                }
            })
            .collect()
    }
    pub fn structured(&self, text: &str, account: Option<&str>) -> Result<Vec<Message>> {
        let _snapshot = if self.db.is_autocommit() {
            Some(self.db.unchecked_transaction()?)
        } else {
            None
        };
        let mut query = Query::parse(text)?;
        query.restrict(account);
        self.cached_messages(self.query(&query)?)
    }
    pub fn find(&self, text: &str, account: Option<&str>) -> Result<Vec<Message>> {
        let _snapshot = if self.db.is_autocommit() {
            Some(self.db.unchecked_transaction()?)
        } else {
            None
        };
        if text.contains('"')
            || text.split_whitespace().any(|word| {
                [
                    "from:", "to:", "subject:", "account:", "folder:", "is:", "has:", "after:",
                    "before:",
                ]
                .iter()
                .any(|prefix| word.starts_with(prefix))
            })
        {
            return self.structured(text, account);
        }
        self.literal(text, account)
    }
    /// Literal text mode never interprets punctuation as search operators.
    pub fn literal(&self, text: &str, account: Option<&str>) -> Result<Vec<Message>> {
        let _snapshot = if self.db.is_autocommit() {
            Some(self.db.unchecked_transaction()?)
        } else {
            None
        };
        let mut found = if text.trim().is_empty() {
            vec![]
        } else {
            let mut query = Query::literal(text);
            query.prefix = true;
            query.restrict(account);
            self.cached_messages(self.query(&query)?)?
        };
        let existing: HashSet<_> = found.iter().map(|message| message.id.clone()).collect();
        if found.len() >= 200 {
            return Ok(found);
        }
        let mut statement = self.db.prepare(
            "SELECT metadata FROM entries WHERE (?1 IS NULL OR account=?1) ORDER BY date DESC",
        )?;
        let mut candidates = vec![];
        for row in statement.query_map([account], |row| row.get::<_, String>(0))? {
            let message: Message = serde_json::from_str(&row?)?;
            if existing.contains(&message.id) {
                continue;
            }
            if let Some(score) = crate::app::fuzzy_score(&message.visible_text(), text) {
                candidates.push((score, message));
                if text.trim().is_empty() && candidates.len() >= 200 {
                    break;
                }
                if candidates.len() >= 400 {
                    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.date.cmp(&a.1.date)));
                    candidates.truncate(200);
                }
            }
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.date.cmp(&a.1.date)));
        found.extend(
            candidates
                .into_iter()
                .take(200 - found.len())
                .map(|(_, message)| message),
        );
        Ok(found)
    }
    pub fn fill_store(&self, store: &dyn MailStore) -> Result<()> {
        let mut cursor = None;
        loop {
            let page = store.messages(&Scope::default(), cursor.as_ref(), PAGE_SIZE)?;
            for message in page.messages {
                let parsed = store
                    .raw(&message.id)
                    .and_then(|raw| crate::mime::parse(&raw))
                    .ok();
                let text: String = parsed
                    .as_ref()
                    .map(|parsed| parsed.text.clone())
                    .unwrap_or_default();
                self.record(
                    &message,
                    &text,
                    parsed
                        .as_ref()
                        .map(|parsed| parsed.recipients.as_str())
                        .unwrap_or(""),
                    &serde_json::to_string(&message)?,
                )?;
                self.record_thread(
                    &message.id,
                    parsed
                        .as_ref()
                        .map(|p| p.thread_tokens.as_slice())
                        .unwrap_or_default(),
                )?;
            }
            cursor = page.next;
            if cursor.is_none() {
                return Ok(());
            }
        }
    }
    fn put_item(
        &self,
        reader: &PimdirReader,
        account: &str,
        folder: &str,
        label: &str,
        item: PimdirItem,
    ) -> Result<bool> {
        if item.retention.is_some() {
            return Ok(false);
        }
        let summary = match item.summary {
            Some(PimdirSummary::Mail(summary)) => summary,
            _ => Default::default(),
        };
        let message = Message {
            id: MessageId {
                account: account.into(),
                folder: folder.into(),
                seq: item.seq,
            },
            sender: summary.sender.unwrap_or_default(),
            subject: summary.subject,
            date: item.sort_key,
            seen: item.flags.contains("\\Seen"),
            flagged: item.flags.contains("\\Flagged"),
            attachment: summary.attachment.unwrap_or(false),
            size: summary.size.unwrap_or(0),
        };
        let body = item
            .object
            .as_ref()
            .and_then(|hash| reader.blobs().reader(hash).ok().flatten());
        let available = body.is_some();
        let fingerprint = format!(
            "{}|{:?}|{available}|{label}",
            serde_json::to_string(&message)?,
            item.object
        );
        let key = serde_json::to_string(&message.id)?;
        let old: Option<String> = self
            .db
            .query_row(
                "SELECT fingerprint FROM entries WHERE key=?1",
                [&key],
                |row| row.get(0),
            )
            .optional()?;
        let has_headers: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM thread_headers WHERE key=?1)",
            [&key],
            |r| r.get(0),
        )?;
        if old.as_deref() == Some(&fingerprint) && has_headers {
            return Ok(false);
        }
        let parsed = body.and_then(|file| {
            let mut raw = vec![];
            file.take((MAX_MIME + 1) as u64)
                .read_to_end(&mut raw)
                .ok()?;
            crate::mime::parse(&raw).ok()
        });
        let text = format!(
            "{label}\n{}\n{}",
            parsed
                .as_ref()
                .map(|parsed| parsed
                    .attachments
                    .iter()
                    .map(|attachment| attachment.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" "))
                .unwrap_or_default(),
            parsed
                .as_ref()
                .map(|parsed| parsed.text.clone())
                .unwrap_or_default()
        );
        let changed = self.record(
            &message,
            &text,
            parsed
                .as_ref()
                .map(|parsed| parsed.recipients.as_str())
                .unwrap_or(""),
            &fingerprint,
        )?;
        self.record_thread(
            &message.id,
            parsed
                .as_ref()
                .map(|p| p.thread_tokens.as_slice())
                .unwrap_or_default(),
        )?;
        if item.object.is_some() && !available {
            self.db.execute(
                "INSERT OR REPLACE INTO missing_bodies VALUES(?1,?2,?3,?4)",
                params![key, account, folder, item.seq],
            )?;
        }
        Ok(changed)
    }
    fn record_thread(&self, id: &MessageId, tokens: &[String]) -> Result<()> {
        let key = serde_json::to_string(id)?;
        let tx = self.db.unchecked_transaction()?;
        self.db
            .execute("DELETE FROM thread_links WHERE key=?1", [&key])?;
        for token in tokens {
            self.db.execute(
                "INSERT OR IGNORE INTO thread_links VALUES(?1,?2)",
                params![key, token],
            )?;
        }
        self.db
            .execute("INSERT OR IGNORE INTO thread_headers VALUES(?1)", [&key])?;
        tx.commit()?;
        Ok(())
    }
    /// Exact RFC header connectivity, including shared missing ancestors. The
    /// account constraint applies at every recursion step, not just afterwards.
    pub fn thread(&self, id: &MessageId, account: Option<&str>) -> Result<(Vec<Message>, bool)> {
        let key = serde_json::to_string(id)?;
        let mut statement = self.db.prepare(
            "WITH RECURSIVE related(key) AS (
            SELECT key FROM entries WHERE key=?1 AND (?2 IS NULL OR account=?2)
            UNION
            SELECT other.key FROM related r JOIN thread_links mine ON mine.key=r.key
            JOIN thread_links other ON other.token=mine.token JOIN entries e ON e.key=other.key
            WHERE (?2 IS NULL OR e.account=?2) LIMIT 51)
            SELECT metadata FROM entries JOIN related USING(key) ORDER BY date",
        )?;
        let rows = statement
            .query_map(params![key, account], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let limited = rows.len() > 50;
        Ok((
            rows.into_iter()
                .take(50)
                .map(|s| serde_json::from_str(&s).map_err(Into::into))
                .collect::<Result<_>>()?,
            limited,
        ))
    }
    fn refresh_account(&self, account: &AccountConfig, stop: &AtomicBool) -> Result<usize> {
        let reader = PimdirReader::open(&account.store)?;
        let cursor = reader.change_cursor()?;
        let mut source = account.store.to_string_lossy().into_owned();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(account.store.join("pimdir.db"))?;
            source.push_str(&format!("#{}:{}", metadata.dev(), metadata.ino()));
        }
        let previous: Option<(String, i64, i64)> = self
            .db
            .query_row(
                "SELECT source,changed,purges FROM cursors WHERE account=?1",
                [&account.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let mut full = previous.as_ref().is_none_or(|(old, changed, purges)| {
            old != &source || *purges != cursor.purges || *changed > cursor.changed
        });
        let since = previous
            .as_ref()
            .map(|(_, changed, _)| *changed)
            .unwrap_or(0);
        if !full && since == cursor.changed {
            return self.refresh_missing(&reader, account, stop);
        }
        full |= !reader.collections_changed_since(since, 1)?.is_empty();
        let folders = reader
            .list_collections()?
            .into_iter()
            .filter(|folder| folder.kind.starts_with("message/rfc822"))
            .map(|folder| (folder.id, folder.name))
            .collect::<std::collections::HashMap<_, _>>();
        let mut updated = 0;
        if full {
            self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS seen_keys(key TEXT PRIMARY KEY); DELETE FROM seen_keys;")?;
            for (folder, label) in &folders {
                let mut after = None;
                loop {
                    anyhow::ensure!(!stop.load(Ordering::Relaxed), "Index update cancelled");
                    let page = reader.list_summaries(
                        folder,
                        after
                            .as_ref()
                            .map(|(date, seq): &(String, i64)| (date.as_str(), *seq)),
                        PAGE_SIZE,
                    )?;
                    if page.is_empty() {
                        break;
                    }
                    after = page.last().map(|item| (item.sort_key.clone(), item.seq));
                    for item in page {
                        anyhow::ensure!(!stop.load(Ordering::Relaxed), "Index update cancelled");
                        let key = serde_json::to_string(&MessageId {
                            account: account.id.clone(),
                            folder: folder.clone(),
                            seq: item.seq,
                        })?;
                        self.db
                            .execute("INSERT OR IGNORE INTO seen_keys VALUES(?1)", [key])?;
                        updated += usize::from(self.put_item(
                            &reader,
                            &account.id,
                            folder,
                            label,
                            item,
                        )?);
                    }
                }
            }
            let stale: Vec<String> = self.db.prepare("SELECT key FROM entries WHERE account=?1 AND key NOT IN (SELECT key FROM seen_keys)")?.query_map([&account.id], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?;
            for key in stale {
                self.remove_key(&key)?;
                updated += 1;
            }
        } else {
            let mut after = since;
            loop {
                anyhow::ensure!(!stop.load(Ordering::Relaxed), "Index update cancelled");
                let page = reader.items_changed_since(after, PAGE_SIZE)?;
                if page.is_empty() {
                    break;
                }
                after = page.last().unwrap().changed;
                for change in page {
                    if change.changed > cursor.changed {
                        continue;
                    }
                    let key = serde_json::to_string(&MessageId {
                        account: account.id.clone(),
                        folder: change.collection.clone(),
                        seq: change.seq,
                    })?;
                    if change.deleted || !folders.contains_key(&change.collection) {
                        self.remove_key(&key)?;
                        updated += 1;
                    } else if let Some(item) = reader.get_item(&change.collection, change.seq)? {
                        updated += usize::from(self.put_item(
                            &reader,
                            &account.id,
                            &change.collection,
                            &folders[&change.collection],
                            item,
                        )?);
                    }
                }
                if after >= cursor.changed {
                    break;
                }
            }
        }
        self.db.execute("INSERT INTO cursors VALUES(?1,?2,?3,?4) ON CONFLICT(account) DO UPDATE SET source=excluded.source,changed=excluded.changed,purges=excluded.purges", params![account.id, source, cursor.changed, cursor.purges])?;
        Ok(updated + self.refresh_missing(&reader, account, stop)?)
    }
    fn refresh_missing(
        &self,
        reader: &PimdirReader,
        account: &AccountConfig,
        stop: &AtomicBool,
    ) -> Result<usize> {
        let rows: Vec<(String, String, i64)> = self
            .db
            .prepare("SELECT key,folder,seq FROM missing_bodies WHERE account=?1")?
            .query_map([&account.id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        if rows.is_empty() {
            return Ok(0);
        }
        let labels: std::collections::HashMap<_, _> = reader
            .list_collections()?
            .into_iter()
            .map(|folder| (folder.id, folder.name))
            .collect();
        let mut updated = 0;
        for (key, folder, seq) in rows {
            anyhow::ensure!(!stop.load(Ordering::Relaxed), "Index update cancelled");
            if let Some(item) = reader.get_item(&folder, seq)? {
                updated += usize::from(self.put_item(
                    reader,
                    &account.id,
                    &folder,
                    labels.get(&folder).map(String::as_str).unwrap_or(&folder),
                    item,
                )?);
            } else {
                self.remove_key(&key)?;
                updated += 1;
            }
        }
        Ok(updated)
    }
    pub fn refresh_native(&self, accounts: &[AccountConfig], stop: &AtomicBool) -> Result<usize> {
        let present: HashSet<_> = accounts.iter().map(|account| account.id.as_str()).collect();
        let rows: Vec<String> = self
            .db
            .prepare("SELECT DISTINCT account FROM entries")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut removed = 0;
        for account in rows {
            if !present.contains(account.as_str()) {
                let keys: Vec<String> = self
                    .db
                    .prepare("SELECT key FROM entries WHERE account=?1")?
                    .query_map([&account], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                for key in keys {
                    self.remove_key(&key)?;
                    removed += 1;
                }
                self.db
                    .execute("DELETE FROM cursors WHERE account=?1", [&account])?;
            }
        }
        let mut count = removed;
        let mut errors = vec![];
        for account in accounts {
            anyhow::ensure!(!stop.load(Ordering::Relaxed), "Index update cancelled");
            match self.refresh_account(account, stop) {
                Ok(updated) => count += updated,
                Err(error) => errors.push(format!("{}: {error:#}", account.id)),
            }
        }
        if !errors.is_empty() {
            bail!("{}", errors.join(" · "));
        }
        Ok(count)
    }
}

pub struct Maintenance {
    sender: tokio::sync::mpsc::Sender<()>,
    pub events: tokio::sync::mpsc::Receiver<Result<usize, String>>,
    stop: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}
impl Maintenance {
    pub fn start(path: PathBuf, accounts: Vec<AccountConfig>) -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::channel(1);
        let (events, receiver) = tokio::sync::mpsc::channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let task = tokio::task::spawn_blocking(move || {
            while requests.blocking_recv().is_some() && !cancelled.load(Ordering::Relaxed) {
                let result = SearchIndex::open(&path)
                    .and_then(|index| index.refresh_native(&accounts, &cancelled))
                    .map_err(|error| format!("{error:#}"));
                if events.blocking_send(result).is_err() {
                    break;
                }
            }
        });
        Self {
            sender,
            events: receiver,
            stop,
            task,
        }
    }
    pub fn request(&self) -> bool {
        self.sender.try_send(()).is_ok()
    }
}
impl Drop for Maintenance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc_threads_cross_folders_keep_account_scope_and_survive_missing_ancestors() {
        let index = SearchIndex::new().unwrap();
        let mut messages = vec![];
        for (account, folder, seq, tokens) in [
            ("work", "INBOX", 1, vec!["root", "one"]),
            ("work", "Sent", 2, vec!["root", "one", "two"]),
            ("personal", "INBOX", 1, vec!["root", "personal"]),
            ("work", "INBOX", 3, vec!["unrelated"]),
        ] {
            let message = Message {
                id: MessageId {
                    account: account.into(),
                    folder: folder.into(),
                    seq,
                },
                sender: "alice@example.com".into(),
                subject: "Same subject".into(),
                date: format!("2026-10-{seq:02}"),
                seen: false,
                flagged: false,
                attachment: false,
                size: 0,
            };
            index
                .record(
                    &message,
                    "body",
                    "recipient@example.com",
                    &format!("{account}-{seq}"),
                )
                .unwrap();
            index
                .record_thread(
                    &message.id,
                    &tokens.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                )
                .unwrap();
            messages.push(message);
        }
        let (found, limited) = index.thread(&messages[0].id, Some("work")).unwrap();
        assert!(!limited);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|m| m.id.account == "work"));
        assert_eq!(index.thread(&messages[0].id, None).unwrap().0.len(), 3);
        index
            .remove_key(&serde_json::to_string(&messages[0].id).unwrap())
            .unwrap();
        assert_eq!(
            index.thread(&messages[1].id, Some("work")).unwrap().0.len(),
            1
        );
        assert!(
            index
                .thread(&messages[1].id, Some("personal"))
                .unwrap()
                .0
                .is_empty()
        );
        let parsed = crate::mime::parse(b"Message-ID: <reply@example.com>\r\nIn-Reply-To: <parent@example.com>\r\nReferences: <root@example.com> <parent@example.com>\r\n\r\nreply").unwrap();
        assert_eq!(
            parsed.thread_tokens,
            vec![
                "parent@example.com",
                "reply@example.com",
                "root@example.com"
            ]
        );
    }
}

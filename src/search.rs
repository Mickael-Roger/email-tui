use crate::{
    domain::*,
    mime,
    storage::{MailStore, PAGE_SIZE},
};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params, params_from_iter, types::Value};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Deliberately small grammar: AND terms, quoted phrases and typed operators.
/// Boolean/negation syntax is rejected instead of acquiring different meaning.
#[derive(Debug)]
pub struct Query {
    terms: Vec<String>,
    filters: Vec<(String, String)>,
    pub(crate) prefix: bool,
}
impl Query {
    pub(crate) fn literal(source: &str) -> Self {
        Self {
            terms: source.split_whitespace().map(str::to_string).collect(),
            filters: vec![],
            prefix: true,
        }
    }
    pub(crate) fn restrict(&mut self, account: Option<&str>) {
        if let Some(account) = account {
            self.filters.push(("account".into(), account.into()));
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        let mut query = Self {
            terms: vec![],
            filters: vec![],
            prefix: false,
        };
        for (term, literal) in search_words(s)? {
            if !literal && (["OR", "AND", "NOT"].contains(&term.as_str()) || term.starts_with('-'))
            {
                bail!("Use implicit AND terms; OR, NOT and negation are not supported in v0.1");
            }
            if let Some((field, value)) = term.split_once(':').filter(|_| !literal) {
                if ![
                    "from", "to", "subject", "account", "folder", "is", "has", "after", "before",
                ]
                .contains(&field)
                    || value.is_empty()
                {
                    bail!("Unknown or empty search operator: {field}");
                }
                if field == "is" && !["read", "unread", "flagged"].contains(&value) {
                    bail!("is: expects read, unread or flagged");
                }
                if field == "has" && value != "attachment" {
                    bail!("has: expects attachment");
                }
                if ["after", "before"].contains(&field) && !valid_date(value) {
                    bail!("Dates must be valid YYYY-MM-DD calendar dates (UTC)");
                }
                query.filters.push((field.into(), value.into()));
            } else if !term.is_empty() {
                query.terms.push(term);
            }
        }
        if query.terms.is_empty() && query.filters.is_empty() {
            bail!("Search query is empty");
        }
        Ok(query)
    }
}

/// Search is not shell syntax: apostrophes inside words are ordinary text.
/// Keep quoted values intact, including parentheses, and distinguish a quoted
/// literal ("OR", "subject:example") from an actual operator.
fn search_words(source: &str) -> Result<Vec<(String, bool)>> {
    let mut words = vec![];
    let mut value = String::new();
    let mut quote = None;
    let mut literal = false;
    let mut started = false;
    let mut chars = source.chars();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\\' && q == '"' {
                value.push(chars.next().context("Malformed search escape")?);
            } else {
                value.push(c);
            }
        } else if c.is_whitespace() {
            if started {
                words.push((std::mem::take(&mut value), literal));
                literal = false;
                started = false;
            }
        } else if c == '"' || (c == '\'' && (value.is_empty() || value.ends_with(':'))) {
            if !started {
                literal = true;
            }
            quote = Some(c);
            started = true;
        } else {
            if c == '(' || c == ')' {
                bail!(
                    "Boolean groups are not supported; quote parentheses inside subjects or phrases"
                );
            }
            if c == '\\' {
                value.push(chars.next().context("Malformed search escape")?);
            } else {
                value.push(c);
            }
            started = true;
        }
    }
    if quote.is_some() {
        bail!("Malformed search quoting: close the quoted value");
    }
    if started {
        words.push((value, literal));
    }
    Ok(words)
}
fn valid_date(s: &str) -> bool {
    let parts: Vec<_> = s.split('-').collect();
    if parts.len() != 3
        || s.len() != 10
        || parts[0].len() != 4
        || parts[1].len() != 2
        || parts[2].len() != 2
    {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (
        parts[0].parse::<u32>(),
        parts[1].parse::<u32>(),
        parts[2].parse::<u32>(),
    ) else {
        return false;
    };
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = match m {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    y > 0 && d > 0 && d <= days
}

pub struct SearchIndex {
    pub(crate) db: Connection,
}
impl SearchIndex {
    pub fn new() -> Result<Self> {
        let db = Connection::open_in_memory()?;
        Self::initialize(&db)?;
        Ok(Self { db })
    }
    fn initialize(db: &Connection) -> Result<()> {
        db.busy_timeout(std::time::Duration::from_secs(10))?;
        let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        db.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS mail USING fts5(id UNINDEXED, account UNINDEXED, folder UNINDEXED, sender, recipient, subject, body, date UNINDEXED, seen UNINDEXED, flagged UNINDEXED, attachment UNINDEXED, tokenize='unicode61 remove_diacritics 2');
            CREATE TABLE IF NOT EXISTS entries(key TEXT PRIMARY KEY, fts_rowid INTEGER NOT NULL, metadata TEXT NOT NULL, fingerprint TEXT NOT NULL, account TEXT NOT NULL, folder TEXT NOT NULL, date TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS entries_account_date ON entries(account,date DESC);
            CREATE TABLE IF NOT EXISTS cursors(account TEXT PRIMARY KEY, source TEXT NOT NULL, changed INTEGER NOT NULL, purges INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS missing_bodies(key TEXT PRIMARY KEY, account TEXT NOT NULL, folder TEXT NOT NULL, seq INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS thread_headers(key TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS thread_links(key TEXT NOT NULL, token TEXT NOT NULL, PRIMARY KEY(key,token));
            CREATE INDEX IF NOT EXISTS thread_links_token ON thread_links(token);
            PRAGMA user_version=3;")?;
        if version < 3 {
            db.execute("DELETE FROM cursors", [])?;
        }
        Ok(())
    }
    pub fn open(path: &std::path::Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .open(path)?;
        }
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(10))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        Self::initialize(&db)?;
        Ok(Self { db })
    }
    pub fn reader(path: &std::path::Path) -> Result<Self> {
        let db = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self { db })
    }
    pub fn insert(&self, message: &Message, text: &str, recipients: &str) -> Result<()> {
        let id = serde_json::to_string(&message.id)?;
        self.db.execute(
            "INSERT INTO mail VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                id,
                message.id.account,
                message.id.folder,
                message.sender,
                recipients,
                message.subject,
                text,
                message.date,
                message.seen as i64,
                message.flagged as i64,
                message.attachment as i64
            ],
        )?;
        Ok(())
    }
    pub fn query(&self, query: &Query) -> Result<Vec<MessageId>> {
        let mut sql = "SELECT id FROM mail WHERE 1=1".to_string();
        let mut values: Vec<Value> = vec![];
        if !query.terms.is_empty() {
            sql.push_str(" AND mail MATCH ?");
            values.push(
                query
                    .terms
                    .iter()
                    .map(|s| {
                        format!(
                            "\"{}\"{}",
                            s.replace('"', "\"\""),
                            if query.prefix { "*" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" AND ")
                    .into(),
            );
        }
        for (field, value) in &query.filters {
            match field.as_str() {
                "account" | "folder" => {
                    sql.push_str(&format!(" AND {field} = ?"));
                    values.push(value.clone().into());
                }
                "from" | "to" | "subject" => {
                    let column = match field.as_str() {
                        "from" => "sender",
                        "to" => "recipient",
                        _ => "subject",
                    };
                    sql.push_str(&format!(" AND instr(lower({column}), lower(?)) > 0"));
                    values.push(value.clone().into());
                }
                "is" => {
                    sql.push_str(match value.as_str() {
                        "read" => " AND seen = 1",
                        "unread" => " AND seen = 0",
                        _ => " AND flagged = 1",
                    });
                }
                "has" => sql.push_str(" AND attachment = 1"),
                "after" => {
                    sql.push_str(" AND substr(date,1,10) > ?");
                    values.push(value.clone().into());
                }
                "before" => {
                    sql.push_str(" AND substr(date,1,10) < ?");
                    values.push(value.clone().into());
                }
                _ => unreachable!(),
            }
        }
        sql.push_str(if query.prefix && !query.terms.is_empty() {
            " ORDER BY bm25(mail,0,0,0,2,1,5,1,0,0,0,0), date DESC LIMIT 200"
        } else {
            " ORDER BY date DESC LIMIT 200"
        });
        let mut statement = self.db.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values), |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
}

pub fn run(
    store: &dyn MailStore,
    source: &str,
    account: Option<&str>,
    generation: u64,
    current: &Arc<AtomicU64>,
    mut progress: impl FnMut(u64),
) -> Result<Vec<Message>> {
    let mut query = Query::parse(source)?;
    // Enforce the active account before LIMIT, even when the query itself
    // contains an account operator. Account switching never broadens access.
    if let Some(account) = account {
        query.filters.push(("account".into(), account.into()));
    }
    let index = SearchIndex::new()?;
    let mut cursor = None;
    let mut count = 0;
    loop {
        if current.load(Ordering::Relaxed) != generation {
            bail!("Search cancelled");
        }
        let page = store.messages(&Scope::default(), cursor.as_ref(), PAGE_SIZE)?;
        for m in &page.messages {
            if current.load(Ordering::Relaxed) != generation {
                bail!("Search cancelled");
            }
            // Missing bodies still contribute metadata; no hydration/network request.
            let parsed = store.raw(&m.id).and_then(|raw| mime::parse(&raw)).ok();
            let text: String = parsed
                .as_ref()
                .map(|p| p.text.chars().take(65_536).collect())
                .unwrap_or_default();
            index.insert(
                m,
                &text,
                parsed.as_ref().map(|p| p.recipients.as_str()).unwrap_or(""),
            )?;
            count += 1;
        }
        progress(count);
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    let ids: std::collections::HashSet<_> = index.query(&query)?.into_iter().collect();
    let mut found = vec![];
    let mut cursor = None;
    loop {
        if current.load(Ordering::Relaxed) != generation {
            bail!("Search cancelled");
        }
        let page = store.messages(&Scope::default(), cursor.as_ref(), PAGE_SIZE)?;
        found.extend(page.messages.into_iter().filter(|m| ids.contains(&m.id)));
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    Ok(found)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grammar_rejects_boolean_bad_dates_and_injection() {
        for q in [
            "(x OR y)",
            "-from:alice",
            "is:wrong",
            "after:2026-02-30",
            "before:2026-13-01",
            "'unclosed",
        ] {
            assert!(Query::parse(q).is_err(), "{q}");
        }
        assert!(Query::parse("after:2024-02-29").is_ok());
        let index = SearchIndex::new().unwrap();
        assert!(
            index
                .query(&Query::parse("\"' OR 1=1 --\"").unwrap())
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn cross_account_body_search_and_structured_scope() {
        let store = crate::storage::DemoStore::new();
        let current = Arc::new(AtomicU64::new(1));
        let hits = run(
            &store,
            "account:work from:alice@example.com kubernetes",
            None,
            1,
            &current,
            |_| {},
        )
        .unwrap();
        assert!(!hits.is_empty());
        assert!(
            hits.iter()
                .all(|m| m.id.account == "work" && m.sender == "alice@example.com")
        );
        assert!(run(&store, "incident", None, 0, &current, |_| {}).is_err());
    }
}

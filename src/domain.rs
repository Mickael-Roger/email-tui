use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MessageId {
    pub account: String,
    pub folder: String,
    pub seq: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub label: String,
    pub email: String,
}

#[derive(Clone, Debug)]
pub struct Folder {
    pub account: String,
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub role: Option<String>,
    pub total: u64,
    pub unread: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub sender: String,
    pub subject: String,
    pub date: String,
    pub seen: bool,
    pub flagged: bool,
    pub attachment: bool,
    pub size: u64,
}

impl Message {
    pub fn visible_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.id.account, self.id.folder, self.sender, self.subject, self.date
        )
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    pub messages: Vec<Message>,
    pub total: u64,
    pub next: Option<Cursor>,
}

pub type Cursor = std::collections::BTreeMap<(String, String), (String, i64)>;

#[derive(Clone, Debug, Default)]
pub struct Scope {
    pub account: Option<String>,
    pub folder: Option<String>,
    pub inbox_only: bool,
}

#[derive(Clone, Debug)]
pub enum Mutation {
    Seen(bool),
    Flagged(bool),
    Move(String),
    Copy(String),
    Trash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationResult {
    AppliedLocal,
    Queued(String),
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingMail {
    pub account: String,
    pub id: i64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub account: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<std::path::PathBuf>,
    pub reply: Option<ReplyHeaders>,
}
#[derive(Clone, Debug, Default)]
pub struct ReplyHeaders {
    pub message_id: String,
    pub references: Vec<String>,
}
impl Draft {
    pub fn recipients(&self) -> anyhow::Result<Vec<String>> {
        let recipients: Vec<_> = self
            .to
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        anyhow::ensure!(!recipients.is_empty(), "Enter at least one recipient");
        for address in &recipients {
            anyhow::ensure!(
                address
                    .split_once('@')
                    .is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty())
                    && !address
                        .chars()
                        .any(|c| c.is_whitespace() || c.is_control() || "<>\";".contains(c)),
                "Use comma-separated email addresses (name@example.com)"
            );
        }
        Ok(recipients)
    }
    pub fn mime(&self, from: &str) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(
            !self.subject.chars().any(char::is_control) && !from.chars().any(char::is_control),
            "Headers cannot contain control characters"
        );
        let recipients = if self.to.trim().is_empty() {
            vec![]
        } else {
            self.recipients()?
        };
        let mut builder = mail_builder::MessageBuilder::new()
            .from(from)
            .to(recipients)
            .subject(self.subject.as_str())
            .text_body(self.body.as_str());
        if let Some(reply) = &self.reply {
            anyhow::ensure!(
                !reply.message_id.chars().any(char::is_control)
                    && reply
                        .references
                        .iter()
                        .all(|r| !r.chars().any(char::is_control)),
                "Reply headers cannot contain control characters"
            );
            if !reply.message_id.is_empty() {
                builder = builder.in_reply_to(reply.message_id.as_str());
                let mut references = reply.references.clone();
                if !references.contains(&reply.message_id) {
                    references.push(reply.message_id.clone());
                }
                builder = builder.references(references);
            }
        }
        use anyhow::Context;
        use std::io::Read;
        let mut total = 0;
        for path in &self.attachments {
            let mut bytes = vec![];
            std::fs::File::open(path)
                .with_context(|| format!("Read attachment {}", path.display()))?
                .take((crate::storage::MAX_MIME + 1) as u64)
                .read_to_end(&mut bytes)?;
            total += bytes.len();
            anyhow::ensure!(
                total <= crate::storage::MAX_MIME,
                "Attachments exceed the 16 MiB MIME limit"
            );
            let name = path
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("Invalid attachment filename"))?
                .to_string_lossy()
                .into_owned();
            builder = builder.attachment(crate::files::content_type(path), name, bytes);
        }
        let raw = builder.write_to_vec()?;
        anyhow::ensure!(
            raw.len() <= crate::storage::MAX_MIME,
            "Encoded email exceeds the 16 MiB MIME limit"
        );
        Ok(raw)
    }
}

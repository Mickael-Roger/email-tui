use anyhow::{Context, Result, bail};
use mail_parser::{MessageParser, MimeHeaders};

#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    pub content_type: String,
    pub bytes: std::sync::Arc<[u8]>,
}

fn attachment_bytes(raw: &[u8], part: &mail_parser::MessagePart<'_>) -> std::sync::Arc<[u8]> {
    // Text bodies are charset-normalized for reading. Downloads keep the file's
    // original charset bytes, removing only its MIME transfer encoding.
    if matches!(
        part.body,
        mail_parser::PartType::Text(_) | mail_parser::PartType::Html(_)
    ) && let Some(encoded) = raw.get(part.offset_body as usize..part.offset_end as usize)
    {
        let decoded = match part.encoding {
            mail_parser::Encoding::Base64 => mail_parser::decoders::base64::base64_decode(encoded),
            mail_parser::Encoding::QuotedPrintable => {
                mail_parser::decoders::quoted_printable::quoted_printable_decode(encoded)
            }
            mail_parser::Encoding::None => None,
        };
        return decoded
            .map(std::sync::Arc::from)
            .unwrap_or_else(|| std::sync::Arc::from(encoded));
    }
    std::sync::Arc::from(part.contents())
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub cid: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct ParsedMessage {
    pub thread_tokens: Vec<String>,
    pub text: String,
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
    pub assets: Vec<Asset>,
    pub recipients: String,
}

/// Keep terminal control sequences and bidi overrides out of every TUI string.
pub fn terminal_text(s: &str) -> String {
    s.chars()
        .filter(|c| {
            (!c.is_control() || *c == '\n' || *c == '\t')
                && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

pub fn parse(raw: &[u8]) -> Result<ParsedMessage> {
    if raw.len() > crate::storage::MAX_MIME {
        bail!("MIME exceeds 16 MiB limit");
    }
    let message = MessageParser::default()
        .parse(raw)
        .context("Malformed MIME message")?;
    if message.parts.len() > 512 {
        bail!("MIME exceeds 512 part limit");
    }
    let html = message.body_html(0).map(|s| s.into_owned());
    let text = if let Some(part) = message
        .text_part(0)
        .filter(|p| matches!(p.body, mail_parser::PartType::Text(_)))
    {
        part.text_contents().unwrap_or_default().to_string()
    } else if let Some(html) = &html {
        let safe = ammonia::clean(html);
        html2text::from_read(safe.as_bytes(), 90)
            .unwrap_or_else(|_| "Unable to convert HTML to text".into())
    } else {
        "[No readable text part]".into()
    };
    let mut assets = vec![];
    for part in &message.parts {
        if let Some(cid) = part.content_id() {
            let content_type = part
                .content_type()
                .map(|t| format!("{}/{}", t.c_type, t.c_subtype.as_deref().unwrap_or("")))
                .unwrap_or_default()
                .to_lowercase();
            if ["image/png", "image/jpeg", "image/gif", "image/webp"]
                .contains(&content_type.as_str())
                && part.contents().len() <= 2 * 1024 * 1024
            {
                let cid = normalize_cid(cid);
                if !assets.iter().any(|a: &Asset| a.cid == cid) {
                    assets.push(Asset {
                        cid,
                        content_type,
                        bytes: part.contents().to_vec(),
                    });
                }
            }
        }
    }
    let recipients = message
        .to()
        .map(|a| {
            a.iter()
                .filter_map(|a| a.address())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    Ok(ParsedMessage {
        thread_tokens: {
            let mut tokens: Vec<String> = message
                .references()
                .as_text_list()
                .unwrap_or_default()
                .iter()
                .chain(
                    message
                        .in_reply_to()
                        .as_text_list()
                        .unwrap_or_default()
                        .iter(),
                )
                .map(|s| s.to_string())
                .collect();
            if let Some(id) = message.message_id() {
                tokens.push(id.into());
            }
            // A single Message-ID is represented as Text by some parsers.
            if let Some(id) = message.in_reply_to().as_text() {
                tokens.push(id.into());
            }
            if let Some(id) = message.references().as_text() {
                tokens.push(id.into());
            }
            tokens.sort();
            tokens.dedup();
            tokens.truncate(100);
            tokens
        },
        text: terminal_text(&text),
        html,
        recipients: terminal_text(&recipients),
        attachments: message
            .attachments()
            .enumerate()
            .map(|(index, part)| Attachment {
                name: terminal_text(
                    part.attachment_name()
                        .unwrap_or(&format!("attachment-{}", index + 1)),
                ),
                content_type: part
                    .content_type()
                    .map(|t| {
                        format!(
                            "{}/{}",
                            t.c_type,
                            t.c_subtype.as_deref().unwrap_or("octet-stream")
                        )
                    })
                    .unwrap_or_else(|| "application/octet-stream".into()),
                bytes: attachment_bytes(raw, part),
            })
            .collect(),
        assets,
    })
}

pub fn normalize_cid(s: &str) -> String {
    s.trim()
        .trim_start_matches("cid:")
        .trim_matches(['<', '>'])
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_download_keeps_original_charset_bytes_instead_of_the_readers_utf8() {
        let raw = b"MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nHello\r\n--x\r\nContent-Type: text/plain; charset=iso-8859-1\r\nContent-Disposition: attachment; filename=cafe.txt\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\ncaf=E9\r\n--x--\r\n";
        let parsed = parse(raw).unwrap();
        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(parsed.attachments[0].name, "cafe.txt");
        assert_eq!(&*parsed.attachments[0].bytes, b"caf\xe9");
    }
    #[test]
    fn multipart_plain_preferred_and_cid_decoded() {
        let parsed = parse(include_bytes!("../tests/fixtures/related.eml")).unwrap();
        assert!(parsed.text.contains("Hello team"));
        assert!(!parsed.text.contains("<html>"));
        assert_eq!(parsed.assets[0].cid, "release-logo");
        assert!(parsed.assets[0].bytes.starts_with(b"\x89PNG"));
    }
    #[test]
    fn html_fallback_and_controls() {
        let p = parse(b"Content-Type: text/html\r\n\r\n<h1>Bonjour</h1><script>bad()</script>")
            .unwrap();
        assert!(p.text.contains("Bonjour"));
        assert!(!p.text.contains("bad()"));
        assert_eq!(terminal_text("hello\x1b[2J\u{202e}"), "hello[2J");
    }
}

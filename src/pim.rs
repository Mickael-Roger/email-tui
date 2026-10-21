//! Local vdir collections: RFC5545 calendars and vCard 3/4 address books.
//! Edits patch the original component, preserving unknown properties, timezones,
//! alarms and recurrence exceptions. The filename is deliberately not the UID.
use crate::config::PimCollection;
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_FILE: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub collection: String,
    pub collection_label: String,
    pub uid: String,
    pub summary: String,
    pub start: String,
    pub end: String,
    pub location: String,
    pub description: String,
    pub recurrence: String,
    pub timezone: String,
    pub file: PathBuf,
    pub source: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    pub book: String,
    pub book_label: String,
    pub uid: String,
    pub name: String,
    pub email: String,
    pub phone: String,
    pub organization: String,
    pub file: PathBuf,
    pub source: String,
}
#[derive(Default, Debug)]
pub struct Snapshot {
    pub events: Vec<Event>,
    pub contacts: Vec<Contact>,
    pub warnings: Vec<String>,
}
#[derive(Clone, Debug)]
pub enum Change {
    Load,
    Event {
        collection: PimCollection,
        entry: Event,
    },
    Contact {
        book: PimCollection,
        entry: Contact,
    },
    Delete {
        file: PathBuf,
        source: String,
    },
}
pub fn execute(
    calendars: &[PimCollection],
    books: &[PimCollection],
    change: Change,
) -> Result<Snapshot> {
    match change {
        Change::Load => {}
        Change::Event { collection, entry } => write_event(&collection, &entry)?,
        Change::Contact { book, entry } => write_contact(&book, &entry)?,
        Change::Delete { file, source } => {
            ensure!(
                calendars
                    .iter()
                    .chain(books)
                    .any(|c| file.parent() == Some(c.path.as_path())),
                "Entry outside configured collections"
            );
            check_original(&file, &source)?;
            fs::remove_file(&file).with_context(|| format!("Delete {}", file.display()))?;
        }
    }
    Ok(load(calendars, books))
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(c) => out.push(c),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\\n")
        .replace(';', "\\;")
        .replace(',', "\\,")
}
fn lines(data: &str) -> Vec<String> {
    let mut result = Vec::<String>::new();
    for line in data.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = result.last_mut() {
                last.push_str(&line[1..]);
            }
        } else {
            result.push(line.to_owned());
        }
    }
    result
}
fn property(line: &str) -> Option<(&str, &str, &str)> {
    // Colons in quoted TZID/parameter values are not value separators.
    let mut quoted = false;
    let (index, _) = line.char_indices().find(|(_, c)| {
        if *c == '"' {
            quoted = !quoted;
        }
        *c == ':' && !quoted
    })?;
    let key = &line[..index];
    let name = key.split(';').next()?.rsplit('.').next()?;
    Some((name, key, &line[index + 1..]))
}
fn range(lines: &[String], kind: &str) -> Result<(usize, usize)> {
    let mut start = None;
    let mut depth = 0;
    for (i, line) in lines.iter().enumerate() {
        if line.eq_ignore_ascii_case(&format!("BEGIN:{kind}")) && start.is_none() {
            start = Some(i);
            depth = 1;
            continue;
        }
        if start.is_none() {
            continue;
        }
        if line.to_ascii_uppercase().starts_with("BEGIN:") {
            depth += 1;
        }
        if line.to_ascii_uppercase().starts_with("END:") {
            depth -= 1;
            if depth == 0 {
                let start = start.take().unwrap();
                // Prefer the master VEVENT if exceptions precede it.
                if kind == "VEVENT"
                    && lines[start..i].iter().any(|l| {
                        property(l).is_some_and(|(n, _, _)| n.eq_ignore_ascii_case("RECURRENCE-ID"))
                    })
                {
                    continue;
                }
                return Ok((start, i));
            }
        }
    }
    bail!("Missing or incomplete {kind} component")
}
fn get(lines: &[String], bounds: (usize, usize), key: &str) -> String {
    let mut depth = 0;
    for line in &lines[bounds.0 + 1..bounds.1] {
        if line.to_ascii_uppercase().starts_with("BEGIN:") {
            depth += 1;
        }
        if depth == 0
            && let Some((name, _, value)) = property(line)
            && name.eq_ignore_ascii_case(key)
        {
            return unescape(value);
        }
        if line.to_ascii_uppercase().starts_with("END:") {
            depth -= 1;
        }
    }
    String::new()
}
fn read(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Not a regular PIM file: {}",
        path.display()
    );
    let mut contents = String::new();
    fs::File::open(path)?
        .take(MAX_FILE + 1)
        .read_to_string(&mut contents)?;
    ensure!(
        contents.len() as u64 <= MAX_FILE,
        "PIM file exceeds 4 MiB: {}",
        path.display()
    );
    Ok(contents)
}
pub fn load(calendars: &[PimCollection], books: &[PimCollection]) -> Snapshot {
    let mut snapshot = Snapshot::default();
    for (collections, extension) in [(calendars, "ics"), (books, "vcf")] {
        for collection in collections {
            let entries = match fs::read_dir(&collection.path) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    snapshot
                        .warnings
                        .push(format!("{}: {e}", collection.path.display()));
                    continue;
                }
            };
            for entry in entries {
                let result = (|| -> Result<()> {
                    let entry = entry?;
                    let file = entry.path();
                    if file.extension().and_then(|s| s.to_str()) != Some(extension) {
                        return Ok(());
                    }
                    let source = read(&file).with_context(|| format!("Read {}", file.display()))?;
                    let logical = lines(&source);
                    let bounds = range(
                        &logical,
                        if extension == "ics" {
                            "VEVENT"
                        } else {
                            "VCARD"
                        },
                    )
                    .with_context(|| format!("Parse {}", file.display()))?;
                    let uid = get(&logical, bounds, "UID");
                    if extension == "ics" {
                        ensure!(!uid.is_empty(), "{}: UID missing", file.display());
                        let start = get(&logical, bounds, "DTSTART");
                        ensure!(!start.is_empty(), "Event DTSTART missing");
                        let timezone = logical[bounds.0..bounds.1]
                            .iter()
                            .find_map(|line| {
                                let (name, key, _) = property(line)?;
                                if !name.eq_ignore_ascii_case("DTSTART") {
                                    return None;
                                }
                                key.split(';').find_map(|p| {
                                    p.strip_prefix("TZID=")
                                        .map(|s| s.trim_matches('"').to_string())
                                })
                            })
                            .unwrap_or_default();
                        snapshot.events.push(Event {
                            collection: collection.id.clone(),
                            collection_label: collection.label.clone(),
                            uid,
                            summary: get(&logical, bounds, "SUMMARY"),
                            start,
                            end: get(&logical, bounds, "DTEND"),
                            location: get(&logical, bounds, "LOCATION"),
                            description: get(&logical, bounds, "DESCRIPTION"),
                            recurrence: get(&logical, bounds, "RRULE"),
                            timezone,
                            file,
                            source,
                        });
                    } else {
                        snapshot.contacts.push(Contact {
                            book: collection.id.clone(),
                            book_label: collection.label.clone(),
                            uid,
                            name: get(&logical, bounds, "FN"),
                            email: get(&logical, bounds, "EMAIL"),
                            phone: get(&logical, bounds, "TEL"),
                            organization: get(&logical, bounds, "ORG"),
                            file,
                            source,
                        });
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    snapshot
                        .warnings
                        .push(format!("{}: {error:#}", collection.label));
                }
            }
        }
    }
    snapshot.events.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then(a.summary.cmp(&b.summary))
            .then(a.file.cmp(&b.file))
    });
    snapshot.contacts.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.file.cmp(&b.file))
    });
    snapshot
}

/// Replace only the first top-level property (preserve secondary emails/phones).
fn set(logical: &mut Vec<String>, kind: &str, name: &str, value: &str, date: bool) -> Result<()> {
    let bounds = range(logical, kind)?;
    if get(logical, bounds, name) == value {
        return Ok(());
    }
    let mut depth = 0;
    for index in bounds.0 + 1..bounds.1 {
        let line = &logical[index];
        if line.to_ascii_uppercase().starts_with("BEGIN:") {
            depth += 1;
        }
        if depth == 0
            && let Some((n, key, _)) = property(line)
            && n.eq_ignore_ascii_case(name)
        {
            if value.is_empty() {
                logical.remove(index);
            } else {
                let key = if date && value.len() == 8 {
                    format!("{name};VALUE=DATE")
                } else if date
                    && (key.to_ascii_uppercase().contains("VALUE=DATE") || value.ends_with('Z'))
                {
                    name.to_string()
                } else {
                    key.to_string()
                };
                logical[index] = format!(
                    "{key}:{}",
                    if date {
                        value.to_string()
                    } else {
                        escape(value)
                    }
                );
            }
            return Ok(());
        }
        if line.to_ascii_uppercase().starts_with("END:") {
            depth -= 1;
        }
    }
    if !value.is_empty() {
        let key = if date && value.len() == 8 {
            format!("{name};VALUE=DATE")
        } else {
            name.into()
        };
        logical.insert(
            bounds.1,
            format!(
                "{key}:{}",
                if date {
                    value.to_string()
                } else {
                    escape(value)
                }
            ),
        );
    }
    Ok(())
}
fn serialize(logical: &[String]) -> String {
    let mut result = String::new();
    for line in logical {
        let mut rest = line.as_str();
        let mut limit = 75;
        while rest.len() > limit {
            let mut index = limit;
            while !rest.is_char_boundary(index) {
                index -= 1;
            }
            result.push_str(&rest[..index]);
            result.push_str("\r\n ");
            rest = &rest[index..];
            limit = 74;
        }
        result.push_str(rest);
        result.push_str("\r\n");
    }
    result
}
fn check_original(path: &Path, source: &str) -> Result<()> {
    ensure!(
        read(path)? == source,
        "Entry changed on disk; reload before editing/deleting it"
    );
    Ok(())
}
fn persist(
    collection: &PimCollection,
    file: &Path,
    source: &str,
    uid: &str,
    extension: &str,
    contents: &str,
) -> Result<()> {
    ensure!(
        !uid.is_empty() && !uid.chars().any(char::is_control),
        "Invalid UID"
    );
    fs::create_dir_all(&collection.path)?;
    let new = source.is_empty();
    let path = if new {
        collection.path.join(format!(
            "mailtui-{:016x}.{extension}",
            rand::random::<u64>()
        ))
    } else {
        file.to_path_buf()
    };
    ensure!(
        path.parent() == Some(collection.path.as_path()),
        "Entry outside collection"
    );
    let mut temporary = tempfile::Builder::new()
        .prefix(".mailtui-")
        .suffix(".tmp")
        .tempfile_in(&collection.path)?;
    temporary.write_all(contents.as_bytes())?;
    temporary.as_file().sync_all()?;
    if new {
        temporary.persist_noclobber(&path)?;
    } else {
        check_original(&path, source)?;
        temporary
            .as_file()
            .set_permissions(fs::metadata(&path)?.permissions())?;
        temporary.persist(&path)?;
    }
    fs::File::open(&collection.path)?.sync_all()?;
    Ok(())
}
pub fn valid_date(value: &str) -> bool {
    let raw = value.strip_suffix('Z').unwrap_or(value);
    if raw.len() != 8 && raw.len() != 15 {
        return false;
    }
    if !raw.bytes().enumerate().all(|(i, c)| {
        if i == 8 {
            c == b'T'
        } else {
            c.is_ascii_digit()
        }
    }) {
        return false;
    }
    let year: u32 = raw[..4].parse().unwrap_or(0);
    let month: usize = raw[4..6].parse().unwrap_or(0);
    let day: u32 = raw[6..8].parse().unwrap_or(0);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let lengths = [
        0,
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if year == 0
        || month == 0
        || month > 12
        || day == 0
        || day > lengths[month]
        || (raw.len() == 8 && value.ends_with('Z'))
    {
        return false;
    }
    raw.len() == 8
        || (raw[9..11].parse::<u32>().unwrap_or(99) < 24
            && raw[11..13].parse::<u32>().unwrap_or(99) < 60
            && raw[13..15].parse::<u32>().unwrap_or(99) < 60)
}
fn event_document(event: &Event) -> Result<String> {
    ensure!(
        !event.summary.trim().is_empty(),
        "Event summary is required"
    );
    ensure!(
        valid_date(&event.start),
        "Start: use YYYYMMDD or YYYYMMDDTHHMMSS (optional Z for UTC)"
    );
    ensure!(
        event.end.is_empty()
            || (valid_date(&event.end)
                && event.start.len() == event.end.len()
                && event.end > event.start),
        "End must follow start, using the same date/time format; all-day end is exclusive"
    );
    let mut logical = if event.source.is_empty() {
        lines(
            "BEGIN:VCALENDAR\nVERSION:2.0\nPRODID:-//MailTUI//EN\nBEGIN:VEVENT\nEND:VEVENT\nEND:VCALENDAR",
        )
    } else {
        lines(&event.source)
    };
    let original_rule = get(&logical, range(&logical, "VEVENT")?, "RRULE");
    if event.recurrence != original_rule && !event.recurrence.is_empty() {
        let mut keys = std::collections::HashSet::new();
        for part in event.recurrence.split(';') {
            let (key, value) = part
                .split_once('=')
                .context("Repeat rule must use KEY=VALUE;KEY=VALUE syntax")?;
            ensure!(
                !key.is_empty()
                    && key.bytes().all(|b| b.is_ascii_uppercase())
                    && !value.is_empty()
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b",+-".contains(&b))
                    && keys.insert(key),
                "Invalid or duplicate repeat rule property"
            );
            if key == "FREQ" {
                ensure!(
                    [
                        "DAILY", "WEEKLY", "MONTHLY", "YEARLY", "HOURLY", "MINUTELY", "SECONDLY"
                    ]
                    .contains(&value),
                    "Invalid repeat frequency"
                );
            }
            if key == "COUNT" || key == "INTERVAL" {
                ensure!(
                    value.parse::<u32>().is_ok_and(|n| n > 0),
                    "Repeat count/interval must be positive"
                );
            }
            if key == "UNTIL" {
                ensure!(valid_date(value), "Invalid repeat UNTIL date");
            }
        }
        ensure!(
            keys.contains("FREQ") && !(keys.contains("COUNT") && keys.contains("UNTIL")),
            "Repeat rule requires FREQ and cannot combine COUNT and UNTIL"
        );
    }
    set(&mut logical, "VEVENT", "RRULE", &event.recurrence, true)?;
    for (name, value, date) in [
        ("UID", event.uid.as_str(), false),
        ("SUMMARY", &event.summary, false),
        ("DTSTART", &event.start, true),
        ("DTEND", &event.end, true),
        ("LOCATION", &event.location, false),
        ("DESCRIPTION", &event.description, false),
    ] {
        set(&mut logical, "VEVENT", name, value, date)?;
    }
    set(&mut logical, "VEVENT", "DTSTAMP", &utc_now(), true)?;
    set(&mut logical, "VEVENT", "LAST-MODIFIED", &utc_now(), true)?;
    // DTEND and DURATION are mutually exclusive.
    if !event.end.is_empty() {
        set(&mut logical, "VEVENT", "DURATION", "", false)?;
    }
    // A newly added local DTEND inherits DTSTART's TZID when appropriate.
    if !event.timezone.is_empty() && event.end.len() == 15 {
        let bounds = range(&logical, "VEVENT")?;
        if let Some(line) = logical[bounds.0..bounds.1]
            .iter_mut()
            .find(|l| l.starts_with("DTEND:"))
        {
            *line = format!("DTEND;TZID=\"{}\":{}", event.timezone, event.end);
        }
    }
    let bounds = range(&logical, "VEVENT")?;
    let sequence = get(&logical, bounds, "SEQUENCE")
        .parse::<u64>()
        .unwrap_or(0)
        .saturating_add(1)
        .to_string();
    set(&mut logical, "VEVENT", "SEQUENCE", &sequence, false)?;
    Ok(serialize(&logical))
}
fn contact_document(contact: &Contact) -> Result<String> {
    ensure!(!contact.name.trim().is_empty(), "Contact name is required");
    let mut logical = if contact.source.is_empty() {
        lines(&format!(
            "BEGIN:VCARD\nVERSION:3.0\nN:;{};;;\nEND:VCARD",
            escape(&contact.name)
        ))
    } else {
        lines(&contact.source)
    };
    let bounds = range(&logical, "VCARD")?;
    let phone_uri = logical[bounds.0..bounds.1]
        .iter()
        .find_map(|line| {
            let (name, key, _) = property(line)?;
            name.eq_ignore_ascii_case("TEL").then(|| {
                key.to_ascii_uppercase().contains("VALUE=URI")
                    || (get(&logical, bounds, "VERSION") == "4.0"
                        && !key.to_ascii_uppercase().contains("VALUE=TEXT"))
            })
        })
        .unwrap_or(get(&logical, bounds, "VERSION") == "4.0");
    let phone = if phone_uri && !contact.phone.is_empty() && !contact.phone.contains(':') {
        format!("tel:{}", contact.phone)
    } else {
        contact.phone.clone()
    };
    for (name, value) in [
        ("UID", &contact.uid),
        ("FN", &contact.name),
        ("EMAIL", &contact.email),
        ("TEL", &phone),
        ("ORG", &contact.organization),
    ] {
        set(&mut logical, "VCARD", name, value, false)?;
    }
    set(&mut logical, "VCARD", "REV", &utc_now(), true)?;
    Ok(serialize(&logical))
}
pub fn validate_change(change: &Change) -> Result<()> {
    match change {
        Change::Event { entry, .. } => {
            event_document(entry)?;
        }
        Change::Contact { entry, .. } => {
            contact_document(entry)?;
        }
        _ => {}
    }
    Ok(())
}
pub fn write_event(collection: &PimCollection, event: &Event) -> Result<()> {
    persist(
        collection,
        &event.file,
        &event.source,
        &event.uid,
        "ics",
        &event_document(event)?,
    )
}
pub fn write_contact(book: &PimCollection, contact: &Contact) -> Result<()> {
    persist(
        book,
        &contact.file,
        &contact.source,
        &contact.uid,
        "vcf",
        &contact_document(contact)?,
    )
}
pub fn display_date(value: &str) -> String {
    if !valid_date(value) {
        return value.into();
    }
    let date = format!("{}-{}-{}", &value[..4], &value[4..6], &value[6..8]);
    if value.len() == 8 {
        format!("{date} all day")
    } else {
        format!(
            "{date} {}:{}{}",
            &value[9..11],
            &value[11..13],
            if value.ends_with('Z') { " UTC" } else { "" }
        )
    }
}
pub fn utc_now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = seconds.div_euclid(86400);
    let daytime = seconds.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        daytime / 3600,
        (daytime % 3600) / 60,
        daytime % 60
    )
}

//! Local vdir collections: RFC5545 events/tasks and vCard 3/4 address books.
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
/// Dates projected for the list, independent of the stored series master.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventOccurrence {
    pub start: String,
    pub end: String,
    pub starts_at: chrono::DateTime<chrono::Utc>,
    pub ends_at: chrono::DateTime<chrono::Utc>,
    pub upcoming: bool,
}
impl EventOccurrence {
    pub fn is_ongoing(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.starts_at <= now && now < self.ends_at
    }
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub collection: String,
    pub collection_label: String,
    pub uid: String,
    pub summary: String,
    pub start: String,
    pub due: String,
    pub status: String,
    pub priority: String,
    pub description: String,
    pub file: PathBuf,
    pub source: String,
}
#[derive(Default, Debug)]
pub struct Snapshot {
    pub events: Vec<Event>,
    pub contacts: Vec<Contact>,
    pub tasks: Vec<Task>,
    pub upcoming_events: std::collections::HashSet<PathBuf>,
    pub event_occurrences: std::collections::HashMap<PathBuf, EventOccurrence>,
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
    Task {
        collection: PimCollection,
        entry: Task,
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
    execute_with_tasks(calendars, books, &[], change)
}
pub fn execute_with_tasks(
    calendars: &[PimCollection],
    books: &[PimCollection],
    tasks: &[PimCollection],
    change: Change,
) -> Result<Snapshot> {
    match change {
        Change::Load => {}
        Change::Event { collection, entry } => write_event(&collection, &entry)?,
        Change::Contact { book, entry } => write_contact(&book, &entry)?,
        Change::Task { collection, entry } => write_task(&collection, &entry)?,
        Change::Delete { file, source } => {
            ensure!(
                calendars
                    .iter()
                    .chain(books)
                    .chain(tasks)
                    .any(|c| file.parent() == Some(c.path.as_path())),
                "Entry outside configured collections"
            );
            check_original(&file, &source)?;
            fs::remove_file(&file).with_context(|| format!("Delete {}", file.display()))?;
        }
    }
    Ok(load_with_tasks(calendars, books, tasks))
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
                if (kind == "VEVENT" || kind == "VTODO")
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
    load_with_tasks(calendars, books, &[])
}
pub fn load_with_tasks(
    calendars: &[PimCollection],
    books: &[PimCollection],
    tasks: &[PimCollection],
) -> Snapshot {
    let mut snapshot = Snapshot::default();
    let now = chrono::Utc::now();
    for (collections, extension, kind) in [
        (calendars, "ics", "VEVENT"),
        (books, "vcf", "VCARD"),
        (tasks, "ics", "VTODO"),
    ] {
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
                    // CalDAV collections may contain both events and tasks.
                    if extension == "ics"
                        && !logical
                            .iter()
                            .any(|l| l.eq_ignore_ascii_case(&format!("BEGIN:{kind}")))
                        && logical.iter().any(|l| {
                            l.eq_ignore_ascii_case(if kind == "VEVENT" {
                                "BEGIN:VTODO"
                            } else {
                                "BEGIN:VEVENT"
                            })
                        })
                    {
                        return Ok(());
                    }
                    let bounds = range(&logical, kind)
                        .with_context(|| format!("Parse {}", file.display()))?;
                    let uid = get(&logical, bounds, "UID");
                    if kind == "VTODO" {
                        ensure!(!uid.is_empty(), "{}: UID missing", file.display());
                        let status = get(&logical, bounds, "STATUS");
                        snapshot.tasks.push(Task {
                            collection: collection.id.clone(),
                            collection_label: collection.label.clone(),
                            uid,
                            summary: get(&logical, bounds, "SUMMARY"),
                            start: get(&logical, bounds, "DTSTART"),
                            due: get(&logical, bounds, "DUE"),
                            status: if status.is_empty() {
                                "NEEDS-ACTION".into()
                            } else {
                                status
                            },
                            priority: get(&logical, bounds, "PRIORITY"),
                            description: get(&logical, bounds, "DESCRIPTION"),
                            file,
                            source,
                        });
                    } else if kind == "VEVENT" {
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
    snapshot.tasks.sort_by(|a, b| {
        (
            task_is_closed(&a.status),
            a.due.is_empty(),
            &a.due,
            &a.summary,
            &a.file,
        )
            .cmp(&(
                task_is_closed(&b.status),
                b.due.is_empty(),
                &b.due,
                &b.summary,
                &b.file,
            ))
    });
    refresh_occurrences(&mut snapshot, now);
    snapshot
}

/// Refresh only time-dependent projections from already loaded documents.
/// This does not read directories or files.
pub fn refresh_occurrences(snapshot: &mut Snapshot, now: chrono::DateTime<chrono::Utc>) {
    snapshot.upcoming_events.clear();
    snapshot.event_occurrences.clear();
    for event in &snapshot.events {
        if let Some(occurrence) = event_occurrence(event, now) {
            if occurrence.upcoming {
                snapshot.upcoming_events.insert(event.file.clone());
            }
            snapshot
                .event_occurrences
                .insert(event.file.clone(), occurrence);
        } else {
            // Keep entries with unsupported dates/rules accessible for inspection.
            snapshot.upcoming_events.insert(event.file.clone());
        }
    }
}

fn event_time(value: &str, timezone: &str) -> Option<chrono::DateTime<rrule::Tz>> {
    use chrono::TimeZone;
    let local = if value.len() == 8 {
        chrono::NaiveDate::parse_from_str(value, "%Y%m%d")
            .ok()?
            .and_hms_opt(0, 0, 0)?
    } else {
        chrono::NaiveDateTime::parse_from_str(value.trim_end_matches('Z'), "%Y%m%dT%H%M%S").ok()?
    };
    let tz = if value.ends_with('Z') {
        rrule::Tz::UTC
    } else if timezone.is_empty() {
        rrule::Tz::LOCAL
    } else {
        timezone.parse::<chrono_tz::Tz>().ok()?.into()
    };
    tz.from_local_datetime(&local).earliest()
}

/// Whether this event (or a recurrence) has not ended yet.
pub fn event_is_upcoming(event: &Event, now: chrono::DateTime<chrono::Utc>) -> bool {
    event_occurrence(event, now).is_none_or(|occurrence| occurrence.upcoming)
}

/// Project a series onto its first ongoing/future occurrence, or its last past
/// occurrence when the series has ended. Executed by the worker, never on draw.
pub fn event_occurrence(
    event: &Event,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<EventOccurrence> {
    let start = event_time(&event.start, &event.timezone)?;
    let logical = lines(&event.source);
    let bounds = range(&logical, "VEVENT").ok();
    let duration = bounds
        .map(|b| get(&logical, b, "DURATION"))
        .unwrap_or_default();
    let end_timezone = bounds
        .and_then(|b| property_timezone(&logical, b, "DTEND"))
        .unwrap_or(&event.timezone);
    let end = if event.end.is_empty() {
        if !duration.is_empty() {
            ical_duration(&duration).and_then(|duration| start.checked_add_signed(duration))
        } else if event.start.len() == 8 {
            start.checked_add_days(chrono::Days::new(1))
        } else {
            Some(start)
        }
    } else {
        event_time(&event.end, end_timezone)
    }?;
    let has_additional_dates = bounds.is_some_and(|b| {
        logical[b.0 + 1..b.1]
            .iter()
            .any(|l| property(l).is_some_and(|(n, _, _)| n.eq_ignore_ascii_case("RDATE")))
    });
    if event.recurrence.is_empty() && !has_additional_dates {
        return Some(project_occurrence(
            start,
            end,
            &event.start,
            &event.end,
            now,
        ));
    }
    let duration = (end - start).max(chrono::Duration::zero());
    let mut after = now.checked_sub_signed(duration)?;
    if duration > chrono::Duration::zero() {
        after = after.checked_add_signed(chrono::Duration::nanoseconds(1))?;
    }
    let mut set = rrule::RRuleSet::new(start);
    if event.recurrence.is_empty() {
        set = set.rdate(start);
    } else {
        let rule = event
            .recurrence
            .parse::<rrule::RRule<rrule::Unvalidated>>()
            .ok()?
            .validate(start)
            .ok()?;
        set = set.rrule(rule);
    }
    // Exclusions and additional dates apply to the master, not its alarms.
    if let Some(bounds) = bounds {
        let mut depth = 0;
        for line in &logical[bounds.0 + 1..bounds.1] {
            if line.starts_with("BEGIN:") {
                depth += 1;
            }
            if depth == 0
                && let Some((name, key, values)) = property(line)
                && ["EXDATE", "RDATE"]
                    .iter()
                    .any(|p| name.eq_ignore_ascii_case(p))
            {
                let timezone = key
                    .split(';')
                    .find_map(|p| p.strip_prefix("TZID="))
                    .unwrap_or(&event.timezone)
                    .trim_matches('"');
                for value in values.split(',') {
                    if let Some(date) = event_time(value, timezone) {
                        set = if name.eq_ignore_ascii_case("EXDATE") {
                            set.exdate(date)
                        } else {
                            set.rdate(date)
                        };
                    }
                }
            }
            if line.starts_with("END:") {
                depth -= 1;
            }
        }
    }
    // A detached exception replaces its RECURRENCE-ID, including cancellations.
    let mut exceptions = Vec::new();
    for bounds in event_component_ranges(&logical) {
        let recurrence_id = get(&logical, bounds, "RECURRENCE-ID");
        if recurrence_id.is_empty() || get(&logical, bounds, "UID") != event.uid {
            continue;
        }
        let timezone =
            property_timezone(&logical, bounds, "RECURRENCE-ID").unwrap_or(&event.timezone);
        if let Some(date) = event_time(&recurrence_id, timezone) {
            set = set.exdate(date);
        }
        if get(&logical, bounds, "STATUS").eq_ignore_ascii_case("CANCELLED") {
            continue;
        }
        let value = get(&logical, bounds, "DTSTART");
        let timezone = property_timezone(&logical, bounds, "DTSTART").unwrap_or(&event.timezone);
        let Some(exception_start) = event_time(&value, timezone) else {
            continue;
        };
        let end_value = get(&logical, bounds, "DTEND");
        let end_timezone = property_timezone(&logical, bounds, "DTEND").unwrap_or(timezone);
        let exception_end = if end_value.is_empty() {
            let exception_duration = get(&logical, bounds, "DURATION");
            let duration = if exception_duration.is_empty() {
                Some(duration)
            } else {
                ical_duration(&exception_duration)
            };
            duration.and_then(|duration| exception_start.checked_add_signed(duration))
        } else {
            event_time(&end_value, end_timezone)
        };
        if let Some(exception_end) = exception_end {
            exceptions.push(project_occurrence(
                exception_start,
                exception_end,
                &value,
                &end_value,
                now,
            ));
        }
    }
    let result = set
        .clone()
        .after(after.with_timezone(&start.timezone()))
        .all(1);
    if result.dates.is_empty() && result.limited {
        return None;
    }
    if let Some(next) = result.dates.first() {
        let next_end = occurrence_end(*next, start, end, event.start.len() == 8)?;
        exceptions.push(project_occurrence(
            *next,
            next_end,
            &event.start,
            &event.end,
            now,
        ));
    }
    if let Some(next) = exceptions
        .iter()
        .filter(|e| e.upcoming)
        .min_by_key(|e| e.starts_at)
    {
        return Some(next.clone());
    }
    // History mode still shows one row per series, using its most recent dates.
    let past = set
        .before(now.with_timezone(&start.timezone()))
        .all(u16::MAX);
    if past.limited {
        return None;
    }
    if let Some(last) = past.dates.last() {
        let last_end = occurrence_end(*last, start, end, event.start.len() == 8)?;
        exceptions.push(project_occurrence(
            *last,
            last_end,
            &event.start,
            &event.end,
            now,
        ));
    }
    // A series with every occurrence excluded has no row to display by default.
    exceptions
        .into_iter()
        .max_by_key(|e| e.starts_at)
        .or_else(|| {
            let mut excluded = project_occurrence(start, end, &event.start, &event.end, now);
            excluded.upcoming = false;
            Some(excluded)
        })
}

fn project_occurrence(
    start: chrono::DateTime<rrule::Tz>,
    end: chrono::DateTime<rrule::Tz>,
    start_template: &str,
    end_template: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> EventOccurrence {
    fn format(date: chrono::DateTime<rrule::Tz>, template: &str) -> String {
        if template.len() == 8 {
            date.format("%Y%m%d").to_string()
        } else if template.ends_with('Z') {
            date.with_timezone(&chrono::Utc)
                .format("%Y%m%dT%H%M%SZ")
                .to_string()
        } else {
            date.format("%Y%m%dT%H%M%S").to_string()
        }
    }
    EventOccurrence {
        start: format(start, start_template),
        end: format(
            end,
            if end_template.is_empty() {
                start_template
            } else {
                end_template
            },
        ),
        starts_at: start.with_timezone(&chrono::Utc),
        ends_at: end.with_timezone(&chrono::Utc),
        upcoming: start >= now || end > now,
    }
}
fn occurrence_end(
    next: chrono::DateTime<rrule::Tz>,
    start: chrono::DateTime<rrule::Tz>,
    end: chrono::DateTime<rrule::Tz>,
    all_day: bool,
) -> Option<chrono::DateTime<rrule::Tz>> {
    if all_day {
        let days = (end.date_naive() - start.date_naive()).num_days().max(0) as u64;
        next.checked_add_days(chrono::Days::new(days))
    } else {
        next.checked_add_signed(end - start)
            .map(|date| date.with_timezone(&end.timezone()))
    }
}
fn property_timezone<'a>(
    logical: &'a [String],
    bounds: (usize, usize),
    name: &str,
) -> Option<&'a str> {
    let mut depth = 0;
    for line in &logical[bounds.0 + 1..bounds.1] {
        if line.to_ascii_uppercase().starts_with("BEGIN:") {
            depth += 1;
        }
        if depth == 0
            && let Some((n, key, _)) = property(line)
            && n.eq_ignore_ascii_case(name)
        {
            return key
                .split(';')
                .find_map(|p| p.strip_prefix("TZID="))
                .map(|s| s.trim_matches('"'));
        }
        if line.to_ascii_uppercase().starts_with("END:") {
            depth -= 1;
        }
    }
    None
}
fn event_component_ranges(logical: &[String]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = None;
    let mut depth = 0;
    for (index, line) in logical.iter().enumerate() {
        if start.is_none() {
            if line.eq_ignore_ascii_case("BEGIN:VEVENT") {
                start = Some(index);
                depth = 1;
            }
        } else if line.to_ascii_uppercase().starts_with("BEGIN:") {
            depth += 1;
        } else if line.to_ascii_uppercase().starts_with("END:") {
            depth -= 1;
            if depth == 0 {
                ranges.push((start.take().unwrap(), index));
            }
        }
    }
    ranges
}

fn ical_duration(value: &str) -> Option<chrono::Duration> {
    let mut seconds = 0i64;
    let mut number = None::<i64>;
    let mut fields = 0;
    for c in value.strip_prefix('P')?.chars() {
        if let Some(digit) = c.to_digit(10) {
            number = Some(
                number
                    .unwrap_or(0)
                    .checked_mul(10)?
                    .checked_add(digit as i64)?,
            );
        } else if c == 'T' && number.is_none() {
            continue;
        } else {
            let scale = match c {
                'W' => 604800,
                'D' => 86400,
                'H' => 3600,
                'M' => 60,
                'S' => 1,
                _ => return None,
            };
            seconds = seconds.checked_add(number.take()?.checked_mul(scale)?)?;
            fields += 1;
        }
    }
    if number.is_some() || fields == 0 {
        return None;
    }
    chrono::Duration::try_seconds(seconds)
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
        Change::Task { entry, .. } => {
            task_document(entry)?;
        }
        _ => {}
    }
    Ok(())
}
fn task_document(task: &Task) -> Result<String> {
    ensure!(!task.summary.trim().is_empty(), "Task summary is required");
    for date in [&task.start, &task.due] {
        ensure!(
            date.is_empty() || valid_date(date),
            "Task dates: use YYYYMMDD or YYYYMMDDTHHMMSS[Z]"
        );
    }
    ensure!(
        task.start.is_empty()
            || task.due.is_empty()
            || (task.start.len() == task.due.len() && task.due > task.start),
        "Due must follow start, using the same date/time format"
    );
    ensure!(
        ["NEEDS-ACTION", "IN-PROCESS", "COMPLETED", "CANCELLED"].contains(&task.status.as_str()),
        "Status: NEEDS-ACTION, IN-PROCESS, COMPLETED or CANCELLED"
    );
    ensure!(
        task.priority.is_empty() || task.priority.parse::<u8>().is_ok_and(|v| v <= 9),
        "Priority: 0 (unspecified) to 9 (lowest), 1 is highest"
    );
    let mut logical = if task.source.is_empty() {
        lines(
            "BEGIN:VCALENDAR\nVERSION:2.0\nPRODID:-//MailTUI//EN\nBEGIN:VTODO\nEND:VTODO\nEND:VCALENDAR",
        )
    } else {
        lines(&task.source)
    };
    let prior_status = get(&logical, range(&logical, "VTODO")?, "STATUS");
    for (name, value, date) in [
        ("UID", &task.uid, false),
        ("SUMMARY", &task.summary, false),
        ("DTSTART", &task.start, true),
        ("DUE", &task.due, true),
        ("STATUS", &task.status, false),
        ("PRIORITY", &task.priority, false),
        ("DESCRIPTION", &task.description, false),
    ] {
        set(&mut logical, "VTODO", name, value, date)?;
    }
    if task.status != prior_status {
        let now = utc_now();
        set(
            &mut logical,
            "VTODO",
            "COMPLETED",
            if task.status == "COMPLETED" { &now } else { "" },
            true,
        )?;
        set(
            &mut logical,
            "VTODO",
            "PERCENT-COMPLETE",
            if task.status == "COMPLETED" {
                "100"
            } else if task.status == "NEEDS-ACTION" {
                "0"
            } else {
                ""
            },
            false,
        )?;
    }
    if !task.due.is_empty() {
        set(&mut logical, "VTODO", "DURATION", "", false)?;
    }
    set(&mut logical, "VTODO", "DTSTAMP", &utc_now(), true)?;
    set(&mut logical, "VTODO", "LAST-MODIFIED", &utc_now(), true)?;
    let sequence = get(&logical, range(&logical, "VTODO")?, "SEQUENCE")
        .parse::<u64>()
        .unwrap_or(0)
        .saturating_add(1)
        .to_string();
    set(&mut logical, "VTODO", "SEQUENCE", &sequence, false)?;
    Ok(serialize(&logical))
}
pub fn write_task(collection: &PimCollection, task: &Task) -> Result<()> {
    persist(
        collection,
        &task.file,
        &task.source,
        &task.uid,
        "ics",
        &task_document(task)?,
    )
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
pub fn task_status_label(status: &str) -> &str {
    if status.eq_ignore_ascii_case("NEEDS-ACTION") {
        "To do"
    } else {
        status
    }
}

pub fn task_is_closed(status: &str) -> bool {
    status.eq_ignore_ascii_case("COMPLETED") || status.eq_ignore_ascii_case("CANCELLED")
}

pub fn display_task_due(value: &str) -> String {
    if value.len() == 8 && valid_date(value) {
        format!("{}-{}-{}", &value[..4], &value[4..6], &value[6..8])
    } else {
        display_date(value)
    }
}

pub fn display_date(value: &str) -> String {
    if !valid_date(value) {
        return value.into();
    }
    let date = format!("{}-{}-{}", &value[..4], &value[4..6], &value[6..8]);
    if value.len() == 8 {
        date
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

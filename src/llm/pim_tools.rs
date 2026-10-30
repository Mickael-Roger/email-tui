use super::Proposal;
use crate::{
    config::PimCollection,
    pim::{self, Change, Contact, Event},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};

#[derive(Clone, Default)]
pub(super) struct PimTools {
    pub calendars: Vec<PimCollection>,
    pub books: Vec<PimCollection>,
}

fn reference(collection: &str, file: &std::path::Path) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    collection.hash(&mut hash);
    file.hash(&mut hash);
    format!("pim-{:016x}", hash.finish())
}
fn event_json(e: &Event, full: bool) -> Value {
    let mut value = json!({"event":reference(&e.collection,&e.file),"calendar":e.collection,"summary":e.summary.chars().take(2000).collect::<String>(),"start":e.start,"end":e.end,"timezone":e.timezone,"recurring":!e.recurrence.is_empty(),"series_master_only":true});
    if full {
        value["location"] = json!(e.location.chars().take(4000).collect::<String>());
        value["description"] = json!(e.description.chars().take(16000).collect::<String>());
        value["description_truncated"] = json!(e.description.chars().count() > 16000);
        value["recurrence"] = json!(e.recurrence);
    }
    value
}
fn contact_json(c: &Contact) -> Value {
    json!({"contact":reference(&c.book,&c.file),"book":c.book,"name":c.name.chars().take(2000).collect::<String>(),"email":c.email.chars().take(2000).collect::<String>(),"phone":c.phone.chars().take(2000).collect::<String>(),"organization":c.organization.chars().take(2000).collect::<String>()})
}
fn text(args: &Value, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            ensure!(value.len() <= 65536, "{key} exceeds 64 KiB");
            Ok(Some(value.clone()))
        }
        _ => bail!("{key} must be a string"),
    }
}
fn required(args: &Value, key: &str) -> Result<String> {
    text(args, key)?
        .filter(|s| !s.trim().is_empty())
        .with_context(|| format!("{key} is required"))
}
fn page(args: &Value) -> Result<(usize, usize)> {
    let n = |key: &str, default| -> Result<usize> {
        args.get(key)
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .with_context(|| format!("{key} must be a nonnegative integer"))
            })
            .unwrap_or(Ok(default))
    };
    Ok((n("offset", 0)?.min(100000), n("limit", 10)?.clamp(1, 20)))
}
fn spec(name: &str, description: &str, properties: Value, required: Vec<&str>) -> Value {
    json!({"name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}})
}
impl PimTools {
    pub fn specs(&self) -> Vec<Value> {
        let text = json!({"type":"string"});
        let offset = json!({"type":"integer","minimum":0});
        let limit = json!({"type":"integer","minimum":1,"maximum":20});
        let mut specs = vec![];
        if !self.calendars.is_empty() {
            specs.push(spec("list_calendars","List enabled local calendars with exact IDs. Calendar access is separate from the mail account scope.",json!({}),vec![]));
            specs.push(spec("search_events","Search local ICS event/series masters by text and optional calendar ID. Date bounds use YYYYMMDD and filter the master's DTSTART, not expanded recurring occurrences. Empty query lists entries. Read a returned event reference before changing it.",json!({"query":text,"calendar":text,"after":text,"before":text,"offset":offset,"limit":limit}),vec![]));
            specs.push(spec("read_event","Read a local event/series master by its opaque reference from search_events; includes description, timezone and recurrence rule.",json!({"event":text}),vec!["event"]));
            let mut fields = json!({"summary":text,"start":text,"end":text,"location":text,"description":text,"recurrence":text});
            fields["calendar"] = text.clone();
            specs.push(spec("propose_create_event","Propose creating an ICS event in an exact calendar ID. start/end: YYYYMMDD or YYYYMMDDTHHMMSS[Z]; all-day end is exclusive. recurrence is an optional RRULE such as FREQ=WEEKLY;BYDAY=MO. Does not write; user applies the proposal.",fields.clone(),vec!["calendar","summary","start"]));
            fields.as_object_mut().unwrap().remove("calendar");
            fields["event"] = text.clone();
            specs.push(spec("propose_update_event","Propose modifying an existing event/entire series by opaque event reference. Omit fields to preserve them; an empty optional field clears it. Calendar is unchanged. Existing timezone/alarms/exceptions are preserved. Does not write.",fields,vec!["event"]));
            specs.push(spec("propose_delete_event","Propose deleting the whole event/recurring series (including exceptions) by opaque reference. Does not delete until user applies.",json!({"event":text}),vec!["event"]));
        }
        if !self.books.is_empty() {
            specs.push(spec("list_address_books","List enabled local vCard address books with exact IDs. These are independent of the mail account scope.",json!({}),vec![]));
            specs.push(spec("search_contacts","Search local contacts by name, primary email, phone, organization or book. Optional book is an exact address book ID. Empty query lists contacts. Returns opaque contact references; use read_contact before editing/deleting.",json!({"query":text,"book":text,"offset":offset,"limit":limit}),vec![]));
            specs.push(spec("read_contact","Read a local contact's editable primary fields by opaque reference from search_contacts. Other imported fields remain preserved.",json!({"contact":text}),vec!["contact"]));
            let mut fields =
                json!({"name":text,"email":text,"phone":text,"organization":text,"book":text});
            specs.push(spec("propose_create_contact","Propose creating a vCard contact in an exact address book ID. Does not write until the user applies.",fields.clone(),vec!["book","name"]));
            fields.as_object_mut().unwrap().remove("book");
            fields["contact"] = text.clone();
            specs.push(spec("propose_update_contact","Propose editing a contact's primary fields by opaque reference. Omitted fields, secondary emails/phones, photos and unknown fields are preserved. Empty optional strings clear primary fields. Does not write.",fields,vec!["contact"]));
            specs.push(spec("propose_delete_contact","Propose deleting a local contact by opaque reference. Does not delete until the user applies.",json!({"contact":text}),vec!["contact"]));
        }
        specs
    }
    pub fn execute(&self, name: &str, args: &Value) -> Result<(Value, Option<Proposal>)> {
        let calendar = matches!(
            name,
            "list_calendars"
                | "search_events"
                | "read_event"
                | "propose_create_event"
                | "propose_update_event"
                | "propose_delete_event"
        );
        let contact = matches!(
            name,
            "list_address_books"
                | "search_contacts"
                | "read_contact"
                | "propose_create_contact"
                | "propose_update_contact"
                | "propose_delete_contact"
        );
        ensure!(calendar || contact, "Unknown local tool: {name}");
        ensure!(
            !(calendar && self.calendars.is_empty()),
            "Calendars disabled or not configured"
        );
        ensure!(
            !(contact && self.books.is_empty()),
            "Contacts disabled or not configured"
        );
        if name == "list_calendars" || name == "list_address_books" {
            let items = if calendar {
                &self.calendars
            } else {
                &self.books
            };
            return Ok((
                json!({"collections":items.iter().map(|c|json!({"id":c.id,"label":c.label})).collect::<Vec<_>>()}),
                None,
            ));
        }
        let snapshot = pim::load(
            if calendar { &self.calendars } else { &[] },
            if contact { &self.books } else { &[] },
        );
        let warnings: Vec<_> = snapshot
            .warnings
            .iter()
            .take(20)
            .map(|w| w.chars().take(1000).collect::<String>())
            .collect();
        if name == "search_events" || name == "search_contacts" {
            let (offset, limit) = page(args)?;
            let query = text(args, "query")?.unwrap_or_default().to_lowercase();
            let selected = text(args, if calendar { "calendar" } else { "book" })?
                .filter(|s| !s.trim().is_empty());
            if let Some(id) = &selected {
                ensure!(
                    if calendar {
                        &self.calendars
                    } else {
                        &self.books
                    }
                    .iter()
                    .any(|c| &c.id == id),
                    "Unknown collection ID; list collections first"
                );
            }
            let after = text(args, "after")?.filter(|s| !s.trim().is_empty());
            let before = text(args, "before")?.filter(|s| !s.trim().is_empty());
            for date in [&after, &before].into_iter().flatten() {
                ensure!(
                    date.len() == 8 && pim::valid_date(date),
                    "Event date bounds must be YYYYMMDD"
                );
            }
            let items: Vec<Value> = if calendar {
                snapshot
                    .events
                    .iter()
                    .filter(|e| selected.as_ref().is_none_or(|id| id == &e.collection))
                    .filter(|e| {
                        format!(
                            "{} {} {} {} {}",
                            e.summary, e.location, e.description, e.collection, e.collection_label
                        )
                        .to_lowercase()
                        .contains(&query)
                    })
                    .filter(|e| {
                        after
                            .as_ref()
                            .is_none_or(|date| e.start.get(..8).is_some_and(|d| d >= date.as_str()))
                            && before.as_ref().is_none_or(|date| {
                                e.start.get(..8).is_some_and(|d| d < date.as_str())
                            })
                    })
                    .map(|e| event_json(e, false))
                    .collect()
            } else {
                snapshot
                    .contacts
                    .iter()
                    .filter(|c| selected.as_ref().is_none_or(|id| id == &c.book))
                    .filter(|c| {
                        format!(
                            "{} {} {} {} {} {}",
                            c.name, c.email, c.phone, c.organization, c.book, c.book_label
                        )
                        .to_lowercase()
                        .contains(&query)
                    })
                    .map(contact_json)
                    .collect()
            };
            let count = items.len();
            let next = offset.saturating_add(limit);
            return Ok((
                json!({"items":items.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"next_offset":if next<count{Some(next)}else{None},"local_only":true,"series_master_only":calendar,"warnings":warnings}),
                None,
            ));
        }
        let delete_label;
        let change = if calendar {
            let mut entry = if name == "propose_create_event" {
                let id = required(args, "calendar")?;
                let c = self
                    .calendars
                    .iter()
                    .find(|c| c.id == id)
                    .context("Unknown calendar; use list_calendars")?;
                Event {
                    collection: c.id.clone(),
                    collection_label: c.label.clone(),
                    uid: format!("mailtui-{:016x}", rand::random::<u64>()),
                    summary: required(args, "summary")?,
                    start: required(args, "start")?,
                    end: String::new(),
                    location: String::new(),
                    description: String::new(),
                    recurrence: String::new(),
                    timezone: String::new(),
                    file: Default::default(),
                    source: String::new(),
                }
            } else {
                let id = required(args, "event")?;
                snapshot
                    .events
                    .into_iter()
                    .find(|e| reference(&e.collection, &e.file) == id)
                    .context("Unknown event reference; use search_events again")?
            };
            delete_label = format!(
                "{} · {} · {}",
                entry.summary, entry.start, entry.collection_label
            );
            if name == "read_event" {
                return Ok((event_json(&entry, true), None));
            }
            if name == "propose_delete_event" {
                Change::Delete {
                    file: entry.file,
                    source: entry.source,
                }
            } else {
                for (key, field) in [
                    ("summary", &mut entry.summary),
                    ("start", &mut entry.start),
                    ("end", &mut entry.end),
                    ("location", &mut entry.location),
                    ("description", &mut entry.description),
                    ("recurrence", &mut entry.recurrence),
                ] {
                    if let Some(value) = text(args, key)? {
                        *field = value;
                    }
                }
                let collection = self
                    .calendars
                    .iter()
                    .find(|c| c.id == entry.collection)
                    .context("Calendar unavailable")?
                    .clone();
                Change::Event { collection, entry }
            }
        } else {
            let mut entry = if name == "propose_create_contact" {
                let id = required(args, "book")?;
                let b = self
                    .books
                    .iter()
                    .find(|b| b.id == id)
                    .context("Unknown address book; use list_address_books")?;
                Contact {
                    book: b.id.clone(),
                    book_label: b.label.clone(),
                    uid: format!("mailtui-{:016x}", rand::random::<u64>()),
                    name: required(args, "name")?,
                    email: String::new(),
                    phone: String::new(),
                    organization: String::new(),
                    addresses: Default::default(),
                    file: Default::default(),
                    source: String::new(),
                }
            } else {
                let id = required(args, "contact")?;
                snapshot
                    .contacts
                    .into_iter()
                    .find(|c| reference(&c.book, &c.file) == id)
                    .context("Unknown contact reference; use search_contacts again")?
            };
            delete_label = format!("{} · {} · {}", entry.name, entry.email, entry.book_label);
            if name == "read_contact" {
                return Ok((contact_json(&entry), None));
            }
            if name == "propose_delete_contact" {
                Change::Delete {
                    file: entry.file,
                    source: entry.source,
                }
            } else {
                for (key, field) in [
                    ("name", &mut entry.name),
                    ("email", &mut entry.email),
                    ("phone", &mut entry.phone),
                    ("organization", &mut entry.organization),
                ] {
                    if let Some(value) = text(args, key)? {
                        *field = value;
                    }
                }
                if entry.uid.is_empty() {
                    entry.uid = format!("mailtui-{:016x}", rand::random::<u64>());
                }
                let book = self
                    .books
                    .iter()
                    .find(|b| b.id == entry.book)
                    .context("Address book unavailable")?
                    .clone();
                Change::Contact {
                    book,
                    entry: Box::new(entry),
                }
            }
        };
        pim::validate_change(&change)?;
        let summary = match &change {
            Change::Event { collection, entry } => format!(
                "{} event/series · {} · {} → {} · {}\nLocation: {}\nTimezone: {}\nRepeat: {}\nDescription: {}",
                if entry.source.is_empty() {
                    "Create"
                } else {
                    "Update"
                },
                entry.summary,
                entry.start,
                entry.end,
                collection.label,
                entry.location,
                entry.timezone,
                entry.recurrence,
                entry.description
            ),
            Change::Contact { book, entry } => format!(
                "{} contact · {} · {} · {} · {}\nOrganization: {}",
                if entry.source.is_empty() {
                    "Create"
                } else {
                    "Update"
                },
                entry.name,
                entry.email,
                entry.phone,
                book.label,
                entry.organization
            ),
            Change::Delete { .. } => format!(
                "Delete {} · {}",
                if calendar {
                    "entire event/series"
                } else {
                    "contact"
                },
                delete_label
            ),
            Change::Load | Change::Task { .. } => unreachable!(),
        };
        Ok((
            json!({"status":"proposed","applied":false,"action":name,"warnings":warnings}),
            Some(Proposal::Pim {
                change: Box::new(change),
                summary,
            }),
        ))
    }
}

#[cfg(test)]
#[path = "pim_tools_tests.rs"]
mod tests;

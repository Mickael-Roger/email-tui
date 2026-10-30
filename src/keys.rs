use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NextBranch,
    PreviousBranch,
    CollapseAll,
    Command,
    Filter,
    Search,
    Help,
    Back,
    Next,
    Previous,
    FirstPrefix,
    Last,
    HalfUp,
    PageDown,
    PageUp,
    Open,
    Read,
    Select,
    SelectRange,
    SelectAll,
    ClearSelection,
    Sort,
    Seen,
    Unseen,
    ToggleSeen,
    Flag,
    Delete,
    Move,
    Copy,
    Html,
    HtmlRemote,
    SaveAttachments,
    Assistant,
    Reload,
    Sync,
    TogglePimHistory,
    Quit,
    Compose,
    CreateFolder,
    Reply,
    ReplyAll,
    Forward,
    More,
    Collapse,
    Expand,
    ViewSlot(usize),
    EditItem,
    SortSubject,
    SortDate,
    SortAccount,
    SortStatus,
    ToggleHeader,
    ToggleWide,
    ToggleBreadcrumbs,
    Aliases,
}

pub struct Binding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    pub key: &'static str,
    pub action: Action,
    pub description: &'static str,
}

pub fn bindings() -> Vec<Binding> {
    use KeyCode::*;
    use KeyModifiers as M;
    let definitions = [
        (
            Char('a'),
            M::CONTROL,
            "Ctrl+a",
            Action::Assistant,
            "Open or resume the optional mail assistant · :ai",
        ),
        (
            Char('s'),
            M::CONTROL,
            "Ctrl+s",
            Action::SaveAttachments,
            "Save attachments / save calendar or contact form",
        ),
        (
            Char(':'),
            M::NONE,
            ":",
            Action::Command,
            "Command mode · ↑/↓ history",
        ),
        (
            Char('/'),
            M::NONE,
            "/",
            Action::Search,
            "Mail search / calendar or contact filter",
        ),
        (
            Char('?'),
            M::NONE,
            "?",
            Action::Help,
            "Open this keyboard reference",
        ),
        (
            Esc,
            M::NONE,
            "Esc",
            Action::Back,
            "Cancel / clear filter / previous view",
        ),
        (Char('q'), M::NONE, "q", Action::Back, "Back; quit at root"),
        (
            Down,
            M::NONE,
            "↓",
            Action::Next,
            "Next tree row / scroll down",
        ),
        (
            Up,
            M::NONE,
            "↑",
            Action::Previous,
            "Previous tree row / scroll up",
        ),
        (PageDown, M::NONE, "PgDn", Action::PageDown, "Page down"),
        (PageUp, M::NONE, "PgUp", Action::PageUp, "Page up"),
        (
            Char('d'),
            M::CONTROL,
            "Ctrl+d",
            Action::Delete,
            "Delete mail/calendar/contact item · confirmation",
        ),
        (
            Char('c'),
            M::CONTROL,
            "Ctrl+c",
            Action::Quit,
            "Press twice within 3 seconds to quit",
        ),
        (
            Enter,
            M::NONE,
            "Enter",
            Action::Open,
            "Fold/read mail; view calendar event or contact",
        ),
        (
            Char(' '),
            M::NONE,
            "Space",
            Action::Select,
            "Toggle selected message (never an account or folder)",
        ),
        (
            Char('a'),
            M::NONE,
            "a",
            Action::SelectAll,
            "Select all visible loaded messages",
        ),
        (
            Char('N'),
            M::NONE,
            "Shift+n",
            Action::SortSubject,
            "Sort messages by subject within each folder",
        ),
        (
            Char('A'),
            M::NONE,
            "Shift+a",
            Action::SortDate,
            "Sort messages by date within each folder",
        ),
        (
            Char('P'),
            M::NONE,
            "Shift+p",
            Action::SortAccount,
            "Sort account branches (all-account scope)",
        ),
        (
            Char('S'),
            M::NONE,
            "Shift+s",
            Action::SortStatus,
            "Sort messages by status within each folder",
        ),
        (
            Char('s'),
            M::NONE,
            "s",
            Action::Sort,
            "Cycle message sort: date/from/subject/size",
        ),
        (Char('i'), M::NONE, "i", Action::Seen, "Mark messages read"),
        (
            Char('t'),
            M::NONE,
            "t",
            Action::ToggleSeen,
            "Toggle read/unread · mixed selection becomes read",
        ),
        (
            Char('u'),
            M::NONE,
            "u",
            Action::Unseen,
            "Use account / mark messages unread",
        ),
        (
            Char('m'),
            M::NONE,
            "m",
            Action::Move,
            "Move messages · fuzzy destination picker",
        ),
        (
            Char('y'),
            M::NONE,
            "y",
            Action::Copy,
            "Copy messages to folder",
        ),
        (
            Char('h'),
            M::NONE,
            "h",
            Action::Html,
            "Open HTML · remote images blocked",
        ),
        (
            Char('H'),
            M::NONE,
            "H",
            Action::HtmlRemote,
            "Open HTML · remote images enabled",
        ),
        (
            Char('c'),
            M::NONE,
            "c",
            Action::Compose,
            "Compose mail / create event or contact · editing controls in form",
        ),
        (
            Char('e'),
            M::NONE,
            "e",
            Action::EditItem,
            "Edit selected calendar/contact item",
        ),
        (
            Char('n'),
            M::NONE,
            "n",
            Action::CreateFolder,
            "Create directory / event / contact",
        ),
        (
            Char('r'),
            M::NONE,
            "r",
            Action::Reply,
            "Explain reply capability",
        ),
        (
            Char('R'),
            M::NONE,
            "R",
            Action::ReplyAll,
            "Explain reply-all capability",
        ),
        (
            Char('f'),
            M::NONE,
            "f",
            Action::Forward,
            "Explain forward capability",
        ),
        (
            Char('0'),
            M::NONE,
            "0",
            Action::ViewSlot(0),
            "Mail view · all mail accounts",
        ),
        (
            Char('1'),
            M::NONE,
            "1",
            Action::ViewSlot(1),
            "Calendar view when configured",
        ),
        (
            Char('2'),
            M::NONE,
            "2",
            Action::ViewSlot(2),
            "Contacts view · indexed mail and CardDAV address books",
        ),
        (
            Char('3'),
            M::NONE,
            "3",
            Action::ViewSlot(3),
            "Tasks view when configured",
        ),
        (
            Char('r'),
            M::CONTROL,
            "Ctrl+r",
            Action::Sync,
            "Synchronize calendars or tasks · :sync",
        ),
        (
            Char('v'),
            M::NONE,
            "v",
            Action::TogglePimHistory,
            "Show/hide past events or closed tasks",
        ),
    ];
    definitions
        .into_iter()
        .map(|(code, modifiers, key, action, description)| Binding {
            code,
            modifiers,
            key,
            action,
            description,
        })
        .collect()
}

pub fn validate_bindings() -> anyhow::Result<()> {
    let mut seen = HashSet::new();
    for b in bindings() {
        anyhow::ensure!(
            seen.insert((b.code, b.modifiers.bits())),
            "Keymap conflict: {}",
            b.key
        );
    }
    Ok(())
}

pub fn dispatch(key: KeyEvent) -> Option<Action> {
    let modifiers = key.modifiers & !KeyModifiers::SHIFT;
    bindings()
        .into_iter()
        .find(|b| b.code == key.code && b.modifiers == modifiers)
        .map(|b| b.action)
}

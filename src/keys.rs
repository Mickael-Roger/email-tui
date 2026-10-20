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
    Flag,
    Delete,
    Move,
    Copy,
    Html,
    HtmlRemote,
    SaveAttachments,
    Assistant,
    Reload,
    Quit,
    Compose,
    CreateFolder,
    Reply,
    ReplyAll,
    Forward,
    More,
    Collapse,
    Expand,
    AccountSlot(usize),
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
            "Save attachments from the opened message",
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
            "Local full-text search · fuzzy result picker",
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
            "Move marked messages to Trash · confirmation",
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
            "Fold/unfold account or folder; read message; load more",
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
            "Compose email · Ctrl+T attachments · Ctrl+S submits · Ctrl+O saves draft",
        ),
        (
            Char('n'),
            M::NONE,
            "n",
            Action::CreateFolder,
            "Create directory",
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
            Action::AccountSlot(0),
            "All accounts, shown as top-level tree branches",
        ),
        (
            Char('1'),
            M::NONE,
            "1",
            Action::AccountSlot(1),
            "Account 1 in configuration order",
        ),
        (
            Char('2'),
            M::NONE,
            "2",
            Action::AccountSlot(2),
            "Account 2 in configuration order",
        ),
        (
            Char('3'),
            M::NONE,
            "3",
            Action::AccountSlot(3),
            "Account 3 in configuration order",
        ),
        (
            Char('4'),
            M::NONE,
            "4",
            Action::AccountSlot(4),
            "Account 4 in configuration order",
        ),
        (
            Char('5'),
            M::NONE,
            "5",
            Action::AccountSlot(5),
            "Account 5 in configuration order",
        ),
        (
            Char('6'),
            M::NONE,
            "6",
            Action::AccountSlot(6),
            "Account 6 in configuration order",
        ),
        (
            Char('7'),
            M::NONE,
            "7",
            Action::AccountSlot(7),
            "Account 7 in configuration order",
        ),
        (
            Char('8'),
            M::NONE,
            "8",
            Action::AccountSlot(8),
            "Account 8 in configuration order",
        ),
        (
            Char('9'),
            M::NONE,
            "9",
            Action::AccountSlot(9),
            "Account 9 in configuration order",
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

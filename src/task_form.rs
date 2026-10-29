use crate::{input::Input, pim::Task};
use chrono::{Datelike, NaiveDate, Timelike};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub const STATUSES: [(&str, &str); 4] = [
    ("NEEDS-ACTION", "To do"),
    ("IN-PROCESS", "In progress"),
    ("COMPLETED", "Completed"),
    ("CANCELLED", "Cancelled"),
];
pub const PRIORITIES: [&str; 10] = [
    "None",
    "1 · Highest",
    "2 · Very high",
    "3 · High",
    "4 · Above normal",
    "5 · Normal",
    "6 · Below normal",
    "7 · Low",
    "8 · Very low",
    "9 · Lowest",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    List,
    Title,
    Status,
    Priority,
    Due,
    Start,
    Description,
    Save,
    Cancel,
}
impl Field {
    pub const ALL: [Self; 9] = [
        Self::List,
        Self::Title,
        Self::Status,
        Self::Priority,
        Self::Due,
        Self::Start,
        Self::Description,
        Self::Save,
        Self::Cancel,
    ];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|field| *field == self).unwrap()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    List,
    Status,
    Priority,
}
#[derive(Clone, Debug)]
pub enum Popup {
    Choice { kind: Choice, row: usize },
    Date { field: Field, picker: DatePicker },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Continue,
    Save,
    Cancel,
}

#[derive(Clone, Debug)]
pub struct TaskEditor {
    pub collection: usize,
    pub original: Option<usize>,
    pub title: Input,
    pub description: Input,
    pub due: String,
    pub start: String,
    pub status: String,
    pub priority: String,
    pub focus: Field,
    pub popup: Option<Popup>,
    pub error: Option<String>,
}
impl TaskEditor {
    pub fn new(collection: usize, original: Option<usize>, task: Option<&Task>) -> Self {
        let input = |value: String| {
            let mut input = Input::default();
            input.set(value);
            input
        };
        Self {
            collection,
            original,
            title: input(task.map(|t| t.summary.clone()).unwrap_or_default()),
            description: input(task.map(|t| t.description.clone()).unwrap_or_default()),
            due: task.map(|t| t.due.clone()).unwrap_or_default(),
            start: task.map(|t| t.start.clone()).unwrap_or_default(),
            status: task
                .map(|t| t.status.clone())
                .unwrap_or_else(|| "NEEDS-ACTION".into()),
            priority: task.map(|t| t.priority.clone()).unwrap_or_default(),
            focus: if original.is_some() {
                Field::Title
            } else {
                Field::List
            },
            popup: None,
            error: None,
        }
    }
    pub fn status_label(&self) -> &str {
        STATUSES
            .iter()
            .find(|(value, _)| value.eq_ignore_ascii_case(&self.status))
            .map(|(_, label)| *label)
            .unwrap_or(&self.status)
    }
    pub fn priority_index(&self) -> usize {
        self.priority
            .parse::<usize>()
            .ok()
            .filter(|p| *p <= 9)
            .unwrap_or(0)
    }
    pub fn inputs(&self) -> Vec<Input> {
        [
            &self.title.text,
            &self.due,
            &self.status,
            &self.priority,
            &self.description.text,
            &self.start,
        ]
        .into_iter()
        .map(|text| {
            let mut input = Input::default();
            input.set(text.clone());
            input
        })
        .collect()
    }
    fn move_focus(&mut self, delta: isize) {
        let mut index =
            (self.focus.index() as isize + delta).rem_euclid(Field::ALL.len() as isize) as usize;
        if self.original.is_some() && Field::ALL[index] == Field::List {
            index = if delta > 0 { 1 } else { Field::ALL.len() - 1 };
        }
        self.focus = Field::ALL[index];
    }
    fn choose(&mut self, kind: Choice, row: usize) {
        match kind {
            Choice::List => self.collection = row,
            Choice::Status => self.status = STATUSES[row].0.into(),
            Choice::Priority => self.priority = row.to_string(),
        }
        self.error = None;
    }
    fn choice(&self) -> Option<(Choice, usize)> {
        match self.focus {
            Field::List if self.original.is_none() => Some((Choice::List, self.collection)),
            Field::Status => Some((
                Choice::Status,
                STATUSES
                    .iter()
                    .position(|(status, _)| status.eq_ignore_ascii_case(&self.status))
                    .unwrap_or(0),
            )),
            Field::Priority => Some((Choice::Priority, self.priority_index())),
            _ => None,
        }
    }
    pub fn key(&mut self, key: KeyEvent, lists: usize) -> Action {
        if let Some(popup) = self.popup.take() {
            match popup {
                Popup::Choice { kind, mut row } => {
                    let count = match kind {
                        Choice::List => lists,
                        Choice::Status => STATUSES.len(),
                        Choice::Priority => PRIORITIES.len(),
                    };
                    if key.code == KeyCode::Esc || count == 0 {
                        return Action::Continue;
                    }
                    match key.code {
                        KeyCode::Up => row = (row + count - 1) % count,
                        KeyCode::Down => row = (row + 1) % count,
                        KeyCode::Home => row = 0,
                        KeyCode::End => row = count - 1,
                        KeyCode::Enter => {
                            self.choose(kind, row);
                            return Action::Continue;
                        }
                        KeyCode::Char(c) if c.is_ascii_digit() && key.modifiers.is_empty() => {
                            let number = c.to_digit(10).unwrap() as usize;
                            let index = if kind == Choice::Priority {
                                Some(number)
                            } else {
                                number.checked_sub(1)
                            };
                            if let Some(index) = index.filter(|index| *index < count) {
                                self.choose(kind, index);
                                return Action::Continue;
                            }
                        }
                        _ => {}
                    }
                    self.popup = Some(Popup::Choice { kind, row });
                }
                Popup::Date { field, mut picker } => match picker.key(key) {
                    DateAction::Continue => self.popup = Some(Popup::Date { field, picker }),
                    DateAction::Cancel => {}
                    DateAction::Apply(value) => {
                        if field == Field::Due {
                            self.due = value;
                        } else {
                            self.start = value;
                        }
                        self.error = None;
                    }
                },
            }
            return Action::Continue;
        }
        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::Save;
        }
        match key.code {
            KeyCode::Esc => return Action::Cancel,
            KeyCode::Tab => self.move_focus(1),
            KeyCode::BackTab => self.move_focus(-1),
            KeyCode::Up | KeyCode::Down if self.focus != Field::Description => {
                self.move_focus(if key.code == KeyCode::Up { -1 } else { 1 })
            }
            KeyCode::Left | KeyCode::Right if self.choice().is_some() => {
                let (kind, row) = self.choice().unwrap();
                let count = match kind {
                    Choice::List => lists,
                    Choice::Status => STATUSES.len(),
                    Choice::Priority => PRIORITIES.len(),
                };
                if count > 0 {
                    self.choose(
                        kind,
                        (row + if key.code == KeyCode::Right {
                            1
                        } else {
                            count - 1
                        }) % count,
                    );
                }
            }
            KeyCode::Left | KeyCode::Right if matches!(self.focus, Field::Save | Field::Cancel) => {
                self.focus = if self.focus == Field::Save {
                    Field::Cancel
                } else {
                    Field::Save
                };
            }
            KeyCode::Enter | KeyCode::Char(' ') if self.choice().is_some() => {
                let (kind, row) = self.choice().unwrap();
                self.popup = Some(Popup::Choice { kind, row });
            }
            KeyCode::Enter | KeyCode::Char(' ')
                if matches!(self.focus, Field::Due | Field::Start) =>
            {
                let value = if self.focus == Field::Due {
                    &self.due
                } else {
                    &self.start
                };
                self.popup = Some(Popup::Date {
                    field: self.focus,
                    picker: DatePicker::new(value),
                });
            }
            KeyCode::Delete | KeyCode::Backspace
                if matches!(self.focus, Field::Due | Field::Start) =>
            {
                if self.focus == Field::Due {
                    self.due.clear();
                } else {
                    self.start.clear();
                }
                self.error = None;
            }
            KeyCode::Enter if self.focus == Field::Save => return Action::Save,
            KeyCode::Enter if self.focus == Field::Cancel => return Action::Cancel,
            KeyCode::Enter if self.focus == Field::Description => {
                if self.description.text.len() < 8192 {
                    self.description.text.insert(self.description.cursor, '\n');
                    self.description.cursor += 1;
                }
            }
            KeyCode::Enter => self.move_focus(1),
            _ if self.focus == Field::Title => {
                self.title.key(key);
                self.error = None;
            }
            _ if self.focus == Field::Description => {
                self.description.key(key);
                self.error = None;
            }
            _ => {}
        }
        Action::Continue
    }
    pub fn paste(&mut self, text: &str) {
        if self.popup.is_some() {
            return;
        }
        match self.focus {
            Field::Title => self.title.insert(text),
            Field::Description => {
                let text: String = text
                    .replace("\r\n", "\n")
                    .replace('\r', "\n")
                    .chars()
                    .filter(|c| *c == '\n' || !c.is_control())
                    .collect();
                if self.description.text.len() + text.len() <= 8192 {
                    self.description
                        .text
                        .insert_str(self.description.cursor, &text);
                    self.description.cursor += text.len();
                }
            }
            _ => return,
        }
        self.error = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateMode {
    Date,
    LocalTime,
    Utc,
}
impl DateMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Date => "Date only",
            Self::LocalTime => "With time",
            Self::Utc => "With time · UTC",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatePart {
    Calendar,
    Mode,
    Hour,
    Minute,
    Apply,
    Clear,
}
#[derive(Clone, Debug)]
pub struct DatePicker {
    pub date: NaiveDate,
    pub hour: u32,
    pub minute: u32,
    second: u32,
    pub mode: DateMode,
    pub focus: DatePart,
}
pub enum DateAction {
    Continue,
    Apply(String),
    Cancel,
}
impl DatePicker {
    pub fn new(value: &str) -> Self {
        let raw = value.trim_end_matches('Z');
        let datetime = chrono::NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S").ok();
        let date = datetime
            .map(|d| d.date())
            .or_else(|| NaiveDate::parse_from_str(raw, "%Y%m%d").ok())
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        Self {
            date,
            hour: datetime.map(|d| d.hour()).unwrap_or(9),
            minute: datetime.map(|d| d.minute()).unwrap_or(0),
            second: datetime.map(|d| d.second()).unwrap_or(0),
            mode: if datetime.is_none() {
                DateMode::Date
            } else if value.ends_with('Z') {
                DateMode::Utc
            } else {
                DateMode::LocalTime
            },
            focus: DatePart::Calendar,
        }
    }
    pub fn value(&self) -> String {
        if self.mode == DateMode::Date {
            self.date.format("%Y%m%d").to_string()
        } else {
            format!(
                "{}T{:02}{:02}{:02}{}",
                self.date.format("%Y%m%d"),
                self.hour,
                self.minute,
                self.second,
                if self.mode == DateMode::Utc { "Z" } else { "" }
            )
        }
    }
    fn step_date(&mut self, days: i64) {
        if let Some(date) = self
            .date
            .checked_add_signed(chrono::Duration::days(days))
            .filter(|d| (1..=9999).contains(&d.year()))
        {
            self.date = date;
        }
    }
    fn step_month(&mut self, forward: bool) {
        let date = if forward {
            self.date.checked_add_months(chrono::Months::new(1))
        } else {
            self.date.checked_sub_months(chrono::Months::new(1))
        };
        if let Some(date) = date.filter(|d| (1..=9999).contains(&d.year())) {
            self.date = date;
        }
    }
    fn move_focus(&mut self, delta: isize) {
        let parts: &[DatePart] = if self.mode == DateMode::Date {
            &[
                DatePart::Calendar,
                DatePart::Mode,
                DatePart::Apply,
                DatePart::Clear,
            ]
        } else {
            &[
                DatePart::Calendar,
                DatePart::Mode,
                DatePart::Hour,
                DatePart::Minute,
                DatePart::Apply,
                DatePart::Clear,
            ]
        };
        let index = parts.iter().position(|p| *p == self.focus).unwrap_or(0);
        self.focus = parts[(index as isize + delta).rem_euclid(parts.len() as isize) as usize];
    }
    pub fn key(&mut self, key: KeyEvent) -> DateAction {
        match key.code {
            KeyCode::Esc => return DateAction::Cancel,
            KeyCode::Delete | KeyCode::Backspace => return DateAction::Apply(String::new()),
            KeyCode::Tab => self.move_focus(1),
            KeyCode::BackTab => self.move_focus(-1),
            KeyCode::Enter if self.focus == DatePart::Clear => {
                return DateAction::Apply(String::new());
            }
            KeyCode::Enter if matches!(self.focus, DatePart::Calendar | DatePart::Apply) => {
                return DateAction::Apply(self.value());
            }
            KeyCode::Enter => self.move_focus(1),
            KeyCode::Home | KeyCode::Char('t') if self.focus == DatePart::Calendar => {
                self.date = if self.mode == DateMode::Utc {
                    chrono::Utc::now().date_naive()
                } else {
                    chrono::Local::now().date_naive()
                }
            }
            KeyCode::PageUp => self.step_month(false),
            KeyCode::PageDown => self.step_month(true),
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                let forward = matches!(key.code, KeyCode::Right | KeyCode::Up);
                match self.focus {
                    DatePart::Calendar => self.step_date(match key.code {
                        KeyCode::Left => -1,
                        KeyCode::Right => 1,
                        KeyCode::Up => -7,
                        _ => 7,
                    }),
                    DatePart::Mode => {
                        let modes = [DateMode::Date, DateMode::LocalTime, DateMode::Utc];
                        let index = modes.iter().position(|mode| *mode == self.mode).unwrap();
                        self.mode = modes[(index + if forward { 1 } else { 2 }) % modes.len()];
                    }
                    DatePart::Hour => self.hour = (self.hour + if forward { 1 } else { 23 }) % 24,
                    DatePart::Minute => {
                        self.minute = (self.minute + if forward { 1 } else { 59 }) % 60
                    }
                    _ => self.move_focus(if forward { 1 } else { -1 }),
                }
            }
            _ => {}
        }
        DateAction::Continue
    }
}

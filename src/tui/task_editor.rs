use super::{Palette, block, text};
use crate::{
    app::App,
    input::Input,
    task_form::{
        Choice, DateMode, DatePart, DatePicker, Field, PRIORITIES, Popup, STATUSES, TaskEditor,
    },
};
use chrono::{Datelike, NaiveDate};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = width
        .min(area.width.saturating_sub(2))
        .max(1)
        .min(area.width);
    let height = height
        .min(area.height.saturating_sub(2))
        .max(1)
        .min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect, p: &Palette, editor: &TaskEditor) {
    let rect = popup(area, 84, 27);
    frame.render_widget(Clear, rect);
    let border = block(
        if editor.original.is_some() {
            " Edit task "
        } else {
            " New task "
        },
        p,
    )
    .style(p.base);
    let inner = border.inner(rect);
    frame.render_widget(border, rect);
    let parts = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(u16::from(editor.error.is_some())),
        Constraint::Length(if inner.height >= 7 && editor.error.is_none() {
            2
        } else {
            1
        }),
    ])
    .split(inner);
    let form = Form { app, p, editor };
    if parts[0].width >= 54 && parts[0].height >= 18 {
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(parts[0]);
        form.field(frame, rows[0], Field::List);
        form.field(frame, rows[1], Field::Title);
        for (row, fields) in [
            (rows[2], [Field::Status, Field::Priority]),
            (rows[3], [Field::Due, Field::Start]),
            (rows[5], [Field::Save, Field::Cancel]),
        ] {
            let columns =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .spacing(1)
                    .split(row);
            for (column, field) in columns.iter().zip(fields) {
                form.field(frame, *column, field);
            }
        }
        form.field(frame, rows[4], Field::Description);
    } else {
        // Keep the focused control visible on small terminals.
        let row_height = if parts[0].height >= 6 { 3 } else { 1 };
        let visible = (parts[0].height / row_height).max(1) as usize;
        let first = editor
            .focus
            .index()
            .saturating_sub(visible.saturating_sub(1));
        for (row, field) in Field::ALL.iter().skip(first).take(visible).enumerate() {
            let rect = Rect::new(
                parts[0].x,
                parts[0].y + row as u16 * row_height,
                parts[0].width,
                row_height.min(parts[0].height),
            );
            form.field(frame, rect, *field);
        }
    }
    if let Some(error) = &editor.error {
        frame.render_widget(
            Paragraph::new(text(error)).style(Style::default().fg(p.hot)),
            parts[1],
        );
    }
    let help = if app.pim_pending {
        "Saving task…"
    } else {
        "Tab / Shift+Tab fields · Enter select · Ctrl+S save · Esc cancel\n←/→ choices · Dates open a calendar · Enter adds a line in description"
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(p.muted)),
        parts[2],
    );
    match &editor.popup {
        Some(Popup::Choice { kind, row }) => draw_choices(frame, app, area, p, *kind, *row),
        Some(Popup::Date { field, picker }) => draw_date(frame, area, p, *field, picker),
        None => {}
    }
}

struct Form<'a> {
    app: &'a App,
    p: &'a Palette,
    editor: &'a TaskEditor,
}
impl Form<'_> {
    fn field(&self, frame: &mut Frame, rect: Rect, field: Field) {
        let editor = self.editor;
        let active = editor.focus == field && editor.popup.is_none() && !self.app.pim_pending;
        let (label, value) = match field {
            Field::List => (
                if editor.original.is_some() {
                    "Task list · fixed"
                } else {
                    "Task list ▾"
                },
                self.app
                    .task_collections
                    .get(editor.collection)
                    .map(|c| text(&c.label))
                    .unwrap_or_default(),
            ),
            Field::Title => ("Title", text(&editor.title.text)),
            Field::Status => ("Status ▾", text(editor.status_label())),
            Field::Priority => ("Priority ▾", PRIORITIES[editor.priority_index()].into()),
            Field::Due => ("Due date ▾", date_label(&editor.due)),
            Field::Start => ("Start date ▾", date_label(&editor.start)),
            Field::Description => ("Description", editor.description.text.clone()),
            Field::Save => ("", "Save".into()),
            Field::Cancel => ("", "Cancel".into()),
        };
        let buttons = matches!(field, Field::Save | Field::Cancel);
        let mut style = self.p.base;
        if field == Field::Priority {
            style = style.fg(priority_color(self.p, editor.priority_index()));
        }
        if field == Field::List && editor.original.is_some() {
            style = Style::default().fg(self.p.muted);
        }
        if buttons && active {
            style = self.p.selected;
        }
        let inner = if rect.height >= 3 {
            let border = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if active {
                    self.p.accent
                } else {
                    self.p.muted
                }))
                .title(Span::styled(
                    if label.is_empty() {
                        String::new()
                    } else {
                        format!(" {label} ")
                    },
                    if active {
                        Style::default()
                            .fg(self.p.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(self.p.muted)
                    },
                ));
            let inner = border.inner(rect);
            frame.render_widget(border, rect);
            inner
        } else {
            rect
        };
        let input = match field {
            Field::Title => Some(&editor.title),
            Field::Description => Some(&editor.description),
            _ => None,
        };
        if let Some(input) = input {
            draw_input(
                frame,
                inner,
                input,
                active,
                self.p,
                if field == Field::Title {
                    "Task title"
                } else {
                    "Optional notes…"
                },
            );
        } else {
            let value = if rect.height < 3 && !label.is_empty() {
                format!("{label}: {value}")
            } else {
                value
            };
            frame.render_widget(
                Paragraph::new(value)
                    .alignment(if buttons {
                        Alignment::Center
                    } else {
                        Alignment::Left
                    })
                    .style(if active && !buttons {
                        self.p.selected
                    } else {
                        style
                    }),
                inner,
            );
        }
    }
}

fn date_label(value: &str) -> String {
    if value.is_empty() {
        "Not set".into()
    } else {
        text(&crate::pim::display_task_due(value))
    }
}
fn priority_color(p: &Palette, priority: usize) -> ratatui::style::Color {
    match priority {
        1..=4 => p.priority_high,
        5 => p.priority_medium,
        6..=9 => p.priority_low,
        _ => p.base.fg.unwrap_or(ratatui::style::Color::Reset),
    }
}
fn draw_input(
    frame: &mut Frame,
    rect: Rect,
    input: &Input,
    active: bool,
    p: &Palette,
    placeholder: &str,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let before = &input.text[..input.cursor];
    let line = before.chars().filter(|c| *c == '\n').count();
    let column = UnicodeWidthStr::width(before.rsplit('\n').next().unwrap_or(""));
    let y = line.saturating_sub(rect.height.saturating_sub(1) as usize);
    let x = column.saturating_sub(rect.width.saturating_sub(1) as usize);
    frame.render_widget(
        Paragraph::new(if input.text.is_empty() {
            placeholder.into()
        } else {
            crate::mime::terminal_text(&input.text)
        })
        .style(if input.text.is_empty() {
            Style::default().fg(p.muted)
        } else {
            p.base
        })
        .scroll((
            y.min(u16::MAX as usize) as u16,
            x.min(u16::MAX as usize) as u16,
        )),
        rect,
    );
    if active {
        frame.set_cursor_position((
            rect.x
                + column
                    .saturating_sub(x)
                    .min(rect.width.saturating_sub(1) as usize) as u16,
            rect.y
                + line
                    .saturating_sub(y)
                    .min(rect.height.saturating_sub(1) as usize) as u16,
        ));
    }
}

fn draw_choices(frame: &mut Frame, app: &App, area: Rect, p: &Palette, kind: Choice, row: usize) {
    let (title, options): (&str, Vec<String>) = match kind {
        Choice::List => (
            " Task list ",
            app.task_collections
                .iter()
                .enumerate()
                .map(|(index, c)| format!("{} · {}", index + 1, text(&c.label)))
                .collect(),
        ),
        Choice::Status => (
            " Status ",
            STATUSES
                .iter()
                .enumerate()
                .map(|(index, (_, label))| format!("{} · {label}", index + 1))
                .collect(),
        ),
        Choice::Priority => (
            " Priority ",
            PRIORITIES
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    if index == 0 {
                        "0 · None".into()
                    } else {
                        (*label).into()
                    }
                })
                .collect(),
        ),
    };
    let rect = popup(area, 46, (options.len() as u16).saturating_add(5));
    frame.render_widget(Clear, rect);
    let border = block(title, p).style(p.base);
    let inner = border.inner(rect);
    frame.render_widget(border, rect);
    let parts = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(if inner.height >= 5 { 2 } else { 1 }),
    ])
    .split(inner);
    let first = row.saturating_sub(parts[0].height.saturating_sub(1) as usize);
    let lines: Vec<_> = options
        .iter()
        .enumerate()
        .skip(first)
        .map(|(index, label)| {
            Line::styled(
                format!("{} {label}", if index == row { "›" } else { " " }),
                if index == row {
                    p.selected
                } else if kind == Choice::Priority {
                    p.base.fg(priority_color(p, index))
                } else {
                    p.base
                },
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), parts[0]);
    frame.render_widget(
        Paragraph::new("↑/↓ choose · Enter apply · Esc back\nNumber keys select directly")
            .style(Style::default().fg(p.muted)),
        parts[1],
    );
}

fn draw_date(frame: &mut Frame, area: Rect, p: &Palette, field: Field, picker: &DatePicker) {
    let rect = popup(area, 46, 21);
    frame.render_widget(Clear, rect);
    let border = block(
        if field == Field::Due {
            " Due date "
        } else {
            " Start date "
        },
        p,
    )
    .style(p.base);
    let inner = border.inner(rect);
    frame.render_widget(border, rect);
    if inner.height >= 19 && inner.width >= 26 {
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(inner);
        date_control(
            frame,
            rows[0],
            "Mode · ←/→",
            picker.mode.label(),
            picker.focus == DatePart::Mode,
            p,
        );
        calendar(frame, rows[1], picker, p);
        if picker.mode == DateMode::Date {
            frame.render_widget(
                Paragraph::new("No time set · choose a timed mode to add one")
                    .style(Style::default().fg(p.muted)),
                rows[2],
            );
        } else {
            let columns =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .spacing(1)
                    .split(rows[2]);
            date_control(
                frame,
                columns[0],
                "Hour · ↑/↓",
                &format!("{:02}", picker.hour),
                picker.focus == DatePart::Hour,
                p,
            );
            date_control(
                frame,
                columns[1],
                "Minute · ↑/↓",
                &format!("{:02}", picker.minute),
                picker.focus == DatePart::Minute,
                p,
            );
        }
        let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .spacing(1)
            .split(rows[3]);
        date_control(
            frame,
            columns[0],
            "",
            "Use date",
            picker.focus == DatePart::Apply,
            p,
        );
        date_control(
            frame,
            columns[1],
            "",
            "No date",
            picker.focus == DatePart::Clear,
            p,
        );
        frame.render_widget(Paragraph::new("Arrows date · PgUp/PgDn month · Home today\nTab options · Enter apply · Del clear · Esc").style(Style::default().fg(p.muted)), rows[4]);
    } else {
        let parts = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(if inner.height >= 6 { 2 } else { 1 }),
        ])
        .split(inner);
        let mut controls = vec![
            (
                DatePart::Calendar,
                format!("Date: ‹ {} ›", picker.date.format("%Y-%m-%d")),
            ),
            (DatePart::Mode, format!("Mode: ‹ {} ›", picker.mode.label())),
        ];
        if picker.mode != DateMode::Date {
            controls.extend([
                (DatePart::Hour, format!("Hour: ‹ {:02} ›", picker.hour)),
                (
                    DatePart::Minute,
                    format!("Minute: ‹ {:02} ›", picker.minute),
                ),
            ]);
        }
        controls.extend([
            (DatePart::Apply, "[ Use date ]".into()),
            (DatePart::Clear, "[ No date ]".into()),
        ]);
        let selected = controls
            .iter()
            .position(|(part, _)| *part == picker.focus)
            .unwrap_or(0);
        let first = selected.saturating_sub(parts[0].height.saturating_sub(1) as usize);
        let lines: Vec<_> = controls
            .into_iter()
            .skip(first)
            .map(|(part, value)| {
                Line::styled(
                    value,
                    if part == picker.focus {
                        p.selected
                    } else {
                        p.base
                    },
                )
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), parts[0]);
        frame.render_widget(
            Paragraph::new(
                "Arrows change · Tab · Enter apply\nPgUp/PgDn month · Del clear · Esc back",
            )
            .style(Style::default().fg(p.muted)),
            parts[1],
        );
    }
}

fn date_control(
    frame: &mut Frame,
    rect: Rect,
    label: &str,
    value: &str,
    active: bool,
    p: &Palette,
) {
    frame.render_widget(
        Paragraph::new(value)
            .alignment(Alignment::Center)
            .style(if active { p.selected } else { p.base })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(label)
                    .border_style(Style::default().fg(if active { p.accent } else { p.muted })),
            ),
        rect,
    );
}
fn calendar(frame: &mut Frame, area: Rect, picker: &DatePicker, p: &Palette) {
    let rect = Rect::new(
        area.x + area.width.saturating_sub(21) / 2,
        area.y,
        area.width.min(21),
        area.height,
    );
    let first = picker.date.with_day(1).unwrap();
    let offset = first.weekday().num_days_from_monday() as i64;
    let today = if picker.mode == DateMode::Utc {
        chrono::Utc::now().date_naive()
    } else {
        chrono::Local::now().date_naive()
    };
    let mut lines = vec![
        Line::from(picker.date.format("%B %Y").to_string()).alignment(Alignment::Center),
        Line::styled("Mo Tu We Th Fr Sa Su", Style::default().fg(p.muted)),
    ];
    for week in 0..6 {
        let mut spans = vec![];
        for weekday in 0..7 {
            let index = week * 7 + weekday;
            let date: Option<NaiveDate> =
                first.checked_add_signed(chrono::Duration::days(index - offset));
            if let Some(date) = date.filter(|date| date.month() == picker.date.month()) {
                let style = if date == picker.date && picker.focus == DatePart::Calendar {
                    p.selected
                } else if date == picker.date {
                    Style::default()
                        .fg(p.accent)
                        .add_modifier(Modifier::UNDERLINED)
                } else if date == today {
                    Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
                } else {
                    p.base
                };
                spans.push(Span::styled(format!("{:>2} ", date.day()), style));
            } else {
                spans.push(Span::raw("   "));
            }
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), rect);
}

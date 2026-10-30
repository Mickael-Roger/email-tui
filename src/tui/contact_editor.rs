use super::{
    Palette, block,
    task_editor::{draw_input, popup},
    text,
};
use crate::app::{App, Overlay};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect, p: &Palette, overlay: &Overlay) {
    let Overlay::PimForm {
        collection,
        original,
        fields,
        field,
        error,
        book_menu,
        contact_page,
        ..
    } = overlay
    else {
        return;
    };
    let rect = popup(area, 84, if *contact_page == 0 { 27 } else { 29 });
    frame.render_widget(Clear, rect);
    let border = block(
        if original.is_some() {
            " Edit CardDAV contact "
        } else {
            " New CardDAV contact "
        },
        p,
    )
    .style(p.base);
    let inner = border.inner(rect);
    frame.render_widget(border, rect);
    let parts = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(u16::from(error.is_some())),
        Constraint::Length(if inner.height >= 7 { 2 } else { 1 }),
    ])
    .split(inner);
    let tabs = ["General", "Personal", "Work", "Other"]
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            Span::styled(
                format!(" {label} "),
                if index == *contact_page {
                    p.selected
                } else {
                    Style::default().fg(p.muted)
                },
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Line::from(tabs)).alignment(Alignment::Center),
        parts[0],
    );
    let form = parts[1];
    let save = fields.len() + 1;
    let cancel = save + 1;
    let mut positions = vec![0];
    if *contact_page == 0 {
        positions.extend(1..=4);
    } else {
        let start = 5 + (contact_page - 1) * 5;
        positions.extend(start..start + 5);
    }
    positions.extend([save, cancel]);
    let label_for = |position: usize| -> &str {
        if position == save {
            "Save"
        } else if position == cancel {
            "Cancel"
        } else if position <= 4 {
            ["Address book", "Name", "Email", "Phone", "Organization"][position]
        } else {
            [
                "Street / address lines",
                "City",
                "Region / State",
                "Postal code",
                "Country",
            ][(position - 5) % 5]
        }
    };
    let placeholder_for = |position: usize| -> &str {
        if position <= 4 {
            [
                "",
                "Full name (required)",
                "name@example.com",
                "+33…",
                "Company or organization",
            ][position]
        } else {
            [
                "Street, building, apartment…",
                "City",
                "Region / State / Province",
                "Postal code",
                "Country",
            ][(position - 5) % 5]
        }
    };
    let row_height = if form.height >= 6 { 3 } else { 1 };
    let street_extra = if *contact_page == 0 { 0 } else { 2 };
    let roomy = form.height >= (positions.len() as u16 - 1) * 3 + street_extra;
    let visible = (form.height / row_height).max(1) as usize;
    let focused = positions
        .iter()
        .position(|position| position == field)
        .unwrap_or(0);
    let first = if roomy {
        0
    } else {
        focused.saturating_sub(visible.saturating_sub(1))
    };
    for (slot, &position) in
        positions
            .iter()
            .enumerate()
            .skip(first)
            .take(if roomy { positions.len() } else { visible })
    {
        let rect = if roomy && position >= save {
            let buttons = Rect::new(
                form.x,
                form.y + (positions.len() as u16 - 2) * 3 + street_extra,
                form.width,
                3,
            );
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .spacing(1)
                .split(buttons)[position - save]
        } else {
            Rect::new(
                form.x,
                form.y
                    + (slot - first) as u16 * row_height
                    + if roomy && slot > 1 { street_extra } else { 0 },
                form.width,
                (row_height + if roomy && slot == 1 { street_extra } else { 0 }).min(form.height),
            )
        };
        let active = *field == position && book_menu.is_none() && !app.pim_pending;
        let button = position >= save;
        let label = if position == 0 {
            if original.is_some() {
                "Address book · fixed"
            } else {
                "Address book ▾"
            }
        } else if button {
            ""
        } else {
            label_for(position)
        };
        let content = if rect.height >= 3 {
            let border = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if active { p.accent } else { p.muted }))
                .title(Span::styled(
                    if label.is_empty() {
                        String::new()
                    } else {
                        format!(" {label} ")
                    },
                    Style::default()
                        .fg(if active { p.accent } else { p.muted })
                        .add_modifier(if active {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ));
            let content = border.inner(rect);
            frame.render_widget(border, rect);
            content
        } else {
            rect
        };
        if position > 0 && position <= fields.len() {
            if rect.height < 3 {
                // A compact label keeps the focused field identifiable even on tiny terminals.
                let columns = Layout::horizontal([
                    Constraint::Length((label_for(position).len() + 2) as u16),
                    Constraint::Min(1),
                ])
                .split(content);
                frame.render_widget(
                    Paragraph::new(format!("{}: ", label_for(position)))
                        .style(Style::default().fg(if active { p.accent } else { p.muted })),
                    columns[0],
                );
                draw_input(
                    frame,
                    columns[1],
                    &fields[position - 1],
                    active,
                    p,
                    placeholder_for(position),
                );
            } else {
                draw_input(
                    frame,
                    content,
                    &fields[position - 1],
                    active,
                    p,
                    placeholder_for(position),
                );
            }
        } else {
            let value = if position == 0 {
                let name = app
                    .contact_books
                    .get(*collection)
                    .map(|book| text(&book.label))
                    .unwrap_or_default();
                if rect.height < 3 {
                    format!("{label}: {name}")
                } else {
                    name
                }
            } else {
                label_for(position).into()
            };
            frame.render_widget(
                Paragraph::new(value)
                    .alignment(if button {
                        Alignment::Center
                    } else {
                        Alignment::Left
                    })
                    .style(if active {
                        p.selected
                    } else if position == 0 && original.is_some() {
                        Style::default().fg(p.muted)
                    } else {
                        p.base
                    }),
                content,
            );
        }
    }
    if let Some(error) = error {
        frame.render_widget(
            Paragraph::new(text(error)).style(Style::default().fg(p.hot)),
            parts[2],
        );
    }
    frame.render_widget(Paragraph::new(if app.pim_pending { "Saving contact…" } else {
        "PgUp/PgDn sections · Tab fields · Ctrl+S save · Esc cancel\nEnter select / next · Alt+Enter new street line · ←/→ choices"
    }).style(Style::default().fg(p.muted)), parts[3]);
    if let Some(row) = book_menu {
        let rect = popup(
            area,
            52,
            app.contact_books
                .len()
                .saturating_add(4)
                .min(u16::MAX as usize) as u16,
        );
        frame.render_widget(Clear, rect);
        let border = block(" Address book · ↑/↓ choose · Enter select ", p).style(p.base);
        let inner = border.inner(rect);
        frame.render_widget(border, rect);
        let first = row.saturating_sub(inner.height.saturating_sub(1) as usize);
        let lines: Vec<_> = app
            .contact_books
            .iter()
            .enumerate()
            .skip(first)
            .map(|(index, book)| {
                Line::styled(
                    format!(
                        "{} {} · {}",
                        if index == *row { "›" } else { " " },
                        text(&book.label),
                        text(&book.id)
                    ),
                    if index == *row { p.selected } else { p.base },
                )
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

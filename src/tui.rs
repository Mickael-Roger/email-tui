use crate::{
    app::{App, InputMode, Overlay, View, bindings},
    mime::terminal_text,
    tree::{self, Node},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, TableState, Wrap},
};
use unicode_width::UnicodeWidthStr;

struct Palette {
    base: Style,
    accent: Color,
    muted: Color,
    hot: Color,
    account: Color,
    selected: Style,
    border: Color,
}
fn palette(theme: &str) -> Palette {
    match theme {
        "no-color" => Palette {
            base: Style::default(),
            accent: Color::Reset,
            muted: Color::Reset,
            hot: Color::Reset,
            account: Color::Reset,
            selected: Style::default().add_modifier(Modifier::REVERSED),
            border: Color::Reset,
        },
        "high-contrast" => Palette {
            base: Style::default().fg(Color::White).bg(Color::Black),
            accent: Color::Cyan,
            muted: Color::White,
            hot: Color::Yellow,
            account: Color::Indexed(214),
            selected: Style::default().fg(Color::Black).bg(Color::Yellow),
            border: Color::White,
        },
        _ => Palette {
            base: Style::default().fg(Color::White).bg(Color::Black),
            accent: Color::Cyan,
            muted: Color::DarkGray,
            hot: Color::Yellow,
            account: Color::Indexed(214),
            selected: Style::default()
                .fg(Color::White)
                .bg(Color::Indexed(24))
                .add_modifier(Modifier::BOLD),
            border: Color::Cyan,
        },
    }
}
fn text(s: &str) -> String {
    terminal_text(s).replace(['\n', '\t', '\r'], " ")
}
fn block(title: impl Into<String>, p: &Palette) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .title(Span::styled(
            title.into(),
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Center)
}

/// The legacy preview argument is accepted for API/config compatibility only.
/// Mail browsing always renders one full-width Messages tree.
pub fn draw(frame: &mut Frame, app: &mut App, _preview: bool) {
    let p = palette(&app.theme);
    let area = frame.area();
    frame.render_widget(Block::default().style(p.base), area);
    if area.width < 20 || area.height < 6 {
        frame.render_widget(Paragraph::new("MailTUI\nResize terminal\n:q to quit"), area);
        return;
    }
    let compact = area.width < 50 || area.height < 15;
    let parts = Layout::vertical([
        Constraint::Length(if !app.show_header {
            0
        } else if compact {
            3
        } else {
            (app.accounts.len() + 1).clamp(7, 10) as u16
        }),
        Constraint::Min(1),
        Constraint::Length(u16::from(app.show_breadcrumbs)),
        Constraint::Length(1),
        Constraint::Length(u16::from(app.input.is_some())),
    ])
    .split(area);
    header(frame, app, parts[0], &p, compact);
    app.page_height = parts[1].height.saturating_sub(3) as usize;
    match app.view {
        View::Messages => messages(frame, app, parts[1], &p),
        View::Calendar => {
            let mut labels = vec!["START", "EVENT"];
            let mut widths = vec![Constraint::Length(20), Constraint::Min(10)];
            if area.width >= 60 {
                labels.push("CALENDAR");
                widths.push(Constraint::Length(14));
            }
            if area.width >= 90 {
                labels.push("LOCATION");
                widths.push(Constraint::Length(18));
            }
            if area.width >= 115 {
                labels.push("END");
                widths.push(Constraint::Length(20));
            }
            let rows = app
                .pim_rows()
                .into_iter()
                .map(|i| {
                    let event = &app.calendar_events[i];
                    let mut cells = vec![
                        crate::pim::display_date(&event.start),
                        text(&format!(
                            "{}{}",
                            if event.recurrence.is_empty() {
                                ""
                            } else {
                                "↻ "
                            },
                            event.summary
                        )),
                    ];
                    if area.width >= 60 {
                        cells.push(text(&event.collection_label));
                    }
                    if area.width >= 90 {
                        cells.push(text(&event.location));
                    }
                    if area.width >= 115 {
                        cells.push(crate::pim::display_date(&event.end));
                    }
                    Row::new(cells)
                })
                .collect();
            table(
                frame,
                app,
                parts[1],
                &p,
                &format!(
                    " Calendar · {} entries{} ",
                    app.pim_rows().len(),
                    if app.pim_loading {
                        " · refreshing"
                    } else {
                        ""
                    }
                ),
                labels,
                rows,
                widths,
            );
        }
        View::Contacts => {
            let mut labels = vec!["NAME"];
            let mut widths = vec![Constraint::Min(14)];
            if area.width >= 45 {
                labels.push("EMAIL");
                widths.push(Constraint::Length(26));
            }
            if area.width >= 65 {
                labels.push("ADDRESS BOOK");
                widths.push(Constraint::Length(14));
            }
            if area.width >= 90 {
                labels.push("PHONE");
                widths.push(Constraint::Length(18));
            }
            if area.width >= 120 {
                labels.push("ORGANIZATION");
                widths.push(Constraint::Length(18));
            }
            let rows = app
                .pim_rows()
                .into_iter()
                .map(|i| {
                    let contact = &app.contacts[i];
                    let mut cells = vec![text(&contact.name)];
                    if area.width >= 45 {
                        cells.push(text(&contact.email));
                    }
                    if area.width >= 65 {
                        cells.push(text(&contact.book_label));
                    }
                    if area.width >= 90 {
                        cells.push(text(&contact.phone));
                    }
                    if area.width >= 120 {
                        cells.push(text(&contact.organization));
                    }
                    Row::new(cells)
                })
                .collect();
            table(
                frame,
                app,
                parts[1],
                &p,
                &format!(
                    " Contacts · {} people{} ",
                    app.pim_rows().len(),
                    if app.pim_loading {
                        " · refreshing"
                    } else {
                        ""
                    }
                ),
                labels,
                rows,
                widths,
            );
        }
        View::Accounts => {
            let rows = app
                .account_rows()
                .into_iter()
                .map(|index| {
                    let a = index.map(|i| &app.accounts[i]);
                    let unread: u64 = app
                        .folders
                        .iter()
                        .filter(|f| a.is_none_or(|a| f.account == a.id))
                        .map(|f| f.unread)
                        .sum();
                    Row::new(vec![
                        if a.map(|a| &a.id) == app.scope.account.as_ref() {
                            "*".into()
                        } else {
                            String::new()
                        },
                        a.map(|a| text(&a.id)).unwrap_or_else(|| "all".into()),
                        a.map(|a| text(&a.label))
                            .unwrap_or_else(|| "All accounts".into()),
                        a.map(|a| text(&a.email)).unwrap_or_else(|| "—".into()),
                        unread.to_string(),
                    ])
                })
                .collect();
            table(
                frame,
                app,
                parts[1],
                &p,
                &format!(" Accounts(all)[{}] ", app.account_rows().len()),
                vec!["ACTIVE", "ACCOUNT", "NAME", "ADDRESS", "UNREAD"],
                rows,
                vec![
                    Constraint::Length(6),
                    Constraint::Length(12),
                    Constraint::Length(14),
                    Constraint::Min(15),
                    Constraint::Length(7),
                ],
            );
        }
        View::Detail => {
            let metadata = app
                .detail
                .as_ref()
                .map(|m| {
                    format!(
                        "{}\nFrom: {}\nTo: {}\n{} / {} · {}\n\n",
                        text(&m.subject),
                        text(&m.sender),
                        app.parsed
                            .as_ref()
                            .map(|p| text(&p.recipients))
                            .unwrap_or_default(),
                        text(&m.id.account),
                        text(&m.id.folder),
                        text(&m.date)
                    )
                })
                .unwrap_or_default();
            let body = app
                .parsed
                .as_ref()
                .map(|p| {
                    format!(
                        "{}\n\nAttachments: {}",
                        p.text,
                        if p.attachments.is_empty() {
                            "none".into()
                        } else {
                            p.attachments
                                .iter()
                                .map(|a| {
                                    format!(
                                        "{} {} ({} KiB)",
                                        crate::files::icon(&a.content_type),
                                        text(&a.name),
                                        a.bytes.len().div_ceil(1024)
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                                + " · Ctrl+S save"
                        }
                    )
                })
                .unwrap_or_else(|| {
                    if app.busy {
                        "Reading local MIME…".into()
                    } else {
                        "Message body unavailable. See status below.".into()
                    }
                });
            paragraph(frame, app, parts[1], &p, format!("{metadata}{body}"));
        }
        View::Pending => paragraph(
            frame,
            app,
            parts[1],
            &p,
            if app.pending.is_empty() {
                "No outgoing mail.".into()
            } else {
                app.pending
                    .iter()
                    .map(|mail| {
                        format!(
                            "{} · #{} · {}",
                            text(&mail.account),
                            mail.id,
                            mail.error
                                .as_ref()
                                .or_else(|| app.send_errors.get(&mail.account))
                                .map_or_else(|| "Sending".into(), |error| text(error))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        ),
        View::Status => paragraph(
            frame,
            app,
            parts[1],
            &p,
            format!(
                "Backend: {}\nAccounts: {}\nFolders: {}\nLoaded message rows: {} / {} maximum\nExpanded folders: {}\nOutgoing mail: {}\n\nBrowser: one Messages tree, accounts → folders → mail\nMetadata: lazy per-folder pages, no MIME in list reads\nStorage: public reader + producer queue\nLive updates: public change cursors, every 2s\nSearch: MailTUI local SQLite FTS5 + fuzzy metadata\nIndex updates: additions/deletions/hydration\nViewer: loopback · h/H and page button control remote images\n\nLatest status:\n{}",
                if app.demo {
                    "DEMO (volatile)"
                } else {
                    "pimdir"
                },
                app.accounts.len(),
                app.folders.len(),
                app.messages.len(),
                crate::app::MAX_LOADED,
                app.expanded.len(),
                app.pending.len(),
                app.toast
            ),
        ),
        View::Help => paragraph(frame, app, parts[1], &p, help(&app.filter)),
        View::History => paragraph(
            frame,
            app,
            parts[1],
            &p,
            app.history
                .iter()
                .map(|s| format!(":{}", text(s)))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    }
    let context = if matches!(app.view, View::Calendar | View::Contacts) {
        format!(
            "{} collections · / filter · Enter details",
            if app.view == View::Calendar {
                app.calendar_collections.len()
            } else {
                app.contact_books.len()
            }
        )
    } else {
        app.focused_folder()
            .map(|(a, f)| format!("{} / {}", text(&a), text(&f)))
            .unwrap_or_else(|| "account → folder → messages".into())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(
                    " {}({}) ",
                    app.view.name().to_lowercase(),
                    if matches!(app.view, View::Calendar | View::Contacts) {
                        "all collections".into()
                    } else {
                        text(app.account_name())
                    }
                ),
                if app.theme == "no-color" {
                    p.selected
                } else {
                    Style::default()
                        .fg(Color::Black)
                        .bg(p.hot)
                        .add_modifier(Modifier::BOLD)
                },
            ),
            Span::styled(
                format!(
                    "  {context}  {} marked  {}",
                    app.selected.len(),
                    if app.filter.is_empty() {
                        String::new()
                    } else {
                        format!("/{}", text(&app.filter))
                    }
                ),
                Style::default().fg(p.accent),
            ),
        ])),
        parts[2],
    );
    frame.render_widget(
        Paragraph::new(text(app.quit_hint().unwrap_or(&app.toast)))
            .style(Style::default().fg(p.hot)),
        parts[3],
    );
    if let Some((mode, input)) = &app.input {
        let prefix = match mode {
            InputMode::Command => ":",
            InputMode::Filter => "/",
            InputMode::Move => "move to: ",
            InputMode::Copy => "copy to: ",
        };
        let before = &input.text[..input.cursor];
        let available = parts[4].width.saturating_sub(prefix.len() as u16 + 1) as usize;
        let start = if UnicodeWidthStr::width(before) <= available {
            0
        } else {
            unicode_segmentation::UnicodeSegmentation::grapheme_indices(before, true)
                .map(|(i, _)| i)
                .find(|i| UnicodeWidthStr::width(&before[*i..]) <= available)
                .unwrap_or(before.len())
        };
        frame.render_widget(
            Paragraph::new(format!("{prefix}{}", &input.text[start..]))
                .style(Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
            parts[4],
        );
        frame.set_cursor_position((
            parts[4].x
                + (prefix.len() + UnicodeWidthStr::width(&before[start..]))
                    .min(parts[4].width.saturating_sub(1) as usize) as u16,
            parts[4].y,
        ));
        if matches!(mode, InputMode::Command) && app.quit_hint().is_none() {
            frame.render_widget(
                Paragraph::new(
                    app.completions(&input.text)
                        .into_iter()
                        .take(6)
                        .collect::<Vec<_>>()
                        .join("  "),
                )
                .style(Style::default().fg(p.muted)),
                parts[3],
            );
        }
    }
    if let Some(overlay) = &app.overlay {
        let (title,body) = match overlay {
            Overlay::Assistant => { assistant(frame, app, area, &p); return; },
            Overlay::Help => (" Keyboard help · ↑/↓ scroll · Esc close ",help("")),
            Overlay::PimDetails(body) => (" Entry details · ↑/↓ scroll · Esc close ",body.clone()),
            Overlay::Aliases => (" Resource aliases · Esc close ","mail, messages        Single account/folder/mail tree\nfolders, directories  Focus folder in Messages (same view)\ndir <path>            Expand/focus folder in active account\naccounts, acc, ns      Account namespace selector\nns <account|all>       Switch account scope\ninbox                 Expand inbox in current scope\nunified, all          All accounts with inboxes expanded\nsearch <query>        Results grouped in the Messages tree\nmore                  Next page of focused folder\noutbox / status       Pending operations / diagnostics\nhelp / history        Keyboard reference / commands\nquit, q               Exit".into()),
            Overlay::Confirm { ids,confirm } => { delete_dialog(frame,app,area,&p,ids,*confirm); return; }
            Overlay::Find { .. } | Overlay::Move { .. } | Overlay::CreateFolder { .. } | Overlay::Compose { .. } | Overlay::Files { .. } | Overlay::Attachments { .. } | Overlay::SaveAttachments { .. } | Overlay::PimForm { .. } => { form_dialog(frame, app, area, &p, overlay); return; }
            Overlay::Quit => (" Outgoing mail ",format!("{} messages have not finished sending.\n\nEnter: quit · Esc: return",app.pending.len())),
            Overlay::PimDelete { contact,index,confirm } => (" Confirm deletion ",format!("Delete {}?\n{}\n\n{} · n/Esc cancel",text(if *contact {app.contacts.get(*index).map(|c|c.name.as_str()).unwrap_or("contact")} else {app.calendar_events.get(*index).map(|e|e.summary.as_str()).unwrap_or("event")}),if !*contact && app.calendar_events.get(*index).is_some_and(|e|!e.recurrence.is_empty()){"This deletes the entire recurring series, including exceptions."}else{""},if *confirm{"[Delete selected] Enter confirms"}else{"[Cancel selected] Press y to select Delete"})),
            Overlay::Url(url) => (" Browser viewer · private URL ",format!("{}\n\n{url}\n\nThis bearer URL expires automatically.\nExit revokes all sessions.\nEsc: close",
                if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() { "SSH detected: this URL belongs to the remote host.\nUse an explicit localhost port forward to access it locally." } else { "Open this URL in your browser:" })),
        };
        let popup = centered(
            area,
            if matches!(overlay, Overlay::Help) {
                90
            } else {
                78
            },
            80,
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(body)
                .style(p.base)
                .block(block(title, &p))
                .wrap(Wrap { trim: false })
                .scroll((
                    if matches!(overlay, Overlay::Help | Overlay::PimDetails(_)) {
                        app.help_scroll.min(u16::MAX as usize) as u16
                    } else {
                        0
                    },
                    0,
                )),
            popup,
        );
    }
}

fn header(frame: &mut Frame, app: &App, area: Rect, p: &Palette, compact: bool) {
    if area.height == 0 {
        return;
    }
    let key_style = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let scope_style = Style::default()
        .fg(if app.theme == "no-color" {
            Color::Reset
        } else {
            Color::Magenta
        })
        .add_modifier(Modifier::BOLD);
    let mut views = vec![(
        0,
        "Mail",
        !matches!(app.view, View::Calendar | View::Contacts),
    )];
    if !app.calendar_collections.is_empty() {
        views.push((1, "Calendar", app.view == View::Calendar));
    }
    if !app.contact_books.is_empty() {
        views.push((2, "Contacts", app.view == View::Contacts));
    }
    if compact {
        let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);
        frame.render_widget(
            Paragraph::new(
                views
                    .iter()
                    .map(|(n, label, _)| format!("<{n}> {label}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .style(key_style),
            columns[0],
        );
        frame.render_widget(
            Paragraph::new(if matches!(app.view, View::Calendar | View::Contacts) {
                "<c> Create · <e> Edit\n<enter> Details\n<Ctrl+D> Delete"
            } else {
                "<t> Read/unread\n<m> Move mail\n<c> Compose mail"
            })
            .style(key_style),
            columns[1],
        );
        return;
    }
    let logo_width = if area.width >= 140 { 24 } else { 0 };
    let columns = Layout::horizontal([
        Constraint::Percentage(35),
        Constraint::Percentage(35),
        Constraint::Min(10),
        Constraint::Length(logo_width),
    ])
    .spacing(1)
    .split(area);
    let hints = match app.view {
        View::Calendar => vec![
            ("<c>", "New event"),
            ("<e>", "Edit event"),
            ("<ctrl-d>", "Delete event"),
            ("<0>", "Mail"),
        ],
        View::Contacts => vec![
            ("<c>", "New contact"),
            ("<e>", "Edit contact"),
            ("<ctrl-d>", "Delete contact"),
            ("<0>", "Mail"),
        ],
        View::Accounts => vec![
            ("<?>", "Help"),
            ("<enter/u>", "Use account"),
            ("<0>", "All accounts"),
            ("</>", "Filter"),
            ("<:>", "Commands"),
            ("<esc>", "Back"),
        ],
        View::Detail => vec![
            ("<?>", "Help"),
            ("<t>", "Read / unread"),
            ("<ctrl-d>", "Delete"),
            ("<h/H>", "HTML / remote"),
            ("<ctrl-s>", "Save attachments"),
            ("<pgdn/up>", "Page"),
            ("<esc>", "Back"),
        ],
        _ => vec![
            ("<?>", "Help"),
            ("<t>", "Read / unread"),
            ("<ctrl-d>", "Delete mail"),
            ("<enter>", "Unfold / read"),
            ("<h/H>", "HTML / remote"),
            ("</>", "Local search"),
        ],
    };
    let extra = if matches!(app.view, View::Calendar | View::Contacts) {
        [
            ("<enter>", "Details"),
            ("</>", "Filter all collections"),
            ("<:reload>", "Reload files"),
            ("<?> ", "Help"),
            ("<ctrl-c>", "Twice to quit"),
            ("<esc>", "Back"),
        ]
    } else {
        [
            ("<m>", "Move mail"),
            ("<c>", "Compose mail"),
            ("<n>", "New directory"),
            ("<space>", "Mark mail"),
            ("<ctrl-c>", "Twice to quit"),
            ("<esc>", "Back"),
        ]
    };
    for (column, hints) in [hints, extra.to_vec()].into_iter().enumerate() {
        frame.render_widget(
            Paragraph::new(
                hints
                    .into_iter()
                    .map(|(key, label)| {
                        Line::from(vec![
                            Span::styled(format!("{key:<11}"), key_style),
                            Span::styled(label, Style::default().fg(p.muted)),
                        ])
                    })
                    .collect::<Vec<_>>(),
            ),
            columns[column],
        );
    }
    frame.render_widget(
        Paragraph::new(
            views
                .into_iter()
                .map(|(i, name, active)| {
                    Line::from(vec![
                        Span::styled(format!("<{i}> "), scope_style),
                        Span::styled(
                            text(name),
                            if active {
                                Style::default().fg(p.account).add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(p.muted)
                            },
                        ),
                    ])
                })
                .chain([Line::styled(
                    app.pending_label(),
                    Style::default().fg(p.muted),
                )])
                .collect::<Vec<_>>(),
        ),
        columns[2],
    );
    if logo_width > 0 {
        frame.render_widget(Paragraph::new(" __  __       _ _ \n|  \\/  | __ _(_) |\n| |\\/| |/ _` | | |\n| |  | | (_| | | |\n|_|  |_|\\__,_|_|_|\n       T U I      ").alignment(Alignment::Right).style(Style::default().fg(p.hot).add_modifier(Modifier::BOLD)),columns[3]);
    }
}

fn messages(frame: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    let sender = area.width >= 60;
    let date = area.width >= 75;
    let counts = area.width >= 90;
    let size = app.wide && area.width >= 100;
    let tree = app.tree_rows();
    let rows = tree
        .iter()
        .map(|row| {
            let indent = "  ".repeat(row.depth.min(24));
            let (_status, name, from, when, count, bytes, style) = match row.node {
                Node::Account(i) => {
                    let a = &app.accounts[i];
                    let open = !app.collapsed_accounts.contains(&a.id);
                    let (unread, total) = app
                        .folders
                        .iter()
                        .filter(|f| f.account == a.id)
                        .fold((0u64, 0u64), |(u, t), f| (u + f.unread, t + f.total));
                    (
                        String::new(),
                        format!(
                            "{indent}{} 📬 {}{}",
                            if open { "▾" } else { "▸" },
                            text(&a.id),
                            if counts {
                                String::new()
                            } else {
                                format!(" ({unread}/{total})")
                            }
                        ),
                        text(&a.email),
                        String::new(),
                        format!("{unread}/{total}"),
                        String::new(),
                        Style::default().fg(p.account).add_modifier(Modifier::BOLD),
                    )
                }
                Node::Folder(i) => {
                    let f = &app.folders[i];
                    let open = app.expanded.contains(&tree::folder_key(f));
                    (
                        String::new(),
                        format!(
                            "{indent}{} {} {}{}",
                            if open { "▾" } else { "▸" },
                            if open { "📂" } else { "📁" },
                            text(&f.name),
                            if counts {
                                String::new()
                            } else {
                                format!(" ({}/{})", f.unread, f.total)
                            }
                        ),
                        String::new(),
                        String::new(),
                        format!("{}/{}", f.unread, f.total),
                        String::new(),
                        Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
                    )
                }
                Node::Message(i) => {
                    let m = &app.messages[i];
                    let marked = app.selected.contains(&m.id);
                    (
                        String::new(),
                        format!(
                            "{indent}{}{}{}",
                            if m.flagged { "★ " } else { "" },
                            if m.attachment { "📎 " } else { "" },
                            text(&m.subject)
                        ),
                        text(&m.sender),
                        text(m.date.get(..16).unwrap_or(&m.date)).replace('T', " "),
                        String::new(),
                        format!("{}K", m.size.div_ceil(1024)),
                        if marked {
                            if app.theme == "no-color" {
                                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                            } else {
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(p.hot)
                                    .add_modifier(Modifier::BOLD)
                            }
                        } else if !m.seen {
                            let style = Style::default().add_modifier(Modifier::BOLD);
                            if app.theme == "no-color" {
                                style
                            } else {
                                style.fg(Color::Green)
                            }
                        } else {
                            Style::default()
                        },
                    )
                }
                Node::More(i) => {
                    let f = &app.folders[i];
                    let loaded = app
                        .messages
                        .iter()
                        .filter(|m| m.id.account == f.account && m.id.folder == f.id)
                        .count();
                    (
                        String::new(),
                        format!(
                            "{indent}… {} ({loaded}/{}) · Enter",
                            if loaded == 0 {
                                "Load messages"
                            } else {
                                "Load more"
                            },
                            f.total
                        ),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        Style::default().fg(p.hot),
                    )
                }
                Node::Status(i) => {
                    let state = app.pages.get(&tree::folder_key(&app.folders[i]));
                    let label = state
                        .and_then(|p| p.error.as_deref())
                        .map(|e| format!("Error: {} · Enter retries", text(e)))
                        .unwrap_or_else(|| "Loading local mail…".into());
                    (
                        String::new(),
                        format!("{indent}{label}"),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        Style::default().fg(p.hot),
                    )
                }
            };
            let mut cells = vec![name];
            if sender {
                cells.push(from);
            }
            if date {
                cells.push(when);
            }
            if counts {
                cells.push(count);
            }
            if size {
                cells.push(bytes);
            }
            Row::new(cells).style(style)
        })
        .collect();
    let mut headers = vec!["NAME / SUBJECT"];
    let mut widths = vec![Constraint::Min(10)];
    if sender {
        headers.push("FROM");
        widths.push(Constraint::Percentage(25));
    }
    if date {
        headers.push("DATE");
        widths.push(Constraint::Length(16));
    }
    if counts {
        headers.push("NEW/TOTAL");
        widths.push(Constraint::Length(9));
    }
    if size {
        headers.push("SIZE");
        widths.push(Constraint::Length(6));
    }
    for label in &mut headers {
        *label = match (app.sort_name(), *label, app.sort_descending()) {
            ("subject", "NAME / SUBJECT", true) => "NAME / SUBJECT↓",
            ("subject", "NAME / SUBJECT", false) => "NAME / SUBJECT↑",
            ("date", "DATE", true) => "DATE↓",
            ("date", "DATE", false) => "DATE↑",
            ("from", "FROM", true) => "FROM↓",
            ("from", "FROM", false) => "FROM↑",
            _ => *label,
        };
    }
    let shown = tree
        .iter()
        .filter(|r| matches!(r.node, Node::Message(_)))
        .count();
    let folders = app
        .folders
        .iter()
        .filter(|f| app.scope.account.as_ref().is_none_or(|a| a == &f.account))
        .count();
    let title = format!(
        " Messages({})[{} mail / {} folders]{} ",
        text(app.account_name()),
        shown,
        folders,
        if app.searching() {
            format!(" /{}", text(&app.search))
        } else {
            String::new()
        }
    );
    table(frame, app, area, p, &title, headers, rows, widths);
}

#[allow(clippy::too_many_arguments)]
fn table(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    p: &Palette,
    title: &str,
    header: Vec<&str>,
    rows: Vec<Row<'static>>,
    widths: Vec<Constraint>,
) {
    let count = rows.len();
    let heading = if app.theme == "no-color" {
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(p.accent)
            .add_modifier(Modifier::BOLD)
    };
    let widget = Table::new(rows, widths)
        .header(Row::new(header).style(heading))
        .block(block(title.to_string(), p))
        .row_highlight_style(
            if app.view == View::Messages
                && app.focused().is_some_and(|m| app.selected.contains(&m.id))
            {
                if app.theme == "no-color" {
                    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Black)
                        .bg(p.hot)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                }
            } else {
                p.selected
            },
        )
        .highlight_symbol("")
        .column_spacing(1);
    let mut state = TableState::default()
        .with_offset(app.table_offset)
        .with_selected(if count == 0 {
            None
        } else {
            Some(app.row.min(count - 1))
        });
    frame.render_stateful_widget(widget, area, &mut state);
    app.table_offset = state.offset();
}
fn assistant(frame: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    use unicode_segmentation::UnicodeSegmentation;
    let popup = centered(area, 80, 80);
    frame.render_widget(Clear, popup);
    let border = block(format!(" Assistant · {} ", text(&app.llm.label())), p).style(p.base);
    let inner = border.inner(popup);
    frame.render_widget(border, popup);
    let compact = inner.height < 14 || inner.width < 65;
    let parts = Layout::vertical([
        Constraint::Length(if compact { 1 } else { 2 }),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(if compact { 1 } else { 2 }),
    ])
    .split(inner);
    let anchor = app
        .ai
        .anchor
        .as_ref()
        .map(|m| text(&m.subject))
        .unwrap_or_else(|| "Global mail conversation".into());
    frame.render_widget(
        Paragraph::new(if compact {
            format!(
                "Scope: {} · {anchor}",
                text(app.ai.account.as_deref().unwrap_or("all accounts"))
            )
        } else {
            format!(
                "Context: {anchor}\nSearch scope: {}",
                text(app.ai.account.as_deref().unwrap_or("all accounts"))
            )
        })
        .style(Style::default().fg(p.accent)),
        parts[0],
    );
    let content = app.ai.text();
    let width = parts[1].width.max(1) as usize;
    let mut lines = vec![];
    for line in content.lines() {
        let mut current = String::new();
        let mut used = 0;
        for g in line.graphemes(true) {
            let n = UnicodeWidthStr::width(g);
            if used + n > width && !current.is_empty() {
                lines.push(Line::from(std::mem::take(&mut current)));
                used = 0;
            }
            current.push_str(g);
            used += n;
        }
        lines.push(Line::from(current));
    }
    let max_scroll = lines.len().saturating_sub(parts[1].height as usize);
    app.ai.scroll = app.ai.scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(lines).scroll((
            max_scroll
                .saturating_sub(app.ai.scroll)
                .min(u16::MAX as usize) as u16,
            0,
        )),
        parts[1],
    );
    app.page_height = parts[1].height.max(1) as usize;
    frame.render_widget(
        Paragraph::new(format!(
            "{}{}",
            if app.ai.busy { "● " } else { "" },
            text(&app.ai.status),
        ))
        .style(Style::default().fg(p.hot)),
        parts[2],
    );
    edit_field(frame, parts[3], "Ask:", &app.ai.input, true, p);
    let help = if compact {
        "Enter send · Esc close · /stop"
    } else {
        "Enter send · Esc close/resume · PgUp/PgDn scroll · /stop interrupt\n/models · /open N source · /apply N proposal · /new /global /status"
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(p.muted)),
        parts[4],
    );
}

fn edit_field(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    input: &crate::input::Input,
    active: bool,
    p: &Palette,
) {
    let columns = Layout::horizontal([Constraint::Length(9), Constraint::Min(1)]).split(area);
    frame.render_widget(
        Paragraph::new(label).style(if active { p.selected } else { p.base }),
        columns[0],
    );
    let before = &input.text[..input.cursor];
    let line = before.chars().filter(|c| *c == '\n').count();
    let column = UnicodeWidthStr::width(before.rsplit('\n').next().unwrap_or(""));
    let y = line
        .saturating_sub(columns[1].height.saturating_sub(1) as usize)
        .min(u16::MAX as usize);
    let x = column
        .saturating_sub(columns[1].width.saturating_sub(1) as usize)
        .min(u16::MAX as usize);
    frame.render_widget(
        Paragraph::new(terminal_text(&input.text)).scroll((y as u16, x as u16)),
        columns[1],
    );
    if active && columns[1].width > 0 && columns[1].height > 0 {
        frame.set_cursor_position((
            columns[1].x
                + column
                    .saturating_sub(x)
                    .min(columns[1].width.saturating_sub(1) as usize) as u16,
            columns[1].y
                + line
                    .saturating_sub(y)
                    .min(columns[1].height.saturating_sub(1) as usize) as u16,
        ));
    }
}

fn form_dialog(frame: &mut Frame, app: &App, area: Rect, p: &Palette, overlay: &Overlay) {
    let popup = centered(area, 90, 85);
    frame.render_widget(Clear, popup);
    let title = match overlay {
        Overlay::Attachments { .. } => " 📎 Attachments · select files to save ",
        Overlay::SaveAttachments { .. } => " Save attachments ",
        Overlay::Find { .. } => " Search · MailTUI local index ",
        Overlay::Move { .. } => " Move mail · fuzzy directory filter ",
        Overlay::CreateFolder { .. } => " Create directory ",
        Overlay::Files { .. } => " Attach files · fuzzy filter · multiple selection ",
        Overlay::PimForm {
            contact, original, ..
        } => {
            if *contact {
                if original.is_some() {
                    " Edit contact · Tab field · Enter save "
                } else {
                    " New contact · Tab field · Enter save "
                }
            } else if original.is_some() {
                " Edit event · Tab field · Enter save "
            } else {
                " New event · Tab field · Enter save "
            }
        }
        _ => " Compose email ",
    };
    let border = block(title, p).style(p.base);
    let inner = border.inner(popup);
    frame.render_widget(border, popup);
    match overlay {
        Overlay::Attachments {
            query,
            row,
            selected,
        } => {
            let parts = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(inner);
            edit_field(frame, parts[0], "Filter:", query, true, p);
            let attachments = app
                .parsed
                .as_ref()
                .map(|parsed| parsed.attachments.as_slice())
                .unwrap_or_default();
            let matches = crate::app::attachment_matches(attachments, &query.text);
            frame.render_widget(
                Paragraph::new(format!(
                    "{} files · {} selected",
                    attachments.len(),
                    selected.len()
                ))
                .style(Style::default().fg(p.muted)),
                parts[1],
            );
            let rows: Vec<_> = matches
                .iter()
                .map(|index| {
                    let attachment = &attachments[*index];
                    Row::new([
                        format!(
                            "{} {} {}",
                            if selected.contains(index) { "✓" } else { " " },
                            crate::files::icon(&attachment.content_type),
                            text(&attachment.name)
                        ),
                        format!("{} KiB", attachment.bytes.len().div_ceil(1024)),
                    ])
                })
                .collect();
            let mut state = TableState::default().with_selected(
                (!rows.is_empty()).then_some((*row).min(rows.len().saturating_sub(1))),
            );
            frame.render_stateful_widget(
                Table::new(rows, [Constraint::Min(1), Constraint::Length(12)])
                    .row_highlight_style(p.selected),
                parts[2],
                &mut state,
            );
            frame.render_widget(Paragraph::new("Space/Tab mark · Ctrl+A all · Enter save\nEsc close · Enter without marks saves the pointed file").style(Style::default().fg(p.muted)), parts[3]);
        }
        Overlay::SaveAttachments {
            picker,
            directory,
            error,
        } => {
            let parts = Layout::vertical([
                Constraint::Length(2),
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(inner);
            let count = if let Overlay::Attachments { selected, .. } = picker.as_ref() {
                selected.len()
            } else {
                0
            };
            frame.render_widget(
                Paragraph::new(format!("Save {count} attachment(s) to this directory:"))
                    .style(p.base),
                parts[0],
            );
            edit_field(frame, parts[1], "Folder:", directory, !app.form_pending, p);
            if let Some(error) = error {
                frame.render_widget(
                    Paragraph::new(text(error)).style(Style::default().fg(p.hot)),
                    parts[2],
                );
            }
            frame.render_widget(Paragraph::new(if app.form_pending { "Saving attachments…" } else { "Enter save · Esc back to selection\nExisting files keep their names; duplicates receive a numbered suffix." }).style(Style::default().fg(p.muted)), parts[3]);
        }
        Overlay::Find {
            query,
            messages,
            row,
            loading,
            error,
            account,
            ..
        } => {
            let parts = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);
            edit_field(frame, parts[0], "Search:", query, true, p);
            frame.render_widget(
                Paragraph::new(format!(
                    "Local text + fuzzy metadata · {} · {} results",
                    text(account.as_deref().unwrap_or("all")),
                    messages.len()
                ))
                .style(Style::default().fg(p.muted)),
                parts[1],
            );
            let rows: Vec<_> = messages
                .iter()
                .map(|message| {
                    Row::new([
                        text(&message.subject),
                        text(&message.sender),
                        text(&format!("{} / {}", message.id.account, message.id.folder)),
                    ])
                })
                .collect();
            let mut state = TableState::default().with_selected(
                (!rows.is_empty()).then_some((*row).min(rows.len().saturating_sub(1))),
            );
            frame.render_stateful_widget(
                Table::new(
                    rows,
                    [
                        Constraint::Percentage(45),
                        Constraint::Percentage(25),
                        Constraint::Min(1),
                    ],
                )
                .row_highlight_style(p.selected),
                parts[2],
                &mut state,
            );
            let status = if let Some(error) = error {
                text(error)
            } else if app.index_busy {
                "Updating local index… · ↑/↓ choose · Enter read · Esc close".into()
            } else if *loading {
                "Searching local index…".into()
            } else {
                "↑/↓ choose · Enter read · Esc close · live local index".into()
            };
            frame.render_widget(
                Paragraph::new(status).style(Style::default().fg(p.hot)),
                parts[3],
            );
        }
        Overlay::Files {
            path,
            entries,
            query,
            row,
            selected,
            loading,
            error,
            ..
        } => {
            let parts = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(inner);
            frame.render_widget(Paragraph::new(text(&path.to_string_lossy())), parts[0]);
            edit_field(frame, parts[1], "Filter:", query, true, p);
            let matches = crate::app::file_matches(entries, &query.text);
            let rows: Vec<_> = matches
                .iter()
                .map(|index| {
                    let entry = &entries[*index];
                    Row::new([
                        format!(
                            "{} {} {}{}",
                            if selected.contains(&entry.path) {
                                "✓"
                            } else {
                                " "
                            },
                            if entry.directory {
                                "📁"
                            } else {
                                crate::files::icon(crate::files::content_type(&entry.path))
                            },
                            text(&entry.name),
                            if entry.directory { "/" } else { "" }
                        ),
                        if entry.directory {
                            "directory".into()
                        } else {
                            format!("{} KiB", entry.size.div_ceil(1024))
                        },
                    ])
                })
                .collect();
            let mut state = TableState::default().with_selected(
                (!rows.is_empty()).then_some((*row).min(rows.len().saturating_sub(1))),
            );
            frame.render_stateful_widget(
                Table::new(rows, [Constraint::Min(1), Constraint::Length(12)])
                    .row_highlight_style(p.selected),
                parts[2],
                &mut state,
            );
            let status = if *loading {
                "Loading…".into()
            } else if let Some(error) = error {
                text(error)
            } else {
                format!(
                    "{} selected · Tab mark · Enter open/attach · Backspace parent",
                    selected.len()
                )
            };
            frame.render_widget(
                Paragraph::new(format!(
                    "{status}\nCtrl+S attach · Ctrl+U clear marks · Esc cancel"
                ))
                .style(Style::default().fg(p.hot)),
                parts[3],
            );
        }
        Overlay::PimForm {
            contact,
            collection,
            original,
            fields,
            field,
            error,
            ..
        } => {
            let labels: &[&str] = if *contact {
                &["Name", "Email", "Phone", "Organization"]
            } else {
                &[
                    "Summary",
                    "Start (ICS date/time)",
                    "End (optional)",
                    "Location",
                    "Description",
                    "Repeat rule (RRULE)",
                ]
            };
            let parts = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(u16::from(error.is_some())),
                Constraint::Length(if inner.height >= 8 { 2 } else { 1 }),
            ])
            .split(inner);
            let height = parts[0].height as usize;
            let start = field.saturating_sub(height.saturating_sub(1));
            for position in start..(start + height).min(fields.len() + 1) {
                let rect = Rect::new(
                    parts[0].x,
                    parts[0].y + (position - start) as u16,
                    parts[0].width,
                    1,
                );
                if position == 0 {
                    let groups = if *contact {
                        &app.contact_books
                    } else {
                        &app.calendar_collections
                    };
                    let label = groups
                        .get(*collection)
                        .map(|c| c.label.as_str())
                        .unwrap_or("unavailable");
                    frame.render_widget(
                        Paragraph::new(format!(
                            "{}: {} {}",
                            if *contact { "Address book" } else { "Calendar" },
                            text(label),
                            if original.is_some() {
                                "(existing entry)"
                            } else {
                                "←/→ choose"
                            }
                        ))
                        .style(if *field == 0 {
                            p.selected
                        } else {
                            p.base
                        }),
                        rect,
                    );
                } else {
                    edit_field(
                        frame,
                        rect,
                        &format!("{}:", labels[position - 1]),
                        &fields[position - 1],
                        position == *field && !app.pim_pending,
                        p,
                    );
                }
            }
            if let Some(error) = error {
                frame.render_widget(
                    Paragraph::new(text(error)).style(Style::default().fg(p.hot)),
                    parts[1],
                );
            }
            frame.render_widget(
                Paragraph::new(if app.pim_pending{"Saving locally…"}else if *contact{"Tab fields · Ctrl+S save · Esc cancel\n←/→ choose address book in first field"}else{"Tab fields · Ctrl+S save · Esc cancel\nDates: YYYYMMDD or YYYYMMDDTHHMMSS[Z] · all-day end exclusive"})
                    .style(Style::default().fg(p.muted)),
                parts[2],
            );
        }
        Overlay::Move { ids, query, row } => {
            let parts = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);
            edit_field(frame, parts[0], "Filter:", query, true, p);
            let destinations = app.move_destinations(ids, &query.text);
            let folders: std::collections::HashMap<_, _> = app
                .folders
                .iter()
                .map(|f| ((f.account.as_str(), f.id.as_str()), f))
                .collect();
            let rows: Vec<_> = destinations
                .iter()
                .map(|(account, folder)| {
                    let label = folders
                        .get(&(account.as_str(), folder.as_str()))
                        .filter(|f| !folder.ends_with(f.name.as_str()))
                        .map(|f| format!("{} [{folder}]", f.name))
                        .unwrap_or_else(|| folder.clone());
                    Row::new([text(&format!("{account} / {label}"))])
                })
                .collect();
            let mut state = TableState::default().with_selected(
                (!rows.is_empty()).then_some((*row).min(rows.len().saturating_sub(1))),
            );
            frame.render_stateful_widget(
                Table::new(rows, [Constraint::Min(1)]).row_highlight_style(p.selected),
                parts[1],
                &mut state,
            );
            frame.render_widget(
                Paragraph::new(if destinations.is_empty() {
                    "No matches · Esc cancel"
                } else {
                    "↑/↓ or Tab · Enter move · Esc cancel"
                })
                .style(Style::default().fg(p.hot)),
                parts[2],
            );
        }
        Overlay::CreateFolder {
            account,
            anchor,
            parent,
            name,
        } => {
            let parts = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);
            frame.render_widget(
                Paragraph::new(format!("Account: {}", text(account))),
                parts[0],
            );
            frame.render_widget(
                Paragraph::new(format!(
                    "Parent: {} (Tab changes)",
                    if *parent { text(anchor) } else { "/".into() }
                )),
                parts[1],
            );
            edit_field(frame, parts[2], "Name:", name, !app.form_pending, p);
            frame.render_widget(
                Paragraph::new(if app.form_pending {
                    "Queuing…"
                } else {
                    "Enter create · Esc cancel"
                }),
                parts[3],
            );
        }
        Overlay::Compose {
            account,
            field,
            fields,
            attachments,
        } => {
            let parts = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
                Constraint::Length(2),
            ])
            .split(inner);
            let sender = &app.accounts[*account];
            frame.render_widget(
                Paragraph::new(format!(
                    "Attachments ({}): {}\nCtrl+T browse / change attachments",
                    attachments.len(),
                    attachments
                        .iter()
                        .filter_map(|path| path.file_name())
                        .map(|name| text(&name.to_string_lossy()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .style(Style::default().fg(p.accent)),
                parts[4],
            );
            frame.render_widget(
                Paragraph::new(format!(
                    "From: {} <{}> (↑/↓ changes account)",
                    text(&sender.id),
                    text(&sender.email)
                ))
                .style(if *field == 0 { p.selected } else { p.base }),
                parts[0],
            );
            for (index, label) in ["To:", "Subject:", "Body:"].into_iter().enumerate() {
                edit_field(
                    frame,
                    parts[index + 1],
                    label,
                    &fields[index],
                    *field == index + 1 && !app.form_pending,
                    p,
                );
            }
            frame.render_widget(
                Paragraph::new(if app.form_pending {
                    "Queuing…"
                } else {
                    "Tab / Shift+Tab fields · Ctrl+S send\nCtrl+O save draft · Esc cancel"
                })
                .style(Style::default().fg(p.hot)),
                parts[5],
            );
        }
        _ => {}
    }
}

fn paragraph(frame: &mut Frame, app: &App, area: Rect, p: &Palette, body: String) {
    frame.render_widget(
        Paragraph::new(terminal_text(&body))
            .block(block(format!(" {} ", app.view.name()), p))
            .wrap(Wrap { trim: false })
            .scroll((app.body_scroll.min(u16::MAX as usize) as u16, 0)),
        area,
    );
}
fn delete_dialog(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    p: &Palette,
    ids: &[crate::domain::MessageId],
    confirm: bool,
) {
    let mut scopes = std::collections::BTreeMap::new();
    for id in ids {
        *scopes.entry((&id.account, &id.folder)).or_insert(0usize) += 1;
    }
    let mut popup = centered(area, 78, 85);
    popup.height = popup.height.min(13);
    popup.y = area.y + area.height.saturating_sub(popup.height) / 2;
    frame.render_widget(Clear, popup);
    let border = block(" Delete messages? ", p).style(p.base);
    let inner = border.inner(popup);
    frame.render_widget(border, popup);
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(format!("Move {} marked message(s) to Trash?", ids.len()))
            .style(Style::default().fg(p.hot))
            .wrap(Wrap { trim: false }),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(
            scopes
                .into_iter()
                .map(|((a, f), n)| format!("{n:>4}  {} / {}", text(a), text(f)))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .scroll((app.help_scroll.min(u16::MAX as usize) as u16, 0)),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(if app.demo {
            "DEMO · local memory only"
        } else {
            "Move selected messages to Trash"
        })
        .style(Style::default().fg(p.muted)),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" [ Cancel ] ", if confirm { p.base } else { p.selected }),
            Span::raw("   "),
            Span::styled(" [ Delete ] ", if confirm { p.selected } else { p.base }),
        ]))
        .alignment(Alignment::Center),
        rows[3],
    );
    frame.render_widget(
        Paragraph::new("Tab: choose  Enter: accept  Esc: cancel")
            .style(Style::default().fg(p.accent)),
        rows[4],
    );
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = (area.width as u32 * width as u32 / 100)
        .max(1)
        .min(area.width as u32) as u16;
    let h = (area.height as u32 * height as u32 / 100)
        .max(1)
        .min(area.height as u32) as u16;
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}
pub fn help(filter: &str) -> String {
    let mut s = String::from("MailTUI · one Messages tree\n\n");
    for b in bindings().iter().filter(|b| {
        format!("{} {}", b.key, b.description)
            .to_lowercase()
            .contains(&filter.to_lowercase())
    }) {
        s.push_str(&format!("{:<14} {}\n", b.key, b.description));
    }
    s.push_str("\nLocal search\n/ opens the local search picker. Type words or fuzzy metadata.\nSearch sender, recipients, subject and downloaded body.\nUse arrows to choose a result, Enter to read it, Esc to return.\n\nNavigation\nArrows move; Enter folds/unfolds a branch or reads a message.\nh/H opens HTML without/with remote images; the page button toggles both.\nSpace marks a message; a selects visible messages. Ctrl+D confirms Trash.\nm moves mail, c composes, n creates a directory.\nThe composer and file picker show their own editing controls.\nCtrl+C twice within three seconds quits.\n\nCommands\n:ns / :accounts — selector; :ns <id|all> switches directly\n:dir <path> — open/focus a directory; :inbox / :unified — open inboxes\n:search <query> — local results in the tree; Esc returns\n:compose, :mkdir, :more, :reload, :outbox, :status, :help, :quit\n:unread, :flagged, :sort <field> [asc|desc], :theme <name>\n\nAutomatic updates\nMail, folder counts and search results update automatically.\nForms defer list updates until closed.\nPending shows outgoing mail until sending is confirmed; :outbox shows details.\n");
    s
}

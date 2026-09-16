//! Ratatui dashboard: header + Servers | Channels | Messages+Composer | Members.
//! Concord layout parity (pane widths, selection marker, unread/mention badges).

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::{app::{App, Focus, Popup}, markdown::render_markdown};

pub fn draw(f: &mut Frame, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());

    draw_header(f, app, root[0]);
    draw_main(f, app, root[1]);
    draw_status(f, app, root[2]);

    match app.popup {
        Popup::None | Popup::ConfirmDelete | Popup::ConfirmLogout => {}
        _ => draw_popup(f, app),
    }
    if app.popup == Popup::ConfirmDelete || app.popup == Popup::ConfirmLogout {
        draw_confirm(f, app);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let server = app.selected_server().map(|s| s.name.as_str()).unwrap_or("—");
    let channel = app.selected_channel().map(|c| c.name.as_str()).unwrap_or("—");
    let topic = app.selected_channel().and_then(|c| c.topic.clone()).unwrap_or_default();
    let title = format!(" tuimessager · {server} · #{channel}  {topic}");
    let p = Paragraph::new(Line::from(vec![
        Span::styled(title, Style::new().fg(Color::Cyan).bold()),
    ]));
    f.render_widget(p, area);
}

fn draw_main(f: &mut Frame, app: &App, area: Rect) {
    let mut constraints = vec![];
    if app.show_servers {
        constraints.push(Constraint::Length(20));
    }
    if app.show_channels {
        constraints.push(Constraint::Length(24));
    }
    constraints.push(Constraint::Min(20));
    if app.show_members {
        constraints.push(Constraint::Length(22));
    }
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);
    let mut i = 0;
    if app.show_servers {
        draw_servers(f, app, panes[i]);
        i += 1;
    }
    if app.show_channels {
        draw_channels(f, app, panes[i]);
        i += 1;
    }
    draw_messages(f, app, panes[i]);
    i += 1;
    if app.show_members {
        draw_members(f, app, panes[i]);
        let _ = i;
    }
}

fn pane_block(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(if focused {
            Style::new().fg(Color::Cyan).bold()
        } else {
            Style::new().fg(Color::DarkGray)
        })
        .title(format!(" {title} "))
        .border_type(ratatui::widgets::BorderType::Rounded)
}

fn draw_servers(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .servers
        .iter()
        .enumerate()
        .map(|(idx, s)| {
            let marker = if idx == app.server_idx { "▸ " } else { "  " };
            let mut spans = vec![Span::styled(marker, Style::new().fg(Color::Cyan).bold())];
            spans.push(Span::raw(s.name.clone()));
            if s.mentioned {
                spans.push(Span::styled(" [@]", Style::new().fg(Color::Rgb(255, 165, 0))));
            } else if s.unread {
                spans.push(Span::styled(" •", Style::new().fg(Color::Green).bold()));
            }
            let mut item = ListItem::new(Line::from(spans));
            if idx == app.server_idx && app.focus == Focus::Servers {
                item = item.style(Style::new().bg(Color::DarkGray));
            }
            item
        })
        .collect();
    f.render_widget(List::new(items).block(pane_block("Servers", app.focus == Focus::Servers)), area);
}

fn draw_channels(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .channels
        .iter()
        .enumerate()
        .map(|(idx, c)| {
            let marker = if idx == app.channel_idx { "▸ " } else { "  " };
            let icon = match c.kind {
                tuimessager_protocol::ChannelKind::Thread => "🧵",
                tuimessager_protocol::ChannelKind::ForumPost => "📝",
                tuimessager_protocol::ChannelKind::VoicePlaceholder => "🔊",
                tuimessager_protocol::ChannelKind::Direct => "✉",
                tuimessager_protocol::ChannelKind::Announcement => "📢",
                tuimessager_protocol::ChannelKind::Text => "#",
            };
            let mut spans = vec![
                Span::styled(marker, Style::new().fg(Color::Cyan).bold()),
                Span::styled(format!("{icon} "), Style::new().fg(Color::DarkGray)),
                Span::raw(c.name.clone()),
            ];
            if c.mentioned {
                spans.push(Span::styled(" [@]", Style::new().fg(Color::Rgb(255, 165, 0))));
            } else if c.unread_count > 0 {
                spans.push(Span::styled(format!(" ({})", c.unread_count), Style::new().fg(Color::Green)));
            }
            let mut item = ListItem::new(Line::from(spans));
            if idx == app.channel_idx && app.focus == Focus::Channels {
                item = item.style(Style::new().bg(Color::DarkGray));
            }
            item
        })
        .collect();
    f.render_widget(List::new(items).block(pane_block("Channels", app.focus == Focus::Channels)), area);
}

fn draw_members(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .members
        .iter()
        .enumerate()
        .map(|(idx, m)| {
            let dot = match m.presence {
                tuimessager_protocol::Presence::Online => Span::styled("● ", Style::new().fg(Color::Green)),
                tuimessager_protocol::Presence::Idle => Span::styled("● ", Style::new().fg(Color::Yellow)),
                tuimessager_protocol::Presence::Dnd => Span::styled("● ", Style::new().fg(Color::Red)),
                tuimessager_protocol::Presence::Offline => Span::styled("○ ", Style::new().dim()),
            };
            let marker = if idx == app.member_idx { "▸ " } else { "  " };
            let mut item = ListItem::new(Line::from(vec![
                Span::styled(marker, Style::new().fg(Color::Cyan).bold()),
                dot,
                Span::raw(m.user.display().to_string()),
            ]));
            if idx == app.member_idx && app.focus == Focus::Members {
                item = item.style(Style::new().bg(Color::DarkGray));
            }
            item
        })
        .collect();
    f.render_widget(List::new(items).block(pane_block("Members", app.focus == Focus::Members)), area);
}

fn draw_messages(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(composer_height(app))])
        .split(area);

    // Message list (newest at bottom; scroll window anchored to msg_idx).
    let mut lines: Vec<Line> = Vec::new();
    let start = app.msg_scroll;
    for (idx, m) in app.messages.iter().enumerate().skip(start) {
        let selected = idx == app.msg_idx && app.focus == Focus::Messages;
        let marker = if selected { "▸ " } else { "  " };
        let time = app.format_time(&m.created_at);
        let edited = if m.edited_at.is_some() { " (edited)" } else { "" };
        let pinned = if m.pinned { " 📌" } else { "" };
        let reply = if m.reply_to.is_some() { " ↩" } else { "" };
        let header = Line::from(vec![
            Span::styled(marker, Style::new().fg(Color::Cyan).bold()),
            Span::styled(m.author.display().to_string(), Style::new().bold()),
            Span::styled(format!(" {time}{edited}{pinned}{reply}"), Style::new().fg(Color::DarkGray)),
        ]);
        lines.push(header);
        for body in render_markdown(&m.content) {
            let mut indented = vec![Span::raw("    ")];
            indented.extend(body.spans);
            lines.push(Line::from(indented));
        }
        if !m.reactions.is_empty() {
            let chips: Vec<Span> = m.reactions.iter().flat_map(|r| {
                let style = if r.me {
                    Style::new().fg(Color::Yellow)
                } else {
                    Style::new().fg(Color::Cyan)
                };
                vec![
                    Span::styled(format!(" {} {} ", r.emoji, r.count), style),
                ]
            }).collect();
            let mut row = vec![Span::raw("    ")];
            row.extend(chips);
            lines.push(Line::from(row));
        }
        if app.options.display.show_images {
            // v1: attachments render as link rows (Concord image protocols roadmap).
            for a in &m.attachments {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(format!("🖼 {} ({})", a.filename, a.url), Style::new().fg(Color::Cyan).underlined()),
                ]));
            }
        }
        lines.push(Line::from(""));
    }
    if lines.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  No messages yet. Press i to compose.",
            Style::new().fg(Color::DarkGray).italic(),
        )]));
    }
    let list = Paragraph::new(lines)
        .block(pane_block("Messages", app.focus == Focus::Messages))
        .wrap(Wrap { trim: false })
        .scroll((0, 0));
    f.render_widget(list, chunks[0]);

    // Composer
    let title = if let Some(id) = app.editing {
        let _ = id;
        "Edit (Enter save, Esc cancel)"
    } else if let Some(r) = app.reply_to {
        let _ = r;
        "Reply (Enter send, Esc cancel)"
    } else if app.composer_open {
        "Composer (Enter send, Esc cancel)"
    } else {
        "Composer (i to write)"
    };
    let body = if app.composer_open || !app.composer.is_empty() {
        format!("{}▊", app.composer)
    } else {
        String::new()
    };
    let composer = Paragraph::new(body)
        .block(pane_block(title, app.composer_open))
        .wrap(Wrap { trim: false });
    f.render_widget(composer, chunks[1]);
}

fn composer_height(app: &App) -> u16 {
    if app.composer_open {
        5.min(3 + (app.composer.lines().count() as u16)) + 2
    } else {
        3
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let unread = app.notifications.iter().filter(|n| !n.read).count();
    let inbox = if unread > 0 { format!(" · inbox({unread})") } else { String::new() };
    let text = if app.status.is_empty() {
        format!(" 1-4 focus · Tab cycle · / search · Space shortcuts · i compose · q quit{inbox}")
    } else {
        format!(" {}", app.status)
    };
    f.render_widget(Paragraph::new(Line::from(Span::styled(text, Style::new().fg(Color::DarkGray)))), area);
}

fn draw_popup(f: &mut Frame, app: &App) {
    let area = centered(f.area(), 70, 60);
    f.render_widget(Clear, area);
    let (title, items): (&str, Vec<Line>) = match app.popup {
        Popup::Help => ("Shortcuts (Space)", help_lines()),
        Popup::ChannelSwitcher => {
            let rows = app
                .switcher_items()
                .into_iter()
                .enumerate()
                .map(|(vis, (_, ci))| {
                    let c = &app.channels[ci];
                    let marker = if vis == app.popup_idx { "▸ " } else { "  " };
                    Line::from(vec![
                        Span::styled(marker, Style::new().fg(Color::Cyan).bold()),
                        Span::raw(format!("#{}  {}", c.name, c.topic.clone().unwrap_or_default())),
                    ])
                })
                .collect();
            ("Channel switcher (Space Space)", rows)
        }
        Popup::Search => ("Search (/)", vec![Line::from(format!("Query: {}", app.popup_input))]),
        Popup::Inbox => {
            let rows = app
                .notifications
                .iter()
                .take(20)
                .enumerate()
                .map(|(i, n)| {
                    let marker = if i == app.popup_idx { "▸ " } else { "  " };
                    Line::from(vec![
                        Span::styled(marker, Style::new().fg(Color::Cyan).bold()),
                        Span::styled(format!("{}: ", n.message.author.display()), Style::new().bold()),
                        Span::raw(n.message.content.chars().take(80).collect::<String>()),
                    ])
                })
                .collect();
            ("Inbox (Space n)", rows)
        }
        Popup::Emoji => {
            let emojis = ["🔥", "👍", "❤️", "😂", "🎉", "😮", "😢", "🙏", "👀", "💯"];
            let rows = emojis
                .iter()
                .enumerate()
                .filter(|(_, e)| e.contains(app.popup_input.trim()) || app.popup_input.trim().is_empty())
                .enumerate()
                .map(|(vis, (_, e))| {
                    let marker = if vis == app.popup_idx { "▸ " } else { "  " };
                    Line::from(vec![
                        Span::styled(marker, Style::new().fg(Color::Cyan).bold()),
                        Span::raw(e.to_string()),
                    ])
                })
                .collect();
            ("Emoji (:)", rows)
        }
        Popup::MessageActions => (
            "Message actions (Space a)",
            vec![
                Line::from("y copy · r react · R reply · e edit · d delete · o open URL · P pin"),
                Line::from("Voice: placeholder in v1 (no audio yet) · threads via #channels"),
            ],
        ),
        Popup::None | Popup::ConfirmDelete | Popup::ConfirmLogout => ("", vec![]),
    };
    let mut text = vec![Line::from(Span::styled(
        format!("Filter: {}", app.popup_input),
        Style::new().fg(Color::DarkGray),
    ))];
    text.extend(items);
    let w = Paragraph::new(text).block(pane_block(title, true)).wrap(Wrap { trim: false });
    f.render_widget(w, area);
}

fn draw_confirm(f: &mut Frame, app: &App) {
    let area = centered(f.area(), 50, 20);
    f.render_widget(Clear, area);
    let (title, lines): (&str, Vec<Line>) = if app.popup == Popup::ConfirmLogout {
        (
            "Log out",
            vec![
                Line::from(format!("Logged in as {} ({})", app.me.display(), app.me.name)),
                Line::from("Invalidate this session and quit?"),
                Line::from("y confirm · Esc cancel"),
            ],
        )
    } else {
        let msg = app.selected_message().map(|m| m.content.chars().take(60).collect::<String>()).unwrap_or_default();
        (
            "Confirm",
            vec![
                Line::from("Delete this message?"),
                Line::from(Span::styled(msg, Style::new().fg(Color::DarkGray))),
                Line::from("y confirm · Esc cancel"),
            ],
        )
    };
    let w = Paragraph::new(lines)
        .block(pane_block(title, true).style(Style::new().fg(Color::Red)));
    f.render_widget(w, area);
}

fn help_lines() -> Vec<Line<'static>> {
    vec![
        Line::from("1/2/3/4 focus · Tab/S-Tab cycle · j/k move · gg/G top/bottom"),
        Line::from("Ctrl-d/u half-page · i compose · Enter send · Esc close"),
        Line::from("/ search · y copy · r react · R reply · e edit · d delete"),
        Line::from("o open URL · Space Space switcher · Space a actions"),
        Line::from("Space n inbox · Space o options · Space p profile · Space r redraw"),
        Line::from("Space l log out · Space v voice placeholder"),
        Line::from("Voice channels are UI placeholders in v1 (self-hosted audio roadmap)"),
    ]
}

fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(h[1])[1]
}

pub fn styled_modifier() -> Modifier {
    Modifier::BOLD
}

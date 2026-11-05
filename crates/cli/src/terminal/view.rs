use super::state::{Overlay, Ui, text};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

pub(super) fn draw(frame: &mut Frame<'_>, ui: &mut Ui) {
    let area = frame.area();
    if area.width < 32 || area.height < 10 {
        frame.render_widget(
            Paragraph::new("The terminal needs 32 columns and 10 rows."),
            area,
        );
        return;
    }
    let slots = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(5),
            Constraint::Length(2),
        ])
        .split(area);
    let session = ui
        .preferences
        .session
        .as_deref()
        .unwrap_or("No active conversation");
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                text(&ui.title),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::from(text(session)),
        ]),
        slots[0],
    );
    let body = if ui.reply.is_empty() {
        ui.transcript.clone()
    } else {
        format!("{}\n\nReply\n{}", ui.transcript, ui.reply)
    };
    let body = if body.is_empty() {
        "RustyTavern\n\nUse /setup to configure your model.\nUse /character and /persona to select identities.\nUse /new to start a conversation.\n\nType / and press Tab to open commands.".into()
    } else {
        body
    };
    let paragraph = Paragraph::new(body).wrap(Wrap { trim: false });
    let total = paragraph.line_count(slots[1].width).min(u16::MAX as usize) as u16;
    let offset = total
        .saturating_sub(slots[1].height)
        .saturating_sub(ui.scroll);
    frame.render_widget(paragraph.scroll((offset, 0)), slots[1]);
    ui.composer.set_block(
        Block::default()
            .borders(Borders::ALL)
            .title(if ui.busy {
                " Message · the operation is active "
            } else {
                " Message · /help "
            })
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    ui.composer.set_cursor_line_style(Style::default());
    frame.render_widget(&ui.composer, slots[2]);
    let progress = if ui.busy {
        ["·", "•", "●", "•"][ui.tick % 4]
    } else {
        ""
    };
    frame.render_widget(Paragraph::new(vec![Line::from(text(&format!("{progress} {}",ui.notice))),Line::styled("Enter sends · Alt+Enter adds a line · Tab opens commands · Ctrl+C cancels · Ctrl+D exits",Style::default().fg(Color::DarkGray))]),slots[3]);
    let Some(overlay) = &mut ui.overlay else {
        return;
    };
    let popup = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, popup);
    match overlay {
        Overlay::Menu {
            title,
            items,
            selected,
        } => {
            let list = List::new(items.iter().map(|(name, _)| ListItem::new(text(name))))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} · Enter selects · Esc closes ", text(title))),
                )
                .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
                .highlight_symbol("› ");
            frame.render_stateful_widget(
                list,
                popup,
                &mut ListState::default().with_selected(Some(*selected)),
            );
        }
        Overlay::Document {
            title,
            text: body,
            scroll,
        } => {
            frame.render_widget(
                Paragraph::new(body.as_str())
                    .wrap(Wrap { trim: false })
                    .scroll((*scroll, 0))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(format!(" {} · Page Up/Down · Esc closes ", text(title))),
                    ),
                popup,
            );
        }
        Overlay::Form {
            title,
            fields,
            selected,
            ..
        } => {
            let block = Block::default().borders(Borders::ALL).title(format!(
                " {} · field {}/{} ",
                text(title),
                *selected + 1,
                fields.len()
            ));
            let inner = block.inner(popup);
            frame.render_widget(block, popup);
            let count = (usize::from(inner.height.saturating_sub(2)) / 4).max(1);
            let first = selected.saturating_sub(count - 1);
            for (index, field) in fields.iter_mut().enumerate().skip(first).take(count) {
                let y = inner.y + ((index - first) * 4) as u16;
                if y + 3 >= inner.bottom() {
                    break;
                }
                frame.render_widget(
                    Paragraph::new(field.name).style(Style::default().fg(if index == *selected {
                        Color::Cyan
                    } else {
                        Color::Gray
                    })),
                    Rect::new(inner.x, y, inner.width, 1),
                );
                field
                    .editor
                    .set_block(Block::default().borders(Borders::ALL).border_style(
                        Style::default().fg(if index == *selected {
                            Color::Cyan
                        } else {
                            Color::DarkGray
                        }),
                    ));
                field.editor.set_cursor_line_style(Style::default());
                if index != *selected {
                    field.editor.set_cursor_style(Style::default());
                } else {
                    field
                        .editor
                        .set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
                }
                frame.render_widget(&field.editor, Rect::new(inner.x, y + 1, inner.width, 3));
            }
            frame.render_widget(
                Paragraph::new(text(&ui.notice)).style(Style::default().fg(Color::Yellow)),
                Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 1),
            );
            frame.render_widget(
                Paragraph::new(
                    "Tab changes fields · Enter applies · Alt+Enter adds a line · Esc closes",
                ),
                Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
            );
        }
    }
}

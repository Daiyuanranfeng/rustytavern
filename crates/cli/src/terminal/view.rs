use super::state::{
    FormKind, Hit, Message, Overlay, PREVIEW_TABS, Step, Ui, slash_matches, text, visible,
};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use rustytavern::session::{GenerationMode, Role};
use std::sync::OnceLock;
use syntect::{
    easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet, util::LinesWithEndings,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The function colors the lines of a code block. It returns `None` for an unknown language.
fn highlight(language: &str, code: &str) -> Option<Vec<Vec<Span<'static>>>> {
    static ASSETS: OnceLock<(SyntaxSet, ThemeSet)> = OnceLock::new();
    let (syntaxes, themes) = ASSETS.get_or_init(|| {
        (
            SyntaxSet::load_defaults_newlines(),
            ThemeSet::load_defaults(),
        )
    });
    let syntax = syntaxes.find_syntax_by_token(language)?;
    let mut highlighter = HighlightLines::new(syntax, &themes.themes["base16-ocean.dark"]);
    LinesWithEndings::from(code)
        .map(|line| {
            let regions = highlighter.highlight_line(line, syntaxes).ok()?;
            Some(
                regions
                    .into_iter()
                    .map(|(style, piece)| {
                        let color = style.foreground;
                        Span::styled(
                            piece.trim_end_matches('\n').to_owned(),
                            Style::default().fg(Color::Rgb(color.r, color.g, color.b)),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

/// The renderer converts Markdown into styled terminal lines.
#[derive(Default)]
struct Markdown {
    lines: Vec<Line<'static>>,
    styles: Vec<Style>,
    quotes: usize,
    lists: Vec<Option<u64>>,
    gap: bool,
    item: bool,
}
impl Markdown {
    fn line(&mut self) {
        let indent = "  ".repeat(self.lists.len().saturating_sub(1));
        self.lines.push(Line::from(Span::styled(
            format!("{}{indent}", "│ ".repeat(self.quotes)),
            Style::default().fg(Color::DarkGray),
        )));
    }
    fn block(&mut self) {
        if self.gap {
            self.line();
        }
        self.gap = false;
        self.line();
    }
    fn push(&mut self, value: &str, style: Style) {
        if self.lines.is_empty() {
            self.line();
        }
        if let Some(line) = self.lines.last_mut() {
            line.spans.push(Span::styled(value.to_owned(), style));
        }
    }
    fn nest(&mut self, change: impl Fn(Style) -> Style) {
        self.styles.push(change(self.style()));
    }
    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }
}

pub(super) fn markdown(source: &str) -> Vec<Line<'static>> {
    let mut md = Markdown::default();
    let mut code: Option<(String, String)> = None;
    for event in Parser::new_ext(source, Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Start(Tag::Paragraph) if md.item => md.item = false,
            Event::Start(Tag::Paragraph) => md.block(),
            Event::Start(Tag::Heading { .. }) => {
                md.block();
                md.nest(|s| s.fg(Color::Cyan).add_modifier(Modifier::BOLD));
            }
            Event::Start(Tag::BlockQuote(_)) => md.quotes += 1,
            Event::Start(Tag::CodeBlock(kind)) => {
                md.block();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").into()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((language, String::new()));
            }
            Event::Start(Tag::List(start)) => md.lists.push(start),
            Event::Start(Tag::Item) => {
                md.block();
                let marker = match md.lists.last_mut() {
                    Some(Some(number)) => {
                        *number += 1;
                        format!("{}. ", *number - 1)
                    }
                    _ => "• ".into(),
                };
                md.push(&marker, Style::default().fg(Color::DarkGray));
                md.item = true;
            }
            Event::Start(Tag::Emphasis) => md.nest(|s| s.add_modifier(Modifier::ITALIC)),
            Event::Start(Tag::Strong) => md.nest(|s| s.add_modifier(Modifier::BOLD)),
            Event::Start(Tag::Strikethrough) => md.nest(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Event::Start(Tag::Link { .. }) => md.nest(|s| s.add_modifier(Modifier::UNDERLINED)),
            Event::End(TagEnd::Paragraph) => md.gap = true,
            Event::End(TagEnd::Heading(_)) => {
                md.styles.pop();
                md.gap = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                let (language, source) = code.take().unwrap_or_default();
                let plain = || {
                    source
                        .lines()
                        .map(|line| {
                            vec![Span::styled(
                                line.to_owned(),
                                Style::default().fg(Color::Yellow),
                            )]
                        })
                        .collect()
                };
                let lines: Vec<Vec<Span<'static>>> =
                    highlight(&language, &source).unwrap_or_else(plain);
                for (index, spans) in lines.into_iter().enumerate() {
                    if index > 0 {
                        md.line();
                    }
                    md.push("  ", Style::default());
                    if let Some(line) = md.lines.last_mut() {
                        line.spans.extend(spans);
                    }
                }
                md.gap = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                md.quotes -= 1;
                md.gap = true;
            }
            Event::End(TagEnd::List(_)) => {
                md.lists.pop();
                md.gap = true;
            }
            Event::End(TagEnd::Item) => md.item = false,
            Event::End(
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link,
            ) => _ = md.styles.pop(),
            Event::Text(value) if code.is_some() => {
                if let Some((_, source)) = &mut code {
                    source.push_str(&value);
                }
            }
            Event::Text(value) | Event::Html(value) | Event::InlineHtml(value) => {
                md.push(&value, md.style())
            }
            Event::Code(value) => md.push(&value, md.style().fg(Color::Yellow)),
            Event::SoftBreak | Event::HardBreak => md.line(),
            Event::Rule => {
                md.block();
                md.push("────────", Style::default().fg(Color::DarkGray));
                md.gap = true;
            }
            _ => {}
        }
    }
    md.lines
}

/// The function appends a titled block of Markdown text. An empty body adds nothing.
fn section(lines: &mut Vec<Line<'static>>, title: String, body: &str) {
    let body = text(body.trim());
    if body.is_empty() {
        return;
    }
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    lines.push(Line::styled(
        title,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    lines.extend(markdown(&body));
}

/// The function returns the number of items in each preview tab.
fn preview_counts(data: &serde_json::Value) -> [usize; 4] {
    let count = |value: &serde_json::Value| value.as_array().map_or(0, Vec::len);
    [
        0,
        1 + count(&data["alternate_greetings"]),
        count(&data["character_book"]["entries"]),
        0,
    ]
}

/// The function renders one tab of the character preview.
fn profile(card: &rustytavern::card::Card, tab: usize) -> Vec<Line<'static>> {
    let data = card.data();
    let field = |key: &str| data[key].as_str().unwrap_or_default();
    let dim = Style::default().fg(Color::DarkGray);
    let mut lines = Vec::new();
    match tab {
        0 => {
            lines.push(Line::styled(
                text(card.name()),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            if !field("nickname").is_empty() {
                lines.push(Line::styled(format!("“{}”", text(field("nickname"))), dim));
            }
            let mut facts = Vec::new();
            if !field("creator").is_empty() {
                facts.push(format!("by {}", text(field("creator"))));
            }
            if !field("character_version").is_empty() {
                facts.push(format!("version {}", text(field("character_version"))));
            }
            let [_, greetings, entries, _] = preview_counts(data);
            facts.push(format!("{greetings} greetings"));
            facts.push(format!("{entries} lorebook entries"));
            facts.push(format!(
                "{} · {}",
                card.format.to_uppercase(),
                text(card.document["spec"].as_str().unwrap_or("legacy"))
            ));
            lines.push(Line::styled(facts.join(" · "), dim));
            let tags: Vec<Span<'static>> = data["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|tag| tag.as_str())
                .flat_map(|tag| {
                    [
                        Span::styled(
                            format!(" {} ", text(tag).replace('\n', " ")),
                            Style::default().fg(Color::Black).bg(Color::Gray),
                        ),
                        Span::raw(" "),
                    ]
                })
                .collect();
            if !tags.is_empty() {
                lines.push(Line::default());
                lines.push(Line::from(tags));
            }
            if card.ensure_compatible(false).is_err() {
                lines.push(Line::default());
                lines.push(Line::styled(
                    "⚠ RustyTavern does not support some features of this card. Chats ignore them.",
                    Style::default().fg(Color::Yellow),
                ));
            }
            for (title, key) in [
                ("Creator notes", "creator_notes"),
                ("Description", "description"),
                ("Personality", "personality"),
                ("Scenario", "scenario"),
            ] {
                section(&mut lines, title.into(), field(key));
            }
        }
        1 => {
            section(&mut lines, "1 · First message".into(), field("first_mes"));
            let alternates = data["alternate_greetings"].as_array().into_iter().flatten();
            for (index, greeting) in alternates.enumerate() {
                section(
                    &mut lines,
                    format!("{} · Alternate greeting", index + 2),
                    greeting.as_str().unwrap_or_default(),
                );
            }
            let group = data["group_only_greetings"].as_array().map_or(0, Vec::len);
            if group > 0 {
                lines.push(Line::default());
                lines.push(Line::styled(
                    format!("{group} greetings are for group chats only."),
                    dim,
                ));
            }
        }
        2 => {
            let book = &data["character_book"];
            let entries = book["entries"].as_array().map_or(&[][..], Vec::as_slice);
            if entries.is_empty() {
                lines.push(Line::styled("This card has no lorebook.", dim));
            }
            let name = book["name"].as_str().unwrap_or("Lorebook");
            section(
                &mut lines,
                text(name),
                book["description"].as_str().unwrap_or_default(),
            );
            for (index, entry) in entries.iter().enumerate() {
                let name = ["comment", "name"]
                    .iter()
                    .find_map(|key| entry[key].as_str().filter(|value| !value.is_empty()))
                    .unwrap_or("Entry");
                let mut title = format!("{} · {}", index + 1, text(name));
                if entry["enabled"] == false {
                    title.push_str(" · disabled");
                }
                if entry["constant"] == true {
                    title.push_str(" · always active");
                }
                let keys: Vec<&str> = entry["keys"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|key| key.as_str())
                    .collect();
                section(&mut lines, title, entry["content"].as_str().unwrap_or(" "));
                if !keys.is_empty() {
                    lines.push(Line::styled(
                        format!("Keys: {}", text(&keys.join(", "))),
                        dim,
                    ));
                }
            }
        }
        _ => {
            for (title, key) in [
                ("System prompt", "system_prompt"),
                ("Post-history instructions", "post_history_instructions"),
                ("Example messages", "mes_example"),
            ] {
                section(&mut lines, title.into(), field(key));
            }
            let assets: Vec<String> = data["assets"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|asset| {
                    format!(
                        "- {} · {} · {}",
                        asset["type"].as_str().unwrap_or("asset"),
                        asset["name"].as_str().unwrap_or_default(),
                        asset["uri"].as_str().unwrap_or_default()
                    )
                })
                .collect();
            section(&mut lines, "Assets".into(), &assets.join("\n"));
            let extensions: Vec<&str> = data["extensions"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(key, _)| key.as_str())
                .collect();
            section(&mut lines, "Extensions".into(), &extensions.join(", "));
            let diagnostics: Vec<String> = card
                .diagnostics
                .iter()
                .map(|item| format!("- {} · {} · {:?}", item.field, item.code, item.severity))
                .collect();
            section(&mut lines, "Compatibility".into(), &diagnostics.join("\n"));
            if let Some(source) = &card.provenance {
                let mut facts = vec![format!("- Address: {}", source.requested_url)];
                facts.extend(
                    source
                        .license
                        .as_ref()
                        .map(|value| format!("- License: {value}")),
                );
                section(&mut lines, "Source".into(), &facts.join("\n"));
            }
            if lines.is_empty() {
                lines.push(Line::styled("This card has no prompt overrides.", dim));
            }
        }
    }
    lines
}

/// The function wraps styled lines at word boundaries.
fn wrap(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    for line in lines {
        let mut current: Vec<Span<'static>> = Vec::new();
        let mut used = 0;
        for span in line.spans {
            for word in span.content.split_inclusive(' ') {
                let mut pieces = vec![String::new()];
                for character in word.chars() {
                    let piece = pieces.last_mut().unwrap();
                    if piece.width() + character.width().unwrap_or(0) > width {
                        pieces.push(String::new());
                    }
                    pieces.last_mut().unwrap().push(character);
                }
                for piece in pieces {
                    if used > 0 && used + piece.trim_end().width() > width {
                        result.push(Line::from(std::mem::take(&mut current)));
                        used = 0;
                    }
                    used += piece.width();
                    current.push(Span::styled(piece, span.style));
                }
            }
        }
        result.push(Line::from(current));
    }
    for line in &mut result {
        if let Some(last) = line.spans.last_mut() {
            last.content = last.content.trim_end().to_owned().into();
        }
    }
    result
}

/// The function draws a rounded box around wrapped message lines.
fn bubble(body: Vec<Line<'static>>, width: usize, color: Color) -> Vec<Line<'static>> {
    let body = wrap(body, width.saturating_sub(4).max(8));
    let inner = body.iter().map(Line::width).max().unwrap_or(0);
    let border = Style::default().fg(color);
    let edge = |left: &str, right: &str| {
        Line::styled(format!("{left}{}{right}", "─".repeat(inner + 2)), border)
    };
    let mut lines = vec![edge("╭", "╮")];
    for line in body {
        let pad = " ".repeat(inner - line.width() + 1);
        let mut spans = vec![Span::styled("│ ", border)];
        spans.extend(line.spans);
        spans.push(Span::styled(format!("{pad}│"), border));
        lines.push(Line::from(spans));
    }
    lines.push(edge("╰", "╯"));
    lines
}

/// The function reads the bubble colors of a theme. An invalid color keeps its default.
pub(super) fn bubbles(theme: &rustytavern::extension::Extension) -> [Color; 2] {
    let color = |value: &Option<String>, default| {
        value
            .as_deref()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    [
        color(&theme.user_bubble, Color::Green),
        color(&theme.assistant_bubble, Color::Cyan),
    ]
}

fn message(
    lines: &mut Vec<Line<'static>>,
    (width, colors): (usize, [Color; 2]),
    user: bool,
    label: &str,
    body: Vec<Line<'static>>,
) {
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let color = colors[usize::from(!user)];
    let alignment = if user {
        Alignment::Right
    } else {
        Alignment::Left
    };
    lines.push(
        Line::styled(
            label.to_owned(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )
        .alignment(alignment),
    );
    lines.extend(
        bubble(body, width, color)
            .into_iter()
            .map(|line| line.alignment(alignment)),
    );
}

/// The function draws one small bubble for each step. A step in the scrollback stays collapsed.
fn steps(lines: &mut Vec<Line<'static>>, steps: &[Step], tick: usize, width: usize) {
    let dim = Style::default().fg(Color::DarkGray);
    for step in steps {
        let (icon, name) = match step.tool.as_deref() {
            None => ("✻", "Thinking"),
            Some(call) if call.starts_with("mcp__") => ("⧉", call),
            Some(call) if call.starts_with("web") => ("↗", call),
            Some(call) => ("⚙", call),
        };
        let name: String = name
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(width.saturating_sub(12))
            .collect();
        let state = match (step.done, step.tool.is_some()) {
            (false, _) => ["◐", "◓", "◑", "◒"][tick % 4],
            (true, true) => "✓",
            (true, false) => "",
        };
        let mut body = vec![Line::from(vec![
            Span::styled(
                format!("{} {icon} ", if step.open { "▾" } else { "▸" }),
                dim,
            ),
            Span::styled(
                name,
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {state}"), dim),
        ])];
        if step.open {
            body.extend(
                step.body
                    .lines()
                    .take(40)
                    .map(|line| Line::styled(line.to_owned(), dim.add_modifier(Modifier::ITALIC))),
            );
        }
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.extend(bubble(body, width, Color::DarkGray));
    }
}

fn bubble_width(width: u16) -> usize {
    (usize::from(width) * 3 / 4).max(16)
}

/// The function renders queued messages for the scrollback. A blank line separates them from earlier output.
pub(super) fn history(messages: &[Message], width: u16, colors: [Color; 2]) -> Vec<Line<'static>> {
    let width = bubble_width(width);
    let mut lines = Vec::new();
    for item in messages {
        let collapsed: Vec<Step> = item
            .steps
            .iter()
            .map(|step| Step {
                tool: step.tool.clone(),
                body: String::new(),
                done: true,
                open: false,
            })
            .collect();
        steps(&mut lines, &collapsed, 0, width);
        message(
            &mut lines,
            (width, colors),
            item.user,
            &item.label,
            markdown(&item.text),
        );
    }
    lines.insert(0, Line::default());
    lines
}

/// The function returns the lines of the active exchange: the sent message, the steps, the reply, and a failure.
fn live(ui: &Ui, width: u16) -> Vec<Line<'static>> {
    let width = bubble_width(width);
    let mut lines = Vec::new();
    if !ui.outgoing.is_empty() {
        message(
            &mut lines,
            (width, ui.bubbles),
            true,
            &ui.names[1],
            markdown(&ui.outgoing),
        );
    }
    steps(&mut lines, &ui.steps, ui.tick, width);
    if ui.streaming && ui.mode == GenerationMode::Impersonate {
        let body = markdown(&format!("{}▍", ui.reply));
        message(&mut lines, (width, ui.bubbles), true, &ui.names[1], body);
    } else if ui.streaming || !ui.reply.is_empty() {
        let mut body = markdown(&ui.reply);
        let dim = Style::default().fg(Color::DarkGray);
        if ui.reply.is_empty() {
            body = vec![Line::styled(["·  ", "·· ", "···"][ui.tick / 4 % 3], dim)];
        } else if ui.streaming
            && let Some(line) = body.last_mut()
        {
            line.spans.push(Span::styled("▍", dim));
        }
        let label = if ui.mode == GenerationMode::Continue {
            format!("{} · continues", ui.names[0])
        } else {
            ui.names[0].clone()
        };
        message(&mut lines, (width, ui.bubbles), false, &label, body);
    }
    if let Some(failure) = &ui.failure {
        let style = Style::default().fg(Color::Red);
        let label = format!("{} · the reply failed", ui.names[0]);
        let body = vec![Line::styled(format!("⚠ {failure}"), style)];
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::styled(label, style.add_modifier(Modifier::BOLD)));
        lines.extend(bubble(body, width, Color::Red));
    }
    if let Some(head) = ui.head.as_ref().filter(|_| ui.idle() && lines.is_empty()) {
        lines.push(actions(ui, head));
    }
    lines
}

/// The function returns the action bar for the last message. The keys work while the message box is empty.
fn actions(ui: &Ui, head: &rustytavern::session::Message) -> Line<'static> {
    let assistant = head.role == Role::Assistant;
    // A greeting has no user message before it, so it has no retry.
    let reply = assistant && head.parent_id.is_some();
    let mut parts = Vec::new();
    if ui.alternatives.len() > 1 {
        let index = ui.alternatives.iter().position(|id| *id == head.id);
        parts.push(format!(
            "‹ {}/{} › ←→ alternatives",
            index.map_or(0, |i| i + 1),
            ui.alternatives.len()
        ));
    } else if reply {
        parts.push("→ another reply".into());
    }
    parts.push("↑ edit".into());
    if reply {
        parts.push("Ctrl+R retry".into());
    } else if !assistant {
        parts.push("Ctrl+R reply".into());
    }
    if assistant {
        parts.push("/continue".into());
    }
    parts.push("/impersonate".into());
    parts.push("Esc Esc messages".into());
    Line::styled(parts.join(" · "), Style::default().fg(Color::DarkGray))
}

fn composer_height(ui: &Ui) -> u16 {
    ui.composer.lines().len().clamp(1, 8) as u16 + 2
}

fn popup_height(ui: &Ui) -> u16 {
    ui.typed_command()
        .map_or(0, |typed| slash_matches(&typed).count() as u16 + 2)
}

/// The function returns the height of the inline area: the active exchange, the command popup, the composer, and the status.
pub(super) fn height(ui: &Ui, width: u16, screen: u16) -> u16 {
    let live = Paragraph::new(live(ui, width))
        .wrap(Wrap { trim: false })
        .line_count(width)
        .min(usize::from(screen)) as u16;
    (live.max(popup_height(ui)) + composer_height(ui) + 2).min(screen)
}

fn inline(frame: &mut Frame<'_>, ui: &mut Ui, area: Rect) {
    let slots = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(composer_height(ui)),
            Constraint::Length(2),
        ])
        .split(area);
    let dim = Style::default().fg(Color::DarkGray);
    let paragraph = Paragraph::new(live(ui, area.width)).wrap(Wrap { trim: false });
    let total = paragraph.line_count(slots[0].width).min(u16::MAX as usize) as u16;
    let offset = total.saturating_sub(slots[0].height);
    frame.render_widget(paragraph.scroll((offset, 0)), slots[0]);
    ui.composer.set_block(
        Block::default()
            .borders(Borders::ALL)
            .title(if ui.busy {
                " Message · the operation is active "
            } else {
                " Message · Enter sends · Alt+Enter newline · /help "
            })
            .border_style(dim),
    );
    ui.composer.set_cursor_line_style(Style::default());
    frame.render_widget(&ui.composer, slots[1]);
    let progress = if ui.busy {
        ["·", "•", "●", "•"][ui.tick % 4]
    } else {
        ""
    };
    let strong = Style::default().add_modifier(Modifier::BOLD);
    let mut status = vec![Span::styled(text(&ui.title), strong.fg(Color::Cyan))];
    match &ui.model {
        Some([model, provider]) => status.extend([
            Span::styled(" · Model ", dim),
            Span::styled(text(model), strong),
            Span::styled(" · Provider ", dim),
            Span::styled(text(provider), strong),
            Span::styled(
                if ui.preferences.session.is_some() {
                    ""
                } else {
                    " · for new chats"
                },
                dim,
            ),
        ]),
        None => status.push(Span::styled(" · No model · use /model", dim)),
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(text(&format!("{progress} {}", ui.notice))),
            Line::from(status),
        ]),
        slots[2],
    );
    if let Some(typed) = ui.typed_command() {
        let matches: Vec<_> = slash_matches(&typed).collect();
        let height = (matches.len() as u16 + 2).min(slots[0].height);
        let popup = Rect::new(
            slots[1].x,
            slots[1].y.saturating_sub(height),
            slots[1].width,
            height,
        );
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(dim)
            .title(" Commands · ↑↓ selects · Tab completes · Enter runs ");
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        for (row, (name, description)) in matches
            .into_iter()
            .enumerate()
            .take(usize::from(inner.height))
        {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        format!("{name:<12}"),
                        Style::default()
                            .fg(if row == ui.slash {
                                Color::Cyan
                            } else {
                                Color::White
                            })
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(*description, Style::default().fg(Color::Gray)),
                ])),
                Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
            );
        }
    }
}

/// The function draws a dialog on the alternate screen, or the inline area below the scrollback.
pub(super) fn draw(frame: &mut Frame<'_>, ui: &mut Ui) {
    ui.hits.clear();
    let area = frame.area();
    let small = if ui.overlay.is_some() { 10 } else { 3 };
    if area.width < 32 || area.height < small {
        frame.render_widget(
            Paragraph::new("The terminal needs 32 columns and 10 rows."),
            area,
        );
        return;
    }
    if ui.overlay.is_none() {
        return inline(frame, ui, area);
    }
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
            filter,
            tabs,
            tab,
        } => {
            let shown = visible(items, filter, tabs, *tab);
            let hint = if filter.is_empty() {
                "click or Enter selects · type to filter · Esc closes".into()
            } else {
                format!("filter: {}", text(filter))
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} · {hint} ", text(title)))
                .title_bottom(Line::styled(
                    format!(" {} ", text(&ui.notice)),
                    Style::default().fg(Color::Yellow),
                ));
            let mut inner = block.inner(popup);
            frame.render_widget(block, popup);
            if !tabs.is_empty() && inner.height > 3 {
                let mut x = inner.x;
                for (index, (label, _)) in tabs.iter().enumerate() {
                    let label = format!(" {} ", text(label));
                    let width = (label.width() as u16).min(inner.right().saturating_sub(x));
                    let area = Rect::new(x, inner.y, width, 1);
                    let style = if index == *tab {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    frame.render_widget(Paragraph::new(Span::styled(label, style)), area);
                    ui.hits.push((area, Hit::Tab(index)));
                    x = x.saturating_add(width + 1);
                }
                frame.render_widget(
                    Paragraph::new("─".repeat(usize::from(inner.width)))
                        .style(Style::default().fg(Color::DarkGray)),
                    Rect::new(inner.x, inner.y + 1, inner.width, 1),
                );
                inner = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
            }
            let list = List::new(
                shown
                    .iter()
                    .map(|&index| ListItem::new(text(&items[index].0))),
            )
            .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
            .highlight_symbol("› ");
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, inner, &mut state);
            for row in 0..inner.height {
                let index = state.offset() + usize::from(row);
                if index < shown.len() {
                    ui.hits.push((
                        Rect::new(inner.x, inner.y + row, inner.width, 1),
                        Hit::Item(index),
                    ));
                }
            }
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
        Overlay::Preview {
            card,
            image,
            tab,
            scroll,
            ..
        } => {
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " Add {}? · ←→ changes the tab · ↑↓ scrolls ",
                    text(card.name())
                ))
                .title_bottom(Line::styled(
                    format!(" {} ", text(&ui.notice)),
                    Style::default().fg(Color::Yellow),
                ));
            let inner = block.inner(popup);
            frame.render_widget(block, popup);
            if inner.height < 6 {
                return;
            }
            let counts = preview_counts(card.data());
            let mut x = inner.x;
            for (index, name) in PREVIEW_TABS.iter().enumerate() {
                let label = match counts[index] {
                    0 => format!(" {name} "),
                    count => format!(" {name} ({count}) "),
                };
                let width = (label.width() as u16).min(inner.right().saturating_sub(x));
                let area = Rect::new(x, inner.y, width, 1);
                let style = if index == *tab {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                frame.render_widget(Paragraph::new(Span::styled(label, style)), area);
                ui.hits.push((area, Hit::Tab(index)));
                x = x.saturating_add(width + 1);
            }
            let mut body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 4);
            if let Some(image) = image
                && *tab == 0
                && body.width >= 60
            {
                let width = (body.width * 2 / 5).min(56);
                frame.render_stateful_widget(
                    ratatui_image::StatefulImage::default(),
                    Rect { width, ..body },
                    image.as_mut(),
                );
                body = Rect::new(
                    body.x + width + 2,
                    body.y,
                    body.width - width - 2,
                    body.height,
                );
            }
            let paragraph = Paragraph::new(profile(card, *tab)).wrap(Wrap { trim: false });
            let total = paragraph.line_count(body.width).min(u16::MAX as usize) as u16;
            *scroll = (*scroll).min(total.saturating_sub(body.height));
            frame.render_widget(paragraph.scroll((*scroll, 0)), body);
            let footer = inner.bottom().saturating_sub(1);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "[ Add character ]",
                        Style::default().fg(Color::Black).bg(Color::Cyan),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        "[ Cancel ]",
                        Style::default().fg(Color::Black).bg(Color::Gray),
                    ),
                    Span::raw("  Enter adds and selects · Esc cancels"),
                ])),
                Rect::new(inner.x, footer, inner.width, 1),
            );
            ui.hits
                .push((Rect::new(inner.x, footer, 17, 1), Hit::Apply));
            ui.hits
                .push((Rect::new(inner.x + 19, footer, 10, 1), Hit::Close));
        }
        Overlay::Form {
            title,
            kind,
            fields,
            selected,
        } => {
            let tall = matches!(kind, FormKind::Edit(_));
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
                ui.hits
                    .push((Rect::new(inner.x, y, inner.width, 4), Hit::Item(index)));
                frame.render_widget(
                    Paragraph::new(field.name).style(Style::default().fg(if index == *selected {
                        Color::Cyan
                    } else {
                        Color::Gray
                    })),
                    Rect::new(inner.x, y, inner.width, 1),
                );
                let variable = field.editor.lines().iter().all(|line| {
                    line.bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                });
                if field.secret && !variable {
                    field.editor.set_mask_char('•');
                } else {
                    field.editor.clear_mask_char();
                }
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
                let rows = if tall {
                    inner.height.saturating_sub(3).max(3)
                } else {
                    3
                };
                frame.render_widget(&field.editor, Rect::new(inner.x, y + 1, inner.width, rows));
            }
            frame.render_widget(
                Paragraph::new(text(&ui.notice)).style(Style::default().fg(Color::Yellow)),
                Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 1),
            );
            let footer = inner.bottom().saturating_sub(1);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "[ Apply ]",
                        Style::default().fg(Color::Black).bg(Color::Cyan),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        "[ Cancel ]",
                        Style::default().fg(Color::Black).bg(Color::Gray),
                    ),
                    Span::raw("  Tab changes fields · Enter applies · Alt+Enter adds a line"),
                ])),
                Rect::new(inner.x, footer, inner.width, 1),
            );
            ui.hits.push((Rect::new(inner.x, footer, 9, 1), Hit::Apply));
            ui.hits
                .push((Rect::new(inner.x + 11, footer, 10, 1), Hit::Close));
        }
    }
    let close = Rect::new(popup.right().saturating_sub(5), popup.y, 3, 1);
    frame.render_widget(Paragraph::new("[×]"), close);
    ui.hits.push((close, Hit::Close));
}

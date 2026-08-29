use super::syntax::highlight_code_line;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

const ORANGE: Color = Color::Rgb(255, 107, 0);

#[derive(Clone, Copy)]
struct ListState {
    next: Option<u64>,
}

struct Renderer {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    style: Style,
    style_stack: Vec<Style>,
    links: Vec<String>,
    lists: Vec<ListState>,
    quote_depth: usize,
    code_language: Option<String>,
    table_cell: usize,
}

impl Renderer {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            current: Vec::new(),
            style: Style::default(),
            style_stack: Vec::new(),
            links: Vec::new(),
            lists: Vec::new(),
            quote_depth: 0,
            code_language: None,
            table_cell: 0,
        }
    }

    fn ensure_prefix(&mut self) {
        if self.current.is_empty() && self.quote_depth > 0 {
            self.current.push(Span::styled(
                format!(" {}", "│ ".repeat(self.quote_depth)),
                Style::default().fg(ORANGE),
            ));
        }
    }

    fn push(&mut self, value: &str, style: Style) {
        self.ensure_prefix();
        let mut start = 0;
        for (index, character) in value.char_indices() {
            if character != '\n' {
                continue;
            }
            if index > start {
                self.push_span(safe_terminal_text(&value[start..index]), style);
            }
            self.flush(false);
            start = index + 1;
        }
        if start < value.len() {
            self.push_span(safe_terminal_text(&value[start..]), style);
        }
    }

    fn push_span(&mut self, value: String, style: Style) {
        if let Some(previous) = self.current.last_mut()
            && previous.style == style
        {
            previous.content.to_mut().push_str(&value);
        } else {
            self.current.push(Span::styled(value, style));
        }
    }

    fn flush(&mut self, force: bool) {
        if self.current.is_empty() && !force {
            return;
        }
        let mut line = Line::from(std::mem::take(&mut self.current));
        let content = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        let trimmed = content.trim_start();
        let color = if trimmed.starts_with("diff --git") || trimmed.starts_with("@@") {
            Some(Color::Cyan)
        } else if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
            Some(Color::Green)
        } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
            Some(Color::Red)
        } else {
            None
        };
        if let Some(color) = color {
            for span in &mut line.spans {
                span.style.fg = Some(color);
            }
        }
        self.lines.push(line);
    }

    fn push_code(&mut self, value: &str) {
        let language = self.code_language.as_deref().unwrap_or("text");
        let mut parts = value.split('\n').peekable();
        while let Some(raw) = parts.next() {
            if raw.is_empty() && parts.peek().is_none() {
                break;
            }
            self.lines
                .push(Line::from(highlight_code_line(language, raw)));
        }
    }

    fn start_style(&mut self, style: Style) {
        self.style_stack.push(self.style);
        self.style = style;
    }

    fn end_style(&mut self) {
        self.style = self.style_stack.pop().unwrap_or_default();
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if self.current.is_empty()
                    && self.lists.is_empty()
                    && self.quote_depth == 0
                    && self.table_cell == 0
                {
                    self.push(" ", Style::default());
                }
            }
            Tag::Heading { level, .. } => {
                self.flush(false);
                let color = if matches!(
                    level,
                    pulldown_cmark::HeadingLevel::H1
                        | pulldown_cmark::HeadingLevel::H2
                        | pulldown_cmark::HeadingLevel::H3
                ) {
                    ORANGE
                } else {
                    Color::White
                };
                self.push(" ", Style::default());
                self.start_style(Style::default().fg(color).add_modifier(Modifier::BOLD));
            }
            Tag::BlockQuote(_) => {
                self.flush(false);
                self.quote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.flush(false);
                let language = match kind {
                    CodeBlockKind::Indented => "text".to_owned(),
                    CodeBlockKind::Fenced(value) => value
                        .split_whitespace()
                        .next()
                        .filter(|value| !value.is_empty())
                        .unwrap_or("text")
                        .to_owned(),
                };
                self.lines.push(Line::from(Span::styled(
                    format!(" ┌─ {language}"),
                    Style::default().fg(Color::DarkGray),
                )));
                self.code_language = Some(language);
            }
            Tag::List(start) => {
                self.flush(false);
                self.lists.push(ListState { next: start });
            }
            Tag::Item => {
                self.flush(false);
                self.ensure_prefix();
                self.current
                    .push(Span::raw("  ".repeat(self.lists.len().saturating_sub(1))));
                let marker = self
                    .lists
                    .last_mut()
                    .map(|list| match list.next.as_mut() {
                        Some(next) => {
                            let marker = format!("{next}. ");
                            *next = next.saturating_add(1);
                            marker
                        }
                        None => "• ".into(),
                    })
                    .unwrap_or_else(|| "• ".into());
                self.current
                    .push(Span::styled(marker, Style::default().fg(ORANGE)));
            }
            Tag::Table(_) => {
                self.flush(false);
                self.table_cell = 0;
            }
            Tag::TableHead => {}
            Tag::TableRow => {
                self.flush(false);
                self.table_cell = 0;
                self.push(" │ ", Style::default().fg(Color::DarkGray));
            }
            Tag::TableCell => {
                if self.table_cell > 0 {
                    self.push(" │ ", Style::default().fg(Color::DarkGray));
                }
                self.table_cell += 1;
            }
            Tag::Emphasis => self.start_style(self.style.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.start_style(self.style.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.start_style(self.style.add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { dest_url, .. } => {
                self.links.push(safe_terminal_text(dest_url.as_ref()));
                self.start_style(
                    self.style
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::UNDERLINED),
                );
            }
            Tag::Image { dest_url, .. } => {
                self.links
                    .push(format!("image: {}", safe_terminal_text(dest_url.as_ref())));
                self.start_style(self.style.fg(Color::Cyan));
            }
            Tag::FootnoteDefinition(label) => {
                self.flush(false);
                self.push(
                    &format!("[{}] ", safe_terminal_text(label.as_ref())),
                    Style::default().fg(ORANGE),
                );
            }
            Tag::DefinitionListTitle => {
                self.flush(false);
                self.start_style(self.style.add_modifier(Modifier::BOLD));
            }
            Tag::DefinitionListDefinition => {
                self.flush(false);
                self.push("  — ", Style::default().fg(ORANGE));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::TableRow => {
                self.flush(false);
                if matches!(tag, TagEnd::Heading(_)) {
                    self.end_style();
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush(false);
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.code_language = None;
            }
            TagEnd::List(_) => {
                self.flush(false);
                self.lists.pop();
            }
            TagEnd::TableHead => {
                self.flush(false);
                self.lines.push(Line::from(Span::styled(
                    " ├────────────────",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            TagEnd::Table => {
                self.flush(false);
                self.table_cell = 0;
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::DefinitionListTitle => self.end_style(),
            TagEnd::Link | TagEnd::Image => {
                self.end_style();
                if let Some(destination) = self.links.pop() {
                    self.push(
                        &format!(" ({destination})"),
                        Style::default().fg(Color::DarkGray),
                    );
                }
            }
            TagEnd::FootnoteDefinition | TagEnd::DefinitionListDefinition => self.flush(false),
            _ => {}
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(value) if self.code_language.is_some() => self.push_code(value.as_ref()),
            Event::Text(value) => self.push(value.as_ref(), self.style),
            Event::Code(value) => {
                self.push(value.as_ref(), self.style.fg(Color::Cyan));
            }
            Event::InlineMath(value) => {
                self.push(
                    &format!("${}$", safe_terminal_text(value.as_ref())),
                    self.style,
                );
            }
            Event::DisplayMath(value) => {
                self.flush(false);
                self.push(
                    &format!("$${}$$", safe_terminal_text(value.as_ref())),
                    self.style.fg(Color::Cyan),
                );
                self.flush(false);
            }
            Event::Html(value) | Event::InlineHtml(value) => {
                self.push(value.as_ref(), self.style.fg(Color::DarkGray));
            }
            Event::FootnoteReference(label) => {
                self.push(
                    &format!("[{}]", safe_terminal_text(label.as_ref())),
                    self.style.fg(Color::Cyan),
                );
            }
            Event::SoftBreak => self.flush(true),
            Event::HardBreak => self.flush(true),
            Event::Rule => {
                self.flush(false);
                self.lines.push(Line::from(Span::styled(
                    " ─────────────────",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            Event::TaskListMarker(checked) => self.push(
                if checked { "☑ " } else { "☐ " },
                Style::default().fg(if checked {
                    Color::Green
                } else {
                    Color::DarkGray
                }),
            ),
        }
    }
}

fn safe_terminal_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character == '\t' {
                ' '
            } else if character.is_control() {
                '�'
            } else {
                character
            }
        })
        .collect()
}

pub(super) fn markdown_lines(text: &str) -> Vec<Line<'static>> {
    let mut options = Options::ENABLE_GFM;
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_DEFINITION_LIST);
    let mut renderer = Renderer::new();
    for event in Parser::new_ext(text, options) {
        renderer.event(event);
    }
    renderer.flush(false);
    renderer.lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered_text(markdown: &str) -> String {
        markdown_lines(markdown)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_gfm_tables_tasks_strikethrough_and_safe_links() {
        let rendered = rendered_text(
            "| State | Value |\n| --- | ---: |\n| Done | ~~old~~ new |\n\n- [x] shipped\n- [ ] audit\n\n[docs](https://example.com)",
        );
        assert!(rendered.contains("State │ Value"));
        assert!(rendered.contains("Done"));
        assert!(rendered.contains("☑ shipped"));
        assert!(rendered.contains("☐ audit"));
        assert!(rendered.contains("docs (https://example.com)"));
    }

    #[test]
    fn replaces_terminal_control_characters() {
        assert_eq!(rendered_text("safe\u{1b}[31m"), " safe�[31m");
    }
}

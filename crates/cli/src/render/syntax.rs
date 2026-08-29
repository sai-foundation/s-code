use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

const KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "def",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "fn",
    "for",
    "from",
    "function",
    "if",
    "impl",
    "import",
    "in",
    "interface",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "new",
    "none",
    "null",
    "pub",
    "raise",
    "ref",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "switch",
    "throw",
    "trait",
    "true",
    "try",
    "type",
    "use",
    "var",
    "where",
    "while",
    "with",
    "yield",
];

fn comment_marker(language: &str) -> Option<&'static str> {
    match language.to_ascii_lowercase().as_str() {
        "python" | "py" | "bash" | "sh" | "shell" | "zsh" | "toml" | "yaml" | "yml" => Some("#"),
        "rust" | "rs" | "javascript" | "js" | "jsx" | "typescript" | "ts" | "tsx" | "java"
        | "c" | "cpp" | "c++" | "csharp" | "cs" | "go" | "swift" | "kotlin" => Some("//"),
        _ => None,
    }
}

fn push_plain(spans: &mut Vec<Span<'static>>, text: &mut String) {
    if !text.is_empty() {
        spans.push(Span::styled(
            std::mem::take(text),
            Style::default().fg(Color::Cyan),
        ));
    }
}

pub(super) fn highlight_code_line(language: &str, line: &str) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(" │ ", Style::default().fg(Color::DarkGray))];
    let marker = comment_marker(language);
    let mut plain = String::new();
    let characters = line.char_indices().collect::<Vec<_>>();
    let mut index = 0;
    while index < characters.len() {
        let (byte, character) = characters[index];
        let tail = &line[byte..];
        if marker.is_some_and(|marker| tail.starts_with(marker)) {
            push_plain(&mut spans, &mut plain);
            spans.push(Span::styled(
                tail.to_owned(),
                Style::default().fg(Color::DarkGray),
            ));
            return spans;
        }
        if matches!(character, '"' | '\'' | '`') {
            push_plain(&mut spans, &mut plain);
            let quote = character;
            let mut end = byte + character.len_utf8();
            let mut escaped = false;
            index += 1;
            while index < characters.len() {
                let (next_byte, next) = characters[index];
                end = next_byte + next.len_utf8();
                if next == quote && !escaped {
                    index += 1;
                    break;
                }
                escaped = next == '\\' && !escaped;
                if next != '\\' {
                    escaped = false;
                }
                index += 1;
            }
            spans.push(Span::styled(
                line[byte..end].to_owned(),
                Style::default().fg(Color::Green),
            ));
            continue;
        }
        if character.is_ascii_digit() {
            push_plain(&mut spans, &mut plain);
            let start = byte;
            index += 1;
            while index < characters.len()
                && matches!(characters[index].1, '0'..='9' | '.' | '_' | 'a'..='f' | 'A'..='F' | 'x' | 'o')
            {
                index += 1;
            }
            let end = characters
                .get(index)
                .map_or(line.len(), |(next_byte, _)| *next_byte);
            spans.push(Span::styled(
                line[start..end].to_owned(),
                Style::default().fg(Color::Yellow),
            ));
            continue;
        }
        if character == '_' || character.is_alphabetic() {
            push_plain(&mut spans, &mut plain);
            let start = byte;
            index += 1;
            while index < characters.len()
                && (characters[index].1 == '_' || characters[index].1.is_alphanumeric())
            {
                index += 1;
            }
            let end = characters
                .get(index)
                .map_or(line.len(), |(next_byte, _)| *next_byte);
            let word = &line[start..end];
            let style = if KEYWORDS.contains(&word.to_ascii_lowercase().as_str()) {
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            spans.push(Span::styled(word.to_owned(), style));
            continue;
        }
        plain.push(character);
        index += 1;
    }
    push_plain(&mut spans, &mut plain);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_keywords_strings_numbers_and_comments_without_losing_text() {
        let spans = highlight_code_line("rust", r#"let value = "safe"; // note 42"#);
        assert_eq!(
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            r#" │ let value = "safe"; // note 42"#
        );
        assert!(
            spans
                .iter()
                .any(|span| span.style.fg == Some(Color::Magenta))
        );
        assert!(spans.iter().any(|span| span.style.fg == Some(Color::Green)));
        assert!(
            spans
                .iter()
                .any(|span| span.content.contains("// note")
                    && span.style.fg == Some(Color::DarkGray))
        );
    }
}

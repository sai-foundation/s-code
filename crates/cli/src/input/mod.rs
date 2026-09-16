use ratatui::buffer::CellWidth;
use unicode_segmentation::UnicodeSegmentation;

pub(crate) mod history;
pub(crate) mod paste;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InputBuffer {
    text: String,
    cursor: usize,
    killed: String,
    undo: Option<(String, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputLayout<'a> {
    pub(crate) rows: Vec<&'a str>,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_row: usize,
}

impl InputBuffer {
    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn trim(&self) -> &str {
        self.text.trim()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.save_undo();
        self.text.clear();
        self.cursor = 0;
    }

    pub(crate) fn take(&mut self) -> String {
        self.cursor = 0;
        self.undo = None;
        std::mem::take(&mut self.text)
    }

    pub(crate) fn insert(&mut self, character: char) {
        self.save_undo();
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    pub(crate) fn insert_str(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        self.save_undo();
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
    }

    pub(crate) fn replace(&mut self, value: &str) {
        self.save_undo();
        self.text.clear();
        self.text.push_str(value);
        self.cursor = self.text.len();
    }

    pub(crate) fn backspace(&mut self) {
        let Some(previous) = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
        else {
            return;
        };
        self.save_undo();
        self.text.drain(previous..self.cursor);
        self.cursor = previous;
    }

    pub(crate) fn delete(&mut self) {
        let Some(character) = self.text[self.cursor..].chars().next() else {
            return;
        };
        self.save_undo();
        self.text
            .drain(self.cursor..self.cursor + character.len_utf8());
    }

    pub(crate) fn move_left(&mut self) {
        if let Some(previous) = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
        {
            self.cursor = previous;
        }
    }

    pub(crate) fn move_right(&mut self) {
        if let Some(character) = self.text[self.cursor..].chars().next() {
            self.cursor += character.len_utf8();
        }
    }

    pub(crate) fn move_line_start(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
    }

    pub(crate) fn move_line_end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.cursor + index);
    }

    pub(crate) fn kill_to_line_end(&mut self) {
        let end = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.cursor + index);
        if end == self.cursor {
            return;
        }
        self.save_undo();
        self.killed = self.text[self.cursor..end].to_owned();
        self.text.drain(self.cursor..end);
    }

    pub(crate) fn kill_to_line_start(&mut self) {
        let start = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        if start == self.cursor {
            return;
        }
        self.save_undo();
        self.killed = self.text[start..self.cursor].to_owned();
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    pub(crate) fn kill_previous_word(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prefix = &self.text[..self.cursor];
        let trimmed = prefix.trim_end_matches(char::is_whitespace);
        let start = trimmed.rfind(char::is_whitespace).map_or(0, |index| {
            index + trimmed[index..].chars().next().unwrap().len_utf8()
        });
        self.save_undo();
        self.killed = self.text[start..self.cursor].to_owned();
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    pub(crate) fn paste_killed(&mut self) {
        let killed = self.killed.clone();
        self.insert_str(&killed);
    }

    pub(crate) fn undo(&mut self) {
        if let Some((text, cursor)) = self.undo.take() {
            self.text = text;
            self.cursor = cursor;
        }
    }

    pub(crate) fn layout(&self, width: u16) -> InputLayout<'_> {
        let width = usize::from(width.max(1));
        let mut rows = Vec::new();
        let mut row_start = 0;
        let mut column = 0_usize;
        let mut cursor = None;
        let cursor_position = |column: usize, row: usize| {
            debug_assert!(column <= width);
            if column == width {
                (0, row + 1)
            } else {
                (column, row)
            }
        };

        for (index, grapheme) in self.text.grapheme_indices(true) {
            let grapheme_end = index + grapheme.len();
            if grapheme.contains('\n') {
                if self.cursor >= index && self.cursor < grapheme_end {
                    cursor = Some(cursor_position(column, rows.len()));
                }
                rows.push(&self.text[row_start..index]);
                row_start = grapheme_end;
                column = 0;
                continue;
            }

            let grapheme_width = if grapheme.contains(char::is_control) {
                0
            } else {
                usize::from(grapheme.cell_width()).min(width)
            };
            if grapheme_width > 0 && column > 0 && column.saturating_add(grapheme_width) > width {
                rows.push(&self.text[row_start..index]);
                row_start = index;
                column = 0;
            }
            if index == self.cursor {
                cursor = Some(cursor_position(column, rows.len()));
            } else if self.cursor > index && self.cursor < grapheme_end {
                let prefix = &self.text[index..self.cursor];
                let prefix_width = if prefix.contains(char::is_control) {
                    0
                } else {
                    usize::from(prefix.cell_width()).min(grapheme_width)
                };
                cursor = Some(cursor_position(
                    column.saturating_add(prefix_width),
                    rows.len(),
                ));
            }
            column = column.saturating_add(grapheme_width);
        }

        if self.cursor == self.text.len() {
            cursor = Some(cursor_position(column, rows.len()));
        }
        rows.push(&self.text[row_start..]);

        let (cursor_x, cursor_row) = cursor.expect("input cursor must be on a character boundary");
        while rows.len() <= cursor_row {
            rows.push(&self.text[self.text.len()..]);
        }
        InputLayout {
            rows,
            cursor_x: u16::try_from(cursor_x).unwrap_or(u16::MAX),
            cursor_row,
        }
    }

    fn save_undo(&mut self) {
        self.undo = Some((self.text.clone(), self.cursor));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_unicode_without_splitting_code_points() {
        let mut input = InputBuffer::default();
        input.insert_str("修复🧡");
        input.move_left();
        input.backspace();
        assert_eq!(input.as_str(), "修🧡");
        input.delete();
        assert_eq!(input.as_str(), "修");
    }

    #[test]
    fn lays_out_wrapped_text_and_cursor_from_the_same_boundaries() {
        let mut input = InputBuffer::default();
        input.insert_str("ab中文");
        let layout = input.layout(5);
        assert_eq!(layout.rows, vec!["ab中", "文"]);
        assert_eq!((layout.cursor_x, layout.cursor_row), (2, 1));
    }

    #[test]
    fn layout_keeps_extended_graphemes_intact() {
        let mut input = InputBuffer::default();
        input.insert_str("a👩‍🔬x");
        let layout = input.layout(3);
        assert_eq!(layout.rows, vec!["a👩‍🔬", "x"]);
        assert_eq!((layout.cursor_x, layout.cursor_row), (1, 1));

        input.replace("a#\u{fe0f}x");
        let layout = input.layout(3);
        assert_eq!(layout.rows, vec!["a#\u{fe0f}", "x"]);
        assert_eq!((layout.cursor_x, layout.cursor_row), (1, 1));

        input.replace("a👩‍🔬");
        input.move_left();
        let layout = input.layout(3);
        assert_eq!(layout.rows, vec!["a👩‍🔬", ""]);
        assert_eq!((layout.cursor_x, layout.cursor_row), (0, 1));
    }

    #[test]
    fn layout_preserves_spaces_and_explicit_empty_lines() {
        let mut input = InputBuffer::default();
        input.insert_str("hello world\n\nnext");
        let layout = input.layout(10);
        assert_eq!(layout.rows, vec!["hello worl", "d", "", "next"]);
        assert_eq!((layout.cursor_x, layout.cursor_row), (4, 3));
    }

    #[test]
    fn exact_width_newline_does_not_create_an_extra_visual_row() {
        let mut input = InputBuffer::default();
        input.insert_str("abc\nx");
        let layout = input.layout(3);
        assert_eq!(layout.rows, vec!["abc", "x"]);
        assert_eq!((layout.cursor_x, layout.cursor_row), (1, 1));

        input.replace("abc");
        let layout = input.layout(3);
        assert_eq!(layout.rows, vec!["abc", ""]);
        assert_eq!((layout.cursor_x, layout.cursor_row), (0, 1));

        input.replace("abc\n");
        let layout = input.layout(3);
        assert_eq!(layout.rows, vec!["abc", ""]);
        assert_eq!((layout.cursor_x, layout.cursor_row), (0, 1));
    }

    #[test]
    fn cursor_at_a_soft_wrap_uses_the_next_visual_row() {
        let mut input = InputBuffer::default();
        input.insert_str("abcX");
        input.move_left();
        let layout = input.layout(3);
        assert_eq!(layout.rows, vec!["abc", "X"]);
        assert_eq!((layout.cursor_x, layout.cursor_row), (0, 1));
    }
}

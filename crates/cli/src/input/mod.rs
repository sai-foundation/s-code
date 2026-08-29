use unicode_width::UnicodeWidthChar;

pub(crate) mod history;
pub(crate) mod paste;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InputBuffer {
    text: String,
    cursor: usize,
    killed: String,
    undo: Option<(String, usize)>,
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

    pub(crate) fn cursor_position(&self, width: u16) -> (u16, u16) {
        let width = usize::from(width.max(1));
        let mut row = 0_usize;
        let mut column = 0_usize;
        for character in self.text[..self.cursor].chars() {
            if character == '\n' {
                row += 1;
                column = 0;
                continue;
            }
            let character_width = character.width().unwrap_or(0);
            if column + character_width > width {
                row += 1;
                column = 0;
            }
            column += character_width;
            if column >= width {
                row += column / width;
                column %= width;
            }
        }
        (
            u16::try_from(column).unwrap_or(u16::MAX),
            u16::try_from(row).unwrap_or(u16::MAX),
        )
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
    fn reports_wrapped_wide_character_cursor_position() {
        let mut input = InputBuffer::default();
        input.insert_str("ab中文");
        assert_eq!(input.cursor_position(5), (2, 1));
    }
}

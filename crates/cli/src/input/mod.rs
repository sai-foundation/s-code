use ratatui::buffer::CellWidth;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub(crate) mod history;
pub(crate) mod paste;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InputBuffer {
    text: String,
    cursor: usize,
    killed: String,
    undo: Option<(String, usize)>,
    mouse_selection: Option<MouseSelection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionUnit {
    Character,
    Word,
    Line,
}

impl SelectionUnit {
    fn range(self, text: &str, position: usize) -> Range<usize> {
        let position = floor_grapheme_boundary(text, position);
        match self {
            Self::Character => text[position..]
                .graphemes(true)
                .next()
                .map_or(position..position, |grapheme| {
                    position..position + grapheme.len()
                }),
            Self::Word => text
                .split_word_bound_indices()
                .map(|(start, word)| start..start + word.len())
                .find(|range| range.contains(&position))
                .unwrap_or(text.len()..text.len()),
            // Line selection follows logical newlines, never visual wrapping.
            Self::Line => {
                let start = text[..position]
                    .rfind('\n')
                    .map_or(0, |newline| newline + 1);
                let end = text[position..]
                    .find('\n')
                    .map_or(text.len(), |newline| position + newline + 1);
                start..end
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MouseSelection {
    origin: Range<usize>,
    unit: SelectionUnit,
    dragging: bool,
    viewport_top: usize,
    moved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputLayout<'a> {
    pub(crate) rows: Vec<&'a str>,
    /// Source byte range for each visual row. Soft wraps have adjacent ranges;
    /// a hard newline leaves its source bytes between consecutive ranges.
    pub(crate) row_ranges: Vec<Range<usize>>,
    pub(crate) hard_breaks: Vec<bool>,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_row: usize,
    width: u16,
}

impl InputLayout<'_> {
    /// Map a visual row and terminal cell column back to a source byte offset.
    /// Both cells occupied by a wide grapheme resolve to its leading boundary.
    pub(crate) fn position_at(&self, row: usize, column: u16) -> Option<usize> {
        let text = *self.rows.get(row)?;
        let range = self.row_ranges.get(row)?;
        let target = usize::from(column);
        let mut display_column = 0_usize;
        for (offset, grapheme) in text.grapheme_indices(true) {
            let width = grapheme_width(grapheme, usize::from(self.width));
            if display_column.saturating_add(width) > target {
                return Some(range.start + offset);
            }
            display_column = display_column.saturating_add(width);
        }
        Some(range.end)
    }
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

    #[cfg(test)]
    fn cursor(&self) -> usize {
        self.cursor
    }

    pub(crate) fn selection_range(&self) -> Option<Range<usize>> {
        let selection = self.mouse_selection.as_ref()?;
        if selection.unit == SelectionUnit::Character && !selection.moved {
            return None;
        }
        let start = selection.origin.start.min(self.cursor);
        let end = selection.origin.end.max(self.cursor);
        (start < end).then_some(start..end)
    }

    pub(crate) fn selected_text(&self) -> Option<&str> {
        self.selection_range().map(|range| &self.text[range])
    }

    pub(crate) fn mouse_selection_is_dragging(&self) -> bool {
        self.mouse_selection
            .as_ref()
            .is_some_and(|selection| selection.dragging)
    }

    #[cfg(test)]
    pub(crate) fn begin_mouse_selection(&mut self, position: usize, unit: SelectionUnit) {
        self.begin_mouse_selection_in_viewport(position, unit, 0);
    }

    pub(crate) fn begin_mouse_selection_in_viewport(
        &mut self,
        position: usize,
        unit: SelectionUnit,
        viewport_top: usize,
    ) {
        let origin = unit.range(&self.text, position);
        self.cursor = if unit == SelectionUnit::Character {
            origin.start
        } else {
            origin.end
        };
        self.mouse_selection = Some(MouseSelection {
            origin,
            unit,
            dragging: true,
            viewport_top,
            moved: false,
        });
    }

    pub(crate) fn mouse_selection_viewport_top(&self) -> Option<usize> {
        self.mouse_selection
            .as_ref()
            .map(|selection| selection.viewport_top)
    }

    pub(crate) fn set_mouse_selection_viewport_top(&mut self, top: usize) {
        if let Some(selection) = self.mouse_selection.as_mut() {
            selection.viewport_top = top;
        }
    }

    pub(crate) fn extend_mouse_selection(&mut self, position: usize) {
        let Some(selection) = self.mouse_selection.as_mut() else {
            return;
        };
        if !selection.dragging {
            return;
        }
        let target = selection.unit.range(&self.text, position);
        if selection.unit == SelectionUnit::Character && target.start == selection.origin.start {
            self.cursor = selection.origin.start;
            selection.moved = false;
            return;
        }
        self.cursor = if target.start < selection.origin.start {
            target.start
        } else {
            target.end
        };
        selection.moved = true;
    }

    pub(crate) fn end_mouse_selection(&mut self) {
        if let Some(selection) = &mut self.mouse_selection {
            selection.dragging = false;
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.mouse_selection = None;
    }

    pub(crate) fn clear(&mut self) {
        self.save_undo();
        self.text.clear();
        self.cursor = 0;
        self.clear_selection();
    }

    pub(crate) fn take(&mut self) -> String {
        self.cursor = 0;
        self.undo = None;
        self.clear_selection();
        std::mem::take(&mut self.text)
    }

    pub(crate) fn insert(&mut self, character: char) {
        let mut encoded = [0_u8; 4];
        self.insert_str(character.encode_utf8(&mut encoded));
    }

    pub(crate) fn insert_str(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        if self.replace_selection(value, false) {
            return;
        }
        self.clear_selection();
        self.save_undo();
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
    }

    pub(crate) fn replace(&mut self, value: &str) {
        self.save_undo();
        self.text.clear();
        self.text.push_str(value);
        self.cursor = self.text.len();
        self.clear_selection();
    }

    pub(crate) fn backspace(&mut self) {
        if self.replace_selection("", false) {
            return;
        }
        self.clear_selection();
        let Some(previous) = self.text[..self.cursor]
            .grapheme_indices(true)
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
        if self.replace_selection("", false) {
            return;
        }
        self.clear_selection();
        let Some(grapheme_len) = self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map(str::len)
        else {
            return;
        };
        self.save_undo();
        self.text.drain(self.cursor..self.cursor + grapheme_len);
    }

    pub(crate) fn move_left(&mut self) {
        if let Some(range) = self.selection_range() {
            self.cursor = range.start;
            self.clear_selection();
            return;
        }
        self.clear_selection();
        if let Some(previous) = self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map(|(index, _)| index)
        {
            self.cursor = previous;
        }
    }

    pub(crate) fn move_right(&mut self) {
        if let Some(range) = self.selection_range() {
            self.cursor = range.end;
            self.clear_selection();
            return;
        }
        self.clear_selection();
        if let Some(grapheme) = self.text[self.cursor..].graphemes(true).next() {
            self.cursor += grapheme.len();
        }
    }

    pub(crate) fn move_line_start(&mut self) {
        self.clear_selection();
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
    }

    pub(crate) fn move_line_end(&mut self) {
        self.clear_selection();
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.cursor + index);
    }

    pub(crate) fn kill_to_line_end(&mut self) {
        if self.replace_selection("", true) {
            return;
        }
        self.clear_selection();
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
        if self.replace_selection("", true) {
            return;
        }
        self.clear_selection();
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
        if self.replace_selection("", true) {
            return;
        }
        self.clear_selection();
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
        self.clear_selection();
        if let Some((text, cursor)) = self.undo.take() {
            self.text = text;
            self.cursor = cursor;
        }
    }

    pub(crate) fn layout(&self, width: u16) -> InputLayout<'_> {
        let layout_width = width.max(1);
        let width = usize::from(layout_width);
        let mut rows = Vec::new();
        let mut row_ranges = Vec::new();
        let mut hard_breaks = Vec::new();
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
                row_ranges.push(row_start..index);
                hard_breaks.push(true);
                row_start = grapheme_end;
                column = 0;
                continue;
            }

            let grapheme_width = grapheme_width(grapheme, width);
            if grapheme_width > 0 && column > 0 && column.saturating_add(grapheme_width) > width {
                rows.push(&self.text[row_start..index]);
                row_ranges.push(row_start..index);
                hard_breaks.push(false);
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
        row_ranges.push(row_start..self.text.len());
        hard_breaks.push(false);

        let (cursor_x, cursor_row) = cursor.expect("input cursor must be on a character boundary");
        while rows.len() <= cursor_row {
            rows.push(&self.text[self.text.len()..]);
            row_ranges.push(self.text.len()..self.text.len());
            hard_breaks.push(false);
        }
        InputLayout {
            rows,
            row_ranges,
            hard_breaks,
            cursor_x: u16::try_from(cursor_x).unwrap_or(u16::MAX),
            cursor_row,
            width: layout_width,
        }
    }

    fn replace_selection(&mut self, value: &str, capture_killed: bool) -> bool {
        let Some(range) = self.selection_range() else {
            return false;
        };
        self.save_undo();
        if capture_killed {
            self.killed = self.text[range.clone()].to_owned();
        }
        let start = range.start;
        self.text.replace_range(range, value);
        self.cursor = start + value.len();
        self.clear_selection();
        true
    }

    fn save_undo(&mut self) {
        self.undo = Some((self.text.clone(), self.cursor));
    }
}

fn floor_grapheme_boundary(text: &str, position: usize) -> usize {
    let position = position.min(text.len());
    if position == text.len() {
        return position;
    }
    text.grapheme_indices(true)
        .map(|(index, _)| index)
        .take_while(|index| *index <= position)
        .last()
        .unwrap_or(0)
}

fn grapheme_width(grapheme: &str, maximum: usize) -> usize {
    if grapheme.contains(char::is_control) {
        0
    } else {
        usize::from(grapheme.cell_width()).min(maximum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_unicode_without_splitting_graphemes() {
        let mut input = InputBuffer::default();
        input.insert_str("修复🧡");
        input.move_left();
        input.backspace();
        assert_eq!(input.as_str(), "修🧡");
        input.delete();
        assert_eq!(input.as_str(), "修");

        input.replace("ae\u{301}👩‍🔬");
        input.move_left();
        input.backspace();
        assert_eq!(input.as_str(), "a👩‍🔬");
        input.delete();
        assert_eq!(input.as_str(), "a");
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
        assert_eq!(layout.rows, vec!["a👩‍🔬"]);
        assert_eq!((layout.cursor_x, layout.cursor_row), (1, 0));
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

    #[test]
    fn layout_maps_visual_rows_and_wide_cells_to_source_offsets() {
        let mut input = InputBuffer::default();
        input.replace("abcX\n\n界");
        let layout = input.layout(3);

        assert_eq!(layout.rows, vec!["abc", "X", "", "界"]);
        assert_eq!(layout.row_ranges, vec![0..3, 3..4, 5..5, 6..9]);
        assert_eq!(layout.hard_breaks, vec![false, true, true, false]);
        assert_eq!(layout.position_at(0, 0), Some(0));
        assert_eq!(layout.position_at(0, 2), Some(2));
        assert_eq!(layout.position_at(0, 20), Some(3));
        assert_eq!(layout.position_at(2, 0), Some(5));
        assert_eq!(layout.position_at(3, 0), Some(6));
        assert_eq!(layout.position_at(3, 1), Some(6));
        assert_eq!(layout.position_at(3, 2), Some(9));
        assert_eq!(layout.position_at(4, 0), None);
    }

    #[test]
    fn hit_testing_keeps_combining_and_zwj_graphemes_atomic() {
        let mut input = InputBuffer::default();
        input.replace("a界e\u{301}👩‍🔬x");
        let layout = input.layout(6);
        let wide_start = "a".len();
        let combining_start = "a界".len();
        let emoji_start = "a界e\u{301}".len();

        assert_eq!(layout.rows, vec!["a界e\u{301}👩‍🔬", "x"]);
        assert_eq!(layout.position_at(0, 1), Some(wide_start));
        assert_eq!(layout.position_at(0, 2), Some(wide_start));
        assert_eq!(layout.position_at(0, 3), Some(combining_start));
        assert_eq!(layout.position_at(0, 4), Some(emoji_start));
        assert_eq!(layout.position_at(0, 5), Some(emoji_start));
        assert_eq!(layout.position_at(0, 6), Some(layout.row_ranges[0].end));

        input.replace("界");
        let narrow = input.layout(1);
        assert_eq!(narrow.position_at(0, 0), Some(0));
        assert_eq!(narrow.position_at(0, 1), Some("界".len()));
    }

    #[test]
    fn mouse_selection_uses_unicode_word_and_logical_line_boundaries() {
        let mut input = InputBuffer::default();
        input.replace("one two\n三 四");

        input.begin_mouse_selection(5, SelectionUnit::Word);
        assert_eq!(input.selected_text(), Some("two"));
        assert!(input.mouse_selection_is_dragging());
        input.end_mouse_selection();
        assert!(!input.mouse_selection_is_dragging());

        input.begin_mouse_selection(5, SelectionUnit::Word);
        input.extend_mouse_selection(1);
        assert_eq!(input.selected_text(), Some("one two"));

        input.begin_mouse_selection(5, SelectionUnit::Line);
        assert_eq!(input.selected_text(), Some("one two\n"));
        input.begin_mouse_selection(input.as_str().len(), SelectionUnit::Line);
        assert_eq!(input.selected_text(), Some("三 四"));
    }

    #[test]
    fn selection_preserves_hard_newlines_but_not_soft_wraps() {
        let mut input = InputBuffer::default();
        input.replace("abcX\nY");
        let (start, end) = {
            let layout = input.layout(3);
            assert_eq!(layout.rows, vec!["abc", "X", "Y"]);
            assert_eq!(layout.hard_breaks, vec![false, true, false]);
            (
                layout.position_at(0, 2).unwrap(),
                layout.position_at(2, 1).unwrap(),
            )
        };

        input.begin_mouse_selection(start, SelectionUnit::Character);
        input.extend_mouse_selection(end);
        assert_eq!(input.selected_text(), Some("cX\nY"));
    }

    #[test]
    fn typing_and_deletion_replace_mouse_selection_as_one_edit() {
        let mut input = InputBuffer::default();
        input.replace("hello");
        input.begin_mouse_selection(1, SelectionUnit::Character);
        input.extend_mouse_selection(3);
        input.insert('i');
        assert_eq!(input.as_str(), "hio");
        assert_eq!(input.cursor(), 2);
        assert_eq!(input.selection_range(), None);
        input.undo();
        assert_eq!(input.as_str(), "hello");

        input.begin_mouse_selection(1, SelectionUnit::Character);
        input.extend_mouse_selection(3);
        input.backspace();
        assert_eq!(input.as_str(), "ho");
        input.undo();
        input.begin_mouse_selection(3, SelectionUnit::Character);
        input.extend_mouse_selection(1);
        assert_eq!(input.selection_range(), Some(1..4));
        input.delete();
        assert_eq!(input.as_str(), "ho");
    }

    #[test]
    fn kill_and_paste_treat_selection_as_the_edit_target() {
        let mut input = InputBuffer::default();
        input.replace("hello world");
        input.begin_mouse_selection(1, SelectionUnit::Character);
        input.extend_mouse_selection(4);
        input.kill_to_line_end();
        assert_eq!(input.as_str(), "h world");
        assert_eq!(input.cursor(), 1);
        input.paste_killed();
        assert_eq!(input.as_str(), "hello world");
    }

    #[test]
    fn horizontal_motion_collapses_selection_to_the_nearest_edge() {
        let mut input = InputBuffer::default();
        input.replace("a👩‍🔬b");
        let emoji_end = "a👩‍🔬".len();

        input.begin_mouse_selection(1, SelectionUnit::Character);
        input.extend_mouse_selection(emoji_end);
        input.move_left();
        assert_eq!(input.cursor(), 1);
        assert_eq!(input.selection_range(), None);

        input.begin_mouse_selection(1, SelectionUnit::Character);
        input.extend_mouse_selection(emoji_end);
        input.move_right();
        assert_eq!(input.cursor(), input.as_str().len());
        assert_eq!(input.selection_range(), None);
    }

    #[test]
    fn character_drag_includes_both_cells_but_a_click_remains_a_caret() {
        let mut input = InputBuffer::default();
        input.replace("abc");

        input.begin_mouse_selection(0, SelectionUnit::Character);
        assert_eq!(input.selected_text(), None);
        input.extend_mouse_selection(1);
        assert_eq!(input.selected_text(), Some("ab"));

        input.begin_mouse_selection(1, SelectionUnit::Character);
        input.extend_mouse_selection(0);
        assert_eq!(input.selected_text(), Some("ab"));
    }
}

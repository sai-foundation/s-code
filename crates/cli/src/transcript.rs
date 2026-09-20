use ratatui::{
    buffer::CellWidth,
    layout::Alignment,
    style::{Modifier, Style},
    text::{Line, Span},
};
use s_code_protocol::Id;
use std::{
    cmp::Ordering,
    collections::{HashSet, VecDeque},
    ops::Range,
    sync::Arc,
};
use unicode_segmentation::UnicodeSegmentation;

/// Stable identity for one independently changing section of the transcript.
///
/// Server transcript items keep their durable [`Id`]. Local-only sections use
/// a kind and ordinal that are unique within one document and remain stable
/// across adjacent snapshots while that visible section persists.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TranscriptBlockKey {
    Item(Id),
    PendingInput(Id),
    PendingAttachment(Id),
    Local { kind: &'static str, ordinal: usize },
}

impl TranscriptBlockKey {
    pub(crate) const fn local(kind: &'static str, ordinal: usize) -> Self {
        Self::Local { kind, ordinal }
    }
}

#[derive(Clone, Debug)]
struct StyleRange {
    bytes: Range<usize>,
    style: Style,
}

/// One source line. Newlines between these lines are semantic; visual rows
/// introduced by [`TranscriptLayout`] are not.
#[derive(Clone, Debug)]
pub(crate) struct TranscriptLogicalLine {
    text: String,
    styles: Vec<StyleRange>,
    style: Style,
    alignment: Option<Alignment>,
}

impl TranscriptLogicalLine {
    fn style_at(&self, byte: usize) -> Style {
        self.styles
            .iter()
            .find(|range| range.bytes.contains(&byte))
            .map_or_else(Style::default, |range| range.style)
    }

    #[cfg(test)]
    fn to_line(&self) -> Line<'static> {
        let spans = self
            .styles
            .iter()
            .filter_map(|range| {
                self.text
                    .get(range.bytes.clone())
                    .map(|text| Span::styled(text.to_owned(), range.style))
            })
            .collect::<Vec<_>>();
        Line {
            style: self.style,
            alignment: self.alignment,
            spans,
        }
    }
}

impl From<Line<'static>> for TranscriptLogicalLine {
    fn from(line: Line<'static>) -> Self {
        let mut text = String::new();
        let mut styles: Vec<StyleRange> = Vec::new();
        for span in line.spans {
            let start = text.len();
            for grapheme in span.content.graphemes(true) {
                // Match Ratatui's text rendering: terminal controls do not
                // occupy cells and must not leak into copied text.
                if grapheme == "\t" || !grapheme.contains(char::is_control) {
                    text.push_str(grapheme);
                }
            }
            let end = text.len();
            if start == end {
                continue;
            }
            if let Some(previous) = styles.last_mut()
                && previous.style == span.style
                && previous.bytes.end == start
            {
                previous.bytes.end = end;
            } else {
                styles.push(StyleRange {
                    bytes: start..end,
                    style: span.style,
                });
            }
        }
        Self {
            text,
            styles,
            style: line.style,
            alignment: line.alignment,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TranscriptBlock {
    key: TranscriptBlockKey,
    lines: Vec<TranscriptLogicalLine>,
}

impl TranscriptBlock {
    pub(crate) fn new(
        key: TranscriptBlockKey,
        lines: impl IntoIterator<Item = Line<'static>>,
    ) -> Self {
        Self {
            key,
            lines: lines.into_iter().map(TranscriptLogicalLine::from).collect(),
        }
    }

    pub(crate) fn key(&self) -> &TranscriptBlockKey {
        &self.key
    }

    pub(crate) fn lines(&self) -> &[TranscriptLogicalLine] {
        &self.lines
    }
}

/// Immutable source representation of the transcript.
#[derive(Clone, Debug, Default)]
pub(crate) struct TranscriptDocument {
    blocks: Vec<TranscriptBlock>,
}

impl TranscriptDocument {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push_block(
        &mut self,
        key: TranscriptBlockKey,
        lines: impl IntoIterator<Item = Line<'static>>,
    ) {
        self.push(TranscriptBlock::new(key, lines));
    }

    pub(crate) fn push(&mut self, block: TranscriptBlock) {
        assert!(
            self.blocks.iter().all(|existing| existing.key != block.key),
            "transcript block keys must be unique within a document"
        );
        self.blocks.push(block);
    }

    pub(crate) fn blocks(&self) -> &[TranscriptBlock] {
        &self.blocks
    }

    /// Flatten the source lines without adding visual soft wraps.
    #[cfg(test)]
    pub(crate) fn logical_lines(&self) -> Vec<Line<'static>> {
        self.blocks
            .iter()
            .flat_map(|block| block.lines.iter().map(TranscriptLogicalLine::to_line))
            .collect()
    }

    pub(crate) fn logical_text(&self) -> String {
        let mut text = String::new();
        let mut first = true;
        for line in self.blocks.iter().flat_map(|block| &block.lines) {
            if !first {
                text.push('\n');
            }
            first = false;
            text.push_str(&line.text);
        }
        text
    }

    fn block_index(&self, key: &TranscriptBlockKey) -> Option<usize> {
        self.blocks.iter().position(|block| &block.key == key)
    }

    fn line(&self, anchor: &TextAnchor) -> Option<&TranscriptLogicalLine> {
        self.blocks
            .get(self.block_index(&anchor.block)?)?
            .lines
            .get(anchor.line)
    }

    fn validated_anchor(&self, anchor: &TextAnchor) -> Option<TextAnchor> {
        let line = self.line(anchor)?;
        let requested = anchor.byte.min(line.text.len());
        let byte = if requested == line.text.len() {
            requested
        } else {
            line.text
                .grapheme_indices(true)
                .map(|(start, _)| start)
                .take_while(|start| *start <= requested)
                .last()
                .unwrap_or(0)
        };
        Some(TextAnchor {
            block: anchor.block.clone(),
            line: anchor.line,
            byte,
        })
    }

    fn logical_offset(&self, anchor: &TextAnchor) -> Option<usize> {
        let anchor = self.validated_anchor(anchor)?;
        let mut offset = 0_usize;
        let mut first = true;
        for block in &self.blocks {
            for (line_index, line) in block.lines.iter().enumerate() {
                if !first {
                    offset = offset.saturating_add(1);
                }
                first = false;
                if block.key == anchor.block && line_index == anchor.line {
                    return Some(offset.saturating_add(anchor.byte));
                }
                offset = offset.saturating_add(line.text.len());
            }
        }
        None
    }

    fn compare_anchors(&self, left: &TextAnchor, right: &TextAnchor) -> Option<Ordering> {
        Some(self.logical_offset(left)?.cmp(&self.logical_offset(right)?))
    }

    fn display_with_selection(
        current: Self,
        selection: &TranscriptSelection,
    ) -> Arc<TranscriptDocument> {
        let snapshot = selection.snapshot();
        let start = snapshot.block_index(&selection.start.block).unwrap_or(0);
        let end = snapshot.block_index(&selection.end.block).unwrap_or(start);
        let pinned_through = start.max(end);
        let Some(boundary) = snapshot.blocks.get(pinned_through) else {
            return snapshot.clone();
        };
        let Some(current_boundary) = current.block_index(&boundary.key) else {
            // Volatile pending/local blocks can legitimately disappear. The
            // immutable selection still owns its copied text, but the visible
            // transcript must continue with the current document.
            return Arc::new(current);
        };

        // Keep the prefix at its snapshot positions so inserts before the
        // selection cannot move text under the pointer. From that boundary
        // onward, preserve the canonical current order.
        let mut blocks = snapshot.blocks[..=pinned_through].to_vec();
        let selected_end = if snapshot
            .compare_anchors(&selection.start, &selection.end)
            .is_some_and(Ordering::is_gt)
        {
            &selection.start
        } else {
            &selection.end
        };
        if selected_end.block == boundary.key
            && block_matches_through(boundary, &current.blocks[current_boundary], selected_end)
        {
            // Append-only streaming inside the boundary is safe because the
            // selected prefix is identical; keep its new suffix visible.
            blocks[pinned_through] = current.blocks[current_boundary].clone();
        }
        let pinned_keys = blocks
            .iter()
            .map(|block| block.key.clone())
            .collect::<HashSet<_>>();
        blocks.extend(
            current
                .blocks
                .into_iter()
                .skip(current_boundary + 1)
                .filter(|block| !pinned_keys.contains(&block.key)),
        );
        Arc::new(Self { blocks })
    }

    fn next_line_start(&self, anchor: &TextAnchor) -> Option<TextAnchor> {
        let block_index = self.block_index(&anchor.block)?;
        if anchor.line + 1 < self.blocks[block_index].lines.len() {
            return Some(TextAnchor {
                block: anchor.block.clone(),
                line: anchor.line + 1,
                byte: 0,
            });
        }
        self.blocks
            .iter()
            .skip(block_index + 1)
            .find(|block| !block.lines.is_empty())
            .map(|block| TextAnchor {
                block: block.key.clone(),
                line: 0,
                byte: 0,
            })
    }

    fn unit_range(
        &self,
        anchor: &TextAnchor,
        unit: SelectionUnit,
    ) -> Option<(TextAnchor, TextAnchor)> {
        let anchor = self.validated_anchor(anchor)?;
        let line = self.line(&anchor)?;
        let range = match unit {
            SelectionUnit::Character => line.text[anchor.byte..]
                .graphemes(true)
                .next()
                .map_or(anchor.byte..anchor.byte, |grapheme| {
                    anchor.byte..anchor.byte + grapheme.len()
                }),
            SelectionUnit::Word => line
                .text
                .split_word_bound_indices()
                .map(|(start, word)| start..start + word.len())
                .find(|range| range.contains(&anchor.byte))
                .unwrap_or(line.text.len()..line.text.len()),
            SelectionUnit::Line => 0..line.text.len(),
        };
        let start = TextAnchor {
            block: anchor.block.clone(),
            line: anchor.line,
            byte: range.start,
        };
        let end = if unit == SelectionUnit::Line {
            self.next_line_start(&anchor).unwrap_or_else(|| TextAnchor {
                block: anchor.block.clone(),
                line: anchor.line,
                byte: range.end,
            })
        } else {
            TextAnchor {
                block: anchor.block,
                line: anchor.line,
                byte: range.end,
            }
        };
        Some((start, end))
    }

    fn text_between(&self, start: &TextAnchor, end: &TextAnchor) -> Option<String> {
        let mut start = self.logical_offset(start)?;
        let mut end = self.logical_offset(end)?;
        if start > end {
            std::mem::swap(&mut start, &mut end);
        }
        if start == end {
            return None;
        }
        self.logical_text().get(start..end).map(str::to_owned)
    }
}

fn block_matches_through(
    snapshot: &TranscriptBlock,
    current: &TranscriptBlock,
    through: &TextAnchor,
) -> bool {
    if snapshot.key != current.key || snapshot.key != through.block {
        return false;
    }
    let Some(snapshot_line) = snapshot.lines.get(through.line) else {
        return false;
    };
    let Some(current_line) = current.lines.get(through.line) else {
        return false;
    };
    if current.lines.len() < through.line + 1
        || snapshot.lines[..through.line]
            .iter()
            .zip(&current.lines[..through.line])
            .any(|(snapshot, current)| snapshot.text != current.text)
    {
        return false;
    }
    snapshot_line
        .text
        .get(..through.byte)
        .is_some_and(|prefix| current_line.text.starts_with(prefix))
}

/// A source position. `byte` is measured in the visible text of one logical
/// line, never in a visual soft-wrapped row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextAnchor {
    pub(crate) block: TranscriptBlockKey,
    pub(crate) line: usize,
    pub(crate) byte: usize,
}

#[derive(Clone, Debug)]
struct SourceGrapheme {
    symbol: String,
    bytes: Range<usize>,
    width: u16,
    style: Style,
    whitespace: bool,
}

#[derive(Clone, Debug)]
struct VisualGrapheme {
    symbol: String,
    source: Range<usize>,
    global: Range<usize>,
    width: u16,
    style: Style,
}

#[derive(Clone, Debug)]
struct VisualRow {
    block: TranscriptBlockKey,
    line: usize,
    source: Range<usize>,
    global: Range<usize>,
    graphemes: Vec<VisualGrapheme>,
    first_column: u16,
    line_style: Style,
    alignment: Option<Alignment>,
    hard_break_after: bool,
}

/// Authoritative source-to-cell layout for one immutable document and width.
/// Rendering and hit testing both consume these exact visual rows.
#[derive(Clone, Debug)]
pub(crate) struct TranscriptLayout {
    document: Arc<TranscriptDocument>,
    width: u16,
    rows: Arc<[VisualRow]>,
}

impl TranscriptLayout {
    pub(crate) fn new(document: Arc<TranscriptDocument>, width: u16) -> Self {
        let width = width.max(1);
        let mut rows = Vec::new();
        let mut global_offset = 0_usize;
        let mut first_line = true;

        for block in document.blocks() {
            for (line_index, line) in block.lines().iter().enumerate() {
                if !first_line {
                    global_offset = global_offset.saturating_add(1);
                }
                first_line = false;
                let line_global_start = global_offset;
                let graphemes = source_graphemes(line);
                let wrapped = wrap_graphemes(&graphemes, width);
                let wrapped_len = wrapped.len();
                let mut previous_end = 0_usize;

                for (row_index, indices) in wrapped.into_iter().enumerate() {
                    let next_start = indices
                        .first()
                        .map_or(previous_end, |index| graphemes[*index].bytes.start);
                    let source_start = if row_index == 0 { 0 } else { previous_end };
                    let source_end = if row_index + 1 == wrapped_len {
                        line.text.len()
                    } else {
                        // The following row's first visible grapheme is the
                        // boundary after whitespace omitted by word wrapping.
                        // It is filled below once all wrapped rows are known.
                        next_start
                    };
                    let visual = indices
                        .iter()
                        .map(|index| {
                            let grapheme = &graphemes[*index];
                            VisualGrapheme {
                                symbol: grapheme.symbol.clone(),
                                source: grapheme.bytes.clone(),
                                global: line_global_start + grapheme.bytes.start
                                    ..line_global_start + grapheme.bytes.end,
                                width: grapheme.width,
                                style: grapheme.style,
                            }
                        })
                        .collect::<Vec<_>>();
                    let row_width = visual.iter().fold(0_u16, |total, grapheme| {
                        total.saturating_add(grapheme.width)
                    });
                    let first_column = alignment_offset(line.alignment, width, row_width);
                    rows.push(VisualRow {
                        block: block.key().clone(),
                        line: line_index,
                        source: source_start..source_end,
                        global: line_global_start + source_start..line_global_start + source_end,
                        graphemes: visual,
                        first_column,
                        line_style: line.style,
                        alignment: line.alignment,
                        hard_break_after: row_index + 1 == wrapped_len,
                    });
                    previous_end = source_end;
                }
                global_offset = global_offset.saturating_add(line.text.len());
            }
        }

        // The wrapping pass above cannot know the next row boundary while it
        // is emitting the current row. Repair each non-final row from the next
        // row's first visible source offset. This also accounts for Ratatui's
        // omitted wrapping whitespace.
        for index in 0..rows.len().saturating_sub(1) {
            if rows[index].hard_break_after
                || rows[index].block != rows[index + 1].block
                || rows[index].line != rows[index + 1].line
            {
                continue;
            }
            let next_start = rows[index + 1]
                .graphemes
                .first()
                .map_or(rows[index + 1].source.start, |grapheme| {
                    grapheme.source.start
                });
            let line_global_start = rows[index].global.start - rows[index].source.start;
            rows[index].source.end = next_start;
            rows[index].global.end = line_global_start + next_start;
            rows[index + 1].source.start = next_start;
            rows[index + 1].global.start = line_global_start + next_start;
        }

        Self {
            document,
            width,
            rows: rows.into(),
        }
    }

    pub(crate) fn document(&self) -> &Arc<TranscriptDocument> {
        &self.document
    }

    #[cfg(test)]
    pub(crate) const fn width(&self) -> u16 {
        self.width
    }

    pub(crate) fn height(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn max_scroll(&self, viewport_height: u16) -> usize {
        self.height().saturating_sub(usize::from(viewport_height))
    }

    /// Map a visual row/cell to a grapheme boundary. Both cells occupied by a
    /// wide grapheme resolve to the same source byte.
    pub(crate) fn position_at(&self, row: usize, column: u16) -> Option<TextAnchor> {
        let row = self.rows.get(row)?;
        let mut current = row.first_column;
        for grapheme in &row.graphemes {
            current = current.saturating_add(grapheme.width);
            if column < current {
                return Some(TextAnchor {
                    block: row.block.clone(),
                    line: row.line,
                    byte: grapheme.source.start,
                });
            }
        }
        let byte = row
            .graphemes
            .last()
            .map_or(row.source.end, |grapheme| grapheme.source.end);
        Some(TextAnchor {
            block: row.block.clone(),
            line: row.line,
            byte,
        })
    }

    /// Find the visual row containing a source anchor after any resize/reflow.
    #[cfg(test)]
    pub(crate) fn row_for_anchor(&self, anchor: &TextAnchor) -> Option<usize> {
        let offset = self.document.logical_offset(anchor)?;
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.global.start <= offset)
            .map(|(index, _)| index)
            .next_back()
    }

    pub(crate) fn rendered_lines(
        &self,
        selection: Option<&TranscriptSelection>,
    ) -> Vec<Line<'static>> {
        let selected =
            selection.and_then(|selection| selection.ordered_offsets_in(self.document.as_ref()));
        self.rows
            .iter()
            .map(|row| rendered_row(row, selected.clone()))
            .collect()
    }

    #[cfg(test)]
    fn rendered_text(&self) -> Vec<String> {
        self.rendered_lines(None)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect()
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectionUnit {
    Character,
    Word,
    Line,
}

impl SelectionUnit {
    pub(crate) const fn from_clicks(clicks: u8) -> Self {
        match clicks {
            2 => Self::Word,
            3 => Self::Line,
            _ => Self::Character,
        }
    }
}

/// Selection anchors are tied to an immutable document snapshot. Rebuilding a
/// layout at a different width changes only visual rows, never selected text.
#[derive(Clone, Debug)]
pub(crate) struct TranscriptSelection {
    snapshot: Arc<TranscriptDocument>,
    drag_layout: TranscriptLayout,
    start: TextAnchor,
    end: TextAnchor,
    origin: (TextAnchor, TextAnchor),
    unit: SelectionUnit,
    dragging: bool,
}

impl TranscriptSelection {
    #[cfg(test)]
    pub(crate) fn begin(
        snapshot: Arc<TranscriptDocument>,
        width: u16,
        row: usize,
        column: u16,
        clicks: u8,
    ) -> Option<Self> {
        let layout = TranscriptLayout::new(Arc::clone(&snapshot), width);
        Self::begin_in_layout(&layout, row, column, clicks)
    }

    pub(crate) fn begin_in_layout(
        layout: &TranscriptLayout,
        row: usize,
        column: u16,
        clicks: u8,
    ) -> Option<Self> {
        let anchor = layout.position_at(row, column)?;
        let unit = SelectionUnit::from_clicks(clicks);
        let snapshot = Arc::clone(layout.document());
        let (origin_start, origin_end) = snapshot.unit_range(&anchor, unit)?;
        let (start, end) = if unit == SelectionUnit::Character {
            (origin_start.clone(), origin_start.clone())
        } else {
            (origin_start.clone(), origin_end.clone())
        };
        Some(Self {
            snapshot,
            drag_layout: layout.clone(),
            start,
            end,
            origin: (origin_start, origin_end),
            unit,
            dragging: true,
        })
    }

    pub(crate) fn snapshot(&self) -> &Arc<TranscriptDocument> {
        &self.snapshot
    }

    #[cfg(test)]
    pub(crate) fn start(&self) -> &TextAnchor {
        &self.start
    }

    pub(crate) fn layout(&self, width: u16) -> TranscriptLayout {
        if self.drag_layout.width == width.max(1) {
            self.drag_layout.clone()
        } else {
            TranscriptLayout::new(Arc::clone(&self.snapshot), width)
        }
    }

    pub(crate) fn display_layout(
        &self,
        current: TranscriptDocument,
        width: u16,
    ) -> TranscriptLayout {
        if self.dragging {
            return self.layout(width);
        }
        TranscriptLayout::new(
            TranscriptDocument::display_with_selection(current, self),
            width,
        )
    }

    #[cfg(test)]
    pub(crate) fn extend(&mut self, width: u16, row: usize, column: u16) -> bool {
        let layout = self.layout(width);
        self.extend_in_layout(&layout, row, column)
    }

    pub(crate) fn extend_in_layout(
        &mut self,
        layout: &TranscriptLayout,
        row: usize,
        column: u16,
    ) -> bool {
        if !self.dragging || !Arc::ptr_eq(&self.snapshot, layout.document()) {
            return false;
        }
        let Some(anchor) = layout.position_at(row, column) else {
            return false;
        };
        let Some((range_start, range_end)) = self.snapshot.unit_range(&anchor, self.unit) else {
            return false;
        };
        if self.unit == SelectionUnit::Character && range_start == self.origin.0 {
            let changed = self.start != self.origin.0 || self.end != self.origin.0;
            self.start = self.origin.0.clone();
            self.end = self.origin.0.clone();
            self.dragging = true;
            return changed;
        }
        let backwards = self
            .snapshot
            .compare_anchors(&range_start, &self.origin.0)
            .is_some_and(Ordering::is_lt);
        if backwards {
            self.start = range_start;
            self.end = self.origin.1.clone();
        } else {
            self.start = self.origin.0.clone();
            self.end = range_end;
        }
        self.dragging = true;
        true
    }

    pub(crate) fn end_drag(&mut self) {
        self.dragging = false;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ordered_offsets().is_none_or(|range| range.is_empty())
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        self.snapshot.text_between(&self.start, &self.end)
    }

    fn ordered_offsets(&self) -> Option<Range<usize>> {
        self.ordered_offsets_in(self.snapshot.as_ref())
    }

    fn ordered_offsets_in(&self, document: &TranscriptDocument) -> Option<Range<usize>> {
        let mut start = document.logical_offset(&self.start)?;
        let mut end = document.logical_offset(&self.end)?;
        if start > end {
            std::mem::swap(&mut start, &mut end);
        }
        Some(start..end)
    }
}

fn source_graphemes(line: &TranscriptLogicalLine) -> Vec<SourceGrapheme> {
    line.text
        .grapheme_indices(true)
        .map(|(start, source)| {
            let (symbol, width) = if source == "\t" {
                ("    ".to_owned(), 4)
            } else {
                (source.to_owned(), source.cell_width())
            };
            SourceGrapheme {
                symbol,
                bytes: start..start + source.len(),
                width,
                style: line.style_at(start),
                whitespace: source == "\u{200b}"
                    || (source.chars().all(char::is_whitespace) && source != "\u{00a0}"),
            }
        })
        .collect()
}

/// Ratatui-compatible word wrapping with `trim: false`. Whitespace consumed at
/// a wrap boundary is deliberately absent from the visual rows but remains in
/// their source ranges, so copying never loses it.
fn wrap_graphemes(graphemes: &[SourceGrapheme], width: u16) -> Vec<Vec<usize>> {
    let mut wrapped = Vec::new();
    let mut pending_line = Vec::new();
    let mut pending_word = Vec::new();
    let mut pending_whitespace: VecDeque<usize> = VecDeque::new();
    let mut line_width = 0_u16;
    let mut word_width = 0_u16;
    let mut whitespace_width = 0_u16;
    let mut non_whitespace_previous = false;

    for (index, grapheme) in graphemes.iter().enumerate() {
        let symbol_width = grapheme.width;
        if symbol_width > width {
            continue;
        }

        let word_found = non_whitespace_previous && grapheme.whitespace;
        let untrimmed_overflow = pending_line.is_empty()
            && word_width
                .saturating_add(whitespace_width)
                .saturating_add(symbol_width)
                > width;
        if word_found || untrimmed_overflow {
            pending_line.extend(pending_whitespace.drain(..));
            line_width = line_width.saturating_add(whitespace_width);
            pending_line.append(&mut pending_word);
            line_width = line_width.saturating_add(word_width);
            whitespace_width = 0;
            word_width = 0;
        }

        let line_full = line_width >= width;
        let pending_word_overflow = symbol_width > 0
            && line_width
                .saturating_add(whitespace_width)
                .saturating_add(word_width)
                >= width;
        if line_full || pending_word_overflow {
            let mut remaining_width = width.saturating_sub(line_width);
            wrapped.push(std::mem::take(&mut pending_line));
            line_width = 0;

            while let Some(pending) = pending_whitespace.front().copied() {
                let pending_width = graphemes[pending].width;
                if pending_width > remaining_width {
                    break;
                }
                whitespace_width = whitespace_width.saturating_sub(pending_width);
                remaining_width = remaining_width.saturating_sub(pending_width);
                pending_whitespace.pop_front();
            }
            if grapheme.whitespace && pending_whitespace.is_empty() {
                continue;
            }
        }

        if grapheme.whitespace {
            whitespace_width = whitespace_width.saturating_add(symbol_width);
            pending_whitespace.push_back(index);
        } else {
            word_width = word_width.saturating_add(symbol_width);
            pending_word.push(index);
        }
        non_whitespace_previous = !grapheme.whitespace;
    }

    // `trim: false` always preserves the remaining whitespace.
    pending_line.extend(pending_whitespace);
    pending_line.append(&mut pending_word);
    if !pending_line.is_empty() {
        wrapped.push(pending_line);
    }
    if wrapped.is_empty() {
        wrapped.push(Vec::new());
    }

    // Ratatui's word wrapper can produce an over-wide row when an odd cell
    // width meets consecutive double-width graphemes. Such a row is later
    // clipped if rendered as an already-wrapped `Line`. Split it here so this
    // layout remains authoritative and never loses the rightmost grapheme.
    let mut fitted = Vec::with_capacity(wrapped.len());
    for row in wrapped {
        let mut current = Vec::new();
        let mut current_width = 0_u16;
        for index in row {
            let grapheme_width = graphemes[index].width;
            if !current.is_empty() && current_width.saturating_add(grapheme_width) > width {
                fitted.push(std::mem::take(&mut current));
                current_width = 0;
            }
            current.push(index);
            current_width = current_width.saturating_add(grapheme_width);
        }
        if !current.is_empty() {
            fitted.push(current);
        }
    }
    if fitted.is_empty() {
        fitted.push(Vec::new());
    }
    fitted
}

fn alignment_offset(alignment: Option<Alignment>, width: u16, row_width: u16) -> u16 {
    let padding = width.saturating_sub(row_width);
    match alignment.unwrap_or(Alignment::Left) {
        Alignment::Left => 0,
        Alignment::Center => padding / 2,
        Alignment::Right => padding,
    }
}

fn rendered_row(row: &VisualRow, selected: Option<Range<usize>>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for grapheme in &row.graphemes {
        let mut style = grapheme.style;
        if selected.as_ref().is_some_and(|selected| {
            grapheme.global.start < selected.end && grapheme.global.end > selected.start
        }) {
            style = style.add_modifier(Modifier::REVERSED);
        }
        if let Some(previous) = spans.last_mut()
            && previous.style == style
        {
            previous.content.to_mut().push_str(&grapheme.symbol);
        } else {
            spans.push(Span::styled(grapheme.symbol.clone(), style));
        }
    }
    Line {
        style: row.line_style,
        alignment: row.alignment,
        spans,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::Color,
        widgets::{Paragraph, Widget, Wrap},
    };

    fn key(value: &str) -> TranscriptBlockKey {
        TranscriptBlockKey::Item(Id(value.into()))
    }

    fn document(lines: &[&str]) -> Arc<TranscriptDocument> {
        let mut document = TranscriptDocument::new();
        document.push_block(
            key("item"),
            lines
                .iter()
                .map(|line| Line::from((*line).to_owned()))
                .collect::<Vec<_>>(),
        );
        Arc::new(document)
    }

    fn ratatui_rows(text: &str, width: u16) -> Vec<String> {
        let paragraph = Paragraph::new(text.to_owned()).wrap(Wrap { trim: false });
        let height = u16::try_from(paragraph.line_count(width)).unwrap();
        buffer_rows(paragraph, width, height)
    }

    fn layout_rows(layout: &TranscriptLayout) -> Vec<String> {
        let height = u16::try_from(layout.height()).unwrap();
        buffer_rows(
            Paragraph::new(layout.rendered_lines(None)),
            layout.width(),
            height,
        )
    }

    fn buffer_rows(paragraph: Paragraph<'_>, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        paragraph.render(area, &mut buffer);
        (0..height)
            .map(|row| {
                let rendered = (0..width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>();
                rendered.trim_end().to_owned()
            })
            .collect()
    }

    #[test]
    fn wraps_on_words_like_ratatui_without_trimming_source_text() {
        let layout = TranscriptLayout::new(
            document(&["AAA AAA AAAAA AA AAAAAA", " B", "  C", "   D"]),
            10,
        );
        assert_eq!(
            layout.rendered_text(),
            vec!["AAA AAA", "AAAAA AA", "AAAAAA", " B", "  C", "   D"]
        );
        assert_eq!(
            layout.document.logical_text(),
            "AAA AAA AAAAA AA AAAAAA\n B\n  C\n   D"
        );
    }

    #[test]
    fn wrapping_matches_ratatui_trim_false_for_representative_text() {
        let samples = [
            "",
            "a",
            "hello world",
            "AAA AAA AAAAA AA AAAAAA",
            "AAAAAAAAAAAAAAAAAAAA    AAA",
            "               4 Indent",
            "a  b   c",
        ];
        for text in samples {
            for width in 1..=12 {
                let actual = layout_rows(&TranscriptLayout::new(document(&[text]), width));
                assert_eq!(
                    actual,
                    ratatui_rows(text, width),
                    "{text:?} at width {width}"
                );
            }
        }
    }

    #[test]
    fn wide_graphemes_never_overflow_an_odd_width_row() {
        let layout = TranscriptLayout::new(document(&["界界界界界"]), 3);
        assert_eq!(layout.rendered_text(), vec!["界", "界", "界", "界", "界"]);
        assert_eq!(layout_rows(&layout).len(), 5);
        assert!(layout_rows(&layout).iter().all(|row| row == "界"));
    }

    #[test]
    fn preserves_ratatui_indentation_at_wrap_boundaries() {
        let layout = TranscriptLayout::new(
            document(&["AAAAAAAAAAAAAAAAAAAA    AAA", "               4 Indent"]),
            20,
        );
        assert_eq!(
            layout.rendered_text(),
            vec![
                "AAAAAAAAAAAAAAAAAAAA",
                "   AAA",
                "               4",
                "Indent"
            ]
        );
    }

    #[test]
    fn copying_preserves_soft_wrap_spaces_but_only_hard_line_newlines() {
        let snapshot = document(&["alpha beta", "gamma"]);
        let mut selection = TranscriptSelection::begin(Arc::clone(&snapshot), 5, 0, 0, 1).unwrap();
        assert!(selection.extend(5, 1, u16::MAX));
        assert_eq!(selection.selected_text().as_deref(), Some("alpha beta"));
        assert!(selection.extend(5, 2, u16::MAX));
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("alpha beta\ngamma")
        );
    }

    #[test]
    fn padding_hit_does_not_jump_across_omitted_wrapping_space() {
        let snapshot = document(&["hello world"]);
        let layout = TranscriptLayout::new(Arc::clone(&snapshot), 8);
        assert_eq!(layout.rendered_text(), vec!["hello", "world"]);
        assert_eq!(layout.position_at(0, u16::MAX).unwrap().byte, 5);

        let selection = TranscriptSelection::begin_in_layout(&layout, 0, u16::MAX, 2).unwrap();
        assert_eq!(selection.selected_text().as_deref(), Some(" "));
    }

    #[test]
    fn hit_testing_uses_extended_graphemes_and_terminal_cells() {
        let layout = TranscriptLayout::new(document(&["a界e\u{301}👩‍💻"]), 12);
        let offsets = (0..12)
            .map(|column| layout.position_at(0, column).unwrap().byte)
            .collect::<Vec<_>>();
        assert_eq!(offsets, vec![0, 1, 1, 4, 7, 7, 18, 18, 18, 18, 18, 18]);
    }

    #[test]
    fn word_and_line_units_select_semantic_source_ranges() {
        let snapshot = document(&["alpha beta", "gamma"]);
        let word = TranscriptSelection::begin(Arc::clone(&snapshot), 20, 0, 7, 2).unwrap();
        assert_eq!(word.selected_text().as_deref(), Some("beta"));

        let line = TranscriptSelection::begin(snapshot, 20, 0, 2, 3).unwrap();
        assert_eq!(line.selected_text().as_deref(), Some("alpha beta\n"));
    }

    #[test]
    fn reverse_drag_normalizes_the_selected_text() {
        let snapshot = document(&["zero one two"]);
        let mut selection = TranscriptSelection::begin(snapshot, 20, 0, u16::MAX, 1).unwrap();
        assert!(selection.extend(20, 0, 5));
        assert_eq!(selection.selected_text().as_deref(), Some("one two"));
    }

    #[test]
    fn source_anchors_survive_resize_and_reflow() {
        let snapshot = document(&["one two three four"]);
        let mut selection = TranscriptSelection::begin(Arc::clone(&snapshot), 20, 0, 0, 1).unwrap();
        assert!(selection.extend(20, 0, u16::MAX));
        let selected = selection.selected_text();

        let narrow = selection.layout(5);
        assert!(narrow.height() > 1);
        assert_eq!(selection.selected_text(), selected);
        assert_eq!(narrow.row_for_anchor(selection.start()), Some(0));
    }

    #[test]
    fn selected_prefix_is_pinned_while_later_live_blocks_keep_updating() {
        let mut snapshot_document = TranscriptDocument::new();
        snapshot_document.push_block(key("selected"), [Line::from("frozen")]);
        snapshot_document.push_block(key("later"), [Line::from("old later")]);
        let snapshot = Arc::new(snapshot_document);
        let mut selection = TranscriptSelection::begin(Arc::clone(&snapshot), 40, 0, 0, 1).unwrap();
        assert!(selection.extend(40, 0, u16::MAX));
        selection.end_drag();

        let mut current = TranscriptDocument::new();
        current.push_block(key("selected"), [Line::from("changed selected")]);
        current.push_block(key("later"), [Line::from("new later")]);
        current.push_block(key("appended"), [Line::from("new block")]);
        let layout = selection.display_layout(current, 40);

        assert_eq!(
            layout.rendered_text(),
            vec!["frozen", "new later", "new block"]
        );
        assert_eq!(selection.selected_text().as_deref(), Some("frozen"));
    }

    #[test]
    fn current_suffix_keeps_canonical_order_around_new_blocks() {
        let mut snapshot_document = TranscriptDocument::new();
        snapshot_document.push_block(key("selected"), [Line::from("selected")]);
        snapshot_document.push_block(key("answer"), [Line::from("old answer")]);
        let snapshot = Arc::new(snapshot_document);
        let mut selection = TranscriptSelection::begin(Arc::clone(&snapshot), 40, 0, 0, 1).unwrap();
        assert!(selection.extend(40, 0, u16::MAX));
        selection.end_drag();

        let mut current = TranscriptDocument::new();
        current.push_block(key("selected"), [Line::from("changed selected")]);
        current.push_block(key("tool"), [Line::from("new tool")]);
        current.push_block(key("answer"), [Line::from("new answer")]);

        assert_eq!(
            selection.display_layout(current, 40).rendered_text(),
            vec!["selected", "new tool", "new answer"]
        );
    }

    #[test]
    fn selected_streaming_block_is_frozen_while_dragging_then_shows_live_suffix() {
        let snapshot = document(&["partial"]);
        let mut selection = TranscriptSelection::begin(Arc::clone(&snapshot), 40, 0, 0, 1).unwrap();
        assert!(selection.extend(40, 0, u16::MAX));

        let mut current = TranscriptDocument::new();
        current.push_block(key("item"), [Line::from("partial response complete")]);
        let active_layout = selection.display_layout(current.clone(), 40);
        assert_eq!(active_layout.rendered_text(), vec!["partial"]);

        selection.end_drag();
        let released_layout = selection.display_layout(current, 40);
        assert_eq!(
            released_layout.rendered_text(),
            vec!["partial response complete"]
        );
        assert_eq!(selection.selected_text().as_deref(), Some("partial"));
    }

    #[test]
    fn prepended_history_is_held_outside_a_stable_selection_snapshot() {
        let snapshot = document(&["selected"]);
        let mut selection = TranscriptSelection::begin(Arc::clone(&snapshot), 40, 0, 0, 1).unwrap();
        assert!(selection.extend(40, 0, u16::MAX));
        selection.end_drag();

        let mut current = TranscriptDocument::new();
        current.push_block(key("older"), [Line::from("older history")]);
        current.push_block(key("item"), [Line::from("selected")]);

        assert_eq!(
            selection.display_layout(current, 40).rendered_text(),
            vec!["selected"]
        );
    }

    #[test]
    fn removed_selected_block_does_not_freeze_the_visible_transcript() {
        let snapshot = document(&["selected pending input"]);
        let mut selection = TranscriptSelection::begin(Arc::clone(&snapshot), 40, 0, 0, 1).unwrap();
        assert!(selection.extend(40, 0, u16::MAX));
        selection.end_drag();

        let mut current = TranscriptDocument::new();
        current.push_block(key("replacement"), [Line::from("current transcript")]);
        assert_eq!(
            selection.display_layout(current, 40).rendered_text(),
            vec!["current transcript"]
        );
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("selected pending input")
        );
    }

    #[test]
    fn tabs_render_as_cells_but_copy_as_tabs() {
        let snapshot = document(&["a\tb"]);
        let mut selection = TranscriptSelection::begin(Arc::clone(&snapshot), 20, 0, 0, 1).unwrap();
        assert!(selection.extend(20, 0, u16::MAX));
        assert_eq!(selection.selected_text().as_deref(), Some("a\tb"));
        assert_eq!(selection.layout(20).rendered_text(), vec!["a    b"]);
    }

    #[test]
    fn highlighting_uses_the_same_grapheme_ranges_as_hit_testing() {
        let mut document = TranscriptDocument::new();
        document.push_block(
            key("styled"),
            [Line::from(vec![
                Span::styled("ab", Style::default().fg(Color::Green)),
                Span::styled("界", Style::default().fg(Color::Cyan)),
            ])],
        );
        let snapshot = Arc::new(document);
        let mut selection = TranscriptSelection::begin(Arc::clone(&snapshot), 10, 0, 0, 1).unwrap();
        assert!(selection.extend(10, 0, u16::MAX));
        let layout = selection.layout(10);
        let lines = layout.rendered_lines(Some(&selection));
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0]
                .spans
                .iter()
                .all(|span| span.style.add_modifier.contains(Modifier::REVERSED))
        );
    }

    #[test]
    #[should_panic(expected = "transcript block keys must be unique")]
    fn rejects_duplicate_stable_block_keys() {
        let mut document = TranscriptDocument::new();
        document.push_block(key("duplicate"), [Line::from("first")]);
        document.push_block(key("duplicate"), [Line::from("second")]);
    }
}

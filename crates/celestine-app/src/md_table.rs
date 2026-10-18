//! GFM pipe-table detect / parse / serialize for the inline table editor.

use std::ops::Range;

/// Column alignment from the separator row (`---`, `:---`, `:---:`, `---:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    None,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdTable {
    pub headers: Vec<String>,
    pub alignments: Vec<Align>,
    pub rows: Vec<Vec<String>>,
}

impl MdTable {
    pub fn col_count(&self) -> usize {
        self.headers
            .len()
            .max(self.alignments.len())
            .max(self.rows.iter().map(|r| r.len()).max().unwrap_or(0))
            .max(1)
    }

    pub fn ensure_shape(&mut self) {
        let cols = self.col_count();
        self.headers.resize(cols, String::new());
        self.alignments.resize(cols, Align::None);
        for row in &mut self.rows {
            row.resize(cols, String::new());
        }
    }

    pub fn add_column(&mut self) {
        self.ensure_shape();
        let n = self.headers.len() + 1;
        self.headers.push(format!("Column {n}"));
        self.alignments.push(Align::None);
        for row in &mut self.rows {
            row.push(String::new());
        }
    }

    pub fn add_row(&mut self) {
        self.ensure_shape();
        let cols = self.col_count();
        self.rows.push(vec![String::new(); cols]);
    }

    pub fn remove_column(&mut self, ci: usize) {
        self.ensure_shape();
        if self.col_count() <= 1 || ci >= self.col_count() {
            return;
        }
        if ci < self.headers.len() {
            self.headers.remove(ci);
        }
        if ci < self.alignments.len() {
            self.alignments.remove(ci);
        }
        for row in &mut self.rows {
            if ci < row.len() {
                row.remove(ci);
            }
        }
    }

    pub fn remove_row(&mut self, ri: usize) {
        if ri < self.rows.len() {
            self.rows.remove(ri);
        }
    }
}

#[derive(Debug, Clone)]
pub enum Segment {
    Text { range: Range<usize> },
    Table { range: Range<usize>, table: MdTable },
}

/// Split a note buffer into text and GFM table segments (byte ranges).
pub fn segment_note(buf: &str) -> Vec<Segment> {
    let fences = fence_ranges(buf);
    let lines: Vec<(usize, usize, usize)> = line_spans(buf).collect();
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut i = 0usize;

    while i < lines.len() {
        let (start, end, _next) = lines[i];
        if in_ranges(&fences, start) {
            i += 1;
            continue;
        }
        if i + 1 >= lines.len() {
            break;
        }
        let Some(headers) = parse_row(&buf[start..end]) else {
            i += 1;
            continue;
        };
        let (s_start, s_end, s_next) = lines[i + 1];
        if in_ranges(&fences, s_start) {
            i += 1;
            continue;
        }
        let Some(sep_cells) = parse_row(&buf[s_start..s_end]) else {
            i += 1;
            continue;
        };
        if !is_separator_row(&sep_cells) {
            i += 1;
            continue;
        }

        let alignments: Vec<Align> = sep_cells.iter().map(|c| parse_align(c)).collect();
        let mut rows = Vec::new();
        let mut table_end = s_next;
        let mut j = i + 2;
        while j < lines.len() {
            let (r_start, r_end, r_next) = lines[j];
            if in_ranges(&fences, r_start) {
                break;
            }
            let line = &buf[r_start..r_end];
            if line.trim().is_empty() {
                break;
            }
            let Some(cells) = parse_row(line) else {
                break;
            };
            rows.push(cells);
            table_end = r_next;
            j += 1;
        }

        // Require at least header + separator (body rows optional).
        if cursor < start {
            out.push(Segment::Text {
                range: cursor..start,
            });
        }
        let mut table = MdTable {
            headers,
            alignments,
            rows,
        };
        table.ensure_shape();
        out.push(Segment::Table {
            range: start..table_end,
            table,
        });
        cursor = table_end;
        i = j;
    }

    if cursor < buf.len() {
        out.push(Segment::Text {
            range: cursor..buf.len(),
        });
    } else if out.is_empty() {
        out.push(Segment::Text {
            range: 0..buf.len(),
        });
    }
    out
}

/// Serialize a table to GFM pipe Markdown (trailing newline).
pub fn serialize(table: &MdTable) -> String {
    let mut t = table.clone();
    t.ensure_shape();
    let cols = t.col_count();
    let mut widths: Vec<usize> = vec![3; cols];
    for (i, h) in t.headers.iter().enumerate() {
        widths[i] = widths[i].max(display_width(h)).max(3);
    }
    for row in &t.rows {
        for (i, cell) in row.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(display_width(cell)).max(3);
        }
    }

    let mut out = String::new();
    write_row(&mut out, &t.headers, &widths);
    out.push('\n');
    write_sep(&mut out, &t.alignments, &widths);
    out.push('\n');
    for row in &t.rows {
        write_row(&mut out, row, &widths);
        out.push('\n');
    }
    out
}

fn display_width(s: &str) -> usize {
    s.chars().count()
}

fn write_row(out: &mut String, cells: &[String], widths: &[usize]) {
    out.push('|');
    for (i, w) in widths.iter().enumerate() {
        let cell = cells.get(i).map(|s| s.as_str()).unwrap_or("");
        out.push(' ');
        out.push_str(cell);
        let pad = w.saturating_sub(display_width(cell));
        for _ in 0..pad {
            out.push(' ');
        }
        out.push(' ');
        out.push('|');
    }
}

fn write_sep(out: &mut String, aligns: &[Align], widths: &[usize]) {
    out.push('|');
    for (i, w) in widths.iter().enumerate() {
        let align = aligns.get(i).copied().unwrap_or(Align::None);
        out.push(' ');
        match align {
            Align::Left | Align::Center => out.push(':'),
            Align::None | Align::Right => {}
        }
        let dashes = (*w).max(3);
        for _ in 0..dashes {
            out.push('-');
        }
        match align {
            Align::Right | Align::Center => out.push(':'),
            Align::None | Align::Left => {}
        }
        out.push(' ');
        out.push('|');
    }
}

fn parse_row(line: &str) -> Option<Vec<String>> {
    let line = line.trim_end_matches(['\n', '\r']);
    let trimmed = line.trim();
    if trimmed.is_empty() || !trimmed.contains('|') {
        return None;
    }
    // Reject bare horizontal rules mistaken for rows.
    if trimmed.chars().all(|c| c == '-' || c == '|' || c == ':' || c == ' ')
        && !trimmed.contains("---")
        && trimmed.matches('-').count() < 3
    {
        // still allow separator detection via is_separator_row
    }
    let mut cells: Vec<String> = trimmed.split('|').map(|s| s.trim().to_string()).collect();
    if trimmed.starts_with('|') {
        cells.remove(0);
    }
    if trimmed.ends_with('|') && !cells.is_empty() {
        cells.pop();
    }
    if cells.is_empty() {
        return None;
    }
    // A single cell without pipes on both sides that is just words is not a table row
    // unless the line actually used `|`.
    Some(cells)
}

fn is_separator_row(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim();
            if c.is_empty() {
                return false;
            }
            let b = c.as_bytes();
            let mut i = 0usize;
            if b.get(i) == Some(&b':') {
                i += 1;
            }
            let dash_start = i;
            while i < b.len() && b[i] == b'-' {
                i += 1;
            }
            if i - dash_start < 3 {
                return false;
            }
            if b.get(i) == Some(&b':') {
                i += 1;
            }
            i == b.len()
        })
}

fn parse_align(cell: &str) -> Align {
    let c = cell.trim();
    let left = c.starts_with(':');
    let right = c.ends_with(':');
    match (left, right) {
        (true, true) => Align::Center,
        (true, false) => Align::Left,
        (false, true) => Align::Right,
        (false, false) => Align::None,
    }
}

fn line_spans(s: &str) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
    let mut start = 0usize;
    let bytes = s.as_bytes();
    std::iter::from_fn(move || {
        if start > bytes.len() {
            return None;
        }
        if start == bytes.len() {
            // Allow a final empty line only when the previous ended with newline —
            // we stop when start is past end.
            return None;
        }
        let mut i = start;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        let end = i;
        let next = if i < bytes.len() { i + 1 } else { i };
        let item = (start, end, next);
        start = if next == end && end == bytes.len() {
            bytes.len() + 1
        } else {
            next
        };
        Some(item)
    })
}

fn fence_ranges(body: &str) -> Vec<(usize, usize)> {
    let mut fence: Option<(u8, usize, usize)> = None;
    let mut fenced = Vec::new();
    for (start, end, next) in line_spans(body) {
        let line = &body[start..end];
        let stripped = line.trim_start_matches(' ');
        if line.len() - stripped.len() > 3 {
            continue;
        }
        let first = stripped.as_bytes().first().copied();
        let Some(c @ (b'`' | b'~')) = first else {
            continue;
        };
        let run = stripped.bytes().take_while(|b| *b == c).count();
        if run < 3 {
            continue;
        }
        match fence {
            None => {
                if c == b'`' && stripped[run..].contains('`') {
                    continue;
                }
                fence = Some((c, run, start));
            }
            Some((fc, flen, fstart))
                if fc == c && run >= flen && stripped[run..].trim().is_empty() =>
            {
                fenced.push((fstart, next));
                fence = None;
            }
            Some(_) => {}
        }
    }
    if let Some((_, _, fstart)) = fence {
        fenced.push((fstart, body.len()));
    }
    fenced
}

fn in_ranges(ranges: &[(usize, usize)], pos: usize) -> bool {
    ranges.iter().any(|&(a, b)| pos >= a && pos < b)
}

/// Byte offset → char offset in `text`.
pub fn byte_to_char(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].chars().count()
}

/// Char offset → byte offset in `text`.
pub fn char_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

/// Number of source lines covered by a byte range (at least 1 if non-empty).
pub fn line_count_in_range(text: &str, range: &Range<usize>) -> usize {
    let slice = &text[range.start.min(text.len())..range.end.min(text.len())];
    if slice.is_empty() {
        return 0;
    }
    let n = slice.bytes().filter(|&b| b == b'\n').count();
    if slice.ends_with('\n') {
        n
    } else {
        n + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_simple() {
        let src = "| A | B |\n| --- | --- |\n| 1 | 2 |\n";
        let segs = segment_note(src);
        assert_eq!(segs.len(), 1);
        let Segment::Table { table, .. } = &segs[0] else {
            panic!("expected table");
        };
        assert_eq!(table.headers, vec!["A", "B"]);
        assert_eq!(
            table.rows,
            vec![vec!["1".to_string(), "2".to_string()]]
        );
        let out = serialize(table);
        let segs2 = segment_note(&out);
        let Segment::Table { table: t2, .. } = &segs2[0] else {
            panic!("re-parse");
        };
        assert_eq!(t2.headers, table.headers);
        assert_eq!(t2.rows, table.rows);
    }

    #[test]
    fn alignments() {
        let src = "| L | C | R |\n| :--- | :---: | ---: |\n| a | b | c |\n";
        let segs = segment_note(src);
        let Segment::Table { table, .. } = &segs[0] else {
            panic!("table");
        };
        assert_eq!(
            table.alignments,
            vec![Align::Left, Align::Center, Align::Right]
        );
    }

    #[test]
    fn incomplete_stays_text() {
        let src = "| A | B |\n| 1 | 2 |\n";
        let segs = segment_note(src);
        assert!(matches!(segs.as_slice(), [Segment::Text { .. }]));
    }

    #[test]
    fn fenced_code_ignored() {
        let src = "```\n| A | B |\n| --- | --- |\n| 1 | 2 |\n```\n";
        let segs = segment_note(src);
        assert!(matches!(segs.as_slice(), [Segment::Text { .. }]));
    }

    #[test]
    fn text_around_table() {
        let src = "before\n\n| H |\n| --- |\n| v |\n\nafter\n";
        let segs = segment_note(src);
        assert_eq!(segs.len(), 3);
        assert!(matches!(segs[0], Segment::Text { .. }));
        assert!(matches!(segs[1], Segment::Table { .. }));
        assert!(matches!(segs[2], Segment::Text { .. }));
    }

    #[test]
    fn header_only_table() {
        let src = "| A | B |\n| --- | --- |\n";
        let segs = segment_note(src);
        let Segment::Table { table, .. } = &segs[0] else {
            panic!("table");
        };
        assert!(table.rows.is_empty());
        assert_eq!(table.headers.len(), 2);
    }
}

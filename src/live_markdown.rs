use std::{collections::BTreeMap, ops::Range};

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Style {
    pub size: i32,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub accent: bool,
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Marker {
    Bullet,
    Number(u64),
    Task(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: String,
    pub source: i32,
    pub align: Alignment,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableRow {
    pub columns: usize,
    pub header: bool,
    pub cells: Vec<Cell>,
}

#[derive(Default, Debug, PartialEq)]
pub struct Block {
    pub base: Style,
    pub quote: i32,
    pub list: i32,
    pub marker: Option<Marker>,
    pub rule: bool,
    pub separator: bool,
    pub table: Option<TableRow>,
}

#[derive(Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub start: i32,
    pub end: i32,
    pub runs: Vec<(Range<i32>, Style)>,
    pub block: Block,
}

impl Block {
    pub fn drawn(&self) -> bool {
        self.table.is_some() || self.marker.is_some() || self.quote != 0 || self.rule
    }
}

impl Line {
    pub fn formatted(&self) -> bool {
        !self.runs.is_empty() || self.block != Block::default()
    }
}

pub fn lines(source: &str) -> Vec<Line> {
    let mut spans = Vec::new();
    let mut blocks = BTreeMap::<usize, Block>::new();
    let line_start = |at: usize| source[..at].rfind('\n').map_or(0, |newline| newline + 1);
    let mut numbering = Vec::<Option<u64>>::new();
    let mut alignments = Vec::new();
    let mut row: Option<usize> = None;
    let mut cell: Option<Cell> = None;
    let mut add = |range: Range<usize>, style: Style| {
        if !range.is_empty() {
            spans.push((range, style));
        }
    };
    let hidden = Style {
        hidden: true,
        ..Style::default()
    };
    let parser = Parser::new_ext(
        source,
        Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TABLES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_GFM,
    )
    .into_offset_iter();
    for (event, range) in parser {
        let token = &source[range.clone()];
        match &event {
            Event::Start(Tag::Table(columns)) => alignments.clone_from(columns),
            Event::End(TagEnd::Table) => alignments.clear(),
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                let start_of_line = line_start(range.start);
                let header = matches!(event, Event::Start(Tag::TableHead));
                let block = blocks.entry(start_of_line).or_default();
                block.table = Some(TableRow {
                    columns: alignments.len(),
                    header,
                    cells: Vec::new(),
                });
                let line = source[start_of_line..]
                    .split('\n')
                    .next()
                    .unwrap_or_default();
                add(start_of_line..start_of_line + line.len(), hidden);
                row = Some(start_of_line);
                if header && let Some(next) = source.get(start_of_line + line.len() + 1..) {
                    let separator = next.split('\n').next().unwrap_or_default();
                    let at = start_of_line + line.len() + 1;
                    blocks.entry(at).or_default().separator = true;
                    add(at..at + separator.len(), hidden);
                }
            }
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => row = None,
            Event::Start(Tag::TableCell) => {
                let start_of_line = line_start(range.start);
                if let Some(table) = row
                    .and_then(|r| blocks.get(&r))
                    .and_then(|b| b.table.as_ref())
                {
                    let spaces = token.len() - token.trim_start_matches([' ', '\t']).len();
                    cell = Some(Cell {
                        text: String::new(),
                        source: source[start_of_line..range.start + spaces]
                            .encode_utf16()
                            .count() as i32,
                        align: alignments
                            .get(table.cells.len())
                            .copied()
                            .unwrap_or(Alignment::None),
                    });
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let (Some(start), Some(cell)) = (row, cell.take())
                    && let Some(table) = blocks.get_mut(&start).and_then(|b| b.table.as_mut())
                {
                    table.cells.push(cell);
                }
            }
            Event::Text(text) | Event::Code(text) if cell.is_some() => {
                if let Some(cell) = &mut cell {
                    cell.text.push_str(text);
                }
            }
            Event::SoftBreak | Event::HardBreak if cell.is_some() => {
                if let Some(cell) = &mut cell {
                    cell.text.push(' ');
                }
            }
            Event::Start(Tag::List(start)) => numbering.push(*start),
            Event::End(TagEnd::List(_)) => {
                numbering.pop();
            }
            Event::Start(Tag::Item) => {
                let start_of_line = line_start(range.start);
                let level = numbering.len() as i32;
                for row_start in physical_lines(source, line_start(range.start), range.end) {
                    blocks.entry(row_start).or_default().list = level;
                }
                let marker = numbering
                    .last_mut()
                    .and_then(|number| number.as_mut())
                    .map_or(Marker::Bullet, |number| {
                        let marker = Marker::Number(*number);
                        *number = number.saturating_add(1);
                        marker
                    });
                blocks.entry(start_of_line).or_default().marker = Some(marker);
                let marker_end = token.find([' ', '\t', '\n']).unwrap_or(token.len());
                let tail = token[marker_end..].split('\n').next().unwrap_or_default();
                let end = range.start + marker_end + tail.len()
                    - tail.trim_start_matches([' ', '\t']).len();
                add(range.start..end, hidden);
            }
            Event::TaskListMarker(checked) => {
                blocks.entry(line_start(range.start)).or_default().marker =
                    Some(Marker::Task(*checked));
                let following = source[range.end..]
                    .bytes()
                    .take_while(|&b| matches!(b, b' ' | b'\t'))
                    .count();
                add(range.start..range.end + following, hidden);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                for row_start in physical_lines(source, line_start(range.start), range.end) {
                    let block = blocks.entry(row_start).or_default();
                    block.quote += 1;
                    let line = source[row_start..].split('\n').next().unwrap_or_default();
                    let mut tail = line.trim_start_matches([' ', '\t']);
                    for _ in 0..block.quote {
                        let Some(rest) = tail.strip_prefix('>') else {
                            break;
                        };
                        tail = rest.trim_start_matches([' ', '\t']);
                    }
                    add(row_start..row_start + line.len() - tail.len(), hidden);
                }
            }
            Event::Rule => {
                blocks.entry(line_start(range.start)).or_default().rule = true;
                add(range.clone(), hidden);
            }
            Event::FootnoteReference(_) if token.starts_with("[^") && token.ends_with(']') => {
                add(range.start..range.start + 2, hidden);
                add(range.end - 1..range.end, hidden);
                add(
                    range.start + 2..range.end - 1,
                    Style {
                        size: 180,
                        accent: true,
                        ..Style::default()
                    },
                );
            }
            Event::Start(Tag::FootnoteDefinition(_)) => {
                if let Some(end) = token.find("]:") {
                    add(
                        range.start..range.start + end + 2,
                        Style {
                            accent: true,
                            bold: true,
                            ..Style::default()
                        },
                    );
                }
            }
            _ => {}
        }
        let mark = |count| (range.start + count)..(range.end - count);
        let inline = match event {
            Event::Start(Tag::Strong) => Some((
                2,
                Style {
                    bold: true,
                    ..Style::default()
                },
            )),
            Event::Start(Tag::Emphasis) => Some((
                1,
                Style {
                    italic: true,
                    ..Style::default()
                },
            )),
            Event::Start(Tag::Strikethrough) => Some((
                token.bytes().take_while(|&b| b == b'~').count().min(2),
                Style {
                    strike: true,
                    ..Style::default()
                },
            )),
            Event::Code(_) => Some((
                token.bytes().take_while(|&b| b == b'\x60').count(),
                Style {
                    code: true,
                    ..Style::default()
                },
            )),
            _ => None,
        };
        if let Some((count, style)) = inline {
            if token.len() > 2 * count {
                let inner = mark(count);
                add(range.start..inner.start, hidden);
                add(inner.clone(), style);
                add(inner.end..range.end, hidden);
            }
        } else {
            match event {
                Event::Start(Tag::Heading { level, .. }) if token.trim_start().starts_with('#') => {
                    let first = token.split('\n').next().unwrap_or(token);
                    let content = first.trim_start_matches([' ', '\t']);
                    let prefix = content
                        .trim_start_matches('#')
                        .trim_start_matches([' ', '\t']);
                    let start = range.start + first.len() - prefix.len();
                    let end = range.start + first.len();
                    add(range.start..start, hidden);
                    let size = [500, 380, 320, 280, 250, 230][level as usize - 1];
                    blocks.entry(line_start(range.start)).or_default().base = Style {
                        size,
                        bold: true,
                        ..Style::default()
                    };
                    add(
                        start..end,
                        Style {
                            size,
                            bold: true,
                            ..Style::default()
                        },
                    );
                    let trimmed = prefix.trim_end_matches([' ', '\t']);
                    let body = trimmed.trim_end_matches('#');
                    if body.len() < trimmed.len()
                        && (body.is_empty() || body.ends_with([' ', '\t']))
                    {
                        add(
                            (start + body.trim_end_matches([' ', '\t']).len())..end,
                            hidden,
                        );
                    }
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    let size = if level as usize == 1 { 500 } else { 380 };
                    if let Some((body, underline)) = token.trim_end_matches('\n').rsplit_once('\n')
                    {
                        for at in physical_lines(
                            source,
                            line_start(range.start),
                            range.start + body.len(),
                        ) {
                            blocks.entry(at).or_default().base = Style {
                                size,
                                bold: true,
                                ..Style::default()
                            };
                        }
                        add(
                            range.start..range.start + body.len(),
                            Style {
                                size,
                                bold: true,
                                ..Style::default()
                            },
                        );
                        let at = range.start + body.len() + 1;
                        blocks.entry(at).or_default().separator = true;
                        add(at..at + underline.len(), hidden);
                    }
                }
                Event::Start(Tag::Link { .. }) if token.starts_with('[') => {
                    if let Some(close) = token.rfind("](").or_else(|| token.find("][")) {
                        add(range.start..range.start + 1, hidden);
                        add(
                            range.start + 1..range.start + close,
                            Style {
                                accent: true,
                                ..Style::default()
                            },
                        );
                        add(range.start + close..range.end, hidden);
                    }
                }
                Event::Start(Tag::Link { .. })
                    if token.starts_with('<') && token.ends_with('>') =>
                {
                    add(range.start..range.start + 1, hidden);
                    add(range.end - 1..range.end, hidden);
                    add(
                        range.start + 1..range.end - 1,
                        Style {
                            accent: true,
                            ..Style::default()
                        },
                    );
                }
                Event::Start(Tag::Image { .. }) if token.starts_with("![") => {
                    if let Some(close) = token.rfind("](").or_else(|| token.find("][")) {
                        add(range.start..range.start + 2, hidden);
                        add(
                            range.start + 2..range.start + close,
                            Style {
                                accent: true,
                                italic: true,
                                ..Style::default()
                            },
                        );
                        add(range.start + close..range.end, hidden);
                    }
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    for at in physical_lines(source, line_start(range.start), range.end) {
                        blocks.entry(at).or_default().base = Style {
                            code: true,
                            ..Style::default()
                        };
                    }
                    if let CodeBlockKind::Fenced(_) = kind {
                        let fence = token.as_bytes().first().copied();
                        let first = token.split('\n').next().unwrap_or_default();
                        add(range.start..range.start + first.len(), hidden);
                        if let Some((body, last)) = token.trim_end_matches('\n').rsplit_once('\n')
                            && last.trim().len() >= 3
                            && last.trim().bytes().all(|b| Some(b) == fence)
                        {
                            let at = range.start + body.len() + 1;
                            add(at..at + last.len(), hidden);
                        }
                    }
                    add(
                        range,
                        Style {
                            code: true,
                            ..Style::default()
                        },
                    );
                }
                _ => {}
            }
        }
    }
    spans.sort_by_key(|(range, _)| range.start);
    let mut pending = spans.iter().peekable();
    let mut overlapping = Vec::<&(Range<usize>, Style)>::new();
    let mut boundaries = Vec::new();
    let mut active = Vec::<&(Range<usize>, Style)>::new();
    let (mut byte, mut start) = (0, 0);
    source
        .split('\n')
        .map(|text| {
            let end_byte = byte + text.len();
            overlapping.retain(|(range, _)| range.end > byte);
            while pending
                .peek()
                .is_some_and(|(range, _)| range.start < end_byte)
            {
                if let Some(span) = pending.next().filter(|(range, _)| range.end > byte) {
                    overlapping.push(span);
                }
            }

            boundaries.clear();
            boundaries.extend([byte, end_byte]);
            for (range, _) in &overlapping {
                boundaries.extend([range.start.max(byte), range.end.min(end_byte)]);
            }
            boundaries.sort_unstable();
            boundaries.dedup();
            let mut runs = Vec::new();
            let mut offset = 0;
            let mut next_span = overlapping.iter().copied().peekable();
            active.clear();
            for part in boundaries.windows(2) {
                let at = part[0];
                let width = source[at..part[1]].encode_utf16().count() as i32;
                active.retain(|(range, _)| range.end > at);
                while next_span.peek().is_some_and(|(range, _)| range.start <= at) {
                    if let Some(span) = next_span.next().filter(|(range, _)| range.end > at) {
                        active.push(span);
                    }
                }
                let mut style = Style::default();
                for (_, other) in &active {
                    style.size = style.size.max(other.size);
                    style.bold |= other.bold;
                    style.italic |= other.italic;
                    style.strike |= other.strike;
                    style.code |= other.code;
                    style.accent |= other.accent;
                    style.hidden |= other.hidden;
                }
                if style != Style::default() {
                    runs.push((offset..offset + width, style));
                }
                offset += width;
            }
            let end = start + offset;
            let line = Line {
                text: text.into(),
                start,
                end,
                runs,
                block: blocks.remove(&byte).unwrap_or_default(),
            };
            byte = end_byte + 1;
            start = end + 1;
            line
        })
        .collect()
}

fn physical_lines(source: &str, start: usize, end: usize) -> impl Iterator<Item = usize> + '_ {
    std::iter::successors((start < end).then_some(start), move |&position| {
        let next = position + source[position..end].find('\n')? + 1;
        (next < end).then_some(next)
    })
}

pub fn line_at(lines: &[Line], caret: i32) -> usize {
    lines
        .partition_point(|line| line.start <= caret)
        .saturating_sub(1)
}

pub fn same_style(a: &Line, b: &Line) -> bool {
    a.text == b.text && a.runs == b.runs && a.block == b.block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_inline_styles_keep_utf16_offsets_and_nested_formatting() {
        let input = "**bold _🦀_** ".repeat(2000);
        let document = lines(&input);
        let line = &document[0];
        assert_eq!(line.end, input.encode_utf16().count() as i32);
        let nested: Vec<_> = line
            .runs
            .iter()
            .filter(|(_, style)| style.bold && style.italic && !style.hidden)
            .collect();
        assert_eq!(nested.len(), 2000);
        let units: Vec<_> = input.encode_utf16().collect();
        for (range, _) in nested {
            assert_eq!(
                String::from_utf16_lossy(&units[range.start as usize..range.end as usize]),
                "🦀"
            );
        }
    }

    #[test]
    fn understands_tables_and_lists_without_changing_source_offsets() {
        let source = "Title\n=====\n\n| Key | Value |\n| :-- | --: |\n| 🦀 | **ready** |\n\n- first\n  - [x] nested\n\n7. numbered\n8. next\n\n> quote\n\n---\n";
        let rows = lines(source);
        assert_eq!(rows[0].block.base.size, 500);
        assert!(rows[1].block.separator);
        let ruler = 4;
        assert!(rows[ruler].block.separator);
        let header = rows[3].block.table.as_ref().unwrap();
        assert!(header.header);
        assert_eq!(header.columns, 2);
        let body = rows[5].block.table.as_ref().unwrap();
        assert_eq!(body.cells[0].text, "🦀");
        assert_eq!(body.cells[1].text, "ready");
        assert_eq!(body.cells[1].source, 7);
        assert_eq!(body.cells[1].align, Alignment::Right);
        assert_eq!(rows[7].block.marker, Some(Marker::Bullet));
        assert_eq!(rows[8].block.marker, Some(Marker::Task(true)));
        assert_eq!(rows[8].block.list, 2);
        assert_eq!(rows[10].block.marker, Some(Marker::Number(7)));
        assert_eq!(rows[11].block.marker, Some(Marker::Number(8)));
        assert_eq!(rows[13].block.quote, 1);
        assert!(rows[15].block.rule);
    }

    #[test]
    fn markup_ranges_are_utf16_and_code_is_not_parsed_as_emphasis() {
        let rows = lines("# 🦀 **Ж**\n\n~~~md\n**plain**\n~~~\n");
        assert_eq!(rows[0].end, 10);
        assert_eq!(
            rows[0]
                .runs
                .iter()
                .filter(|(_, style)| style.hidden)
                .map(|(range, _)| range.clone())
                .collect::<Vec<_>>(),
            [0..2, 5..7, 8..10]
        );
        assert!(rows[3].runs[0].1.code);
        assert!(!rows[3].runs[0].1.hidden);
        assert_eq!(line_at(&rows, rows[3].start), 3);
    }
}

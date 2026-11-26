use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
};

use html5gum::{Token, Tokenizer, emitters::default::StartTag};

use crate::document::{Document, ReadingHtml};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    fn colors(self) -> [(u8, u8, u8); 6] {
        match self {
            Self::Light => [
                (56, 34, 24),
                (114, 89, 74),
                (155, 69, 28),
                (248, 237, 226),
                (244, 223, 206),
                (232, 212, 196),
            ],
            Self::Dark => [
                (247, 232, 217),
                (196, 171, 152),
                (255, 168, 117),
                (55, 40, 32),
                (73, 48, 36),
                (80, 56, 44),
            ],
        }
    }
}

pub struct Rendered {
    pub rtf: String,
    pub links: Vec<Link>,
    pub anchors: Vec<Anchor>,
    pub urls: Vec<String>,
    pub width_sensitive: bool,
}

#[derive(Debug, Eq, PartialEq)]
struct Heading {
    text: String,
    position: i32,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Anchor {
    pub id: String,
    pub position: i32,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Link {
    pub start: i32,
    pub end: i32,
    pub target: LinkTarget,
}

#[derive(Debug, Eq, PartialEq)]
pub enum LinkTarget {
    Document(usize),
    Anchor(String),
}

pub fn render(document: &Document, source: &str, theme: Theme, width_twips: i32) -> Rendered {
    render_html(ReadingHtml::new(document, source), theme, width_twips)
}

fn render_html(document: ReadingHtml<'_>, theme: Theme, width_twips: i32) -> Rendered {
    let mut renderer = Renderer {
        document: &document,
        width: width_twips.max(2_400),
        rtf: String::with_capacity(document.html.len() + 250),
        position: 0,
        paragraph: None,
        spans: Vec::new(),
        lists: Vec::new(),
        list_prefix: None,
        quotes: 0,
        in_footnote: false,
        in_table: false,
        pending_space: false,
        has_text: false,
        heading: None,
        links: Vec::new(),
        anchors: Vec::new(),
        occupied_ids: HashSet::new(),
        heading_suffix: HashMap::new(),
        image_budget: 8 * 1024 * 1024,
    };
    renderer.rtf.push_str(
        r"{\rtf1\ansi\ansicpg1252\deff0\uc1{\fonttbl{\f0\fnil Segoe UI;}{\f1\fmodern Consolas;}}{\colortbl;",
    );
    for (red, green, blue) in theme.colors() {
        let _ = write!(renderer.rtf, r"\red{red}\green{green}\blue{blue};");
    }
    renderer.rtf.push('}');

    let mut tokens = Tokenizer::new(document.html.as_str()).flatten();
    while let Some(token) = tokens.next() {
        if matches!(&token, Token::StartTag(tag) if tag.name.as_ref() == b"table") {
            renderer.table(read_table(&mut tokens));
        } else {
            renderer.token(token);
        }
    }
    renderer.end_paragraph();
    renderer.rtf.push('}');
    Rendered {
        rtf: renderer.rtf,
        links: renderer.links,
        anchors: renderer.anchors,
        urls: document.links,
        width_sensitive: document.html.contains("<table") || document.html.contains("<img"),
    }
}

struct Renderer<'a> {
    document: &'a ReadingHtml<'a>,
    width: i32,
    rtf: String,
    position: i32,
    paragraph: Option<Paragraph>,
    spans: Vec<OpenSpan>,
    lists: Vec<List>,
    list_prefix: Option<Option<u64>>,
    quotes: i32,
    in_footnote: bool,
    in_table: bool,
    pending_space: bool,
    has_text: bool,
    heading: Option<Heading>,
    links: Vec<Link>,
    anchors: Vec<Anchor>,
    occupied_ids: HashSet<String>,
    heading_suffix: HashMap<String, usize>,
    image_budget: usize,
}

#[derive(Clone, Copy)]
enum Paragraph {
    Text,
    Heading(u8),
    Code,
    ListItem,
    Rule,
    TableRow,
}

struct OpenSpan {
    name: &'static [u8],
    link: Option<(i32, LinkTarget)>,
}

enum List {
    Ordered(u64),
    Unordered,
}

impl Renderer<'_> {
    fn token(&mut self, token: Token) {
        match token {
            Token::StartTag(tag) => self.start(&tag),
            Token::EndTag(tag) => self.end(&tag.name),
            Token::String(text) => {
                let text = String::from_utf8_lossy(&text.value);
                if matches!(self.paragraph, Some(Paragraph::Code)) {
                    for line in text.split_inclusive('\n') {
                        if let Some(content) = line.strip_suffix('\n') {
                            self.write_text(content);
                            self.line_break();
                        } else {
                            self.write_text(line);
                        }
                    }
                } else {
                    for word in text.split_inclusive(char::is_whitespace) {
                        let content = word.trim_end_matches(char::is_whitespace);
                        if !content.is_empty() {
                            self.ensure_paragraph();
                            self.flush_space();
                            self.write_text(content);
                        }
                        if word.len() > content.len() {
                            self.pending_space = self.has_text;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: &StartTag<()>) {
        let name: &[u8] = &tag.name;
        if self.in_table
            && !matches!(
                name,
                b"a" | b"b"
                    | b"strong"
                    | b"i"
                    | b"em"
                    | b"u"
                    | b"s"
                    | b"del"
                    | b"code"
                    | b"kbd"
                    | b"sup"
                    | b"sub"
                    | b"mark"
                    | b"br"
            )
        {
            self.pending_space = self.has_text;
            return;
        }

        match name {
            b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" => {
                let level = name[1] - b'0';
                self.begin_paragraph(Paragraph::Heading(level));
                self.heading = Some(Heading {
                    text: String::new(),
                    position: self.position,
                });
                self.align(tag);
            }
            b"p" | b"summary" => {
                if !matches!(self.paragraph, Some(Paragraph::ListItem)) || self.has_text {
                    self.begin_paragraph(Paragraph::Text);
                }
                self.align(tag);
            }
            b"pre" => self.begin_paragraph(Paragraph::Code),
            b"blockquote" => {
                self.end_paragraph();
                self.quotes += 1;
            }
            b"ul" | b"ol" => {
                self.end_paragraph();
                self.lists.push(if name == b"ul" {
                    List::Unordered
                } else {
                    List::Ordered(
                        attribute(tag, b"start")
                            .and_then(|text| text.parse().ok())
                            .unwrap_or(1),
                    )
                });
            }
            b"li" => {
                self.begin_paragraph(Paragraph::ListItem);
                self.list_prefix = Some(self.advance_list());
            }
            b"input" if attribute(tag, b"type") == Some("checkbox") => {
                let checked = attribute(tag, b"checked").is_some();
                self.list_prefix = None;
                self.ensure_paragraph();
                self.write_text(if checked { "☑" } else { "☐" });
                self.tab();
            }
            b"sup" if attribute(tag, b"class") == Some("footnote-reference") => {
                self.start_span(b"sup", r"\super");
            }
            b"div" if attribute(tag, b"class") == Some("footnote-definition") => {
                self.end_paragraph();
                self.in_footnote = true;
                if let Some(id) = attribute(tag, b"id") {
                    self.occupied_ids.insert(id.into());
                    self.anchors.push(Anchor {
                        id: id.into(),
                        position: self.position,
                    });
                }
            }
            b"div" | b"figure" | b"figcaption" | b"details" | b"dl" | b"dt" | b"dd" => {
                self.end_paragraph();
            }
            b"br" => self.line_break(),
            b"hr" => {
                self.begin_paragraph(Paragraph::Rule);
                self.end_paragraph();
            }
            b"a" => {
                let Some(fragment) =
                    attribute(tag, b"href").and_then(|href| href.strip_prefix('#'))
                else {
                    return;
                };
                let target = match fragment.strip_prefix("sketio-link-") {
                    Some(index) => index.parse().ok().map(LinkTarget::Document),
                    None => Some(LinkTarget::Anchor(fragment.into())),
                };
                self.start_span(b"a", r"\ul\cf3");
                let start = self.position;
                if let Some(span) = self.spans.last_mut() {
                    span.link = target.map(|target| (start, target));
                }
            }
            b"img" => self.image(tag),
            b"b" => self.start_span(b"b", r"\b"),
            b"strong" => self.start_span(b"strong", r"\b"),
            b"i" => self.start_span(b"i", r"\i"),
            b"em" => self.start_span(b"em", r"\i"),
            b"u" => self.start_span(b"u", r"\ul"),
            b"s" => self.start_span(b"s", r"\strike"),
            b"del" => self.start_span(b"del", r"\strike"),
            b"ins" => self.start_span(b"ins", r"\ul"),
            b"sub" => self.start_span(b"sub", r"\sub"),
            b"sup" => self.start_span(b"sup", r"\super"),
            b"mark" => self.start_span(b"mark", r"\highlight5"),
            b"kbd" => self.start_span(b"kbd", r"\f1\cf3\highlight4"),
            b"code" if !matches!(self.paragraph, Some(Paragraph::Code)) => {
                self.start_span(b"code", r"\f1\cf3\highlight4");
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &[u8]) {
        if let Some(index) = self.spans.iter().rposition(|span| span.name == name) {
            self.end_spans(index);
            return;
        }
        if self.in_table {
            return;
        }
        match name {
            b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" => self.end_paragraph(),
            b"p" | b"summary" | b"pre" | b"li" | b"figure" | b"figcaption" | b"details" | b"dl"
            | b"dt" | b"dd" => self.end_paragraph(),
            b"div" => {
                self.end_paragraph();
                self.in_footnote = false;
            }
            b"blockquote" => {
                self.end_paragraph();
                self.quotes = (self.quotes - 1).max(0);
            }
            b"ul" | b"ol" => {
                self.end_paragraph();
                self.lists.pop();
            }
            _ => {}
        }
    }

    fn begin_paragraph(&mut self, paragraph: Paragraph) {
        self.end_paragraph();
        self.paragraph = Some(paragraph);
        self.rtf
            .push_str(r"\pard\plain\f0\cf1\fs23\sl310\slmult1\sa140");
        let indent = (self.lists.len() as i32 * 330 + self.quotes * 290).min(self.width / 3);
        if indent > 0 {
            let _ = write!(self.rtf, r"\li{indent}");
        }
        if self.quotes > 0 {
            self.rtf
                .push_str(r"\brdrl\brdrs\brdrw18\brdrcf3\brsp64\cf2");
        }
        match paragraph {
            Paragraph::Text if self.in_footnote => self.rtf.push_str(r"\fs20\cf2"),
            Paragraph::Heading(level) => match level {
                1 => self.rtf.push_str(r"\keepn\b\fs55\sa250"),
                2 => self.rtf.push_str(r"\keepn\b\fs37\sb150\sa160"),
                3 => self.rtf.push_str(r"\keepn\b\fs29\sb120\sa140"),
                _ => self.rtf.push_str(r"\keepn\b\fs24\sb90"),
            },
            Paragraph::Code => self
                .rtf
                .push_str(r"\f1\fs20\sl275\slmult1\sa180\highlight4"),
            Paragraph::Rule => self.rtf.push_str(r"\brdrb\brdrs\brdrw10\brdrcf6\fs12"),
            Paragraph::ListItem => {
                let _ = write!(
                    self.rtf,
                    r"\li{}\fi-260\tx{}\sa72",
                    indent + 260,
                    indent + 260
                );
            }
            Paragraph::TableRow => self
                .rtf
                .push_str(r"\fs21\sb56\sa56\brdrb\brdrs\brdrw8\brdrcf6"),
            _ => {}
        }
        self.rtf.push(' ');
    }

    fn ensure_paragraph(&mut self) {
        if self.paragraph.is_none() {
            self.begin_paragraph(Paragraph::Text);
        }
        if let Some(number) = self.list_prefix.take() {
            let prefix = number.map_or_else(|| "•".into(), |number| format!("{number}."));
            self.write_text(&prefix);
            self.tab();
        }
    }

    fn end_paragraph(&mut self) {
        if self.paragraph.take().is_some() {
            self.end_spans(0);
            self.finish_heading();
            self.rtf.push_str("\\par\n");
            self.position += 1;
        }
        self.has_text = false;
        self.pending_space = false;
        self.list_prefix = None;
    }

    fn finish_heading(&mut self) {
        let Some(heading) = self.heading.take() else {
            return;
        };
        let base = slug(&heading.text);
        let mut id = base.clone();
        if !self.occupied_ids.insert(id.clone()) {
            let suffix = self.heading_suffix.entry(base.clone()).or_insert(1);
            loop {
                id = format!("{base}-{suffix}");
                *suffix += 1;
                if self.occupied_ids.insert(id.clone()) {
                    break;
                }
            }
        }
        self.anchors.push(Anchor {
            id,
            position: heading.position,
        });
    }

    fn align(&mut self, tag: &StartTag<()>) {
        match attribute(tag, b"align") {
            Some("center") => self.rtf.push_str(r"\qc "),
            Some("right") => self.rtf.push_str(r"\qr "),
            _ => {}
        }
    }

    fn advance_list(&mut self) -> Option<u64> {
        match self.lists.last_mut()? {
            List::Ordered(next) => {
                let current = *next;
                *next = next.saturating_add(1);
                Some(current)
            }
            List::Unordered => None,
        }
    }

    fn start_span(&mut self, name: &'static [u8], formatting: &str) {
        self.ensure_paragraph();
        self.flush_space();
        self.rtf.push('{');
        self.rtf.push_str(formatting);
        self.rtf.push(' ');
        self.spans.push(OpenSpan { name, link: None });
    }

    fn end_spans(&mut self, keep: usize) {
        while self.spans.len() > keep {
            if let Some(span) = self.spans.pop() {
                self.rtf.push('}');
                if let Some((start, target)) = span.link
                    && self.position > start
                {
                    self.links.push(Link {
                        start,
                        end: self.position,
                        target,
                    });
                }
            }
        }
    }

    fn flush_space(&mut self) {
        if self.pending_space {
            self.pending_space = false;
            self.write_text(" ");
        }
    }

    fn write_text(&mut self, text: &str) {
        if let Some(heading) = &mut self.heading {
            heading.text.push_str(text);
        }
        for code in text.encode_utf16() {
            match code {
                9 => self.rtf.push_str(r"\tab "),
                0..=31 | 127 => continue,
                92 | 123 | 125 => {
                    self.rtf.push('\\');
                    self.rtf.push(char::from_u32(code.into()).unwrap());
                }
                32..=126 => self.rtf.push(char::from_u32(code.into()).unwrap()),
                _ => {
                    let _ = write!(self.rtf, r"\u{}?", code as i16);
                }
            }
            self.position += 1;
            self.has_text = true;
        }
    }

    fn line_break(&mut self) {
        self.ensure_paragraph();
        self.rtf.push_str(r"\line ");
        self.position += 1;
        self.has_text = false;
        self.pending_space = false;
    }

    fn tab(&mut self) {
        self.pending_space = false;
        self.rtf.push_str(r"\tab ");
        self.position += 1;
    }

    fn image(&mut self, tag: &StartTag<()>) {
        self.ensure_paragraph();
        self.flush_space();
        let alt = attribute(tag, b"alt").unwrap_or("");
        let Some((kind, bytes)) = attribute(tag, b"src")
            .and_then(|src| src.strip_prefix("/image/"))
            .and_then(|key| self.document.image(key))
            .filter(|(_, bytes)| bytes.len() <= self.image_budget)
        else {
            self.image_alt(alt);
            return;
        };
        let (format, dimensions) = match kind {
            "image/png" => (r"\pngblip", png_size(&bytes)),
            "image/jpeg" => (r"\jpegblip", jpeg_size(&bytes)),
            _ => ("", None),
        };
        let Some((w, h)) = dimensions
            .filter(|(w, h)| *w > 0 && *h > 0 && u64::from(*w) * u64::from(*h) <= 40_000_000)
        else {
            self.image_alt(alt);
            return;
        };

        let width = attribute(tag, b"width")
            .and_then(|width| width.parse::<i32>().ok())
            .filter(|width| *width > 0)
            .map_or(w as i64, i64::from);
        let width = (width * 15).min(i64::from(self.width)).max(15);
        let height = ((width * i64::from(h)) / i64::from(w)).clamp(15, 31_680);
        let _ = writeln!(
            self.rtf,
            r"{{\pict{format}\picw{w}\pich{h}\picwgoal{width}\pichgoal{height}"
        );
        const HEX: &[u8; 16] = b"0123456789abcdef";
        self.rtf.reserve(bytes.len().saturating_mul(2));
        self.image_budget -= bytes.len();
        for byte in bytes {
            self.rtf.push(char::from(HEX[(byte >> 4) as usize]));
            self.rtf.push(char::from(HEX[(byte & 15) as usize]));
        }
        self.rtf.push('}');
        self.position += 1;
        self.has_text = true;
    }

    fn image_alt(&mut self, alt: &str) {
        self.write_text(if alt.is_empty() { "Image" } else { alt });
    }

    fn table(&mut self, rows: Vec<Vec<Cell>>) {
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        self.end_paragraph();
        self.in_table = true;
        let pitch = self.width / columns as i32;
        for row in rows {
            if columns > 63 || pitch < 180 {
                // Keep exceptionally wide tables readable when a control cannot fit native cells.
                self.begin_paragraph(Paragraph::TableRow);
                for (index, cell) in row.into_iter().enumerate() {
                    if index > 0 {
                        self.tab();
                    }
                    self.table_cell(cell);
                }
                self.end_paragraph();
                continue;
            }
            self.rtf.push_str(r"\trowd\trgaph90\trleft0");
            for (column, cell) in (0..columns).map(|i| (i, row.get(i))) {
                self.rtf.push_str(concat!(
                    r"\clbrdrt\brdrs\brdrw10\brdrcf6",
                    r"\clbrdrl\brdrs\brdrw10\brdrcf6",
                    r"\clbrdrb\brdrs\brdrw10\brdrcf6",
                    r"\clbrdrr\brdrs\brdrw10\brdrcf6"
                ));
                if cell.is_some_and(|cell| cell.header) {
                    self.rtf.push_str(r"\clcbpat5");
                }
                let edge = self.width * (column + 1) as i32 / columns as i32;
                let _ = write!(self.rtf, r"\cellx{edge}");
            }
            self.position += 2; // RichEdit's row-start marker and paragraph break.
            for cell in row
                .into_iter()
                .chain(std::iter::repeat_with(Cell::default))
                .take(columns)
            {
                self.paragraph = Some(Paragraph::TableRow);
                self.rtf
                    .push_str(r"\pard\plain\intbl\f0\cf1\fs21\sb55\sa55");
                self.rtf.push_str(cell.alignment);
                self.rtf.push(' ');
                self.table_cell(cell);
                self.rtf.push_str(r"\cell");
                self.position += 1;
            }
            self.paragraph = None;
            self.rtf.push_str("\\row\n");
            self.position += 2; // RichEdit's row-end marker and paragraph break.
        }
        self.in_table = false;
    }

    fn table_cell(&mut self, cell: Cell) {
        self.has_text = false;
        self.pending_space = false;
        if cell.header {
            self.rtf.push_str(r"{\b ");
        }
        for token in cell.tokens {
            self.token(token);
        }
        self.end_spans(0);
        if cell.header {
            self.rtf.push('}');
        }
    }
}

#[derive(Default)]
struct Cell {
    header: bool,
    alignment: &'static str,
    tokens: Vec<Token>,
}

fn read_table(tokens: &mut impl Iterator<Item = Token>) -> Vec<Vec<Cell>> {
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut open_cell = false;
    for token in tokens {
        match &token {
            Token::EndTag(tag) if tag.name.as_ref() == b"table" => break,
            Token::StartTag(tag) if tag.name.as_ref() == b"tr" => {
                rows.push(Vec::new());
                open_cell = false;
            }
            Token::StartTag(tag) if matches!(tag.name.as_ref(), b"th" | b"td") => {
                if rows.is_empty() {
                    rows.push(Vec::new());
                }
                if let Some(row) = rows.last_mut() {
                    row.push(Cell {
                        header: tag.name.as_ref() == b"th",
                        alignment: match attribute(tag, b"align").or_else(|| {
                            let (name, value) = attribute(tag, b"style")?.split_once(':')?;
                            (name.trim() == "text-align")
                                .then(|| value.trim().trim_end_matches(';'))
                        }) {
                            Some("center") => r"\qc",
                            Some("right") => r"\qr",
                            _ => "",
                        },
                        ..Cell::default()
                    });
                }
                open_cell = true;
            }
            Token::EndTag(tag) if matches!(tag.name.as_ref(), b"th" | b"td" | b"tr") => {
                open_cell = false;
            }
            _ if open_cell => {
                if let Some(cell) = rows.last_mut().and_then(|row| row.last_mut()) {
                    cell.tokens.push(token);
                }
            }
            _ => {}
        }
    }
    rows
}

fn attribute<'a>(tag: &'a StartTag<()>, key: &[u8]) -> Option<&'a str> {
    tag.attributes
        .get(key)
        .and_then(|item| std::str::from_utf8(&item.value).ok())
}

fn slug(text: &str) -> String {
    let mut slug = String::new();
    for value in text.trim().chars().flat_map(char::to_lowercase) {
        if value.is_alphanumeric() || matches!(value, '-' | '_') {
            slug.push(value);
        } else if value.is_whitespace() {
            slug.push('-');
        }
    }
    if slug.is_empty() {
        "section".into()
    } else {
        slug
    }
}

fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((
        u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?),
        u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?),
    ))
}

fn jpeg_size(mut bytes: &[u8]) -> Option<(u32, u32)> {
    bytes = bytes.strip_prefix(b"\xff\xd8")?;
    while !bytes.is_empty() {
        bytes = bytes.strip_prefix(b"\xff")?;
        while let Some(rest) = bytes.strip_prefix(b"\xff") {
            bytes = rest;
        }
        let (&marker, rest) = bytes.split_first()?;
        if marker == 0xd9 || marker == 0xda {
            return None;
        }
        let length = u16::from_be_bytes(rest.get(..2)?.try_into().ok()?) as usize;
        let segment = rest.get(2..length)?;
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let height = u16::from_be_bytes(segment.get(1..3)?.try_into().ok()?);
            let width = u16::from_be_bytes(segment.get(3..5)?.try_into().ok()?);
            return Some((u32::from(width), u32::from(height)));
        }
        bytes = &rest[length..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn static_welcome() -> &'static Document {
        static WELCOME: std::sync::OnceLock<Document> = std::sync::OnceLock::new();
        WELCOME.get_or_init(Document::welcome)
    }

    fn html(source: &str) -> ReadingHtml<'static> {
        let mut document = ReadingHtml::new(static_welcome(), "");
        document.html = source.into();
        document
    }

    #[test]
    fn exposes_unicode_link_positions_and_duplicate_heading_anchors() {
        let document = html(
            r##"<h1>Résumé</h1>
<p>🦊 <a href="#sketio-link-7"><strong>Go</strong></a></p>
<h2>Résumé</h2>"##,
        );
        let view = render_html(document, Theme::Light, 9_000);
        assert!(view.rtf.starts_with(r"{\rtf1"));
        assert!(view.rtf.contains(r"\b Go"));
        assert_eq!(view.anchors[0].id, "résumé");
        assert_eq!(view.anchors[1].id, "résumé-1");
        assert_eq!(view.anchors[1].position, 13);
        assert_eq!(
            view.links,
            [Link {
                start: 10,
                end: 12,
                target: LinkTarget::Document(7)
            }]
        );
    }

    #[test]
    fn formats_typical_markdown_elements_and_footnotes() {
        let document = html(
            r##"<blockquote><p>Quote</p></blockquote><ol start="10"><li><em>First</em></li></ol>
<pre><code>if { x }</code></pre><table><thead><tr><th>One</th><th>Two</th></tr></thead>
<tbody><tr><td><a href="#note">Link</a></td><td>Cell</td></tr></tbody></table>
<div class="footnote-definition" id="note"><sup>1</sup><p>Note</p></div>"##,
        );
        let view = render_html(document, Theme::Dark, 10_000);
        for sequence in [
            r"\brdrl",
            r"10.\tab",
            r"\i First",
            r"\{ x \}",
            r"\cellx5000",
            r"\super",
        ] {
            assert!(view.rtf.contains(sequence), "missing {sequence}");
        }
        assert_eq!(view.links[0].target, LinkTarget::Anchor("note".into()));
        assert_eq!(view.anchors[0].id, "note");
    }

    #[test]
    fn replaces_a_list_bullet_with_its_task_marker() {
        let document =
            html(r#"<ul><li><input disabled="" type="checkbox" checked=""/>Done</li></ul>"#);
        let view = render_html(document, Theme::Light, 8_000);
        assert!(view.rtf.contains(r"\u9745?\tab Done"));
        assert!(!view.rtf.contains(r"\u8226?"));
    }

    #[test]
    fn recognizes_safe_image_dimensions() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x02";
        assert_eq!(png_size(png), Some((1, 2)));
        let jpeg = b"\xff\xd8\xff\xc0\0\x07\x08\0\x02\0\x01";
        assert_eq!(jpeg_size(jpeg), Some((1, 2)));
        assert_eq!(jpeg_size(b"\xff\xd8\xff\xe0\0\x01"), None);
    }
}

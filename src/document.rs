use std::{
    fmt::Write as _,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use html5gum::{DefaultEmitter, Token, Tokenizer};
use percent_encoding::percent_decode_str;
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, html};

pub const DOCUMENT_LIMIT: u64 = 16 * 1024 * 1024;
const IMAGE_LIMIT: u64 = 32 * 1024 * 1024;
const WELCOME: &str = include_str!("../ui/welcome.md");

pub struct Document {
    pub title: String,
    pub char_count: usize,
    pub editor_char_count: usize,
    pub line_endings: &'static str,
    pub encoding: &'static str,
    pub path: Option<PathBuf>,
    pub untitled: Option<bool>,
    pub source: String,
}

impl Document {
    pub fn welcome() -> Self {
        Self::from_source(WELCOME.into(), None)
    }

    pub fn absolute_path(path: &Path) -> Result<PathBuf, String> {
        if !is_supported(path) {
            return Err("Choose a Markdown or plain text file (.md, .markdown, .txt).".into());
        }
        std::path::absolute(path).map_err(|error| error.to_string())
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        let path = Self::absolute_path(path)?;
        let bytes = read_limited(&path, DOCUMENT_LIMIT)
            .map_err(|error| format!("Could not read the document. {error}"))?;
        let (content, encoding) = decode(bytes)?;
        if content.contains('\0') {
            return Err("This file contains binary data.".into());
        }
        let mut document = Self::from_source(content, Some(path));
        document.encoding = encoding;
        Ok(document)
    }

    pub fn from_source(source: String, path: Option<PathBuf>) -> Self {
        let char_count = source.chars().count();
        let (line_endings, crlf) = line_endings(&source);
        Self {
            title: path
                .as_deref()
                .and_then(Path::file_name)
                .map_or_else(|| "Start here".into(), |name| name.to_string_lossy().into()),
            char_count,
            editor_char_count: char_count - crlf,
            line_endings,
            encoding: "UTF-8",
            path,
            untitled: None,
            source,
        }
    }

    pub fn is_markdown(&self) -> bool {
        self.untitled
            .unwrap_or_else(|| self.path.as_deref().is_none_or(is_markdown))
    }

    pub fn untitled(markdown: bool) -> Self {
        let mut document = Self::from_source(String::new(), None);
        document.untitled = Some(markdown);
        document.title = if markdown {
            "Untitled.md"
        } else {
            "Untitled.txt"
        }
        .into();
        document
    }

    pub fn can_edit(&self) -> bool {
        self.path.is_some() || self.untitled.is_some()
    }

    pub fn at_path(&self, source: String, path: PathBuf) -> Self {
        let mut document = Self::from_source(source, Some(path));
        document.encoding = self.encoding;
        document
    }
}

pub struct ReadingHtml<'a> {
    pub html: String,
    pub links: Vec<String>,
    images: Vec<PathBuf>,
    path: Option<&'a Path>,
}

impl<'a> ReadingHtml<'a> {
    pub fn new(document: &'a Document, source: &str) -> Self {
        let mut doc = Self {
            html: String::new(),
            links: Vec::new(),
            images: Vec::new(),
            path: document.path.as_deref(),
        };
        if !document.is_markdown() {
            doc.html.push_str("<pre>");
            escape_text(&mut doc.html, source);
            doc.html.push_str("</pre>");
            return doc;
        }
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_GFM;
        let mut output = String::with_capacity(source.len());
        let mut events = Parser::new_ext(source, options).peekable();
        let sanitized = std::iter::from_fn(|| {
            let event = events.next()?;
            // A block may contain tags whose attributes span several Markdown lines.
            if let Event::Html(raw) = event {
                let mut source = std::borrow::Cow::Borrowed(raw.as_ref());
                while let Some(Event::Html(next)) =
                    events.next_if(|event| matches!(event, Event::Html(_)))
                {
                    source.to_mut().push_str(&next);
                }
                return Some(Event::Html(doc.static_html(&source).into()));
            }
            Some(doc.event(event))
        });
        html::push_html(&mut output, sanitized);
        doc.html = output;
        doc
    }

    fn event<'source>(&mut self, event: Event<'source>) -> Event<'source> {
        match event {
            Event::InlineHtml(source) => Event::InlineHtml(self.static_html(&source).into()),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let dest_url = CowStr::from(self.link_token(dest_url.into_string()));
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                })
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let dest_url = self.image_token(&dest_url);
                Event::Start(Tag::Image {
                    link_type,
                    dest_url: dest_url.into(),
                    title,
                    id,
                })
            }
            event => event,
        }
    }

    fn link_token(&mut self, url: String) -> String {
        let index = self.links.len();
        self.links.push(url);
        format!("#sketio-link-{index}")
    }

    fn image_token(&mut self, url: &str) -> String {
        let source = self
            .path
            .and_then(Path::parent)
            .and_then(|parent| relative_path(parent, url));
        let Some(path) = source.filter(|path| image_mime(path).is_some()) else {
            return "/image/unavailable".into();
        };
        let index = self.images.len();
        self.images.push(path);
        format!("/image/{index}")
    }

    fn static_html(&mut self, source: &str) -> String {
        let mut output = String::with_capacity(source.len());
        let mut emitter = DefaultEmitter::<()>::default();
        emitter.naively_switch_states(true);

        // Rebuild the markup. Untrusted strings can only reach the output as text
        // or escaped attributes, and navigation always goes through the Rust host.
        for token in Tokenizer::new_with_emitter(source, emitter).flatten() {
            match token {
                Token::StartTag(tag) => {
                    let name = String::from_utf8_lossy(&tag.name);
                    if !static_element(&name) && !matches!(name.as_ref(), "br" | "hr" | "img") {
                        output.push_str("&lt;");
                        escape_text(&mut output, &name);
                        output.push_str("&gt;");
                        continue;
                    }
                    output.push('<');
                    output.push_str(&name);
                    let attr = |key: &[u8]| {
                        tag.attributes
                            .get(key)
                            .and_then(|item| std::str::from_utf8(&item.value).ok())
                    };
                    if let Some(title) = attr(b"title") {
                        write_attribute(&mut output, "title", title);
                    }
                    match name.as_ref() {
                        "a" => {
                            if let Some(href) = attr(b"href") {
                                write_attribute(&mut output, "href", &self.link_token(href.into()));
                            }
                        }
                        "img" => {
                            if let Some(src) = attr(b"src") {
                                write_attribute(&mut output, "src", &self.image_token(src));
                            }
                            if let Some(alt) = attr(b"alt") {
                                write_attribute(&mut output, "alt", alt);
                            }
                            for dimension in ["width", "height"] {
                                if let Some(value) = attr(dimension.as_bytes())
                                    .and_then(|value| value.parse::<u16>().ok())
                                    .filter(|value| *value > 0 && *value <= 16_384)
                                {
                                    let _ = write!(output, " {dimension}=\"{value}\"");
                                }
                            }
                        }
                        "p" | "div" | "td" | "th" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                            if let Some(align @ ("left" | "center" | "right")) = attr(b"align") {
                                write_attribute(&mut output, "align", align);
                            }
                        }
                        "details" if tag.attributes.contains_key(b"open".as_slice()) => {
                            output.push_str(" open");
                        }
                        _ => {}
                    }
                    output.push('>');
                }
                Token::EndTag(tag) => {
                    let name = String::from_utf8_lossy(&tag.name);
                    if static_element(&name) {
                        output.push_str("</");
                        output.push_str(&name);
                        output.push('>');
                    } else if !matches!(name.as_ref(), "br" | "hr" | "img") {
                        output.push_str("&lt;/");
                        escape_text(&mut output, &name);
                        output.push_str("&gt;");
                    }
                }
                Token::String(text) => {
                    escape_text(&mut output, &String::from_utf8_lossy(&text.value))
                }
                Token::Comment(_) | Token::Doctype(_) | Token::Error(_) => {}
            }
        }
        output
    }

    pub fn image(&self, key: &str) -> Option<(&'static str, Vec<u8>)> {
        let path = self.images.get(key.parse::<usize>().ok()?)?;
        Some((image_mime(path)?, read_limited(path, IMAGE_LIMIT).ok()?))
    }
}

fn static_element(name: &str) -> bool {
    matches!(
        name,
        "a" | "b"
            | "blockquote"
            | "caption"
            | "code"
            | "dd"
            | "del"
            | "details"
            | "div"
            | "dl"
            | "dt"
            | "em"
            | "figcaption"
            | "figure"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "i"
            | "ins"
            | "kbd"
            | "li"
            | "mark"
            | "ol"
            | "p"
            | "pre"
            | "s"
            | "samp"
            | "small"
            | "span"
            | "strong"
            | "sub"
            | "summary"
            | "sup"
            | "table"
            | "tbody"
            | "td"
            | "tfoot"
            | "th"
            | "thead"
            | "tr"
            | "u"
            | "ul"
    )
}

fn write_attribute(output: &mut String, name: &str, value: &str) {
    let _ = write!(output, " {name}=\"");
    escape_text(output, value);
    output.push('"');
}

fn escape_text(output: &mut String, value: &str) {
    let mut last = 0;
    for (index, byte) in value.bytes().enumerate() {
        let replacement = match byte {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            _ => continue,
        };
        output.push_str(&value[last..index]);
        output.push_str(replacement);
        last = index + 1;
    }
    output.push_str(&value[last..]);
}

pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

pub fn is_supported(path: &Path) -> bool {
    is_markdown(path)
        || path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
}

pub fn relative_path(parent: &Path, url: &str) -> Option<PathBuf> {
    let url = url.split(['#', '?']).next()?;
    let decoded = percent_decode_str(url).decode_utf8().ok()?;
    if decoded.is_empty()
        || decoded.starts_with(['/', '\\'])
        || decoded.contains(':')
        || decoded.chars().any(char::is_control)
    {
        return None;
    }
    Some(parent.join(decoded.as_ref()))
}

pub fn reserved_windows_name(name: &str) -> bool {
    let basename = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    let name = basename.to_ascii_uppercase();
    if matches!(
        name.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    name.strip_prefix("COM")
        .or_else(|| name.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

fn read_limited(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    use std::path::{Component, Prefix};

    let device = path.components().any(|part| match part {
        Component::Normal(name) => reserved_windows_name(&name.to_string_lossy()),
        Component::Prefix(prefix) => {
            matches!(prefix.kind(), Prefix::DeviceNS(_) | Prefix::Verbatim(_))
        }
        _ => false,
    });
    if device {
        return Err(io::Error::other("Windows device names cannot be opened."));
    }
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("The path does not point to a file."));
    }
    let size = metadata.len();
    if size > limit {
        return Err(io::Error::other(format!(
            "File exceeds {} MiB.",
            limit / 1024 / 1024
        )));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("File grew beyond the size limit."));
    }
    Ok(bytes)
}

fn line_endings(source: &str) -> (&'static str, usize) {
    let bytes = source.as_bytes();
    let mut crlf = 0;
    let mut cr = false;
    for (index, _) in source.match_indices('\r') {
        if bytes.get(index + 1) == Some(&b'\n') {
            crlf += 1;
        } else {
            cr = true;
        }
    }
    let lf = if cr && crlf != 0 {
        return ("Mixed", crlf);
    } else if crlf == 0 {
        source.contains('\n')
    } else {
        source.matches('\n').count() != crlf
    };
    let ending = match (crlf != 0, lf, cr) {
        (true, false, false) => "CRLF",
        (false, true, false) => "LF",
        (false, false, true) => "CR",
        (false, false, false) => "",
        _ => "Mixed",
    };
    (ending, crlf)
}

fn decode(mut bytes: Vec<u8>) -> Result<(String, &'static str), String> {
    let invalid_encoding = || "Save this document as UTF-8 or UTF-16 and try again.".to_owned();
    if let [0xff, 0xfe, body @ ..] | [0xfe, 0xff, body @ ..] = bytes.as_slice() {
        if body.len() % 2 != 0 {
            return Err(invalid_encoding());
        }
        let (to_word, encoding) = if bytes[0] == 0xff {
            (u16::from_le_bytes as fn(_) -> _, "UTF-16 LE")
        } else {
            (u16::from_be_bytes as fn(_) -> _, "UTF-16 BE")
        };
        let words = body.as_chunks::<2>().0.iter().copied().map(to_word);
        let mut content = String::with_capacity(body.len() / 2);
        for character in char::decode_utf16(words) {
            content.push(character.map_err(|_| invalid_encoding())?);
        }
        return Ok((content, encoding));
    }
    let encoding = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        bytes.copy_within(3.., 0);
        bytes.truncate(bytes.len() - 3);
        "UTF-8 BOM"
    } else {
        "UTF-8"
    };
    String::from_utf8(bytes)
        .map(|content| (content, encoding))
        .map_err(|_| invalid_encoding())
}

fn image_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpeg" | "jpg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        "avif" => Some("image/avif"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_tables_tasks_code_and_footnotes() {
        let markdown = "# Hello\n\n| A |\n| - |\n| B |\n\n- [x] Done\n\n```rust\nlet x = 1;\n```\n\nA note.[^1]\n\n[^1]: Text";
        let source = Document::from_source(markdown.into(), None);
        let document = ReadingHtml::new(&source, markdown);
        for element in [
            "<h1>",
            "<table>",
            "type=\"checkbox\"",
            "language-rust",
            "footnote-definition",
        ] {
            assert!(document.html.contains(element), "missing {element}");
        }
    }

    #[test]
    fn escapes_html_and_keeps_urls_out_of_active_attributes() {
        let markdown = "<script>alert(1)</script>\n\n[link](javascript:alert%281%29)\n\n![remote](https://example.com/a.png)";
        let source = Document::from_source(markdown.into(), Some("folder/doc.md".into()));
        let document = ReadingHtml::new(&source, markdown);
        assert!(document.html.contains("&lt;script&gt;"));
        assert!(document.html.contains("href=\"#sketio-link-0\""));
        assert!(!document.html.contains("https://example.com"));
        assert!(!document.html.contains("javascript:"));
    }

    #[test]
    fn renders_static_readme_html_and_rewrites_links() {
        let markdown = r##"<p align="center" style="position:fixed">
<img
  src="brand%20mark.png" width="900" alt="Sketio! &amp; friends" onerror="run()">
<a href="guide.md#start" onclick="run()">Guide</a>
</p>

<details open><summary>More</summary>

Press <kbd>Ctrl</kbd>.

</details>"##;
        let source = Document::from_source(markdown.into(), Some("folder/doc.md".into()));
        let document = ReadingHtml::new(&source, markdown);
        assert!(document.html.contains(r#"<p align="center">"#));
        assert!(document.html.contains(r#"src="/image/0""#));
        assert!(document.html.contains(r#"width="900""#));
        assert!(document.html.contains(r#"alt="Sketio! &amp; friends""#));
        assert!(
            document
                .html
                .contains(r##"<a href="#sketio-link-0">Guide</a>"##)
        );
        assert!(
            document
                .html
                .contains("<details open><summary>More</summary>")
        );
        assert!(document.html.contains("<kbd>Ctrl</kbd>"));
        for forbidden in ["onerror", "onclick", "position:fixed"] {
            assert!(!document.html.contains(forbidden));
        }
        assert_eq!(document.links, ["guide.md#start"]);
        assert_eq!(
            document.images,
            [Path::new("folder").join("brand mark.png")]
        );
        assert!(document.image("1").is_none());
    }

    #[test]
    fn html_attributes_cannot_create_markup_or_network_requests() {
        let markdown = r#"<img src="https://example.org/image.webp" srcset="https://example.org/2.webp" alt="&quot; onclick=&quot;run()"><svg><foreignObject><iframe src="https://example.org/"></iframe></foreignObject></svg>"#;
        let source = Document::from_source(markdown.into(), Some("folder/doc.md".into()));
        let document = ReadingHtml::new(&source, markdown);
        assert!(document.html.contains(r#"src="/image/unavailable""#));
        assert!(
            document
                .html
                .contains(r#"alt="&quot; onclick=&quot;run()""#)
        );
        assert!(!document.html.contains("https://"));
        assert!(!document.html.contains("<svg"));
        assert!(!document.html.contains("<iframe"));
    }

    #[test]
    fn rejects_device_names_before_opening_a_file() {
        for name in ["NUL.md", "cOn.md", "COM1.png", "LPT¹.webp", "AUX .svg"] {
            assert!(reserved_windows_name(name));
            assert!(
                read_limited(Path::new(name), 1)
                    .unwrap_err()
                    .to_string()
                    .contains("device")
            );
        }
        assert!(!reserved_windows_name("notes.md"));
        assert!(!reserved_windows_name("COM12.md"));
    }

    #[test]
    fn resolves_local_references_and_decodes_bom() {
        let folder = Path::new("project");
        assert_eq!(
            relative_path(folder, "../My%20Note.md#idea"),
            Some(folder.join("../My Note.md"))
        );
        for link in [
            "https://example.org",
            "//host/share",
            "C%3a/file.md",
            "%5c%5chost",
            "%00.md",
        ] {
            assert!(relative_path(folder, link).is_none(), "{link}");
        }
        assert_eq!(
            decode(vec![0xff, 0xfe, 0x16, 0x04]).unwrap(),
            ("Ж".into(), "UTF-16 LE"),
        );
        assert_eq!(
            decode(b"\xef\xbb\xbfHi".to_vec()).unwrap(),
            ("Hi".into(), "UTF-8 BOM"),
        );
        assert!(decode(b"\xff".to_vec()).is_err());
    }

    #[test]
    fn decodes_big_endian_surrogates_and_rejects_truncated_utf16() {
        assert_eq!(
            decode(vec![0xfe, 0xff, 0xd8, 0x3e, 0xdd, 0x80]).unwrap(),
            ("🦀".into(), "UTF-16 BE"),
        );
        assert!(decode(vec![0xff, 0xfe, 0x04]).is_err());
        assert!(decode(vec![0xfe, 0xff, 0xd8, 0x3e]).is_err());
    }

    #[test]
    fn counts_editor_characters_and_distinguishes_line_endings() {
        for (source, line_ending, editor_chars) in [
            ("", "", 0),
            ("🦀\n", "LF", 2),
            ("x\ry\r", "CR", 4),
            ("a\r\né\r\n", "CRLF", 4),
            ("a\rb\r\nc\n", "Mixed", 6),
            ("\r\n\n", "Mixed", 2),
        ] {
            let document = Document::from_source(source.into(), None);
            assert_eq!(document.line_endings, line_ending);
            assert_eq!(document.char_count, source.chars().count());
            assert_eq!(document.editor_char_count, editor_chars);
        }
    }
}

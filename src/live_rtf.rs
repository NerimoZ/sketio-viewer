use std::{fmt::Write, mem::MaybeUninit};

use crate::{
    live_markdown::{Line, Style},
    native_paint::Palette,
    text_tools::{self, Font},
};

const CHUNK_BYTES: usize = 4096;

pub struct PlainText<'a> {
    remaining: &'a str,
    output: String,
    consumed: usize,
    closed: bool,
}

impl<'a> PlainText<'a> {
    pub fn new(source: &'a str, palette: Palette) -> Self {
        let output = text_tools::with_font(|font| {
            let mut rtf = String::with_capacity(CHUNK_BYTES + 16);
            header(&mut rtf, font, palette);
            write_style(&mut rtf, scaled_style(Style::default(), font), None);
            rtf
        });
        Self {
            remaining: source,
            output,
            consumed: 0,
            closed: false,
        }
    }

    pub fn finished(&self) -> bool {
        self.closed && self.consumed == self.output.len()
    }

    pub fn read(&mut self, output: &mut [MaybeUninit<u8>]) -> usize {
        if self.consumed == self.output.len() && !self.closed {
            self.refill();
        }
        let remaining = &self.output.as_bytes()[self.consumed..];
        let count = remaining.len().min(output.len());
        output[..count].write_copy_of_slice(&remaining[..count]);
        self.consumed += count;
        count
    }

    fn refill(&mut self) {
        self.output.clear();
        self.consumed = 0;
        while self.output.len() < CHUNK_BYTES && !self.remaining.is_empty() {
            let budget = CHUNK_BYTES - self.output.len() + 4;
            let end = self
                .remaining
                .floor_char_boundary(budget.min(self.remaining.len()));
            let chunk = &self.remaining[..end];
            let escape = chunk
                .bytes()
                .position(|byte| !matches!(byte, 32..=126) || matches!(byte, b'\\' | b'{' | b'}'));
            let start = escape.unwrap_or(end);
            self.output.push_str(&chunk[..start]);
            self.remaining = &self.remaining[start..];
            if escape.is_none() {
                continue;
            }
            let character = self.remaining.chars().next().unwrap();
            self.remaining = &self.remaining[character.len_utf8()..];
            match character {
                '\r' => {
                    self.remaining = self.remaining.strip_prefix('\n').unwrap_or(self.remaining);
                    self.output.push_str("\\par\n");
                }
                '\n' => self.output.push_str("\\par\n"),
                character => {
                    for &unit in character.encode_utf16(&mut [0; 2]).iter() {
                        write_unit(&mut self.output, unit);
                    }
                }
            }
        }
        if self.remaining.is_empty() {
            // The importer consumes the final paragraph marker.
            self.output.push_str(r"\par}");
            self.closed = true;
        }
    }
}

pub struct Paragraph {
    pub indent: i32,
    pub height: i32,
    pub padding: i32,
    pub collapsed: bool,
}

pub fn render(
    lines: &[Line],
    active: Option<usize>,
    palette: Palette,
    mut paragraph: impl FnMut(&Line, bool) -> Paragraph,
) -> String {
    text_tools::with_font(|font| {
        let mut output = String::new();
        header(&mut output, font, palette);
        for (index, line) in lines.iter().enumerate() {
            let raw = Some(index) == active;
            let p = paragraph(line, raw);
            output.push_str(r"\pard\plain");
            if p.indent != 0 {
                let _ = write!(output, r"\li{}", p.indent);
            }
            if p.padding != 0 {
                let _ = write!(output, r"\sa{}", p.padding);
            }
            if p.collapsed {
                output.push_str(r"\sl-15\slmult0");
            } else if p.height != 0 {
                let _ = write!(output, r"\sl{}\slmult0", p.height);
            }
            let base = scaled_style(line.block.base, font);
            write_style(&mut output, base, None);
            if raw || line.runs.is_empty() {
                write_text(&mut output, line.text.encode_utf16());
            } else {
                let mut units = line.text.encode_utf16();
                let mut last = 0;
                for (range, formatting) in &line.runs {
                    let (start, end) = (range.start as usize, range.end as usize);
                    write_text(&mut output, units.by_ref().take(start - last));
                    output.push('{');
                    write_style(&mut output, scaled_style(*formatting, font), Some(base));
                    write_text(&mut output, units.by_ref().take(end - start));
                    output.push('}');
                    last = end;
                }
                write_text(&mut output, units);
            }
            if index + 1 < lines.len() {
                output.push_str("\\par\n");
            }
        }
        output.push('}');
        output
    })
}

fn header(output: &mut String, font: &Font, palette: Palette) {
    output.push_str(r"{\rtf1\ansi\ansicpg1252\deff0\uc1{\fonttbl{\f0\fnil ");
    write_text(output, font.face.encode_utf16());
    output.push_str(r";}{\f1\fmodern Consolas;}}{\colortbl;");
    for color in [palette.ink, palette.accent] {
        let _ = write!(
            output,
            r"\red{}\green{}\blue{};",
            color & 255,
            (color >> 8) & 255,
            (color >> 16) & 255
        );
    }
    output.push('}');
}

fn scaled_style(style: Style, font: &Font) -> Style {
    let height = if style.size == 0 {
        font.height
    } else {
        style.size * font.height / 220
    };
    Style {
        size: (height + 5) / 10,
        bold: style.bold || font.bold,
        italic: style.italic || font.italic,
        ..style
    }
}

fn write_style(output: &mut String, style: Style, base: Option<Style>) {
    // \plain clears character effects. Inline groups inherit the paragraph's style.
    let inherited = base.unwrap_or_default();
    // An explicit font resets RichEdit's fallback after a different script.
    output.push_str(if style.code { r"\f1" } else { r"\f0" });
    if base.is_none() || style.size != inherited.size {
        let _ = write!(output, r"\fs{}", style.size);
    }
    let accent_color = style.code || style.accent;
    if base.is_none() || accent_color != (inherited.code || inherited.accent) {
        output.push_str(if accent_color { r"\cf2" } else { r"\cf1" });
    }
    for (enabled, was_enabled, on, off) in [
        (style.bold, inherited.bold, r"\b", r"\b0"),
        (style.italic, inherited.italic, r"\i", r"\i0"),
        (style.strike, inherited.strike, r"\strike", r"\strike0"),
        (style.accent, inherited.accent, r"\ul", r"\ulnone"),
        (style.hidden, inherited.hidden, r"\v", r"\v0"),
    ] {
        if enabled != was_enabled {
            output.push_str(if enabled { on } else { off });
        }
    }
    output.push(' ');
}

fn write_text(output: &mut String, units: impl Iterator<Item = u16>) {
    for unit in units {
        write_unit(output, unit);
    }
}

fn write_unit(output: &mut String, unit: u16) {
    match unit {
        9 => output.push_str(r"\tab "),
        32..=126 => {
            let ch = char::from(unit as u8);
            if matches!(ch, '\\' | '{' | '}') {
                output.push('\\');
            }
            output.push(ch);
        }
        _ => {
            let _ = write!(output, r"\u{}?", unit as i16);
        }
    }
}

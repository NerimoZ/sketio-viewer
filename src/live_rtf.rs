use std::fmt::Write;

use crate::{
    live_markdown::{Line, Style},
    native_paint::Palette,
    text_tools::{self, Font},
};

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
        let mut output = String::from(r"{\rtf1\ansi\ansicpg1252\deff0\uc1{\fonttbl{\f0\fnil ");
        write_text(&mut output, font.face.encode_utf16());
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
            style(&mut output, line.block.base, font);
            if raw || line.runs.is_empty() {
                write_text(&mut output, line.text.encode_utf16());
            } else {
                let mut units = line.text.encode_utf16();
                let mut last = 0;
                for (range, formatting) in &line.runs {
                    let (start, end) = (range.start as usize, range.end as usize);
                    write_text(&mut output, units.by_ref().take(start - last));
                    output.push('{');
                    style(&mut output, *formatting, font);
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

fn style(output: &mut String, style: Style, font: &Font) {
    let height = if style.size == 0 {
        font.height
    } else {
        style.size * font.height / 220
    };
    let _ = write!(
        output,
        r"\f{}\fs{}\cf{}",
        i32::from(style.code),
        (height + 5) / 10,
        if style.code || style.accent { 2 } else { 1 }
    );
    for (enabled, on, off) in [
        (style.bold || font.bold, r"\b", r"\b0"),
        (style.italic || font.italic, r"\i", r"\i0"),
        (style.strike, r"\strike", r"\strike0"),
        (style.accent, r"\ul", r"\ulnone"),
        (style.hidden, r"\v", r"\v0"),
    ] {
        output.push_str(if enabled { on } else { off });
    }
    output.push(' ');
}

fn write_text(output: &mut String, units: impl Iterator<Item = u16>) {
    for unit in units {
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
}

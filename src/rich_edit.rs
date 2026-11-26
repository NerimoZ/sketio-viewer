use std::{
    ffi::OsStr,
    mem::{self, MaybeUninit},
    os::windows::ffi::OsStrExt,
    ptr, slice,
};

use crate::{live_markdown::Style, live_rtf, native_paint::Palette};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT, WPARAM},
    Graphics::Gdi::InvalidateRect,
    System::LibraryLoader::LoadLibraryW,
    UI::{
        Controls::{NMHDR, ShowScrollBar},
        WindowsAndMessaging::{
            ES_AUTOVSCROLL, ES_MULTILINE, ES_NOHIDESEL, ES_READONLY, IsWindowVisible, SB_HORZ,
            SendMessageW, WM_LBUTTONUP, WM_SETREDRAW, WM_SETTEXT, WM_USER,
        },
    },
};
use windows_sys::core::GUID;

pub const CLASS: &str = "RICHEDIT50W";
pub const STYLE: u32 = (ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY | ES_NOHIDESEL) as u32;

// RichEdit messages and structures follow the packing and definitions in the Windows SDK's Richedit.h.
const EM_EXGETSEL: u32 = WM_USER + 52;
const EM_EXLINEFROMCHAR: u32 = WM_USER + 54;
const EM_GETTEXTRANGE: u32 = WM_USER + 75;
const EM_SETTARGETDEVICE: u32 = WM_USER + 72;
const EM_LINEINDEX: u32 = 0x00bb;
const ENM_SELCHANGE: LPARAM = 0x0008_0000;
const EN_SELCHANGE: u32 = 0x0702;
const EM_EXSETSEL: u32 = WM_USER + 55;
const EM_SETBKGNDCOLOR: u32 = WM_USER + 67;
const EM_SETCHARFORMAT: u32 = WM_USER + 68;
const EM_SETEVENTMASK: u32 = WM_USER + 69;
const EM_STREAMIN: u32 = WM_USER + 73;
const EM_GETSCROLLPOS: u32 = WM_USER + 221;
const EM_SETSCROLLPOS: u32 = WM_USER + 222;
const EM_SETZOOM: u32 = WM_USER + 225;
const EM_GETTEXTLENGTHEX: u32 = WM_USER + 95;
const EM_POSFROMCHAR: u32 = 0x00d6;
const EM_CHARFROMPOS: u32 = 0x00d7;
const EM_SETRECTNP: u32 = 0x00b4;
const EM_SCROLLCARET: u32 = 0x00b7;
const SF_RTF: WPARAM = 0x0002;
const SF_TEXT_UTF8: WPARAM = (65001 << 16) | 0x0020 | 0x0001; // CP_UTF8 | SF_USECODEPAGE | SF_TEXT
const STREAM_TEXT_THRESHOLD: usize = 256 * 1024;
const SFF_SELECTION: WPARAM = 0x8000;
const ENM_LINK: LPARAM = 0x0400_0000;
const EN_LINK: u32 = 0x070b;
const SCF_SELECTION: WPARAM = 1;
const CFM_LINK: u32 = 0x20;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
#[repr(C)]
pub struct CharRange {
    pub start: i32,
    pub end: i32,
}

#[repr(C, packed(4))]
struct EditStream {
    cookie: usize,
    error: u32,
    callback: unsafe extern "system" fn(usize, *mut u8, i32, *mut i32) -> u32,
}

#[repr(C, packed(4))]
struct LinkNotification {
    header: NMHDR,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    range: CharRange,
}

#[repr(C)]
struct CharFormat {
    size: u32,
    mask: u32,
    effects: u32,
    height: i32,
    offset: i32,
    text_color: u32,
    charset: u8,
    pitch_family: u8,
    face: [u16; 32],
}

pub fn read_only(control: HWND, locked: bool) {
    unsafe { SendMessageW(control, 0x00cf, usize::from(locked), 0) }; // EM_SETREADONLY
}

pub fn set_palette(control: HWND, palette: Palette) {
    unsafe { SendMessageW(control, EM_SETBKGNDCOLOR, 0, palette.paper as LPARAM) };
    format(control, Style::default(), palette, 0);
}

pub fn new_text(control: HWND, source: &str, palette: Palette) {
    unsafe {
        SendMessageW(control, EM_SETEVENTMASK, 0, 0);
        SendMessageW(control, WM_SETTEXT, 0, wide("").as_ptr() as LPARAM);
        set_palette(control, palette);
        SendMessageW(
            control,
            WM_USER + 53,
            0,
            crate::document::DOCUMENT_LIMIT as LPARAM,
        );
    }
    // Large files avoid both a whole UTF-16 buffer here and RichEdit's WM_SETTEXT copy.
    let streamed = source.len() >= STREAM_TEXT_THRESHOLD
        && (stream_plain_rtf(control, source, palette)
            || (!source.starts_with('\u{feff}')
                && stream_in(control, source, SF_TEXT_UTF8).is_ok()));
    if !streamed {
        let source = crate::file_edit::normalized(source);
        unsafe { SendMessageW(control, WM_SETTEXT, 0, wide(&*source).as_ptr() as LPARAM) };
    }
    unsafe {
        SendMessageW(control, 0x00cd, 0, 0); // EM_EMPTYUNDOBUFFER
        SendMessageW(control, EM_SETEVENTMASK, 0, ENM_SELCHANGE | 1); // ENM_CHANGE
    }
}

/// Installs a fresh editable document without streaming the same text in as plain text first.
pub fn new_styled_text(control: HWND, rtf: &str, palette: Palette) -> std::io::Result<()> {
    unsafe {
        SendMessageW(control, EM_SETEVENTMASK, 0, 0);
        SendMessageW(control, EM_SETBKGNDCOLOR, 0, palette.paper as LPARAM);
        SendMessageW(
            control,
            WM_USER + 53,
            0,
            crate::document::DOCUMENT_LIMIT as LPARAM,
        );
    }
    let result = stream_in(control, rtf, SF_RTF);
    unsafe {
        SendMessageW(control, 0x00cd, 0, 0); // EM_EMPTYUNDOBUFFER
        SendMessageW(control, EM_SETEVENTMASK, 0, ENM_SELCHANGE | 1); // ENM_CHANGE
    }
    result
}

pub fn source(control: HWND) -> String {
    get_text(control, 0)
}

pub fn search_text(control: HWND) -> String {
    String::from_utf16_lossy(&search_units(control))
}

pub fn search_units(control: HWND) -> Vec<u16> {
    // Preserve native character offsets across table delimiters and embedded objects.
    const GT_RAWTEXT: u32 = 4;
    let mut text = get_text_units(control, GT_RAWTEXT);
    normalize_search_units(&mut text);
    text
}

fn normalize_search_units(text: &mut [u16]) {
    let mut units = text.iter_mut().peekable();
    while let Some(unit) = units.next() {
        *unit = match *unit {
            0x000d => 0x000a,
            0xfff9 | 0xfffb | 0x0007 => 0x0009,
            0xd800..=0xdbff
                if units
                    .peek()
                    .is_some_and(|next| matches!(**next, 0xdc00..=0xdfff)) =>
            {
                units.next();
                *unit
            }
            0xd800..=0xdfff => 0xfffd,
            value => value,
        };
    }
}

fn get_text(control: HWND, flags: u32) -> String {
    String::from_utf16_lossy(&get_text_units(control, flags)).replace('\r', "\n")
}

fn get_text_units(control: HWND, flags: u32) -> Vec<u16> {
    #[repr(C, packed(4))]
    struct GetText {
        bytes: u32,
        flags: u32,
        code_page: u32,
        default_char: usize,
        used_default_char: usize,
    }
    let length = text_length(control);
    let mut text = vec![0u16; length + 1];
    let request = GetText {
        bytes: (text.len() * size_of::<u16>()) as u32,
        flags,
        code_page: 1200,
        default_char: 0,
        used_default_char: 0,
    };
    let used = unsafe {
        SendMessageW(
            control,
            WM_USER + 94,
            ptr::from_ref(&request) as WPARAM,
            text.as_mut_ptr() as LPARAM,
        )
    }
    .max(0) as usize;
    text.truncate(used);
    text
}

pub fn set_paragraph(
    control: HWND,
    at: CharRange,
    indent: i32,
    min_height: i32,
    space_after: i32,
    collapsed: bool,
) {
    #[repr(C)]
    struct ParaFormat2 {
        size: u32,
        mask: u32,
        numbering: u16,
        effects: u16,
        indent: i32,
        right: i32,
        offset: i32,
        alignment: u16,
        tab_count: i16,
        tabs: [i32; 32],
        space_before: i32,
        space_after: i32,
        line_spacing: i32,
        style: i16,
        spacing_rule: u8,
        outline: u8,
        extended: [u16; 8],
    }
    let format = ParaFormat2 {
        size: size_of::<ParaFormat2>() as u32,
        mask: 0x0001 | 0x0080 | 0x0100, // PFM_STARTINDENT | PFM_SPACEAFTER | PFM_LINESPACING
        space_after,
        indent,
        line_spacing: if collapsed { 15 } else { min_height },
        spacing_rule: if collapsed {
            4
        } else if min_height > 0 {
            3
        } else {
            0
        },
        ..unsafe { mem::zeroed() }
    };
    select(control, at, false);
    unsafe { SendMessageW(control, WM_USER + 71, 0, ptr::from_ref(&format) as LPARAM) }; // EM_SETPARAFORMAT
}

pub fn set_style(control: HWND, range: CharRange, style: Style, palette: Palette) {
    select(control, range, false);
    format(control, style, palette, SCF_SELECTION);
}

fn format(control: HWND, style: Style, palette: Palette, scope: WPARAM) {
    const BOLD: u32 = 1;
    const ITALIC: u32 = 2;
    const UNDERLINE: u32 = 4;
    const STRIKE: u32 = 8;
    const HIDDEN: u32 = 0x100;
    const SIZE: u32 = 0x8000_0000;
    const COLOR: u32 = 0x4000_0000;
    const FACE: u32 = 0x2000_0000;
    #[repr(C)]
    struct Format2 {
        size: u32,
        mask: u32,
        effects: u32,
        height: i32,
        offset: i32,
        text_color: u32,
        charset: u8,
        pitch_family: u8,
        face: [u16; 32],
        weight: u16,
        extended: [u32; 6],
    }
    let (height, face, bold, italic) = crate::text_tools::with_font(|font| {
        let height = if style.size == 0 {
            font.height
        } else {
            style.size * font.height / 220
        };
        let mut face = [0; 32];
        let family = if style.code { "Consolas" } else { &font.face };
        for (slot, unit) in face.iter_mut().zip(family.encode_utf16()) {
            *slot = unit;
        }
        (
            height,
            face,
            style.bold || font.bold,
            style.italic || font.italic,
        )
    });
    let appearance = Format2 {
        size: mem::size_of::<Format2>() as u32,
        mask: SIZE | COLOR | FACE | BOLD | ITALIC | UNDERLINE | STRIKE | HIDDEN,
        effects: (u32::from(bold) * BOLD)
            | (u32::from(italic) * ITALIC)
            | (u32::from(style.accent) * UNDERLINE)
            | (u32::from(style.strike) * STRIKE)
            | (u32::from(style.hidden) * HIDDEN),
        height,
        text_color: if style.accent || style.code {
            palette.accent
        } else {
            palette.ink
        },
        offset: 0,
        charset: 1,
        pitch_family: 0,
        face,
        weight: 0,
        extended: [0; 6],
    };
    unsafe {
        SendMessageW(
            control,
            EM_SETCHARFORMAT,
            scope,
            ptr::from_ref(&appearance) as LPARAM,
        )
    };
}

pub fn replace(control: HWND, range: CharRange, text: &[u16]) {
    select(control, range, false);
    unsafe {
        SendMessageW(control, 0x00c2, 1, text.as_ptr() as LPARAM);
    } // EM_REPLACESEL
}

pub fn plain_paste(control: HWND) {
    unsafe { SendMessageW(control, WM_USER + 64, 13, 0) }; // EM_PASTESPECIAL, CF_UNICODETEXT
}

pub fn format_without_undo(control: HWND, apply: impl FnOnce()) {
    // ITextDocument comes from RichEdit's IRichEditOle via QueryInterface.
    type Interface = *mut *const usize;
    type Release = unsafe extern "system" fn(Interface) -> u32;
    type Query = unsafe extern "system" fn(Interface, *const GUID, *mut Interface) -> i32;
    type Undo = unsafe extern "system" fn(Interface, i32, *mut i32) -> i32;
    type Freeze = unsafe extern "system" fn(Interface, *mut i32) -> i32;
    const TEXT_DOCUMENT: GUID = GUID::from_u128(0x8cc497c0_a1df_11ce_8098_00aa0047be5d);
    const TOM_SUSPEND: i32 = -9999995;
    const TOM_RESUME: i32 = -9999994;
    let mut ole: Interface = ptr::null_mut();
    unsafe {
        SendMessageW(control, WM_USER + 60, 0, ptr::from_mut(&mut ole) as LPARAM);
        if ole.is_null() {
            return;
        }
        let mut document: Interface = ptr::null_mut();
        let query: Query = mem::transmute(**ole);
        let result = query(ole, &TEXT_DOCUMENT, &mut document);
        let release: Release = mem::transmute(*(*ole).add(2));
        release(ole);
        if result >= 0 && !document.is_null() {
            let undo: Undo = mem::transmute(*(*document).add(22));
            if undo(document, TOM_SUSPEND, ptr::null_mut()) >= 0 {
                let freeze: Freeze = mem::transmute(*(*document).add(18));
                let unfreeze: Freeze = mem::transmute(*(*document).add(19));
                let mut count = 0;
                let frozen = freeze(document, &mut count) >= 0;
                apply();
                if frozen {
                    unfreeze(document, &mut count);
                }
                undo(document, TOM_RESUME, ptr::null_mut());
            }
            let release: Release = mem::transmute(*(*document).add(2));
            release(document);
        }
    }
}

pub fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain([0]).collect()
}

pub fn load_control() -> std::io::Result<()> {
    let library = unsafe { LoadLibraryW(wide("msftedit.dll").as_ptr()) };
    if library.is_null() {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_wrap(control: HWND, wrap: bool) {
    unsafe {
        SendMessageW(control, EM_SETTARGETDEVICE, 0, LPARAM::from(!wrap));
        ShowScrollBar(control, SB_HORZ, i32::from(!wrap));
    }
}

pub unsafe fn selection_changed(data: LPARAM, control: HWND) -> bool {
    if data == 0 {
        return false;
    }
    let header = unsafe { &*(data as *const NMHDR) };
    header.hwndFrom == control && header.code == EN_SELCHANGE
}

#[derive(Clone, Copy)]
pub struct Cursor {
    pub line: i32,
    pub column: usize,
    pub selected: usize,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            line: 1,
            column: 1,
            selected: 0,
        }
    }
}

pub fn cursor(control: HWND, selection: CharRange) -> Cursor {
    let caret = selection.end.max(0);
    let line =
        unsafe { SendMessageW(control, EM_EXLINEFROMCHAR, 0, caret as LPARAM) as i32 }.max(0);
    let start = unsafe { SendMessageW(control, EM_LINEINDEX, line as WPARAM, 0) as i32 }.max(0);
    Cursor {
        line: line + 1,
        column: unicode_count(control, CharRange { start, end: caret }) + 1,
        selected: unicode_count(control, selection),
    }
}

fn unicode_count(control: HWND, range: CharRange) -> usize {
    let len = range.end.saturating_sub(range.start).max(0) as usize;
    if len == 0 {
        return 0;
    }
    #[repr(C)]
    struct TextRange {
        range: CharRange,
        text: *mut u16,
    }
    let mut text = vec![0u16; len + 1];
    let mut request = TextRange {
        range,
        text: text.as_mut_ptr(),
    };
    let copied = unsafe {
        SendMessageW(
            control,
            EM_GETTEXTRANGE,
            0,
            ptr::from_mut(&mut request) as LPARAM,
        )
    };
    if copied < 0 {
        return len;
    }
    char::decode_utf16(text[..len.min(copied as usize)].iter().copied()).count()
}

pub fn zoom(control: HWND, percent: u32) -> bool {
    unsafe { SendMessageW(control, EM_SETZOOM, percent as WPARAM, 100) != 0 }
}

pub fn char_at(control: HWND, point: POINT) -> Option<i32> {
    let index =
        unsafe { SendMessageW(control, EM_CHARFROMPOS, 0, ptr::from_ref(&point) as LPARAM) } as i32;
    (index >= 0).then_some(index)
}

pub fn char_position(control: HWND, index: i32) -> Option<POINT> {
    let mut point = POINT { x: 0, y: i32::MIN };
    unsafe {
        SendMessageW(
            control,
            EM_POSFROMCHAR,
            ptr::from_mut(&mut point) as WPARAM,
            index as LPARAM,
        )
    };
    (point.y != i32::MIN).then_some(point)
}

pub fn position(control: HWND) -> POINT {
    let mut point = POINT { x: 0, y: 0 };
    unsafe {
        SendMessageW(
            control,
            EM_GETSCROLLPOS,
            0,
            ptr::from_mut(&mut point) as LPARAM,
        )
    };
    point
}

pub fn text_length(control: HWND) -> usize {
    #[repr(C)]
    struct LengthOptions {
        flags: u32,
        code_page: u32,
    }
    const GTL_PRECISE_NUMCHARS: u32 = 0x0002 | 0x0008;
    let options = LengthOptions {
        flags: GTL_PRECISE_NUMCHARS,
        code_page: 1200,
    };
    let length = unsafe {
        SendMessageW(
            control,
            EM_GETTEXTLENGTHEX,
            ptr::from_ref(&options) as WPARAM,
            0,
        )
    };
    length.max(0) as usize
}

pub fn last_line_y(control: HWND) -> Option<i32> {
    let length = text_length(control);
    char_position(control, length.checked_sub(1)? as i32).map(|point| point.y)
}

pub fn set_position(control: HWND, point: POINT) {
    unsafe { SendMessageW(control, EM_SETSCROLLPOS, 0, ptr::from_ref(&point) as LPARAM) };
}

pub fn size(control: HWND, width: i32, height: i32, pad: i32) {
    let rect = RECT {
        left: pad,
        top: 4,
        right: (width - pad).max(pad + 1),
        bottom: height.max(5),
    };
    unsafe { SendMessageW(control, EM_SETRECTNP, 0, ptr::from_ref(&rect) as LPARAM) };
}

pub fn select(control: HWND, range: CharRange, scroll: bool) {
    unsafe {
        SendMessageW(control, EM_EXSETSEL, 0, ptr::from_ref(&range) as LPARAM);
        if scroll {
            SendMessageW(control, EM_SCROLLCARET, 0, 0);
        }
    }
}

pub fn selection(control: HWND) -> CharRange {
    let mut range = CharRange::default();
    unsafe { SendMessageW(control, EM_EXGETSEL, 0, ptr::from_mut(&mut range) as LPARAM) };
    range
}

pub unsafe fn clicked_link(data: LPARAM, control: HWND) -> Option<CharRange> {
    if data == 0 {
        return None;
    }
    // SAFETY: WM_NOTIFY passes a valid NMHDR for this synchronous message.
    let header = unsafe { &*(data as *const NMHDR) };
    if header.hwndFrom != control || header.code != EN_LINK {
        return None;
    }
    let event = unsafe { ptr::read_unaligned(data as *const LinkNotification) };
    (event.message == WM_LBUTTONUP).then_some(event.range)
}

pub fn show(
    control: HWND,
    rtf: &str,
    background: u32,
    links: impl IntoIterator<Item = CharRange>,
) -> std::io::Result<()> {
    let repaint = unsafe { IsWindowVisible(control) != 0 };
    let resume_drawing = || {
        if repaint {
            unsafe {
                SendMessageW(control, WM_SETREDRAW, 1, 0);
                InvalidateRect(control, ptr::null(), 0);
            }
        }
    };
    unsafe {
        if repaint {
            SendMessageW(control, WM_SETREDRAW, 0, 0);
        }
        SendMessageW(control, EM_SETEVENTMASK, 0, 0);
        // Clear editor-mode formatting before importing a new native layout.
        SendMessageW(control, WM_SETTEXT, 0, wide("").as_ptr() as LPARAM);
        SendMessageW(control, EM_SETBKGNDCOLOR, 0, background as LPARAM);
    }
    if let Err(error) = stream_in(control, rtf, SF_RTF) {
        resume_drawing();
        return Err(error);
    }
    let format = CharFormat {
        size: mem::size_of::<CharFormat>() as u32,
        mask: CFM_LINK,
        effects: CFM_LINK,
        height: 0,
        offset: 0,
        text_color: 0,
        charset: 0,
        pitch_family: 0,
        face: [0; 32],
    };
    unsafe { SendMessageW(control, EM_SETEVENTMASK, 0, ENM_LINK | ENM_SELCHANGE) };
    for range in links {
        select(control, range, false);
        unsafe {
            SendMessageW(
                control,
                EM_SETCHARFORMAT,
                SCF_SELECTION,
                ptr::from_ref(&format) as LPARAM,
            )
        };
    }
    select(control, CharRange::default(), true);
    resume_drawing();
    Ok(())
}

/// Imports identical text with new formatting while keeping the native edit history.
/// The caller restores the caret and scroll position after the import.
pub fn restyle_rtf(control: HWND, rtf: &str) -> std::io::Result<()> {
    let mut result = Err(std::io::Error::other("RichEdit could not suspend undo"));
    format_without_undo(control, || {
        let mask = unsafe { SendMessageW(control, EM_SETEVENTMASK, 0, 0) };
        select(control, CharRange { start: 0, end: -1 }, false);
        result = stream_in(control, rtf, SF_RTF | SFF_SELECTION);
        unsafe { SendMessageW(control, EM_SETEVENTMASK, 0, mask) };
    });
    result
}

fn stream_in(control: HWND, source: &str, flags: WPARAM) -> std::io::Result<()> {
    let mut input = StreamInput {
        remaining: source,
        normalize_newlines: flags == SF_TEXT_UTF8,
    };
    stream_from(control, flags, &mut |buffer| input.read(buffer))?;
    if !input.remaining.is_empty() {
        return Err(std::io::Error::other(
            "RichEdit did not read the full text stream",
        ));
    }
    Ok(())
}

fn stream_plain_rtf(control: HWND, source: &str, palette: Palette) -> bool {
    // Use the native text importer for unusual control characters.
    if source
        .bytes()
        .any(|byte| matches!(byte, 0..=8 | 11..=12 | 14..=31 | 127))
    {
        return false;
    }
    let mut rtf = live_rtf::PlainText::new(source, palette);
    stream_from(control, SF_RTF, &mut |buffer| rtf.read(buffer)).is_ok() && rtf.finished()
}

fn stream_from<F>(control: HWND, flags: WPARAM, read: &mut F) -> std::io::Result<()>
where
    F: FnMut(&mut [MaybeUninit<u8>]) -> usize,
{
    let mut stream = EditStream {
        cookie: ptr::from_mut(read) as usize,
        error: 0,
        callback: read_stream::<F>,
    };
    unsafe {
        SendMessageW(
            control,
            EM_STREAMIN,
            flags,
            ptr::from_mut(&mut stream) as LPARAM,
        );
    }
    let error = stream.error;
    if error != 0 {
        return Err(std::io::Error::from_raw_os_error(error as i32));
    }
    Ok(())
}

struct StreamInput<'a> {
    remaining: &'a str,
    normalize_newlines: bool,
}

impl StreamInput<'_> {
    fn read(&mut self, output: &mut [MaybeUninit<u8>]) -> usize {
        let count = self
            .remaining
            .floor_char_boundary(self.remaining.len().min(output.len()));
        let (mut text, tail) = self.remaining.split_at(count);
        self.remaining = tail;
        let mut written = 0;
        if self.normalize_newlines {
            // Keep CRLF together even when RichEdit requests a chunk ending at the CR.
            if text.ends_with('\r') {
                self.remaining = tail.strip_prefix('\n').unwrap_or(tail);
            }
            while let Some((line, rest)) = text.split_once('\r') {
                output[written..written + line.len()].write_copy_of_slice(line.as_bytes());
                written += line.len();
                output[written].write(b'\n');
                written += 1;
                text = rest.strip_prefix('\n').unwrap_or(rest);
            }
        }
        output[written..written + text.len()].write_copy_of_slice(text.as_bytes());
        written + text.len()
    }
}

unsafe extern "system" fn read_stream<F>(
    cookie: usize,
    data: *mut u8,
    amount: i32,
    read: *mut i32,
) -> u32
where
    F: FnMut(&mut [MaybeUninit<u8>]) -> usize,
{
    let input = unsafe { &mut *(cookie as *mut F) };
    let count = if amount > 0 {
        input(unsafe { slice::from_raw_parts_mut(data.cast(), amount as usize) })
    } else {
        0
    };
    unsafe { *read = count as i32 };
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_POPUP};

    #[test]
    fn large_plain_text_stream_preserves_unicode_line_breaks_and_undo() {
        load_control().unwrap();
        let control = unsafe {
            CreateWindowExW(
                0,
                wide(CLASS).as_ptr(),
                ptr::null(),
                WS_POPUP | STYLE,
                0,
                0,
                450,
                300,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        assert!(!control.is_null());
        let original = format!(
            "{}{}",
            "a".repeat(4095),
            "🦀Привет\r\n世界\rX\r\r\n{\\rtf1}\tY\n".repeat(8500)
        );
        assert!(original.len() >= STREAM_TEXT_THRESHOLD);
        let expected = crate::file_edit::normalized(&original);
        read_only(control, false);
        unsafe {
            SendMessageW(
                control,
                WM_USER + 53,
                0,
                crate::document::DOCUMENT_LIMIT as LPARAM,
            );
        }
        assert!(stream_plain_rtf(control, &original, Palette::new(false)));
        assert!(
            source(control) == expected,
            "RTF stream changed the document text"
        );
        new_text(control, &original, Palette::new(false));

        let loaded = source(control);
        let mismatch = loaded
            .chars()
            .zip(expected.chars())
            .position(|(a, b)| a != b);
        assert_eq!(
            mismatch,
            None,
            "{:?}",
            loaded.chars().skip(4090).take(40).collect::<String>()
        );
        assert_eq!(loaded.len(), expected.len());
        let start = rich_edit_test_undo(control);
        assert_eq!(start, 0);
        replace(control, CharRange { start: 0, end: 1 }, &wide("b"));
        assert!(source(control).starts_with("baa"));
        assert_ne!(rich_edit_test_undo(control), 0);
        assert!(source(control) == expected);
        unsafe { DestroyWindow(control) };
    }

    fn rich_edit_test_undo(control: HWND) -> isize {
        unsafe { SendMessageW(control, 0x00c7, 0, 0) } // EM_UNDO
    }

    #[test]
    fn replacing_live_formatting_with_reading_preserves_native_table_positions() {
        use crate::{
            document::Document,
            rtf::{Theme, render},
        };

        load_control().unwrap();
        let control = unsafe {
            CreateWindowExW(
                0,
                wide(CLASS).as_ptr(),
                wide("").as_ptr(),
                WS_POPUP | STYLE,
                0,
                0,
                450,
                300,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        assert!(!control.is_null());
        let source = "# Intro\n\n| First | Second |\n| --- | ---: |\n| A long left cell that must wrap before the right cell begins | [Jump](#after) |\n\n## After";
        let document = Document::untitled(true);
        let view = render(&document, source, Theme::Dark, 5400);
        let palette = Palette::new(true);
        read_only(control, false);
        new_text(control, source, palette);
        let left_cell = source.find("A long").unwrap() as i32;
        set_paragraph(
            control,
            CharRange {
                start: left_cell,
                end: left_cell,
            },
            0,
            2000,
            100,
            false,
        );
        set_style(
            control,
            CharRange {
                start: left_cell,
                end: left_cell + 6,
            },
            Style {
                hidden: true,
                ..Style::default()
            },
            palette,
        );
        read_only(control, true);
        show(
            control,
            &view.rtf,
            palette.paper,
            view.links.iter().map(|link| CharRange {
                start: link.start,
                end: link.end,
            }),
        )
        .unwrap();

        assert_eq!(get_text(control, 4).matches('\u{fff9}').count(), 2);
        let raw = get_text_units(control, 4);
        let displayed = search_units(control);
        assert_eq!(displayed.len(), raw.len());
        for (source, normalized) in raw.into_iter().zip(&displayed) {
            if matches!(source, 0xfff9 | 0xfffb | 0x0007) {
                assert_eq!(*normalized, 0x0009);
            }
        }
        let link = &view.links[0];
        assert_eq!(
            String::from_utf16_lossy(&displayed[link.start as usize..link.end as usize]),
            "Jump",
        );
        let next = view
            .anchors
            .iter()
            .find(|anchor| anchor.id == "after")
            .unwrap();
        assert_eq!(
            String::from_utf16_lossy(&displayed[next.position as usize..]),
            "After"
        );
        let jump = char_position(control, link.start).unwrap();
        let search = search_text(control);
        let wrapping = search.find("right cell").unwrap() as i32;
        let right = char_position(control, wrapping).unwrap();
        assert!(jump.x > right.x + 100);
        assert!(jump.y < right.y);
        assert!(char_position(control, next.position).unwrap().y > right.y);
        unsafe { DestroyWindow(control) };
    }

    #[test]
    fn search_normalization_keeps_object_and_utf16_offsets() {
        let mut text = [
            0xfff9, 0x0007, 0xd83e, 0xdd80, 0xfffc, 0x000d, 0xfffb, 0xdc00, 0xd800,
        ];
        normalize_search_units(&mut text);
        assert_eq!(
            text,
            [
                0x0009, 0x0009, 0xd83e, 0xdd80, 0xfffc, 0x000a, 0x0009, 0xfffd, 0xfffd
            ],
        );
    }

    #[test]
    fn bulk_restyle_preserves_text_and_native_undo() {
        load_control().unwrap();
        let control = unsafe {
            CreateWindowExW(
                0,
                wide(CLASS).as_ptr(),
                wide("").as_ptr(),
                WS_POPUP | STYLE,
                0,
                0,
                350,
                200,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        assert!(!control.is_null());
        read_only(control, false);
        new_text(control, "One\nTwo 🦀\n\n", Palette::new(false));
        replace(control, CharRange { start: 3, end: 3 }, &wide(" A"));
        replace(control, CharRange { start: 9, end: 9 }, &wide(" B"));
        restyle_rtf(
            control,
            r"{\rtf1\ansi\uc1\pard\plain\v One\v0  A\par Two B \u-10178?\u-8832?\par\par}",
        )
        .unwrap();
        assert_eq!(source(control), "One A\nTwo B 🦀\n\n");
        assert_eq!(get_text(control, 8), " A\nTwo B 🦀\n\n");
        unsafe {
            SendMessageW(control, 0x00c7, 0, 0);
        }; // EM_UNDO
        assert_eq!(source(control), "One A\nTwo 🦀\n\n");
        unsafe {
            SendMessageW(control, 0x00c7, 0, 0);
        };
        assert_eq!(source(control), "One\nTwo 🦀\n\n");
        unsafe {
            SendMessageW(control, WM_USER + 84, 0, 0);
        }; // EM_REDO
        assert_eq!(source(control), "One A\nTwo 🦀\n\n");
        unsafe { DestroyWindow(control) };
    }

    #[test]
    fn live_formatting_keeps_hidden_source_and_does_not_add_an_undo_action() {
        load_control().unwrap();
        let control = unsafe {
            CreateWindowExW(
                0,
                wide(CLASS).as_ptr(),
                wide("").as_ptr(),
                WS_POPUP | STYLE,
                0,
                0,
                350,
                200,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        assert!(!control.is_null());
        let palette = Palette::new(false);
        read_only(control, false);
        new_text(control, "# 🦀\n\nSecond", palette);
        let mut formatted = false;
        format_without_undo(control, || {
            formatted = true;
            set_style(
                control,
                CharRange { start: 0, end: 2 },
                Style {
                    hidden: true,
                    ..Style::default()
                },
                palette,
            );
        });
        assert!(formatted);
        assert_eq!(get_text(control, 8), "🦀\n\nSecond"); // GT_NOHIDDENTEXT
        format_without_undo(control, || {
            set_style(
                control,
                CharRange { start: 0, end: 2 },
                Style::default(),
                palette,
            );
        });
        assert_eq!(get_text(control, 8), "# 🦀\n\nSecond");
        assert_eq!(source(control), "# 🦀\n\nSecond");
        assert_eq!(unsafe { SendMessageW(control, 0x00c6, 0, 0) }, 0); // EM_CANUNDO
        unsafe { DestroyWindow(control) };
    }
}

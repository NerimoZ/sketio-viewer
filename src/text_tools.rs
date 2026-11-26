use std::{cell::RefCell, fmt::Write, fs, mem, path::Path, ptr};

use pulldown_cmark::{Event, Options, Parser, Tag};
use serde::{Deserialize, Serialize};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS},
    Graphics::Gdi::LOGFONTW,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::Dialogs::{
            CF_FORCEFONTEXIST, CF_INITTOLOGFONTSTRUCT, CF_LIMITSIZE, CF_NOSCRIPTSEL,
            CF_SCREENFONTS, CHOOSEFONTW, ChooseFontW,
        },
        WindowsAndMessaging::*,
    },
};

use crate::rich_edit::{CharRange, wide};

#[derive(Clone, Serialize, Deserialize)]
pub struct Font {
    pub face: String,
    pub height: i32,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            face: "Segoe UI".into(),
            height: 220,
            bold: false,
            italic: false,
        }
    }
}

thread_local! {
    static FONT: RefCell<Font> = RefCell::new(Font::default());
}

pub fn with_font<T>(apply: impl FnOnce(&Font) -> T) -> T {
    FONT.with_borrow(apply)
}

pub fn set_font(font: Font) {
    FONT.set(font);
}

pub fn load_font(directory: Option<&Path>) {
    let font = directory
        .and_then(|dir| fs::read(dir.join("font.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Font>(&bytes).ok())
        .filter(|font| {
            (120..=1440).contains(&font.height)
                && (1..32).contains(&font.face.encode_utf16().count())
                && !font.face.contains([';', '\0'])
        });
    if let Some(font) = font {
        set_font(font);
    }
}

pub fn choose_font(owner: HWND, dpi: i32) -> Option<Font> {
    let mut initial: LOGFONTW = unsafe { mem::zeroed() };
    with_font(|font| {
        initial.lfHeight = -(font.height * dpi + 720) / 1440;
        initial.lfWeight = if font.bold { 700 } else { 400 };
        initial.lfItalic = u8::from(font.italic);
        initial.lfCharSet = 1; // DEFAULT_CHARSET
        for (slot, unit) in initial.lfFaceName.iter_mut().zip(font.face.encode_utf16()) {
            *slot = unit;
        }
    });
    let mut dialog = CHOOSEFONTW {
        lStructSize: size_of::<CHOOSEFONTW>() as u32,
        hwndOwner: owner,
        lpLogFont: &mut initial,
        Flags: CF_SCREENFONTS
            | CF_INITTOLOGFONTSTRUCT
            | CF_FORCEFONTEXIST
            | CF_LIMITSIZE
            | CF_NOSCRIPTSEL,
        nSizeMin: 6,
        nSizeMax: 72,
        ..unsafe { mem::zeroed() }
    };
    if unsafe { ChooseFontW(&mut dialog) } == 0 {
        return None;
    }
    let end = initial
        .lfFaceName
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(initial.lfFaceName.len());
    let face = String::from_utf16_lossy(&initial.lfFaceName[..end]);
    (!face.is_empty()).then_some(Font {
        face,
        height: (dialog.iPointSize * 2).clamp(120, 1440),
        bold: initial.lfWeight >= 700,
        italic: initial.lfItalic != 0,
    })
}

pub fn apply_reading_font(rtf: &mut String) {
    with_font(|font| {
        let standard = Font::default();
        if font.face != standard.face {
            let mut name = String::new();
            for unit in font.face.encode_utf16() {
                match unit {
                    59 => {}
                    92 | 123 | 125 => {
                        name.push('\\');
                        name.push(char::from(unit as u8));
                    }
                    32..=126 => name.push(char::from(unit as u8)),
                    _ => {
                        let _ = write!(name, "\\u{}?", unit as i16);
                    }
                }
            }
            *rtf = rtf.replacen(r"\f0\fnil Segoe UI;", &format!(r"\f0\fnil {name};"), 1);
        }
        if font.bold || font.italic {
            let prefix = r"\pard\plain\f0\cf1";
            let bold = if font.bold { r"\b" } else { "" };
            let italic = if font.italic { r"\i" } else { "" };
            *rtf = rtf.replace(prefix, &format!("{prefix}{bold}{italic}"));
        }
        if font.height == standard.height {
            return;
        }
        let mut scaled = String::with_capacity(rtf.len());
        let mut rest = 0;
        for (at, _) in rtf.match_indices(r"\fs") {
            if rtf.as_bytes()[..at]
                .iter()
                .rev()
                .take_while(|&&unit| unit == b'\\')
                .count()
                % 2
                == 1
            {
                continue;
            }
            let start = at + 3;
            let digits = rtf.as_bytes()[start..]
                .iter()
                .take_while(|unit| unit.is_ascii_digit())
                .count();
            if digits == 0 {
                continue;
            }
            let end = start + digits;
            if let Ok(size) = rtf[start..end].parse::<i32>() {
                scaled.push_str(&rtf[rest..start]);
                let _ = write!(scaled, "{}", (size * font.height / standard.height).max(1));
                rest = end;
            }
        }
        scaled.push_str(&rtf[rest..]);
        *rtf = scaled;
    });
}

pub fn local_timestamp() -> Option<String> {
    let mut date = [0u16; 96];
    let mut time = [0u16; 96];
    let date_len = unsafe {
        GetDateFormatEx(
            ptr::null(),
            DATE_SHORTDATE,
            ptr::null(),
            ptr::null(),
            date.as_mut_ptr(),
            date.len() as i32,
            ptr::null(),
        )
    };
    let time_len = unsafe {
        GetTimeFormatEx(
            ptr::null(),
            TIME_NOSECONDS,
            ptr::null(),
            ptr::null(),
            time.as_mut_ptr(),
            time.len() as i32,
        )
    };
    (date_len > 1 && time_len > 1).then(|| {
        format!(
            "{} {}",
            String::from_utf16_lossy(&time[..time_len as usize - 1]),
            String::from_utf16_lossy(&date[..date_len as usize - 1]),
        )
    })
}

const LINE_INPUT: i32 = 100;
const LINE_HINT: i32 = 101;
const DIALOG_TITLE: u32 = 102;
// DWLP_USER follows the pointer-sized message result and dialog procedure fields.
const DIALOG_USER: i32 = (2 * size_of::<isize>()) as i32;

pub fn ask_line(owner: HWND, source: &str, cursor: i32) -> Option<i32> {
    let mut offsets = vec![0];
    let mut offset = 0;
    for ch in source.chars() {
        offset += ch.len_utf16() as i32;
        if ch == '\n' {
            offsets.push(offset);
        }
    }
    let prompt = LinePrompt {
        current: offsets.partition_point(|&offset| offset <= cursor),
        length: offsets.len(),
    };
    let mut template = DialogTemplate::new("Go to line", 4, 83);
    template.control(
        WS_VISIBLE | WS_CHILD,
        [12, 27, 196, 11],
        LINE_HINT as u16,
        0x82,
        "",
    );
    template.control(
        WS_VISIBLE
            | WS_CHILD
            | WS_TABSTOP
            | WS_CLIPSIBLINGS
            | ES_NUMBER as u32
            | ES_AUTOHSCROLL as u32,
        [17, 44, 186, 10],
        LINE_INPUT as u16,
        0x81,
        "",
    );
    template.button([99, 63, 53, 15], IDOK, "Go to");
    template.button([157, 63, 51, 15], IDCANCEL, "Cancel");
    let result = show_dialog(
        owner,
        &template,
        Some(line_dialog),
        ptr::from_ref(&prompt) as LPARAM,
    );
    let line = usize::try_from(result).ok()?.checked_sub(1)?;
    offsets.get(line).copied()
}

struct LinePrompt {
    current: usize,
    length: usize,
}

unsafe extern "system" fn line_dialog(
    window: HWND,
    message: u32,
    word: WPARAM,
    param: LPARAM,
) -> isize {
    if let Some(result) = unsafe { style_dialog(window, message, word, param) } {
        return result;
    }
    match message {
        WM_INITDIALOG => {
            let prompt = unsafe { &*(param as *const LinePrompt) };
            let hint = wide(format!("Line number (1–{})", prompt.length));
            let current = wide(prompt.current.max(1).to_string());
            unsafe {
                SetWindowLongPtrW(window, DIALOG_USER, param);
                SetDlgItemTextW(window, LINE_HINT, hint.as_ptr());
                SetDlgItemTextW(window, LINE_INPUT, current.as_ptr());
                let input = GetDlgItem(window, LINE_INPUT);
                SetWindowPos(
                    input,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
                SendMessageW(input, 0x00c5 /* EM_LIMITTEXT */, 10, 0);
                windows_sys::Win32::UI::Shell::SetWindowSubclass(
                    input,
                    Some(line_input_proc),
                    1,
                    0,
                );
            }
        }
        WM_COMMAND if word as u16 as i32 == IDOK => {
            let prompt = unsafe { &*(GetWindowLongPtrW(window, DIALOG_USER) as *const LinePrompt) };
            let mut valid = 0;
            let line = unsafe { GetDlgItemInt(window, LINE_INPUT, &mut valid, 0) } as usize;
            unsafe {
                if valid != 0 && (1..=prompt.length).contains(&line) {
                    EndDialog(window, line as isize);
                } else {
                    let hint = wide(format!("Enter a line from 1 to {}.", prompt.length));
                    SetDlgItemTextW(window, LINE_HINT, hint.as_ptr());
                }
            }
        }
        WM_CLOSE | WM_COMMAND if message == WM_CLOSE || word as u16 as i32 == IDCANCEL => {
            unsafe { EndDialog(window, 0) };
        }
        _ => return 0,
    }
    1
}

struct DialogTemplate(Vec<u16>);

impl DialogTemplate {
    fn new(title: &str, count: u16, height: u16) -> Self {
        let mut dialog = Self(Vec::new());
        dialog.dword(WS_POPUP | WS_CLIPCHILDREN | DS_CENTER as u32 | DS_SETFONT as u32);
        dialog.dword(0);
        dialog.0.extend([count + 1, 0, 0, 220, height, 0, 0]);
        dialog.string(title);
        dialog.0.push(9);
        dialog.string("Segoe UI");
        dialog.control(
            WS_VISIBLE | WS_CHILD | 0x000d, /* SS_OWNERDRAW */
            [12, 7, 196, 15],
            DIALOG_TITLE as u16,
            0x82,
            title,
        );
        dialog
    }

    fn control(&mut self, style: u32, area: [u16; 4], id: u16, class: u16, title: &str) {
        if !self.0.len().is_multiple_of(2) {
            self.0.push(0);
        }
        self.dword(style);
        self.dword(0);
        self.0.extend(area);
        self.0.extend([id, 0xffff, class]);
        self.string(title);
        self.0.push(0);
    }

    fn dword(&mut self, value: u32) {
        self.0.extend([value as u16, (value >> 16) as u16]);
    }

    fn string(&mut self, value: &str) {
        self.0.extend(value.encode_utf16().chain([0]));
    }

    fn button(&mut self, area: [u16; 4], id: i32, label: &str) {
        self.control(
            WS_VISIBLE | WS_CHILD | WS_TABSTOP | BS_OWNERDRAW as u32,
            area,
            id as u16,
            0x80,
            label,
        );
    }

    fn aligned(&self) -> Vec<u32> {
        self.0
            .chunks(2)
            .map(|units| u32::from(units[0]) | u32::from(*units.get(1).unwrap_or(&0)) << 16)
            .collect()
    }
}

thread_local! {
    static DIALOG_DARK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn set_dialog_theme(dark: bool) {
    DIALOG_DARK.set(dark);
}

fn show_dialog(owner: HWND, template: &DialogTemplate, proc: DLGPROC, data: LPARAM) -> isize {
    let storage = template.aligned();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(ptr::null()),
            storage.as_ptr().cast(),
            owner,
            proc,
            data,
        )
    }
}

pub fn confirm_save(owner: HWND, name: &str) -> i32 {
    prompt(
        owner,
        "Save changes?",
        &format!("Save your changes to “{name}”?"),
        true,
    )
}

pub fn alert(owner: HWND, message: &str) {
    prompt(owner, "Sketio Viewer", message, false);
}

fn prompt(owner: HWND, title: &str, message: &str, confirm: bool) -> i32 {
    let body_height = (message
        .lines()
        .map(|line| (line.chars().count() / 45 + 1) * 11)
        .sum::<usize>()
        + 8)
    .clamp(28, 170) as u16;
    let bottom = 30 + body_height;
    let mut template = DialogTemplate::new(title, if confirm { 4 } else { 2 }, bottom + 23);
    template.control(
        WS_VISIBLE
            | WS_CHILD
            | ES_READONLY as u32
            | ES_MULTILINE as u32
            | ES_AUTOVSCROLL as u32
            | if body_height == 170 { WS_VSCROLL } else { 0 },
        [12, 24, 196, body_height],
        103,
        0x81,
        message,
    );
    if confirm {
        template.button([12, bottom, 72, 16], IDNO, "Don’t save");
        template.button([101, bottom, 50, 16], IDCANCEL, "Cancel");
        template.button([157, bottom, 51, 16], IDYES, "Save");
    } else {
        template.button([157, bottom, 51, 16], IDOK, "OK");
    }
    show_dialog(
        owner,
        &template,
        Some(prompt_proc),
        if confirm { IDYES } else { IDOK } as LPARAM,
    ) as i32
}

unsafe extern "system" fn prompt_proc(
    window: HWND,
    message: u32,
    word: WPARAM,
    data: LPARAM,
) -> isize {
    if let Some(result) = unsafe { style_dialog(window, message, word, data) } {
        return result;
    }
    match message {
        WM_INITDIALOG => unsafe {
            SendMessageW(window, DM_SETDEFID, data as WPARAM, 0);
            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(GetDlgItem(
                window,
                data as i32,
            ));
            0
        },
        WM_CLOSE => unsafe {
            EndDialog(window, IDCANCEL as isize);
            1
        },
        WM_COMMAND if matches!(word as u16 as i32, IDOK | IDYES | IDNO | IDCANCEL) => unsafe {
            EndDialog(window, (word as u16) as isize);
            1
        },
        _ => 0,
    }
}

unsafe fn style_dialog(window: HWND, message: u32, word: WPARAM, data: LPARAM) -> Option<isize> {
    use crate::native_paint::{Palette, mix, rounded, text};
    use windows_sys::Win32::{
        Graphics::Gdi::*,
        UI::{Controls::*, HiDpi::GetDpiForWindow},
    };
    let palette = Palette::new(DIALOG_DARK.get());
    match message {
        WM_NCHITTEST => unsafe {
            let mut point = windows_sys::Win32::Foundation::POINT {
                x: i32::from(data as i16),
                y: i32::from((data >> 16) as i16),
            };
            ScreenToClient(window, &mut point);
            let mut title = std::mem::zeroed();
            GetWindowRect(GetDlgItem(window, DIALOG_TITLE as i32), &mut title);
            let mut end = windows_sys::Win32::Foundation::POINT {
                x: title.right,
                y: title.bottom,
            };
            ScreenToClient(window, &mut end);
            if point.y >= 0 && point.y < end.y {
                SetWindowLongPtrW(window, 0 /* DWLP_MSGRESULT */, HTCAPTION as isize);
                Some(1)
            } else {
                None
            }
        },
        WM_NCLBUTTONDOWN if word == HTCAPTION as WPARAM => unsafe {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetCapture;
            let mut bounds = std::mem::zeroed();
            GetWindowRect(window, &mut bounds);
            let x = i32::from(data as i16) - bounds.left;
            let y = i32::from((data >> 16) as i16) - bounds.top;
            SetWindowLongPtrW(
                window,
                GWLP_USERDATA,
                1 + ((x as u16 as isize) | ((y as u16 as isize) << 16)),
            );
            SetCapture(window);
            Some(1)
        },
        WM_MOUSEMOVE if unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } != 0 => unsafe {
            let offset = GetWindowLongPtrW(window, GWLP_USERDATA) - 1;
            let mut cursor = std::mem::zeroed();
            GetCursorPos(&mut cursor);
            SetWindowPos(
                window,
                ptr::null_mut(),
                cursor.x - i32::from(offset as i16),
                cursor.y - i32::from((offset >> 16) as i16),
                0,
                0,
                SWP_NOZORDER | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            Some(1)
        },
        WM_LBUTTONUP if unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } != 0 => unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
            Some(1)
        },
        WM_CAPTURECHANGED => unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            None
        },
        WM_INITDIALOG => {
            let mut rect = unsafe { std::mem::zeroed() };
            unsafe { GetWindowRect(window, &mut rect) };
            crate::floating::disable_shadow(window);
            crate::floating::clip_round(
                window,
                rect.right - rect.left,
                rect.bottom - rect.top,
                (unsafe { GetDpiForWindow(window) } as i32 * 26 / 96).max(20),
            );
            None
        }
        WM_ERASEBKGND | WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => unsafe {
            // DC_BRUSH supplies a per-DC color without allocating a brush per control.
            let dc = word as HDC;
            let brush = GetStockObject(DC_BRUSH);
            SetDCBrushColor(dc, palette.paper);
            SetBkColor(dc, palette.paper);
            SetTextColor(dc, palette.ink);
            if message == WM_ERASEBKGND {
                let mut area = std::mem::zeroed();
                GetClientRect(window, &mut area);
                FillRect(dc, &area, brush);
                let input = GetDlgItem(window, LINE_INPUT);
                if !input.is_null() {
                    let dpi = GetDpiForWindow(window) as i32;
                    let scale = |n| n * dpi / 96;
                    let mut field = std::mem::zeroed();
                    GetWindowRect(input, &mut field);
                    MapWindowPoints(ptr::null_mut(), window, ptr::from_mut(&mut field).cast(), 2);
                    InflateRect(&mut field, scale(9), scale(8));
                    rounded(dc, field, scale(9), palette.status, Some(palette.line));
                }
                Some(1)
            } else {
                Some(brush as isize)
            }
        },
        WM_DRAWITEM if data != 0 => unsafe {
            let item = &*(data as *const DRAWITEMSTRUCT);
            let dpi = GetDpiForWindow(window) as i32;
            let mut label = [0u16; 128];
            let n = GetWindowTextW(item.hwndItem, label.as_mut_ptr(), label.len() as i32).max(0);
            let label = String::from_utf16_lossy(&label[..n as usize]);
            let is_title = item.CtlID == DIALOG_TITLE;
            let primary = matches!(item.CtlID as i32, IDYES | IDOK);
            let focused = item.itemState & (ODS_FOCUS | ODS_SELECTED) != 0;
            let fill = if primary {
                mix(
                    palette.status,
                    palette.accent,
                    if focused { 51 } else { 34 },
                )
            } else if focused {
                palette.toolbar
            } else {
                palette.status
            };
            SetDCBrushColor(item.hDC, palette.paper);
            FillRect(item.hDC, &item.rcItem, GetStockObject(DC_BRUSH));
            if !is_title {
                rounded(
                    item.hDC,
                    item.rcItem,
                    7 * dpi / 96,
                    fill,
                    Some(if focused {
                        palette.accent
                    } else {
                        palette.line
                    }),
                );
            }
            text(
                item.hDC,
                item.rcItem,
                &label,
                if primary { palette.accent } else { palette.ink },
                (if is_title { 16 } else { 13 }) * dpi / 96,
                600,
                !is_title,
            );
            Some(1)
        },
        _ => None,
    }
}

// Keep the native numeric editor for input, caret movement and accessibility.
// Paint its short text and selection with the same palette as the search field.
unsafe extern "system" fn line_input_proc(
    window: HWND,
    message: u32,
    word: WPARAM,
    data: LPARAM,
    id: usize,
    _: usize,
) -> isize {
    use windows_sys::Win32::{
        Foundation::{RECT, SIZE},
        Graphics::Gdi::*,
        UI::{
            Input::KeyboardAndMouse::GetFocus,
            Shell::{DefSubclassProc, RemoveWindowSubclass},
        },
    };
    if message == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(window, Some(line_input_proc), id) };
    }
    if message == WM_ERASEBKGND {
        return 1;
    }
    if message == WM_PAINT {
        let palette = crate::native_paint::Palette::new(DIALOG_DARK.get());
        let mut paint = unsafe { mem::zeroed() };
        let dc = unsafe { BeginPaint(window, &mut paint) };
        let hidden = unsafe { HideCaret(window) } != 0;
        unsafe {
            let mut area: RECT = mem::zeroed();
            GetClientRect(window, &mut area);
            crate::native_paint::paint_buffered(dc, paint.rcPaint, |buffer| {
                let brush = GetStockObject(DC_BRUSH);
                SetDCBrushColor(buffer, palette.status);
                FillRect(buffer, &area, brush);
                let font = SendMessageW(window, WM_GETFONT, 0, 0) as HFONT;
                let old = SelectObject(buffer, font);
                let mut value = [0u16; 12];
                let len =
                    GetWindowTextW(window, value.as_mut_ptr(), value.len() as i32).max(0) as usize;
                let mut begin = 0u32;
                let mut end = 0u32;
                SendMessageW(
                    window,
                    0x00b0, /* EM_GETSEL */
                    ptr::from_mut(&mut begin) as WPARAM,
                    ptr::from_mut(&mut end) as LPARAM,
                );
                let width = |prefix: usize| {
                    let mut size: SIZE = mem::zeroed();
                    GetTextExtentPoint32W(
                        buffer,
                        value.as_ptr(),
                        prefix.min(len) as i32,
                        &mut size,
                    );
                    size.cx
                };
                let begin = (begin as usize).min(len);
                let end = (end as usize).min(len);
                let selected = GetFocus() == window && begin < end;
                if selected {
                    let highlight = RECT {
                        left: width(begin),
                        right: width(end),
                        top: 0,
                        bottom: area.bottom,
                    };
                    SetDCBrushColor(
                        buffer,
                        crate::native_paint::mix(palette.status, palette.accent, 90),
                    );
                    FillRect(buffer, &highlight, brush);
                }
                SetBkMode(buffer, TRANSPARENT as i32);
                SetTextColor(buffer, palette.ink);
                TextOutW(buffer, 0, 0, value.as_ptr(), len as i32);
                SelectObject(buffer, old);
            });
            if hidden {
                ShowCaret(window);
            }
            EndPaint(window, &paint);
        }
        return 0;
    }
    let result = unsafe { DefSubclassProc(window, message, word, data) };
    if matches!(
        message,
        WM_KEYDOWN
            | WM_CHAR
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_SETFOCUS
            | WM_KILLFOCUS
            | WM_SETTEXT
            | 0x00b1
    ) {
        unsafe { InvalidateRect(window, ptr::null(), 0) };
    }
    result
}

pub fn clear_formatting(source: &str, range: CharRange) -> Option<(CharRange, String)> {
    let to_byte = |units: i32| {
        let mut used = 0;
        source
            .char_indices()
            .find(|&(_, ch)| {
                let at = used;
                used += ch.len_utf16() as i32;
                at >= units
            })
            .map_or(source.len(), |(index, _)| index)
    };
    let selection = to_byte(range.start)..to_byte(range.end);
    if selection.is_empty() {
        return None;
    }

    let mut edit = selection.clone();
    let mut strips = Vec::new();
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_GFM;
    for (event, span) in Parser::new_ext(source, options).into_offset_iter() {
        let text = &source[span.clone()];
        let (prefix, suffix_at) = match event {
            Event::Start(Tag::Strong) if text.len() > 4 => (2, text.len() - 2),
            Event::Start(Tag::Emphasis) if text.len() > 2 => (1, text.len() - 1),
            Event::Start(Tag::Strikethrough) if text.len() > 4 && text.starts_with("~~") => {
                (2, text.len() - 2)
            }
            Event::Start(Tag::Strikethrough) if text.len() > 2 && text.starts_with('~') => {
                (1, text.len() - 1)
            }
            Event::Code(_) => {
                let ticks = text.bytes().take_while(|&byte| byte == b'\x60').count();
                if ticks == 0 || text.len() <= ticks * 2 {
                    continue;
                }
                (ticks, text.len() - ticks)
            }
            Event::Start(Tag::Link { .. }) if text.starts_with('[') => {
                let Some(end) = text.rfind("](").or_else(|| text.find("][")) else {
                    continue;
                };
                (1, end)
            }
            Event::Start(Tag::Heading { .. }) if text.starts_with('#') => {
                let prefix = text.bytes().take_while(|&byte| byte == b'#').count();
                let content = &text[prefix..];
                let first = text.len() - content.trim_start_matches([' ', '\t']).len();
                let body =
                    text[..text.trim_end_matches([' ', '\t', '\n']).len()].trim_end_matches('#');
                let end = if body.len() < text.trim_end_matches([' ', '\t', '\n']).len()
                    && body.ends_with([' ', '\t'])
                {
                    body.trim_end_matches([' ', '\t']).len()
                } else {
                    text.trim_end_matches('\n').len()
                };
                (first.min(end), end)
            }
            _ => continue,
        };
        let begin = span.start + prefix;
        let end = span.start + suffix_at;
        if selection.start > begin || selection.end < end {
            continue;
        }
        let end_of_markup = if matches!(event, Event::Start(Tag::Heading { .. })) {
            span.start + text.trim_end_matches('\n').len()
        } else {
            span.end
        };
        edit.start = edit.start.min(span.start);
        edit.end = edit.end.max(end_of_markup);
        strips.push(span.start..begin);
        strips.push(end..end_of_markup);
    }
    if strips.is_empty() {
        return None;
    }
    strips.sort_unstable_by_key(|span| span.start);
    let mut cursor = edit.start;
    let mut plain = String::new();
    for span in strips.into_iter().filter(|span| !span.is_empty()) {
        if span.start > cursor {
            plain.push_str(&source[cursor..span.start]);
        }
        cursor = cursor.max(span.end);
    }
    plain.push_str(&source[cursor..edit.end]);
    let start = source[..edit.start].encode_utf16().count() as i32;
    let end = start + source[edit].encode_utf16().count() as i32;
    Some((CharRange { start, end }, plain))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_formatting_preserves_selection_text_and_surroundings() {
        let source = "# Title\n🦀 **bold** and [label](https://example.org)\n";
        let range = CharRange {
            start: 3,
            end: source.encode_utf16().count() as i32,
        };
        let (_, cleared) = clear_formatting(source, range).unwrap();
        assert_eq!(cleared, "itle\n🦀 bold and label\n");

        let range = CharRange { start: 13, end: 17 };
        let (_, cleared) = clear_formatting(source, range).unwrap();
        assert_eq!(cleared, "bold");

        let whole = CharRange {
            start: 0,
            end: source.encode_utf16().count() as i32,
        };
        let (_, cleared) = clear_formatting(source, whole).unwrap();
        assert_eq!(cleared, "Title\n🦀 bold and label\n");
    }
}

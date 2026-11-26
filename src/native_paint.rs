use std::{cell::RefCell, collections::HashMap, mem, ptr};

use crate::{
    chrome::{Action, Chrome},
    document::Document,
    live_markdown::{Block, Cell, Marker, TableRow},
};
use windows_sys::{
    Win32::{
        Foundation::{RECT, SIZE},
        Graphics::Gdi::{
            AC_SRC_OVER, AlphaBlend, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BitBlt,
            CLEARTYPE_QUALITY, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
            CreateSolidBrush, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CALCRECT, DT_CENTER,
            DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK,
            DeleteDC, DeleteObject, DrawTextW, FillRect, GetDC, GetStockObject,
            GetTextExtentPoint32W, HDC, HFONT, HGDIOBJ, IntersectClipRect, NULL_PEN, PS_SOLID,
            ReleaseDC, RestoreDC, RoundRect, SRCCOPY, SaveDC, SelectObject, SetBkMode,
            SetTextColor, SetViewportOrgEx, StretchDIBits, TRANSPARENT,
        },
        UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx, HICON},
    },
    w,
};

#[derive(Clone, Copy)]
pub struct Palette {
    pub toolbar: u32,
    tab: u32,
    pub status: u32,
    pub paper: u32,
    pub ink: u32,
    pub muted: u32,
    pub line: u32,
    pub accent: u32,
}

impl Palette {
    pub fn new(dark: bool) -> Self {
        let (toolbar, tab, status, paper, ink, muted, line, accent) = if dark {
            (
                0x211a17, 0x423127, 0x241c18, 0x2d221c, 0xf7e8d9, 0xc4ab98, 0x50382c, 0xffa875,
            )
        } else {
            (
                0xf5e8db, 0xfff9f1, 0xf9eee4, 0xfffcf8, 0x382218, 0x72594a, 0xe5d2c2, 0x9b451c,
            )
        };
        Self {
            toolbar: rgb(toolbar),
            tab: rgb(tab),
            status: rgb(status),
            paper: rgb(paper),
            ink: rgb(ink),
            muted: rgb(muted),
            line: rgb(line),
            accent: rgb(accent),
        }
    }
}

const fn rgb(color: u32) -> u32 {
    (color & 0x0000ff) << 16 | (color & 0x00ff00) | (color & 0xff0000) >> 16
}

pub(crate) fn mix(from: u32, to: u32, amount: u32) -> u32 {
    let channel = |shift: u32| -> u32 {
        let a = (from >> shift) & 0xffu32;
        let b = (to >> shift) & 0xffu32;
        ((a * (255 - amount) + b * amount + 127) / 255) << shift
    };
    channel(0) | channel(8) | channel(16)
}

fn dp(value: i32, dpi: i32) -> i32 {
    (value * dpi + 48) / 96
}

fn bounds(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

fn fill(dc: HDC, rect: RECT, color: u32) {
    if rect.left >= rect.right || rect.top >= rect.bottom {
        return;
    }
    unsafe {
        let brush = CreateSolidBrush(color);
        if !brush.is_null() {
            FillRect(dc, &rect, brush);
            DeleteObject(brush as HGDIOBJ);
        }
    }
}

pub(crate) fn rounded(dc: HDC, rect: RECT, radius: i32, color: u32, border: Option<u32>) {
    if rect.left >= rect.right || rect.top >= rect.bottom {
        return;
    }
    unsafe {
        let brush = CreateSolidBrush(color);
        let pen = border.map_or_else(
            || GetStockObject(NULL_PEN),
            |color| CreatePen(PS_SOLID, 1, color) as HGDIOBJ,
        );
        if !brush.is_null() && !pen.is_null() {
            let old_brush = SelectObject(dc, brush as HGDIOBJ);
            let old_pen = SelectObject(dc, pen);
            RoundRect(
                dc,
                rect.left,
                rect.top,
                rect.right,
                rect.bottom,
                radius * 2,
                radius * 2,
            );
            SelectObject(dc, old_pen);
            SelectObject(dc, old_brush);
        }
        if border.is_some() && !pen.is_null() {
            DeleteObject(pen);
        }
        if !brush.is_null() {
            DeleteObject(brush as HGDIOBJ);
        }
    }
}

#[derive(Default)]
struct Fonts(HashMap<(i32, i32, bool), HFONT>);

impl Drop for Fonts {
    fn drop(&mut self) {
        for &font in self.0.values() {
            if !font.is_null() {
                unsafe { DeleteObject(font as HGDIOBJ) };
            }
        }
    }
}

thread_local! {
    static FONTS: RefCell<Fonts> = RefCell::new(Fonts::default());
}

fn font(size: i32, weight: i32, symbol: bool) -> HFONT {
    FONTS.with(|fonts| {
        *fonts
            .borrow_mut()
            .0
            .entry((size, weight, symbol))
            .or_insert_with(|| unsafe {
                CreateFontW(
                    -size,
                    0,
                    0,
                    0,
                    weight,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    0,
                    0,
                    CLEARTYPE_QUALITY as u32,
                    0,
                    if symbol {
                        w!("Segoe MDL2 Assets")
                    } else {
                        w!("Segoe UI")
                    },
                )
            })
    })
}

pub(crate) fn text(
    dc: HDC,
    rect: RECT,
    label: &str,
    color: u32,
    size: i32,
    weight: i32,
    centered: bool,
) {
    let flags = DT_END_ELLIPSIS | if centered { DT_CENTER } else { 0 };
    draw_text(dc, rect, label, color, font(size, weight, false), flags);
}

pub fn interface_font(dpi: i32) -> HFONT {
    font(dp(14, dpi), 400, false)
}

pub fn tab_width(title: &str) -> i32 {
    let text = title.encode_utf16().collect::<Vec<_>>();
    let mut size = SIZE { cx: 0, cy: 0 };
    unsafe {
        let dc = GetDC(ptr::null_mut());
        if !dc.is_null() {
            let previous = SelectObject(dc, font(13, 550, false));
            GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
            SelectObject(dc, previous);
            ReleaseDC(ptr::null_mut(), dc);
        }
    }
    (size.cx + 54).clamp(145, 218)
}

fn glyph(dc: HDC, rect: RECT, code: &str, color: u32, size: i32) {
    draw_text(dc, rect, code, color, font(size, 400, true), DT_CENTER);
}

fn draw_text(dc: HDC, mut rect: RECT, label: &str, color: u32, hfont: HFONT, flags: u32) {
    if label.is_empty() || rect.left >= rect.right || rect.top >= rect.bottom {
        return;
    }
    let chars: Vec<_> = label.encode_utf16().collect();
    unsafe {
        let old_font = if hfont.is_null() {
            ptr::null_mut()
        } else {
            SelectObject(dc, hfont as HGDIOBJ)
        };
        let old_mode = SetBkMode(dc, TRANSPARENT as i32);
        let old_color = SetTextColor(dc, color);
        DrawTextW(
            dc,
            chars.as_ptr(),
            chars.len() as i32,
            &mut rect,
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | flags,
        );
        SetTextColor(dc, old_color);
        SetBkMode(dc, old_mode);
        if !old_font.is_null() {
            SelectObject(dc, old_font);
        }
    }
}

pub struct Header<'a> {
    pub chrome: &'a Chrome,
    pub icon: HICON,
    pub titles: &'a [&'a str],
    pub selected: usize,
    pub hover: Option<Action>,
    pub maximized: bool,
}

pub fn paint_document_background(dc: HDC, dirty: RECT, palette: Palette) {
    fill(dc, dirty, palette.paper);
}

pub fn paint_buffered(dc: HDC, dirty: RECT, draw: impl FnOnce(HDC)) {
    let width = dirty.right - dirty.left;
    let height = dirty.bottom - dirty.top;
    if width <= 0 || height <= 0 {
        return;
    }

    unsafe {
        let buffer = CreateCompatibleDC(dc);
        if buffer.is_null() {
            draw(dc);
            return;
        }
        let bitmap = CreateCompatibleBitmap(dc, width, height);
        if bitmap.is_null() {
            DeleteDC(buffer);
            draw(dc);
            return;
        }
        let previous = SelectObject(buffer, bitmap);
        SetViewportOrgEx(buffer, -dirty.left, -dirty.top, ptr::null_mut());
        draw(buffer);
        SetViewportOrgEx(buffer, 0, 0, ptr::null_mut());
        BitBlt(
            dc, dirty.left, dirty.top, width, height, buffer, 0, 0, SRCCOPY,
        );
        SelectObject(buffer, previous);
        DeleteObject(bitmap);
        DeleteDC(buffer);
    }
}

pub fn paint_shell(dc: HDC, client: RECT, dpi: i32, palette: Palette, header: Header<'_>) {
    let px = |value| dp(value, dpi);
    fill(dc, client, palette.paper);
    let tabs = header.chrome.tab_bottom;
    let menu = header.chrome.menu_bottom;
    fill(dc, bounds(0, 0, client.right, tabs), palette.toolbar);
    fill(dc, bounds(0, tabs, client.right, menu), palette.status);
    fill(dc, bounds(0, tabs - 1, client.right, tabs), palette.line);
    fill(dc, bounds(0, menu - 1, client.right, menu), palette.line);
    if !header.icon.is_null() {
        let size = px(26);
        unsafe {
            DrawIconEx(
                dc,
                px(9),
                (tabs - size) / 2 + px(5),
                header.icon,
                size,
                size,
                0,
                ptr::null_mut(),
                DI_NORMAL,
            )
        };
    }

    for slot in &header.chrome.slots {
        if let Action::CloseTab(index) = slot.action
            && index != header.selected
            && !matches!(header.hover, Some(Action::Tab(other) | Action::CloseTab(other)) if other == index)
        {
            continue;
        }
        let area = slot.area;
        let enabled = header.chrome.action_enabled(slot.action);
        let hot = enabled && header.hover == Some(slot.action);
        let clip = if matches!(slot.action, Action::Tab(_) | Action::CloseTab(_)) {
            let region = header.chrome.tab_region;
            unsafe {
                let saved = SaveDC(dc);
                if saved != 0 {
                    IntersectClipRect(dc, region.left, region.top, region.right, region.bottom);
                }
                saved
            }
        } else {
            0
        };
        match slot.action {
            Action::Tab(index) => {
                let active = index == header.selected;
                let hovered = hot || header.hover == Some(Action::CloseTab(index));
                let highlighted = active || hovered;
                if highlighted {
                    rounded(dc, area, px(9), palette.tab, Some(palette.line));
                }
                let label = header.titles.get(index).copied().unwrap_or_default();
                draw_text(
                    dc,
                    bounds(
                        area.left + px(10),
                        area.top,
                        area.right - px(9),
                        area.bottom,
                    ),
                    label,
                    if highlighted {
                        palette.ink
                    } else {
                        palette.muted
                    },
                    font(px(13), 550, false),
                    0,
                );
            }
            Action::Menu(item) => {
                if hot {
                    rounded(dc, area, px(6), palette.tab, None);
                }
                text(dc, area, item.label(), palette.ink, px(13), 550, true);
            }
            Action::CloseTab(_) => {
                let inset = px(1);
                let button = bounds(
                    area.left + inset,
                    area.top + inset,
                    area.right - inset,
                    area.bottom - inset,
                );
                fade_edges(
                    dc,
                    bounds(button.left - px(10), button.top, button.left, button.bottom),
                    px(10),
                    palette.tab,
                    false,
                    true,
                );
                fill(
                    dc,
                    bounds(button.left, button.top, area.right, button.bottom),
                    palette.tab,
                );
                if hot {
                    rounded(dc, button, px(7), mix(palette.tab, palette.ink, 30), None);
                }
                glyph(dc, button, "\u{e8bb}", palette.ink, px(13));
            }
            action => {
                let backdrop = if hot {
                    if action == Action::CloseWindow {
                        rgb(0xb83e35)
                    } else {
                        mix(palette.toolbar, palette.ink, 29)
                    }
                } else {
                    palette.toolbar
                };
                if hot {
                    if matches!(
                        action,
                        Action::Open | Action::ScrollLeft | Action::ScrollRight
                    ) {
                        rounded(dc, area, px(5), backdrop, None);
                    } else {
                        fill(dc, area, backdrop);
                    }
                }
                let color = if !enabled {
                    mix(palette.toolbar, palette.ink, 79)
                } else if hot && action == Action::CloseWindow {
                    rgb(0xffffff)
                } else {
                    palette.ink
                };
                if action == Action::Maximize && !header.maximized {
                    let x = (area.left + area.right) / 2;
                    let y = (area.top + area.bottom) / 2 + px(1);
                    let half = px(5).max(3);
                    rounded(
                        dc,
                        bounds(x - half, y - half, x + half + 1, y + half + 1),
                        px(2),
                        backdrop,
                        Some(color),
                    );
                    continue;
                }
                let (icon, size) = match action {
                    Action::Minimize => ("\u{e921}", 12),
                    Action::Maximize => ("\u{e923}", 12),
                    Action::CloseWindow => ("\u{e8bb}", 12),
                    Action::Open => ("\u{e710}", 13),
                    Action::ScrollLeft => ("\u{e76b}", 12),
                    Action::ScrollRight => ("\u{e76c}", 12),
                    _ => continue,
                };
                let shift = if action == Action::Minimize { px(2) } else { 0 };
                glyph(
                    dc,
                    bounds(area.left, area.top + shift, area.right, area.bottom + shift),
                    icon,
                    color,
                    px(size),
                );
            }
        }
        if clip != 0 {
            unsafe { RestoreDC(dc, clip) };
        }
    }
    paint_tab_edges(dc, header.chrome, dpi, palette.toolbar);
}

fn paint_tab_edges(dc: HDC, chrome: &Chrome, dpi: i32, background: u32) {
    fade_edges(
        dc,
        chrome.tab_region,
        dp(19, dpi).min(chrome.track_width / 3),
        background,
        chrome.scroll > 0,
        chrome.scroll < chrome.max_scroll,
    );
}

fn fade_edges(dc: HDC, region: RECT, width: i32, background: u32, left: bool, right: bool) {
    if (!left && !right) || width < 2 || region.bottom <= region.top {
        return;
    }
    unsafe {
        let source = CreateCompatibleDC(dc);
        if source.is_null() {
            return;
        }
        let pixel = CreateCompatibleBitmap(dc, 1, 1);
        if pixel.is_null() {
            DeleteDC(source);
            return;
        }
        let old = SelectObject(source, pixel);
        fill(source, bounds(0, 0, 1, 1), background);
        for distance in 0..width {
            let alpha = (244 * (width - 1 - distance) / (width - 1)) as u8;
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: 0,
            };
            if left {
                AlphaBlend(
                    dc,
                    region.left + distance,
                    region.top,
                    1,
                    region.bottom - region.top,
                    source,
                    0,
                    0,
                    1,
                    1,
                    blend,
                );
            }
            if right {
                AlphaBlend(
                    dc,
                    region.right - 1 - distance,
                    region.top,
                    1,
                    region.bottom - region.top,
                    source,
                    0,
                    0,
                    1,
                    1,
                    blend,
                );
            }
        }
        SelectObject(source, old);
        DeleteObject(pixel);
        DeleteDC(source);
    }
}

pub fn paint_scrollbar(
    dc: HDC,
    track: RECT,
    thumb: Option<RECT>,
    dpi: i32,
    palette: Palette,
    emphasis: i32,
) {
    let Some(thumb) = thumb else {
        fill(dc, track, palette.paper);
        return;
    };
    let width = track.right - track.left;
    let height = track.bottom - track.top;
    if width <= 0 || height <= 0 {
        return;
    }

    let center = width as f32 * 0.5;
    let radius = (2.0 + 2.0 * emphasis as f32 / 1000.0) * dpi as f32 / 96.0;
    let top = (thumb.top - track.top) as f32;
    let bottom = (thumb.bottom - track.top) as f32;
    let radius = radius.min((bottom - top) * 0.5);
    let color = mix(
        palette.paper,
        palette.accent,
        120 + (emphasis * 72 / 1000) as u32,
    );
    let rail = mix(
        palette.paper,
        palette.status,
        (emphasis * 180 / 1000) as u32,
    );
    let mut pixels = vec![rgb(palette.paper); (width * height) as usize];
    for (y, row) in pixels.chunks_exact_mut(width as usize).enumerate() {
        let y = y as f32 + 0.5;
        for (x, pixel) in row.iter_mut().enumerate() {
            let dx = (x as f32 + 0.5 - center).abs();
            let background = if dx <= center - dp(3, dpi) as f32 {
                rail
            } else {
                palette.paper
            };
            let coverage = if y >= top && y < bottom && dx <= radius + 0.5 {
                capsule_coverage(dx, y, top, bottom, radius)
            } else {
                0
            };
            *pixel = rgb(mix(background, color, coverage));
        }
    }

    let mut info: BITMAPINFO = unsafe { mem::zeroed() };
    info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = -height;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    unsafe {
        StretchDIBits(
            dc,
            track.left,
            track.top,
            width,
            height,
            0,
            0,
            width,
            height,
            pixels.as_ptr().cast(),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}

fn capsule_coverage(dx: f32, y: f32, top: f32, bottom: f32, radius: f32) -> u32 {
    let dy = (top + radius - y).max(y - (bottom - radius)).max(0.0);
    ((radius + 0.5 - dx.hypot(dy)).clamp(0.0, 1.0) * 255.0).round() as u32
}

#[cfg(test)]
mod scrollbar_tests {
    use super::capsule_coverage;

    #[test]
    fn expanding_thumb_keeps_smooth_symmetric_caps() {
        for radius in [2.0, 2.5, 3.0, 3.5, 4.0] {
            let top = capsule_coverage(1.5, 10.5, 10.0, 50.0, radius);
            let bottom = capsule_coverage(1.5, 49.5, 10.0, 50.0, radius);
            assert_eq!(top, bottom);
            assert!((1..255).contains(&top));
            assert_eq!(capsule_coverage(0.5, 30.5, 10.0, 50.0, radius), 255);
        }
    }
}

pub fn paint_popup_frame(dc: HDC, rect: RECT, dpi: i32, palette: Palette) {
    fill(dc, rect, palette.status);
    rounded(dc, rect, dp(19, dpi), palette.status, Some(palette.line));
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FindButton {
    Expand,
    Previous,
    Next,
    Options,
    Close,
    Replace,
    ReplaceAll,
}

pub struct FindLayout {
    pub field: RECT,
    pub query: RECT,
    pub replacement_field: RECT,
    pub replacement: RECT,
    pub count: RECT,
    pub buttons: [(FindButton, RECT); 7],
}

impl FindLayout {
    pub fn new(width: i32, dpi: i32) -> Self {
        let p = |n| dp(n, dpi);
        let field = bounds(p(34), p(6), width - p(114), p(38));
        let count = bounds(
            field.right - p(57),
            field.top,
            field.right - p(7),
            field.bottom,
        );
        let query = bounds(field.left + p(23), p(12), count.left - p(3), p(33));
        let replacement_field = bounds(p(34), p(44), width - p(154), p(74));
        let replacement = bounds(
            replacement_field.left + p(8),
            p(49),
            replacement_field.right - p(5),
            p(70),
        );
        let buttons = [
            (FindButton::Expand, bounds(p(5), p(8), p(31), p(36))),
            (
                FindButton::Previous,
                bounds(width - p(111), p(8), width - p(85), p(36)),
            ),
            (
                FindButton::Next,
                bounds(width - p(84), p(8), width - p(58), p(36)),
            ),
            (
                FindButton::Options,
                bounds(width - p(57), p(8), width - p(31), p(36)),
            ),
            (
                FindButton::Close,
                bounds(width - p(30), p(8), width - p(4), p(36)),
            ),
            (
                FindButton::Replace,
                bounds(width - p(149), p(44), width - p(84), p(74)),
            ),
            (
                FindButton::ReplaceAll,
                bounds(width - p(80), p(44), width - p(6), p(74)),
            ),
        ];
        Self {
            field,
            query,
            replacement_field,
            replacement,
            count,
            buttons,
        }
    }

    pub fn hit(
        &self,
        point: windows_sys::Win32::Foundation::POINT,
        expanded: bool,
    ) -> Option<FindButton> {
        self.buttons.iter().find_map(|&(action, r)| {
            (point.x >= r.left
                && point.x < r.right
                && point.y >= r.top
                && point.y < r.bottom
                && (expanded || !matches!(action, FindButton::Replace | FindButton::ReplaceAll)))
            .then_some(action)
        })
    }
}

pub struct FindPanel<'a> {
    pub layout: &'a FindLayout,
    pub count: &'a str,
    pub hover: Option<FindButton>,
    pub expanded: bool,
    pub editable: bool,
    pub has_matches: bool,
}

pub fn paint_find_panel(dc: HDC, rect: RECT, dpi: i32, palette: Palette, find: FindPanel<'_>) {
    let px = |value| dp(value, dpi);
    let layout = find.layout;
    fill(dc, rect, palette.status);
    rounded(dc, rect, px(14), palette.status, Some(palette.line));
    rounded(dc, layout.field, px(9), palette.paper, Some(palette.line));
    glyph(
        dc,
        bounds(
            layout.field.left + px(2),
            px(8),
            layout.field.left + px(23),
            px(36),
        ),
        "\u{e721}",
        palette.accent,
        px(13),
    );
    text(
        dc,
        layout.count,
        find.count,
        palette.muted,
        px(11),
        500,
        true,
    );
    if find.expanded {
        rounded(
            dc,
            layout.replacement_field,
            px(8),
            palette.paper,
            Some(palette.line),
        );
    }
    for &(button, area) in &layout.buttons {
        let replacement = matches!(button, FindButton::Replace | FindButton::ReplaceAll);
        if replacement && !find.expanded {
            continue;
        }
        let disabled = (button == FindButton::Expand && !find.editable)
            || (matches!(
                button,
                FindButton::Next
                    | FindButton::Previous
                    | FindButton::Replace
                    | FindButton::ReplaceAll
            ) && !find.has_matches);
        if replacement || (!disabled && find.hover == Some(button)) {
            let color = mix(
                palette.status,
                palette.accent,
                if find.hover == Some(button) { 42 } else { 20 },
            );
            rounded(dc, area, px(7), color, replacement.then_some(palette.line));
        }
        let color = if disabled {
            mix(palette.status, palette.muted, 115)
        } else {
            palette.ink
        };
        if replacement {
            text(
                dc,
                area,
                if button == FindButton::Replace {
                    "Replace"
                } else {
                    "Replace all"
                },
                color,
                px(11),
                550,
                true,
            );
        } else {
            let symbol = match button {
                FindButton::Expand if find.expanded => "\u{e70e}",
                FindButton::Expand => "\u{e76c}",
                FindButton::Previous => "\u{e70e}",
                FindButton::Next => "\u{e70d}",
                FindButton::Options => "\u{e713}",
                FindButton::Close => "\u{e8bb}",
                _ => unreachable!(),
            };
            glyph(dc, area, symbol, color, px(13));
        }
    }
}

pub fn table_height(dc: HDC, row: &TableRow, width: i32, dpi: i32, zoom: u32) -> i32 {
    let pad = dp(8, dpi);
    let size = (dp(15, dpi) * zoom as i32 / 100).max(8);
    let cell_width = width / row.columns.max(1) as i32;
    row.cells
        .iter()
        .map(|cell| {
            let area = bounds(0, 0, (cell_width - pad * 2).max(4), 0);
            draw_cell(
                dc,
                area,
                cell,
                font(size, if row.header { 650 } else { 400 }, false),
                true,
            )
        })
        .max()
        .unwrap_or(0)
        .max(size + 3)
        + dp(10, dpi)
}

pub fn raw_table_height(dc: HDC, text: &str, width: i32, dpi: i32, zoom: u32) -> i32 {
    let size = (dp(15, dpi) * zoom as i32 / 100).max(8);
    let area = bounds(0, 0, width, 0);
    let lines = draw_wrapped(dc, area, text, font(size, 400, false), DT_CALCRECT);
    lines.max(size + 3)
}

fn draw_cell(dc: HDC, area: RECT, cell: &Cell, face: HFONT, measure: bool) -> i32 {
    let align = if measure {
        DT_CALCRECT
    } else {
        match cell.align {
            pulldown_cmark::Alignment::Center => DT_CENTER,
            pulldown_cmark::Alignment::Right => DT_RIGHT,
            _ => 0,
        }
    };
    draw_wrapped(dc, area, &cell.text, face, align)
}

fn draw_wrapped(dc: HDC, mut area: RECT, text: &str, face: HFONT, flags: u32) -> i32 {
    let text: Vec<u16> = text.encode_utf16().collect();
    if text.is_empty() {
        return 0;
    }
    unsafe {
        let old = SelectObject(dc, face);
        let height = DrawTextW(
            dc,
            text.as_ptr(),
            text.len() as i32,
            &mut area,
            DT_WORDBREAK | DT_NOPREFIX | flags,
        );
        SelectObject(dc, old);
        height
    }
}

pub fn paint_markdown_block(
    dc: HDC,
    area: RECT,
    block: &Block,
    dpi: i32,
    zoom: u32,
    palette: Palette,
) {
    let px = |value| dp(value, dpi);
    if let Some(row) = &block.table {
        let background = mix(
            palette.paper,
            if row.header {
                palette.accent
            } else {
                palette.line
            },
            if row.header { 27 } else { 18 },
        );
        fill(dc, area, background);
        let pencil = mix(palette.paper, palette.muted, 84);
        let width = area.right - area.left;
        let size = (px(15) * zoom as i32 / 100).max(8);
        let face = font(size, if row.header { 650 } else { 400 }, false);
        unsafe {
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, palette.ink);
        }
        for column in 0..row.columns {
            let x = area.left + width * column as i32 / row.columns as i32;
            let next = area.left + width * (column + 1) as i32 / row.columns as i32;
            fill(dc, bounds(x, area.top, x + 1, area.bottom), pencil);
            if let Some(cell) = row.cells.get(column) {
                let cell_area = bounds(x + px(8), area.top + px(5), next - px(8), area.bottom);
                if cell_area.right > cell_area.left {
                    let saved = unsafe { SaveDC(dc) };
                    unsafe {
                        IntersectClipRect(dc, x + 1, area.top, next, area.bottom);
                    }
                    draw_cell(dc, cell_area, cell, face, false);
                    unsafe {
                        RestoreDC(dc, saved);
                    }
                }
            }
        }
        fill(
            dc,
            bounds(area.right - 1, area.top, area.right, area.bottom),
            pencil,
        );
        fill(
            dc,
            bounds(area.left, area.top, area.right, area.top + 1),
            pencil,
        );
        if area.bottom > area.top + 1 {
            fill(
                dc,
                bounds(area.left, area.bottom - 1, area.right, area.bottom),
                pencil,
            );
        }
        return;
    }

    let scale = |twips| twips * dpi * zoom as i32 / 144_000;
    for depth in 0..block.quote {
        let x = area.left + scale(depth * 240);
        fill(
            dc,
            bounds(x, area.top, x + px(2), area.bottom),
            mix(palette.paper, palette.accent, 128),
        );
    }
    if block.rule {
        let y = (area.top + area.bottom) / 2;
        fill(dc, bounds(area.left, y, area.right, y + 1), palette.line);
    }
    if let Some(marker) = &block.marker {
        let left = area.left + scale(block.quote * 240 + (block.list - 1) * 330);
        let area = bounds(
            left,
            area.top,
            left + scale(330),
            (area.top + px(19) * zoom as i32 / 100).min(area.bottom),
        );
        let label = match marker {
            Marker::Bullet => "•".to_owned(),
            Marker::Number(number) => format!("{number}."),
            Marker::Task(checked) => {
                let x = area.left + (area.right - area.left - px(12)) / 2;
                let mid = (area.top + area.bottom) / 2;
                let box_area = bounds(x, mid - px(6), x + px(12), mid + px(6));
                let ink = if *checked {
                    palette.accent
                } else {
                    palette.muted
                };
                rounded(dc, box_area, px(3), palette.paper, Some(ink));
                if *checked {
                    text(dc, box_area, "✓", ink, px(11), 650, true);
                }
                return;
            }
        };
        text(
            dc,
            area,
            &label,
            palette.muted,
            (px(15) * zoom as i32 / 100).max(8),
            550,
            true,
        );
    }
}

pub fn markdown_button(width: i32, top: i32, bottom: i32, dpi: i32) -> RECT {
    bounds(
        width - dp(108, dpi),
        top + dp(3, dpi),
        width - dp(8, dpi),
        bottom - dp(3, dpi),
    )
}

pub struct StatusLine<'a> {
    pub document: &'a Document,
    pub zoom: u32,
    pub cursor: crate::rich_edit::Cursor,
    pub char_count: usize,
    pub markdown: Option<(RECT, bool, bool)>,
}

pub fn paint_status(dc: HDC, bar: RECT, dpi: i32, palette: Palette, status: StatusLine<'_>) {
    let StatusLine {
        document: doc,
        zoom,
        cursor,
        char_count,
        markdown,
    } = status;
    let px = |value| dp(value, dpi);
    fill(dc, bar, palette.status);
    fill(
        dc,
        bounds(bar.left, bar.top, bar.right, bar.top + 1),
        palette.line,
    );
    let details_right = markdown.map_or(bar.right, |(area, _, _)| area.left);
    let right = details_right - px(if bar.right < px(470) { 108 } else { 145 });
    let position = format!("Ln {}, Col {}", cursor.line, cursor.column);
    text(
        dc,
        bounds(px(13), bar.top, px(114), bar.bottom),
        &position,
        palette.muted,
        px(12),
        550,
        false,
    );
    if doc.can_edit() {
        let count = if cursor.selected > 0 {
            format!("{} selected  ·  {} chars", cursor.selected, char_count)
        } else {
            format!("{} chars", char_count)
        };
        text(
            dc,
            bounds(px(119), bar.top, right - px(7), bar.bottom),
            &count,
            palette.muted,
            px(12),
            500,
            false,
        );
    }
    let details = if !doc.can_edit() {
        format!("{zoom}%")
    } else if doc.line_endings.is_empty() {
        format!("{zoom}%   ·   {}", doc.encoding)
    } else {
        format!("{zoom}%   ·   {}   ·   {}", doc.line_endings, doc.encoding)
    };
    text(
        dc,
        bounds(right, bar.top, details_right - px(8), bar.bottom),
        &details,
        palette.muted,
        px(12),
        500,
        false,
    );
    if let Some((area, enabled, hover)) = markdown {
        let color = if enabled {
            palette.accent
        } else {
            palette.muted
        };
        let background = mix(palette.status, color, if hover { 46 } else { 20 });
        rounded(
            dc,
            area,
            px(6),
            background,
            Some(mix(palette.status, color, 56)),
        );
        text(
            dc,
            area,
            if enabled {
                "Markdown on"
            } else {
                "Markdown off"
            },
            color,
            px(12),
            550,
            true,
        );
    }
}

pub fn draw_menu_item(
    dc: HDC,
    area: RECT,
    item: &crate::floating::Item,
    dpi: i32,
    palette: Palette,
    selected: bool,
) {
    let label = item.label;
    let px = |value| dp(value, dpi);
    fill(dc, area, palette.status);
    if label.is_empty() {
        let inset = dp(12, dpi);
        let y = (area.top + area.bottom) / 2;
        fill(
            dc,
            bounds(area.left + inset, y, area.right - inset, y + 1),
            palette.line,
        );
        return;
    }

    if selected && item.enabled {
        let row = bounds(
            area.left + px(3),
            area.top + px(2),
            area.right - px(3),
            area.bottom - px(2),
        );
        rounded(dc, row, px(7), palette.tab, Some(palette.line));
        fill(
            dc,
            bounds(
                row.left + px(3),
                row.top + px(4),
                row.left + px(5),
                row.bottom - px(4),
            ),
            palette.accent,
        );
    }
    if item.checked {
        glyph(
            dc,
            bounds(area.left + px(5), area.top, area.left + px(24), area.bottom),
            "\u{e73e}",
            palette.accent,
            px(11),
        );
    }
    let (title, shortcut) = label.split_once('\t').unwrap_or((label, ""));
    let divider = area.right - px(if shortcut.is_empty() { 12 } else { 100 });
    let ink = if !item.enabled {
        mix(palette.status, palette.muted, 165)
    } else {
        palette.ink
    };
    text(
        dc,
        bounds(area.left + px(28), area.top, divider, area.bottom),
        title,
        ink,
        px(13),
        550,
        false,
    );
    text(
        dc,
        bounds(divider + px(8), area.top, area.right - px(8), area.bottom),
        shortcut,
        palette.muted,
        px(12),
        400,
        false,
    );
}

use std::{cell::RefCell, io, mem, ptr, time::Instant};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CreateRoundRectRgn, DeleteObject, EndPaint, GetMonitorInfoW, InvalidateRect,
        MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, PAINTSTRUCT, SetWindowRgn,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::WM_MOUSELEAVE,
        Input::KeyboardAndMouse::{
            SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DOWN, VK_END, VK_ESCAPE,
            VK_HOME, VK_RETURN, VK_UP,
        },
        WindowsAndMessaging::*,
    },
};

use windows_sys::{Win32::Graphics::Gdi::UpdateWindow, w};

use windows_sys::Win32::Graphics::Dwm::{
    DWMNCRP_DISABLED, DWMWA_NCRENDERING_POLICY, DwmSetWindowAttribute,
};

use crate::{
    native_paint::{Palette, draw_menu_item, paint_buffered, paint_popup_frame},
    rich_edit::wide,
};

const CLASS: *const u16 = w!("SketioViewerMenu");
const FADE_TIMER: usize = 1;

#[derive(Debug)]
pub struct Item {
    pub command: usize,
    pub label: &'static str,
    pub enabled: bool,
    pub checked: bool,
}

struct Popup {
    owner: HWND,
    items: Vec<Item>,
    dpi: i32,
    palette: Palette,
    selected: Option<usize>,
    started: Instant,
    animations: bool,
    closing: Option<(Instant, u8)>,
    alpha: u8,
    command: Option<usize>,
}

pub fn register(instance: HINSTANCE) -> io::Result<()> {
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        lpszClassName: CLASS,
        ..unsafe { mem::zeroed() }
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn popup(owner: HWND, mut anchor: POINT, dpi: i32, palette: Palette, items: Vec<Item>) -> HWND {
    let width = dpi * 270 / 96;
    let height = dpi * 12 / 96 + items.iter().map(|item| item_height(item, dpi)).sum::<i32>();
    let screen = unsafe { MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST) };
    let mut monitor = MONITORINFO {
        cbSize: mem::size_of::<MONITORINFO>() as u32,
        ..unsafe { mem::zeroed() }
    };
    if unsafe { GetMonitorInfoW(screen, &mut monitor) } != 0 {
        let work = monitor.rcWork;
        anchor.x = anchor
            .x
            .clamp(work.left, work.right.saturating_sub(width).max(work.left));
        anchor.y = anchor
            .y
            .clamp(work.top, work.bottom.saturating_sub(height).max(work.top));
    }
    let animations = animations_enabled();
    let mut data = Some(Box::new(RefCell::new(Popup {
        owner,
        items,
        dpi,
        palette,
        selected: None,
        started: Instant::now(),
        animations,
        closing: None,
        alpha: 30,
        command: None,
    })));
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | if animations { WS_EX_LAYERED } else { 0 },
            CLASS,
            wide("Viewer menu").as_ptr(),
            WS_POPUP,
            anchor.x,
            anchor.y,
            width,
            height,
            owner,
            ptr::null_mut(),
            GetModuleHandleW(ptr::null()),
            ptr::from_mut(&mut data).cast(),
        );
        if hwnd.is_null() {
            return hwnd;
        }
        disable_shadow(hwnd);
        clip_round(hwnd, width, height, dpi * 19 / 96);
        if animations {
            SetLayeredWindowAttributes(hwnd, 0, 30, LWA_ALPHA);
            SetTimer(hwnd, FADE_TIMER, 15, None);
        }
        ShowWindow(hwnd, SW_SHOW);
        SetFocus(hwnd);
        UpdateWindow(hwnd);
        hwnd
    }
}

pub fn disable_shadow(window: HWND) {
    let policy = DWMNCRP_DISABLED;
    unsafe {
        DwmSetWindowAttribute(
            window,
            DWMWA_NCRENDERING_POLICY as u32,
            ptr::from_ref(&policy).cast(),
            mem::size_of_val(&policy) as u32,
        );
    }
}

pub fn clip_round(hwnd: HWND, width: i32, height: i32, radius: i32) {
    let region = unsafe { CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius) };
    if !region.is_null() && unsafe { SetWindowRgn(hwnd, region, 1) } == 0 {
        unsafe { DeleteObject(region) };
    }
}

pub fn animations_enabled() -> bool {
    let mut enabled = 1i32;
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            ptr::from_mut(&mut enabled).cast(),
            0,
        )
    };
    enabled != 0
}

fn item_height(item: &Item, dpi: i32) -> i32 {
    dpi * if item.label.is_empty() { 9 } else { 30 } / 96
}

impl Popup {
    fn px(&self, value: i32) -> i32 {
        self.dpi * value / 96
    }

    fn hit(&self, window: HWND, data: LPARAM) -> Option<usize> {
        let x = i32::from(data as i16);
        let y = i32::from((data >> 16) as i16);
        let mut rect: RECT = unsafe { mem::zeroed() };
        unsafe { GetClientRect(window, &mut rect) };
        if x < self.px(5) || x >= rect.right - self.px(5) || y < self.px(6) {
            return None;
        }
        let mut bottom = self.px(6);
        self.items.iter().enumerate().find_map(|(index, item)| {
            let top = bottom;
            bottom += item_height(item, self.dpi);
            (item.enabled && y >= top && y < bottom).then_some(index)
        })
    }

    fn select(&mut self, window: HWND, index: Option<usize>) {
        if self.selected != index {
            self.selected = index;
            unsafe { InvalidateRect(window, ptr::null(), 0) };
        }
    }

    fn step(&mut self, window: HWND, previous: bool) {
        let count = self.items.len();
        if count == 0 {
            return;
        }
        let initial = self
            .selected
            .unwrap_or(if previous { 0 } else { count - 1 });
        let index = (1..=count)
            .map(|jump| {
                if previous {
                    (initial + count - jump) % count
                } else {
                    (initial + jump) % count
                }
            })
            .find(|&index| self.items[index].enabled);
        self.select(window, index);
    }

    fn invoke(&mut self, window: HWND, index: Option<usize>) {
        if let Some(item) = index
            .and_then(|index| self.items.get(index))
            .filter(|item| item.enabled)
        {
            self.command = Some(item.command);
            unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
        }
    }

    fn paint(&self, window: HWND) {
        let mut paint: PAINTSTRUCT = unsafe { mem::zeroed() };
        let mut rect: RECT = unsafe { mem::zeroed() };
        unsafe { GetClientRect(window, &mut rect) };
        let dc = unsafe { BeginPaint(window, &mut paint) };
        paint_buffered(dc, paint.rcPaint, |dc| {
            paint_popup_frame(dc, rect, self.dpi, self.palette);
            let mut bottom = self.px(6);
            for (index, item) in self.items.iter().enumerate() {
                let top = bottom;
                bottom += item_height(item, self.dpi);
                let area = RECT {
                    left: self.px(6),
                    top,
                    right: rect.right - self.px(6),
                    bottom,
                };
                draw_menu_item(
                    dc,
                    area,
                    item,
                    self.dpi,
                    self.palette,
                    self.selected == Some(index),
                );
            }
        });
        unsafe { EndPaint(window, &paint) };
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    word: WPARAM,
    long: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(long as *const CREATESTRUCTW) };
        let holder = unsafe { &mut *create.lpCreateParams.cast::<Option<Box<RefCell<Popup>>>>() };
        let Some(data) = holder.take() else { return 0 };
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(data) as isize) };
        return 1;
    }
    let data = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as *mut RefCell<Popup> };
    if message == WM_NCDESTROY && !data.is_null() {
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            drop(Box::from_raw(data));
        }
    } else if let Some(mut data) =
        unsafe { data.as_ref() }.and_then(|cell| cell.try_borrow_mut().ok())
    {
        if data.closing.is_some()
            && matches!(
                message,
                WM_KEYDOWN | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEMOVE
            )
        {
            return 0;
        }
        match message {
            WM_CLOSE => {
                if data.animations {
                    if data.closing.is_none() {
                        data.closing = Some((Instant::now(), data.alpha));
                        unsafe { SetTimer(window, FADE_TIMER, 15, None) };
                    }
                } else {
                    let action = data.command;
                    let owner = data.owner;
                    drop(data);
                    unsafe {
                        DestroyWindow(window);
                        if let Some(action) = action {
                            PostMessageW(owner, WM_COMMAND, action, 0);
                        }
                    }
                }
                return 0;
            }
            WM_ERASEBKGND => return 1,
            WM_PAINT => {
                data.paint(window);
                return 0;
            }
            WM_KILLFOCUS => {
                unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
                return 0;
            }
            WM_MOUSEMOVE => {
                let index = data.hit(window, long);
                data.select(window, index);
                let mut track = TRACKMOUSEEVENT {
                    cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: window,
                    dwHoverTime: 0,
                };
                unsafe { TrackMouseEvent(&mut track) };
                return 0;
            }
            WM_MOUSELEAVE => {
                data.select(window, None);
                return 0;
            }
            WM_LBUTTONUP => {
                let index = data.hit(window, long);
                data.invoke(window, index);
                return 0;
            }
            WM_KEYDOWN => {
                match word as u16 {
                    VK_ESCAPE => {
                        unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
                    }
                    VK_DOWN => data.step(window, false),
                    VK_UP => data.step(window, true),
                    VK_HOME => {
                        let index = data.items.iter().position(|item| item.enabled);
                        data.select(window, index);
                    }
                    VK_END => {
                        let index = data.items.iter().rposition(|item| item.enabled);
                        data.select(window, index);
                    }
                    VK_RETURN => {
                        let index = data.selected;
                        data.invoke(window, index);
                    }
                    _ => {}
                }
                return 0;
            }
            WM_TIMER if word == FADE_TIMER && data.animations => {
                if let Some((started, beginning)) = data.closing {
                    let progress = (started.elapsed().as_secs_f32() / 0.075).min(1.0);
                    if progress >= 1.0 {
                        let action = data.command;
                        let owner = data.owner;
                        drop(data);
                        unsafe {
                            DestroyWindow(window);
                            if let Some(action) = action {
                                PostMessageW(owner, WM_COMMAND, action, 0);
                            }
                        }
                        return 0;
                    }
                    data.alpha = (f32::from(beginning) * (1.0 - progress)) as u8;
                } else {
                    let progress = (data.started.elapsed().as_secs_f32() / 0.11).min(1.0);
                    data.alpha = (30.0 + 225.0 * (1.0 - (1.0 - progress).powi(3))) as u8;
                    if progress >= 1.0 {
                        unsafe { KillTimer(window, FADE_TIMER) };
                    }
                }
                unsafe { SetLayeredWindowAttributes(window, 0, data.alpha, LWA_ALPHA) };
                return 0;
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(window, message, word, long) }
}

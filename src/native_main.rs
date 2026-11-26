#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chrome;
mod document;
mod file_edit;
mod find_text;
mod floating;
mod live_markdown;
mod live_rtf;
mod native_paint;
mod printing;
mod rich_edit;
mod rtf;
mod session;
mod text_tools;

use std::{
    borrow::Cow,
    cell::RefCell,
    env, fs, io, mem,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
    ptr,
    rc::Rc,
    time::{Duration, Instant},
};

use chrome::{Action, Chrome, Menu, TITLE_HEIGHT};
use document::{Document, is_supported, relative_path};
use native_paint::{
    FindButton, FindLayout, FindPanel, Header, Palette, interface_font, paint_buffered,
    paint_find_panel, paint_scrollbar, paint_shell, paint_status, tab_width,
};
use rich_edit::{CharRange, wide};
use rtf::{LinkTarget, Rendered, Theme, render};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
            DwmSetWindowAttribute,
        },
        Gdi::{
            BeginPaint, ClientToScreen, CreateRectRgn, CreateSolidBrush, DeleteObject, EndPaint,
            GetDC, GetMonitorInfoW, HBRUSH, HDC, InvalidateRect, MONITOR_DEFAULTTONEAREST,
            MONITORINFO, MonitorFromWindow, PAINTSTRUCT, ReleaseDC, ScreenToClient, SetBkColor,
            SetTextColor, SetWindowRgn, UpdateWindow,
        },
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{EM_GETLINECOUNT, EM_SETCUEBANNER, EM_SETSEL, WM_MOUSELEAVE},
        HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
            GetSystemMetricsForDpi, SetProcessDpiAwarenessContext,
        },
        Input::KeyboardAndMouse::{
            GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT,
            TrackMouseEvent, VK_ADD, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_F3, VK_F5, VK_HOME,
            VK_INSERT, VK_LEFT, VK_MENU, VK_NEXT, VK_OEM_MINUS, VK_OEM_PLUS, VK_PRIOR, VK_RETURN,
            VK_SHIFT, VK_SUBTRACT, VK_TAB, VK_UP,
        },
        Shell::{
            DefSubclassProc, DragAcceptFiles, DragFinish, DragQueryFileW, HDROP,
            RemoveWindowSubclass, SetCurrentProcessExplicitAppUserModelID, SetWindowSubclass,
            ShellExecuteW,
        },
        WindowsAndMessaging::*,
    },
};

const APP_NAME: &str = "Sketio Viewer";
const APP_USER_MODEL_ID: &str = "Sketio.Viewer";
// Keep existing preferences and tabs when updating from the original executable.
const SETTINGS_DIRECTORY: &str = "Sketio Documents Viewer";
const WINDOW_CLASS: &str = "SketioDocumentsViewer";
const APPLICATION_ICON: *const u16 = ptr::without_provenance(1);
const BACK: usize = 100;
const RELOAD: usize = 101;
const SEARCH: usize = 102;
const THEME: usize = 103;
const OPEN: usize = 104;
const SEARCH_EDIT: usize = 106;
const SEARCH_SURFACE: usize = 112;
const SEARCH_CLASS: &str = "SketioViewerSearch";
const SEARCH_PREVIOUS: usize = 107;
const SEARCH_NEXT: usize = 108;
const SEARCH_CLOSE: usize = 109;
const REPLACE_EDIT: usize = 110;
const SEARCH_REPLACE: usize = 147;
const SEARCH_REPLACE_ALL: usize = 148;
const SEARCH_EXPAND: usize = 149;
const SEARCH_CASE: usize = 150;
const SEARCH_WRAP: usize = 151;
const SEARCH_OPTIONS: usize = 152;
const FIND_REPLACE: usize = 153;
const SHOW_SEARCH_OPTIONS: u32 = WM_APP + 4;
const READER: usize = 111;
const RESIZE_TIMER: usize = 1;
const SCROLL_TIMER: usize = 2;
const EDIT_TIMER: usize = 3;
const MARKDOWN_APPLY_TIMER: usize = 4;
const MARKDOWN_PREF_TIMER: usize = 5;
const UPDATE_ACTIVE_LINE: u32 = WM_APP + 3;
const SHOW_MENU: u32 = WM_APP + 1;
const WINDOW_WIDTH: i32 = 560;
const WINDOW_HEIGHT: i32 = 744;
const CLOSE_TAB: usize = 120;
const COPY_TEXT: usize = 121;
const SELECT_ALL: usize = 122;
const ZOOM_IN: usize = 123;
const ZOOM_OUT: usize = 124;
const ZOOM_RESET: usize = 125;
const SHOW_TAB_MENU: u32 = WM_APP + 2;
const WHEEL_CONTROL_MASK: WPARAM = 0x0008;
const NEW_TAB: usize = 126;
const CLOSE_OTHER_TABS: usize = 127;
const CLOSE_TABS_RIGHT: usize = 128;
const CLOSE_WINDOW: usize = 129;
const EXIT: usize = 130;
const TOGGLE_STATUS: usize = 131;
const TOGGLE_WRAP: usize = 132;
const SAVE: usize = 133;
const TOGGLE_MARKDOWN: usize = 134;
const READING_VIEW: usize = 135;
const UNDO: usize = 136;
const REDO: usize = 137;
const CUT_TEXT: usize = 138;
const PASTE_TEXT: usize = 139;
const NEW_WINDOW: usize = 140;
const NEW_MARKDOWN: usize = 141;
const SAVE_AS: usize = 142;
const SAVE_ALL: usize = 143;
const PRINT: usize = 145;
const DELETE_TEXT: usize = 154;
const CLEAR_FORMATTING: usize = 155;
const GO_TO_LINE: usize = 156;
const INSERT_TIMESTAMP: usize = 157;
const CHOOSE_FONT: usize = 158;
const MENU_BAR: i32 = 32;
const STATUS_BAR: i32 = 28;

struct Controls {
    document: HWND,
    query: HWND,
    replacement: HWND,
    search: HWND,
}

impl Controls {
    fn create(window: HWND) -> io::Result<Self> {
        let search = child(
            window,
            SEARCH_SURFACE,
            SEARCH_CLASS,
            "",
            WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
        )?;
        let query = child(
            search,
            SEARCH_EDIT,
            "EDIT",
            "",
            WS_TABSTOP | WS_CLIPSIBLINGS | ES_AUTOHSCROLL as u32,
        )?;
        let replacement = child(
            search,
            REPLACE_EDIT,
            "EDIT",
            "",
            WS_TABSTOP | WS_CLIPSIBLINGS | ES_AUTOHSCROLL as u32,
        )?;
        let document = child(
            window,
            READER,
            rich_edit::CLASS,
            "",
            rich_edit::STYLE | WS_VISIBLE | WS_VSCROLL | WS_TABSTOP | WS_CLIPSIBLINGS,
        )?;
        Ok(Self {
            document,
            query,
            replacement,
            search,
        })
    }
}

struct Tab {
    width: i32,
    document: Rc<Document>,
    position: POINT,
    trail: Vec<PathBuf>,
    draft: Option<String>,
    deferred: bool,
    caret: CharRange,
    reading: bool,
}

impl Tab {
    fn new(document: Rc<Document>) -> Self {
        Self {
            width: tab_width(&document.title),
            document,
            position: POINT { x: 0, y: 0 },
            trail: Vec::new(),
            draft: None,
            deferred: false,
            caret: CharRange::default(),
            reading: false,
        }
    }
    fn restore(path: PathBuf) -> Result<Self, String> {
        let path = Document::absolute_path(&path)?;
        let mut tab = Self::new(Rc::new(Document::from_source(String::new(), Some(path))));
        tab.deferred = true;
        Ok(tab)
    }

    fn load(&mut self) -> Result<(), String> {
        if self.deferred {
            let path = self.document.path.as_deref().unwrap();
            self.document = Rc::new(Document::open(path)?);
            self.deferred = false;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkdownMode {
    Live,
    Source,
    Reading,
}

struct App {
    window: HWND,
    controls: Controls,
    document: Rc<Document>,
    tabs: Vec<Tab>,
    active_tab: usize,
    tab_scroll: i32,
    tab_motion: Option<Motion>,
    hover: Option<Action>,
    pressed: Option<Action>,
    zoom: u32,
    zoom_delta: i32,
    zoom_motion: Option<ZoomMotion>,
    show_status: bool,
    word_wrap: bool,
    status_selection: Option<CharRange>,
    markdown_mode: MarkdownMode,
    pending_markdown: Option<MarkdownMode>,
    markdown_pref_dirty: bool,
    live_lines: Vec<live_markdown::Line>,
    active_line: Option<usize>,
    edit_pending: bool,
    markdown_hover: bool,
    char_count: usize,
    cursor: rich_edit::Cursor,
    tab_context: Option<usize>,
    rendered: Option<Rendered>,
    theme: Theme,
    paper_brush: HBRUSH,
    settings: Option<PathBuf>,
    trail: Vec<PathBuf>,
    width_twips: i32,
    dpi: i32,
    find_open: bool,
    find_expanded: bool,
    find_offset: i32,
    find_height: i32,
    find_slide: Option<Motion>,
    find_growth: Option<Motion>,
    find_case: bool,
    find_wrap: bool,
    find_hover: Option<FindButton>,
    matches: find_text::Matches,
    find_count: String,
    separate_window: bool,
    printing: printing::PageSetup,
    popup: HWND,
    layout: Layout,
    scroll: ScrollMetrics,
    scroll_drag: Option<i32>,
    wheel_delta: i32,
    scroll_motion: Option<Motion>,
    thumb_motion: Option<Motion>,
    thumb_emphasis: i32,
    track_hover: bool,
    measured_scroll: bool,
    scroll_limit: Option<i32>,
    animations: bool,
}

struct Layout {
    client: RECT,
    track: RECT,
    reader: RECT,
    toolbar_bottom: i32,
    status_top: i32,
}

fn main() {
    if let Err(error) = run() {
        alert(
            ptr::null_mut(),
            &format!("Could not start Sketio Viewer.\n\n{error}"),
        );
    }
}

fn run() -> io::Result<()> {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    unsafe { SetCurrentProcessExplicitAppUserModelID(wide(APP_USER_MODEL_ID).as_ptr()) };
    rich_edit::load_control()?;
    let settings = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|local| local.join(SETTINGS_DIRECTORY));
    text_tools::load_font(settings.as_deref());
    let theme = match settings
        .as_deref()
        .and_then(|path| fs::read_to_string(path.join("theme.txt")).ok())
        .as_deref()
        .map(str::trim)
    {
        Some("dark") => Theme::Dark,
        _ => Theme::Light,
    };
    let show_status = read_view_option(settings.as_deref(), "status-bar.txt");
    let word_wrap = read_view_option(settings.as_deref(), "word-wrap.txt");
    let markdown_mode = if read_view_option(settings.as_deref(), "markdown.txt") {
        MarkdownMode::Live
    } else {
        MarkdownMode::Source
    };
    let size = read_window_size(settings.as_deref());
    let dpi = unsafe { GetDpiForSystem() }.max(96) as i32;
    let (initial_width, initial_height) = initial_size(size, dpi);
    let separate_window = env::args_os().any(|arg| arg == "--new-window");
    let Startup {
        tabs,
        active,
        zoom,
        error: startup_error,
    } = startup_tabs(settings.as_deref(), separate_window);
    let document = Rc::clone(&tabs[active].document);
    let position = tabs[active].position;
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    floating::register(instance)?;
    let search_name = wide(SEARCH_CLASS);
    let search_class = WNDCLASSW {
        lpfnWndProc: Some(search_proc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        lpszClassName: search_name.as_ptr(),
        ..unsafe { mem::zeroed() }
    };
    if unsafe { RegisterClassW(&search_class) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let class_name = wide(WINDOW_CLASS);
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hIcon: unsafe { LoadIconW(instance, APPLICATION_ICON) },
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        lpszClassName: class_name.as_ptr(),
        ..unsafe { mem::zeroed() }
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let window = unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            wide(window_title(&document)).as_ptr(),
            WS_POPUP
                | WS_THICKFRAME
                | WS_SYSMENU
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            initial_width,
            initial_height,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if window.is_null() {
        return Err(io::Error::last_os_error());
    }
    set_window_icons(window);
    floating::disable_shadow(window);
    let controls = Controls::create(window)?;
    let app = Box::new(RefCell::new(App {
        window,
        controls,
        tabs,
        active_tab: active,
        tab_scroll: 0,
        tab_motion: None,
        hover: None,
        pressed: None,
        zoom,
        zoom_delta: 0,
        zoom_motion: None,
        show_status,
        word_wrap,
        status_selection: None,
        markdown_mode,
        pending_markdown: None,
        markdown_pref_dirty: false,
        live_lines: Vec::new(),
        active_line: None,
        edit_pending: false,
        markdown_hover: false,
        char_count: document.char_count,
        cursor: rich_edit::Cursor::default(),
        tab_context: None,
        document,
        rendered: None,
        theme,
        settings,
        trail: Vec::new(),
        paper_brush: ptr::null_mut(),
        width_twips: 0,
        dpi: unsafe { GetDpiForWindow(window) }.max(96) as i32,
        find_open: false,
        find_expanded: false,
        find_offset: -10,
        find_height: 44,
        find_slide: None,
        find_growth: None,
        find_case: false,
        find_wrap: true,
        find_hover: None,
        matches: find_text::Matches::default(),
        find_count: String::new(),
        separate_window,
        printing: printing::PageSetup::default(),
        popup: ptr::null_mut(),
        layout: Layout::new(initial_width, initial_height, dpi, show_status),
        scroll: ScrollMetrics::default(),
        scroll_drag: None,
        wheel_delta: 0,
        scroll_motion: None,
        thumb_motion: None,
        thumb_emphasis: 0,
        track_hover: false,
        measured_scroll: false,
        scroll_limit: None,
        animations: floating::animations_enabled(),
    }));
    unsafe {
        SetWindowLongPtrW(window, GWLP_USERDATA, ptr::from_ref(app.as_ref()) as isize);
        let reader = app.borrow().controls.document;
        if SetWindowSubclass(
            reader,
            Some(editor_proc),
            1,
            ptr::from_ref(app.as_ref()) as usize,
        ) == 0
        {
            DestroyWindow(window);
            return Err(io::Error::last_os_error());
        }
        DragAcceptFiles(window, 1);
        let corners = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            window,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            ptr::from_ref(&corners).cast(),
            mem::size_of_val(&corners) as u32,
        );
    }
    {
        let mut app = app.borrow_mut();
        app.apply_theme();
        app.resize();
        app.reveal_active_tab();
        if !app.word_wrap {
            rich_edit::set_wrap(app.controls.document, false);
        }
        app.refresh(false).map_err(io::Error::other)?;
        rich_edit::set_position(app.controls.document, position);
        app.sync_scroll();
        app.save_session();
    }
    unsafe {
        ShowWindow(
            window,
            if size.maximized {
                SW_SHOWMAXIMIZED
            } else {
                SW_SHOWNORMAL
            },
        );
        UpdateWindow(window);
        let reader = app.borrow().controls.document;
        SetFocus(reader);
    }
    if let Some(error) = startup_error {
        alert(window, &error);
    }
    let mut message: MSG = unsafe { mem::zeroed() };
    loop {
        let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
        if result == 0 {
            break;
        }
        if result == -1 {
            return Err(io::Error::last_os_error());
        }
        if matches!(
            message.message,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_NCLBUTTONDOWN
        ) {
            let mut app = app.borrow_mut();
            if message.hwnd != app.popup {
                app.close_popup();
            }
        }
        let for_popup = message.hwnd == app.borrow().popup;
        if !for_popup
            && matches!(message.message, WM_MOUSEWHEEL | WM_MOUSEHWHEEL)
            && app
                .borrow_mut()
                .route_wheel(message.message, message.wParam, message.lParam)
        {
            continue;
        }
        if !for_popup && app.borrow_mut().handle_key(&message) {
            app.borrow_mut().sync_scroll();
            continue;
        }
        unsafe {
            let editor_input = {
                let mut app = app.borrow_mut();
                if message.hwnd == app.controls.document
                    && matches!(message.message, WM_KEYDOWN | WM_CHAR)
                {
                    app.apply_pending_markdown();
                }
                app.editable() && message.hwnd == app.controls.document
            };
            if editor_input || IsDialogMessageW(window, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        app.borrow_mut().sync_scroll();
    }
    Ok(())
}

impl Layout {
    fn new(width: i32, height: i32, dpi: i32, show_status: bool) -> Self {
        let s = |value| dpi * value / 96;
        let toolbar_bottom = s(TITLE_HEIGHT) + s(MENU_BAR);
        let body_top = toolbar_bottom;
        let status_top = (height - if show_status { s(STATUS_BAR) } else { 0 }).max(body_top);
        let track_left = (width - s(20)).max(1);
        let reader = rect(s(16), body_top + s(8), track_left, status_top - s(6));
        Self {
            client: rect(0, 0, width, height),
            track: rect(track_left, reader.top, width, reader.bottom),
            reader,
            toolbar_bottom,
            status_top,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct ScrollMetrics {
    offset: i32,
    range: i32,
    viewport: i32,
}

impl ScrollMetrics {
    fn read(reader: HWND) -> Self {
        let mut info = SCROLLINFO {
            cbSize: mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            ..unsafe { mem::zeroed() }
        };
        if unsafe { GetScrollInfo(reader, SB_VERT, &mut info) } == 0 {
            return Self::default();
        }
        let viewport = info.nPage as i32;
        Self {
            offset: (info.nPos - info.nMin).max(0),
            range: (info.nMax - info.nMin - viewport + 1).max(0),
            viewport,
        }
    }

    fn thumb(self, track: RECT, minimum: i32) -> Option<RECT> {
        let height = track.bottom - track.top;
        if self.range == 0 || self.viewport == 0 || height <= minimum {
            return None;
        }
        let length = scale_position(self.viewport, self.range + self.viewport, height)
            .clamp(minimum, height);
        let offset = scale_position(self.offset.min(self.range), self.range, height - length);
        Some(rect(
            track.left,
            track.top + offset,
            track.right,
            track.top + offset + length,
        ))
    }
}

#[derive(Clone, Copy)]
struct Motion {
    from: i32,
    to: i32,
    start: Instant,
    duration: Duration,
}

impl Motion {
    fn new(from: i32, to: i32, duration_ms: u64) -> Self {
        Self {
            from,
            to,
            start: Instant::now(),
            duration: Duration::from_millis(duration_ms),
        }
    }

    fn sample(self) -> (i32, bool) {
        let progress = (self.start.elapsed().as_secs_f64() / self.duration.as_secs_f64()).min(1.0);
        let ease = 1.0 - (1.0 - progress) * (1.0 - progress);
        (
            (f64::from(self.from) + f64::from(self.to - self.from) * ease).round() as i32,
            progress >= 1.0,
        )
    }
}

#[derive(Clone, Copy)]
struct ZoomMotion {
    scale: Motion,
    anchor: Option<ZoomAnchor>,
}

#[derive(Clone, Copy)]
enum ZoomAnchor {
    Top,
    Character { index: i32, point: POINT },
}

fn scale_position(value: i32, from: i32, to: i32) -> i32 {
    (i64::from(value) * i64::from(to) / i64::from(from.max(1))) as i32
}

impl App {
    fn save_session(&self) {
        if self.separate_window {
            return;
        }
        let current = rich_edit::position(self.controls.document);
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let position = if index == self.active_tab {
                    current
                } else {
                    tab.position
                };
                session::Tab::new(
                    tab.document.path.as_deref(),
                    [position.x, position.y],
                    tab.document.untitled,
                )
            })
            .collect();
        let workspace = session::Workspace {
            tabs,
            active: self.active_tab,
            zoom: self.requested_zoom(),
        };
        let _ = session::save(self.settings.as_deref(), &workspace);
    }

    fn chrome(&self) -> Chrome {
        Chrome::new(
            self.layout.client.right,
            self.dpi,
            &self.tabs.iter().map(|tab| tab.width).collect::<Vec<_>>(),
            self.tab_scroll,
        )
    }

    fn reveal_active_tab(&mut self) {
        self.tab_motion = None;
        self.tab_scroll = self.chrome().reveal(self.active_tab);
        self.redraw_tab_strip();
    }

    fn redraw_chrome(&self) {
        let chrome = rect(0, 0, self.layout.client.right, self.layout.toolbar_bottom);
        unsafe { InvalidateRect(self.window, &chrome, 0) };
    }

    fn redraw_tab_strip(&self) {
        let strip = rect(0, 0, self.layout.client.right, self.px(TITLE_HEIGHT));
        unsafe { InvalidateRect(self.window, &strip, 0) };
    }

    fn scroll_tabs_by(&mut self, distance: i32) {
        let chrome = self.chrome();
        let from = chrome.scroll;
        let target = self
            .tab_motion
            .map_or(from, |motion| motion.to)
            .saturating_add(distance)
            .clamp(0, chrome.max_scroll);
        if target == from {
            self.tab_motion = None;
            return;
        }
        if self.animations {
            self.tab_motion = Some(Motion::new(from, target, 118));
            self.start_animation_timer();
        } else {
            self.tab_scroll = target;
            self.redraw_tab_strip();
        }
    }

    fn route_wheel(&mut self, message: u32, word: WPARAM, long: LPARAM) -> bool {
        let mut point = mouse_point(long);
        unsafe { ScreenToClient(self.window, &mut point) };
        let control =
            word & WHEEL_CONTROL_MASK != 0 || unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
        if !control && self.chrome().over_tab_strip(point) {
            let delta = i32::from((word >> 16) as i16);
            let direction = if message == WM_MOUSEHWHEEL { 1 } else { -1 };
            self.scroll_tabs_by(direction * delta * self.px(78) / WHEEL_DELTA as i32);
            true
        } else if message == WM_MOUSEWHEEL {
            let delta = i32::from((word >> 16) as i16);
            if control {
                self.zoom_delta += delta;
                let steps = self.zoom_delta / WHEEL_DELTA as i32;
                self.zoom_delta %= WHEEL_DELTA as i32;
                if steps != 0 {
                    let reader = self.layout.reader;
                    let pointer = (point.x >= reader.left
                        && point.x < reader.right
                        && point.y >= reader.top
                        && point.y < reader.bottom)
                        .then(|| {
                            let mut point = mouse_point(long);
                            unsafe { ScreenToClient(self.controls.document, &mut point) };
                            point
                        });
                    self.set_zoom(
                        self.requested_zoom().saturating_add_signed(steps * 10),
                        pointer,
                    );
                }
            } else {
                self.finish_zoom();
                self.scroll_wheel(delta);
            }
            true
        } else {
            false
        }
    }

    fn remember_tab(&mut self) {
        self.apply_pending_markdown();
        self.finish_zoom();
        self.flush_edit();
        let tab = &mut self.tabs[self.active_tab];
        if self.rendered.is_none() {
            tab.caret = rich_edit::selection(self.controls.document);
        }
        tab.position = rich_edit::position(self.controls.document);
        tab.trail = mem::take(&mut self.trail);
    }

    fn show_tab(&mut self, index: usize) {
        self.active_tab = index;
        let tab = &mut self.tabs[index];
        self.document = Rc::clone(&tab.document);
        self.trail = mem::take(&mut tab.trail);
        let position = tab.position;
        if let Err(error) = self.refresh(false) {
            alert(self.window, &error);
        }
        if self.editable() {
            rich_edit::select(self.controls.document, self.tabs[index].caret, false);
            self.update_active_line();
        }
        unsafe { SetFocus(self.controls.document) };
        rich_edit::set_position(self.controls.document, position);
        self.sync_scroll();
        self.reveal_active_tab();
        self.save_session();
    }

    fn switch_tab(&mut self, index: usize) {
        if index != self.active_tab && index < self.tabs.len() {
            if let Err(error) = self.tabs[index].load() {
                alert(self.window, &error);
                return;
            }
            self.remember_tab();
            self.show_tab(index);
        }
    }

    fn step_tab(&mut self, previous: bool) {
        let count = self.tabs.len();
        let target = if previous {
            (self.active_tab + count - 1) % count
        } else {
            (self.active_tab + 1) % count
        };
        self.switch_tab(target);
    }

    fn close_tab(&mut self, index: usize) {
        self.apply_pending_markdown();
        if index >= self.tabs.len() || !self.confirm_tab(index) {
            return;
        }
        self.tabs.remove(index);
        if index == self.active_tab {
            let (next, error) = load_available_tab(&mut self.tabs, index, false);
            self.show_tab(next);
            if let Some(error) = error {
                alert(self.window, &error);
            }
        } else {
            if index < self.active_tab {
                self.active_tab -= 1;
            }
            self.reveal_active_tab();
            self.save_session();
            self.redraw();
        }
    }

    fn add_document(&mut self, path: &Path) {
        let path = match Document::absolute_path(path) {
            Ok(path) => path,
            Err(error) => {
                alert(self.window, &error);
                return;
            }
        };
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.document.path.as_ref() == Some(&path))
        {
            self.switch_tab(index);
            return;
        }
        let document = match Document::open(&path) {
            Ok(document) => document,
            Err(error) => {
                alert(self.window, &error);
                return;
            }
        };
        let tab = Tab::new(Rc::new(document));
        if !self.document.can_edit()
            || (self.document.untitled.is_some()
                && self.tabs[self.active_tab].draft.is_none()
                && !self.edit_pending)
        {
            self.tabs[self.active_tab] = tab;
            self.show_tab(self.active_tab);
        } else {
            self.remember_tab();
            self.tabs.push(tab);
            self.show_tab(self.tabs.len() - 1);
        }
    }

    fn new_tab(&mut self, markdown: bool) {
        self.remember_tab();
        self.tabs
            .push(Tab::new(Rc::new(Document::untitled(markdown))));
        self.show_tab(self.tabs.len() - 1);
    }

    fn close_other_tabs(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if !(0..self.tabs.len())
            .filter(|&other| other != index)
            .all(|other| self.confirm_tab(other))
        {
            return;
        }
        self.switch_tab(index);
        if self.active_tab != index {
            return;
        }
        let keep = self.tabs.swap_remove(index);
        self.tabs.clear();
        self.tabs.push(keep);
        self.active_tab = 0;
        self.reveal_active_tab();
        self.save_session();
        self.redraw();
    }

    fn close_tabs_to_right(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if !(index + 1..self.tabs.len()).all(|other| self.confirm_tab(other)) {
            return;
        }
        if self.active_tab > index {
            self.switch_tab(index);
        }
        if self.active_tab > index {
            return;
        }
        self.tabs.truncate(index + 1);
        self.reveal_active_tab();
        self.save_session();
        self.redraw();
    }

    fn activate_chrome(&mut self, action: Action) {
        match action {
            Action::Tab(index) => self.switch_tab(index),
            Action::CloseTab(index) => self.close_tab(index),
            Action::Open => self.command(OPEN, 0),
            Action::ScrollLeft | Action::ScrollRight => {
                let chrome = self.chrome();
                if chrome.action_enabled(action) {
                    let step = (chrome.track_width * 2 / 3).max(self.px(80));
                    self.scroll_tabs_by(if action == Action::ScrollLeft {
                        -step
                    } else {
                        step
                    });
                }
            }
            Action::Minimize => unsafe {
                ShowWindow(self.window, SW_MINIMIZE);
            },
            Action::Maximize => {
                unsafe {
                    ShowWindow(
                        self.window,
                        if IsZoomed(self.window) != 0 {
                            SW_RESTORE
                        } else {
                            SW_MAXIMIZE
                        },
                    );
                }
                self.resize();
                let _ = self.refresh(true);
            }
            Action::CloseWindow => unsafe {
                PostMessageW(self.window, WM_CLOSE, 0, 0);
            },
            Action::Menu(menu) => unsafe {
                PostMessageW(self.window, SHOW_MENU, menu as WPARAM, 0);
            },
        }
    }

    fn close_popup(&mut self) {
        let previous = mem::replace(&mut self.popup, ptr::null_mut());
        if !previous.is_null() && unsafe { IsWindow(previous) } != 0 {
            unsafe { PostMessageW(previous, WM_CLOSE, 0, 0) };
        }
    }

    fn update_hover(&mut self, point: POINT) {
        let status_hover = self.markdown_hit(point);
        if status_hover != self.markdown_hover {
            self.markdown_hover = status_hover;
            self.redraw_status();
        }
        let action = self.chrome().hit(point);
        if action == self.hover && !status_hover {
            return;
        }
        let chrome_changed = self.hover != action;
        self.hover = action;
        let mut track = TRACKMOUSEEVENT {
            cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: self.window,
            dwHoverTime: 0,
        };
        unsafe { TrackMouseEvent(&mut track) };
        if chrome_changed {
            self.redraw_chrome();
        }
    }

    fn requested_zoom(&self) -> u32 {
        self.zoom_motion
            .map_or(self.zoom, |zoom| zoom.scale.to as u32)
    }

    fn adjust_zoom(&mut self, command: usize) {
        let percent = match command {
            ZOOM_IN => self.requested_zoom() + 10,
            ZOOM_OUT => self.requested_zoom().saturating_sub(10),
            _ => 100,
        };
        self.set_zoom(percent, None);
    }

    fn set_zoom(&mut self, percent: u32, pointer: Option<POINT>) {
        let percent = percent.clamp(60, 200);
        if self.requested_zoom() == percent {
            return;
        }
        let reader = self.controls.document;
        let anchor = if pointer.is_none() && rich_edit::position(reader).y == 0 {
            Some(ZoomAnchor::Top)
        } else {
            let point = pointer.unwrap_or(POINT {
                x: self.px(12),
                y: 1,
            });
            rich_edit::char_at(reader, point).and_then(|index| {
                rich_edit::char_position(reader, index)
                    .map(|point| ZoomAnchor::Character { index, point })
            })
        };
        self.scroll_motion = None;
        self.zoom_motion = Some(ZoomMotion {
            scale: Motion::new(self.zoom as i32, percent as i32, 155),
            anchor,
        });
        if self.animations {
            self.start_animation_timer();
        } else {
            self.finish_zoom();
        }
    }

    fn restore_zoom_anchor(&self, anchor: Option<ZoomAnchor>) {
        let reader = self.controls.document;
        let Some(anchor) = anchor else {
            return;
        };
        let current = rich_edit::position(reader);
        let target = match anchor {
            ZoomAnchor::Top => POINT { x: current.x, y: 0 },
            ZoomAnchor::Character { index, point } => {
                let Some(new_point) = rich_edit::char_position(reader, index) else {
                    return;
                };
                POINT {
                    x: if self.word_wrap {
                        current.x
                    } else {
                        (current.x + new_point.x - point.x).max(0)
                    },
                    y: (current.y + new_point.y - point.y).max(0),
                }
            }
        };
        if target.x != current.x || target.y != current.y {
            rich_edit::set_position(reader, target);
        }
    }

    fn apply_zoom(&mut self, percent: u32, anchor: Option<ZoomAnchor>) {
        if self.zoom == percent {
            return;
        }
        let reader = self.controls.document;
        unsafe { SendMessageW(reader, WM_SETREDRAW, 0, 0) };
        if rich_edit::zoom(reader, percent) {
            self.zoom = percent;
            self.measured_scroll = false;
            self.scroll_limit = None;
            self.restore_zoom_anchor(anchor);
        }
        unsafe {
            SendMessageW(reader, WM_SETREDRAW, 1, 0);
            InvalidateRect(reader, ptr::null(), 0);
        }
        self.sync_scroll();
        unsafe {
            InvalidateRect(
                self.window,
                &rect(
                    0,
                    self.layout.status_top,
                    self.layout.client.right,
                    self.layout.client.bottom,
                ),
                0,
            );
        }
    }

    fn finish_zoom(&mut self) {
        if let Some(zoom) = self.zoom_motion.take() {
            self.apply_zoom(zoom.scale.to as u32, zoom.anchor);
            self.reflow(zoom.anchor);
            self.save_session();
        }
    }

    fn document_width_twips(&self) -> i32 {
        let width = (self.layout.reader.right - self.layout.reader.left - self.px(12)).max(1);
        width * 144_000 / self.dpi / self.zoom as i32
    }

    fn reflow(&mut self, anchor: Option<ZoomAnchor>) {
        let width_twips = self.document_width_twips();
        if (width_twips - self.width_twips).abs() <= 30 {
            return;
        }
        if self.live_enabled() {
            self.width_twips = width_twips;
            let tables: Vec<_> = self
                .live_lines
                .iter()
                .enumerate()
                .filter_map(|(index, line)| line.block.table.is_some().then_some(index))
                .collect();
            if !tables.is_empty() {
                self.apply_line_styles(&tables, None, false);
                self.restore_zoom_anchor(anchor);
                self.sync_scroll();
            }
            return;
        }
        // The native reader rewraps text itself. Images and tables are sized when generating RTF.
        if self
            .rendered
            .as_ref()
            .is_some_and(|view| view.width_sensitive)
        {
            if let Err(error) = self.refresh(true) {
                alert(self.window, &error);
            }
            self.restore_zoom_anchor(anchor);
            self.sync_scroll();
        } else {
            self.width_twips = width_twips;
        }
    }

    fn sync_cursor(&mut self) {
        if !self.show_status {
            return;
        }
        let selection = rich_edit::selection(self.controls.document);
        if self.status_selection == Some(selection) {
            return;
        }
        self.status_selection = Some(selection);
        self.cursor = rich_edit::cursor(self.controls.document, selection);
        self.redraw_status();
    }

    fn redraw_status(&self) {
        let status = rect(
            0,
            self.layout.status_top,
            self.layout.client.right,
            self.layout.client.bottom,
        );
        unsafe { InvalidateRect(self.window, &status, 0) };
    }

    fn toggle_status(&mut self) {
        let position = rich_edit::position(self.controls.document);
        self.show_status = !self.show_status;
        self.resize();
        rich_edit::set_position(self.controls.document, position);
        self.status_selection = None;
        self.sync_cursor();
        self.sync_scroll();
        save_view_option(self.settings.as_deref(), "status-bar.txt", self.show_status);
    }

    fn toggle_wrap(&mut self) {
        let reader = self.controls.document;
        let mut position = rich_edit::position(reader);
        self.word_wrap = !self.word_wrap;
        self.scroll_motion = None;
        self.measured_scroll = false;
        self.scroll_limit = None;
        rich_edit::set_wrap(reader, self.word_wrap);
        if self.word_wrap {
            position.x = 0;
        }
        rich_edit::set_position(reader, position);
        self.status_selection = None;
        self.sync_cursor();
        self.sync_scroll();
        save_view_option(self.settings.as_deref(), "word-wrap.txt", self.word_wrap);
    }

    fn palette(&self) -> Palette {
        Palette::new(self.theme == Theme::Dark)
    }

    fn px(&self, value: i32) -> i32 {
        self.dpi * value / 96
    }

    fn apply_theme(&mut self) {
        text_tools::set_dialog_theme(matches!(self.theme, Theme::Dark));
        if !self.paper_brush.is_null() {
            unsafe { DeleteObject(self.paper_brush) };
        }
        self.paper_brush = unsafe { CreateSolidBrush(self.palette().paper) };
        let dark = i32::from(self.theme == Theme::Dark);
        unsafe {
            DwmSetWindowAttribute(
                self.window,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                ptr::from_ref(&dark).cast(),
                mem::size_of_val(&dark) as u32,
            );
        }
    }

    fn resize(&mut self) {
        self.measured_scroll = false;
        self.scroll_limit = None;
        let mut client = rect(0, 0, 0, 0);
        unsafe { GetClientRect(self.window, &mut client) };
        self.layout = Layout::new(client.right, client.bottom, self.dpi, self.show_status);
        let reader = self.layout.reader;
        let width = (reader.right - reader.left).max(1);
        let height = (reader.bottom - reader.top).max(1);
        let native_scroll_width = unsafe { GetSystemMetricsForDpi(SM_CXVSCROLL, self.dpi as u32) };
        // The clipped part preserves RichEdit's scroll state while our track owns the visible UI.
        place(
            self.controls.document,
            reader.left,
            reader.top,
            width + native_scroll_width,
            height,
        );
        let region = unsafe { CreateRectRgn(0, 0, width, height) };
        if !region.is_null() && unsafe { SetWindowRgn(self.controls.document, region, 1) } == 0 {
            unsafe { DeleteObject(region) };
        }
        rich_edit::size(self.controls.document, width, height, self.px(6));
        self.place_search();
        self.sync_scroll();
        self.redraw();
    }

    fn place_search(&self) {
        let search = self.controls.search;
        if !self.find_open && self.find_slide.is_none() {
            visible(search, false);
            return;
        }
        let width = self.px(478).min(self.layout.client.right - self.px(24));
        let height = self.px(self.find_height);
        let x = (self.layout.client.right - width) / 2;
        let y = self.layout.toolbar_bottom + self.px(12 + self.find_offset);
        let layout = FindLayout::new(width, self.dpi);
        place(search, x, y, width, height);
        floating::clip_round(search, width, height, self.px(14));
        for (input, r, label) in [
            (self.controls.query, layout.query, "Find"),
            (
                self.controls.replacement,
                layout.replacement,
                "Replace with",
            ),
        ] {
            place(
                input,
                r.left,
                r.top,
                (r.right - r.left).max(1),
                r.bottom - r.top,
            );
            unsafe {
                SendMessageW(input, WM_SETFONT, interface_font(self.dpi) as WPARAM, 0);
                SendMessageW(input, EM_SETCUEBANNER, 1, wide(label).as_ptr() as LPARAM);
            }
        }
        visible(self.controls.query, true);
        visible(
            self.controls.replacement,
            self.find_expanded && self.find_height >= 68 && self.editable(),
        );
        unsafe {
            SetWindowPos(
                search,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            InvalidateRect(search, ptr::null(), 0);
        }
    }

    fn show_find(&mut self, open: bool) {
        if self.find_open != open {
            self.find_open = open;
            let offset = if open { 0 } else { -10 };
            if self.animations {
                self.find_slide = Some(Motion::new(self.find_offset, offset, 150));
                self.start_animation_timer();
            } else {
                self.find_offset = offset;
            }
        }
        self.place_search();
        if open {
            self.rebuild_search();
            unsafe {
                SetFocus(self.controls.query);
                SendMessageW(self.controls.query, EM_SETSEL, 0, -1);
            }
        } else {
            self.find_hover = None;
            unsafe { SetFocus(self.controls.document) };
        }
    }

    fn expand_find(&mut self, expanded: bool) {
        let expanded = expanded && self.editable();
        if expanded == self.find_expanded {
            return;
        }
        self.find_expanded = expanded;
        let height = if expanded { 81 } else { 44 };
        if self.animations && self.find_open {
            self.find_growth = Some(Motion::new(self.find_height, height, 135));
            self.start_animation_timer();
        } else {
            self.find_height = height;
        }
        self.place_search();
    }

    fn repaint_find(&self) {
        unsafe { InvalidateRect(self.controls.search, ptr::null(), 0) };
    }

    fn sync_scroll(&mut self) {
        let control = self.controls.document;
        let mut metrics = ScrollMetrics::read(control);
        if let Some(limit) = self.scroll_limit {
            if metrics.offset > limit {
                let position = rich_edit::position(control);
                rich_edit::set_position(
                    control,
                    POINT {
                        x: position.x,
                        y: position.y.saturating_sub(metrics.offset - limit).max(0),
                    },
                );
                metrics = ScrollMetrics::read(control);
            }
            metrics.range = metrics.range.min(limit);
            metrics.offset = metrics.offset.min(metrics.range);
        }
        if self.scroll != metrics {
            let moved = self.scroll.offset != metrics.offset;
            self.scroll = metrics;
            unsafe {
                InvalidateRect(self.window, &self.layout.track, 0);
                if moved && self.live_enabled() {
                    InvalidateRect(control, ptr::null(), 0);
                }
            }
        }
    }

    fn update_track_hover(&mut self, point: Option<POINT>) {
        let track = self.layout.track;
        let hover = self.scroll_drag.is_some()
            || point.is_some_and(|p| {
                p.x >= track.left && p.x < track.right && p.y >= track.top && p.y < track.bottom
            });
        if hover == self.track_hover {
            return;
        }
        self.track_hover = hover;
        if hover {
            let mut track = TRACKMOUSEEVENT {
                cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.window,
                dwHoverTime: 0,
            };
            unsafe { TrackMouseEvent(&mut track) };
        }
        let target = if hover { 1_000 } else { 0 };
        if self.animations {
            self.thumb_motion = Some(Motion::new(self.thumb_emphasis, target, 105));
            self.start_animation_timer();
        } else {
            self.thumb_emphasis = target;
            unsafe { InvalidateRect(self.window, &track, 0) };
        }
    }

    fn start_animation_timer(&self) {
        unsafe { SetTimer(self.window, SCROLL_TIMER, 15, None) };
    }

    fn measure_scroll_extent(&mut self) {
        if self.measured_scroll || self.scroll.range == 0 {
            return;
        }
        let control = self.controls.document;
        let position = rich_edit::position(control);
        unsafe {
            SendMessageW(control, WM_SETREDRAW, 0, 0);
            SendMessageW(control, WM_VSCROLL, SB_BOTTOM as WPARAM, 0);
            SendMessageW(control, EM_GETLINECOUNT, 0, 0);
            SendMessageW(control, WM_VSCROLL, SB_BOTTOM as WPARAM, 0);
        }
        let bottom_scroll = ScrollMetrics::read(control);
        self.scroll_limit = rich_edit::last_line_y(control).map(|last| {
            // RichEdit can include blank space after the last displayed paragraph in its range.
            let padding = self.px(46) * self.zoom as i32 / 100;
            (bottom_scroll.offset + last + padding - bottom_scroll.viewport)
                .max(0)
                .min(bottom_scroll.range)
        });
        let target = self
            .scroll_limit
            .map_or(position.y, |limit| position.y.min(limit));
        rich_edit::set_position(
            control,
            POINT {
                x: position.x,
                y: target,
            },
        );
        unsafe {
            SendMessageW(control, WM_SETREDRAW, 1, 0);
            InvalidateRect(control, ptr::null(), 0);
        }
        self.sync_scroll();
        self.measured_scroll = true;
    }

    fn animate_offset(&mut self, offset: i32, duration_ms: u64) {
        let control = self.controls.document;
        let position = rich_edit::position(control);
        let target = position
            .y
            .saturating_add(offset.clamp(0, self.scroll.range) - self.scroll.offset)
            .max(0);
        if target == position.y {
            self.scroll_motion = None;
            return;
        }
        if self.animations {
            self.scroll_motion = Some(Motion::new(position.y, target, duration_ms));
            self.start_animation_timer();
        } else {
            rich_edit::set_position(
                control,
                POINT {
                    x: position.x,
                    y: target,
                },
            );
            self.sync_scroll();
        }
    }

    fn animation_frame(&mut self) {
        let animated_find = self.find_slide.is_some() || self.find_growth.is_some();
        if let Some(motion) = self.find_slide {
            let (offset, finished) = motion.sample();
            self.find_offset = offset;
            if finished {
                self.find_slide = None;
            }
        }
        if let Some(motion) = self.find_growth {
            let (height, finished) = motion.sample();
            self.find_height = height;
            if finished {
                self.find_growth = None;
            }
        }
        if animated_find {
            self.place_search();
        }

        if let Some(zoom) = self.zoom_motion {
            let (percent, finished) = zoom.scale.sample();
            if finished {
                self.finish_zoom();
            } else {
                self.apply_zoom(percent as u32, zoom.anchor);
            }
        }
        if let Some(motion) = self.scroll_motion {
            let (offset, finished) = motion.sample();
            let control = self.controls.document;
            let current = rich_edit::position(control);
            if offset != current.y {
                rich_edit::set_position(
                    control,
                    POINT {
                        x: current.x,
                        y: offset,
                    },
                );
                self.sync_scroll();
            }
            if finished {
                self.scroll_motion = None;
            }
        }
        if let Some(motion) = self.thumb_motion {
            let (width, finished) = motion.sample();
            self.thumb_emphasis = width;
            unsafe { InvalidateRect(self.window, &self.layout.track, 0) };
            if finished {
                self.thumb_motion = None;
            }
        }
        if let Some(motion) = self.tab_motion {
            let (offset, finished) = motion.sample();
            self.tab_scroll = offset;
            self.redraw_tab_strip();
            if finished {
                self.tab_motion = None;
            }
        }
        if self.scroll_motion.is_none()
            && self.thumb_motion.is_none()
            && self.tab_motion.is_none()
            && self.zoom_motion.is_none()
            && self.find_slide.is_none()
            && self.find_growth.is_none()
        {
            unsafe { KillTimer(self.window, SCROLL_TIMER) };
        }
    }

    fn animate_revealed_text(&mut self, before: POINT) {
        let control = self.controls.document;
        if !self.animations {
            self.sync_scroll();
            return;
        }
        let after = rich_edit::position(control);
        if before.y != after.y {
            self.sync_scroll();
            let destination = self.scroll.offset;
            rich_edit::set_position(control, before);
            self.sync_scroll();
            self.animate_offset(destination, 150);
        }
    }

    fn start_scroll(&mut self, p: POINT) -> bool {
        let track = self.layout.track;
        if p.x < track.left || p.x >= track.right || p.y < track.top || p.y >= track.bottom {
            return false;
        }
        self.scroll_motion = None;
        self.measure_scroll_extent();
        let Some(thumb) = self.scroll.thumb(track, self.px(24)) else {
            return false;
        };
        self.scroll_drag = Some(if p.y >= thumb.top && p.y < thumb.bottom {
            p.y - thumb.top
        } else {
            (thumb.bottom - thumb.top) / 2
        });
        unsafe { SetCapture(self.window) };
        self.update_track_hover(Some(p));
        self.scroll_to(p.y);
        true
    }

    fn scroll_to(&mut self, y: i32) {
        let track = self.layout.track;
        let Some((grab, thumb)) = self.scroll_drag.zip(self.scroll.thumb(track, self.px(24)))
        else {
            return;
        };
        let travel = (track.bottom - track.top - (thumb.bottom - thumb.top)).max(1);
        let fraction = (y - track.top - grab).clamp(0, travel);
        self.animate_offset(scale_position(fraction, travel, self.scroll.range), 64);
    }

    fn scroll_wheel(&mut self, delta: i32) {
        self.measure_scroll_extent();
        self.wheel_delta += delta;
        let steps = self.wheel_delta / WHEEL_DELTA as i32;
        self.wheel_delta %= WHEEL_DELTA as i32;
        if steps == 0 {
            return;
        }

        let mut lines = 3u32;
        unsafe {
            SystemParametersInfoW(
                SPI_GETWHEELSCROLLLINES,
                0,
                ptr::from_mut(&mut lines).cast(),
                0,
            )
        };
        let page = self.scroll.viewport.max(self.px(120));
        let line = self.px(27) * self.zoom as i32 / 100;
        let distance = if lines == u32::MAX {
            page
        } else {
            (u64::from(lines) * line as u64).min(page as u64) as i32
        };
        self.scroll_by(-steps.saturating_mul(distance), 150);
    }

    fn scroll_by(&mut self, pixels: i32, duration_ms: u64) {
        let position = rich_edit::position(self.controls.document);
        let pending = self
            .scroll_motion
            .map_or(0, |motion| motion.to - position.y);
        let next = self
            .scroll
            .offset
            .saturating_add(pending)
            .saturating_add(pixels);
        if next > self.scroll.range {
            self.measure_scroll_extent();
        }
        self.animate_offset(next, duration_ms);
    }

    fn redraw(&self) {
        unsafe { InvalidateRect(self.window, ptr::null(), 0) };
    }

    fn refresh(&mut self, preserve: bool) -> Result<(), String> {
        self.zoom = self.requested_zoom();
        self.zoom_motion = None;
        self.scroll_motion = None;
        self.measured_scroll = false;
        self.scroll_limit = None;
        let reader = self.controls.document;
        let old_position = rich_edit::position(reader);
        let old_selection = preserve.then(|| match (self.wants_editor(), self.editable()) {
            (true, false) => self.tabs[self.active_tab].caret,
            (false, true) => CharRange::default(),
            _ => rich_edit::selection(reader),
        });
        let width_twips = self.document_width_twips();
        let draft = self.tabs[self.active_tab].draft.as_deref();
        rich_edit::read_only(reader, !self.wants_editor());
        if self.wants_editor() {
            let source = draft.map_or_else(
                || file_edit::normalized(&self.document.source),
                Cow::Borrowed,
            );
            self.char_count = match (&source, draft) {
                (Cow::Borrowed(_), None) => self.document.char_count,
                _ => source.chars().count(),
            };
            self.rendered = None;
            self.live_lines = if self.live_enabled() {
                live_markdown::lines(&source)
            } else {
                Vec::new()
            };
            let selection = old_selection.unwrap_or_default();
            self.active_line = (!self.live_lines.is_empty())
                .then(|| live_markdown::line_at(&self.live_lines, selection.end));
            let palette = self.palette();
            let styled = self.live_lines.iter().any(live_markdown::Line::formatted)
                && rich_edit::new_styled_text(reader, &self.live_document_rtf(), palette).is_ok();
            if !styled {
                rich_edit::new_text(reader, &source, palette);
            }
            rich_edit::select(reader, selection, false);
            if styled {
                self.set_live_typing_style(selection);
            } else if self.live_lines.iter().any(live_markdown::Line::formatted) {
                self.show_cached_live();
            }
        } else {
            let source = draft.unwrap_or(&self.document.source);
            self.char_count = draft.map_or(self.document.char_count, |text| text.chars().count());
            let mut rendered = render(&self.document, source, self.theme, width_twips);
            text_tools::apply_reading_font(&mut rendered.rtf);
            rich_edit::show(
                reader,
                &rendered.rtf,
                self.palette().paper,
                rendered.links.iter().map(|link| CharRange {
                    start: link.start,
                    end: link.end,
                }),
            )
            .map_err(|error| format!("Could not display this Markdown file. {error}"))?;
            rendered.rtf = String::new();
            self.rendered = Some(rendered);
            self.live_lines = Vec::new();
            self.active_line = None;
            if let Some(selection) = old_selection {
                rich_edit::select(reader, selection, false);
            }
        }
        rich_edit::zoom(reader, self.zoom);
        if preserve {
            rich_edit::set_position(reader, old_position);
        }
        self.width_twips = width_twips;
        if !self.editable() {
            self.expand_find(false);
        }
        if self.find_open {
            self.rebuild_search();
        }
        self.update_title();
        self.status_selection = None;
        self.sync_cursor();
        self.sync_scroll();
        self.redraw();
        Ok(())
    }

    fn markdown_mode(&self) -> MarkdownMode {
        if self.tabs[self.active_tab].reading {
            MarkdownMode::Reading
        } else {
            self.markdown_mode
        }
    }

    fn requested_markdown(&self) -> MarkdownMode {
        self.pending_markdown
            .unwrap_or_else(|| self.markdown_mode())
    }

    fn request_markdown_toggle(&mut self) {
        if !self.document.can_edit() || !self.document.is_markdown() {
            return;
        }
        let next = if self.requested_markdown() == MarkdownMode::Source {
            MarkdownMode::Live
        } else {
            MarkdownMode::Source
        };
        self.pending_markdown = (next != self.markdown_mode()).then_some(next);
        unsafe {
            if self.pending_markdown.is_some() {
                SetTimer(self.window, MARKDOWN_APPLY_TIMER, 24, None);
            } else {
                KillTimer(self.window, MARKDOWN_APPLY_TIMER);
            }
        }
        self.redraw_status();
    }

    fn apply_pending_markdown(&mut self) {
        unsafe { KillTimer(self.window, MARKDOWN_APPLY_TIMER) };
        if let Some(mode) = self.pending_markdown.take() {
            self.set_markdown_mode(mode);
        }
    }

    fn save_markdown_preference(&mut self) {
        unsafe { KillTimer(self.window, MARKDOWN_PREF_TIMER) };
        if mem::take(&mut self.markdown_pref_dirty) {
            save_view_option(
                self.settings.as_deref(),
                "markdown.txt",
                self.markdown_mode == MarkdownMode::Live,
            );
        }
    }

    fn wants_editor(&self) -> bool {
        self.document.can_edit()
            && (!self.document.is_markdown() || self.markdown_mode() != MarkdownMode::Reading)
    }

    fn editable(&self) -> bool {
        self.document.can_edit() && self.rendered.is_none()
    }

    fn live_enabled(&self) -> bool {
        self.document.can_edit()
            && self.document.is_markdown()
            && self.markdown_mode() == MarkdownMode::Live
    }

    fn markdown_button(&self) -> Option<RECT> {
        (self.show_status && self.document.can_edit() && self.document.is_markdown()).then(|| {
            native_paint::markdown_button(
                self.layout.client.right,
                self.layout.status_top,
                self.layout.client.bottom,
                self.dpi,
            )
        })
    }

    fn markdown_hit(&self, point: POINT) -> bool {
        self.markdown_button().is_some_and(|r| {
            point.x >= r.left && point.x < r.right && point.y >= r.top && point.y < r.bottom
        })
    }

    fn update_title(&self) {
        let mut title = window_title(&self.document);
        if self.tabs[self.active_tab].draft.is_some() {
            title.insert_str(0, "* ");
        }
        unsafe { SetWindowTextW(self.window, wide(title).as_ptr()) };
    }

    fn set_markdown_mode(&mut self, mode: MarkdownMode) {
        if !self.document.can_edit() || !self.document.is_markdown() || self.markdown_mode() == mode
        {
            return;
        }
        self.flush_edit();
        let old_editor = self.editable();
        let selection = rich_edit::selection(self.controls.document);
        if old_editor {
            self.tabs[self.active_tab].caret = selection;
        }
        self.tabs[self.active_tab].reading = mode == MarkdownMode::Reading;
        if mode != MarkdownMode::Reading {
            self.markdown_mode = mode;
            self.markdown_pref_dirty = true;
            unsafe { SetTimer(self.window, MARKDOWN_PREF_TIMER, 300, None) };
        }
        if old_editor && self.wants_editor() {
            self.scroll_motion = None;
            self.measured_scroll = false;
            self.scroll_limit = None;
            if self.live_enabled() {
                if self.live_lines.is_empty() {
                    self.live_lines =
                        live_markdown::lines(&rich_edit::source(self.controls.document));
                }
                self.show_cached_live();
            } else {
                let count = rich_edit::text_length(self.controls.document) as i32;
                self.apply_line_styles(&[], Some(count), false);
            }
        } else {
            if let Err(error) = self.refresh(true) {
                alert(self.window, &error);
            }
        }
        unsafe { SetFocus(self.controls.document) };
        self.sync_cursor();
        self.sync_scroll();
        self.redraw_status();
    }

    fn queue_edit(&mut self) {
        self.edit_pending = true;
        unsafe { SetTimer(self.window, EDIT_TIMER, 160, None) };
    }

    fn flush_edit(&mut self) {
        if !mem::take(&mut self.edit_pending) || !self.editable() {
            return;
        }
        unsafe { KillTimer(self.window, EDIT_TIMER) };
        let source = rich_edit::source(self.controls.document);
        if self.live_enabled() {
            self.restyle(live_markdown::lines(&source), false);
        } else {
            self.live_lines.clear();
            self.active_line = None;
        }
        self.char_count = source.chars().count();
        let tab = &mut self.tabs[self.active_tab];
        let was_dirty = tab.draft.is_some();
        tab.draft = (file_edit::normalized(&self.document.source) != source).then_some(source);
        if was_dirty != tab.draft.is_some() {
            self.update_title();
            self.redraw_tab_strip();
        }
        self.status_selection = None;
        self.sync_cursor();
        if self.find_open {
            self.rebuild_search();
        }
        self.sync_scroll();
    }

    fn live_document_rtf(&self) -> String {
        let reader = self.controls.document;
        let dc = unsafe { GetDC(reader) };
        let rtf = live_rtf::render(
            &self.live_lines,
            self.active_line,
            self.palette(),
            |line, raw| self.live_paragraph(line, raw, dc),
        );
        if !dc.is_null() {
            unsafe { ReleaseDC(reader, dc) };
        }
        rtf
    }

    fn set_live_typing_style(&self, selection: CharRange) {
        if selection.start == selection.end
            && let Some(line) = self.active_line.and_then(|i| self.live_lines.get(i))
        {
            let reader = self.controls.document;
            rich_edit::format_without_undo(reader, || {
                rich_edit::set_style(reader, selection, line.block.base, self.palette())
            });
        }
    }

    fn show_cached_live(&mut self) {
        let reader = self.controls.document;
        let selection = rich_edit::selection(reader);
        self.active_line = Some(live_markdown::line_at(&self.live_lines, selection.end));
        if !self.live_lines.iter().any(live_markdown::Line::formatted) {
            return;
        }
        let position = rich_edit::position(reader);
        let anchor = self.line_style_anchor();
        let palette = self.palette();
        let rtf = self.live_document_rtf();
        unsafe { SendMessageW(reader, WM_SETREDRAW, 0, 0) };
        let result = rich_edit::restyle_rtf(reader, &rtf);
        if result.is_ok() {
            rich_edit::select(reader, selection, false);
            self.set_live_typing_style(selection);
            self.restore_line_style_anchor(position, anchor);
        }
        unsafe {
            SendMessageW(reader, WM_SETREDRAW, 1, 0);
            InvalidateRect(reader, ptr::null(), 0);
        }
        if let Err(error) = result {
            // Retain the exact source if the native stream rejects its style data.
            let source = self
                .live_lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            rich_edit::new_text(reader, &source, palette);
            rich_edit::select(reader, selection, false);
            rich_edit::set_position(reader, position);
            let formatted: Vec<_> = self
                .live_lines
                .iter()
                .enumerate()
                .filter_map(|(index, line)| line.formatted().then_some(index))
                .collect();
            self.apply_line_styles(&formatted, None, true);
            eprintln!("Could not stream Markdown styles: {error}");
        }
    }

    fn restyle(&mut self, lines: Vec<live_markdown::Line>, all: bool) {
        let caret = rich_edit::selection(self.controls.document);
        let active = live_markdown::line_at(&lines, caret.end);
        let old = &self.live_lines;
        let prefix = if all {
            0
        } else {
            old.iter()
                .zip(&lines)
                .take_while(|(a, b)| live_markdown::same_style(a, b))
                .count()
        };
        let suffix = if all {
            0
        } else {
            old[prefix..]
                .iter()
                .rev()
                .zip(lines[prefix..].iter().rev())
                .take_while(|(a, b)| live_markdown::same_style(a, b))
                .count()
        };
        let previous = self.active_line.and_then(|index| {
            if index < prefix {
                Some(index)
            } else if index >= old.len() - suffix {
                Some(lines.len() - (old.len() - index))
            } else {
                None
            }
        });
        let reset = all.then(|| lines.last().map_or(0, |line| line.end));
        let mut changed: Vec<_> = (prefix..lines.len() - suffix)
            .filter(|&index| !all || lines[index].formatted())
            .collect();
        changed.extend(previous);
        changed.push(active);
        changed.sort_unstable();
        changed.dedup();
        self.live_lines = lines;
        self.active_line = Some(active);
        self.apply_line_styles(&changed, reset, false);
    }

    fn update_active_line(&mut self) {
        if !self.live_enabled() || self.live_lines.is_empty() {
            return;
        }
        let selected = rich_edit::selection(self.controls.document);
        if selected.start != selected.end {
            return;
        }
        let active = live_markdown::line_at(&self.live_lines, selected.end);
        if self.edit_pending {
            if self.active_line != Some(active) {
                self.flush_edit();
            }
            return;
        }
        self.activate_line(active);
    }

    fn activate_line(&mut self, active: usize) {
        let previous = self.active_line.replace(active);
        if previous == Some(active) {
            return;
        }
        let changed: Vec<_> = previous
            .into_iter()
            .chain([active])
            .filter(|&line| self.live_lines[line].formatted())
            .collect();
        if !changed.is_empty() {
            self.apply_line_styles(&changed, None, false);
            self.sync_scroll();
        }
    }

    fn table_geometry(&self) -> (i32, i32) {
        let margin = self.px(6);
        (
            margin,
            (self.layout.reader.right - self.layout.reader.left - 2 * margin).max(1),
        )
    }

    fn line_style_anchor(&self) -> Option<(i32, i32)> {
        let reader = self.controls.document;
        let viewport = self.layout.reader.bottom - self.layout.reader.top;
        let at = |index| rich_edit::char_position(reader, index).map(|point| (index, point.y));
        self.active_line
            .and_then(|index| self.live_lines.get(index))
            .and_then(|line| at(line.start))
            .filter(|&(_, y)| (0..viewport).contains(&y))
            .or_else(|| {
                rich_edit::char_at(
                    reader,
                    POINT {
                        x: self.px(6),
                        y: 4,
                    },
                )
                .and_then(at)
            })
    }

    fn restore_line_style_anchor(&self, original: POINT, anchor: Option<(i32, i32)>) {
        let reader = self.controls.document;
        let mut target = original;
        if original.y > 0
            && let Some((index, y)) = anchor
            && let Some(after) = rich_edit::char_position(reader, index)
        {
            target.y = (rich_edit::position(reader).y + after.y - y).max(0);
        }
        rich_edit::set_position(reader, target);
    }

    fn live_paragraph(
        &self,
        line: &live_markdown::Line,
        raw: bool,
        dc: HDC,
    ) -> live_rtf::Paragraph {
        let block = &line.block;
        let width = self.table_geometry().1;
        let twips = |pixels| (pixels * 144_000) / (self.dpi * self.zoom as i32);
        let (height, padding) =
            block
                .table
                .as_ref()
                .filter(|_| !dc.is_null())
                .map_or((0, 0), |row| {
                    let pixels = native_paint::table_height(dc, row, width, self.dpi, self.zoom);
                    if raw {
                        let raw_height = native_paint::raw_table_height(
                            dc,
                            &line.text,
                            if self.word_wrap { width } else { i32::MAX / 2 },
                            self.dpi,
                            self.zoom,
                        );
                        (0, twips((pixels - raw_height).max(0)))
                    } else {
                        (twips(pixels), 0)
                    }
                });
        let indent = if raw {
            0
        } else {
            (block.list * 330 + block.quote * 240).min(self.document_width_twips() / 2)
        };
        live_rtf::Paragraph {
            indent,
            height,
            padding,
            collapsed: block.separator && !raw,
        }
    }

    fn apply_line_styles(&self, changed: &[usize], reset: Option<i32>, already_plain: bool) {
        let reader = self.controls.document;
        let selection = rich_edit::selection(reader);
        let position = rich_edit::position(reader);
        let anchor = self.line_style_anchor();
        let palette = self.palette();
        let plain = live_markdown::Style::default();
        let dc = unsafe { GetDC(reader) };
        let set = |start, end, style| {
            if start < end {
                rich_edit::set_style(reader, CharRange { start, end }, style, palette);
            }
        };
        unsafe { SendMessageW(reader, WM_SETREDRAW, 0, 0) };
        rich_edit::format_without_undo(reader, || {
            if let Some(end) = reset {
                let range = CharRange { start: 0, end };
                set(0, end, plain);
                rich_edit::set_paragraph(reader, range, 0, 0, 0, false);
            }
            for &index in changed {
                let line = &self.live_lines[index];
                let raw = Some(index) == self.active_line;
                let block = &line.block;
                let p = self.live_paragraph(line, raw, dc);
                if !already_plain || p.height != 0 || p.padding != 0 || p.indent != 0 || p.collapsed
                {
                    rich_edit::set_paragraph(
                        reader,
                        CharRange {
                            start: line.start,
                            end: line.end,
                        },
                        p.indent,
                        p.height,
                        p.padding,
                        p.collapsed,
                    );
                }
                if !already_plain || block.base != plain {
                    set(line.start, line.end, block.base);
                }
                if !raw {
                    for (run, style) in &line.runs {
                        set(line.start + run.start, line.start + run.end, *style);
                    }
                }
            }
            rich_edit::select(reader, selection, false);
            if selection.start == selection.end {
                let style = self
                    .active_line
                    .and_then(|index| self.live_lines.get(index))
                    .filter(|_| !changed.is_empty())
                    .map_or(plain, |line| line.block.base);
                rich_edit::set_style(reader, selection, style, palette);
            }
        });
        self.restore_line_style_anchor(position, anchor);
        unsafe {
            SendMessageW(reader, WM_SETREDRAW, 1, 0);
            InvalidateRect(reader, ptr::null(), 0);
        }
        if !dc.is_null() {
            unsafe { ReleaseDC(reader, dc) };
        }
    }

    fn decorated_rows(&self, top: i32, bottom: i32) -> std::ops::Range<usize> {
        let row_at = |y| {
            rich_edit::char_at(self.controls.document, POINT { x: self.px(6), y })
                .map(|position| live_markdown::line_at(&self.live_lines, position))
        };
        let start = row_at(top).unwrap_or(0).saturating_sub(2);
        let end = row_at(bottom)
            .map_or(self.live_lines.len(), |row| row + 3)
            .min(self.live_lines.len());
        start..end
    }

    fn row_box(&self, row: usize) -> Option<RECT> {
        let reader = self.controls.document;
        let line = &self.live_lines[row];
        // The paragraph mark gives a position even when its whole Markdown row is concealed.
        let top = rich_edit::char_position(reader, line.start)?.y;
        let (left, width) = self.table_geometry();
        let bottom = self
            .live_lines
            .get(row + 1)
            .and_then(|next| rich_edit::char_position(reader, next.start))
            .map_or_else(
                || {
                    let mut height = self.px(20) * self.zoom as i32 / 100;
                    if let Some(table) = &line.block.table {
                        let dc = unsafe { GetDC(reader) };
                        if !dc.is_null() {
                            height =
                                native_paint::table_height(dc, table, width, self.dpi, self.zoom);
                            unsafe { ReleaseDC(reader, dc) };
                        }
                    }
                    top + height
                },
                |next| next.y,
            );
        (bottom > top).then(|| rect(left, top, left + width, bottom))
    }

    fn paint_editor_blocks(&self, dc: HDC, dirty: RECT) {
        let top = dirty.top.max(0);
        let bottom = dirty
            .bottom
            .min(self.layout.reader.bottom - self.layout.reader.top);
        let palette = self.palette();
        for row in self.decorated_rows(top, bottom) {
            let block = &self.live_lines[row].block;
            if Some(row) == self.active_line || !block.drawn() {
                continue;
            }
            if let Some(area) = self
                .row_box(row)
                .filter(|area| area.bottom > top && area.top < bottom)
            {
                native_paint::paint_markdown_block(dc, area, block, self.dpi, self.zoom, palette);
            }
        }
    }

    fn select_editor_block(&mut self, point: POINT) -> bool {
        let target = self.decorated_rows(point.y, point.y).find_map(|row| {
            let line = &self.live_lines[row];
            let block = &line.block;
            if Some(row) == self.active_line || !(block.rule || block.table.is_some()) {
                return None;
            }
            let area = self.row_box(row).filter(|a| {
                point.y >= a.top && point.y < a.bottom && point.x >= a.left && point.x < a.right
            })?;
            let offset = block
                .table
                .as_ref()
                .and_then(|table| {
                    let column = (point.x - area.left) as usize * table.columns
                        / (area.right - area.left) as usize;
                    table.cells.get(column).map(|cell| cell.source)
                })
                .unwrap_or(0);
            Some((row, line.start + offset))
        });
        let Some((row, position)) = target else {
            return false;
        };
        self.activate_line(row);
        let reader = self.controls.document;
        unsafe { SetFocus(reader) };
        rich_edit::select(
            reader,
            CharRange {
                start: position,
                end: position,
            },
            false,
        );
        true
    }

    fn save_tab(&mut self, index: usize, choose_path: bool) -> bool {
        self.flush_edit();
        let tab = &self.tabs[index];
        if !tab.document.can_edit() {
            return true;
        }
        let markdown = tab.document.is_markdown();
        let path = if choose_path || tab.document.path.is_none() {
            let mut dialog = rfd::FileDialog::new()
                .set_title("Save document")
                .set_file_name(&tab.document.title);
            for (name, extensions) in if markdown {
                [
                    ("Markdown documents", &["md", "markdown"][..]),
                    ("Plain text", &["txt"][..]),
                ]
            } else {
                [
                    ("Plain text", &["txt"][..]),
                    ("Markdown documents", &["md", "markdown"][..]),
                ]
            } {
                dialog = dialog.add_filter(name, extensions);
            }
            if let Some(parent) = tab.document.path.as_deref().and_then(Path::parent) {
                dialog = dialog.set_directory(parent);
            }
            let Some(mut path) = dialog.save_file() else {
                return false;
            };
            if path.extension().is_none() {
                path.set_extension(if markdown { "md" } else { "txt" });
            }
            if !is_supported(&path) {
                alert(self.window, "Use a .txt, .md, or .markdown extension.");
                return false;
            }
            path
        } else if let Some(path) = &tab.document.path {
            if tab.draft.is_none() {
                return true;
            }
            path.clone()
        } else {
            return false;
        };
        if self.tabs.iter().enumerate().any(|(other, tab)| {
            other != index
                && tab
                    .document
                    .path
                    .as_ref()
                    .is_some_and(|open| same_file(open, &path))
        }) {
            alert(self.window, "This file is already open in another tab.");
            return false;
        }
        let source = tab
            .draft
            .clone()
            .map(Cow::Owned)
            .unwrap_or_else(|| file_edit::normalized(&tab.document.source));
        let result = if tab
            .document
            .path
            .as_ref()
            .is_some_and(|old| same_file(old, &path))
        {
            file_edit::save(
                &tab.document,
                tab.document.path.as_deref().unwrap_or(&path),
                &source,
            )
        } else {
            file_edit::save_as(&tab.document, &path, &source)
        };
        match result {
            Ok(document) => {
                let changed_format = markdown != document.is_markdown();
                let tab = &mut self.tabs[index];
                tab.document = Rc::new(document);
                tab.draft = None;
                tab.width = tab_width(&tab.document.title);
                if index == self.active_tab {
                    self.document = Rc::clone(&tab.document);
                    if changed_format {
                        tab.reading = false;
                        if let Err(error) = self.refresh(true) {
                            alert(self.window, &error);
                        }
                    }
                    self.update_title();
                    self.redraw_status();
                    self.reveal_active_tab();
                }
                self.redraw_tab_strip();
                self.save_session();
                true
            }
            Err(error) => {
                alert(
                    self.window,
                    &format!("Could not save {}.\n\n{error}", tab.document.title),
                );
                false
            }
        }
    }

    fn confirm_tab(&mut self, index: usize) -> bool {
        self.flush_edit();
        let tab = &self.tabs[index];
        if tab.draft.is_none() {
            return true;
        }
        match text_tools::confirm_save(self.window, &tab.document.title) {
            IDYES => self.save_tab(index, false),
            IDNO => true,
            _ => false,
        }
    }

    fn navigate(&mut self, path: &Path, fragment: &str, back: bool, reload: bool) {
        if !self.confirm_tab(self.active_tab) {
            return;
        }
        match Document::open(path) {
            Ok(document) => {
                if back {
                    self.trail.pop();
                } else if let Some(previous) = self
                    .document
                    .path
                    .as_ref()
                    .filter(|old| Some(*old) != document.path.as_ref())
                {
                    self.trail.push(previous.clone());
                }
                self.document = Rc::new(document);
                self.tabs[self.active_tab].document = Rc::clone(&self.document);
                self.tabs[self.active_tab].draft = None;
                self.tabs[self.active_tab].caret = CharRange::default();
                self.tabs[self.active_tab].width = tab_width(&self.document.title);
                self.reveal_active_tab();
                if let Err(error) = self.refresh(reload) {
                    alert(self.window, &error);
                }
                if !fragment.is_empty() {
                    if self.document.is_markdown() && self.editable() {
                        self.set_markdown_mode(MarkdownMode::Reading);
                    }
                    self.jump(fragment);
                }
                self.save_session();
            }
            Err(error) => alert(self.window, &error),
        }
    }

    fn jump(&mut self, id: &str) {
        let id = percent_encoding::percent_decode_str(id).decode_utf8_lossy();
        if let Some(anchor) = self
            .rendered
            .as_ref()
            .and_then(|data| data.anchors.iter().find(|anchor| anchor.id == id))
        {
            self.jump_to(anchor.position);
        }
    }

    fn jump_to(&mut self, position: i32) {
        let reader = self.controls.document;
        let before = rich_edit::position(reader);
        rich_edit::select(
            reader,
            CharRange {
                start: position,
                end: position,
            },
            true,
        );
        self.animate_revealed_text(before);
        unsafe { SetFocus(reader) };
    }

    fn follow(&mut self, range: CharRange) {
        let target = self
            .rendered
            .as_ref()
            .and_then(|data| {
                data.links
                    .iter()
                    .find(|link| link.start < range.end && range.start < link.end)
            })
            .map(|link| match &link.target {
                LinkTarget::Anchor(id) => Err(id.clone()),
                LinkTarget::Document(index) => Ok(*index),
            });
        let index = match target {
            Some(Ok(index)) => index,
            Some(Err(id)) => {
                self.jump(&id);
                return;
            }
            None => return,
        };
        let Some(href) = self
            .rendered
            .as_ref()
            .and_then(|view| view.urls.get(index))
            .cloned()
        else {
            return;
        };
        if href
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
            || href
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        {
            let result = unsafe {
                ShellExecuteW(
                    self.window,
                    wide("open").as_ptr(),
                    wide(&href).as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    SW_SHOWNORMAL,
                )
            };
            if result as isize <= 32 {
                alert(self.window, "Windows could not open this link.");
            }
            return;
        }
        let (path, fragment) = href.split_once('#').unwrap_or((&href, ""));
        if path.is_empty() && !fragment.is_empty() {
            self.jump(fragment);
            return;
        }
        let local = self
            .document
            .path
            .as_deref()
            .and_then(Path::parent)
            .and_then(|dir| relative_path(dir, path));
        if let Some(path) = local.filter(|path| is_supported(path)) {
            self.navigate(&path, fragment, false, false);
        } else {
            alert(
                self.window,
                "This link does not point to a supported document or a web page.",
            );
        }
    }

    fn command(&mut self, id: usize, notification: u32) {
        match id {
            OPEN => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Open a document")
                    .add_filter("Supported documents", &["md", "markdown", "txt"])
                    .add_filter("Markdown documents", &["md", "markdown"])
                    .add_filter("Plain text", &["txt"]);
                if let Some(parent) = self.document.path.as_deref().and_then(Path::parent) {
                    dialog = dialog.set_directory(parent);
                }
                if let Some(path) = dialog.pick_file() {
                    self.add_document(&path);
                }
            }
            SAVE | SAVE_AS => {
                self.save_tab(self.active_tab, id == SAVE_AS);
            }
            SAVE_ALL => {
                self.flush_edit();
                for index in 0..self.tabs.len() {
                    if self.tabs[index].draft.is_some() && !self.save_tab(index, false) {
                        break;
                    }
                }
            }
            NEW_WINDOW => {
                let result = env::current_exe()
                    .and_then(|exe| std::process::Command::new(exe).arg("--new-window").spawn());
                if let Err(error) = result {
                    alert(self.window, &error.to_string());
                }
            }
            NEW_MARKDOWN => self.new_tab(true),
            PRINT => {
                self.flush_edit();
                let source = self.tabs[self.active_tab]
                    .draft
                    .as_deref()
                    .unwrap_or(&self.document.source);
                if let Err(error) = self.printing.print(self.window, &self.document, source) {
                    alert(self.window, &error);
                }
            }
            TOGGLE_MARKDOWN => self.request_markdown_toggle(),
            READING_VIEW => {
                self.apply_pending_markdown();
                let target = if self.markdown_mode() == MarkdownMode::Reading {
                    self.markdown_mode
                } else {
                    MarkdownMode::Reading
                };
                self.set_markdown_mode(target);
            }
            UNDO | REDO | CUT_TEXT | PASTE_TEXT | DELETE_TEXT if self.editable() => unsafe {
                SetFocus(self.controls.document);
                match id {
                    UNDO => {
                        SendMessageW(self.controls.document, 0x00c7, 0, 0);
                    }
                    REDO => {
                        SendMessageW(self.controls.document, WM_USER + 84, 0, 0);
                    }
                    CUT_TEXT => {
                        SendMessageW(self.controls.document, WM_CUT, 0, 0);
                    }
                    DELETE_TEXT => {
                        SendMessageW(self.controls.document, WM_CLEAR, 0, 0);
                    }
                    _ => rich_edit::plain_paste(self.controls.document),
                }
                self.queue_edit();
            },
            CLEAR_FORMATTING if self.editable() && self.document.is_markdown() => {
                let reader = self.controls.document;
                let selection = rich_edit::selection(reader);
                if let Some((range, plain)) =
                    text_tools::clear_formatting(&rich_edit::source(reader), selection)
                {
                    rich_edit::replace(reader, range, &wide(&plain));
                    rich_edit::select(
                        reader,
                        CharRange {
                            start: range.start,
                            end: range.start + plain.encode_utf16().count() as i32,
                        },
                        false,
                    );
                    unsafe { SetFocus(reader) };
                    self.queue_edit();
                }
            }
            GO_TO_LINE => {
                let reader = self.controls.document;
                if let Some(offset) = text_tools::ask_line(
                    self.window,
                    &rich_edit::search_text(reader),
                    rich_edit::selection(reader).end,
                ) {
                    if self.live_enabled() && !self.live_lines.is_empty() {
                        self.flush_edit();
                        let line = live_markdown::line_at(&self.live_lines, offset);
                        self.activate_line(line);
                    }
                    self.jump_to(offset);
                } else {
                    unsafe { SetFocus(reader) };
                }
            }
            INSERT_TIMESTAMP if self.editable() => {
                if let Some(timestamp) = text_tools::local_timestamp() {
                    let reader = self.controls.document;
                    rich_edit::replace(reader, rich_edit::selection(reader), &wide(&timestamp));
                    unsafe { SetFocus(reader) };
                    self.queue_edit();
                }
            }
            CHOOSE_FONT => {
                if let Some(font) = text_tools::choose_font(self.window, self.dpi) {
                    self.flush_edit();
                    if let Ok(settings) = serde_json::to_vec(&font) {
                        save_setting(self.settings.as_deref(), "font.json", settings);
                    }
                    text_tools::set_font(font);
                    if self.editable() {
                        let reader = self.controls.document;
                        rich_edit::set_palette(reader, self.palette());
                        if self.live_enabled()
                            && self.live_lines.iter().any(live_markdown::Line::formatted)
                        {
                            self.show_cached_live();
                        } else {
                            self.apply_line_styles(
                                &[],
                                Some(rich_edit::text_length(reader) as i32),
                                false,
                            );
                        }
                        self.measured_scroll = false;
                        self.scroll_limit = None;
                        self.sync_scroll();
                        self.redraw();
                    } else if let Err(error) = self.refresh(true) {
                        alert(self.window, &error);
                    }
                }
                unsafe { SetFocus(self.controls.document) };
            }
            READER if notification == EN_CHANGE && self.editable() => self.queue_edit(),
            BACK => {
                if let Some(path) = self.trail.last().cloned() {
                    self.navigate(&path, "", true, false);
                }
            }
            RELOAD => {
                if let Some(path) = self.document.path.clone() {
                    self.navigate(&path, "", false, true);
                }
            }
            THEME => {
                self.theme = if self.theme == Theme::Dark {
                    Theme::Light
                } else {
                    Theme::Dark
                };
                self.flush_edit();
                self.apply_theme();
                if self.editable() {
                    let reader = self.controls.document;
                    rich_edit::set_palette(reader, self.palette());
                    let source = rich_edit::source(reader);
                    if self.live_enabled() {
                        self.restyle(live_markdown::lines(&source), true);
                    } else {
                        self.apply_line_styles(
                            &[],
                            Some(source.encode_utf16().count() as i32),
                            false,
                        );
                    }
                    self.redraw();
                } else if let Err(error) = self.refresh(true) {
                    alert(self.window, &error);
                }
                let value = if self.theme == Theme::Dark {
                    "dark"
                } else {
                    "light"
                };
                save_setting(self.settings.as_deref(), "theme.txt", value);
            }
            SEARCH => self.show_find(true),
            FIND_REPLACE => {
                self.show_find(true);
                self.expand_find(true);
            }
            CLOSE_TAB => {
                let index = self.tab_context.take().unwrap_or(self.active_tab);
                self.close_tab(index);
            }
            NEW_TAB => {
                self.tab_context = None;
                self.new_tab(false);
            }
            CLOSE_OTHER_TABS => {
                if let Some(index) = self.tab_context.take() {
                    self.close_other_tabs(index);
                }
            }
            CLOSE_TABS_RIGHT => {
                if let Some(index) = self.tab_context.take() {
                    self.close_tabs_to_right(index);
                }
            }
            CLOSE_WINDOW | EXIT => unsafe {
                PostMessageW(self.window, WM_CLOSE, 0, 0);
            },
            TOGGLE_STATUS => self.toggle_status(),
            TOGGLE_WRAP => self.toggle_wrap(),
            COPY_TEXT => unsafe {
                SendMessageW(self.controls.document, WM_COPY, 0, 0);
            },
            SELECT_ALL => {
                rich_edit::select(
                    self.controls.document,
                    CharRange {
                        start: 0,
                        end: i32::MAX,
                    },
                    false,
                );
                unsafe { SetFocus(self.controls.document) };
            }
            ZOOM_IN | ZOOM_OUT | ZOOM_RESET => self.adjust_zoom(id),
            SEARCH_CLOSE => self.show_find(false),
            SEARCH_NEXT => self.search(false, false),
            SEARCH_PREVIOUS => self.search(true, false),
            SEARCH_EXPAND => self.expand_find(!self.find_expanded),
            SEARCH_EDIT if notification == EN_CHANGE && self.find_open => self.search(false, true),
            SEARCH_CASE => {
                self.find_case = !self.find_case;
                self.search(false, true);
            }
            SEARCH_WRAP => self.find_wrap = !self.find_wrap,
            SEARCH_REPLACE | SEARCH_REPLACE_ALL => self.replace_match(id == SEARCH_REPLACE_ALL),
            _ => {}
        }
    }

    fn rebuild_search(&mut self) {
        let query = control_text(self.controls.query);
        self.matches = if query.is_empty() {
            find_text::Matches::default()
        } else {
            find_text::Matches::new(
                &rich_edit::search_text(self.controls.document),
                &query,
                self.find_case,
            )
        };
        self.update_find_count(query.is_empty());
    }

    fn update_find_count(&mut self, empty_query: bool) {
        let selection = rich_edit::selection(self.controls.document);
        self.find_count = if empty_query {
            String::new()
        } else if self.matches.empty() {
            "0 matches".into()
        } else if let Some(index) = self.matches.index(selection) {
            format!("{}/{}", index + 1, self.matches.len())
        } else {
            self.matches.len().to_string()
        };
        self.repaint_find();
    }

    fn search(&mut self, reverse: bool, restart: bool) {
        if self.edit_pending {
            self.flush_edit();
            if !self.find_open {
                self.rebuild_search();
            }
        } else if restart || !self.find_open {
            self.rebuild_search();
        }
        let reader = self.controls.document;
        let selection = rich_edit::selection(reader);
        let index = if restart {
            self.matches.index(selection).or_else(|| {
                self.matches.next(
                    CharRange {
                        start: selection.start,
                        end: selection.start,
                    },
                    reverse,
                    true,
                )
            })
        } else {
            self.matches.next(selection, reverse, self.find_wrap)
        };
        if let Some(range) = index.and_then(|index| self.matches.range(index)) {
            let before = rich_edit::position(reader);
            self.reveal_match(range);
            rich_edit::select(reader, range, true);
            self.animate_revealed_text(before);
            self.sync_cursor();
        }
        self.update_find_count(control_text(self.controls.query).is_empty());
    }

    fn reveal_match(&mut self, range: CharRange) {
        if self.live_enabled() && !self.live_lines.is_empty() {
            let line = live_markdown::line_at(&self.live_lines, range.start);
            self.activate_line(line);
        }
    }

    fn replace_match(&mut self, all: bool) {
        if !self.editable() {
            return;
        }
        self.flush_edit();
        if self.matches.empty() {
            return;
        }
        let reader = self.controls.document;
        let replacement = control_text(self.controls.replacement);
        if all {
            let Some(output) = self
                .matches
                .replace_all(&replacement, document::DOCUMENT_LIMIT as usize)
            else {
                alert(
                    self.window,
                    "The replacement would exceed the document size limit.",
                );
                return;
            };
            let selection = rich_edit::selection(reader);
            let scroll = rich_edit::position(reader);
            rich_edit::replace(
                reader,
                CharRange {
                    start: 0,
                    end: i32::MAX,
                },
                &output,
            );
            let end = rich_edit::text_length(reader) as i32;
            let caret = selection.start.min(end);
            rich_edit::select(
                reader,
                CharRange {
                    start: caret,
                    end: caret,
                },
                false,
            );
            rich_edit::set_position(reader, scroll);
            self.queue_edit();
            self.flush_edit();
            self.rebuild_search();
        } else {
            let selection = rich_edit::selection(reader);
            let Some(index) = self.matches.index(selection) else {
                self.search(false, false);
                return;
            };
            if let Some(range) = self.matches.range(index) {
                self.reveal_match(range);
                rich_edit::replace(reader, range, &wide(replacement));
                self.queue_edit();
                self.flush_edit();
                self.search(false, false);
            }
        }
    }

    fn handle_key(&mut self, message: &MSG) -> bool {
        if !matches!(message.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
            return false;
        }
        let down = |key: u16| unsafe { GetKeyState(key as i32) < 0 };
        let key = message.wParam as u16;
        if message.hwnd == self.controls.document && !self.editable() && !down(VK_SHIFT) {
            let page = (self.scroll.viewport - self.px(42)).max(self.px(70));
            let distance = match key {
                VK_PRIOR => -page,
                VK_NEXT => page,
                VK_UP if !down(VK_CONTROL) => -self.px(28),
                VK_DOWN if !down(VK_CONTROL) => self.px(28),
                _ => 0,
            };
            if distance != 0 {
                self.scroll_by(distance, 170);
                return true;
            }
            if down(VK_CONTROL) && matches!(key, VK_HOME | VK_END) {
                if key == VK_END {
                    self.measure_scroll_extent();
                }
                self.animate_offset(if key == VK_HOME { 0 } else { self.scroll.range }, 205);
                return true;
            }
        }
        let action = match key {
            k if k == b'O' as u16 && down(VK_CONTROL) => OPEN,
            k if k == b'S' as u16 && down(VK_CONTROL) && down(VK_MENU) => SAVE_ALL,
            k if k == b'S' as u16 && down(VK_CONTROL) && down(VK_SHIFT) => SAVE_AS,
            k if k == b'S' as u16 && down(VK_CONTROL) => SAVE,
            k if k == b'N' as u16 && down(VK_CONTROL) && down(VK_SHIFT) => NEW_WINDOW,
            k if k == b'N' as u16 && down(VK_CONTROL) => NEW_TAB,
            k if k == b'P' as u16 && down(VK_CONTROL) => PRINT,
            k if k == b'W' as u16 && down(VK_CONTROL) && down(VK_SHIFT) => CLOSE_WINDOW,
            k if k == b'V' as u16
                && down(VK_CONTROL)
                && message.hwnd == self.controls.document
                && self.editable() =>
            {
                PASTE_TEXT
            }
            VK_INSERT
                if down(VK_SHIFT) && message.hwnd == self.controls.document && self.editable() =>
            {
                PASTE_TEXT
            }
            k if k == b'W' as u16 && down(VK_CONTROL) => {
                self.tab_context = None;
                CLOSE_TAB
            }
            k if k == b'T' as u16 && down(VK_CONTROL) => NEW_TAB,
            VK_TAB if down(VK_CONTROL) => {
                self.step_tab(down(VK_SHIFT));
                return true;
            }
            VK_TAB
                if self.find_expanded
                    && [self.controls.query, self.controls.replacement].contains(&message.hwnd) =>
            {
                let target = if message.hwnd == self.controls.query {
                    self.controls.replacement
                } else {
                    self.controls.query
                };
                unsafe { SetFocus(target) };
                return true;
            }
            VK_OEM_PLUS | VK_ADD if down(VK_CONTROL) => ZOOM_IN,
            VK_OEM_MINUS | VK_SUBTRACT if down(VK_CONTROL) => ZOOM_OUT,
            k if k == b'0' as u16 && down(VK_CONTROL) => ZOOM_RESET,
            k if k == b'F' as u16 && down(VK_CONTROL) => SEARCH,
            k if k == b'H' as u16 && down(VK_CONTROL) && self.editable() => FIND_REPLACE,
            VK_RETURN if self.find_open && message.hwnd == self.controls.replacement => {
                if down(VK_CONTROL) {
                    SEARCH_REPLACE_ALL
                } else {
                    SEARCH_REPLACE
                }
            }
            k if k == b'L' as u16 && down(VK_CONTROL) && down(VK_SHIFT) => THEME,
            k if k == b'R' as u16 && down(VK_CONTROL) => RELOAD,
            k if k == b'G' as u16 && down(VK_CONTROL) => GO_TO_LINE,
            VK_F5 if self.editable() => INSERT_TIMESTAMP,
            VK_LEFT if down(VK_MENU) => BACK,
            VK_RETURN if self.find_open && message.hwnd == self.controls.query => {
                if down(VK_SHIFT) {
                    SEARCH_PREVIOUS
                } else {
                    SEARCH_NEXT
                }
            }
            VK_F3 if down(VK_SHIFT) => SEARCH_PREVIOUS,
            VK_F3 => SEARCH_NEXT,
            VK_ESCAPE if self.find_open => SEARCH_CLOSE,
            _ => return false,
        };
        self.command(action, 0);
        true
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if !self.paper_brush.is_null() {
            unsafe { DeleteObject(self.paper_brush) };
        }
    }
}

#[derive(Clone, Copy)]
struct WindowSize {
    width: i32,
    height: i32,
    maximized: bool,
}

impl Default for WindowSize {
    fn default() -> Self {
        Self {
            width: WINDOW_WIDTH,
            height: WINDOW_HEIGHT,
            maximized: false,
        }
    }
}

fn read_window_size(directory: Option<&Path>) -> WindowSize {
    directory
        .and_then(|path| fs::read_to_string(path.join("window.txt")).ok())
        .and_then(|settings| parse_window_size(&settings))
        .unwrap_or_default()
}

fn parse_window_size(raw: &str) -> Option<WindowSize> {
    let mut values = raw.split_ascii_whitespace();
    let width: i32 = values.next()?.parse().ok()?;
    let height: i32 = values.next()?.parse().ok()?;
    let maximized: bool = values.next()?.parse().ok()?;
    if values.next().is_some()
        || !(360..=16_384).contains(&width)
        || !(380..=16_384).contains(&height)
    {
        return None;
    }
    Some(WindowSize {
        width,
        height,
        maximized,
    })
}

fn initial_size(size: WindowSize, dpi: i32) -> (i32, i32) {
    let mut work = RECT {
        left: 0,
        top: 0,
        right: i32::MAX,
        bottom: i32::MAX,
    };
    unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, ptr::from_mut(&mut work).cast(), 0) };
    (
        (size.width * dpi / 96).min(work.right - work.left),
        (size.height * dpi / 96).min(work.bottom - work.top),
    )
}

struct Startup {
    tabs: Vec<Tab>,
    active: usize,
    zoom: u32,
    error: Option<String>,
}

fn startup_tabs(directory: Option<&Path>, separate: bool) -> Startup {
    let mut tabs = Vec::new();
    let mut active = 0;
    let mut zoom = 100;
    if let Some(workspace) = (!separate).then(|| session::load(directory)).flatten() {
        zoom = workspace.zoom.clamp(60, 200) / 10 * 10;
        for (index, saved) in workspace.tabs.into_iter().enumerate() {
            let mut tab = match saved.path() {
                Some(path) => match Tab::restore(path) {
                    Ok(tab) => tab,
                    Err(_) => continue,
                },
                None => Tab::new(Rc::new(
                    saved
                        .untitled
                        .map_or_else(Document::welcome, Document::untitled),
                )),
            };
            if index <= workspace.active {
                active = tabs.len();
            }
            tab.position = POINT {
                x: saved.position[0].max(0),
                y: saved.position[1].max(0),
            };
            tabs.push(tab);
        }
    } else if !separate
        && let Some(document) =
            read_recent_document(directory).and_then(|path| Document::open(&path).ok())
    {
        tabs.push(Tab::new(Rc::new(document)));
    }

    let mut explicit_tab = None;
    let mut error = None;
    for path in env::args_os()
        .skip(1)
        .filter(|arg| arg != "--new-window")
        .map(PathBuf::from)
    {
        match Document::open(&path) {
            Ok(document) => {
                let index = if let Some(index) = tabs
                    .iter()
                    .position(|tab| tab.document.path == document.path)
                {
                    if tabs[index].deferred {
                        tabs[index].document = Rc::new(document);
                        tabs[index].deferred = false;
                    }
                    index
                } else if tabs.len() == 1 && tabs[0].document.path.is_none() {
                    tabs[0] = Tab::new(Rc::new(document));
                    0
                } else {
                    let index = tabs.len();
                    tabs.push(Tab::new(Rc::new(document)));
                    index
                };
                explicit_tab.get_or_insert(index);
            }
            Err(message) => {
                error.get_or_insert(message);
            }
        }
    }
    let (active, _) = load_available_tab(&mut tabs, explicit_tab.unwrap_or(active), separate);
    Startup {
        tabs,
        active,
        zoom,
        error,
    }
}

fn load_available_tab(
    tabs: &mut Vec<Tab>,
    preferred: usize,
    separate: bool,
) -> (usize, Option<String>) {
    let mut error = None;
    while !tabs.is_empty() {
        let index = preferred.min(tabs.len() - 1);
        match tabs[index].load() {
            Ok(()) => return (index, error),
            Err(message) => {
                error.get_or_insert(message);
                tabs.remove(index);
            }
        }
    }
    let document = if separate {
        Document::untitled(false)
    } else {
        Document::welcome()
    };
    tabs.push(Tab::new(Rc::new(document)));
    (0, error)
}

fn read_recent_document(directory: Option<&Path>) -> Option<PathBuf> {
    let bytes = fs::read(directory?.join("last-document.dat")).ok()?;
    if bytes.is_empty() || bytes.len() > 65_536 || bytes.len() % 2 != 0 {
        return None;
    }
    let path = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    Some(PathBuf::from(std::ffi::OsString::from_wide(&path)))
}

fn save_setting(directory: Option<&Path>, name: &str, value: impl AsRef<[u8]>) {
    if let Some(directory) = directory {
        let _ = fs::create_dir_all(directory).and_then(|()| fs::write(directory.join(name), value));
    }
}

fn save_window_size(app: &App) {
    if app.separate_window {
        return;
    }
    let mut placement = WINDOWPLACEMENT {
        length: mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..unsafe { mem::zeroed() }
    };
    if unsafe { GetWindowPlacement(app.window, &mut placement) } == 0 {
        return;
    }
    let rect = placement.rcNormalPosition;
    let width = ((rect.right - rect.left) * 96 + app.dpi / 2) / app.dpi;
    let height = ((rect.bottom - rect.top) * 96 + app.dpi / 2) / app.dpi;
    let maximized =
        unsafe { IsZoomed(app.window) } != 0 || placement.flags & WPF_RESTORETOMAXIMIZED != 0;
    let value = format!("{width} {height} {maximized}");
    if parse_window_size(&value).is_some() {
        save_setting(app.settings.as_deref(), "window.txt", &value);
    }
}

fn read_view_option(directory: Option<&Path>, file: &str) -> bool {
    directory
        .and_then(|directory| fs::read_to_string(directory.join(file)).ok())
        .is_none_or(|value| value.trim() != "off")
}

fn save_view_option(directory: Option<&Path>, file: &str, enabled: bool) {
    save_setting(directory, file, if enabled { "on" } else { "off" });
}

fn menu_entries(menu: Menu) -> &'static [(usize, &'static str)] {
    match menu {
        Menu::File => &[
            (NEW_TAB, "New tab\tCtrl+N"),
            (NEW_WINDOW, "New window\tCtrl+Shift+N"),
            (NEW_MARKDOWN, "New Markdown tab"),
            (OPEN, "Open…\tCtrl+O"),
            (0, ""),
            (SAVE, "Save\tCtrl+S"),
            (SAVE_AS, "Save as…\tCtrl+Shift+S"),
            (SAVE_ALL, "Save all\tCtrl+Alt+S"),
            (RELOAD, "Reload\tCtrl+R"),
            (0, ""),
            (PRINT, "Print…\tCtrl+P"),
            (0, ""),
            (CLOSE_TAB, "Close tab\tCtrl+W"),
            (CLOSE_WINDOW, "Close window\tCtrl+Shift+W"),
            (EXIT, "Exit"),
        ],
        Menu::Text => &[
            (UNDO, "Undo\tCtrl+Z"),
            (REDO, "Redo\tCtrl+Y"),
            (0, ""),
            (CUT_TEXT, "Cut\tCtrl+X"),
            (COPY_TEXT, "Copy\tCtrl+C"),
            (PASTE_TEXT, "Paste\tCtrl+V"),
            (DELETE_TEXT, "Delete\tDel"),
            (0, ""),
            (CLEAR_FORMATTING, "Clear formatting"),
            (0, ""),
            (SEARCH, "Find…\tCtrl+F"),
            (SEARCH_NEXT, "Find next\tF3"),
            (SEARCH_PREVIOUS, "Find previous\tShift+F3"),
            (FIND_REPLACE, "Replace…\tCtrl+H"),
            (GO_TO_LINE, "Go to line…\tCtrl+G"),
            (0, ""),
            (SELECT_ALL, "Select all\tCtrl+A"),
            (INSERT_TIMESTAMP, "Time and date\tF5"),
            (0, ""),
            (CHOOSE_FONT, "Font…"),
        ],
        Menu::View => &[
            (ZOOM_IN, "Zoom in\tCtrl++"),
            (ZOOM_OUT, "Zoom out\tCtrl+-"),
            (ZOOM_RESET, "Reset zoom\tCtrl+0"),
            (TOGGLE_STATUS, "Status bar"),
            (TOGGLE_WRAP, "Word wrap"),
            (TOGGLE_MARKDOWN, "Markdown formatting"),
            (READING_VIEW, "Reading view"),
            (THEME, "Switch theme\tCtrl+Shift+L"),
        ],
    }
}

const TAB_MENU: &[(usize, &str)] = &[
    (NEW_TAB, "New tab\tCtrl+T"),
    (CLOSE_TAB, "Close tab"),
    (CLOSE_OTHER_TABS, "Close other tabs"),
    (CLOSE_TABS_RIGHT, "Close tabs to the right"),
];

fn show_menu(state: &RefCell<App>, window: HWND, menu: Menu) {
    let Ok(app) = state.try_borrow() else { return };
    let Some(slot) = app
        .chrome()
        .slots
        .into_iter()
        .find(|slot| slot.action == Action::Menu(menu))
    else {
        return;
    };
    let anchor = POINT {
        x: slot.area.left,
        y: slot.area.bottom + app.px(3),
    };
    drop(app);
    show_popup(state, window, menu_entries(menu), anchor, None);
}

fn show_popup(
    state: &RefCell<App>,
    window: HWND,
    entries: &[(usize, &'static str)],
    mut anchor: POINT,
    tab: Option<usize>,
) {
    let Ok(mut app) = state.try_borrow_mut() else {
        return;
    };
    if tab.is_some_and(|index| index >= app.tabs.len()) {
        return;
    }
    app.close_popup();
    app.tab_context = tab;
    let palette = app.palette();
    let dpi = app.dpi;
    let reader = app.controls.document;
    let selection = rich_edit::selection(reader);
    let has_selection = selection.start != selection.end;
    let items = entries
        .iter()
        .map(|&(command, label)| floating::Item {
            command,
            label,
            enabled: match command {
                0 => false,
                RELOAD => app.document.path.is_some(),
                SAVE | SAVE_AS | PRINT => app.document.can_edit(),
                SAVE_ALL => app.edit_pending || app.tabs.iter().any(|tab| tab.draft.is_some()),
                TOGGLE_MARKDOWN | READING_VIEW => {
                    app.document.can_edit() && app.document.is_markdown()
                }
                UNDO => app.editable() && unsafe { SendMessageW(reader, 0x00c6, 0, 0) } != 0,
                REDO => app.editable() && unsafe { SendMessageW(reader, WM_USER + 85, 0, 0) } != 0,
                COPY_TEXT => has_selection,
                CUT_TEXT | DELETE_TEXT => app.editable() && has_selection,
                PASTE_TEXT => {
                    app.editable() && unsafe { SendMessageW(reader, WM_USER + 50, 13, 0) } != 0
                }
                CLEAR_FORMATTING => app.editable() && has_selection && app.document.is_markdown(),
                FIND_REPLACE | INSERT_TIMESTAMP => app.editable(),
                SEARCH_NEXT | SEARCH_PREVIOUS => !control_text(app.controls.query).is_empty(),
                CLOSE_OTHER_TABS => app.tabs.len() > 1,
                CLOSE_TABS_RIGHT => tab.is_some_and(|index| index + 1 < app.tabs.len()),
                _ => true,
            },
            checked: match command {
                TOGGLE_WRAP => app.word_wrap,
                SEARCH_CASE => app.find_case,
                SEARCH_WRAP => app.find_wrap,
                TOGGLE_STATUS => app.show_status,
                TOGGLE_MARKDOWN => app.markdown_mode() != MarkdownMode::Source,
                READING_VIEW => app.markdown_mode() == MarkdownMode::Reading,
                _ => false,
            },
        })
        .collect();
    drop(app);
    unsafe { ClientToScreen(window, &mut anchor) };
    let popup = floating::popup(window, anchor, dpi, palette, items);
    if let Ok(mut app) = state.try_borrow_mut() {
        app.popup = popup;
    }
}

const SEARCH_MENU: &[(usize, &str)] = &[(SEARCH_CASE, "Match case"), (SEARCH_WRAP, "Wrap around")];

fn control_text(control: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(control) }.max(0);
    let mut buffer = vec![0u16; length as usize + 1];
    let used = unsafe { GetWindowTextW(control, buffer.as_mut_ptr(), length + 1) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..used])
}

unsafe extern "system" fn search_proc(
    window: HWND,
    message: u32,
    word: WPARAM,
    long: LPARAM,
) -> LRESULT {
    let parent = unsafe { GetParent(window) };
    let state = unsafe { GetWindowLongPtrW(parent, GWLP_USERDATA) as *const RefCell<App> };
    if message == WM_ERASEBKGND {
        return 1;
    }
    if message == WM_COMMAND {
        unsafe { PostMessageW(parent, message, word, long) };
        return 0;
    }
    if let Some(mut app) = unsafe { state.as_ref() }.and_then(|state| state.try_borrow_mut().ok()) {
        let mut client = rect(0, 0, 0, 0);
        unsafe { GetClientRect(window, &mut client) };
        let layout = FindLayout::new(client.right, app.dpi);
        match message {
            WM_MOUSEMOVE => {
                let hover = layout.hit(mouse_point(long), app.find_expanded);
                if app.find_hover != hover {
                    app.find_hover = hover;
                    app.repaint_find();
                }
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: window,
                    dwHoverTime: 0,
                };
                unsafe { TrackMouseEvent(&mut track) };
                return 0;
            }
            WM_MOUSELEAVE => {
                if app.find_hover.take().is_some() {
                    app.repaint_find();
                }
                return 0;
            }
            WM_CTLCOLOREDIT => {
                let palette = app.palette();
                unsafe {
                    SetTextColor(word as HDC, palette.ink);
                    SetBkColor(word as HDC, palette.paper);
                }
                return app.paper_brush as LRESULT;
            }
            WM_PAINT => {
                let mut paint: PAINTSTRUCT = unsafe { mem::zeroed() };
                let dc = unsafe { BeginPaint(window, &mut paint) };
                paint_buffered(dc, paint.rcPaint, |dc| {
                    paint_find_panel(
                        dc,
                        client,
                        app.dpi,
                        app.palette(),
                        FindPanel {
                            layout: &layout,
                            count: &app.find_count,
                            hover: app.find_hover,
                            expanded: app.find_expanded,
                            editable: app.editable(),
                            has_matches: !app.matches.empty(),
                        },
                    );
                });
                unsafe { EndPaint(window, &paint) };
                return 0;
            }
            WM_LBUTTONUP => {
                if let Some(action) = layout.hit(mouse_point(long), app.find_expanded) {
                    let command = match action {
                        FindButton::Expand => SEARCH_EXPAND,
                        FindButton::Previous => SEARCH_PREVIOUS,
                        FindButton::Next => SEARCH_NEXT,
                        FindButton::Close => SEARCH_CLOSE,
                        FindButton::Options => SEARCH_OPTIONS,
                        FindButton::Replace => SEARCH_REPLACE,
                        FindButton::ReplaceAll => SEARCH_REPLACE_ALL,
                    };
                    if command == SEARCH_OPTIONS {
                        let mut anchor = POINT {
                            x: client.right - app.px(160),
                            y: client.bottom + app.px(4),
                        };
                        unsafe {
                            ClientToScreen(window, &mut anchor);
                            ScreenToClient(parent, &mut anchor);
                            PostMessageW(
                                parent,
                                SHOW_SEARCH_OPTIONS,
                                0,
                                (anchor.x as u16 as LPARAM) | ((anchor.y as u16 as LPARAM) << 16),
                            );
                        }
                    } else {
                        unsafe { PostMessageW(parent, WM_COMMAND, command, 0) };
                    }
                }
                return 0;
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(window, message, word, long) }
}

unsafe extern "system" fn editor_proc(
    window: HWND,
    message: u32,
    word: WPARAM,
    long: LPARAM,
    id: usize,
    state: usize,
) -> LRESULT {
    let app = unsafe { &*(state as *const RefCell<App>) };
    if message == WM_LBUTTONDOWN
        && let Ok(mut app) = app.try_borrow_mut()
    {
        app.apply_pending_markdown();
        app.scroll_motion = None;
        if app.live_enabled() {
            app.flush_edit();
        }
        if app.live_enabled() && app.select_editor_block(mouse_point(long)) {
            app.sync_cursor();
            return 0;
        }
    }
    if message == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(window, Some(editor_proc), id) };
    }
    if matches!(message, WM_PAINT | WM_ERASEBKGND)
        && let Ok(app) = app.try_borrow()
        && app.live_enabled()
    {
        if message == WM_ERASEBKGND {
            return 1;
        }
        let mut paint: PAINTSTRUCT = unsafe { mem::zeroed() };
        let dc = unsafe { BeginPaint(window, &mut paint) };
        let hidden = unsafe { HideCaret(window) } != 0;
        paint_buffered(dc, paint.rcPaint, |buffer| {
            native_paint::paint_document_background(buffer, paint.rcPaint, app.palette());
            unsafe {
                DefSubclassProc(
                    window,
                    WM_PRINTCLIENT,
                    buffer as WPARAM,
                    PRF_CLIENT as LPARAM,
                )
            };
            app.paint_editor_blocks(buffer, paint.rcPaint);
        });
        if hidden {
            unsafe { ShowCaret(window) };
        }
        unsafe { EndPaint(window, &paint) };
        return 0;
    }
    let result = unsafe { DefSubclassProc(window, message, word, long) };
    if message == WM_CHAR
        && word == VK_RETURN as WPARAM
        && let Ok(app) = app.try_borrow()
        && app.live_enabled()
    {
        unsafe { PostMessageW(app.window, UPDATE_ACTIVE_LINE, 1, 0) };
    }
    result
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    word: WPARAM,
    long: LPARAM,
) -> LRESULT {
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as *const RefCell<App> };
    let app_state = unsafe { state.as_ref() };
    match message {
        SHOW_MENU => {
            if let Some(state) = app_state
                && let Some(menu) = Menu::from_id(word)
            {
                show_menu(state, window, menu);
            }
            return 0;
        }
        SHOW_SEARCH_OPTIONS => {
            if let Some(state) = app_state {
                show_popup(state, window, SEARCH_MENU, mouse_point(long), None);
            }
            return 0;
        }
        SHOW_TAB_MENU => {
            if let Some(state) = app_state {
                show_popup(state, window, TAB_MENU, mouse_point(long), Some(word));
            }
            return 0;
        }
        WM_NCCALCSIZE | WM_NCPAINT => return 0,
        WM_NCACTIVATE => return 1,
        WM_NCHITTEST => return hit_test(window, long),
        WM_CLOSE => {
            if let Some(state) = app_state {
                let Ok(mut app) = state.try_borrow_mut() else {
                    return 0;
                };
                if !(0..app.tabs.len()).all(|index| app.confirm_tab(index)) {
                    return 0;
                }
                app.apply_pending_markdown();
                app.save_markdown_preference();
                save_window_size(&app);
                app.save_session();
            }
            unsafe { DestroyWindow(window) };
            return 0;
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            return 0;
        }
        WM_GETMINMAXINFO if long != 0 => {
            let dpi = unsafe { GetDpiForWindow(window) }.max(96) as i32;
            let limits = unsafe { &mut *(long as *mut MINMAXINFO) };
            limits.ptMinTrackSize = POINT {
                x: dpi * 360 / 96,
                y: dpi * 380 / 96,
            };
            let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
            let mut info = MONITORINFO {
                cbSize: mem::size_of::<MONITORINFO>() as u32,
                ..unsafe { mem::zeroed() }
            };
            if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
                limits.ptMaxPosition = POINT {
                    x: info.rcWork.left - info.rcMonitor.left,
                    y: info.rcWork.top - info.rcMonitor.top,
                };
                limits.ptMaxSize = POINT {
                    x: info.rcWork.right - info.rcWork.left,
                    y: info.rcWork.bottom - info.rcWork.top,
                };
            }
            return 0;
        }
        _ => {}
    }
    if let Some(mut app) = app_state.and_then(|state| state.try_borrow_mut().ok()) {
        match message {
            WM_EXITSIZEMOVE => save_window_size(&app),
            WM_ERASEBKGND => return 1,
            WM_PAINT => {
                let mut paint: PAINTSTRUCT = unsafe { mem::zeroed() };
                let dc = unsafe { BeginPaint(window, &mut paint) };
                paint_buffered(dc, paint.rcPaint, |dc| {
                    let dirty = paint.rcPaint;
                    if dirty.top < app.layout.toolbar_bottom {
                        let chrome = app.chrome();
                        let titles = app
                            .tabs
                            .iter()
                            .map(|tab| {
                                if tab.draft.is_some() {
                                    std::borrow::Cow::Owned(format!("• {}", tab.document.title))
                                } else {
                                    std::borrow::Cow::Borrowed(tab.document.title.as_str())
                                }
                            })
                            .collect::<Vec<_>>();
                        let titles = titles
                            .iter()
                            .map(|title| title.as_ref())
                            .collect::<Vec<_>>();
                        paint_shell(
                            dc,
                            app.layout.client,
                            app.dpi,
                            app.palette(),
                            Header {
                                chrome: &chrome,
                                icon: unsafe {
                                    SendMessageW(window, WM_GETICON, ICON_BIG as WPARAM, 0)
                                } as HICON,
                                titles: &titles,
                                selected: app.active_tab,
                                hover: app.hover,
                                maximized: unsafe { IsZoomed(window) } != 0,
                            },
                        );
                    } else if dirty.top < app.layout.status_top {
                        native_paint::paint_document_background(dc, dirty, app.palette());
                    }
                    if dirty.right > app.layout.track.left
                        && dirty.left < app.layout.track.right
                        && dirty.top < app.layout.track.bottom
                        && dirty.bottom > app.layout.track.top
                    {
                        paint_scrollbar(
                            dc,
                            app.layout.track,
                            app.scroll.thumb(app.layout.track, app.px(24)),
                            app.dpi,
                            app.palette(),
                            app.thumb_emphasis,
                        );
                    }
                    if app.show_status && dirty.bottom > app.layout.status_top {
                        paint_status(
                            dc,
                            rect(
                                0,
                                app.layout.status_top,
                                app.layout.client.right,
                                app.layout.client.bottom,
                            ),
                            app.dpi,
                            app.palette(),
                            native_paint::StatusLine {
                                document: &app.document,
                                zoom: app.zoom,
                                cursor: app.cursor,
                                char_count: app.char_count,
                                markdown: app.markdown_button().map(|area| {
                                    (
                                        area,
                                        app.requested_markdown() != MarkdownMode::Source,
                                        app.markdown_hover,
                                    )
                                }),
                            },
                        );
                    }
                });
                unsafe { EndPaint(window, &paint) };
                return 0;
            }
            WM_RBUTTONUP => {
                if let Some(Action::Tab(index) | Action::CloseTab(index)) =
                    app.chrome().hit(mouse_point(long))
                {
                    unsafe { PostMessageW(window, SHOW_TAB_MENU, index, long) };
                    return 0;
                }
            }
            WM_LBUTTONDOWN => {
                let point = mouse_point(long);
                if app.markdown_hit(point) {
                    app.request_markdown_toggle();
                    return 0;
                }
                app.apply_pending_markdown();
                if let Some(action) = app.chrome().hit(point) {
                    app.pressed = Some(action);
                    unsafe { SetCapture(window) };
                    return 0;
                }
                if app.start_scroll(point) {
                    return 0;
                }
            }
            WM_MOUSEMOVE => {
                let point = mouse_point(long);
                app.update_track_hover(Some(point));
                if app.scroll_drag.is_some() {
                    app.scroll_to(point.y);
                } else {
                    app.update_hover(point);
                }
                return 0;
            }
            WM_MOUSELEAVE => {
                if app.hover.take().is_some() {
                    app.redraw_chrome();
                }
                app.update_track_hover(None);
                if mem::take(&mut app.markdown_hover) {
                    app.redraw_status();
                }
                return 0;
            }
            WM_LBUTTONUP => {
                if app.scroll_drag.take().is_some() {
                    unsafe { ReleaseCapture() };
                    app.update_track_hover(Some(mouse_point(long)));
                    return 0;
                }
                if let Some(pressed) = app.pressed.take() {
                    unsafe { ReleaseCapture() };
                    if app.chrome().hit(mouse_point(long)) == Some(pressed) {
                        app.activate_chrome(pressed);
                    }
                    return 0;
                }
            }
            WM_CAPTURECHANGED => {
                app.scroll_drag = None;
                app.pressed = None;
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                if app.route_wheel(message, word, long) {
                    return 0;
                }
            }
            WM_CTLCOLOREDIT => {
                let palette = app.palette();
                let dc = word as windows_sys::Win32::Graphics::Gdi::HDC;
                unsafe {
                    SetTextColor(dc, palette.ink);
                    SetBkColor(dc, palette.paper);
                }
                return app.paper_brush as LRESULT;
            }
            WM_DPICHANGED if long != 0 => {
                app.dpi = (word as u16).max(96) as i32;
                set_window_icons(window);
                let bounds = unsafe { &*(long as *const RECT) };
                unsafe {
                    SetWindowPos(
                        window,
                        ptr::null_mut(),
                        bounds.left,
                        bounds.top,
                        bounds.right - bounds.left,
                        bounds.bottom - bounds.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                app.resize();
                if !app.editable() {
                    let _ = app.refresh(true);
                }
                return 0;
            }
            WM_SIZE => {
                app.close_popup();
                app.resize();
                unsafe { SetTimer(window, RESIZE_TIMER, 140, None) };
                return 0;
            }
            UPDATE_ACTIVE_LINE => {
                if word != 0 {
                    app.flush_edit();
                }
                app.update_active_line();
                app.sync_cursor();
                return 0;
            }
            WM_TIMER if word == MARKDOWN_APPLY_TIMER => {
                app.apply_pending_markdown();
                return 0;
            }
            WM_TIMER if word == MARKDOWN_PREF_TIMER => {
                app.save_markdown_preference();
                return 0;
            }
            WM_TIMER if word == EDIT_TIMER => {
                app.flush_edit();
                return 0;
            }
            WM_TIMER if word == SCROLL_TIMER => {
                app.animation_frame();
                return 0;
            }
            WM_TIMER if word == RESIZE_TIMER => {
                unsafe { KillTimer(window, RESIZE_TIMER) };
                if app.zoom_motion.is_none() {
                    app.reflow(None);
                }
                return 0;
            }
            WM_COMMAND => {
                app.command(word & 0xffff, ((word >> 16) & 0xffff) as u32);
                return 0;
            }
            WM_NOTIFY => {
                if unsafe { rich_edit::selection_changed(long, app.controls.document) } {
                    app.sync_cursor();
                    if app.live_enabled() {
                        unsafe { PostMessageW(window, UPDATE_ACTIVE_LINE, 0, 0) };
                    }
                    return 0;
                }
                if let Some(range) = unsafe { rich_edit::clicked_link(long, app.controls.document) }
                {
                    app.follow(range);
                    return 1;
                }
            }
            WM_DROPFILES => {
                if let Some(path) = dropped_file(word as HDROP) {
                    app.add_document(&path);
                }
                return 0;
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(window, message, word, long) }
}

fn dropped_file(drop: HDROP) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let count = unsafe { DragQueryFileW(drop, u32::MAX, ptr::null_mut(), 0) };
    let mut chosen = None;
    for index in 0..count {
        let size = unsafe { DragQueryFileW(drop, index, ptr::null_mut(), 0) };
        if size == 0 {
            continue;
        }
        let mut text = vec![0u16; size as usize + 1];
        let used = unsafe { DragQueryFileW(drop, index, text.as_mut_ptr(), size + 1) };
        let path = PathBuf::from(std::ffi::OsString::from_wide(&text[..used as usize]));
        if is_supported(&path) {
            chosen = Some(path);
            break;
        }
        if chosen.is_none() {
            chosen = Some(path);
        }
    }
    unsafe { DragFinish(drop) };
    chosen
}

fn set_window_icons(window: HWND) {
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let dpi = unsafe { GetDpiForWindow(window) }.max(96);
    for (kind, width, height) in [
        (ICON_SMALL, SM_CXSMICON, SM_CYSMICON),
        (ICON_BIG, SM_CXICON, SM_CYICON),
    ] {
        let icon = unsafe {
            LoadImageW(
                instance,
                APPLICATION_ICON,
                IMAGE_ICON,
                GetSystemMetricsForDpi(width, dpi),
                GetSystemMetricsForDpi(height, dpi),
                LR_SHARED,
            )
        };
        if !icon.is_null() {
            unsafe { SendMessageW(window, WM_SETICON, kind as WPARAM, icon as LPARAM) };
        }
    }
}

fn window_title(doc: &Document) -> String {
    if doc.can_edit() {
        format!("{} — {APP_NAME}", doc.title)
    } else {
        APP_NAME.into()
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    left == right
        || match (fs::canonicalize(left), fs::canonicalize(right)) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
}

fn alert(window: HWND, text: &str) {
    text_tools::alert(window, text);
}

fn child(parent: HWND, id: usize, class: &str, title: &str, style: u32) -> io::Result<HWND> {
    let handle = unsafe {
        CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(title).as_ptr(),
            WS_CHILD | style,
            0,
            0,
            0,
            0,
            parent,
            id as HMENU,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        )
    };
    if handle.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(handle)
    }
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

fn place(window: HWND, x: i32, y: i32, width: i32, height: i32) {
    unsafe { MoveWindow(window, x, y, width.max(1), height.max(1), 1) };
}

fn visible(window: HWND, show: bool) {
    unsafe { ShowWindow(window, if show { SW_SHOWNA } else { SW_HIDE }) };
}

fn mouse_point(value: LPARAM) -> POINT {
    POINT {
        x: value as i16 as i32,
        y: (value >> 16) as i16 as i32,
    }
}

fn hit_test(window: HWND, position: LPARAM) -> LRESULT {
    let mut position = mouse_point(position);
    let mut client = rect(0, 0, 0, 0);
    unsafe {
        ScreenToClient(window, &mut position);
        GetClientRect(window, &mut client);
    }
    let dpi = unsafe { GetDpiForWindow(window) }.max(96) as i32;
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as *const RefCell<App> };
    let action = if position.y < TITLE_HEIGHT * dpi / 96 {
        unsafe { state.as_ref() }
            .and_then(|state| state.try_borrow().ok())
            .and_then(|app| app.chrome().hit(position))
    } else {
        None
    };
    if matches!(
        action,
        Some(Action::Minimize | Action::Maximize | Action::CloseWindow)
    ) {
        return HTCLIENT as LRESULT;
    }
    if unsafe { IsZoomed(window) } == 0 {
        let edge = 5 * dpi / 96;
        let left = position.x < edge;
        let right = position.x >= client.right - edge;
        let top = position.y < edge;
        let bottom = position.y >= client.bottom - edge;
        let zone = match (left, right, top, bottom) {
            (true, _, true, _) => HTTOPLEFT,
            (_, true, true, _) => HTTOPRIGHT,
            (true, _, _, true) => HTBOTTOMLEFT,
            (_, true, _, true) => HTBOTTOMRIGHT,
            (true, _, _, _) => HTLEFT,
            (_, true, _, _) => HTRIGHT,
            (_, _, true, _) => HTTOP,
            (_, _, _, true) => HTBOTTOM,
            _ => HTCLIENT,
        };
        if zone != HTCLIENT {
            return zone as LRESULT;
        }
    }
    if position.y < TITLE_HEIGHT * dpi / 96 && action.is_none() {
        return HTCAPTION as LRESULT;
    }
    HTCLIENT as LRESULT
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn deferred_tab_reads_only_on_activation_and_survives_a_failed_attempt() {
        let path = env::temp_dir().join(format!("sketio-deferred-{}.md", std::process::id()));
        let mut tab = Tab::restore(path.clone()).unwrap();
        assert!(tab.deferred);
        assert_eq!(tab.document.path.as_deref(), Some(path.as_path()));
        assert!(tab.load().is_err());
        assert!(tab.deferred);
        let mut file = fs::File::create_new(&path).unwrap();
        let result = (|| {
            use io::Write;
            file.write_all("# Opened later 🦀".as_bytes())?;
            Ok::<_, io::Error>(tab.load())
        })();
        drop(file);
        fs::remove_file(path).unwrap();
        result.unwrap().unwrap();
        assert!(!tab.deferred);
        assert_eq!(tab.document.source, "# Opened later 🦀");
        assert!(tab.load().is_ok());
    }

    #[test]
    fn saved_window_size_validates_dimensions_and_state() {
        let size = parse_window_size("650 710 true\n").unwrap();
        assert_eq!((size.width, size.height, size.maximized), (650, 710, true));
        for invalid in [
            "200 710 false",
            "650 99999 false",
            "650 710",
            "650 710 true trailing",
        ] {
            assert!(parse_window_size(invalid).is_none());
        }
    }
}

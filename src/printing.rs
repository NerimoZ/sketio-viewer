use std::{mem, ptr};

use windows_sys::Win32::{
    Foundation::{GlobalFree, HGLOBAL, HWND, RECT},
    Graphics::Gdi::{
        DeleteDC, GetDeviceCaps, HDC, HORZRES, LOGPIXELSX, LOGPIXELSY, PHYSICALHEIGHT,
        PHYSICALOFFSETX, PHYSICALOFFSETY, PHYSICALWIDTH, VERTRES,
    },
    Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage},
    UI::{
        Controls::Dialogs::*,
        WindowsAndMessaging::{CreateWindowExW, DestroyWindow, SendMessageW, WM_USER, WS_CHILD},
    },
};

use crate::{
    document::Document,
    native_paint::Palette,
    rich_edit::{self, CharRange, wide},
    rtf,
};

pub struct PageSetup {
    mode: HGLOBAL,
    names: HGLOBAL,
    margins: RECT,
}

impl Default for PageSetup {
    fn default() -> Self {
        Self {
            mode: ptr::null_mut(),
            names: ptr::null_mut(),
            margins: RECT {
                left: 750,
                top: 750,
                right: 750,
                bottom: 750,
            },
        }
    }
}

impl Drop for PageSetup {
    fn drop(&mut self) {
        for resource in [self.mode, self.names] {
            if !resource.is_null() {
                unsafe { GlobalFree(resource) };
            }
        }
    }
}

impl PageSetup {
    pub fn print(&mut self, owner: HWND, document: &Document, source: &str) -> Result<(), String> {
        let mut dialog = PRINTDLGW {
            lStructSize: size_of::<PRINTDLGW>() as u32,
            hwndOwner: owner,
            hDevMode: self.mode,
            hDevNames: self.names,
            Flags: PD_RETURNDC | PD_NOSELECTION | PD_USEDEVMODECOPIESANDCOLLATE,
            nMinPage: 1,
            nMaxPage: u16::MAX,
            nFromPage: 1,
            nToPage: 1,
            nCopies: 1,
            ..unsafe { mem::zeroed() }
        };
        let accepted = unsafe { PrintDlgW(&mut dialog) };
        self.mode = dialog.hDevMode;
        self.names = dialog.hDevNames;
        if accepted == 0 {
            return dialog_error("Print");
        }
        if dialog.hDC.is_null() {
            return Err("Windows did not return a printer.".into());
        }
        let pages = if dialog.Flags & PD_PAGENUMS != 0 {
            (dialog.nFromPage, dialog.nToPage)
        } else {
            (1, u16::MAX)
        };
        let result = self.print_on(owner, document, source, dialog.hDC, pages);
        unsafe { DeleteDC(dialog.hDC) };
        result
    }

    fn print_on(
        &self,
        owner: HWND,
        doc: &Document,
        source: &str,
        dc: HDC,
        pages: (u16, u16),
    ) -> Result<(), String> {
        let caps = |index: u32| unsafe { GetDeviceCaps(dc, index as i32) };
        let px = caps(LOGPIXELSX).max(1);
        let py = caps(LOGPIXELSY).max(1);
        let x = |value| value * 1440 / px;
        let y = |value| value * 1440 / py;
        let offset_x = x(caps(PHYSICALOFFSETX));
        let offset_y = y(caps(PHYSICALOFFSETY));
        let area = RECT {
            left: (self.margins.left * 144 / 100 - offset_x).max(0),
            top: (self.margins.top * 144 / 100 - offset_y).max(0),
            right: (x(caps(PHYSICALWIDTH)) - self.margins.right * 144 / 100 - offset_x)
                .min(x(caps(HORZRES))),
            bottom: (y(caps(PHYSICALHEIGHT)) - self.margins.bottom * 144 / 100 - offset_y)
                .min(y(caps(VERTRES))),
        };
        if area.right <= area.left || area.bottom <= area.top {
            return Err("The selected margins leave no printable area.".into());
        }
        let reader = unsafe {
            CreateWindowExW(
                0,
                wide(rich_edit::CLASS).as_ptr(),
                wide("").as_ptr(),
                WS_CHILD | rich_edit::STYLE,
                0,
                0,
                800,
                1000,
                owner,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if reader.is_null() {
            return Err("Could not prepare the document for printing.".into());
        }
        let result = (|| {
            let white = Palette::new(false);
            if doc.is_markdown() {
                let rendered = rtf::render(doc, source, rtf::Theme::Light, area.right - area.left);
                rich_edit::show(reader, &rendered.rtf, white.paper, [])
                    .map_err(|error| error.to_string())?;
            } else {
                rich_edit::new_text(reader, source, white);
            }
            paginated_print(
                reader,
                dc,
                area,
                RECT {
                    left: 0,
                    top: 0,
                    right: x(caps(PHYSICALWIDTH)),
                    bottom: y(caps(PHYSICALHEIGHT)),
                },
                &doc.title,
                pages,
            )
        })();
        unsafe { DestroyWindow(reader) };
        result
    }
}

fn dialog_error(label: &str) -> Result<(), String> {
    let code = unsafe { CommDlgExtendedError() };
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{label} could not open (Windows error {code:#x})."))
    }
}

fn paginated_print(
    reader: HWND,
    dc: HDC,
    area: RECT,
    page_area: RECT,
    title: &str,
    pages: (u16, u16),
) -> Result<(), String> {
    const EM_FORMATRANGE: u32 = WM_USER + 57;
    #[repr(C)]
    struct FormatRange {
        dc: HDC,
        target: HDC,
        area: RECT,
        page: RECT,
        range: CharRange,
    }
    let mut range = FormatRange {
        dc,
        target: dc,
        area,
        page: page_area,
        range: CharRange { start: 0, end: -1 },
    };
    let length = rich_edit::text_length(reader) as i32;
    let mut started = false;
    let result = (|| {
        for page in 1..=pages.1 {
            if page > 1 && range.range.start >= length {
                break;
            }
            let draw = page >= pages.0;
            if draw && !started {
                let title = wide(title);
                let info = DOCINFOW {
                    cbSize: size_of::<DOCINFOW>() as i32,
                    lpszDocName: title.as_ptr(),
                    ..unsafe { mem::zeroed() }
                };
                if unsafe { StartDocW(dc, &info) } <= 0 {
                    return Err("The printer could not start the print job.".into());
                }
                started = true;
            }
            if draw && unsafe { StartPage(dc) } <= 0 {
                return Err("The printer could not start a page.".into());
            }
            let next = unsafe {
                SendMessageW(
                    reader,
                    EM_FORMATRANGE,
                    usize::from(draw),
                    ptr::from_mut(&mut range) as isize,
                )
            } as i32;
            if draw && unsafe { EndPage(dc) } <= 0 {
                return Err("The printer could not finish a page.".into());
            }
            if next <= range.range.start && range.range.start < length {
                return Err("The document did not fit within the printable area.".into());
            }
            range.range.start = next;
        }
        if !started {
            return Err("The selected page range is outside the document.".into());
        }
        if unsafe { EndDoc(dc) } <= 0 {
            return Err("The printer could not finish the print job.".into());
        }
        Ok(())
    })();
    unsafe {
        SendMessageW(reader, EM_FORMATRANGE, 0, 0);
        if result.is_err() && started {
            AbortDoc(dc);
        }
    }
    result
}

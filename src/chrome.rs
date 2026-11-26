use windows_sys::Win32::Foundation::{POINT, RECT};

pub const TITLE_HEIGHT: i32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Menu {
    File,
    Text,
    View,
}

impl Menu {
    pub fn from_id(id: usize) -> Option<Self> {
        [Self::File, Self::Text, Self::View]
            .into_iter()
            .find(|menu| *menu as usize == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Text => "Text",
            Self::View => "View",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Tab(usize),
    CloseTab(usize),
    Open,
    ScrollLeft,
    ScrollRight,
    Minimize,
    Maximize,
    CloseWindow,
    Menu(Menu),
}

pub struct Slot {
    pub area: RECT,
    pub action: Action,
}

pub struct Chrome {
    pub tab_bottom: i32,
    pub menu_bottom: i32,
    pub slots: Vec<Slot>,
    pub tab_region: RECT,
    pub scroll: i32,
    pub max_scroll: i32,
    pub track_width: i32,
    tabs: Vec<(i32, i32)>,
    strip: RECT,
}

impl Chrome {
    pub fn new(width: i32, dpi: i32, tab_widths: &[i32], scroll: i32) -> Self {
        let px = |value| value * dpi / 96;
        let tab_bottom = px(TITLE_HEIGHT);
        let menu_bottom = tab_bottom + px(32);
        let mut slots = Vec::new();
        let mut add = |area, action| slots.push(Slot { area, action });
        let system_width = px(43);
        let system_left = width - system_width * 3;
        for (index, action) in [Action::Minimize, Action::Maximize, Action::CloseWindow]
            .into_iter()
            .enumerate()
        {
            let left = system_left + system_width * index as i32;
            add(rect(left, 0, left + system_width, px(33)), action);
        }

        let tab_left = px(51);
        let tab_top = px(14);
        let tab_end = tab_bottom - px(3);
        let gap = px(3);
        let button = px(30);
        let arrow_to_plus = px(15);
        let strip_end = system_left - px(44);
        let strip = rect(tab_left, tab_top, strip_end, tab_end);
        let without_arrows = (strip_end - tab_left - button - gap).max(px(32));
        let widths: Vec<_> = tab_widths.iter().map(|width| px(*width)).collect();
        let gaps = gap * widths.len().saturating_sub(1) as i32;
        let cap = tab_width_cap(&widths, (without_arrows - gaps).max(0), px(116));
        let mut cursor = 0;
        let tabs: Vec<_> = widths
            .iter()
            .map(|width| {
                let start = cursor;
                let end = start + width.min(&cap);
                cursor = end + gap;
                (start, end)
            })
            .collect();
        let natural = cursor.saturating_sub(gap);
        let overflow = natural > without_arrows;
        let left = tab_left + if overflow { button } else { 0 };
        let track_width = if overflow {
            (without_arrows - button * 2 - arrow_to_plus).max(px(32))
        } else {
            natural
        };
        let right = left + track_width;
        let tab_region = rect(left, tab_top, right, tab_end);
        let max_scroll = (natural - track_width).max(0);
        let scroll = scroll.clamp(0, max_scroll);

        if overflow {
            add(
                square_at(tab_left, tab_top, button, tab_end),
                Action::ScrollLeft,
            );
        }
        for (index, &(start, end)) in tabs.iter().enumerate() {
            let area = rect(left + start - scroll, tab_top, left + end - scroll, tab_end);
            if area.right <= left || area.left >= right {
                continue;
            }
            add(area, Action::Tab(index));
            add(
                square_at(area.right - button - gap, area.top, button, area.bottom),
                Action::CloseTab(index),
            );
        }
        let plus_left = if overflow {
            add(
                square_at(right, tab_top, button, tab_end),
                Action::ScrollRight,
            );
            right + button + arrow_to_plus
        } else {
            right + gap
        };
        add(square_at(plus_left, tab_top, button, tab_end), Action::Open);
        for (index, menu) in [Menu::File, Menu::Text, Menu::View].into_iter().enumerate() {
            let left = px(8 + 51 * index as i32);
            add(
                rect(left, tab_bottom + px(2), left + px(48), menu_bottom - px(2)),
                Action::Menu(menu),
            );
        }
        Self {
            tab_bottom,
            menu_bottom,
            slots,
            tab_region,
            scroll,
            max_scroll,
            track_width,
            tabs,
            strip,
        }
    }

    pub fn hit(&self, p: POINT) -> Option<Action> {
        self.slots
            .iter()
            .rev()
            .find(|slot| {
                contains(slot.area, p)
                    && (!matches!(slot.action, Action::Tab(_) | Action::CloseTab(_))
                        || contains(self.tab_region, p))
            })
            .map(|slot| slot.action)
    }

    pub fn over_tab_strip(&self, point: POINT) -> bool {
        contains(self.strip, point)
    }

    pub fn action_enabled(&self, action: Action) -> bool {
        match action {
            Action::ScrollLeft => self.scroll > 0,
            Action::ScrollRight => self.scroll < self.max_scroll,
            _ => true,
        }
    }

    pub fn reveal(&self, index: usize) -> i32 {
        let Some(&(start, end)) = self.tabs.get(index) else {
            return self.scroll;
        };
        if start < self.scroll || end - start > self.track_width {
            start.min(self.max_scroll)
        } else if end > self.scroll + self.track_width {
            (end - self.track_width).min(self.max_scroll)
        } else {
            self.scroll
        }
    }
}

fn tab_width_cap(widths: &[i32], available: i32, minimum: i32) -> i32 {
    let maximum = widths.iter().copied().max().unwrap_or(minimum);
    if widths.iter().sum::<i32>() <= available {
        return maximum;
    }

    // Keep short tabs readable before opting into horizontal scrolling.
    let mut low = minimum.min(maximum);
    let mut high = maximum;
    while low < high {
        let middle = low + (high - low + 1) / 2;
        if widths.iter().map(|width| (*width).min(middle)).sum::<i32>() <= available {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

fn square_at(left: i32, top: i32, size: i32, bottom: i32) -> RECT {
    let y = top + (bottom - top - size + 1) / 2;
    rect(left, y, left + size, y + size)
}

fn contains(rect: RECT, point: POINT) -> bool {
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflowing_tabs_scroll_independently_and_clip_hit_targets() {
        let widths = [145; 7];
        let first = Chrome::new(560, 96, &widths, 0);
        assert!(first.max_scroll > 0);
        assert!(!first.action_enabled(Action::ScrollLeft));
        assert!(first.action_enabled(Action::ScrollRight));
        let final_page = Chrome::new(560, 96, &widths, first.reveal(6));
        assert!(final_page.action_enabled(Action::ScrollLeft));
        assert!(!final_page.action_enabled(Action::ScrollRight));
        let point = POINT {
            x: final_page.tab_region.left - 1,
            y: 27,
        };
        assert!(!matches!(
            final_page.hit(point),
            Some(Action::Tab(_) | Action::CloseTab(_))
        ));
        assert!(
            final_page
                .slots
                .iter()
                .any(|slot| slot.action == Action::Tab(6))
        );
    }

    #[test]
    fn compresses_tabs_before_scrolling_and_keeps_the_caption_gap() {
        let single = Chrome::new(560, 96, &[145], 0);
        let crowded = Chrome::new(620, 96, &[145; 3], 0);
        let first_tab = |chrome: &Chrome| {
            chrome
                .slots
                .iter()
                .find(|slot| slot.action == Action::Tab(0))
                .unwrap()
                .area
        };
        let width = |area: RECT| area.right - area.left;
        assert_eq!(width(first_tab(&single)), 145);
        assert!((116..145).contains(&width(first_tab(&crowded))));
        assert_eq!(crowded.max_scroll, 0);
        for chrome in [single, crowded, Chrome::new(560, 96, &[145; 7], 0)] {
            let plus = chrome
                .slots
                .iter()
                .find(|slot| slot.action == Action::Open)
                .unwrap();
            let system = chrome
                .slots
                .iter()
                .find(|slot| slot.action == Action::Minimize)
                .unwrap();
            assert!(system.area.left - plus.area.right >= 44);
            if let Some(arrow) = chrome
                .slots
                .iter()
                .find(|slot| slot.action == Action::ScrollRight)
            {
                assert!(plus.area.left - arrow.area.right >= 15);
            }
        }
    }

    #[test]
    fn tab_controls_have_the_same_square_hitbox() {
        let chrome = Chrome::new(560, 96, &[145; 7], 0);
        let area = |action| {
            chrome
                .slots
                .iter()
                .find(|slot| slot.action == action)
                .unwrap()
                .area
        };
        let plus = area(Action::Open);
        for action in [Action::ScrollLeft, Action::ScrollRight, Action::CloseTab(0)] {
            let control = area(action);
            assert_eq!(plus.right - plus.left, control.right - control.left);
            assert_eq!(plus.top, control.top);
            assert_eq!(plus.bottom, control.bottom);
        }
        let close = area(Action::CloseTab(0));
        let tab = area(Action::Tab(0));
        assert!(close.right < tab.right);
        assert_eq!(
            (close.top + close.bottom) / 2,
            (tab.top + tab.bottom + 1) / 2
        );
    }
}

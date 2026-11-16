//! Shared physical-pixel geometry; element order never changes the capture pipeline.
use super::api::{Point, Rect};
use crate::settings::{Element, Settings, Visual};

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub text: Rect,
    pub visual: Rect,
    pub artwork: Rect,
    pub buttons: [Rect; 4],
    order: [Element; 4],
    vertical: bool,
    two_lines: bool,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            text: Rect::default(),
            visual: Rect::default(),
            artwork: Rect::default(),
            buttons: [Rect::default(); 4],
            order: Element::ALL,
            vertical: false,
            two_lines: false,
        }
    }
}
impl Layout {
    pub fn has_visual(&self) -> bool {
        self.visual.width() > 0 && self.visual.height() > 0
    }
    pub fn title(&self) -> Rect {
        if self.two_lines {
            Rect { bottom: self.text.top + self.text.height() * 19 / 34, ..self.text }
        } else {
            self.text
        }
    }
    pub fn artist(&self) -> Rect {
        if self.two_lines { Rect { top: self.title().bottom, ..self.text } } else { Rect::default() }
    }
    pub fn element(&self, e: Element) -> Rect {
        match e {
            Element::Artwork => self.artwork,
            Element::Text => self.text,
            Element::Visual => self.visual,
            Element::Controls => {
                let mut rect = Rect::default();
                for b in self.buttons.iter().filter(|r| r.width() > 0 && r.height() > 0) {
                    if rect.width() == 0 {
                        rect = *b;
                    } else {
                        rect.left = rect.left.min(b.left);
                        rect.right = rect.right.max(b.right);
                    }
                }
                rect
            }
        }
    }
    pub fn hit(&self, p: Point) -> Option<Element> {
        self.order.into_iter().find(|&e| {
            let r = self.element(e);
            p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
        })
    }
    pub fn new(w: i32, h: i32, dpi: u32, s: &Settings) -> Self {
        let mut r = Self { order: s.order, ..Self::default() };
        if w <= 0 || h <= 0 {
            return r;
        }
        let px = |v| ((v as i64 * dpi as i64 + 48) / 96) as i32;
        let gap = px(6);
        let row = h.min(px(40));
        let side = px(30).min(row).min(w);
        let button_width = px(24).min(side);
        let enabled = |i| if i == 3 { s.menu_button } else { s.controls };
        if w < px(120) && h > px(80) {
            // A vertical taskbar stacks the same ordered groups instead of clipping a horizontal row.
            r.vertical = true;
            let mut heights = [0; 4];
            heights[0] = if s.artwork && h >= w + px(40) { w } else { 0 };
            heights[1] = px(if s.two_lines { 34 } else { 24 });
            heights[3] = if s.controls || s.menu_button { side } else { 0 };
            let remaining = h - heights.iter().sum::<i32>() - gap * 3;
            heights[2] = if s.visual != Visual::Off && w >= px(24) && remaining >= px(24) { remaining } else { 0 };
            let mut y = 0;
            for e in s.order {
                let height =
                    if e == Element::Artwork && h - y < w { 0 } else { heights[e as usize].min((h - y).max(0)) };
                if height == 0 {
                    continue;
                }
                let area = Rect { left: 0, top: y, right: w, bottom: y + height };
                match e {
                    Element::Artwork => r.artwork = area,
                    Element::Text => r.text = Rect { left: gap.min(w), right: (w - gap).max(gap.min(w)), ..area },
                    Element::Visual => r.visual = area,
                    Element::Controls => {
                        let mut selected = [false; 4];
                        let mut count = 0;
                        for i in [1, 3, 0, 2].into_iter().filter(|&i| enabled(i)) {
                            if (count + 1) * button_width > w {
                                break;
                            }
                            selected[i] = true;
                            count += 1;
                        }
                        let mut x = (w - count * button_width) / 2;
                        for (i, show) in selected.into_iter().enumerate() {
                            if show {
                                r.buttons[i] = Rect { left: x, right: x + button_width, ..area };
                                x += button_width;
                            }
                        }
                    }
                }
                y += height + gap;
            }
        } else {
            let cover = if s.artwork && w >= h + px(64) + gap { h } else { 0 };
            let mut budget = w - cover - if cover > 0 { gap } else { 0 };
            let mut selected = [false; 4];
            let mut count = 0;
            for i in [1, 3, 0, 2].into_iter().filter(|&i| enabled(i)) {
                if (count + 1) * button_width + gap + px(64) > budget {
                    break;
                }
                selected[i] = true;
                count += 1;
            }
            let controls = count * button_width;
            budget -= controls + if controls > 0 { gap } else { 0 };
            let vw = if s.visual != Visual::Off && budget >= px(150) { (budget / 3).clamp(px(48), px(144)) } else { 0 };
            let text_w = (budget - vw - if vw > 0 { gap } else { 0 }).max(0);
            let th = row.min(px(if s.two_lines { 34 } else { 24 }));
            let mut x = 0;
            for e in s.order {
                let width = [cover, text_w, vw, controls][e as usize];
                if width <= 0 {
                    continue;
                }
                match e {
                    Element::Artwork => r.artwork = Rect { left: x, top: 0, right: x + width, bottom: h },
                    Element::Text => {
                        r.text = Rect { left: x, top: (row - th) / 2, right: x + width, bottom: (row + th) / 2 }
                    }
                    Element::Visual => {
                        r.visual =
                            Rect { left: x, top: 0, right: x + width, bottom: if s.full_height { h } else { row } }
                    }
                    Element::Controls => {
                        let mut bx = x;
                        for (i, show) in selected.into_iter().enumerate() {
                            if show {
                                r.buttons[i] = Rect {
                                    left: bx,
                                    top: (row - side) / 2,
                                    right: bx + button_width,
                                    bottom: (row + side) / 2,
                                };
                                bx += button_width;
                            }
                        }
                    }
                }
                x += width + gap;
            }
        }
        for a in [&mut r.text, &mut r.visual, &mut r.artwork].into_iter().chain(r.buttons.iter_mut()) {
            a.left = a.left.clamp(0, w);
            a.right = a.right.clamp(a.left, w);
            a.top = a.top.clamp(0, h);
            a.bottom = a.bottom.clamp(a.top, h);
        }
        r.two_lines = s.two_lines && r.text.height() >= px(32);
        r
    }
    pub fn fit_text(&mut self, width: i32, dpi: u32) {
        if self.vertical || !self.has_visual() {
            return;
        }
        let spare = self.text.width() - width.max(0).min(self.text.width());
        if spare <= 0 {
            return;
        }
        let gap = ((6 * dpi + 48) / 96) as i32;
        let mut x = 0;
        for e in self.order {
            let mut area = self.element(e);
            if area.width() == 0 {
                continue;
            }
            let width = area.width()
                + if e == Element::Visual {
                    spare
                } else if e == Element::Text {
                    -spare
                } else {
                    0
                };
            let shift = x - area.left;
            area.left = x;
            area.right = x + width;
            match e {
                Element::Artwork => self.artwork = area,
                Element::Text => self.text = area,
                Element::Visual => self.visual = area,
                Element::Controls => {
                    for b in self.buttons.iter_mut().filter(|b| b.width() > 0) {
                        b.left += shift;
                        b.right += shift;
                    }
                }
            }
            x += width + gap;
        }
    }
}

pub(super) fn reorder(order: &mut [Element; 4], element: Element, target: usize) {
    let Some(from) = order.iter().position(|&e| e == element) else {
        return;
    };
    let target = target.min(3);
    if from < target {
        order[from..=target].rotate_left(1);
    } else {
        order[target..=from].rotate_right(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_buttons_form_a_compact_ordered_group_in_both_orientations() {
        let settings = Settings { controls: true, menu_button: true, ..Settings::default() };
        for (w, h) in [(480, 40), (96, 600)] {
            let layout = Layout::new(w, h, 96, &settings);
            assert!(layout.buttons.iter().all(|r| r.width() == 24));
            for pair in layout.buttons.windows(2) {
                assert_eq!(pair[0].right, pair[1].left);
            }
        }
    }
    #[test]
    fn every_element_permutation_stays_disjoint_after_fitting_text() {
        for a in Element::ALL {
            for b in Element::ALL.into_iter().filter(|&e| e != a) {
                for c in Element::ALL.into_iter().filter(|&e| e != a && e != b) {
                    let d = Element::ALL.into_iter().find(|&e| e != a && e != b && e != c).unwrap();
                    let settings = Settings { order: [a, b, c, d], artwork: true, ..Settings::default() };
                    for (w, h, dpi) in [(480, 40, 96), (720, 60, 144), (96, 600, 96)] {
                        let mut l = Layout::new(w, h, dpi, &settings);
                        l.fit_text(72 * dpi as i32 / 96, dpi);
                        let areas = Element::ALL.map(|e| l.element(e));
                        for (i, r) in areas.into_iter().enumerate().filter(|(_, r)| r.width() > 0 && r.height() > 0) {
                            assert!(r.left >= 0 && r.top >= 0 && r.right <= w && r.bottom <= h);
                            assert_eq!(
                                l.hit(Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 }),
                                Some(Element::ALL[i])
                            );
                            for other in areas.iter().skip(i + 1).filter(|r| r.width() > 0 && r.height() > 0) {
                                assert!(!r.intersects(*other));
                            }
                        }
                        if l.artwork.width() > 0 {
                            assert_eq!(l.artwork.width(), l.artwork.height());
                        }
                    }
                }
            }
        }
        let mut order = Element::ALL;
        reorder(&mut order, Element::Visual, 0);
        assert_eq!(order, [Element::Visual, Element::Artwork, Element::Text, Element::Controls]);
        reorder(&mut order, Element::Visual, 3);
        assert_eq!(order[3], Element::Visual);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A list whose rows are put in another order by dragging them: while one
//! is dragged, the others make way where it would fall, sliding, and it
//! waits there, faint, for it to be let go. Where it falls is told by the
//! pointer against where the rows were as the drag began, so that rows
//! moving under it do not move it.
//!
//! The list takes the drag over it and the drop, `over` and `drop`, rows
//! and the room between them alike; each row starts and ends it, `start`
//! and `end`, is `draggable`, and takes its place from `style`.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// A drag under way: from which row, to where, and the rows as they were.
#[derive(Debug, Clone, PartialEq)]
struct Drag {
    from: usize,
    to: usize,
    /// Each row's top, from the list's, and its height, in pixels.
    tops: Vec<f64>,
    heights: Vec<f64>,
    /// The room between two rows.
    gap: f64,
}

impl Drag {
    /// How far a row goes, while the drag is where it is.
    fn shift(&self, at: usize) -> f64 {
        let (from, to) = (self.from, self.to);
        if at == from {
            // The one dragged, where it would fall.
            return match to.cmp(&from) {
                std::cmp::Ordering::Greater => {
                    self.tops[to] + self.heights[to] - (self.tops[from] + self.heights[from])
                }
                std::cmp::Ordering::Less => self.tops[to] - self.tops[from],
                std::cmp::Ordering::Equal => 0.0,
            };
        }
        let room = self.heights[from] + self.gap;
        if from < to && at > from && at <= to {
            -room
        } else if to < from && at >= to && at < from {
            room
        } else {
            0.0
        }
    }

    /// Where it would fall with the pointer `y` pixels below the list's
    /// top: before every row whose middle is below it, the one dragged
    /// aside.
    fn target(&self, y: f64) -> usize {
        (0..self.tops.len())
            .filter(|at| *at != self.from)
            .filter(|at| self.tops[*at] + self.heights[*at] / 2.0 < y)
            .count()
    }
}

/// A list's rows, as one of them is dragged.
#[derive(Clone, Copy)]
pub struct Sortable {
    drag: RwSignal<Option<Drag>>,
}

impl Sortable {
    pub fn new() -> Self {
        Self {
            drag: RwSignal::new(None),
        }
    }

    /// Whether a row is being dragged.
    pub fn sorting(self) -> bool {
        self.drag.with(Option::is_some)
    }

    /// Whether this row is the one dragged.
    pub fn dragged(self, at: usize) -> bool {
        self.drag.with(|d| d.as_ref().is_some_and(|d| d.from == at))
    }

    /// Where a row goes while one is dragged.
    pub fn style(self, at: usize) -> String {
        self.drag.with(|d| {
            d.as_ref()
                .map(|d| d.shift(at))
                .filter(|shift| *shift != 0.0)
                .map(|shift| format!("transform: translateY({shift}px)"))
                .unwrap_or_default()
        })
    }

    /// A row starts being dragged: the rows measured as they are, its
    /// siblings that drag too, in their order.
    pub fn start(self, ev: &leptos::ev::DragEvent, at: usize) {
        if let Some(data) = ev.data_transfer() {
            data.set_effect_allowed("move");
            // Firefox starts no drag without data.
            let _ = data.set_data("text/plain", &at.to_string());
        }
        let Some(row) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let Some(list) = row.parent_element() else {
            return;
        };
        let top = list.get_bounding_client_rect().top();
        let children = list.children();
        let rows: Vec<web_sys::DomRect> = (0..children.length())
            .filter_map(|i| children.item(i))
            .filter(|c| c.get_attribute("draggable").as_deref() == Some("true"))
            .map(|c| c.get_bounding_client_rect())
            .collect();
        if at >= rows.len() {
            return;
        }
        let tops: Vec<f64> = rows.iter().map(|r| r.top() - top).collect();
        let heights: Vec<f64> = rows.iter().map(web_sys::DomRect::height).collect();
        let gap = match (tops.first(), tops.get(1), heights.first()) {
            (Some(first), Some(second), Some(height)) => (second - first - height).max(0.0),
            _ => 0.0,
        };
        let drag = Drag {
            from: at,
            to: at,
            tops,
            heights,
            gap,
        };
        // After the browser has taken its picture of the row, as it was.
        request_animation_frame(move || {
            self.drag.try_set(Some(drag));
        });
    }

    /// The pointer over the list, with a row dragged: where it would fall.
    pub fn over(self, ev: &leptos::ev::DragEvent) {
        if !self.drag.with_untracked(Option::is_some) {
            return;
        }
        ev.prevent_default();
        let Some(list) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let y = f64::from(ev.client_y()) - list.get_bounding_client_rect().top();
        let to = self
            .drag
            .with_untracked(|d| d.as_ref().map(|d| d.target(y)));
        if let Some(to) = to
            && self
                .drag
                .with_untracked(|d| d.as_ref().is_some_and(|d| d.to != to))
        {
            self.drag.update(|d| {
                if let Some(d) = d {
                    d.to = to;
                }
            });
        }
    }

    /// Let go over the list: the row goes where it would fall.
    pub fn drop(self, ev: &leptos::ev::DragEvent, reorder: impl Fn(usize, usize)) {
        ev.prevent_default();
        if let Some(drag) = self.drag.get_untracked() {
            self.drag.set(None);
            if drag.from != drag.to {
                reorder(drag.from, drag.to);
            }
        }
    }

    /// The drag over, dropped or not.
    pub fn end(self) {
        self.drag.try_set(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four rows 50 pixels tall, 8 apart.
    fn drag(from: usize, to: usize) -> Drag {
        Drag {
            from,
            to,
            tops: vec![0.0, 58.0, 116.0, 174.0],
            heights: vec![50.0; 4],
            gap: 8.0,
        }
    }

    #[test]
    fn the_others_make_way() {
        // The first down to the third: the second and third up, it down.
        let down = drag(0, 2);
        assert_eq!(
            (0..4).map(|at| down.shift(at)).collect::<Vec<_>>(),
            [116.0, -58.0, -58.0, 0.0]
        );
        // The last up to the second.
        let up = drag(3, 1);
        assert_eq!(
            (0..4).map(|at| up.shift(at)).collect::<Vec<_>>(),
            [0.0, 58.0, 58.0, -116.0]
        );
        assert!((0..4).all(|at| drag(2, 2).shift(at) == 0.0));
    }

    #[test]
    fn it_falls_where_the_pointer_is() {
        let from_second = drag(1, 1);
        assert_eq!(from_second.target(-10.0), 0);
        assert_eq!(from_second.target(20.0), 0);
        assert_eq!(from_second.target(30.0), 1);
        assert_eq!(from_second.target(150.0), 2);
        assert_eq!(from_second.target(500.0), 3);
    }
}

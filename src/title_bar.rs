use eframe::egui::{pos2, Rect};

pub struct TitleLayout {
    pub path: Rect,
    pub status: Rect,
    pub count: Rect,
    pub controls: [Rect; 3],
}

pub fn layout(bar: Rect, path_width: f32, count_width: f32, status_width: f32) -> TitleLayout {
    let button_width = 36.0_f32.min(bar.width() / 3.0);
    let controls = std::array::from_fn(|i| {
        let right = bar.right() - (2 - i) as f32 * button_width;
        Rect::from_min_max(
            pos2(right - button_width, bar.top()),
            pos2(right, bar.bottom()),
        )
    });
    let content_right = (controls[0].left() - 8.0).max(bar.left());
    let available = (content_right - bar.left()).max(0.0);
    let count_width = count_width.min(available);
    let status_gap = if status_width > 0.0 { 8.0 } else { 0.0 };
    let status_width = status_width
        .min((available - count_width - status_gap - 8.0 - path_width.min(64.0)).max(0.0));
    let fits = path_width + 8.0 + count_width + status_gap + status_width <= available;
    let count_left = if fits {
        bar.left() + path_width + 8.0
    } else {
        (content_right - count_width - status_gap - status_width).max(bar.left())
    };
    let count = Rect::from_min_max(
        pos2(count_left, bar.top()),
        pos2(count_left + count_width, bar.bottom()),
    );
    let status_left = (count.right() + status_gap).min(content_right);
    let status = Rect::from_min_max(
        pos2(status_left, bar.top()),
        pos2(
            (status_left + status_width).min(content_right),
            bar.bottom(),
        ),
    );
    let path_right = (count.left() - 8.0).max(bar.left());
    let path = Rect::from_min_max(bar.min, pos2(path_right, bar.bottom()));
    TitleLayout {
        path,
        status,
        count,
        controls,
    }
}

#[derive(Default)]
pub struct Marquee {
    path: String,
    width: f32,
    started: f64,
}

impl Marquee {
    pub fn offset(&mut self, path: &str, width: f32, text_width: f32, now: f64) -> f32 {
        if self.path != path || (self.width - width).abs() > 0.5 {
            self.path = path.to_owned();
            self.width = width;
            self.started = now;
        }
        let overflow = (text_width - width).max(0.0);
        if overflow == 0.0 || width <= 0.0 {
            return 0.0;
        }
        let speed = 32.0;
        let travel = overflow as f64 / speed;
        let phase = (now - self.started).max(0.0) % (travel + 3.0);
        ((phase - 1.5).max(0.0).min(travel) * speed) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    #[test]
    fn text_regions_never_overlap_controls_or_each_other() {
        for width in [240.0, 400.0, 800.0] {
            let bar = Rect::from_min_size(pos2(8.0, 0.0), egui::vec2(width - 16.0, 28.0));
            let regions = layout(bar, 1000.0, 70.0, 300.0);
            assert!(regions.path.right() <= regions.count.left());
            assert!(regions.count.right() <= regions.status.left());
            assert!(regions.status.right() <= regions.controls[0].left());
            assert_eq!(regions.count.width(), 70.0);
            for button in regions.controls {
                assert!(bar.contains_rect(button));
                assert_eq!(button.width(), 36.0);
            }
        }
    }

    #[test]
    fn short_title_keeps_metadata_after_path_and_long_title_docks_it() {
        let bar = Rect::from_min_size(pos2(8.0, 0.0), egui::vec2(784.0, 28.0));
        let short = layout(bar, 100.0, 70.0, 150.0);
        assert_eq!(short.path.width(), 100.0);
        assert_eq!(short.count.left(), short.path.right() + 8.0);
        assert_eq!(short.status.left(), short.count.right() + 8.0);
        assert!(short.status.right() < short.controls[0].left() - 8.0);
        let long = layout(bar, 1000.0, 70.0, 150.0);
        assert_eq!(long.status.right(), long.controls[0].left() - 8.0);
        assert!(long.path.width() < 1000.0);
    }

    #[test]
    fn marquee_pauses_moves_loops_and_resets_for_path_or_width() {
        let mut marquee = Marquee::default();
        assert_eq!(marquee.offset("path", 100.0, 164.0, 0.0), 0.0);
        assert_eq!(marquee.offset("path", 100.0, 164.0, 1.0), 0.0);
        assert_eq!(marquee.offset("path", 100.0, 164.0, 2.5), 32.0);
        assert_eq!(marquee.offset("path", 100.0, 164.0, 4.0), 64.0);
        assert_eq!(marquee.offset("path", 100.0, 164.0, 5.0), 0.0);
        assert_eq!(marquee.offset("new", 100.0, 164.0, 6.0), 0.0);
        assert_eq!(marquee.offset("new", 90.0, 164.0, 9.0), 0.0);
        assert_eq!(marquee.offset("new", 200.0, 164.0, 20.0), 0.0);
    }
}

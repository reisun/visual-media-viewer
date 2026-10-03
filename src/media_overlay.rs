use eframe::egui;

#[derive(Clone, Copy)]
pub enum MediaOverlay {
    Loading,
    Stopped,
}

pub fn paint(ui: &mut egui::Ui, bounds: egui::Rect, state: MediaOverlay) {
    let panel = egui::Rect::from_center_size(bounds.center(), egui::vec2(48.0, 48.0));
    let icon = egui::Rect::from_center_size(bounds.center(), egui::vec2(22.0, 22.0));
    let painter = ui
        .painter()
        .with_clip_rect(bounds.intersect(ui.clip_rect()));
    painter.rect_filled(panel, 8.0, egui::Color32::from_black_alpha(170));
    match state {
        MediaOverlay::Loading => {
            ui.put(
                icon,
                egui::Spinner::new().size(22.0).color(egui::Color32::WHITE),
            )
            .on_hover_text("読み込み中");
        }
        MediaOverlay::Stopped => {
            painter.rect_filled(icon, 2.0, egui::Color32::WHITE);
            ui.interact(panel, ui.id().with("media_stopped"), egui::Sense::hover())
                .on_hover_text("停止");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopped_overlay_is_centered_and_painted_after_media() {
        let ctx = egui::Context::default();
        let media_color = egui::Color32::from_rgb(20, 80, 120);
        let mut bounds = egui::Rect::NOTHING;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 240.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    bounds = ui.available_rect_before_wrap();
                    ui.painter().rect_filled(bounds, 0.0, media_color);
                    paint(ui, bounds, MediaOverlay::Stopped);
                });
            },
        );
        let rectangles: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Rect(rect) = &shape.shape {
                    Some(rect)
                } else {
                    None
                }
            })
            .collect();
        let media = rectangles
            .iter()
            .position(|rect| rect.fill == media_color)
            .unwrap();
        let backdrop = rectangles
            .iter()
            .position(|rect| rect.fill == egui::Color32::from_black_alpha(170))
            .unwrap();
        let mark = rectangles
            .iter()
            .position(|rect| {
                rect.fill == egui::Color32::WHITE && rect.rect.size() == egui::vec2(22.0, 22.0)
            })
            .unwrap();
        assert!(media < backdrop && backdrop < mark);
        assert_eq!(rectangles[mark].rect.center(), bounds.center());
        assert!(rectangles[backdrop]
            .rect
            .contains_rect(rectangles[mark].rect));
    }
}

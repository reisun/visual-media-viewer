use crate::shell_integration::{self, Status};
use eframe::egui;
use std::sync::mpsc;

enum Action {
    Refresh,
    RegisterApp(bool),
    DefaultApps,
    Thumbnails(bool),
}

struct Completion {
    status: Result<Status, String>,
    error: Option<String>,
}

#[derive(Default)]
pub struct ShellIntegrationUi {
    open: bool,
    focused: bool,
    status: Option<Status>,
    error: Option<String>,
    pending: Option<mpsc::Receiver<Completion>>,
}

impl ShellIntegrationUi {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, ctx: &egui::Context) {
        self.open = true;
        self.start(Action::Refresh, ctx);
    }

    fn start(&mut self, action: Action, ctx: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = match action {
                Action::Refresh => Ok(()),
                Action::RegisterApp(true) => shell_integration::register_app(),
                Action::RegisterApp(false) => shell_integration::unregister_app(),
                Action::DefaultApps => shell_integration::inspect()
                    .and_then(|status| {
                        if status.app_registered {
                            Ok(())
                        } else {
                            shell_integration::register_app()
                        }
                    })
                    .and_then(|_| shell_integration::open_default_apps()),
                Action::Thumbnails(enabled) => shell_integration::set_thumbnails_enabled(enabled),
            };
            let _ = tx.send(Completion {
                status: shell_integration::inspect(),
                error: result.err(),
            });
            ctx.request_repaint();
        });
    }

    pub fn draw(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(completion) => {
                    self.pending = None;
                    self.error = completion.error;
                    match completion.status {
                        Ok(status) => self.status = Some(status),
                        Err(error) => {
                            self.status = None;
                            self.error.get_or_insert(error);
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.status = None;
                    self.error = Some("Windowsの設定を確認できませんでした。".to_owned());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let focused = ctx.input(|input| input.focused);
        if focused && !self.focused {
            self.start(Action::Refresh, ctx);
        }
        self.focused = focused;

        let mut open = self.open;
        let mut action = None;
        egui::Window::new("Windowsとの連携")
            .open(&mut open)
            .collapsible(false)
            .default_width(560.0)
            .max_width((ctx.screen_rect().width() - 24.0).max(240.0))
            .max_height((ctx.screen_rect().height() - 48.0).max(120.0))
            .constrain_to(ctx.screen_rect().shrink(6.0))
            .vscroll(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(self.pending.is_none(), egui::Button::new("状態を更新"))
                        .clicked()
                    {
                        action = Some(Action::Refresh);
                    }
                    if self.pending.is_some() {
                        ui.spinner();
                    }
                });
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                let Some(status) = &self.status else { return };
                ui.add_enabled_ui(self.pending.is_none(), |ui| {
                    ui.separator();
                    ui.strong("ファイルの関連付け");
                    let mut registered = status.app_registered;
                    if ui.checkbox(&mut registered, "関連付け候補に登録").changed() {
                        action = Some(Action::RegisterApp(registered));
                    }
                    if status.app_partial {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("候補登録が一部残っています。");
                            if ui.button("修復").clicked() {
                                action = Some(Action::RegisterApp(true));
                            }
                            if ui.button("候補登録を解除").clicked() {
                                action = Some(Action::RegisterApp(false));
                            }
                        });
                    }
                    if status.app_update_available && ui.button("関連付け候補を更新").clicked()
                    {
                        action = Some(Action::RegisterApp(true));
                    }
                    ui.label(format!(
                        "既定のアプリ: {} / {} 形式",
                        status.default_count, status.total_count
                    ));
                    if ui.button("既定のアプリを設定...").clicked() {
                        action = Some(Action::DefaultApps);
                    }

                    ui.separator();
                    ui.strong("エクスプローラーのサムネイル対応");
                    let mut thumbnails = status.thumbnails_enabled;
                    if ui
                        .checkbox(&mut thumbnails, "サムネイル対応を登録")
                        .changed()
                    {
                        action = Some(Action::Thumbnails(thumbnails));
                    }
                    if status.thumbnail_update_available
                        && ui.button("サムネイル部品を更新").clicked()
                    {
                        action = Some(Action::Thumbnails(true));
                    }
                    ui.label(format!(
                        "登録済み: {} / {} 形式",
                        status.thumbnail_count, status.total_count
                    ));
                    ui.label(format!(
                        "適用中: {} / {} 形式",
                        status.thumbnail_effective_count, status.total_count
                    ));
                    if status.thumbnails_partial {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("一部の登録が一致していません。");
                            if ui.button("修復").clicked() {
                                action = Some(Action::Thumbnails(true));
                            }
                            if ui.button("登録を解除").clicked() {
                                action = Some(Action::Thumbnails(false));
                            }
                        });
                    }

                    ui.separator();
                    egui::ScrollArea::both()
                        .max_height((ctx.screen_rect().height() - 360.0).clamp(80.0, 300.0))
                        .show(ui, |ui| {
                            egui::Grid::new("shell_extension_status")
                                .num_columns(3)
                                .striped(true)
                                .show(ui, |ui| {
                                    ui.strong("拡張子");
                                    ui.strong("既定のアプリ");
                                    ui.strong("サムネイル登録");
                                    ui.end_row();
                                    for extension in &status.extensions {
                                        ui.label(&extension.extension);
                                        ui.label(if extension.is_default {
                                            "このビューアー"
                                        } else {
                                            "未設定 / 他のアプリ"
                                        });
                                        ui.label(
                                            match (
                                                extension.thumbnail_registered,
                                                extension.thumbnail_effective,
                                            ) {
                                                (true, true) => "有効",
                                                (true, false) => "登録済み / 他の設定優先",
                                                (false, true) => "別の登録で有効",
                                                (false, false) => "未登録",
                                            },
                                        );
                                        ui.end_row();
                                    }
                                });
                        });
                });
            });
        self.open = open;
        if let Some(action) = action {
            self.start(action, ctx);
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_integration::ExtensionStatus;

    #[test]
    fn integration_panel_fits_small_and_large_windows_with_partial_state() {
        for size in [egui::vec2(320.0, 480.0), egui::vec2(1280.0, 800.0)] {
            let ctx = egui::Context::default();
            let mut panel = ShellIntegrationUi {
                open: true,
                focused: true,
                status: Some(Status {
                    app_registered: false,
                    app_partial: true,
                    total_count: 21,
                    thumbnails_partial: true,
                    extensions: (0..21)
                        .map(|index| ExtensionStatus {
                            extension: format!(".extension{index}"),
                            thumbnail_registered: true,
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                }),
                error: Some(
                    "A registration is incomplete. The previous settings were preserved.".into(),
                ),
                pending: None,
            };
            for frame in 0..3 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        time: Some(frame as f64 / 60.0),
                        focused: true,
                        ..Default::default()
                    },
                    |ctx| panel.draw(ctx),
                );
            }
            let rect = ctx
                .memory(|memory| memory.area_rect(egui::Id::new("Windowsとの連携")))
                .unwrap();
            assert!(
                rect.width() <= size.x,
                "panel width: {rect:?}, viewport: {size:?}"
            );
            assert!(
                rect.height() <= size.y,
                "panel height: {rect:?}, viewport: {size:?}"
            );
            assert!(panel.pending.is_none());
        }
    }
}

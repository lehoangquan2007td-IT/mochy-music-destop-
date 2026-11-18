//! `mochy-signin` — the Google sign-in window of Mochy Music's YouTube account.
//!
//! mochy-app runs it only when the user clicks "Sign in with Google", so the
//! main app never loads WebView2 (on the GNU toolchain `wry` needs
//! `WebView2Loader.dll` next to the exe; a missing DLL then breaks only this
//! helper, never the player).
//!
//! ```text
//! mochy-signin --profile <dir> [--fresh] [--lang vi|en]   # run by mochy-app
//! mochy-signin                                            # double-clicked
//! ```
//!
//! Run by mochy-app (`--profile`), on success it prints one JSON line to
//! stdout and exits 0: `{"cookie": "...", "dataSyncId": "...", "visitorData": "..."}`.
//! Errors print `{"error": "..."}` and exit 1; closing the window prints
//! nothing and exits 2. The cookie only ever goes to the parent's pipe.
//!
//! Started on its own (no `--profile`), it saves the session itself
//! (DPAPI-encrypted `yt_session.bin`, the same file mochy-app uses); a
//! running Mochy picks it up within a few seconds, otherwise on next start.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod login;

#[cfg(not(windows))]
fn main() {
    println!("{}", serde_json::json!({ "error": "the sign-in window needs Windows (WebView2)" }));
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    use std::path::PathBuf;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut profile: Option<PathBuf> = None;
    let mut fresh = false;
    let mut vi = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--profile" => profile = it.next().map(PathBuf::from),
            "--fresh" => fresh = true,
            "--lang" => vi = it.next().is_some_and(|l| l == "vi"),
            _ => {}
        }
    }
    // Double-clicked: use Mochy's own profile and save the session ourselves.
    let standalone = profile.is_none();
    if standalone {
        vi = mochy_storage::SettingsStore::open_default()
            .load()
            .map(|s| matches!(s.appearance.language, mochy_core::settings::Language::Vi))
            .unwrap_or(true);
    }
    let profile = profile.unwrap_or_else(mochy_storage::yt_session::webview_dir);
    let title = if vi { "Mochy — Đăng nhập YouTube" } else { "Mochy — Sign in to YouTube" };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([980.0, 780.0])
            .with_min_inner_size([480.0, 560.0])
            .with_active(true),
        ..Default::default()
    };
    let result = eframe::run_native(
        "mochy-signin",
        options,
        Box::new(move |cc| {
            app::install_fonts(&cc.egui_ctx);
            Ok(Box::new(app::SignIn::new(login::LoginWindow::new(profile, fresh), vi, standalone)))
        }),
    );
    if let Err(e) = result {
        app::fail(&format!("window: {e}"));
    }
    // Window closed without finishing.
    std::process::exit(2);
}

#[cfg(windows)]
mod app {
    use std::io::Write;
    use std::sync::Arc;

    use crate::login::{LoginWindow, Outcome};

    const BAR_H: f32 = 52.0;

    pub fn fail(msg: &str) -> ! {
        println!("{}", serde_json::json!({ "error": msg }));
        let _ = std::io::stdout().flush();
        std::process::exit(1);
    }

    /// Segoe UI first so Vietnamese renders (egui's bundled font lacks it).
    pub fn install_fonts(ctx: &egui::Context) {
        let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") else { return };
        let mut defs = egui::FontDefinitions::default();
        defs.font_data.insert("segoeui".into(), Arc::new(egui::FontData::from_owned(bytes)));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            defs.families.entry(family).or_default().insert(0, "segoeui".into());
        }
        ctx.set_fonts(defs);
    }

    pub struct SignIn {
        login: Option<LoginWindow>,
        vi: bool,
        standalone: bool,
        /// Standalone: the session was saved at this moment (show a note, then quit).
        saved_at: Option<std::time::Instant>,
        error: Option<String>,
    }

    impl SignIn {
        pub fn new(login: LoginWindow, vi: bool, standalone: bool) -> Self {
            Self { login: Some(login), vi, standalone, saved_at: None, error: None }
        }

        /// Standalone end screen (the WebView is gone by now).
        fn end_screen(&mut self, ui: &mut egui::Ui) {
            let ctx = ui.ctx().clone();
            egui::CentralPanel::default().show(ui, |ui| {
                ui.add_space(ui.available_height() * 0.35);
                ui.vertical_centered(|ui| {
                    if let Some(e) = &self.error {
                        ui.label(egui::RichText::new(self.t("Signing in failed", "Đăng nhập thất bại")).size(20.0).strong());
                        ui.label(e.as_str());
                        ui.add_space(12.0);
                        if ui.button(self.t("Close", "Đóng")).clicked() {
                            std::process::exit(1);
                        }
                    } else {
                        ui.label(egui::RichText::new(self.t("Signed in!", "Đã đăng nhập!")).size(22.0).strong());
                        ui.label(self.t(
                            "Go back to Mochy — it picks the account up in a few seconds (or the next time it starts).",
                            "Quay lại Mochy — app sẽ tự nhận tài khoản sau vài giây (hoặc ở lần mở tiếp theo).",
                        ));
                    }
                });
            });
            if self.saved_at.is_some_and(|t| t.elapsed().as_secs() >= 4) {
                std::process::exit(0);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        fn t(&self, en: &'static str, vi: &'static str) -> &'static str {
            if self.vi { vi } else { en }
        }
    }

    impl eframe::App for SignIn {
        fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
            if self.login.is_none() {
                self.end_screen(ui);
                return;
            }
            let ctx = ui.ctx().clone();
            let screen = ctx.content_rect();
            let mut cancel = false;
            egui::Area::new(egui::Id::unique("mochy_signin_bar")).order(egui::Order::Foreground).fixed_pos(screen.min).show(&ctx, |ui| {
                egui::Frame::new().fill(egui::Color32::from_rgb(0x1C, 0x1C, 0x20)).inner_margin(egui::Margin::symmetric(16, 10)).show(ui, |ui| {
                    ui.set_width(screen.width() - 32.0);
                    ui.set_height(BAR_H - 20.0);
                    ui.horizontal_centered(|ui| {
                        ui.label(egui::RichText::new(self.t("Sign in to YouTube", "Đăng nhập YouTube")).strong().size(15.0).color(egui::Color32::WHITE));
                        ui.label(
                            egui::RichText::new(self.t(
                                "Your password goes to Google only. Mochy keeps the session cookie, encrypted on this PC.",
                                "Mật khẩu chỉ gửi tới Google. Mochy chỉ giữ cookie phiên, được mã hóa trên máy này.",
                            ))
                            .size(12.0)
                            .color(egui::Color32::from_gray(170)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(self.t("Cancel", "Hủy")).clicked() {
                                cancel = true;
                            }
                        });
                    });
                });
            });
            let Some(login) = self.login.as_mut() else { return };
            if cancel {
                eprintln!("sign-in cancelled at {:?}", login.current_url());
                std::process::exit(2);
            }
            let area = egui::Rect::from_min_max(egui::pos2(screen.min.x, screen.min.y + BAR_H), screen.max);
            match login.frame(frame, area, ctx.pixels_per_point()) {
                Outcome::Pending => ctx.request_repaint_after(std::time::Duration::from_millis(300)),
                Outcome::Done(s) if self.standalone => {
                    let session = mochy_storage::yt_session::Session {
                        cookie: s.cookie,
                        data_sync_id: s.data_sync_id,
                        visitor_data: s.visitor_data,
                    };
                    // Dropping the LoginWindow closes the WebView; the end screen shows.
                    self.login = None;
                    match mochy_storage::yt_session::save_session(&session) {
                        Ok(()) => self.saved_at = Some(std::time::Instant::now()),
                        Err(e) => self.error = Some(e),
                    }
                }
                Outcome::Failed(e) if self.standalone => {
                    self.login = None;
                    self.error = Some(e);
                }
                Outcome::Done(s) => {
                    println!(
                        "{}",
                        serde_json::json!({ "cookie": s.cookie, "dataSyncId": s.data_sync_id, "visitorData": s.visitor_data })
                    );
                    let _ = std::io::stdout().flush();
                    std::process::exit(0);
                }
                Outcome::Failed(e) => fail(&e),
            }
        }
    }
}

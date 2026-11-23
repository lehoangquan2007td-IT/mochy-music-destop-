//! Settings → YouTube account, and the account button in the top-right
//! corner of the main view.

use egui::{Align2, Color32, CornerRadius, FontId, RichText, Sense, Stroke, Ui, vec2};
use mochy_core::settings::AppSettings;

use super::{Request, Status, request, shared};
use crate::actions::{Action, Ctx};
use crate::images::ImageSrc;
use crate::nav::Route;
use crate::ui::i18n::{self, tr};
use crate::widgets::{self, TEXT, TEXT_DIM};

const ERROR: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x8A);
const OK: Color32 = Color32::from_rgb(0x8A, 0xE0, 0x9A);

fn group(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).size(18.0).strong().color(TEXT));
    ui.add_space(6.0);
    egui::Frame::new().fill(widgets::SURFACE).corner_radius(14).inner_margin(16).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 10.0;
        body(ui);
    });
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(TEXT_DIM));
}

const AVATAR: f32 = 32.0;

/// The account button (top-right corner). Signed out: "Sign in", which opens
/// the Google sign-in window (mochy-signin). Signed in: the avatar, with a
/// menu for sync, switching account, the account settings and signing out.
/// Lay it out right-to-left.
pub fn corner_button(ui: &mut Ui, cx: &mut Ctx) {
    let Some(sh) = shared() else { return };
    let (status, account, syncing) = {
        let v = sh.view.lock().expect("account view");
        (v.status.clone(), v.account.clone(), v.syncing)
    };
    match status {
        Status::SignedOut => {
            let label = RichText::new(tr("Sign in")).strong().color(Color32::BLACK);
            let b = egui::Button::new(label).fill(Color32::WHITE).corner_radius(16).min_size(vec2(96.0, AVATAR));
            let r = ui.add(b).on_hover_text(tr("Sign in with Google"));
            if r.clicked() {
                if cfg!(windows) {
                    request(Request::SignIn { fresh: false });
                } else {
                    cx.act(Action::Navigate(Route::Settings));
                }
            }
        }
        Status::SigningIn => {
            if ui.button(tr("Cancel")).clicked() {
                request(Request::CancelSignIn);
            }
            ui.label(RichText::new(tr("Signing in…")).color(TEXT_DIM));
            ui.spinner();
        }
        Status::Verifying => {
            ui.label(RichText::new(tr("Checking the session with YouTube…")).color(TEXT_DIM));
            ui.spinner();
        }
        Status::SignedIn | Status::Expired => {
            let expired = status == Status::Expired;
            let (rect, resp) = ui.allocate_exact_size(vec2(AVATAR, AVATAR), Sense::click());
            let tex = account.thumbnail_url.as_ref().and_then(|u| cx.images.get(&ImageSrc::Url(u.clone()), 64));
            if let Some(t) = &tex {
                widgets::paint_artwork(ui, rect, Some(t), AVATAR / 2.0);
            } else {
                ui.painter().circle_filled(rect.center(), AVATAR / 2.0, cx.accent);
                let initial = account.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, initial, FontId::proportional(15.0), Color32::WHITE);
            }
            if resp.hovered() {
                ui.painter().circle_stroke(rect.center(), AVATAR / 2.0 + 1.5, Stroke::new(2.0, Color32::from_white_alpha(90)));
            }
            if expired {
                ui.painter().circle(rect.right_top() + vec2(-4.0, 4.0), 5.0, ERROR, Stroke::new(2.0, Color32::BLACK));
            }
            let resp = resp
                .on_hover_text(if account.name.is_empty() { tr("Signed in") } else { &account.name })
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            egui::Popup::menu(&resp).show(|ui| {
                ui.set_min_width(220.0);
                ui.add_space(4.0);
                ui.label(RichText::new(if account.name.is_empty() { tr("Signed in") } else { &account.name }).size(15.0).strong().color(TEXT));
                if let Some(e) = account.email.as_deref().or(account.channel_handle.as_deref()) {
                    ui.label(RichText::new(e).size(12.0).color(TEXT_DIM));
                }
                ui.separator();
                if expired {
                    ui.label(RichText::new(tr("The session expired. Sign in again to keep syncing.")).size(12.0).color(ERROR));
                    if cfg!(windows) && ui.button(tr("Sign in again")).clicked() {
                        request(Request::SignIn { fresh: false });
                    }
                } else {
                    let label = if syncing { tr("Syncing…") } else { tr("Sync now") };
                    if ui.add_enabled(!syncing, egui::Button::new(label)).clicked() {
                        request(Request::SyncNow);
                    }
                }
                if cfg!(windows) && ui.button(tr("Use another account")).clicked() {
                    request(Request::SignIn { fresh: true });
                }
                if ui.button(tr("Account settings")).clicked() {
                    cx.act(Action::Navigate(Route::Settings));
                }
                ui.separator();
                if ui.add(egui::Button::new(RichText::new(tr("Sign out")).color(ERROR)).corner_radius(CornerRadius::same(6))).clicked() {
                    request(Request::SignOut);
                }
            });
        }
    }
}

/// One-line entry point from `pages/settings.rs`.
pub fn settings_section(ui: &mut Ui, s: &mut AppSettings) {
    let Some(sh) = shared() else { return };
    let (status, account, syncing, error) = {
        let v = sh.view.lock().expect("account view");
        (v.status.clone(), v.account.clone(), v.syncing, v.error.clone())
    };
    group(ui, tr("YouTube account"), |ui| {
        match status {
            Status::SignedOut => {
                hint(ui, tr("Sign in to get your YouTube Music home, mixes from youtube.com and your liked songs, playlists, albums and artists in Mochy."));
                sign_in_buttons(ui, false);
                paste_cookie(ui);
            }
            Status::SigningIn => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("The Google sign-in page is open — finish signing in there."));
                });
                if ui.button(tr("Cancel")).clicked() {
                    request(Request::CancelSignIn);
                }
            }
            Status::Verifying => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Checking the session with YouTube…"));
                });
            }
            Status::SignedIn | Status::Expired => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(if account.name.is_empty() { tr("Signed in") } else { &account.name }).size(16.0).strong());
                    let extra: Vec<&str> = [account.email.as_deref(), account.channel_handle.as_deref()].into_iter().flatten().collect();
                    if !extra.is_empty() {
                        ui.label(RichText::new(extra.join(" · ")).color(TEXT_DIM));
                    }
                });
                if status == Status::Expired {
                    ui.label(RichText::new(tr("The session expired or was signed out on another device. Sign in again to keep syncing.")).color(ERROR));
                    sign_in_buttons(ui, true);
                } else {
                    sync_line(ui, &account, syncing);
                }
                ui.horizontal(|ui| {
                    if status == Status::SignedIn {
                        let label = if syncing { tr("Syncing…") } else { tr("Sync now") };
                        if ui.add_enabled(!syncing, egui::Button::new(label)).clicked() {
                            request(Request::SyncNow);
                        }
                    }
                    #[cfg(windows)]
                    if ui.button(tr("Use another account")).clicked() {
                        request(Request::SignIn { fresh: true });
                    }
                    if ui.button(tr("Sign out")).clicked() {
                        request(Request::SignOut);
                    }
                });
                if status == Status::Expired {
                    paste_cookie(ui);
                }
            }
        }
        if let Some(e) = &error {
            ui.label(RichText::new(e).color(ERROR).size(12.5));
        }

        ui.separator();
        let y = &mut s.youtube_account;
        ui.checkbox(&mut y.home_mixes, tr("Show mixes from the youtube.com home page on Home"));
        ui.checkbox(&mut y.home_music_videos, tr("Also show music videos from youtube.com (guessed from the channel, may include non-music)"));
        ui.checkbox(&mut y.home_history, tr("Show my YouTube history on Home"));
        ui.checkbox(&mut y.sync_on_start, tr("Sync the library when Mochy starts (at most every 6 hours)"));
        hint(ui, tr("Read-only: Mochy never changes your YouTube account. Liked songs are added to your likes here; synced playlists, albums and artists are saved to the library and removed again when you remove them on YouTube. The session is encrypted for your Windows user and only sent to YouTube."));
    });
}

fn sign_in_buttons(ui: &mut Ui, again: bool) {
    #[cfg(windows)]
    {
        let label = if again { tr("Sign in again") } else { tr("Sign in with Google") };
        if ui.add(egui::Button::new(RichText::new(label).strong()).min_size(egui::vec2(180.0, 32.0))).clicked() {
            request(Request::SignIn { fresh: false });
        }
    }
    #[cfg(not(windows))]
    {
        let _ = again;
        hint(ui, tr("Signing in inside the app needs Windows — paste the cookie instead."));
    }
}

fn sync_line(ui: &mut Ui, account: &super::store::AccountFile, syncing: bool) {
    if syncing {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tr("Syncing your YouTube library…"));
        });
        return;
    }
    match (account.last_sync_ms, &account.last_report) {
        (Some(at), Some(r)) => {
            let ago = i18n::ago_label(((crate::browse::now_ms() - at).max(0) / 60_000) as u64);
            ui.label(RichText::new(format!("{} {ago}", tr("Last synced"))).color(OK));
            hint(ui, &i18n::yt_sync_counts(r.liked_total, r.playlists, r.albums, r.artists, r.history));
        }
        _ => hint(ui, tr("Not synced yet.")),
    }
}

fn paste_cookie(ui: &mut Ui) {
    let id = egui::Id::unique("mochy_yt_cookie_paste");
    egui::CollapsingHeader::new(RichText::new(tr("Advanced: paste a cookie")).color(TEXT_DIM)).id_salt(id).show(ui, |ui| {
        hint(ui, tr("In a browser signed in to music.youtube.com, open the developer tools (F12) → Network, click any request to music.youtube.com and copy the whole \"Cookie\" request header."));
        let mut text: String = ui.data_mut(|d| d.get_temp::<String>(id).unwrap_or_default());
        ui.add(egui::TextEdit::multiline(&mut text).password(true).desired_rows(2).desired_width(f32::INFINITY).hint_text("SAPISID=…; SID=…; …"));
        ui.horizontal(|ui| {
            if ui.add_enabled(!text.trim().is_empty(), egui::Button::new(tr("Use this cookie"))).clicked() {
                request(Request::PasteCookie(std::mem::take(&mut text)));
            }
            if !text.is_empty() && ui.button(tr("Clear")).clicked() {
                text.clear();
            }
        });
        ui.data_mut(|d| d.insert_temp(id, text));
    });
}

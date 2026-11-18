//! Settings → YouTube account.

use egui::{Color32, RichText, Ui};
use mochy_core::settings::AppSettings;

use super::{Request, Status, request, shared};
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

//! Settings → Integrations (Discord, scrobbling) and Import & sync.

use std::sync::atomic::Ordering;

use egui::{Color32, RichText, Ui};
use mochy_core::integrations::{DiscordSettings, PresenceButton, PresenceButtonTarget, PresenceImage, PresenceKind, ScrobbleSettings};
use mochy_core::settings::AppSettings;
use mochy_integrations::discord::activity::{PLACEHOLDERS, PresenceActivity};
use mochy_integrations::discord::{NowPlaying, PresenceConnection};
use mochy_integrations::scrobble::scrobbler::{account_mut, api_for, sign_in_error_message};
use mochy_integrations::scrobble::ScrobbleSite;
use mochy_integrations::unix_millis;

use super::{Patch, SiteAuth, shared, site_index};
use crate::ui::i18n::tr;
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
    if let Some(sh) = shared() {
        sh.apply_patches(s);
    }
    group(ui, tr("Discord"), |ui| discord(ui, &mut s.integrations.discord));
    group(ui, tr("Scrobbling"), |ui| scrobbling(ui, &mut s.integrations.scrobble));
    group(ui, tr("Import & sync"), |ui| {
        hint(ui, tr("Bring playlists and liked songs over from Spotify, Apple Music, Tidal, Deezer, YouTube Music or a CSV file. Songs are matched on YouTube Music and saved as local playlists."));
        ui.horizontal(|ui| {
            if ui.button(tr("Import playlists…")).clicked() {
                super::open_importer();
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Discord
// ---------------------------------------------------------------------------

fn status_text(st: &PresenceConnection, user: Option<&str>) -> (String, Color32) {
    match st {
        PresenceConnection::Off => (tr("Off").to_string(), TEXT_DIM),
        PresenceConnection::Idle => (tr("Connected — nothing playing").to_string(), TEXT_DIM),
        PresenceConnection::Connecting => (tr("Connecting to Discord…").to_string(), TEXT_DIM),
        PresenceConnection::Live => {
            let base = tr("Showing on your profile");
            (match user {
                Some(u) => format!("{base} ({u})"),
                None => base.to_string(),
            }, OK)
        }
        PresenceConnection::Retrying => (tr("Discord isn't running — trying again…").to_string(), TEXT_DIM),
        PresenceConnection::Error(e) => (format!("{} {e}", tr("Discord refused the status:")), ERROR),
    }
}

fn image_label(i: PresenceImage) -> &'static str {
    tr(i.label())
}

fn image_combo(ui: &mut Ui, id: &str, value: &mut PresenceImage, large: bool) {
    egui::ComboBox::from_id_salt(id).selected_text(image_label(*value)).show_ui(ui, |ui| {
        for i in PresenceImage::ALL {
            if large && !i.for_large_image() {
                continue;
            }
            ui.selectable_value(value, i, image_label(i));
        }
    });
}

fn button_editor(ui: &mut Ui, id: &str, label: &str, b: &mut PresenceButton) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::TextEdit::singleline(&mut b.text).desired_width(200.0).char_limit(64));
        egui::ComboBox::from_id_salt(id).selected_text(tr(b.target.label())).show_ui(ui, |ui| {
            for t in PresenceButtonTarget::ALL {
                ui.selectable_value(&mut b.target, t, tr(t.label()));
            }
        });
    });
}

fn line_editor(ui: &mut Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.add_sized([110.0, 18.0], egui::Label::new(label));
        ui.add(egui::TextEdit::singleline(value).desired_width(280.0).char_limit(128));
    });
}

fn discord(ui: &mut Ui, d: &mut DiscordSettings) {
    let (st, user) = shared().map(|s| s.presence.lock().unwrap().clone()).unwrap_or((PresenceConnection::Off, None));
    ui.checkbox(&mut d.enabled, tr("Show what you're listening to on Discord"));
    let (text, color) = if d.enabled && d.application_id.trim().is_empty() {
        (tr("Enter a Discord application ID to turn it on.").to_string(), ERROR)
    } else {
        status_text(&st, user.as_deref())
    };
    ui.label(RichText::new(text).color(color));
    ui.horizontal(|ui| {
        ui.label(tr("Application ID"));
        let r = ui.add(egui::TextEdit::singleline(&mut d.application_id).desired_width(220.0).hint_text("123456789012345678"));
        if r.changed() {
            d.application_id.retain(|c| c.is_ascii_digit());
        }
        if ui.small_button(tr("Developer Portal")).clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab("https://discord.com/developers/applications"));
        }
    });
    hint(ui, tr("Discord shows the status through its desktop app. Create an application in the Discord Developer Portal (its name is what your profile shows), copy its Application ID here, and keep Discord running."));

    egui::CollapsingHeader::new(tr("Customize the status")).id_salt("discord_custom").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(tr("Activity"));
            egui::ComboBox::from_id_salt("discord_kind")
                .selected_text(match d.kind {
                    PresenceKind::Listening => tr("Listening to"),
                    PresenceKind::Playing => tr("Playing"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.kind, PresenceKind::Listening, tr("Listening to"));
                    ui.selectable_value(&mut d.kind, PresenceKind::Playing, tr("Playing"));
                });
        });
        line_editor(ui, tr("First line"), &mut d.details);
        line_editor(ui, tr("Second line"), &mut d.state);
        line_editor(ui, tr("Big picture text"), &mut d.large_text);
        line_editor(ui, tr("Small picture text"), &mut d.small_text);
        let ph: Vec<String> = PLACEHOLDERS.iter().map(|(k, v)| format!("{k} = {}", tr(v))).collect();
        hint(ui, &format!("{} {}", tr("Placeholders:"), ph.join(" · ")));
        ui.horizontal(|ui| {
            ui.label(tr("Big picture"));
            image_combo(ui, "discord_large", &mut d.large_image, true);
            ui.label(tr("Small picture"));
            image_combo(ui, "discord_small", &mut d.small_image, false);
        });
        if d.large_image == PresenceImage::Custom {
            line_editor(ui, tr("Big picture URL"), &mut d.large_custom_image);
        }
        if d.small_image == PresenceImage::Custom {
            line_editor(ui, tr("Small picture URL"), &mut d.small_custom_image);
        }
        button_editor(ui, "discord_b1", tr("Button 1"), &mut d.button1);
        button_editor(ui, "discord_b2", tr("Button 2"), &mut d.button2);
        hint(ui, tr("An empty line or button label is left off."));
        ui.checkbox(&mut d.clear_on_pause, tr("Clear the status when paused"));
        ui.horizontal(|ui| {
            ui.label(tr("Refresh every"));
            ui.add(egui::Slider::new(&mut d.refresh_seconds, DiscordSettings::MIN_REFRESH_SECONDS..=DiscordSettings::MAX_REFRESH_SECONDS).suffix(" s"));
        });
        if ui.small_button(tr("Reset to defaults")).clicked() {
            let keep = (d.enabled, d.application_id.clone());
            *d = DiscordSettings { enabled: keep.0, application_id: keep.1, ..Default::default() };
        }
        preview(ui, d);
    });
}

/// What friends will see, from the same object that is sent.
fn preview(ui: &mut Ui, d: &DiscordSettings) {
    let now = unix_millis();
    let track = shared().and_then(|s| s.now_playing.lock().unwrap().clone()).unwrap_or_else(|| NowPlaying::sample(now));
    let a = PresenceActivity::build(&track, d, now);
    egui::Frame::new().fill(widgets::SURFACE_2).corner_radius(10).inner_margin(12).show(ui, |ui| {
        ui.label(RichText::new(tr("Preview")).size(12.0).color(TEXT_DIM));
        ui.label(RichText::new(match d.kind {
            PresenceKind::Listening => tr("Listening to your Discord app"),
            PresenceKind::Playing => tr("Playing your Discord app"),
        }).size(12.0).strong());
        if let Some(t) = &a.details {
            ui.label(RichText::new(t).strong().color(TEXT));
        }
        if let Some(t) = &a.state {
            ui.label(RichText::new(t).color(TEXT_DIM));
        }
        if let Some(t) = &a.small_text {
            ui.label(RichText::new(format!("◦ {t}")).size(12.0).color(TEXT_DIM));
        }
        if let (Some(s), Some(e)) = (a.start_ms, a.end_ms) {
            ui.label(RichText::new(format!("{} / {}", widgets::fmt_time((now - s) as f64 / 1000.0), widgets::fmt_time((e - s) as f64 / 1000.0))).size(12.0).color(TEXT_DIM));
        }
        ui.horizontal(|ui| {
            for b in &a.buttons {
                ui.add_enabled(false, egui::Button::new(&b.label));
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Scrobbling
// ---------------------------------------------------------------------------

fn scrobbling(ui: &mut Ui, s: &mut ScrobbleSettings) {
    hint(ui, tr("Send what you listen to to Last.fm and Libre.fm. A song counts once it has played for half its length or four minutes, whichever comes first; plays made offline are kept and sent later."));
    for site in ScrobbleSite::ALL {
        site_card(ui, s, site);
    }
    ui.checkbox(&mut s.use_now_playing, tr("Show \"Now Playing\" while a song plays"));
    ui.checkbox(&mut s.send_likes, tr("Send likes as loved tracks"));
    let pending = shared().map(|x| x.pending.load(Ordering::Relaxed)).unwrap_or(0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{}: {pending}", tr("Plays waiting to be sent"))).color(TEXT_DIM));
        if pending > 0
            && ui.small_button(tr("Send now")).clicked()
            && let Some(sh) = shared()
        {
            let sc = sh.scrobbler.clone();
            sh.svc.rt.spawn(async move {
                sc.send().await;
            });
        }
    });
    egui::CollapsingHeader::new(tr("When a play counts")).id_salt("scrobble_rules").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(tr("Played fraction"));
            let mut pct = (s.delay_percent * 100.0).round() as u32;
            if ui.add(egui::Slider::new(&mut pct, 10..=100).suffix(" %")).changed() {
                s.delay_percent = pct as f32 / 100.0;
            }
        });
        ui.horizontal(|ui| {
            ui.label(tr("Or after at most"));
            ui.add(egui::Slider::new(&mut s.delay_seconds, 30..=600).suffix(" s"));
        });
        ui.horizontal(|ui| {
            ui.label(tr("Ignore songs shorter than"));
            ui.add(egui::Slider::new(&mut s.min_song_seconds, 0..=120).suffix(" s"));
        });
        if ui.small_button(tr("Reset to defaults")).clicked() {
            let d = ScrobbleSettings::default();
            s.delay_percent = d.delay_percent;
            s.delay_seconds = d.delay_seconds;
            s.min_song_seconds = d.min_song_seconds;
        }
    });
}

fn site_card(ui: &mut Ui, s: &mut ScrobbleSettings, site: ScrobbleSite) {
    let Some(sh) = shared() else { return };
    let idx = site_index(site);
    egui::Frame::new().fill(widgets::SURFACE_2).corner_radius(10).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(site.label()).strong().size(15.0));
        let acc = account_mut(s, site);
        if acc.is_signed_in() {
            ui.label(RichText::new(format!("{} {}", tr("Signed in as"), acc.username)).color(OK));
            ui.horizontal(|ui| {
                ui.checkbox(&mut acc.enabled, tr("Scrobble"));
                if ui.small_button(tr("Open profile")).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(site.profile_url(&acc.username)));
                }
                if ui.small_button(tr("Sign out")).clicked() {
                    acc.session_key.clear();
                    acc.username.clear();
                    acc.enabled = false;
                    sh.set_auth(site, SiteAuth::Idle);
                }
            });
            return;
        }
        ui.horizontal(|ui| {
            ui.add_sized([80.0, 18.0], egui::Label::new(tr("API key")));
            ui.add(egui::TextEdit::singleline(&mut acc.api_key).desired_width(280.0));
        });
        ui.horizontal(|ui| {
            ui.add_sized([80.0, 18.0], egui::Label::new(tr("Secret")));
            ui.add(egui::TextEdit::singleline(&mut acc.api_secret).password(true).desired_width(280.0));
        });
        ui.horizontal_wrapped(|ui| {
            hint(ui, match site {
                ScrobbleSite::LastFm => tr("Create your own API account on Last.fm and paste its key and secret. They stay on this computer."),
                ScrobbleSite::LibreFm => tr("Libre.fm accepts any 32-character key and secret you make up. They stay on this computer."),
            });
            if ui.small_button(tr("Get an API key")).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(site.api_account_url()));
            }
        });
        let configured = acc.is_configured();
        let auth = sh.auth.lock().unwrap()[idx].clone();
        match &auth {
            SiteAuth::Working => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Signing in…"));
                });
            }
            SiteAuth::Browser { token, url, opened } => {
                if !opened {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url.clone()));
                    sh.auth.lock().unwrap()[idx] = SiteAuth::Browser { token: token.clone(), url: url.clone(), opened: true };
                }
                hint(ui, tr("Approve Mochy in the browser page that just opened, then come back and finish."));
                ui.horizontal(|ui| {
                    if ui.button(tr("I've approved it — finish")).clicked() {
                        finish_browser_sign_in(acc, site, token.clone());
                    }
                    if ui.small_button(tr("Open the page again")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url.clone()));
                    }
                    if ui.small_button(tr("Cancel")).clicked() {
                        sh.set_auth(site, SiteAuth::Idle);
                    }
                });
            }
            SiteAuth::Idle | SiteAuth::Error(_) => {
                if let SiteAuth::Error(e) = &auth {
                    ui.label(RichText::new(e).color(ERROR));
                }
                ui.add_enabled_ui(configured, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button(tr("Sign in with the browser")).clicked() {
                            start_browser_sign_in(acc, site);
                        }
                        if site == ScrobbleSite::LastFm {
                            hint(ui, tr("(recommended)"));
                        }
                    });
                    let mut login = sh.login.lock().unwrap();
                    let (user, pass) = &mut login[idx];
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(user).hint_text(tr("Username")).desired_width(140.0));
                        ui.add(egui::TextEdit::singleline(pass).hint_text(tr("Password")).password(true).desired_width(140.0));
                        if ui.button(tr("Sign in")).clicked() && !user.trim().is_empty() && !pass.is_empty() {
                            let (u, p) = (user.trim().to_string(), std::mem::take(pass));
                            password_sign_in(acc, site, u, p);
                        }
                    });
                    hint(ui, tr("The password only goes to the service and isn't saved."));
                });
                if ui.small_button(tr("Create an account")).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(site.sign_up_url()));
                }
            }
        }
    });
}

fn start_browser_sign_in(acc: &mochy_core::integrations::ScrobbleAccount, site: ScrobbleSite) {
    let Some(sh) = shared() else { return };
    let api = api_for(site, acc);
    sh.set_auth(site, SiteAuth::Working);
    let sh2 = sh.clone();
    sh.svc.rt.spawn(async move {
        match api.get_token().await {
            Ok(token) => {
                let url = api.auth_url(&token);
                sh2.set_auth(site, SiteAuth::Browser { token, url, opened: false });
            }
            Err(e) => sh2.set_auth(site, SiteAuth::Error(sign_in_error_message(&e))),
        }
    });
}

fn finish_browser_sign_in(acc: &mochy_core::integrations::ScrobbleAccount, site: ScrobbleSite, token: String) {
    let Some(sh) = shared() else { return };
    let api = api_for(site, acc);
    let previous = SiteAuth::Browser { token: token.clone(), url: api.auth_url(&token), opened: true };
    sh.set_auth(site, SiteAuth::Working);
    let sh2 = sh.clone();
    sh.svc.rt.spawn(async move {
        match api.get_session(&token).await {
            Ok(session) => {
                sh2.push_patch(Patch::SignedIn { site, name: session.name, key: session.key });
                sh2.set_auth(site, SiteAuth::Idle);
            }
            // not approved yet: back to waiting
            Err(e) if e.is_unauthorized_token() => sh2.set_auth(site, previous),
            Err(e) => sh2.set_auth(site, SiteAuth::Error(sign_in_error_message(&e))),
        }
    });
}

fn password_sign_in(acc: &mochy_core::integrations::ScrobbleAccount, site: ScrobbleSite, user: String, pass: String) {
    let Some(sh) = shared() else { return };
    let api = api_for(site, acc);
    sh.set_auth(site, SiteAuth::Working);
    let sh2 = sh.clone();
    sh.svc.rt.spawn(async move {
        match api.get_mobile_session(&user, &pass).await {
            Ok(session) => {
                sh2.push_patch(Patch::SignedIn { site, name: session.name, key: session.key });
                sh2.set_auth(site, SiteAuth::Idle);
            }
            Err(e) => sh2.set_auth(site, SiteAuth::Error(sign_in_error_message(&e))),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Headless draw of the whole section (no hub running: status lines fall
    /// back to "Off", account cards are skipped). The language is left alone:
    /// it is process-global and the i18n test owns it.
    #[test]
    fn section_draws_and_keeps_settings() {
        let ctx = egui::Context::default();
        let mut s = AppSettings::default();
        s.integrations.discord.enabled = true;
        for _ in 0..3 {
            let mut out = ctx.run_ui(Default::default(), |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| settings_section(ui, &mut s));
            });
            out.textures_delta.clear();
        }
        assert!(s.integrations.discord.enabled);
        assert_eq!(s.integrations.scrobble, ScrobbleSettings::default());
    }
}

//! Left sidebar: logo, main sections, local playlists.

use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, TextureHandle, Ui, pos2, vec2};
use mochy_core::models::Playlist;

use crate::actions::{Action, Ctx};
use crate::nav::{LibTab, Route};
use crate::ui::i18n::tr;
use crate::widgets::{self, TEXT, TEXT_DIM, icons, icons::Icon};

fn nav_item(ui: &mut Ui, icon: Icon, label: &str, selected: bool, accent: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    let fill = if selected {
        Color32::from_white_alpha(22)
    } else if resp.hovered() {
        Color32::from_white_alpha(10)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    let ir = Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(20.0, 20.0));
    icons::paint(ui.painter(), ir, icon, if selected { widgets::accent_light(accent) } else { TEXT_DIM });
    ui.painter().text(
        pos2(rect.left() + 44.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.5),
        if selected { TEXT } else { TEXT_DIM },
    );
    resp
}

pub fn show(ui: &mut Ui, cx: &mut Ctx, route: &Route, playlists: &[Playlist], logo: Option<&TextureHandle>) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if let Some(t) = logo {
            ui.add(egui::Image::from_texture(t).fit_to_exact_size(vec2(30.0, 30.0)).corner_radius(8));
        }
        ui.label(egui::RichText::new("Mochy").size(20.0).strong().color(TEXT));
    });
    ui.add_space(18.0);

    let sec = route.section();
    let items = [
        (Icon::Home, tr("Home"), Route::Home, 0u8),
        (Icon::Search, tr("Search"), Route::Search, 1),
        (Icon::Library, tr("Library"), Route::Library(LibTab::Liked), 2),
        (Icon::AudioLab, "Audio Lab", Route::AudioLab, 3),
        (Icon::Settings, tr("Settings"), Route::Settings, 4),
    ];
    for (icon, label, r, s) in items {
        let selected = sec == s && !matches!(route, Route::LocalPlaylist(_));
        if nav_item(ui, icon, label, selected, cx.accent).clicked() {
            cx.act(Action::Navigate(r));
        }
    }

    ui.add_space(14.0);
    ui.separator();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(tr("PLAYLISTS")).size(11.5).color(TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icons::icon_button(ui, Icon::Plus, 24.0, false, cx.accent).on_hover_text(tr("New playlist")).clicked() {
                cx.act(Action::NewPlaylist(None));
            }
        });
    });
    ui.add_space(4.0);
    egui::ScrollArea::vertical().id_salt("sidebar_playlists").auto_shrink([false, false]).show(ui, |ui| {
        if nav_item(ui, Icon::HeartFilled, tr("Liked songs"), matches!(route, Route::Library(LibTab::Liked)), cx.accent).clicked() {
            cx.act(Action::Navigate(Route::Library(LibTab::Liked)));
        }
        if nav_item(ui, Icon::Folder, tr("Local files"), matches!(route, Route::Library(LibTab::Local)), cx.accent).clicked() {
            cx.act(Action::Navigate(Route::Library(LibTab::Local)));
        }
        for p in playlists {
            let selected = matches!(route, Route::LocalPlaylist(id) if *id == p.id);
            let resp = nav_item(ui, Icon::Playlist, &p.name, selected, cx.accent);
            if resp.clicked() {
                cx.act(Action::Navigate(Route::LocalPlaylist(p.id.clone())));
            }
            resp.context_menu(|ui| {
                if ui.button(tr("Rename…")).clicked() {
                    cx.act(Action::RenamePlaylist(p.id.clone()));
                    ui.close();
                }
                if ui.button(tr("Delete…")).clicked() {
                    cx.act(Action::DeletePlaylist(p.id.clone()));
                    ui.close();
                }
            });
        }
    });
}

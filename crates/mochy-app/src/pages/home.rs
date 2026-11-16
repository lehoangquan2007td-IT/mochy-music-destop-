//! Home (spec §7.2): Recently played / Most played from the DB, then YTM
//! home shelves as horizontal card rows.

use egui::{RichText, Ui};

use super::Data;
use crate::actions::{Ctx, LoadState};
use crate::ui::i18n::tr;
use crate::widgets::{self, TEXT_DIM};

/// Returns `true` when the user asked to retry loading the home feed.
pub fn show(ui: &mut Ui, cx: &mut Ctx, data: &Data) -> bool {
    let mut retry = false;
    widgets::page_title(ui, greeting());

    widgets::song_card_row(ui, cx, tr("Recently played"), &data.history, "recent", true);
    widgets::song_card_row(ui, cx, tr("Most played"), &data.most_played, "most", true);

    match &data.home {
        None | Some(LoadState::Loading) => {
            ui.add_space(12.0);
            widgets::loading(ui, tr("Loading YouTube Music…"));
        }
        Some(LoadState::Failed(e)) => {
            ui.add_space(12.0);
            retry = widgets::error_box(ui, e);
        }
        Some(LoadState::Ready(shelves)) => {
            if shelves.is_empty() {
                ui.label(RichText::new(tr("YouTube Music returned no shelves.")).color(TEXT_DIM));
                retry = ui.button(tr("Reload")).clicked();
            }
            for (i, shelf) in shelves.iter().enumerate() {
                if shelf.items.is_empty() {
                    continue;
                }
                widgets::shelf_row(ui, cx, shelf, ("home", i));
            }
        }
    }
    ui.add_space(24.0);
    retry
}

fn greeting() -> &'static str {
    // Local time isn't available without a tz crate; use UTC-free wording.
    "Listen now"
}

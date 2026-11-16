//! UI language: English source strings with a Vietnamese table.
//!
//! `tr(key)` returns the translation for the active language; unknown keys
//! pass through unchanged, so a missing entry degrades to English instead of
//! breaking the UI. The language is stored in settings (`appearance.language`)
//! and pushed into the global here once per frame by `app.rs`, before drawing.
//!
//! Dynamic strings (counts, durations, tooltips with values) use the small
//! helpers below instead of `format!` chains at the call site.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::LazyLock;

use mochy_core::settings::Language;

static LANG: AtomicU8 = AtomicU8::new(0); // 0 = En, 1 = Vi

/// Push the active language for this frame (called by `app.rs` before drawing).
pub fn set_lang(l: Language) {
    LANG.store(u8::from(matches!(l, Language::Vi)), Ordering::Relaxed);
}

fn vi() -> bool {
    LANG.load(Ordering::Relaxed) == 1
}

/// Translate `key` (an English source string) into the active language.
pub fn tr(key: &'static str) -> &'static str {
    if !vi() {
        return key;
    }
    VI.get(key).copied().unwrap_or(key)
}

static VI: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        // ---- sidebar ---------------------------------------------------------
        ("Home", "Trang chủ"),
        ("Search", "Tìm kiếm"),
        ("Library", "Thư viện"),
        ("Settings", "Cài đặt"),
        ("PLAYLISTS", "DANH SÁCH PHÁT"),
        ("New playlist", "Danh sách phát mới"),
        ("Liked songs", "Bài hát đã thích"),
        ("Local files", "Tệp cục bộ"),
        ("Rename…", "Đổi tên…"),
        ("Delete…", "Xóa…"),
        // ---- nav tabs / side tabs --------------------------------------------
        ("Playlists", "Danh sách phát"),
        ("History", "Lịch sử"),
        ("Most played", "Nghe nhiều nhất"),
        ("Lyrics", "Lời bài hát"),
        ("Queue", "Hàng đợi"),
        ("Related", "Liên quan"),
        // ---- home ------------------------------------------------------------
        ("Listen now", "Nghe ngay"),
        ("Recently played", "Nghe gần đây"),
        ("Loading YouTube Music…", "Đang tải YouTube Music…"),
        ("YouTube Music returned no shelves.", "YouTube Music không trả về mục nào."),
        ("Reload", "Tải lại"),
        // ---- library ---------------------------------------------------------
        ("Refresh", "Làm mới"),
        ("No liked songs yet — tap ♥ on any song.", "Chưa có bài hát nào được thích — bấm ♥ vào bài hát bất kỳ."),
        ("No local files yet — add a music folder.", "Chưa có tệp cục bộ nào — thêm một thư mục nhạc."),
        ("Nothing played yet.", "Chưa nghe bài hát nào."),
        ("No playlists yet.", "Chưa có danh sách phát nào."),
        ("Open", "Mở"),
        ("Cancel scan", "Hủy quét"),
        ("Add folder", "Thêm thư mục"),
        ("Play", "Phát"),
        ("Shuffle", "Trộn bài"),
        // ---- search ----------------------------------------------------------
        ("All", "Tất cả"),
        ("Songs", "Bài hát"),
        ("Videos", "Video"),
        ("Albums", "Album"),
        ("Artists", "Nghệ sĩ"),
        ("Search songs, albums, artists, playlists", "Tìm bài hát, album, nghệ sĩ, danh sách phát"),
        ("Searching…", "Đang tìm…"),
        ("Recent searches", "Tìm kiếm gần đây"),
        ("No results", "Không có kết quả"),
        ("Show all songs", "Xem tất cả bài hát"),
        // ---- detail ----------------------------------------------------------
        ("Loading…", "Đang tải…"),
        ("Album", "Album"),
        ("Artist", "Nghệ sĩ"),
        ("Playlist", "Danh sách phát"),
        ("Your playlist", "Danh sách phát của bạn"),
        ("Radio", "Radio"),
        ("Add to queue", "Thêm vào hàng đợi"),
        ("Playlist not found", "Không tìm thấy danh sách phát"),
        ("Loading playlist…", "Đang tải danh sách phát…"),
        ("Rename", "Đổi tên"),
        ("Delete", "Xóa"),
        ("This playlist is empty. Add songs from their ⋯ menu.", "Danh sách phát trống. Thêm bài hát qua menu ⋯ của bài hát."),
        // ---- player bar / now playing -----------------------------------------
        ("Previous (Ctrl+←)", "Bài trước (Ctrl+←)"),
        ("Play / pause (Space)", "Phát / tạm dừng (Space)"),
        ("Next (Ctrl+→)", "Bài tiếp theo (Ctrl+→)"),
        ("Repeat: off", "Lặp lại: tắt"),
        ("Repeat: all", "Lặp lại: tất cả"),
        ("Repeat: one", "Lặp lại: một bài"),
        ("Open now playing", "Mở màn hình đang phát"),
        ("Unlike", "Bỏ thích"),
        ("Like", "Thích"),
        ("Nothing playing", "Không có gì đang phát"),
        ("Now playing", "Đang phát"),
        ("Close (Esc)", "Đóng (Esc)"),
        ("Lyrics are turned off in Settings → Lyrics.", "Lời bài hát đang tắt trong Cài đặt → Lời bài hát."),
        ("Offset", "Độ lệch"),
        ("global", "toàn cục"),
        ("Reset", "Đặt lại"),
        ("· shuffled", "· đã trộn"),
        ("Clear", "Xóa hết"),
        ("Clear the queue", "Xóa hết hàng đợi"),
        ("The queue is empty.", "Hàng đợi trống."),
        ("Remove", "Gỡ bỏ"),
        ("Play now", "Phát ngay"),
        ("Move up", "Chuyển lên"),
        ("Move down", "Chuyển xuống"),
        ("Play next", "Phát tiếp theo"),
        ("Remove from queue", "Gỡ khỏi hàng đợi"),
        ("Related songs are available for YouTube Music tracks.", "Bài hát liên quan chỉ có cho nhạc YouTube Music."),
        ("Loading related…", "Đang tải bài liên quan…"),
        ("No related songs.", "Không có bài hát liên quan."),
        // ---- song menu / widgets -----------------------------------------------
        ("Something went wrong", "Đã xảy ra lỗi"),
        ("Retry", "Thử lại"),
        ("Unknown artist", "Nghệ sĩ không rõ"),
        ("Start radio", "Bắt đầu radio"),
        ("Remove from liked songs", "Xóa khỏi bài hát đã thích"),
        ("Add to liked songs", "Thêm vào bài hát đã thích"),
        ("Add to playlist", "Thêm vào danh sách phát"),
        ("New playlist…", "Danh sách phát mới…"),
        ("Remove from this playlist", "Gỡ khỏi danh sách phát này"),
        ("Go to album", "Đến album"),
        ("Copy link", "Sao chép liên kết"),
        // ---- dialogs / toasts ---------------------------------------------------
        ("Rename playlist", "Đổi tên danh sách phát"),
        ("Delete playlist", "Xóa danh sách phát"),
        ("Playlist name", "Tên danh sách phát"),
        ("Create", "Tạo"),
        ("Cancel", "Hủy"),
        ("Link copied", "Đã sao chép liên kết"),
        ("Created playlist", "Đã tạo danh sách phát"),
        ("Deleted", "Đã xóa"),
        ("Playing next", "Phát tiếp theo"),
        ("Added to", "Đã thêm vào"),
        ("Added to queue", "Đã thêm vào hàng đợi"),
        ("Scan cancelled", "Đã hủy quét"),
        // ---- settings -------------------------------------------------------------
        ("Playback", "Phát nhạc"),
        ("Appearance", "Giao diện"),
        ("Storage", "Bộ nhớ"),
        ("About", "Giới thiệu"),
        ("Language", "Ngôn ngữ"),
        ("Audio quality", "Chất lượng âm thanh"),
        ("Auto", "Tự động"),
        ("Low", "Thấp"),
        ("Medium", "Trung bình"),
        ("High", "Cao"),
        ("Normalize volume (YouTube loudness)", "Chuẩn hóa âm lượng (loudness YouTube)"),
        ("Use ReplayGain tags for local files", "Dùng thẻ ReplayGain cho tệp cục bộ"),
        ("Skip to the next song when a stream fails", "Tự chuyển bài khi stream gặp lỗi"),
        ("Skip silence", "Bỏ qua đoạn im lặng"),
        ("Skip silence is saved but not yet applied by the desktop audio engine.", "Bỏ qua đoạn im lặng đã lưu nhưng engine âm thanh desktop chưa áp dụng."),
        ("Restore the queue on startup", "Khôi phục hàng đợi khi khởi động"),
        ("Output device", "Thiết bị xuất"),
        ("System default", "Mặc định hệ thống"),
        ("Loading devices…", "Đang tải thiết bị…"),
        ("No devices reported by the engine", "Engine không báo thiết bị nào"),
        ("Dynamic theme (accent from artwork)", "Giao diện động (màu nhấn theo ảnh bìa)"),
        ("Custom accent color", "Màu nhấn tùy chỉnh"),
        ("Turn off the dynamic theme to pick a fixed accent color.", "Tắt giao diện động để chọn màu nhấn cố định."),
        ("Pure black background", "Nền đen hoàn toàn"),
        ("Cover corner radius", "Bo góc ảnh bìa"),
        ("Show lyrics", "Hiện lời bài hát"),
        ("Text size", "Cỡ chữ"),
        ("Providers (top = highest priority)", "Nhà cung cấp (trên cùng = ưu tiên cao nhất)"),
        ("Lyric sync offset (all songs)", "Độ lệch lời bài hát (tất cả bài)"),
        ("Lyrics run ahead of the vocals? Press − to delay them (Bluetooth / TV setups).", "Lời chạy trước tiếng hát? Bấm − để trễ lại (hữu ích với tai nghe Bluetooth / TV)."),
        ("Data", "Dữ liệu"),
        ("Database", "Cơ sở dữ liệu"),
        ("Cache", "Bộ nhớ đệm"),
        ("Artwork cache", "Bộ nhớ đệm ảnh bìa"),
        ("Log file", "Tệp nhật ký"),
        ("Clear artwork cache", "Xóa bộ nhớ đệm ảnh bìa"),
        ("YouTube Music: connected", "YouTube Music: đã kết nối"),
        ("YouTube Music: connecting…", "YouTube Music: đang kết nối…"),
        ("A desktop port of the Mochy Music Android app, written in Rust (egui).", "Bản desktop của ứng dụng Mochy Music Android, viết bằng Rust (egui)."),
        ("Based on the original", "Dựa trên dự án gốc"),
        ("project (a Metrolist fork) — all credit for the design and features goes to its authors.", "dự án (fork của Metrolist) — thiết kế và tính năng thuộc về các tác giả gốc."),
        ("Not affiliated with Google or YouTube.", "Không liên kết với Google hay YouTube."),
        // ---- downloads / song edit ---------------------------------------------
        ("Downloaded", "Đã tải xuống"),
        ("Download", "Tải xuống"),
        ("Downloads", "Bản tải xuống"),
        ("Remove download", "Xóa bản tải xuống"),
        ("Cancel download", "Hủy tải xuống"),
        ("Download started", "Đã bắt đầu tải xuống"),
        ("Downloading {n} songs", "Đang tải {n} bài hát"),
        ("Downloading {done}/{total}", "Đang tải {done}/{total}"),
        ("Downloads finished: {ok} saved", "Tải xong: đã lưu {ok} bài"),
        (", {n} failed", ", {n} bài lỗi"),
        ("Click to cancel", "Bấm để hủy"),
        ("Waiting", "Đang chờ"),
        ("Downloading", "Đang tải"),
        ("Failed", "Lỗi"),
        ("Cancelled", "Đã hủy"),
        ("Finding stream…", "Đang tìm luồng…"),
        ("Saving…", "Đang lưu…"),
        ("In progress", "Đang tải"),
        ("Cancel all", "Hủy tất cả"),
        ("Retry failed", "Thử lại bài lỗi"),
        ("Open folder", "Mở thư mục"),
        ("No downloads yet — right-click a song and choose Download.", "Chưa có bản tải xuống — nhấp chuột phải vào bài hát và chọn Tải xuống."),
        ("File missing", "Không tìm thấy tệp"),
        ("The server sent an empty file", "Máy chủ trả về tệp rỗng"),
        ("Couldn't delete the file", "Không xóa được tệp"),
        ("Downloads & storage", "Tải xuống & bộ nhớ"),
        ("Download folder", "Thư mục tải xuống"),
        ("Browse…", "Chọn…"),
        ("File name", "Tên tệp"),
        ("Tokens:", "Biến:"),
        ("sub-folders with", "thư mục con bằng"),
        ("e.g.", "vd."),
        ("Download quality", "Chất lượng tải xuống"),
        ("Prefer Opus (.opus) over AAC (.m4a)", "Ưu tiên Opus (.opus) hơn AAC (.m4a)"),
        ("Downloads at once", "Số bài tải cùng lúc"),
        ("Embed cover art", "Nhúng ảnh bìa"),
        ("Embed lyrics", "Nhúng lời bài hát"),
        ("Save lyrics file (.lrc)", "Lưu tệp lời (.lrc)"),
        ("Play downloaded songs from disk (offline)", "Phát bài đã tải từ ổ đĩa (ngoại tuyến)"),
        ("Download songs as I like them", "Tự tải bài hát khi tôi thích"),
        ("Song cache", "Bộ nhớ đệm bài hát"),
        ("Clear cache", "Xóa bộ nhớ đệm"),
        ("Cache folder", "Thư mục đệm"),
        ("Off", "Tắt"),
        ("Unlimited", "Không giới hạn"),
        ("Keeps played songs so they play again without a connection. Each song is fetched a second time in the background.", "Giữ lại bài đã phát để nghe lại khi không có mạng. Mỗi bài được tải thêm một lần trong nền."),
        ("Edit song…", "Sửa bài hát…"),
        ("Edit song", "Sửa bài hát"),
        ("Title", "Tiêu đề"),
        ("Year", "Năm"),
        ("Separate with commas", "Ngăn cách bằng dấu phẩy"),
        ("Cover", "Ảnh bìa"),
        ("Use link", "Dùng liên kết"),
        ("Choose picture…", "Chọn ảnh…"),
        ("Choose a cover picture", "Chọn ảnh bìa"),
        ("Pictures", "Hình ảnh"),
        ("Enter a link starting with https://", "Nhập liên kết bắt đầu bằng https://"),
        ("Not a picture", "Không phải hình ảnh"),
        ("Save", "Lưu"),
        ("Song updated", "Đã cập nhật bài hát"),
        ("Import lyrics…", "Nhập lời bài hát…"),
        ("Import lyrics file…", "Nhập tệp lời…"),
        ("Import lyrics", "Nhập lời bài hát"),
        ("Lyrics imported", "Đã nhập lời bài hát"),
        ("No lyrics found in that file", "Không tìm thấy lời trong tệp đó"),
        ("All files", "Tất cả tệp"),
        ("{n} lines", "{n} dòng"),
        ("synced", "đồng bộ"),
        ("plain", "không đồng bộ"),
        (".lrc, .ttml, .srt, .txt …", ".lrc, .ttml, .srt, .txt …"),
    ])
});

// ---------------------------------------------------------------------------
// Dynamic strings (values embedded)
// ---------------------------------------------------------------------------

/// "{n} songs"
pub fn songs_count(n: usize) -> String {
    if vi() { format!("{n} bài hát") } else { format!("{n} songs") }
}

/// "{m} min" / "{h} h {m} min" (mirrors `detail::total_duration`).
pub fn duration_label(total_min: u64) -> String {
    if total_min == 0 {
        return String::new();
    }
    if vi() {
        if total_min >= 60 { format!("{} giờ {} phút", total_min / 60, total_min % 60) } else { format!("{total_min} phút") }
    } else if total_min >= 60 {
        format!("{} h {} min", total_min / 60, total_min % 60)
    } else {
        format!("{total_min} min")
    }
}

/// "Volume {p}% (Ctrl+↑/↓)"
pub fn volume_tip(pct: i64) -> String {
    if vi() { format!("Âm lượng {pct}% (Ctrl+↑/↓)") } else { format!("Volume {pct}% (Ctrl+↑/↓)") }
}

/// "Go to artist: {name}"
pub fn goto_artist(name: &str) -> String {
    if vi() { format!("Đến nghệ sĩ: {name}") } else { format!("Go to artist: {name}") }
}

/// "Added {n} songs to queue"
pub fn added_to_queue(n: usize) -> String {
    if vi() { format!("Đã thêm {n} bài hát vào hàng đợi") } else { format!("Added {n} songs to queue") }
}

/// "Scanning {dir}…"
pub fn scan_dir(dir: &str) -> String {
    if vi() { format!("Đang quét {dir}…") } else { format!("Scanning {dir}…") }
}

/// "Scanned {scanned} files, {added} added…"
pub fn scan_progress(scanned: usize, added: usize) -> String {
    if vi() { format!("Đã quét {scanned} tệp, thêm {added} bài…") } else { format!("Scanned {scanned} files, {added} added…") }
}

/// "Added {n} songs" (end of a folder scan)
pub fn scan_added(n: usize) -> String {
    if vi() { format!("Đã thêm {n} bài hát") } else { format!("Added {n} songs") }
}

/// `Delete "{name}"? This can't be undone.`
pub fn delete_confirm(name: &str) -> String {
    if vi() { format!("Xóa \"{name}\"? Không thể hoàn tác.") } else { format!("Delete \"{name}\"? This can't be undone.") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sequential (single) test: `tr` reads a process-global language, so
    /// parallel test threads would race on `set_lang`.
    #[test]
    fn language_selection_and_helpers() {
        set_lang(Language::En);
        assert_eq!(tr("Home"), "Home");
        assert_eq!(tr("not a real key"), "not a real key");
        assert_eq!(songs_count(3), "3 songs");

        set_lang(Language::Vi);
        assert_eq!(tr("Home"), "Trang chủ");
        assert_eq!(tr("Settings"), "Cài đặt");
        assert_eq!(songs_count(3), "3 bài hát");
        // Unknown keys fall back to English.
        assert_eq!(tr("Definitely not in the table"), "Definitely not in the table");
        assert_eq!(volume_tip(50), "Âm lượng 50% (Ctrl+↑/↓)");
        assert_eq!(duration_label(75), "1 giờ 15 phút");
        assert_eq!(duration_label(9), "9 phút");
        assert_eq!(delete_confirm("Mix"), "Xóa \"Mix\"? Không thể hoàn tác.");

        set_lang(Language::En);
        assert_eq!(volume_tip(50), "Volume 50% (Ctrl+↑/↓)");
        assert_eq!(duration_label(75), "1 h 15 min");
    }
}

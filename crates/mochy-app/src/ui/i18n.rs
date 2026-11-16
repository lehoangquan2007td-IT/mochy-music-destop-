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
        // ---- library/explore/stats ----
        ("Explore", "Khám phá"),
        ("Stats", "Thống kê"),
        ("Recently added", "Mới thêm gần đây"),
        ("Good morning", "Chào buổi sáng"),
        ("Good afternoon", "Chào buổi chiều"),
        ("Good evening", "Chào buổi tối"),
        ("Good night", "Chúc ngủ ngon"),
        ("Quick picks", "Gợi ý nhanh"),
        ("Forgotten favorites", "Bài yêu thích bị lãng quên"),
        ("See all", "Xem tất cả"),
        ("New releases", "Mới phát hành"),
        ("New release albums", "Album mới phát hành"),
        ("Charts", "Bảng xếp hạng"),
        ("Moods & genres", "Tâm trạng & thể loại"),
        ("Loading charts…", "Đang tải bảng xếp hạng…"),
        ("Nothing here yet.", "Chưa có gì ở đây."),
        ("This shelf has no full list.", "Mục này không có danh sách đầy đủ."),
        ("More", "Thêm"),
        ("Save to library", "Lưu vào thư viện"),
        ("In library", "Đã lưu"),
        ("Saved to library", "Đã lưu vào thư viện"),
        ("Add to library", "Thêm vào thư viện"),
        ("Added to library", "Đã thêm vào thư viện"),
        ("Remove from library", "Xóa khỏi thư viện"),
        ("Removed from library", "Đã xóa khỏi thư viện"),
        ("Remove from history", "Xóa khỏi lịch sử"),
        ("Remove all plays from history", "Xóa mọi lượt nghe khỏi lịch sử"),
        ("Removed from history", "Đã xóa khỏi lịch sử"),
        ("Clear history", "Xóa lịch sử"),
        ("Clear history…", "Xóa lịch sử…"),
        ("Remove every play from the history?", "Xóa toàn bộ lượt nghe khỏi lịch sử?"),
        ("History cleared", "Đã xóa lịch sử"),
        ("Nothing to play", "Không có gì để phát"),
        ("Couldn't load songs", "Không tải được bài hát"),
        ("Open on YouTube Music", "Mở trên YouTube Music"),
        ("Search in library", "Tìm trong thư viện"),
        ("Sort", "Sắp xếp"),
        ("Reverse order", "Đảo thứ tự"),
        ("Date added", "Ngày thêm"),
        ("Name", "Tên"),
        ("Play time", "Thời gian nghe"),
        ("Song count", "Số bài hát"),
        ("Saved only", "Chỉ mục đã lưu"),
        ("Grid", "Lưới"),
        ("List", "Danh sách"),
        ("Liked", "Đã thích"),
        ("Song", "Bài hát"),
        ("Your library is empty — add songs from their ⋯ menu.", "Thư viện trống — thêm bài hát từ menu ⋯ của chúng."),
        ("No results in your library.", "Không có kết quả trong thư viện."),
        ("No albums yet — save one from its page or ⋯ menu.", "Chưa có album nào — lưu album từ trang của nó hoặc menu ⋯."),
        ("No artists yet.", "Chưa có nghệ sĩ nào."),
        ("Nothing added yet.", "Chưa thêm gì."),
        ("No songs from this artist in your library.", "Thư viện chưa có bài nào của nghệ sĩ này."),
        ("No songs from this album in your library.", "Thư viện chưa có bài nào của album này."),
        ("Today", "Hôm nay"),
        ("Yesterday", "Hôm qua"),
        ("This week", "Tuần này"),
        ("Last week", "Tuần trước"),
        ("January", "Tháng 1"),
        ("February", "Tháng 2"),
        ("March", "Tháng 3"),
        ("April", "Tháng 4"),
        ("May", "Tháng 5"),
        ("June", "Tháng 6"),
        ("July", "Tháng 7"),
        ("August", "Tháng 8"),
        ("September", "Tháng 9"),
        ("October", "Tháng 10"),
        ("November", "Tháng 11"),
        ("December", "Tháng 12"),
        ("Monday", "Thứ Hai"),
        ("Tuesday", "Thứ Ba"),
        ("Wednesday", "Thứ Tư"),
        ("Thursday", "Thứ Năm"),
        ("Friday", "Thứ Sáu"),
        ("Saturday", "Thứ Bảy"),
        ("Sunday", "Chủ Nhật"),
        ("Mon", "T2"),
        ("Tue", "T3"),
        ("Wed", "T4"),
        ("Thu", "T5"),
        ("Fri", "T6"),
        ("Sat", "T7"),
        ("Sun", "CN"),
        ("Week", "Tuần"),
        ("Month", "Tháng"),
        ("Year", "Năm"),
        ("All time", "Mọi lúc"),
        ("Previous", "Trước"),
        ("Next", "Tiếp"),
        ("Recap", "Tổng kết"),
        ("Your listening story", "Câu chuyện nghe nhạc của bạn"),
        ("Nothing played in {period} yet.", "Chưa nghe bài nào trong {period}."),
        ("Play a few songs and this fills itself in.", "Nghe vài bài và trang này sẽ tự điền đầy."),
        ("Total listening time", "Tổng thời gian nghe"),
        ("plays", "lượt nghe"),
        ("songs", "bài hát"),
        ("artists", "nghệ sĩ"),
        ("albums", "album"),
        ("Listening by day", "Nghe theo ngày"),
        ("Listening by month", "Nghe theo tháng"),
        ("By hour of day", "Theo giờ trong ngày"),
        ("By weekday", "Theo thứ trong tuần"),
        ("in the mornings", "vào buổi sáng"),
        ("in the afternoons", "vào buổi chiều"),
        ("in the evenings", "vào buổi tối"),
        ("at night", "vào ban đêm"),
        ("You love to listen {when}, around {hour}", "Bạn thích nghe nhạc {when}, khoảng {hour}"),
        ("Your favourite day is {day}, {hours} hours", "Ngày bạn thích nhất là {day}, {hours} giờ"),
        ("Top songs", "Bài hát hàng đầu"),
        ("Top artists", "Nghệ sĩ hàng đầu"),
        ("Top albums", "Album hàng đầu"),
        ("{n} plays", "{n} lượt nghe"),
        ("{n} plays!", "{n} lượt nghe!"),
        ("{n} plays · {h} hours", "{n} lượt nghe · {h} giờ"),
        ("Everything here comes from what you played in this app and never leaves your computer.", "Mọi số liệu ở đây đến từ những gì bạn nghe trong ứng dụng và không bao giờ rời khỏi máy tính của bạn."),
        ("Listening with Mochy since {date}", "Nghe nhạc cùng Mochy từ {date}"),
        ("{period} recap", "Tổng kết {period}"),
        ("you have been listening to music for", "bạn đã nghe nhạc tổng cộng"),
        ("{plays} plays · {songs} songs · {artists} artists", "{plays} lượt nghe · {songs} bài hát · {artists} nghệ sĩ"),
        ("Your favourite song", "Bài hát bạn yêu thích nhất"),
        ("{n} more than your #2", "nhiều hơn bài #2 tới {n} lượt"),
        ("neck and neck with your #2", "ngang ngửa với bài #2"),
        ("You really love listening to this artist", "Bạn thật sự rất thích nghe nghệ sĩ này"),
        ("Your top 5", "Top 5 của bạn"),
        ("And you love to listen {when}, exactly around", "Và bạn thích nghe nhạc {when}, đúng vào khoảng"),
        ("totaling {h} hours at this time!", "tổng cộng {h} giờ vào khung giờ này!"),
        ("is your favourite day to listen to music", "là ngày bạn thích nghe nhạc nhất"),
        ("totaling {h} hours", "tổng cộng {h} giờ"),
        ("Let's see your bill", "Cùng xem hóa đơn của bạn"),
        ("TOP ARTIST", "NGHỆ SĨ HÀNG ĐẦU"),
        ("TOP MUSIC", "BÀI HÁT HÀNG ĐẦU"),
        ("TOTAL: {t} playtime", "TỔNG: {t} thời gian nghe"),
        ("END OF LINE", "HẾT"),
        ("There you go", "Thế đấy!"),
        ("Click to continue", "Bấm để tiếp tục"),
        ("Close", "Đóng"),
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

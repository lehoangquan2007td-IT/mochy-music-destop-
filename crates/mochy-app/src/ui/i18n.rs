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
        // ---- lyrics extras ----
        ("Translating…", "Đang dịch…"),
        ("Translation failed", "Dịch thất bại"),
        ("Hide translation", "Ẩn bản dịch"),
        ("Translate lyrics", "Dịch lời bài hát"),
        ("Hide romanization", "Ẩn phiên âm"),
        ("Show romanization", "Hiện phiên âm"),
        ("(check the connection)", "(kiểm tra kết nối mạng)"),
        ("Lyrics display", "Hiển thị lời bài hát"),
        ("Alignment", "Căn lề"),
        ("Left", "Trái"),
        ("Center", "Giữa"),
        ("Right", "Phải"),
        ("Font weight", "Độ đậm chữ"),
        ("Regular", "Thường"),
        ("Medium", "Vừa"),
        ("Semibold", "Hơi đậm"),
        ("Bold", "Đậm"),
        ("Line spacing", "Giãn dòng"),
        ("Dim inactive lines", "Làm mờ các dòng không hát"),
        ("Blur lines away from the current one", "Nhòe các dòng ở xa dòng đang hát"),
        ("Click a line to seek to it", "Bấm vào một dòng để tua tới đó"),
        ("Translation / romanization size", "Cỡ chữ bản dịch / phiên âm"),
        ("Romanization", "Phiên âm"),
        ("Show romanization under lyrics", "Hiện phiên âm dưới lời bài hát"),
        ("Japanese (romaji), Korean, Chinese (pinyin), Russian and other Cyrillic languages, Hindi and Punjabi.", "Tiếng Nhật (romaji), tiếng Hàn, tiếng Trung (pinyin), tiếng Nga và các tiếng dùng chữ Kirin khác, tiếng Hindi và Punjab."),
        ("Show the romanization as the line, original underneath", "Hiện phiên âm làm dòng chính, lời gốc ở dưới"),
        ("Translation", "Bản dịch"),
        ("Translate into", "Dịch sang"),
        ("Lyrics language", "Ngôn ngữ của lời"),
        ("Detect automatically", "Tự nhận diện"),
        ("Translate every song automatically", "Tự động dịch mọi bài hát"),
        ("Uses Google Translate (no account needed). Press 文A above the lyrics to translate one song. Translations are cached.", "Dùng Google Dịch (không cần tài khoản). Bấm 文A cạnh lời bài hát để dịch một bài. Bản dịch được lưu lại."),
        ("English", "Tiếng Anh"),
        ("Vietnamese", "Tiếng Việt"),
        ("Japanese", "Tiếng Nhật"),
        ("Korean", "Tiếng Hàn"),
        ("Chinese", "Tiếng Trung"),
        ("Chinese (Simplified)", "Tiếng Trung (Giản thể)"),
        ("Chinese (Traditional)", "Tiếng Trung (Phồn thể)"),
        ("Thai", "Tiếng Thái"),
        ("Indonesian", "Tiếng Indonesia"),
        ("Malay", "Tiếng Mã Lai"),
        ("Filipino", "Tiếng Philippines"),
        ("Hindi", "Tiếng Hindi"),
        ("Punjabi", "Tiếng Punjab"),
        ("Arabic", "Tiếng Ả Rập"),
        ("Turkish", "Tiếng Thổ Nhĩ Kỳ"),
        ("Russian", "Tiếng Nga"),
        ("Ukrainian", "Tiếng Ukraina"),
        ("Serbian", "Tiếng Serbia"),
        ("Bulgarian", "Tiếng Bulgaria"),
        ("Belarusian", "Tiếng Belarus"),
        ("Kyrgyz", "Tiếng Kyrgyz"),
        ("Macedonian", "Tiếng Macedonia"),
        ("Polish", "Tiếng Ba Lan"),
        ("German", "Tiếng Đức"),
        ("French", "Tiếng Pháp"),
        ("Spanish", "Tiếng Tây Ban Nha"),
        ("Portuguese", "Tiếng Bồ Đào Nha"),
        ("Italian", "Tiếng Ý"),
        ("Dutch", "Tiếng Hà Lan"),
        ("Swedish", "Tiếng Thụy Điển"),
        // ---- audio lab 2 / player styles ----
        // hub
        ("Audio", "Audio"),
        ("Lab", "Lab"),
        ("Shape how everything sounds", "Tùy chỉnh mọi âm thanh theo ý bạn"),
        ("OFF", "TẮT"),
        ("ON", "BẬT"),
        ("Custom", "Tùy chỉnh"),
        ("Equalizer", "Bộ cân bằng"),
        ("Audio FX", "Hiệu ứng âm thanh"),
        ("NO CORRECTION", "CHƯA HIỆU CHỈNH"),
        ("Headphones", "Tai nghe"),
        ("YouTube · {q}", "YouTube · {q}"),
        // equalizer
        ("Parametric, up to 20 bands", "Tham số, tối đa 20 dải"),
        ("Presets", "Cài sẵn"),
        ("Graph", "Đồ thị"),
        ("10-band", "10 dải"),
        ("Reset to flat", "Đặt về phẳng"),
        ("Preamp", "Tiền khuếch đại"),
        ("Auto preamp", "Tiền khuếch đại tự động"),
        ("Headroom from the curve: {a} dB · total preamp {t} dB", "Khoảng dự trữ theo đường cong: {a} dB · tổng tiền khuếch đại {t} dB"),
        ("⚠ Peaks reach {p} dB — this can clip. Turn on Auto preamp or lower the preamp by {l} dB.", "⚠ Đỉnh đạt {p} dB — có thể bị vỡ tiếng. Bật Tiền khuếch đại tự động hoặc giảm tiền khuếch đại {l} dB."),
        ("Headphone correction: {name}", "Hiệu chỉnh tai nghe: {name}"),
        ("{n} filters from AutoEq, under your curve (dashed pink line)", "{n} bộ lọc từ AutoEq, chạy dưới đường cong của bạn (nét đứt màu hồng)"),
        ("Change…", "Đổi…"),
        ("Your presets", "Cài sẵn của bạn"),
        ("None yet — save the current curve below.", "Chưa có — lưu đường cong hiện tại ở dưới."),
        ("Preset name", "Tên cài sẵn"),
        ("Save as preset", "Lưu thành cài sẵn"),
        ("OK", "OK"),
        ("Band {n}", "Dải {n}"),
        ("Enabled", "Bật"),
        ("Reset gain", "Đặt lại độ lợi"),
        ("Delete band", "Xóa dải"),
        ("Add band here", "Thêm dải tại đây"),
        ("Flatten all gains", "Làm phẳng tất cả"),
        ("Double-click to add a band", "Nhấp đúp để thêm dải"),
        ("+ Add band", "+ Thêm dải"),
        ("Drag a dot to move it · scroll for Q · double-click to add · right-click for options", "Kéo chấm để di chuyển · cuộn để chỉnh Q · nhấp đúp để thêm · chuột phải để xem tùy chọn"),
        ("Frequency", "Tần số"),
        ("Gain", "Độ lợi"),
        ("This EQ uses custom bands. The classic view needs the 10 standard bands (31 Hz – 16 kHz).", "EQ này dùng dải tùy chỉnh. Chế độ cổ điển cần 10 dải chuẩn (31 Hz – 16 kHz)."),
        ("Convert to 10 bands", "Chuyển thành 10 dải"),
        ("Fits the current curve onto 10 peaking bands", "Khớp đường cong hiện tại vào 10 dải đỉnh"),
        ("Double-click a fader to reset it", "Nhấp đúp một thanh trượt để đặt lại"),
        ("AutoEQ file", "Tệp AutoEQ"),
        ("Import or export ParametricEQ.txt", "Nhập hoặc xuất ParametricEQ.txt"),
        ("Import pasted", "Nhập nội dung đã dán"),
        ("Open file…", "Mở tệp…"),
        ("Copy current EQ", "Sao chép EQ hiện tại"),
        ("Copied to clipboard", "Đã sao chép vào bộ nhớ tạm"),
        ("Save as file…", "Lưu thành tệp…"),
        // EQ presets
        ("Flat", "Phẳng"),
        ("Acoustic", "Acoustic"),
        ("Bass Booster", "Tăng bass"),
        ("Bass Reducer", "Giảm bass"),
        ("Classical", "Cổ điển"),
        ("Dance", "Dance"),
        ("Deep", "Trầm sâu"),
        ("Electronic", "Điện tử"),
        ("Hip-Hop", "Hip-Hop"),
        ("Jazz", "Jazz"),
        ("Lounge", "Lounge"),
        ("Piano", "Piano"),
        ("Pop", "Pop"),
        ("R&B", "R&B"),
        ("Rock", "Rock"),
        ("Small Speakers", "Loa nhỏ"),
        ("Spoken Word", "Giọng nói"),
        ("Treble Booster", "Tăng treble"),
        ("Treble Reducer", "Giảm treble"),
        ("Vocal Booster", "Tăng giọng hát"),
        // headphones (AutoEq database)
        ("Couldn't reach the AutoEq database", "Không kết nối được cơ sở dữ liệu AutoEq"),
        ("Couldn't load {name}. Check your connection.", "Không tải được {name}. Hãy kiểm tra kết nối mạng."),
        ("Headphone correction", "Hiệu chỉnh tai nghe"),
        ("FREQUENCY RESPONSE", "ĐÁP ỨNG TẦN SỐ"),
        ("Corrected", "Đã hiệu chỉnh"),
        ("Target", "Mục tiêu"),
        ("Original", "Gốc"),
        ("Pick your headphones below to see their curve.", "Chọn tai nghe của bạn ở dưới để xem đường cong."),
        ("Loading {name}…", "Đang tải {name}…"),
        ("by {source}", "đo bởi {source}"),
        ("{n} filters", "{n} bộ lọc"),
        ("Apply correction", "Áp dụng hiệu chỉnh"),
        ("Plays under your own EQ curve", "Chạy dưới đường cong EQ của bạn"),
        ("Filter bands", "Số dải lọc"),
        ("{n} bands", "{n} dải"),
        ("Correct up to", "Hiệu chỉnh đến"),
        ("Bass", "Bass"),
        ("Tilt", "Độ nghiêng"),
        ("Tuning needs the measurement — it is loading.", "Cần dữ liệu đo để tinh chỉnh — đang tải."),
        ("Use AutoEq's fit", "Dùng bản khớp của AutoEq"),
        ("AutoEq database", "Cơ sở dữ liệu AutoEq"),
        ("Measured headphones from the AutoEq project — pick yours", "Tai nghe đã đo từ dự án AutoEq — chọn tai nghe của bạn"),
        ("{n} models", "{n} mẫu"),
        ("Search headphones, e.g. HD 650", "Tìm tai nghe, ví dụ HD 650"),
        ("Try again", "Thử lại"),
        ("No headphones match \"{q}\"", "Không có tai nghe nào khớp \"{q}\""),
        ("{n} profiles", "{n} hồ sơ đo"),
        // audio fx
        ("ALL OFF", "TẮT HẾT"),
        ("{a} and {n} more", "{a} và {n} hiệu ứng khác"),
        ("Nothing on — every effect is off", "Chưa bật gì — mọi hiệu ứng đang tắt"),
        ("Reverb", "Vang"),
        ("Put the music in a room", "Đặt âm nhạc vào một căn phòng"),
        ("Room", "Phòng"),
        ("Studio", "Phòng thu"),
        ("Bathroom", "Phòng tắm"),
        ("Plate", "Plate"),
        ("Hall", "Hội trường"),
        ("Cathedral", "Nhà thờ lớn"),
        ("Stadium", "Sân vận động"),
        ("Dreamy", "Mơ màng"),
        ("Mix", "Trộn"),
        ("Room size", "Kích thước phòng"),
        ("Damping", "Độ tắt dần"),
        ("Wet", "Tín hiệu vang"),
        ("Fine tune", "Tinh chỉnh"),
        ("Dry", "Tín hiệu gốc"),
        ("Width", "Độ rộng"),
        ("Pre-delay", "Trễ trước"),
        ("Low cut", "Cắt trầm"),
        ("Loudness", "Độ lớn"),
        ("Louder, with the peaks held down", "To hơn, đỉnh vẫn được giữ"),
        ("Gentle", "Nhẹ"),
        ("Punchy", "Mạnh"),
        ("Night", "Ban đêm"),
        ("Max", "Tối đa"),
        ("Boost", "Tăng"),
        ("Compression", "Nén"),
        ("More compression brings quiet parts up and keeps loud parts from jumping out.", "Nén nhiều hơn sẽ nâng đoạn nhỏ và giữ đoạn lớn không bị vọt lên."),
        ("Even out songs", "Cân bằng âm lượng giữa các bài"),
        ("ReplayGain tags of local files, so every song plays at the same level", "Dùng thẻ ReplayGain của tệp cục bộ để mọi bài phát cùng mức"),
        ("ReplayGain aims at −18 LUFS; the preamp lifts everything back up. Applies from the next song.", "ReplayGain nhắm −18 LUFS; tiền khuếch đại nâng lại tất cả. Áp dụng từ bài tiếp theo."),
        ("YouTube loudness", "Độ lớn YouTube"),
        ("Use YouTube's loudness value for streamed songs", "Dùng giá trị độ lớn của YouTube cho bài phát trực tuyến"),
        ("Spatial", "Không gian"),
        ("Music in front of you instead of inside your head", "Âm nhạc ở phía trước thay vì trong đầu bạn"),
        ("Crossfeed", "Trộn chéo"),
        ("Speakers", "Loa"),
        ("Light", "Nhẹ"),
        ("Natural", "Tự nhiên"),
        ("Strong", "Mạnh"),
        ("Head shadow", "Bóng đầu"),
        ("Amount", "Mức độ"),
        ("Speaker angle", "Góc loa"),
        ("Two speakers in front of you, each heard by both ears — the far one a little later and darker.", "Hai loa trước mặt, mỗi tai đều nghe cả hai — loa xa đến muộn và tối hơn một chút."),
        ("Stereo width", "Độ rộng stereo"),
        ("Mono", "Mono"),
        ("Mono audio", "Âm thanh mono"),
        ("Both ears hear the whole mix", "Hai tai nghe toàn bộ bản phối"),
        ("Speed & pitch", "Tốc độ & cao độ"),
        ("Pitch stays put", "Giữ nguyên cao độ"),
        ("Pitch follows the speed, like a record", "Cao độ theo tốc độ, như đĩa than"),
        ("Normal", "Bình thường"),
        ("Slowed", "Chậm"),
        ("Slowed + reverb", "Chậm + vang"),
        ("Nightcore", "Nightcore"),
        ("Speed", "Tốc độ"),
        ("Keep pitch", "Giữ cao độ"),
        ("Change the speed without changing the pitch", "Đổi tốc độ mà không đổi cao độ"),
        ("Pitch", "Cao độ"),
        ("Volume curve", "Đường cong âm lượng"),
        ("Finer steps at low volume", "Bước chỉnh mịn hơn ở âm lượng nhỏ"),
        ("Exponential", "Hàm mũ"),
        ("Steep", "Dốc"),
        ("Curve", "Đường cong"),
        ("Volume limit", "Giới hạn âm lượng"),
        ("The slider's quiet half gets most of the room, so small changes stay small. The limit caps the loudest setting.", "Nửa nhỏ của thanh trượt chiếm nhiều chỗ hơn nên thay đổi nhỏ vẫn nhỏ. Giới hạn chặn mức to nhất."),
        // quality / stream info
        ("Lossless", "Lossless"),
        ("Hi-Res Lossless", "Lossless độ phân giải cao"),
        ("Dolby Atmos", "Dolby Atmos"),
        ("Streaming quality", "Chất lượng phát trực tuyến"),
        ("What YouTube Music hands over", "Những gì YouTube Music gửi về"),
        ("The best stream the client offers — usually Opus ~160 kbps.", "Luồng tốt nhất mà client cung cấp — thường là Opus ~160 kbps."),
        ("Saves data. HE-AAC: the desktop decoder plays only its core, so it sounds muffled.", "Tiết kiệm dữ liệu. HE-AAC: bộ giải mã desktop chỉ phát phần lõi nên nghe bị đục."),
        ("AAC ~128 kbps.", "AAC ~128 kbps."),
        ("The highest bitrate available for the song.", "Bitrate cao nhất có cho bài hát."),
        ("Applies from the next song.", "Áp dụng từ bài tiếp theo."),
        ("Format & stats for nerds", "Định dạng & thông số chi tiết"),
        ("Play a song to see its format, decoder, DSP chain and output.", "Phát một bài để xem định dạng, bộ giải mã, chuỗi DSP và đầu ra."),
        ("Exclusive output", "Đầu ra độc quyền"),
        ("Bit-perfect playback straight to a DAC", "Phát bit-perfect thẳng tới DAC"),
        ("The desktop player shares the device with the system mixer (no exclusive mode yet). For the cleanest path, set the device's format in the system sound settings to the song's sample rate.", "Trình phát desktop dùng chung thiết bị với bộ trộn của hệ thống (chưa có chế độ độc quyền). Để tín hiệu sạch nhất, hãy đặt định dạng thiết bị trong cài đặt âm thanh hệ thống bằng tần số lấy mẫu của bài hát."),
        ("Stereo", "Stereo"),
        ("{n} channels", "{n} kênh"),
        ("Bit depth", "Độ sâu bit"),
        ("Sample rate", "Tần số lấy mẫu"),
        ("Bitrate", "Bitrate"),
        ("{v} (average)", "{v} (trung bình)"),
        ("Channels", "Kênh"),
        ("Container", "Định dạng chứa"),
        ("Format (itag)", "Định dạng (itag)"),
        ("Codec string", "Chuỗi codec"),
        ("Client", "Client"),
        ("File size", "Kích thước tệp"),
        ("Source", "Nguồn"),
        ("Local file", "Tệp cục bộ"),
        ("Media ID", "Mã media"),
        ("Volume", "Âm lượng"),
        ("Format", "Định dạng"),
        ("Decoder", "Bộ giải mã"),
        ("Runs on", "Chạy trên"),
        ("Software (CPU)", "Phần mềm (CPU)"),
        ("Codec", "Codec"),
        ("Output", "Đầu ra"),
        ("32-bit float PCM", "PCM số thực 32-bit"),
        ("Output rate", "Tần số đầu ra"),
        ("Time-stretch (WSOLA) + sinc resampler", "Co giãn thời gian (WSOLA) + bộ lấy mẫu lại sinc"),
        ("Mochy sinc resampler (Kaiser)", "Bộ lấy mẫu lại sinc của Mochy (Kaiser)"),
        ("None — rates match", "Không — tần số đã khớp"),
        ("Type", "Loại"),
        ("In → out", "Vào → ra"),
        ("{v} in and out", "{v} cả vào và ra"),
        ("Cut-off", "Tần số cắt"),
        ("Quality", "Chất lượng"),
        ("Untouched", "Giữ nguyên"),
        ("{v} semitones", "{v} nửa cung"),
        ("Follows speed", "Theo tốc độ"),
        ("Resampler", "Bộ lấy mẫu lại"),
        ("EQ ({n} bands)", "EQ ({n} dải)"),
        ("Peak limiter", "Bộ giới hạn đỉnh"),
        ("Virtual speakers", "Loa ảo"),
        ("Processing", "Xử lý"),
        ("32-bit float", "Số thực 32-bit"),
        ("Effects", "Hiệu ứng"),
        ("Normalization", "Chuẩn hóa"),
        ("ReplayGain", "ReplayGain"),
        ("DSP", "DSP"),
        ("Device", "Thiết bị"),
        ("Audio API", "API âm thanh"),
        ("Device format", "Định dạng thiết bị"),
        ("Mode", "Chế độ"),
        ("Shared (system mixer)", "Dùng chung (bộ trộn hệ thống)"),
        ("Stream info", "Thông tin luồng"),
        ("Open Audio Lab", "Mở Audio Lab"),
        // player styles
        ("Now Playing", "Đang phát"),
        ("Scrubbing", "Đang tua"),
        ("{weekday}, {day} {month}", "{weekday}, {day} {month}"),
        ("Sunday", "Chủ nhật"),
        ("Monday", "Thứ hai"),
        ("Tuesday", "Thứ ba"),
        ("Wednesday", "Thứ tư"),
        ("Thursday", "Thứ năm"),
        ("Friday", "Thứ sáu"),
        ("Saturday", "Thứ bảy"),
        ("January", "tháng 1"),
        ("February", "tháng 2"),
        ("March", "tháng 3"),
        ("April", "tháng 4"),
        ("May", "tháng 5"),
        ("June", "tháng 6"),
        ("July", "tháng 7"),
        ("August", "tháng 8"),
        ("September", "tháng 9"),
        ("October", "tháng 10"),
        ("November", "tháng 11"),
        ("December", "tháng 12"),
        ("Flagship", "Flagship"),
        ("Turntable", "Mâm đĩa than"),
        ("Click wheel", "Bánh xe bấm"),
        ("Bedside clock", "Đồng hồ đầu giường"),
        ("Big cover, lyrics and queue beside it", "Ảnh bìa lớn, lời bài hát và hàng đợi bên cạnh"),
        ("A record player; its arm is the seek bar", "Máy hát đĩa than; cần đọc chính là thanh tua"),
        ("A pocket player — turn the wheel for volume", "Máy nghe nhạc bỏ túi — xoay bánh xe để chỉnh âm lượng"),
        ("A quiet clock for the nightstand", "Đồng hồ yên tĩnh cho đầu giường"),
        ("Player style", "Kiểu trình phát"),
        ("Background", "Nền"),
        ("Cover gradient", "Chuyển màu theo ảnh bìa"),
        ("Blurred cover", "Ảnh bìa làm mờ"),
        ("Ambient", "Ambient"),
        ("Dark gradient", "Chuyển màu tối"),
        ("Solid", "Màu đơn"),
        ("Cover", "Ảnh bìa"),
        ("Rounded", "Bo góc"),
        ("Square", "Vuông"),
        ("Disc", "Đĩa"),
        ("Record label", "Nhãn đĩa than"),
        ("Artwork size", "Kích thước ảnh bìa"),
        ("Artwork corner radius", "Bo góc ảnh bìa"),
        ("Records turn while playing", "Đĩa quay khi đang phát"),
        ("Audio format badge", "Nhãn định dạng âm thanh"),
        ("Show sample rate in the badge", "Hiện tần số lấy mẫu trong nhãn"),
        ("Show the volume slider", "Hiện thanh âm lượng"),
        ("Show lyrics / queue beside the player", "Hiện lời bài hát / hàng đợi cạnh trình phát"),
        ("Clock face", "Mặt đồng hồ"),
        ("Big", "Lớn"),
        ("Stacked", "Xếp chồng"),
        ("Analog", "Kim"),
        ("24-hour clock", "Đồng hồ 24 giờ"),
        ("Dim after", "Làm tối sau"),
        ("0 = never dim. A mouse move wakes the clock.", "0 = không bao giờ làm tối. Di chuột để đánh thức đồng hồ."),
        ("Warm ink at night", "Chữ màu ấm vào ban đêm"),
        ("Colour", "Màu"),
        ("Silver", "Bạc"),
        ("Black", "Đen"),
        ("True dark", "Đen tuyền"),
        ("Red", "Đỏ"),
        ("Turn the wheel (drag round it or scroll) for the volume, click the centre to scrub, MENU closes.", "Xoay bánh xe (kéo vòng quanh hoặc cuộn) để chỉnh âm lượng, bấm nút giữa để tua, MENU để đóng."),
        // appearance
        ("Motion & glass", "Chuyển động & kính mờ"),
        ("Animations", "Hiệu ứng chuyển động"),
        ("Animation speed", "Tốc độ chuyển động"),
        ("Glass effects (frosted cover backgrounds)", "Hiệu ứng kính (nền ảnh bìa mờ)"),
        ("Turn glass off on slow GPUs: the player backgrounds are drawn flat.", "Tắt hiệu ứng kính trên GPU yếu: nền trình phát sẽ được vẽ phẳng."),
        ("Accent", "Màu nhấn"),
        ("From artwork", "Theo ảnh bìa"),
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

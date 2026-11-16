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
        // ---- integrations ----
        ("An import is still running. Try again once it's done.", "Một lần nhập khác đang chạy. Hãy thử lại khi nó xong."),
        ("Reading the playlist…", "Đang đọc danh sách phát…"),
        ("No songs found in that file. It needs a title and artist column (TuneMyMusic, Exportify or Mochy CSV), or one \"Artist - Title\" per line.", "Không tìm thấy bài hát nào trong tệp. Tệp cần có cột tên bài và nghệ sĩ (CSV của TuneMyMusic, Exportify hoặc Mochy), hoặc mỗi dòng một \"Nghệ sĩ - Tên bài\"."),
        ("Connecting…", "Đang kết nối…"),
        ("Reading your library…", "Đang đọc thư viện của bạn…"),
        ("Reading", "Đang đọc"),
        ("Import undone: {n} songs left the library.", "Đã hoàn tác: {n} bài hát đã rời thư viện."),
        ("Import playlists", "Nhập danh sách phát"),
        ("Link", "Liên kết"),
        ("File or list", "Tệp hoặc danh sách"),
        ("Account", "Tài khoản"),
        ("Paste a link to a public playlist or album.", "Dán liên kết tới một danh sách phát hoặc album công khai."),
        ("Read", "Đọc"),
        ("Spotify links only share the first 100 songs of a playlist, and private playlists can't be read by link; use a CSV export or the account import for those.", "Liên kết Spotify chỉ chia sẻ 100 bài đầu của danh sách phát, và danh sách riêng tư không đọc được qua liên kết; hãy dùng tệp CSV xuất ra hoặc nhập từ tài khoản."),
        ("A CSV from TuneMyMusic, Exportify or Mochy, or a text file with one \"Artist - Title\" per line.", "Tệp CSV từ TuneMyMusic, Exportify hoặc Mochy, hoặc tệp văn bản mỗi dòng một \"Nghệ sĩ - Tên bài\"."),
        ("Choose a file…", "Chọn tệp…"),
        ("Or paste a list:", "Hoặc dán danh sách:"),
        ("Imported playlist", "Danh sách phát đã nhập"),
        ("Read list", "Đọc danh sách"),
        ("No songs found. Write one \"Artist - Title\" per line.", "Không tìm thấy bài hát. Hãy viết mỗi dòng một \"Nghệ sĩ - Tên bài\"."),
        ("Read your own library from a signed-in account. Mochy has no built-in browser, so copy the session token from the service's web player yourself; it is only kept in memory and only sent to that service.", "Đọc thư viện của bạn từ tài khoản đã đăng nhập. Mochy không có trình duyệt tích hợp, nên hãy tự sao chép mã phiên (token) từ trình phát web của dịch vụ; mã chỉ được giữ trong bộ nhớ và chỉ gửi tới chính dịch vụ đó."),
        ("Service", "Dịch vụ"),
        ("Open web player", "Mở trình phát web"),
        ("Sign in at open.spotify.com, press F12 → Network, pick a request to api.spotify.com or api-partner.spotify.com and copy the text after \"Bearer \" in its authorization header. Tokens expire after about an hour.", "Đăng nhập open.spotify.com, nhấn F12 → Network, chọn một yêu cầu tới api.spotify.com hoặc api-partner.spotify.com và sao chép phần sau \"Bearer \" trong header authorization. Mã hết hạn sau khoảng một giờ."),
        ("Sign in at music.apple.com, press F12 → Network, pick a request to amp-api.music.apple.com and copy the Bearer token from its authorization header and the media-user-token header.", "Đăng nhập music.apple.com, nhấn F12 → Network, chọn một yêu cầu tới amp-api.music.apple.com và sao chép mã Bearer trong header authorization cùng header media-user-token."),
        ("Sign in at listen.tidal.com, press F12 → Network, pick a request to api.tidal.com and copy the Bearer token from its authorization header.", "Đăng nhập listen.tidal.com, nhấn F12 → Network, chọn một yêu cầu tới api.tidal.com và sao chép mã Bearer trong header authorization."),
        ("Developer token", "Mã nhà phát triển"),
        ("Access token", "Mã truy cập"),
        ("Connect", "Kết nối"),
        ("Connected to", "Đã kết nối với"),
        ("Select all", "Chọn tất cả"),
        ("Select none", "Bỏ chọn tất cả"),
        ("Read selected", "Đọc mục đã chọn"),
        ("Disconnect", "Ngắt kết nối"),
        ("Imports", "Các lần nhập"),
        ("Nothing imported yet.", "Chưa nhập gì."),
        ("{songs} songs, {playlists} playlists", "{songs} bài hát, {playlists} danh sách phát"),
        ("Undo", "Hoàn tác"),
        ("Songs by source", "Bài hát theo nguồn"),
        ("Delete all imports from it", "Xóa mọi thứ đã nhập từ nguồn này"),
        ("Imported playlists", "Danh sách phát đã nhập"),
        ("Re-sync reads a playlist again and adds the songs that are new on the service; songs you removed here stay out.", "Đồng bộ lại sẽ đọc lại danh sách phát và thêm các bài mới trên dịch vụ; bài bạn đã xóa ở đây sẽ không bị thêm lại."),
        ("Re-sync", "Đồng bộ lại"),
        ("re-sync with the account import", "đồng bộ lại bằng cách nhập từ tài khoản"),
        ("no link saved — import its link again", "chưa lưu liên kết — hãy nhập lại liên kết của nó"),
        ("follows YouTube", "theo YouTube"),
        ("Re-sync all", "Đồng bộ lại tất cả"),
        ("Ready to import", "Sẵn sàng nhập"),
        ("a file", "một tệp"),
        ("From", "Từ"),
        ("Library songs", "Bài hát trong thư viện"),
        ("Each song is looked up on YouTube Music; playlists you imported before are topped up instead of copied again.", "Mỗi bài sẽ được tìm trên YouTube Music; danh sách phát đã nhập trước đó sẽ được bổ sung thay vì sao chép lại."),
        ("Import", "Nhập"),
        ("Back", "Quay lại"),
        ("Re-syncing from", "Đang đồng bộ lại từ"),
        ("Importing from", "Đang nhập từ"),
        ("Found", "Đã tìm thấy"),
        ("Stop", "Dừng"),
        ("You can close this window; the import keeps going.", "Bạn có thể đóng cửa sổ này; việc nhập vẫn tiếp tục."),
        ("Re-sync finished", "Đồng bộ lại xong"),
        ("{added} new songs in {changed} of {n} playlists", "{added} bài mới trong {changed}/{n} danh sách phát"),
        ("Couldn't read:", "Không đọc được:"),
        ("Import stopped", "Đã dừng nhập"),
        ("Import finished", "Nhập xong"),
        ("Found {m} of {t} songs", "Tìm thấy {m}/{t} bài hát"),
        ("{n} playlists created", "Đã tạo {n} danh sách phát"),
        ("{n} songs liked", "Đã thích {n} bài hát"),
        ("Not found", "Không tìm thấy"),
        ("Pick the right song yourself; the choice is remembered for later imports and re-syncs.", "Hãy tự chọn đúng bài; lựa chọn sẽ được nhớ cho các lần nhập và đồng bộ lại sau."),
        ("Find", "Tìm"),
        ("Nothing found on YouTube Music.", "Không tìm thấy gì trên YouTube Music."),
        ("Use", "Dùng"),
        ("Close", "Đóng"),
        ("Done", "Xong"),
        ("Spotify only shares a playlist's first 100 songs by link. Use the account import to check the rest.", "Spotify chỉ chia sẻ 100 bài đầu qua liên kết. Hãy nhập từ tài khoản để kiểm tra phần còn lại."),
        ("The scrobbling service signed you out. Sign in again in Settings.", "Dịch vụ scrobble đã đăng xuất bạn. Hãy đăng nhập lại trong Cài đặt."),
        ("Discord", "Discord"),
        ("Scrobbling", "Scrobble"),
        ("Import & sync", "Nhập & đồng bộ"),
        ("Bring playlists and liked songs over from Spotify, Apple Music, Tidal, Deezer, YouTube Music or a CSV file. Songs are matched on YouTube Music and saved as local playlists.", "Chuyển danh sách phát và bài đã thích từ Spotify, Apple Music, Tidal, Deezer, YouTube Music hoặc tệp CSV. Bài hát được tìm trên YouTube Music và lưu thành danh sách phát trên máy."),
        ("Import playlists…", "Nhập danh sách phát…"),
        ("Off", "Tắt"),
        ("Connected — nothing playing", "Đã kết nối — không phát gì"),
        ("Connecting to Discord…", "Đang kết nối với Discord…"),
        ("Showing on your profile", "Đang hiển thị trên hồ sơ của bạn"),
        ("Discord isn't running — trying again…", "Discord chưa chạy — đang thử lại…"),
        ("Discord refused the status:", "Discord từ chối trạng thái:"),
        ("Show what you're listening to on Discord", "Hiển thị bài bạn đang nghe trên Discord"),
        ("Enter a Discord application ID to turn it on.", "Nhập Application ID của Discord để bật."),
        ("Application ID", "Application ID"),
        ("Developer Portal", "Developer Portal"),
        ("Discord shows the status through its desktop app. Create an application in the Discord Developer Portal (its name is what your profile shows), copy its Application ID here, and keep Discord running.", "Discord hiển thị trạng thái qua ứng dụng desktop của nó. Hãy tạo một ứng dụng trong Discord Developer Portal (tên ứng dụng là thứ hồ sơ của bạn hiển thị), dán Application ID vào đây và giữ Discord đang chạy."),
        ("Customize the status", "Tùy chỉnh trạng thái"),
        ("Activity", "Hoạt động"),
        ("Listening to", "Đang nghe"),
        ("Playing", "Đang chơi"),
        ("First line", "Dòng thứ nhất"),
        ("Second line", "Dòng thứ hai"),
        ("Big picture text", "Chữ trên ảnh lớn"),
        ("Small picture text", "Chữ trên ảnh nhỏ"),
        ("Placeholders:", "Biến thay thế:"),
        ("Big picture", "Ảnh lớn"),
        ("Small picture", "Ảnh nhỏ"),
        ("Big picture URL", "URL ảnh lớn"),
        ("Small picture URL", "URL ảnh nhỏ"),
        ("Button 1", "Nút 1"),
        ("Button 2", "Nút 2"),
        ("An empty line or button label is left off.", "Dòng hoặc nhãn nút để trống sẽ bị bỏ qua."),
        ("Clear the status when paused", "Xóa trạng thái khi tạm dừng"),
        ("Refresh every", "Làm mới mỗi"),
        ("Reset to defaults", "Đặt lại mặc định"),
        ("Preview", "Xem trước"),
        ("Listening to your Discord app", "Đang nghe ứng dụng Discord của bạn"),
        ("Playing your Discord app", "Đang chơi ứng dụng Discord của bạn"),
        ("Send what you listen to to Last.fm and Libre.fm. A song counts once it has played for half its length or four minutes, whichever comes first; plays made offline are kept and sent later.", "Gửi những gì bạn nghe tới Last.fm và Libre.fm. Một bài được tính khi đã phát được nửa thời lượng hoặc bốn phút, tùy điều kiện nào đến trước; lượt nghe khi ngoại tuyến được giữ lại và gửi sau."),
        ("Show \"Now Playing\" while a song plays", "Hiển thị \"Đang phát\" khi một bài đang phát"),
        ("Send likes as loved tracks", "Gửi bài đã thích thành bài yêu thích (love)"),
        ("Plays waiting to be sent", "Lượt nghe đang chờ gửi"),
        ("Send now", "Gửi ngay"),
        ("When a play counts", "Khi nào một lượt nghe được tính"),
        ("Played fraction", "Tỉ lệ đã phát"),
        ("Or after at most", "Hoặc sau tối đa"),
        ("Ignore songs shorter than", "Bỏ qua bài ngắn hơn"),
        ("Signed in as", "Đã đăng nhập với tên"),
        ("Scrobble", "Scrobble"),
        ("Open profile", "Mở hồ sơ"),
        ("Sign out", "Đăng xuất"),
        ("API key", "API key"),
        ("Secret", "Secret"),
        ("Create your own API account on Last.fm and paste its key and secret. They stay on this computer.", "Tạo tài khoản API của riêng bạn trên Last.fm rồi dán key và secret vào đây. Chúng chỉ được lưu trên máy này."),
        ("Libre.fm accepts any 32-character key and secret you make up. They stay on this computer.", "Libre.fm chấp nhận key và secret bất kỳ dài 32 ký tự do bạn tự đặt. Chúng chỉ được lưu trên máy này."),
        ("Get an API key", "Lấy API key"),
        ("Signing in…", "Đang đăng nhập…"),
        ("Approve Mochy in the browser page that just opened, then come back and finish.", "Hãy cho phép Mochy trên trang trình duyệt vừa mở, rồi quay lại và hoàn tất."),
        ("I've approved it — finish", "Tôi đã cho phép — hoàn tất"),
        ("Open the page again", "Mở lại trang"),
        ("Sign in with the browser", "Đăng nhập bằng trình duyệt"),
        ("(recommended)", "(khuyên dùng)"),
        ("Username", "Tên người dùng"),
        ("Password", "Mật khẩu"),
        ("Sign in", "Đăng nhập"),
        ("The password only goes to the service and isn't saved.", "Mật khẩu chỉ được gửi tới dịch vụ và không được lưu."),
        ("Create an account", "Tạo tài khoản"),
        ("Turn it off", "Tắt"),
        ("Music artwork", "Ảnh bìa bài hát"),
        ("Artist artwork", "Ảnh nghệ sĩ"),
        ("Playback status", "Trạng thái phát"),
        ("Mochy logo", "Logo Mochy"),
        ("Your own picture", "Ảnh của bạn"),
        ("Mochy on GitHub", "Mochy trên GitHub"),
        ("Song name", "Tên bài hát"),
        ("Artist name", "Tên nghệ sĩ"),
        ("Format and bitrate", "Định dạng và bitrate"),
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

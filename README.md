# 🎵 Mochy Music — Desktop (Rust)

<div align="center">

**Bản desktop Windows của [Mochy Music](https://github.com/realidkroo/mochy-music) (Android) — thuần Rust, không Electron/WebView.**

### ⬇️ TẢI BẢN MỚI NHẤT (.exe) — CHỈ 1 CLICK

[![Download](https://img.shields.io/badge/⬇️_Tải_mochy--app.exe-v0.1.5-2ea44f?style=for-the-badge&logo=github)](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/download/v0.1.5/mochy-music-desktop-v0.1.5-windows-x64.zip)

[![Latest release](https://img.shields.io/github/v/release/lehoangquan2007td-IT/mochy-music-destop-)](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/latest)
[![License](https://img.shields.io/github/license/lehoangquan2007td-IT/mochy-music-destop-)](LICENSE)

*Tiếng Anh: [Download the latest .exe from Releases](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/latest) — no installer needed.*

</div>

## 📥 Hướng dẫn tải về nhanh (Windows 10/11 x64)

1. Bấm nút **⬇️ Tải** ở trên (hoặc vào **[Releases](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/latest)**) → nhận file `mochy-music-desktop-v0.1.5-windows-x64.zip` (~11 MB).
2. Giải nén → chạy **`mochy-app.exe`**. Không cần cài đặt, không cần runtime nào thêm (đóng bản đang chạy trước khi đè).
3. Xong! Dữ liệu app nằm trong `%LOCALAPPDATA%\MochyMusic`. Phiên bản hiển thị trong **Settings**.

| Bản phát hành | Ngày | Nội dung | Gói tải về |
|---|---|---|---|
| **v0.1.5** (mới nhất) | 16/11/2026 | Tối ưu ảnh bìa: cover-fit khít khung, nguồn maxres nét hơn, cắt dải đen, mipmap | [zip](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/download/v0.1.5/mochy-music-desktop-v0.1.5-windows-x64.zip) |
| v0.1.4 | 16/11/2026 | Sửa lyric chạy trước nhạc: offset bù trễ toàn cục (Settings → Lyrics) + bấm lyric seek đúng nhịp | [zip](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/download/v0.1.4/mochy-music-desktop-v0.1.4-windows-x64.zip) |
| v0.1.3 | 16/11/2026 | Chọn ngôn ngữ giao diện English / Tiếng Việt trong Settings | [zip](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/download/v0.1.3/mochy-music-desktop-v0.1.3-windows-x64.zip) |
| v0.1.2 | 16/11/2026 | Fix lyric chạy trước nhạc + bump version | [zip](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/download/v0.1.2/mochy-music-desktop-v0.1.2-windows-x64.zip) |
| v0.1.1 | 16/11/2026 | Fix lyric chạy trước nhạc (chưa bump version) | [zip](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/download/v0.1.1/mochy-music-desktop-v0.1.1-windows-x64.zip) |
| v0.1.0 | 16/11/2026 | Bản đầu (đã gồm fix đề xuất tìm kiếm) | [zip](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/download/v0.1.0/mochy-music-desktop-v0.1.0-windows-x64.zip) |

> 💡 Link **[releases/latest](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/latest)** luôn trỏ về bản mới nhất — ghi bookmark để cập nhật khi có phiên bản sau.
>
> 📌 **v0.1.1**: sửa lỗi lời bài hát hiện **trước** nhạc 300 ms — giờ dòng lời sáng lên đúng khi câu hát được nghe. Chi tiết trong [Release notes](https://github.com/lehoangquan2007td-IT/mochy-music-destop-/releases/tag/v0.1.1).

---

## Giới thiệu

Desktop port of [Mochy Music](https://github.com/realidkroo/mochy-music) (Android,
GPL-3.0) written in **pure Rust** (no Node/Electron): `egui` GUI, `symphonia` +
`cpal` audio engine with a full Audio Lab DSP chain, `reqwest` networking.

> Status: v0.2.0 — most Android features ported. YouTube Music browse/search/playback, local files,
> synced lyrics and the Audio Lab all work end to end. See "Known limitations".
> `docs/PORT_SPEC.md` is the port specification extracted from the Android sources.

## ✨ Features

- **YouTube Music**: Home shelves, search (suggestions, history, songs/videos/albums/artists/playlists),
  album / artist / playlist pages, radio autoplay when the queue ends, next-track prefetch.
- **Local files**: add a folder, tags + embedded artwork via `lofty`; AAC/M4A, MP3, FLAC, ALAC,
  Vorbis, Opus (WebM/Ogg, pure-Rust decoder), WAV/AIFF/CAF.
- **Audio Lab** (applied live, click-free): parametric EQ up to 20 bands with draggable response
  curve (the plotted curve is the exact filter response), 21 built-in presets + user presets,
  AutoEq `ParametricEQ.txt` import/export, auto-preamp, always-on peak limiter, loudness
  processor, ReplayGain / YouTube loudness normalization, exponential volume curve,
  speed 0.1–8× with optional pitch preservation (WSOLA) and semitone shift, Freeverb reverb
  with 8 presets, spatial (crossfeed, virtual speakers, mono, width), live spectrum.
- **Lyrics**: 10 providers queried in parallel and scored like Android (Apple Music/BetterLyrics,
  YouLyPlus, LrcRed, SimpMusic, LrcLib, Deezer, NetEase, QQMusic, Kugou); word-synced karaoke,
  duets, background vocals, click-to-seek, per-song offset (saved).
- **Library**: songs / albums / artists / playlists / recently added with search, sort and grid/list;
  liked songs, local playlists, history grouped by day with "remove from history", most played;
  save albums/artists/playlists; persistent queue.
- **Explore**: new releases, charts, moods & genres, "see all" pages, home mood chips,
  local shelves (quick picks, recently played, forgotten favorites) and infinite scroll.
- **Stats & Recap**: listening time, top songs/artists/albums per week/month/year,
  charts by day/hour/weekday, and a paged recap story.
- **Transitions**: gapless, crossfade and beat-aware Crossmix (tempo/beat/key analysis, 14 styles),
  fades on play/pause/skip/seek, skip silence, sleep timer, per-song speed/pitch memory.
- **Downloads & offline**: download songs/albums/playlists (Opus remuxed to `.opus`, AAC as `.m4a`,
  tags + cover + lyrics), download queue, Downloaded tab, offline playback, auto-download liked songs,
  optional stream cache; edit song title/artist/album/cover and import lyrics files
  (LRC/TTML/SRT/VTT/ASS/YRC/QRC/KRC).
- **Lyrics extras**: translation (Google Translate), romanization (Japanese, Korean, Chinese, Cyrillic,
  Hindi, Punjabi), YouTube Music / YouTube captions providers, embedded and sidecar `.lrc` lyrics,
  auto-timing of plain lyrics from captions, alignment/weight/blur display options.
- **Audio Lab v2**: hub with Equalizer / Audio FX / Headphones / Quality; AutoEq database browser
  (6000+ headphones, fitted correction under your EQ); FX presets (Nightcore, Slowed + reverb…);
  stream info ("stats for nerds").
- **Now Playing styles**: Flagship (backgrounds, cover shapes), Turntable, Click wheel, Bedside clock;
  animation speed, glass toggle, accent colours.
- **Integrations**: Discord Rich Presence (your own application id), Last.fm / Libre.fm scrobbling with
  offline queue, playlist importer (Spotify / Apple Music / Deezer / YouTube links, CSV, text lists)
  with Android's TrackMatcher, undo and re-sync.
- **Desktop**: Now Playing view with artwork-derived gradient & accent, output-device picker,
  OS media keys / Windows SMTC, keyboard shortcuts (Space, Ctrl+←/→, ←/→, Ctrl+↑/↓, Ctrl+F),
  remembers window size. Inter font (Vietnamese OK) + system CJK/Korean/Thai fallbacks.

## 🧩 Crates

| Crate | Role |
|---|---|
| `mochy-core` | Shared models, settings, EQ math (RBJ biquads, response, auto-preamp), presets |
| `mochy-audio` | Engine: symphonia decode thread → lock-free ring → cpal; DSP chain; HTTPS ranged streaming |
| `mochy-innertube` | YouTube Music InnerTube client (search/browse/queue, VISIONOS-first stream walk) |
| `mochy-lyrics` | Lyrics providers, LRC/TTML parsing, Android-compatible scoring |
| `mochy-storage` | SQLite (Room-v43-compatible schema), lenient/atomic settings, cache paths |
| `mochy-app` | eframe/egui desktop app |

## 🔧 Build & run

```powershell
# Windows (MSYS2 gcc toolchain, no Visual Studio needed)
$env:Path = "$env:USERPROFILE\.cargo\bin;C:\msys64\ucrt64\bin;C:\Program Files\CMake\bin;$env:Path"
cargo build --release -j 4        # -j limits memory use
./target/release/mochy-app.exe
```

Data lives in `%LOCALAPPDATA%\MochyMusic` (`mochy.db`, `settings.json`, `logs\mochy.log`).

### Developer tools

- `cargo test --workspace` — ~170 unit/integration tests (DSP accuracy, parsers, DB schema, queue logic).
- `mochy-app --smoke "<query>" --secs N` — headless end-to-end: search → resolve → play → seek → skip.
- `MOCHY_SHOTS=<dir> [MOCHY_SHOTS_VIDEO=<videoId>] mochy-app` — screenshots of each page, then exit.
- `cargo run -p mochy-innertube --example ytm_smoke` — live YouTube Music API + stream check.
- `cargo run -p mochy-lyrics --example lyrics -- "<title>" "<artist>" <secs> [provider|all]`.
- `cargo run -p mochy-audio --example play -- <path-or-url> [secs]`.
- `cargo run -p mochy-app --example widgets_demo` — Audio Lab / lyrics / theme widgets standalone.

## ⚠️ Known limitations

- YouTube streams come from a single client (VISIONOS, no PoToken). YouTube intermittently
  refuses it ("stream cut off" / bot check); playback then fails until it recovers. Auto-skip
  stops after 3 consecutive failures to avoid making it worse. The Android app's extra
  fallbacks (InnerTubeX backend, player-JS signature deciphering) are not ported.
- Not yet ported: Shazam recognition, YouTube account sign-in/sync, music videos and
  Spotify Canvas, Android-only features (USB DAC exclusive mode, car/TV/widgets).
- Crossmix analysis downloads part of the current/next song; switch to Crossfade in
  Settings → Transitions to save data.
- "Low" quality uses HE-AAC; symphonia decodes only the LC core (sounds muffled) — use Auto/Medium.

---

## 📄 License

GPL-3.0-or-later (inherited from Mochy Music). Not affiliated with YouTube/Google.

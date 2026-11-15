# Mochy Music — Desktop Port Specification

**Target**: Rust engineers porting the Android app **Mochy Music** to desktop (Windows/Linux, pure Rust + egui).
**Source of truth**: Kotlin sources at `mochy-music-source` (app module `app/src/main/kotlin/com/mochyy/mochymusic/`, provider modules under `providers/`, runtime modules under `modules/`). Every URL, path, and constant below was read from that source. Cited file names are relative to those roots.

---

## 1. Product Overview

Mochy is a streaming-first music player for YouTube Music with a local library, an elaborate audio DSP stack ("Audio Lab"), a 13-provider lyrics engine, and many third-party integrations. Package `com.mochyy.mochymusic`; DB file `mochy.db`; UI is **dark-only** (`Theme.kt`: `const val ForceDarkTheme = true`). Lineage: fork of Metrolist (innertube helper library package is `com.metrolist.innertubex`). Repo: `github.com/realidkroo/Mochy-Music`.

Gradle modules (`settings.gradle.kts`): `app`; providers `{canvas, innertube, kugou, lrclib, lastfm, betterlyrics, simpmusic, youlyplus, shazamkit, artistvideo, applecanvas, paxsenixlyrics, directlyrics, musixmatch, genius, wikipedia}`; modules `{api, bridge, applemusic, monochrome, jiosaavn}`.

### 1.1 Feature inventory (desktop scope)

| Area | Android behavior | Desktop phase |
|---|---|---|
| YTM search & stream playback | full client walk, InnerTubeX backend | **MVP** |
| Local files | `localUri` on SongEntity, artwork extraction, embedded lyrics | **MVP** |
| Queue / shuffle / repeat / autoplay | timeline windows, infinite radio queue | **MVP** |
| Library DB + playlists + likes | Room v43 schema (§5) | **MVP** |
| Lyrics (synced + word-sync) | 13 providers, LRC dialect (§3) | line-sync **MVP**, word-sync Phase 2 |
| 10-band EQ + presets + preamp | Audio Lab (§4) | **MVP** |
| Speed / pitch / volume curve | sonic on audio track (§4.6) | **MVP** |
| Themes / dynamic accent | Palette from artwork (§7) | **MVP** |
| Settings persistence | DataStore 350+ keys (§5.3) | **MVP** |
| Downloads (opus) | DownloadUtil, .opus remux | Phase 2 |
| Importer (Spotify/Apple/Tidal/CSV/device) | §6 | Phase 2 |
| FLAC module (Monochrome) | §2.7 | Phase 2 |
| Discord rich presence | §8.1 | Phase 2 |
| Last.fm / Libre.fm scrobbling | §8.2 | Phase 2 |
| Shazam recognition | §8.3 | Phase 2 |
| Word-sync karaoke visuals | LyricsMotionStyle.MOTION | Phase 2 |
| MPRIS (Linux) / SMTC (Windows) | MediaLibrarySession equivalents | Phase 2 |
| Global hotkeys | n/a on Android | Phase 2 |

### 1.2 Porting notes that shape everything

- The Android app is **Kotlin + Media3/ExoPlayer + Compose**. The playback engine is a pipeline of Media3 `AudioProcessor`s; every one of them is fully specified in §4 and must be re-implemented as pure Rust DSP in a `cpal`/`symphonia` callback chain.
- All server dialogs are plain HTTPS/JSON — no gRPC, no protobuf (except InnerTube JSON bodies). Everything is portable with `reqwest` + `serde`.
- Timestamps: DB uses `LocalDateTime` (ISO), DataStore mixes epoch-millis and ISO strings.

---

## 2. Innertube / YouTube Music client

Files: `providers/innertube/src/main/kotlin/com/music/innertube/{InnerTube.kt, YouTube.kt, YouTubeClient.kt, pages/NewPipe.kt}`; app side `utils/YTPlayerUtils.kt`, `utils/InnerTubeXStreams.kt`.

### 2.1 API base and headers

- Base: `https://music.youtube.com/youtubei/v1/{endpoint}?pretty=false` (POST, JSON body).
- Required headers (from `InnerTube.kt`): `Content-Type: application/json`, `X-YouTube-Client-Name: <id>`, `X-YouTube-Client-Version: <version>`, `User-Agent` (client-specific), optional `Cookie` (SAPISID, HSID, SID…), `Authorization: SAPISIDHASH <epochSeconds>_<sha1("<epoch> <sapisid> <origin>")>` when logged in, `X-Origin: https://music.youtube.com`.
- WEB client User-Agent: `Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0`.
- Retry policy: transient failures retried with backoff; connect/read timeouts from `NetworkConfig.kt`.
- Visitor data: fetched from `https://music.youtube.com/sw.js_data` (strip first 5 chars of the `visitorData` JSON string; accept only regex `^Cg[t|s]`), or minted via `POST {origin}/youtubei/v1/visitor_id`. VISIONOS mints its **own** visitor id.

### 2.2 Client constants (`YouTubeClient.kt`)

| Client | id | version | notes |
|---|---|---|---|
| WEB | 1 | `2.20260708.00.00` | default browse/search |
| WEB_REMIX | 67 | `1.20260707.12.00` | YTM app client; needs `signatureTimestamp` + poToken for player |
| VISIONOS | 101 | `1.04` | **main stream client**; `apiOrigin www.youtube.com`, `apiFormatVersion 2`, `clientScreen WATCH`, ownVisitorId, no PoToken needed |
| TVHTML5_SAMSUNG | — | — | account/age-restricted fallback |
| WEB_EMBEDDED_PLAYER | 56 | — | age-restricted fallback |
| TVHTML5_DOWNGRADED | — | — | age-restricted fallback |
| IOS | 5 | — | captions provider; alternate streams |
| ANDROID_VR / ANDROID_VR_NO_AUTH | — | — | captions / no-auth captions |

### 2.3 Endpoints (`YouTube.kt`)

| Purpose | Endpoint | Notes |
|---|---|---|
| Search | `POST search` | body `{context, query, params: <filter>}` |
| Suggestions | `POST music/get_search_suggestions` | input `input: query` |
| Browse (album/artist/playlist/home/explore/charts/moods) | `POST browse` | `browseId` (+ optional `params`) |
| Next/queue/radio/related/lyrics | `POST next` | `videoId`, `playlistId`, `playlistSetVideoId`, `index`, `params`; lyrics browse id arrives inside `next` response |
| Player (stream formats) | `POST player` | `videoId`, `contentCheckOk:true`, `racyCheckOk:true`; WEB_REMIX adds `signatureTimestamp` |
| Transcript | `POST get_transcript` | base64 params built as `"\n{11-char block}{videoId}"` (0x0A-prefixed protobuf) |
| Like/unlike | `POST music/private/v1/like(like|dislike|removelike)` | needs auth cookie |
| Playlist add/remove/rename | `POST music/private/v1/playlist/edit` | actions `playlistEditAddItem` (with `setVideoId` bookkeeping), `playlistEditRemoveItem`, `playlistEditSetName`, etc. |
| Subscribe channel | `POST subscription/unsubscribe` / `subscribe` via browse ids | |

Search filter `params` (exact, URL-encoded protobufs):

| Filter | params |
|---|---|
| FILTER_SONG | `EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D` |
| FILTER_VIDEO | `EgWKAQIQAWoKEAkQChAFEAMQBA%3D%3D` |
| FILTER_ALBUM | `EgWKAQIYAWoKEAkQChAFEAMQBA%3D%3D` |
| FILTER_ARTIST | `EgWKAQIgAWoKEAkQChAFEAMQBA%3D%3D` |
| FEATURED_PLAYLIST | `EgeKAQQoADgBagwQDhAKEAMQBRAJEAQ%3D` |
| COMMUNITY_PLAYLIST | `EgeKAQQoAEABagoQAxAEEAoQCRAF` |
| Charts | browse `FEmusic_charts`, params `ggMGCgQIgAQ%3D` |

Library filter params exist as `YouTube.LibraryFilter` constants (songs/uploads/playlists/artists/albums) — same `browse` call with `FEmusic_library_privately_owned_*` browse ids. `MAX_GET_QUEUE_SIZE = 1000`.

Response parsing walks `musicResponsiveListItemRenderer` (flex column texts: runs[0].text), `musicTwoRowItemRenderer`, `musicShelfRenderer.contents`, `musicCarouselShelfRenderer`, `tabs[0].tabRenderer.content`, `musicDescriptionShelfRenderer` etc. (`YouTube.kt`, `MusicResponsiveListItemRenderer.kt`).

### 2.4 Stream acquisition order (`YTPlayerUtils.kt`, `InnerTubeXStreams.kt`)

1. **InnerTubeX backend first** (metrolist `innertubex` library): if enabled (`StreamBackendKey`/`streamBackend` = `INNERTUBEX` vs `CLASSIC`), ask it; give up after **12 s**, fall through.
2. **Mochy's own client walk**: `MAIN_CLIENT = VISIONOS` first (direct URLs, no PoToken), then `STREAM_FALLBACK_CLIENTS` (WEB_REMIX needs `signatureTimestamp` + poToken; TVHTML5_SAMSUNG / WEB_EMBEDDED_PLAYER / TVHTML5_DOWNGRADED serve age-restricted content when logged in). `ACCOUNT_CLIENTS` are appended for age-gated videos.
3. **NewPipe fallback** (`pages/NewPipe.kt`): `NewPipeExtractor.getStreamUrl` uses `YoutubeJavaScriptPlayerManager` to decipher signatures; `newPipePlayer` returns (itag, url) pairs.
4. Bot-check handling: if VISIONOS fails with a bot check it **rests 30 min**; at most `MAX_STAND_INS = 2` stand-in uploads are substituted before giving up; premium-only tracks are detected and reported.

### 2.5 Format / itag selection (`YTPlayerUtils.kt`)

- Quality mapping (`AudioQuality` enum): `AUTO` (best available), YouTube tiers `LOW`/`MEDIUM`/`HIGH`, Monochrome-only `LOSSLESS` / `HI_RES_LOSSLESS` / `DOLBY_ATMOS`.
- Bitrate targets: LOW ≈ **70 kbps**, MEDIUM ≈ **135 kbps**, HIGH = best available.
- Ranking: `qualityRank` = AUDIO_QUALITY_HIGH > MEDIUM > LOW; `codecRank` = opus > mp4a; then prefer direct URL, more channels, higher bitrate, higher sample rate. Default UI quality is `AUTO` (pref `audioQuality`); `DataSaverKey` lowers streams and disables canvas/video backgrounds.
- MEDIUM on the InnerTubeX backend maps to the MP4 (AAC 128k) format.

### 2.6 URL validation and lifetime

- Validation = ranged GET at **last byte** (`contentLength - 1`); untokened URLs are probed at byte **`UNTOKENED_WINDOW_PROBE_BYTE = 1_048_576`** because untokened URLs serve only the first **~786 KB ≈ 28–32 s**.
- URLs are post-processed: `n`-param transform via `EjsNTransformSolver` / `CipherDeobfuscator`; PoToken appended as `pot=`; per-client `streamHeaders` are required on the media request (Range + UA).
- Download resumes refuse a mid-download itag change (`DownloadUtil.kt`: `downloadItags` map, error "Stream format changed part way through").

### 2.7 Source modules (modules/api, `MochyModule.kt`)

`MODULE_API_VERSION = 3`. `ModuleFeature ∈ {AudioSource, Lyrics, Look, Metadata}`. A `SourceModule` answers `resolve(TrackQuery{title, artists, album, durationMs, isrc}, picks) → ResolveResult{Found(SourceStream), NotFound, Unavailable}`. `SourceStream{url, format: Progressive|Hls|Dash, headers, drm(Widevine license+initData), sourceLabel, qualityLabel, matched(MatchedTrack confidence 0..1), codec, sampleRateHz}`. Host provides per-module `storage`, proxied `http`, `log`, `folder`.

- **Monochrome** (FLAC/TIDAL-backed): streamer `https://tracks.monochrome.st/search/tracks?q=<q>&limit=8`; relay servers `GET /search?q=<q>&n=<n>` then `GET /s/{id}?q=flac` with header `X-Relay-Client: relay/2`; FLAC `STREAMINFO` is parsed for sample rate/bits (`flacInfo`); hi-res labels; cache: miss memory **30 min**, unreachable retry **10 min**.
- **JioSaavn**: `https://www.jiosaavn.com/api.php?__call=search.getResults&_format=json&_marker=0&api_version=4&ctx=android&p=1&n=<count>&q=<query>`, decrypt key `38346591`; quality choices 320/160/96; same 30 min/10 min caches.

---

## 3. Lyrics system

Files: `lyrics/{LyricsHelper.kt, LyricsProviderRegistry.kt, LyricsUtils.kt, CaptionAligner.kt, VideoLyricsSync.kt, EmbeddedLyrics.kt}`; providers under `providers/{youlyplus, betterlyrics, paxsenixlyrics, lrclib, simpmusic, kugou, directlyrics, musixmatch, genius, wikipedia, applecanvas}`.

### 3.1 Provider registry (`LyricsProviderRegistry.kt`)

Default order (13 app providers + module providers): **BetterLyrics, YouLyPlus, Paxsenix, LrcRed, SimpMusic, LrcLib, Deezer, YouTubeSubtitle, YouTubeMusic, NetEase, QQMusic, Kugou, YouTubeCaptions**. Display name trap: BetterLyrics is shown to users as **"Apple Music"**. Enable switches: `enableKugou`, `enableLrclib`, `enableBetterLyrics`, `enableSimpMusic`, `enableYouLyPlus`, `enablePaxsenix`, `enableNetEase`, `enableQQMusic`, `enableDeezer`, `enableLrcRed` (DataStore booleans, default on).

### 3.2 Provider endpoints (exact)

| Provider | Request | Response → format |
|---|---|---|
| BetterLyrics ("Apple Music") | `GET https://lyrics-api.boidu.dev/getLyrics?s=<title>&a=<artist>&d=<durationSec>&al=<album>` | TTML → LRC (`TTMLParser.kt`: words → `<word:start:end>` lines, `{agent:}`, `{bg}`) |
| YouLyPlus | `GET {base}/v2/lyrics/get?title=&artist=&duration=&album=&id=&isrc=`; bases `https://lyricsplus.prjktla.my.id`, `https://lyricsplus.binimum.org` | syllable `<mm:ss.mmm>` + `{bg}` / `{agent:}` markers |
| Paxsenix | `GET https://lyrics.paxsenix.org/apple-music/search?q=` then `/apple-music/lyrics?id=` | TTML; match scoring: duration ±2 s = +100 etc. |
| LrcRed | `GET https://lrc.red` search (TTML) | TTML |
| SimpMusic | `GET https://api-lyrics.simpmusic.org/v1/{videoId}`; fallback `https://musicfy-yt-music-server.onrender.com/v1/` | richSyncLyrics > syncedLyrics > plainLyrics; duration filter ±10 s |
| LrcLib | `GET https://lrclib.net/api/search?track_name=&artist_name=&album_name=` or `?q=`; also `/api/get` | UA `Mochy (+https://github.com/realidkroo/Mochy-Music)`; `bestMatchingForRelaxed` |
| Deezer | `GET https://api.deezer.com/search` → `POST https://pipe.deezer.com/api` (GraphQL); token: `GET https://auth.deezer.com/login/anonymous` (JWT, 5 min lifetime, renewed 30 s early) | word timings → joined with CJK spacing rules |
| NetEase | `GET https://music.163.com/api/search/get` → `GET https://music.163.com/api/song/lyric` | YRC word-sync parse |
| QQMusic | `POST https://u.y.qq.com/cgi-bin/musicu.fcg` (`music.search`, `music.musichallSong.PlayLyricInfo`) | timed lines |
| Kugou | `GET https://mobileservice.kugou.com/api/v3/search/song?version=9108&plat=0&pagesize=8&showtype=0&keyword=...` → `GET https://lyrics.kugou.com/search?ver=1&man=yes&client=pc&duration=<ms|hash=...>` → `GET https://lyrics.kugou.com/download?fmt=lrc&charset=utf8&client=pc&ver=1&id=&accesskey=` | base64 `content`; duration tolerance **8 s**; strip head/tail up to **30 lines** of `.+].+[:：].+` credit lines; keep lines matching `^\[(\d\d):(\d\d)\.(\d{2,3})\]` |
| YouTubeMusic | `next` → lyrics browse endpoint | rich/line LRC |
| YouTubeSubtitle | `get_transcript` (base64 params `"\n{11ch}{videoId}"`) | caption cues |
| YouTubeCaptions | player captions (clients `ANDROID_VR_NO_AUTH`, `IOS`, `WEB_REMIX`) → `timedText?fmt=json3` | json3 word cues |
| Musixmatch (module) | `https://apic.musixmatch.com/ws/1.1/` with `usertoken` (`token.get`; all-zero token = captcha), `app_id=android-player-v1.0`, length slack 3 s | rank + pick |
| Genius (metadata) | `https://genius.com/api` (UA `Mochy (...)`), match threshold 60 | about-song/credits, not timed |
| Wikipedia (metadata) | `https://www.wikidata.org/w/api.php` then `https://{lang}.wikipedia.org/w/api.php?action=query&prop=extracts&exintro=1&explaintext=1` | about text |
| AppleCanvas (canvas) | token scraped from `https://music.apple.com/us/browse` HTML; `GET https://itunes.apple.com/search`; `https://amp-api.music.apple.com` | animated canvas covers; cache 24 h |

### 3.3 Resolution & scoring (`LyricsHelper.kt`)

- Provider order persisted in DataStore `lyricsProviderOrder` (JSON); single preferred provider `lyricsProvider` (default `YOULYPLUS`).
- All eligible providers are queried **in parallel, each with a 35 s timeout**.
- `pickBest` scoring: `ScriptWeight = 10000`, `SyncWeight = 1000`, `DuetWeight = 100`, then provider-order index as tiebreak.
- Chinese-market tiering: KuGou/NetEase/QQMusic form their own tier; `LanguageReferenceProviders = {YouTubeMusic, YouTubeSubtitle}` provide the reference language.
- Providers whose output is syllable-split are demoted (`isSyllableSplit`).

### 3.4 LRC dialect (`LyricsUtils.kt`)

| Element | Format |
|---|---|
| Line time | `[mm:ss.mmm]` — 2-3 fractional digits |
| Word timing | next line: `<text:start:end|...>` — offsets in **seconds** |
| Rich sync | inline `<mm:ss.cc>word` |
| Voice tags | `{agent:X}`; background vocals: `{bg}` |
| All lines share a timestamp | ⇒ demote to `SyncKind.PLAIN` |

`SyncKind ∈ {NONE, PLAIN, LINE, WORD}`; `Script` enum + `dominantScript` used for CJK scoring; unsynced lines get `UntimedLineDurationMs = 5000`; display applies `LineLeadInMs = 300` (line highlights this early). Word-timing repair: `fixWordSpacing` / spaces-dropped / syllable merge (CJK spacing, dropped trailing spaces).

### 3.5 Storage & extras

- Room table `lyrics`: `id` (= videoId) PK, `lyrics TEXT`, `provider TEXT default 'Unknown'`, `translatedLyrics TEXT default ''`, `translationLanguage TEXT default ''`, `translationMode TEXT default ''`. Sentinel `"LYRICS_NOT_FOUND"`.
- Per-song: `song.lyricsOffset` (Int, ms — positive shows lines earlier), `song.romanizeLyrics` (Bool, default true).
- Romanization switches per language (Japanese, Korean, Chinese, Russian, Ukrainian, Serbian, Bulgarian, Belarusian, Kyrgyz, Macedonian, Hindi, Punjabi) + `lyricsRomanizeAsMain`, `lyricsRomanizeCyrillicByLine`.
- Translation: OpenRouter (`openRouterApiKey/baseUrl/model`, `aiProvider`) or DeepL (`deeplApiKey`, `deeplFormality`); `translateMode`, `translateLanguage`, `translateSourceLanguage`; YouTube caption translation (`youtubeCaptionTranslation`).
- Video alignment: `VideoLyricsSync` runs 1.5 s after a music video starts playing (`VideoLyricsSyncDelayMs = 1_500`); `CaptionAligner` matches lyric lines to caption cues (2-gram for CJK, else 3-gram; requires ≥ `MinAnchors` anchors and `MinMatchedShare` of usable lines), producing a `SongVideoMap`.
- Embedded lyrics: local files' LRC/SYLT read by `EmbeddedLyrics.kt`.
---

## 4. Audio engine

Files: `playback/{MusicService.kt, PlayerConnection.kt, CrossmixDirector.kt, TransportTransitions.kt, audio/*}`, `eq/{AudioLabStore.kt, AudioFxService.kt, EqualizerService.kt, audio/*, autoeq/*, data/*}`, `crossmix/*`.

### 4.1 Player & queue (`MusicService.kt`, `PlayerConnection.kt`)

- One foreground service (`MusicService : MediaLibraryService`) owns one main `ExoPlayer` (+ a secondary player only for crossmix). UI binds via `PlayerConnection`.
- Queue = player timeline windows; `queueWindows`/`currentWindowIndex` published from one callback as an atomic `queueSnapshot`. Progress UI polls at **66 ms** (`progressState` callbackFlow loop).
- Shuffle: `DefaultShuffleOrder` (recomputed on reorder in shuffled mode); Repeat: `REPEAT_MODE_OFF/ONE/ALL` persisted (`RepeatModeKey` Int, `ShuffleModeKey` Bool).
- Previous-vs-restart threshold: `currentPosition > 3000 ms` → restart song.
- Persistent queue + `PersistentShuffleAcrossQueuesKey`; infinite queue appends radio results (`infiniteSource`, used/failed seed sets, runway retry with growing delay); `DisableLoadMoreWhenRepeatAllKey` stops appends when repeat-all is on; `queueGeneration` counter invalidates in-flight loads when the queue is replaced.
- Stream failure: failed stream counts down and auto-skips (`StreamFailure(startedAt, timeoutMs)`, pref `streamFailureTimeout`, `autoSkipNextOnError`, `dontSkipOnFailure`).
- Position polling loop sleeps at most `PositionWakeUs = 100_000` µs; speed/pitch drags are applied at most every `SpeedApplyGapMs = 60 ms`.
- Skip-silence: `SkipSilenceKey` + instant mode (`SilenceDetectorAudioProcessor`), applied to both players.

### 4.2 "Audio Lab" persistence (`AudioLabStore.kt`)

SharedPreferences file **`audio_lab`**, JSON values, debounced save **400 ms**:

| Key | Shape (defaults) |
|---|---|
| `eq` | `EqSettings{enabled, nodes[ParametricEQBand], preamp, autoPreamp=true, presetId, correction(ParametricEQ?), correctionEnabled}` |
| `reverb` | `ReverbSettings{enabled=false, mix=0.5, roomSize=0.55, damping=0.45, wet=0.6, dry=1, width=1, preDelayMs=12, lowCutHz=180, presetId="room"}` |
| `loudness` | `LoudnessSettings{boostDb=4, compression=0.3}` |
| `spatial` | `SpatialSettings{crossfeed(cutoffHz=700, feedDb=4.5), speakerAngleDeg=30, surround, mono}` |
| `playback` | `PlaybackSettings{speed 0.1–8, preservePitch, pitchSemitones}` |
| `volume` | `VolumeSettings{exponent=2, limit=1}` |
| `normalize` | `NormalizeSettings{mode=ReplayGainMode.Track, preampDb=3}` |
| `presets` | `List<EqPreset{id, name, nodes, preamp}>` (user presets `user_<millis>`; legacy imports `legacy_*`) |

Legacy migration: reads SharedPreferences `nanosonic_eq_profiles` once (flag `migrated_legacy_eq`), imports active profile + sorted profiles (skipping id `musicfy_custom_eq`).

### 4.3 Equalizer (`data/ParametricEQ.kt`, `BiquadFilter.kt`, `CustomEqualizerAudioProcessor.kt`)

- Parametric EQ, **MAX_BANDS = 20**; band = `{frequency Hz, gain dB, q (default 1.41), filterType, enabled}`; `FilterType ∈ {PK (peaking), LSC (low-shelf), HSC (high-shelf), LPQ, HPQ}` — RBJ cookbook biquads.
- UI clamps gain to **±12 dB** (`MaxGainDb = 12.0`); response curve plotted log-spaced **20–20000 Hz**.
- `design(settings)`: audible bands = own enabled nodes + correction filters (when `correctionEnabled`); preamp = `settings.preamp + headroom`, where headroom = **auto preamp** when `autoPreamp` (see §4.5), else correction's preamp when correction is on, else 0. Empty bands + 0 preamp ⇒ straight wire (design null).
- New designs **crossfade in** (no reset/re-seek); the peak limiter is always in the chain because toggling its delay would skip audio.

### 4.4 Built-in 10-band presets (`EqPresets` in `AudioLabStore.kt`)

Band frequencies (Hz): **31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000**, all `Q = 1.41`, filter `PK` (`OctaveQ = 1.41`). Gains in dB:

| id | name | 31 | 62 | 125 | 250 | 500 | 1k | 2k | 4k | 8k | 16k |
|---|---|---|---|---|---|---|---|---|---|---|---|
| flat | Flat | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| acoustic | Acoustic | 4.5 | 4.5 | 3.5 | 1.0 | 1.5 | 1.5 | 3.0 | 3.5 | 3.0 | 1.5 |
| bass_boost | Bass Booster | 5.5 | 4.5 | 3.5 | 2.5 | 1.2 | 0 | 0 | 0 | 0 | 0 |
| bass_cut | Bass Reducer | −5.5 | −4.5 | −3.5 | −2.5 | −1.2 | 0 | 0 | 0 | 0 | 0 |
| classical | Classical | 4.5 | 3.5 | 3.0 | 2.5 | −1.5 | −1.5 | 0 | 2.0 | 3.0 | 3.5 |
| dance | Dance | 3.5 | 6.5 | 5.0 | 0 | 2.0 | 3.5 | 5.0 | 4.5 | 3.5 | 0 |
| deep | Deep | 5.0 | 3.5 | 1.5 | 1.0 | 3.0 | 2.5 | 1.5 | −2.0 | −3.5 | −4.5 |
| electronic | Electronic | 4.5 | 4.0 | 1.5 | 0 | −2.0 | 2.0 | 1.0 | 1.5 | 4.5 | 5.0 |
| hiphop | Hip-Hop | 5.0 | 4.0 | 1.5 | 3.0 | −1.0 | −1.0 | 1.5 | −0.5 | 2.0 | 3.0 |
| jazz | Jazz | 4.0 | 3.0 | 1.5 | 2.0 | −1.5 | −1.5 | 0 | 1.5 | 3.0 | 3.5 |
| loudness | Loudness | 6.0 | 4.0 | 0 | 0 | −2.0 | 0 | −1.0 | −5.0 | 5.0 | 1.0 |
| lounge | Lounge | −3.0 | −1.5 | −0.5 | 1.5 | 4.0 | 2.5 | 0 | −1.5 | 2.0 | 1.0 |
| piano | Piano | 3.0 | 2.0 | 0 | 2.5 | 3.0 | 1.5 | 3.5 | 4.5 | 3.0 | 3.5 |
| pop | Pop | −1.5 | −1.0 | 0 | 2.0 | 4.0 | 4.0 | 2.0 | 0 | −1.0 | −1.5 |
| rnb | R&B | 2.5 | 7.0 | 5.5 | 1.5 | −2.5 | −1.5 | 2.5 | 3.0 | 3.0 | 3.5 |
| rock | Rock | 5.0 | 4.0 | 3.0 | 1.5 | −0.5 | −1.0 | 0.5 | 2.5 | 3.5 | 4.5 |
| small_speakers | Small Speakers | 5.5 | 4.0 | 3.5 | 2.5 | 1.0 | 0 | −1.0 | −2.0 | −3.5 | −4.0 |
| spoken | Spoken Word | −3.5 | −0.5 | 0 | 0.5 | 3.5 | 4.5 | 5.0 | 4.0 | 2.5 | 0 |
| treble_boost | Treble Booster | 0 | 0 | 0 | 0 | 0 | 1.0 | 2.5 | 3.5 | 4.5 | 5.5 |
| treble_cut | Treble Reducer | 0 | 0 | 0 | 0 | 0 | −1.0 | −2.5 | −3.5 | −4.5 | −5.5 |
| vocal | Vocal Booster | −1.5 | −3.0 | −3.0 | 1.5 | 3.5 | 3.5 | 3.0 | 1.5 | 0 | −1.5 |

### 4.5 AutoEq + auto preamp (`autoeq/AutoEqRepository.kt`, `AutoEqFitter.kt`, `PeakLimiter.kt`)

- AutoEq database: `https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results` — `INDEX.md` cached **7 days** (`IndexMaxAgeMs = 7 * 24 * 3600 * 1000`); per-headphone files fetched on pick and kept for good; UA `Mochy`.
- Import format: **AutoEq `ParametricEQ.txt`** (`ParametricEQParser.kt`): `Preamp: X dB` + `Filter N: ON PK Fc <Hz> Gain <dB> Q <Q>`.
- Fitter: 10 bands default (`BandChoices = {5, 8, 10, 15}`), `SampleRate = 48_000`, `MaxBoostDb = 8.0`, `MaxCutDb = 12.0`, optimize = 160 iterations with decay `1.0 - 0.6*t/160`; preamp rounded to `round(-maxBoost*10)/10`.
- Auto preamp headroom: response overshoot beyond `boostRoomDb(f) = 4.0 + 3.0 * max(0, log2(f/1000))` dB (limiter takes bass better than treble); headroom = `-max(over)`.
- **PeakLimiter** (always-on, `PeakLimiter.kt`): lookahead **5 ms**, hold **25 ms** (one 40 Hz cycle), exponential release **150 ms** (`releaseStep = 1 - exp(-1/(0.15*sr))`), ceiling **1.0**, settling `1e-6`; minimum-gain over sliding window via monotonic queue; held gains smoothed by window average; a 16-bit PCM delay path avoids double rounding; resting state = pure delay.

### 4.6 DSP processors and FX chain

Chain (per player, 16-bit PCM in Media3): decode → **SincResampler** (hi-fi resampler) → **EQ (CustomEqualizerAudioProcessor, includes limiter)** → **SilenceDetector** → **CrossmixTransition** → **Reverb** → **Spatial** → **Loudness** (last "so it also catches the reverb's peaks") → **ReplayGain** → **VolumeCurve** at the sink. Speed/pitch are applied at the audio track (sonic), NOT through processors.

| Processor | Key constants |
|---|---|
| `SincResampler` | polyphase kaiser-windowed sinc; stopband **120 dB**; base taps **192** (scaled up for downsampling); passband = min-nyquist − kaiser transition; `MaxCoefficients = 200_000` caps design; float in/out; tables cached per (in,out) pair |
| `LoudnessAudioProcessor` | `LoudnessParams{enabled, boostDb=4 (clamp 0–12), compression=0.3 (0–1)}`; threshold `−10 − 24·c` dB; slope `1 − 1/(1+3c)`; makeup `= −threshold·slope·0.5`; attack 8 ms, release 180 ms; stereo-linked envelope per **32-frame block**; limiter ceiling **0.944 (−0.5 dBFS)**, soft knee at **0.95** via tanh; enable/disable ramps over **0.15 s**; `loudnessReductionDb` for the meter |
| `ReplayGain.kt` | reads Vorbis comments `REPLAYGAIN_TRACK_GAIN/PEAK`, `REPLAYGAIN_ALBUM_GAIN/PEAK`; mode Track default, preampDb **3**; `factor = 10^((gain+preamp)/20)`, capped by `1/peak`, clamped to **[0.03, 1]**; never boosts past full scale; number regex `[-+]?\d+(?:\.\d+)?` |
| `VolumeCurve` | `gain(volume) = limit · v^exponent` when enabled; exponent clamped **1–5** (default 2), limit clamped **0.05–1** (default 1); disabled = linear |
| `SpatialAudioProcessor` | crossfeed: cutoff **700 Hz**, feed **4.5 dB**; virtual speakers angle **30°**, speaker gain **0.6**, surround gain **0.45**; Brown & Duda spherical head: radius **0.0875 m**, speed of sound **343**, `AlphaMin=0.1`, `ThetaMin=150°`; mono sums then re-spreads (mono ≈ −6.02 + 3.01 dB) |
| `ReverbAudioProcessor` | Freeverb-style: params from §4.2; presets **Room(0.5,0.55,0.45,0.6,1,12ms,180Hz), Studio(0.4,0.3,0.65,0.45,0.8,5,250), Bathroom(0.5,0.42,0.05,0.7,0.6,2,120), Plate(0.45,0.65,0.1,0.55,1,0,300), Hall(0.5,0.78,0.35,0.6,1,25,150), Cathedral(0.55,0.95,0.2,0.7,1,40,120), Stadium(0.6,0.9,0.5,0.65,1,60,200), Dreamy(0.7,0.93,0.6,0.85,1,30,300)** (mix, roomSize, damping, wet, width, preDelayMs, lowCutHz) |

### 4.7 Speed / pitch / per-song tuning

- `PlaybackSettings.speed` **0.1–8.0**, `preservePitch`, `pitchSemitones`; applied via sonic on the audio track so processor chains are not flushed.
- Per-song overrides (`SongTuning.kt`, "a song with its own speed and pitch plays at those, every other song at audio lab's").
- Musical-video sync: `speed = playbackParameters.speed` used to map wall time → media time.

### 4.8 Transitions: crossfade / crossmix (`crossmix/*`, `CrossmixDirector.kt`)

- `CrossmixMode ∈ {AUTO_CROSSFADE, OFF, MANUAL_CROSSFADE, CROSSMIX}`; default **CROSSMIX** (`DefaultTransitionMode`); legacy crossfade booleans (`crossfadeEnabled`, `crossfadeDuration` default **5000 ms**, `crossfadeGapless=true`) feed `transitionModeOf(prefs)`: saved mode wins, else legacy switch ⇒ MANUAL_CROSSFADE, else CROSSMIX.
- Planner knobs: `CrossmixLength{SHORT=0.6, MEDIUM=1.0, LONG=1.5}` overlap scale; `CrossmixTempoMatch{OFF=0.006, GENTLE=0.04, FULL=0.10}` max speed-up; style `AUTO` or a named `CrossmixStyle`; `stunts=true` (backspins/tape stops/risers when energy jumps); `alwaysMix=true` (never a plain fade, never a drop at the end); learnt `crossmixStartLatencyMs`.
- Transport fades: `TransportFadeEnabledKey` (on by default), `TransportFadeSeconds` default **2 s**, range **0.5–10 s**; `TransportEffect ∈ {OFF, ECHO, REWIND, TAPE_STOP, FAR_AWAY, REVERB, FILTER}`.
- Skip blend: `SkipBlendKey`, `SkipBlendAhead` 1–4 (`SkipBlendAheadMax = 4`; data saver pins it to 1); the next songs' starts are pre-fetched so blended skips begin instantly.

### 4.9 Downloads (`DownloadUtil.kt`, `DownloadExporter.kt`, `OggOpusRemux.kt`)

- `DownloadQuality{HIGH "The best YouTube has: ~160 kbps Opus", MEDIUM "Around 128 kbps, half the space", LOW "The smallest files, ~50–70 kbps"}`; used as `downloadQuality.youTubeQuality` for the stream request. Per-provider failure order is remembered per song; the starting itag is pinned (`downloadItags`).
- Saved audio stays as-is; only YouTube's webm/opus is remuxed losslessly into a plain **.opus** container (players don't see .webm as music).
- Exporter (`mochy-preserve` folder, covers 1200 px): music/playlists/liked sub-folders, lyrics sidecars; `MochyCsvExporter.kt` writes the app's own CSV.

---

## 5. Data model & storage

Files: `db/{MusicDatabase.kt, DatabaseDao.kt, entities/*, daos/SpeedDialDao.kt}`.

### 5.1 SQLite database

File **`mochy.db`**, **schema version 43** (`MusicDatabaseVersion = 43`), Room, **WAL**, `busy_timeout=60000`, `cache_size=-16000`, `wal_autocheckpoint=1000`, `synchronous=NORMAL`, 4-thread query/transaction executors. Desktop port should keep the same table/column names (the importer/exporter and future migrations depend on them).

**Tables (23)** — PKs and FKs in parentheses:

| Table | Columns (beyond PK/FK) |
|---|---|
| `song` | PK `id`; title, duration(Int −1), thumbnailUrl, albumId, albumName, explicit(0), year, date, dateModified, liked(0), likedDate, totalPlayTime, inLibrary, dateDownload, isLocal(0), libraryAddToken, libraryRemoveToken, lyricsOffset(0), romanizeLyrics(1), isDownloaded(0), isUploaded(0), isVideo(0), localUri, addedByLike(0); index on albumId |
| `artist` | PK `id` (`LA`+8 random letters); name, thumbnailUrl, channelId, lastUpdateTime, bookmarkedAt, isLocal(0) |
| `album` | PK `id`; playlistId, title, year, thumbnailUrl, themeColor, songCount, duration, explicit, lastUpdateTime, bookmarkedAt, likedDate, inLibrary, description, isLocal, isUploaded |
| `playlist` | PK `id` (`LP`+8 letters); name, browseId, createdAt, lastUpdateTime, isEditable(1), bookmarkedAt, remoteSongCount, playEndpointParams, thumbnailUrl, shuffleEndpointParams, radioEndpointParams, isLocal(0), isAutoSync(0), description |
| `song_artist_map` | PK (songId, artistId); position |
| `song_album_map` | PK (songId, albumId); `index` |
| `album_artist_map` | PK (albumId, artistId); `order` |
| `playlist_song_map` | PK auto; playlistId→playlist CASCADE, songId→song CASCADE; position; setVideoId; indexes both |
| `search_history` | PK auto; `query` UNIQUE |
| `format` | PK `id` (=songId); itag, mimeType, codecs, bitrate, sampleRate, contentLength, loudnessDb, perceptualLoudnessDb, playbackUrl |
| `lyrics` | PK `id` (=videoId); lyrics, provider('Unknown'), translatedLyrics(''), translationLanguage(''), translationMode('') |
| `event` | PK auto; songId→song CASCADE; timestamp, playTime |
| `related_song_map` | PK auto; songId→song, relatedSongId→song (both CASCADE) |
| `set_video_id` | PK `videoId`; setVideoId |
| `playCount` | PK (song, year, month); count |
| `recognition_history` | PK auto; trackId(idx); title, artist, album, coverArtUrl, coverArtHqUrl, genre, releaseDate, label, shazamUrl, appleMusicUrl, spotifyUrl, isrc, youtubeVideoId, recognizedAt, liked |
| `speed_dial_item` | PK `id`; secondaryId, title, subtitle, thumbnailUrl, type ∈ {SONG, ALBUM, ARTIST, PLAYLIST}, explicit, createDate |
| `playlist_event` | PK auto; playlistId→playlist CASCADE; timestamp |
| `imported_song` | PK (songId, source); importedAt — **no FK** (deliberate) |
| `import_session` | PK auto; source, startedAt (`DEVICE_SOURCE = "device"`) |
| `import_session_song` | PK (sessionId, songId); addedToLibrary, liked |
| `imported_playlist` | PK `playlistId`; source(idx), sessionId, importedAt, remoteId, remoteUrl, lastSyncedAt |
| `imported_playlist_track` | PK (playlistId, trackKey) |
| `song_edit` | PK `songId`; title, artists(JSON), album, year, cover, videoId, noVideo(0) |

**Views**: `sorted_song_artist_map`, `sorted_song_album_map` (ordered by position), `playlist_song_map_preview` (`position <= 3`).

Special playlist ids: `LP_LIKED` (liked), `LP_DOWNLOADED` (downloads). Like-toggle semantics: liking also adds to library with `addedByLike=1` and calls `YouTube.likeVideo`; `toggleLibrary(syncToYouTube)` calls `YouTube.toggleSongLibrary`.

### 5.2 In-memory queue model

`MediaMetadata{id, title, artists[{id?, name}], duration, thumbnailUrl, album{id,title}?, setVideoId?, musicVideoType? (ATV = audio; others = video), explicit, liked, likedDate, inLibrary, libraryAddToken, libraryRemoveToken, suggestedBy, localUri}` (`models/MediaMetadata.kt`); `SongEdits` overlays `song_edit` rows at display time.

### 5.3 Settings (Jetpack DataStore `Preferences`)

One Preferences DataStore, **350+ keys** (full list in `constants/PreferenceKeys.kt`; grouped by prefix). Important groups with exact key names:

| Group | Keys (type) |
|---|---|
| Identity/session | `visitorData` (string), `dataSyncId`, `innerTubeCookie`, `accountName`, `accountEmail`, `accountChannelHandle`, `useLoginForBrowse` |
| Appearance | `dynamicTheme` (bool), `selectedThemeColor` (int ARGB), `darkMode` (string: AUTO/ON/OFF), `pureBlack`, `pureBlackMiniPlayer`, `thumbnailCornerRadius` (float), `cropAlbumArt`, `gridItemSize`, `sliderStyle`, `squigglySlider`, `animationSpeed` (float), `density_scale_factor`, `custom_density_scale_value`, `disableBlur`, `blurQuality`, `chromeGlassStyle`, `enableProgressiveBlur`, `elementBlur`, `blurStrengthPx` |
| Layout | `homeStyle` (MOCHYY/LISTEN_NOW/HOP_IN), `playlistStyle` (FLOW/CLASSIC/MINIMAL), `playerStyle` (11 styles, §7.3), `useNewPlayerDesign`, `useNewMiniPlayerDesign`, `appleMusicDarkChrome`, `slimNavBar`, `defaultOpenTab` |
| Playback | `audioQuality` (AUTO/LOW/MEDIUM/HIGH/LOSSLESS/HI_RES_LOSSLESS/DOLBY_ATMOS), `downloadQuality` (HIGH/MEDIUM/LOW), `streamBackend` (INNERTUBEX/CLASSIC), `ipVersion` (AUTO/IPV4/IPV6), `dataSaver`, `dataSaverMobileOnly`, `audioNormalization`, `skipSilence`, `skipSilenceInstant`, `crossfadeEnabled`, `crossfadeDuration` (float), `crossfadeGapless`, `crossmixMode`, `crossmixLength`, `crossmixTempoMatch`, `crossmixStyle`, `crossmixStunts`, `crossmixAlwaysMix`, `crossmixStartLatencyMs` (float), `transportFadeEnabled`, `transportFadeSeconds`, `transportEffect`, `skipBlend`, `skipBlendAhead`, `persistentQueue`, `rememberShuffleAndRepeat`, `shuffleMode`, `repeatMode` (int), `playerVolume` (float), `infiniteQueue`, `autoLoadMore`, `preventDuplicateTracksInQueue`, `shufflePlaylistFirst`, `similarContent`, `forcePlayback`, `studioForVideos`, `videoQuality`, `disableVideoAutoplay`, `playVideoBackground` |
| Failover | `dontSkipOnFailure`, `streamFailureTimeout` (int), `autoSkipNextOnError`, `stopMusicOnTaskClear`, `stopPlaybackOnTaskRemoved` |
| Lyrics | `lyricsProvider`, `lyricsProviderOrder`, per-provider enables (§3.1), `showLyrics`, `swipeLyrics`, `carLyrics`, `floatingLyrics(+X/Y)`, `lyricsTextPosition` (LEFT/CENTER/RIGHT), `lyricsClick`, `lyricsWaveAnimation`, `lyricsHighBloom`, `lyricsScrollKey`, `lyricsGlowEffect`, `appleMusicLyricsBlur`, `lyricsStandardBlur`, `lyricsMotionStyle` (MOTION/LEGACY/BORING/PLAIN), `lyricsTextSize` (float), `lyricsLineSpacing` (float), `translateLyrics`, `deeplApiKey`, `deeplFormality`, `openRouterApiKey`, `aiProvider`, `openRouterBaseUrl`, `openRouterModel`, `translateMode`, `translateLanguage`, `translateSourceLanguage`, `youtubeCaptionTranslation`, `syncLyricsToVideo`, 12 `lyricsRomanize*` booleans |
| Audio hardware | `enableOffload`, `preferUsbDac`, `usbDacPassthrough`, `hiResFloatOutput`, `hiFiResampler`, `audioPassMode`, `audioPassFixedVolume`, `audioPassRestore` (JSON), `mixWithOtherApps`, `resumeOnBluetoothConnect`, `pauseOnMute`, `keepScreenOn` |
| Sync | `ytmSync`, `selectedYtmPlaylists`, `ytSyncNewPlaylists`, `ytReportPlays`, `ytPullHistory`, `ytHistoryLastSeen`, `ytSyncLikedSongs`, `ytSyncSourcesTagged`, `ytExcludeImports`, `ytSyncHold`, `last_like_song_sync`/`last_library_song_sync`/`last_album_sync`/`last_artist_sync`/`last_playlist_sync`/`last_full_sync` (long, epoch ms), `SYNC_COOLDOWN = 30 * 60 s` |
| Discord | `discordAccessToken/RefreshToken/TokenExpiresAt`, `discordProfile` (JSON), `discordLoginState/Verifier`, `discordRPCEnable`, 15 presence-formatting keys (§8.1) |
| Scrobbling | `lastfmSession`, `lastfmUsername`, `lastfmScrobblingEnable`, `librefm*` mirror, `lastfmUseNowPlaying`, `lastfmUseSendLikes`, `scrobbleDelayPercent` (float), `scrobbleMinSongDuration` (int), `scrobbleDelaySeconds` (int) |
| Sorts/filters | `songSortType/Descending`, `playlistSongSortType/Descending`, `autoPlaylistSongSortType/Descending`, `likedSongSortType/Descending`, `artistSortType/Descending`, `albumSortType/Descending`, `playlistSortType/Descending`, `mixSortType/Descending`, `songFilter`, `artistFilter`, `albumFilter`, `albumsFromLikedSongs`, view types `artistViewType/albumViewType/playlistViewType` |
| Library content | `hideExplicit`, `hideVideoSongs`, `hideYoutubeShorts`, `showArtistDescription`, `showArtistSubscriberCount`, `showMonthlyListeners`, `showArtistVideo`, `showArtistBackgroundVideo`, `pauseListenHistory`, `pauseSearchHistory` |
| Caches | `maxImageCacheSize` (int), `maxSongCacheSize` (int) |
| Proxy | `proxyEnabled`, `proxyUrl`, `proxyType`, `proxyUsername`, `proxyPassword` |
| Backend extras | `enableCustomApi`, `enableMonochromeBackend`, `enableMonochromePlaybackBackend`, `monochromePlaybackApiUrl/Token`, `monochromeTurnstileJwt/Expiry`, `monochromeFallbackToYouTube`, `amazonMusicApiUrl`, `tidalToAsinUrl`, `turnstileSiteKey/BypassToken`, `deezerFallbackEnabled/Url`, `apiInstances`, `enable_amazon_music_backend`, `amazon_music_instances_list`, `streamingInstances`, `qobuzInstances`, `monochromeInstances` |
| Onboarding/profile | `isFirstRun`, `setupCompleted`, `setupImportCompleted`, `username`, `profilePicUri`, `profileCardNumber` ("USR#n"), `profileJoinedEpochDay` |
| Misc | `appLanguage`, `contentLanguage`, `contentCountry`, `suggestionRegion`, `searchSource` (LOCAL/ONLINE), `developerMode`, `offlineMode`, `disableAiFilter`, `showStreamSourceToast`, `lyricsCacheWipe` (int, generation 1) |
---

## 6. Importer (Phase 2)

Files: `importer/*`, `importer/account/*`, `importer/transfer/*`.

### 6.1 Sources

| Source | Transport | Notes |
|---|---|---|
| Spotify | Web-player endpoints first (token captured in browser); Web API `https://api.spotify.com/v1` as fallback — web-player token is rate-limited, so Web API waits `WEB_API_AFTER_MS = 30_000` for the web player first; 429 ⇒ retry after 2–30 s | `/me` (identity), `/me/tracks?limit=50` (liked = pseudo-playlist `spotify:liked`), `/me/playlists?limit=50`, `/playlists/{id}/tracks?limit=100` (older API: `/items`); JSON paths `items[].track.{name,artists[].name,album.name,duration_ms,id}`, `entry.added_at` |
| Apple Music | session-capture importer (`AppleMusicAccount.kt`) | media-user token + storefront cookies |
| Tidal | `TidalAccount.kt` (+ `tidalToAsinUrl` bridge) | playlists/likes via account API |
| YouTube Music | `YouTubeMusicAccount.kt`, sync managed by `YouTubeSyncManager.kt` | §5.3 sync keys; likes/library/playlists/history pull; `YtSyncHold` after undo |
| CSV | `CsvImport.kt` (+ app's own format `MochyCsvExporter.kt`) | title/artist/album/duration columns |
| Device files | `DeviceMusicScanner.kt`, `LocalAudioImport.kt`, `LocalImportService.kt` | source key `device` |
| Links | `LinkImporter.kt` (share links of services) | |
| Transfer archive | `TransferArchive/Export/Import/Restore.kt` | full library backup/restore |

### 6.2 Track matching (`TrackMatcher.kt`) — port the algorithm exactly

Search query = cleaned title + first artist. Title cleaning: bracketed noise removed by regex `[([]... (feat|ft|with|remaster|remastered|version|edit|mono|stereo|deluxe|explicit|clean|bonus|from|official|audio|video|lyrics?|visualizer|hd|hq|4k) ...[))]`; dash suffix ` - (remaster|version|edit|...)$`; feat tail. Text normalize: NFKD → strip marks → lowercase → non-alnum → space. Artist split on `, & ; / x and feat ft featuring with`.

Score = `title·0.5 + artist·0.25 + duration·0.25 + albumScore + uploadScore − versionPenalty`:

| Term | Rule |
|---|---|
| title | `similarity = max(jaccard(tokens), levenshtein(compact), containment=0.8)`; below `MIN_TITLE = 0.45` ⇒ 0 |
| artist | max pairwise similarity (0.5 if none); title-contained artist bump to 0.8 |
| duration | Δ≤3 s ⇒ 1.0; ≤8 ⇒ 0.6; ≤20 ⇒ 0.1; ≤60 ⇒ −0.3; else −1.0 (missing ⇒ 0.5) |
| album | same album (≥0.8 similarity) ⇒ +0.05; candidate on live album source isn't ⇒ −0.3 |
| upload | `MUSIC_VIDEO_TYPE_ATV` ⇒ +0.05; `UGC` ⇒ −0.08; album present ⇒ +0.05 |
| version penalty | each mismatched word of {remix, live, instrumental, inst, acoustic, karaoke, cover, sped up, slowed, nightcore, reverb, 8d, stage, dance practice, practice, performance, fancam, concert, rehearsal, reaction, 1 hour, hour, loop, extended, japanese, korean, english, chinese} ⇒ −0.3 each |

Accept thresholds: `SONG_ACCEPT = 0.62`, `VIDEO_ACCEPT = 0.70`, `SONG_FALLBACK_ACCEPT = 0.50`. Order: songs first; duration-near (≤3 s) candidates preferred within the same pass; videos only if songs fail; final fallback songs at 0.50. Search retries twice with 800 ms delay; per-import cache keyed `normTitle|normArtist|durationMs/5000`.

### 6.3 Undo / re-sync bookkeeping

Every import writes an `import_session` + per-song `import_session_song` (only what it changed is undone). Playlists record `remoteId/remoteUrl/lastSyncedAt` for Re-sync, and `imported_playlist_track` lists already-matched tracks so Re-sync only adds new ones.

---

## 7. UI / design (Compose → egui mapping)

### 7.1 Navigation

Bottom tabs (`Screens.kt`, `MainScreens`): **Home** (`route "home"`), **Search** (`"search_input"`), **Library** (`"library"`), **Settings** (`"settings"`). `defaultOpenTab` pref; double-tapping the Search tab re-focuses the field. Nested routes include album/artist/playlist/lyrics/queue/settings pages, plus modals (menus, sheets).

### 7.2 Home (`HomeScreen.kt`)

Sections (ids): `recently_played`, `most_played`, `history`, `dont_forget_these_songs`, `account_playlists`, `speed_dial`, `from_the_community`, `artist_list`, `all_time_hits`, `live_shows`, `music_videos`, `category_{id}`, `home_page_section_{i}_{title}`. Styles (`homeStyle`): **Mochyy / Listen now / Hop in**. Card rows cascade-fade in on first reveal (≤4 items staggered 60 ms apart); grid prefetch strategies tuned (4 nested cards / 6 grid rows).

### 7.3 Player styles (`PlayerStyle` enum)

FLAGSHIP, CLASSIC, TURNTABLE, VIDEO (full video), IPOD, POSTER, TIMELINE, LYRICS (lyrics stage), BEDSIDE (clock), BEDSIDE_TILES, MODULE. Properties: `editable` = FLAGSHIP/CLASSIC/TURNTABLE/POSTER/IPOD/bedside; `flagshipFamily` = FLAGSHIP/CLASSIC/TURNTABLE; `hasFullEditor` = FLAGSHIP/CLASSIC; `turnable` = TURNTABLE/POSTER/TIMELINE/LYRICS/MODULE/bedside; `availableOnTv` = FLAGSHIP/POSTER/VIDEO/TIMELINE/bedside. Selector columns: `[FLAGSHIP, CLASSIC], [TURNTABLE], [VIDEO], [IPOD], [BEDSIDE_TILES, BEDSIDE], [POSTER], [TIMELINE, LYRICS]`. Stored-name migration: `SIMPLE/SIMPLE_COMPACT→TIMELINE`, `LYRICS_ABSTRACT→LYRICS`, `DEFAULT→FLAGSHIP`. The desktop MVP should implement the **Flagship** look (frosted cover-fade background, dynamic accent) and defer the others.

### 7.4 Theme & color

- `MochyTheme`: **dark-only**; optional `pureBlack`. Dark scheme: primary `#E0E0E0`, onPrimary black, primaryContainer `#333333`, secondary `#B0B0B0`, secondaryContainer `#2C2C2C`, surfaceVariant `#1A1A1A`, background/surface black, text white. (Light values exist but are unused.)
- Accent: default `DefaultThemeColor = 0xFF8E8E8E`; user override stored as ARGB int in `selectedThemeColor`; **dynamic accent** = `Bitmap.extractThemeColor()` — AndroidX `Palette` `maximumColorCount = 8` then `Score.score` (dominant-swatch scoring); gradients use `extractGradientColors()` (64 swatches, Score anchored to `0xFF4285F4`, top-2 by luminance; fallback `#595959`/`#0D0D0D`). Desktop: implement Palette+Score equivalent (k-means bins on argb → weighted scoring) or approximate with a fixed palette.
- Fonts: **Inter** (`Font.kt InterFontFamily`); a decorative family (`bbh_bartle`) for special headings. Material 3 typography in `Type.kt`.
- Library tab pills: track `#161618`, chip `#232326`, selected `#3D3D41`, 3 dp track padding, 4 dp gaps. Settings search bubbles: `#1C1C1E` / selected `#3A3A3E`.

### 7.5 Screens list (settings index, `MochySettingsScreen.kt`)

Categories: **Customisation, App behaviour, Audio Lab, Modules, Integrations, Storage, Import & sync, Experimental**. Notable pages: Cover rounding; Playlist styles; Player styles (+ "Customize player" studio); Home styles; Blur/Progressive blur/quality/strength/element blur; Animation (+speed); Lyrics animation (Style 1/2/Boring/Plain); Gestures (swipe left/right actions, player swipe-down-anywhere); Larp mode (fake hi-res badge easter egg); App language; Offline mode; Data saver; Proxy; Albums from liked songs; Keep playing when closed; Play alongside other apps; Pause when muted; Save player's last state; Local song auto metadata; Keep shuffle on; music-video toggles; Skip silence; format badge + sample rate; **Audio Lab** (Transition/Blend skips/Equalizer/AutoEQ/Audio Quality/Audio Pass/USB DAC/Hi-res output/Hi-fi resampler/Reverb/Loudness/Normalization/Spatial/Dolby Atmos/Playback speed/Exponential volume); **Modules** (Apple Music, Monochrome, Provider order); **Integrations** (Discord, Last.fm, Libre.fm, YouTube Music sync); **Storage** (music cache limit, manage/remove downloads); **Import & sync**; **Experimental** (device integration/TV remote, Big disc styles, Music haptics, Cipher, Playback diagnostics, Repeat initial setup, Show where songs play from, Force playback).

### 7.6 Onboarding & extras

Setup wizard (`setup/SetupWizardScreen.kt`, `WelcomeStep.kt`): welcome → import ("What to sync", providers) → completion; `setupCompleted`, `setupImportCompleted` gates. Extras to expect: Speed dial pins (capped by `SpeedDialDao.MaxPinned`, oldest evicted), profile card (USR# + join date), Recap ("26 recap", monthly, `enable26Recap`/`forceMonthlyRecap`), Wrapped card, widgets, TV/car modes, floating lyrics window, bedside clock.

### 7.7 Lyrics display

Motion styles: **Style 1 (MOTION)** words rise as sung + held notes swell/glow + lines cascade (AGSL shader), **Style 2 (LEGACY)** wave across sung letters, **BORING** = Style 1 without shader, **PLAIN** = sweep only. Legacy animation names (FADE/GLOW/SLIDE/KARAOKE/APPLE/APPLE_V2/MOCHY_1/LYRICS_V2/METRO_LYRICS) exist only for migration. Options: blur (`appleMusicLyricsBlur`, `lyricsStandardBlur`), glow/bloom, click-to-seek, wave, text position left/center/right, font size + line spacing floats. Desktop Phase 2 karaoke = Style 1 word-rise using word-level timings from §3.4.

---

## 8. Integrations

### 8.1 Discord rich presence (`discord/*`)

- **OAuth PKCE, no client secret**: `GET https://discord.com/oauth2/authorize?client_id=<BuildConfig.DISCORD_CLIENT_ID>&response_type=code&redirect_uri=discord-<ClientId>:/authorize/callback&scope=openid identify sdk.social_layer_presence&state=<24>&code_challenge=<S256(64-char verifier)>&code_challenge_method=S256`. Redirect handled by a deep link; state checked; token exchanged at `POST https://discord.com/api/v10/oauth2/token` (`grant_type=authorization_code`). Refresh runs **10 min before expiry** (`RenewEarlyMs = 600_000`); revocation supported; profile via `/users/@me` saved as JSON.
- **Gateway**: `wss://gateway.discord.gg/?v=9&encoding=json`, identify with token + properties + capabilities `1<<4|1<<5|1<<12|1<<16`, intents `(1<<12)|(1<<18)|(1<<19)|(1<<22)|...`; opcodes: 0 dispatch (READY), 1 heartbeat, 2 identify, 3 **PRESENCE_UPDATE**, 7 reconnect, 9 invalid session, 10 hello, 11 heartbeat ack; close **4004** = auth failed; ready timeout **20 s**; connection is short-lived — closing the app clears the status naturally; no auto-reconnect (owner reopens).
- Presence payload: `{type, details, details_url, state, timestamps{start,end} (±2500 ms slack), assets{large/small image+text}, buttons:[label], button_urls:[url]}`. Formatting templates (activity name/details/state/large/small text+image) stored as DataStore keys; `discordPresenceRefreshSeconds`, spin artwork (`discordPresenceSpinSeconds/Fps`, custom GIF encoder), custom uploaded images, `discordPresenceClearOnPause` (status drops on pause instead of freezing).

### 8.2 Last.fm / Libre.fm scrobbling (`providers/lastfm/Audioscrobbler.kt`, `scrobble/*`)

- Audioscrobbler 2.0: POST form to `apiRoot` (`https://ws.audioscrobbler.com/2.0/`, Libre.fm mirror), `method`, `api_key`, `sk`, params, `api_sig = md5(sorted "k=v" concat + secret)`, `format=json` (not signed). Mobile session: `auth.getMobileSession` with `password` (last.fm) or `authToken = md5(lower(user)+md5(password))` (libre.fm). Methods: `track.updateNowPlaying`, `track.scrobble` (batch ≤ **50**, `artist[i]/track[i]/album[i]/duration[i]/timestamp[i]`), `track.love`/`track.unlove`.
- Error codes: 4 wrong login, 6/7 permanent reject, 9 signed out; temporary = {−1 (server), 8, 11, 16, 29}; timeouts 15/30/30 s; UA `Mochy`.
- App rules: `scrobbleDelayPercent` (float), `scrobbleMinSongDuration` (int), `scrobbleDelaySeconds` (int); Now Playing + love-sync to every signed-in service; session keys in DataStore (`lastfmSession`, `librefmSession`).

### 8.3 Shazam recognition (`providers/shazamkit/*`, `recognition/*`)

- Audio fingerprint built by **VibraSignature** (port of the librespot-org/vibra algorithm — peaks → constellation → hash pairs; pure Rust port is feasible); POST recognize JSON with signature + `sampleDurationMs`.
- Client rules: `MAX_CONCURRENT_REQUESTS = 2`, `MIN_REQUEST_INTERVAL_MS = 1000`, `MAX_RETRIES = 3` (initial delay 2000 ms), result cache **300 s**, queue cap **50**; Dalvik-style UAs and a timezone list; timeouts 30 s.
- Results stored in `recognition_history` (all fields incl. `youtubeVideoId` and store links); like/queue actions from history.

---

## 9. Port plan

### 9.1 Proposed crates

| Crate | Covers |
|---|---|
| `ytm` | §2: InnerTube client, client walk, stream resolver, search/browse/next parsing, auth (SAPISIDHASH, cookies), visitor minting |
| `lyrics` | §3: providers, LRC dialect, scoring, cache |
| `dsp` | §4: biquads, peak limiter, loudness, replaygain, spatial, reverb, volume curve, sinc resampler, crossmix planner |
| `db` | §5: rusqlite schema identical to Room v43, migrations |
| `library` | local files (symphonia tags), artwork, embedded lyrics |
| `importer` | §6 (Phase 2) |
| `integrations` | §8 (Phase 2): discord gateway, audioscrobbler, shazam |
| `app` | egui shell: 4-tab nav, player, queue, settings, theme engine, hotkeys, MPRIS/SMTC |

### 9.2 Android → Rust substitution table

| Android | Rust |
|---|---|
| Media3 ExoPlayer | `symphonia` (decode) + `cpal`/`rodio` output; custom `StreamSource` for ranged HTTP |
| `AudioProcessor` chain | sample-rate-agnostic f32 pipeline in the cpal callback; keep §4.6 order |
| Room | `rusqlite` (same schema/names, WAL on) |
| DataStore Preferences | flat JSON/TOML settings file; keep key names (§5.3) for import compat |
| Palette/Score | port `maximumColorCount=8` k-means + `Score.score` weighted ranking |
| Coil image cache | `image` + LRU disk cache |
| Ktor/OkHttp | `reqwest` (+cookies) |
| kotlinx-serialization | `serde_json` |
| MediaSession | **MPRIS** (dbus) / **SMTC** (windows crate) — Phase 2 |
| YouTube sync timers | tokio tasks with the same cooldowns (30 min) |

### 9.3 Milestones

1. **M0 (2–3 wk)**: settings store + theme/accent engine + DB schema + nav shell.
2. **M1 (4–6 wk)**: `ytm` search + player responses + InnerTubeX/classic stream pipeline + validation; queue/shuffle/repeat/infinite queue; local files.
3. **M2 (3–4 wk)**: lyrics MVP (line sync, provider order, cache, offset), EQ (10-band + 21 presets + preamp + limiter), loudness/replaygain/volume/speed, persistence.
4. **M3 (2–3 wk)**: polish — downloads (.opus remux), MPRIS/SMTC, global hotkeys, importer.
5. **Phase 2 backlog**: word-sync karaoke, FLAC module, Discord/LFM/Shazam, bedside/fancy player styles, canvas, recap.

### 9.4 Risks

- **PoToken / n-param churn**: YouTube client versions drift (they are already bumped in-source: WEB `2.20260708.00.00`, WEB_REMIX `1.20260707.12.00`). Make clients configurable at runtime; keep the VISIONOS-first walk and NewPipe fallback.
- **Ranged-URL windows**: untokened URLs die at ~786 KB; validation must probe last byte before play.
- **Lyrics servers**: two of the 13 (YouLyPlus bases, SimpMusic fallback) are community hosts; implement graceful degradation + the 35 s parallel timeout.
- **DSP parity**: the peak limiter always runs in-chain; removing it changes latency. Match lookahead (5 ms) and hold (25 ms) exactly, or EQ-loudness behavior diverges.
- **Schema drift**: keep table/column names; `imported_song` intentionally has **no FK**.

---

## QUICK REFERENCE

| Thing | Value |
|---|---|
| API base | `https://music.youtube.com/youtubei/v1/` |
| Main clients | VISIONOS (101, `1.04`, no PoToken, first) → WEB_REMIX (67, `1.20260707.12.00`) → TVHTML5_SAMSUNG / WEB_EMBEDDED_PLAYER / TVHTML5_DOWNGRADED (age-gate) → NewPipe decipher fallback |
| InnerTubeX backend | first attempt, 12 s give-up; quality MEDIUM ⇒ MP4 AAC 128k |
| Search filter params | SONG `EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D`, VIDEO `EgWKAQIQAWoKEAkQChAFEAMQBA%3D%3D`, ALBUM `EgWKAQIYAWoKEAkQChAFEAMQBA%3D%3D`, ARTIST `EgWKAQIgAWoKEAkQChAFEAMQBA%3D%3D`, FEATURED_PLAYLIST `EgeKAQQoADgBagwQDhAKEAMQBRAJEAQ%3D`, COMMUNITY_PLAYLIST `EgeKAQQoAEABagoQAxAEEAoQCRAF`; charts `FEmusic_charts` + `ggMGCgQIgAQ%3D` |
| Queue cap | `MAX_GET_QUEUE_SIZE = 1000` |
| Stream validation | ranged GET at `contentLength−1`; untokened probe at byte `1_048_576`; untokened window ≈ 786 KB (28–32 s) |
| URL fixups | n-param transform (`EjsNTransformSolver`), `pot=` append, per-client headers |
| Bot guard | VISIONOS rests 30 min; `MAX_STAND_INS = 2` |
| FLAC module | `tracks.monochrome.st/search/tracks?q=&limit=8`; relay `/search?q=&n=` + `/s/{id}?q=flac` + `X-Relay-Client: relay/2` |
| Lyrics priority (defaults) | BetterLyrics("Apple Music"), YouLyPlus, Paxsenix, LrcRed, SimpMusic, LrcLib, Deezer, YouTubeSubtitle, YouTubeMusic, NetEase, QQMusic, Kugou, YouTubeCaptions |
| Lyrics fetch | parallel, 35 s timeout each; pickBest: Script 10000 / Sync 1000 / Duet 100 / order tiebreak |
| LRC dialect | `[mm:ss.mmm]` + `<word:start:end>` (seconds) next line; `{agent:X}`, `{bg}`; rich `<mm:ss.cc>word`; LineLeadIn 300 ms; untimed 5000 ms; same-timestamp ⇒ PLAIN |
| Lyrics store | table `lyrics{id=videoId, lyrics, provider, translatedLyrics, translationLanguage, translationMode}`; sentinel `LYRICS_NOT_FOUND`; per-song `lyricsOffset` ms |
| EQ | parametric, ≤20 bands, PK/LSC/HSC/LPQ/HPQ, ±12 dB UI, 10-band presets @ 31–16 kHz Q=1.41 (21 built-ins, §4.4), preamp + auto-headroom (`4 + 3·log2(f/1k)` dB) |
| Limiter | lookahead 5 ms / hold 25 ms / release 150 ms / ceiling 1.0, always on |
| Loudness | boost 4 dB (≤12), compression 0.3, ceiling −0.5 dBFS, knee 0.95, attack 8 ms / release 180 ms |
| ReplayGain | Track mode, preamp 3 dB, factor clamp [0.03, 1], capped by 1/peak |
| Volume curve | `gain = limit · v^exponent` (exp 2, limit 1) |
| Speed | 0.1–8.0, sonic on audio track, per-song tuning overrides, 60 ms apply gap |
| Resampler | kaiser sinc, 120 dB stopband, 192 taps base, ≤200k coefficients |
| Transitions | CROSSMIX default; lengths SHORT/MEDIUM/LONG = 0.6/1.0/1.5; tempo match ≤0.10; transport fade 2 s (0.5–10); skip blend 1–4 ahead |
| DB | `mochy.db` v43, WAL, 23 tables + 3 views; ids `LA…`/`LP…`; liked `LP_LIKED` |
| Downloads | HIGH ~160 kbps Opus / MEDIUM ~128 / LOW 50–70; webm→.opus remux |
| Importer match | score = title .5 + artist .25 + duration .25 ± album/upload/version; accepts 0.62/0.70/0.50; duration near ≤3 s |
| Discord | PKCE OAuth (`openid identify sdk.social_layer_presence`), gateway `wss://gateway.discord.gg/?v=9&encoding=json`, presence op 3, renew 10 min early |
| Last.fm | Audioscrobbler 2.0, md5-sorted-sig + secret, scrobble ≤50/req, temp errors {−1,8,11,16,29} |
| Shazam | Vibra signature, ≤2 concurrent, ≥1 s spacing, cache 300 s |
| Theme | dark-only; accent default `#8E8E8E`; dynamic accent = Palette(8) + Score; gradients Palette(64) |
| Player styles | FLAGSHIP (MVP), CLASSIC, TURNTABLE, VIDEO, IPOD, POSTER, TIMELINE, LYRICS, BEDSIDE, BEDSIDE_TILES, MODULE |

*End of specification.*

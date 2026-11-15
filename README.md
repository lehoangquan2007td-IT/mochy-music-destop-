# Mochy Music — Desktop (Rust)

Desktop port of [Mochy Music](https://github.com/realidkroo/mochy-music) (Android,
GPL-3.0) written in **pure Rust** (no Node/Electron): `egui` GUI + `cpal`/`rodio`
audio engine + `symphonia` decode + `reqwest` networking.

> Status: WIP — MVP in progress. See `docs/PORT_SPEC.md` for the full port
> specification extracted from the Android sources.

## Crates

| Crate | Role |
|---|---|
| `mochy-core` | Shared models, settings, EQ definitions (21 built-in presets), ids |
| `mochy-audio` | Playback engine: symphonia decode, 10-band biquad EQ, always-on peak limiter, sinc resampler, speed |
| `mochy-innertube` | YouTube Music InnerTube client (search/browse/queue/stream walk, VISIONOS-first) |
| `mochy-lyrics` | Lyrics providers (LrcLib, BetterLyrics, YouLyPlus, SimpMusic, …) + LRC dialect |
| `mochy-storage` | SQLite (Room-v43-compatible schema), settings, cache paths |
| `mochy-app` | egui desktop app |

## Build

```powershell
# Windows (MSYS2 gcc toolchain, no Visual Studio needed)
$env:Path = "$env:USERPROFILE\.cargo\bin;C:\msys64\ucrt64\bin;C:\Program Files\CMake\bin;$env:Path"
cargo build --release
./target/release/mochy-app.exe
```

License: GPL-3.0-or-later (inherited from Mochy Music). Not affiliated with
YouTube/Google; streams are fetched from third-party services on request.

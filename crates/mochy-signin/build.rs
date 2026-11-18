//! Put `WebView2Loader.dll` next to the built exe (`target/<profile>/`).
//!
//! On the GNU toolchain `webview2-com-sys` links the loader dynamically, so
//! `mochy-signin.exe` needs the DLL beside it. The copy here keeps
//! `cargo run` / dev builds working; release packaging ships the same file
//! (`x64/WebView2Loader.dll`, Microsoft WebView2 SDK redistributable).

fn main() {
    println!("cargo:rerun-if-changed=x64/WebView2Loader.dll");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Ok(out) = std::env::var("OUT_DIR") else { return };
    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out
    let Some(profile_dir) = std::path::Path::new(&out).ancestors().nth(3) else { return };
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("x64").join("WebView2Loader.dll");
    let dst = profile_dir.join("WebView2Loader.dll");
    if let Err(e) = std::fs::copy(&src, &dst) {
        println!("cargo:warning=could not copy WebView2Loader.dll to {}: {e}", dst.display());
    }
}

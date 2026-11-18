//! On Windows, copy the FFmpeg DLLs next to the built executable so that
//! `cargo run` and the `target\release\liteview.exe` binary work without
//! touching PATH. Uses the same `FFMPEG_DIR` that ffmpeg-sys-next reads.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    println!("cargo:rerun-if-changed=build.rs");

    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let video = std::env::var_os("CARGO_FEATURE_VIDEO").is_some();
    let Some(ffmpeg_dir) = std::env::var_os("FFMPEG_DIR") else { return };
    if !windows || !video {
        return;
    }
    // OUT_DIR is target/<profile>/build/<pkg>-<hash>/out; the exe lives in target/<profile>.
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let Some(exe_dir) = out_dir.ancestors().nth(3) else { return };
    let Ok(entries) = std::fs::read_dir(Path::new(&ffmpeg_dir).join("bin")) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) {
            let _ = std::fs::copy(&path, exe_dir.join(entry.file_name()));
        }
    }
}

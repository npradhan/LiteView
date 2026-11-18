//! On Windows, copy the FFmpeg DLLs next to the built executable so that
//! `cargo run` and the `target\release\liteview.exe` binary work without
//! touching PATH. Uses the same `FFMPEG_DIR` that ffmpeg-sys-next reads.
//! Reports what it did as build warnings, since a missing DLL otherwise only
//! shows up as STATUS_DLL_NOT_FOUND when the program starts.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    println!("cargo:rerun-if-changed=build.rs");

    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let video = std::env::var_os("CARGO_FEATURE_VIDEO").is_some();
    if !windows || !video {
        return;
    }
    let Some(ffmpeg_dir) = std::env::var_os("FFMPEG_DIR") else {
        println!(
            "cargo:warning=FFMPEG_DIR is not set, so the FFmpeg DLLs were not copied next to \
             liteview.exe. Copy them from your FFmpeg bin folder yourself."
        );
        return;
    };
    // OUT_DIR is target/<profile>/build/<pkg>-<hash>/out; the exe lives in target/<profile>.
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let Some(exe_dir) = out_dir.ancestors().nth(3) else {
        return;
    };
    let bin = Path::new(&ffmpeg_dir).join("bin");
    let entries = match std::fs::read_dir(&bin) {
        Ok(e) => e,
        Err(e) => {
            println!(
                "cargo:warning=Could not read the FFmpeg DLL folder {}: {e}",
                bin.display()
            );
            return;
        }
    };
    let mut copied = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) {
            match std::fs::copy(&path, exe_dir.join(entry.file_name())) {
                Ok(_) => copied.push(entry.file_name().to_string_lossy().into_owned()),
                Err(e) => println!(
                    "cargo:warning=Could not copy {} to {}: {e}",
                    path.display(),
                    exe_dir.display()
                ),
            }
        }
    }
    if copied.is_empty() {
        println!(
            "cargo:warning=No DLLs found in {}. Is FFMPEG_DIR pointing at a *shared* FFmpeg build?",
            bin.display()
        );
    } else {
        println!(
            "cargo:warning=Copied {} FFmpeg DLLs to {}: {}",
            copied.len(),
            exe_dir.display(),
            copied.join(", ")
        );
    }
}

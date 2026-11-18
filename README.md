# LiteView

A fast, lightweight full-screen image, comic and video viewer written in Rust (egui/eframe).

## Features

- Starts with a small window with a **Load Folder** button, then switches to full screen
  (or pass a folder / archive / image path on the command line to open it straight away).
- Full-screen viewing, images fitted to the screen.
- Thumbnail column on the left with the file name under each thumbnail; click one to open it.
  Animated GIF, PNG and WebP images are marked with an **A** badge and videos with a ▶ badge.
- Animated GIF, APNG and animated WebP playback.
- Video playback with sound (via FFmpeg), with an auto-hiding seek bar.
- Comic-book archives are opened and their pages listed alongside loose images.
- Background, multi-threaded decoding with neighbour prefetching, so paging is instant.
- Thumbnails load lazily (only those scrolled into view), so huge folders open immediately.
- EXIF orientation of photos and the rotation flag of phone videos are honoured.

## Supported formats

| Type | Extensions |
| --- | --- |
| Images | JPEG (`jpg jpeg jpe jfif`), PNG / APNG, GIF, BMP / DIB, TIFF, WebP, TGA, ICO / CUR, PSD, JPEG XL, PNM (`pbm pgm ppm pnm pam`), DDS, HDR, OpenEXR, QOI, Farbfeld |
| Animated | GIF, APNG, animated WebP |
| Video | MP4 / M4V, MOV, MKV, WebM, AVI, WMV / ASF, FLV / F4V, MPEG (`mpg mpeg mpe m2v`), TS / M2TS / MTS, VOB, 3GP, OGV, DivX, RealMedia, in any codec FFmpeg decodes (H.264, HEVC, AV1, VP8/9, MPEG-4, …) |
| Archives | CBZ / ZIP, CBR / RAR, CB7 / 7Z, CBT / TAR |

The image and archive formats cover what CDisplayEx opens, except PDF, AVIF and HEIC.
Videos are played from loose files only, not from inside archives.

## Controls

| Key | Action |
| --- | --- |
| → / ← | Next / previous image or video |
| Mouse wheel (over image) | Next / previous image or video |
| Home / End | First / last item |
| Page Down / Page Up | Jump 10 items |
| Click a thumbnail | Open that item |
| Tab | Show / hide the thumbnail column |
| F11 | Toggle full screen |
| L (or O), or right-click → Load Folder… | Open another folder |
| Z | Fit-to-width mode for images taller than they are wide (toggle) |
| ↑ / ↓ (hold), mouse wheel | Scroll up and down a tall image in fit-to-width mode |
| Esc | Quit (or close the right-click menu) |

Fit-to-width mode (Z) stays on as you move between images; it only changes images that are taller
than they are wide, and each one starts at the top. In this mode the mouse wheel scrolls the
image instead of changing pages.

While a video is showing:

| Key | Action |
| --- | --- |
| Space, or click the video | Play / pause |
| M | Mute / unmute |
| Shift + → / ← | Jump forward / back 10 seconds |
| Click or drag the seek bar | Jump to that point |
| Right-click | Play / Pause and Mute are also in the menu |

Videos loop: when one ends it starts again from the beginning.
The controls bar appears when you move the mouse and hides after two seconds of playback.

## Building

You need [Rust](https://rustup.rs) and a C++ compiler. The RAR support compiles bundled C++
sources: use the MSVC Build Tools on Windows, the Xcode command-line tools on macOS, and g++ or
clang on Linux.

Video support needs the FFmpeg libraries (version 4 or newer) and `libclang`, which generates the
FFmpeg bindings. If you don't want video, skip the FFmpeg steps and build with
`cargo build --release --no-default-features`.

### Windows

1. Install LLVM (provides libclang):

   ```bash
   winget install LLVM.LLVM
   ```

2. Download a **shared release** FFmpeg build, e.g. `ffmpeg-n7.1-latest-win64-gpl-shared-7.1.zip` from
   <https://github.com/BtbN/FFmpeg-Builds/releases>, and unzip it to, e.g., `C:\ffmpeg`
   (so that `C:\ffmpeg\bin`, `C:\ffmpeg\include` and `C:\ffmpeg\lib` exist).
   Release builds (names starting with `ffmpeg-n`) are recommended over the
   `ffmpeg-master-latest-…` development snapshots.
3. Point the build at both (PowerShell, in the window you'll build from):

   ```bash
   $env:FFMPEG_DIR = "C:\ffmpeg"; $env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
   ```

4. Build:

   ```bash
   cargo build --release
   ```

   If you switch to a different FFmpeg build later, run `cargo clean -p ffmpeg-sys-next` first so
   the FFmpeg bindings are regenerated from the new headers.

   The FFmpeg DLLs are copied next to `target\release\liteview.exe` automatically. To move the
   program elsewhere, copy those DLLs along with the `.exe`.

### macOS

```bash
brew install ffmpeg pkg-config
```

```bash
cargo build --release
```

### Linux (Debian / Ubuntu)

```bash
sudo apt install build-essential pkg-config clang libclang-dev libasound2-dev libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev
```

```bash
cargo build --release
```

## Running

The program is built to `target/release/liteview` (`liteview.exe` on Windows).

```bash
liteview                      # start window with a Load Folder button
liteview ~/Pictures           # opens a folder
liteview ~/Comics/issue1.cbz  # opens a single archive
```

Or run it straight from the source tree with `cargo run --release -- <path>`.

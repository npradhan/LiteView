# LiteView

A fast, lightweight full-screen image and comic viewer written in Rust (egui/eframe).

## Features

- Pick a folder at start-up (or pass a folder / archive / image path on the command line).
- Full-screen viewing, images fitted to the screen.
- Thumbnail column on the left with the file name under each thumbnail; click one to open it.
- Animated GIF, APNG and animated WebP playback.
- Comic-book archives are opened and their pages listed alongside loose images.
- Background, multi-threaded decoding with neighbour prefetching, so paging is instant.
- Thumbnails load lazily (only those scrolled into view), so huge folders open immediately.
- EXIF orientation is honoured.

## Supported formats

| Type | Extensions |
| --- | --- |
| Images | JPEG (`jpg jpeg jpe jfif`), PNG / APNG, GIF, BMP / DIB, TIFF, WebP, TGA, ICO / CUR, PSD, JPEG XL, PNM (`pbm pgm ppm pnm pam`), DDS, HDR, OpenEXR, QOI, Farbfeld |
| Animated | GIF, APNG, animated WebP |
| Archives | CBZ / ZIP, CBR / RAR, CB7 / 7Z, CBT / TAR |

This covers the image and archive formats CDisplayEx opens, except PDF, AVIF and HEIC. Decoding
those needs native libraries (pdfium, dav1d, libheif), and LiteView builds without native libraries.

## Controls

| Key | Action |
| --- | --- |
| → / ← | Next / previous image |
| Mouse wheel (over image) | Next / previous image |
| Home / End | First / last image |
| Page Down / Page Up | Jump 10 images |
| Click a thumbnail | Open that image |
| Tab | Show / hide the thumbnail column |
| F11 | Toggle full screen |
| Right-click → Load Folder… | Open another folder |
| O | Open another folder |
| Esc | Quit |

## Building

```bash
cargo build --release
```

The binary ends up at `target/release/liteview` (`liteview.exe` on Windows). RAR support compiles
the bundled unrar C++ sources, so you need a C++ compiler (MSVC on Windows, g++/clang elsewhere).

```bash
liteview                      # asks for a folder
liteview ~/Pictures           # opens a folder
liteview ~/Comics/issue1.cbz  # opens a single archive
```

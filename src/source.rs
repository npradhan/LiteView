//! Discovery of viewable pages: loose image files in a folder, plus the images
//! inside comic-book / archive files (CBZ/ZIP, CBR/RAR, CB7/7Z, CBT/TAR).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Image file extensions that can be decoded.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "apng", "gif", "bmp", "dib", "tif", "tiff", "webp", "tga",
    "icb", "vda", "vst", "ico", "cur", "psd", "pbm", "pgm", "ppm", "pnm", "pam", "dds", "hdr",
    "exr", "qoi", "ff", "farbfeld", "jxl",
];

/// Video file extensions (played with FFmpeg when built with the `video` feature).
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "qt", "mkv", "webm", "avi", "wmv", "asf", "flv", "f4v", "mpg", "mpeg",
    "mpe", "m2v", "ts", "m2ts", "mts", "vob", "3gp", "3g2", "ogv", "divx", "rm", "rmvb",
];

/// Archive file extensions that are opened and their images listed as pages.
pub const ARCHIVE_EXTENSIONS: &[&str] = &["cbz", "zip", "cbr", "rar", "cb7", "7z", "cbt", "tar"];

fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

pub fn is_image_name(name: &str) -> bool {
    IMAGE_EXTENSIONS.contains(&ext_of(name).as_str())
}

pub fn is_video_name(name: &str) -> bool {
    cfg!(feature = "video") && VIDEO_EXTENSIONS.contains(&ext_of(name).as_str())
}

pub fn is_archive_name(name: &str) -> bool {
    ARCHIVE_EXTENSIONS.contains(&ext_of(name).as_str())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArchiveKind {
    Zip,
    Rar,
    SevenZ,
    Tar,
}

impl ArchiveKind {
    fn from_path(path: &Path) -> Option<Self> {
        match ext_of(&path.to_string_lossy()).as_str() {
            "cbz" | "zip" => Some(Self::Zip),
            "cbr" | "rar" => Some(Self::Rar),
            "cb7" | "7z" => Some(Self::SevenZ),
            "cbt" | "tar" => Some(Self::Tar),
            _ => None,
        }
    }
}

/// An opened archive. Zip archives support cheap random access; the other formats
/// are sequential (often solid), so they are unpacked into memory once, on first use.
pub struct Archive {
    path: PathBuf,
    kind: ArchiveKind,
    zip: Mutex<Option<zip::ZipArchive<BufReader<File>>>>,
    unpacked: OnceLock<Result<HashMap<String, Arc<Vec<u8>>>, String>>,
}

impl Archive {
    fn open(path: &Path) -> Option<(Arc<Archive>, Vec<String>)> {
        let kind = ArchiveKind::from_path(path)?;
        let mut zip_handle = None;
        let names: Vec<String> = match kind {
            ArchiveKind::Zip => {
                let za = zip::ZipArchive::new(BufReader::new(File::open(path).ok()?)).ok()?;
                let names = za
                    .file_names()
                    .filter(|n| !n.ends_with('/'))
                    .map(str::to_owned)
                    .collect();
                zip_handle = Some(za);
                names
            }
            ArchiveKind::Rar => unrar::Archive::new(path)
                .open_for_listing()
                .ok()?
                .filter_map(|e| e.ok())
                .filter(|e| e.is_file())
                .map(|e| e.filename.to_string_lossy().into_owned())
                .collect(),
            ArchiveKind::SevenZ => {
                let reader =
                    sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty()).ok()?;
                reader
                    .archive()
                    .files
                    .iter()
                    .filter(|f| !f.is_directory() && f.has_stream())
                    .map(|f| f.name().to_owned())
                    .collect()
            }
            ArchiveKind::Tar => {
                let mut ar = tar::Archive::new(BufReader::new(File::open(path).ok()?));
                ar.entries()
                    .ok()?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.header().entry_type().is_file())
                    .filter_map(|e| e.path().ok().map(|p| p.to_string_lossy().into_owned()))
                    .collect()
            }
        };
        let mut images: Vec<String> = names
            .into_iter()
            .filter(|n| is_image_name(n) && !is_hidden_entry(n))
            .collect();
        images.sort_by(|a, b| natural_cmp(a, b));
        let archive = Archive {
            path: path.to_owned(),
            kind,
            zip: Mutex::new(zip_handle),
            unpacked: OnceLock::new(),
        };
        Some((Arc::new(archive), images))
    }

    fn read_entry(&self, name: &str) -> Result<Vec<u8>, String> {
        if self.kind == ArchiveKind::Zip {
            let mut guard = self.zip.lock().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                let f = File::open(&self.path).map_err(|e| e.to_string())?;
                *guard = Some(zip::ZipArchive::new(BufReader::new(f)).map_err(|e| e.to_string())?);
            }
            let za = guard.as_mut().unwrap();
            let mut entry = za.by_name(name).map_err(|e| e.to_string())?;
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            return Ok(buf);
        }
        let map = self.unpacked.get_or_init(|| self.unpack_all());
        match map {
            Ok(map) => map
                .get(name)
                .map(|d| d.as_ref().clone())
                .ok_or_else(|| format!("entry '{name}' not found")),
            Err(e) => Err(e.clone()),
        }
    }

    fn unpack_all(&self) -> Result<HashMap<String, Arc<Vec<u8>>>, String> {
        let mut out = HashMap::new();
        match self.kind {
            ArchiveKind::Zip => unreachable!(),
            ArchiveKind::Rar => {
                let mut ar = unrar::Archive::new(&self.path)
                    .open_for_processing()
                    .map_err(|e| e.to_string())?;
                while let Some(header) = ar.read_header().map_err(|e| e.to_string())? {
                    let entry = header.entry();
                    let name = entry.filename.to_string_lossy().into_owned();
                    ar = if entry.is_file() && is_image_name(&name) {
                        let (data, rest) = header.read().map_err(|e| e.to_string())?;
                        out.insert(name, Arc::new(data));
                        rest
                    } else {
                        header.skip().map_err(|e| e.to_string())?
                    };
                }
            }
            ArchiveKind::SevenZ => {
                let mut reader =
                    sevenz_rust::SevenZReader::open(&self.path, sevenz_rust::Password::empty())
                        .map_err(|e| e.to_string())?;
                reader
                    .for_each_entries(|entry, rd| {
                        if !entry.is_directory() && is_image_name(entry.name()) {
                            let mut buf = Vec::with_capacity(entry.size() as usize);
                            rd.read_to_end(&mut buf)?;
                            out.insert(entry.name().to_owned(), Arc::new(buf));
                        } else {
                            // Must drain the stream to advance in solid archives.
                            std::io::copy(rd, &mut std::io::sink())?;
                        }
                        Ok(true)
                    })
                    .map_err(|e| e.to_string())?;
            }
            ArchiveKind::Tar => {
                let f = File::open(&self.path).map_err(|e| e.to_string())?;
                let mut ar = tar::Archive::new(BufReader::new(f));
                for entry in ar.entries().map_err(|e| e.to_string())? {
                    let mut entry = entry.map_err(|e| e.to_string())?;
                    if !entry.header().entry_type().is_file() {
                        continue;
                    }
                    let name = entry
                        .path()
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .into_owned();
                    if is_image_name(&name) {
                        let mut buf = Vec::new();
                        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                        out.insert(name, Arc::new(buf));
                    }
                }
            }
        }
        Ok(out)
    }
}

fn is_hidden_entry(name: &str) -> bool {
    name.split(['/', '\\'])
        .any(|part| part.starts_with('.') || part == "__MACOSX")
}

/// Where a page's bytes come from.
#[derive(Clone)]
pub enum PageSource {
    File(PathBuf),
    ArchiveEntry {
        archive: Arc<Archive>,
        entry: String,
    },
}

/// One viewable image.
#[derive(Clone)]
pub struct Page {
    pub source: PageSource,
    /// Name shown under the thumbnail.
    pub display_name: String,
}

impl Page {
    /// File name used for format detection.
    pub fn file_name(&self) -> &str {
        match &self.source {
            PageSource::File(p) => p.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            PageSource::ArchiveEntry { entry, .. } => entry.as_str(),
        }
    }

    /// Path of a video file (videos are only played from loose files, not archives).
    pub fn video_path(&self) -> Option<&Path> {
        match &self.source {
            PageSource::File(p) if is_video_name(&self.display_name) => Some(p),
            _ => None,
        }
    }

    pub fn read_bytes(&self) -> Result<Vec<u8>, String> {
        match &self.source {
            PageSource::File(p) => std::fs::read(p).map_err(|e| e.to_string()),
            PageSource::ArchiveEntry { archive, entry } => archive.read_entry(entry),
        }
    }
}

/// Collect all pages from a folder (non-recursive), or from a single archive/image file.
pub fn scan(path: &Path) -> Vec<Page> {
    let mut pages = Vec::new();
    if path.is_file() {
        add_file(path, &mut pages);
        return pages;
    }
    let Ok(read_dir) = std::fs::read_dir(path) else {
        return pages;
    };
    let mut files: Vec<PathBuf> = read_dir
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort_by(|a, b| {
        natural_cmp(
            &a.file_name().unwrap_or_default().to_string_lossy(),
            &b.file_name().unwrap_or_default().to_string_lossy(),
        )
    });
    for f in &files {
        add_file(f, &mut pages);
    }
    pages
}

fn add_file(path: &Path, pages: &mut Vec<Page>) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.starts_with('.') {
        return;
    }
    if is_image_name(&name) || is_video_name(&name) {
        pages.push(Page {
            source: PageSource::File(path.to_owned()),
            display_name: name,
        });
    } else if is_archive_name(&name) {
        if let Some((archive, entries)) = Archive::open(path) {
            for entry in entries {
                let short = entry
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&entry)
                    .to_owned();
                pages.push(Page {
                    display_name: format!("{name} › {short}"),
                    source: PageSource::ArchiveEntry {
                        archive: archive.clone(),
                        entry,
                    },
                });
            }
        }
    }
}

/// Natural ("human") ordering: `page2` sorts before `page10`. Case-insensitive.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(|c| c.is_ascii_digit()) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("page2.jpg", "page10.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("Page02", "page2"), Ordering::Equal);
        assert_eq!(natural_cmp("a", "B"), Ordering::Less);
        let mut v = vec!["10.png", "9.png", "1.png", "x.png"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["1.png", "9.png", "10.png", "x.png"]);
    }
}

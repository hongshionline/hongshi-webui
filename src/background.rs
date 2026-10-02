//! The picture the user put behind the interface.
//!
//! ## Where the file is, and where it is not
//!
//! The shell **does not copy it**. `background_path` in the settings file is a
//! reference, and this module is what turns that reference into bytes on every
//! request: the file is read where the user keeps it, and the moment it stops
//! resolving the interface falls back to the artwork built into the binary.
//!
//! That is a deliberate trade against the obvious alternative — copy the file into
//! the client's own folder and own the lifetime of it. Copying would survive the
//! user moving or deleting their picture, and it would also mean a 20 MB wallpaper
//! silently duplicated on disk, a second copy to keep in step, and a client
//! directory that grows every time somebody tries a background they do not keep.
//! A path plus a visible fallback is the behaviour that is easy to explain.
//!
//! ## Why the format is sniffed rather than trusted
//!
//! The extension is a claim, and a `.jpg` that is really an animated GIF would be
//! accepted by it. The first bytes are what the browser will actually decode, so
//! those decide, and a file that is not one of [`Kind::usable`] is refused with a
//! sentence that names what *is* accepted. GIF is the interesting refusal: it is a
//! perfectly good image format and it is **animated**, which a window background
//! cannot be — so it is refused rather than silently showing its first frame.
//!
//! ## The size cap
//!
//! 20 MB, checked here rather than in the interface, because the interface is not
//! what reads the file. A bound also has to exist for a reason that is not about
//! taste: the whole file is held in memory to be served, and the endpoint is
//! reachable by anything on the machine.
//!
//! ## What this module does *not* do
//!
//! It does not decode, resize or re-encode anything. The crate has no dependencies
//! by design, and a background does not need any: blur and crop are the browser's
//! job (`filter` and `background-position`), so the picture that reaches the page is
//! the user's own file, byte for byte.

use std::path::{Path, PathBuf};

/// The largest image the shell will read. See the note in the module docs.
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;

/// How much of a file is enough to identify it.
const HEAD_BYTES: usize = 16;

/// What the first bytes of a file say it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Png,
    Webp,
    Bmp,
    Avif,
    /// A real image format, and animated — see the module docs for why it is refused.
    Gif,
    /// Nothing we recognise.
    Other,
}

impl Kind {
    pub fn content_type(self) -> &'static str {
        match self {
            Kind::Jpeg => "image/jpeg",
            Kind::Png => "image/png",
            Kind::Webp => "image/webp",
            Kind::Bmp => "image/bmp",
            Kind::Avif => "image/avif",
            Kind::Gif => "image/gif",
            Kind::Other => "application/octet-stream",
        }
    }

    /// The formats the interface accepts, named the way the refusal names them.
    pub const ACCEPTED: &'static str = "JPG、PNG、WebP、BMP 或 AVIF";

    /// A short name for the format, for JSON. Not for people — [`Kind::ACCEPTED`] is
    /// what a refusal quotes, and Chinese is the interface's language.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Jpeg => "jpeg",
            Kind::Png => "png",
            Kind::Webp => "webp",
            Kind::Bmp => "bmp",
            Kind::Avif => "avif",
            Kind::Gif => "gif",
            Kind::Other => "unknown",
        }
    }

    pub fn usable(self) -> bool {
        !matches!(self, Kind::Gif | Kind::Other)
    }

    /// The `accept` attribute for the page's file picker.
    pub fn accept_attribute() -> &'static str {
        "image/jpeg,image/png,image/webp,image/bmp,image/avif"
    }
}

/// Why a configured background cannot be used.
///
/// Every variant carries enough to write a sentence the user can act on: "the file
/// is missing" and "the file is a 40 MB TIFF" send somebody to different places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// Nothing is configured — the built-in artwork is the background, which is not
    /// a failure. Kept in the same enum so a caller can answer "why is my picture
    /// not showing" from one match.
    NotSet,
    Relative,
    Missing,
    NotAFile,
    TooLarge(u64),
    Gif,
    Unknown,
    Unreadable(String),
}

impl Problem {
    /// The machine-readable half, for JSON and for tests.
    pub fn state(&self) -> &'static str {
        match self {
            Problem::NotSet => "none",
            Problem::Relative => "relative",
            Problem::Missing => "missing",
            Problem::NotAFile => "not_a_file",
            Problem::TooLarge(_) => "too_large",
            Problem::Gif => "gif",
            Problem::Unknown => "unsupported",
            Problem::Unreadable(_) => "unreadable",
        }
    }

    /// The half a person reads. Chinese, like the interface: this string is shown
    /// in the settings page rather than logged.
    pub fn reason(&self) -> String {
        match self {
            Problem::NotSet => "没有设置自定义背景，正在使用内置背景".to_string(),
            Problem::Relative => {
                "这是一个相对路径。请填完整路径，例如 C:\\Users\\你\\Pictures\\bg.jpg".to_string()
            }
            Problem::Missing => "找不到这个文件，它可能被移动或删除了".to_string(),
            Problem::NotAFile => "这是一个文件夹，不是图片文件".to_string(),
            Problem::TooLarge(bytes) => format!(
                "图片有 {:.1} MB，超过了 20 MB 上限",
                *bytes as f64 / (1024.0 * 1024.0)
            ),
            Problem::Gif => "不支持 GIF：会动的图片不能做背景".to_string(),
            Problem::Unknown => {
                format!("这看起来不是图片文件，支持 {}", Kind::ACCEPTED)
            }
            Problem::Unreadable(err) => format!("读不了这个文件：{err}"),
        }
    }
}

/// A configured background that is there, is an image, and is small enough.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub path: PathBuf,
    pub kind: Kind,
    pub bytes: u64,
    /// Milliseconds since the Unix epoch, for the `?v=` the page appends and for the
    /// `ETag` that lets a reload be answered with a 304 instead of 20 MB.
    pub modified_ms: u64,
}

impl Resolved {
    /// A validator that changes when the file does.
    pub fn etag(&self) -> String {
        format!("\"{}-{}\"", self.modified_ms, self.bytes)
    }
}

/// Identify an image from its first bytes. See the module docs for why not the name.
pub fn sniff(head: &[u8]) -> Kind {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Kind::Jpeg;
    }
    if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Kind::Png;
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Kind::Gif;
    }
    if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        return Kind::Webp;
    }
    if head.starts_with(b"BM") {
        return Kind::Bmp;
    }
    // AVIF is ISO-BMFF: a box type, then a brand. `avis` is the same codec in a
    // sequence, which a still background may as well use.
    if head.len() >= 12 && &head[4..8] == b"ftyp" && (&head[8..12] == b"avif" || &head[8..12] == b"avis")
    {
        return Kind::Avif;
    }
    Kind::Other
}

/// Look at what the user configured and decide whether it is usable.
///
/// The whole answer — the problem as well as the file — comes back as a value, so
/// the two callers that need it (the endpoint that serves the picture and the health
/// endpoint that reports it) cannot disagree about why it is not showing.
pub fn resolve(configured: &str) -> Result<Resolved, Problem> {
    resolve_with_limit(configured, MAX_BYTES)
}

/// [`resolve`] with the cap as a parameter, so the test for the cap does not have to
/// write 20 MB to the disk.
pub fn resolve_with_limit(configured: &str, limit: u64) -> Result<Resolved, Problem> {
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        return Err(Problem::NotSet);
    }
    let path = PathBuf::from(trimmed);
    // A relative path would be resolved against whatever directory the client
    // happened to be started in, which for a double-clicked download is not a
    // directory the user has ever seen. Refused with a sentence rather than guessed.
    if !is_absolute(&path) {
        return Err(Problem::Relative);
    }

    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) => {
            return Err(match err.kind() {
                std::io::ErrorKind::NotFound => Problem::Missing,
                _ => Problem::Unreadable(err.to_string()),
            })
        }
    };
    if !metadata.is_file() {
        return Err(Problem::NotAFile);
    }
    let bytes = metadata.len();
    if bytes > limit {
        return Err(Problem::TooLarge(bytes));
    }

    let head = read_head(&path).map_err(|err| Problem::Unreadable(err.to_string()))?;
    let kind = sniff(&head);
    match kind {
        Kind::Gif => Err(Problem::Gif),
        Kind::Other => Err(Problem::Unknown),
        kind => Ok(Resolved {
            path,
            kind,
            bytes,
            modified_ms: modified_ms(&metadata),
        }),
    }
}

/// Read the file to serve it.
pub fn read(resolved: &Resolved) -> Result<Vec<u8>, Problem> {
    let bytes = std::fs::read(&resolved.path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => Problem::Missing,
        _ => Problem::Unreadable(err.to_string()),
    })?;
    // The file can grow between the check and the read, and the bound is what keeps
    // a hostile or accidental 4 GB file out of memory.
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Problem::TooLarge(bytes.len() as u64));
    }
    Ok(bytes)
}

/// Whether a path is one the user could have given us in full.
fn is_absolute(path: &Path) -> bool {
    if path.is_absolute() {
        return true;
    }
    // Windows: `C:bg.jpg` — a drive-relative path — is not absolute to `Path`, and
    // neither is `\bg.jpg` without a drive. Both are refused by the check above,
    // which is right; the one shape that needs naming is a UNC share, which `Path`
    // does treat as absolute, so nothing more is needed here.
    false
}

fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut head = vec![0u8; HEAD_BYTES];
    let mut filled = 0;
    while filled < HEAD_BYTES {
        match file.read(&mut head[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    head.truncate(filled);
    Ok(head)
}

fn modified_ms(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("hongshi-background-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const PNG_HEAD: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
    const JPEG_HEAD: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1];
    const GIF_HEAD: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00";

    #[test]
    fn every_format_is_identified_by_its_own_bytes() {
        assert_eq!(sniff(PNG_HEAD), Kind::Png);
        assert_eq!(sniff(JPEG_HEAD), Kind::Jpeg);
        assert_eq!(sniff(GIF_HEAD), Kind::Gif);
        assert_eq!(
            sniff(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
            Kind::Webp,
            "RIFF alone is a WAV as easily as a WebP"
        );
        assert_eq!(sniff(b"BM\x36\x00\x00\x00\x00\x00"), Kind::Bmp);
        assert_eq!(sniff(b"\x00\x00\x00\x20ftypavif"), Kind::Avif);
        assert_eq!(sniff(b"RIFF\x24\x00\x00\x00WAVEfmt "), Kind::Other);
        assert_eq!(sniff(b"{\"not\":\"an image\"}"), Kind::Other);
        assert_eq!(sniff(b""), Kind::Other, "an empty file is not an image");
    }

    #[test]
    fn only_the_still_formats_are_usable() {
        assert!(Kind::Png.usable() && Kind::Jpeg.usable() && Kind::Webp.usable());
        assert!(Kind::Bmp.usable() && Kind::Avif.usable());
        assert!(!Kind::Gif.usable(), "an animated background is not a background");
        assert!(!Kind::Other.usable());
    }

    #[test]
    fn a_missing_or_relative_path_is_refused_with_its_own_reason() {
        assert_eq!(resolve("").unwrap_err(), Problem::NotSet);
        assert_eq!(resolve("   ").unwrap_err(), Problem::NotSet);
        assert_eq!(resolve("bg.png").unwrap_err(), Problem::Relative);
        assert_eq!(resolve(r"Pictures\bg.png").unwrap_err(), Problem::Relative);

        let dir = scratch("reasons");
        let missing = dir.join("not-here.png");
        assert_eq!(
            resolve(&missing.display().to_string()).unwrap_err(),
            Problem::Missing
        );
        assert_eq!(
            resolve(&dir.display().to_string()).unwrap_err(),
            Problem::NotAFile,
            "a directory is not a picture"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_real_png_resolves_and_an_oversized_one_does_not() {
        let dir = scratch("resolve");
        let png = dir.join("wall.png");
        let mut body = PNG_HEAD.to_vec();
        body.extend_from_slice(&[0u8; 64]);
        std::fs::write(&png, &body).unwrap();

        let resolved = resolve(&png.display().to_string()).expect("a real png resolves");
        assert_eq!(resolved.kind, Kind::Png);
        assert_eq!(resolved.bytes, body.len() as u64);
        assert_eq!(read(&resolved).unwrap(), body, "served byte for byte");
        assert!(resolved.etag().contains(&body.len().to_string()));

        let too_big = resolve_with_limit(&png.display().to_string(), 16).unwrap_err();
        assert_eq!(too_big, Problem::TooLarge(body.len() as u64));
        assert!(too_big.reason().contains("20 MB") || too_big.reason().contains("MB"));

        let gif = dir.join("anim.gif");
        std::fs::write(&gif, GIF_HEAD).unwrap();
        assert_eq!(
            resolve(&gif.display().to_string()).unwrap_err(),
            Problem::Gif
        );

        // The extension is a claim, and this one lies: a GIF named .png is refused
        // for what it is.
        let liar = dir.join("liar.png");
        std::fs::write(&liar, GIF_HEAD).unwrap();
        assert_eq!(resolve(&liar.display().to_string()).unwrap_err(), Problem::Gif);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_problem_has_a_state_and_a_sentence() {
        let problems = [
            Problem::NotSet,
            Problem::Relative,
            Problem::Missing,
            Problem::NotAFile,
            Problem::TooLarge(21 * 1024 * 1024),
            Problem::Gif,
            Problem::Unknown,
            Problem::Unreadable("access denied".to_string()),
        ];
        for problem in problems {
            assert!(!problem.state().is_empty());
            let reason = problem.reason();
            assert!(reason.chars().count() > 6, "{problem:?} needs a real sentence");
            assert!(
                !reason.contains("Problem"),
                "{reason} leaked a Rust name into the interface"
            );
        }
    }

    #[test]
    fn a_file_that_vanishes_after_the_check_is_reported_missing() {
        // The whole feature is "remember where it is and read it later", so the file
        // going away between the two is not an edge case, it is Tuesday.
        let dir = scratch("vanished");
        let png = dir.join("gone.png");
        std::fs::write(&png, PNG_HEAD).unwrap();
        let resolved = resolve(&png.display().to_string()).unwrap();
        std::fs::remove_file(&png).unwrap();
        assert_eq!(read(&resolved).unwrap_err(), Problem::Missing);
        std::fs::remove_dir_all(&dir).ok();
    }
}

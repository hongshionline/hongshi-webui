//! Static assets, and the HTTP Range handling they need.
//!
//! Two-layer design, deliberately:
//!
//! * **Embedded** — `include_str!` puts the four web files into the executable,
//!   so the shipped single-file binary serves the UI with no external files at
//!   all. This is the whole point of the shell: one download, double-click,
//!   browser opens.
//! * **Runtime override** — if a `web/` directory is found next to the executable
//!   (or in the working directory, or wherever `--web-dir` points), files are
//!   served from disk instead. Editing HTML then only needs F5, not a rebuild,
//!   which is how the pages are meant to be developed.

use std::path::{Path, PathBuf};

/// The files compiled into the binary. Keeping the table explicit (rather than
/// globbing at build time) means a typo in a filename is a compile error.
pub const EMBEDDED: &[(&str, &str, &str)] = &[
    ("/", "text/html; charset=utf-8", include_str!("../web/index.html")),
    ("/index.html", "text/html; charset=utf-8", include_str!("../web/index.html")),
    ("/main.css", "text/css; charset=utf-8", include_str!("../web/main.css")),
    ("/app.css", "text/css; charset=utf-8", include_str!("../web/app.css")),
    ("/icons.svg", "image/svg+xml", include_str!("../web/icons.svg")),
    ("/app.js", "text/javascript; charset=utf-8", include_str!("../web/app.js")),
    ("/pages.js", "text/javascript; charset=utf-8", include_str!("../web/pages.js")),
];

/// Assets that are not text and so are served straight from disk rather than
/// being embedded as a string. Kept apart from [`EMBEDDED`] because
/// `include_bytes!` and `include_str!` cannot share a table.
pub const EMBEDDED_BINARY: &[(&str, &str, &[u8])] = &[
    ("/icon.png", "image/png", include_bytes!("../web/icon.png")),
    // 首页的游戏宣传图。Discord 上的这张 key art 是 648×648 的正方形，而卡片槽是
    // 一块宽广告位，所以切线在构建前就做完了（裁掉纯天空的上半、把 logo 单独贴回
    // 左上角），仓库里存的是成品：一张 1152×649 的主图。
    //
    // 另有 640×360 的缩略图和 864×864 的原图，都在 `artwork/`：没有任何代码请求它们，
    // 而 `web/asset/` 是**浏览器能取到的文件集合**、其中每个文件又都 include_bytes! 进
    // 二进制 —— 一张没人取的文件就是所有用户的下载体积。这条规矩现在是 verify.ps1 里
    // 的一条检查（asset/ 下每个文件都必须被某处引用）。
    //
    // WebP 而不是 JPEG 源文件：同一张图 q82 下 98 KB 对 172 KB，而这个二进制是要
    // 下载两次的（桌面与 Android 各一次）。`content_type_for` 本来就有 image/webp，
    // 三个主流内核从 2020 年起都支持，所以没有留 JPEG 回退的必要。
    (
        "/asset/hero-minecraft.webp",
        "image/webp",
        include_bytes!("../web/asset/hero-minecraft.webp"),
    ),
    // 窗口底图：一张 1920×1080 的低多边形火山场景，作为 `body` 的固定背景铺满整窗。
    //
    // 原图是 216 KB 的 PNG，而它的 alpha 通道**每一个像素都是 255** —— 25% 的体积
    // 花在一个从没用过的通道上。转 WebP 后 16.8 KB，少了 92%，而画面是暗部低对比度的
    // 贴图，压缩损失看不出来。原 PNG 留在 `artwork/` 里做源文件，不进二进制，也不在
    // 被服务的目录里。
    (
        "/asset/lowpoly.webp",
        "image/webp",
        include_bytes!("../web/asset/lowpoly.webp"),
    ),
    // 帮助页《什么是游戏端口》的三张操作截图：ESC 菜单、世界选项里的多人游戏设置、
    // 以及聊天框报出端口的那一行。
    //
    // 这一页是给人「照着做」的，图片必须随二进制走 —— 它和主界面一样只依赖回环，
    // 不能去外网取图。原 PNG（1.5 MB / 0.6 MB / 15.6 MB）不进仓库：前两张是随手截的
    // 屏幕，第三张是 3840×2054 的原生截图，留在仓库里只会让 clone 变慢，而它们随时
    // 可以再截一次。仓库里存的是切好的成品，三张加起来 176 KB。
    (
        "/asset/help-port-menu.webp",
        "image/webp",
        include_bytes!("../web/asset/help-port-menu.webp"),
    ),
    (
        "/asset/help-port-lan.webp",
        "image/webp",
        include_bytes!("../web/asset/help-port-lan.webp"),
    ),
    (
        "/asset/help-port-chat.webp",
        "image/webp",
        include_bytes!("../web/asset/help-port-chat.webp"),
    ),
    // 《朋友怎么加入房间》的三张：主界面（左下角写着版本与模组数）、多人游戏界面、
    // 直接连接输入地址。同样是切好的成品，原 PNG（1.9 / 1.1 / 1.0 MB）不进仓库。
    (
        "/asset/help-join-main.webp",
        "image/webp",
        include_bytes!("../web/asset/help-join-main.webp"),
    ),
    (
        "/asset/help-join-multiplayer.webp",
        "image/webp",
        include_bytes!("../web/asset/help-join-multiplayer.webp"),
    ),
    (
        "/asset/help-join-direct.webp",
        "image/webp",
        include_bytes!("../web/asset/help-join-direct.webp"),
    ),
    // 《朋友连不上怎么办》的六张报错截图，从团队早先那份 `常见问题.pdf`（7 页 PPT）里
    // 抽出来的。尺寸本来就是原图大小（856×512 等），只有 1920×1080 那张缩到了 1280。
    //
    // 它们是 JPEG 再编码的，所以质量取 88 而不是别处的 80：这已经是第二代了，而报错文字
    // 压在花花绿绿的画面上，低质量最先糊掉的就是那几个字（而那几个字正是整页的入口）。
    (
        "/asset/help-trouble-refused.webp",
        "image/webp",
        include_bytes!("../web/asset/help-trouble-refused.webp"),
    ),
    (
        "/asset/help-trouble-lost.webp",
        "image/webp",
        include_bytes!("../web/asset/help-trouble-lost.webp"),
    ),
    (
        "/asset/help-trouble-signature.webp",
        "image/webp",
        include_bytes!("../web/asset/help-trouble-signature.webp"),
    ),
    (
        "/asset/help-trouble-registry.webp",
        "image/webp",
        include_bytes!("../web/asset/help-trouble-registry.webp"),
    ),
    (
        "/asset/help-trouble-auth.webp",
        "image/webp",
        include_bytes!("../web/asset/help-trouble-auth.webp"),
    ),
    (
        "/asset/help-trouble-unknownhost.webp",
        "image/webp",
        include_bytes!("../web/asset/help-trouble-unknownhost.webp"),
    ),
    // The Latin subset of Nunito, a variable 400-700 face, 38 KB. Latin only on
    // purpose: the UI's Chinese comes from the face below, so this never has to be
    // re-cut when the copy changes, and `unicode-range` is not needed because the file
    // simply has no CJK glyphs to shadow.
    (
        "/fonts/nunito-latin.woff2",
        "font/woff2",
        include_bytes!("../web/fonts/nunito-latin.woff2"),
    ),
    // Resource Han Rounded, a rounded CJK face, subset per weight. The full face is
    // 14 MB each — twenty times the rest of the binary — so `scripts/build-cjk-subset.py`
    // cuts them down to the characters this client can put on screen. Regular carries
    // the 3755 common characters as well, because 每日资讯 headlines come from the
    // server and can contain anything; Bold carries only the client's own text, which
    // is the only Chinese this interface ever renders at ≥600.
    (
        "/fonts/han-rounded-regular.woff2",
        "font/woff2",
        include_bytes!("../web/fonts/han-rounded-regular.woff2"),
    ),
    (
        "/fonts/han-rounded-bold.woff2",
        "font/woff2",
        include_bytes!("../web/fonts/han-rounded-bold.woff2"),
    ),
];

/// The page scripts, embedded here as well as served, so a structural check can
/// run at test time against exactly the bytes that ship.
#[cfg(test)]
const SHELL_SCRIPT: &str = include_str!("../web/app.js");
#[cfg(test)]
const PAGE_SCRIPT: &str = include_str!("../web/pages.js");
#[cfg(test)]
const STYLE_SHEET: &str = include_str!("../web/main.css");
#[cfg(test)]
const APP_STYLE_SHEET: &str = include_str!("../web/app.css");

/// One asset resolved to bytes, wherever it came from.
pub struct Asset {
    pub body: Vec<u8>,
    pub content_type: &'static str,
    /// Path on disk, when the bytes came from the runtime layer. Logged so it is
    /// never a mystery which copy of the page the browser is showing.
    pub source: Option<PathBuf>,
}

/// Either layer is capable of answering; this only decides which one does.
pub struct Assets {
    runtime_root: Option<PathBuf>,
}

impl Assets {
    /// `explicit` comes from `--web-dir`. When it is absent the override layer is
    /// looked for next to the executable and then in the working directory.
    pub fn discover(explicit: Option<&Path>) -> Assets {
        let runtime_root = match explicit {
            Some(dir) => {
                if is_whole_directory(dir) {
                    // `--web-dir .` (a plausible typo for `--web-dir web`) would
                    // serve every file in the tree to anyone who can reach the
                    // port, including this crate's own source. The lookup only ever
                    // reads below the root, so there is no escape — but "the root
                    // is the entire project" is not a web root by any reading.
                    crate::log::warn_fields(
                        "refusing a web directory that is the whole tree; pass the directory that holds the pages",
                        &[("path", &dir.display().to_string())],
                    );
                    None
                } else if dir.is_dir() {
                    Some(absolutize(dir))
                } else {
                    crate::log::warn_fields(
                        "web directory does not exist, using embedded assets",
                        &[("path", &dir.display().to_string())],
                    );
                    None
                }
            }
            None => candidate_roots().into_iter().find(|dir| dir.is_dir()),
        };
        Assets { runtime_root }
    }

    pub fn runtime_root(&self) -> Option<&Path> {
        self.runtime_root.as_deref()
    }

    /// Map a request path to bytes.
    ///
    /// Returns `None` for unknown assets *and* for paths that try to walk out of
    /// the web root: there is no reason to distinguish the two to a caller, and
    /// `..` never resolves to anything served from here.
    pub fn lookup(&self, path: &str) -> Option<Asset> {
        if let Some(root) = &self.runtime_root {
            if let Some(asset) = self.lookup_on_disk(root, path) {
                return Some(asset);
            }
        }
        if let Some((_, content_type, body)) = EMBEDDED
            .iter()
            .find(|(route, _, _)| *route == path)
            .map(|(route, content_type, body)| (route, content_type, body.as_bytes()))
        {
            return Some(Asset {
                body: body.to_vec(),
                content_type,
                source: None,
            });
        }
        EMBEDDED_BINARY
            .iter()
            .find(|(route, _, _)| *route == path)
            .map(|(_, content_type, body)| Asset {
                body: body.to_vec(),
                content_type,
                source: None,
            })
    }

    /// The page shell, for a path that is one of the app's own routes.
    ///
    /// The interface uses real paths (`/connect`, `/settings`) so a reload or a
    /// bookmark lands somewhere sensible, and the same document answers all of
    /// them. Only used for paths without a file extension, so a missing
    /// `/main.css` is still an honest 404 rather than a page pretending to be CSS.
    pub fn lookup_shell(&self) -> Option<Asset> {
        self.lookup("/")
    }

    fn lookup_on_disk(&self, root: &Path, path: &str) -> Option<Asset> {
        let relative = safe_relative_path(path)?;
        let file = root.join(&relative);
        let body = std::fs::read(&file).ok()?;
        Some(Asset {
            body,
            content_type: content_type_for(&relative),
            source: Some(file),
        })
    }
}

/// Reject anything that is not a plain relative path.
///
/// This is the only place a request string turns into a filesystem path, so the
/// checks live here rather than being repeated by callers: no absolute paths, no
/// drive letters, no `.`/`..` segments, no empty segments, and no characters that
/// have no business in a web asset name.
fn safe_relative_path(path: &str) -> Option<PathBuf> {
    // `/` is the directory index, and a directory has no filename to serve.
    let path = if path == "/" || path.is_empty() { "/index.html" } else { path };
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let mut relative = PathBuf::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        // Percent signs only appear in a path via double-encoding (`%252e`), since
        // the request path reaches here already decoded. No asset in the table is
        // named with one, so allowing it would only ever help a traversal.
        if segment.contains(['\\', ':', '\0', '%']) {
            return None;
        }
        // Windows device names are not files: `NUL` opens and reads as empty,
        // `CON`/`COM1`/`LPT1` can block a read forever and even consume console
        // input — which, in the double-clicked shell, is the user's keyboard.
        // They are also reachable without any path traversal, because Win32
        // resolves `NUL` anywhere in a path.
        if is_reserved_device_name(segment) {
            return None;
        }
        relative.push(segment);
    }
    Some(relative)
}

/// Whether a path segment is a Win32 reserved device name.
///
/// The name is taken up to the first `.` because `NUL.txt` is also the device,
/// and Win32 strips trailing dots and spaces, so `NUL.` and `CON ` resolve too.
fn is_reserved_device_name(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .trim_end_matches([' ', '.']);
    if stem.eq_ignore_ascii_case("CON")
        || stem.eq_ignore_ascii_case("PRN")
        || stem.eq_ignore_ascii_case("AUX")
        || stem.eq_ignore_ascii_case("NUL")
    {
        return true;
    }
    // COM1-9 and LPT1-9 (COM0/LPT0 are not devices, and the superscript variants
    // are not reachable through percent-decoding of ASCII names).
    for prefix in ["COM", "LPT"] {
        if let Some(digit) = stem.strip_prefix(prefix)
            && digit.len() == 1
            && matches!(digit.as_bytes()[0], b'1'..=b'9')
        {
            return true;
        }
    }
    false
}

fn content_type_for(relative: &Path) -> &'static str {
    match relative
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("mp4") => "video/mp4",
        Some("txt") | Some("md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn absolutize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether `dir` names the whole tree rather than a directory of pages.
///
/// A web root that is a project root, a home directory or a drive is a
/// configuration mistake, not a configuration choice.
fn is_whole_directory(dir: &Path) -> bool {
    let resolved = absolutize(dir);
    let Some(name) = resolved.file_name() else {
        // No final component: a filesystem root, or a bare `.`.
        return true;
    };
    if name.is_empty() {
        return true;
    }
    // A project root is recognisable by what lives in it.
    ["Cargo.toml", "package.json", ".git", "pyproject.toml"]
        .iter()
        .any(|marker| resolved.join(marker).exists())
}

/// `web/` next to the executable first, then `web/` in the working directory.
fn candidate_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.join("web"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd.join("web"));
        // `cargo run` puts the executable in `target/<profile>/`, three levels
        // below the crate root — worth one extra guess while developing.
        roots.push(cwd.join("shell").join("web"));
    }
    roots
}

// ---------------------------------------------------------------------------
// Range requests
// ---------------------------------------------------------------------------

/// A single byte range, already resolved against the asset length.
#[derive(Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

/// What a `Range` header amounts to.
#[derive(Debug, PartialEq, Eq)]
pub enum RangeOutcome {
    /// Serve this slice with `206`.
    Serve(ByteRange),
    /// Answer `416` with `Content-Range: bytes */len`: the unit is right and the
    /// range is inside it, but the bytes asked for do not exist.
    Unsatisfiable,
    /// Ignore the header and answer `200` with the whole file.
    ///
    /// RFC 9110 §14.2 and §14.5.1 require this for a range unit the server does
    /// not support and for a range set it will not honour — refusing them with
    /// `416` is wrong, because `416` means "the bytes you asked for are not
    /// there", not "I do not do that". A multi-range request lands here: it is
    /// legal, this server serves one range at a time, and the whole file is a
    /// legal answer to it.
    Ignore,
}

/// Parse a `Range` header against a known length.
///
/// Only what a browser needs for a `<video>` element is implemented: one range,
/// and the two forms it sends (`bytes=a-b`, `bytes=a-`). Anything else degrades to
/// [`RangeOutcome::Ignore`] rather than to an error, per the RFC rules described
/// there.
pub fn parse_range(header: &str, len: u64) -> RangeOutcome {
    let Some(spec) = header.strip_prefix("bytes=") else {
        // Some other range unit entirely (`items=0-1`): ignore it.
        return RangeOutcome::Ignore;
    };
    let spec = spec.trim();
    if spec.contains(',') {
        // A range set. Legal to ask, and legal to answer with the whole file.
        return RangeOutcome::Ignore;
    }
    if len == 0 {
        // Nothing to be unsatisfied about, and `bytes */0` would be noise.
        return RangeOutcome::Ignore;
    }
    let Some((start, end)) = spec.split_once('-') else {
        return RangeOutcome::Ignore;
    };
    let (start, end) = match (start.trim(), end.trim()) {
        ("", suffix) => {
            // `bytes=-N`: the last N bytes.
            let Ok(n) = suffix.parse::<u64>() else {
                return RangeOutcome::Ignore;
            };
            if n == 0 {
                return RangeOutcome::Unsatisfiable;
            }
            (len.saturating_sub(n), len - 1)
        }
        (first, "") => match first.parse::<u64>() {
            Ok(first) => (first, len - 1),
            Err(_) => return RangeOutcome::Ignore,
        },
        (first, last) => match (first.parse::<u64>(), last.parse::<u64>()) {
            (Ok(first), Ok(last)) => (first, last),
            _ => return RangeOutcome::Ignore,
        },
    };
    if start > end || start >= len {
        return RangeOutcome::Unsatisfiable;
    }
    RangeOutcome::Serve(ByteRange {
        start,
        end: end.min(len - 1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::process::Command;

    fn served(header: &str, len: u64) -> Option<(u64, u64)> {
        match parse_range(header, len) {
            RangeOutcome::Serve(range) => Some((range.start, range.end)),
            _ => None,
        }
    }

    #[test]
    fn ranges_a_browser_actually_sends() {
        assert_eq!(served("bytes=0-", 100), Some((0, 99)));
        assert_eq!(served("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(served("bytes=90-200", 100), Some((90, 99)), "clamped to the end");
        assert_eq!(served("bytes=-10", 100), Some((90, 99)), "suffix range");
    }

    #[test]
    fn unsatisfiable_ranges_are_refused_with_416() {
        assert_eq!(parse_range("bytes=100-", 100), RangeOutcome::Unsatisfiable, "start past the end");
        assert_eq!(parse_range("bytes=5-4", 100), RangeOutcome::Unsatisfiable, "inverted");
        assert_eq!(parse_range("bytes=-0", 100), RangeOutcome::Unsatisfiable, "zero-length suffix");
    }

    #[test]
    fn ranges_this_server_does_not_honour_are_ignored_not_refused() {
        // RFC 9110 §14.2 and §14.5.1: an unsupported range unit, or a range set
        // the server will not honour, is ignored — answered with the whole file —
        // not rejected with 416. 416 means "those bytes do not exist".
        for header in [
            "bytes=0-10,20-30",   // a range set: legal to ask, legal to answer in full
            "items=0-10",         // another unit entirely
            "bytes=abc-def",      // unparsable
            "bytes=0",            // no dash
            "bytes=18446744073709551616-", // overflows u64
            "",
        ] {
            assert_eq!(
                parse_range(header, 100),
                RangeOutcome::Ignore,
                "{header:?} should be ignored, not refused"
            );
        }
        // A zero-length asset has nothing to satisfy, so any range is ignored.
        assert_eq!(parse_range("bytes=0-", 0), RangeOutcome::Ignore);
    }

    #[test]
    fn stray_paths_never_escape_the_web_root() {
        assert_eq!(safe_relative_path("/main.css"), Some(PathBuf::from("main.css")));
        assert_eq!(
            safe_relative_path("/asset/icons/linux.svg"),
            Some(PathBuf::from("asset").join("icons").join("linux.svg"))
        );
        for hostile in [
            "/../secret",
            "/..%2fsecret",
            "/a/../../b",
            "/./a",
            "/a//b",
            "/C:/Windows/win.ini",
            "/a\\b",
            "/%2e%2e/secret",
        ] {
            assert_eq!(safe_relative_path(hostile), None, "{hostile} was accepted");
        }
    }

    #[test]
    fn the_root_path_maps_to_the_directory_index() {
        // `/` is what a browser asks for first, and a directory has no filename:
        // it has to resolve to index.html on disk or the runtime override layer
        // would never serve a page.
        assert_eq!(safe_relative_path("/"), Some(PathBuf::from("index.html")));
        assert_eq!(safe_relative_path("/index.html"), Some(PathBuf::from("index.html")));
    }

    #[test]
    fn a_web_root_that_is_the_whole_tree_is_refused() {
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        // `--web-dir .` from the crate root: a plausible typo for `--web-dir web`,
        // and it would serve Cargo.toml, src/ and target/ to anyone who can reach
        // the port.
        assert!(is_whole_directory(crate_root), "the crate root has a Cargo.toml");
        assert!(!is_whole_directory(&crate_root.join("web")), "web/ is a real web root");
    }

    #[test]
    fn lookup_prefers_disk_then_falls_back_to_embedded() {
        let dir = std::env::temp_dir().join(format!("hongshi-shell-assets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), b"<html>from disk</html>").unwrap();

        let assets = Assets::discover(Some(&dir));
        let index = assets.lookup("/").expect("index");
        assert_eq!(index.body, b"<html>from disk</html>".to_vec());
        assert!(index.source.is_some());

        // Not on disk -> embedded copy still answers.
        let css = assets.lookup("/main.css").expect("main.css");
        assert!(css.source.is_none());
        assert!(!css.body.is_empty());

        assert!(assets.lookup("/nope.html").is_none());
        assert!(assets.lookup("/../Cargo.toml").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn content_types_follow_the_extension() {
        assert_eq!(content_type_for(Path::new("a.html")), "text/html; charset=utf-8");
        assert_eq!(content_type_for(Path::new("a.MP4")), "video/mp4");
        assert_eq!(content_type_for(Path::new("a.woff2")), "font/woff2");
        assert_eq!(content_type_for(Path::new("a.unknown")), "application/octet-stream");
    }

    #[test]
    fn the_shell_page_and_its_scripts_agree_with_each_other() {
        // A cheap structural check on the shipped UI, and one that has earned its
        // place: a lost `+` between two parts of a string concatenation is legal
        // JavaScript (automatic semicolon insertion ends the expression early), so
        // the page silently loses whole sections of markup and still parses. That
        // happened twice while building the interface, and both times every other
        // test passed. This asserts that the HTML defines every id the scripts ask
        // for, and that each script is complete rather than truncated.
        let html = EMBEDDED
            .iter()
            .find(|(route, _, _)| *route == "/")
            .expect("the shell page is embedded")
            .2;

        let script_ids = extract_required_ids(PAGE_SCRIPT);
        assert!(
            script_ids.len() > 5,
            "the id scanner found only {} ids; it is broken, not the page",
            script_ids.len()
        );
        for id in script_ids {
            assert!(
                html.contains(&format!("id=\"{id}\"")) || PAGE_SCRIPT.contains(&format!("#{id}\"")),
                "the interface asks for #{id}, which the page does not define"
            );
        }

        // Both scripts are one IIFE. A truncated concatenation leaves the braces
        // unbalanced and takes the closing `})();` with it, so those are what to
        // assert — not "the file ends with a brace", which is simply not how a
        // JavaScript file ends. Line endings are stripped first: this file is
        // edited on Windows and the two scripts do not currently agree on CRLF.
        for (name, source) in [("app.js", SHELL_SCRIPT), ("pages.js", PAGE_SCRIPT)] {
            let normalized = source.replace("\r\n", "\n");
            let trimmed = normalized.trim_end();
            let tail: String = trimmed.chars().rev().take(8).collect::<String>().chars().rev().collect();
            assert!(
                trimmed.ends_with("})();"),
                "{name} does not end with its IIFE closer; it ends with {tail:?}"
            );
        }

        // Stylesheets are not scripts: what matters is that their braces balance,
        // which a truncated rule takes away.
        for (name, source) in [("main.css", STYLE_SHEET), ("app.css", APP_STYLE_SHEET)] {
            let opens = source.matches('{').count();
            let closes = source.matches('}').count();
            assert_eq!(opens, closes, "{name} has unbalanced braces ({opens} vs {closes})");
        }
        for (name, source) in [("app.js", SHELL_SCRIPT), ("pages.js", PAGE_SCRIPT)] {
            let opens = source.matches('{').count();
            let closes = source.matches('}').count();
            assert_eq!(
                opens, closes,
                "{name} has unbalanced braces ({opens} open, {closes} close), which is what a \
                 lost concatenation operator looks like"
            );
        }
        assert!(
            PAGE_SCRIPT.contains("S.ready()"),
            "pages.js must start the shell once its routes are registered"
        );
        assert!(
            SHELL_SCRIPT.contains("window.Shell ="),
            "app.js must publish the shell object pages.js expects"
        );
    }

    #[test]
    fn the_page_scripts_declare_every_name_they_read() {
        // Reading an undeclared name throws a ReferenceError, and `node --check` is a
        // syntax check that cannot see it. This is not hypothetical: `pages.js` used
        // `sessionKv` in three places without ever declaring it, which threw inside
        // the home page builder. Because the router called that builder bare, the
        // throw travelled up through `boot()` and cancelled everything after it — the
        // health poll, the one-second tick and the drawer wiring — so the only visible
        // symptom was a status pill stuck on "connecting" and one empty section.
        //
        // The scan itself lives in `scripts/undeclared.mjs` rather than here: telling a
        // read from a property access or an object key needs more JavaScript than a
        // Rust test wants to grow, and the script can be pointed at a file on its own.
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/undeclared.mjs");
        if !script.is_file() {
            eprintln!("skipping: {} is missing", script.display());
            return;
        }

        let output = match Command::new("node")
            .arg(&script)
            .arg("web/app.js")
            .arg("web/pages.js")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
        {
            Ok(output) => output,
            // A machine without node still gets a working build; `verify.ps1` reports
            // the skip loudly, which is where a missing tool should be noticed.
            Err(err) => {
                eprintln!("skipping undeclared-name scan: could not run node ({err})");
                return;
            }
        };

        let report = String::from_utf8_lossy(&output.stderr).trim().to_string();
        assert!(
            output.status.success(),
            "a page script reads a name it never declares:\n{report}"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("no undeclared names"),
            "the scanner did not report a clean run, so it may not have run at all:\n{report}"
        );
    }

    /// Ids the scripts look up that the markup may have to provide.
    fn extract_required_ids(script: &str) -> BTreeSet<String> {
        let mut ids = BTreeSet::new();
        for marker in ["getElementById(\"", "querySelector(\"#"] {
            let mut rest = script;
            while let Some((_, after)) = rest.split_once(marker) {
                let Some((id, tail)) = after.split_once('"') else { break };
                if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    ids.insert(id.to_string());
                }
                rest = tail;
            }
        }
        ids
    }

    #[test]
    fn embedded_routes_are_well_formed() {
        let mut seen = BTreeSet::new();
        for (route, content_type, body) in EMBEDDED {
            assert!(route.starts_with('/'), "{route} must be absolute");
            assert!(!body.is_empty(), "{route} is empty");
            assert!(content_type.contains('/'), "{route} has no media type");
            assert!(seen.insert(*route), "{route} is listed twice");
        }
        assert!(seen.contains("/"));
    }
}

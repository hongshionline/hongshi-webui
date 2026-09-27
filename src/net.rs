//! Fetching the official site's JSON from the shell.
//!
//! The page cannot do this itself: it is served from `http://127.0.0.1:<port>`,
//! so a request to `https://hongshi.site/api/…` is cross-origin and the browser
//! blocks it. The shell proxies instead, which also means the site sees one client
//! rather than every user's browser, and the CSP can keep `connect-src 'self'`.
//!
//! **On Windows this uses WinHTTP**, through a handful of FFI declarations in this
//! file. That choice is worth stating: the crate has no dependencies, so a
//! pure-Rust HTTPS client would mean shipping a TLS stack (rustls + ring + a
//! certificate store) inside a 356 KB binary, and a hand-rolled TLS client is not
//! something to ship at all. WinHTTP is part of the operating system, uses the
//! system certificate store, and understands the system proxy configuration —
//! which is exactly what a user behind Clash or a corporate proxy needs, and what
//! a bundled TLS stack would have to re-learn.
//!
//! Elsewhere, plain `http://` is fetched over a TCP socket and `https://` reports
//! itself as unsupported rather than pretending.

use std::time::Duration;

use crate::log::{debug_fields, warn_fields};

/// How long a request may take end to end.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// The outcome of one fetch. A non-2xx answer is `Ok` with its status, so the
/// caller can show the server's own words; only transport problems are `Err`.
pub enum Fetched {
    /// Status code and body.
    Response { status: u16, body: Vec<u8> },
    /// This build cannot reach that URL at all (no HTTPS support on this platform).
    Unsupported(String),
    /// The request did not complete.
    Failed(String),
}

impl Fetched {
    pub fn status(&self) -> Option<u16> {
        match self {
            Fetched::Response { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub fn body_text(&self) -> String {
        match self {
            Fetched::Response { body, .. } => String::from_utf8_lossy(body).into_owned(),
            Fetched::Unsupported(reason) => format!("unsupported: {reason}"),
            Fetched::Failed(reason) => format!("failed: {reason}"),
        }
    }

    /// Describe the outcome for the page: a short machine-readable state plus a
    /// human reason, so the interface never has to guess why something is empty.
    pub fn state(&self) -> &'static str {
        match self {
            Fetched::Response { status, .. } if (200..300).contains(status) => "ok",
            Fetched::Response { status, .. } if *status == 404 => "missing",
            Fetched::Response { .. } => "error",
            Fetched::Unsupported(_) => "unsupported",
            Fetched::Failed(_) => "unreachable",
        }
    }

    pub fn reason(&self) -> String {
        match self {
            Fetched::Response { status, .. } if (200..300).contains(status) => String::new(),
            Fetched::Response { status, .. } if *status == 404 => {
                "服务端还没有这个接口".to_string()
            }
            Fetched::Response { status, .. } => format!("服务端返回 HTTP {status}"),
            Fetched::Unsupported(reason) => reason.clone(),
            Fetched::Failed(reason) => reason.clone(),
        }
    }
}

/// How the shell should reach the network.
#[derive(Debug, Clone, Default)]
pub struct NetOptions {
    /// Explicit proxy, e.g. `http://127.0.0.1:7897`. Empty means "ask the system".
    pub proxy: String,
    /// Whether to use the system proxy configuration when `proxy` is empty.
    pub use_system_proxy: bool,
}

impl NetOptions {
    /// The options the user's settings describe.
    pub fn from_settings(settings: &crate::config::Settings) -> NetOptions {
        NetOptions {
            proxy: settings.proxy.clone(),
            use_system_proxy: settings.use_system_proxy,
        }
    }
}

/// An HTTP client the host provides, used instead of this crate's own socket.
///
/// The seam exists because the fallback client below is deliberately minimal: it
/// speaks plain `http://` over a TCP socket and refuses `https://` outright, which
/// is the honest answer for a desktop build that has WinHTTP and nothing else. A
/// host that *does* have a real HTTP client — Android, whose `HttpsURLConnection`
/// carries the system trust store, the system proxy and the platform's TLS — would
/// otherwise be stuck on that refusal for every request to `api_base`, which
/// defaults to `https://hongshi.site`.
///
/// So the host installs one of these at startup and the shell routes every fetch
/// through it. Nothing about the desktop path changes: no backend is installed
/// there, and [`fetch_with`] falls through to the same code it always ran.
pub trait HttpBackend: Send + Sync {
    /// What the settings page calls this client. The host knows its own name and
    /// this crate does not, so the host is what answers.
    fn name(&self) -> &'static str {
        "系统 HTTP 客户端"
    }

    /// GET `url`. `Err` is a transport failure and carries the reason; an HTTP
    /// error status is `Ok`, exactly as the platform clients elsewhere report it,
    /// so the page can show the server's own words.
    fn get(&self, url: &str, options: &NetOptions, timeout: Duration)
    -> Result<(u16, Vec<u8>), String>;
}

/// The installed backend, if the host has one.
///
/// Process-global and write-once on purpose: it is set during startup, before the
/// accept loop exists, so nothing can race it, and a second install is a
/// programming error rather than something to silently accept.
static HTTP_BACKEND: std::sync::OnceLock<Box<dyn HttpBackend>> = std::sync::OnceLock::new();

/// Install the host's HTTP client. Called once at startup.
pub fn install_http_backend(backend: Box<dyn HttpBackend>) -> Result<(), String> {
    HTTP_BACKEND
        .set(backend)
        .map_err(|_| "an HTTP backend is already installed".to_string())
}

/// Whether a backend is installed — shown in the settings page.
pub fn has_http_backend() -> bool {
    HTTP_BACKEND.get().is_some()
}

/// The installed backend, for callers that want to describe or use it.
pub fn http_backend() -> Option<&'static dyn HttpBackend> {
    HTTP_BACKEND.get().map(|backend| backend.as_ref())
}

/// GET `url`, sending `Accept: application/json`.
pub fn get_json(url: &str, options: &NetOptions) -> Fetched {
    debug_fields("fetching", &[("url", url)]);
    let result = platform_get(url, options);
    match &result {
        Fetched::Response { status, body } => debug_fields(
            "fetched",
            &[("url", url), ("status", &status.to_string()), ("bytes", &body.len().to_string())],
        ),
        other => warn_fields(
            "fetch did not complete",
            &[("url", url), ("reason", &other.reason())],
        ),
    }
    result
}

/// GET `url` for its bytes rather than its text, with a longer deadline.
///
/// The kernel is around a megabyte and is served straight from the origin, so it
/// needs more than the 15-second budget a metadata request gets. `Fetched` is reused
/// so a caller handles a 404 or an unreachable host exactly as it does elsewhere —
/// and the site answers a broken download with a real status in both of its body
/// shapes, which is what makes that possible.
pub fn get_bytes(url: &str, options: &NetOptions, timeout: Duration) -> Fetched {
    debug_fields("downloading", &[("url", url)]);
    let result = platform_get_within(url, options, timeout);
    match &result {
        Fetched::Response { status, body } => debug_fields(
            "downloaded",
            &[("url", url), ("status", &status.to_string()), ("bytes", &body.len().to_string())],
        ),
        other => warn_fields(
            "download did not complete",
            &[("url", url), ("reason", &other.reason())],
        ),
    }
    result
}

/// Whether this build can fetch the given scheme at all.
fn unsupported_scheme(url: &str) -> Option<String> {
    if url.starts_with("http://") {
        None
    } else if url.starts_with("https://") {
        None
    } else {
        Some(format!("不支持的地址：{url}"))
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// One fetch, however this build can make it.
fn platform_get(url: &str, options: &NetOptions) -> Fetched {
    platform_get_within(url, options, REQUEST_TIMEOUT)
}

/// One fetch with an explicit deadline: the host's backend if it installed one,
/// otherwise the platform's own client.
///
/// `unsupported_scheme` is checked here rather than in each branch, so a host that
/// installs a backend cannot be handed a `ftp://` URL by accident.
fn platform_get_within(url: &str, options: &NetOptions, timeout: Duration) -> Fetched {
    if let Some(reason) = unsupported_scheme(url) {
        return Fetched::Unsupported(reason);
    }
    fetch_with(HTTP_BACKEND.get().map(|backend| backend.as_ref()), url, options, timeout)
}

/// The dispatch, written as a pure function of the backend it is given.
///
/// The backend is a parameter rather than read from [`HTTP_BACKEND`] here so the
/// decision can be tested without installing a process-global one — a test that
/// did that would leak into every other test in the same binary.
fn fetch_with(
    backend: Option<&dyn HttpBackend>,
    url: &str,
    options: &NetOptions,
    timeout: Duration,
) -> Fetched {
    if let Some(backend) = backend {
        return match backend.get(url, options, timeout) {
            Ok((status, body)) => Fetched::Response { status, body },
            Err(reason) => Fetched::Failed(reason),
        };
    }
    native_get_within(url, options, timeout)
}

// ---------------------------------------------------------------------------
// Windows: WinHTTP
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn native_get_within(url: &str, options: &NetOptions, timeout: Duration) -> Fetched {
    match winhttp::get(url, options, timeout) {
        Ok((status, body)) => Fetched::Response { status, body },
        Err(reason) => Fetched::Failed(reason),
    }
}

#[cfg(windows)]
mod winhttp {
    //! The small part of WinHTTP this crate needs: open a session, one request,
    //! read the status and the body.
    //!
    //! Every declaration is local to this module on purpose. `windows-sys` would
    //! be a dependency for ten functions, and the surface here is small enough to
    //! read in one sitting.

    use std::ffi::c_void;
    use std::time::Duration;

    use super::NetOptions;
    use crate::log::debug_fields;

    type Handle = *mut c_void;

    #[link(name = "winhttp")]
    unsafe extern "system" {
        fn WinHttpOpen(
            agent: *const u16,
            access_type: u32,
            proxy: *const u16,
            bypass: *const u16,
            flags: u32,
        ) -> Handle;
        fn WinHttpConnect(session: Handle, server: *const u16, port: u16, reserved: u32) -> Handle;
        fn WinHttpOpenRequest(
            connect: Handle,
            verb: *const u16,
            object: *const u16,
            version: *const u16,
            referrer: *const u16,
            accept_types: *const *const u16,
            flags: u32,
        ) -> Handle;
        fn WinHttpSendRequest(
            request: Handle,
            headers: *const u16,
            headers_length: u32,
            optional: *mut c_void,
            optional_length: u32,
            total_length: u32,
            context: usize,
        ) -> i32;
        fn WinHttpReceiveResponse(request: Handle, reserved: *mut c_void) -> i32;
        fn WinHttpQueryHeaders(
            request: Handle,
            info_level: u32,
            name: *const u16,
            buffer: *mut c_void,
            buffer_length: *mut u32,
            index: *mut u32,
        ) -> i32;
        fn WinHttpQueryDataAvailable(request: Handle, available: *mut u32) -> i32;
        fn WinHttpReadData(request: Handle, buffer: *mut c_void, to_read: u32, read: *mut u32) -> i32;
        fn WinHttpSetTimeouts(
            handle: Handle,
            resolve: i32,
            connect: i32,
            send: i32,
            receive: i32,
        ) -> i32;
        fn WinHttpSetOption(handle: Handle, option: u32, buffer: *mut c_void, length: u32) -> i32;
        fn WinHttpGetIEProxyConfigForCurrentUser(info: *mut WinHttpGetProxyForUrlInfo) -> i32;
        fn WinHttpCloseHandle(handle: Handle) -> i32;
    }

    const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
    const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
    const WINHTTP_ACCESS_TYPE_NAMED_PROXY: u32 = 3;

    const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
    const WINHTTP_FLAG_REFRESH: u32 = 0x0000_0100;

    const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
    const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;

    const WINHTTP_OPTION_PROXY: u32 = 38;

    #[repr(C)]
    struct WinHttpProxyInfo {
        access_type: u32,
        proxy: *mut u16,
        proxy_bypass: *mut u16,
    }

    /// `WINHTTP_CURRENT_USER_IE_PROXY_CONFIG`, filled by
    /// `WinHttpGetIEProxyConfigForCurrentUser`. Every string it hands back must be
    /// released with `GlobalFree`.
    #[repr(C)]
    struct WinHttpGetProxyForUrlInfo {
        auto_detect: i32,
        auto_config_url: *mut u16,
        proxy: *mut u16,
        proxy_bypass: *mut u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalFree(handle: *mut c_void) -> *mut c_void;
    }

    /// Owns a WinHTTP handle and closes it on drop, so no path can leak one.
    struct Owned(Handle);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the handle came from a WinHTTP open call and is closed
                // exactly once, here.
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    /// Collects UTF-16 with a terminating NUL, which is what WinHTTP expects.
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error(what: &str) -> String {
        format!("{what} 失败（Windows 错误 {}）", std::io::Error::last_os_error())
    }

    pub fn get(url: &str, options: &NetOptions, timeout: Duration) -> Result<(u16, Vec<u8>), String> {
        let (secure, host, port, path) = split_url(url)?;

        let agent = wide("hongshi-shell/0.1");
        // SAFETY: all pointers below are valid for the duration of the call, and
        // every returned handle is owned by an `Owned` or closed explicitly.
        unsafe {
            let session = Owned(WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            ));
            if session.0.is_null() {
                return Err(last_error("WinHttpOpen"));
            }

            configure_proxy(session.0, options)?;

            let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
            WinHttpSetTimeouts(session.0, millis, millis, millis, millis);

            let host_wide = wide(&host);
            let connect = Owned(WinHttpConnect(session.0, host_wide.as_ptr(), port, 0));
            if connect.0.is_null() {
                return Err(last_error("WinHttpConnect"));
            }

            let verb = wide("GET");
            let object = wide(&path);
            let accept = wide("application/json");
            let accept_types = [accept.as_ptr(), std::ptr::null()];
            let flags = if secure { WINHTTP_FLAG_SECURE | WINHTTP_FLAG_REFRESH } else { WINHTTP_FLAG_REFRESH };
            let request = Owned(WinHttpOpenRequest(
                connect.0,
                verb.as_ptr(),
                object.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                accept_types.as_ptr(),
                flags,
            ));
            if request.0.is_null() {
                return Err(last_error("WinHttpOpenRequest"));
            }

            let headers = wide("Accept: application/json\r\n");
            if WinHttpSendRequest(
                request.0,
                headers.as_ptr(),
                u32::MAX, // -1L: the header string is NUL-terminated
                std::ptr::null_mut(),
                0,
                0,
                0,
            ) == 0
            {
                return Err(last_error("WinHttpSendRequest"));
            }
            if WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0 {
                return Err(last_error("WinHttpReceiveResponse"));
            }

            let status = query_status(request.0)?;

            let mut body = Vec::new();
            loop {
                let mut available: u32 = 0;
                if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                    return Err(last_error("WinHttpQueryDataAvailable"));
                }
                if available == 0 {
                    break;
                }
                let mut chunk = vec![0u8; available.min(64 * 1024) as usize];
                let mut read: u32 = 0;
                if WinHttpReadData(request.0, chunk.as_mut_ptr().cast(), chunk.len() as u32, &mut read) == 0
                {
                    return Err(last_error("WinHttpReadData"));
                }
                if read == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..read as usize]);
                // A response this large is not one of our endpoints.
                if body.len() > 4 * 1024 * 1024 {
                    return Err("响应过大".to_string());
                }
            }
            Ok((status, body))
        }
    }

    /// Wire up the proxy: the explicit one if set, the machine's own setting if
    /// allowed, and nothing at all if the user turned that off.
    ///
    /// Reading the system proxy is what makes a running Clash / v2ray on
    /// `127.0.0.1:7897` work without being typed into the settings page.
    unsafe fn configure_proxy(session: Handle, options: &NetOptions) -> Result<(), String> {
        let explicit = options.proxy.trim();
        if !explicit.is_empty() {
            let proxy = wide(explicit);
            let mut info = WinHttpProxyInfo {
                access_type: WINHTTP_ACCESS_TYPE_NAMED_PROXY,
                proxy: proxy.as_ptr() as *mut u16,
                proxy_bypass: std::ptr::null_mut(),
            };
            // SAFETY: `info` and its string outlive the call, and the string is
            // NUL-terminated as WinHTTP requires.
            if unsafe {
                WinHttpSetOption(
                    session,
                    WINHTTP_OPTION_PROXY,
                    (&mut info as *mut WinHttpProxyInfo).cast(),
                    std::mem::size_of::<WinHttpProxyInfo>() as u32,
                )
            } == 0
            {
                return Err(last_error("WinHttpSetOption(proxy)"));
            }
            return Ok(());
        }

        if options.use_system_proxy {
            // SAFETY: every pointer the API returns is released below, exactly
            // once. `session` is borrowed, not owned — it is closed by its own
            // `Owned` in `get`.
            unsafe {
                let mut current = WinHttpGetProxyForUrlInfo {
                    auto_detect: 0,
                    auto_config_url: std::ptr::null_mut(),
                    proxy: std::ptr::null_mut(),
                    proxy_bypass: std::ptr::null_mut(),
                };
                if WinHttpGetIEProxyConfigForCurrentUser(&mut current) != 0 {
                    if !current.proxy.is_null() {
                        let proxy_text = from_wide(current.proxy);
                        if !proxy_text.is_empty() {
                            let proxy = wide(&proxy_text);
                            let mut info = WinHttpProxyInfo {
                                access_type: WINHTTP_ACCESS_TYPE_NAMED_PROXY,
                                proxy: proxy.as_ptr() as *mut u16,
                                proxy_bypass: std::ptr::null_mut(),
                            };
                            WinHttpSetOption(
                                session,
                                WINHTTP_OPTION_PROXY,
                                (&mut info as *mut WinHttpProxyInfo).cast(),
                                std::mem::size_of::<WinHttpProxyInfo>() as u32,
                            );
                            debug_fields("using the system proxy", &[("proxy", &proxy_text)]);
                        }
                    }
                    for pointer in [current.auto_config_url, current.proxy, current.proxy_bypass] {
                        if !pointer.is_null() {
                            GlobalFree(pointer.cast());
                        }
                    }
                }
            }
            return Ok(());
        }

        let mut info = WinHttpProxyInfo {
            access_type: WINHTTP_ACCESS_TYPE_NO_PROXY,
            proxy: std::ptr::null_mut(),
            proxy_bypass: std::ptr::null_mut(),
        };
        // SAFETY: as in the explicit case.
        unsafe {
            WinHttpSetOption(
                session,
                WINHTTP_OPTION_PROXY,
                (&mut info as *mut WinHttpProxyInfo).cast(),
                std::mem::size_of::<WinHttpProxyInfo>() as u32,
            );
        }
        Ok(())
    }

    /// Read a NUL-terminated UTF-16 string WinHTTP allocated.
    fn from_wide(pointer: *const u16) -> String {
        if pointer.is_null() {
            return String::new();
        }
        let mut length = 0usize;
        // SAFETY: the caller guarantees this is a NUL-terminated string.
        unsafe {
            while *pointer.add(length) != 0 {
                length += 1;
            }
            let slice = std::slice::from_raw_parts(pointer, length);
            String::from_utf16_lossy(slice)
        }
    }

    unsafe fn query_status(request: Handle) -> Result<u16, String> {
        let mut status: u32 = 0;
        let mut length = std::mem::size_of::<u32>() as u32;
        // SAFETY: the buffer is a u32 and `length` says so.
        let ok = unsafe {
            WinHttpQueryHeaders(
                request,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                std::ptr::null(),
                (&mut status as *mut u32).cast(),
                &mut length,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(last_error("WinHttpQueryHeaders"));
        }
        Ok(status as u16)
    }

    /// Split a URL into (is_secure, host, port, request path).
    fn split_url(url: &str) -> Result<(bool, String, u16, String), String> {
        let (secure, rest) = if let Some(rest) = url.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = url.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(format!("不支持的地址：{url}"));
        };
        let (authority, path) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, "/".to_string()),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() => (
                host.to_string(),
                port.parse::<u16>()
                    .map_err(|_| format!("端口不正确：{authority}"))?,
            ),
            _ => (authority.to_string(), if secure { 443 } else { 80 }),
        };
        if host.is_empty() {
            return Err(format!("主机名为空：{url}"));
        }
        Ok((secure, host, port, path))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn urls_split_into_what_winhttp_needs() {
            assert_eq!(
                split_url("https://hongshi.site/api/daily_news").ok(),
                Some((true, "hongshi.site".to_string(), 443, "/api/daily_news".to_string()))
            );
            assert_eq!(
                split_url("http://127.0.0.1:3000/api/server/list").ok(),
                Some((false, "127.0.0.1".to_string(), 3000, "/api/server/list".to_string()))
            );
            assert_eq!(
                split_url("https://example.test").ok(),
                Some((true, "example.test".to_string(), 443, "/".to_string()))
            );
        }

        #[test]
        fn junk_urls_are_refused_with_a_reason() {
            assert!(split_url("ftp://example.test/x").is_err());
            assert!(split_url("https://example.test:notaport/").is_err());
            assert!(split_url("https:///nohost").is_err());
        }
    }
}

// ---------------------------------------------------------------------------
// Other platforms
// ---------------------------------------------------------------------------

/// The fallback client: plain `http://` over a TCP socket, `https://` refused.
///
/// Reached only when the host installed no [`HttpBackend`]. That is the desktop
/// case on Linux and macOS, where there is no system HTTP client this crate can
/// reach without a dependency, and the refusal is the honest answer.
#[cfg(not(windows))]
fn native_get_within(url: &str, options: &NetOptions, timeout: Duration) -> Fetched {
    if url.starts_with("https://") {
        return Fetched::Unsupported(
            "此平台版本不支持 HTTPS，请在设置里把服务地址改成 http:// 开头的地址".to_string(),
        );
    }
    match plain_http_get(url, options, timeout) {
        Ok((status, body)) => Fetched::Response { status, body },
        Err(reason) => Fetched::Failed(reason),
    }
}

#[cfg(not(windows))]
fn plain_http_get(
    url: &str,
    options: &NetOptions,
    timeout: Duration,
) -> Result<(u16, Vec<u8>), String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let rest = url.strip_prefix("http://").ok_or("只支持 http://")?;
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    let target = if options.proxy.trim().is_empty() {
        authority.to_string()
    } else {
        // A forward proxy wants the absolute form as the request target.
        options
            .proxy
            .trim()
            .trim_start_matches("http://")
            .to_string()
    };

    let mut stream = TcpStream::connect(&target).map_err(|err| format!("连接 {target} 失败: {err}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|err| err.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|err| err.to_string())?;
    let request = format!(
        "GET {url} HTTP/1.1\r\nHost: {authority}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    // `path` is unused in the proxy form; kept for the direct form below.
    let _ = path;
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("发送请求失败: {err}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|err| format!("读取响应失败: {err}"))?;

    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or("响应没有状态行")?;
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.as_bytes().to_vec())
        .unwrap_or_default();
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A backend that answers everything, so the tests below exercise the
    /// dispatch rather than any socket.
    struct Fake(&'static str);

    impl HttpBackend for Fake {
        fn name(&self) -> &'static str {
            self.0
        }

        fn get(
            &self,
            url: &str,
            _options: &NetOptions,
            _timeout: Duration,
        ) -> Result<(u16, Vec<u8>), String> {
            Ok((200, format!("{{\"url\":\"{url}\"}}").into_bytes()))
        }
    }

    /// A backend that cannot reach anything.
    struct Broken(&'static str);

    impl HttpBackend for Broken {
        fn get(
            &self,
            _url: &str,
            _options: &NetOptions,
            _timeout: Duration,
        ) -> Result<(u16, Vec<u8>), String> {
            Err(self.0.to_string())
        }
    }

    /// A backend whose server answered, with a status that is not 2xx.
    struct Status(u16);

    impl HttpBackend for Status {
        fn get(
            &self,
            _url: &str,
            _options: &NetOptions,
            _timeout: Duration,
        ) -> Result<(u16, Vec<u8>), String> {
            Ok((self.0, b"{}".to_vec()))
        }
    }

    fn one_second() -> Duration {
        Duration::from_secs(1)
    }

    #[test]
    fn an_installed_backend_answers_the_fetch() {
        // The point of the seam: a host with a real HTTP client is what gets used,
        // including for the `https://` the fallback client refuses outright.
        let options = NetOptions::default();
        let fetched = fetch_with(
            Some(&Fake("测试客户端")),
            "https://example.test/api/server/list",
            &options,
            one_second(),
        );
        assert_eq!(fetched.status(), Some(200));
        assert_eq!(fetched.state(), "ok");
        assert_eq!(fetched.reason(), "");
        assert!(fetched.body_text().contains("example.test"), "{}", fetched.body_text());
    }

    #[test]
    fn a_backend_failure_is_reported_as_unreachable_not_as_a_status() {
        // A transport failure and a 404 are different answers, and the page shows
        // different words for them; collapsing one into the other is how a node
        // list ends up silently empty.
        let fetched = fetch_with(
            Some(&Broken("连接被拒绝")),
            "https://example.test/",
            &NetOptions::default(),
            one_second(),
        );
        assert_eq!(fetched.state(), "unreachable");
        assert_eq!(fetched.status(), None);
        assert_eq!(fetched.reason(), "连接被拒绝");
    }

    #[test]
    fn an_http_error_status_still_carries_the_server_own_words() {
        let fetched = fetch_with(
            Some(&Status(404)),
            "https://example.test/api/webui/version",
            &NetOptions::default(),
            one_second(),
        );
        assert_eq!(fetched.state(), "missing");
        assert_eq!(fetched.status(), Some(404));
        assert!(!fetched.reason().is_empty());

        let fetched = fetch_with(
            Some(&Status(503)),
            "https://example.test/",
            &NetOptions::default(),
            one_second(),
        );
        assert_eq!(fetched.state(), "error");
        assert!(fetched.reason().contains("503"), "{}", fetched.reason());
    }

    #[test]
    fn the_backend_is_never_handed_a_scheme_this_build_cannot_fetch() {
        // `platform_get_within` refuses the scheme before it consults anything, so
        // a backend cannot be asked to make sense of `ftp://` — and, more to the
        // point, no socket is opened for it.
        let fetched =
            platform_get_within("ftp://example.test/x", &NetOptions::default(), one_second());
        assert_eq!(fetched.state(), "unsupported");
        assert!(fetched.body_text().contains("ftp://example.test/x"));
    }

    #[test]
    fn no_backend_installed_means_the_platform_client_is_what_runs() {
        // Asserted through the registry rather than by fetching: this test must not
        // make a network request on a build that has a real platform client.
        assert!(!has_http_backend(), "no test may install a process-global backend");
        assert!(http_backend().is_none());
    }

    #[test]
    fn a_backend_names_itself() {
        // The settings page shows whatever the host calls its client, because this
        // crate has never seen that client and should not be guessing at it.
        let backend = Fake("Android HttpsURLConnection");
        assert_eq!(backend.name(), "Android HttpsURLConnection");
    }
}

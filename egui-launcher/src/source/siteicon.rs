//! FR-SRC-11 第二半：引擎图标（显示 + 「下载站点图标」）。
//!
//! 口径逐条照 AHK 版 `Src/Providers/WebSearchProvider.ahk`：
//! [`web_icon_spec`] ← `IconFor`、[`download_icon`] ← `DownloadIcon`、
//! [`icon_candidates`] ← `IconCandidates`、[`image_ext`] ← `ImageType`、
//! [`absolute_url`] ← `_Absolute`。差别只在联网那一层：AHK 用 `WinHttp.WinHttpRequest.5.1`
//! 这个 COM 对象，这里直调 WinHTTP（[`fetch_winhttp`]），不引入 HTTP crate。
//!
//! 联网只发生在用户点「下载图标」那一刻，且**只访问该引擎网址所在的站点**（不经过第三方
//! 图标服务）。所有函数都收一个可注入的 `fetch`，单测因此不联网。
//!
//! 三条有意偏差（写进发布说明）：
//! - **出厂不带品牌图标**：AHK 有 `Resources\Icons\Web\<Id>.png` 那九张，本仓没有 ⇒
//!   没下载过图标时用默认浏览器图标（`url:`），不是各家 logo。
//! - **相对路径按「请求的网址」解析**：AHK 用跳转后的最终网址（`WinHttpRequest.Option(1)`）。
//!   差别只在跨域跳转的站点上，本批不追那一次查询。
//! - **`%VAR%` 不展开**：AHK 走 `Path.Resolve` 展开环境变量，本仓只认绝对路径与相对数据目录。

use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// 图标文件上限（AHK `MaxIconBytes`）。
pub const MAX_ICON_BYTES: usize = 1024 * 1024;
/// 首页 HTML 上限：比它大就不扫了（AHK `MaxPageBytes`）。
pub const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
/// 扫 HTML 只看前 512 KB（AHK `Min(Size, 512 * 1024)`）。
pub const PAGE_SCAN_BYTES: usize = 512 * 1024;

/// 一次 GET 的回报。`body` 最多 `limit` 字节；资源比 `limit` 长时只给前 `limit` 字节并置
/// `truncated`——调用方靠这一位判"太大了"，不必把整幅读回来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchReply {
    pub status: u16,
    pub body: Vec<u8>,
    pub truncated: bool,
}

/// 可注入的取网页函数：`(网址, 最多读多少字节)`。
pub type Fetcher<'a> = &'a dyn Fn(&str, usize) -> Result<FetchReply, String>;

/// 下载成功的结果。`relative` 是**相对数据目录**的写法（换电脑仍可用），直接存回
/// `WebEngine.icon`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadedIcon {
    pub relative: String,
    pub ext: String,
    pub bytes: usize,
}

/// 文件头认图片类型：`png` / `ico` / `jpg` / `gif` / `bmp`，不是图片给空串。
/// 长度门槛照抄 AHK（BMP 要 ≥26 字节，因为它的头有那么多）。
pub fn image_ext(body: &[u8]) -> &'static str {
    let n = body.len();
    if n >= 8 && body[0] == 0x89 && body[1] == 0x50 && body[2] == 0x4E && body[3] == 0x47 {
        return "png";
    }
    if n >= 6 && body[0] == 0 && body[1] == 0 && body[2] == 1 && body[3] == 0 {
        return "ico";
    }
    if n >= 3 && body[0] == 0xFF && body[1] == 0xD8 && body[2] == 0xFF {
        return "jpg";
    }
    if n >= 6 && body[0] == 0x47 && body[1] == 0x49 && body[2] == 0x46 && body[3] == 0x38 {
        return "gif";
    }
    if n >= 26 && body[0] == 0x42 && body[1] == 0x4D {
        return "bmp";
    }
    ""
}

/// 网址的「origin」：`scheme://host[:port]`。不是 http(s) 网址给 `None`。
/// 末尾那个冒号要去掉（`http://host:/x` ⇒ `http://host`），与 AHK `RTrim(m[1], ":")` 一致。
pub fn origin_of(url: &str) -> Option<String> {
    let scheme_len = scheme_len(url)?;
    let rest = &url[scheme_len..];
    let end = rest
        .find(['/', '?', '#', '{', '}'])
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let origin = &url[..scheme_len + end];
    Some(origin.trim_end_matches(':').to_string())
}

/// `http://` / `https://` 的字节长度。用 `str::get` 而不是切片：查询词里可能有中文，
/// 按下标硬切会切在多字节字符中间直接 panic。
fn scheme_len(url: &str) -> Option<usize> {
    if url.get(..8).is_some_and(|h| h.eq_ignore_ascii_case("https://")) {
        return Some(8);
    }
    if url.get(..7).is_some_and(|h| h.eq_ignore_ascii_case("http://")) {
        return Some(7);
    }
    None
}

/// 把网页里的相对地址补成完整网址（AHK `_Absolute`）。
pub fn absolute_url(href: &str, base: &str) -> String {
    if scheme_len(href).is_some() {
        return href.to_string();
    }
    let Some((scheme, host)) = split_base(base) else {
        return href.to_string();
    };
    if let Some(rest) = href.strip_prefix("//") {
        return format!("{scheme}//{rest}");
    }
    if href.starts_with('/') {
        return format!("{scheme}//{host}{href}");
    }
    let dir = base.split(['?', '#']).next().unwrap_or(base);
    let dir = if dir_is_bare_host(dir) {
        format!("{dir}/")
    } else {
        match dir.rsplit_once('/') {
            Some((head, _)) => format!("{head}/"),
            None => format!("{dir}/"),
        }
    };
    format!("{dir}{href}")
}

/// `scheme:` + 主机名（不含端口之后的斜杠）。拆不出来给 `None`。
fn split_base(base: &str) -> Option<(&'static str, &str)> {
    let n = scheme_len(base)?;
    let scheme = if n == 8 { "https:" } else { "http:" };
    let rest = &base[n..];
    let end = rest
        .find(['/', '?', '#', '{', '}'])
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some((scheme, &rest[..end]))
}

/// `https://x.com` 这种只有主机的写法（没有路径段）。
fn dir_is_bare_host(dir: &str) -> bool {
    let Some(n) = scheme_len(dir) else {
        return false;
    };
    let rest = &dir[n..];
    !rest.is_empty() && !rest.contains('/')
}

/// 网页里写的图标候选，**清晰的在前**：`apple-touch-icon` 一律压过普通 `icon`（它通常是
/// 180 px 的 PNG），同类里按 `sizes` 的数字从大到小；同分保持原文次序（稳定降序）。
///
/// 跳过 SVG（shell 画不了）、`data:` 内联图标、以及 `rel` 不是图标类的 `<link>`。
pub fn icon_candidates(html: &str, page_url: &str) -> Vec<String> {
    let mut found: Vec<(String, i64)> = Vec::new();
    for m in link_re().find_iter(html) {
        let tag = m.as_str();
        let a = attrs_of(tag);
        if a.href.is_empty() || !rel_is_icon(&a.rel) {
            continue;
        }
        let href = a.href.replace("&amp;", "&");
        let href_l = href.to_ascii_lowercase();
        if href_l.starts_with("data:") || has_svg_ext(&href_l) || a.kind.to_ascii_lowercase().contains("svg") {
            continue;
        }
        let apple = a.rel.to_ascii_lowercase().contains("apple-touch-icon");
        let size = parse_size(&a.sizes).unwrap_or(if apple { 180 } else { 32 });
        found.push((
            absolute_url(&href, page_url),
            if apple { 10000 } else { 0 } + size,
        ));
    }
    found.sort_by(|a, b| b.1.cmp(&a.1));
    let mut out: Vec<String> = Vec::new();
    for (url, _) in found {
        if !out.contains(&url) {
            out.push(url);
        }
    }
    out
}

fn link_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<link\b[^>]*>").expect("link 正则"))
}

fn attr_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)\s(rel|href|sizes|type)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#)
            .expect("属性正则")
    })
}

fn size_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\d+)x\d+").expect("尺寸正则"))
}

#[derive(Default)]
struct Attrs {
    rel: String,
    href: String,
    sizes: String,
    kind: String,
}

/// 取一个 `<link>` 标签里我们要的四个属性（同名取第一个，与 AHK `_Attribute` 一致）。
fn attrs_of(tag: &str) -> Attrs {
    let mut a = Attrs::default();
    let mut seen = [false; 4];
    for c in attr_re().captures_iter(tag) {
        let Some(name) = c.get(1) else { continue };
        let which = match name.as_str().to_ascii_lowercase().as_str() {
            "rel" => 0usize,
            "href" => 1,
            "sizes" => 2,
            "type" => 3,
            _ => continue,
        };
        if seen[which] {
            continue;
        }
        seen[which] = true;
        let val = c
            .get(2)
            .or_else(|| c.get(3))
            .or_else(|| c.get(4))
            .map(|m| m.as_str().trim())
            .unwrap_or("");
        match which {
            0 => a.rel = val.to_string(),
            1 => a.href = val.to_string(),
            2 => a.sizes = val.to_string(),
            _ => a.kind = val.to_string(),
        }
    }
    a
}

/// `rel` 里是否有 `icon` / `apple-touch-icon` / `apple-touch-icon-precomposed` 这个词
/// （按空白切词，`shortcut icon` 也算）。
fn rel_is_icon(rel: &str) -> bool {
    rel.split_whitespace().any(|w| {
        let w = w.to_ascii_lowercase();
        w == "icon" || w == "apple-touch-icon" || w == "apple-touch-icon-precomposed"
    })
}

/// `\.svgz?([?#]|$)` 的同义判断。
fn has_svg_ext(href: &str) -> bool {
    let mut from = 0usize;
    while let Some(p) = href[from..].find(".svg") {
        let at = from + p + 4;
        let rest = &href[at..];
        if rest.is_empty() || rest.starts_with('?') || rest.starts_with('#') {
            return true;
        }
        if let Some(r) = rest.strip_prefix('z') {
            if r.is_empty() || r.starts_with('?') || r.starts_with('#') {
                return true;
            }
        }
        from = at;
    }
    false
}

/// `sizes="64x64"` 里的第一个数。
fn parse_size(sizes: &str) -> Option<i64> {
    size_re()
        .captures(sizes)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
}

/// 图标文件的文件名：只留 `[A-Za-z0-9_.-]`，其余变 `_`；清洗完是空的就用主机名。
/// 分隔符不在白名单里 ⇒ 不可能拼出 `..\..\` 这种越界路径。
pub fn safe_icon_name(name: &str, origin: &str) -> String {
    let cleaned = sanitize_name(name.trim());
    // 全空、全是点（`.` / `..` 会被文件系统当成"当前/上级目录"）都退回主机名。
    if !cleaned.is_empty() && cleaned.chars().any(|c| c != '.') {
        return cleaned;
    }
    let host = origin.split_once("://").map(|(_, h)| h).unwrap_or(origin);
    sanitize_name(host)
}

fn sanitize_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 引擎 `icon` 字段 → 图标规格（这一格是 [`crate::source::icon::parse_spec`] 认的写法）。
///
/// - 空 ⇒ `url:`（浏览器默认图标；本仓不带品牌图标，见模块头那条偏差）
/// - 带 `res:` / `ext:` / `url:` / `folder:` / `thumb:` / `shell:` 前缀 ⇒ 原样
/// - 绝对路径（`X:\`、`\\srv\`、`%VAR%`）⇒ 原样；其余相对数据目录
/// - 文件不存在 ⇒ `url:`
/// - 图片（png/jpg/bmp/gif）⇒ `thumb:<路径>`；其余（含 `.ico`）⇒ 裸路径走 shell 取图标
pub fn web_icon_spec(icon: &str, data_dir: &Path) -> String {
    let spec = icon.trim().trim_matches(['"', ' ', '\t']);
    if spec.is_empty() {
        return "url:".to_string();
    }
    let lower = spec.to_ascii_lowercase();
    for p in ["res:", "ext:", "url:", "folder:", "thumb:", "shell:"] {
        if lower.starts_with(p) {
            return spec.to_string();
        }
    }
    let path = if looks_absolute(spec) {
        std::path::PathBuf::from(spec)
    } else {
        data_dir.join(spec)
    };
    if !path.exists() {
        return "url:".to_string();
    }
    let shown = path.to_string_lossy().to_string();
    match image_ext_by_name(&shown) {
        Some(_) => format!("thumb:{shown}"),
        None => shown,
    }
}

fn looks_absolute(spec: &str) -> bool {
    let b = spec.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') {
        return true;
    }
    spec.starts_with("\\\\") || spec.starts_with('%')
}

fn image_ext_by_name(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "bmp" => Some("bmp"),
        "gif" => Some("gif"),
        _ => None,
    }
}

/// 把 `{query}` 换成空串、`{query|默认值}` 换成默认值——下载图标时没有"用户敲的词"。
fn fill_for_icon(template: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(pos) = rest.find("{query") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 6..];
        if let Some(stripped) = after.strip_prefix('}') {
            rest = stripped;
        } else if let Some(stripped) = after.strip_prefix('|') {
            match stripped.find('}') {
                Some(end) => {
                    out.push_str(&stripped[..end]);
                    rest = &stripped[end + 1..];
                }
                None => {
                    out.push_str("{query");
                    rest = after;
                }
            }
        } else {
            out.push_str("{query");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// 「每个候选地址为什么没成」的计数，只用来在最后挑一句能指路的话。
///
/// 不记网址、不记站点名——这份结构最后会进 Toast，也可能进日志。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct DownloadFaults {
    tried: usize,
    unreachable: usize,
    too_big: usize,
    not_image: usize,
    missing: usize,
}

/// 把四类失败翻成一句能指路的话。顺序是"越具体越靠前"：
/// 太大 > 全连不上 > 全都不是图片 > 全都没有 > 混杂（把三类都摊开）。
fn failure_message(f: DownloadFaults) -> String {
    if f.tried == 0 {
        return "站点上没有可用的图标地址".to_string();
    }
    if f.too_big > 0 {
        return format!("图标超过 1 MB（试了 {} 个地址）", f.tried);
    }
    if f.unreachable == f.tried {
        return format!("连不上这个站点（试了 {} 个地址，检查网络或代理）", f.tried);
    }
    if f.not_image == f.tried {
        return "站点给的图标不是图片（认不出 PNG/ICO/JPG/GIF/BMP）".to_string();
    }
    if f.missing == f.tried {
        return format!("站点上没找到能用的图标（{} 个地址都没有）", f.tried);
    }
    format!(
        "没找到能用的图标（{} 个地址：{} 个没有、{} 个连不上、{} 个不是图片）",
        f.tried, f.missing, f.unreachable, f.not_image
    )
}

/// 下载 `address` 所在站点的图标，存成 `<data_dir>\Icons\<name>.<扩展名>`。
///
/// 顺序：先看首页里写的图标（`apple-touch-icon` 优先），再试 `/apple-touch-icon.png`
/// 与 `/favicon.ico`。只接受 PNG/ICO/JPG/GIF/BMP（看文件头），最大 [`MAX_ICON_BYTES`]。
/// 返回的 `relative` 是相对数据目录的写法，直接存回 `WebEngine.icon`。
///
/// **只访问该站点**；`fetch` 只用于单测替换联网。
pub fn download_icon(
    address: &str,
    name: &str,
    data_dir: &Path,
    fetch: Fetcher<'_>,
) -> Result<DownloadedIcon, String> {
    let page = fill_for_icon(address);
    let origin = origin_of(&page).ok_or_else(|| "不是网页地址（要 http:// 或 https:// 开头）".to_string())?;

    let mut candidates: Vec<String> = Vec::new();
    if let Ok(reply) = fetch(&format!("{origin}/"), MAX_PAGE_BYTES) {
        if reply.status == 200 && !reply.body.is_empty() && !reply.truncated {
            let scan = reply.body.len().min(PAGE_SCAN_BYTES);
            let html = String::from_utf8_lossy(&reply.body[..scan]).to_string();
            candidates = icon_candidates(&html, &format!("{origin}/"));
        }
    }
    for fallback in [
        format!("{origin}/apple-touch-icon.png"),
        format!("{origin}/favicon.ico"),
    ] {
        if !candidates.contains(&fallback) {
            candidates.push(fallback);
        }
    }

    let name = safe_icon_name(name, &origin);
    let mut faults = DownloadFaults::default();
    for candidate in candidates {
        faults.tried += 1;
        let Ok(reply) = fetch(&candidate, MAX_ICON_BYTES) else {
            faults.unreachable += 1;
            continue;
        };
        if reply.truncated {
            faults.too_big += 1;
            continue;
        }
        if reply.status != 200 || reply.body.is_empty() {
            faults.missing += 1;
            continue;
        }
        let ext = image_ext(&reply.body);
        if ext.is_empty() {
            faults.not_image += 1;
            continue;
        }
        let folder = data_dir.join("Icons");
        std::fs::create_dir_all(&folder).map_err(|e| format!("建不了 Icons 目录：{e}"))?;
        // 同名而格式不同的旧图标先删掉——否则 `Icons\g.png` 与 `Icons\g.ico` 会同时躺着，
        // 换一条引擎时谁也说不清现在用的是哪张。
        for old in ["png", "ico", "jpg", "gif", "bmp"] {
            let _ = std::fs::remove_file(folder.join(format!("{name}.{old}")));
        }
        std::fs::write(folder.join(format!("{name}.{ext}")), &reply.body)
            .map_err(|e| format!("写不了图标文件：{e}"))?;
        return Ok(DownloadedIcon {
            relative: format!("Icons\\{name}.{ext}"),
            ext: ext.to_string(),
            bytes: reply.body.len(),
        });
    }
    Err(failure_message(faults))
}

// ---- 联网：WinHTTP 直调（批次 D2 第二半；AHK 那边是 WinHttpRequest COM）----

/// GET 一个网址。`limit` 是**最多读多少字节**：资源更长时只给前 `limit` 字节并置
/// `truncated`，供调用方判"太大了"。超时照 AHK：解析 5 s、连接 5 s、发送 8 s、接收 8 s。
///
/// 走 `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`（跟系统代理设置，Win8.1+ 才有这个档）。
#[cfg(windows)]
pub fn fetch_winhttp(url: &str, limit: usize) -> Result<FetchReply, String> {
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use windows::core::PCWSTR;
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPEN_REQUEST_FLAGS,
        WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
        WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders,
        WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    };

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    let (secure, host, port, path) =
        split_http_url(url).ok_or_else(|| "不是网页地址".to_string())?;
    let agent = wide("Mozilla/5.0 (Windows NT 10.0; Win64; x64) egui-launcher");
    let whost = wide(&host);
    let wpath = wide(&path);
    let verb = wide("GET");

    unsafe {
        let session = Handle(WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err("WinHttpOpen 失败".to_string());
        }
        let _ = WinHttpSetTimeouts(session.0, 5000, 5000, 8000, 8000);
        let connect = Handle(WinHttpConnect(session.0, PCWSTR(whost.as_ptr()), port, 0));
        if connect.0.is_null() {
            return Err("连不上这个站点".to_string());
        }
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            PCWSTR(verb.as_ptr()),
            PCWSTR(wpath.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            if secure {
                WINHTTP_FLAG_SECURE
            } else {
                WINHTTP_OPEN_REQUEST_FLAGS(0)
            },
        ));
        if request.0.is_null() {
            return Err("WinHttpOpenRequest 失败".to_string());
        }
        WinHttpSendRequest(request.0, None, None, 0, 0, 0)
            .map_err(|e| format!("请求发不出去：{e}"))?;
        WinHttpReceiveResponse(request.0, null_mut()).map_err(|e| format!("等不到响应：{e}"))?;

        let mut status: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut len,
            null_mut(),
        )
        .map_err(|e| format!("读不到状态码：{e}"))?;

        let mut body: Vec<u8> = Vec::new();
        let mut truncated = false;
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let mut avail: u32 = 0;
            WinHttpQueryDataAvailable(request.0, &mut avail)
                .map_err(|e| format!("读响应失败：{e}"))?;
            if avail == 0 {
                break;
            }
            let want = (avail as usize).min(buf.len());
            let mut read: u32 = 0;
            WinHttpReadData(
                request.0,
                buf.as_mut_ptr() as *mut c_void,
                want as u32,
                &mut read,
            )
            .map_err(|e| format!("读响应失败：{e}"))?;
            if read == 0 {
                break;
            }
            let room = limit.saturating_sub(body.len());
            if read as usize > room {
                body.extend_from_slice(&buf[..room]);
                truncated = true;
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
        }
        Ok(FetchReply {
            status: status as u16,
            body,
            truncated,
        })
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 拆 `http(s)://host[:port][/path]`。没写端口按协议给 443 / 80；没写路径给 `/`。
#[cfg(windows)]
fn split_http_url(url: &str) -> Option<(bool, String, u16, String)> {
    let n = scheme_len(url)?;
    let secure = n == 8;
    let rest = &url[n..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p))
            if !h.is_empty() && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) =>
        {
            (h.to_string(), p.parse::<u16>().ok()?)
        }
        _ => (
            authority.to_string(),
            if secure { 443 } else { 80 },
        ),
    };
    let mut path = if end >= rest.len() {
        "/".to_string()
    } else {
        rest[end..].to_string()
    };
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    Some((secure, host, port, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "egui-launcher-siteicon-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// AHK `Tests/RunTests.ahk::SiteIcon` 那份夹具，一字不差。
    const FIXTURE: &str = concat!(
        r#"<html><head><link rel="icon" href="/favicon-16.png" sizes="16x16"><link rel="shortcut icon" href="/favicon.ico">"#,
        r#"<LINK REL="icon" TYPE="image/svg+xml" HREF="/icon.svg"><link href="//cdn.example.com/touch.png?v=2&amp;x=1" rel="apple-touch-icon">"#,
        r#"<link rel='icon' href='img/icon-64.png' sizes='64x64'><link rel=stylesheet href=/a.css><link rel=icon href=data:image/png;base64,AAAA></head>"#,
    );

    #[test]
    fn candidates_follow_the_reference_order() {
        let got = icon_candidates(FIXTURE, "https://www.example.com/sub/page.html?q=1");
        assert_eq!(
            got,
            vec![
                "https://cdn.example.com/touch.png?v=2&x=1",
                "https://www.example.com/sub/img/icon-64.png",
                "https://www.example.com/favicon.ico",
                "https://www.example.com/favicon-16.png",
            ]
        );
    }

    #[test]
    fn absolute_url_resolves_every_relative_form() {
        let base = "https://www.example.com/sub/page.html?q=1";
        assert_eq!(absolute_url("https://x.com/a.png", base), "https://x.com/a.png");
        assert_eq!(absolute_url("//cdn.x.com/a.png", base), "https://cdn.x.com/a.png");
        assert_eq!(absolute_url("/a.png", base), "https://www.example.com/a.png");
        assert_eq!(absolute_url("img/a.png", base), "https://www.example.com/sub/img/a.png");
        // 只有主机、没有路径：要补那一条斜杠
        assert_eq!(absolute_url("a.png", "https://x.com"), "https://x.com/a.png");
        // 不是 http(s) 的 base：原样返回
        assert_eq!(absolute_url("a.png", "file:///tmp/x"), "a.png");
    }

    #[test]
    fn image_type_reads_magic_numbers() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut ico = [0u8; 16];
        ico[2] = 1;
        let text = b"<html>.........";
        assert_eq!(image_ext(&png), "png");
        assert_eq!(image_ext(&ico), "ico");
        assert_eq!(image_ext(text), "");
        assert_eq!(image_ext(&[0xFF, 0xD8, 0xFF, 0, 0, 0]), "jpg");
        assert_eq!(image_ext(b"GIF89a.........."), "gif");
        // BMP 要 ≥26 字节：25 字节的同前缀不算
        let mut bmp = [0u8; 26];
        bmp[0] = b'B';
        bmp[1] = b'M';
        assert_eq!(image_ext(&bmp), "bmp");
        assert_eq!(image_ext(&bmp[..25]), "");
    }

    #[test]
    fn download_saves_the_first_real_image() {
        let dir = tmp_dir("save");
        std::fs::create_dir_all(dir.join("Icons")).unwrap();
        std::fs::write(dir.join("Icons").join("ex.png"), b"old").unwrap();

        let html = FIXTURE.as_bytes().to_vec();
        let mut ico = vec![0u8; 16];
        ico[2] = 1;
        let asked = std::cell::RefCell::new(Vec::<String>::new());
        let fetch = |target: &str, _limit: usize| -> Result<FetchReply, String> {
            asked.borrow_mut().push(target.to_string());
            let (status, body) = match target {
                "https://www.example.com/" => (200u16, html.clone()),
                "https://www.example.com/favicon.ico" => (200, ico.clone()),
                "https://www.example.com/sub/img/icon-64.png" => (200, b"<html>.........".to_vec()),
                _ => (404, Vec::new()),
            };
            Ok(FetchReply {
                status,
                body,
                truncated: false,
            })
        };
        let got = download_icon(
            "https://www.example.com/search?q={query}",
            "ex",
            &dir,
            &fetch,
        )
        .unwrap();

        assert_eq!(got.relative, "Icons\\ex.ico");
        assert_eq!(got.ext, "ico");
        let seen = asked.borrow();
        assert_eq!(seen.first().map(String::as_str), Some("https://www.example.com/"));
        drop(seen);
        assert_eq!(std::fs::read(dir.join("Icons").join("ex.ico")).unwrap().len(), 16);
        assert!(!dir.join("Icons").join("ex.png").exists(), "同名旧图标要删掉");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_uses_the_template_default_and_reports_the_host() {
        let dir = tmp_dir("default");
        let asked = std::cell::RefCell::new(Vec::<String>::new());
        let fetch = |target: &str, _limit: usize| -> Result<FetchReply, String> {
            asked.borrow_mut().push(target.to_string());
            Ok(FetchReply {
                status: 404,
                body: Vec::new(),
                truncated: false,
            })
        };
        let err = download_icon("http://localhost:{query|8000}/x", "", &dir, &fetch).unwrap_err();
        let seen = asked.borrow();
        assert_eq!(seen.last().map(String::as_str), Some("http://localhost:8000/favicon.ico"));
        drop(seen);
        assert!(err.contains("没找到"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_rejects_what_is_not_a_web_address() {        let dir = tmp_dir("badurl");
        let fetch = |_t: &str, _l: usize| -> Result<FetchReply, String> { Ok(FetchReply { status: 0, body: Vec::new(), truncated: false }) };
        assert!(download_icon("notes.txt", "x", &dir, &fetch).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_rejects_an_oversize_icon() {
        let dir = tmp_dir("big");
        let png = vec![0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let fetch = |_t: &str, _l: usize| -> Result<FetchReply, String> {
            Ok(FetchReply {
                status: 200,
                body: png.clone(),
                truncated: true,
            })
        };
        let err = download_icon("https://big.example.com/", "big", &dir, &fetch).unwrap_err();
        assert!(err.contains("超过 1 MB"), "{err}");
        assert!(!dir.join("Icons").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failure_message_points_at_the_reason() {
        assert!(failure_message(DownloadFaults::default()).contains("没有可用的图标地址"));
        assert!(
            failure_message(DownloadFaults { tried: 2, too_big: 1, missing: 1, ..Default::default() })
                .contains("超过 1 MB")
        );
        assert!(
            failure_message(DownloadFaults { tried: 2, unreachable: 2, ..Default::default() })
                .contains("连不上")
        );
        assert!(
            failure_message(DownloadFaults { tried: 2, not_image: 2, ..Default::default() })
                .contains("不是图片")
        );
        assert!(
            failure_message(DownloadFaults { tried: 2, missing: 2, ..Default::default() })
                .contains("没找到")
        );
        let mixed = DownloadFaults {
            tried: 3,
            missing: 1,
            unreachable: 1,
            not_image: 1,
            ..Default::default()
        };
        assert_eq!(
            failure_message(mixed),
            "没找到能用的图标（3 个地址：1 个没有、1 个连不上、1 个不是图片）"
        );
    }

    #[test]
    fn download_says_unreachable_when_every_fetch_fails() {
        let dir = tmp_dir("offline");
        let fetch = |_t: &str, _l: usize| -> Result<FetchReply, String> {
            Err("连不上这个站点".to_string())
        };
        let err = download_icon("https://offline.example.com/", "off", &dir, &fetch).unwrap_err();
        assert!(err.contains("连不上"), "{err}");
        assert!(!dir.join("Icons").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_says_not_an_image_when_the_bytes_are_html() {
        let dir = tmp_dir("notimage");
        let fetch = |_t: &str, _l: usize| -> Result<FetchReply, String> {
            Ok(FetchReply {
                status: 200,
                body: b"<html>.........".to_vec(),
                truncated: false,
            })
        };
        let err =
            download_icon("https://plain.example.com/", "plain", &dir, &fetch).unwrap_err();
        assert!(err.contains("不是图片"), "{err}");
        assert!(!dir.join("Icons").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_mixes_the_reasons_when_candidates_fail_differently() {
        let dir = tmp_dir("mixed");
        let fetch = |target: &str, _l: usize| -> Result<FetchReply, String> {
            match target {
                // 首页 404 ⇒ 没有 <link> 候选，只剩两条兜底
                "https://www.mixed.com/" => Ok(FetchReply {
                    status: 404,
                    body: Vec::new(),
                    truncated: false,
                }),
                "https://www.mixed.com/apple-touch-icon.png" => {
                    Err("连不上这个站点".to_string())
                }
                _ => Ok(FetchReply {
                    status: 404,
                    body: Vec::new(),
                    truncated: false,
                }),
            }
        };
        let err = download_icon("https://www.mixed.com/", "mix", &dir, &fetch).unwrap_err();
        assert!(err.contains("1 个没有") && err.contains("1 个连不上"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn icon_name_cannot_escape_the_folder() {
        assert_eq!(safe_icon_name("../../evil", "https://x.com"), ".._.._evil");
        assert_eq!(safe_icon_name("", "https://www.x.com"), "www.x.com");
        assert_eq!(safe_icon_name("  ", "https://www.x.com"), "www.x.com");
        assert_eq!(safe_icon_name("g", "https://x.com"), "g");
    }

    #[test]
    fn web_icon_spec_follows_the_reference_rules() {
        let dir = tmp_dir("spec");
        assert_eq!(web_icon_spec("", &dir), "url:");
        assert_eq!(web_icon_spec("   ", &dir), "url:");
        assert_eq!(web_icon_spec("res:imageres.dll,-114", &dir), "res:imageres.dll,-114");
        assert_eq!(web_icon_spec("url:https://x.com", &dir), "url:https://x.com");
        // 不存在的相对路径 → 浏览器默认图标
        assert_eq!(web_icon_spec("Icons\\nope.png", &dir), "url:");

        std::fs::create_dir_all(dir.join("Icons")).unwrap();
        std::fs::write(dir.join("Icons").join("g.png"), [0u8; 4]).unwrap();
        std::fs::write(dir.join("Icons").join("h.ico"), [0u8; 4]).unwrap();
        let png = web_icon_spec("Icons\\g.png", &dir);
        assert!(png.starts_with("thumb:"), "{png}");
        assert!(png.ends_with("Icons\\g.png"), "{png}");
        // .ico 走裸路径（shell 自己取图标），不进 thumb
        let ico = web_icon_spec("Icons/h.ico", &dir);
        assert!(!ico.starts_with("thumb:"), "{ico}");
        assert!(ico.ends_with("h.ico"), "{ico}");

        // 绝对路径照用
        let abs = dir.join("Icons").join("g.png").to_string_lossy().to_string();
        assert_eq!(web_icon_spec(&abs, Path::new("C:\\nowhere")), format!("thumb:{abs}"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fill_for_icon_uses_defaults_and_drops_the_placeholder() {
        assert_eq!(
            fill_for_icon("http://localhost:{query|8000}/x"),
            "http://localhost:8000/x"
        );
        assert_eq!(fill_for_icon("https://x.com/s?q={query}"), "https://x.com/s?q=");
        assert_eq!(fill_for_icon("https://x.com/"), "https://x.com/");
        assert_eq!(fill_for_icon("https://x.com/{query"), "https://x.com/{query");
    }

    #[test]
    fn origin_and_url_split() {
        assert_eq!(origin_of("https://www.example.com/a?b=1"), Some("https://www.example.com".into()));
        assert_eq!(origin_of("http://localhost:8000/x"), Some("http://localhost:8000".into()));
        assert_eq!(origin_of("http://host:/x"), Some("http://host".into()));
        assert_eq!(origin_of("notes.txt"), None);
        assert_eq!(
            split_http_url("https://x.com/a/b?c=1"),
            Some((true, "x.com".to_string(), 443, "/a/b?c=1".to_string()))
        );
        assert_eq!(
            split_http_url("http://127.0.0.1:81"),
            Some((false, "127.0.0.1".to_string(), 81, "/".to_string()))
        );
    }

    /// **要联网**的证伪实验（平时不跑：`cargo test siteicon_real_network -- --ignored`）。
    /// 单测替掉的是 `fetch`，也就是"取回来之后怎么判"；这条补的是单测够不着的那一段：
    /// `fetch_winhttp` 真能把首页与 `/favicon.ico` 拿回来、候选顺序与文件头校验在真响应上照旧成立。
    /// 断言的落点是"磁盘上出现一张真图片、字节数与回执一致"。
    #[test]
    #[ignore = "要联网：手动 cargo test siteicon_real_network -- --ignored 才跑"]
    fn siteicon_real_network() {
        let dir = tmp_dir("net");
        let fetch = |url: &str, limit: usize| fetch_winhttp(url, limit);
        let got = download_icon("https://www.bing.com/search?q={query}", "netprobe", &dir, &fetch)
            .expect("联网取图标应当成功");
        let path = dir.join("Icons").join(format!("netprobe.{}", got.ext));
        let bytes = std::fs::read(&path).expect("图标没落盘");
        log_info!("联网探针：{}，{} 字节，{} 格式", got.relative, got.bytes, got.ext);
        assert_eq!(bytes.len(), got.bytes, "回执字节数与文件不一致");
        assert!(!image_ext(&bytes).is_empty(), "落盘的不是能认的图片");
        assert_eq!(got.relative, format!("Icons\\netprobe.{}", got.ext));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

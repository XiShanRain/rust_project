//! FR-SRC-03 图标提取：把条目 `source_path` 里的图标规格转成 RGBA 像素，供主线程 load_texture。
//!
//! 规格语法（与 AHK 版 `Src\UI\IconCache.ahk` 同口径，直接写在 `source_path` 里）：
//!
//! | 写法 | 取法 |
//! |---|---|
//! | `res:imageres.dll,-114` | `PrivateExtractIconsW`，负数＝资源 id，非负＝序号 |
//! | `ext:.pdf` | `SHGetFileInfoW` + `SHGFI_USEFILEATTRIBUTES`，文件不必存在 |
//! | `url:<任意 url>` | 同上，作用在 `.url` 上（浏览器默认图标） |
//! | `folder:<路径>` | 同上，带 `FILE_ATTRIBUTE_DIRECTORY` |
//! | `thumb:<路径>` | `IShellItemImageFactory::GetImage`，取不到退普通文件图标 |
//! | 其余 | 裸路径走 `SHGetFileInfoW`（真文件，可能带自定义图标） |
//!
//! 缓存键由 [`cache_key`] 决定：可执行/快捷方式这类「每条一个图标」的按路径，
//! 文档类按扩展名（一堆 `.pdf` 共用一张纹理），带前缀的规格按规格原文小写。
//!
//! 管线全在后台线程执行，绝不碰 egui：
//! 取 `HICON` → 32bpp DIB Section → `DrawIconEx(DI_NORMAL)` → 去预乘 BGRA→RGBA。
//! 老式（无 alpha）图标再走一趟 `GetIconInfo` + `GetDIBits(hbmMask)` 补 alpha，
//! 白＝透明、黑＝不透明；遮罩也取不到时才退化成整幅不透明。

use std::collections::HashMap;
use std::ffi::c_void;

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetObjectW, HDC, HGDIOBJ, HBITMAP,
    SelectObject,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize, IBindCtx,
};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON,
    SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SIIGBF, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
    SIIGBF_THUMBNAILONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DI_NORMAL, DestroyIcon, DrawIconEx, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW,
};
use windows::core::PCWSTR;

/// 纹理缓存上限。与 AHK 版 `MaxIcons` 一致。
pub const ICON_CACHE_MAX: usize = 1500;
/// 超限后修剪到的条数（`ICON_CACHE_MAX * 80%`）。
pub const ICON_CACHE_KEEP: usize = 1200;

/// 这些扩展名的图标「每条都不一样」（快捷方式带自己的 IconLocation），缓存键用整条路径。
/// 其余扩展名按扩展名共用一张纹理——与 AHK `_CacheKey` 同一份名单。
const PER_PATH_EXTS: [&str; 8] = ["exe", "lnk", "ico", "url", "appref-ms", "msc", "cpl", "scr"];

/// 解析后的图标规格。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconSpec {
    /// 裸路径或任何认不出的写法：交给 shell 按真文件取图标。
    Path(String),
    /// `res:<dll 或 exe>,<序号 | -资源id>`
    Res { file: String, index: i32 },
    /// `ext:.pdf`（已归一成小写、带点）
    Ext(String),
    /// `url:<任意>`
    Url,
    /// `folder:<路径>`，载荷可空＝通用文件夹图标
    Folder(String),
    /// `thumb:<路径>`
    Thumb(String),
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `PrivateExtractIconsW` 只收 `&[u16; 260]`（MAX_PATH），超长路径截断。
fn wide_fixed260(s: &str) -> [u16; 260] {
    let mut buf = [0u16; 260];
    for (dst, src) in buf.iter_mut().zip(s.encode_utf16()).take(259) {
        *dst = src;
    }
    buf
}

/// 扩展名小写、不带点。`C:\a\b.PDF` ⇒ `Some("pdf")`；`C:\Windows` ⇒ `None`。
fn ext_of(path: &str) -> Option<String> {
    let tail = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let dot = tail.rfind('.')?;
    let e = tail[dot + 1..].trim();
    (!e.is_empty()).then(|| e.to_ascii_lowercase())
}

/// 把 `source_path` 的原文解析成规格。空串返回 `None`（这条没有图标）。
///
/// 认不出的写法一律回落 [`IconSpec::Path`]：宁可让 shell 试一次失败，也不要静默吞掉
/// 用户在「图标源」框里手敲的内容。
pub fn parse_spec(raw: &str) -> Option<IconSpec> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = strip_prefix_ci(s, "res:") {
        if let Some((file, idx)) = rest.rsplit_once(',') {
            let file = file.trim();
            if !file.is_empty() {
                if let Ok(index) = idx.trim().parse::<i32>() {
                    return Some(IconSpec::Res {
                        file: file.to_string(),
                        index,
                    });
                }
            }
        }
        return Some(IconSpec::Path(s.to_string()));
    }
    if let Some(rest) = strip_prefix_ci(s, "ext:") {
        let e = rest.trim().trim_start_matches('.').trim();
        if !e.is_empty() {
            return Some(IconSpec::Ext(format!(".{}", e.to_ascii_lowercase())));
        }
        return Some(IconSpec::Path(s.to_string()));
    }
    if let Some(rest) = strip_prefix_ci(s, "url:") {
        let _ = rest;
        return Some(IconSpec::Url);
    }
    if let Some(rest) = strip_prefix_ci(s, "folder:") {
        return Some(IconSpec::Folder(rest.trim().to_string()));
    }
    if let Some(rest) = strip_prefix_ci(s, "thumb:") {
        let p = rest.trim();
        if !p.is_empty() {
            return Some(IconSpec::Thumb(p.to_string()));
        }
        return Some(IconSpec::Path(s.to_string()));
    }
    Some(IconSpec::Path(s.to_string()))
}

/// 纹理缓存的键。`None` ＝这条没有图标（`source_path` 为空）。
///
/// 规则（对照 AHK `_CacheKey`）：
/// - 带前缀的规格 ⇒ 整条规格小写（同一批 `.pdf` 共用 `ext:.pdf`，36 个设置页共用一条 `res:`）
/// - 裸路径：`PER_PATH_EXTS` 里的或没有扩展名的 ⇒ 整条路径小写；其余 ⇒ `ext:.xxx`
pub fn cache_key(raw: &str) -> Option<String> {
    match parse_spec(raw)? {
        IconSpec::Path(p) => match ext_of(&p) {
            Some(e) if !PER_PATH_EXTS.contains(&e.as_str()) => Some(format!("ext:.{e}")),
            _ => Some(p.to_ascii_lowercase()),
        },
        IconSpec::Ext(e) => Some(format!("ext:{}", e.to_ascii_lowercase())),
        other => Some(spec_text(&other).to_ascii_lowercase()),
    }
}

/// 规格的规范文本（用于缓存键）。
fn spec_text(spec: &IconSpec) -> String {
    match spec {
        IconSpec::Path(p) => p.clone(),
        IconSpec::Res { file, index } => format!("res:{file},{index}"),
        IconSpec::Ext(e) => format!("ext:{e}"),
        IconSpec::Url => "url:".to_string(),
        IconSpec::Folder(p) => format!("folder:{p}"),
        IconSpec::Thumb(p) => format!("thumb:{p}"),
    }
}

/// 日志里的规格简称：**只出文件名或类别，绝不出目录**（日志不得留用户路径）。
pub fn log_label(raw: &str) -> String {
    let tail = |p: &str| p.rsplit(['\\', '/']).next().unwrap_or("?").to_string();
    match parse_spec(raw) {
        None => "?".to_string(),
        Some(IconSpec::Path(p)) => tail(&p),
        Some(IconSpec::Res { file, index }) => format!("res:{},{index}", tail(&file)),
        Some(IconSpec::Ext(e)) => format!("ext:{e}"),
        Some(IconSpec::Url) => "url:".to_string(),
        Some(IconSpec::Folder(_)) => "folder:".to_string(),
        Some(IconSpec::Thumb(p)) => format!("thumb:{}", tail(&p)),
    }
}

/// 缓存超限时该淘汰哪些键：按 `last_use` 升序（同刻按键名）取最旧的，淘汰到 `ICON_CACHE_KEEP`。
pub fn evict_keys(last_use: &HashMap<String, u64>) -> Vec<String> {
    if last_use.len() <= ICON_CACHE_MAX {
        return Vec::new();
    }
    let excess = last_use.len() - ICON_CACHE_KEEP;
    let mut v: Vec<(&String, &u64)> = last_use.iter().collect();
    v.sort_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(b.0)));
    v.into_iter().take(excess).map(|(k, _)| k.clone()).collect()
}

/// 盘符开头的绝对路径里出现 `/` 时统一成 `\`。
/// 2026-10-29 真机抓到：出厂种子的 PowerShell 图标源曾是
/// `C:\WINDOWS\System32\WindowsPowerShell/v1.0/powershell.exe`（`system32()` 拼进来的那段带正斜杠），
/// `SHGetFileInfoW` 对混用分隔符的路径直接返回 0 ⇒ 那一行恒无图标。老清单里已经存下这种写法，
/// 所以在这一层兜，不只改种子。
/// 只动盘符路径：`https://…` 这类交给 shell 试的写法原样保留。
fn normalize_path(p: &str) -> std::borrow::Cow<'_, str> {
    let b = p.as_bytes();
    let drive_abs = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    if drive_abs && p.contains('/') {
        std::borrow::Cow::Owned(p.replace('/', "\\"))
    } else {
        std::borrow::Cow::Borrowed(p)
    }
}

/// 提取 `raw` 规格的图标，转成 (straight-alpha RGBA, width, height)。
/// `px` 是请求的物理像素边长（调用方按 DPI 算好，见 `dpi::icon_px`）；
/// 真图标走 shell 的按尺寸取图，扩展名 / `.url` / 资源索引这类没有磁盘实体的
/// 由系统按属性给图，只有 LARGEICON 一档。
/// 失败返回 None（留占位）。
pub fn extract_at(raw: &str, px: u32) -> Option<(Vec<u8>, u32, u32)> {
    let spec = parse_spec(raw)?;
    let px = px.clamp(16, 256);
    unsafe {
        let icon_image = |path: &str| {
            image_rgba(path, px, px, SIIGBF_BIGGERSIZEOK | SIIGBF_ICONONLY)
        };
        match &spec {
            IconSpec::Thumb(path) => {
                let path = normalize_path(path);
                if let Some(hit) =
                    image_rgba(&path, px, px, SIIGBF_BIGGERSIZEOK | SIIGBF_THUMBNAILONLY)
                {
                    return Some(hit);
                }
                // 缩略图取不到（文件没有缩略图 / 不是文件）⇒ 退图标。
                if let Some(hit) = icon_image(&path) {
                    return Some(hit);
                }
                from_hicon(shell_icon(&path, None), px, px)
            }
            // 真实路径：先让 shell 按目标像素直接给图，比把 32px 系统图标放大清楚。
            IconSpec::Path(p) => {
                let path = normalize_path(p);
                if let Some(hit) = icon_image(&path) {
                    return Some(hit);
                }
                from_hicon(shell_icon(&path, None), px, px)
            }
            IconSpec::Folder(p) => {
                if !p.is_empty() {
                    if let Some(hit) = icon_image(p) {
                        return Some(hit);
                    }
                }
                from_hicon(hicon_for(&spec, px, px), px, px)
            }
            _ => from_hicon(hicon_for(&spec, px, px), px, px),
        }
    }
}

/// 规格 → `HICON`（`thumb:` 已在上层分流，这里不会收到）。
unsafe fn hicon_for(spec: &IconSpec, cx: u32, cy: u32) -> Option<HICON> {
    unsafe {
        match spec {
            IconSpec::Path(p) => shell_icon(&normalize_path(p), None),
            IconSpec::Ext(e) => shell_icon(e, Some(FILE_ATTRIBUTE_NORMAL)),
            IconSpec::Url => shell_icon(".url", Some(FILE_ATTRIBUTE_NORMAL)),
            IconSpec::Folder(p) => {
                let target = if p.is_empty() { "folder" } else { p.as_str() };
                shell_icon(target, Some(FILE_ATTRIBUTE_DIRECTORY))
            }
            IconSpec::Thumb(p) => shell_icon(p, None),
            IconSpec::Res { file, index } => {
                let wide = wide_fixed260(file);
                let mut icons = [HICON::default(); 1];
                let n = PrivateExtractIconsW(
                    &wide,
                    *index,
                    cx as i32,
                    cy as i32,
                    Some(&mut icons),
                    None,
                    0,
                );
                (n > 0 && !icons[0].0.is_null()).then_some(icons[0])
            }
        }
    }
}

/// `SHGetFileInfoW(SHGFI_ICON|SHGFI_LARGEICON)`。`attrs` 给 `Some(..)` 时再带
/// `SHGFI_USEFILEATTRIBUTES`——那条路径不必真实存在，shell 只按扩展名/属性给图。
unsafe fn shell_icon(path: &str, attrs: Option<FILE_FLAGS_AND_ATTRIBUTES>) -> Option<HICON> {
    unsafe {
        let wide = wide_null(path);
        let mut shfi: SHFILEINFOW = core::mem::zeroed();
        let mut flags = SHGFI_ICON | SHGFI_LARGEICON;
        if attrs.is_some() {
            flags |= SHGFI_USEFILEATTRIBUTES;
        }
        let ok = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            attrs.unwrap_or(FILE_FLAGS_AND_ATTRIBUTES(0)),
            Some(&mut shfi),
            core::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        );
        (ok != 0 && !shfi.hIcon.0.is_null()).then_some(shfi.hIcon)
    }
}

/// `HICON` → straight-alpha RGBA。`hicon` 的释放交给调用方（这里只管画）。
unsafe fn from_hicon(hicon: Option<HICON>, cx: u32, cy: u32) -> Option<(Vec<u8>, u32, u32)> {
    let hicon = hicon?;
    unsafe {
        // 遮罩是按位图原生尺寸取的，画得比原生大时 `GetDIBits` 整块失败 ⇒ 老图标会退成
        // 整幅不透明（黑角）。所以绘制尺寸一律夹到原生尺寸：系统只给 32px 的图标，
        // 就老老实实画 32px 再由 egui 缩放。
        let (cx, cy) = match hicon_size(hicon) {
            Some((nw, nh)) => (cx.min(nw).max(1), cy.min(nh).max(1)),
            None => (cx, cy),
        };
        let rgba = render_icon_to_rgba(hicon, cx, cy);
        let result = rgba.map(|mut buf| {
            if !unpremultiply(&mut buf) {
                // 老式图标：DIB 里没有 alpha，靠 AND 遮罩补；遮罩也拿不到才整幅不透明。
                match mask_alpha(hicon, cx, cy) {
                    Some(mask) => {
                        for (px, a) in buf.chunks_exact_mut(4).zip(mask) {
                            px[3] = a;
                        }
                    }
                    None => {
                        for px in buf.chunks_exact_mut(4) {
                            px[3] = 255;
                        }
                    }
                }
            }
            (buf, cx, cy)
        });
        let _ = DestroyIcon(hicon);
        result
    }
}

/// 预乘 BGRA → straight RGBA，原地改。整幅 alpha 全 0 时返回 false 且**不动 RGB**
/// （老式图标那一路要留着 DrawIconEx 画出来的颜色，只补 alpha）。
fn unpremultiply(buf: &mut [u8]) -> bool {
    let any_alpha = buf.chunks_exact(4).any(|px| px[3] > 0);
    if !any_alpha {
        return false;
    }
    for px in buf.chunks_exact_mut(4) {
        let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
        if a > 0 {
            let scale = |v: u8| ((v as u32 * 255) / a as u32).min(255) as u8;
            px[0] = scale(r);
            px[1] = scale(g);
            px[2] = scale(b);
        } else {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        }
    }
    true
}

/// `HICON` 的彩色位图原生尺寸。单色图标没有 `hbmColor`，返回 `None`。
unsafe fn hicon_size(hicon: HICON) -> Option<(u32, u32)> {
    unsafe {
        let mut ii: ICONINFO = core::mem::zeroed();
        if GetIconInfo(hicon, &mut ii).is_err() {
            return None;
        }
        let mut size = None;
        if !ii.hbmColor.0.is_null() {
            let mut bm: BITMAP = core::mem::zeroed();
            let got = GetObjectW(
                HGDIOBJ(ii.hbmColor.0),
                core::mem::size_of::<BITMAP>() as i32,
                Some((&mut bm as *mut BITMAP).cast::<c_void>()),
            );
            if got > 0 && bm.bmWidth > 0 && bm.bmHeight > 0 {
                size = Some((bm.bmWidth as u32, bm.bmHeight as u32));
            }
        }
        if !ii.hbmColor.0.is_null() {
            let _ = DeleteObject(ii.hbmColor.into());
        }
        if !ii.hbmMask.0.is_null() {
            let _ = DeleteObject(ii.hbmMask.into());
        }
        size
    }
}

/// `ICONINFO.hbmMask` → 每像素 alpha（黑＝不透明 255，白＝透明 0）。
/// 取不到遮罩返回 `None`，调用方退化成整幅不透明。
unsafe fn mask_alpha(hicon: HICON, w: u32, h: u32) -> Option<Vec<u8>> {
    unsafe {
        let mut ii: ICONINFO = core::mem::zeroed();
        if GetIconInfo(hicon, &mut ii).is_err() || ii.hbmMask.0.is_null() {
            return None;
        }
        let bits = dib_bits(ii.hbmMask, w, h);
        let _ = DeleteObject(ii.hbmMask.into());
        if !ii.hbmColor.0.is_null() {
            let _ = DeleteObject(ii.hbmColor.into());
        }
        let bits = bits?;
        // 1bpp 掩码转 32bpp 后是 0x00000000（不透明）与 0x00FFFFFF（透明）。
        Some(bits.chunks_exact(4).map(|p| if p[0] == 0 { 255 } else { 0 }).collect())
    }
}

/// 任意 `HBITMAP` 的 32bpp top-down 像素（预乘 BGRA）。
unsafe fn dib_bits(hbm: HBITMAP, w: u32, h: u32) -> Option<Vec<u8>> {
    unsafe {
        let mut bmi: BITMAPINFO = core::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: core::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // 负值 = top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let hdc: HDC = CreateCompatibleDC(None);
        let got = GetDIBits(
            hdc,
            hbm,
            0,
            h,
            Some(buf.as_mut_ptr().cast::<c_void>()),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(hdc);
        (got != 0).then_some(buf)
    }
}

/// `GetImage` 的通用外壳：`thumb:` 走缩略图，高 DPI 下取图标走 `SIIGBF_ICONONLY`。
/// 不是文件、取不到图、COM 起不来都返回 `None`。
///
/// 返回的尺寸是 shell 实际给的（未必等于请求的 cx/cy），调用方按实际宽高建纹理。
unsafe fn image_rgba(
    path: &str,
    cx: u32,
    cy: u32,
    flags: SIIGBF,
) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        // 加载线程多半没初始化过 COM；自己起一次、用完自己收。
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let ours = hr.is_ok();
        let result = (|| {
            let wide = wide_null(path);
            let factory: IShellItemImageFactory =
                SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>).ok()?;
            let size = SIZE {
                cx: cx as i32,
                cy: cy as i32,
            };
            let hbm = factory.GetImage(size, flags).ok()?;
            let mut bm: BITMAP = core::mem::zeroed();
            let got = GetObjectW(
                HGDIOBJ(hbm.0),
                core::mem::size_of::<BITMAP>() as i32,
                Some((&mut bm as *mut BITMAP).cast::<c_void>()),
            );
            let (w, h) = (bm.bmWidth.max(1) as u32, bm.bmHeight.max(1) as u32);
            let bits = if got > 0 { dib_bits(hbm, w, h) } else { None };
            let _ = DeleteObject(hbm.into());
            let mut bits = bits?;
            if !unpremultiply(&mut bits) {
                for px in bits.chunks_exact_mut(4) {
                    px[3] = 255;
                }
            }
            Some((bits, w, h))
        })();
        if ours {
            CoUninitialize();
        }
        result
    }
}

/// 把 `HICON` 画进一块 32bpp top-down DIB，拷贝回 (w*h*4) 的预乘 BGRA 字节缓冲。
unsafe fn render_icon_to_rgba(hicon: HICON, w: u32, h: u32) -> Option<Vec<u8>> {
    unsafe {
        let mut bmi: BITMAPINFO = core::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: core::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // 负值 = top-down，第 0 行是顶行
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };

        let mut pbits: *mut c_void = core::ptr::null_mut();
        let hdc: HDC = CreateCompatibleDC(None);
        let hbm = CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut pbits, None, 0).ok()?;
        if pbits.is_null() {
            let _ = DeleteDC(hdc);
            return None;
        }
        let old = SelectObject(hdc, HGDIOBJ(hbm.0));
        let _ = DrawIconEx(hdc, 0, 0, hicon, w as i32, h as i32, 0, None, DI_NORMAL);

        let bytes = (w * h * 4) as usize;
        let mut buf = vec![0u8; bytes];
        std::ptr::copy_nonoverlapping(pbits as *const u8, buf.as_mut_ptr(), bytes);

        SelectObject(hdc, old);
        let _ = DeleteObject(hbm.into());
        let _ = DeleteDC(hdc);
        Some(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_spec_recognises_every_prefix() {
        assert_eq!(
            parse_spec("res:imageres.dll,-114"),
            Some(IconSpec::Res {
                file: "imageres.dll".into(),
                index: -114
            })
        );
        assert_eq!(
            parse_spec(r"RES:C:\Windows\shell32.dll, 3"),
            Some(IconSpec::Res {
                file: r"C:\Windows\shell32.dll".into(),
                index: 3
            })
        );
        assert_eq!(parse_spec("ext:.PDF"), Some(IconSpec::Ext(".pdf".into())));
        assert_eq!(parse_spec("ext:pdf"), Some(IconSpec::Ext(".pdf".into())));
        assert_eq!(parse_spec("url:https://example.com"), Some(IconSpec::Url));
        assert_eq!(parse_spec("folder:"), Some(IconSpec::Folder(String::new())));
        assert_eq!(
            parse_spec(r"thumb:C:\a\b.mp4"),
            Some(IconSpec::Thumb(r"C:\a\b.mp4".into()))
        );
        assert_eq!(
            parse_spec(r"C:\Windows\notepad.exe"),
            Some(IconSpec::Path(r"C:\Windows\notepad.exe".into()))
        );
        assert_eq!(
            parse_spec("shell:RecycleBinFolder"),
            Some(IconSpec::Path("shell:RecycleBinFolder".into()))
        );
        assert_eq!(parse_spec("   "), None);
    }

    #[test]
    fn malformed_prefixed_spec_falls_back_to_path() {
        // 少了索引 / 少了扩展名：不静默吞掉，交给 shell 试一次。
        assert_eq!(
            parse_spec("res:imageres.dll"),
            Some(IconSpec::Path("res:imageres.dll".into()))
        );
        assert_eq!(parse_spec("ext:"), Some(IconSpec::Path("ext:".into())));
        assert_eq!(parse_spec("thumb:  "), Some(IconSpec::Path("thumb:".into())));
        assert_eq!(
            parse_spec("res:,7"),
            Some(IconSpec::Path("res:,7".into())),
            "文件名为空也不算规格"
        );
    }

    #[test]
    fn cache_key_groups_documents_but_not_executables() {
        assert_eq!(
            cache_key(r"D:\a\报告.pdf").as_deref(),
            Some("ext:.pdf"),
            "一堆 .pdf 共用一张纹理"
        );
        assert_eq!(cache_key(r"D:\b\另一个.PDF").as_deref(), Some("ext:.pdf"));
        assert_eq!(
            cache_key(r"D:\GreenTool\App.exe").as_deref(),
            Some(r"d:\greentool\app.exe"),
            "exe 每条一个图标"
        );
        assert_eq!(
            cache_key(r"D:\桌面\某某.lnk").as_deref(),
            Some(r"d:\桌面\某某.lnk"),
            "快捷方式带自己的 IconLocation"
        );
        assert_eq!(
            cache_key(r"C:\Windows").as_deref(),
            Some(r"c:\windows"),
            "无扩展名按路径"
        );
        assert_eq!(
            cache_key("shell:MyComputerFolder").as_deref(),
            Some("shell:mycomputerfolder")
        );
    }

    #[test]
    fn cache_key_keeps_prefixed_specs_whole() {
        assert_eq!(
            cache_key("res:imageres.dll,-114").as_deref(),
            Some("res:imageres.dll,-114")
        );
        assert_eq!(cache_key("ext:.pdf").as_deref(), Some("ext:.pdf"));
        assert_eq!(cache_key("url:anything").as_deref(), Some("url:"));
        assert_eq!(cache_key("folder:").as_deref(), Some("folder:"));
        assert_eq!(
            cache_key(r"thumb:C:\a\b.mp4").as_deref(),
            Some(r"thumb:c:\a\b.mp4")
        );
        assert_eq!(cache_key(""), None, "空白 source_path 没有图标");
    }

    #[test]
    fn evict_keys_trims_to_eighty_percent_only_when_over_max() {
        let mk = |n: usize| -> HashMap<String, u64> {
            (0..n).map(|i| (format!("k{i:05}"), i as u64)).collect()
        };
        assert!(evict_keys(&mk(ICON_CACHE_MAX)).is_empty(), "刚好满不修剪");
        let over = evict_keys(&mk(ICON_CACHE_MAX + 1));
        assert_eq!(
            over.len(),
            ICON_CACHE_MAX + 1 - ICON_CACHE_KEEP,
            "1510 条要剪掉 310 条"
        );
        assert_eq!(over[0], "k00000", "先淘汰最久没用的");
        assert_eq!(over[over.len() - 1], format!("k{:05}", over.len() - 1));
        assert_eq!(evict_keys(&mk(3000)).len(), 3000 - ICON_CACHE_KEEP);
    }

    #[test]
    fn evict_keys_breaks_ties_by_key_for_determinism() {
        let mut m = HashMap::new();
        for i in 0..ICON_CACHE_MAX {
            m.insert(format!("a{i:05}"), 7);
        }
        m.insert("b00000".to_string(), 7);
        let over = evict_keys(&m);
        assert_eq!(over.len(), ICON_CACHE_MAX + 1 - ICON_CACHE_KEEP);
        assert_eq!(over[0], "a00000", "同刻按键名升序，结果可复现");
    }

    /// 真机抓到的那条：`system32()` 拼进来的段带正斜杠 ⇒ `SHGetFileInfoW` 返回 0。
    /// 盘符路径里的 `/` 一律归一成 `\`；URL 与相对命令名原样不动。
    #[test]
    fn drive_absolute_paths_get_their_slashes_normalised() {
        assert_eq!(
            normalize_path(r"C:\WINDOWS\System32\WindowsPowerShell/v1.0/powershell.exe"),
            r"C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe"
        );
        assert_eq!(normalize_path(r"c:/a/b.txt"), r"c:\a\b.txt");
        // 本来就干净的路径一次分配都不做（Cow::Borrowed）。
        assert!(matches!(
            normalize_path(r"C:\a\b.txt"),
            std::borrow::Cow::Borrowed(_)
        ));
        assert_eq!(normalize_path("https://example.com/a/b"), "https://example.com/a/b");
        assert_eq!(normalize_path("calc.exe"), "calc.exe");
        assert_eq!(normalize_path(""), "");
    }
}

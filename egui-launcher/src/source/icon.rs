//! FR-SRC-03 图标提取：把 exe / .lnk 的图标转成 RGBA 像素，供主线程 load_texture。
//!
//! 管线（全部在后台线程执行，绝不碰 egui）：
//! `SHGetFileInfoW(SHGFI_ICON|SHGFI_LARGEICON)` → `HICON` → 32bpp DIB Section →
//! `DrawIconEx(DI_NORMAL)` → 去预乘 BGRA→RGBA。返回的字节是 straight-alpha RGBA。
//!
//! 简化处（后续精细化）：无 alpha 的老式 8bpp 图标暂按不透明处理（背景可能偏黑），
//! 未接 GetIconInfo 的 AND 遮罩；LRU 缓存 1500 尚未做。

use std::ffi::c_void;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW};
use windows::Win32::UI::WindowsAndMessaging::{
    DI_NORMAL, DestroyIcon, DrawIconEx, GetSystemMetrics, HICON, SM_CXICON, SM_CYICON,
};
use windows::core::PCWSTR;

fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 提取 `path` 的大图标，转成 (straight-alpha RGBA, width, height)。失败返回 None（留占位）。
pub fn extract(path: &str) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        let wide = wide_null(path);
        let mut shfi: SHFILEINFOW = core::mem::zeroed();
        let ok = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut shfi),
            core::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        let hicon: HICON = shfi.hIcon;
        if ok == 0 || hicon.0.is_null() {
            return None;
        }

        let cx = GetSystemMetrics(SM_CXICON).max(1) as u32;
        let cy = GetSystemMetrics(SM_CYICON).max(1) as u32;
        let mut rgba = match render_icon_to_rgba(hicon, cx, cy) {
            Some(buf) => buf,
            None => {
                let _ = DestroyIcon(hicon);
                return None;
            }
        };
        let _ = DestroyIcon(hicon);

        // 去预乘 BGRA → straight RGBA；若整幅 alpha 全 0（老式图标无 alpha），退化为不透明。
        let mut has_alpha = false;
        for px in rgba.chunks_exact_mut(4) {
            // DIB 内存布局为 BGRA（预乘）
            let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
            if a > 0 {
                has_alpha = true;
                let scale = |v: u8| ((v as u32 * 255) / a as u32).min(255) as u8;
                let (nr, ng, nb) = (scale(r), scale(g), scale(b));
                px[0] = nr;
                px[1] = ng;
                px[2] = nb;
                px[3] = a;
            } else {
                px[0] = 0;
                px[1] = 0;
                px[2] = 0;
                px[3] = 0;
            }
        }
        if !has_alpha {
            for px in rgba.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }

        Some((rgba, cx, cy))
    }
}

/// 把 HICON 画进一块 32bpp top-down DIB，拷贝回 (w*h*4) 的预乘 BGRA 字节缓冲。
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

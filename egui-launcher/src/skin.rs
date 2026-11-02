//! 批次 SKIN（FR-UIX-11 皮肤与提示音）：背景图的解码 + 几何，和呼出提示音的播放。
//!
//! 两条通道互不依赖，也都不碰纹理生命周期（建/换/清在 `src/main.rs`）：
//! - [`decode`]：WIC（Windows Imaging Component）解任何它能认的容器（bmp / png / jpg / tif / gif…），
//!   一次调用出 RGBA 缓冲。
//! - [`play_popup_sound`]：`PlaySoundW(SND_RESOURCE)`，wav 由 `assets/app.rc` 内嵌进 exe，
//!   与图标、版本信息走同一条 rc.exe 通道（NFR-10「内嵌资源不依赖外部文件」第五格）。
//!
//! 下面六条是动手前探针实测出来的（数字与逐行输出在 `target/wic-probe/evidence.txt`），
//! 全部属于「抄错也不报错」那一类，所以逐条钉在代码旁边：
//!
//! 1. 后台线程必须自己 `CoInitializeEx`。没 init 的线程上 `CoCreateInstance` 给 `0x800401F0`
//!    （CO_E_NOTINITIALIZED，我原先猜的是 `0x80040154`），编译／运行／日志全绿，只有图出不来。
//! 2. 内存闸不是优化：3840×2400 原图按 native 解码一次要 **36,864,000** 字节；同一张过
//!    `IWICBitmapScaler` 缩到 800×500 是 **1,600,000** 字节、73~106 ms。所以 [`decode`] 必走缩放。
//! 3. 24bpp 的源（JPEG、24bpp BMP）native 就是 `24bppBGR` ⇒ 必须再过一次 `IWICFormatConverter`
//!    到 `32bppBGRA`，否则 stride 与缓冲对不上；PNG colortype=6 native 已是 `32bppBGRA`。
//! 4. PNG 的 alpha 是**直通**的（写 A=60+x*20，读回逐格 0 处不符 ⇒ 不是预乘）⇒ 上层用
//!    `ColorImage::from_rgba_unmultiplied`；24bpp 源补进来的 alpha 是 255。
//! 5. 失败只认 HRESULT 的**数字**：消息文本按系统语言本地化（实测「系统找不到指定的文件。」），
//!    不能当判据。三个负样本 = `0x88982F50`（不是图像）／`0x80070002`（文件不在）／`0x80070003`（路径不通）。
//! 6. `IWICBitmapScaler` 本身会改像素：8×6→32×24 把 `px(0,0)=[120,20,10]` 打成 `[120,17,8]`。
//!    ⇒ 像素精确的断言只能走「源已经在预算内、不需要缩放」那条路，本模块的缩放分支只断言尺寸与长度。
//!
//! 圆角靠**顶点扇形**而不是把蒙版烤进纹理：蒙版半径的像素值 = 逻辑半径 ×（纹理像素／面板逻辑像素），
//! 那个比例随窗口宽度和 DPI 变，烤进去的尺寸一换就错（要么图溢出圆角，要么角上多缺一块）。
//! 扇形按当帧的面板矩形现算，位置→uv 是仿射的，三角形之间不会接缝错位。

use std::time::Instant;

use egui::{Pos2, Rect};
use windows::core::{Error, IUnknown, PCWSTR};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmapSource, IWICFormatConverter,
    IWICImagingFactory, IWICPalette, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeHighQualityCubic, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnLoad,
};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_NODEFAULT, SND_RESOURCE};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

/// 纹理的像素预算（面积，不是边长）。上限的来源：面板最宽 `WINDOW_WIDTH_MAX = 900` 逻辑像素，
/// 高按内容可以到 ~800，200% DPI 下是 1800×1600 = 2,880,000 px——比这道闸还大，
/// 所以取 1,800,000 px（= 7.2 MB RGBA 缓冲，实测 4K native 解码那份 36,864,000 字节的约 1/5），
/// 宁可让大图在超大面板里略微软一点，也不让一张壁纸把 NFR-03 的常驻预算吃掉一块。
pub const PIXEL_BUDGET: usize = 1_800_000;

/// 每个圆角采样的弧段数。`r = 10` 逻辑像素、3 段 ⇒ 弦高误差 `r*(1-cos(15°)) ≈ 0.34` px。
const ARC_SEGMENTS: usize = 3;

/// 内嵌 wav 的资源 id，必须与 `assets/app.rc` 里 `1 WAVE "popup.wav"` 的那个 1 一致。
/// 对账是下面这条 `wave_resource_id_matches_the_rc`（SKIN-丁 已就位）：那个 1、那一行引号里的
/// 文件名、以及按 .rc 自己的目录解析出来的那颗文件本体（在不在、是不是 RIFF…WAVE），三样一起核。
pub const SOUND_RESOURCE_ID: usize = 1;

/// 解好的一张背景图：RGBA 逐行、左上起，`rgba.len() == w * h * 4`。
#[derive(Debug)]
pub struct Decoded {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// 面积超过预算时该缩到多大：等比缩，面积刚好落回预算内，绝不放大。
///
/// 源尺寸非法（0 任一边）返回 `None`——调用方不猜，直接放弃这张图。
pub fn scale_target(src_w: u32, src_h: u32, budget: usize) -> Option<(u32, u32)> {
    if src_w == 0 || src_h == 0 {
        return None;
    }
    let area = src_w as usize * src_h as usize;
    if area <= budget {
        return Some((src_w, src_h));
    }
    let k = (budget as f64 / area as f64).sqrt();
    let w = ((src_w as f64 * k) as u32).max(1);
    let h = ((src_h as f64 * k) as u32).max(1);
    Some((w, h))
}

/// `src` 这比例的一块图，等比铺满 `box_w × box_h` 的矩形要裁掉的居中那块，返回 uv 矩形。
///
/// 长边贴齐、短边居中裁（cover / 无黑边）。任一尺寸非法就退回整张 uv（不裁），
/// 让调用方照常画——「背景图不见了」比「背景图有点裁切」更让人以为程序坏了。
pub fn cover_uv(src_w: u32, src_h: u32, box_w: f32, box_h: f32) -> Rect {
    let full = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    if src_w == 0 || src_h == 0 || box_w <= 0.0 || box_h <= 0.0 {
        return full;
    }
    let src_ar = src_w as f32 / src_h as f32;
    let box_ar = box_w / box_h;
    if src_ar > box_ar {
        // 源更宽 ⇒ 横向裁
        let span = box_ar / src_ar;
        let lo = (1.0 - span) / 2.0;
        Rect::from_min_max(Pos2::new(lo, 0.0), Pos2::new(lo + span, 1.0))
    } else {
        // 源更高 ⇒ 纵向裁
        let span = src_ar / box_ar;
        let lo = (1.0 - span) / 2.0;
        Rect::from_min_max(Pos2::new(0.0, lo), Pos2::new(1.0, lo + span))
    }
}

/// 一块带圆角的矩形切成「中心 + 一圈周长顶点 + 扇形三角」的结果，位置与 uv 一一对应。
///
/// `radius` 传逻辑像素（上层给 `theme::WINDOW_RADIUS`，与面板框同一个数）；
/// `src_w/src_h` 只用来算 [`cover_uv`]，与面板比例不一致时裁掉多余的那边。
/// 退化输入（宽高为 0、源尺寸 0）返回空扇形——上层据此什么都不画。
#[derive(Debug, Default, Clone)]
pub struct Fan {
    pub pos: Vec<Pos2>,
    pub uv: Vec<Pos2>,
    /// 每三个下标一个三角形，下标进 [`Fan::pos`]。第 0 个顶点恒为中心。
    pub tri: Vec<u32>,
}

/// 扇形的周长顶点数（不含中心）。
fn fan_vertex_count(radius: f32, w: f32, h: f32) -> usize {
    if radius_effective(radius, w, h) < 0.5 {
        4
    } else {
        4 * (ARC_SEGMENTS + 1)
    }
}

/// 半径收进矩形内切的一半；负数当 0。
fn radius_effective(radius: f32, w: f32, h: f32) -> f32 {
    if radius.is_nan() || radius <= 0.0 {
        0.0
    } else {
        radius.min(w * 0.5).min(h * 0.5).max(0.0)
    }
}

pub fn image_fan(rect: Rect, radius: f32, src_w: u32, src_h: u32) -> Fan {
    let (w, h) = (rect.width(), rect.height());
    if w <= 0.0 || h <= 0.0 || src_w == 0 || src_h == 0 {
        return Fan::default();
    }
    let r = radius_effective(radius, w, h);
    let mut ring: Vec<Pos2> = Vec::with_capacity(fan_vertex_count(r, w, h));
    if r < 0.5 {
        // 没有圆角 ⇒ 就是四个角，省掉一圈重复点。
        ring.push(rect.left_top());
        ring.push(rect.right_top());
        ring.push(rect.right_bottom());
        ring.push(rect.left_bottom());
    } else {
        // 屏幕坐标 y 向下 ⇒ 四个角的弧依次是 180→270、270→360、0→90、90→180，
        // 连起来是从「左上弧的左端点」起顺时针一圈。
        let tl = Pos2::new(rect.min.x + r, rect.min.y + r);
        let tr = Pos2::new(rect.max.x - r, rect.min.y + r);
        let br = Pos2::new(rect.max.x - r, rect.max.y - r);
        let bl = Pos2::new(rect.min.x + r, rect.max.y - r);
        arc(tl, r, 180.0, 270.0, &mut ring);
        arc(tr, r, 270.0, 360.0, &mut ring);
        arc(br, r, 0.0, 90.0, &mut ring);
        arc(bl, r, 90.0, 180.0, &mut ring);
    }

    // 位置 → uv 是同一个仿射（裁切窗口只是把 0..1 线性重映射），所以逐顶点算一次就够，
    // 三角形内部由插值补全，跨三角形不会错位。
    let uv = cover_uv(src_w, src_h, w, h);
    let mut fan = Fan::default();
    fan.pos.reserve(ring.len() + 1);
    fan.uv.reserve(ring.len() + 1);
    fan.tri.reserve(ring.len() * 3);
    fan.pos.push(rect.center());
    fan.uv.push(map_uv(rect.center(), rect, w, h, uv));
    for p in ring {
        fan.pos.push(p);
        fan.uv.push(map_uv(p, rect, w, h, uv));
    }
    let n = fan.pos.len() as u32;
    for i in 1..n {
        let next = if i + 1 == n { 1 } else { i + 1 };
        fan.tri.extend_from_slice(&[0, i, next]);
    }
    fan
}

fn arc(center: Pos2, r: f32, from_deg: f32, to_deg: f32, out: &mut Vec<Pos2>) {
    let steps = ARC_SEGMENTS;
    for i in 0..=steps {
        let t = from_deg + (to_deg - from_deg) * i as f32 / steps as f32;
        let rad = t.to_radians();
        out.push(Pos2::new(center.x + r * rad.cos(), center.y + r * rad.sin()));
    }
}

fn map_uv(p: Pos2, rect: Rect, w: f32, h: f32, uv: Rect) -> Pos2 {
    let fu = (p.x - rect.min.x) / w;
    let fv = (p.y - rect.min.y) / h;
    Pos2::new(
        uv.min.x + fu * (uv.max.x - uv.min.x),
        uv.min.y + fv * (uv.max.y - uv.min.y),
    )
}

/// WIC 解出来的缓冲是 BGRA（实测 `px(0,0)` 读到 `[120,20,10]`，已知是 R10 G20 B120 ⇒ B 在前），
/// egui 的 `ColorImage` 要 RGBA。就地换两头，alpha 原样留着（直通，不是预乘）。
///
/// 长度不是 4 的倍数时尾部那几个字节不碰——本模块只会交进整幅缓冲，这条是防御性的说明，不是分支。
pub fn bgra_to_rgba(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

/// 只留文件名，日志里不落整条路径（与 `source::icon::log_label` 同一份隐私口径）。
pub fn log_label(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or("?")
        .to_string()
}

/// 解一张背景图：WIC 解码 → 必过的缩放闸 → 必过的格式转换 → RGBA。
///
/// 任何一步失败都只记一行日志、返回 `None`；不 panic、不重试、也不管调用方在哪个线程
/// （自己 `CoInitializeEx`，谁先 init 谁负责收，见 [`crate::source::icon`] 里同一个写法）。
/// 只取第 0 帧：实测 `assets/app.ico` 有 5 帧（48/32/24/16/256，且不保证最大帧在第一位），
/// GIF 动画也只会拿到第一帧——背景图要的是「一张静止图」，不是播放器。
pub fn decode(path: &str) -> Option<Decoded> {
    let t0 = Instant::now();
    match unsafe { decode_inner(path) } {
        Ok((w, h, rgba)) => {
            log_debug!(
                "skin: 背景图 {} 解出 {w}x{h}（{} 字节，预算 {PIXEL_BUDGET} 像素）用时 {} ms",
                log_label(path),
                rgba.len(),
                t0.elapsed().as_millis()
            );
            Some(Decoded { w, h, rgba })
        }
        Err(code) => {
            log_info!(
                "skin: 背景图 {} 解码失败 hr=0x{code:08X}（认不出格式／文件不在／尺寸为 0 都在这一档）",
                log_label(path)
            );
            None
        }
    }
}

unsafe fn decode_inner(path: &str) -> Result<(u32, u32, Vec<u8>), i32> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let ours = hr.is_ok();
        let result = (|| -> Result<(u32, u32, Vec<u8>), i32> {
            let factory: IWICImagingFactory = CoCreateInstance(
                &CLSID_WICImagingFactory,
                // windows 0.62 的 `Param` 只给 `Option<&T>` 实现了，`None::<IUnknown>` 过不了
                // （编译期就报错，不是静默失败）。
                None::<&IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
            .map_err(code)?;
            let wide = wide_null(path);
            let decoder = factory
                .CreateDecoderFromFilename(
                    PCWSTR(wide.as_ptr()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnLoad,
                )
                .map_err(code)?;
            let frame = decoder.GetFrame(0).map_err(code)?;
            let (mut sw, mut sh) = (0u32, 0u32);
            frame
                .GetSize(&mut sw as *mut u32, &mut sh as *mut u32)
                .map_err(code)?;
            // 尺寸为 0 的合法容器（少数畸形 tif）自己造一个 E_INVALIDARG：日志里与真失败同格式。
            if sw == 0 || sh == 0 {
                return Err(0x8007_0057u32 as i32);
            }
            let (tw, th) = scale_target(sw, sh, PIXEL_BUDGET).unwrap_or((sw, sh));
            let conv: IWICFormatConverter = factory.CreateFormatConverter().map_err(code)?;
            if (tw, th) != (sw, sh) {
                let scaler = factory.CreateBitmapScaler().map_err(code)?;
                scaler
                    .Initialize(&frame, tw, th, WICBitmapInterpolationModeHighQualityCubic)
                    .map_err(code)?;
                convert(&conv, &scaler)?;
            } else {
                convert(&conv, &frame)?;
            }
            let (mut w, mut h) = (0u32, 0u32);
            conv.GetSize(&mut w as *mut u32, &mut h as *mut u32)
                .map_err(code)?;
            let stride = w * 4;
            let mut buf = vec![0u8; stride as usize * h as usize];
            conv.CopyPixels(std::ptr::null(), stride, &mut buf)
                .map_err(code)?;
            bgra_to_rgba(&mut buf);
            Ok((w, h, buf))
        })();
        if ours {
            CoUninitialize();
        }
        result
    }
}

/// 转换器那一趟。24bpp 的源不过这一关就没有 alpha 通道，stride 也对不上（探针实测第 3 条）。
///
/// 泛型只收 `Param<IWICBitmapSource>`：`IWICBitmapFrameDecode`（不缩放那条）与
/// `IWICBitmapScaler`（缩放那条）都靠 `interface_hierarchy!` 直接进去，不需要 `.cast()`。
fn convert<P0: windows::core::Param<IWICBitmapSource>>(conv: &IWICFormatConverter, src: P0) -> Result<(), i32> {
    unsafe {
        conv.Initialize(
            src,
            &GUID_WICPixelFormat32bppBGRA,
            WICBitmapDitherTypeNone,
            None::<&IWICPalette>,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .map_err(code)
    }
}

/// HRESULT 抽成数字——文本按系统语言本地化，不能进判据（探针实测第 5 条）。
fn code(e: Error) -> i32 {
    e.code().0
}

fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 播一次呼出提示音。wav 在 exe 的资源段里（`assets/app.rc` 的 `1 WAVE`），不读外部文件。
///
/// 三条实测钉住的写法，改错都不报错（后果分别是没声音／多一声／判据失真）：
/// - 名字必须是**整数编码**的 `PCWSTR(1 as *const u16)`。探针里换成字符串名 `"1!WAVE"` 返回 0。
///   类型那一头恰好相反：rc.exe 往 `.res` 里写的 WAVE 类型是**字符串 "WAVE"**，不是数字 id
///   （同一份 .res 里 ICON 那五条写的是数字 3）。探针实测这种条目 `FindResourceW(id=1, WAVE)`
///   返回 OK、`SizeofResource=13274`，⇒ 「名字要整数、类型是字符串」两头都只能照抄，没有自由度。
///   （工装：`target/skin-rc-probe/res-entries.ps1` 解的就是这个 .res 条目表。）
/// - 必须带 `SND_NODEFAULT`。它的文档语义是「找不到就别放系统默认音」——没有 rc.exe 的构建机上
///   资源段是空的，那条退路要的是安静，不是每呼一次「叮」一下。
/// - 这里的返回值在 RESOURCE 这条路上**有一点**意义：探针实测坏 id 返回 0（异步也照样 0，不像
///   FILE+ASYNC 坏路径仍返回 1）。但 0 到底指「什么都没放」还是「回落到系统默认音」，它分不开——
///   带不带 `SND_NODEFAULT` 都是 0，我原先「去掉 NODEFAULT 就该静默返回 1」的猜想被实测否掉了。
///   ⇒ TRUE 只证明资源被取走了，不证明人耳听见；那一格只能真人听。
pub fn play_popup_sound() -> bool {
    unsafe {
        // 自己这颗 exe 的模块句柄。`None::<PCWSTR>` 在 windows 0.62 里过不了 `Param`（探针踩过），
        // 要取当前进程得传显式的空指针 `PCWSTR::null()`。
        let Ok(hmod) = GetModuleHandleW(PCWSTR::null()) else {
            log_info!("skin: 提示音没放，取模块句柄失败（内嵌资源找不到自己的镜像）");
            return false;
        };
        let flags = SND_RESOURCE | SND_ASYNC | SND_NODEFAULT;
        let played = PlaySoundW(PCWSTR(SOUND_RESOURCE_ID as *const u16), Some(hmod), flags);
        if played.as_bool() {
            // 成功也留一行：这一档只有详细日志里有，写的就是上面那条「TRUE 不等于听见」——
            // 日志里出现它，只说明资源段里那颗 WAVE 被取走了。
            log_debug!("skin: 提示音已投递（PlaySoundW=TRUE，只证明 id={SOUND_RESOURCE_ID} 的 WAVE 取走了，不证明人耳听见）");
        } else {
            log_info!(
                "skin: 提示音没放，资源段里没有 id={SOUND_RESOURCE_ID} 的 WAVE（构建机上 rc.exe 没跑成就是这个）"
            );
        }
        played.as_bool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_max(Pos2::new(x, y), Pos2::new(x + w, y + h))
    }

    #[test]
    fn scale_target_never_upscales_and_always_lands_inside_the_budget() {
        // 预算内原样不动 —— 这条同时是「像素精确断言走哪条路」的开关。
        assert_eq!(scale_target(8, 6, PIXEL_BUDGET), Some((8, 6)));
        assert_eq!(scale_target(1800, 1000, 1_800_000), Some((1800, 1000)));
        // 0 任一边：不猜，直接 None。
        assert_eq!(scale_target(0, 100, PIXEL_BUDGET), None);
        assert_eq!(scale_target(100, 0, PIXEL_BUDGET), None);
        // 实测那张 3840×2400（native 解码 36,864,000 字节）缩完必须落回预算，且不许放大任何一边。
        let (w, h) = scale_target(3840, 2400, PIXEL_BUDGET).unwrap();
        let area = w as usize * h as usize;
        assert!(area <= PIXEL_BUDGET, "area {area} 超预算 {PIXEL_BUDGET}");
        assert!(w < 3840 && h < 2400);
        // 等比：两条边的比例差在一个像素以内。
        let (a, b) = (w as f64 / 3840.0, h as f64 / 2400.0);
        assert!((a - b).abs() < 2.0 / 2400.0, "比例跑了 {a} vs {b}");
        // 缓冲字节数就是这条闸的物理意义：预算 1,800,000 像素 ⇒ 上限 7,200,000 字节，
        // 而探针实测同一张 3840×2400 不过闸的 native 解码是 36,864,000 字节。
        const MEASURED_NATIVE_BYTES: usize = 36_864_000;
        assert_eq!(w * h * 4, area as u32 * 4);
        assert!(w * h * 4 <= 4 * PIXEL_BUDGET as u32);
        // 下限也要钉——这条测的是「刚好落回预算」里的那个「刚好」。把 `.sqrt()` 抄丢一次，
        // 3840×2400 缩成 750×468：面积不超预算、两边都没放大、比例仍等比、字节数仍对、
        // 「比 native 小 5 倍」仍成立 ⇒ 上面每一条照旧全绿，而真机上背景图凭空糊掉一倍多的边。
        // 实测证伪跑过：删掉 `.sqrt()` ⇒ 本条红（351,000 够不着 900,000），加回来 ⇒ 绿。
        assert!(
            area * 2 >= PIXEL_BUDGET,
            "缩过头了：面积 {area}，连预算 {PIXEL_BUDGET} 的一半都够不着"
        );
        assert!(
            MEASURED_NATIVE_BYTES / (w * h * 4) as usize >= 5,
            "闸买到不到 5 倍，等于没按探针的数字办事"
        );
        // 极端的长条也不能漏：面积闸是唯一的闸。**必须挑一条真超预算的**——
        // 20000×2 只有 40,000 像素，远在 1,800,000 之内，压根不进缩放分支，
        // 我第一版就拿它断言 `h == 1`，红在 `left: 2`（那条 `.max(1)` 从没被执行过）。
        // 2,000,000×2 = 4,000,000 像素 ⇒ k=√0.45 ⇒ 短边 1.34 截成 1，才真走 clamp。
        let (w2, h2) = scale_target(2_000_000, 2, PIXEL_BUDGET).unwrap();
        assert_eq!(h2, 1, "短边截到 0 会被 .max(1) 顶回 1");
        assert!(w2 as usize * h2 as usize <= PIXEL_BUDGET);
    }

    #[test]
    fn cover_uv_crops_the_axis_that_has_too_much() {
        // 源比盒子宽 ⇒ 只裁横向，纵向留满。
        let uv = cover_uv(2000, 1000, 100.0, 100.0);
        assert_eq!(uv.min.y, 0.0);
        assert_eq!(uv.max.y, 1.0);
        let span = uv.width();
        assert!((span - 0.5).abs() < 1e-5, "该裁掉一半，实际 {span}");
        assert!((uv.min.x - (1.0 - span) / 2.0).abs() < 1e-5, "没居中");
        // 源比盒子高 ⇒ 只裁纵向。
        let uv2 = cover_uv(1000, 2000, 100.0, 100.0);
        assert_eq!(uv2.min.x, 0.0);
        assert_eq!(uv2.max.x, 1.0);
        assert!((uv2.height() - 0.5).abs() < 1e-5);
        // 比例一致 ⇒ 整张，一点不裁。
        let uv3 = cover_uv(1600, 900, 320.0, 180.0);
        // emath 0.36.2 没有 `Pos2::splat`，写上去是编译错误（第一次 cargo check 抓到）。
        assert_eq!(uv3, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)));
        // 退化输入退回整张 —— 不许变成「什么都不画」。
        assert_eq!(cover_uv(0, 10, 100.0, 100.0), uv3);
        assert_eq!(cover_uv(10, 10, 0.0, 100.0), uv3);
    }

    #[test]
    fn bgra_to_rgba_swaps_only_the_two_color_ends() {
        let mut buf = vec![120u8, 20, 10, 60, 1, 2, 3, 255, 9, 9, 9];
        bgra_to_rgba(&mut buf);
        assert_eq!(&buf[0..4], &[10, 20, 120, 60]);
        assert_eq!(&buf[4..8], &[3, 2, 1, 255]);
        // 尾部不足 4 字节不碰（探针实测的 alpha 直通 ⇒ 60 原样留着，没跟着换）。
        assert_eq!(&buf[8..], &[9, 9, 9]);
    }

    #[test]
    fn fan_without_radius_is_a_plain_rect_fan() {
        // 源比例 == 盒子比例 ⇒ cover_uv 不裁 ⇒ uv 就是位置归一化。
        // 第一版这里传的是 400×400（正方形塞进 2:1 的框）却断言「uv 不裁」，
        // 红在 `left: [0.0 0.2]`——而那个 "0.2" 其实是 **0.25**：emath 0.36.2 的
        // `Pos2::Debug` 写死 `{:.1}`（emath-0.36.2/src/pos2.rs:355），打印本身掉了精度。
        // ⇒ 几何断言的失败消息不能再拿方括括号里的数当判据，逐字段加容差自己格式化。
        let r = rect(5.0, 7.0, 200.0, 100.0);
        let f = image_fan(r, 0.0, 800, 400);
        assert_eq!(f.pos.len(), 5, "中心 + 四角");
        assert_eq!(f.tri.len() / 3, 4);
        assert_eq!(f.pos[0], r.center());
        // 顺时针一圈：左上、右上、右下、左下。
        let want = [
            Pos2::new(5.0, 7.0),
            Pos2::new(205.0, 7.0),
            Pos2::new(205.0, 107.0),
            Pos2::new(5.0, 107.0),
        ];
        assert_eq!(&f.pos[1..], &want[..]);
        assert_eq!(f.uv[1], Pos2::new(0.0, 0.0));
        assert_eq!(f.uv[2], Pos2::new(1.0, 0.0));
        assert_eq!(f.uv[3], Pos2::new(1.0, 1.0));
        assert_eq!(f.uv[4], Pos2::new(0.0, 1.0));
        // 中心那条也得对上（扇形靠它，位置→uv 是同一个仿射）。
        assert_eq!(f.uv[0], Pos2::new(0.5, 0.5));
        // 换成比例不匹配的源 ⇒ 只裁一边，另一边仍然铺满。
        // 方源塞 2:1 的框：横向铺满 (0..1)，纵向裁中间那块 (0.25..0.75)。
        let c = image_fan(r, 0.0, 400, 400);
        assert_eq!(c.uv[1].x, 0.0, "横向不该裁");
        assert_eq!(c.uv[3].x, 1.0, "横向不该裁");
        assert!(
            (c.uv[1].y - 0.25).abs() < 1e-6,
            "纵向该从 0.25 起，实际 {:.6}",
            c.uv[1].y
        );
        assert!(
            (c.uv[3].y - 0.75).abs() < 1e-6,
            "纵向该到 0.75 止，实际 {:.6}",
            c.uv[3].y
        );
    }

    #[test]
    fn fan_with_radius_bites_each_corner_and_stays_inside() {
        let r = rect(0.0, 0.0, 100.0, 60.0);
        let radius = 10.0f32;
        let f = image_fan(r, radius, 800, 800);
        assert_eq!(f.pos.len(), 4 * (ARC_SEGMENTS + 1) + 1);
        assert_eq!(f.tri.len() / 3, f.pos.len() - 1);
        assert_eq!(f.tri.len() % 3, 0);
        assert_eq!(f.uv.len(), f.pos.len());
        // 每个三角形的第一个下标恒是中心 ⇒ 是个扇形，不是随便连的网。
        assert!(f.tri.chunks_exact(3).all(|t| t[0] == 0));
        // 所有顶点在矩形内；而矩形左上角那个点**不在**顶点里（被弧咬掉了）。
        for p in &f.pos {
            assert!(r.contains(*p), "{p:?} 越界");
        }
        let corner = Pos2::new(0.0, 0.0);
        assert!(!f.pos.iter().any(|p| (*p - corner).length() < 1e-3), "圆角没生效");
        // 左上弧的第一个采样点是 (0, r)；一圈走完，末点是左下弧的终点 (0, h - r)。
        // 两个都贴左边缘、都不是那个直角 ⇒ 圆角真的把角咬掉了。
        let first = f.pos[1];
        assert!(
            (first.x - 0.0).abs() < 1e-4 && (first.y - radius).abs() < 1e-4,
            "左上弧起点应为 (0, {radius})，实际 ({:.4}, {:.4})",
            first.x,
            first.y
        );
        let last = *f.pos.last().unwrap();
        assert!(
            (last.x - 0.0).abs() < 1e-4 && (last.y - (60.0 - radius)).abs() < 1e-4,
            "左下弧终点应为 (0, {}), 实际 ({:.4}, {:.4})",
            60.0 - radius,
            last.x,
            last.y
        );
        // uv 也得全在 0..1 里，否则采样到纹理外面就是一圈怪边。
        for u in &f.uv {
            assert!((0.0..=1.0).contains(&u.x) && (0.0..=1.0).contains(&u.y), "{u:?}");
        }
        // 上面那些抓不住「某段弧的终止角写错」：把左上弧从 180→270 改成 180→360，
        // 点数、三角数、首点、末点、越界、uv 全都照旧对——只有那圈点里有 4 个跑进了
        // 矩形内部。⇒ 真正的判据是「周长顶点必须在圆角矩形的**边界**上」：
        // 把点夹进内缩 r 的方框，到边界的距离（SDF）恒等于 r。
        let inset = Rect::from_min_max(
            Pos2::new(r.min.x + radius, r.min.y + radius),
            Pos2::new(r.max.x - radius, r.max.y - radius),
        );
        for (i, p) in f.pos[1..].iter().enumerate() {
            let c = Pos2::new(p.x.clamp(inset.min.x, inset.max.x), p.y.clamp(inset.min.y, inset.max.y));
            let d = (*p - c).length();
            assert!(
                (d - radius).abs() < 1e-3,
                "顶点 #{i} {p} 到边界 {d}，应为 {radius}"
            );
        }
        // 四颗角都要被咬：每颗角的圆心附近都要有 ARC_SEGMENTS+1 个等距点。
        for cen in [
            Pos2::new(r.min.x + radius, r.min.y + radius),
            Pos2::new(r.max.x - radius, r.min.y + radius),
            Pos2::new(r.max.x - radius, r.max.y - radius),
            Pos2::new(r.min.x + radius, r.max.y - radius),
        ] {
            let n = f.pos[1..]
                .iter()
                .copied()
                .filter(|p| (*p - cen).length() >= radius - 1e-3
                    && (*p - cen).length() <= radius + 1e-3)
                .count();
            assert!(
                n >= ARC_SEGMENTS + 1,
                "圆心 {cen} 周围只有 {n} 个弧点，该有 {}",
                ARC_SEGMENTS + 1
            );
        }
    }

    #[test]
    fn fan_radius_is_clamped_and_degenerate_input_yields_nothing() {
        // 半径比短边的一半还大 ⇒ 收成半圆（不越界、不出现负弧）。
        let f = image_fan(rect(0.0, 0.0, 40.0, 20.0), 500.0, 100, 100);
        for p in &f.pos {
            assert!(p.x >= 0.0 && p.x <= 40.0 && p.y >= 0.0 && p.y <= 20.0, "{p:?}");
        }
        assert_eq!(f.tri.len() / 3, f.pos.len() - 1);
        // 退化输入：空扇形 = 上层什么都不画。
        assert!(image_fan(rect(0.0, 0.0, 0.0, 10.0), 4.0, 100, 100).pos.is_empty());
        assert!(image_fan(rect(0.0, 0.0, 10.0, 10.0), 4.0, 0, 100).pos.is_empty());
        assert_eq!(image_fan(rect(0.0, 0.0, 10.0, 10.0), f32::NAN, 100, 100).pos.len(), 5);
    }

    #[test]
    fn log_label_keeps_only_the_file_name() {
        assert_eq!(log_label(r"C:\Users\baoqi\Pictures\a b.jpg"), "a b.jpg");
        assert_eq!(log_label("x/y/z.png"), "z.png");
        assert_eq!(log_label(""), "");
    }

    // ---- 真解码的哨兵：走的是应用将来同一份代码，不是为测试单开的路 ----

    #[test]
    fn decode_reads_back_the_first_frame_of_the_embedded_icon() {
        // `assets/app.ico` 的帧表实测是 48/32/24/16/256（第 0 帧 48×48，而且**不是**最大帧）。
        // 48*48 远在预算内 ⇒ 走的是不缩放那条分支，所以尺寸与像素的断言都是精确的（探针第 6 条：
        // 一过 scaler 就不许再断言像素）。
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/app.ico");
        let d = decode(path).expect("仓库里的 assets/app.ico 必须解得开，解不开就是调用面坏了");
        assert_eq!((d.w, d.h), (48, 48));
        assert_eq!(d.rgba.len(), 48 * 48 * 4);
        // 探针在同一帧上读到 px(0,0) BGRA=[0,0,0,0]、px(mid) BGRA=[253,249,248,255]
        // （mid = (h/2)*stride + (w/2)*4，与本模块的取法一致）。
        // 换成 RGBA 后中点必须是 [248,249,253,255] —— 两头换了、alpha 没跟着换。
        // 左上角那格全 0，换序后仍是 0，它证不了字节序，所以只留着当「不许 panic」的对照。
        assert_eq!(&d.rgba[0..4], &[0, 0, 0, 0]);
        let mid = (d.h / 2) as usize * (d.w * 4) as usize + (d.w / 2) as usize * 4;
        // 这颗点的牙验过：把上面那句 `bgra_to_rgba` 临时注掉，这条断言当场红在
        // `left: [253, 249, 248, 255]`——正是探针读到的原始 BGRA，换序真的发生了。
        assert_eq!(&d.rgba[mid..mid + 4], &[248, 249, 253, 255]);
    }

    #[test]
    fn decode_fails_with_an_hresult_number_and_never_panics() {
        // 三个负样本的码都是探针实测过的：文本是中文还是英文都不作判据。
        let missing = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/__nope__.png");
        assert_eq!(unsafe { decode_inner(missing) }.unwrap_err(), 0x8007_0002u32 as i32);
        let not_image = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        assert_eq!(unsafe { decode_inner(not_image) }.unwrap_err(), 0x8898_2f50u32 as i32);
        // 空串与整条解码路：只许返回 None。
        assert!(decode("").is_none());
        assert!(decode("C:\\").is_none());
    }

    // ---- 资源脚本对表：id、那一行点的文件、文件本体，三样一起核 ----

    /// `SOUND_RESOURCE_ID` 与 `assets/app.rc` 里那条 `1 WAVE` 对得上，且那条引号里的文件真在
    /// .rc 自己的目录下、真的是一颗 RIFF…WAVE。
    ///
    /// 为什么这条链值得钉：每一环抄错的后果都是「一声不响」，而构建照样绿。
    /// - id 错了 ⇒ `PlaySoundW` 返回 0。实测过坏 id 带不带 `SND_NODEFAULT` 都是 0，运行时除了
    ///   一行 info 日志没有任何信号（`play_popup_sound` 的注释）。
    /// - 引号里的文件名错了 ⇒ rc.exe 报 `error RC2135 : file not found` + 退出码 1、不产出 `.res`，
    ///   `build.rs` 把它记成「资源脚本跳过」——整段资源一起没（图标、版本信息、提示音三批的账）。
    /// - 文件在但内容不是 wav ⇒ rc.exe 照收。实测：把 `Cargo.toml`（6,307 字节）改名成 `fake.wav`
    ///   交给同一个 rc.exe ⇒ 退出码 0、`.res` 里那条 WAVE 的 data 就是 6,307。所以文件本体这一段
    ///   只有下面那三条头断言拦得住，而它的后果又是一声不响。
    ///
    /// 筛法照批次 VER 的教训：本文件里第一个 token 是 `1` 的行有三条（ICON / WAVE / VERSIONINFO），
    /// 所以判据是「第二个 token == WAVE」，不是「第一个 token == 1」。
    #[test]
    fn wave_resource_id_matches_the_rc() {
        let rc = include_str!("../assets/app.rc");
        let wave_lines: Vec<&str> = rc
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//") && line.split_whitespace().nth(1) == Some("WAVE"))
            .collect();
        assert_eq!(wave_lines.len(), 1, "资源脚本里有效的 `1 WAVE` 行应该只有一条，实数 {}", wave_lines.len());
        let line = wave_lines[0];
        let id: usize = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .parse()
            .unwrap_or_else(|_| panic!("WAVE 行开头的 id 不是数字：{line}"));
        assert_eq!(id, SOUND_RESOURCE_ID, "资源脚本里的 WAVE id 与 skin::SOUND_RESOURCE_ID 不一致");

        // 引号里那个名字是**相对 .rc 自己所在目录**解析的（实测见 assets/app.rc 顶部注释第 ① 条：
        // .rc 目录与 cwd 各有一份候选文件时，落进 .res 的是 .rc 目录那份）。基准就按同一个来：
        // `.rc` 在 `assets\` 下，所以从包根的 `assets` 起找，而不是从测试进程的 cwd 起找——后者
        // 这次也是包根、结果一样，两条路分不出谁选错了基准。
        let file = line.split('"').nth(1).unwrap_or_default();
        assert!(!file.is_empty(), "WAVE 行引号里没有文件名：{line}");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(file);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("资源脚本点名的 {} 读不到：{e}", path.display()));
        // 44 = RIFF 头 12 + `fmt ` 块 8+16 + `data` 头 8，比这短的不可能是能放的 wav。
        assert!(bytes.len() > 44, "{} 只有 {} 字节，塞不满一个最小 wav 头", path.display(), bytes.len());
        assert_eq!(&bytes[0..4], b"RIFF", "{} 开头不是 RIFF", path.display());
        assert_eq!(&bytes[8..12], b"WAVE", "{} 第 8..12 字节不是 WAVE", path.display());
    }
}

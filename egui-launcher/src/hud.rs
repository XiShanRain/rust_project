//! FR-UIX-09：独立提示层（HUD）。
//!
//! 落地形态是 eframe 的延迟子视口——一个独立顶层窗口，点击穿透、不激活、不占任务栏。
//! E-08 实测（2026-10-24）四条判据全过：穿透（HUD 覆盖处点击落到下方窗口）、不抢焦点
//! （创建/二次显示/重新叫回时前台都不变）、主窗 `Visible(false)` 后 HUD 仍以 60fps 自绘、
//! 两个窗口都隐藏后仍能把 HUD 叫回来（重建 34ms）。
//!
//! 实测还发现两处 winit 不替我们做的事，必须首帧自己补：`with_taskbar(false)` 只删任务栏
//! 按钮、**不设** `WS_EX_TOOLWINDOW`，且顶层窗口默认带 `WS_EX_APPWINDOW` ⇒ 不清掉的话 HUD
//! 会出现在 Alt+Tab 里，被选中一次就抢走焦点。
//!
//! 动画与计时全部在 HUD 自己的回调里推进：主窗隐藏后 eframe 不再调 `ui()`，靠主窗推进的
//! 状态机会停住。回调每帧 `request_repaint()`，实测能单独把视口维持在 60fps。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GWL_EXSTYLE, GetCursorPos, GetWindowLongPtrW, SetWindowLongPtrW,
    WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::PCWSTR;

use crate::theme::Theme;

/// 前台事件（如「已复制」）的默认停留时间。
pub const HOLD_FOREGROUND: Duration = Duration::from_millis(1500);

const FADE_IN_SECS: f32 = 0.100;
const FADE_OUT_SECS: f32 = 0.150;
/// FR-UIX-09：最大宽 520，超出自动换行。
const MAX_W: f32 = 520.0;
const PAD: f32 = 14.0;
const OPACITY: u8 = 240;
/// HUD 与主窗之间的垂直缝。
const GAP: f32 = 8.0;
/// 与工作区边缘留的余量（避免贴边被系统裁掉一像素）。
const SCREEN_MARGIN: f32 = 8.0;
const MIN_H: f32 = 44.0;
/// 主窗隐藏后，3 秒内的提示仍放在主窗原来那块屏上（FR-UIX-09）。
const REUSE_SCREEN: Duration = Duration::from_secs(3);

pub struct Hud {
    shared: Arc<Mutex<Shared>>,
    /// 子窗口标题。无边框窗口里不可见，只用来在首帧找回自己的 HWND 补扩展样式；
    /// 带上进程号避免和别的实例撞名。
    title: String,
    id: egui::ViewportId,
}

struct Shared {
    showing: Option<Showing>,
    /// 上一次提示所在显示器的工作区（物理像素）+ 时刻。
    last_work: Option<(egui::Rect, Instant)>,
}

struct Showing {
    text: String,
    /// 子窗口标题，首帧据此找回 HWND。
    title: String,
    /// 已经带好 240 不透明度的主题色。
    bg: egui::Color32,
    fg: egui::Color32,
    border: egui::Color32,
    font: f32,
    started: Instant,
    hold: Duration,
    pos: egui::Pos2,
    size: egui::Vec2,
    /// 首帧给窗口补过扩展样式没有。
    styled: bool,
}

impl Hud {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                showing: None,
                last_work: None,
            })),
            title: format!("ALTRun HUD {}", std::process::id()),
            id: egui::ViewportId::from_hash_of("alt-run-hud"),
        }
    }

    /// 弹出一条提示。颜色取主题键，尺寸按文本排版结果算（宽 ≤ 520），位置优先贴主窗下方。
    /// 必须在主窗 `ui()` 里调用——要读主窗的 `outer_rect`，也要用主窗的字体排版。
    ///
    /// `fallback` 是「主窗没开时该落到哪块屏」的工作区（物理像素），由调用方按 FR-WIN-09 的
    /// `show_on` 算好传进来——HUD 自己不认识那个设置。链路见 [`pick_work`]。
    pub fn notify(
        &mut self,
        ctx: &egui::Context,
        text: String,
        theme: &Theme,
        hold: Duration,
        fallback: Option<egui::Rect>,
    ) {
        let ppp = ctx.pixels_per_point();
        let host = ctx.input(|i| i.viewport().outer_rect);

        let host_pt = host.map(|h| {
            (
                (h.center().x * ppp).round() as i32,
                (h.center().y * ppp).round() as i32,
            )
        });
        let last = {
            let s = self.shared.lock().unwrap();
            s.last_work
                .filter(|(_, at)| at.elapsed() < REUSE_SCREEN)
                .map(|(r, _)| r)
        };
        let work_phys = pick_work(
            host_pt.and_then(monitor_work_at),
            last,
            fallback,
            win_cursor().and_then(monitor_work_at),
        )
        .unwrap_or_else(|| {
            let size = ctx
                .input(|i| i.viewport().monitor_size)
                .unwrap_or(egui::vec2(1280.0, 720.0));
            egui::Rect::from_min_size(egui::Pos2::ZERO, size * ppp)
        });
        let work = egui::Rect::from_min_max(
            egui::pos2(work_phys.min.x / ppp, work_phys.min.y / ppp),
            egui::pos2(work_phys.max.x / ppp, work_phys.max.y / ppp),
        );

        let font = theme.input_font_size.clamp(14.0, 26.0);
        let wrap = (MAX_W.min(work.width() - 2.0 * SCREEN_MARGIN) - 2.0 * PAD - 2.0).max(1.0);
        let galley = ctx.fonts_mut(|f| {
            f.layout(
                text.clone(),
                egui::FontId::proportional(font),
                theme.input_text,
                wrap,
            )
        });
        let size = egui::vec2(
            hud_width(galley.size().x, work.width()),
            (galley.size().y + 2.0 * PAD + 2.0).max(MIN_H),
        );
        let pos = place(host, work, size);

        let mut s = self.shared.lock().unwrap();
        s.last_work = Some((work_phys, Instant::now()));
        // 与其余动作日志同口径：只记长度与停留档位，提示原文不落盘。
        eprintln!(
            "HUD：显示（{} 个字符，停留 {} ms）",
            text.chars().count(),
            hold.as_millis()
        );
        s.showing = Some(Showing {
            text,
            title: self.title.clone(),
            bg: with_alpha(theme.background, OPACITY as f32 / 255.0),
            fg: theme.input_text,
            border: theme.border,
            font,
            started: Instant::now(),
            hold,
            pos,
            size,
            styled: false,
        });
        drop(s);

        self.render(ctx);
    }

    /// 主窗每帧调用；提示显示期间负责把子视口声明出来。主窗隐藏期间这个函数不跑，
    /// 但已声明的视口不会被回收（eframe 的清理只在正常渲染 pass 里做），HUD 自己继续画。
    pub fn render(&mut self, ctx: &egui::Context) {
        let geometry = {
            let s = self.shared.lock().unwrap();
            s.showing.as_ref().map(|sh| (sh.pos, sh.size))
        };
        let Some((pos, size)) = geometry else { return };
        let shared = self.shared.clone();
        let title = self.title.clone();
        ctx.show_viewport_deferred(self.id, builder(&title, pos, size), move |ui, _| {
            draw(ui, &shared);
        });
    }
}

fn builder(title: &str, pos: egui::Pos2, size: egui::Vec2) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(size)
        .with_position(pos)
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_taskbar(false)
        .with_active(false)
        .with_mouse_passthrough(true)
        .with_resizable(false)
}

fn draw(ui: &mut egui::Ui, shared: &Arc<Mutex<Shared>>) {
    let now = Instant::now();
    let (text, font, bg, fg, border) = {
        let mut s = shared.lock().unwrap();
        let Some(sh) = s.showing.as_mut() else {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            return;
        };
        if !sh.styled {
            sh.styled = patch_window_style(&sh.title, true);
        }

        let held = sh.hold.as_secs_f32();
        let elapsed = now.saturating_duration_since(sh.started).as_secs_f32();
        if elapsed >= held + FADE_OUT_SECS {
            s.showing = None;
            drop(s);
            eprintln!("HUD：收起（显示 {:.0} ms）", elapsed * 1000.0);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            return;
        }
        let alpha = if elapsed < FADE_IN_SECS {
            let t = elapsed / FADE_IN_SECS;
            1.0 - (1.0 - t).powi(3)
        } else if elapsed > held {
            (1.0 - (elapsed - held) / FADE_OUT_SECS).max(0.0)
        } else {
            1.0
        };
        (
            sh.text.clone(),
            sh.font,
            with_alpha(sh.bg, alpha),
            with_alpha(sh.fg, alpha),
            with_alpha(sh.border, alpha),
        )
    };

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(bg)
                .stroke(egui::Stroke::new(1.0, border))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::same(PAD as i8)),
        )
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(font).color(fg));
        });
    ui.ctx().request_repaint();
}

/// 首帧把窗口从「普通置顶窗」改成「工具窗」：不进 Alt+Tab / 任务栏。
/// `no_activate = true` 是 HUD 的口径（也不激活、不抢焦点）；大字显示传 `false`——
/// 它必须能拿到键盘焦点，`Esc` 才收得到。
pub(crate) fn patch_window_style(title: &str, no_activate: bool) -> bool {
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let Ok(hwnd) = (unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) }) else {
        return false;
    };
    let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let mut want = (ex | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0;
    if no_activate {
        want |= WS_EX_NOACTIVATE.0;
    } else {
        want &= !WS_EX_NOACTIVATE.0;
    }
    if want != ex {
        unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want as isize) };
    }
    true
}

pub(crate) fn with_alpha(color: egui::Color32, factor: f32) -> egui::Color32 {
    let a = (color.a() as f32 * factor).round().clamp(0.0, 255.0) as u8;
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

/// 当前鼠标位置（物理像素）。
pub(crate) fn win_cursor() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p) }.ok().map(|_| (p.x, p.y))
}

/// 某个屏幕坐标所在显示器的工作区（物理像素，已排除任务栏）。
pub(crate) fn monitor_work_at(pt: (i32, i32)) -> Option<egui::Rect> {
    let hmon = unsafe {
        MonitorFromPoint(
            POINT { x: pt.0, y: pt.1 },
            MONITOR_DEFAULTTONEAREST,
        )
    };
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(hmon, &mut mi) }.as_bool() {
        return None;
    }
    let r = mi.rcWork;
    Some(egui::Rect::from_min_max(
        egui::pos2(r.left as f32, r.top as f32),
        egui::pos2(r.right as f32, r.bottom as f32),
    ))
}

/// 无主窗时按哪块屏摆：主窗所在屏 → 3 秒内上一次用过的屏（「隐藏后沿用同一屏」）
/// → `show_on` 指定的屏（FR-WIN-09）→ 鼠标所在屏。四个候选取第一个有值的。
fn pick_work(
    host: Option<egui::Rect>,
    last: Option<egui::Rect>,
    fallback: Option<egui::Rect>,
    cursor: Option<egui::Rect>,
) -> Option<egui::Rect> {
    host.or(last).or(fallback).or(cursor)
}

/// 宽度取文本自然宽，最多 520，且不超出工作区。
fn hud_width(natural: f32, work_w: f32) -> f32 {
    (natural + 2.0 * PAD + 2.0)
        .min(MAX_W)
        .min((work_w - 2.0 * SCREEN_MARGIN).max(1.0))
}

/// 有主窗时优先贴下方居中、放不下转上方；主窗没开时放在工作区 2/3 高度。
fn place(host: Option<egui::Rect>, work: egui::Rect, size: egui::Vec2) -> egui::Pos2 {
    let max_x = (work.right() - size.x).max(work.left());
    let anchor_x = host.map_or(work.center().x, |h| h.center().x);
    let x = (anchor_x - size.x / 2.0).clamp(work.left(), max_x);

    let y = if let Some(h) = host {
        let below = h.max.y + GAP;
        let above = h.min.y - size.y - GAP;
        if below + size.y <= work.bottom() - SCREEN_MARGIN {
            below
        } else if above >= work.top() + SCREEN_MARGIN {
            above
        } else {
            (work.bottom() - size.y - SCREEN_MARGIN).max(work.top() + SCREEN_MARGIN)
        }
    } else {
        (work.top() + work.height() * 2.0 / 3.0)
            .min(work.bottom() - size.y - SCREEN_MARGIN)
            .max(work.top() + SCREEN_MARGIN)
    };
    egui::pos2(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1040.0))
    }

    fn host(left: f32, top: f32, w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(w, h))
    }

    #[test]
    fn below_host_when_room() {
        let p = place(Some(host(700.0, 100.0, 500.0, 400.0)), work(), egui::vec2(300.0, 50.0));
        assert_eq!(p, egui::pos2(800.0, 100.0 + 400.0 + GAP));
    }

    #[test]
    fn flips_above_when_no_room_below() {
        let p = place(Some(host(700.0, 600.0, 500.0, 400.0)), work(), egui::vec2(300.0, 50.0));
        assert_eq!(p.y, 600.0 - 50.0 - GAP);
    }

    #[test]
    fn centers_on_two_thirds_when_host_hidden() {
        let p = place(None, work(), egui::vec2(300.0, 50.0));
        assert_eq!(p.x, 810.0);
        assert!((p.y - 1040.0 * 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn stays_inside_work_area() {
        let p = place(
            Some(host(1800.0, 100.0, 100.0, 100.0)),
            work(),
            egui::vec2(300.0, 50.0),
        );
        assert_eq!(p.x, 1920.0 - 300.0);
    }

    #[test]
    fn width_caps_at_520() {
        assert_eq!(hud_width(900.0, 1920.0), MAX_W);
        assert_eq!(hud_width(100.0, 1920.0), 100.0 + 2.0 * PAD + 2.0);
    }

    #[test]
    fn width_respects_narrow_screen() {
        assert_eq!(hud_width(900.0, 400.0), 400.0 - 2.0 * SCREEN_MARGIN);
    }

    #[test]
    fn work_precedence_host_then_last_then_show_on_then_cursor() {
        let a = host(0.0, 0.0, 1920.0, 1040.0);
        let b = host(1920.0, 0.0, 1920.0, 1040.0);
        let c = host(-1920.0, 0.0, 1920.0, 1040.0);
        let d = host(0.0, -1080.0, 1920.0, 1040.0);
        assert_eq!(pick_work(Some(a), Some(b), Some(c), Some(d)), Some(a));
        assert_eq!(pick_work(None, Some(b), Some(c), Some(d)), Some(b));
        // 主窗没开、3 秒也过了 ⇒ 用 `show_on` 指定的屏，而不是鼠标屏。
        assert_eq!(pick_work(None, None, Some(c), Some(d)), Some(c));
        assert_eq!(pick_work(None, None, None, Some(d)), Some(d));
        assert_eq!(pick_work(None, None, None, None), None);
    }
}

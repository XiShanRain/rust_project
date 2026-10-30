//! FR-UIX-10：大字显示。
//!
//! 形态与 AHK 版 `LargeType.ahk` 对齐：屏幕中央一块无边框置顶窗，`141414` 底 + 白字，
//! 字号按文本长度分档，放不下就自动降档，`Esc`（以及点击、切走焦点）关闭。与 HUD 相反，
//! 这个窗口**要抢焦点**——不然 `Esc` 收不到；因此它是 `with_active(true)`、不做点击穿透，
//! 也不设 `WS_EX_NOACTIVATE`。
//!
//! 与 HUD 一样是延迟子视口。区别是 HUD 在主窗隐藏后仍要自绘，所以把动画与计时全放在
//! 自己的回调里；大字显示则让主窗留在原位——实测（2026-10-24）把主窗 `Visible(false)`
//! 之后一关大字，焦点会落到本进程那个隐藏的事件窗上，按键全部丢失；主窗留着，
//! 关闭时自己把焦点还给主窗（`SetForegroundWindow`），用户可以直接接着改查询。
//! 主窗留在原位还需要一处配套：大字窗必然抢走焦点，FR-WIN-04 本来会在 50ms 后把主窗
//! 藏到托盘，所以主窗那边的 keep_open 列表加了 [`LargeType::showing`]。
//!
//! 为什么不用 `ViewportCommand::Focus` 发回 ROOT：真机实测（2026-10-24）连跑两次，
//! 系统把大字窗藏起来后挑的「下一个活动窗」并不稳定（一次是主窗、一次是外部程序），
//! egui 那条命令没能改变结局；换成隐藏前由本进程直接激活主窗就稳定了。

use std::sync::{Arc, Mutex};

use eframe::egui;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, SetForegroundWindow, SetWindowLongPtrW, WS_EX_APPWINDOW,
    WS_EX_TOOLWINDOW,
};
use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
use windows::core::PCWSTR;

use crate::hud;

/// AHK 版 `g.BackColor := "141414"` + `SetFont cFFFFFF`，硬编码，不跟随主题。
const BG_RGB: egui::Color32 = egui::Color32::from_rgb(0x14, 0x14, 0x14);
const FG: egui::Color32 = egui::Color32::WHITE;
/// `WinSetTransparent(235)`：约 92% 不透明，与 AHK 版同一个数。
const OPACITY: f32 = 235.0 / 255.0;
/// AHK 版 `pad := Win.Scale(40)`。
const PAD: f32 = 40.0;
/// 文本最大占工作区宽度的比例（AHK `maxW := Round(areaW * 0.85) - 2 * pad`）。
const MAX_WIDTH_RATIO: f32 = 0.85;
/// 与工作区边缘的余量，保证窗口（含圆角）不贴边被裁。
const SCREEN_MARGIN: f32 = 8.0;
/// AHK 版 `Win.SetCorner(g.Hwnd)` 用的是主题圆角；这里固定一个视觉上够用的小圆角。
const CORNER: f32 = 12.0;
/// 首帧把窗口样式补成工具窗之前，最多请求几次焦点。
const FOCUS_TRIES: u32 = 30;

pub struct LargeType {
    shared: Arc<Mutex<Shared>>,
    /// 无边框窗口里不可见，只在首帧用来找回自己的 HWND 补扩展样式；带进程号防撞名。
    title: String,
    id: egui::ViewportId,
    /// 主窗 HWND（E-05 取的那一个），关闭时把焦点还回去用。
    /// 存成数值是因为 deferred 视口的回调要求 `Send + Sync`，裸 `HWND` 不满足。
    root: Option<usize>,
}

struct Shared {
    showing: Option<Showing>,
}

struct Showing {
    text: String,
    title: String,
    font: f32,
    /// 排版时用的换行宽度，绘制必须用同一个值，否则窗口尺寸与文字对不上。
    wrap: f32,
    pos: egui::Pos2,
    size: egui::Vec2,
    /// 首帧给窗口补过扩展样式没有。
    styled: bool,
    /// 这个视口有没有真正拿到过焦点。没拿到过就不认「失焦」——创建初期的几帧
    /// `focused` 可能还是 false，照它关会闪一下就消失。
    had_focus: bool,
    /// 还没拿到焦点时已请求过几次（见 [`FOCUS_TRIES`]）。
    focus_tries: u32,
}

impl LargeType {
    pub fn new(root: Option<HWND>) -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared { showing: None })),
            title: format!("ALTRun Large Type {}", std::process::id()),
            id: egui::ViewportId::from_hash_of("alt-run-large-type"),
            root: root.map(|h| h.0 as usize),
        }
    }

    /// 在工作区中央弹出大字。`text` 为空（或只有空白）时什么都不做、返回 `false`。
    /// 必须在主窗 `ui()` 里调用：要用主窗的字体排版，也要借它的 `pixels_per_point`。
    pub fn show(&mut self, ctx: &egui::Context, text: String) -> bool {
        let text = text.trim().to_string();
        if text.is_empty() {
            return false;
        }

        let ppp = ctx.pixels_per_point();
        let work_phys = hud::win_cursor()
            .and_then(hud::monitor_work_at)
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

        let chars = text.chars().count();
        let (font, wrap, text_size) = fit(work, chars, |font, wrap| {
            ctx.fonts_mut(|f| {
                f.layout(
                    text.clone(),
                    egui::FontId::proportional(font),
                    FG,
                    wrap,
                )
                .size()
            })
        });
        let size = window_size(text_size, work);
        let pos = centered(work, size);

        // 与其余动作日志同口径：只记长度与字号，显示原文不落盘。
        log_info!(
            "大字显示：显示（{chars} 个字符，字号 {font}，窗口 {}×{}）",
            size.x.round(),
            size.y.round()
        );
        let mut s = self.shared.lock().unwrap();
        s.showing = Some(Showing {
            text,
            title: self.title.clone(),
            font,
            wrap,
            pos,
            size,
            styled: false,
            had_focus: false,
            focus_tries: 0,
        });
        drop(s);

        self.render(ctx);
        // 收起时只发 `Visible(false)`，视口对象仍留在 egui 里；再次显示必须显式叫回来，
        // 否则同一个 ViewportId 会一直停在隐藏态。
        ctx.send_viewport_cmd_to(self.id, egui::ViewportCommand::Visible(true));
        true
    }

    /// 大字窗现在是不是开着。FR-WIN-04 的失焦自动隐藏据此豁免——不然大字窗一拿走
    /// 焦点，主窗 50ms 后就把自己藏了，关闭时再想激活它已经是个隐藏窗口。
    pub fn showing(&self) -> bool {
        self.shared.lock().unwrap().showing.is_some()
    }

    /// 主窗每帧调用；显示期间负责把子视口声明出来。主窗隐藏期间这里不跑，
    /// 但已声明的视口由它自己的回调续命（E-08 实测）。
    pub fn render(&mut self, ctx: &egui::Context) {
        let geometry = {
            let s = self.shared.lock().unwrap();
            s.showing.as_ref().map(|sh| (sh.pos, sh.size))
        };
        let Some((pos, size)) = geometry else { return };
        let shared = self.shared.clone();
        let title = self.title.clone();
        let root = self.root;
        ctx.show_viewport_deferred(self.id, builder(&title, pos, size), move |ui, _| {
            draw(ui, &shared, root);
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
        .with_active(true)
        .with_resizable(false)
}

fn draw(ui: &mut egui::Ui, shared: &Arc<Mutex<Shared>>, root: Option<usize>) {
    let ctx = ui.ctx().clone();
    let (text, font, wrap) = {
        let mut s = shared.lock().unwrap();
        let Some(sh) = s.showing.as_mut() else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            return;
        };
        if !sh.styled {
            sh.styled = patch_window_style(&sh.title);
        }

        let focused = ctx
            .input(|i| i.viewport().focused)
            .unwrap_or(false);
        if focused {
            sh.had_focus = true;
        } else if !sh.had_focus && sh.focus_tries < FOCUS_TRIES {
            sh.focus_tries += 1;
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let click = ctx.input(|i| i.pointer.any_click());
        let blurred = sh.had_focus && !focused;
        if esc || click || blurred {
            let reason = if esc {
                "Esc"
            } else if click {
                "点击"
            } else {
                "失焦"
            };
            s.showing = None;
            drop(s);
            log_info!("大字显示：收起（{reason}）");
            // 自己按键关掉的，焦点要还回主窗，接着改查询。**必须在藏窗之前**：
            // 藏起来的一瞬间系统会自己挑"下一个活动窗"，挑到谁不可控；由本进程
            // （此刻正是前台进程）先把主窗激活，藏窗就不会再转移焦点。
            // 「失焦」这一档不还——那时用户已经去点别的程序了，抢回来反而是错的。
            if (esc || click) && !return_to_root(root) {
                log_info!("大字显示：焦点没能还给主窗（SetForegroundWindow 失败）");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            return;
        }

        (sh.text.clone(), sh.font, sh.wrap)
    };

    let bg = hud::with_alpha(BG_RGB, OPACITY);
    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(bg)
                .corner_radius(CORNER)
                .inner_margin(egui::Margin::same(PAD as i8)),
        )
        .show(ui, |ui| {
            let galley = ui.ctx().fonts_mut(|f| {
                f.layout(
                    text,
                    egui::FontId::proportional(font),
                    FG,
                    wrap,
                )
            });
            let rect = ui.max_rect();
            let pos = egui::pos2(
                rect.left() + (rect.width() - galley.size().x).max(0.0) * 0.5,
                rect.top() + (rect.height() - galley.size().y).max(0.0) * 0.5,
            );
            ui.painter().galley(pos, galley, FG);
        });
}

/// 首帧把窗口补成工具窗：不进 Alt+Tab / 任务栏。与 HUD 的版本共用一份逻辑，
/// 区别是大字显示**要**能拿焦点，所以不设 `WS_EX_NOACTIVATE`。
fn patch_window_style(title: &str) -> bool {
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let Ok(hwnd) = (unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) }) else {
        return false;
    };
    let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let want = (ex | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0;
    if want != ex {
        unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want as isize) };
    }
    true
}

/// 把前台还给主窗。只在**本进程还是前台**（大字窗有焦点）时调用才有把握成功；
/// 调用方须在藏大字窗之前调。
fn return_to_root(root: Option<usize>) -> bool {
    let Some(raw) = root else { return false };
    if raw == 0 {
        return false;
    }
    unsafe { SetForegroundWindow(HWND(raw as *mut _)) }.as_bool()
}

/// 字号阶梯（需求书 FR-UIX-10 原文；字符数计），与 AHK 版逐档一致。
fn ladder_font(chars: usize) -> f32 {
    match chars {
        0..=12 => 96.0,
        13..=30 => 64.0,
        31..=80 => 40.0,
        81..=300 => 28.0,
        _ => 18.0,
    }
}

/// 阶梯档位装不下时的下一档。低于需求书阶梯（18）后继续收，直到 8——宁可字小，
/// 也不让窗口溢出屏幕。
fn next_smaller(size: f32) -> Option<f32> {
    const STEPS: [f32; 8] = [64.0, 40.0, 28.0, 24.0, 18.0, 14.0, 10.0, 8.0];
    STEPS.iter().copied().find(|&step| step < size)
}

/// 文本排列宽度上限：工作区宽度的 85% 再扣掉两侧留白。
fn wrap_limit(work: egui::Rect) -> f32 {
    (work.width() * MAX_WIDTH_RATIO - 2.0 * PAD).max(1.0)
}

/// 选字号，直到文本竖着放得进工作区。`measure(font, wrap)` 返回该字号下的文本尺寸；
/// 换行宽度取「自然宽 ≤ 上限就不换行，否则按上限换行」（AHK 的 `AutoSize` + 超宽重排）。
fn fit(
    work: egui::Rect,
    chars: usize,
    measure: impl Fn(f32, f32) -> egui::Vec2,
) -> (f32, f32, egui::Vec2) {
    let max_w = wrap_limit(work);
    let max_h = (work.height() - 2.0 * SCREEN_MARGIN).max(1.0);
    let mut font = ladder_font(chars);
    loop {
        let natural = measure(font, f32::INFINITY);
        let (wrap, size) = if natural.x <= max_w {
            (natural.x.max(1.0), natural)
        } else {
            (max_w, measure(font, max_w))
        };
        match next_smaller(font) {
            Some(smaller) if size.y + 2.0 * PAD > max_h => font = smaller,
            _ => return (font, wrap, size),
        }
    }
}

/// 窗口尺寸 = 文本尺寸 + 两侧留白，且不超出工作区（极端长文本在 8 号仍超高时截断）。
fn window_size(text: egui::Vec2, work: egui::Rect) -> egui::Vec2 {
    egui::vec2(
        (text.x + 2.0 * PAD).min((work.width() - 2.0 * SCREEN_MARGIN).max(1.0)),
        (text.y + 2.0 * PAD).min((work.height() - 2.0 * SCREEN_MARGIN).max(1.0)),
    )
}

/// 在工作区内居中。
fn centered(work: egui::Rect, size: egui::Vec2) -> egui::Pos2 {
    egui::pos2(
        work.left() + (work.width() - size.x) * 0.5,
        work.top() + (work.height() - size.y) * 0.5,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1040.0))
    }

    #[test]
    fn ladder_matches_the_requirement() {
        assert_eq!(ladder_font(1), 96.0);
        assert_eq!(ladder_font(12), 96.0);
        assert_eq!(ladder_font(13), 64.0);
        assert_eq!(ladder_font(30), 64.0);
        assert_eq!(ladder_font(31), 40.0);
        assert_eq!(ladder_font(80), 40.0);
        assert_eq!(ladder_font(81), 28.0);
        assert_eq!(ladder_font(300), 28.0);
        assert_eq!(ladder_font(301), 18.0);
        assert_eq!(ladder_font(5000), 18.0);
    }

    #[test]
    fn shrink_steps_are_strictly_smaller_and_terminate() {
        let mut seen = Vec::new();
        let mut font = ladder_font(1);
        while let Some(next) = next_smaller(font) {
            assert!(next < font, "降档必须严格变小：{font} → {next}");
            seen.push(next);
            font = next;
        }
        // 需求书阶梯之后的兜底档位必须能落到 8，长文本才有不溢出屏幕的机会。
        assert_eq!(seen, vec![64.0, 40.0, 28.0, 24.0, 18.0, 14.0, 10.0, 8.0]);
        assert_eq!(next_smaller(18.0), Some(14.0));
        assert_eq!(next_smaller(8.0), None);
    }

    #[test]
    fn wrap_limit_is_85_percent_minus_padding() {
        assert_eq!(wrap_limit(work()), 1920.0 * 0.85 - 2.0 * PAD);
        // 窄屏也不能算出 0 或负数（换行宽度 0 会把每个字都排成一行）。
        assert_eq!(wrap_limit(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(10.0, 10.0)
        )), 1.0);
    }

    #[test]
    fn short_text_keeps_the_top_ladder_and_does_not_wrap() {
        // 一个按字号线性缩放的假排版：10 个字符 × 0.6 字宽。
        let (font, wrap, size) = fit(work(), 10, |font, _wrap| {
            egui::vec2(font * 6.0, font * 1.2)
        });
        assert_eq!(font, 96.0);
        assert_eq!(wrap, 96.0 * 6.0);
        assert_eq!(size, egui::vec2(96.0 * 6.0, 96.0 * 1.2));
    }

    #[test]
    fn wide_text_wraps_at_the_limit_instead_of_widening_the_window() {
        let (font, wrap, size) = fit(work(), 10, |font, wrap| {
            // 自然宽 2000；按 wrap 换行后高度翻倍。
            if wrap.is_finite() {
                egui::vec2(wrap, font * 2.5)
            } else {
                egui::vec2(2000.0, font * 1.2)
            }
        });
        assert_eq!(font, 96.0);
        assert_eq!(wrap, wrap_limit(work()));
        assert_eq!(size.x, wrap_limit(work()));
        assert!(size.x + 2.0 * PAD < work().width());
    }

    #[test]
    fn tall_text_shrinks_until_it_fits() {
        // 假排版：高度 = 字号 × 40（96 号时 3840 高，明显超屏；40 号也还有 1600）。
        let (font, _, size) = fit(work(), 10, |font, _wrap| {
            egui::vec2(font * 3.0, font * 40.0)
        });
        assert!(size.y + 2.0 * PAD <= work().height() - 2.0 * SCREEN_MARGIN);
        assert!(font < 96.0, "装不下就必须降档");
        // 需求书阶梯（96→64→40→28）之后继续收，到 18 才放得下（720 + 80 ≤ 1024）。
        assert_eq!(font, 18.0);
    }

    #[test]
    fn absurd_text_stops_at_the_floor_and_still_clamps() {
        let (font, _, size) = fit(work(), 10, |font, _wrap| {
            egui::vec2(font * 3.0, font * 200.0)
        });
        assert_eq!(font, 8.0);
        let size = window_size(size, work());
        assert!(size.x <= work().width() - 2.0 * SCREEN_MARGIN);
        assert!(size.y <= work().height() - 2.0 * SCREEN_MARGIN);
    }

    #[test]
    fn window_is_centered_and_inside_the_work_area() {
        let w = work();
        let size = egui::vec2(600.0, 200.0);
        let pos = centered(w, size);
        assert_eq!(pos.x, w.left() + (w.width() - 600.0) / 2.0);
        assert_eq!(pos.y, w.top() + (w.height() - 200.0) / 2.0);

        // 内容再大也只占工作区减两侧余量。
        let huge = window_size(egui::vec2(5000.0, 5000.0), w);
        assert_eq!(huge.x, w.width() - 2.0 * SCREEN_MARGIN);
        assert_eq!(huge.y, w.height() - 2.0 * SCREEN_MARGIN);
        let pos = centered(w, huge);
        assert!(pos.x >= w.left());
        assert!(pos.y >= w.top());
        assert!(pos.x + huge.x <= w.right());
        assert!(pos.y + huge.y <= w.bottom());
    }
}

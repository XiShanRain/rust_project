//! FR-WIN-03 双击呼出：连按两下 `Ctrl` 或 `Shift` 把窗口呼出（再连按两下收起）。
//!
//! 语义整条照 AHK 版：`Src/Core/App.ahk:459-490` 的 `DoubleTapMs / TapHoldMs / _TapDown /
//! _TapUp / TapDecision`，验收矩阵照 `Tests/RunTests.ahk:1000-1010` 那张表。三档由
//! `AppSettings.dbl_tap` 决定（`off` / `ctrl` / `shift`，出厂关）。
//!
//! 三条判据逐条对上 AHK：
//! ① **两次单独的短按**——松开那一刻"最后按下的键"必须还是这颗键本身（`prior_vk == vk`）。
//!    中间按过 `C`（`Ctrl+C`）或另一颗键（`Shift+CapsLock`）就作废；这是需求书
//!    FR-WIN-03 点名「`Shift+CapsLock` 等组合不受影响」那条。
//! ② **按住不超过 300ms**：长按（`held > TapHoldMs`）算"用户就是在用这个修饰键"，不配对。
//! ③ **两下间隔不超过 400ms**：第一下松开到第二下松开的距离。
//!
//! 与 AHK 的两处**有意偏差**，都写在需求书 FR-WIN-03 里：
//! * 最小间隔 25ms（`MIN_GAP_MS`）：AHK 没有这条。需求书点名要「忽略远控软件的同步抖动」，
//!   而抖动表现出来就是同一颗键的两下 up 挨得比人能打出来的还近 ⇒ 这种配对直接作废，
//!   **不做**「当一下处理」（那样会把用户真正想要的第一下也吃掉）。
//! * 不滤 `LLKHF_INJECTED`：远控软件（以及本项目的自动化测试）发的就是注入键，
//!   滤掉等于把需求书那半句话反向实现。
//!
//! 纯函数 + 纯状态机，零 Win32：钩子、时间戳、投递都在 `main.rs`。这里只管
//! 「这一次按键该不该算一次「双击」」。

use serde::{Deserialize, Serialize};

/// 两下之间的窗口（AHK `DoubleTapMs`）。
pub const DOUBLE_TAP_MS: u32 = 400;
/// 单下按住的阈值，超过就当"长按"（AHK `TapHoldMs`）。
pub const TAP_HOLD_MS: u32 = 300;
/// 需求书 FR-WIN-03 的抖动抑制：两下间隔比这还短的不可能是人打出来的。
pub const MIN_GAP_MS: u32 = 25;

/// 左右修饰键的 VK。低级键盘钩子（`WH_KEYBOARD_LL`）给的就是这一对**分左右**的码，
/// 不是 `VK_CONTROL`/`VK_SHIFT` 那两个笼统值。
pub const VK_LSHIFT: u32 = 0xA0;
pub const VK_RSHIFT: u32 = 0xA1;
pub const VK_LCONTROL: u32 = 0xA2;
pub const VK_RCONTROL: u32 = 0xA3;
/// 笼统的 `VK_SHIFT` / `VK_CONTROL`。真键盘不走这两个，但 `SendInput` 里的
/// `wVk = VK_CONTROL` 与某些重映射工具就是它们 ⇒ 一并认，左右不分。
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;

/// 双击呼出用哪一颗键（`AppSettings.dbl_tap`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DblTap {
    /// 出厂：不用双击呼出。
    #[default]
    Off,
    Ctrl,
    Shift,
}

impl DblTap {
    /// 设置页三选一的顺序就是这张表（关、Ctrl、Shift）。
    pub const ALL: [Self; 3] = [Self::Off, Self::Ctrl, Self::Shift];

    /// 落盘与设置页用的档位名（与 `SecondaryAction::as_str` 同口径）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Ctrl => "ctrl",
            Self::Shift => "shift",
        }
    }

    /// 设置页显示名。
    pub fn label(self) -> &'static str {
        crate::i18n::t(match self {
            Self::Off => "dbltap.off",
            Self::Ctrl => "dbltap.ctrl",
            Self::Shift => "dbltap.shift",
        })
    }

    /// `WM_APP_SET_DOUBLE_TAP` 的 `wparam`。只在这条消息上用，不落盘。
    pub fn code(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Ctrl => 1,
            Self::Shift => 2,
        }
    }

    /// 消息里的码 → 档位。越界返回 `None`（投递端与热键线程同版本，正常不会发生；
    /// 真发生了就当没收到，不在热键线程里 panic）。
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Off),
            1 => Some(Self::Ctrl),
            2 => Some(Self::Shift),
            _ => None,
        }
    }

    /// 这颗 vk 归不归本档管。
    pub fn matches_vk(self, vk: u32) -> bool {
        match self {
            Self::Off => false,
            Self::Ctrl => matches!(vk, VK_LCONTROL | VK_RCONTROL | VK_CONTROL),
            Self::Shift => matches!(vk, VK_LSHIFT | VK_RSHIFT | VK_SHIFT),
        }
    }

    /// 要不要装低级键盘钩子。`Off` 不装：不装就一个按键事件都看不见，代价为零。
    pub fn needs_hook(self) -> bool {
        self != Self::Off
    }
}

/// 一次按键抬起的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapDecision {
    /// 第二下 ⇒ 切一次显隐（AHK 里的 `"show"`）。
    Show,
    /// 第一下 ⇒ 记下时刻等第二下（AHK 里的 `"first"`）。
    First,
    /// 不算数：是组合键、按太久、隔太久（不算第一下）、或是抖动（AHK 里的 `"reset"`）。
    Reset,
}

/// 计时状态机。**时间戳由调用方给**（毫秒，同一个单调时钟），这里不读时钟 ⇒ 单测能直接喂数。
///
/// `const fn new` 是为了能写成 `static TAP_TRACKER: Mutex<TapTracker> = Mutex::new(TapTracker::new(DblTap::Off));`
/// 那种不需要 `OnceLock`/`LazyLock` 的静态（钩子回调里取锁的路径越短越好）。
#[derive(Debug)]
pub struct TapTracker {
    mode: DblTap,
    /// 最后按下的那个键（AHK `A_PriorKey`，**任何键都记**，包括不归本档管的）。
    prior_vk: u32,
    /// 本档的修饰键是否正按着。按住不放时的自动重复不重记时刻（AHK `_tapHeld`）。
    held: bool,
    down_at: u32,
    /// 上一"第一下"松开的时刻；`None` = 没有在等第二下的。
    last_tap_at: Option<u32>,
}

impl TapTracker {
    pub const fn new(mode: DblTap) -> Self {
        Self {
            mode,
            prior_vk: 0,
            held: false,
            down_at: 0,
            last_tap_at: None,
        }
    }

    /// 换档。**半下没抬的那颗键不许跨档配对** ⇒ 除了模式，其余状态一律清空。
    pub fn set_mode(&mut self, mode: DblTap) {
        *self = Self::new(mode);
    }

    /// 喂一次按键事件。`now_ms` 只需在两次调用之间单调（同一进程内 `Instant` 的毫秒数）。
    ///
    /// 只有抬起才可能返回 `Show`/`First`；按下与不归本档管的键一律 `Reset`
    /// （调用方只对 `Show` 有动作，`Reset` 是"什么都不做"的写法）。
    pub fn on_key(&mut self, vk: u32, is_up: bool, now_ms: u32) -> TapDecision {
        if !self.mode.needs_hook() {
            return TapDecision::Reset;
        }
        let mine = self.mode.matches_vk(vk);
        if !is_up {
            // AHK 的 `A_PriorKey` 是"任何键的最后一次按下"，所以这一句对所有键都执行。
            self.prior_vk = vk;
            if mine && !self.held {
                // 自动重复（按住不放时系统会连续发 down）不重记时刻。
                self.held = true;
                self.down_at = now_ms;
            }
            return TapDecision::Reset;
        }
        if !mine {
            return TapDecision::Reset;
        }
        self.held = false;
        let held_ms = now_ms.saturating_sub(self.down_at);
        let since_last = self.last_tap_at.map(|t| now_ms.saturating_sub(t));
        let decision = tap_decision(self.prior_vk, vk, held_ms, since_last);
        match decision {
            TapDecision::Show => self.last_tap_at = None,
            TapDecision::First => self.last_tap_at = Some(now_ms),
            TapDecision::Reset => self.last_tap_at = None,
        }
        decision
    }
}

/// 判定本体（AHK `App.TapDecision` 的逐字转写 + 抖动一条）。
///
/// * `prior_vk`：松开那一刻最后按下的键；`vk`：正在松开的那颗。
/// * `held_ms`：这一下按了多久；`since_last`：`None` = 之前没有待配对的"第一下"。
///
/// 顺序有意义：先判"是不是单独按的"（组合键直接作废，**不**顺延成第一下），再判时长，
/// 最后才是配对窗口。抖动那条插在配对之前：它作废的是这次配对，但也不把它记成新的第一下
/// ——两下挨得比 25ms 还近，第二下本身不可信。
pub fn tap_decision(
    prior_vk: u32,
    vk: u32,
    held_ms: u32,
    since_last: Option<u32>,
) -> TapDecision {
    if prior_vk != vk || held_ms > TAP_HOLD_MS {
        return TapDecision::Reset;
    }
    match since_last {
        Some(gap) if gap < MIN_GAP_MS => TapDecision::Reset,
        Some(gap) if gap <= DOUBLE_TAP_MS => TapDecision::Show,
        _ => TapDecision::First,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 喂一次完整的"按下 + 抬起"，返回抬起那一刻的结论。
    fn tap(t: &mut TapTracker, vk: u32, down_ms: u32, up_ms: u32) -> TapDecision {
        let _ = t.on_key(vk, false, down_ms);
        t.on_key(vk, true, up_ms)
    }

    /// AHK `Tests/RunTests.ahk:1000-1010` 那张矩阵，逐条照搬。
    #[test]
    fn ahk_tap_matrix() {
        let mut t = TapTracker::new(DblTap::Ctrl);
        // first tap（AHK: priorKey=LControl, key=LCtrl, held 80, sinceLast 99999）
        assert_eq!(tap(&mut t, VK_LCONTROL, 0, 80), TapDecision::First);
        // second tap（间隔 250 ⇒ 认）
        assert_eq!(tap(&mut t, VK_LCONTROL, 330, 410), TapDecision::Show);
        // 认过之后 `_tapLast := 0`：第三下必须重新从 First 开始
        assert_eq!(tap(&mut t, VK_LCONTROL, 500, 580), TapDecision::First);
        // right ctrl：左右是两个键，但各自都能配成对
        assert_eq!(tap(&mut t, VK_RCONTROL, 600, 680), TapDecision::Show);
        // shift 档
        let mut s = TapTracker::new(DblTap::Shift);
        assert_eq!(tap(&mut s, VK_LSHIFT, 0, 80), TapDecision::First);
        assert_eq!(tap(&mut s, VK_RSHIFT, 200, 280), TapDecision::Show);
    }

    #[test]
    fn too_slow_restarts_instead_of_pairing() {
        let mut t = TapTracker::new(DblTap::Ctrl);
        assert_eq!(tap(&mut t, VK_LCONTROL, 0, 80), TapDecision::First);
        // AHK "too slow"：间隔 900（> 400）⇒ 这次算新的第一下，不是 show
        assert_eq!(tap(&mut t, VK_LCONTROL, 900, 980), TapDecision::First);
        // 而且它还真的被记成了新的第一下 ⇒ 紧跟的那一下能配上
        assert_eq!(tap(&mut t, VK_LCONTROL, 1000, 1080), TapDecision::Show);
    }

    #[test]
    fn combo_key_resets_and_does_not_count_as_first() {
        let mut t = TapTracker::new(DblTap::Ctrl);
        let _ = t.on_key(VK_LCONTROL, false, 0); // 按住 Ctrl
        let _ = t.on_key(0x43, false, 10); // C
        let _ = t.on_key(0x43, true, 20); // 松开 C
        // 松开 Ctrl：prior_vk 已经是 C ⇒ 组合键，作废
        assert_eq!(t.on_key(VK_LCONTROL, true, 80), TapDecision::Reset);
        // 作废不清"第一下"之外的东西，但它本身没被记成第一下 ⇒ 下一对仍然 First→Show
        assert_eq!(tap(&mut t, VK_LCONTROL, 200, 280), TapDecision::First);
        assert_eq!(tap(&mut t, VK_LCONTROL, 300, 380), TapDecision::Show);
    }

    #[test]
    fn shift_capslock_is_not_a_double_tap() {
        // 需求书 FR-WIN-03 点名的那一条：`Shift+CapsLock` 不算双击 Shift。
        let mut t = TapTracker::new(DblTap::Shift);
        let _ = t.on_key(VK_LSHIFT, false, 0);
        let _ = t.on_key(0x14, false, 30); // CapsLock
        let _ = t.on_key(0x14, true, 60);
        assert_eq!(t.on_key(VK_LSHIFT, true, 90), TapDecision::Reset);
    }

    #[test]
    fn held_too_long_is_not_a_tap() {
        let mut t = TapTracker::new(DblTap::Ctrl);
        // AHK "held too long"：按住 800ms（> 300）⇒ reset
        assert_eq!(tap(&mut t, VK_LCONTROL, 0, 800), TapDecision::Reset);
        // 300ms 整仍然算（判据是 `> TapHoldMs`，不是 `>=`）
        assert_eq!(tap(&mut t, VK_LCONTROL, 1000, 1300), TapDecision::First);
    }

    #[test]
    fn auto_repeat_does_not_extend_the_hold() {
        let mut t = TapTracker::new(DblTap::Ctrl);
        // 按住不放时系统会连续发 down；时刻只认第一次
        for at in [0, 30, 60, 90, 120, 150, 180, 210, 240, 270] {
            let _ = t.on_key(VK_LCONTROL, false, at);
        }
        // 250 是第一次 down 之后 250ms ⇒ 没过 300；若按最后一次重复算只有 10ms，
        // 两种都比 300 小，所以这条真正挡的是"重复 down 把 prior_vk/held 搞乱"：
        // 结论仍然是 First（能配对），不是 Reset。
        assert_eq!(t.on_key(VK_LCONTROL, true, 250), TapDecision::First);
    }

    #[test]
    fn jitter_under_25ms_is_dropped() {
        // 需求书 FR-WIN-03 新增的那一条（AHK 没有）：远控软件的同步抖动。
        let mut t = TapTracker::new(DblTap::Ctrl);
        assert_eq!(tap(&mut t, VK_LCONTROL, 0, 80), TapDecision::First);
        // 第二下两下贴着 10ms ⇒ 作废，且**不**把它记成新的第一下
        assert_eq!(tap(&mut t, VK_LCONTROL, 85, 90), TapDecision::Reset);
        // 再来一下，跟上一个"真第一下"（80）已经隔了 400 多 ⇒ 是新第一下
        assert_eq!(tap(&mut t, VK_LCONTROL, 500, 580), TapDecision::First);
    }

    #[test]
    fn off_mode_sees_nothing() {
        let mut t = TapTracker::new(DblTap::Off);
        assert!(!DblTap::Off.needs_hook());
        assert_eq!(tap(&mut t, VK_LCONTROL, 0, 80), TapDecision::Reset);
        assert_eq!(tap(&mut t, VK_LCONTROL, 100, 180), TapDecision::Reset);
    }

    #[test]
    fn other_modifiers_do_not_leak_into_the_wrong_mode() {
        // Shift 档下按 Ctrl 不算数（也不进配对），但仍然会更新 prior_vk：
        // 按住 Shift 再按 Ctrl、松开 Shift ⇒ 组合键 ⇒ Reset。
        let mut t = TapTracker::new(DblTap::Shift);
        assert_eq!(tap(&mut t, VK_LCONTROL, 0, 80), TapDecision::Reset);
        let _ = t.on_key(VK_LSHIFT, false, 100);
        let _ = t.on_key(VK_LCONTROL, false, 120);
        let _ = t.on_key(VK_LCONTROL, true, 140);
        assert_eq!(t.on_key(VK_LSHIFT, true, 160), TapDecision::Reset);
    }

    #[test]
    fn changing_mode_drops_a_half_pressed_key() {
        let mut t = TapTracker::new(DblTap::Ctrl);
        assert_eq!(tap(&mut t, VK_LCONTROL, 0, 80), TapDecision::First);
        // 换档重置 ⇒ 上一档的"第一下"不许跨档配对
        t.set_mode(DblTap::Ctrl);
        assert_eq!(tap(&mut t, VK_LCONTROL, 100, 180), TapDecision::First);
        // 半下没抬就换档：这次抬起是"孤儿"（`prior_vk` 已被清成 0）⇒ 作废，
        // 不许被当成上一档的第二下。
        let _ = t.on_key(VK_LCONTROL, false, 200);
        t.set_mode(DblTap::Ctrl);
        assert_eq!(t.on_key(VK_LCONTROL, true, 280), TapDecision::Reset);
    }

    #[test]
    fn code_round_trips_and_labels_are_stable() {
        for d in DblTap::ALL {
            assert_eq!(DblTap::from_code(d.code()), Some(d));
            assert!(!d.label().is_empty());
        }
        assert_eq!(DblTap::from_code(9), None);
        assert_eq!(DblTap::default(), DblTap::Off);
        assert_eq!(DblTap::Ctrl.as_str(), "ctrl");
    }
}

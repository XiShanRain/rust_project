//! 批次 F9（FR-UIX-06）：窗口合成层的两件事——「藏 → 画 → 露」防首帧白闪、关闭系统过渡动画。
//!
//! 两件都只设 DWM 属性：不改窗口几何、不进 egui 渲染管线，DWM 不认某个属性时返回失败，
//! 调用方记一条日志、行为退回改动前。
//!
//! 这里唯一能弄坏主功能的失败方式是**窗口一直被藏着**——用户按热键没反应、还以为程序挂了。
//! 所以放行有两条判据（看够帧数 / 等够时间），任一条满足就放（[`release_now`]）。
//!
//! 但时间闸**只在这一遍 `ui()` 真的跑起来时才查得到**：一帧都不跑的话没有任何代码能读钟。
//! 「藏住之后再也不跑帧」那条路不归这里管，由调用方两条兜住——呼出时主动
//! `request_repaint()` 要一帧、隐藏时（[`CloakGate`] 还在手上）当场放行。

use std::time::Instant;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAK, DWMWA_CLOAKED, DWMWA_TRANSITIONS_FORCEDISABLED, DwmGetWindowAttribute,
    DwmSetWindowAttribute,
};

/// 至少要看过的 `ui()` 遍数。
///
/// eframe 的循环是 update → tessellate → paint，所以第 2 遍 `ui()` 开始时第 1 遍已经画完并提交。
/// 在第 2 遍里放行，露出来的是"有内容的那一帧"，而不是窗口刚建 / 刚 `ShowWindow` 时的空框。
pub const CLOAK_MIN_FRAMES: u8 = 2;

/// 兜底闸：某一遍 `ui()` 已经晚于这个时长，那一遍就直接放行（不必再等够帧数）。
///
/// 它救不了"藏起来之后再也没有下一帧"——没有帧就没有代码读钟（那条路由调用方的
/// `request_repaint()` 与 `on_hide` 里的当场放行负责）。它管的是另一个坏情形：
/// 呼出之后第一次跑帧就已经过了这么久，再等第 2 帧只是白白多藏一帧。
pub const CLOAK_DEADLINE_MS: u64 = 300;

/// 放行判据（纯函数，便于逐格钉边界）。
#[must_use]
pub fn release_now(painted_frames: u8, elapsed_ms: u64) -> bool {
    painted_frames >= CLOAK_MIN_FRAMES || elapsed_ms >= CLOAK_DEADLINE_MS
}

/// 一次「藏」的现场。放在 `Option` 里：`None` ＝ 没藏着。
#[derive(Debug, Clone, Copy)]
pub struct CloakGate {
    armed_at: Instant,
    frames: u8,
}

impl CloakGate {
    #[must_use]
    pub fn arm(now: Instant) -> Self {
        Self {
            armed_at: now,
            frames: 0,
        }
    }

    /// 记一遍 `ui()`，返回这一遍该不该放行。
    pub fn tick(&mut self, now: Instant) -> bool {
        self.frames = self.frames.saturating_add(1);
        release_now(self.frames, self.elapsed_ms_at(now))
    }

    #[must_use]
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms_at(Instant::now())
    }

    #[must_use]
    fn elapsed_ms_at(&self, now: Instant) -> u64 {
        // `Instant::duration_since` 在旧版会 panic；这里用 saturating 那支，时钟回拨也得算数。
        u64::try_from(now.saturating_duration_since(self.armed_at).as_millis()).unwrap_or(u64::MAX)
    }

    #[must_use]
    pub fn frames(&self) -> u8 {
        self.frames
    }
}

/// 设 / 清 `DWMWA_CLOAK`。返回 false ＝ DWM 不认这个属性，调用方应退回改动前的行为。
pub fn set_cloak(hwnd: HWND, cloak: bool) -> bool {
    let value: i32 = i32::from(cloak);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            (&value as *const i32).cast(),
            std::mem::size_of::<i32>() as u32,
        )
        .is_ok()
    }
}

/// 回读 `DWMWA_CLOAKED`（DWM 自己算的"当前是否被藏"），用于日志自证。
/// `None` ＝ 这个属性读不出来（老系统 / DWM 合成未开）。
pub fn read_cloaked(hwnd: HWND) -> Option<bool> {
    let mut value: u32 = 0;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut value as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
        .ok()
    }
    .map(|()| value != 0)
}

/// 关掉 DWM 给窗口加的显示/隐藏过渡动画（本程序自己的淡入是 egui 画的 alpha，不受影响）。
pub fn disable_transitions(hwnd: HWND) -> bool {
    let value: i32 = 1;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            (&value as *const i32).cast(),
            std::mem::size_of::<i32>() as u32,
        )
        .is_ok()
    }
}

/// 回读过渡动画开关，用于日志自证。
pub fn read_transitions_disabled(hwnd: HWND) -> Option<bool> {
    let mut value: u32 = 0;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            (&mut value as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
        .ok()
    }
    .map(|()| value != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 两条判据各自都能单独放行——少一条就等于把"窗口永久藏住"这条故障重新装回去。
    #[test]
    fn either_criterion_alone_releases() {
        assert!(!release_now(0, 0), "刚藏起来既没画也没等，不许放");
        assert!(!release_now(1, 0), "只画过一遍：第 1 遍还没提交，放出来就是空框");
        assert!(release_now(CLOAK_MIN_FRAMES, 0), "画够帧数就该放，不看时间");
        assert!(release_now(0, CLOAK_DEADLINE_MS), "一帧都没等到，时间到了也必须放");
        assert!(!release_now(1, CLOAK_DEADLINE_MS - 1), "两个判据都差一点 ⇒ 不放");
    }

    /// 边界钉死：deadline 前 1 ms 不放、正好到点放；帧数同理。
    #[test]
    fn boundaries_are_inclusive_on_the_release_side() {
        assert!(!release_now(1, CLOAK_DEADLINE_MS - 1));
        assert!(release_now(1, CLOAK_DEADLINE_MS));
        assert!(!release_now(CLOAK_MIN_FRAMES - 1, 0));
        assert!(release_now(CLOAK_MIN_FRAMES, 0));
        // 帧数是个 u8：数满也不能绕回 0（saturing_add）。
        assert!(release_now(u8::MAX, 0));
    }

    /// 门的状态机：arm 之后第 1 遍不放、第 2 遍放；另一条路是"一直在画但没画够"被时间闸救出来。
    #[test]
    fn gate_holds_one_frame_then_releases_on_the_second() {
        let t0 = Instant::now();
        let mut gate = CloakGate::arm(t0);
        assert!(!gate.tick(t0 + Duration::from_millis(5)), "第 1 遍：第 1 帧还没提交");
        assert_eq!(gate.frames(), 1);
        assert!(gate.tick(t0 + Duration::from_millis(10)), "第 2 遍：第 1 帧已经画完");
        assert_eq!(gate.frames(), 2);
    }

    /// 时间闸那条路：第 1 遍 `ui()` 就已经晚于兜底时长时直接放行（少藏一帧）；
    /// 差 1 ms 则不放。
    ///
    /// 注意这条闸**只在有帧的时候查得到**——「藏起来之后再也不跑帧」时它救不了，
    /// 那条路由调用方 `request_repaint()` 与 `on_hide` 当场放行两条堵（见模块文档）。
    #[test]
    fn a_late_first_frame_is_released_by_the_deadline() {
        let t0 = Instant::now();
        let mut gate = CloakGate::arm(t0);
        assert!(
            !gate.tick(t0 + Duration::from_millis(CLOAK_DEADLINE_MS - 1)),
            "差 1 ms：既没画够也不到点 ⇒ 不放"
        );
        assert_eq!(gate.frames(), 1);

        let mut late = CloakGate::arm(t0);
        assert!(
            late.tick(t0 + Duration::from_millis(CLOAK_DEADLINE_MS)),
            "第 1 遍就来晚了 ⇒ 当场放行，不必等第 2 帧"
        );
        assert_eq!(late.frames(), 1, "它是第 1 遍就放的，不是靠帧数凑够的");
    }
}

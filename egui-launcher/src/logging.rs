//! 批次 G5（FR-SYS-03）：日志级别闸门 + 性能日志统一前缀。
//!
//! 本程序不引 `tracing`（取向：零新增依赖面，见需求书 FR-SYS-03 的 2026-10-19 注记），
//! 日志仍是 stderr —— GUI 子系统下由 `redirect_stderr_if_orphan()` 牵到 `launcher.log`。
//! 这里只加一层**级别闸门**：全部调用点过 `log_debug!` / `log_info!`，`eprintln!` 只活在
//! 两个宏体里（`src/` 其余位置 `grep -c 'eprintln!('` 归零就是这条纪律的判据）。
//!
//! 两档够用的理由：本程序的日志不是给终端用户读的故事，是**排障证据**。「标准」保留
//! 生命周期、用户动作、写盘结果与失败；「详细」再加逐条 / 逐帧 / 逐键的诊断与 `perf:`
//! 计时。三档以上（Warn / Error）在这里没有落点：现有 478 条日志里没有任何一条带
//! "这行比其他行更该被保留"的机器判据，硬分只会把分类成本摊给未来每一次改动。
//!
//! 缺省跟随构建（FR-SYS-03 原文「Debug 构建 DEBUG、Release 构建 INFO」），设置页可覆盖。

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 两档详细度。`Debug` = 详细（更啰嗦），`Info` = 标准。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
}

impl LogLevel {
    /// 设置页三选一的中间两档（第一档是「跟随构建」，落盘为 `None`）。
    pub const ALL: [Self; 2] = [Self::Info, Self::Debug];

    /// 落盘用的名字（与 `DblTap::as_str` 同口径）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
        }
    }

    /// 设置页显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Debug => "详细",
            Self::Info => "标准",
        }
    }

    /// 设置页说明（一句话：多出来的是哪些行）。
    pub fn hint(self) -> &'static str {
        match self {
            Self::Debug => "再加逐条 / 逐键诊断与 `perf:` 计时行（排障时用）",
            Self::Info => "生命周期、用户动作、写盘结果与失败",
        }
    }

    /// 档位序号：数字越小越啰嗦（闸门只比大小，不认名字）。
    const fn code(self) -> u8 {
        match self {
            Self::Debug => 0,
            Self::Info => 1,
        }
    }

    /// 需求书 FR-SYS-03 的缺省：Debug 构建 DEBUG、Release 构建 INFO。
    pub const fn fallback() -> Self {
        if cfg!(debug_assertions) {
            Self::Debug
        } else {
            Self::Info
        }
    }
}

/// 闸门本体。初始值就是「跟随构建」，所以**设置还没读出来之前**的日志（CLI 解析、
/// 单实例判定）也按构建缺省过滤，不需要在 `main()` 的最开头抢一次初始化。
static LEVEL: AtomicU8 = AtomicU8::new(LogLevel::fallback().code());

/// 生效级别。
pub fn level() -> LogLevel {
    if LEVEL.load(Ordering::Relaxed) <= LogLevel::Debug.code() {
        LogLevel::Debug
    } else {
        LogLevel::Info
    }
}

/// 设置生效级别。只动这一格原子量：已在飞的那一帧不会被打断。
pub fn set_level(level: LogLevel) {
    LEVEL.store(level.code(), Ordering::Relaxed);
}

/// 这一条过不过闸门。纯判据抽出来是为了单测能一次钉住整张表（`enabled` 只做取值）。
pub fn passes(configured: LogLevel, wanted: LogLevel) -> bool {
    wanted.code() >= configured.code()
}

/// 当前配置下这条要不要打。
pub fn enabled(wanted: LogLevel) -> bool {
    passes(level(), wanted)
}

/// NFR-01 的统一前缀行文本：`perf: <label> <毫秒> ms[ <extra>]`。
///
/// 纯函数（不打日志、不读时钟）⇒ 单测能钉住格式；`extra` 只放计数与标签，
/// **绝不放查询原文**（需求书 FR-SYS-03 的隐私口径：日志不记用户输入）。
pub fn perf_line(label: &str, dur: Duration, extra: &str) -> String {
    let ms = dur.as_secs_f64() * 1000.0;
    if extra.is_empty() {
        format!("perf: {label} {ms:.2} ms")
    } else {
        format!("perf: {label} {ms:.2} ms {extra}")
    }
}

/// 打一条性能日志（详细档）。`extra` 为空的常见形态见 [`perf_line`]。
pub fn perf(label: &str, dur: Duration, extra: &str) {
    if enabled(LogLevel::Debug) {
        eprintln!("{}", perf_line(label, dur, extra));
    }
}

/// 详细档才打。
macro_rules! log_debug {
    ($($arg:tt)*) => {
        if $crate::logging::enabled($crate::logging::LogLevel::Debug) {
            eprintln!($($arg)*);
        }
    };
}

/// 标准档即打（`log_info!` 是**最常用的那一个**：既有 478 处全部先落在这一档）。
macro_rules! log_info {
    ($($arg:tt)*) => {
        if $crate::logging::enabled($crate::logging::LogLevel::Info) {
            eprintln!($($arg)*);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_table_is_a_threshold_table() {
        // 标准档：Info 过、Debug 不过。详细档：两条都过。
        assert!(passes(LogLevel::Info, LogLevel::Info));
        assert!(!passes(LogLevel::Info, LogLevel::Debug));
        assert!(passes(LogLevel::Debug, LogLevel::Debug));
        assert!(passes(LogLevel::Debug, LogLevel::Info));
    }

    #[test]
    fn set_level_round_trips_and_restores_previous() {
        let before = level();
        set_level(LogLevel::Info);
        assert_eq!(level(), LogLevel::Info);
        assert!(!enabled(LogLevel::Debug));
        set_level(LogLevel::Debug);
        assert_eq!(level(), LogLevel::Debug);
        assert!(enabled(LogLevel::Debug));
        set_level(before);
        assert_eq!(level(), before);
    }

    #[test]
    fn fallback_follows_the_build_kind() {
        // `cargo test` 默认是 debug 构建 ⇒ 缺省该是「详细」；这条断言同时钉住
        // `cfg!(debug_assertions)` 没有被写反（写反了在这里一眼可见）。
        assert_eq!(LogLevel::fallback(), LogLevel::Debug);
        assert_eq!(LogLevel::fallback().label(), "详细");
    }

    #[test]
    fn serde_shape_is_lowercase_for_the_settings_file() {
        let v = serde_json::to_value(LogLevel::Debug).unwrap();
        assert_eq!(v, serde_json::json!("debug"));
        let back: LogLevel = serde_json::from_value(serde_json::json!("info")).unwrap();
        assert_eq!(back, LogLevel::Info);
        // 落盘名与 serde 派生名必须同字：两处各写一遍正是漂移的入口。
        for lv in LogLevel::ALL {
            assert_eq!(serde_json::to_value(lv).unwrap(), serde_json::json!(lv.as_str()));
        }
    }

    #[test]
    fn perf_line_has_the_agreed_prefix_and_two_decimals() {
        let line = perf_line("search", Duration::from_micros(1234), "7 hits");
        assert_eq!(line, "perf: search 1.23 ms 7 hits");
        let bare = perf_line("startup", Duration::from_millis(2), "");
        assert_eq!(bare, "perf: startup 2.00 ms");
        assert!(!bare.ends_with(' '), "没有 extra 时不留尾空格（脚本要按空格切）");
    }
}

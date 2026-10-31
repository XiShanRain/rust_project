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
        crate::i18n::t(match self {
            Self::Debug => "log.level.detailed",
            Self::Info => "log.level.standard",
        })
    }

    /// 设置页说明（一句话：多出来的是哪些行）。
    pub fn hint(self) -> &'static str {
        crate::i18n::t(match self {
            Self::Debug => "log.level.detailed_hint",
            Self::Info => "log.level.standard_hint",
        })
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

/// 各标签的验收预算（毫秒）。`None` = 需求书与开发计划都没给数，不设门槛。
///
/// - `search`：NFR-01「查询响应 ≤ 16 ms（单帧）」。`perf: search` 只量计划那一小段，
///   是整帧的一部分 ⇒ 过了 16 ms 那一帧必然已经掉了；拿需求值当闸门只会漏报、不会误报。
/// - `startup`：开发计划 1.1「热启动 ≤ 1s」。
/// - `index`：没有需求依据（NFR-01 的 3 万条/P95 是文件搜索场景，不是建索引），留空。
pub fn perf_budget(label: &str) -> Option<f64> {
    match label {
        "search" => Some(16.0),
        "startup" => Some(1000.0),
        _ => None,
    }
}

/// 打一条性能日志（详细档）。`extra` 为空的常见形态见 [`perf_line`]。
pub fn perf(label: &str, dur: Duration, extra: &str) {
    if enabled(LogLevel::Debug) {
        eprintln!("{}", perf_line_with_budget(label, dur, extra));
    }
}

/// [`perf_line`] 的超预算版：超过 [`perf_budget`] 就在行尾追加「（超预算 N ms）」。
///
/// 尾巴加在**最后**：`target/g6-p95.ps1` 只按 `^perf: search <数> ms` 取样本，
/// 前面的前缀与数字一个字符都不动。
pub fn perf_line_with_budget(label: &str, dur: Duration, extra: &str) -> String {
    let line = perf_line(label, dur, extra);
    let ms = dur.as_secs_f64() * 1000.0;
    match perf_budget(label) {
        Some(budget) if ms > budget => format!("{line}（超预算 {:.2} ms）", ms - budget),
        _ => line,
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

    #[test]
    fn perf_budget_covers_only_the_two_labels_with_a_requirement() {
        assert_eq!(perf_budget("search"), Some(16.0), "NFR-01 单帧 16 ms");
        assert_eq!(perf_budget("startup"), Some(1000.0), "开发计划 1.1 热启动 ≤1s");
        assert_eq!(perf_budget("index"), None, "索引没有需求门槛，别自己造一个");
    }

    #[test]
    fn over_budget_line_keeps_the_prefix_and_says_by_how_much() {
        let ok = perf_line_with_budget("search", Duration::from_micros(12_340), "7 hits");
        assert_eq!(ok, "perf: search 12.34 ms 7 hits");
        assert!(!ok.contains("超预算"));

        let slow = perf_line_with_budget("search", Duration::from_millis(20), "");
        assert_eq!(slow, "perf: search 20.00 ms（超预算 4.00 ms）");
        assert!(
            slow.starts_with("perf: search 20.00 ms"),
            "尾巴不许动前缀——`target/g6-p95.ps1` 按 `^perf: search <数> ms` 取样"
        );

        let edge = perf_line_with_budget("search", Duration::from_millis(16), "");
        assert_eq!(edge, "perf: search 16.00 ms", "正好等于预算不算超");

        let unbounded = perf_line_with_budget("index", Duration::from_millis(900), "");
        assert_eq!(unbounded, "perf: index 900.00 ms", "没有预算的标签不加尾巴");
    }
}

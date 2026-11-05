//! 批次 G5（FR-SYS-03）：日志级别闸门 + 性能日志统一前缀。
//!
//! 本程序不引 `tracing`（取向：零新增依赖面，见需求书 FR-SYS-03 的 2026-10-19 注记），
//! 日志仍是 stderr —— GUI 子系统下由 `redirect_stderr_if_orphan()` 牵到 `launcher.log`。
//! 这里只加一层**级别闸门**：全部调用点过 `log_debug!` / `log_info!`，`eprintln!` 只活在
//! 两个宏体、[`perf`] 与 [`install_panic_hook`] 那四处（`src/` 其余位置 `grep -c 'eprintln!('`
//! 归零就是这条纪律的判据）。
//! 批次 LOGTS 起，行尾送达戳只由 [`stamped_line`] 一个人加：正文由调用点写，戳由漏斗加，
//! 中间没有人能把戳漏掉。批次 PNCS 把 §一百零四 庚 登记的两处「不覆盖」补了进来
//! （[`install_panic_hook`] 的摘要行、`application.rs` 的 `log_absent_note`），所以判据从
//! 「三个漏斗」换成「`stamped_line` 的调用点清单」：它有几处，全仓就长几处戳。
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

/// 一条日志的落笔形态：正文 + 行尾送达戳 `‹正文› [HH:MM:SS.mmm]`（批次 LOGTS）。
///
/// 戳放**行尾**不是随手选的：两处判据都钉在行首——`launcher.log` 首行靠
/// `strip_prefix(LOG_DATE_TAG)` 取归档名（`application.rs` 的 `log_content_date`），
/// `perf:` 那一族脚本按 `^perf: <label> <数> ms` 取样。放行首就得连这两条一起改，
/// 放行尾则两条一个字都不动，而「这一行是什么时候落盘的」照样拿得到。
///
/// 纯函数（不读时钟）⇒ 单测能钉住组成律；时钟只活在 [`stamp_now`] 一处。
pub fn stamped_line(body: &str, clock: &str) -> String {
    format!("{body} [{clock}]")
}

/// 现在的本地时刻戳。走 `application.rs` 那条唯一取时间的路（`GetLocalTime`），
/// 不在这里另起一条 UTC 或 `SystemTime` 的换算。
pub fn stamp_now() -> String {
    crate::source::application::local_now_clock_ms()
}

/// 打一条性能日志（详细档）。`extra` 为空的常见形态见 [`perf_line`]。
pub fn perf(label: &str, dur: Duration, extra: &str) {
    if enabled(LogLevel::Debug) {
        let clock = stamp_now();
        eprintln!(
            "{}",
            stamped_line(&perf_line_with_budget(label, dur, extra), &clock)
        );
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

/// panic 摘要行的形态（批次 PNCS）：`panic：‹载荷›｜‹文件:行:列› [HH:MM:SS.mmm]`。
///
/// 走 [`stamped_line`] 同一个组成律 ⇒ 「全仓的戳只有一个人加」这条不因新漏斗而破。载荷先压平
/// （[`flatten_newlines`]）：一条 panic 只占日志一行，否则戳会掉到载荷最后一行、`[戳]` 不再在行尾。
///
/// 纯函数（不读时钟、不装钩子）⇒ 单测能一次钉住位置缺失的兜底与压平律。
pub fn panic_report_line(message: &str, location: &str, clock: &str) -> String {
    stamped_line(
        &format!("panic：{}｜{}", flatten_newlines(message), location),
        clock,
    )
}

/// 换行压成字面量 `\n`；先归一 `\r\n`，否则 Windows 载荷会在行中留一个裸回车。
fn flatten_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\\n")
}

/// panic 载荷转文本：`String` / `&str` 两种形态各一条，剩下的一律给固定标注。
///
/// 这里不试图把非字符串载荷打印出来（`dyn Any` 没有 `Debug`，拿 `TypeId` 的哈希数字填进日志
/// 也没有判据价值）——紧跟其后的默认钩子多行块本来就以 `{:?}` 打印同一份载荷，细节一个字没丢。
fn payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "<非字符串载荷>".to_string()
    }
}

/// 装 panic 摘要钩子（批次 PNCS：结掉 §一百零四 庚①「panic 那几行没有戳」）。
///
/// 调用位置有硬约束：必须在 `redirect_stderr_if_orphan()` **之后**——换句柄只管之后的写，
/// 早于它装钩子会让这条摘要行流向一个没人接的控制台。
///
/// 链式而不是替换：`take_hook` 先取走默认钩子存下来，本钩子写完带戳摘要行后把同一份
/// `PanicHookInfo` 原样交回。默认那块多行输出（`panicked at …` 与 `RUST_BACKTRACE` 的栈）
/// 一个字不少 ⇒ 现有按 `panicked at` 取样的取证通道不受影响，净增的只有「这一炸发生在
/// 几点几分几秒几毫秒」这一行。
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = match info.location() {
            Some(l) => format!("{}:{}:{}", l.file(), l.line(), l.column()),
            None => "位置未知".to_string(),
        };
        eprintln!(
            "{}",
            panic_report_line(&payload_text(info.payload()), &location, &stamp_now())
        );
        default(info);
    }));
}

/// 详细档才打。
macro_rules! log_debug {
    ($($arg:tt)*) => {
        if $crate::logging::enabled($crate::logging::LogLevel::Debug) {
            let __clock = $crate::logging::stamp_now();
            eprintln!(
                "{}",
                $crate::logging::stamped_line(&format!($($arg)*), &__clock)
            );
        }
    };
}

/// 标准档即打（`log_info!` 是**最常用的那一个**：既有 478 处全部先落在这一档）。
macro_rules! log_info {
    ($($arg:tt)*) => {
        if $crate::logging::enabled($crate::logging::LogLevel::Info) {
            let __clock = $crate::logging::stamp_now();
            eprintln!(
                "{}",
                $crate::logging::stamped_line(&format!($($arg)*), &__clock)
            );
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
        // 批次 PRIC：原先写死 `Debug`＋"详细"（前提句是「`cargo test` 默认 debug 构建」），`--release` 一跑就红。
        let (want, label) = if cfg!(debug_assertions) { (LogLevel::Debug, "详细") } else { (LogLevel::Info, "标准") };
        assert_eq!(LogLevel::fallback(), want, "缺省级别跟着构建形态走");
        assert_eq!(LogLevel::fallback().label(), label, "两头各钉一个字面量，标签换边也红");
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
    fn stamp_now_reads_a_clock_not_a_date() {
        // 真读一次系统时钟：形状恒为 `HH:MM:SS.mmm`＝12 字符，日期一个字符都不许进来。
        let s = stamp_now();
        let b = s.as_bytes();
        assert_eq!(b.len(), 12, "戳恒宽 12：{s:?}");
        for i in [2, 5] {
            assert_eq!(b[i], b':', "第 {i} 格该是冒号：{s:?}");
        }
        assert_eq!(b[8], b'.', "秒与毫秒之间一个点：{s:?}");
        for i in [0, 1, 3, 4, 6, 7, 9, 10, 11] {
            assert!(b[i].is_ascii_digit(), "第 {i} 格该是数字：{s:?}");
        }
        let hour: u8 = s[0..2].parse().unwrap();
        let min: u8 = s[3..5].parse().unwrap();
        let sec: u8 = s[6..8].parse().unwrap();
        let ms: u16 = s[9..12].parse().unwrap();
        assert!(hour < 24 && min < 60 && sec < 60 && ms < 1000, "越界的时钟：{s:?}");
        assert_eq!(s.find('-'), None, "戳里不许有日期分隔符");
    }

    #[test]
    fn stamped_line_moves_the_body_byte_for_byte_and_puts_the_clock_last() {
        let body = "perf: search 20.00 ms（超预算 4.00 ms）";
        let line = stamped_line(body, "07:08:09.010");
        assert_eq!(line, "perf: search 20.00 ms（超预算 4.00 ms） [07:08:09.010]");
        assert!(line.starts_with(body), "正文一字不动、也不许被挪出行首");
        assert!(line.ends_with(']'), "戳是这一行的最后一段");
        // 行尾才是戳：从行尾倒着剥，剥掉的正好是 ` [HH:MM:SS.mmm]`＝15 字符。
        assert_eq!(line.len(), body.len() + 15);
    }

    #[test]
    fn the_stamp_survives_both_perf_and_gate_paths() {
        // 三条漏斗共用同一个组成函数：`perf()` 的正文是 `perf_line_with_budget` 给的，
        // 加了戳之后行首那条取样锚仍然完整（`^perf: <label> <数> ms`）。
        let clock = "23:59:59.999";
        let line = stamped_line(&perf_line_with_budget("search", Duration::from_millis(20), ""), clock);
        assert!(line.starts_with("perf: search 20.00 ms"), "取样锚还在行首");
        assert!(line.ends_with(" [23:59:59.999]"), "戳在行尾");
        let plain = stamped_line("文件索引：本轮建成 8 条", clock);
        assert!(plain.starts_with("文件索引："), "回执行的行首也没被挪走");
    }

    #[test]
    fn a_line_stamp_is_cheap_enough_for_per_frame_logs() {
        // 上限闸 + 下界闸（成规：上限型断言要两道闸）。这道成本账测的是「一次取时刻 + 一次
        // format + 一次堆分配」；`black_box` 负责证明循环真跑了、没被优化成一趟空手。
        // 上界放到 20 µs/条：真机若慢到这个量级，逐帧日志就会开始改变它自己要观测的时序。
        const N: u32 = 50_000;
        let t = std::time::Instant::now();
        let mut sink = String::new();
        for _ in 0..N {
            sink = std::hint::black_box(stamp_now());
        }
        let total = t.elapsed().as_secs_f64() * 1e6; // µs
        let per = total / N as f64;
        std::hint::black_box(&sink);
        assert!(per > 0.0, "这一趟一个纳秒都没花 ⇒ 循环被优化掉了，数不可信");
        assert!(per < 20.0, "实测 {per:.3} µs/条，已经贵到会改写被观测的时序");
        println!("line stamp 实测：{per:.3} µs/条（{N} 次，合计 {total:.0} µs｜构建形态自报 debug_assertions={} rel_opt={} dev_opt={}）", cfg!(debug_assertions), option_env!("CARGO_PROFILE_RELEASE_OPT_LEVEL").unwrap_or("-"), option_env!("CARGO_PROFILE_DEV_OPT_LEVEL").unwrap_or("-"));
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

    /// 批次 PNCS 的形状闸：panic 摘要行走 [`stamped_line`] 同一个组成律——行首 `panic：`、
    /// 载荷之后接位置、戳收尾。写死一整句字面量，三个部件换任何一边都红。
    #[test]
    fn a_panic_summary_line_is_body_then_location_then_the_clock() {
        let line = panic_report_line("attempt to subtract with overflow", "src/main.rs:19535:5", "07:08:09.010");
        assert_eq!(
            line,
            "panic：attempt to subtract with overflow｜src/main.rs:19535:5 [07:08:09.010]"
        );
        assert!(line.starts_with("panic："), "行首要一眼认出这是一次炸，不是一行普通日志");
        let body = "panic：attempt to subtract with overflow｜src/main.rs:19535:5";
        assert_eq!(line.len(), body.len() + 15, "加戳只多 15 字节，正文一个字符没动");
    }

    /// 载荷三种形态各有去处。第三种不硬凑文本：`dyn Any` 没有 `Debug`，而紧跟其后的默认钩子块
    /// 本来就以 `{:?}` 打印同一份载荷——这里塞一个 `TypeId` 哈希数字只会多一条读不懂的行。
    #[test]
    fn payload_text_takes_string_and_str_and_labels_everything_else() {
        let owned: String = " owned ".into();
        assert_eq!(payload_text(&owned), " owned ", "String 载荷原样");
        let lit: &str = "lit";
        assert_eq!(payload_text(&lit), "lit", "&str 载荷（panic!(\"字面量\") 走这一条）");
        assert_eq!(payload_text(&7u32), "<非字符串载荷>", "非字符串给固定标注，不给猜的");
    }

    /// 压平律：一条 panic 只占日志一行，否则 `[戳]` 会掉到载荷最后一行、`^` 取样锚也不再是行首。
    #[test]
    fn a_multiline_payload_collapses_so_the_clock_stays_at_the_line_end() {
        let line = panic_report_line("甲\n乙\r\n丙", "src/main.rs:1:2", "23:59:59.999");
        assert_eq!(line, "panic：甲\\n乙\\n丙｜src/main.rs:1:2 [23:59:59.999]");
        assert!(
            !line.contains('\n') && !line.contains('\r'),
            "裸换行/回车一个都不许留下（CRLF 先归一再压，Windows 载荷不在行中留裸回车）"
        );
        assert!(line.ends_with(" [23:59:59.999]"), "压平后戳仍在行尾");
    }

    /// 位置缺失不是「没有位置这一段」：给一个字面量，行首判据与戳的位置都不变。
    #[test]
    fn a_panic_without_a_location_still_makes_one_line() {
        assert_eq!(
            panic_report_line("x", "位置未知", "00:00:00.000"),
            "panic：x｜位置未知 [00:00:00.000]"
        );
    }

    /// 端到端那一案：**它自己会炸**，只作为子进程里的那一案跑（父案见下一格）。
    /// 装钩子用 `take_hook` 取走的是 libtest 自己的钩子 ⇒ 这一案同时验了链式没吞掉原生块。
    #[test]
    #[ignore]
    fn the_child_panics_on_purpose() {
        install_panic_hook();
        panic!("PNCS-自炸 {}\n第二行", 42);
    }

    /// 批次 PNCS 的端到端一条：摘要行真打出来了、戳是活的时钟，且默认那块 `panicked at` 一个字没少。
    ///
    /// 子进程是**本测试二进制自己**（`current_exe`），不是产品 exe ⇒ 零窗口、零产品进程。
    /// 两条 stdout/stderr 合并后判：libtest 在 `--nocapture` 下把 panic 打到 stderr，但「钩子链没断」
    /// 这一句的判据不依赖它落哪条流；「戳真进了 `launcher.log`」属另一件事——那是 stderr 被
    /// `redirect_stderr_if_orphan()` 接管之后的口径，本条不证（真机待验）。
    #[test]
    fn the_panic_hook_emits_one_stamped_line_and_keeps_the_default_block() {
        let exe = std::env::current_exe().expect("拿不到当前测试二进制的路径");
        let out = std::process::Command::new(&exe)
            .args([
                "--exact",
                "logging::tests::the_child_panics_on_purpose",
                "--ignored",
                "--nocapture",
            ])
            .env("RUST_BACKTRACE", "0")
            .output()
            .expect("子进程（测试二进制自身）起不来");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        // 先钉「那一案真的跑了」：否则下面任何一条缺失都可能只是过滤器没命中（没算 ≠ 算了没说）。
        assert!(text.contains("running 1 test"), "子进程一个测试都没跑：{text}");
        assert!(
            text.contains("the_child_panics_on_purpose"),
            "过滤器没命中那一案，后面三条判据不成立：{text}"
        );
        assert!(
            !out.status.success(),
            "自炸案却成功退出了 ⇒ 它没炸成，下面的行当然也不该有：{text}"
        );

        let hits: Vec<&str> = text.lines().filter(|l| l.starts_with("panic：")).collect();
        assert_eq!(hits.len(), 1, "带戳摘要行该恰好一条，实测 {hits:?}");
        let line = hits[0];
        assert!(
            line.contains("PNCS-自炸 42\\n第二行"),
            "载荷没压平或丢了：{line}"
        );
        assert!(line.contains("logging.rs"), "位置段没带上源文件名：{line}");
        assert!(
            text.contains("panicked at"),
            "默认钩子那块没了 ⇒ take_hook 的链式调用断了：{text}"
        );

        // 戳是当场取的时钟，不是字面量：形状同 `stamp_now`（宽 12、两处冒号、一个点）。
        let clock = line
            .rsplit_once(" [")
            .map(|(_, c)| c.strip_suffix(']').unwrap_or(c))
            .expect("行尾没有 ` [戳]` 这一段");
        let b = clock.as_bytes();
        assert_eq!(b.len(), 12, "戳宽 12：{clock:?}");
        assert!(b[2] == b':' && b[5] == b':' && b[8] == b'.', "戳的分隔符位置错了：{clock:?}");
        assert!(
            clock.chars().enumerate().all(|(i, c)| matches!(i, 2 | 5 | 8) || c.is_ascii_digit()),
            "戳里除两个冒号一个点外必须全是数字：{clock:?}"
        );
        println!("panic 摘要行实测：{line}");
    }

    // ===================== 批次 WRIT：写那一条腿到底多少钱 =====================
    //
    // 历批把这一格一直登记成「做不了」（§一百零六 癸①原文：「工装要量它就得真往盘上写 50,000 行
    // （还会污染日志目录），本批不写」）。本批把那两个前提各自换掉：**写的是 `target/writ/` 下的
    // 沙箱文件**（数据目录里那份 `launcher.log` 一个字节都不碰），**跑的是测试二进制自己**
    // （`current_exe`，零窗口、零产品进程）。
    //
    // 句柄形状对着产品抄：`redirect_stderr_if_orphan()` 走 `OpenOptions::append(true)` 再把
    // `file.as_raw_handle()` 交给 `SetStdHandle(STD_ERROR_HANDLE, …)` ⇒ 之后每一句 `eprintln!`
    // 都是「无缓冲地往一条 append 句柄 WriteFile」。父案用 `Stdio::from(同样形状的 File)` 接管
    // 子进程的 stderr，复现的就是同一条路径：同一个句柄种类、同样的 append 访问、同样不过缓冲区。
    //
    // 口径限制（先写在这儿，免得读数被读大）：量到的是**到系统文件缓存**的成本，不是耐久介质的
    // 成本——产品和本工装都不调 `FlushFileBuffers`，`WriteFile` 返回只证明字节交给了内核。

    /// 七条腿的名字。父案与 `target/writ/` 工装读同一张表，两边不许各写一份。
    const WRIT_LEGS: [&str; 7] = ["W0", "W1", "W1b", "W2", "W3", "W4", "W5"];

    /// 一条腿的循环体付什么（判据全在注释里，读数只在报告行里）：
    /// - `W0` 组成腿：`stamped_line(&format!(…), &stamp_now())`，一个字不写盘。
    /// - `W1` 写腿·单次：`write_all(整行含换行)` ⇒ 每行一次 WriteFile，循环内不组行。
    /// - `W1b` 写腿·两次：正文一次 + 换行一次 ⇒ 与 W1 只差多一次 WriteFile，与 W2 只差没有 `format_args`。
    /// - `W2` 写腿·宏形态：`eprintln!("{}", 预合成整行)` ⇒ 走产品那句的打印分发。
    /// - `W3` 全腿：真 `log_info!`（闸门 + 组行 + 盖戳 + 打印）——产品每行实际付的就是这个。
    /// - `W4` 批处理对照：循环内只 `push_str`，循环外一次 `write_all` 把 n 行一起交出去。
    /// - `W5` 闸门抑制档：Info 档下的 `log_debug!` ⇒ 一条都不该落盘，付的是「被挡」那一小段。
    ///
    /// 首尾两条哨兵走真漏斗（`log_info!`）：文件里「有这两条」＝句柄通了、这一案真跑了；
    /// 「只有这两条」＝这条腿一个字没写（W0/W5 的判据就靠它，否则「挡住了」与「没跑」同形）。
    #[test]
    #[ignore]
    fn writ_child_leg() {
        use std::io::Write;
        let Ok(leg) = std::env::var("WRIT_LEG") else {
            println!("WRIT-SKIP｜没给 WRIT_LEG，子进程一个字都没干");
            return;
        };
        assert!(WRIT_LEGS.contains(&leg.as_str()), "未知腿名：{leg}");
        let n: usize = std::env::var("WRIT_LINES").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let warm: usize = std::env::var("WRIT_WARM").ok().and_then(|v| v.parse().ok()).unwrap_or(0);

        // 档位写死，不靠构建缺省：W3 要 Info 档下的 `log_info!`，W5 要 Debug 行被闸门挡下来。
        set_level(LogLevel::Info);
        log_info!("WRIT-哨兵 {leg} 起");

        // 预合成的整行：W1 / W1b / W2 / W4 落盘字节完全一致（宽度可逐行核，才算「只量写」）。
        let pre_line = stamped_line("文件索引：本轮建成 512 条", &stamp_now());
        let pre_full = format!("{pre_line}\n");

        // `buf` 由调用方拿着、按 `&mut` 交给循环体：W4 要在计时段结束后自己把缓冲区写出去，
        // 关进闭包里就拿不回来了。其余腿忽略这个参数。
        let mut buf = String::new();
        let mut sink: Box<dyn FnMut(usize, &mut String)> = match leg.as_str() {
            "W0" => Box::new(|i: usize, _: &mut String| {
                std::hint::black_box(stamped_line(&format!("文件索引：本轮建成 {i} 条"), &stamp_now()));
            }),
            "W1" => Box::new(|_: usize, _: &mut String| {
                let _ = std::io::stderr().write_all(pre_full.as_bytes());
            }),
            "W1b" => Box::new(|_: usize, _: &mut String| {
                let mut e = std::io::stderr();
                let _ = e.write_all(pre_line.as_bytes());
                let _ = e.write_all(b"\n");
            }),
            "W2" => Box::new(|_: usize, _: &mut String| {
                eprintln!("{pre_line}");
            }),
            "W3" => Box::new(|i: usize, _: &mut String| {
                log_info!("文件索引：本轮建成 {i} 条");
            }),
            "W4" => Box::new(|_: usize, b: &mut String| b.push_str(&pre_full)),
            "W5" => Box::new(|i: usize, _: &mut String| {
                log_debug!("文件索引：本轮建成 {i} 条");
            }),
            other => unreachable!("未知腿名：{other}"),
        };

        // 预热跑同一份循环体，读数丢弃、字节照落（行数判据把 warm 一起算进去）。
        let _ = loop_us(warm, &mut buf, &mut sink);
        if leg == "W4" {
            // 预热那一段整块交出去（不计入读数），缓冲区清空后计时段才只剩「一次写 n 行」。
            let _ = std::io::stderr().write_all(buf.as_bytes());
            buf.clear();
        }
        let loop_us_ = loop_us(n, &mut buf, &mut sink);

        // W4 的尾巴：一次 WriteFile 把 n 行全交出去。它和 `loop_us` 分开报，合不合算由工装算，
        // 不在子进程里替读数做决定。
        let mut extra_us = 0.0f64;
        if leg == "W4" {
            let t = std::time::Instant::now();
            let _ = std::io::stderr().write_all(buf.as_bytes());
            extra_us = t.elapsed().as_secs_f64() * 1e6;
        }

        log_info!("WRIT-哨兵 {leg} 止");
        let per = if n == 0 { "NA".to_string() } else { format!("{:.4}", loop_us_ / n as f64) };
        println!(
            "WRIT-REPORT｜leg={leg}｜n={n}｜warm={warm}｜level={}｜loop_us={loop_us_:.3}｜us_per_line={per}｜extra_us={extra_us:.3}",
            level().as_str()
        );
    }

    /// 跑 `iters` 次循环体，返回总微秒。单独拉出来是为了让「预热」与「计时」两趟用同一个函数、
    /// 同一个闭包——两趟代码不同，读数就没有意义。
    fn loop_us(iters: usize, buf: &mut String, f: &mut dyn FnMut(usize, &mut String)) -> f64 {
        let t = std::time::Instant::now();
        for i in 0..iters {
            f(i, buf);
        }
        t.elapsed().as_secs_f64() * 1e6
    }

    /// 父案：工装的**自检片**（小 n，进 `cargo test` 的常规绿检）。它证三件事——
    /// ① 子进程 stderr 真被接到 `target/writ/` 下的文件（哨兵行在、且产品在位）；
    /// ② 落盘行数与「哨兵 2 + warm + n」逐条相等 ⇒ 写这条腿没被静默吞掉，也没多写；
    /// ③ W0 与 W5 只留 2 行哨兵 ⇒ 「这条腿不写盘」是量出来的，不是假设的。
    ///
    /// 零窗口、零产品进程：子进程是本测试二进制自己，文件在构建目录下。
    #[test]
    fn the_write_leg_rig_lands_exactly_the_lines_it_claims() {
        const N: usize = 400;
        const WARM: usize = 3;
        let dir = std::path::Path::new("target/writ");
        std::fs::create_dir_all(dir).expect("target/writ 建不出来");

        // (腿, 这条腿的循环体写不写盘)。不写的腿（W0 只组行、W5 被闸门挡）连预热都不落盘，
        // 期望只剩 2 行哨兵——「2」在这里是判据不是空转，因为同案里哨兵必须 2 行在场。
        let plan = [("W0", false), ("W1", true), ("W3", true), ("W5", false)];
        let mut readings = Vec::new();
        for (leg, writes) in plan {
            let expect_body = if writes { WARM + N } else { 0 };
            let path = dir.join(format!("rig-{leg}-{}.log", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .expect("沙箱日志文件打不开");
            let exe = std::env::current_exe().expect("拿不到当前测试二进制的路径");
            let out = std::process::Command::new(&exe)
                .args(["--exact", "logging::tests::writ_child_leg", "--ignored", "--nocapture"])
                .env("WRIT_LEG", leg)
                .env("WRIT_LINES", N.to_string())
                .env("WRIT_WARM", WARM.to_string())
                .env("RUST_BACKTRACE", "0")
                .stderr(std::process::Stdio::from(file))
                .output()
                .expect("子进程（测试二进制自身）起不来");
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            // 先钉「那一案真跑了」：报告行缺失时下面所有判据都不成立（没算 ≠ 算了没说）。
            assert!(text.contains("running 1 test"), "{leg}：子进程一个测试都没跑：{text}");
            assert!(out.status.success(), "{leg}：子进程退出码非 0：{text}");
            let report = text
                .lines()
                .find(|l| l.starts_with("WRIT-REPORT"))
                .unwrap_or_else(|| panic!("{leg}：没有 WRIT-REPORT 行，工装没跑到读数：{text}"));
            let fields: std::collections::HashMap<_, _> = report
                .split('｜')
                .filter_map(|kv| kv.split_once('='))
                .collect();
            assert_eq!(fields.get("leg").copied(), Some(leg), "{leg}：报告行报的是别的腿");
            assert_eq!(fields.get("level").copied(), Some("info"), "{leg}：档位不是 Info，W3/W5 的判据不成立");
            assert_ne!(fields.get("us_per_line").copied(), Some("NA"), "{leg}：n=0 却跑进了这一案 ⇒ 环境变量没传下去");
            let per: f64 = fields["us_per_line"].parse().expect("us_per_line 不是数");
            assert!(per > 0.0, "{leg}：每条 0 µs ⇒ 循环被优化掉了，读数不可信（实测 {per}）");

            let lines: Vec<String> = std::fs::read_to_string(&path)
                .expect("沙箱文件读不回来")
                .lines()
                .map(|l| l.to_string())
                .collect();
            let sentinel = lines.iter().filter(|l| l.contains("WRIT-哨兵")).count();
            assert_eq!(sentinel, 2, "{leg}：哨兵行该恰好 2 条（起 + 止），实测 {sentinel} ⇒ 句柄没通或漏斗没跑");
            assert_eq!(
                lines.len(),
                2 + expect_body,
                "{leg}：落盘行数不对（要 2 哨兵 + {expect_body} 条循环体落的行）"
            );
            readings.push(format!("{leg}: {per:.4} us/条, 行数 {}", lines.len()));

            // 只量写的腿：正文行逐字节相同（循环里没组行）；全腿：首尾两行内容不同（循环真在动 i）。
            let body_lines: Vec<&String> = lines
                .iter()
                .filter(|l| !l.contains("WRIT-哨兵"))
                .collect();
            if leg == "W1" {
                assert_eq!(body_lines.iter().collect::<std::collections::HashSet<_>>().len(), 1, "W1：预合成行不该逐条不同");
            }
            if leg == "W3" {
                assert!(body_lines[0].contains("本轮建成 0 条"), "W3：第一条正文不是 i=0：{}", body_lines[0]);
                assert!(body_lines[WARM].contains("本轮建成 0 条"), "W3：预热段之后第一条正文不是 i=0");
                assert!(body_lines[WARM + N - 1].contains("本轮建成 399 条"), "W3：最后一条正文不是 i=399 ⇒ 中间丢了行");
            }
            let _ = std::fs::remove_file(&path);
        }
        println!("写腿工装自检：{}", readings.join(" | "));
    }
}

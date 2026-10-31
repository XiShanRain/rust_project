//! FR-SYS-06（批次 D8）：使用统计桶——按功能计数、**只记次数不记内容**。
//!
//! 存 `usage.json`，形态只有两层：`日期 → 桶名 → 次数`。这里没有、也不许有
//! 任何用户原文（查询词、条目名、路径、窗口标题）——需求书 FR-SYS-03 的隐私口径
//! 对这份文件同样成立，单测 `json_carries_only_dates_buckets_and_counts` 钉住它。
//!
//! **与 `LaunchItem::use_count` 的分工**：那个字段是"这一条用过几次"，只喂排序、
//! 从不给用户看；本模块是"哪一类功能用过几次 + 哪天用过"，只给设置页看。
//! 两者来源相同（`mark_used` 与几个执行点），但**不互相推导**：条目被删掉之后
//! 它贡献过的次数仍留在本文件里，那才是"使用统计"该有的口径。
//!
//! 保留 400 天（需求书 §文件表）：每次记账顺手裁一次，裁的是"今天往前走 400 天"，
//! 不是"留最新 400 条"——隔一年再打开程序时，后者会把三年前的记录当"最近 400 天"。
//!
//! 日期换算不引 chrono：本仓至今只有 `GetLocalTime` 这一条取本地历日的路，
//! 而"某天往前 Nth 天"用 Howard Hinnant 的 `days_from_civil` / `civil_from_days`
//! 两个十几行的纯函数就够，且可以原地单测（跨闰日、跨年都钉在测试里）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// `usage.json` 的字段形态版本。目前只有 1；将来加字段走与 `settings.json` 同一套
/// "缺键就是旧版 + 一次升一版"的路子，别把版本号塞进桶名里。
pub const USAGE_SCHEMA: u32 = 1;

/// 保留天数（需求书 §文件表：`usage.json` 使用统计，保留 400 天）。
/// 含今天 ⇒ 最早留到"今天 - 399"。
pub const KEEP_DAYS: i64 = 400;

/// 功能桶。`as_str` 的结果会写进 `usage.json` ⇒ **只增不改**：
/// 改一个名字等于把用户已有的那份次数清零（旧键变成"未知桶"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    /// 清单项（应用 / 命令 / 网址 / 文件夹 / 文件）。
    Item,
    /// 启动器自己的系统命令（`internal:`，FR-SRC-17）。
    Internal,
    /// 计算器结果复制（FR-SRC-10）。
    Calc,
    /// 网页搜索（FR-SRC-11）。
    Web,
    /// 自定义命令源（FR-SRC-05，带 `{query}` 的那一类）。
    Custom,
    /// 文件搜索 / 目录浏览行打开（FR-SRC-14）。
    File,
    /// `>` 终端命令（FR-SRC-16）。
    Terminal,
}

impl Bucket {
    /// 展示与校验共用的顺序：设置页那三行占比表按这个次序排。
    pub const ALL: [Bucket; 7] = [
        Bucket::Item,
        Bucket::Internal,
        Bucket::Calc,
        Bucket::Web,
        Bucket::Custom,
        Bucket::File,
        Bucket::Terminal,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Bucket::Item => "item",
            Bucket::Internal => "internal",
            Bucket::Calc => "calc",
            Bucket::Web => "web",
            Bucket::Custom => "custom",
            Bucket::File => "file",
            Bucket::Terminal => "terminal",
        }
    }

    pub fn label(self) -> &'static str {
        crate::i18n::t(match self {
            Bucket::Item => "usage.bucket.item",
            Bucket::Internal => "usage.bucket.internal",
            Bucket::Calc => "usage.bucket.calc",
            Bucket::Web => "usage.bucket.web",
            Bucket::Custom => "usage.bucket.custom",
            Bucket::File => "usage.bucket.file",
            Bucket::Terminal => "usage.bucket.terminal",
        })
    }
}

/// 一天的计数：桶名 → 次数。键是 [`Bucket::as_str`] 的字符串，**不是枚举值**——
/// 将来本程序认不出的桶（手改配置、或新版本写下的）读进来只是"不展示"，
/// 不会被整份判坏，也不会在回写时丢掉。
pub type DayBuckets = BTreeMap<String, u32>;

/// 一次成功执行该记进哪个桶。
///
/// `custom`（带 `{query}` 的自定义命令，FR-SRC-05）对「清单项」是**替代**关系而不是叠加：
/// 批次 D8 时清单项与自定义各记了一次，同一次执行在统计里 +2（2026-10-30 去重）。
pub fn bucket_for(internal: bool, custom: bool) -> Bucket {
    if custom {
        Bucket::Custom
    } else if internal {
        Bucket::Internal
    } else {
        Bucket::Item
    }
}

/// 整份统计。`days` 的键是本地日期 `YYYY-MM-DD`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageStats {
    /// 缺键＝老文件（本文件是批次 D8 才有的，所以实际不会出现；留着是给将来升版用）。
    #[serde(default = "default_schema")]
    pub schema_version: u32,
    #[serde(default)]
    pub days: BTreeMap<String, DayBuckets>,
}

fn default_schema() -> u32 {
    USAGE_SCHEMA
}

impl Default for UsageStats {
    fn default() -> Self {
        Self {
            schema_version: USAGE_SCHEMA,
            days: BTreeMap::new(),
        }
    }
}

impl UsageStats {
    /// 记一次。返回该桶当天的累计次数（调用方只拿来打日志，别用它判"变了没有"——
    /// 每次调用都算变了，落盘与否由调用方的写入门决定）。
    pub fn record(&mut self, day: &str, bucket: Bucket) -> u32 {
        let slot = self
            .days
            .entry(day.to_string())
            .or_default()
            .entry(bucket.as_str().to_string())
            .or_insert(0);
        *slot = slot.saturating_add(1);
        *slot
    }

    /// 裁掉 `today` 往前第 `keep` 天（不含）之前的记录；返回删掉了几天的条目。
    /// `keep` 表达"含今天一共保留多少天"，所以 400 天 = 保留 `today-399`。
    /// 认不出日期的键（手改坏了）一并删掉：留着它既算不进任何窗口，也没法判断该不该留。
    pub fn trim(&mut self, today: &str, keep: i64) -> usize {
        let Some(today_days) = parse_days(today) else {
            // 今天都认不出来就一天都不删——宁可多留，不可误删（这一条与日志归档
            // "锚读不出来就不归档，绝不猜一个填上去"同一取向）。
            return 0;
        };
        let keep = keep.max(1);
        let oldest = today_days - (keep - 1);
        let before = self.days.len();
        self.days.retain(|k, _| match parse_days(k) {
            Some(d) => d >= oldest,
            None => false,
        });
        before - self.days.len()
    }

    /// 所有桶、所有天的总次数。未知桶也算——合计是"这套功能一共被用了多少次"，
    /// 不因为本程序认不出某个桶名就把用户那份数据从合计里抹掉。
    pub fn total(&self) -> u64 {
        self.days
            .values()
            .flat_map(|d| d.values())
            .map(|&n| u64::from(n))
            .sum()
    }

    pub fn bucket_total(&self, bucket: Bucket) -> u64 {
        let key = bucket.as_str();
        self.days
            .values()
            .filter_map(|d| d.get(key))
            .map(|&n| u64::from(n))
            .sum()
    }

    /// 认不出的桶名的次数合计（设置页只在它非零时多显示一行「其他」）。
    pub fn unknown_total(&self) -> u64 {
        let known: Vec<&str> = Bucket::ALL.iter().map(|b| b.as_str()).collect();
        self.days
            .values()
            .flat_map(|d| d.iter())
            .filter(|(k, _)| !known.iter().any(|known_key| known_key == &k.as_str()))
            .map(|(_, &n)| u64::from(n))
            .sum()
    }

    pub fn day_total(&self, day: &str) -> u64 {
        self.days
            .get(day)
            .map(|d| d.values().map(|&n| u64::from(n)).sum())
            .unwrap_or(0)
    }

    /// 有记录的**天数**（键在表里就算，哪怕计数被手改成了 0）。
    pub fn days_recorded(&self) -> usize {
        self.days.len()
    }

    /// 最早的那一天（`days` 是 `BTreeMap`，键是 ISO 日期 ⇒ 第一个就是最早）。
    pub fn first_day(&self) -> Option<&str> {
        self.days.keys().next().map(String::as_str)
    }

    /// 近 `n` 天（含今天）的每日总次数，**最旧的在前**。没有记录的那天给 0。
    /// 这是柱状图唯一的数据源：它必须是连续的一段日期，不能只列"有记录的天"，
    /// 否则空白日会把柱子挤到一起，"最近没怎么用"看起来像"天天都在用"。
    pub fn last_days(&self, today: &str, n: usize) -> Vec<(String, u64)> {
        last_days(today, n)
            .into_iter()
            .map(|d| {
                let total = self.day_total(&d);
                (d, total)
            })
            .collect()
    }

    /// 清成空表（设置页那个「清除统计」）。`schema_version` 留着——清的是计数，不是格式。
    pub fn clear(&mut self) {
        self.days.clear();
    }
}

/// 近 `n` 天（含今天）的 ISO 日期，最旧的在前。`today` 认不出就返回空表。
pub fn last_days(today: &str, n: usize) -> Vec<String> {
    let Some(today_days) = parse_days(today) else {
        return Vec::new();
    };
    (0..n as i64)
        .rev()
        .map(|back| format_days(today_days - back))
        .collect()
}

/// 解析 `YYYY-MM-DD`，返回自 1970-01-01 起的天数。格式对不上、月份/日越界一律 `None`。
/// 越界检查不是多余的：`2026-13-40` 这种手改出来的键如果被当成合法日期，
/// `trim` 会把它换算成一个离谱的天数留在表里，柱状图与合计却永远看不到它。
pub fn parse_days(iso: &str) -> Option<i64> {
    let b = iso.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return None;
    }
    let y: i64 = iso[0..4].parse().ok()?;
    let m: u32 = iso[5..7].parse().ok()?;
    let d: u32 = iso[8..10].parse().ok()?;
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// 天数 → `YYYY-MM-DD`。
pub fn format_days(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `iso + delta` 天；`iso` 认不出给 `None`。
pub fn add_days(iso: &str, delta: i64) -> Option<String> {
    parse_days(iso).map(|d| format_days(d + delta))
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Howard Hinnant 的 `days_from_civil`（公历 → 1970 起的天数）。
/// 与 `civil_from_days` 互为逆；两者都只在整数上动，跨闰日/跨世纪不用特判。
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant 的 `civil_from_days`（1970 起的天数 → 公历）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_for_records_a_custom_command_only_once() {
        assert_eq!(bucket_for(false, false), Bucket::Item);
        assert_eq!(bucket_for(true, false), Bucket::Internal);
        assert_eq!(bucket_for(false, true), Bucket::Custom);
        // 自定义命令同时"在清单里"：仍只落自定义这一桶（批次 F11 去重那条口径）。
        assert_eq!(bucket_for(true, true), Bucket::Custom);
    }

    /// 公历换算的两个方向必须互逆，跨闰日、跨年、跨世纪都得对上——
    /// 保留 400 天那条判据完全落在这两个函数上，它们错一天，裁掉的就会是今天。
    #[test]
    fn civil_day_conversion_round_trips_across_leap_and_year_end() {
        assert_eq!(add_days("2024-02-28", 1).as_deref(), Some("2024-02-29"));
        assert_eq!(add_days("2024-02-29", 1).as_deref(), Some("2024-03-01"));
        assert_eq!(add_days("2024-03-01", -1).as_deref(), Some("2024-02-29"));
        assert_eq!(add_days("2026-12-31", 1).as_deref(), Some("2027-01-01"));
        assert_eq!(add_days("2027-01-01", -1).as_deref(), Some("2026-12-31"));
        assert_eq!(add_days("2000-02-28", 1).as_deref(), Some("2000-02-29"));
        assert_eq!(add_days("1900-02-28", 1).as_deref(), Some("1900-03-01"));
        for iso in ["1970-01-01", "2026-10-28", "2100-02-28", "1999-12-31"] {
            let days = parse_days(iso).unwrap();
            assert_eq!(format_days(days), iso, "往返不一致：{iso}");
        }
    }

    /// 格式与越界：手改坏的键不许被当成合法日期（否则 `trim` 会把它算成某个荒唐的天数）。
    #[test]
    fn parse_rejects_malformed_or_impossible_dates() {
        for bad in [
            "", "2026-1-1", "2026/10/28", "2026-13-01", "2026-00-10", "2026-02-30",
            "2026-04-31", "abcd-ef-gh",
        ] {
            assert_eq!(parse_days(bad), None, "不该认成日期：{bad}");
        }
        assert!(parse_days("2024-02-29").is_some(), "闰年 2 月 29 是合法日期");
    }

    /// 记账的落点：同一天同一个桶累加，别的天/别的桶一律不动。
    #[test]
    fn record_lands_on_the_right_day_and_bucket() {
        let mut s = UsageStats::default();
        assert_eq!(s.record("2026-10-28", Bucket::Item), 1);
        assert_eq!(s.record("2026-10-28", Bucket::Item), 2);
        assert_eq!(s.record("2026-10-28", Bucket::Web), 1);
        assert_eq!(s.record("2026-10-27", Bucket::Item), 1);
        assert_eq!(s.bucket_total(Bucket::Item), 3);
        assert_eq!(s.bucket_total(Bucket::Web), 1);
        assert_eq!(s.day_total("2026-10-28"), 3);
        assert_eq!(s.day_total("2026-10-27"), 1);
        assert_eq!(s.total(), 4);
        assert_eq!(s.days_recorded(), 2);
        assert_eq!(s.first_day(), Some("2026-10-27"));
    }

    /// 保留期边界：`today-399` 留、`today-400` 删。差一天这条断言就会红——
    /// 这是本批最容易写错一格的地方（`395` 与 `400` 只差两个字符）。
    #[test]
    fn trim_keeps_exactly_four_hundred_days_including_today() {
        let today = "2026-10-28";
        let oldest_kept = add_days(today, -(KEEP_DAYS - 1)).unwrap();
        let first_dropped = add_days(today, -KEEP_DAYS).unwrap();
        let mut s = UsageStats::default();
        s.record(&oldest_kept, Bucket::Item);
        s.record(&first_dropped, Bucket::Item);
        s.record(today, Bucket::Calc);
        let dropped = s.trim(today, KEEP_DAYS);
        assert_eq!(dropped, 1, "只该删掉今天往前第 400 天那一天");
        assert!(s.days.contains_key(&oldest_kept), "第 399 天必须留着");
        assert!(!s.days.contains_key(&first_dropped), "第 400 天必须删掉");
        assert!(s.days.contains_key(today));
        assert_eq!(s.trim(today, KEEP_DAYS), 0, "再裁一次不该有变化");
    }

    /// 认不出的日子在裁剪时一并清掉；今天本身认不出就一天都不删（宁可多留）。
    #[test]
    fn trim_drops_unparsable_keys_but_never_when_today_is_unknown() {
        let mut s = UsageStats::default();
        s.record("2026-10-28", Bucket::Item);
        s.record("2026-13-40", Bucket::Item);
        assert_eq!(s.trim("不是日期", KEEP_DAYS), 0);
        assert_eq!(s.days_recorded(), 2, "今天认不出 ⇒ 一条都不删");
        assert_eq!(s.trim("2026-10-28", KEEP_DAYS), 1);
        assert_eq!(s.days_recorded(), 1);
    }

    /// 30 日窗口必须是**连续**的 30 个日期（含空白日），且跨月/跨年都对得上。
    /// 若只列有记录的天，"最近没用"会看起来像"天天在用"。
    #[test]
    fn last_days_is_contiguous_across_month_and_year_boundaries() {
        let days = last_days("2026-03-01", 30);
        assert_eq!(days.len(), 30);
        assert_eq!(days.first().map(String::as_str), Some("2026-01-31"));
        assert_eq!(days.last().map(String::as_str), Some("2026-03-01"));

        let days = last_days("2027-01-01", 3);
        assert_eq!(days, vec!["2026-12-30", "2026-12-31", "2027-01-01"]);

        let mut s = UsageStats::default();
        s.record("2026-03-01", Bucket::Item);
        s.record("2026-02-15", Bucket::Item);
        let window = s.last_days("2026-03-01", 30);
        assert_eq!(window.len(), 30);
        assert_eq!(window.last().map(|(d, n)| (d.as_str(), *n)), Some(("2026-03-01", 1)));
        assert_eq!(
            window
                .iter()
                .find(|(d, _)| d == "2026-02-15")
                .map(|(_, n)| *n),
            Some(1)
        );
        assert_eq!(
            window.iter().find(|(d, _)| d == "2026-02-16").map(|(_, n)| *n),
            Some(0),
            "没有记录的那天要给 0，而不是缺一行"
        );
        assert!(s.last_days("坏日期", 30).is_empty());
    }

    /// **隐私判据（需求书验收："统计页有数据且不含任何输入文本"）**：
    /// 序列化出来的每一个键只能是日期或桶名，每一个值只能是数字——
    /// 将来谁想把"查询词/条目名"顺手塞进这个文件，这条会红。
    #[test]
    fn json_carries_only_dates_buckets_and_counts() {
        let mut s = UsageStats::default();
        for b in Bucket::ALL {
            s.record("2026-10-28", b);
        }
        let v = serde_json::to_value(&s).unwrap();
        let obj = v.as_object().unwrap();
        let known: Vec<&str> = Bucket::ALL.iter().map(|b| b.as_str()).collect();
        let day = obj["days"]["2026-10-28"].as_object().unwrap();
        for (k, val) in day {
            assert!(
                known.contains(&k.as_str()),
                "日子下面只许有已知桶名，出现了 {k}"
            );
            assert!(val.is_u64(), "桶里的值只能是次数，出现了 {val}");
        }
        let mut top: Vec<&str> = obj.keys().map(String::as_str).collect();
        top.sort_unstable();
        assert_eq!(
            top,
            ["days", "schema_version"],
            "顶层只许有 schema_version 与 days"
        );
        assert!(obj["schema_version"].is_u64());
        // 整份 JSON 里不许出现任何非上述形状的字符串值：桶名/日期是键，值只能是数字。
        fn walk(v: &serde_json::Value) {
            match v {
                serde_json::Value::String(s) => panic!("统计文件里出现了字符串值：{s}"),
                serde_json::Value::Array(_) | serde_json::Value::Bool(_) | serde_json::Value::Null => {
                    panic!("统计文件里出现了不该有的形态：{v}")
                }
                serde_json::Value::Object(m) => m.values().for_each(walk),
                serde_json::Value::Number(_) => {}
            }
        }
        walk(&v);
    }

    /// 本程序认不出的桶：读进来不许丢，回写也不许丢（否则新版本写下的数据
    /// 被老版本打开一次就永久没了）。它不在占比表里，但算进合计。
    #[test]
    fn unknown_bucket_is_preserved_and_counted_in_total_only() {
        let mut s = UsageStats::default();
        s.days
            .entry("2026-10-28".into())
            .or_default()
            .insert("clipboard".into(), 7);
        s.record("2026-10-28", Bucket::Item);
        assert_eq!(s.unknown_total(), 7);
        assert_eq!(s.total(), 8);
        assert_eq!(s.bucket_total(Bucket::Item), 1);
        let json = serde_json::to_string(&s).unwrap();
        let back: UsageStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s, "读写一轮不许丢掉未知桶");
        assert_eq!(back.days["2026-10-28"]["clipboard"], 7);
    }

    /// 缺键的老文件（或手写的空对象）读成默认值；`days` 里空对象也不炸。
    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: UsageStats = serde_json::from_str("{}").unwrap();
        assert_eq!(s.schema_version, USAGE_SCHEMA);
        assert!(s.days.is_empty());
        let s: UsageStats = serde_json::from_str(r#"{"days":{}}"#).unwrap();
        assert_eq!(s.total(), 0);
        assert_eq!(s.first_day(), None);
        assert_eq!(s.last_days("2026-10-28", 30).len(), 30);
    }

    /// 清空只清计数，格式版本留着；清完再记账照常。
    #[test]
    fn clear_keeps_schema_and_allows_fresh_records() {
        let mut s = UsageStats::default();
        s.record("2026-10-28", Bucket::File);
        s.clear();
        assert_eq!(s.total(), 0);
        assert_eq!(s.days_recorded(), 0);
        assert_eq!(s.schema_version, USAGE_SCHEMA);
        assert_eq!(s.record("2026-10-29", Bucket::File), 1);
    }
}

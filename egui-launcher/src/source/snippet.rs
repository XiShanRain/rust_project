//! FR-SRC-07 片段：`snip` 列全部、`snip 词` 只搜片段，回车把正文粘回呼出前的窗口。
//!
//! 与其它源的接法同脚本源（FR-SRC-15）：片段不进清单，产出的是头段行
//! （[`SourceRow::Snippet`]）。`plan()` 的匹配链一个字不动 —— 这是 A10 插槽的验收口径。
//!
//! 本批只做「手动搜 + 粘回」；`;keyword` 全局自动展开（FR-SRC-08）要键盘钩子，单列下一批。
//! `auto_expand` 字段照需求书 4.2.2 保留在数据模型里（编辑时不丢原值），但还没有生效路径，
//! 「自动展开」这个开关也就先不给 UI。
//!
//! 日志口径（FR-SYS-03）：片段名、关键字、正文都算用户内容，**任何一条都不落日志**，
//! 只记条数与字符数。

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::matching;

use super::{DataSource, SearchCtx, SourceId, SourceRow};

/// 出厂触发词（需求书 4.2.3 `features.snippets`）。
pub const DEFAULT_KEYWORD: &str = "snip";
/// 粘贴延迟出厂值（同表）。
pub const DEFAULT_PASTE_DELAY_MS: u32 = 300;
/// 延迟的可配范围：太小会让目标程序来不及响应 `Ctrl+V`，太大就是用户干等。
pub const MIN_PASTE_DELAY_MS: u32 = 50;
pub const MAX_PASTE_DELAY_MS: u32 = 3000;
/// 缩写总长度上限（需求书 4.2.2：前缀 + 关键字 ≤ 40 且不含空格）。
pub const MAX_ABBR_CHARS: usize = 40;
/// 正文搜索最少字数（需求书 FR-SRC-07）。
pub const MIN_BODY_CHARS: usize = 3;
/// 正文命中的固定分值（同条）。
const BODY_SCORE: f32 = 35.0;
/// 行上副标题（正文预览）最多几个字。
const PREVIEW_CHARS: usize = 60;
/// 数据文件变化检查节流。片段是用户手改文件也合法的形态，1 秒内不会漏掉设置页那一次写。
const REFRESH_EVERY: Duration = Duration::from_millis(1000);

/// 一条片段（需求书 4.2.2）：
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    #[serde(default)]
    pub name: String,
    /// 触发码（可选、要短）。空串＝只能靠名称搜到。
    #[serde(default)]
    pub keyword: String,
    #[serde(default)]
    pub text: String,
    /// 留给 FR-SRC-08（`;keyword` 全局展开）。本批只存不读。
    #[serde(default)]
    pub auto_expand: bool,
}

/// `snippets.json` 的落盘形态。与 `param_history.json` / `usage.json` 同一条：
/// 单独一份文件，别去动 `settings.json`（用户会手改的那份配置）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnippetFile {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
}

pub const SCHEMA_VERSION: u32 = 1;

/// 粘贴方式（需求书 FR-SRC-07）。存稳定名，与 `TermShell` 同一口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PasteMode {
    /// 借剪贴板：全格式快照 → 写正文 → `Ctrl+V` → 等延迟 → 还原。
    #[default]
    Clipboard,
    /// 逐字注入（`KEYEVENTF_UNICODE`），不碰剪贴板。
    Type,
}

impl PasteMode {
    pub const ALL: [PasteMode; 2] = [Self::Clipboard, Self::Type];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clipboard => "clipboard",
            Self::Type => "type",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let want = name.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|m| m.as_str() == want)
    }
}

/// 保存前的校验结论。主程序把它翻成 i18n 文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidateError {
    EmptyName,
    EmptyText,
    /// 关键字里带空白：`snip a b` 会被拆成「触发词 + 两个搜索词」，关键字本身不可能这样触发。
    KeywordSpace,
    AbbrTooLong,
}

/// 保存前校验。`prefix` 是当前触发词（`features.snippets` 的关键字）。
pub fn validate(s: &Snippet, prefix: &str) -> Result<(), ValidateError> {
    if s.name.trim().is_empty() {
        return Err(ValidateError::EmptyName);
    }
    if s.text.trim().is_empty() {
        return Err(ValidateError::EmptyText);
    }
    let kw = s.keyword.trim();
    if kw.is_empty() {
        return Ok(());
    }
    if kw.chars().any(char::is_whitespace) {
        return Err(ValidateError::KeywordSpace);
    }
    let abbr = prefix.trim().chars().count() + 1 + kw.chars().count();
    if abbr > MAX_ABBR_CHARS {
        return Err(ValidateError::AbbrTooLong);
    }
    Ok(())
}

/// 读盘之后过一遍：名称/关键字去首尾空白，名称或正文为空的条目直接丢掉
/// （手改文件可能留下半截行；留着它只会在列表里出现一条按回车什么也不做的行）。
/// 返回丢了几条，供调用方记日志。
pub fn sanitize(list: &mut Vec<Snippet>) -> usize {
    let before = list.len();
    for s in list.iter_mut() {
        s.name = s.name.trim().to_string();
        s.keyword = s.keyword.trim().to_string();
    }
    list.retain(|s| !s.name.is_empty() && !s.text.trim().is_empty());
    before - list.len()
}

/// 查询路由。`snip` ⇒ 列全部；`snip 词` ⇒ 只搜片段；别的查询不归本源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    All,
    Search(String),
}

pub fn route(query: &str, keyword: &str) -> Option<Route> {
    let kw = keyword.trim();
    if kw.is_empty() {
        return None;
    }
    let p = matching::parse(query);
    if p.keyword != kw.to_lowercase() {
        return None;
    }
    if !p.has_rest {
        return Some(Route::All);
    }
    let rest = p.rest.trim();
    if rest.is_empty() {
        return Some(Route::All);
    }
    Some(Route::Search(rest.to_string()))
}

/// 一次搜索的命中，按名次排好序。
pub fn search<'a>(list: &'a [Snippet], query: &str, body_search: bool) -> Vec<&'a Snippet> {
    let p = matching::parse(query);
    let chars = p.full.chars().count();
    let lower_text: Vec<String> = if body_search && chars >= MIN_BODY_CHARS {
        list.iter().map(|s| s.text.to_lowercase()).collect()
    } else {
        Vec::new()
    };
    let mut ranked: Vec<(u8, f32, usize)> = Vec::new();
    for (i, s) in list.iter().enumerate() {
        let title = matching::score_text(&p, &s.name).or_else(|| {
            if s.keyword.is_empty() {
                None
            } else {
                matching::score_text(&p, &s.keyword)
            }
        });
        if let Some((tier, score)) = title {
            ranked.push((tier, score, i));
            continue;
        }
        if body_search && chars >= MIN_BODY_CHARS {
            let text = &lower_text[i];
            if p.words.iter().all(|w| text.contains(w.as_str())) {
                // 正文命中固定 35 分、档位压到最低：需求书那句「标题不高亮」就是
                // 「正文命中不升级成标题命中」的另一面。
                ranked.push((0, BODY_SCORE, i));
            }
        }
    }
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.2.cmp(&b.2))
    });
    ranked.into_iter().map(|(_, _, i)| &list[i]).collect()
}

/// 一行可执行的片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetHit {
    pub name: String,
    pub keyword: String,
    pub text: String,
    /// 行上那半灰的副标题：关键字 + 正文首行预览。
    pub subtitle: String,
}

impl SnippetHit {
    pub fn of(s: &Snippet) -> Self {
        let mut subtitle = String::new();
        if !s.keyword.is_empty() {
            subtitle.push_str(&s.keyword);
            subtitle.push_str(" · ");
        }
        subtitle.push_str(&preview(&s.text));
        Self {
            name: s.name.clone(),
            keyword: s.keyword.clone(),
            text: s.text.clone(),
            subtitle,
        }
    }
}

/// 正文首行预览：折叠空白、截到 [`PREVIEW_CHARS`]。多行片段只看第一行有意义的字。
pub fn preview(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            continue;
        }
        if out.chars().count() >= PREVIEW_CHARS {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out.trim_end().to_string()
}

/// 不可执行的提示行：库里一条都没有 / 这一轮没搜到。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetHint {
    pub title: String,
    pub subtitle: String,
}

impl SnippetHint {
    pub fn empty() -> Self {
        Self {
            title: crate::i18n::t("snippet.hint.empty.title").to_string(),
            subtitle: crate::i18n::t("snippet.hint.empty.subtitle").to_string(),
        }
    }

    pub fn no_match() -> Self {
        Self {
            title: crate::i18n::t("snippet.hint.nomatch.title").to_string(),
            subtitle: crate::i18n::t("snippet.hint.nomatch.subtitle").to_string(),
        }
    }
}

// ---- 本机当地时间与占位符展开 ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// 展开占位符要的外部世界。**全部由调用方注入**：日期来自 `GetLocalTime`、剪贴板来自
/// 一次显式读取、UUID 来自 `CoCreateGuid` —— 纯函数这一层不碰系统。
pub struct ExpandInput<'a> {
    pub now: LocalTime,
    pub clipboard: Option<&'a str>,
    pub uuid: String,
}

/// `{cursor}` 的落点：调用方把 `before` 投出去之后，要按 `after` 的字数补等量左箭头。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub before: String,
    pub after: String,
}

/// `{cursor}` 在中间串里的临时记号。控制字符 `U+0001` 不会出现在正常文本里。
const CURSOR_MARK: char = '\u{1}';

/// 展开正文占位符（需求书 4.5）。不认识的 `{…}` 原样保留——宁可让用户看见自己写错了，
/// 也不擅自删掉一段他以为会生效的内容。
pub fn expand(text: &str, input: &ExpandInput<'_>) -> Expanded {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail[..].find('}') else {
            out.push_str(tail);
            rest = "";
            break;
        };
        let token = &tail[1..end];
        match placeholder(token, input) {
            Some(value) => out.push_str(&value),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    match out.find(CURSOR_MARK) {
        Some(pos) => {
            let mut after = out.split_off(pos);
            // 记号本身不投给目标窗口；`after` 里若还有第二个 `{cursor}` 也一并去掉
            // （多写一个光标位置没有意义，按第一个落点处理）。
            after = after.replace(CURSOR_MARK, "");
            Expanded {
                before: out.replace(CURSOR_MARK, ""),
                after,
            }
        }
        None => Expanded {
            before: out,
            after: String::new(),
        },
    }
}

fn placeholder(token: &str, input: &ExpandInput<'_>) -> Option<String> {
    let now = input.now;
    match token {
        "date" => return Some(fmt_datetime("yyyy-MM-dd", now).into_owned()),
        "time" => return Some(fmt_datetime("HH:mm:ss", now).into_owned()),
        "datetime" => return Some(fmt_datetime("yyyy-MM-dd HH:mm:ss", now).into_owned()),
        "uuid" => return Some(input.uuid.clone()),
        "cursor" => return Some(CURSOR_MARK.to_string()),
        "clipboard" => return Some(input.clipboard.unwrap_or_default().to_string()),
        _ => {}
    }
    if let Some(rest) = token.strip_prefix("clipboard:") {
        let n: usize = rest.trim().parse().ok()?;
        return Some(
            input
                .clipboard
                .unwrap_or_default()
                .chars()
                .take(n)
                .collect(),
        );
    }
    if let Some(rest) = token.strip_prefix("time:") {
        if rest.is_empty() {
            return None;
        }
        return Some(fmt_datetime(rest, now).into_owned());
    }
    // 日期：`date`、`date:格式`、`date+7`、`date-1:格式`。
    let rest = token.strip_prefix("date")?;
    if rest.is_empty() {
        return Some(fmt_datetime("yyyy-MM-dd", now).into_owned());
    }
    if let Some(fmt) = rest.strip_prefix(':') {
        if fmt.is_empty() {
            return None;
        }
        return Some(fmt_datetime(fmt, now).into_owned());
    }
    let sign = rest.chars().next()?;
    if sign != '+' && sign != '-' {
        return None;
    }
    let digits_end = rest[1..]
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    let n: i64 = rest[1..digits_end].parse().ok()?;
    let delta = if sign == '-' { -n } else { n };
    let tail = &rest[digits_end..];
    let fmt = if tail.is_empty() {
        "yyyy-MM-dd"
    } else {
        tail.strip_prefix(':')?
    };
    if fmt.is_empty() {
        return None;
    }
    Some(fmt_datetime(fmt, shift_days(now, delta)).into_owned())
}

/// AHK `FormatTime` 式的几个记号：`yyyy` `yy` `MM` `M` `dd` `d` `HH` `H` `mm` `m` `ss` `s`。
/// 其余字符（`-` `/` 空格、中文…）原样保留。
fn fmt_datetime(fmt: &str, t: LocalTime) -> std::borrow::Cow<'_, str> {
    let mut out = String::with_capacity(fmt.len() + 8);
    let mut rest = fmt;
    while !rest.is_empty() {
        let pats: [(&str, String); 12] = [
            ("yyyy", format!("{:04}", t.year)),
            ("yy", format!("{:02}", t.year.rem_euclid(100))),
            ("MM", format!("{:02}", t.month)),
            ("M", t.month.to_string()),
            ("dd", format!("{:02}", t.day)),
            ("d", t.day.to_string()),
            ("HH", format!("{:02}", t.hour)),
            ("H", t.hour.to_string()),
            ("mm", format!("{:02}", t.minute)),
            ("m", t.minute.to_string()),
            ("ss", format!("{:02}", t.second)),
            ("s", t.second.to_string()),
        ];
        match pats.iter().find(|(p, _)| rest.starts_with(p)) {
            Some((p, v)) => {
                out.push_str(v);
                rest = &rest[p.len()..];
            }
            None => {
                let ch = rest.chars().next().unwrap_or_default();
                out.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    std::borrow::Cow::Owned(out)
}

/// 日期加减天数，跨月跨年由 Hinnant 的民用历算法兜住（与 `usage.rs` 同一套）。
fn shift_days(t: LocalTime, delta: i64) -> LocalTime {
    let z = days_from_civil(t.year, t.month, t.day) + delta;
    let (year, month, day) = civil_from_days(z);
    LocalTime { year, month, day, ..t }
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---- 数据源 ----

/// 片段库现场。与脚本源同形：`poll` 检查文件时间戳，变了就整表重载。
pub struct SnippetSource {
    list: Vec<Snippet>,
    stamp: Option<SystemTime>,
    checked: Instant,
    keyword: String,
    body_search: bool,
    /// 数据文件路径。`None`＝还没初始化（单测直接构造时除外）。
    path: Option<PathBuf>,
}

impl Default for SnippetSource {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            stamp: None,
            // 首帧就允许检查一次：呼出时文件可能刚被设置页改过。
            checked: Instant::now() - REFRESH_EVERY,
            keyword: DEFAULT_KEYWORD.to_string(),
            body_search: true,
            path: None,
        }
    }
}

impl SnippetSource {
    #[cfg(test)]
    pub fn with_list(list: Vec<Snippet>) -> Self {
        Self {
            list,
            ..Self::default()
        }
    }

    fn all_rows(&self) -> Vec<SourceRow> {
        if self.list.is_empty() {
            return vec![SourceRow::SnippetHint(SnippetHint::empty())];
        }
        self.list
            .iter()
            .map(|s| SourceRow::Snippet(SnippetHit::of(s)))
            .collect()
    }

    fn search_rows(&self, query: &str) -> Vec<SourceRow> {
        let hits = search(&self.list, query, self.body_search);
        if hits.is_empty() {
            return vec![SourceRow::SnippetHint(SnippetHint::no_match())];
        }
        hits.into_iter()
            .map(|s| SourceRow::Snippet(SnippetHit::of(s)))
            .collect()
    }
}

impl DataSource for SnippetSource {
    fn id(&self) -> SourceId {
        SourceId::Snippets
    }

    fn label(&self) -> &'static str {
        crate::i18n::t("source.snippets.label")
    }

    fn note(&self) -> &'static str {
        crate::i18n::t("source.snippets.note")
    }

    fn init(&mut self) {
        let path = self
            .path
            .clone()
            .unwrap_or_else(|| super::application::snippets_path());
        self.path = Some(path.clone());
        self.reload();
        log_info!("snippets: 已载入 {} 条片段", self.list.len());
    }

    fn poll(&mut self) -> bool {
        if self.checked.elapsed() < REFRESH_EVERY {
            return false;
        }
        self.checked = Instant::now();
        let mut now = self.stamp;
        if let Some(path) = &self.path {
            now = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        }
        if now == self.stamp {
            return false;
        }
        self.reload();
        log_info!("snippets: 文件有变化，重载得 {} 条", self.list.len());
        true
    }

    fn take_over(&mut self, cx: &mut SearchCtx<'_>) -> Option<Vec<SourceRow>> {
        match route(cx.query, &self.keyword)? {
            Route::All => Some(self.all_rows()),
            Route::Search(q) => Some(self.search_rows(&q)),
        }
    }

    fn apply_settings(&mut self, prefs: &super::application::AppSettings) {
        self.keyword = prefs.snippet_keyword_effective().to_string();
        self.body_search = prefs.snippet_body_search;
    }

    fn reload(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let (mut file, bad) = super::application::load_snippets_at(&path);
        let dropped = sanitize(&mut file.snippets);
        if dropped > 0 {
            log_info!("snippets: 丢弃 {dropped} 条缺名称/正文的坏条目");
        }
        if let Some(c) = bad {
            log_info!("snippets: 片段文件读不了，已按 E1 口径隔离（{c:?}）");
        }
        self.list = file.snippets;
        self.stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        self.checked = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(name: &str, keyword: &str, text: &str) -> Snippet {
        Snippet {
            name: name.to_string(),
            keyword: keyword.to_string(),
            text: text.to_string(),
            auto_expand: false,
        }
    }

    fn now() -> LocalTime {
        LocalTime {
            year: 2026,
            month: 10,
            day: 31,
            hour: 9,
            minute: 5,
            second: 7,
        }
    }

    fn input<'a>(clip: Option<&'a str>) -> ExpandInput<'a> {
        ExpandInput {
            now: now(),
            clipboard: clip,
            uuid: "11111111-2222-4333-8444-555555555555".to_string(),
        }
    }

    #[test]
    fn route_only_fires_at_position_zero() {
        assert_eq!(route("snip", "snip"), Some(Route::All));
        assert_eq!(route("snip   ", "snip"), Some(Route::All), "尾随空格仍算列全部");
        assert_eq!(
            route("snip sig", "snip"),
            Some(Route::Search("sig".to_string()))
        );
        assert_eq!(
            route("SNIP Sig", "snip"),
            Some(Route::Search("Sig".to_string())),
            "触发词不分大小写"
        );
        assert_eq!(route("x snip", "snip"), None, "不在首位不算");
        assert_eq!(route("snipped", "snip"), None, "不能把同前缀的普通查询吞掉");
        assert_eq!(route("", "snip"), None);
        assert_eq!(route("snip", ""), None, "空触发词＝本源不参与");
    }

    #[test]
    fn search_hits_name_and_keyword() {
        let list = vec![
            snip("签名", "sig", "此致敬礼"),
            snip("邮箱尾注", "mail", "此致敬礼，西山雨"),
        ];
        let hits = search(&list, "签名", true);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "签名");
        let hits = search(&list, "mail", true);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "邮箱尾注");
    }

    #[test]
    fn body_search_needs_three_chars_and_every_word() {
        let list = vec![snip("签名", "sig", "此致敬礼")];
        assert!(search(&list, "此致", true).is_empty(), "两个字的正文搜索不启用");
        assert_eq!(search(&list, "此致敬", true).len(), 1);
        assert_eq!(search(&list, "此致 敬礼", true).len(), 1, "每个词都出现");
        assert!(
            search(&list, "此致敬礼 明天", true).is_empty(),
            "少一个词就不算命中"
        );
        assert!(
            search(&list, "此致", false).is_empty(),
            "正文搜索关掉后一个字都不看"
        );
    }

    #[test]
    fn title_hits_rank_before_body_hits() {
        let list = vec![
            snip("普通", "a", "这里写着 关键字 三个字"),
            snip("关键字", "b", "别的正文"),
        ];
        let hits = search(&list, "关键字", true);
        assert_eq!(hits[0].name, "关键字", "标题命中压过正文命中");
        assert_eq!(hits[1].name, "普通");
    }

    #[test]
    fn validate_checks_required_fields_and_abbreviation() {
        assert_eq!(
            validate(&snip("  ", "", "x"), "snip"),
            Err(ValidateError::EmptyName)
        );
        assert_eq!(
            validate(&snip("a", "", " "), "snip"),
            Err(ValidateError::EmptyText)
        );
        assert_eq!(
            validate(&snip("a", "two words", "x"), "snip"),
            Err(ValidateError::KeywordSpace)
        );
        // snip(4) + 空格(1) + 35 = 40：正好在上限
        let ok = snip("a", &"k".repeat(35), "x");
        assert_eq!(validate(&ok, "snip"), Ok(()));
        let too_long = snip("a", &"k".repeat(36), "x");
        assert_eq!(validate(&too_long, "snip"), Err(ValidateError::AbbrTooLong));
        assert_eq!(
            validate(&snip("a", "", "x"), "snip"),
            Ok(()),
            "关键字可选"
        );
    }

    #[test]
    fn preview_folds_whitespace_and_truncates() {
        assert_eq!(preview("第一行\n\n第二行"), "第一行 第二行");
        let long = "字".repeat(PREVIEW_CHARS + 5);
        let p = preview(&long);
        assert_eq!(p.chars().count(), PREVIEW_CHARS + 1);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn expand_date_time_datetime() {
        let e = expand("{date} {time} {datetime}", &input(None));
        assert_eq!(e.before, "2026-10-31 09:05:07 2026-10-31 09:05:07");
        assert!(e.after.is_empty());
    }

    #[test]
    fn expand_clipboard_variants() {
        let i = input(Some("abcdef"));
        assert_eq!(expand("{clipboard}", &i).before, "abcdef");
        assert_eq!(expand("{clipboard:3}", &i).before, "abc");
        assert_eq!(expand("{clipboard:99}", &i).before, "abcdef");
        assert_eq!(expand("{clipboard}", &input(None)).before, "");
    }

    #[test]
    fn expand_uuid_and_unknown_tokens() {
        let i = input(None);
        assert_eq!(
            expand("{uuid}", &i).before,
            "11111111-2222-4333-8444-555555555555"
        );
        assert_eq!(
            expand("{unknown} {date", &i).before,
            "{unknown} {date",
            "不认识的占位符原样保留"
        );
    }

    #[test]
    fn expand_date_offsets_and_formats() {
        let i = input(None);
        assert_eq!(expand("{date+7}", &i).before, "2026-11-07");
        assert_eq!(expand("{date-1}", &i).before, "2026-10-30");
        assert_eq!(expand("{date+40}", &i).before, "2026-12-10", "跨月");
        assert_eq!(expand("{date-400}", &i).before, "2025-09-26", "跨年");
        assert_eq!(expand("{date:yyyy/MM/dd}", &i).before, "2026/10/31");
        assert_eq!(expand("{date-1:MMddyyyy}", &i).before, "10302026");
        assert_eq!(expand("{time:HH-mm}", &i).before, "09-05");
        // `H` 是合法记号（小时，不补零）；真不认识的字母才原样留着。
        assert_eq!(expand("{date:H}", &i).before, "9", "单 H 是小时");
        assert_eq!(expand("{date:Q}", &i).before, "Q", "Q 不是日期记号，原样留着");
    }

    #[test]
    fn expand_splits_at_cursor() {
        let e = expand("前{clipboard:2}中{cursor}后", &input(Some("XY")));
        assert_eq!(e.before, "前XY中");
        assert_eq!(e.after, "后");
        let e = expand("没有光标", &input(None));
        assert_eq!(e.after, "");
    }

    #[test]
    fn date_shift_handles_leap_and_year_end() {
        let t = LocalTime {
            year: 2024,
            month: 2,
            day: 28,
            hour: 0,
            minute: 0,
            second: 0,
        };
        assert_eq!(
            shift_days(t, 1),
            LocalTime {
                year: 2024,
                month: 2,
                day: 29,
                hour: 0,
                minute: 0,
                second: 0,
            }
        );
        let t = LocalTime {
            year: 2026,
            month: 12,
            day: 31,
            hour: 0,
            minute: 0,
            second: 0,
        };
        assert_eq!(
            shift_days(t, 1),
            LocalTime {
                year: 2027,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            }
        );
    }

    #[test]
    fn source_lists_all_then_searches() {
        let src = SnippetSource::with_list(vec![
            snip("签名", "sig", "此致敬礼"),
            snip("邮箱", "mail", "x@example.com"),
        ]);
        assert_eq!(src.all_rows().len(), 2);
        assert_eq!(src.search_rows("此致敬").len(), 1, "正文命中出 1 行");
        let empty = src.search_rows("zzz");
        assert_eq!(empty.len(), 1);
        assert!(matches!(empty[0], SourceRow::SnippetHint(_)));
    }

    #[test]
    fn empty_library_shows_a_hint_row() {
        let src = SnippetSource::with_list(Vec::new());
        let rows = src.all_rows();
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0], SourceRow::SnippetHint(_)));
    }
}

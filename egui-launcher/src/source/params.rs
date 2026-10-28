//! FR-ACT-07 参数标记与参数历史（批次 B6，2026-10-28）。
//!
//! 这一层是**纯数据 + 纯函数**：不碰剪贴板、不碰窗口、不落盘（落盘在
//! `application::load_param_history` / `save_param_history`）。主线程在呼出那一刻把
//! 剪贴板文本与前台窗口信息抓成一份 [`Snapshot`] 传进来，执行前再调 [`expand_command`]
//! 把标记换掉。
//!
//! 与 Delphi `untShortCutMan.pas` 对齐的口径（行号指 V1.46.7 源码）：
//! · `TParamType = (ptNone, ptNoEncoding, ptURLQuery, ptUTF8Query)`（`untShortCutMan.pas:33`）；
//! · 标记串 `{%p} {%c} {%wd} {%wt} {%wc}`（`untALTRunOption.pas:28-32`）；
//! · `ParamType = ptNone` 时**整条命令一个字都不换**（`Execute(cmdobj)` 的替换链整段只在
//!   `ParamType <> ptNone` 时走）——所以"无参项却写了标记"要保存时先问一句；
//! · 没有标记但 `ParamType <> ptNone` 时弹参数框并追加：URL 两档拼在命令行尾、
//!   不编码那档作为独立参数传（`untShortCutMan.pas:756-796`）。
//!
//! 三处**刻意偏差**（都写进 RELEASE_NOTES §六十）：
//! 1. Delphi 的替换链是 `else if`，一条命令里只换**第一个命中**的标记；这里每个标记各换各的
//!    （`{%p}` 用输入值、`{%c}` 用剪贴板……）。Delphi 那套在"同时写了 `{%p}` 和 `{%c}`"时
//!    会拿剪贴板去填 `{%p}`，是意外的副作用，不是设计。
//! 2. 编码按 **UTF-8 字节**百分号编码。Delphi 的 `url_query` 编的是系统 ANSI（本机 GBK）字节，
//!    但我们的字符串本来就是 UTF-8，现代端点也按 UTF-8 收；GBK 的百分号形态是历史包袱。
//!    `utf8_query` 与 `url_query` 因此算出来一样——枚举保留第四档只为读得懂旧配置。
//! 3. 保留字符比 Delphi 多两个：数字 `0-9` 与 `~`。Delphi 的 `NoConversion` 漏了数字
//!    （`untUtilities.pas:1119`），`abc123` 会被编成 `abc%31%32%33`，那是 bug 不是特性。

use serde::{Deserialize, Serialize};

/// `{%p}`：用户在参数框里敲的那串（占位符本体）。
pub const MARK_PARAM: &str = "{%p}";
/// `{%c}`：呼出瞬间的剪贴板文本。
pub const MARK_CLIP: &str = "{%c}";
/// `{%wd}`：呼出瞬间的前台窗口句柄（十进制）。
pub const MARK_WIN_ID: &str = "{%wd}";
/// `{%wt}`：呼出瞬间的前台窗口标题。
pub const MARK_WIN_TEXT: &str = "{%wt}";
/// `{%wc}`：呼出瞬间的前台窗口类名。
pub const MARK_WIN_CLASS: &str = "{%wc}";

/// 全部标记（顺序即帮助文案里的展示顺序）。
pub const ALL_MARKS: [&str; 5] = [
    MARK_PARAM,
    MARK_CLIP,
    MARK_WIN_ID,
    MARK_WIN_TEXT,
    MARK_WIN_CLASS,
];

/// 参数编码类型（对齐 Delphi `TParamType`）。
///
/// serde 用蛇形：`"none"` / `"no_encoding"` / `"url_query"` / `"utf8_query"`。
/// 旧清单没有这一格 ⇒ `#[serde(default)]` 落到 [`ParamType::None`]，照旧直接启动。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    /// 无参数：标记不展开，也不弹参数框。
    #[default]
    None,
    /// 参数原样代入（`ParamType_1` = "Param without encoding"）。
    NoEncoding,
    /// 参数按 URL 查询串编码（Delphi 的百度搜索用法）。
    UrlQuery,
    /// 参数按 UTF-8 查询串编码（Delphi 的 Google/Yahoo 用法）。
    Utf8Query,
}

impl ParamType {
    /// 下拉框里的四档（顺序即展示顺序）。
    pub const ALL: [ParamType; 4] = [
        ParamType::None,
        ParamType::NoEncoding,
        ParamType::UrlQuery,
        ParamType::Utf8Query,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ParamType::None => "无参数",
            ParamType::NoEncoding => "参数不编码",
            ParamType::UrlQuery => "URL 编码",
            ParamType::Utf8Query => "UTF-8 编码",
        }
    }

    /// 下拉框右侧那句解释（也是编辑弹窗里那行提示的正文）。
    pub fn hint(self) -> &'static str {
        match self {
            ParamType::None => "回车直接启动；命令行里的参数标记不会展开",
            ParamType::NoEncoding => "参数原样代入（本地程序、带空格的文件名都走这一档）",
            ParamType::UrlQuery => "按 URL 查询串编码（空格→+，中文按 UTF-8 百分号）",
            ParamType::Utf8Query => "同 URL 编码（保留第四档只为认得 Delphi 的旧配置）",
        }
    }
}

/// 呼出那一刻抓下来的那一份现场。**只在内存里活到本次执行完**：
/// 剪贴板文本、窗口标题都是用户内容，绝不落日志、也绝不写进任何数据文件
/// （参数历史里只存用户自己在参数框里敲的那串）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub clipboard: String,
    /// 十进制字符串：标记要的是"文本"，句柄在 Delphi 里也是 `IntToStr` 之后代入的。
    pub window_id: String,
    pub window_title: String,
    pub window_class: String,
}

impl Snapshot {
    fn value(&self, mark: &str) -> Option<&str> {
        match mark {
            MARK_CLIP => Some(&self.clipboard),
            MARK_WIN_ID => Some(&self.window_id),
            MARK_WIN_TEXT => Some(&self.window_title),
            MARK_WIN_CLASS => Some(&self.window_class),
            _ => None,
        }
    }
}

/// 这段文本里有没有参数标记（五个都算）。
pub fn has_marker(text: &str) -> bool {
    ALL_MARKS.iter().any(|m| text.contains(m))
}

/// 这段文本里有没有 `{%p}`（只有它需要弹框问用户）。
pub fn has_param_marker(text: &str) -> bool {
    text.contains(MARK_PARAM)
}

/// 这一条执行前要不要弹参数框。
///
/// 两条来源，与 Delphi `Execute(ShortCutItem)` 同构：
/// · 命令里写了 `{%p}`（其余四种标记能自动抓，不必问）；
/// · 一个标记都没写，但 `ParamType` 不是 none（Delphi 也弹框，然后把参数追加进去）。
pub fn needs_prompt(path: &str, arguments: &str, ty: ParamType) -> bool {
    if ty == ParamType::None {
        return false;
    }
    let any = has_marker(path) || has_marker(arguments);
    !any || has_param_marker(path) || has_param_marker(arguments)
}

/// 按编码档把参数编成一串。`None` 档没有"可代入的值"，原样返回（调用方不该走到这里）。
pub fn encode(text: &str, ty: ParamType) -> String {
    match ty {
        ParamType::None | ParamType::NoEncoding => text.to_string(),
        ParamType::UrlQuery | ParamType::Utf8Query => query_encode(text),
    }
}

/// 百分号编码：保留 `A-Za-z0-9 * @ . _ - ~`，空格→`+`，其余按 UTF-8 字节 `%XX`（大写）。
///
/// 抽成纯函数是为了能钉表格——它决定 `{%wt}` 里一个中文标题能不能安全落进 URL。
pub fn query_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'@' | b'.' | b'_' | b'-'
            | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0f) as usize] as char);
            }
        }
    }
    out
}

/// 单遍展开一个模板。
///
/// **一遍过、不回头扫**：用户参数里即使写着 `{%c}` 也只是一个普通字符串，不会被再换一次
/// （Delphi 的 `Regex.Replace` 同样只替换该次调用匹配到的那个标记）。认不出来的 `{%…}`
/// 原样留着——宁可让用户看见自己敲错了，也不要悄悄吞掉。
pub fn expand_template(template: &str, ty: ParamType, snap: &Snapshot, param: &str) -> String {
    if ty == ParamType::None || template.is_empty() {
        return template.to_string();
    }
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(at) = rest.find("{%") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let hit = ALL_MARKS.iter().find(|m| tail.starts_with(**m));
        match hit {
            Some(mark) => {
                let raw = if *mark == MARK_PARAM {
                    param
                } else {
                    snap.value(mark).unwrap_or("")
                };
                out.push_str(&encode(raw, ty));
                rest = &tail[mark.len()..];
            }
            None => {
                // `{%` 不是已知标记：把这两个字符原样吐出去，从下一个字符继续找。
                out.push_str("{%");
                rest = &tail[2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 一条命令展开之后的两格（`path` / `arguments`）。
///
/// 没有标记但 `ParamType≠none` 时按 Delphi 追加：URL 两档接在命令行尾部（搜索链接就是这么
/// 拼的），不编码那档并到参数里（`notepad.exe` + `C:\a b.txt` 这种要分成两个参数的场合）。
pub fn expand_command(
    path: &str,
    arguments: &str,
    ty: ParamType,
    snap: &Snapshot,
    param: &str,
) -> (String, String) {
    if ty == ParamType::None {
        return (path.to_string(), arguments.to_string());
    }
    if !has_marker(path) && !has_marker(arguments) {
        let value = encode(param, ty);
        return match ty {
            ParamType::NoEncoding => {
                let args = if arguments.trim().is_empty() {
                    value
                } else if value.is_empty() {
                    arguments.to_string()
                } else {
                    format!("{arguments} {value}")
                };
                (path.to_string(), args)
            }
            _ => (format!("{path}{value}"), arguments.to_string()),
        };
    }
    (
        expand_template(path, ty, snap, param),
        expand_template(arguments, ty, snap, param),
    )
}

/// 参数历史上限（Delphi `ParamHistoryLimit` 默认 50，`untALTRunOption.pas:757`）。
pub const HISTORY_LIMIT: usize = 50;

/// 参数历史里的一条：`rank` 是使用次数（Delphi 的 `%-10d|` 那一格），`text` 是参数原文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamEntry {
    #[serde(default)]
    pub rank: i64,
    #[serde(default)]
    pub text: String,
}

/// 参数历史表。**只存用户自己在参数框里敲过的串**：自动抓来的剪贴板/标题从不进来。
///
/// 顺序就是 Delphi 下拉框的顺序（文件里的先后），不是 rank 序：新条目头插，
/// 复用旧条目只加 rank、不挪位置（`frmParam.pas` 的 `btnOKClick`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamHistory {
    #[serde(default)]
    pub entries: Vec<ParamEntry>,
}

impl ParamHistory {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 按展示顺序给参数原文（参数框里的历史列表照这个排）。
    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.text.as_str())
    }

    /// 记一次用户确认过的参数。空串（去空白后）不记。
    ///
    /// 返回 `true` = 这张表真的变了 ⇒ 调用方据此决定要不要写盘。
    ///
    /// 三条口径照 `frmParam.pas:82-110`：
    /// · 已有（**区分大小写**，Delphi `Items.IndexOf` 就是逐字比）⇒ rank + 1，位置不动；
    /// · 新的 ⇒ 头插、rank 0；
    /// · 超过上限 ⇒ 删掉 rank 最小的那一条；并列时删**靠后**的（Delphi 从后往前扫、
    ///   用严格小于，最终留下的是并列里最靠后的那个下标）。
    pub fn touch(&mut self, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        if let Some(e) = self.entries.iter_mut().find(|e| e.text == text) {
            e.rank = e.rank.saturating_add(1);
            return true;
        }
        self.entries.insert(
            0,
            ParamEntry {
                rank: 0,
                text: text.to_string(),
            },
        );
        if self.entries.len() > HISTORY_LIMIT {
            let mut victim = None;
            let mut min = i64::MAX;
            // 从后往前找严格更小的：并列自然落在靠后的那一条上。
            for (i, e) in self.entries.iter().enumerate().rev() {
                if e.rank < min {
                    min = e.rank;
                    victim = Some(i);
                }
            }
            if let Some(i) = victim {
                self.entries.remove(i);
            }
        }
        true
    }

    pub fn clear(&mut self) -> usize {
        let n = self.entries.len();
        self.entries.clear();
        n
    }

    /// 读盘之后再过一遍：丢掉空文本、按上限截断、删掉重复（同一串只留最靠前的那条）。
    /// 手改过文件也不至于让下拉框里出现两行一样的。
    pub fn sanitize(&mut self) {
        let mut seen: Vec<String> = Vec::new();
        self.entries.retain(|e| {
            let t = e.text.trim().to_string();
            if t.is_empty() || seen.iter().any(|s| s == &t) {
                return false;
            }
            seen.push(t);
            true
        });
        for e in &mut self.entries {
            e.text = e.text.trim().to_string();
        }
        self.entries.truncate(HISTORY_LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        Snapshot {
            clipboard: "剪贴板 内容".to_string(),
            window_id: "123456".to_string(),
            window_title: "记事本 - a.txt".to_string(),
            window_class: "Notepad".to_string(),
        }
    }

    #[test]
    fn defaults_to_none_so_old_lists_keep_launching() {
        assert_eq!(ParamType::default(), ParamType::None);
        let v: ParamType = serde_json::from_str("\"none\"").unwrap();
        assert_eq!(v, ParamType::None);
        assert_eq!(
            serde_json::to_string(&ParamType::UrlQuery).unwrap(),
            "\"url_query\""
        );
    }

    #[test]
    fn markers_expand_one_by_one() {
        let s = snap();
        assert_eq!(
            expand_template("x {%p} {%c} {%wd} {%wt} {%wc}", ParamType::NoEncoding, &s, "参数"),
            "x 参数 剪贴板 内容 123456 记事本 - a.txt Notepad"
        );
    }

    #[test]
    fn none_type_never_touches_anything() {
        assert_eq!(
            expand_template("{%p}{%wt}", ParamType::None, &snap(), "参数"),
            "{%p}{%wt}"
        );
        assert_eq!(
            expand_command("a{%c}", "b", ParamType::None, &snap(), "p"),
            ("a{%c}".to_string(), "b".to_string())
        );
    }

    /// 证伪实验：把所有能想到的"注入"都塞进参数，看有没有被二次展开。
    /// 若把 [`expand_template`] 写成连续 `replace`，这一条会立刻红。
    #[test]
    fn injected_marker_text_is_not_expanded_again() {
        let s = snap();
        let out = expand_template(
            "cmd {%p}",
            ParamType::NoEncoding,
            &s,
            "{%c}{%wt}{%p}",
        );
        assert_eq!(out, "cmd {%c}{%wt}{%p}");
        // URL 档会把这些标记的字节编掉，但同样不能变成剪贴板内容。
        let out = expand_template("q={%p}", ParamType::UrlQuery, &s, "{%c}");
        assert_eq!(out, "q=%7B%25c%7D");
    }

    #[test]
    fn unknown_braces_stay_literal() {
        assert_eq!(
            expand_template("{%x} {%", ParamType::NoEncoding, &snap(), "p"),
            "{%x} {%"
        );
    }

    #[test]
    fn query_encode_keeps_the_delphi_safe_set_plus_digits() {
        assert_eq!(query_encode("abcXYZ019*@._-~"), "abcXYZ019*@._-~");
        assert_eq!(query_encode("a b"), "a+b");
        assert_eq!(query_encode("a/b?c=d&e"), "a%2Fb%3Fc%3Dd%26e");
        assert_eq!(query_encode("中"), "%E4%B8%AD");
        // 这条钉的正是与 Delphi 的差异：数字不许被编。
        assert_ne!(query_encode("123"), "%31%32%33");
    }

    #[test]
    fn url_and_utf8_encodings_agree_on_utf8_bytes() {
        let s = snap();
        let a = expand_template("q={%p}", ParamType::UrlQuery, &s, "中文 词");
        let b = expand_template("q={%p}", ParamType::Utf8Query, &s, "中文 词");
        assert_eq!(a, b);
        assert_eq!(a, "q=%E4%B8%AD%E6%96%87+%E8%AF%8D");
    }

    #[test]
    fn no_marker_appends_per_delphi() {
        let s = snap();
        // URL 两档拼在命令行尾。
        assert_eq!(
            expand_command("https://x/s?wd=", "", ParamType::UrlQuery, &s, "a b"),
            ("https://x/s?wd=a+b".to_string(), String::new())
        );
        // 不编码那档并进参数；原来没有参数时不留前导空格。
        assert_eq!(
            expand_command("notepad.exe", "", ParamType::NoEncoding, &s, r"C:\a b.txt"),
            ("notepad.exe".to_string(), r"C:\a b.txt".to_string())
        );
        assert_eq!(
            expand_command("cmd.exe", "/c echo", ParamType::NoEncoding, &s, "x"),
            ("cmd.exe".to_string(), "/c echo x".to_string())
        );
    }

    #[test]
    fn prompt_decision_matches_delphi() {
        assert!(!needs_prompt("a {%p}", "", ParamType::None));
        assert!(needs_prompt("a {%p}", "", ParamType::NoEncoding));
        assert!(!needs_prompt("a {%wt}", "", ParamType::UrlQuery));
        // 一个标记都没有却选了编码类型：Delphi 也弹框，然后把参数追加进去。
        assert!(needs_prompt("notepad.exe", "", ParamType::NoEncoding));
        assert!(!needs_prompt("notepad.exe", "", ParamType::None));
    }

    #[test]
    fn history_new_entry_goes_first_with_rank_zero() {
        let mut h = ParamHistory::default();
        assert!(h.touch("a"));
        assert!(h.touch("b"));
        assert_eq!(h.texts().collect::<Vec<_>>(), vec!["b", "a"]);
        assert_eq!(h.entries[0].rank, 0);
        assert!(!h.touch("   "), "空串不记");
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn history_reuse_bumps_rank_without_moving() {
        let mut h = ParamHistory::default();
        h.touch("a");
        h.touch("b");
        assert!(h.touch("a"));
        assert_eq!(h.texts().collect::<Vec<_>>(), vec!["b", "a"]);
        assert_eq!(h.entries[1].rank, 1);
        // 区分大小写（Delphi 的 Items.IndexOf 同口径）
        h.touch("A");
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn history_caps_at_fifty_and_drops_the_least_used() {
        let mut h = ParamHistory::default();
        for i in 0..HISTORY_LIMIT {
            h.touch(&format!("p{i}"));
        }
        assert_eq!(h.len(), HISTORY_LIMIT);
        // 把 p0 用一次 → 它的 rank 变成 1，成了"最不常用"的变成 p1（rank 0 里靠后的那个）……
        // 注意头插：后进的在前，所以 p1 在列表末尾附近；判定只看 rank。
        h.touch("p0");
        h.touch("new");
        assert_eq!(h.len(), HISTORY_LIMIT);
        assert!(h.texts().any(|t| t == "new"));
        assert!(!h.texts().any(|t| t == "p1"), "rank 并列 0 时踢靠后的那条");
    }

    #[test]
    fn history_sanitize_drops_blank_and_duplicates() {
        let mut h = ParamHistory {
            entries: vec![
                ParamEntry { rank: 0, text: " a ".into() },
                ParamEntry { rank: 3, text: "a".into() },
                ParamEntry { rank: 0, text: "  ".into() },
                ParamEntry { rank: 1, text: "b".into() },
            ],
        };
        h.sanitize();
        assert_eq!(h.texts().collect::<Vec<_>>(), vec!["a", "b"]);
    }

    #[test]
    fn history_round_trips_through_json() {
        let mut h = ParamHistory::default();
        h.touch("中文 参数");
        let json = serde_json::to_string(&h).unwrap();
        let back: ParamHistory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, h);
        // 空文件/缺字段落到空表
        assert!(ParamHistory::default().is_empty());
        let empty: ParamHistory = serde_json::from_str("{}").unwrap();
        assert!(empty.is_empty());
    }
}

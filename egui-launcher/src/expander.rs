//! FR-SRC-08：在任意程序里敲 `;keyword` 自动换成片段正文。
//!
//! 与 AHK 版 `SnippetExpander.ahk` 同一个口径：
//! · 缩写 = 前缀 + 关键字，默认前缀 `;`；不区分大小写；不需要结束符、紧贴别的字也触发（`*?`）；
//! · 三处不展开——本程序自己的窗口、密码框、用户写的排除名单；
//! · 改完片段立即重注册（这一层只负责把表算出来，重装钩子是 `main.rs` 的事）。
//!
//! 这一层只有"纯逻辑"：怎么算一条缩写、怎么匹配、怎么判排除与密码框。
//! 钩子、退格、粘贴、线程归属全在 `main.rs`——低级键盘钩子的回调只能跑在装钩子的那条线程。
//!
//! 隐私口径（FR-SYS-03）：缩写、关键字、正文、窗口标题都是用户内容，**一个字都不落日志**，
//! 日志只记条数与字符数。

use crate::source::snippet::{MAX_ABBR_CHARS, Snippet};

/// 出厂前缀（需求书 4.2.2 的 `;keyword`）。
pub const DEFAULT_PREFIX: &str = ";";

/// 出厂排除名单。三条与 AHK 版 `Features.Snippets.ExpandExclude` 的缺省一字不差：
/// 远程桌面里敲的字属于那头那台机器，密码管理器里敲的字更不该被任何常驻程序看见。
pub const DEFAULT_EXCLUDE: &str = "ahk_exe mstsc.exe, ahk_exe KeePass.exe, ahk_exe KeePassXC.exe";

/// 打字缓冲最多记几个字符。到顶就从最前面丢——缩写最长 40，缓冲只需要装得下"最近打的一段"。
/// 不做无上限：用户整天不触发的话，这条缓冲就是一份键盘记录。
pub const BUFFER_CHARS: usize = 64;

/// Win32 `ES_PASSWORD`。`Edit` / `RichEdit` 的密码位就是这一位。
pub const ES_PASSWORD: u32 = 0x0020;

/// 一条可触发的缩写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Abbr {
    /// 小写后的缩写（前缀 + 关键字），匹配用。
    pub key: String,
    /// 原样大小写，只用于数"该退几格"。
    pub display: String,
    /// 片段正文（占位符留给 `snippet::expand`，这里原样存）。
    pub body: String,
}

/// 一条片段能不能自动展开；能 ⇒ 返回它该用的缩写（前缀 + 关键字）。
///
/// 拒绝的情形都与 AHK 的 `Abbreviation()` 一一对应：没开自动展开、关键字为空、
/// 带空白或反引号（缩写在任意程序里触发，反引号在 AHK 里本来就是转义符）、前缀 + 关键字超 40 字。
/// 另加一条本仓自己的：正文为空的片段不展开——"把缩写删掉、什么都不留"不是用户想要的。
pub fn abbreviation(prefix: &str, s: &Snippet) -> Option<String> {
    if !s.auto_expand {
        return None;
    }
    let kw = s.keyword.trim();
    if kw.is_empty() || kw.chars().any(char::is_whitespace) || kw.contains('`') {
        return None;
    }
    if s.text.trim().is_empty() {
        return None;
    }
    let p = prefix.trim();
    if p.is_empty() || p.chars().any(char::is_whitespace) || p.contains('`') {
        return None;
    }
    let abbr = format!("{p}{kw}");
    if abbr.chars().count() > MAX_ABBR_CHARS {
        return None;
    }
    Some(abbr)
}

/// 整张表。同一条缩写（不区分大小写）只认第一条——两条片段撞缩写时，用户能改的是关键字，
/// 不该由后台列表顺序替他决定今天是哪一条。
pub fn build(list: &[Snippet], prefix: &str) -> Vec<Abbr> {
    let mut out: Vec<Abbr> = Vec::new();
    for s in list {
        let Some(display) = abbreviation(prefix, s) else {
            continue;
        };
        let key = display.to_lowercase();
        if out.iter().any(|a| a.key == key) {
            continue;
        }
        out.push(Abbr {
            key,
            display,
            body: s.text.clone(),
        });
    }
    out
}

/// 最近敲过的字。钩子线程独占（与双击判定的 `TapTracker` 同一口径：写成 `Mutex` 只是因为
/// `static` 要 `Sync`，实际没有并发访问）。
#[derive(Debug, Default, Clone)]
pub struct TypeBuffer {
    chars: Vec<char>,
}

impl TypeBuffer {
    pub const fn new() -> Self {
        Self { chars: Vec::new() }
    }

    pub fn clear(&mut self) {
        self.chars.clear();
    }

    /// 收一个字。到顶先从最前面丢一个——留下的永远是"最近打的那一段"。
    pub fn push(&mut self, ch: char) {
        if self.chars.len() >= BUFFER_CHARS {
            self.chars.remove(0);
        }
        self.chars.push(ch);
    }

    /// 目标窗口收到一次 Backspace。返回 `false` = 缓冲本来就空（调用方不必据此做事）。
    pub fn backspace(&mut self) -> bool {
        self.chars.pop().is_some()
    }

    /// 命中的缩写下标：**最长**的那条后缀，不区分大小写、不限词首（AHK 的 `?`）。
    ///
    /// 用后缀而不是"从头全等"：用户可能在别的词后面直接敲 `;sig`，那一段也得认。
    pub fn matched(&self, abbrs: &[Abbr]) -> Option<usize> {
        if self.chars.is_empty() {
            return None;
        }
        let typed: String = self.chars.iter().flat_map(|c| c.to_lowercase()).collect();
        let mut best: Option<(usize, usize)> = None;
        for (i, a) in abbrs.iter().enumerate() {
            let n = a.key.chars().count();
            if n == 0 || n > typed.chars().count() || !typed.ends_with(&a.key) {
                continue;
            }
            // 长的优先；同长先到先得（`build` 已去重，这里只防前缀相同的两条，如 `;a` 与 `;ab`）。
            if best.is_none_or(|(_, bn)| n > bn) {
                best = Some((i, n));
            }
        }
        best.map(|(i, _)| i)
    }
}

/// 会移动光标或提交输入的键：缓冲到这里就作废（再往回拼一个字已经不是用户打的那一段了）。
/// 值取自 Win32 `VK_*`：Enter / Tab / Esc / Insert / Delete / Home / End / PgUp / PgDn / 四个方向。
pub fn is_break_key(vk: u32) -> bool {
    matches!(
        vk,
        0x0D | 0x09 | 0x1B | 0x2D | 0x2E | 0x24 | 0x23 | 0x21 | 0x22 | 0x25 | 0x26 | 0x27 | 0x28
    )
}

/// 判断排除用的窗口事实。三个字段都是"现场取的"，不认识就传空串。
pub struct WindowFacts<'a> {
    /// exe 全路径（取不到就空）。
    pub exe: &'a str,
    pub class: &'a str,
    pub title: &'a str,
}

/// 一条排除规则。写法照 AHK 的 `ExpandExclude`：逗号分隔，`ahk_exe x.exe` / `ahk_class y` /
/// 其余整条按标题子串。三条都**不区分大小写**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExcludeRule {
    /// 进程名或 exe 全路径。
    Exe(String),
    /// 窗口类名（全等）。
    Class(String),
    /// 标题子串。
    Title(String),
}

/// 把设置页那一格拆成规则表。空项、认不出的写法（`ahk_exe ` 后面空着）直接跳过。
///
/// 指令字之后必须跟空白（或就到头）：`ahk_exe` 单独一条是"用户删剩了指令名"，跳过；
/// `ahk_exename` 不是指令，照整条按标题子串处理。
pub fn parse_exclude(spec: &str) -> Vec<ExcludeRule> {
    // 拆词时已经 trim 过，直接 `strip_prefix("ahk_exe ")` 永远匹配不到"后面空着"那种。
    fn directive<'a>(item: &'a str, head: &str) -> Option<&'a str> {
        let rest = item.strip_prefix(head)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then_some(rest.trim())
    }
    spec.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|item| {
            let lower = item.to_lowercase();
            if let Some(rest) = directive(&lower, "ahk_exe") {
                return (!rest.is_empty()).then(|| ExcludeRule::Exe(rest.to_string()));
            }
            if let Some(rest) = directive(&lower, "ahk_class") {
                return (!rest.is_empty()).then(|| ExcludeRule::Class(rest.to_string()));
            }
            Some(ExcludeRule::Title(lower))
        })
        .collect()
}

/// 这个窗口该不该跳过。`exe` 既比全路径也比文件名——用户写 `ahk_exe KeePass.exe` 时不该
/// 要求他把安装路径也抄上。
pub fn is_excluded(rules: &[ExcludeRule], facts: &WindowFacts<'_>) -> bool {
    if rules.is_empty() {
        return false;
    }
    let exe = facts.exe.to_lowercase();
    let exe_name = exe
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_string();
    let class = facts.class.to_lowercase();
    let title = facts.title.to_lowercase();
    rules.iter().any(|r| match r {
        ExcludeRule::Exe(want) => exe == *want || exe_name == *want,
        ExcludeRule::Class(want) => class == *want,
        ExcludeRule::Title(want) => title.contains(want.as_str()),
    })
}

/// 焦点控件是不是密码框：类名带 `Edit`（`Edit` / `RichEdit20W` / …）且带着 `ES_PASSWORD`。
///
/// 只认 Win32 的这两类。浏览器 / Electron 里的密码框在 Win32 眼里只是一个渲染窗口，
/// 看不出"这里面是密码"——AHK 版同样是这个盲区，靠排除名单兜底。
pub fn is_password_control(class: &str, style: u32) -> bool {
    class.to_lowercase().contains("edit") && (style & ES_PASSWORD) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(name: &str, keyword: &str, text: &str, auto: bool) -> Snippet {
        Snippet {
            name: name.to_string(),
            keyword: keyword.to_string(),
            text: text.to_string(),
            auto_expand: auto,
        }
    }

    #[test]
    fn abbreviation_needs_auto_expand_keyword_and_body() {
        assert_eq!(
            abbreviation(";", &snip("今天", "today", "{date}", true)),
            Some(";today".to_string())
        );
        assert_eq!(
            abbreviation(";", &snip("今天", "today", "{date}", false)),
            None,
            "没勾自动展开的片段一条都不进表"
        );
        assert_eq!(
            abbreviation(";", &snip("今天", "", "{date}", true)),
            None,
            "没有关键字就没法触发"
        );
        assert_eq!(
            abbreviation(";", &snip("今天", "to day", "{date}", true)),
            None,
            "关键字带空格（AHK 同一条）"
        );
        assert_eq!(
            abbreviation(";", &snip("今天", "today", "   ", true)),
            None,
            "正文为空 = 把缩写删掉什么都不留，不做"
        );
        assert_eq!(abbreviation("", &snip("今天", "today", "{date}", true)), None);
    }

    #[test]
    fn abbreviation_respects_the_forty_char_cap() {
        // 上限是「前缀 + 关键字」40 字：`;` + 39 = 40 正好可用。
        let kw = "k".repeat(39);
        assert_eq!(
            abbreviation(";", &snip("x", &kw, "body", true)),
            Some(format!(";{kw}"))
        );
        let kw = "k".repeat(40);
        assert_eq!(
            abbreviation(";", &snip("x", &kw, "body", true)),
            None,
            "前缀 + 关键字 41 字，超需求书 4.2.2 的上限"
        );
        // 前缀长一样算总数（不是只数关键字）。
        assert_eq!(
            abbreviation(";;", &snip("x", &"k".repeat(39), "body", true)),
            None
        );
    }

    #[test]
    fn build_keeps_the_first_of_duplicate_keys() {
        let list = vec![
            snip("A", "sig", "first", true),
            snip("B", "SIG", "second", true),
            snip("C", "mail", "third", false),
        ];
        let abbrs = build(&list, ";");
        assert_eq!(abbrs.len(), 1);
        assert_eq!(abbrs[0].body, "first");
        assert_eq!(abbrs[0].key, ";sig", "key 一律小写");
        assert_eq!(abbrs[0].display, ";sig");
    }

    #[test]
    fn buffer_matches_longest_suffix_case_insensitively() {
        let abbrs = build(
            &[
                snip("a", "a", "A", true),
                snip("ab", "ab", "AB", true),
                snip("sig", "sig", "SIG", true),
            ],
            ";",
        );
        let mut b = TypeBuffer::new();
        for ch in ";a".chars() {
            b.push(ch);
        }
        assert_eq!(b.matched(&abbrs), Some(0));
        b.push('b');
        assert_eq!(b.matched(&abbrs), Some(1), "长的那条优先");
        let mut b = TypeBuffer::new();
        for ch in ";SIG".chars() {
            b.push(ch);
        }
        assert_eq!(b.matched(&abbrs), Some(2), "大小写不敏感");
    }

    #[test]
    fn buffer_matches_in_the_middle_of_a_word() {
        let abbrs = build(&[snip("sig", "sig", "SIG", true)], ";");
        let mut b = TypeBuffer::new();
        for ch in "hello;sig".chars() {
            b.push(ch);
        }
        assert_eq!(b.matched(&abbrs), Some(0), "紧贴别的字也要触发（AHK 的 ?）");
        let mut b = TypeBuffer::new();
        for ch in "hello".chars() {
            b.push(ch);
        }
        assert_eq!(b.matched(&abbrs), None);
    }

    #[test]
    fn buffer_backspaces_and_trims_from_the_front() {
        let mut b = TypeBuffer::new();
        assert!(!b.backspace(), "空缓冲退格不算数");
        for ch in "abcdef".chars() {
            b.push(ch);
        }
        assert!(b.backspace());
        assert_eq!(b.chars.len(), 5);
        let mut b = TypeBuffer::new();
        for i in 0..BUFFER_CHARS + 10 {
            b.push(char::from_u32(0x4E00 + i as u32).unwrap());
        }
        assert_eq!(b.chars.len(), BUFFER_CHARS, "到顶只留最近的一段");
        assert_eq!(
            b.chars.first().copied(),
            char::from_u32(0x4E00 + 10),
            "丢的是最前面那 10 个"
        );
    }

    #[test]
    fn exclude_spec_parses_three_forms_and_ignores_junk() {
        let rules = parse_exclude(
            " ahk_exe mstsc.exe , ahk_class #32770,银行\nahk_exe , ,",
        );
        assert_eq!(
            rules,
            vec![
                ExcludeRule::Exe("mstsc.exe".to_string()),
                ExcludeRule::Class("#32770".to_string()),
                ExcludeRule::Title("银行".to_string()),
            ]
        );
        assert!(parse_exclude("  ").is_empty());
        // 指令字后面必须跟空白：`ahk_exex.dev` 不是指令，整条按标题子串处理；
        // 而只剩指令名的 `ahk_exe`（上面那串里那条）跳过、不能变出一条标题规则。
        assert_eq!(
            parse_exclude("ahk_exex.dev"),
            vec![ExcludeRule::Title("ahk_exex.dev".to_string())]
        );
    }

    #[test]
    fn exclude_matches_exe_name_class_and_title() {
        let rules = parse_exclude(DEFAULT_EXCLUDE);
        let facts = WindowFacts {
            exe: r"C:\Program Files\KeePass\KeePass.exe",
            class: "WindowsForms10.Window.8.app.0",
            title: "KeePass",
        };
        assert!(is_excluded(&rules, &facts), "全路径 + 文件名两种写法都认");
        let facts = WindowFacts {
            exe: "",
            class: "#32770",
            title: "银行插件登录",
        };
        assert!(!is_excluded(&rules, &facts), "不在名单里的窗口不拦");
        let rules = parse_exclude("银行");
        assert!(is_excluded(&rules, &facts), "整条按标题子串，不区分大小写");
        let rules = parse_exclude("ahk_class #32770");
        assert!(is_excluded(&rules, &facts));
        assert!(!is_excluded(&[], &facts), "空名单不拦任何窗口");
    }

    #[test]
    fn password_control_needs_edit_class_and_the_style_bit() {
        assert!(is_password_control("Edit", ES_PASSWORD));
        assert!(is_password_control("RichEdit20W", ES_PASSWORD | 0x0004));
        assert!(!is_password_control("Edit", 0x0004), "普通输入框没有 0x20");
        assert!(!is_password_control("Button", ES_PASSWORD), "不是编辑框");
        assert!(!is_password_control("Chrome_RenderWidgetHostHWND", ES_PASSWORD));
    }

    #[test]
    fn break_keys_cover_navigation_and_commit() {
        for vk in [0x0D, 0x09, 0x1B, 0x2E, 0x25, 0x28] {
            assert!(is_break_key(vk), "0x{vk:02X} 应当作废缓冲");
        }
        for vk in [0x41, 0x20, 0x08, 0x10] {
            assert!(!is_break_key(vk), "0x{vk:02X} 不该作废缓冲");
        }
    }
}

//! FR-SRC-05 自定义命令源：关键字独占、`{query}` 参数填充、无参提示行。
//!
//! 纯函数模块：不碰窗口、不读设置、不落日志。`src/main.rs` 只拿这里的产物去画行与启动。
//!
//! 三条本模块自己定的口径（需求原文没钉，登记进发布说明）：
//! - **关键字精确相等才算触发**：`ping` 不会因为 `pin` 而独占；带空格（`ping `）走
//!   [`exclusive`] 的独占视图，不带空格（`ping`）只在列表头段加一条补全提示行，
//!   不清空其它结果——否则用户每打到某个关键字的**前缀**时整张列表都会闪一下。
//! - **重复关键字只取配置数组里先出现的一条**（需求书 TC-SRC 05-3）。学习别名与手填别名
//!   同在一张词表里，判定口径与匹配层一致（空白分词、大小写不敏感）。
//! - **Url 类型才百分号编码**：`{query}` 填进命令行参数时必须原样（`ping 10.0.0.1`
//!   不能变成 `10.0.0.1` 的编码串），填进 URL 时才编码——复用 FR-SRC-11 的模板语法。

use crate::source::application::{is_url, ItemKind, LaunchItem};
use crate::source::websearch;

/// 模板里是否含 `{query}` 或 `{query|默认值}`。路径四态校验要靠它决定「跳过检查」
/// （含占位符的路径在填充前不是一个真实存在的路径，不能判 Not found）。
pub fn has_query_placeholder(s: &str) -> bool {
    let mut rest = s;
    while let Some(pos) = rest.find('{') {
        let after = &rest[pos + 1..];
        match after.find('}') {
            Some(end) => {
                let inner = &after[..end];
                if inner == "query" || inner.starts_with("query|") {
                    return true;
                }
                rest = &after[end + 1..];
            }
            None => rest = after,
        }
    }
    false
}

/// 出厂条目「启动器目录」用的占位符。清单里存占位符而不是绝对路径，便携才成立：
/// 程序换到别的文件夹后这条自己跟着走，不用重新播种（2026-10-21 用户点名要求）。
pub const LAUNCHER_DIR_PLACEHOLDER: &str = "%LAUNCHER_DIR%";

/// 展开 `%VAR%`（Windows 口径，`std::env` 在 Windows 上本就大小写不敏感）。
/// 未定义的变量原样保留——与 `cmd` 的行为一致，也让用户一眼看出自己写错了哪一个。
///
/// `%LAUNCHER_DIR%` 不是环境变量，由本程序就地解析成**当前 exe 所在目录**：
/// 每次展开都重新取，所以搬家之后不需要改清单。
pub fn expand_env(s: &str) -> String {
    expand_env_with(s, |name| {
        if name.eq_ignore_ascii_case("LAUNCHER_DIR") {
            return std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.as_os_str().to_os_string()));
        }
        std::env::var_os(name)
    })
}

/// 可注入查表的版本（单测用）。**在填 `{query}` 之前调用**：这样用户敲进查询框的
/// 字面 `%PATH%` 不会被当成变量展开。
fn expand_env_with<F>(s: &str, lookup: F) -> String
where
    F: Fn(&str) -> Option<std::ffi::OsString>,
{
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('%') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match lookup(name) {
                    Some(v) => out.push_str(&v.to_string_lossy()),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            // `%%` 或孤零零一个 `%`：当普通字符。
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 一条可执行的自定义命令行。`entry` 是**填好 `{query}` 之后的副本**，
/// 原条目仍在清单里不动；`uid` 保留原条目身份（学习统计与路径状态都按它查）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomHit {
    pub uid: String,
    pub name: String,
    pub entry: LaunchItem,
    pub subtitle: String,
}

/// 需求书 FR-SRC-05 那条 `valid=false` 的行：**不可执行**，`completion` 给出按 Enter
/// 之后该补成什么（`"ping "`）。没有可执行字段，类型上就给不出「误按就跑」的可能。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomHint {
    pub uid: String,
    pub title: String,
    pub subtitle: String,
    pub completion: String,
    /// 条目配置的原始命令行（`path` + `arguments`，`{query}` 保持未填的样子）。
    /// 提示行原本只报"需要参数"，不报它是从哪一条算出来的 ⇒ 用户会以为清单里
    /// 另有一条同名命令（2026-10-21 真人反馈）。
    pub command: String,
}

/// 关键字独占这一格该出什么行（形态与 [`websearch::Exclusive`] 同构）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exclusive {
    /// `kw 词`：只出这一条（需求书的分值 150，压在应用行之前）。
    Rows(Vec<CustomHit>),
    /// `kw ` 但命令还要一个词 ⇒ 只有补全提示行。
    Hints(Vec<CustomHint>),
    /// 首词不是任何条目的关键字 ⇒ 本源不参与。
    NotOurs,
}

/// 这条命令是否要靠用户再输入一段（`path` 或 `arguments` 里有 `{query}`）。
pub fn needs_query(item: &LaunchItem) -> bool {
    has_query_placeholder(&item.path) || has_query_placeholder(&item.arguments)
}

/// 在条目的别名词表里找出与 `typed` 精确相等的那一个（返回条目里存的原样大小写，
/// 补全出去的是用户自己配置的写法，不是他这次敲进来的大小写）。
fn keyword_word<'a>(item: &'a LaunchItem, typed: &str) -> Option<&'a str> {
    item.keyword.split_whitespace().find(|w| w.eq_ignore_ascii_case(typed))
}

/// 按配置数组顺序找第一个关键字精确命中的条目（TC-SRC 05-3：重复只取先出现的一条）。
pub fn find_by_keyword<'a>(items: &'a [LaunchItem], keyword: &str) -> Option<&'a LaunchItem> {
    if keyword.trim().is_empty() {
        return None;
    }
    items.iter().find(|it| keyword_word(it, keyword.trim()).is_some())
}

fn hint_for(item: &LaunchItem, typed: &str) -> CustomHint {
    let kw = keyword_word(item, typed).unwrap_or(typed);
    CustomHint {
        uid: item.uid(),
        title: format!("{}（需要参数）", item.name),
        subtitle: format!("在 '{kw} ' 后输入，然后回车"),
        completion: format!("{kw} "),
        command: [item.path.as_str(), item.arguments.as_str()]
            .join(" ")
            .trim()
            .to_string(),
    }
}

/// `kw 词` 的独占入口。判据链：`has_rest` → 首词命中条目 → 填模板。
/// 填不进去（必需占位符 + 空词）就退成提示行——两种结果天然互斥，不必再单独判空词。
pub fn exclusive(items: &[LaunchItem], raw: &str) -> Exclusive {
    let p = crate::matching::parse(raw);
    // 没有空格就没有独占：`g` 还可能是 `git` 的开头（与 FR-SRC-11 同一条已入档判据）。
    if !p.has_rest || p.keyword.is_empty() {
        return Exclusive::NotOurs;
    }
    let Some(item) = find_by_keyword(items, &p.keyword) else {
        return Exclusive::NotOurs;
    };
    // 需求书把独占限定在「`{query}` 时只出 1 条」。没有占位符的条目吃掉参数，结果是
    // 后半截被静默丢掉（`np 某个文件` 变成直接开记事本）⇒ 这类交回普通搜索。
    if !needs_query(item) {
        return Exclusive::NotOurs;
    }
    match build_hit(item, p.rest.trim()) {
        Some(hit) => Exclusive::Rows(vec![hit]),
        None => Exclusive::Hints(vec![hint_for(item, &p.keyword)]),
    }
}

/// `kw`（不带空格、精确等于关键字）时列表头段的补全提示行。
/// 只给「还缺一个参数」的条目出这一行：不缺参数的条目本来就在结果里，回车直接跑。
pub fn keyword_hint(items: &[LaunchItem], raw: &str) -> Option<CustomHint> {
    let p = crate::matching::parse(raw);
    if p.has_rest || p.keyword.is_empty() {
        return None;
    }
    let item = find_by_keyword(items, &p.keyword)?;
    needs_query(item).then(|| hint_for(item, &p.keyword))
}

/// 把 `query` 填进路径与参数，得到可直接交给 `ShellExecuteExW` 的条目副本。
/// 返回 `None` = 必需占位符遇到空词（需求书的 `valid=false`）。
fn build_hit(item: &LaunchItem, query: &str) -> Option<CustomHit> {
    let encode = item.kind == ItemKind::Url || is_url(&item.path);
    let fill = |tpl: &str| -> Option<String> {
        if tpl.trim().is_empty() {
            return Some(tpl.to_string());
        }
        // 先展开环境变量、后填查询词：用户敲的 `%PATH%` 是文本，不是变量引用。
        let expanded = expand_env(tpl);
        websearch::fill_template_with(&expanded, query, encode)
    };
    let path = fill(&item.path)?;
    let arguments = fill(&item.arguments)?;
    let working_dir = match &item.working_dir {
        Some(d) => Some(fill(d)?),
        None => None,
    };
    let mut entry = item.clone();
    entry.path = path;
    entry.arguments = arguments;
    entry.working_dir = working_dir;
    // 走到这里必定是含 `{query}` 的条目——[`exclusive`] 已把没有占位符的挡在外面。
    let subtitle = "回车运行（参数已填入）".to_string();
    Some(CustomHit { uid: item.uid(), name: item.name.clone(), entry, subtitle })
}

/// 设置页 FR-SRC-05 的「关键字唯一性」提示：返回 `(重复的词, 撞名的条目名)`。
/// 只提示、不阻断（需求书明写「不阻断启动，避免影响 S3」）；同名取先出现的一条由
/// [`find_by_keyword`] 保证。空关键字不参与。
pub fn duplicate_keywords(items: &[LaunchItem]) -> Vec<(String, Vec<String>)> {
    let mut map: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    for item in items {
        for word in item.keyword.split_whitespace() {
            map.entry(word.to_lowercase()).or_default().push(item.name.clone());
        }
    }
    map.into_iter().filter(|(_, names)| names.len() > 1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, keyword: &str, kind: ItemKind, path: &str, args: &str) -> LaunchItem {
        LaunchItem {
            name: name.to_string(),
            keyword: keyword.to_string(),
            kind,
            path: path.to_string(),
            arguments: args.to_string(),
            ..Default::default()
        }
    }

    fn ping() -> LaunchItem {
        item("Ping", "ping", ItemKind::Command, "cmd.exe", "/k ping {query}")
    }

    #[test]
    fn placeholder_detection_matches_both_forms() {
        assert!(has_query_placeholder("/k ping {query}"));
        assert!(has_query_placeholder("http://localhost:{query|8000}"));
        assert!(!has_query_placeholder("/k ping"));
        assert!(!has_query_placeholder("{query-typo}"));
        assert!(!has_query_placeholder("%USERPROFILE%\\Documents"));
    }

    #[test]
    fn exclusive_needs_a_query_placeholder() {
        // 需求书把独占限定在「`{query}` 时」：没有占位符的条目吃掉后半截，等于把用户
        // 敲的字静默扔掉（`np 报告.txt` → 直接开一个空白记事本）。
        let items = vec![item("记事本", "jsb np", ItemKind::Command, "notepad.exe", "")];
        assert_eq!(exclusive(&items, "np 报告.txt"), Exclusive::NotOurs);
        assert_eq!(exclusive(&items, "np "), Exclusive::NotOurs);
        // 同一张表里带占位符的那条照旧独占。
        let items = vec![ping()];
        assert!(matches!(exclusive(&items, "ping 1.1.1.1"), Exclusive::Rows(_)));
    }

    #[test]
    fn env_expansion_leaves_unknown_vars_alone() {
        let out = expand_env_with(r"%ME%\Docs\{query}", |name| {
            (name == "ME").then(|| std::ffi::OsString::from(r"C:\Users\t"))
        });
        assert_eq!(out, r"C:\Users\t\Docs\{query}");
        let out = expand_env_with("%NOPE%\\x", |_| None);
        assert_eq!(out, "%NOPE%\\x");
        assert_eq!(expand_env_with("100%", |_| None), "100%");
    }

    #[test]
    fn exclusive_needs_a_trailing_space() {
        let items = vec![ping()];
        assert_eq!(exclusive(&items, "ping"), Exclusive::NotOurs);
        assert_eq!(exclusive(&items, "pin 1"), Exclusive::NotOurs);
        let Exclusive::Rows(rows) = exclusive(&items, "ping 10.0.0.1") else {
            panic!("`ping 10.0.0.1` 应该是独占可执行行");
        };
        assert_eq!(rows.len(), 1, "独占只出 1 条");
        assert_eq!(rows[0].entry.arguments, "/k ping 10.0.0.1");
        assert_eq!(rows[0].entry.path, "cmd.exe");
        assert_eq!(rows[0].uid, ping().uid(), "身份仍按原条目（未填充）的 uid");
    }

    #[test]
    fn empty_rest_yields_a_hint_with_completion() {
        let items = vec![ping()];
        let Exclusive::Hints(hints) = exclusive(&items, "ping ") else {
            panic!("`ping ` 空词应出提示行");
        };
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].completion, "ping ");
        assert_eq!(hints[0].subtitle, "在 'ping ' 后输入，然后回车");
        assert_eq!(hints[0].command, "cmd.exe /k ping {query}", "提示行要报出它是哪一条命令");
        // 两个空格是同一个空词，不能因为空白差异变成可执行行。
        assert!(matches!(exclusive(&items, "ping   "), Exclusive::Hints(_)));
    }

    #[test]
    fn keyword_hint_only_for_commands_that_need_a_query() {
        let items = vec![ping(), item("画图", "ht", ItemKind::Command, "mspaint.exe", "")];
        let hint = keyword_hint(&items, "ping").expect("`ping` 应给补全提示");
        assert_eq!(hint.completion, "ping ");
        assert!(keyword_hint(&items, "pin").is_none(), "前缀不提示");
        assert!(keyword_hint(&items, "ping x").is_none(), "已带参数不提示");
        assert!(keyword_hint(&items, "ht").is_none(), "不需要参数的条目不提示");
    }

    #[test]
    fn url_query_is_percent_encoded_but_arguments_are_not() {
        let url = item("搜索", "s", ItemKind::Url, "https://x.test/?q={query}", "");
        let Exclusive::Rows(rows) = exclusive(&[url], "s 你 好") else { panic!() };
        assert_eq!(rows[0].entry.path, "https://x.test/?q=%E4%BD%A0%20%E5%A5%BD");

        let cmd = item("回显", "e", ItemKind::Command, "cmd.exe", "/c echo {query}");
        let Exclusive::Rows(rows) = exclusive(&[cmd], "e 你 好") else { panic!() };
        assert_eq!(rows[0].entry.arguments, "/c echo 你 好");
    }

    #[test]
    fn default_value_fills_an_empty_rest() {
        let local = item("本地", "lh", ItemKind::Url, "http://localhost:{query|8000}", "");
        let Exclusive::Rows(rows) = exclusive(&[local], "lh ") else {
            panic!("带默认值的模板在空词时应给出可执行行");
        };
        assert_eq!(rows[0].entry.path, "http://localhost:8000");
    }

    #[test]
    fn env_vars_expand_before_the_query_is_filled() {
        let docs = item("文档", "docs", ItemKind::Folder, "%USERPROFILE%\\Documents\\{query}", "");
        let Exclusive::Rows(rows) = exclusive(&[docs], "docs 报表") else {
            panic!("`docs 报表` 应是独占可执行行");
        };
        let profile = std::env::var("USERPROFILE").expect("Windows 上必有 USERPROFILE");
        assert_eq!(rows[0].entry.path, format!(r"{profile}\Documents\报表"));
    }

    #[test]
    fn duplicate_keyword_uses_the_first_item_only() {
        let items = vec![
            item("第一个", "dup", ItemKind::Command, "a.exe", "{query}"),
            item("第二个", "dup", ItemKind::Command, "b.exe", "{query}"),
        ];
        let Exclusive::Rows(rows) = exclusive(&items, "DUP x") else { panic!() };
        assert_eq!(rows[0].name, "第一个");
        assert_eq!(rows[0].entry.path, "a.exe");
        let dups = duplicate_keywords(&items);
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].0, "dup");
        assert_eq!(dups[0].1, vec!["第一个".to_string(), "第二个".to_string()]);
    }

    /// ~~`command_without_placeholder_still_runs_and_ignores_extra_words`：无占位符的条目
    /// 也照样独占，只在副标题写一句「额外输入被忽略」。~~
    /// 【2026-10-21 用户拍板改判：需求书 FR-SRC-05 的独占只针对「`{query}` 时」。
    /// 出厂集开始给普通命令配 `np`/`ipconfig` 这类关键字之后，这个口子会把用户敲的字
    /// 静默扔掉（`np 报告.txt` 变成直接开空白记事本）⇒ 无占位符一律不独占，交回普通搜索。
    /// 旧断言已删除，旧行为与改动理由留在这段注释里。】
    #[test]
    fn command_without_placeholder_never_takes_the_exclusive_view() {
        let paint = item("画图", "ht", ItemKind::Command, "mspaint.exe", "");
        assert_eq!(exclusive(&[paint.clone()], "ht 多打的词"), Exclusive::NotOurs);
        assert_eq!(exclusive(&[paint], "ht "), Exclusive::NotOurs);
    }
}

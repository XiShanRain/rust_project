//! FR-SRC-11 网页搜索：把「引擎关键字 + 词」填进 URL 模板，产出可以直接交给浏览器的行。
//!
//! 纯函数模块：不出网、不碰窗口、不读设置。`src/main.rs` 只拿这里的产物去画行与启动，
//! 判据（哪个词算哪个引擎、空词算不算有效、URL 怎么编码）全部锁在本模块的单测里。
//!
//! 三条本模块自己定的口径（需求原文没钉，已登记进发布说明）：
//! - **只有首词与某引擎 `keyword` 精确相等才算本源独占**。前缀命中（`goo`）不独占——
//!   那等于让「正在打一个应用名」的用户突然看不见应用结果。
//! - **默认值按原样插入、不编码**：`http://localhost:{query|8000}` 里的 `8000` 属于模板，
//!   不是用户敲进去的内容，编码它没有意义（真编码了也只是同一个串，但语义就错了）。
//! - **模板里没有 `{query}` 也出行**：那种条目本来就是一条网址书签，任何词都直接打开。

use serde::{Deserialize, Serialize};

/// 一条引擎。字段与需求书 FR-SRC-11 一一对应；`icon` 本批只存不显示（「下载站点图标」待定）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebEngine {
    pub id: String,
    pub keyword: String,
    pub title: String,
    /// URL 模板，`{query}` / `{query|默认值}` 由 [`fill_template`] 填充。
    pub url: String,
    #[serde(default)]
    pub icon: String,
}

/// 一条可执行的搜索结果行：按 Enter / 单击 / 数字键就用 `url` 走 `ShellExecuteExW`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub engine_id: String,
    pub title: String,
    pub subtitle: String,
    pub url: String,
}

/// 空词（`g `）时的补全提示行。**刻意没有 `url` 字段**：需求书规定空词 `valid=false`，
/// 类型层面就给不出可执行的行，比运行时再判一次「这行不许按」可靠。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineHint {
    pub engine_id: String,
    pub title: String,
    pub subtitle: String,
    pub completion: String,
}

/// 内置 9 个引擎（需求书 FR-SRC-11 点名的那九个，keyword 一列照抄）。
/// 维基百科按 zh 子域、Google 翻译按 `text=` —— 这两处是选型不是需求，写在发布说明里。
pub fn builtin_engines() -> Vec<WebEngine> {
    let e = |id: &str, keyword: &str, title: &str, url: &str| WebEngine {
        id: id.to_string(),
        keyword: keyword.to_string(),
        title: title.to_string(),
        url: url.to_string(),
        icon: String::new(),
    };
    vec![
        e("google", "g", "Google", "https://www.google.com/search?q={query}"),
        e("bing", "bing", "Bing", "https://www.bing.com/search?q={query}"),
        e("baidu", "bd", "百度", "https://www.baidu.com/s?wd={query}"),
        e("github", "gh", "GitHub", "https://github.com/search?q={query}"),
        e("wikipedia", "wiki", "维基百科", "https://zh.wikipedia.org/wiki/Special:Search?search={query}"),
        e("youtube", "yt", "YouTube", "https://www.youtube.com/results?search_query={query}"),
        e("taobao", "tb", "淘宝", "https://s.taobao.com/search?q={query}"),
        e("jd", "jd", "京东", "https://search.jd.com/Search?keyword={query}"),
        e("translate", "tr", "Google 翻译", "https://translate.google.com/?text={query}"),
    ]
}

/// 兜底序列（FR-QRY-08 的后半）。`files` 是还没落地的 FR-SRC-14 ⇒ [`fallback_rows`]
/// 遇到没有对应引擎的 id 就跳过，而不是整条兜底失效。
pub const FALLBACK_IDS: [&str; 3] = ["google", "files", "bing"];

/// RFC 3986 的 unreserved 之外全部按 UTF-8 字节百分号编码（大写十六进制）。
/// 空格按 `%20` 而不是 `+`：`+` 只在 `application/x-www-form-urlencoded` 的 query 里被解释成空格，
/// 而模板也可能填进路径段（`/wiki/{query}`），`%20` 两处都对。
pub fn percent_encode(s: &str) -> String {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~";
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        if UNRESERVED.contains(byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `{…}` 里那一段是不是查询占位符。用枚举而不是嵌套 `Option`：这两种「没有值」在
/// 空词时走的是完全不同的分支（一个报错、一个用默认值），让它们长得容易被混为一谈的写法
/// 迟早会写错一次。
enum Slot<'a> {
    /// `{query}`：空词就没有可插入的值 ⇒ 需求书那条 `valid=false`。
    Required,
    /// `{query|默认值}`：空词用默认值。
    WithDefault(&'a str),
    /// 别的占位符（`{id}` 之类）：原样保留，不参与填充。
    Foreign,
}

fn slot_of(inner: &str) -> Slot<'_> {
    if inner == "query" {
        Slot::Required
    } else if let Some(def) = inner.strip_prefix("query|") {
        Slot::WithDefault(def)
    } else {
        Slot::Foreign
    }
}

/// 把 `query` 填进模板的所有 `{query}` / `{query|默认值}`（网页搜索口径：百分号编码）。
/// 返回 `None` = 空词且某个占位符没有默认值 ⇒ 需求书那条 `valid=false`。
pub fn fill_template(url: &str, query: &str) -> Option<String> {
    fill_template_with(url, query, true)
}

/// 同上，但由调用方决定要不要编码。批次 U 的 FR-SRC-05 命令源复用这套 `{query}` 语法：
/// URL 目标要编码（空格→`%20`），命令行参数必须原样（`ping 10.0.0.1` 不能变成 `%20`）。
pub fn fill_template_with(template: &str, query: &str, encode: bool) -> Option<String> {
    let mut out = String::with_capacity(template.len() + query.len());
    let mut rest = template;
    while let Some(pos) = rest.find('{') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        match after.find('}').map(|end| (end, slot_of(&after[..end]))) {
            Some((end, Slot::Required)) => {
                if query.is_empty() {
                    return None;
                }
                if encode {
                    out.push_str(&percent_encode(query));
                } else {
                    out.push_str(query);
                }
                rest = &rest[pos + 1 + end + 1..];
            }
            Some((end, Slot::WithDefault(def))) => {
                // 默认值属于模板而不是用户输入 ⇒ 原样插入、不编码。
                if query.is_empty() {
                    out.push_str(def);
                } else if encode {
                    out.push_str(&percent_encode(query));
                } else {
                    out.push_str(query);
                }
                rest = &rest[pos + 1 + end + 1..];
            }
            Some((end, Slot::Foreign)) => {
                out.push('{');
                out.push_str(&after[..end + 1]);
                rest = &rest[pos + 1 + end + 1..];
            }
            // 花括号没闭合：当作普通字符，往后走一格。
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Some(out)
}

fn host_of(url: &str) -> &str {
    let after = url.find("://").map(|i| &url[i + 3..]).unwrap_or(url);
    after.split(['/', '?', '#']).next().unwrap_or(after)
}

/// 独占（首词 = 某引擎关键字）这一格该出什么行。三种结果做成一个类型而不是让调用方
/// 自己拼「先取搜索行、取不到再取提示行」——那样两条判据会各写一遍，迟早写歪。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exclusive {
    /// 有词、且模板填得进去 ⇒ 可执行行（分值 150，压在应用行之前）。
    Rows(Vec<SearchHit>),
    /// 空词、而模板必须要一个词 ⇒ 只有补全提示行（需求书那条 `valid=false`）。
    Hints(Vec<EngineHint>),
    /// 首词不是任何引擎的关键字 ⇒ 本源不参与，正常搜索照跑。
    NotOurs,
}

/// 独占行的唯一入口。判据链：首词精确命中（[`find`]）→ 模板填充（[`fill_template`]）；
/// 后者返回 `None` 只可能是「空词 + 该占位符没有默认值」这一种，于是 `Hints` 与 `Rows`
/// 天然互斥，不需要再单独判一次空词。
pub fn exclusive_rows(engines: &[WebEngine], keyword: &str, rest: &str) -> Exclusive {
    let Some(engine) = find(engines, keyword) else { return Exclusive::NotOurs };
    let word = rest.trim();
    let Some(url) = fill_template(&engine.url, word) else {
        return Exclusive::Hints(vec![EngineHint {
            engine_id: engine.id.clone(),
            title: format!("{}  {}", engine.keyword, engine.title),
            subtitle: "继续输入关键词，回车打开搜索结果".to_string(),
            completion: format!("{} ", engine.keyword),
        }]);
    };
    // 空词能走到这儿只可能是模板自带默认值（`http://localhost:{query|8000}`），
    // 这时标题不再重复那对空书名号。
    let title =
        if word.is_empty() { engine.title.clone() } else { format!("{}「{}」", engine.title, word) };
    Exclusive::Rows(vec![SearchHit {
        engine_id: engine.id.clone(),
        title,
        subtitle: format!("{} · 回车用默认浏览器打开", host_of(&url)),
        url,
    }])
}

/// 结果为空时的兜底行（FR-QRY-08）。查询词就是用户敲的整串；`FALLBACK_IDS` 里
/// 没有对应引擎的 id（现阶段只有 `files`）静默跳过。
pub fn fallback_rows(engines: &[WebEngine], query: &str) -> Vec<SearchHit> {
    let word = query.trim();
    if word.is_empty() {
        return Vec::new();
    }
    FALLBACK_IDS
        .iter()
        .filter_map(|id| engines.iter().find(|e| e.id == *id))
        .filter_map(|engine| {
            Some(SearchHit {
                engine_id: engine.id.clone(),
                title: format!("{}「{}」", engine.title, word),
                subtitle: format!("兜底搜索 · {}", host_of(&engine.url)),
                url: fill_template(&engine.url, word)?,
            })
        })
        .collect()
}

/// 关键字精确匹配（大小写不敏感）。独占判据也用它——判"是不是引擎关键字"和"是哪个引擎"
/// 必须是同一条口径，否则会出现"算独占却取不到引擎"的空行。
pub fn find<'a>(engines: &'a [WebEngine], keyword: &str) -> Option<&'a WebEngine> {
    let key = keyword.trim().to_lowercase();
    if key.is_empty() {
        return None;
    }
    engines.iter().find(|e| e.keyword.to_lowercase() == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engines() -> Vec<WebEngine> {
        builtin_engines()
    }

    #[test]
    fn builtin_table_matches_the_nine_engines_the_spec_names() {
        let list: Vec<(String, String)> =
            engines().iter().map(|e| (e.id.clone(), e.keyword.clone())).collect();
        assert_eq!(
            list,
            vec![
                ("google".into(), "g".into()),
                ("bing".into(), "bing".into()),
                ("baidu".into(), "bd".into()),
                ("github".into(), "gh".into()),
                ("wikipedia".into(), "wiki".into()),
                ("youtube".into(), "yt".into()),
                ("taobao".into(), "tb".into()),
                ("jd".into(), "jd".into()),
                ("translate".into(), "tr".into()),
            ]
        );
        // 关键字撞车会让 `find` 只取第一条，剩下的引擎永久不可达。
        let mut keys: Vec<String> = engines().iter().map(|e| e.keyword.clone()).collect();
        keys.sort();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "关键字必须互不相同");
    }

    #[test]
    fn encodes_everything_outside_unreserved() {
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("a-z_0.~"), "a-z_0.~");
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
        // 中文字面量在本仓库的测试里是安全的：断言的是纯 ASCII 期望串。
        assert_eq!(percent_encode("北京"), "%E5%8C%97%E4%BA%AC");
    }

    #[test]
    fn fills_all_occurrences_and_respects_the_default() {
        assert_eq!(
            fill_template("https://x/s?q={query}", "hi").as_deref(),
            Some("https://x/s?q=hi")
        );
        assert_eq!(
            fill_template("http://localhost:{query|8000}", "").as_deref(),
            Some("http://localhost:8000"),
            "需求书 TC 11-1 那条：空词用默认值"
        );
        assert_eq!(
            fill_template("http://localhost:{query|8000}", "9000").as_deref(),
            Some("http://localhost:9000")
        );
        assert_eq!(
            fill_template("https://x/{query}/y/{query}", "a b").as_deref(),
            Some("https://x/a%20b/y/a%20b")
        );
        assert_eq!(fill_template("https://x?q={query}", ""), None, "空词无默认 ⇒ valid=false");
        assert_eq!(
            fill_template("https://x?q={query|}", "").as_deref(),
            Some("https://x?q="),
            "空默认值也算给了默认值（区别于没有默认）"
        );
        assert_eq!(
            fill_template("https://x/home", "anything").as_deref(),
            Some("https://x/home"),
            "模板没有占位符也出行（那条就是一张网址书签）"
        );
        assert_eq!(
            fill_template("https://x/{id}?q={query}", "hi").as_deref(),
            Some("https://x/{id}?q=hi"),
            "非查询占位符原样留着"
        );
    }

    #[test]
    fn a_keyword_hit_with_a_word_yields_one_executable_row() {
        let Exclusive::Rows(rows) = exclusive_rows(&engines(), "g", "autohotkey v2") else {
            panic!("有词且首词精确命中 ⇒ 应当是可执行行");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].engine_id, "google");
        assert_eq!(rows[0].url, "https://www.google.com/search?q=autohotkey%20v2");
        assert!(rows[0].title.contains("autohotkey v2"));
        // 关键字大小写不敏感，但只有精确相等才算命中（`gg` 不是引擎）。
        assert!(matches!(exclusive_rows(&engines(), "G", "x"), Exclusive::Rows(_)));
        assert_eq!(exclusive_rows(&engines(), "gg", "x"), Exclusive::NotOurs);
        assert_eq!(exclusive_rows(&engines(), "gogle", "x"), Exclusive::NotOurs, "前缀不独占");
    }

    #[test]
    fn an_empty_word_yields_only_a_completion_hint() {
        let Exclusive::Hints(hints) = exclusive_rows(&engines(), "g", "") else {
            panic!("空词 + 必须填值的模板 ⇒ 只能是补全提示");
        };
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].engine_id, "google");
        assert_eq!(hints[0].completion, "g ");
        // 只剩空白也算空词：`g ` 之后还没打字就被回车，不该打开一次空搜索。
        assert!(matches!(exclusive_rows(&engines(), "g", "  "), Exclusive::Hints(_)));
        assert!(matches!(exclusive_rows(&engines(), "g", "foo"), Exclusive::Rows(_)));
        assert_eq!(exclusive_rows(&engines(), "zz", ""), Exclusive::NotOurs);
    }

    #[test]
    fn fallback_skips_sources_that_do_not_exist_yet() {
        let rows = fallback_rows(&engines(), "zzzz");
        assert_eq!(
            rows.iter().map(|r| r.engine_id.as_str()).collect::<Vec<_>>(),
            vec!["google", "bing"],
            "files 源（FR-SRC-14）还没落地 ⇒ 跳过，不拖垮整条兜底"
        );
        assert!(rows[0].url.ends_with("q=zzzz"));
        assert!(fallback_rows(&engines(), "   ").is_empty(), "空查询不兜底");
    }

    #[test]
    fn custom_engine_round_trips_and_takes_part_in_search() {
        let json = r#"{"id":"local","keyword":"loc","title":"本机","url":"http://localhost:{query|8000}","icon":""}"#;
        let engine: WebEngine = serde_json::from_str(json).expect("自定义引擎应可解析");
        let mut list = engines();
        list.push(engine.clone());
        let Exclusive::Rows(rows) = exclusive_rows(&list, "loc", "") else {
            panic!("{}", "有默认值的模板空词也应当出行");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].url, "http://localhost:8000");
        assert_eq!(
            serde_json::to_string(&engine)
                .expect("序列化")
                .replace(" ", ""),
            json.replace(" ", "")
        );
    }

    #[test]
    fn missing_icon_field_still_parses() {
        let e: WebEngine =
            serde_json::from_str(r#"{"id":"a","keyword":"a","title":"A","url":"https://a/{query}"}"#)
                .expect("icon 缺字段应走 default");
        assert_eq!(e.icon, "");
    }
}

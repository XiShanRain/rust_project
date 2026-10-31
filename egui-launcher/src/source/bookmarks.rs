//! FR-SRC-12 浏览器书签：`bm 词` 只搜书签，直接输书签名也能在结果尾巴上找到。
//!
//! 行为口径照 AHK 版 `BookmarkProvider.ahk`：读 Chromium 系四个浏览器（Chrome / Edge /
//! Brave / Vivaldi）**全部 profile** 的 `Bookmarks` 明文 JSON，只收 `http(s)`，按 URL 去重；
//! 启动时读一次，之后每 30 秒看一眼文件修改时间，变了才重读（浏览器正写着时会读失败，
//! 静默跳过、下次再看）。Firefox 的书签在 SQLite 里，本批不支持。
//!
//! 与其它源的接法有一处新东西：**结果尾巴**（[`DataSource::tail`]）。需求书要求书签
//! 「排在应用/命令之后」，而头段行按架构永远排在清单结果之前（批次 A7 的脚本行就是
//! 头段）——所以书签走 `Plan::tail`，渲染在结果段之后，导航/数字键/状态栏按合并行号
//! 一起算。默认结果最多 [`MAX_DEFAULT`] 条、查询至少 [`MIN_DEFAULT_CHARS`] 个字
//! （AHK 同口径：两个字才开始猜，免得一呼出就满屏书签）。
//!
//! 隐私：书签标题与网址都是用户内容，日志只记条数，一个字都不落。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::matching::{self, Parsed};

use super::application;
use super::{DataSource, SearchCtx, SourceId, SourceRow};

/// 只搜书签的前缀（AHK 的 `Keyword` 缺省值）。
pub const KEYWORD: &str = "bm";

/// 浏览器名 + `User Data` 在 `%LOCALAPPDATA%` 里的相对路径。顺序即去重时的优先次序。
pub const BROWSERS: [(&str, &str); 4] = [
    ("Chrome", r"Google\Chrome\User Data"),
    ("Edge", r"Microsoft\Edge\User Data"),
    ("Brave", r"BraveSoftware\Brave-Browser\User Data"),
    ("Vivaldi", r"Vivaldi\User Data"),
];

/// 文件时间戳检查节流（需求书 FR-SRC-12「30 s 轮询」）。
const REFRESH_EVERY: Duration = Duration::from_secs(30);

/// 默认结果里最多露几条书签（AHK `TopIndexes(..., 8)`）。
pub const MAX_DEFAULT: usize = 8;

/// `bm 词` 独占时最多几条（AHK `ProviderRegistry.MaxResults`）。
pub const MAX_EXCLUSIVE: usize = 50;

/// 默认结果从几个字开始匹配。一个字时用户还在打，书签挤进来只会捣乱。
const MIN_DEFAULT_CHARS: usize = 2;

/// 一个 `Bookmarks` 文件：属于哪个浏览器、在哪。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkFile {
    pub browser: &'static str,
    pub path: PathBuf,
}

/// 书签文件里的一条（只留下与搜索有关的字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    /// 小写域名，去掉 `www.` / 端口 / 用户信息；解析不出时是空串。
    pub host: String,
    /// 标题的拼音首字母（中文标题敲 `bd` 也能命中）。标题全是西文时为空串。
    pub pinyin: String,
    pub browser: &'static str,
}

/// `bm` 前缀的三种形态（判据与脚本源的 `route` 同一形状：只看第一个词）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// 首词不是 `bm`，这条查询不归书签管。
    NotOurs,
    /// 只有 `bm`（或 `bm `）：给一行用法提示。
    Usage,
    /// `bm 词`：只搜书签，整张列表归本条。
    Search(String),
}

/// 判定 `bm` 前缀。大小写不敏感（`matching::parse` 已把首词折成小写）。
pub fn route(query: &str) -> Route {
    let p = matching::parse(query);
    if p.keyword != KEYWORD {
        return Route::NotOurs;
    }
    let rest = p.rest.trim();
    if rest.is_empty() {
        Route::Usage
    } else {
        Route::Search(rest.to_string())
    }
}

/// 一个浏览器根目录下所有带 `Bookmarks` 的 profile。
fn profiles_under(root: &Path, browser: &'static str, rel: &str) -> Vec<BookmarkFile> {
    let Ok(rd) = std::fs::read_dir(root.join(rel)) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    // 目录序不保证稳定 ⇒ 排序，保证「新书签在哪个 profile」两轮一致、按 URL 去重的结果可复现。
    dirs.sort();
    dirs.into_iter()
        .map(|dir| BookmarkFile {
            browser,
            path: dir.join("Bookmarks"),
        })
        .filter(|f| f.path.is_file())
        .collect()
}

/// `%LOCALAPPDATA%` 下四个浏览器的书签文件（缺哪个就少哪个）。
pub fn files_under(local_app_data: &Path) -> Vec<BookmarkFile> {
    let mut out = Vec::new();
    for (browser, rel) in BROWSERS {
        out.extend(profiles_under(local_app_data, browser, rel));
    }
    out
}

fn is_web_url(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// 网址里的域名。解析不出时返回空串（行上就只显示浏览器名）。
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    // 用户信息（`user:pass@host`）与端口不属于域名。
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = match host.strip_prefix('[') {
        // IPv6 字面量：`[::1]:8080` 的冒号不是端口分隔符。
        Some(inner) => inner
            .find(']')
            .map(|end| &host[..end + 2])
            .unwrap_or(host),
        None => host.split(':').next().unwrap_or(host),
    };
    host.strip_prefix("www.")
        .unwrap_or(host)
        .to_ascii_lowercase()
}

fn walk_node(node: &serde_json::Value, out: &mut Vec<Bookmark>) {
    if node.get("type").and_then(|t| t.as_str()) == Some("url") {
        let Some(url) = node.get("url").and_then(|u| u.as_str()) else {
            return;
        };
        if !is_web_url(url) {
            return;
        }
        let url = url.trim().to_string();
        let name = node
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .trim();
        // 没起名的书签用网址当标题（AHK 同口径）。
        let title = if name.is_empty() {
            url.clone()
        } else {
            name.to_string()
        };
        let pinyin = application::default_keyword(&title);
        out.push(Bookmark {
            host: host_of(&url),
            title,
            url,
            pinyin,
            browser: "",
        });
        return;
    }
    if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
        for child in children {
            walk_node(child, out);
        }
    }
}

/// 一个 `Bookmarks` 文件的全部网址书签。整个文件坏掉（浏览器正在写）⇒ 空表，不 panic。
pub fn parse_file(text: &str) -> Vec<Bookmark> {
    let mut out = Vec::new();
    let Ok(root) = serde_json::from_str::<serde_json::Value>(text) else {
        return out;
    };
    if let Some(roots) = root.get("roots").and_then(|r| r.as_object()) {
        for node in roots.values() {
            walk_node(node, &mut out);
        }
    }
    out
}

/// 文件的时间戳。`path + mtime` 是这轮读取的"内容指纹"——只在**读成功**之后记进 stamp，
/// 这样浏览器正写着、读失败的那一次不会被当成"已经是最新的"。
fn file_stamp(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let nanos = modified
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("{}:{nanos}", path.to_string_lossy()))
}

/// 能打开的文件拼成的指纹。读不了（被独占 / 刚被删）的文件不进指纹——它一旦重新可读，
/// 指纹就会变，下一轮自然重读。
pub fn stamp_of(files: &[BookmarkFile]) -> String {
    let mut out = String::new();
    for f in files {
        if std::fs::File::open(&f.path).is_err() {
            continue;
        }
        if let Some(part) = file_stamp(&f.path) {
            out.push_str(&part);
            out.push('|');
        }
    }
    out
}

/// 读一遍全部文件。按 [`BROWSERS`] 与 profile 名的次序收集，**按 URL 去重**（先到先得），
/// 同时产出与 [`stamp_of`] 同一口径的指纹。
pub fn load(files: &[BookmarkFile]) -> (Vec<Bookmark>, String) {
    let mut out: Vec<Bookmark> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut stamp = String::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f.path) else {
            continue;
        };
        if let Some(part) = file_stamp(&f.path) {
            stamp.push_str(&part);
            stamp.push('|');
        }
        for mut bookmark in parse_file(&text) {
            bookmark.browser = f.browser;
            if seen.insert(bookmark.url.clone()) {
                out.push(bookmark);
            }
        }
    }
    (out, stamp)
}

/// 一条书签对整次查询的打分：标题 / 域名 / 拼音首字母取最优，档位与清单同一张表。
pub fn score_entry(p: &Parsed, b: &Bookmark) -> Option<(u8, f32)> {
    let mut best = matching::score_text(p, &b.title);
    for extra in [b.host.as_str(), b.pinyin.as_str()] {
        if extra.is_empty() {
            continue;
        }
        if let Some(hit) = matching::score_text(p, extra) {
            if best.is_none_or(|b| hit > b) {
                best = Some(hit);
            }
        }
    }
    best
}

/// 尾巴上/独占列表里的一行。`subtitle`＝「浏览器 · 域名」——同名书签靠它认人。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkHit {
    pub title: String,
    pub url: String,
    pub subtitle: String,
}

impl BookmarkHit {
    fn of(b: &Bookmark) -> Self {
        let subtitle = if b.host.is_empty() {
            b.browser.to_string()
        } else {
            format!("{} · {}", b.browser, b.host)
        };
        Self {
            title: b.title.clone(),
            url: b.url.clone(),
            subtitle,
        }
    }
}

/// 不可执行的说明行：`bm` 单独出现时的用法、以及 `bm 词` 一条都没命中时的说明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkHint {
    pub title: String,
    pub subtitle: String,
}

/// 本批书签源的现场。
pub struct BookmarkSource {
    /// `%LOCALAPPDATA%`。`None`＝还没初始化（单测里用 [`BookmarkSource::with_root`]）。
    root: Option<PathBuf>,
    files: Vec<BookmarkFile>,
    entries: Vec<Bookmark>,
    stamp: String,
    checked: Instant,
}

impl Default for BookmarkSource {
    fn default() -> Self {
        Self {
            root: None,
            files: Vec::new(),
            entries: Vec::new(),
            stamp: String::new(),
            // 首轮就允许检查：呼出时文件可能刚被改过。
            checked: Instant::now() - REFRESH_EVERY,
        }
    }
}

impl BookmarkSource {
    /// 单测用：钉一个假的 `%LOCALAPPDATA%`。
    #[cfg(test)]
    pub fn with_root(root: PathBuf) -> Self {
        let mut s = Self {
            root: Some(root),
            ..Self::default()
        };
        s.refiles_and_load();
        s
    }

    fn refiles_and_load(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.files = files_under(&root);
        let (entries, stamp) = load(&self.files);
        self.entries = entries;
        self.stamp = stamp;
        self.checked = Instant::now();
    }

    /// 按查询打分排好、截断的一组行。
    fn ranked_rows(&self, term: &str, limit: usize) -> Vec<SourceRow> {
        let p = matching::parse(term);
        if p.words.is_empty() {
            return Vec::new();
        }
        let mut ranked: Vec<(u8, f32, usize)> = Vec::new();
        for (i, b) in self.entries.iter().enumerate() {
            if let Some((tier, score)) = score_entry(&p, b) {
                ranked.push((tier, score, i));
            }
        }
        ranked.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| {
                    self.entries[a.2]
                        .title
                        .chars()
                        .count()
                        .cmp(&self.entries[b.2].title.chars().count())
                })
                .then_with(|| {
                    self.entries[a.2]
                        .title
                        .to_lowercase()
                        .cmp(&self.entries[b.2].title.to_lowercase())
                })
        });
        ranked.truncate(limit);
        ranked
            .into_iter()
            .map(|(_, _, i)| SourceRow::Bookmark(BookmarkHit::of(&self.entries[i])))
            .collect()
    }

    fn count_hint(&self) -> BookmarkHint {
        BookmarkHint {
            title: crate::i18n::t("bookmark.hint.title").to_string(),
            subtitle: crate::i18n::tfmt("bookmark.hint.subtitle", &[&self.entries.len()]),
        }
    }

    fn no_match_hint(&self) -> BookmarkHint {
        BookmarkHint {
            title: crate::i18n::t("bookmark.hint.title").to_string(),
            subtitle: crate::i18n::tfmt("bookmark.no_match", &[&self.entries.len()]),
        }
    }
}

impl DataSource for BookmarkSource {
    fn id(&self) -> SourceId {
        SourceId::Bookmarks
    }

    fn label(&self) -> &'static str {
        crate::i18n::t("source.bookmarks.label")
    }

    fn note(&self) -> &'static str {
        crate::i18n::t("source.bookmarks.note")
    }

    fn init(&mut self) {
        if self.root.is_none() {
            self.root = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        }
        self.refiles_and_load();
        log_info!(
            "bookmarks: 已载入 {} 条书签（{} 个浏览器配置）",
            self.entries.len(),
            self.files.len()
        );
    }

    fn poll(&mut self) -> bool {
        if self.checked.elapsed() < REFRESH_EVERY {
            return false;
        }
        let Some(root) = self.root.clone() else {
            return false;
        };
        self.checked = Instant::now();
        // profile 目录可能新增 / 消失（新建配置、删配置），文件表也一起重建。
        self.files = files_under(&root);
        if stamp_of(&self.files) == self.stamp {
            return false;
        }
        let (entries, stamp) = load(&self.files);
        self.entries = entries;
        self.stamp = stamp;
        // 标题与网址不落日志（用户内容）。
        log_info!(
            "bookmarks: 书签文件有变化，重读得 {} 条（{} 个浏览器配置）",
            self.entries.len(),
            self.files.len()
        );
        true
    }

    fn take_over(&mut self, cx: &mut SearchCtx<'_>) -> Option<Vec<SourceRow>> {
        let Route::Search(term) = route(cx.query) else {
            return None;
        };
        let rows = self.ranked_rows(&term, MAX_EXCLUSIVE);
        Some(if rows.is_empty() {
            vec![SourceRow::BookmarkHint(self.no_match_hint())]
        } else {
            rows
        })
    }

    fn head(&mut self, cx: &mut SearchCtx<'_>) -> Vec<SourceRow> {
        // `bm` 单独出现：独占没成立（`take_over` 只认 `bm 词`），这里补一句用法，
        // 顺带报一条"手里有多少书签"，否则用户不知道该不该往下打。
        if route(cx.query) != Route::Usage {
            return Vec::new();
        }
        vec![SourceRow::BookmarkHint(self.count_hint())]
    }

    fn tail(&mut self, cx: &mut SearchCtx<'_>) -> Vec<SourceRow> {
        if route(cx.query) != Route::NotOurs {
            return Vec::new();
        }
        let term = cx.query.trim();
        if term.chars().count() < MIN_DEFAULT_CHARS {
            return Vec::new();
        }
        self.ranked_rows(term, MAX_DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::application::LaunchItem;
    use crate::source::DataSource;
    use crate::source::SourceId;

    fn scratch(name: &str) -> PathBuf {
        let tick = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "egui-launcher-bookmarks-{}-{tick}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("建测试目录");
        dir
    }

    /// 一个最小可用的 Chromium `Bookmarks` 文件：书签栏 + 一个文件夹 + 非网址项。
    const SAMPLE: &str = r#"{
      "checksum": "x",
      "roots": {
        "bookmark_bar": {
          "children": [
            { "type": "url", "name": "Rust 官方", "url": "https://www.rust-lang.org/" },
            { "type": "url", "name": "", "url": "https://example.com/path" },
            { "type": "url", "name": "本地文件", "url": "file:///C:/x.html" },
            { "type": "folder", "name": "资料", "children": [
              { "type": "url", "name": "百度一下", "url": "http://www.baidu.com/s?wd=x" }
            ] }
          ]
        },
        "other": { "children": [] }
      }
    }"#;

    #[test]
    fn parse_walks_folders_and_keeps_only_web_urls() {
        let list = parse_file(SAMPLE);
        let urls: Vec<&str> = list.iter().map(|b| b.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://www.rust-lang.org/",
                "https://example.com/path",
                "http://www.baidu.com/s?wd=x"
            ],
            "嵌套文件夹里的书签要收，file:// 要丢"
        );
        assert_eq!(list[0].host, "rust-lang.org", "www. 要剥掉");
        assert_eq!(list[1].title, "https://example.com/path", "没名的用网址兜底");
        assert_eq!(list[2].host, "baidu.com");
        assert!(!list[2].pinyin.is_empty(), "中文标题要出拼音首字母");
    }

    #[test]
    fn bad_json_is_empty_not_a_panic() {
        assert!(parse_file("").is_empty());
        assert!(parse_file("{ not json").is_empty());
        assert!(parse_file(r#"{"roots": 1}"#).is_empty());
        assert!(parse_file(r#"{"roots":{}}"#).is_empty());
    }

    #[test]
    fn host_strips_userinfo_port_and_brackets() {
        assert_eq!(host_of("https://user:pw@Example.COM:8443/a?b#c"), "example.com");
        assert_eq!(host_of("http://[::1]:8080/x"), "[::1]");
        assert_eq!(host_of("about:blank"), "about");
        assert_eq!(host_of(""), "");
    }

    #[test]
    fn route_recognises_bm_and_leaves_other_words_alone() {
        assert_eq!(route("bm"), Route::Usage);
        assert_eq!(route("bm "), Route::Usage);
        assert_eq!(route("BM docs"), Route::Search("docs".into()));
        assert_eq!(route("bm  rust  book"), Route::Search("rust  book".into()));
        assert_eq!(route("bmw"), Route::NotOurs);
        assert_eq!(route("abm docs"), Route::NotOurs);
        assert_eq!(route(""), Route::NotOurs);
    }

    /// 造两个浏览器、各一个 profile，验证「全 profile + 按 URL 去重（先到先得）」。
    #[test]
    fn load_dedupes_by_url_and_keeps_browser_order() {
        let root = scratch("dedupe");
        let edge = root.join(r"Microsoft\Edge\User Data\Default");
        let chrome = root.join(r"Google\Chrome\User Data\Profile 1");
        std::fs::create_dir_all(&edge).unwrap();
        std::fs::create_dir_all(&chrome).unwrap();
        let both = r#"{"roots":{"bookmark_bar":{"children":[
            {"type":"url","name":"共享","url":"https://shared.example/x"},
            {"type":"url","name":"独有","url":"https://unique.example/y"}]}}}"#;
        std::fs::write(edge.join("Bookmarks"), both).unwrap();
        std::fs::write(chrome.join("Bookmarks"), both).unwrap();

        let files = files_under(&root);
        assert_eq!(files.len(), 2, "两个浏览器各一个 profile");
        assert_eq!(files[0].browser, "Chrome", "先按 BROWSERS 次序");
        assert_eq!(files[1].browser, "Edge");

        let (list, stamp) = load(&files);
        assert_eq!(list.len(), 2, "同 URL 只留一条");
        assert!(list.iter().all(|b| b.browser == "Chrome"), "先到先得");
        assert!(!stamp.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_or_broken_files_are_skipped_without_losing_the_rest() {
        let root = scratch("partial");
        let edge = root.join(r"Microsoft\Edge\User Data\Default");
        std::fs::create_dir_all(&edge).unwrap();
        std::fs::write(
            edge.join("Bookmarks"),
            r#"{"roots":{"bookmark_bar":{"children":[{"type":"url","name":"E","url":"https://e.example/"}]}}}"#,
        )
        .unwrap();
        let files = vec![
            BookmarkFile { browser: "Chrome", path: root.join("nope").join("Bookmarks") },
            BookmarkFile { browser: "Edge", path: edge.join("Bookmarks") },
            BookmarkFile { browser: "Brave", path: root.join("bad").join("Bookmarks") },
        ];
        let (list, stamp) = load(&files);
        assert_eq!(list.len(), 1, "读不到的文件静默跳过，别的照收");
        assert_eq!(list[0].browser, "Edge");
        assert!(!stamp.contains("nope") && !stamp.contains("bad"));
        std::fs::remove_dir_all(&root).ok();
    }

    /// 30 s 轮询的判据：mtime 没变 ⇒ 不重读；变了 ⇒ 重读，且条目跟着换。
    #[test]
    fn poll_reloads_only_after_the_file_changes() {
        let root = scratch("poll");
        let dir = root.join(r"Microsoft\Edge\User Data\Default");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Bookmarks");
        let one = r#"{"roots":{"bookmark_bar":{"children":[{"type":"url","name":"甲","url":"https://one.example/"}]}}}"#;
        let two = r#"{"roots":{"bookmark_bar":{"children":[
            {"type":"url","name":"甲","url":"https://one.example/"},
            {"type":"url","name":"乙","url":"https://two.example/"}]}}}"#;
        std::fs::write(&file, one).unwrap();

        let mut src = BookmarkSource::with_root(root.clone());
        assert_eq!(src.entries.len(), 1);
        assert!(!src.poll(), "30 秒还没到：一次文件都不看");
        // 让节流窗口过去（测试里直接把 checked 拨回去，不 sleep 30 秒）。
        src.checked = Instant::now() - REFRESH_EVERY;
        assert!(!src.poll(), "mtime 没变 ⇒ 不重读");
        std::fs::write(&file, two).unwrap();
        // 文件系统时间戳粒度可能很粗：把 mtime 显式往后拨，避免同一时刻两次写看不出差别。
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        let handle = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
        handle.set_modified(later).unwrap();
        drop(handle);
        src.checked = Instant::now() - REFRESH_EVERY;
        assert!(src.poll(), "mtime 变了 ⇒ 重读");
        assert_eq!(src.entries.len(), 2);
        std::fs::remove_dir_all(&root).ok();
    }

    /// 造一个只有一个 Edge profile 的书签源。返回根目录交给调用方收尾删除。
    fn source_with(tag: &str, list: Vec<(&str, &str)>) -> (BookmarkSource, PathBuf) {
        let root = scratch(tag);
        let dir = root.join(r"Microsoft\Edge\User Data\Default");
        std::fs::create_dir_all(&dir).unwrap();
        let children: Vec<String> = list
            .iter()
            .map(|(name, url)| format!(r#"{{"type":"url","name":"{name}","url":"{url}"}}"#))
            .collect();
        std::fs::write(
            dir.join("Bookmarks"),
            format!(
                r#"{{"roots":{{"bookmark_bar":{{"children":[{}]}}}}}}"#,
                children.join(",")
            ),
        )
        .unwrap();
        (BookmarkSource::with_root(root.clone()), root)
    }

    fn ctx<'a>(query: &'a str, items: &'a [LaunchItem], index: &'a mut crate::matching::MatchIndex) -> SearchCtx<'a> {
        fn on(_: SourceId) -> bool {
            true
        }
        SearchCtx {
            query,
            items,
            engines: &[],
            web_fallback: &[],
            learn_rank: crate::source::application::LearnRank::Count,
            index,
            now_ms: 0,
            term_shell: crate::source::terminal::TermShell::default(),
            source_on: &on,
        }
    }

    #[test]
    fn bm_prefix_takes_over_and_never_shows_list_items() {
        let (mut src, root) = source_with(
            "takeover",
            vec![
                ("Rust 官方", "https://www.rust-lang.org/"),
                ("Rust 文档", "https://doc.rust-lang.org/"),
                ("百度", "https://www.baidu.com/"),
            ],
        );
        let items = vec![LaunchItem {
            name: "rust".into(),
            path: "rust.exe".into(),
            ..Default::default()
        }];
        let mut index = crate::matching::MatchIndex::default();
        let mut cx = ctx("bm rust", &items, &mut index);
        let rows = src.take_over(&mut cx).expect("`bm 词` 必须独占");
        assert_eq!(rows.len(), 2, "只出书签");
        assert!(rows.iter().all(|r| matches!(r, SourceRow::Bookmark(_))));
        assert_eq!(src.take_over(&mut ctx("rust", &items, &mut index)), None, "非 bm 不独占");
        assert_eq!(src.take_over(&mut ctx("bm", &items, &mut index)), None, "裸 bm 只给用法");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bare_bm_puts_a_usage_hint_in_the_head() {
        let (mut src, root) = source_with("usage", vec![("Rust", "https://www.rust-lang.org/")]);
        let items: Vec<LaunchItem> = Vec::new();
        let mut index = crate::matching::MatchIndex::default();
        let rows = src.head(&mut ctx("bm", &items, &mut index));
        assert!(
            matches!(rows.as_slice(), [SourceRow::BookmarkHint(_)]),
            "裸 bm 要给一行用法提示，实际 {rows:?}"
        );
        assert!(src.head(&mut ctx("bm rust", &items, &mut index)).is_empty(), "`bm 词` 归独占，头段不重复");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bm_without_hits_still_takes_over_with_a_hint() {
        let (mut src, root) = source_with("nomatch", vec![("Rust", "https://www.rust-lang.org/")]);
        let items: Vec<LaunchItem> = Vec::new();
        let mut index = crate::matching::MatchIndex::default();
        let mut cx = ctx("bm 不存在的东西", &items, &mut index);
        let rows = src.take_over(&mut cx).expect("没命中也要独占");
        assert!(matches!(rows.as_slice(), [SourceRow::BookmarkHint(_)]));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tail_needs_two_chars_and_stops_at_eight() {
        let many: Vec<(&str, String)> = (0..12)
            .map(|i| ("书签", format!("https://site{i}.example/")))
            .collect();
        let refs: Vec<(&str, &str)> = many.iter().map(|(n, u)| (*n, u.as_str())).collect();
        let (mut src, root) = source_with("tail", refs);
        let items: Vec<LaunchItem> = Vec::new();
        let mut index = crate::matching::MatchIndex::default();
        // 单个字不猜（AHK `StrLen < 2` 同口径）
        assert!(src.tail(&mut ctx("书", &items, &mut index)).is_empty());
        let rows = src.tail(&mut ctx("书签", &items, &mut index));
        assert_eq!(rows.len(), MAX_DEFAULT, "默认结果最多 8 条");
        // 查询带 `bm` 时归独占那一支，尾巴不再重复出。
        assert!(src.tail(&mut ctx("bm 书签", &items, &mut index)).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scoring_matches_title_host_and_pinyin() {
        let b = Bookmark {
            title: "百度一下".into(),
            url: "https://www.baidu.com/".into(),
            host: "baidu.com".into(),
            pinyin: "bdyx".into(),
            browser: "Edge",
        };
        for q in ["百", "baidu", "bdyx"] {
            let p = matching::parse(q);
            assert!(score_entry(&p, &b).is_some(), "{q} 应当命中");
        }
        assert!(score_entry(&matching::parse("zzz"), &b).is_none());
    }

    #[test]
    fn hit_subtitle_names_browser_and_host() {
        let b = Bookmark {
            title: "t".into(),
            url: "https://a.example/".into(),
            host: "a.example".into(),
            pinyin: String::new(),
            browser: "Edge",
        };
        assert_eq!(BookmarkHit::of(&b).subtitle, "Edge · a.example");
        let no_host = Bookmark { host: String::new(), ..b };
        assert_eq!(BookmarkHit::of(&no_host).subtitle, "Edge");
    }
}

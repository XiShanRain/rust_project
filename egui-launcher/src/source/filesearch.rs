//! FR-SRC-14 文件搜索与目录浏览（批次 X）。
//!
//! 索引在后台线程构建（[`spawn_rebuild`]），主线程只整体换快照；目录浏览也走后台
//! （[`spawn_browse`]），因为一个网络盘或十万文件的目录会把 UI 线程按住几百毫秒。
//! 不碰窗口、不读设置——`src/main.rs` 拿产物去画行。
//!
//! 批次 A10 起索引/浏览的全部状态收在 [`FileSource`] 里（原先散在 `LauncherApp` 的八个字段），
//! 后台回报的收取与那两条进度日志也跟着搬进来——「谁的状态谁记，谁记谁说得清」。
//!
//! 五条口径是本批定的，需求原文没钉，逐条登记进发布说明第二十二节：
//! - **文件模式独占整张列表**：六式都带显式触发符（一路径形态 / 二 `'` / 三 `open|find|file` /
//!   四类型词 / 五 `folder` / 六空格开头），触发符没打全之前普通搜索照常 ⇒ 不会"打着打着列表塌了"。
//! - **索引范围是三个用户目录，不是全量扫盘**。开发计划第七章把它列为高风险项
//!   （`node_modules`/`.git` 一进去就是几十万条），扩表必须先配排除规则与设置页（D7/A10）。
//! - **索引不跨启动保存**：冷启动重建一次（后台），此后每 30 分钟一轮。半小时内新建的文件
//!   搜不到是这批改的代价，而浏览模式（直接打路径）永远是即时的。
//! - **打分阶梯自定**（100/90/80/60/30），多词取最低分＝AND 语义；与 §4.3 那张清单条目分值表
//!   不是一套，也不该是一套（文件源没有别名、置顶与使用次数这些维度）。
//! - **过滤词不足 2 字不进文件模式**（需求书的 `min_query_length`）。

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::source::custom::expand_env;
use crate::source::{DataSource, SearchCtx, SourceId, SourceRow};

/// 需求书的 `scope_folders`。`Desktop` 在实机上常被重定向到 OneDrive，所以两个位置都试，
/// 存在才算进范围（[`scope_folders`] 会滤掉不存在的）。
const SCOPE_FOLDER_NAMES: [&str; 6] = [
    "Desktop",
    "Downloads",
    "Documents",
    "OneDrive\\Desktop",
    "OneDrive\\Documents",
    "OneDrive\\Downloads",
];

/// 需求书的 `scope_depth`。
pub const SCOPE_DEPTH: usize = 4;
/// 需求书的 `max_entries`（默认 30000，与开发计划第七章那条风险项同一个数）。
pub const MAX_ENTRIES: usize = 30_000;
/// 需求书的 `min_query_length`。
pub const MIN_QUERY_LEN: usize = 2;
/// 需求书「结果上限 30」。
pub const MAX_RESULTS: usize = 30;
/// 需求书的 `refresh_minutes`（默认 30）。
pub const REBUILD_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// 需求书的 `scope_exclude`：按目录名整段跳过（连它下面的文件一起不进气）。
/// 后两个只在范围扩到盘根时才会碰到，留着是因为这一张表将来会加目录。
const EXCLUDE_DIRS: [&str; 7] = [
    "node_modules",
    ".git",
    ".hg",
    ".svn",
    "__pycache__",
    "$recycle.bin",
    "system volume information",
];

/// 第四式的类型词表：需求书 FR-SRC-14 那张表原样（`doc`/`pic`/`video`/`audio`/`zip`/`exe`/`cad`）。
pub const TYPE_WORDS: [(&str, &[&str]); 7] = [
    (
        "doc",
        &[
            "doc", "docx", "xls", "xlsx", "xlsm", "csv", "ppt", "pptx", "pdf", "txt", "md", "rtf",
            "odt", "ods",
        ],
    ),
    (
        "pic",
        &[
            "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "svg", "ico", "heic",
        ],
    ),
    (
        "video",
        &["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v"],
    ),
    ("audio", &["mp3", "wav", "flac", "aac", "m4a", "ogg", "wma"]),
    (
        "zip",
        &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "iso"],
    ),
    ("exe", &["exe", "msi", "bat", "cmd", "ps1", "lnk"]),
    ("cad", &["dwg", "dxf", "dgn", "dwf", "rvt", "ifc", "skp"]),
];

/// 第三式的动词前缀（需求书：`open|find|file report`）。
const FILE_WORDS: [&str; 3] = ["open", "find", "file"];
/// 第五式：只找文件夹。
const FOLDER_WORD: &str = "folder";

/// 索引里的一条。`path` 存绝对路径字符串（`FileEntry` 要能整表换给主线程，不做 `PathBuf` 转换）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// 一行文件结果。`title` 是文件名本体，`subtitle` 是它所在的目录——后者才是用户在三十条
/// 同名文件里认出自己要找那一条的依据，所以必须占一行副标题而不是省略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHit {
    pub title: String,
    pub path: String,
    pub is_dir: bool,
    pub subtitle: String,
}

/// 本查询该以哪种方式找文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// 没有任何触发符 ⇒ 文件源不参与，普通搜索照常。
    NotOurs,
    /// 路径浏览：`dir` 保证是绝对路径且**以分隔符结尾**，`filter` 是最后那一段（可为空）。
    /// 即时列目录，不查索引。
    Browse { dir: String, filter: String },
    /// 索引搜索。`exts` 空＝不限类型。
    Search {
        words: Vec<String>,
        exts: Vec<String>,
        dirs_only: bool,
    },
}

/// 后台线程回报给主线程的两类结果。
#[derive(Debug)]
pub enum FileUpdate {
    /// 整表替换（旧快照在这条到达前一直可用 ⇒ FR-SRC-04 那条"不清空列表"的取向）。
    Index(Vec<FileEntry>),
    /// 一次目录浏览的回报。带上 `(dir, filter)` 是给主线程核对用的：查询已经变了的话
    /// 这一份就是过期的，列错目录比空一帧严重得多。
    Dir {
        dir: String,
        filter: String,
        hits: Vec<FileHit>,
    },
}

/// 实际进入索引范围的目录（存在的才算）。
pub fn scope_folders() -> Vec<PathBuf> {
    let home = match std::env::var_os("USERPROFILE") {
        Some(v) => PathBuf::from(v),
        None => return Vec::new(),
    };
    SCOPE_FOLDER_NAMES
        .iter()
        .map(|name| home.join(name))
        .filter(|p| p.is_dir())
        .collect()
}

pub fn spawn_rebuild(tx: mpsc::Sender<FileUpdate>) {
    thread::spawn(move || {
        let entries = build_index();
        let _ = tx.send(FileUpdate::Index(entries));
    });
}

pub fn spawn_browse(tx: mpsc::Sender<FileUpdate>, dir: String, filter: String) {
    thread::spawn(move || {
        let hits = browse_hits(&dir, &filter);
        let _ = tx.send(FileUpdate::Dir { dir, filter, hits });
    });
}

// ---- 数据源实现（批次 A10：状态从 `LauncherApp` 搬到这里）----

/// FR-SRC-14 的数据源。索引/浏览的后台状态全部收在这里，`main.rs` 不再持有它们：
/// 原先那八个字段（通道、索引快照、建成时刻、在途旗标、目录回报、查询缓存）只有本源在用。
#[derive(Debug)]
pub struct FileSource {
    tx: mpsc::Sender<FileUpdate>,
    rx: mpsc::Receiver<FileUpdate>,
    /// 整表快照：`FileUpdate::Index` 到达时整体替换，旧快照在那一帧前一直可用。
    index: Vec<FileEntry>,
    /// 上一轮索引建成的时刻（`None` = 还没建成过）；同时兼任 30 分钟重扫的计时起点。
    indexed_at: Option<Instant>,
    rebuild_in_flight: bool,
    /// 一次目录浏览的回报，带 `(dir, filter)` 供查询核对。
    dir_hits: Option<(String, String, Vec<FileHit>)>,
    /// 已派出去、还没被采纳的那次浏览的 `(dir, filter)`。
    dir_in_flight: Option<(String, String)>,
    /// 同一查询串一帧里会被 `plan()` 算好几遍（空格判据、渲染、Enter…），而文件源要扫三万条
    /// ⇒ 按查询串缓存本轮命中。索引回报到达时清空，别让新索引撞上旧缓存。
    hits_cache: Option<(String, Vec<FileHit>)>,
}

impl Default for FileSource {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            index: Vec::new(),
            indexed_at: None,
            rebuild_in_flight: false,
            dir_hits: None,
            dir_in_flight: None,
            hits_cache: None,
        }
    }
}

impl FileSource {
    /// 派一轮后台索引重建。同一时刻只跑一轮，回收据的那帧才允许下一轮；
    /// `force` 忽略最小间隔（冷启动那一次）。
    fn refresh_index(&mut self, force: bool) {
        if self.rebuild_in_flight {
            return;
        }
        if let Some(built) = self.indexed_at {
            if !force && built.elapsed() < REBUILD_INTERVAL {
                return;
            }
        }
        self.rebuild_in_flight = true;
        spawn_rebuild(self.tx.clone());
    }

    /// 派一次目录浏览（读一层，不递归）。列目录也是磁盘 I/O——一个十万文件的下载夹
    /// 或一个正在断线的网络盘会把 UI 线程按住几百毫秒，所以它不进主线程。
    ///
    /// 去重只看两条：已经有这一对 `(dir, filter)` 的结果、或这一对正飞在路上。
    /// 别的组合照样派——打字途中旧的在途请求会被 [`Self::plan`] 的核对判据丢掉，
    /// 宁可多开几个短命线程，也不能让「最新那一次」因为限流而永远没结果。
    fn request_browse(&mut self, dir: &str, filter: &str) {
        if self
            .dir_hits
            .as_ref()
            .is_some_and(|(d, f, _)| d == dir && f == filter)
            || self
                .dir_in_flight
                .as_ref()
                .is_some_and(|(d, f)| d == dir && f == filter)
        {
            return;
        }
        self.dir_in_flight = Some((dir.to_string(), filter.to_string()));
        spawn_browse(self.tx.clone(), dir.to_string(), filter.to_string());
    }

    /// 本查询该不该由文件源独占整张列表。返回 `None` = 没有触发符，普通合并照常。
    fn plan(&mut self, query: &str) -> Option<Vec<SourceRow>> {
        let route = route(query);
        match route {
            Route::NotOurs => None,
            Route::Browse { dir, filter } => {
                self.request_browse(&dir, &filter);
                let hits = match &self.dir_hits {
                    Some((d, f, hits)) if *d == dir && *f == filter => hits.clone(),
                    _ => Vec::new(),
                };
                Some(hits.into_iter().map(SourceRow::File).collect())
            }
            Route::Search {
                words,
                exts,
                dirs_only,
            } => {
                self.refresh_index(false);
                // 冷启动那一阵索引还在后台：给一行说明，而不是让用户对着空列表猜是不是搜坏了。
                if self.indexed_at.is_none() {
                    return Some(vec![SourceRow::Hint(
                        crate::source::websearch::EngineHint {
                            engine_id: "filesearch".to_string(),
                            title: "文件索引建立中".to_string(),
                            subtitle: "首次扫描正在后台进行，稍后再试一次".to_string(),
                            completion: String::new(),
                        },
                    )]);
                }
                let cached = self
                    .hits_cache
                    .as_ref()
                    .filter(|(q, _)| q == query)
                    .map(|(_, hits)| hits.clone());
                let hits = match cached {
                    Some(hits) => hits,
                    None => {
                        let hits = search(&self.index, &words, &exts, dirs_only);
                        self.hits_cache = Some((query.to_string(), hits.clone()));
                        hits
                    }
                };
                Some(hits.into_iter().map(SourceRow::File).collect())
            }
        }
    }
}

impl DataSource for FileSource {
    fn id(&self) -> SourceId {
        SourceId::Files
    }

    fn label(&self) -> &'static str {
        "文件搜索"
    }

    fn note(&self) -> &'static str {
        "`'词`、空格开头、`open|find|file|folder|doc|pic…` 与路径形态会独占列表。关掉之后这些输入回到普通搜索（不起索引、不读磁盘）。"
    }

    fn init(&mut self) {
        self.refresh_index(true);
    }

    /// FR-SRC-17 自身命令「重建文件索引」：与冷启动同口径强制重建（走同一在途闸门）。
    fn rebuild(&mut self) {
        self.refresh_index(true);
    }

    fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(update) = self.rx.try_recv() {
            match update {
                FileUpdate::Index(entries) => {
                    let n = entries.len();
                    self.index = entries;
                    self.indexed_at = Some(Instant::now());
                    self.rebuild_in_flight = false;
                    // 新索引配旧缓存就是"用上一轮的命中回答这一轮的查询"⇒ 一起清掉。
                    self.hits_cache = None;
                    eprintln!(
                        "文件索引：本轮建成 {n} 条（范围＝桌面/下载/文档，深度 4，上限 3 万）"
                    );
                    changed = true;
                }
                FileUpdate::Dir { dir, filter, hits } => {
                    // 只认"当前还在等的那一份"。打字途中派出去的旧请求回来得晚，采纳它会让列表
                    // 显示另一个目录的内容——列错目录比空一帧严重得多。
                    if self
                        .dir_in_flight
                        .as_ref()
                        .is_some_and(|(d, f)| *d == dir && *f == filter)
                    {
                        self.dir_in_flight = None;
                        let n = hits.len();
                        self.dir_hits = Some((dir, filter, hits));
                        eprintln!("文件搜索：目录列表回报（{n} 条，路径不落日志）");
                        changed = true;
                    }
                }
            }
        }
        // 可见期间的 30 分钟重扫兜底（静默启动时 `init` 已派过第一轮）。
        self.refresh_index(false);
        changed
    }

    fn take_over(&mut self, cx: &mut SearchCtx<'_>) -> Option<Vec<SourceRow>> {
        self.plan(cx.query)
    }
}

// ---- 触发路由 ----

/// 撇号触发符的等价集合：半角 `'`、中文输入法打出的弯引号 `‘`/`’`、全角 `＇`。
/// 2026-10-23 真机：中文输入法下按撇号键打出的是 `’`，旧判据只认半角 ⇒
/// `’报告` 掉进普通搜索、界面显示「未找到」而不是文件模式。
const APOSTROPHES: [char; 4] = ['\'', '‘', '’', '＇'];

fn is_apostrophe(c: char) -> bool {
    APOSTROPHES.contains(&c)
}

/// 五式触发 → [`Route`]。判据只看查询串本身（当帧可观测状态），不看索引、不看磁盘。
pub fn route(raw: &str) -> Route {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Route::NotOurs;
    }
    // 第一式：路径浏览。判据是"展开后像不像一个绝对路径"，**不检查它此刻存不存在**——
    // 不存在就列空清单，这比把 `D:\nas\` 当关键词去搜清单更贴近用户意图（他正在打路径）。
    if let Some((dir, filter)) = browse_parts(trimmed) {
        return Route::Browse { dir, filter };
    }

    // 第六式：空格开头进文件模式。查询串里的前导空格只有两种来路——用户真按了空格
    // （`Shift+空格` 在四档 `space_to_run` 下恒为插入一个真空格），或程序化写进去的。
    let space_mode = raw.starts_with(' ');
    let mut words: Vec<String> = Vec::new();
    let mut apostrophe = false;
    for (i, w) in trimmed.split_whitespace().enumerate() {
        if i == 0 && w.starts_with(is_apostrophe) {
            apostrophe = true;
            let body = w.trim_start_matches(is_apostrophe);
            if !body.is_empty() {
                words.push(body.to_lowercase());
            }
            continue;
        }
        words.push(w.to_lowercase());
    }

    // 第二式 `'词` 与第六式（空格开头）只要触发符出现就算进了文件模式；第三～五式（动词 /
    // 类型词 / folder）还得多一个词，因为打到第一个词的过程中它仍是普通查询
    // （`pic` 是某个程序名的前缀时不该列表塌掉）。
    let mut exts: Vec<String> = Vec::new();
    let mut dirs_only = false;
    let mut prefix_word = false;
    if words.len() >= 2 {
        let head = words[0].as_str();
        if let Some((_, list)) = TYPE_WORDS.iter().find(|(k, _)| *k == head) {
            exts = list.iter().map(|s| s.to_string()).collect();
            words.remove(0);
            prefix_word = true;
        } else if head == FOLDER_WORD {
            dirs_only = true;
            words.remove(0);
            prefix_word = true;
        } else if FILE_WORDS.contains(&head) {
            words.remove(0);
            prefix_word = true;
        }
    }
    if !(space_mode || apostrophe || prefix_word) {
        return Route::NotOurs;
    }
    if words.iter().map(|w| w.chars().count()).sum::<usize>() < MIN_QUERY_LEN {
        return Route::NotOurs;
    }
    Route::Search {
        words,
        exts,
        dirs_only,
    }
}

/// 把查询串认成一个路径并切成（目录, 过滤词）。目录那半保证以 `\` 结尾。
pub fn browse_parts(raw: &str) -> Option<(String, String)> {
    if raw.contains('{') {
        return None; // `{query}` 模板不是路径，交给 FR-SRC-05
    }
    let expanded = expand_env(raw.trim());
    if !looks_absolute(&expanded) {
        return None;
    }
    let unified = expanded.replace('/', "\\");
    Some(split_parent(&unified))
}

/// 盘符 `X:\`、盘符带正斜杠 `X:/`、UNC `\\srv\share`。
/// 只认这三种 ⇒ 相对路径（`src\main.rs`）不进浏览模式，那更像在给清单加过滤词。
fn looks_absolute(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/') {
        return true;
    }
    s.starts_with("\\\\") && s.len() > 2
}

/// 在最后一个分隔符处切开；前半（含分隔符）是目录，后半是过滤词。
/// UNC 的前两个反斜杠不参与切分——否则 `\\srv\share` 会切成 `\\` + `srv\share`。
fn split_parent(p: &str) -> (String, String) {
    let unc = p.starts_with("\\\\");
    let tail = if unc { &p[2..] } else { p };
    let offset = if unc { 2 } else { 0 };
    match tail.rfind('\\') {
        // 已经以分隔符结尾 ⇒ 整串就是目录，没有过滤词。
        Some(rel) if rel + 1 == tail.len() => (p.to_string(), String::new()),
        Some(rel) => {
            let cut = rel + offset + 1;
            (p[..cut].to_string(), p[cut..].to_string())
        }
        // 没有可切的分隔符（`\\srv` 这种服务器根）⇒ 整串当目录。
        None => (p.to_string(), String::new()),
    }
}

// ---- 索引构建（后台线程）----

pub fn build_index() -> Vec<FileEntry> {
    let mut out = Vec::new();
    for root in scope_folders() {
        if out.len() >= MAX_ENTRIES {
            break;
        }
        walk(&root, SCOPE_DEPTH, &mut out);
    }
    out.truncate(MAX_ENTRIES);
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<FileEntry>) {
    if depth == 0 || out.len() >= MAX_ENTRIES {
        return;
    }
    // 打不开的目录（拒绝访问、正在断开的网络位置）静默跳过：NFR-04，一个目录读不动
    // 不该让整轮索引失败。
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        if out.len() >= MAX_ENTRIES {
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if EXCLUDE_DIRS.iter().any(|x| name.eq_ignore_ascii_case(x)) {
            continue;
        }
        // `DirEntry::file_type` 在 Windows 上直接取自目录枚举，不是再一次 stat。
        let is_dir = matches!(entry.file_type(), Ok(t) if t.is_dir());
        let path = entry.path();
        out.push(FileEntry {
            name,
            path: path.to_string_lossy().into_owned(),
            is_dir,
        });
        if is_dir {
            walk(&path, depth - 1, out);
        }
    }
}

// ---- 匹配与排序 ----

/// 索引搜索：多词 AND，每词各自打分取最低分（"最弱的那个词"决定这一条排哪儿）。
pub fn search(
    index: &[FileEntry],
    words: &[String],
    exts: &[String],
    dirs_only: bool,
) -> Vec<FileHit> {
    pick(index.iter(), words, exts, dirs_only)
}

/// 目录浏览：只读这一层（不递归），过滤词按同一套打分排序。
/// 上限同样是 `MAX_ENTRIES`（读条目的次数），十万文件的目录在这里也只会扫到前 3 万条就收手。
pub fn browse_hits(dir: &str, filter: &str) -> Vec<FileHit> {
    let entries = scan_dir(dir);
    let words: Vec<String> = if filter.trim().is_empty() {
        Vec::new()
    } else {
        vec![filter.trim().to_lowercase()]
    };
    pick(entries.iter(), &words, &[], false)
}

fn scan_dir(dir: &str) -> Vec<FileEntry> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten().take(MAX_ENTRIES) {
        let is_dir = matches!(entry.file_type(), Ok(t) if t.is_dir());
        out.push(FileEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path().to_string_lossy().into_owned(),
            is_dir,
        });
    }
    out
}

fn pick<'a>(
    entries: impl Iterator<Item = &'a FileEntry>,
    words: &[String],
    exts: &[String],
    dirs_only: bool,
) -> Vec<FileHit> {
    let mut scored: Vec<(i32, String, String, &'a FileEntry)> = Vec::new();
    for entry in entries {
        if dirs_only && !entry.is_dir {
            continue;
        }
        if !exts.is_empty() {
            // 类型词筛的是扩展名 ⇒ 目录一并排除：`C:\Users\pic` 这种目录名不是图片文件，
            // 留着只会让用户以为过滤器坏了。
            match extension(&entry.name).filter(|e| exts.iter().any(|t| t.eq_ignore_ascii_case(e)))
            {
                Some(_) => {}
                None => continue,
            }
        }
        let name = entry.name.to_lowercase();
        let Some(score) = words_score(&name, words) else {
            continue;
        };
        // 无过滤词（浏览一个空过滤词的目录）⇒ 全部同分，次序只看下面的名称。
        let score = if words.is_empty() { 60 } else { score };
        scored.push((score, name, entry.path.clone(), entry));
    }
    // 分值 → 名称长度 → 路径：短名字在前（`report.docx` 该压过 `report-2023-最终-复制.docx`），
    // 路径只用来在完全同名时给出一个不随文件系统次序漂动的稳定位。
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.3.name.chars().count().cmp(&b.3.name.chars().count()))
            .then_with(|| a.2.cmp(&b.2))
    });
    scored.truncate(MAX_RESULTS);
    scored.into_iter().map(|(_, _, _, e)| hit_of(e)).collect()
}

fn hit_of(entry: &FileEntry) -> FileHit {
    FileHit {
        title: entry.name.clone(),
        path: entry.path.clone(),
        is_dir: entry.is_dir,
        subtitle: parent_label(&entry.path),
    }
}

/// 所在目录（去掉最后一段）。行尾弱色副标题就放它。
fn parent_label(path: &str) -> String {
    let unified = path.replace('/', "\\");
    match unified.rfind('\\') {
        Some(i) if i + 1 < unified.len() => unified[..i].to_string(),
        Some(i) => unified[..i].to_string(),
        None => String::new(),
    }
}

/// 扩展名（小写、不含点）。点必须在名字中间或末尾，且不是首字符——`.gitignore` 是文件名不是扩展名。
fn extension(name: &str) -> Option<String> {
    let (stem, dot) = name.rsplit_once('.')?;
    if stem.is_empty() || dot.is_empty() || dot.contains('.') {
        return None;
    }
    Some(dot.to_lowercase())
}

/// 每个词都得命中（AND），整条的分由最弱的那个词决定。任一词零分 ⇒ 整条出局。
fn words_score(name_lower: &str, words: &[String]) -> Option<i32> {
    if words.is_empty() {
        return Some(0);
    }
    let stem = name_lower.rsplit_once('.').map_or(name_lower, |(s, _)| s);
    let mut best = i32::MAX;
    for w in words {
        let s = score_name(name_lower, stem, w);
        if s == 0 {
            return None;
        }
        best = best.min(s);
    }
    Some(best)
}

/// 单词的分值阶梯。`stem` 是去掉扩展名的名字——`报告.docx` 打 `报告` 应算精确命中，
/// 不该掉到"包含"那一档。
fn score_name(name_lower: &str, stem_lower: &str, word: &str) -> i32 {
    if word.is_empty() {
        return 0;
    }
    if name_lower == word || stem_lower == word {
        return 100;
    }
    if name_lower.starts_with(word) || stem_lower.starts_with(word) {
        return 90;
    }
    if name_lower
        .split(|c| matches!(c, '.' | '_' | '-' | ' ' | '$'))
        .any(|token| token.starts_with(word))
    {
        return 80;
    }
    if name_lower.contains(word) {
        return 60;
    }
    if is_subsequence(name_lower, word) {
        return 30;
    }
    0
}

fn is_subsequence(hay: &str, needle: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, path: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: path.to_string(),
            is_dir,
        }
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(|w| w.to_lowercase()).collect()
    }

    #[test]
    fn only_an_explicit_trigger_enters_file_mode() {
        // 没有触发符 ⇒ 普通搜索照常。这一条是"列表不许在用户打字途中塌掉"的闸门。
        assert_eq!(route("报告"), Route::NotOurs);
        assert_eq!(
            route("pic"),
            Route::NotOurs,
            "类型词单独出现说明还在打第一个词"
        );
        assert_eq!(route("folder"), Route::NotOurs);
        assert_eq!(route("open"), Route::NotOurs);
        assert_eq!(route(""), Route::NotOurs);
        // 六式各进各的。
        assert_eq!(
            route("pic 报告"),
            Route::Search {
                words: words("报告"),
                exts: extension_list("pic"),
                dirs_only: false
            }
        );
        assert_eq!(
            route("folder 项目"),
            Route::Search {
                words: words("项目"),
                exts: vec![],
                dirs_only: true
            }
        );
        assert_eq!(
            route("open 报告"),
            Route::Search {
                words: words("报告"),
                exts: vec![],
                dirs_only: false
            }
        );
        assert_eq!(
            route("find 报告"),
            Route::Search {
                words: words("报告"),
                exts: vec![],
                dirs_only: false
            }
        );
        assert_eq!(
            route("file 报告"),
            Route::Search {
                words: words("报告"),
                exts: vec![],
                dirs_only: false
            }
        );
        assert_eq!(
            route("'报告"),
            Route::Search {
                words: words("报告"),
                exts: vec![],
                dirs_only: false
            }
        );
        assert_eq!(
            route(" 报告"),
            Route::Search {
                words: words("报告"),
                exts: vec![],
                dirs_only: false
            }
        );
    }

    #[test]
    fn full_width_apostrophes_also_enter_file_mode() {
        // 2026-10-23 真机 A2：中文输入法打出的 `’` 曾被当普通文本，同一个查询
        // 变成「未找到」而不是文件模式。四种撇号形态必须走同一条路由。
        for q in ["'报告", "‘报告", "’报告", "＇报告"] {
            assert_eq!(
                route(q),
                Route::Search {
                    words: words("报告"),
                    exts: vec![],
                    dirs_only: false
                },
                "触发符 {q:?} 应进文件模式"
            );
        }
        // 只有首词开头才算触发符；后面的弯引号仍是普通查询文本。
        assert_eq!(route("报告 ’最终"), Route::NotOurs);
    }

    fn extension_list(word: &str) -> Vec<String> {
        TYPE_WORDS
            .iter()
            .find(|(k, _)| *k == word)
            .map(|(_, list)| list.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn two_words_after_a_type_prefix_stay_a_file_query() {
        // `pic 报告 最终` ⇒ 两个过滤词都留着（多词 AND），触发词自己不算词。
        let Route::Search {
            words: got, exts, ..
        } = route("pic 报告 最终")
        else {
            panic!("类型词 + 两个词应是索引搜索");
        };
        assert_eq!(got, words("报告 最终"));
        assert!(!exts.is_empty());
    }

    #[test]
    fn trigger_without_an_enough_filter_word_is_not_file_mode() {
        // 需求书的 `min_query_length`＝2：一个字的文件查询命中面太大，等于没过滤。
        assert_eq!(route("open x"), Route::NotOurs);
        assert_eq!(route("'x"), Route::NotOurs);
        assert_eq!(route("'"), Route::NotOurs);
        assert_eq!(route(" "), Route::NotOurs);
        assert!(matches!(route("open xx"), Route::Search { .. }));
    }

    #[test]
    fn path_forms_browse_and_split_into_dir_plus_filter() {
        // 盘符 + 尾分隔符 ⇒ 整串是目录。
        assert_eq!(
            browse_parts(r"D:\proj\rep\"),
            Some((r"D:\proj\rep\".to_string(), String::new()))
        );
        // 没有尾分隔符 ⇒ 最后一段是过滤词，目录是它的父（正在打 `rep` 时列的是 `D:\proj\`）。
        assert_eq!(
            browse_parts(r"D:\proj\rep"),
            Some((r"D:\proj\".to_string(), "rep".to_string()))
        );
        // 正斜杠归一成反斜杠（Windows 两种都收，行内显示只留一种）。
        assert_eq!(
            browse_parts("C:/Users/a/Doc"),
            Some((r"C:\Users\a\".to_string(), "Doc".to_string()))
        );
        // 盘根。
        assert_eq!(
            browse_parts(r"C:\"),
            Some((r"C:\".to_string(), String::new()))
        );
        // UNC：服务器名那一层不能当目录，切分要跳过开头的两个反斜杠。
        assert_eq!(
            browse_parts(r"\\srv\share\报"),
            Some((r"\\srv\share\".to_string(), "报".to_string()))
        );
        // 打到共享名一半时列的是服务器根（`\\srv\`）并用 `share` 过滤——与 `D:\proj\rep` 同一口径。
        assert_eq!(
            browse_parts(r"\\srv\share"),
            Some((r"\\srv\".to_string(), "share".to_string()))
        );
        // 不像绝对路径的一律不浏览（相对路径交给索引搜索/普通搜索）。
        assert_eq!(browse_parts("src\\main.rs"), None);
        assert_eq!(browse_parts("main.rs"), None);
        assert_eq!(
            browse_parts("%USERPROFILE%\\{query}"),
            None,
            "含占位符的是命令模板"
        );
    }

    #[test]
    fn env_vars_expand_before_the_path_is_split() {
        let profile = std::env::var("USERPROFILE").expect("Windows 上必有 USERPROFILE");
        let (dir, filter) = browse_parts(r"%USERPROFILE%\Down").expect("应识别成路径");
        assert_eq!(dir, format!(r"{}\", profile));
        assert_eq!(filter, "Down");
    }

    #[test]
    fn route_prefers_the_path_form_over_the_keyword_form() {
        // 用户正在打路径 ⇒ 别把 `D:\a\` 当成关键词。
        assert!(matches!(route(r"D:\a\"), Route::Browse { .. }));
    }

    #[test]
    fn score_ladder_is_ordered_and_extensible() {
        assert_eq!(
            score_name("report.docx", "report", "report.docx"),
            100,
            "整名精确"
        );
        assert_eq!(
            score_name("report.docx", "report", "report"),
            100,
            "去扩展名后精确"
        );
        assert_eq!(score_name("report.docx", "report", "repo"), 90, "前缀");
        assert_eq!(score_name("my_report.docx", "my_report", "rep"), 80, "词首");
        assert_eq!(score_name("areport.docx", "areport", "port"), 60, "包含");
        assert_eq!(score_name("rte.docx", "rte", "re"), 30, "子序列");
        assert_eq!(score_name("report.docx", "report", "zzz"), 0);
        assert_eq!(
            score_name("report.docx", "report", ""),
            0,
            "空词不给分，否则人人 100"
        );
    }

    #[test]
    fn multi_word_scoring_requires_every_word() {
        // AND：任一词没命中 ⇒ 整条出局。
        assert_eq!(words_score("report.docx", &words("report 最终")), None);
        // 整条的分由最弱的词决定：`docx` 是 `.` 之后那个 token 的词首（80），`report` 精确（100）⇒ 取 80。
        assert_eq!(words_score("report.docx", &words("report docx")), Some(80));
        // `repo` 前缀 90、`report` 精确 100 ⇒ 取 90（阶梯档位在多词下确实逐词比较）。
        assert_eq!(words_score("report.docx", &words("report repo")), Some(90));
    }

    #[test]
    fn search_filters_by_extension_type_dir_and_caps_results() {
        let index = vec![
            entry("报告.docx", r"C:\u\doc\报告.docx", false),
            entry("报告.png", r"C:\u\pic\报告.png", false),
            entry("报告", r"C:\u\pic\报告", true),
            entry("other.txt", r"C:\u\doc\other.txt", false),
        ];
        let pic = search(&index, &words("报告"), &extension_list("pic"), false);
        assert_eq!(pic.len(), 1, "类型词只留图片");
        assert_eq!(pic[0].path, r"C:\u\pic\报告.png");
        assert_eq!(pic[0].subtitle, r"C:\u\pic", "副标题是所在目录");

        let doc = search(&index, &words("报告"), &extension_list("doc"), false);
        assert_eq!(
            doc.iter().map(|h| h.title.as_str()).collect::<Vec<_>>(),
            ["报告.docx"],
            "同名目录不是文档，类型词过滤下不该混进来"
        );

        let dirs = search(&index, &words("报告"), &[], true);
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].is_dir);

        let any = search(&index, &words("报告"), &[], false);
        assert_eq!(any.len(), 3, "不带类型词时三条都算命中");
        assert_eq!(any[0].path, r"C:\u\pic\报告", "同分按名称长度，短的在前");
    }

    #[test]
    fn search_caps_at_max_results() {
        let index: Vec<FileEntry> = (0..MAX_RESULTS + 40)
            .map(|i| {
                entry(
                    &format!("report{i:03}.txt"),
                    &format!(r"C:\u\report{i:03}.txt"),
                    false,
                )
            })
            .collect();
        assert_eq!(
            search(&index, &words("report"), &[], false).len(),
            MAX_RESULTS
        );
    }

    #[test]
    fn empty_index_returns_nothing_instead_of_everything() {
        assert!(search(&[], &words("报告"), &[], false).is_empty());
    }

    #[test]
    fn browse_lists_one_directory_level_and_filters_it() {
        let dir = std::env::temp_dir().join("egui-launcher-filesearch-browse");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        std::fs::write(dir.join("alpha.txt"), b"x").expect("写测试文件");
        std::fs::write(dir.join("beta.md"), b"x").expect("写测试文件");
        std::fs::create_dir(dir.join("gamma")).expect("建子目录");
        let dir_str = dir.to_string_lossy().into_owned() + "\\";

        let all = browse_hits(&dir_str, "");
        assert_eq!(all.len(), 3, "浏览只列这一层，不递归");
        // 空过滤词 ⇒ 同分，按名称长度排：alpha.txt / beta.md / gamma 里短名在前。
        assert_eq!(all[0].title, "gamma");
        let filtered = browse_hits(&dir_str, "be");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].title, "beta.md");
        assert!(browse_hits(&dir_str, "nomatch").is_empty());
        assert!(
            browse_hits(r"Z:\no-such-dir-9f3a1c\", "").is_empty(),
            "读不到的目录给空清单而不是崩"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn excluded_dirs_are_named_by_the_risk_table() {
        // 开发计划第七章点名的两类爆炸源必须在排除表里，否则范围一旦扩大就会索引几十万条。
        for name in ["node_modules", ".git", "__pycache__"] {
            assert!(
                EXCLUDE_DIRS.iter().any(|x| x.eq_ignore_ascii_case(name)),
                "{name} 必须在排除表里"
            );
        }
    }

    #[test]
    fn walk_depth_limit_and_entry_cap_hold() {
        // 深度 1 ⇒ 只到直接子项，孙子不进索引。这里不用 SCOPE_DEPTH 而是显式传 1，
        // 因为常量是"范围有多大"的策略，不是"递归能不能停"的正确性。
        let dir = std::env::temp_dir().join("egui-launcher-filesearch-walk");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("child")).expect("建两层目录");
        std::fs::write(dir.join("child").join("leaf.txt"), b"x").expect("写测试文件");
        let mut out = Vec::new();
        walk(&dir, 2, &mut out);
        assert!(out.iter().any(|e| e.name == "child" && e.is_dir));
        assert!(out.iter().any(|e| e.name == "leaf.txt"));
        let mut shallow = Vec::new();
        walk(&dir, 1, &mut shallow);
        assert!(shallow.iter().any(|e| e.name == "child"));
        assert!(
            !shallow.iter().any(|e| e.name == "leaf.txt"),
            "深度到顶就该停"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scope_is_three_user_folders_not_the_whole_disk() {
        // 这条锁住"范围"这个决定本身：将来谁把它扩到用户根目录，这条测试会红，
        // 逼他先去处理排除规则与设置页（开发计划第七章那条高风险项）。
        assert_eq!(SCOPE_FOLDER_NAMES.len(), 6);
        assert_eq!(SCOPE_DEPTH, 4);
        assert_eq!(MAX_ENTRIES, 30_000);
        assert_eq!(MIN_QUERY_LEN, 2);
        assert_eq!(MAX_RESULTS, 30);
        assert_eq!(REBUILD_INTERVAL, Duration::from_secs(30 * 60));
    }
}

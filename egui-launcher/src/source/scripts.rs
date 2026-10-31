//! FR-SRC-15 脚本目录：`Scripts\` 里的脚本变成可搜索命令。
//!
//! 行为口径照 AHK 版 `ScriptProvider.ahk`：扩展名白名单、头部注释元数据（`@altrun.title/keyword/
//! argument/mode`）、三种运行模式、改动约 2 秒内生效、首次启用生成 3 个示例。
//!
//! 与本程序其它源的接法：脚本**不进清单**（不写 `user_items.json`、不参与学习排序、没有
//! 编辑/删除），因此它不是 `rows()` 的下标，而是头段行（[`SourceRow::Script`]）——与计算器、
//! 网页、文件、终端、速查表同一形态。代价是头段行永远排在应用结果之前（AHK 是 `score+10`
//! 混排），本批按架构口径接受这条偏差，至多 [`MAX_ROWS`] 行。
//!
//! 执行分三条路（`mode`）：`window` 起一个有窗口的进程；`silent` / `output` 用 `cmd /c`
//! 把输出重定向到临时 txt（隐藏窗口），由调用方开一个等待线程收尾。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime};

use crate::matching::{self, Parsed};

use super::application;
use super::{DataSource, SearchCtx, SourceId, SourceRow};

/// 支持的扩展名（小写，不带点）。与 AHK `ScriptProvider.Extensions` 同一张表。
pub const SCRIPT_EXTS: [&str; 5] = ["ahk", "ps1", "bat", "cmd", "py"];

/// 元数据只读文件开头这么多行（需求书 FR-SRC-15「头部注释元数据」；AHK 同为 30 行）。
const HEADER_LINES: usize = 30;
/// 头部采样上限：够装 30 行、又不把一个大脚本整个读进内存。
const HEAD_BYTES: u64 = 64 * 1024;
/// 时间戳检查节流（需求书「时间戳检查节流 2s」）。
const REFRESH_EVERY: Duration = Duration::from_secs(2);
/// 头段最多出几行脚本：头段排在应用结果之前，条数多了一屏就全是脚本。
const MAX_ROWS: usize = 5;
/// 后台脚本最多等多久（AHK `TimeoutMs := 120000`）。
pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(120);

/// 运行模式。`window`＝正常开窗；`silent`＝后台跑完把最后一行输出做通知；
/// `output`＝后台跑完用默认程序打开全部输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScriptMode {
    #[default]
    Window,
    Silent,
    Output,
}

impl ScriptMode {
    pub const ALL: [ScriptMode; 3] = [Self::Window, Self::Silent, Self::Output];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Silent => "silent",
            Self::Output => "output",
        }
    }

    /// 不认识的写法返回 `None`，由调用方回落 [`Default`]（window）——与 `TermShell::from_name`
    /// 同一形状：一个写错的 `mode` 不该让整条脚本消失。
    pub fn from_name(name: &str) -> Option<Self> {
        let want = name.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|m| m.as_str() == want)
    }
}

/// 从注释里读出来的一条脚本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEntry {
    pub path: PathBuf,
    /// 文件名（带扩展名）——行上那半灰的副标题就是它，同屏多个脚本靠它认出哪一个。
    pub file_name: String,
    /// 小写扩展名，决定怎么起进程。
    pub ext: String,
    /// 显示名：`@altrun.title`，缺省是去扩展名的文件名。
    pub title: String,
    /// 触发码（已小写）；空串＝没有。
    pub keyword: String,
    /// 参数提示文字；空串＝不需要参数。
    pub argument: String,
    pub mode: ScriptMode,
    /// 标题的拼音首字母，参与匹配（中文标题敲 `wd` 也能命中）。
    pub pinyin: String,
}

impl ScriptEntry {
    /// 白名单外的扩展名、读不出的文件 ⇒ `None`（目录里混着别的文件是常态）。
    pub fn from_path(path: &Path) -> Option<ScriptEntry> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        if !SCRIPT_EXTS.contains(&ext.as_str()) {
            return None;
        }
        let file_name = path.file_name()?.to_string_lossy().to_string();
        let stem = path.file_stem()?.to_string_lossy().to_string();
        let meta = parse_header(&read_head(path)?);
        let title = meta.title.unwrap_or(stem);
        let pinyin = application::default_keyword(&title);
        Some(ScriptEntry {
            path: path.to_path_buf(),
            file_name,
            ext,
            title,
            keyword: meta.keyword.unwrap_or_default().trim().to_ascii_lowercase(),
            argument: meta.argument.unwrap_or_default().trim().to_string(),
            mode: meta.mode.as_deref().and_then(ScriptMode::from_name).unwrap_or_default(),
            pinyin,
        })
    }
}

/// 头部注释里找到的四个字段（全可选）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Meta {
    pub title: Option<String>,
    pub keyword: Option<String>,
    pub argument: Option<String>,
    pub mode: Option<String>,
}

fn header_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        // 与 AHK 那条同一个形状：注释标记（`;` `#` `//` `::` `rem`）后面跟 `@altrun.键 值`。
        regex::Regex::new(r"(?i)^\s*(?:;|#|//|::|rem\b)\s*@altrun\.(\w+)\s+(.*?)\s*$")
            .expect("脚本元数据正则写错了")
    })
}

/// 前 [`HEADER_LINES`] 行里的 `@altrun.*`；后面的同名行覆盖前面的（AHK `Map` 赋值同口径）。
/// 空值当没写——`@altrun.title` 后面什么都不跟，不该把标题清成空串。
pub fn parse_header(text: &str) -> Meta {
    let mut meta = Meta::default();
    for line in text.lines().take(HEADER_LINES) {
        let Some(cap) = header_re().captures(line) else {
            continue;
        };
        let key = cap[1].to_ascii_lowercase();
        let value = cap[2].trim().to_string();
        if value.is_empty() {
            continue;
        }
        match key.as_str() {
            "title" => meta.title = Some(value),
            "keyword" => meta.keyword = Some(value),
            "argument" => meta.argument = Some(value),
            "mode" => meta.mode = Some(value),
            _ => {}
        }
    }
    meta
}

/// 只读开头 [`HEAD_BYTES`] 字节。UTF-8 BOM 先摘掉，否则第一行的 `@altrun` 前面会多一个
/// 零宽字符、正则匹配不到（记事本存 UTF-8 默认带 BOM）。非 UTF-8 用有损转换——宁可标题
/// 是乱码，也不要整条脚本消失。
fn read_head(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    file.take(HEAD_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).to_string();
    Some(text.strip_prefix('\u{feff}').unwrap_or(&text).to_string())
}

fn is_script(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SCRIPT_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// 目录的「内容指纹」：文件名 + 修改时间。任何新增 / 改名 / 保存都会变。
/// 只看脚本扩展名——目录里顺手放的 readme.txt 改一下不该触发重扫。
pub fn stamp(dir: &Path) -> String {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if !is_script(&path) {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        parts.push(format!(
            "{}\u{1}{mtime}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    parts.sort();
    parts.join("\u{2}")
}

/// 扫一遍目录。单个文件读不出就跳过——一条坏脚本不该让整个目录消失。
pub fn load_dir(dir: &Path) -> Vec<ScriptEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<ScriptEntry> = Vec::new();
    for entry in rd.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        if let Some(e) = ScriptEntry::from_path(&entry.path()) {
            out.push(e);
        }
    }
    out.sort_by(|a, b| {
        a.file_name
            .to_ascii_lowercase()
            .cmp(&b.file_name.to_ascii_lowercase())
    });
    out
}

/// 一条脚本对整次查询的打分：标题 / 关键字 / 标题拼音首字母取最优，档位与清单同一张表。
pub fn score_entry(p: &Parsed, e: &ScriptEntry) -> Option<(u8, f32)> {
    let mut best = matching::score_text(p, &e.title);
    for extra in [e.keyword.as_str(), e.pinyin.as_str()] {
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

/// 一行可执行的脚本。`argument` 非空＝独占行，参数已随查询填好。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptHit {
    pub path: PathBuf,
    pub title: String,
    pub subtitle: String,
    pub ext: String,
    pub mode: ScriptMode,
    pub argument: String,
}

impl ScriptHit {
    fn of(e: &ScriptEntry) -> Self {
        Self {
            path: e.path.clone(),
            title: e.title.clone(),
            subtitle: e.file_name.clone(),
            ext: e.ext.clone(),
            mode: e.mode,
            argument: String::new(),
        }
    }

    /// 独占行：「只有这一条命令，参数是你刚敲的」。
    fn exclusive(e: &ScriptEntry, argument: &str) -> Self {
        Self {
            title: format!("{}: {argument}", e.title),
            argument: argument.to_string(),
            ..Self::of(e)
        }
    }
}

/// 不可执行的补全提示行：整串就是某个需要参数的脚本关键字，Enter 补全成 `keyword `。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptHint {
    pub title: String,
    pub subtitle: String,
    pub completion: String,
}

/// 本批脚本目录的现场。
pub struct ScriptSource {
    /// `None`＝还没初始化过（单测里直接 [`ScriptSource::with_dir`] 的除外）。
    dir: Option<PathBuf>,
    entries: Vec<ScriptEntry>,
    stamp: String,
    checked: Instant,
}

impl Default for ScriptSource {
    fn default() -> Self {
        Self {
            dir: None,
            entries: Vec::new(),
            stamp: String::new(),
            // 首帧就允许检查一次：呼出时目录可能刚被改过。
            checked: Instant::now() - REFRESH_EVERY,
        }
    }
}

impl ScriptSource {
    /// 单测用：直接钉一个目录（不碰真实数据目录，也不生成示例）。
    #[cfg(test)]
    pub fn with_dir(dir: PathBuf) -> Self {
        let mut s = Self {
            dir: Some(dir),
            ..Self::default()
        };
        s.load();
        s
    }

    fn load(&mut self) {
        let Some(dir) = self.dir.clone() else { return };
        self.entries = load_dir(&dir);
        self.stamp = stamp(&dir);
        self.checked = Instant::now();
    }

    /// 目录里此刻该出的脚本行。空查询不出行（否则一呼出就满屏脚本，AHK 同样不列）。
    fn rows_for(&self, query: &str) -> Vec<SourceRow> {
        let p = matching::parse(query);
        if p.words.is_empty() {
            return Vec::new();
        }
        // 整串＝某条脚本的关键字、且它声明了参数 ⇒ 补一行补全提示；那条脚本自己不再
        // 单独出可执行行（否则同一个东西在一屏里出现两次）。
        let hint = if p.has_rest {
            None
        } else {
            self.entries.iter().position(|e| {
                !e.keyword.is_empty() && !e.argument.is_empty() && e.keyword == p.full
            })
        };
        let mut ranked: Vec<(u8, f32, usize)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if Some(i) == hint {
                continue;
            }
            if let Some((tier, score)) = score_entry(&p, e) {
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
        });
        ranked.truncate(MAX_ROWS);
        let mut rows: Vec<SourceRow> = Vec::new();
        if let Some(i) = hint {
            let e = &self.entries[i];
            rows.push(SourceRow::ScriptHint(ScriptHint {
                title: crate::i18n::tfmt("script.hint.title", &[&e.title]),
                subtitle: crate::i18n::tfmt("script.hint.subtitle", &[&e.keyword, &e.argument]),
                completion: format!("{} ", e.keyword),
            }));
        }
        rows.extend(
            ranked
                .iter()
                .map(|&(_, _, i)| SourceRow::Script(ScriptHit::of(&self.entries[i]))),
        );
        rows
    }

    /// 独占：首词＝某脚本关键字、声明了参数、参数非空。
    fn exclusive_for(&self, query: &str) -> Option<SourceRow> {
        let p = matching::parse(query);
        if !p.has_rest || p.keyword.is_empty() {
            return None;
        }
        let argument = p.rest.trim();
        if argument.is_empty() {
            return None;
        }
        self.entries
            .iter()
            .find(|e| !e.keyword.is_empty() && !e.argument.is_empty() && e.keyword == p.keyword)
            .map(|e| SourceRow::Script(ScriptHit::exclusive(e, argument)))
    }
}

impl DataSource for ScriptSource {
    fn id(&self) -> SourceId {
        SourceId::Scripts
    }

    fn label(&self) -> &'static str {
        crate::i18n::t("source.scripts.label")
    }

    fn note(&self) -> &'static str {
        crate::i18n::t("source.scripts.note")
    }

    fn init(&mut self) {
        if self.dir.is_none() {
            self.dir = Some(application::data_dir().join("Scripts"));
        }
        let Some(dir) = self.dir.clone() else { return };
        if !dir.exists() {
            match std::fs::create_dir_all(&dir) {
                Ok(()) => match write_examples(&dir) {
                    Ok(n) => log_info!("scripts: 首次启用，已生成 {n} 个示例脚本"),
                    Err(e) => log_info!("scripts: 示例脚本写入失败（{}）", e.kind()),
                },
                Err(e) => log_info!("scripts: 建目录失败（{}）", e.kind()),
            }
        }
        self.load();
        log_info!("scripts: 已载入 {} 个脚本", self.entries.len());
    }

    fn poll(&mut self) -> bool {
        let Some(dir) = self.dir.clone() else {
            return false;
        };
        if self.checked.elapsed() < REFRESH_EVERY {
            return false;
        }
        self.checked = Instant::now();
        let now = stamp(&dir);
        if now == self.stamp {
            return false;
        }
        self.load();
        log_info!("scripts: 目录有变化，重扫得 {} 条", self.entries.len());
        true
    }

    fn take_over(&mut self, cx: &mut SearchCtx<'_>) -> Option<Vec<SourceRow>> {
        self.exclusive_for(cx.query).map(|row| vec![row])
    }

    fn head(&mut self, cx: &mut SearchCtx<'_>) -> Vec<SourceRow> {
        self.rows_for(cx.query)
    }
}

// ---- 执行 ----

/// [`spawn`] 的两种下场：`window` 已经起来了；`silent` / `output` 还给调用方一个子进程
/// 与捕获文件，由调用方开等待线程（那里才有时钟、通道与 `egui::Context`）。
pub enum Spawned {
    Started,
    Capturing { child: Child, out: PathBuf },
}

/// 等出来的结论（等待线程 → 主线程）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// 脚本文档里写的标题（Toast 里那句「完成」点名的是它，不是路径）。
    pub title: String,
    pub mode: ScriptMode,
    /// 捕获文件。`output` 模式要拿它去交给 `.txt` 的默认程序（AHK 同口径，文件留着）；
    /// 其余几种在等待线程里已经删掉了。
    pub out: PathBuf,
    /// 捕获文件最后一行非空内容（`silent` 模式就是它当通知）。
    pub output: String,
    /// 到 [`SCRIPT_TIMEOUT`] 还没结束 ⇒ 已强杀。
    pub timed_out: bool,
}

/// 等子进程结束，最多 `timeout`。超时或等待出错 ⇒ 杀掉并返回 `Err`。
///
/// `std` 没有 `wait_timeout`（`Child::wait` 会一直挂着）；轮询间隔取得比 AHK 那档 300 ms 短，
/// 是为了让 Toast 在脚本刚结束那一瞬间就出来。
pub fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<(), ()> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(());
            }
        }
    }
}

/// 起进程。工作目录＝脚本自己所在的目录（AHK `Run(command, dir)` 同口径）。
pub fn spawn(hit: &ScriptHit) -> std::io::Result<Spawned> {
    if hit.mode == ScriptMode::Window {
        spawn_window(hit)?;
        return Ok(Spawned::Started);
    }
    spawn_capture(hit)
}

fn spawn_window(hit: &ScriptHit) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    let argument = hit.argument.trim();
    let mut cmd;
    if hit.ext == "bat" || hit.ext == "cmd" {
        // AHK 同口径：`cmd /c "整条命令行"`。外圈这对引号是 cmd 剥引号规则的锚点——
        // 少了它，`cmd /c "带 空格.bat" "参数"` 会被剥掉首尾各一个引号，命令行被拆坏。
        cmd = Command::new("cmd.exe");
        cmd.raw_arg(format!("/c \"{}\"", inner_command(hit)));
    } else {
        let (program, prefix) = match hit.ext.as_str() {
            // `.ps1` 的默认关联通常是记事本 ⇒ 不能交给 ShellExecute，必须点名解释器。
            "ps1" => (
                "powershell.exe",
                vec![
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ],
            ),
            "py" => ("py", Vec::new()),
            _ => {
                if argument.is_empty() {
                    // E-13：不检查 AHK 运行时，也不自己找解释器——没装就让 OS 弹关联框。
                    let dir = hit.path.parent().and_then(|p| p.to_str());
                    return crate::shell_execute(&hit.path.to_string_lossy(), "", dir)
                        .map_err(|e| std::io::Error::other(e.to_string()));
                }
                ("AutoHotkey.exe", Vec::new())
            }
        };
        cmd = Command::new(program);
        cmd.args(prefix).arg(&hit.path);
        if !argument.is_empty() {
            cmd.arg(argument);
        }
    }
    if let Some(dir) = hit.path.parent() {
        cmd.current_dir(dir);
    }
    cmd.creation_flags(CREATE_NEW_CONSOLE);
    cmd.spawn().map(|_| ())
}

fn spawn_capture(hit: &ScriptHit) -> std::io::Result<Spawned> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let out = temp_out_path();
    // 整条命令行（含 `>` 重定向）再包一对引号，与 AHK `ScriptProvider.Run` 同一写法：
    // 不包的话 cmd 的「首字符是引号 ⇒ 首尾各剥一个」会把脚本路径和重定向目标拆坏，
    // 脚本根本没跑、临时文件也没生成（2026-10-31 真机取证：裸写法 exit=1、文件不存在）。
    let line = format!(
        "\"{} > \"{}\" 2>&1\"",
        inner_command(hit),
        out.display()
    );
    let mut cmd = Command::new("cmd.exe");
    // `raw_arg` 原样接在 `/c` 后面：cmd 需要看到整条命令行（含 `>` 重定向），
    // 按参数数组传会被引号规则拆坏。
    cmd.arg("/c").raw_arg(line);
    if let Some(dir) = hit.path.parent() {
        cmd.current_dir(dir);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.creation_flags(CREATE_NO_WINDOW);
    let child = cmd.spawn()?;
    Ok(Spawned::Capturing { child, out })
}

/// 捕获模式下真正跑的那条命令行（不含重定向）。
fn inner_command(hit: &ScriptHit) -> String {
    let path = hit.path.to_string_lossy();
    let args = quote_arg(&hit.argument);
    match hit.ext.as_str() {
        // 不写全路径：解释器在不在 `PATH` 里由 cmd 判，找不到时那行报错本身就是给用户的答案。
        "ahk" => format!("AutoHotkey.exe \"{path}\"{args}"),
        "ps1" => format!("powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"{path}\"{args}"),
        "py" => format!("py \"{path}\"{args}"),
        _ => format!("\"{path}\"{args}"),
    }
}

/// 参数整段包一对引号（用户自己写的引号转义掉），与 AHK `CommandLine` 同一写法。
fn quote_arg(argument: &str) -> String {
    let text = argument.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!(" \"{}\"", text.replace('"', "\\\""))
    }
}

fn temp_out_path() -> PathBuf {
    let tick = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "egui-launcher-script-{}-{tick}.txt",
        std::process::id()
    ))
}

/// 捕获文件的最后一行非空内容（`silent` 模式拿它做通知）。读不出 ⇒ 空串。
pub fn read_last_line(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

/// `silent` 模式的临时文件用完即删；`output` 模式留给用户（`.txt` 的默认程序打开的就是它）。
pub fn remove_temp(path: &Path) {
    let _ = std::fs::remove_file(path);
}

// ---- 示例脚本 ----

/// 首次启用生成的三个示例：各演示一种模式（silent / keyword+argument / output）。
/// 只用 ASCII——Windows PowerShell 5 按 ANSI 读不带 BOM 的文件，中文会变乱码。
/// 文件内容里的 `\n` 在落盘前统一换成 CRLF（cmd 的 `rem` 行、PowerShell 的注释都一样吃）。
const EXAMPLES: [(&str, &str); 3] = [
    (
        "Today.ahk",
        "; @altrun.title  Example: Today's Date
; @altrun.mode   silent
; Runs in the background; the last line it prints is shown as a notification.
; Change the mode to output to read everything it prints, or remove the line to run in a window.
FileAppend(FormatTime(, \"dddd, d MMMM yyyy\"), \"*\")
",
    ),
    (
        "Port.bat",
        "@echo off
rem @altrun.title     Example: Who Uses a Port
rem @altrun.keyword   port
rem @altrun.argument  a port number
rem Type \"port 8080\" in the launcher: the text after the keyword is passed to this script as %1.
rem No mode line: it runs normally in its own window.
netstat -ano | findstr /c:\":%~1 \"
pause
",
    ),
    (
        "IP Addresses.ps1",
        "# @altrun.title  Example: IP Addresses
# @altrun.mode   output
# Runs in the background, then opens everything it prints in your text editor.
Get-NetIPAddress -AddressFamily IPv4 | Where-Object IPAddress -ne '127.0.0.1' | Format-Table InterfaceAlias, IPAddress -AutoSize
",
    ),
];

/// 目录里已经有同名文件就不覆盖（用户改过的示例不能被"重装"覆盖掉）。
fn write_examples(dir: &Path) -> std::io::Result<usize> {
    let mut written = 0;
    for (name, body) in EXAMPLES {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        // 不带 BOM：cmd 读到 BOM 会把第一行当成命令名报错。
        std::fs::write(&path, body.replace('\n', "\r\n"))?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let tick = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "egui-launcher-test-{}-{tick}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("建测试目录");
        dir
    }

    #[test]
    fn header_metadata_accepts_every_comment_marker_and_is_case_insensitive() {
        let text = "\
; @altrun.title  My Tool
# @altrun.keyword  MT
// @altrun.argument  a path
:: @altrun.mode  Silent
rem @ALTRUN.mode  output
";
        let meta = parse_header(text);
        assert_eq!(meta.title.as_deref(), Some("My Tool"));
        assert_eq!(meta.keyword.as_deref(), Some("MT"));
        assert_eq!(meta.argument.as_deref(), Some("a path"));
        assert_eq!(
            meta.mode.as_deref(),
            Some("output"),
            "后面的同名行覆盖前面的"
        );
        assert_eq!(ScriptMode::from_name("OUTPUT"), Some(ScriptMode::Output));
        assert_eq!(ScriptMode::from_name("silent"), Some(ScriptMode::Silent));
        assert_eq!(ScriptMode::from_name("bash"), None, "不认识的交给回车默认");
    }

    #[test]
    fn metadata_only_reads_the_first_thirty_lines() {
        let mut text = String::new();
        for _ in 0..29 {
            text.push_str("; filler\n");
        }
        text.push_str("; @altrun.title  In Range\n");
        text.push_str("; @altrun.keyword  late\n");
        let meta = parse_header(&text);
        assert_eq!(meta.title.as_deref(), Some("In Range"), "第 30 行还算头部");
        assert_eq!(meta.keyword, None, "第 31 行的不算");
    }

    #[test]
    fn only_whitelisted_extensions_become_entries_and_defaults_apply() {
        let dir = scratch("ext");
        std::fs::write(dir.join("a.py"), "print(1)\n").unwrap();
        std::fs::write(dir.join("B.PS1"), "Write-Output 1\n").unwrap();
        std::fs::write(dir.join("c.txt"), "; @altrun.title nope\n").unwrap();
        std::fs::write(dir.join("noext"), "").unwrap();

        let entries = load_dir(&dir);
        let names: Vec<&str> = entries.iter().map(|e| e.file_name.as_str()).collect();
        // 排序按文件名的**小写**排：`a.py` 在 `B.PS1` 之前（扩展名大小写不影响次序）。
        assert_eq!(names, vec!["a.py", "B.PS1"], "白名单外的一个都不收");
        let py = entries.iter().find(|e| e.ext == "py").expect("py 那条");
        assert_eq!(py.title, "a", "缺省标题＝去扩展名的文件名");
        assert_eq!(py.mode, ScriptMode::Window, "缺省模式＝window");
        assert_eq!(py.keyword, "");
        assert_eq!(py.argument, "");
        let ps1 = entries.iter().find(|e| e.ext == "ps1").expect("ps1 那条");
        assert_eq!(ps1.title, "B", "扩展名大小写不影响识别");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bom_and_non_utf8_headers_do_not_hide_the_metadata() {
        let dir = scratch("bom");
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(b"; @altrun.keyword  withbom\r\n");
        std::fs::write(dir.join("bom.bat"), with_bom).unwrap();
        std::fs::write(dir.join("gbk.bat"), b"; @altrun.keyword  gbk\r\n\xD6\xD0\xCE\xC4\r\n").unwrap();

        let entries = load_dir(&dir);
        let bom = entries.iter().find(|e| e.file_name == "bom.bat").unwrap();
        assert_eq!(bom.keyword, "withbom", "BOM 不能把第一行的注释挡掉");
        let gbk = entries.iter().find(|e| e.file_name == "gbk.bat").unwrap();
        assert_eq!(gbk.keyword, "gbk", "后面有非 UTF-8 字节也要读得出元数据");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stamp_changes_when_a_script_is_added_or_touched() {
        let dir = scratch("stamp");
        let a = dir.join("a.py");
        std::fs::write(&a, "print(1)\n").unwrap();
        let first = stamp(&dir);
        assert!(!first.is_empty());
        assert_eq!(first, stamp(&dir), "没动过就是同一个指纹");

        std::fs::write(dir.join("b.bat"), "echo hi\n").unwrap();
        let second = stamp(&dir);
        assert_ne!(first, second, "新增一个脚本指纹必须变");

        // 改名也算变化（文件名进指纹）。
        std::fs::rename(dir.join("b.bat"), dir.join("c.bat")).unwrap();
        assert_ne!(second, stamp(&dir), "改名指纹必须变");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scoring_uses_title_keyword_and_pinyin_initials() {
        let dir = scratch("score");
        std::fs::write(
            dir.join("port.bat"),
            "rem @altrun.title     Who Uses a Port\r\nrem @altrun.keyword   port\r\nrem @altrun.argument  a port number\r\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("cn.py"),
            "; @altrun.title  文档整理\n; @altrun.keyword  dw\r\n",
        )
        .unwrap();
        let src = ScriptSource::with_dir(dir.clone());

        let p = matching::parse("port");
        let e = src.entries.iter().find(|e| e.file_name == "port.bat").unwrap();
        let (tier, score) = score_entry(&p, e).expect("关键字整串命中");
        assert_eq!(tier, 6, "整串相等是 100 档");
        assert_eq!(score, 100.0);

        let p = matching::parse("whoport");
        let (_t, loose) = score_entry(&p, e).expect("子序列兜底");
        assert!(loose > 0.0, "title 的子序列也能命中：{loose}");

        let cn = src.entries.iter().find(|e| e.file_name == "cn.py").unwrap();
        assert_eq!(cn.pinyin, "wdzl");
        assert!(
            score_entry(&matching::parse("wd"), cn).is_some(),
            "中文标题的拼音首字母参与匹配"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exclusive_row_needs_keyword_rest_and_a_declared_argument() {
        let dir = scratch("excl");
        std::fs::write(
            dir.join("port.bat"),
            "rem @altrun.keyword   port\r\nrem @altrun.argument  a port number\r\n",
        )
        .unwrap();
        std::fs::write(dir.join("plain.bat"), "rem @altrun.keyword  plain\r\necho hi\r\n").unwrap();
        let src = ScriptSource::with_dir(dir.clone());

        let row = src.exclusive_for("port 8080").expect("关键字 + 参数 ⇒ 独占");
        match row {
            SourceRow::Script(hit) => {
                assert_eq!(hit.argument, "8080");
                assert_eq!(hit.title, "port: 8080", "标题带参数，与 AHK 同一写法");
                assert_eq!(hit.subtitle, "port.bat");
            }
            other => panic!("应为脚本行，实际 {other:?}"),
        }
        assert!(src.exclusive_for("port").is_none(), "只有关键字 ⇒ 不出独占行");
        assert!(src.exclusive_for("port   ").is_none(), "空格后面没有参数也不算");
        assert!(
            src.exclusive_for("plain x").is_none(),
            "没声明 argument 的脚本不吃独占"
        );
        assert!(src.exclusive_for("portt 8080").is_none(), "关键字要整词相等");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exact_keyword_with_argument_yields_a_completion_hint_not_a_duplicate_row() {
        let dir = scratch("hint");
        std::fs::write(
            dir.join("port.bat"),
            "rem @altrun.title     Port\r\nrem @altrun.keyword   port\r\nrem @altrun.argument  a port number\r\n",
        )
        .unwrap();
        std::fs::write(dir.join("portable.bat"), "rem @altrun.title  Portable\r\n").unwrap();
        let src = ScriptSource::with_dir(dir.clone());

        let rows = src.rows_for("port");
        match rows.first() {
            Some(SourceRow::ScriptHint(hint)) => {
                assert_eq!(hint.completion, "port ", "补全成「关键字 + 空格」");
                assert!(hint.title.contains("Port"), "标题带上脚本名：{}", hint.title);
            }
            other => panic!("第一行应是补全提示，实际 {other:?}"),
        }
        assert!(
            rows.iter().all(|r| !matches!(r, SourceRow::Script(h) if h.subtitle == "port.bat")),
            "同一条脚本不许在提示行之外再出一行"
        );
        assert!(
            rows.iter().any(|r| matches!(r, SourceRow::Script(h) if h.subtitle == "portable.bat")),
            "别的脚本的模糊命中照常出现"
        );

        assert!(src.rows_for("").is_empty(), "空查询不列脚本");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rows_are_capped_so_the_head_never_fills_the_screen() {
        let dir = scratch("cap");
        for i in 0..9 {
            std::fs::write(
                dir.join(format!("item{i}.bat")),
                format!("rem @altrun.title  Item {i}\r\n"),
            )
            .unwrap();
        }
        let src = ScriptSource::with_dir(dir.clone());
        assert!(src.rows_for("item").len() <= MAX_ROWS);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn examples_are_ascii_crlf_without_bom_and_never_overwrite() {
        let dir = scratch("examples");
        assert_eq!(write_examples(&dir).expect("写示例"), 3);
        for (name, _) in EXAMPLES {
            let bytes = std::fs::read(dir.join(name)).unwrap();
            assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "{name} 不许带 BOM");
            assert!(bytes.is_ascii(), "{name} 只用 ASCII（PS 5 按 ANSI 读）");
            assert!(
                bytes.windows(2).any(|w| w == b"\r\n"),
                "{name} 用 CRLF 换行"
            );
            assert!(
                !bytes
                    .iter()
                    .enumerate()
                    .any(|(i, &b)| b == b'\n' && (i == 0 || bytes[i - 1] != b'\r')),
                "{name} 里不许有裸 LF"
            );
        }
        // 用户改过的示例不能被再次覆盖。
        std::fs::write(dir.join("Today.ahk"), "edited\n").unwrap();
        assert_eq!(write_examples(&dir).expect("第二次写示例"), 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("Today.ahk")).unwrap(),
            "edited\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn command_lines_match_the_reference_implementation() {
        let hit = |ext: &str, argument: &str| ScriptHit {
            path: PathBuf::from("C:\\scripts\\my tool.bat"),
            title: "t".into(),
            subtitle: String::new(),
            ext: ext.into(),
            mode: ScriptMode::Silent,
            argument: argument.into(),
        };
        assert_eq!(
            inner_command(&hit("ps1", "x y")),
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"C:\\scripts\\my tool.bat\" \"x y\""
        );
        assert_eq!(
            inner_command(&hit("py", "")),
            "py \"C:\\scripts\\my tool.bat\""
        );
        assert_eq!(
            inner_command(&hit("bat", "8080")),
            "\"C:\\scripts\\my tool.bat\" \"8080\"",
            "bat/cmd 直接跑路径本身"
        );
        assert_eq!(
            inner_command(&hit("ahk", "")),
            "AutoHotkey.exe \"C:\\scripts\\my tool.bat\""
        );
        // 用户自己写的引号要转义，否则会把命令行提前截断。
        assert_eq!(quote_arg("a\"b"), " \"a\\\"b\"");
    }

    #[test]
    fn last_line_skips_trailing_blank_lines() {
        let dir = scratch("lastline");
        let f = dir.join("out.txt");
        std::fs::write(&f, "first\r\nsecond\r\n\r\n   \r\n").unwrap();
        assert_eq!(read_last_line(&f), "second");
        std::fs::write(&f, "").unwrap();
        assert_eq!(read_last_line(&f), "");
        assert_eq!(read_last_line(&dir.join("missing.txt")), "");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 2026-10-31 真机缺陷的回归：捕获模式原来把 `script > temp 2>&1` 裸交给 `cmd /c`，
    /// cmd 的「首字符是引号 ⇒ 首尾各剥一个」把它拆坏——脚本没跑、临时文件也没生成，
    /// `silent` 于是永远报「已运行完毕」。这条测试跑的是真流程（起 cmd、等结束、读末行），
    /// 拆坏时它会读不到末行。
    #[test]
    fn capture_really_runs_the_script_and_reads_its_last_line() {
        let dir = scratch("capture");
        // 路径里带空格：这正是不包外层引号时被拆坏的形状。
        let bat = dir.join("run me.bat");
        std::fs::write(
            &bat,
            "@echo off\r\necho first line\r\necho CAPTURE-LAST-913\r\n",
        )
        .unwrap();
        let hit = ScriptHit {
            path: bat,
            title: "t".into(),
            subtitle: String::new(),
            ext: "bat".into(),
            mode: ScriptMode::Silent,
            argument: String::new(),
        };
        let Spawned::Capturing { mut child, out } = spawn(&hit).expect("起捕获进程") else {
            panic!("silent 必须走捕获通道");
        };
        assert!(
            wait_with_timeout(&mut child, Duration::from_secs(20)).is_ok(),
            "测试脚本必须正常结束"
        );
        assert_eq!(
            read_last_line(&out),
            "CAPTURE-LAST-913",
            "脚本要真的跑起来、输出要真的落进临时文件"
        );
        remove_temp(&out);
        std::fs::remove_dir_all(&dir).ok();
    }
}

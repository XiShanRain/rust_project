//! 数据源：内置种子 + 用户按需清单（curated 模型，2026-10-16 需求转向），
//! 以及可选「批量导入」——后台扫描开始菜单/桌面 `.lnk`，解析出可启动目标。
//!
//! 转向说明：不再开机全扫 186 条快捷方式（噪音大、拖慢搜索）。改为
//! - 内置一批系统工具（计算器/画图/此电脑/回收站…）作底座；
//! - 用户清单 `user_items.json` 由「添加到清单」事件写入（SendTo / 应用内添加 / 批量导入），非自动重扫；
//! - 「批量导入全部」是显式、非默认的一次性动作，走 IShellLinkW 后台扫描。
//!
//! 解析仍用 `windows` 0.62.2 的 `IShellLinkW`（E-03 已实测选型，零新增 crate），
//! 在后台线程内 `CoInitializeEx` STA。本模块只产出纯数据 `LaunchItem`，不含 egui 类型。
//!
//! E-03 实测导出的 4 条约束已在批量扫描里落地：
//! 1. `%VAR%` 形态：`ExpandEnvironmentStringsW` 展开 target / working_dir；
//! 2. 图标空/负索引：本模块不取图标（`source::icon`），仅保证不因图标缺失而丢条目；
//! 3. Store/MSIX target 为空串：`resolve_lnk` 直接丢弃（留给 Get-StartApps 源）；
//! 4. 扫描先按扩展名过滤，单文件解析失败只跳过、不中断整体（NFR-04）。

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use serde::{Deserialize, Serialize};
use pinyin::ToPinyin;
use windows::core::Interface;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, IPersistFile, STGM_READ,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::UI::Shell::{
    IShellLinkW, SHChangeNotify, SLGP_UNCPRIORITY, SHCNE_UPDATEDIR, SHCNE_UPDATEITEM, SHCNF_PATHW,
};

/// `{00021401-0000-0000-C000-000000000046}`，coclass ShellLink（见 E-03 记录）。
const CLSID_SHELL_LINK: windows::core::GUID =
    windows::core::GUID::from_u128(0x00021401_0000_0000_C000_000000000046);

/// 条目类型（对齐 FR-SRC-05 的 `type`）。当前启动一律走 `ShellExecuteExW`（FR-ACT-02），
/// 故 kind 只用于清单落盘、去重与将来的分组/添加对话框，不参与启动分发。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    #[default]
    App,
    Command,
    Url,
    Folder,
    File,
}

impl ItemKind {
    /// 添加/编辑对话框的类型下拉可选项（顺序即展示顺序）。
    pub const ALL: [ItemKind; 5] = [
        ItemKind::App,
        ItemKind::Command,
        ItemKind::Url,
        ItemKind::Folder,
        ItemKind::File,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::App => "应用",
            ItemKind::Command => "命令",
            ItemKind::Url => "网址/协议",
            ItemKind::Folder => "文件夹",
            ItemKind::File => "文件",
        }
    }
}

/// 一条可启动项。字段全部是纯数据，供主线程转成界面条目 / 拼启动命令。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaunchItem {
    pub name: String,
    /// 交给 `ShellExecuteExW` 的 `lpFile`：exe 全路径或命令名、`.lnk` 目标、URL、`shell:` 文件夹、协议。
    pub path: String,
    pub arguments: String,
    pub working_dir: Option<String>,
    /// 图标来源路径（.lnk 或 exe）；空串表示无图标源。启动用 `path`，取图标用 `source_path`。
    pub source_path: String,
    pub kind: ItemKind,
    /// 别名（FR-SRC-22）：搜索时除 `name` 外也匹配它；`#[serde(default)]` 让旧清单缺字段仍可解析。
    #[serde(default)]
    pub keyword: String,
    /// FR-SRC-19 置顶：true=恒排在结果前列（📌），随清单落盘、重启仍在。
    #[serde(default)]
    pub pinned: bool,
    /// FR-SRC-19 学习排序：成功启动累计次数（只作用于排名，不加匹配总分）。
    #[serde(default)]
    pub use_count: u32,
    /// FR-SRC-19 学习排序：最近一次成功启动的 UNIX 毫秒；0=从未启动过。
    #[serde(default)]
    pub last_used_ms: i64,
}

impl LaunchItem {
    /// 条目身份 uid（§4.1.2 `custom:<type|target|arguments>`，**派生、不落盘**）。
    /// 阶段一只有「用户清单」一个数据源，故 provider 段恒为 `custom`。
    /// 必须带 `arguments`：同一 exe 配不同参数是两个不同条目，只按 `path` 判同会把
    /// 后一条静默吞掉（2026-10-17 实测 bug，dedup_tests 已锁）。
    pub fn uid(&self) -> String {
        format!(
            "custom:{}|{}|{}",
            self.kind.label(),
            self.path.trim().to_lowercase(),
            self.arguments.trim().to_lowercase()
        )
    }
}

/// 用户按需清单落盘位置（FR-CFG-03 便携优先，2026-10-17 实现与需求书对齐）：
/// exe 同级目录可写 → `exe 目录\user_items.json`，数据随程序目录走、重装不丢；
/// 不可写或取不到 exe 路径 → 退化 `%LOCALAPPDATA%\egui-launcher\user_items.json`。
/// 路径只解析一次；旧位置有数据且新位置不存在时一次性迁移（旧文件留 `.bak` 备份）。
fn user_list_path() -> PathBuf {
    static LIST_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    LIST_PATH.get_or_init(resolve_list_path).clone()
}

fn appdata_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir())
        .join("egui-launcher")
}

/// 以真实写入试探目录可写性（Program Files 等受 ACL 保护的目录会失败→自动退化）。
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(".write-probe.tmp");
    if std::fs::write(&probe, b"").is_ok() {
        let _ = std::fs::remove_file(&probe);
        true
    } else {
        false
    }
}

fn resolve_list_path() -> PathBuf {
    let portable = std::env::current_exe().ok().and_then(|exe| {
        exe.parent().map(|dir| (dir.to_path_buf(), dir.join("user_items.json")))
    });
    match portable {
        Some((dir, file)) if dir_writable(&dir) => {
            migrate_from_appdata(&file);
            file
        }
        _ => appdata_dir().join("user_items.json"),
    }
}

/// 一次性迁移：新位置不存在且旧位置（`%LOCALAPPDATA%`）有清单 → 复制到便携位置，旧文件改名 `.bak` 保留。
fn migrate_from_appdata(new_path: &Path) {
    let old = appdata_dir().join("user_items.json");
    if new_path.exists() || !old.exists() {
        return;
    }
    if std::fs::copy(&old, new_path).is_ok() {
        let bak = old.with_extension("json.bak");
        let _ = std::fs::rename(&old, &bak);
        eprintln!("source: 清单已迁移到便携位置 {new_path:?}（旧位置备份 {bak:?}）");
    }
}

/// 用户清单文件是否已存在（存在即视为用户已接管，哪怕被清空也不再用种子覆盖）。
pub fn user_list_exists() -> bool {
    user_list_path().exists()
}

/// 读取用户清单；文件缺失或解析失败返回空。
pub fn load_user_items() -> Vec<LaunchItem> {
    let path = user_list_path();
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Vec<LaunchItem>>(&bytes).unwrap_or_else(|e| {
            eprintln!("source: user_items.json 解析失败（忽略）: {e}");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// 覆盖写用户清单（调用方负责先剔除内置种子、只持久化用户新增项）。
pub fn save_user_items(items: &[LaunchItem]) {
    let path = user_list_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_vec_pretty(items) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("source: 写 user_items.json 失败 {path:?}: {e}");
            }
        }
        Err(e) => eprintln!("source: 序列化用户清单失败: {e}"),
    }
}

/// 用户偏好（`settings.json`，与清单同目录、同样便携优先）。
/// 只放真实生效的开关——不设无功能的占位项（设置页立规同口径）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct AppSettings {
    /// 空格执行选中项（FR-ACT-01 的 `space_to_run`，Delphi 版 ALTRun 同名字段）。
    /// 批次 J（2026-10-18 用户裁定）由 bool 扩成四档，见 [`SpaceRun`]。
    #[serde(default)]
    pub space_to_run: SpaceRun,

    /// Tab 键语义（FR-ACT-06，2026-10-18 用户要求两种口径都做实、由设置页二选一）。
    #[serde(default)]
    pub tab_mode: TabMode,

    /// 同一匹配档位之内谁排前面（FR-SRC-19 的学习位序，批次 K：甲乙都做、默认甲）。
    #[serde(default)]
    pub learn_rank: LearnRank,
}

/// 空格执行的四种判据。**为什么不是开关**：bool 只能表达"只有空框才执行"和"永远执行"
/// 两个极端，而"这个空格该执行还是该当文本"取决于此刻的查询与结果集，中间那些规则
/// 一个 bool 表达不了；扩成档位之后，默认档（[`SpaceRun::Word`]）才能既给单手确认、
/// 又保住多词查询里的空格。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpaceRun {
    /// 只在搜索框为空时执行（原 `false`）：多词查询完全不受影响，但单手流要按回车。
    Off,
    /// 默认（丙案）：框为空，**或**查询整串等于高亮行的名称／别名表里的某个词 ⇒ 执行。
    /// `jsb`（记事本别名）→ 空格执行；`visual` 不是任何条目的完整名称 → 空格是文本。
    #[default]
    Word,
    /// 乙案：框为空，**或**当前结果集只剩一条 ⇒ 执行。打到唯一候选即可确认，
    /// 但"唯一"不等于"打完"，想接着补词要用 `Shift+空格` 插空格。
    Unique,
    /// 任何时候空格都执行选中项（原 `true`，参考项目口径），代价是查询里打不出空格。
    Always,
}

impl SpaceRun {
    /// 落盘与设置页用的档位名（与 `TabMode` 的 snake_case 口径一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Word => "word",
            Self::Unique => "unique",
            Self::Always => "always",
        }
    }

    fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "off" => Self::Off,
            "word" => Self::Word,
            "unique" => Self::Unique,
            "always" => Self::Always,
            _ => return None,
        })
    }
}

/// 读盘时先按"旧 bool、后新档位名"两种写法试：`settings.json` 是用户手也能动过的文件，
/// 加字段之前的 `false`/`true` 必须照读且含义不变（false＝仅空框，true＝始终执行）。
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum SpaceRunWire {
    Legacy(bool),
    Name(String),
}

impl serde::Serialize for SpaceRun {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for SpaceRun {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match SpaceRunWire::deserialize(d)? {
            SpaceRunWire::Legacy(b) => Ok(if b { Self::Always } else { Self::Off }),
            SpaceRunWire::Name(n) => {
                Self::from_name(&n).ok_or_else(|| D::Error::custom(format!("未知的 space_to_run 档位：{n:?}")))
            }
        }
    }
}

/// Tab 键的两种口径。旧 `settings.json` 缺这个字段 ⇒ `#[serde(default)]` 回 [`TabMode::Complete`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabMode {
    /// 条件分流（默认）：查询首词是名称或别名的前缀 ⇒ Tab 补全；否则 ⇒ Tab 循环下一项。
    #[default]
    Complete,
    /// 专职导航（Delphi 版 ALTRun 口径）：Tab=下一项、Shift+Tab=上一项，补全不占键位。
    Navigate,
}

/// 匹配档位相同之后，学习位序怎么比（FR-SRC-19，批次 K 起因：默认档里"最近一次"完全压过
/// "累计次数"，实测 8 次的记事本被 117 秒前用过的 3 次命令提示符超到后面）。
/// **两种口径都只作用在同档位之内**：匹配质量永远是第一主键，次数不许去撬动它
/// （需求书 §2.5 已入档判据「学习排序只作用于排名，不直接加总分」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearnRank {
    /// 甲（默认）：累计次数多的在前，次数相同才看最近一次。
    #[default]
    Count,
    /// 乙：比热度 `log2(1 + 次数) × 0.5^(距上次使用的天数 / 半衰期)`。
    /// 取 `log2` 是为了压住量级——8 次不该是 1 次的 8 倍权重；
    /// 半衰期用 `0.5^(days/H)` 而不是 `e^(-days/H)`，后者名义 H 天的实际半衰期是 1.44H 天。
    Decay,
}

impl LearnRank {
    /// 落盘与设置页用的口径名（与 `SpaceRun`/`TabMode` 同口径）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Decay => "decay",
        }
    }
}

/// 偏好文件与清单同目录（复用 `user_list_path` 已解析好的便携判定）。
fn settings_path() -> PathBuf {
    user_list_path().with_file_name("settings.json")
}

/// 读取偏好；文件缺失或解析失败一律回默认值（NFR-04：坏配置不得影响启动）。
pub fn load_settings() -> AppSettings {
    match std::fs::read(settings_path()) {
        Ok(bytes) => serde_json::from_slice::<AppSettings>(&bytes).unwrap_or_else(|e| {
            eprintln!("source: settings.json 解析失败（用默认值）: {e}");
            AppSettings::default()
        }),
        Err(_) => AppSettings::default(),
    }
}

pub fn save_settings(s: &AppSettings) {
    let path = settings_path();
    match serde_json::to_vec_pretty(s) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("source: 写 settings.json 失败 {path:?}: {e}");
            }
        }
        Err(e) => eprintln!("source: 序列化偏好失败: {e}"),
    }
}

/// 内置系统工具底座（对齐 Delphi AltRun 保留的自带程序）。全部走 `ShellExecuteExW`：
/// 命令名靠 App Paths 解析、`shell:` 已知文件夹与 `ms-settings:` 协议原生支持。
/// `path` 用简洁命令名（启动），`source_path` 用 exe 全路径（取图标）；无固定 exe 的给空。
pub fn builtin_seeds() -> Vec<LaunchItem> {
    let windir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let sys32 = |exe: &str| windir.join("System32").join(exe).to_string_lossy().into_owned();

    let command = |name: &str, path: &str, exe: &str| LaunchItem {
        name: name.to_string(),
        path: path.to_string(),
        arguments: String::new(),
        working_dir: None,
        source_path: sys32(exe),
        kind: ItemKind::Command,
        keyword: default_keyword(name),
        ..Default::default()
    };
    let folder = |name: &str, path: &str| LaunchItem {
        name: name.to_string(),
        path: path.to_string(),
        arguments: String::new(),
        working_dir: None,
        source_path: String::new(),
        kind: ItemKind::Folder,
        keyword: default_keyword(name),
        ..Default::default()
    };

    vec![
        command("计算器", "calc.exe", "calc.exe"),
        command("记事本", "notepad.exe", "notepad.exe"),
        command("画图", "mspaint.exe", "mspaint.exe"),
        command("命令提示符", "cmd.exe", "cmd.exe"),
        command("PowerShell", "powershell.exe", "WindowsPowerShell/v1.0/powershell.exe"),
        command("任务管理器", "taskmgr.exe", "Taskmgr.exe"),
        command("控制面板", "control.exe", "control.exe"),
        folder("此电脑", "shell:MyComputerFolder"),
        folder("回收站", "shell:RecycleBinFolder"),
        LaunchItem {
            name: "设置".to_string(),
            path: "ms-settings:".to_string(),
            arguments: String::new(),
            working_dir: None,
            source_path: String::new(),
            kind: ItemKind::Url,
            keyword: default_keyword("设置"),
            ..Default::default()
        },
    ]
}

/// 按派生 [`LaunchItem::uid`] 去重（种子优先在前，批量导入的重复项被剔除）。
/// 早期版本按 `path` 去重 ⇒ 同 exe 不同参数的两条会合并成一条，2026-10-17 修。
pub fn dedup_items(items: Vec<LaunchItem>) -> Vec<LaunchItem> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|it| seen.insert(it.uid()))
        .collect()
}

/// 判断输入是否像 URL/协议（FR-SRC-23）：含 `://`、以 `www.` 起、或是 ≥2 字母的方案名后跟 `:`。
/// 单字母（盘符 `C:`）不算方案，避免把 `C:\path` 误判成 URL。
pub fn is_url(s: &str) -> bool {
    let t = s.trim();
    let low = t.to_lowercase();
    if low.contains("://") || low.starts_with("www.") {
        return true;
    }
    if let Some(idx) = t.find(':') {
        let scheme = &t[..idx];
        let head_alpha = scheme.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false);
        if scheme.len() >= 2 && head_alpha
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            return true; // ms-settings:、mailto: 等
        }
    }
    false
}

/// 把任意输入判成条目类型：URL/协议→`Url`，其余（命令名 / 路径 / .lnk）→`Command`。
pub fn infer_kind(input: &str) -> ItemKind {
    if is_url(input) { ItemKind::Url } else { ItemKind::Command }
}

/// 由名称生成默认别名（FR-SRC-22「默认拼音首字母」）：每个汉字取其拼音首字母
/// （计算器→jsq、控制面板→kzmb），名称里的 ASCII 字母数字原样小写拼接，其余字符跳过。
/// 若名称不含任何汉字则返回空串——纯英文名直接靠 `name.contains` 匹配，无需别名。
pub fn default_keyword(name: &str) -> String {
    if !name.chars().any(|c| c.to_pinyin().is_some()) {
        return String::new();
    }
    let mut out = String::new();
    for c in name.chars() {
        if let Some(py) = c.to_pinyin() {
            out.push_str(py.first_letter());
        } else if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// 提交即学别名（FR-SRC-19 补全，对标 Delphi `Execute(item, KeyWord)` 写 FavoriteList）：
/// 这次用这个词真的启动成功了，就把它攒成该条目的别名，下次直接敲它就命中。
///
/// 门槛比 Delphi 严：Delphi 无条件学，单字也学，于是 `j` 这类一词能让所有含 j 的查询
/// 都吃到 30 档噪声。这里要求 ≥2 字符、不等于名称、且尚未出现在别名里；只攒查询**首词**
/// （FR-QRY-01 的 keyword 那半，后面的参数词不是触发码）。返回是否改变 ⇒ 调用方据此决定写盘。
pub fn learn_alias(item: &mut LaunchItem, word: &str) -> bool {
    let w = word.trim().to_lowercase();
    if w.chars().count() < 2 || w == item.name.to_lowercase() {
        return false;
    }
    if item.keyword.split_whitespace().any(|a| a.to_lowercase() == w) {
        return false;
    }
    item.keyword =
        if item.keyword.trim().is_empty() { w } else { format!("{} {w}", item.keyword.trim()) };
    true
}

/// 把当前输入框内容生成一条可保存条目（FR-SRC-23「添加为快捷项」）。
/// `name` 取末段并去掉 `.lnk` 后缀；启动一律走 `ShellExecuteExW`，故 `path` 原样存输入。
pub fn make_item_from_input(input: &str) -> LaunchItem {
    let trimmed = input.trim();
    let tail = trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed);
    let name = tail.trim_end_matches(".lnk");
    let name = if name.is_empty() { trimmed.to_string() } else { name.to_string() };
    let keyword = default_keyword(&name);
    LaunchItem {
        name,
        path: trimmed.to_string(),
        arguments: String::new(),
        working_dir: None,
        source_path: String::new(),
        kind: infer_kind(trimmed),
        keyword,
        ..Default::default()
    }
}

/// 后台扫描开始菜单/桌面，把结果作为 `kind=App` 的条目一次性送回 `tx`（供「批量导入全部」）。
/// 不写缓存：批量导入是显式动作，主线程收到后去重、落 user_items.json。
pub fn spawn_batch_import(tx: mpsc::Sender<Vec<LaunchItem>>) {    thread::spawn(move || {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let mut files = Vec::new();
        for root in default_roots() {
            walk(&root, 0, &mut files);
        }
        files.sort();
        files.dedup();
        let total = files.len();
        let mut apps = Vec::new();
        let exe_key = self_exe_key();
        let mut skipped_self = 0usize;
        for file in &files {
            if let Some(app) = unsafe { resolve_lnk(file) } {
                // 自排除：target 规范化后就是本 exe（含我们自己写的启动夹快捷方式）。
                if exe_key.as_deref().is_some_and(|key| is_self_target(&app.path, key)) {
                    skipped_self += 1;
                    eprintln!("source: 批量导入自排除（target 即本 exe）：{}", file.display());
                    continue;
                }
                apps.push(app);
            } // 解析失败 / 空 target：跳过不中断（约束 4）
        }
        eprintln!(
            "source: 批量扫描 {} 个 .lnk，解析出 {} 条，自排除 {} 条（线程 {:?}，CoInit={hr:?}）",
            total,
            apps.len(),
            skipped_self,
            thread::current().id()
        );
        unsafe { CoUninitialize() };
        let _ = tx.send(apps);
    });
}

/// 默认扫描根目录（对应 AHK AppSettings：A_Programs / A_ProgramsCommon / A_Desktop / A_DesktopCommon）。
/// 将来做成配置项（用户可选扫描时机与目录），此处先内置。
fn default_roots() -> Vec<PathBuf> {
    let join = |var: &str, rel: &str| -> Option<PathBuf> {
        std::env::var_os(var).map(|p| Path::new(&p).join(rel))
    };
    [
        join("AppData", r"Microsoft\Windows\Start Menu\Programs"),
        join("ProgramData", r"Microsoft\Windows\Start Menu\Programs"),
        join("USERPROFILE", "Desktop"),
        join("PUBLIC", r"Public\Desktop"),
    ]
    .into_iter()
    .flatten()
    .filter(|p| p.is_dir())
    .collect()
}

/// 扫描深度（AHK Depth=3）与接受的扩展名（此步只做 `.lnk`，其余 provider 后续再并入）。
const MAX_DEPTH: u32 = 3;

/// 递归收集 `.lnk` 文件；先按扩展名过滤，再决定是否深入子目录（约束 4）。
fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, depth + 1, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("lnk") {
            out.push(path);
        }
    }
}

/// 批量导入的自排除判据：`.lnk` 的 target 规范化后就是本 exe ⇒ 不收进清单。
/// 起因是开机自启写的 `Programs\Startup\egui-launcher.lnk` 正落在默认扫描根（`Programs`，深度 2）
/// 里，"程序把自己当一条启动项导入"是必然发生而非偶然。
///
/// 判据既不写死文件名、也不用字符串相等：exe 改名或搬家后 `current_exe()` 自己会跟着变，而快捷方式
/// 里存的大小写、短名（`PROGRA~1`）、分隔符都可能与它不一致 ⇒ 两侧都取 `canonicalize` 后的长路径，
/// 再做大小写不敏感比较。任一侧规范化失败一律**不跳过**：把别的程序误判成"自己"而沉默丢一条，
/// 代价比多收一条大得多。
fn is_self_target(target: &str, exe_key: &str) -> bool {
    match Path::new(target).canonicalize() {
        Ok(canon) => canon.to_string_lossy().eq_ignore_ascii_case(exe_key),
        Err(_) => false,
    }
}

/// 本 exe 的规范化字符串键；取不到就视为没有自排除项（同上一条"宁多勿漏"的取向）。
fn self_exe_key() -> Option<String> {
    Some(std::env::current_exe().ok()?.canonicalize().ok()?.to_string_lossy().into_owned())
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 展开 `%VAR%`；失败或为空则原样返回（约束 1）。
pub fn expand_vars(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let src = wide_null(s);
    let needed = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), None) };
    if needed == 0 {
        return s.to_string();
    }
    let mut buf = vec![0u16; needed as usize];
    let written = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut buf)) };
    if written == 0 || written > needed {
        return s.to_string();
    }
    from_wide(&buf)
}

/// 用 IShellLinkW 解析单个 .lnk；target 为空或解析失败返回 None（丢弃，约束 3）。
/// pub 后调用方有二：批量扫描（本文件）与 SendTo 转发的 .lnk 预填解析（main.rs，FR-SRC-20①）。
pub unsafe fn resolve_lnk(path: &Path) -> Option<LaunchItem> {
    unsafe {
        let link: IShellLinkW =
            CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist_file: IPersistFile = link.cast().ok()?;
        let wide = wide_null(&path.to_string_lossy());
        persist_file.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;

        let mut buf = [0u16; 260];
        let target = link
            .GetPath(&mut buf, core::ptr::null_mut::<WIN32_FIND_DATAW>(), SLGP_UNCPRIORITY.0 as u32)
            .map(|_| from_wide(&buf))
            .unwrap_or_default();
        if target.trim().is_empty() {
            return None; // Store/MSIX 或损坏快捷方式：丢弃（约束 3）
        }
        let arguments = link.GetArguments(&mut buf).map(|_| from_wide(&buf)).unwrap_or_default();
        let working_dir_raw =
            link.GetWorkingDirectory(&mut buf).map(|_| from_wide(&buf)).unwrap_or_default();

        let name =
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string();
        let source_path = path.to_string_lossy().into_owned();
        let expanded_target = expand_vars(&target);
        let keyword = default_keyword(&name);

        Some(LaunchItem {
            name,
            path: expanded_target.clone(),
            arguments,
            working_dir: {
                let wd = expand_vars(&working_dir_raw);
                if wd.trim().is_empty() {
                    None
                } else {
                    Some(wd)
                }
            },
            // .lnk 自身作图标源（shell 解析快捷方式自定义图标），启动也走 .lnk 更保真。
            source_path: source_path.clone(),
            kind: ItemKind::App,
            keyword,
            ..Default::default()
        })
    }
}

// ---- FR-SRC-20 ① SendTo 入口安装/卸载（设置页开关的底层动作）----

const SENDTO_LINK_NAME: &str = "egui-launcher.lnk";

fn sendto_link_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(Path::new(&appdata).join(r"Microsoft\Windows\SendTo").join(SENDTO_LINK_NAME))
}

/// 设置页开关的当前态：文件存在且归属校验通过才算「已安装」。
/// 同名文件若是别人建的（target 不是本 exe）→ 报未安装，且后续删除会拒绝碰它。
/// 旧版本程序写的 Arguments=%1 形态也算自己的（历史版本产物），判据 target==本 exe 且 args ∈ {"", "%1"}。
pub fn sendto_installed() -> bool {
    let Some(link) = sendto_link_path() else { return false };
    if !link.exists() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = exe.to_string_lossy().into_owned();
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let owned = unsafe { read_link_target(&link) }
        .map(|(t, a)| t == exe && (a.trim().is_empty() || a.trim() == "%1"))
        .unwrap_or(false);
    if hr.is_ok() {
        unsafe { CoUninitialize() };
    }
    owned
}

/// 关闭开关=删除 SendTo 入口。**只删自己创建的**（需求书安全规则）：
/// 解析失败或 target 不是本 exe 一律拒删并说明；不存在则视为已关闭。
pub fn uninstall_sendto() -> Result<String, String> {
    let link = sendto_link_path().ok_or("缺 APPDATA 环境变量")?;
    if !link.exists() {
        return Ok("SendTo 入口本就不存在，无需删除".to_string());
    }
    let exe = std::env::current_exe()
        .map_err(|e| format!("current_exe 失败: {e}"))?
        .to_string_lossy()
        .into_owned();
    let dir = link.parent().ok_or("SendTo 路径异常")?.to_path_buf();
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let r = (|| -> Result<String, String> {
        let (target, args) =
            unsafe { read_link_target(&link) }.ok_or("解析既有 .lnk 失败，无法确认归属，未删除")?;
        if !(target == exe && (args.trim().is_empty() || args.trim() == "%1")) {
            return Err(format!(
                "「{}」不是本程序创建的，按安全规则不删除",
                link.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
            ));
        }
        std::fs::remove_file(&link).map_err(|e| format!("删除失败: {e}"))?;
        let witem = wide_null(&link.to_string_lossy());
        let wdir = wide_null(&dir.to_string_lossy());
        unsafe {
            SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_PATHW, Some(witem.as_ptr().cast()), None);
            SHChangeNotify(SHCNE_UPDATEDIR, SHCNF_PATHW, Some(wdir.as_ptr().cast()), None);
        }
        Ok("SendTo 入口已删除（归属已验证）".to_string())
    })();
    if hr.is_ok() {
        unsafe { CoUninitialize() };
    }
    r
}

/// 设置页开关「开」：后台线程写 `%APPDATA%\Microsoft\Windows\SendTo\egui-launcher.lnk`
/// （target=本 exe、参数留空由 Explorer 追加被发送路径，免管理员），结果经 `tx` 回报主线程做 Toast。
/// 「内容未变不重写」按需求书安全规则实现。
pub fn spawn_sendto_install(tx: mpsc::Sender<Result<String, String>>) {
    thread::spawn(move || {
        let result = install_sendto_link();
        let _ = tx.send(result);
    });
}

fn install_sendto_link() -> Result<String, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("current_exe 失败: {e}"))?
        .to_string_lossy()
        .into_owned();
    let appdata = std::env::var_os("APPDATA").ok_or("缺 APPDATA 环境变量")?;
    let dir = Path::new(&appdata).join(r"Microsoft\Windows\SendTo");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建 SendTo 目录失败: {e}"))?;
    // 菜单显示名=文件名去后缀，定名 egui-launcher（对齐 FR-SRC-20「发送到 egui-launcher」口径）。
    let link_path = dir.join(SENDTO_LINK_NAME);

    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let r = unsafe { write_sendto_link(&exe, &link_path) };
    if r.is_ok() {
        remove_legacy_sendto(&dir, &exe);
        // 外部进程写入的文件，SendTo 弹层不一定即时感知，主动通知 shell 刷新条目+目录。
        let witem = wide_null(&link_path.to_string_lossy());
        let wdir = wide_null(&dir.to_string_lossy());
        unsafe {
            SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_PATHW, Some(witem.as_ptr().cast()), None);
            SHChangeNotify(SHCNE_UPDATEDIR, SHCNF_PATHW, Some(wdir.as_ptr().cast()), None);
        }
    }
    if hr.is_ok() {
        unsafe { CoUninitialize() };
    }
    r
}

/// 清理上一版程序写的旧名入口「添加到启动器.lnk」——**只删自己创建的**（归属判据：
/// target=当前 exe 且参数=%1），不碰用户自建快捷方式（需求书安全规则）。
fn remove_legacy_sendto(dir: &Path, exe: &str) {
    let old = dir.join("添加到启动器.lnk");
    if !old.exists() {
        return;
    }
    if let Some((target, args)) = unsafe { read_link_target(&old) } {
        if target == exe && args == "%1" {
            match std::fs::remove_file(&old) {
                Ok(_) => eprintln!("sendto: 已清理旧名入口（本程序创建、归属已验证）: {old:?}"),
                Err(e) => eprintln!("sendto: 旧名入口清理失败（不影响新入口）: {e}"),
            }
        }
    }
}

unsafe fn write_sendto_link(exe: &str, link_path: &Path) -> Result<String, String> {
    unsafe {
        // 未变不重写：既有链接的 target 已匹配（且无参数）则跳过（启动路径上的任何 IO 都要有理由）。
        // 2026-10-17 实测更正：SendTo 链接「发送到」机制会把被选路径自动追加为尾参；
        // 显式写 %1 在 Win11 会被原样传入 + 再追加真实路径（弹两个窗），故参考 AHK/Delphi 做法留空参数。
        if link_path.exists() {
            if let Some((target, args)) = read_link_target(link_path) {
                if target == exe && args.trim().is_empty() {
                    return Ok(format!("SendTo 入口已存在且未变化，未重写: {}", link_path.display()));
                }
            }
        }
        let shell: IShellLinkW = CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("CoCreateInstance(ShellLink) 失败: {e}"))?;
        let wexe = wide_null(exe);
        shell.SetPath(PCWSTR(wexe.as_ptr())).map_err(|e| format!("SetPath: {e}"))?;
        shell.SetIconLocation(PCWSTR(wexe.as_ptr()), 0).map_err(|e| format!("SetIconLocation: {e}"))?;
        let wdesc = wide_null("添加到 egui-launcher 快捷项");
        shell.SetDescription(PCWSTR(wdesc.as_ptr())).map_err(|e| format!("SetDescription: {e}"))?;
        let pf: IPersistFile = shell.cast().map_err(|e| format!("cast IPersistFile: {e}"))?;
        let wlink = wide_null(&link_path.to_string_lossy());
        pf.Save(PCWSTR(wlink.as_ptr()), true).map_err(|e| format!("Save: {e}"))?;
        Ok(format!(
            "SendTo 入口已安装：右键文件 → 发送到 → 「egui-launcher」（{}）",
            link_path.display()
        ))
    }
}

/// 读既有 .lnk 的 target+arguments；解析失败返回 None（按「需重写」处理）。
unsafe fn read_link_target(path: &Path) -> Option<(String, String)> {
    unsafe {
        let link: IShellLinkW =
            CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER).ok()?;
        let pf: IPersistFile = link.cast().ok()?;
        let wide = wide_null(&path.to_string_lossy());
        pf.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        let mut buf = [0u16; 260];
        let target = link
            .GetPath(&mut buf, std::ptr::null_mut::<WIN32_FIND_DATAW>(), SLGP_UNCPRIORITY.0 as u32)
            .map(|_| from_wide(&buf))
            .ok()?;
        let args = link.GetArguments(&mut buf).map(|_| from_wide(&buf)).ok()?;
        Some((target, args))
    }
}

// ---- FR-WIN-11 ① 开机自启开关（启动夹 .lnk 带 -Startup）----

const STARTUP_LINK_NAME: &str = "egui-launcher.lnk";
/// FR-WIN-08 已规定的静默启动参数：登录后由启动夹拉起，但不弹窗口。
pub const STARTUP_ARGS: &str = "-Startup";

fn startup_link_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        Path::new(&appdata)
            .join(r"Microsoft\Windows\Start Menu\Programs\Startup")
            .join(STARTUP_LINK_NAME),
    )
}

/// 归属判据（纯函数，便于单测）：target 必须是本 exe，参数必须恰好是 `-Startup`。
/// 二者缺一律视为「别人建的文件」——既不认作已安装，也拒绝删除（需求书安全规则）。
fn is_our_startup_link(exe: &str, target: &str, args: &str) -> bool {
    target == exe && args.trim() == STARTUP_ARGS
}

pub fn startup_installed() -> bool {
    let Some(link) = startup_link_path() else { return false };
    if !link.exists() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = exe.to_string_lossy().into_owned();
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let owned = unsafe { read_link_target(&link) }
        .map(|(t, a)| is_our_startup_link(&exe, &t, &a))
        .unwrap_or(false);
    if hr.is_ok() {
        unsafe { CoUninitialize() };
    }
    owned
}

/// 关闭开关=删除启动夹入口。解析不出归属一律拒删。
pub fn uninstall_startup() -> Result<String, String> {
    let link = startup_link_path().ok_or("缺 APPDATA 环境变量")?;
    if !link.exists() {
        return Ok("开机自启入口本就不存在，无需删除".to_string());
    }
    let exe = std::env::current_exe()
        .map_err(|e| format!("current_exe 失败: {e}"))?
        .to_string_lossy()
        .into_owned();
    let dir = link.parent().ok_or("启动夹路径异常")?.to_path_buf();
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let r = (|| -> Result<String, String> {
        let (target, args) =
            unsafe { read_link_target(&link) }.ok_or("解析既有 .lnk 失败，无法确认归属，未删除")?;
        if !is_our_startup_link(&exe, &target, &args) {
            return Err(format!(
                "「{}」（target={target} 参数={args}）不是本程序创建的开机自启项，按安全规则不删除",
                link.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
            ));
        }
        std::fs::remove_file(&link).map_err(|e| format!("删除失败: {e}"))?;
        notify_shell(&link, &dir);
        Ok("开机自启项已删除（归属已验证）".to_string())
    })();
    if hr.is_ok() {
        unsafe { CoUninitialize() };
    }
    r
}

/// 设置页开关「开」：后台线程写启动夹 `.lnk`，结果经 `tx` 回报主线程做 Toast。
pub fn spawn_startup_install(tx: mpsc::Sender<Result<String, String>>) {
    thread::spawn(move || {
        let result = install_startup_link();
        let _ = tx.send(result);
    });
}

fn install_startup_link() -> Result<String, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("current_exe 失败: {e}"))?
        .to_string_lossy()
        .into_owned();
    let appdata = std::env::var_os("APPDATA").ok_or("缺 APPDATA 环境变量")?;
    let dir = Path::new(&appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建启动夹目录失败: {e}"))?;
    let link_path = dir.join(STARTUP_LINK_NAME);

    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let r = unsafe { write_startup_link(&exe, &link_path) };
    if r.is_ok() {
        notify_shell(&link_path, &dir);
    }
    if hr.is_ok() {
        unsafe { CoUninitialize() };
    }
    r
}

/// 外部进程往启动夹/SendTo 写的文件，shell 不一定即时感知，主动通知条目+目录刷新。
fn notify_shell(item: &Path, dir: &Path) {
    let witem = wide_null(&item.to_string_lossy());
    let wdir = wide_null(&dir.to_string_lossy());
    unsafe {
        SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_PATHW, Some(witem.as_ptr().cast()), None);
        SHChangeNotify(SHCNE_UPDATEDIR, SHCNF_PATHW, Some(wdir.as_ptr().cast()), None);
    }
}

unsafe fn write_startup_link(exe: &str, link_path: &Path) -> Result<String, String> {
    unsafe {
        // 未变不重写：同 SendTo 那套规则，启动路径上的任何 IO 都要有理由。
        if link_path.exists() {
            if let Some((target, args)) = read_link_target(link_path) {
                if is_our_startup_link(exe, &target, &args) {
                    return Ok(format!(
                        "开机自启项已存在且未变化，未重写: {}",
                        link_path.display()
                    ));
                }
            }
        }
        let shell: IShellLinkW = CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("CoCreateInstance(ShellLink) 失败: {e}"))?;
        let wexe = wide_null(exe);
        shell.SetPath(PCWSTR(wexe.as_ptr())).map_err(|e| format!("SetPath: {e}"))?;
        let wargs = wide_null(STARTUP_ARGS);
        shell.SetArguments(PCWSTR(wargs.as_ptr())).map_err(|e| format!("SetArguments: {e}"))?;
        shell.SetIconLocation(PCWSTR(wexe.as_ptr()), 0).map_err(|e| format!("SetIconLocation: {e}"))?;
        let wdesc = wide_null("登录时静默启动 egui-launcher（不弹窗口）");
        shell.SetDescription(PCWSTR(wdesc.as_ptr())).map_err(|e| format!("SetDescription: {e}"))?;
        let pf: IPersistFile = shell.cast().map_err(|e| format!("cast IPersistFile: {e}"))?;
        let wlink = wide_null(&link_path.to_string_lossy());
        pf.Save(PCWSTR(wlink.as_ptr()), true).map_err(|e| format!("Save: {e}"))?;
        Ok(format!("开机自启项已写入：{}（参数 {STARTUP_ARGS}，登录后托盘常驻、不弹窗口）", link_path.display()))
    }
}

#[cfg(test)]
mod dedup_tests {
    use super::*;

    fn with_args(path: &str, arguments: &str) -> LaunchItem {
        LaunchItem {
            name: path.to_string(),
            path: path.to_string(),
            arguments: arguments.to_string(),
            ..Default::default()
        }
    }

    /// 期望：同一 exe、不同参数的两条是不同条目，都该留下（uid 规则含 arguments）。
    #[test]
    fn same_path_different_arguments_both_survive() {
        let v = dedup_items(vec![with_args("cmd.exe", "/k ping"), with_args("cmd.exe", "/k ipconfig")]);
        assert_eq!(v.len(), 2, "实际只留下 {} 条", v.len());
    }

    /// 同一 path + 同一参数（含大小写差）仍是同一条，必须合并。
    #[test]
    fn same_path_same_arguments_dedups_case_insensitively() {
        let v = dedup_items(vec![with_args("CMD.exe", "/K PING"), with_args("cmd.exe", "/k ping ")]);
        assert_eq!(v.len(), 1, "实际留下 {} 条", v.len());
    }

    /// FR-SRC-19 新字段全带 `serde(default)`：加字段之前的旧清单必须原样可读。
    #[test]
    fn legacy_list_without_new_fields_parses() {
        let v: Vec<LaunchItem> = serde_json::from_str(
            r#"[{"name":"计算器","path":"calc.exe","arguments":"","working_dir":null,"source_path":"","kind":"command"}]"#,
        )
        .expect("旧清单应可解析");
        assert_eq!(v.len(), 1);
        assert!(!v[0].pinned);
        assert_eq!((v[0].use_count, v[0].last_used_ms), (0, 0));
    }

    /// 加 `tab_mode` 之前的旧 settings.json 必须照读，且回到默认口径（补全优先）。
    /// 批次 J 把 `space_to_run` 扩成四档后，"旧文件照读"这条更要紧：缺字段 ⇒ 新默认 `word`，
    /// 显式写了 `false`/`true` ⇒ 按老字面意思落进 `off`/`always`，绝不把用户当初选的改掉。
    #[test]
    fn legacy_settings_without_tab_mode_load() {
        let s: AppSettings =
            serde_json::from_str(r#"{"space_to_run":true}"#).expect("旧偏好应可解析");
        assert_eq!(s.space_to_run, SpaceRun::Always);
        assert_eq!(s.tab_mode, TabMode::Complete);
        assert_eq!(s.learn_rank, LearnRank::Count, "旧偏好没有 learn_rank ⇒ 默认甲");
        let empty: AppSettings = serde_json::from_str("{}").expect("空偏好应可解析");
        assert_eq!(
            (empty.space_to_run, empty.tab_mode),
            (SpaceRun::Word, TabMode::Complete)
        );
        let off: AppSettings =
            serde_json::from_str(r#"{"space_to_run":false}"#).expect("旧 false 应可解析");
        assert_eq!(off.space_to_run, SpaceRun::Off);
    }

    /// 新档位名照写照读；未知档位名要报错（坏配置由 `load_settings` 兜底成默认值，
    /// 但报错本身必须发生，否则"档位拼错了却静默当成默认"这种故障查不出来）。
    #[test]
    fn space_run_modes_round_trip_and_reject_unknown() {
        let s: AppSettings = serde_json::from_str(
            r#"{"space_to_run":"word","tab_mode":"navigate","learn_rank":"decay"}"#,
        )
        .unwrap();
        assert_eq!(
            (s.space_to_run, s.tab_mode, s.learn_rank),
            (SpaceRun::Word, TabMode::Navigate, LearnRank::Decay)
        );
        let json = serde_json::to_string(&AppSettings {
            space_to_run: SpaceRun::Unique,
            tab_mode: TabMode::Navigate,
            ..Default::default()
        })
        .expect("档位应能序列化");
        assert!(json.contains(r#""space_to_run":"unique""#), "实际写盘：{json}");
        assert!(json.contains(r#""learn_rank":"count""#), "实际写盘：{json}");
        assert!(serde_json::from_str::<AppSettings>(r#"{"space_to_run":"wod"}"#).is_err());
        assert!(serde_json::from_str::<AppSettings>(r#"{"learn_rank":"decayy"}"#).is_err());
    }

    #[test]
    fn learn_alias_gates_then_appends() {
        let mut it = LaunchItem {
            name: "计算器".to_string(),
            keyword: "jsq".to_string(),
            ..Default::default()
        };
        assert!(!learn_alias(&mut it, "jsq"), "已在别名里 ⇒ 不重复攒");
        assert!(!learn_alias(&mut it, "j"), "单字不攒：会给所有含 j 的查询喂 30 档噪声");
        assert!(!learn_alias(&mut it, "计算器"), "等于名称 ⇒ 没有可学的");
        assert!(!learn_alias(&mut it, "  "), "空白词不攒");
        assert!(learn_alias(&mut it, "CALC"), "新的两字符以上触发词应攒下");
        assert_eq!(it.keyword, "jsq calc", "空格追加，不动默认拼音首字母");
        assert!(!learn_alias(&mut it, "calc"), "大小写去重");

        let mut bare =
            LaunchItem { name: "Notepad".to_string(), keyword: String::new(), ..Default::default() };
        assert!(learn_alias(&mut bare, "np"));
        assert_eq!(bare.keyword, "np", "原本无别名时直接落第一个词");
    }

    /// FR-WIN-11 开机自启的归属判据（安全规则的可测部分）：target 与参数**两个都得对**，
    /// 否则一律视为别人建的文件——既不认作"已安装"，卸载时也会拒绝碰它。
    #[test]
    fn startup_ownership_needs_both_target_and_startup_arg() {
        assert!(is_our_startup_link("C:\\a\\egui-launcher.exe", "C:\\a\\egui-launcher.exe", "-Startup"));
        assert!(
            is_our_startup_link("C:\\a\\e.exe", "C:\\a\\e.exe", " -Startup "),
            "启动夹里的参数串可能带首尾空格，不参与判断"
        );
        assert!(!is_our_startup_link("C:\\a\\e.exe", "C:\\a\\e.exe", ""), "无参数＝不是本项");
        assert!(!is_our_startup_link("C:\\a\\e.exe", "C:\\a\\e.exe", "-PREFERENCES"));
        assert!(
            !is_our_startup_link("C:\\a\\e.exe", "C:\\b\\other.exe", "-Startup"),
            "target 不是本 exe ⇒ 别人的快捷方式"
        );
        assert!(
            !is_our_startup_link("C:\\a\\e.exe", "C:\\A\\E.EXE", "-Startup"),
            "路径按字面比较（同 SendTo 那套口径），大小写不同即不认领"
        );
    }

    /// 自排除判据必须跟着**正在运行的这个 exe** 走，不能写死文件名：改名或搬家后 `current_exe()`
    /// 自然变，判据也就跟着变。这里直接拿本测试自己的可执行文件当 target，顺带验大小写差异
    /// 与"规范化失败就不跳过"。
    #[test]
    fn self_exclusion_follows_the_running_exe_not_a_hardcoded_name() {
        let exe = std::env::current_exe().unwrap();
        let key = exe.canonicalize().unwrap().to_string_lossy().into_owned();
        let plain = exe.to_string_lossy().into_owned();
        assert!(is_self_target(&plain, &key), "本 exe 必须被判成自己");
        assert!(is_self_target(&plain.to_uppercase(), &key), "快捷方式里大小写不同也要认出来");
        assert!(
            !is_self_target(r"C:\no-such-dir-here\zzz.exe", &key),
            "规范化失败 ⇒ 不跳过（宁可多收一条，不误判成自己）"
        );
        assert!(
            !is_self_target(r"C:\Windows\System32\notepad.exe", &key),
            "别的程序不能被判成自己"
        );
    }

    /// 开机自启的「写入 → 归属 → 未变不重写 → 删除 → 拒删别人的」整条往返。
    /// 走的是真实 COM + `IShellLinkW`，但把 `APPDATA` 指到临时目录 ⇒ 替用户挡掉
    /// "为了验证就往真启动夹写文件、还要注销登录一次"的成本，同时真验了落盘与解析。
    #[test]
    fn startup_link_round_trip_only_touches_temp_appdata() {
        let base = std::env::temp_dir().join(format!("egui-launcher-startup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = std::env::var_os("APPDATA");
        // SAFETY: 本进程里只有 sendto/startup 两个前缀读 `APPDATA`，都不在这条测试之外被调用；
        // 结束即还原（含失败路径），不污染同进程其它测试线程。
        unsafe { std::env::set_var("APPDATA", &base) };
        let r = (|| -> Result<(), String> {
            let startup_dir =
                base.join(r"Microsoft\Windows\Start Menu\Programs\Startup");
            let link = startup_dir.join(STARTUP_LINK_NAME);

            let first = install_startup_link()?;
            if !link.exists() {
                return Err(format!("写完了但启动夹里没有 .lnk: {first}"));
            }
            if !startup_installed() {
                return Err("刚写完就判未安装 ⇒ 归属判据与实际写入不一致".to_string());
            }
            let again = install_startup_link()?;
            if !again.contains("未重写") {
                return Err(format!("第二次应命中「未变不重写」，实得：{again}"));
            }

            uninstall_startup()?;
            if link.exists() {
                return Err("删除后文件还在".to_string());
            }
            if startup_installed() {
                return Err("删除后仍判已安装".to_string());
            }

            // 别人的快捷方式（target 不是本 exe）：既不认作已安装，也拒绝删。
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            let foreign = "C:\\Windows\\System32\\notepad.exe";
            unsafe { write_startup_link(foreign, &link) }?;
            if startup_installed() {
                return Err("target 不是本 exe 却判已安装".to_string());
            }
            let msg = uninstall_startup().err().ok_or("别人的快捷方式居然被删了")?;
            if !msg.contains("不是本程序创建") {
                return Err(format!("拒删理由不对：{msg}"));
            }
            if !link.exists() {
                return Err("口头上拒删、实际把文件删了".to_string());
            }
            if hr.is_ok() {
                unsafe { CoUninitialize() };
            }
            Ok(())
        })();
        match old {
            Some(v) => unsafe { std::env::set_var("APPDATA", v) },
            None => unsafe { std::env::remove_var("APPDATA") },
        }
        let _ = std::fs::remove_dir_all(&base);
        r.unwrap();
    }
}

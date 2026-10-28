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
//! 3. Store/MSIX target 为空串：`resolve_lnk` 直接丢弃；2026-10-22 批次 A9 起改由
//!    `scan_store_apps` 从 `shell:AppsFolder` 命名空间补齐（这些应用在文件系统里没有 `.lnk`）；
//! 4. 扫描先按扩展名过滤，单文件解析失败只跳过、不中断整体（NFR-04）。

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use super::websearch;
use crate::source::{DataSource, SearchCtx, SourceId};
use pinyin::ToPinyin;
use serde::{Deserialize, Serialize};
use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IBindCtx, IPersistFile, STGM_READ,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::UI::Shell::{
    AO_NONE, ApplicationActivationManager, BHID_EnumItems, IApplicationActivationManager,
    IEnumShellItems, IShellItem, IShellLinkW, SHCNE_UPDATEDIR, SHCNE_UPDATEITEM, SHCNF_PATHW,
    SHChangeNotify, SHCreateItemFromParsingName, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
    SLGP_UNCPRIORITY,
};
use windows::core::Interface;
use windows::core::{PCWSTR, PWSTR};

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
    /// 批次 A2（FR-SRC-17）：启动器自己的动作，`path` 形如 `internal:<id>`。
    /// `ShellExecuteExW` 起不了它 ⇒ `main.rs` 在启动前分派到内置实现。
    /// **不进 [`ItemKind::ALL`]**：添加/编辑对话框不给用户手填这种类型。
    Internal,
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
            ItemKind::Internal => "系统命令",
        }
    }
}

/// 一条可启动项。字段全部是纯数据，供主线程转成界面条目 / 拼启动命令。
/// 批次 U 起补 `PartialEq`：`HeadRow` 要能比较（里面装着已填好 `{query}` 的条目副本）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// 批次 G1 分类标签：`@分类` 前缀筛选与「缺省分类范围」共用；空＝恒显。
    /// `#[serde(default)]` 让旧清单缺字段仍可解析（不写就是空标签，任何档都显示）。
    #[serde(default)]
    pub tags: Vec<String>,
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

/// 便携数据目录（清单 / 设置 / 日志 / 主题同处，FR-CFG-03）。
/// 主题目录（`Themes\`，FR-UIX-01）也挂在这里——用户把程序拷到 U 盘，皮肤跟着走。
pub fn data_dir() -> PathBuf {
    user_list_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(appdata_dir)
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
        exe.parent()
            .map(|dir| (dir.to_path_buf(), dir.join("user_items.json")))
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

/// 读取用户清单（E1，批次 X）。返回值的第二格只在配置真的坏过时有值，且要分两种下场看
/// （`Corrupted`）：`Kept` 给调用方一句话提示，`Lost` 还要求调用方**这一轮别写盘**。
/// 文件本来就在 ⇒ 两格都是干净的默认值。
pub fn load_user_items() -> (Vec<LaunchItem>, Option<Corrupted>) {
    load_user_items_at(&user_list_path())
}

/// 覆盖写用户清单（调用方负责先剔除内置种子、只持久化用户新增项）。
pub fn save_user_items(items: &[LaunchItem]) {
    let path = user_list_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_vec_pretty(items) {
        Ok(json) => write_atomic(&path, &json, "user_items.json"),
        Err(e) => eprintln!("source: 序列化用户清单失败: {e}"),
    }
}

/// 一份 JSON 读盘的结果：`Ok(内容)`，或 `Err(坏文件的下场)`。
type Loaded<T> = Result<T, Corrupted>;

/// 坏文件的两种下场。**不能折成一格**：折成 `Option<PathBuf>` 之后 `None` 同时表示"没坏"和
/// "坏了但没留住"，调用方就无法知道该不该停手——2026-10-22 真机⑫就是这么丢的现场（留证返回
/// `None` 后照常播种，把唯一那份坏文件覆盖掉了）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Corrupted {
    /// 证据保住了：改名成功（原位置已空）或另存副本成功。里面是留证落点。
    Kept(PathBuf),
    /// 改名与另存都不成，坏文件还挡在原位 ⇒ **任何覆盖写都会销毁唯一证据**，调用方要停手。
    Lost,
}

/// 把 serde 错误原文里**双引号包着的那段**抹掉，只留形态。
///
/// 为什么要抹：`invalid type: string "…"` 那一段就是出问题的那格**内容**，而这三份文件
/// （`settings.json` / `user_items.json` / `search_history.json`）存的就是用户自己写的东西
/// ——FR-SYS-03 定的口径是"日志绝不记用户原文"，批次 E2 已经为逐格容错那条路做过同一件事
/// （`json_kind` 只记形态）。2026-10-27 真机探针：把 `search_history.json` 写成
/// `"C3隐私探针ABC"`，日志里原样出现了那串 ⇒ 整份解析失败这条路的报文是漏的那一格。
/// 反斜杠转义要跟着走，否则串里的 `\"` 会提前把引号区关掉、把后半截留在明面上。
fn serde_shape(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut in_str = false;
    let mut escaped = false;
    for ch in msg.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            if !in_str {
                out.push(ch);
            }
            continue;
        }
        if ch == '"' {
            in_str = !in_str;
            out.push('"');
            continue;
        }
        if !in_str {
            out.push(ch);
        }
    }
    out
}

/// 读 + 解析，失败即留证。清单与偏好共用这一条判据，两条路不能各写一份。
fn load_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Loaded<T> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return Ok(T::default()), // 文件不在（首次运行）：正常，不算错误
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(v) => Ok(v),
        Err(e) => {
            // 报文只留形态：serde 原文会把出问题那格的内容一起打出来（见 [`serde_shape`]）。
            eprintln!("source: {path:?} 解析失败: {}", serde_shape(&e.to_string()));
            Err(quarantine(path, &bytes))
        }
    }
}

/// `.bad` 那一族的名额。占满后还有毫秒时间戳兜底，所以这个数字不再是"同一文件能坏几次"的上限。
const BAD_NAME_SLOTS: usize = 10;

/// 读一份 JSON，把「坏文件的下场」折成调用方好用的两格。**这一层必须逐格构造元组**：
/// 写成 `load_json(&p).unwrap_or_else(|ev| (T::default(), Some(ev)))` 会让类型推断把
/// `unwrap_or_else` 的**返回格**当成 `load_json` 的 `T`，于是 `T` 变成「(内容, 下场)」整个元组
/// ⇒ 解析时按"两元素 JSON 数组"去要求那份清单/偏好 ⇒ **任何正常文件都必然解析失败**。
/// 批次 X 真机⑫看到的「配置文件坏了，已改名留底：user_items.json.bad2」就是这么来的：
/// 每一次启动都把好好的清单改名一份、再从空清单起，`.bad` 一路排到 `.bad2`；
/// 同一条坑的另一半证据是当时 settings 那条报文写着 `expected a tuple of size 2`。
/// 换成 `Corrupted` 之后这条推断不再统一、编译期就报出来了——旧形状能过编译是运气，不是对。
fn load_or_quarantine<T: serde::de::DeserializeOwned + Default>(
    path: &Path,
) -> (T, Option<Corrupted>) {
    match load_json(path) {
        Ok(v) => (v, None),
        Err(ev) => (T::default(), Some(ev)),
    }
}

/// 把坏文件改名为 `xxx.bad`（占用则 `.bad1`…，10 格占满再用 `.bad.<毫秒>`）留证。
/// 改名不动内容 ⇒ 证据保住；改不动（文件被别的进程占着）就退回用内存里的字节另存一份副本，
/// 两条都不成才报 `Lost`。
///
/// 为什么要有时间戳那格：2026-10-22 真机⑫的现场是同一条 `user_items.json` 被反复弄坏，
/// `.bad`…`.bad9` 全被占 ⇒ 老代码直接返回"没留住"，而调用方把"没留住"读成"目录写不进去，
/// 播种也不会覆盖证据"（`Loaded` 旧注释里的原话）——**那个前提是错的**，占满名额跟目录只读
/// 是两回事，于是覆盖写照样发生。名额这一层修不掉判断错误，所以两处一起改。
fn quarantine(path: &Path, bytes: &[u8]) -> Corrupted {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for n in 0..BAD_NAME_SLOTS {
        let suffix = if n == 0 {
            "bad".to_string()
        } else {
            format!("bad{n}")
        };
        if let Some(p) = keep_at(path, bytes, &format!("{name}.{suffix}")) {
            return Corrupted::Kept(p);
        }
    }
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Some(p) = keep_at(path, bytes, &format!("{name}.bad.{ms}")) {
        return Corrupted::Kept(p);
    }
    eprintln!("source: {path:?} 解析失败且留证没成功 ⇒ 覆盖写会丢掉唯一证据，请人工处理");
    Corrupted::Lost
}

/// 一个候选留证名：已存在就跳过（名额已被占）；先试改名（不动内容），改不动再另存副本。
fn keep_at(path: &Path, bytes: &[u8], candidate: &str) -> Option<PathBuf> {
    let bad = path.with_file_name(candidate);
    if bad.exists() {
        return None;
    }
    if std::fs::rename(path, &bad).is_ok() {
        eprintln!("source: 坏文件已改名留证 {bad:?}（默认值启动，不覆盖原内容）");
        return Some(bad);
    }
    if std::fs::write(&bad, bytes).is_ok() {
        eprintln!("source: 坏文件改名失败，已另存副本留证 {bad:?}");
        return Some(bad);
    }
    None
}

/// 原子替换：先写同目录临时文件，成功后再改名盖过去。
///
/// `std::fs::write` 是"把目标截断再写"——中途断电、进程被杀、磁盘满就留下半份 JSON，
/// 下次启动读它必然解析失败，而旧内容已经被截掉了。改名是同卷内的原子操作：
/// 目标要么保持原样，要么完整是新内容，不存在中间态。
/// 临时名带 pid：单实例（FR-SRC-21）之外还有人工开第二份的场景，两个写者不能抢同一个临时名。
fn write_atomic(path: &Path, bytes: &[u8], label: &str) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.{}.tmp", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, bytes) {
        eprintln!("source: 写 {label} 临时文件失败 {path:?}: {e}（原文件未改动）");
        return;
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => {}
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            eprintln!("source: {label} 替换失败 {path:?}: {e}（原文件未改动）");
        }
    }
}

// ---- FR-CFG-01 批次 E2：配置版本、逐版迁移、字段级容错 ----

/// `settings.json` 的字段形态版本。**只有"已有字段的类型或含义变了"才 +1**；
/// 纯新增字段不加号——`#[serde(default)]` 本来就读得动老文件，加号反而让每次改动都得写一条
/// 空迁移。加了号就必须同时在 [`migrate_settings`] 里补一条臂，否则那个版本的文件会被停在
/// "不认识的版本差"那一步（只按已知字段读，不猜）。
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

/// 迁移前备份的名额。写不进备份就不迁移，所以这个数字的实际含义是
/// "同一个版本最多留下几份现场"；5 份用满还在原地打转，说明这份目录本身就写不进去。
const MIGRATE_BACKUP_SLOTS: usize = 5;

/// JSON 值的形态名，只用来写日志。**只取形态、不取值**：`theme`、`terminal_shell`、条目的
/// `name` 这些格子里装的都是用户写的内容，而 serde 的错误原文会把值一起打出来
/// （`invalid type: string "…"`）⇒ 直接打印 `e` 等于把用户数据写进日志，FR-SYS-03 那条线不能破。
fn json_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "空值",
        serde_json::Value::Bool(_) => "布尔",
        serde_json::Value::Number(_) => "数字",
        serde_json::Value::String(_) => "字符串",
        serde_json::Value::Array(_) => "数组",
        serde_json::Value::Object(_) => "对象",
    }
}

/// 逐字段取一格：缺键 ⇒ 保持 `Default` 里那个值（＝需求书「缺失项补默认」）；
/// 类型不符 ⇒ **只兜这一格**并记一条脱敏日志（＝「类型不符取默认并记日志」）。
///
/// 为什么是宏而不是函数：这 18 格类型各不相同，`from_value::<T>` 的 `T` 没法在运行时给。
/// 展开出来的表就是「字段名 → 类型」的清单，而 **字段名即 JSON 键名**（`AppSettings` 与
/// `LaunchItem` 都没有 `rename_all`；容器级 `rename_all` 一旦加上去，这张表会整体读不进来，
/// 症状是"配置全部回默认"——加字段忘写一行也是同一个症状，由
/// `every_settings_field_is_read_from_a_full_file` 那条单测兜住）。
/// `$tag` 是给清单用的前缀（「清单第 3 条：」），偏好那边传空串。
macro_rules! take_field {
    ($map:expr, $out:expr, $tag:expr, $field:ident, $ty:ty) => {
        if let Some(raw) = $map.get(stringify!($field)) {
            match serde_json::from_value::<$ty>(raw.clone()) {
                Ok(x) => $out.$field = x,
                Err(_) => eprintln!(
                    "config: {}字段 `{}` 类型不符（实为{}）⇒ 取默认值",
                    $tag,
                    stringify!($field),
                    json_kind(raw)
                ),
            }
        }
    };
}

/// 版本戳单独判：缺键＝v0（这一格批次 E2 才有），类型不对＝按 v0 处理并记一条。
fn read_version(map: &serde_json::Map<String, serde_json::Value>) -> u32 {
    match map.get("schema_version") {
        None => 0,
        Some(raw) => match serde_json::from_value::<u32>(raw.clone()) {
            Ok(n) => n,
            Err(_) => {
                eprintln!(
                    "config: 字段 `schema_version` 类型不符（实为{}）⇒ 按 v0 处理",
                    json_kind(raw)
                );
                0
            }
        },
    }
}

/// 对象树 → `AppSettings`。`version` 由调用方给（已经判完、可能已经迁过），
/// 不再从树里取第二次。
fn settings_from_map(
    map: &serde_json::Map<String, serde_json::Value>,
    version: u32,
) -> AppSettings {
    let mut s = AppSettings::default();
    s.schema_version = version;
    take_field!(map, s, "", space_to_run, SpaceRun);
    take_field!(map, s, "", tab_mode, TabMode);
    take_field!(map, s, "", learn_rank, LearnRank);
    take_field!(map, s, "", toggle_hotkey, HotkeySpec);
    take_field!(map, s, "", typeback_hotkey, Option<HotkeySpec>);
    take_field!(map, s, "", secondary_action, SecondaryAction);
    take_field!(map, s, "", secondary_hotkey, HotkeySpec);
    take_field!(
        map,
        s,
        "",
        web_engines,
        Vec<crate::source::websearch::WebEngine>
    );
    take_field!(map, s, "", seed_round, u32);
    take_field!(map, s, "", sources_off, Vec<String>);
    take_field!(map, s, "", hidden, Vec<String>);
    take_field!(map, s, "", hidden_stash, Vec<LaunchItem>);
    take_field!(map, s, "", confirm_actions, bool);
    take_field!(map, s, "", terminal_shell, String);
    take_field!(map, s, "", theme, String);
    take_field!(map, s, "", list_rows, ListRows);
    take_field!(map, s, "", row_density, RowDensity);
    take_field!(map, s, "", default_scope, Scope);
    take_field!(map, s, "", appearance, Appearance);
    take_field!(map, s, "", history_size, usize);
    take_field!(map, s, "", file_folders, Vec<String>);
    take_field!(map, s, "", file_depth, usize);
    take_field!(map, s, "", file_exclude, Vec<String>);
    take_field!(map, s, "", file_max_entries, usize);
    take_field!(map, s, "", file_refresh_minutes, u64);
    take_field!(map, s, "", file_min_query_len, usize);
    s
}

/// 一份清单元素 → `LaunchItem`。**不是对象就不认它是条目**（调用方丢它并记下标）：
/// 硬造一条 `name`/`path` 全空的条目会带着空 uid 进清单、和 `dedup_items` 与 `uid()` 的规则打架，
/// 后果比"少一条"更难查。
fn item_from_value(map: &serde_json::Map<String, serde_json::Value>, tag: &str) -> LaunchItem {
    let mut it = LaunchItem::default();
    take_field!(map, it, tag, name, String);
    take_field!(map, it, tag, path, String);
    take_field!(map, it, tag, arguments, String);
    take_field!(map, it, tag, working_dir, Option<String>);
    take_field!(map, it, tag, source_path, String);
    take_field!(map, it, tag, kind, ItemKind);
    take_field!(map, it, tag, keyword, String);
    take_field!(map, it, tag, pinned, bool);
    take_field!(map, it, tag, use_count, u32);
    take_field!(map, it, tag, last_used_ms, i64);
    take_field!(map, it, tag, tags, Vec<String>);
    it
}

/// 数组 → 清单。逐条容错：某一条的某一格类型不对，只兜那一格，**其余 292 条一字不动**。
/// 这是本批对用户数据最实在的一条改动：以前同一种错会把整份清单送进 `.bad`。
///
/// 「成不构成一条」的门槛沿用 E2 之前 serde 的那道：`name`/`path` 在结构体上**没有**
/// `#[serde(default)]`（上面其余格子都有），旧写法少任一键就整份解析失败。逐字段容错把
/// 那个失败点拆掉之后，必须在这一层把门槛补回来，否则 `{"path":"C:\\x.exe"}`（没有名字）
/// 会带着空 `name` 进清单——行上什么都没有、`uid()` 派生成 `custom:||`，与去重规则打架，
/// 后果比少一条更难查（[`item_from_value`] 同一条理由）。出厂种子的构造器
/// （[`command_seed`] 等）每条两个字段都非空 ⇒ 这道门槛不会误剔除正常条目。
fn items_from_tree(list: &[serde_json::Value]) -> Vec<LaunchItem> {
    let mut out = Vec::with_capacity(list.len());
    for (i, raw) in list.iter().enumerate() {
        let tag = format!("清单第 {} 条：", i + 1);
        match raw.as_object() {
            Some(map) => {
                let it = item_from_value(map, &tag);
                if it.name.is_empty() || it.path.is_empty() {
                    eprintln!("config: {tag}缺 `name` 或 `path` ⇒ 这一条不认作条目，其余照常");
                } else {
                    out.push(it);
                }
            }
            None => eprintln!(
                "config: {}不是对象（实为{}）⇒ 这一条不认作条目，其余照常",
                tag,
                json_kind(raw)
            ),
        }
    }
    out
}

/// **一次升一版、不许跳版**：循环里每一步只认识 `v → v+1`，碰到不认识的版本差就停手，
/// 返回实际到达的版本号。停在半路不是错误——那份配置照样按已知字段读得出来（NFR-04）。
///
/// v0 → v1 看着是空步子（只打版本戳、数据一字不动），但它是让后面每一步都有号可对的前置：
/// 批次 J 那次 `space_to_run` 由布尔扩成四档是在自定义 `Deserialize` 里兜的（[`SpaceRun`]），
/// 那种写法只能处理"一种形态改成另一种"，处理不了"改了语义、要换算"，所以从此改走版本号。
fn migrate_settings(
    map: &mut serde_json::Map<String, serde_json::Value>,
    from: u32,
) -> u32 {
    let mut v = from;
    while v < SETTINGS_SCHEMA_VERSION {
        match v {
            0 => {}
            other => {
                eprintln!("config: 没有从 v{other} 往上一版的迁移步子 ⇒ 停在这里，不猜");
                return other;
            }
        }
        v += 1;
        map.insert("schema_version".to_string(), serde_json::Value::from(v as u64));
    }
    v
}

/// 迁移前的**原始字节**备份：`settings.v0.backup.json`，名字被占就 `-1`、`-2`……最多
/// [`MIGRATE_BACKUP_SLOTS`] 份，**绝不覆盖已有同名文件**（那份可能是更早一次的现场）。
/// 落不成就返回 `None` ⇒ 调用方不迁移：宁可不迁移，不能无证据迁移。
fn backup_before_migrate(path: &Path, version: u32) -> Option<PathBuf> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.clone(), String::new()),
    };
    for n in 0..MIGRATE_BACKUP_SLOTS {
        let candidate = if n == 0 {
            format!("{stem}.v{version}.backup{ext}")
        } else {
            format!("{stem}.v{version}.backup-{n}{ext}")
        };
        let target = path.with_file_name(candidate);
        if target.exists() {
            continue;
        }
        write_atomic(&target, &bytes, "迁移前备份");
        if target.exists() {
            return Some(target);
        }
    }
    None
}

/// 合法 JSON、但顶层形状不是这个消费者要的形态（偏好是数组、清单是对象之类）。
/// 按 FR-CFG-02 走与"语法坏"完全同一条留证路径，不新造第二种下场。
/// 这里要再读一次文件只为拿字节：`load_json` 交回来的是解析后的树，而改名留证要的是原字节。
fn shape_fault(path: &Path, got: &serde_json::Value) -> Option<Corrupted> {
    eprintln!(
        "config: {path:?} 顶层形状不对（实为{}）⇒ 与语法坏同路，先留证再按默认值启动",
        json_kind(got)
    );
    std::fs::read(path)
        .ok()
        .map(|bytes| quarantine(path, &bytes))
}

/// 读一份偏好文件（路径参数化，单测才打得到临时目录而不碰用户数据）。
///
/// 三条下场分开走，一条都不许合并：
/// - 文件不在 ⇒ `AppSettings::default()`（v1），不写盘；
/// - 语法坏 / 顶层不是对象 ⇒ 留证（`Corrupted`）+ 默认值，**不写盘**（写盘会覆盖唯一证据）；
/// - 读得动 ⇒ 需要迁移就先备份再逐步迁，**只有真的动了这份树才回写一次**；
///   纯读永远不改盘（保住"未知键"与更高版本戳，也避免每次启动都产生一次写）。
fn load_settings_at(path: &Path) -> (AppSettings, Option<Corrupted>) {
    let (tree, fault) = load_or_quarantine::<serde_json::Value>(path);
    if let Some(e) = fault {
        return (AppSettings::default(), Some(e));
    }
    match tree {
        serde_json::Value::Null => (AppSettings::default(), None),
        serde_json::Value::Object(mut map) => {
            let claimed = read_version(&map);
            if claimed > SETTINGS_SCHEMA_VERSION {
                eprintln!(
                    "config: {path:?} 标的是 v{claimed}，本程序只认到 v{SETTINGS_SCHEMA_VERSION} ⇒ 按认识的字段读，不改它的版本戳、不回写"
                );
                return (settings_from_map(&map, claimed), None);
            }
            if claimed < SETTINGS_SCHEMA_VERSION {
                let Some(kept) = backup_before_migrate(path, claimed) else {
                    eprintln!(
                        "config: 迁移前留证写不进去 ⇒ 本次不迁移（宁可不迁移，不能无证据迁移），照常按 v{claimed} 读"
                    );
                    return (settings_from_map(&map, claimed), None);
                };
                eprintln!("config: 迁移前已留证 {kept:?}");
                let reached = migrate_settings(&mut map, claimed);
                let s = settings_from_map(&map, reached);
                if reached > claimed {
                    save_settings_at(path, &s);
                    eprintln!("config: {path:?} 已从 v{claimed} 迁到 v{reached}");
                }
                return (s, None);
            }
            (settings_from_map(&map, claimed), None)
        }
        other => (AppSettings::default(), shape_fault(path, &other)),
    }
}

/// 数组非空却一条都不成形 ⇒ **整份清单不认**，走与语法坏同一条留证路径。
///
/// 这条判据不能被逐条容错吃掉：逐条容错的前提是"还有别的条读得出来"。全丢光又不留证，
/// 调用方拿到的就是"空清单 + 没有故障"，与"用户把清单删空了"一模一样（`[]`），于是照常
/// 播种写回 ⇒ 唯一那份现场被覆盖——2026-10-22 真机⑫的教训换了实现也不能破。
/// 反过来说，`[1, 2]` 这种一个字节都不值得留的写法本来就该进 `.bad`，批次 X 那条
/// 「真的坏才报坏，且要留证在原位置之外」的断言到此仍然成立。
fn quarantine_total_loss(path: &Path, count: usize) -> Option<Corrupted> {
    eprintln!(
        "config: {path:?} 里 {count} 个元素没一个够作条目 ⇒ 整份清单不认，先留证再按空清单启动"
    );
    std::fs::read(path)
        .ok()
        .map(|bytes| quarantine(path, &bytes))
}

/// 读一份用户清单（路径参数化，同上）。顶层必须是数组；数组里逐条逐字段容错。
/// 与偏好同一处分流：语法坏 / 顶层不是数组 / 非空数组但一条都不成形 ⇒ 留证且不回写；
/// `[]` ⇒ 干净的空清单（用户真的把条目删光了，与"坏"是两回事）。
fn load_user_items_at(path: &Path) -> (Vec<LaunchItem>, Option<Corrupted>) {
    let (tree, fault) = load_or_quarantine::<serde_json::Value>(path);
    if let Some(e) = fault {
        return (Vec::new(), Some(e));
    }
    match tree {
        serde_json::Value::Null => (Vec::new(), None),
        serde_json::Value::Array(list) => {
            let items = items_from_tree(&list);
            if items.is_empty() && !list.is_empty() {
                return (Vec::new(), quarantine_total_loss(path, list.len()));
            }
            (items, None)
        }
        other => (Vec::new(), shape_fault(path, &other)),
    }
}

fn save_settings_at(path: &Path, s: &AppSettings) {
    match serde_json::to_vec_pretty(s) {
        Ok(json) => write_atomic(path, &json, "settings.json"),
        Err(e) => eprintln!("source: 序列化偏好失败: {e}"),
    }
}

/// 用户偏好（`settings.json`，与清单同目录、同样便携优先）。
/// 只放真实生效的开关——不设无功能的占位项（设置页立规同口径）。
///
/// 批次 T 起不再是 `Copy`：`web_engines` 是一张可变长的表，而这张表要能被用户手改。
/// 代价实测出来一处：`Self::new` 里原本在把 `prefs` 按值放进结构体**之后**还读它的两个热键字段，
/// 不再是 `Copy` 就编译不过 ⇒ 那三个初值改成先取出来（`src/main.rs:1608` 起）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// 字段形态版本（FR-CFG-01）。**缺键就是 v0**——这一格是批次 E2 才加的，老文件里没有。
    /// 读盘时它决定走不走迁移链，所以由 [`load_settings_at`] 单独判过再灌进来，
    /// 不由下面的字段表兜（"读到的版本"和"实际用的版本"不能两解）。
    #[serde(default)]
    pub schema_version: u32,

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

    /// 全局呼出 / 隐藏热键（FR-WIN-02 + FR-CFG-05，批次 Q 2026-10-19：此前写死在
    /// `src/main.rs` 的注册调用里）。存的是 Windows 原始数值，见 [`HotkeySpec`]。
    #[serde(default)]
    pub toggle_hotkey: HotkeySpec,

    /// 补字符热键（批次 R，2026-10-20 用户追加需求）：按下它就把 [`AppSettings::toggle_hotkey`]
    /// 那个键**不带修饰时打出的字符**投递给当前焦点窗口。存在的理由只有一条——把裸 `` ` ``
    /// 注册成全局键之后，系统会把那次按键整条吞掉，全机器就再也打不出 `` ` `` 了，
    /// 这个热键就是把字符还回去的出口。`None` = 不启用。
    #[serde(default)]
    pub typeback_hotkey: Option<HotkeySpec>,

    /// 次级动作的形态（批次 E3-丙，2026-10-26 用户正文「快捷键版和面板版两个都做，设置中加一个
    /// 配置，默认快捷键版，用户可以更改配置选择"面板版"」）。见 [`SecondaryAction`]。
    #[serde(default)]
    pub secondary_action: SecondaryAction,

    /// 次级动作键（同批）。**窗口内热键**，不是全局键：只在搜索页有焦点时生效，
    /// 不注册给系统、也不进热键线程。缺省 `Ctrl+Enter`（需求书 FR-ACT-01 的原文键位）。
    ///
    /// 兜底值必须显式指定：`#[serde(default)]` 只会调 `HotkeySpec::default()`，那是**呼出**键
    /// `Ctrl+Alt+Space` ⇒ 旧配置文件读出来的次级动作键会跟呼出键同组（永远收不到按键，
    /// 而且它还是个全局键）。DEFAULT ≠ SECONDARY_DEFAULT 这件事在这一格上是要命的。
    #[serde(default = "secondary_hotkey_default")]
    pub secondary_hotkey: HotkeySpec,

    /// 网页搜索引擎表（FR-SRC-11，批次 T）。字段缺省＝[`websearch::builtin_engines`] 那九条，
    /// 所以旧 `settings.json` 照旧解析、不写就不丢内置项。
    /// 本批**没有编辑入口**：设置页还没有「数据源 / 网页搜索」那一页（与 FR-CFG-04 的偏差已登记），
    /// 要加自定义引擎目前只能手改这个文件——需求书 TC 11-1 那条 `localhost:{query|8000}` 就是这么验的。
    #[serde(default = "crate::source::websearch::builtin_engines")]
    pub web_engines: Vec<websearch::WebEngine>,

    /// 出厂命令集已经自动补到第几轮（§4.2.1）。`0` = 这份清单建于 2026-10-21 扩充出厂集之前
    /// ⇒ 下次启动补一次缺，补完记账。记过账之后就不再自动补 ⇒ 用户删掉的条目不会自己回来；
    /// 误删想找回走设置页的「补回缺失的出厂命令」（见 [`merge_missing_seeds`]）。
    #[serde(default)]
    pub seed_round: u32,

    /// 关掉的数据源（批次 A10「数据源」设置页）。存 [`SourceId::as_str`] 的稳定名而不是枚举值：
    /// 将来插件源的 id 写进老配置、或某个源被删掉，读配置这一侧只认自己认识的名字，
    /// 不会因为多出一个不认识的项就把整份 `settings.json` 判坏（E1 那种事故的形状）。
    #[serde(default)]
    pub sources_off: Vec<String>,

    /// 内置条目里被用户隐藏的那些（FR-SRC-17 的 `hidden[]`）：存派生 uid。
    /// 「隐藏」与「删除」的差别只有一条：隐藏的条目在下次「补回缺失的出厂命令」之后
    /// 仍会被剔除，而删掉的条目会回来（再由用户自己再删）。本批没有 UI 入口——手改这个数组。
    /// 【2026-10-24 批次 F7：有了 UI 入口——主界面空查询下 Ctrl+Del 隐藏出厂条目，
    /// 设置页「高级」页列出并恢复。上一句"没有 UI 入口"保留为当时的记录。】
    /// 【2026-10-24 批次 D6：不再限出厂条目——扫描导入的 .lnk / Store 应用也能隐藏，
    /// 名称与恢复靠下面的 [`Self::hidden_stash`]。】
    #[serde(default)]
    pub hidden: Vec<String>,

    /// 批次 D6：被隐藏条目的完整快照（按派生 uid 覆盖式保存）。
    /// `hidden[]` 只存 uid；出厂条目还能从 [`builtin_seeds`] 重建，而扫描项没有种子，
    /// 一旦丢了快照就永远恢复不了 ⇒ 隐藏时一律存一份，恢复时连置顶与学习计数一起回来。
    /// 老配置没有这一格（`#[serde(default)]`）⇒ 空表，恢复退回按种子重建。
    #[serde(default)]
    pub hidden_stash: Vec<LaunchItem>,

    /// 危险操作（关机 / 重启 / 注销 / 清空回收站）执行前先弹确认（FR-SRC-17 `confirm_actions`）。
    /// **默认开**：这四条都不可撤销，误按一次就丢工作；关掉它等于把回车键直接接到关机命令上。
    #[serde(default = "default_true")]
    pub confirm_actions: bool,

    /// `>` 终端用哪个 shell（FR-SRC-16）。存 [`terminal::TermShell::as_str`] 的稳定名，
    /// 与 `sources_off` 同一口径：本程序不认识的名字回落 `cmd`，不让整份配置判坏。
    #[serde(default = "default_terminal_shell")]
    pub terminal_shell: String,

    /// 当前主题（FR-UIX-01 / 批次 F1）。存主题名；`System` = 跟随系统深浅，
    /// 其余名字先找用户 `Themes\<name>.json`、再找内置。缺省 `Light`：
    /// 需求书那条验收是「删光主题文件仍能启动且为浅色」，默认值不能靠"跟随系统"实现。
    /// 不认识的用户主题在解析侧回落 Light 并记日志，不影响本文件其余字段。
    #[serde(default = "default_theme")]
    pub theme: String,

    /// 主界面列表一屏列多少行（2026-10-24 用户需求：「默认显示 10 条，可配置 10 条 / 全部」）。
    /// 窗口默认高度按 10 行算，两个档位都不改变窗口大小——「全部」只是把截断去掉、靠滚轮翻。
    /// 默认值后来按用户正文改成了 [`ListRows::All`]（Delphi 口径），「10 条」是可选的截断档。
    #[serde(default)]
    pub list_rows: ListRows,

    /// 主界面一行的松紧（2026-10-24 用户正文：「整体界面高度太高了，项与项之间的间距太大了」）。
    /// 三档是用户点名的可选项，默认 [`RowDensity::Snug`]（甲）；第四档 [`RowDensity::Theme`]
    /// 把行高与图标尺寸交还给主题的 `row_height` / `icon_size`（批次 F1 那 24 个键里的两个），
    /// 不换这一档时主题里的这两个键不生效——明写在设置页文案里。
    #[serde(default)]
    pub row_density: RowDensity,

    /// 缺省分类范围（批次 G1）。「全部」＝保持 G1 之前的行为（默认）；
    /// 「基本」＝出厂白名单 + 用户无标签条目，维护类出厂条目只在 `@维护` 时出现。
    /// 显式 `@分类` 临时并进来，不经过这一格。
    #[serde(default)]
    pub default_scope: Scope,

    /// 窗口宽度 / 记住位置 / 状态栏 / 淡入（批次 F8）。见 [`Appearance`]。
    #[serde(default)]
    pub appearance: Appearance,

    /// 搜索历史存几条（FR-QRY-10，批次 C3）。需求书 4.2 定的默认值是 30。
    /// **`0` = 不记历史**——这一格是隐私开关，不是"存零条然后崩"：[`push_history`] 收到 0
    /// 会把表清成空，`recall_history` 拿到空表恒返回"没变化"。
    /// 读进来的值不直接信，用 [`AppSettings::history_limit`] 取（手改成 9999 也只是夹到上限，
    /// 与 [`AppSettings::term_shell`] 对陌生稳定名回落 `cmd` 同一口径）。
    #[serde(default = "default_history_size")]
    pub history_size: usize,

    /// 文件索引的扫描目录清单（FR-SRC-02 / FR-SRC-14 的 `scope_folders`，批次 D7）。
    /// 存 `%VAR%` 形态的原始串（缺省＝[`filesearch::default_scope_folders`] 那六条，
    /// `Desktop` 与 OneDrive 重定向位都列上、建索引时滤掉不存在的）。
    /// 空清单＝不索引任何东西，是合法（虽然没意义）的用户选择，不判坏。
    #[serde(default = "default_file_folders")]
    pub file_folders: Vec<String>,

    /// 递归深度（`scope_depth`）。缺省 4；设置页给 2/4/6/8 四个档位。
    #[serde(default = "default_file_depth")]
    pub file_depth: usize,

    /// 按目录名排除（`scope_exclude`）。缺省＝[`filesearch::default_scope_exclude`] 那七条；
    /// 设置页可增删，「恢复默认排除」写回缺省。
    #[serde(default = "default_file_exclude")]
    pub file_exclude: Vec<String>,

    /// 整张索引的条目上限（`max_entries`）。缺省 30000；设置页给 1万/3万/10万 三个档位。
    #[serde(default = "default_file_max_entries")]
    pub file_max_entries: usize,

    /// 两轮重扫的间隔分钟数（`refresh_minutes`）。缺省 30；设置页给 5/15/30/60/180 五个档位。
    #[serde(default = "default_file_refresh_minutes")]
    pub file_refresh_minutes: u64,

    /// 进文件模式的最小过滤字数（`min_query_length`）。缺省 2；设置页给 1/2/3 三个档位。
    #[serde(default = "default_file_min_query_len")]
    pub file_min_query_len: usize,
}

fn default_terminal_shell() -> String {
    crate::source::terminal::TermShell::default()
        .as_str()
        .to_string()
}

fn default_theme() -> String {
    crate::theme::FALLBACK_THEME.to_string()
}

fn default_true() -> bool {
    true
}

/// 为什么手写而不是派生：`web_engines` 的 serde 默认值只在**字段缺失**时生效，
/// 而整份 `settings.json` 缺失或坏掉时 `load_json` 走的是 `T::default()`——
/// 派生实现给的是空表，于是首次运行与 E1 重建后的网页搜索一条引擎都没有
///（2026-10-23 真机 A6 的根因）。
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            // 一份"程序自己造出来的"配置当然是当前版本。首启写盘带的就是这个号，
            // 于是老文件（缺键 ⇒ v0）与新文件（v1）在日志里一眼分得开。
            schema_version: SETTINGS_SCHEMA_VERSION,
            space_to_run: SpaceRun::default(),
            tab_mode: TabMode::default(),
            learn_rank: LearnRank::default(),
            toggle_hotkey: HotkeySpec::default(),
            typeback_hotkey: None,
            secondary_action: SecondaryAction::default(),
            secondary_hotkey: HotkeySpec::SECONDARY_DEFAULT,
            web_engines: crate::source::websearch::builtin_engines(),
            seed_round: 0,
            sources_off: Vec::new(),
            hidden: Vec::new(),
            hidden_stash: Vec::new(),
            confirm_actions: true,
            terminal_shell: default_terminal_shell(),
            theme: default_theme(),
            list_rows: ListRows::default(),
            row_density: RowDensity::default(),
            default_scope: Scope::default(),
            appearance: Appearance::default(),
            history_size: HISTORY_SIZE_DEFAULT,
            file_folders: default_file_folders(),
            file_depth: default_file_depth(),
            file_exclude: default_file_exclude(),
            file_max_entries: default_file_max_entries(),
            file_refresh_minutes: default_file_refresh_minutes(),
            file_min_query_len: default_file_min_query_len(),
        }
    }
}

/// 需求书 4.2 定的默认条数。
pub const HISTORY_SIZE_DEFAULT: usize = 30;

/// 这一格的上限：滑块给到 200 就到底，手改成一万也只是夹到 200。
/// 历史是**每次都全量重写的小文件**（一条字符串数组），上限不是为了省磁盘，
/// 是为了让"翻历史"这件事在几百次按键后仍然翻得完。
pub const HISTORY_SIZE_MAX: usize = 200;

fn default_history_size() -> usize {
    HISTORY_SIZE_DEFAULT
}

// 文件搜索的六格缺省（批次 D7）——与 [`filesearch::ScanScope::default`] 同源，零漂移。
fn default_file_folders() -> Vec<String> {
    crate::source::filesearch::default_scope_folders()
}

fn default_file_depth() -> usize {
    crate::source::filesearch::SCOPE_DEPTH
}

fn default_file_exclude() -> Vec<String> {
    crate::source::filesearch::default_scope_exclude()
}

fn default_file_max_entries() -> usize {
    crate::source::filesearch::MAX_ENTRIES
}

fn default_file_refresh_minutes() -> u64 {
    crate::source::filesearch::REFRESH_MINUTES_DEFAULT
}

fn default_file_min_query_len() -> usize {
    crate::source::filesearch::MIN_QUERY_LEN
}

impl AppSettings {
    /// 这个源此刻是否启用。名字不认识的一律当"没关"——只影响本程序认识的几个源。
    pub fn source_on(&self, id: SourceId) -> bool {
        !self.sources_off.iter().any(|s| s == id.as_str())
    }

    /// 终端源实际用的 shell：配置里那一格是稳定名，不认识就回落 `cmd`。
    pub fn term_shell(&self) -> crate::source::terminal::TermShell {
        crate::source::terminal::TermShell::from_name(&self.terminal_shell).unwrap_or_default()
    }

    /// 历史实际存几条：夹进 `0..=HISTORY_SIZE_MAX`。配置里那一格是用户手写得出来的，
    /// 与 [`term_shell`](Self::term_shell) 同一口径——**不在读盘时改写它**（纯读不回写是
    /// FR-CFG-01 那条不变量），所以每一次用到都过这道夹子。`0` 是合法的"不记历史"。
    pub fn history_limit(&self) -> usize {
        self.history_size.min(HISTORY_SIZE_MAX)
    }

    /// 文件源实际用的扫描范围（批次 D7）：六格汇编成 [`filesearch::ScanScope`] 并夹回
    /// 有效区间（[`ScanScope::sanitized`]）。夹子在这一步过、**不在读盘时改写配置**——
    /// 与 [`history_limit`](Self::history_limit) 同一条不变量。
    pub fn file_scan_scope(&self) -> crate::source::filesearch::ScanScope {
        crate::source::filesearch::ScanScope {
            folders: self.file_folders.clone(),
            depth: self.file_depth,
            exclude: self.file_exclude.clone(),
            max_entries: self.file_max_entries,
            refresh: std::time::Duration::from_secs(self.file_refresh_minutes * 60),
            min_query_len: self.file_min_query_len,
        }
        .sanitized()
    }

    /// 开 / 关一个源；返回 `true` ＝ 状态真的有变化（调用方据此写盘）。
    pub fn set_source(&mut self, id: SourceId, on: bool) -> bool {
        let listed = self.sources_off.iter().any(|s| s == id.as_str());
        match (on, listed) {
            (true, true) => {
                self.sources_off.retain(|s| s != id.as_str());
                true
            }
            (false, false) => {
                self.sources_off.push(id.as_str().to_string());
                true
            }
            _ => false,
        }
    }
}

// ---- FR-WIN-02 全局呼出热键（批次 Q）----
//
// 位值与 Virtual-Key 码在这里重述一遍，而不是从 `windows` crate 引：判据（[`HotkeySpec::check`]）
// 和展示名（[`HotkeySpec::display`]）必须是纯函数，才能在 `cargo test` 里跑满而不碰系统。
//
// 【2026-10-27 更正】本轮修复前，上面那句末尾还写着「`src/main.rs` 那边用的是 windows 的同名
// 常量，两边是同一套数」——**Shift 这一位不是**。那句话随本批删掉，原文照录在这里（不悄悄抹）。
// 这四位唯一的出口是 `main.rs:2902` 的
// `RegisterHotKey(.., HOT_KEY_MODIFIERS(spec.mods), ..)`，所以这里的数字必须逐位等于 Win32 的
// `MOD_ALT=0x1 / MOD_CONTROL=0x2 / MOD_SHIFT=0x4 / MOD_WIN=0x8 / MOD_NOREPEAT=0x4000`；
// 首版把 Shift 抄成了 `0x0010`（那是 `VK_SHIFT` 的扫描位概念，不是 MOD 位），后果不是显示错而是
// **整条全局键永远注册不上**：带 Shift 的 `mods` 交给 `RegisterHotKey` 直接返回失败，
// `GetLastError()=1004`。实测（`target/e6/hkprobe.ps1`，同一台机器同一个消息窗）：
// `mods=0x4012` 配 F12 失败 1004、配 F13 同样失败 1004（换了键就换不掉 ⇒ 不是"被别人占用"），
// `mods=0x4006` 配 F12 成功。【同日补测 `target/e6/hkprobe2.ps1`：那轮用的是 F12/F13，"换键还失败"
// 并不足以排掉"这两位本来就被别人占着"这一条解释 ⇒ 换 **F15/F16**（本机没有实体键、也没人占）重跑：
// `0x4010`（只有那颗错的 Shift）失败 1004、`0x4012` 失败 1004、`0x4004` 与 `0x4006` 成功；同一条
// `0x4012` 在 F12 上也是失败、`0x4006` 在 F12 上成功 ⇒ 拒绝来自**那一位本身**，与选哪个键无关。
// 探针的读法陷阱一并记下：注册**成功**时 `GetLastWin32Error()` 会给一个陈旧值（这台机器上是 203），
// 只有返回 FALSE 时那位数才有意义。】
// 为什么能拖两个批次没人发现：`grep -c 注册失败` 在 `target/debug`、`target/e5`、`target/c3` 三份
// 日志里都是 0 ⇒ 这台机器上从来没真存进过一条带 Shift 的组合，失败分支一次都没走过。
// 而 [`HotkeySpec::check`] 是明确放行 `Ctrl+Shift+<键>` 的
// （`application.rs` 里 `ok(HKF_SHIFT | HKF_CONTROL, VK_OEM_3)` 那条单测就是它的正面样本）
// ⇒ 判据说合法、系统说非法，两边在 Shift 这一位上分叉了整整两个批次。
// 修法是改这一枚常量，别的地方（`check` / `display` / `mods_named` / `conflicts_local` /
// 窗口内热键的逐位相等）都只按符号位比较，与位值无关，跟着一起对上。
// 边界：修复前录进 `settings.json` 的带 Shift 组合存的是 `0x10`，那位现在读作"没按 Shift"。
// 那种值从来没注册成功过 ⇒ 不存在"以前能用、现在坏了"的旧数据；真遇到就重录一次，不加迁移。

pub const HKF_ALT: u32 = 0x0001;
pub const HKF_CONTROL: u32 = 0x0002;
pub const HKF_WIN: u32 = 0x0008;
pub const HKF_SHIFT: u32 = 0x0004;
/// 按住不放不连发。老版本写死在注册调用里，批次 Q 起恒带上（没有配置它的入口）。
pub const HKF_NOREPEAT: u32 = 0x4000;

pub const VK_BACK: u32 = 0x08;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_SPACE: u32 = 0x20;
/// `0`–`9` 与 `A`–`Z` 的 VK 就是 ASCII 大写。
pub const VK_0: u32 = 0x30;
pub const VK_9: u32 = 0x39;
pub const VK_A: u32 = 0x41;
pub const VK_Z: u32 = 0x5A;
pub const VK_F1: u32 = 0x70;
pub const VK_F24: u32 = 0x87;
pub const VK_DELETE: u32 = 0x2E;
/// 反引号 `` ` `` 键（美式布局上同时是 `~` 键，`Win32_UI_Input_KeyboardAndMouse` 里叫 `VK_OEM_3`）。
/// 批次 R（2026-10-20 用户点名「重音号 ` 」）：这是唯一允许不带修饰键注册成全局热键的键。
pub const VK_OEM_3: u32 = 0xC0;

/// `RegisterHotKey` 失败码：组合已被系统或别的程序占走。这是"全局冲突"唯一的实测途径
/// （Windows 没有枚举别人热键的 API），所以判据只能是"试注册"。
/// 值取自 `windows-0.62.2` 绑定源（`Foundation/mod.rs:2677` = `1409u32`），与需求书附录 E
/// E-01 当年实测的 `HRESULT 0x80070581` 同数（低字 `0x581` = 1409）。**不是 1410**
/// （1410 = `ERROR_CLASS_ALREADY_EXISTS`，同一份绑定源 `:1374`）——写错只会让"已被占用"
/// 那句提示永远不显示、退化成裸错误码。
pub const ERROR_HOTKEY_ALREADY_REGISTERED: u32 = 1409;

/// 全局热键的组合。存数值而不存 `egui::Key`：后者没有显式判别值（`src/data/key.rs` 里
/// 枚举按声明顺序自动编号），依赖一升级就可能漂，而这两个数要直接喂给 `RegisterHotKey`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeySpec {
    /// `fsModifiers` 位掩码（[`HKF_ALT`] 等几个常量相或，含 [`HKF_NOREPEAT`]）。
    pub mods: u32,
    /// Virtual-Key 码。
    pub vk: u32,
}

impl HotkeySpec {
    /// 出厂值 = `Ctrl+Alt+Space`。用 const 而不只是 [`Default::default`]，
    /// 好让 `static` 里的 `Mutex::new` 能在 const 上下文初始化。
    pub const DEFAULT: Self = Self {
        mods: HKF_CONTROL | HKF_ALT | HKF_NOREPEAT,
        vk: VK_SPACE,
    };

    /// 批次 E3-丙：次级动作键的出厂值 = `Ctrl+Enter`（需求书 FR-ACT-01 的原文键位）。
    /// 与 [`Self::DEFAULT`] 一样写成 const，好让 `Default` 之外的地方也能直接引用同一份值。
    pub const SECONDARY_DEFAULT: Self = Self {
        mods: HKF_CONTROL | HKF_NOREPEAT,
        vk: VK_RETURN,
    };

    /// 去掉 [`HKF_NOREPEAT`]：它是行为位，不是键名的一部分，比对与展示都要剥掉它。
    pub fn mods_named(&self) -> u32 {
        self.mods & !HKF_NOREPEAT
    }

    /// 设置页与日志共用同一份实现，避免两处口径漂移。
    pub fn display(&self) -> String {
        let m = self.mods_named();
        let mut s = String::new();
        if m & HKF_CONTROL != 0 {
            s.push_str("Ctrl+");
        }
        if m & HKF_ALT != 0 {
            s.push_str("Alt+");
        }
        if m & HKF_SHIFT != 0 {
            s.push_str("Shift+");
        }
        if m & HKF_WIN != 0 {
            s.push_str("Win+");
        }
        s.push_str(&vk_name(self.vk));
        s
    }

    /// 录制落定前的合法性判据。只管"这个组合能不能当全局键"，冲突另判（本程序键表预检 +
    /// 试注册失败码），因为冲突的一半只有 Windows 知道。
    ///
    /// 批次 R（2026-10-20 用户裁定「只放开 `` ` `` 这一个键」）新增一条例外：**裸 `` ` ``**
    /// 可以不带任何修饰键注册。判据形状是"不带修饰只允许 `` ` ``；单带 Shift 一律不允许"，
    /// 后者是批次 Q 那条老规则的具体化，对 `` ` `` 尤其要紧——`Shift+` 打出来就是 `~`，
    /// 挂成全局键等于把波浪号也从全系统抹掉，而 `` ` `` 本身已经被裸 `` ` `` 那条注册吃掉了，
    /// 换不回任何东西（本机实测：裸 `` ` `` 装着时 `Shift+` `` ` `` 仍能正常打出 `~`）。
    pub fn check(&self) -> Result<(), &'static str> {
        let m = self.mods_named();
        let function = (VK_F1..=VK_F24).contains(&self.vk);
        let letterish =
            matches!(self.vk, VK_A..=VK_Z | VK_0..=VK_9 | VK_SPACE | VK_RETURN | VK_OEM_3);
        if !(function || letterish) {
            return Err("只收字母、数字、空格、回车、反引号 ` 与 F1–F24");
        }
        if function {
            return Ok(());
        }
        // 单带 Shift（不带 Ctrl/Alt/Win）一律挡：这类组合只是打字，挂成全局键就是拦打字。
        if m == HKF_SHIFT {
            return if self.vk == VK_OEM_3 {
                Err(
                    "单带 Shift 的 ` 就是 ~，挂成全局键会连波浪号一起吞掉；要带就带 Ctrl、Alt 或 Win",
                )
            } else {
                Err("只带 Shift 的键都只是打字，不能当全局热键")
            };
        }
        if m == 0 {
            // 无修饰键的唯一例外。裸 `` ` `` 的代价（全系统打不出 `` ` ``）由补字符热键补回来，
            // 见 [`AppSettings::typeback_hotkey`]。
            return self
                .backtick_bare()
                .then_some(())
                .ok_or("不带修饰键的全局热键只允许 ` 这一个键");
        }
        if m & (HKF_CONTROL | HKF_ALT | HKF_WIN) == 0 {
            return Err("这个键要当全局热键，至少得带 Ctrl、Alt 或 Win");
        }
        Ok(())
    }

    /// 是不是"不带任何修饰键的 `` ` ``"——批次 R 里唯一被允许的裸键。
    /// 设置页用它提示代价，`src/main.rs` 的录制期注销也只看主热键是不是这一种形态。
    pub fn backtick_bare(&self) -> bool {
        self.vk == VK_OEM_3 && self.mods_named() == 0
    }

    /// 补字符热键（[`AppSettings::typeback_hotkey`]）专用的判据：先过 [`check`](Self::check)，
    /// 再多两条只有这个槽才有的规矩。放在这里而不是调用点，是因为这两条也能纯函数单测。
    pub fn check_typeback(&self, toggle: &HotkeySpec) -> Result<(), &'static str> {
        self.check()?;
        if self.mods_named() == 0 {
            return Err("补字符热键必须带修饰键：不带修饰的话，吞掉字符的就换成它自己");
        }
        if (self.mods_named(), self.vk) == (toggle.mods_named(), toggle.vk) {
            return Err("补字符热键不能和呼出热键是同一组：系统只会把它当成呼出键");
        }
        Ok(())
    }

    /// 撞上本程序自己的键表（新建 / 编辑 / 删除 / 导航 / 数字行…）。
    pub fn conflicts_local(&self, table: &[(u32, u32)]) -> bool {
        let mine = (self.mods_named(), self.vk);
        table.iter().any(|(m, v)| (*m & !HKF_NOREPEAT, *v) == mine)
    }

    /// 窗口内热键（[`AppSettings::secondary_hotkey`]）的判据。**与 [`check`](Self::check) 分家**
    /// 是因为管的不是一回事：那条管"能不能注册成全局键"（裸 `` ` `` 可以、单带 Shift 不行），
    /// 这条管"窗口里按下去会不会把打字吃掉"——全局键被系统整条吞掉，窗口内热键只吃本程序这一下，
    /// 所以裸键在这里一条都不放开（裸 `` ` `` 的代价已经在全局那一侧付过了）。
    pub fn check_secondary(
        &self,
        toggle: &HotkeySpec,
        typeback: Option<&HotkeySpec>,
        local_table: &[(u32, u32)],
    ) -> Result<(), &'static str> {
        let m = self.mods_named();
        let function = (VK_F1..=VK_F24).contains(&self.vk);
        let letterish =
            matches!(self.vk, VK_A..=VK_Z | VK_0..=VK_9 | VK_SPACE | VK_RETURN | VK_OEM_3);
        if !(function || letterish) {
            return Err("只收字母、数字、空格、回车、反引号 ` 与 F1–F24");
        }
        if m & HKF_WIN != 0 {
            return Err("窗口内热键用不了 Win 键：egui 在 Windows 上不把 Win 传上来，设了也永远匹配不到");
        }
        if m & (HKF_CONTROL | HKF_ALT) == 0 {
            return Err("窗口内热键必须带 Ctrl 或 Alt：裸键与单带 Shift 的键都只是打字，抢走就再也打不出来");
        }
        if (m, self.vk) == (toggle.mods_named(), toggle.vk) {
            return Err("不能和全局呼出热键同一组：那个组合被系统整条吞掉，窗口里永远收不到");
        }
        if let Some(t) = typeback {
            if (m, self.vk) == (t.mods_named(), t.vk) {
                return Err("不能和补字符热键同一组：那颗键要留给把字符还回去");
            }
        }
        if self.conflicts_local(local_table) {
            return Err("这组键已被本程序的固定快捷键占用（完整键表见 ? 速查表）");
        }
        Ok(())
    }
}

impl Default for HotkeySpec {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// `AppSettings::secondary_hotkey` 的 serde 兜底。单独写一个 fn 是因为 `#[serde(default)]`
/// 只会调 `Default::default()`（= 呼出键 Ctrl+Alt+Space），两者不是同一个值。
fn secondary_hotkey_default() -> HotkeySpec {
    HotkeySpec::SECONDARY_DEFAULT
}

/// VK → 展示名。范围外的码原样写成 `VK#xNN`，让日志里看得见"没映射到"而不是显示空白。
/// 键名用英文（Space / Enter）：设置页那张快捷键表一直这么写，且启动日志那行
/// `hotkey: Ctrl+Alt+Space registered` 从批次 A 起就是原文，复测判据直接抄它——
/// 改成「空格」会让同一份判据对不上号。
/// 唯一例外是 [`VK_OEM_3`]，直接显示成 `` ` ``：这个键的名字就是它打出的那个字符，
/// 写成 "Backtick" 反而要用户先去查词，而 `Win+`` ` `` 这种串正是用户提需求时的原话。
fn vk_name(vk: u32) -> String {
    match vk {
        VK_A..=VK_Z => char::from_u32(vk).unwrap_or('?').to_string(),
        VK_0..=VK_9 => char::from_u32(vk).unwrap_or('?').to_string(),
        VK_SPACE => "Space".to_string(),
        VK_RETURN => "Enter".to_string(),
        VK_BACK => "Backspace".to_string(),
        VK_DELETE => "Delete".to_string(),
        VK_OEM_3 => "`".to_string(),
        VK_F1..=VK_F24 => format!("F{}", vk - VK_F1 + 1),
        other => format!("VK#{other:02X}"),
    }
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
            SpaceRunWire::Name(n) => Self::from_name(&n)
                .ok_or_else(|| D::Error::custom(format!("未知的 space_to_run 档位：{n:?}"))),
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

/// 主界面列表的两种列法（2026-10-24 用户需求）。
///
/// 两档都不改窗口高度：窗口一开始就建到 10 行高，「全部」不是把窗口拉长，而是把
/// [`Self::Ten`] 那道截断去掉、让滚轮去翻。这样"写查询时扫一眼十行"和"翻整张清单"
/// 是同一个窗口的两种用法，不会一换档就跳一下大小。
///
/// 默认 [`Self::All`]＝Delphi 版 ALTRun 的口径（`lstShortCut.Height := 10 * ItemHeight`
/// 只是视口高度，结果从不截断）。2026-10-24 用户正文「这个10条功能可以参考Delphi的
/// AltRun版本」⇒ 两个选项保留，但默认从「10 条」改成 Delphi 这一档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListRows {
    /// 只列前 10 行（头段行算在内）——窗口正好一屏，不用滚轮。
    Ten,
    /// 默认：全部结果都列出来，窗口仍 10 行高，超出部分用滚轮翻。
    #[default]
    All,
}

impl ListRows {
    /// 落盘与设置页用的档位名（与 `SpaceRun`/`TabMode`/`LearnRank` 同口径）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ten => "ten",
            Self::All => "all",
        }
    }
}

/// 主界面一行的松紧（2026-10-24 用户正文：「整体界面高度太高了，项与项之间的间距太大了，
/// 参考 Delphi 的 AltRun 版本，在它的基础上做一个优化版」）。
///
/// 三档的具体像素在 `main.rs` 的 [`crate::row_geometry`] 里给（那里才知道主题的 `padding`），
/// 这一层只管"选哪一档"与落盘名。取舍依据：
/// - Delphi 版 `lstShortCut` 的 `ItemHeight = 16`、行距 0、无图标（`frmALTRun.dfm:3339-3354`）
///   ——最密，但 16px 放不下图标和副标题，所以只做"密"这一头的参照；
/// - AHK 版 Light 主题 `RowHeight 54 / IconSize 32`、行距 0、宽 700（`ThemeManager.ahk:209-215`）
///   ——行高与我们原来的 Light 相同，它不显空是因为**行距 0 且窗口宽**；
/// - 我们窗口只有 500 宽，54 + 4 的步进就变成"行很松"。⇒ 甲档取 34（图标 24、标题 13pt
///   与灰副标题仍放得下），比 Delphi 松、比 AHK 紧，是用户点名的默认。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowDensity {
    /// 甲（默认）：行高 34、图标 24、行距 2 ⇒ Light 下窗口高 438（一屏正好十行）。
    #[default]
    Snug,
    /// 乙：行高 40、图标 28、行距 2 ⇒ 498，比甲透气一档。
    Roomy,
    /// 丙：行高 24、图标 16、行距 0 ⇒ 318，最接近 Delphi 的密但仍留图标。
    Dense,
    /// 跟随主题：用主题 JSON 里的 `row_height` / `icon_size`（批次 F1 那两键），行距 4。
    /// 不换到这一档，主题里的这两个键对主界面列表不生效。
    Theme,
}

impl RowDensity {
    /// 落盘与设置页用的档位名（与 [`ListRows::as_str`] 同口径）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snug => "snug",
            Self::Roomy => "roomy",
            Self::Dense => "dense",
            Self::Theme => "theme",
        }
    }
}

/// 批次 E3-丙（FR-ACT-01「`Ctrl+Enter` 走次级动作」+ 开发计划 B3）：那颗键按下去干什么。
///
/// 用户 2026-10-26 裁定两种形态都做、默认快捷键版、可切换，所以这不是"二选一裁掉一个"，
/// 而是同一个键的两种落点。快捷键版执行**一条**确定的次级动作（不给面板，也就没有"再选一次"）；
/// 面板版等于把 `→` 那个入口搬到这颗可配的键上，动作由用户在面板里选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecondaryAction {
    /// 默认：直接执行 [`crate::actions::secondary_action_for`] 给出的那条动作。
    #[default]
    Shortcut,
    /// 打开与 `→` 完全相同的操作面板（过滤词从空开始）。
    Panel,
}

impl SecondaryAction {
    /// 落盘与设置页用的档位名（与 [`Scope::as_str`] 同口径）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shortcut => "shortcut",
            Self::Panel => "panel",
        }
    }

    /// 设置页单选用的显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Shortcut => "快捷键版",
            Self::Panel => "面板版",
        }
    }
}

/// 批次 G1：缺省分类范围（`default_scope`，设置页「通用」页二选一）。
///
/// 「全部」＝所有条目都进候选池（G1 之前的行为，默认）；
/// 「基本」＝出厂白名单（[`FACTORY_TAG_BASIC`]）∪ 用户自己的无标签条目——维护类出厂条目
/// 平时不占列表，显式敲 `@维护` 才临时并进来。显式 `@分类` 不经过这一格（见 [`item_in_scope`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// 默认：所有条目都进候选池。
    #[default]
    All,
    /// 基本档：无标签 ∪ 标了「基本」的条目。
    Basic,
}

impl Scope {
    /// 落盘与设置页用的档位名（与 [`ListRows::as_str`] 同口径）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Basic => "basic",
        }
    }

    /// 设置页单选用的显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Basic => "基本",
        }
    }
}

/// 批次 F8（FR-UIX-02）：`settings.json` 的 `appearance` 段。
///
/// 需求书那句「宽度 / 行数 / 位置 / 淡入淡出属 `appearance` 不属主题」是这一格的来由：
/// 换主题不碰它，主题文件里写这些键也不会生效。「行数」后来由 `list_rows` 以另一种形态落地，
/// 剩下的四项在这里。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// 主界面宽度（96 DPI 逻辑像素）。范围见 [`WINDOW_WIDTH_MIN`] / [`WINDOW_WIDTH_MAX`]，
    /// 读盘与用之前都过 [`clamp_window_width`]——这个文件用户手改得动，0 不能原样进窗口。
    pub width: f32,
    /// 记住上次窗口位置（FR-UIX-03 的 `RememberPosition`）。
    pub remember_position: bool,
    /// 上次隐藏时的窗口左上角（屏幕坐标；多显示器在主屏左侧时为负）。
    /// 启动时要先问系统这个点还在不在显示器上，不在就落回默认位置。
    pub last_pos: Option<[i32; 2]>,
    /// 底部状态栏（FR-UIX-08 的 `status_bar`，默认开）。
    pub status_bar: bool,
    /// 显示时淡入（FR-UIX-07；原文就定的默认关）。
    pub fade: bool,
}

pub const WINDOW_WIDTH_MIN: f32 = 360.0;
pub const WINDOW_WIDTH_MAX: f32 = 900.0;
const WINDOW_WIDTH_DEFAULT: f32 = 500.0;

impl Default for Appearance {
    fn default() -> Self {
        Self {
            width: WINDOW_WIDTH_DEFAULT,
            remember_position: true,
            last_pos: None,
            status_bar: true,
            fade: false,
        }
    }
}

/// 宽度钳制。非有限值（手改成 `null` 之外的怪数）落回默认宽度。
pub fn clamp_window_width(w: f32) -> f32 {
    if w.is_finite() {
        w.clamp(WINDOW_WIDTH_MIN, WINDOW_WIDTH_MAX)
    } else {
        WINDOW_WIDTH_DEFAULT
    }
}

/// 批次 A10：整张清单（应用 + 自定义命令）的模糊匹配源。
///
/// 它只贡献普通结果，从不独占、也不出提示行——那些是其余几个源的事。
/// 关掉它最直接的后果是列表只剩头段（计算器 / 文件 / 网页），清单一条都搜不到。
#[derive(Debug, Default)]
pub struct AppsSource;

impl DataSource for AppsSource {
    fn id(&self) -> SourceId {
        SourceId::Apps
    }

    fn label(&self) -> &'static str {
        "应用程序清单"
    }

    fn note(&self) -> &'static str {
        "主清单的模糊匹配（名称 / 关键字 / 学习别名）。关掉之后清单里一条都搜不出来，只剩计算器、文件、网页这些头段。"
    }

    fn rows(&mut self, cx: &mut SearchCtx<'_>) -> Vec<usize> {
        cx.index
            .search(cx.query, cx.items, cx.learn_rank, cx.now_ms)
    }
}

/// 偏好文件与清单同目录（复用 `user_list_path` 已解析好的便携判定）。
fn settings_path() -> PathBuf {
    user_list_path().with_file_name("settings.json")
}

/// 日志文件路径：同样跟着清单走（便携目录可写就在 exe 旁边，否则退到 `%LOCALAPPDATA%`）。
/// 本程序是窗口子系统 ⇒ 没有控制台可刷日志，`src/main.rs` 的 `redirect_stderr_if_orphan()`
/// 只在「没人听 stderr」时把标准错误接到这里。
pub fn log_path() -> PathBuf {
    user_list_path().with_file_name("launcher.log")
}

/// 单文件封顶：超过就先归档（批次 O 立的数，本批沿用）。自启＝每次登录追加，不封顶就是无限大。
pub const LOG_ROTATE_BYTES: u64 = 512 * 1024;

/// 归档保留份数（不含正在写的 `launcher.log`）⇒ 自启场景下等于"最近 4 天"的启动日志。
pub const LOG_KEEP_ARCHIVES: usize = 3;

/// 文件首行的日期锚。归档要用"这份内容属于哪一天"命名，而这个日期由日志自己记在第一行——
/// 不走 mtime 换算：`FileTimeToSystemTime` 一类转换在本项目现有 feature 下三条路径实测全部不可达，
/// 手搓公历换算不是这一格该背的复杂度。锚读不出来就当没有日期归档，绝不猜一个填上去。
pub const LOG_DATE_TAG: &str = "# log 日期: ";

/// 今天的本地日期时间 `YYYY-MM-DD HH:MM`。`GetLocalTime` 是不引日期库就能拿到本地历日的唯一路子
/// （为此打开 `Win32_System_SystemInformation` feature——批次 O 之后第二次动 feature 列表）。
pub fn local_now_stamp() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute
    )
}

/// 备份文件名用的紧凑本地时间 `YYYYMMDD-HHMMSS`（批次 F7）。
/// 与 [`local_now_stamp`] 同一个来源：不引日期库，也不拿 UTC 冒充本地时间。
fn local_now_compact() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// 从日志首行取日期锚里的 `YYYY-MM-DD`；缺失、越界、非数字一律 None。
fn log_content_date(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut buf = [0u8; 128];
    let n = std::fs::File::open(path)
        .ok()
        .and_then(|mut f| f.read(&mut buf).ok())?;
    let head = std::str::from_utf8(&buf[..n]).ok()?;
    let first_line = head.lines().next()?;
    let date = first_line
        .strip_prefix(LOG_DATE_TAG)?
        .get(..10)?
        .to_string();
    is_iso_date(&date).then_some(date)
}

/// 跨天或超长时把 `launcher.log` 滚成 `launcher-<内容日期>.log`，再把归档裁到 `LOG_KEEP_ARCHIVES` 份，
/// 返回本次启动该写的横幅尾巴（调用方负责在换句柄**之后**写进去，见 `main.rs`）。
pub fn rotate_log_and_stamp() -> String {
    let stamp = local_now_stamp();
    let today = stamp.get(..10).unwrap_or_default().to_string();
    rotate_at(&log_path(), &today);
    stamp
}

/// 归档判据本身：不取系统时间、不碰真实清单目录 ⇒ 能在临时目录里跑完整往返（单测走这条）。
/// 首行没有日期锚的旧文件（批次 O 的产物）归成 `launcher-无日期.log`，这种归类每台机器只会发生一次。
fn rotate_at(path: &Path, today: &str) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    let dated = log_content_date(path);
    let stale = dated.as_deref() != Some(today);
    if stale || meta.len() > LOG_ROTATE_BYTES {
        archive_log(path, dated.as_deref().unwrap_or("无日期"));
    }
}

/// 重命名归档；同名已存在时加 `.2`、`.3`…（同一天两次超长会走到这里）。
fn archive_log(path: &Path, date: &str) -> Option<PathBuf> {
    let dir = path.parent()?;
    let stem = format!("launcher-{date}");
    let mut target = dir.join(format!("{stem}.log"));
    for n in 2..=99 {
        if !target.exists() {
            break;
        }
        target = dir.join(format!("{stem}.{n}.log"));
    }
    std::fs::rename(path, &target).ok()?;
    prune_archives(dir);
    Some(target)
}

/// 这个文件名是不是本程序自己写的归档？判据是**完整的名字形状**，不是前缀：
/// `launcher-<YYYY-MM-DD>.log` 或 `launcher-<YYYY-MM-DD>.<1..99>.log`，外加旧文件那格 `launcher-无日期.log`。
/// 只按 `launcher-` 前缀认会把用户放在同一目录里的 `launcher-notes.log` 之类的文件删掉——
/// 与批次 N「宁多收一条也不误判」、批次 L「归属不明一律不认领」是同一条取向。
fn is_our_archive(name: &str) -> bool {
    let Some(body) = name
        .strip_prefix("launcher-")
        .and_then(|b| b.strip_suffix(".log"))
    else {
        return false;
    };
    let mut parts = body.split('.');
    let stem = parts.next().unwrap_or("");
    if !(is_iso_date(stem) || stem == "无日期") {
        return false;
    }
    // 后缀序号必须是纯数字（`launcher-2026-01-01.a.log` 不是我们写的）。
    let tail: Vec<&str> = parts.collect();
    tail.iter()
        .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
}

/// 裁归档。**只删自己写的文件**：名字必须严格是 `launcher-*.log`，`launcher.log` 本身、
/// 批次 O 留下的 `launcher.log.old`、以及任何别人的东西一概不进这个列表。
fn prune_archives(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut list: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            if !is_our_archive(&name.to_string_lossy()) {
                return None;
            }
            Some((e.metadata().ok()?.modified().ok()?, e.path()))
        })
        .collect();
    if list.len() <= LOG_KEEP_ARCHIVES {
        return;
    }
    list.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in list.iter().skip(LOG_KEEP_ARCHIVES) {
        let _ = std::fs::remove_file(p);
    }
}

/// 读取偏好（E1，批次 X 立的"缺失回默认、坏了先留证再回默认"；E2，批次 E2 在其上加了
/// 版本戳、逐版迁移与字段级容错，三条下场的分流见 [`load_settings_at`]）。
/// 第二格是坏文件的下场（`Corrupted`），调用方拿它给用户提示，并且在 `Lost` 时停手不写。
/// NFR-04：坏配置不得影响启动。
pub fn load_settings() -> (AppSettings, Option<Corrupted>) {
    load_settings_at(&settings_path())
}

pub fn save_settings(s: &AppSettings) {
    save_settings_at(&settings_path(), s);
}

// ---- FR-QRY-10 批次 C3：搜索历史 ----

/// 历史文件与清单同目录（同一个便携判定，句法照 [`settings_path`]）。
///
/// **为什么不塞进 `settings.json`**：这一张是"机器每次执行都重排一遍"的表，而 settings.json 是
/// 用户会自己在编辑器里打开、手改、期望别的东西别碰的表；同一份文件里两种写入节奏互相覆盖，
/// 用户改一次设置就把历史洗掉（或反过来）。分开还有第二条好处：`.bad` 留证名额各算各的，
/// 历史反复写坏不会把清单的名额吃掉（[`quarantine`] 那 10 格是有原因的）。
fn history_path() -> PathBuf {
    user_list_path().with_file_name("search_history.json")
}

/// 读一份搜索历史：**顶层字符串数组，最新在前**。
/// 三条下场沿用 [`load_json`] 那一条判据（本文件注释立过的规矩：清单、偏好、历史共用，
/// 不许各写一份）：不在 ⇒ 空表；语法坏 / 顶层不是数组 / 数组里装的不是字符串 ⇒ 先留证再按空表启动，
/// **且调用方拿到非 `None` 的下场时本轮不许回写**。
fn load_history_at(path: &Path) -> (Vec<String>, Option<Corrupted>) {
    load_or_quarantine(path)
}

pub fn load_history() -> (Vec<String>, Option<Corrupted>) {
    load_history_at(&history_path())
}

pub fn save_history(entries: &[String]) {
    save_history_at(&history_path(), entries);
}

/// 路径参数化的那一份（与 [`load_settings_at`] 同一句法）：单测才打得到临时目录，
/// 不许把历史写进用户真实数据目录。
fn save_history_at(path: &Path, entries: &[String]) {
    match serde_json::to_vec_pretty(entries) {
        Ok(json) => write_atomic(path, &json, "search_history.json"),
        Err(e) => eprintln!("source: 序列化搜索历史失败: {e}"),
    }
}

/// 记一条历史：去重（**不分大小写**）后放到最前，超出 `limit` 的从尾部丢掉。
/// 返回 `true` = 这张表真的变了 ⇒ 调用方据此决定要不要写盘（与 [`AppSettings::set_source`]
/// 同一口径：没变就别多一次写）。
///
/// 与 AHK 版（`Knowledge.AddHistory`）差两处，都是刻意的：
/// - AHK 存的是 `StrLower(text)`，回填时用户敲过的 `C:\Foo` 会变成 `c:\foo`。这一格存的是
///   **可能要再执行一次的东西**，大小写敏感的场合不该被历史改掉 ⇒ **按原样存**，只在去重时比小写。
/// - 首尾空白剥掉（同 AHK 的 `Trim`）：尾巴带个空格的历史回填进输入框，会立刻从"匹配"换成
///   "独占模式"那一类完全不同的口径。
///
/// `limit = 0`（设置页那个隐私档）不需要单独分支：插进去再按上限裁掉，表又回到原样 ⇒
/// 下面那句"变了没有"给 `false`，一条都不记。唯一的例外是**表里原本有存货**：那一次裁剪把
/// 存货清空、返回 `true` ⇒ 落盘一次空表。这是把"我不再记了"顺带落成"我存的那些也没了"，
/// 想只清存货不改上限，用「高级」页那个「清空搜索历史」。
pub fn push_history(entries: &mut Vec<String>, text: &str, limit: usize) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let before = entries.clone();
    let key = t.to_lowercase();
    if let Some(pos) = entries.iter().position(|e| e.to_lowercase() == key) {
        entries.remove(pos);
    }
    entries.insert(0, t.to_string());
    while entries.len() > limit {
        entries.pop();
    }
    *entries != before
}

/// 翻历史的游标。AHK 用两格散字段（`HistoryIndex` 与 `_historyDraft`），这里合一：
/// 回填一次要同时动这两格，分开传给调用方就有"记了一格漏了另一格"的可能，
/// 症状是"再按一次跳回最早的一条"。
///
/// - `index = 0` ＝ 不在翻历史，输入框里是用户自己敲的内容；
/// - `index = n`（1 起算）＝ 正在看第 n 条，[`push_history`] 那张表的第 `n-1` 格。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryBrowse {
    pub index: usize,
    pub draft: String,
}

/// `Ctrl+↑`（`step = +1`，往更早）/ `Ctrl+↓`（`step = -1`，往更新，回到自己敲的那串）。
/// 返回 `Some(要填进输入框的文本)`，并把 `browse` 推到新位置；`None` = 到头了，**什么都不动**。
///
/// 两条从 AHK `RecallHistory` 原样转写的规矩，缺一条就别扭：
/// - 第一次从"不在翻历史"起步时，把此刻输入框里的内容存成草稿 ⇒ 翻到底再翻回来，
///   刚敲的两个字还在。
/// - 候选与输入框**当前内容**相同就再往下一格（循环，不是只走一步）。历史去过重，
///   所以能撞上的只有一种：用户新敲的词恰好等于历史里的某一条——那一格回填了等于没回填，
///   一次按键看不出反应，会被当成"这键没生效"。
///
/// 比较用原文精确比（不按大小写折叠）：与 [`push_history`] 的"按原样存"是同一个取向。
pub fn recall_history(
    entries: &[String],
    current: &str,
    browse: &mut HistoryBrowse,
    step: i32,
) -> Option<String> {
    if step == 0 {
        return None;
    }
    let browsing = browse.index > 0;
    let draft = if browsing {
        browse.draft.clone()
    } else {
        current.to_string()
    };
    let len = entries.len() as i64;
    let mut i = browse.index as i64;
    let text;
    loop {
        i += step as i64;
        if i < 0 || i > len {
            return None; // 已经是最早一条（↑）／已经站在草稿上（↓）
        }
        let cand = if i == 0 {
            draft.clone()
        } else {
            entries[(i - 1) as usize].clone()
        };
        if cand != current || i == 0 {
            text = cand;
            break;
        }
    }
    if !browsing {
        browse.draft = current.to_string();
    }
    browse.index = i as usize;
    Some(text)
}

/// 出厂命令集的轮次（§4.2.1 + 批次 A2）。每次往 [`builtin_seeds`] 里加条目就 +1：
/// 老用户的 `settings.json` 里记的是旧号 ⇒ 启动时自动补一次缺，补完即记账。
/// 记账之后不再自动补 ⇒ **删掉的条目不会因为下次启动又回来**。
///
/// 2026-10-23：1 → 2。FR-SRC-17 的系统命令五组（自身 / 电源与会话 / Windows 工具 /
/// ms-settings 页 / 音量与媒体）随这一轮补进老清单。
/// 2026-10-24：2 → 3。批次 G1 的分类标签随这一轮回填到老条目（[`apply_factory_tags`]）。
pub const SEED_ROUND: u32 = 3;

/// `%WINDIR%\System32\<exe>`：出厂条目的图标源（启动仍用简洁命令名）。
pub(crate) fn system32(exe: &str) -> String {
    let windir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    windir
        .join("System32")
        .join(exe)
        .to_string_lossy()
        .into_owned()
}

/// 名称拼音首字母 + 需求书点名要的关键字（空白分词，别名表本来就支持多键）。
pub(crate) fn seed_keyword(name: &str, explicit: &str) -> String {
    let base = default_keyword(name);
    match (base.is_empty(), explicit.is_empty()) {
        (true, _) => explicit.to_string(),
        (false, true) => base,
        (false, false) => format!("{base} {explicit}"),
    }
}

/// 命令类出厂种子：`path` 是启动用的命令名，`exe` 是 `System32` 下的图标源。
pub(crate) fn command_seed(
    name: &str,
    path: &str,
    args: &str,
    exe: &str,
    explicit: &str,
) -> LaunchItem {
    LaunchItem {
        name: name.to_string(),
        path: path.to_string(),
        arguments: args.to_string(),
        working_dir: None,
        source_path: system32(exe),
        kind: ItemKind::Command,
        keyword: seed_keyword(name, explicit),
        ..Default::default()
    }
}

pub(crate) fn folder_seed(name: &str, path: &str, explicit: &str) -> LaunchItem {
    LaunchItem {
        name: name.to_string(),
        path: path.to_string(),
        arguments: String::new(),
        working_dir: None,
        source_path: String::new(),
        kind: ItemKind::Folder,
        keyword: seed_keyword(name, explicit),
        ..Default::default()
    }
}

pub(crate) fn url_seed(name: &str, path: &str, explicit: &str) -> LaunchItem {
    LaunchItem {
        name: name.to_string(),
        path: path.to_string(),
        arguments: String::new(),
        working_dir: None,
        source_path: String::new(),
        kind: ItemKind::Url,
        keyword: seed_keyword(name, explicit),
        ..Default::default()
    }
}

/// 批次 G1 出厂分类的白名单：「基本」档保留的名字，其余出厂条目全部归「维护」。
///
/// 白名单按**名称**挑（103 条出厂种子名称互不重复，`builtin_seeds_tags_exact` 单测锁）。
/// 出厂条目不挂标签会掉进「恒显」池，「缺省分类范围 = 基本」就筛不掉它们；
/// 用户自己加的条目（批量导入的 .lnk 等）不带标签 ⇒ 任何档都显示。
/// 名单只是数据：增删一条不用动任何逻辑，改完对照 [`builtin_seeds`] 更新即可。
pub const FACTORY_TAG_BASIC: &[&str] = &[
    // 底座 7：Delphi 版一直就在的那组里最常用的
    "计算器",
    "记事本",
    "命令提示符",
    "任务管理器",
    "控制面板",
    "此电脑",
    "Windows 设置",
    // 自身 2
    "启动器设置",
    "退出启动器",
    // 电源与会话 5
    "锁定屏幕",
    "睡眠",
    "关机",
    "重启",
    "关闭显示器",
    // 带参项 1
    "资源管理器",
    // 设置页 5
    "Win·显示",
    "Win·声音",
    "Win·蓝牙",
    "Win·WLAN",
    "Win·默认应用",
    // 音量与媒体 4
    "静音",
    "增加音量",
    "降低音量",
    "播放/暂停",
];

/// 出厂内置命令集（§4.2.1，对齐 AHK 版 `AppSettings._DefaultCommands()`）。
/// 只在清单文件首次新建时整批写入；此后它们就是普通条目——可改名、可改路径、可删除。
///
/// 名字用中文（与这批种子原有的风格一致），AHK 的关键字作为别名并用：
/// 同一条命令另起一个英文名会和现有条目撞 uid，被 [`dedup_items`] 去重掉。
/// `path` 用简洁命令名（启动），`source_path` 用 exe 全路径（取图标）；无固定 exe 的给空。
pub fn builtin_seeds() -> Vec<LaunchItem> {
    let mut seeds = vec![
        // ---- Delphi 版底座（一直就在）----
        command_seed("计算器", "calc.exe", "", "calc.exe", ""),
        // §4.2.1 的 Notepad 关键字是 `np`：不另起一条「Notepad」，同 path 会撞 uid 被去重。
        command_seed("记事本", "notepad.exe", "", "notepad.exe", "np"),
        command_seed("画图", "mspaint.exe", "", "mspaint.exe", ""),
        command_seed("命令提示符", "cmd.exe", "", "cmd.exe", ""),
        command_seed(
            "PowerShell",
            "powershell.exe",
            "",
            "WindowsPowerShell/v1.0/powershell.exe",
            "",
        ),
        command_seed("任务管理器", "taskmgr.exe", "", "Taskmgr.exe", ""),
        command_seed("控制面板", "control.exe", "", "control.exe", ""),
        folder_seed("此电脑", "shell:MyComputerFolder", ""),
        folder_seed("回收站", "shell:RecycleBinFolder", ""),
        // 2026-10-23 从「设置」改名：与「启动器设置」成对，敲「设置」时两条入口一眼可分。
        url_seed("Windows 设置", "ms-settings:", ""),
        // ---- §4.2.1 出厂命令集（2026-10-21 补齐；此前 12 条只落了 Notepad/Paint 两条）----
        // Desktop 在参考项目里与 Notepad 共用 `np`，按 4.2.1 的去重规则不保留。
        // hosts 存的是非提权版：能打开、保存会被系统拒绝（提权要 `*runas`，见 FR-ACT-04）。
        command_seed(
            "编辑 hosts 文件",
            "notepad.exe",
            r"%windir%\System32\drivers\etc\hosts",
            "notepad.exe",
            "hosts",
        ),
        folder_seed("下载", "shell:Downloads", "dl"),
        // 空词回车先补成 `docs `，再敲子目录名；`{query}` 由 custom::build_hit 填进路径。
        folder_seed("文档", r"%USERPROFILE%\Documents\{query}", "docs"),
        // 占位符而不是绝对路径：程序搬文件夹后这条自己跟着走，不用重新播种。
        folder_seed(
            "启动器目录",
            crate::source::custom::LAUNCHER_DIR_PLACEHOLDER,
            "",
        ),
        command_seed("IP 配置", "cmd.exe", "/k ipconfig", "cmd.exe", "ipconfig"),
        command_seed("Ping", "cmd.exe", "/k ping {query}", "cmd.exe", "ping"),
        // 网址尚未确定 ⇒ 用 RFC 2606 保留的 `.invalid` 当占位，改这一条的路径即可。
        url_seed(
            "项目主页（待填网址）",
            "https://example.invalid/altrun",
            "altrun",
        ),
        url_seed(
            "谷歌地图",
            "https://www.google.com/maps/search/?api=1&query={query}",
            "maps",
        ),
        url_seed("系统更新", "ms-settings:windowsupdate", "wu"),
    ];
    // 批次 A2（FR-SRC-17）：系统命令五组并进出厂条目。
    // 剪贴板文本转换组按用户裁定整批暂缓——它要读写剪贴板，与"一条会启动程序的清单条目"
    // 语义不同，留给单独一批（发布说明第三十一节登记待办）。
    seeds.extend(crate::source::system::system_seeds());
    // 批次 G1：出厂条目全部挂分类——白名单「基本」，其余「维护」。
    let basic: std::collections::HashSet<&str> = FACTORY_TAG_BASIC.iter().copied().collect();
    for s in &mut seeds {
        s.tags = vec![if basic.contains(s.name.as_str()) {
            "基本"
        } else {
            "维护"
        }
        .to_string()];
    }
    seeds
}

/// 批次 G1 一次性回填：老清单里按 uid 认得出厂条目的，把出厂标签补上（**标签为空才补**）。
///
/// 只在 seed_round 进位那一轮调用（main.rs 与 [`merge_missing_seeds`] 同一个 if 里）——
/// 之后用户清掉标签不会被下一次启动补回来。用户条目（批量导入的 .lnk 等）uid 对不上
/// 任何出厂种子 ⇒ 不受影响、不带标签、任何档都显示。返回补了几条，幂等（第二轮恒 0）。
pub fn apply_factory_tags(items: &mut [LaunchItem]) -> usize {
    let tags_by_uid: std::collections::HashMap<String, Vec<String>> = builtin_seeds()
        .into_iter()
        .filter(|s| !s.tags.is_empty())
        .map(|s| (s.uid(), s.tags))
        .collect();
    let mut changed = 0;
    for it in items.iter_mut() {
        if it.tags.is_empty() {
            if let Some(tags) = tags_by_uid.get(&it.uid()) {
                it.tags = tags.clone();
                changed += 1;
            }
        }
    }
    changed
}

/// 批次 G1：清单里所有分类名 → 条数（按名排序）。不带标签的条目不出现。
/// 供 `@` 空查询的提示行用；将来条目管理界面按组陈列也用它。
pub fn all_tags(items: &[LaunchItem]) -> std::collections::BTreeMap<String, usize> {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for it in items {
        for t in &it.tags {
            let t = t.trim();
            if !t.is_empty() {
                *counts.entry(t.to_string()).or_insert(0) += 1;
            }
        }
    }
    counts
}

/// 批次 G1：把弹窗「分类」框的自由文本拆成标签表。逗号（含中文逗号）或空白分隔，
/// 去空白、去空项、保序去重。`开发, 维护 工具` ⇒ [开发, 维护, 工具]。
pub fn parse_tags_text(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in s.split(|c: char| c == ',' || c == '，' || c.is_whitespace()) {
        let t = part.trim();
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// 批次 G1：`@分类` 前缀的解析结果。`query` 是剥掉标签后剩下的真实查询串
/// （没有 `@` 前缀时就是原串本身），`tags` 是显式点名的分类（去重、保序）。
pub struct ScopeQuery<'a> {
    pub query: &'a str,
    pub tags: Vec<String>,
}

/// 批次 G1：解析 `@分类` 前缀（`@` 在 char 0 才激活，与 `?`/`>` 的判定同格）。
///
/// - `@维护 磁盘` ⇒ tags=[维护]、query=`磁盘`；
/// - `@维护 @开发 磁盘` ⇒ 两个分类**并集**，query=`磁盘`；
/// - 裸 `@` ⇒ tags 空、query 空（调用方据此放分类提示行）；
/// - 只吃**开头的连续 `@词`**：一旦遇到普通词，后面的 `@词` 全是查询文本
///   （`邮件 我@例子.com` 这类输入不受影响）。
///
/// 分类名是精确比对（含大小写）：名字来自同一条添加/编辑通道，所见即所敲。
/// `@维护` 只写字根、不加空格与全写 `@ 维护` 等价。
pub fn parse_scope_query(q: &str) -> ScopeQuery<'_> {
    if !q.starts_with('@') {
        return ScopeQuery {
            query: q,
            tags: Vec::new(),
        };
    }
    let bytes = q.as_bytes();
    let mut i = 0usize;
    let mut tags: Vec<String> = Vec::new();
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let word = &q[start..i];
        match word.strip_prefix('@') {
            Some(name) if !name.is_empty() => {
                if !tags.iter().any(|t| t == name) {
                    tags.push(name.to_string());
                }
            }
            // 单独的 `@`（名字为空）照吃不记：`@ 磁盘` ≡ `磁盘`，`@` 本身 ⇒ 空查询 + 提示行。
            Some(_) => {}
            _ => {
                i = start;
                break;
            }
        }
    }
    ScopeQuery {
        query: &q[i..],
        tags,
    }
}

/// 批次 G1：这条目落不落进本轮候选池（只筛清单行，头段行不受影响）。
///
/// - 显式 `@分类`：**严格成员**——条目要带着点名分类里的至少一个（并集）；
///   无标签的条目不入选（它不属于任何分类）；
/// - 没有显式分类：按缺省范围——`All` 全放行；`Basic` 放行无标签 ∪ 「基本」。
pub fn item_in_scope(it: &LaunchItem, explicit: &[String], scope: Scope) -> bool {
    if !explicit.is_empty() {
        return it.tags.iter().any(|t| explicit.iter().any(|e| e == t));
    }
    match scope {
        Scope::All => true,
        Scope::Basic => it.tags.is_empty() || it.tags.iter().any(|t| t == "基本"),
    }
}

/// 裸 `@` 提示行的载荷（批次 G1）：列出清单里的全部分类与条数，教人怎么用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeTagHint {
    /// 「分类：基本 24 · 维护 79」。
    pub title: String,
    /// 用法一句话（副标题与点击 Toast 共用）。
    pub subtitle: String,
}

impl ScopeTagHint {
    pub fn from_counts(counts: &std::collections::BTreeMap<String, usize>) -> Self {
        let title = if counts.is_empty() {
            "还没有分类".to_string()
        } else {
            let list: Vec<String> = counts.iter().map(|(k, v)| format!("{k} {v}")).collect();
            format!("分类：{}", list.join(" · "))
        };
        Self {
            title,
            subtitle: "敲 @分类 只看这一类（@维护 @开发 并集），@ 是分类总览".to_string(),
        }
    }
}

/// 把清单里缺的出厂条目**追加到末尾**，返回补了几条。
///
/// 「已存在」认两个键：uid（路径+参数没动过）**或**名称（改过路径/参数的那条也算在）。
/// 这样既不会把用户改过的条目复制一份，删掉之后又能用设置页的动作找回来。
/// 只加不删、不改已有条目：同关键字撞车时按清单先后取第一条（TC-SRC-05-3），
/// 所以用户自己手加的那条（排在前面）继续生效。
pub fn merge_missing_seeds(items: &mut Vec<LaunchItem>) -> usize {
    let uids: std::collections::HashSet<String> = items.iter().map(|i| i.uid()).collect();
    let names: std::collections::HashSet<String> =
        items.iter().map(|i| i.name.trim().to_lowercase()).collect();
    let missing: Vec<LaunchItem> = builtin_seeds()
        .into_iter()
        .filter(|s| !uids.contains(&s.uid()) && !names.contains(&s.name.trim().to_lowercase()))
        .collect();
    let added = missing.len();
    items.extend(missing);
    added
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

/// FR-SRC-17 的 `hidden[]`：按派生 uid 把用户隐藏的条目从清单里剔掉，返回剔了几条。
/// 只认 uid（`uid()` 本身就小写）——同名但路径不同的两条不会互相牵连。
/// 调用时机四处：启动 / 重新载入配置之后、「补回缺失的出厂命令」之后、批量导入合并之前
/// （否则补回来或重新扫描到的条目会把隐藏项又塞回清单）。
pub fn drop_hidden(items: &mut Vec<LaunchItem>, hidden: &[String]) -> usize {
    if hidden.is_empty() {
        return 0;
    }
    let set: std::collections::HashSet<&str> = hidden
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let before = items.len();
    items.retain(|i| !set.contains(i.uid().as_str()));
    before - items.len()
}

/// 批次 F7（FR-CFG-07）：清空学习排序数据——只动 `use_count` / `last_used_ms`，
/// 置顶（`pinned`）与其余字段原样保留。返回统计真的变过的条目数。
///
/// 已学到的别名**清不掉**：`learn_alias` 是往 `keyword` 里追加，与用户手填的别名
/// 事后不可区分（FR-ACT-10）。这是需求书 FR-CFG-07 的已知偏差，不是漏做。
pub fn reset_learning(items: &mut [LaunchItem]) -> usize {
    let mut touched = 0;
    for it in items.iter_mut() {
        if it.use_count != 0 || it.last_used_ms != 0 {
            touched += 1;
        }
        it.use_count = 0;
        it.last_used_ms = 0;
    }
    touched
}

/// 批次 F7（FR-CFG-08）：恢复默认**只重置偏好里的口径类字段**，用户资产一概不动。
///
/// 保留：`toggle_hotkey` / `typeback_hotkey` / `secondary_hotkey`（热键三格——按哪颗键是用户的
/// 手感，不该被"恢复默认"改掉）、`web_engines`（自定义引擎）、
/// `hidden` 与 `hidden_stash`（被隐藏的条目及其快照）、`seed_round`（已补种到第几轮——重置它会让下一轮启动
/// 把用户删掉的出厂条目整批复活）。`user_items.json` 里的自定义命令、置顶、学习统计、
/// 分类标签也不经过这里。
pub fn restore_defaults(s: &mut AppSettings) {
    s.space_to_run = SpaceRun::default();
    s.tab_mode = TabMode::default();
    s.learn_rank = LearnRank::default();
    s.sources_off.clear();
    s.confirm_actions = true;
    s.terminal_shell = default_terminal_shell();
    s.theme = default_theme();
    s.list_rows = ListRows::default();
    s.row_density = RowDensity::default();
    s.default_scope = Scope::default();
    // 批次 E3-丙：次级动作回到「快捷键版」。键位属于热键三格，保留。
    s.secondary_action = SecondaryAction::default();
    // 批次 C3：历史上限回到需求书 4.2 的 30。**只改这一格数字，不动已经存下来的历史**
    // （清内容走高级页那个「清空搜索历史」，两件事分开）。
    s.history_size = HISTORY_SIZE_DEFAULT;
    // 批次 F8：宽度 / 记住位置 / 状态栏 / 淡入也回厂，位置一起清掉（下次启动居中）。
    s.appearance = Appearance::default();
}

/// 破坏性操作前的编号备份（FR-CFG-08「Restore Defaults 前写 `.bak`」）。
/// 名字形如 `settings.json.bak-20261024-153012`；同一秒内再备份就追加 `-1`、`-2`，
/// **绝不覆盖已有备份**。源文件不在 ⇒ 返回 `None`（不凭空造一份空备份）。
pub fn backup_file(path: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let stamp = local_now_compact();
    for n in 0..100 {
        let candidate = if n == 0 {
            format!("{name}.bak-{stamp}")
        } else {
            format!("{name}.bak-{stamp}-{n}")
        };
        let target = path.with_file_name(candidate);
        if target.exists() {
            continue;
        }
        write_atomic(&target, &bytes, "备份");
        if target.exists() {
            return Some(target);
        }
    }
    eprintln!("source: 备份名额用尽，未写成 {path:?}");
    None
}

/// 设置页「高级 → 隐藏的条目」一行。`name` 为 `None` ＝ 既没有快照、也对不上出厂种子
/// （目前唯一来源是启动时自动去重写进去的 Store 版「设置」），只显示原文、不给恢复按钮。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenRow {
    pub uid: String,
    pub name: Option<String>,
}

/// 记一份隐藏快照（批次 D6）。同一个 uid 再藏一次就覆盖旧快照，不堆重复行。
pub fn stash_hidden(stash: &mut Vec<LaunchItem>, item: &LaunchItem) {
    let uid = item.uid();
    stash.retain(|i| i.uid() != uid);
    stash.push(item.clone());
}

/// 把 `hidden[]` 里的 uid 逐条翻译成可展示的行（顺序照配置原样）。
/// 名称有两个来源：先看快照（扫描项只有这一条路），再看出厂种子
/// （F7 时代没有快照的老记录仍能按种子回显名称）。
pub fn hidden_rows(hidden: &[String], stash: &[LaunchItem]) -> Vec<HiddenRow> {
    let seeds: std::collections::HashMap<String, String> = builtin_seeds()
        .into_iter()
        .map(|s| (s.uid(), s.name))
        .collect();
    let stashed: std::collections::HashMap<String, String> = stash
        .iter()
        .map(|i| (i.uid(), i.name.clone()))
        .collect();
    hidden
        .iter()
        .map(|u| u.trim())
        .filter(|u| !u.is_empty())
        .map(|u| HiddenRow {
            uid: u.to_string(),
            name: stashed.get(u).or_else(|| seeds.get(u)).cloned(),
        })
        .collect()
}

/// 取回一条隐藏条目的原始内容：先查快照（扫描项唯一来源），没有再按出厂种子重建。
/// 快照取到即从表里移除（`hidden[]` 的增删由调用方负责）。
fn hidden_payload(stash: &mut Vec<LaunchItem>, uid: &str) -> Option<LaunchItem> {
    if let Some(pos) = stash.iter().position(|i| i.uid() == uid) {
        return Some(stash.remove(pos));
    }
    builtin_seeds().into_iter().find(|s| s.uid() == uid)
}

/// 逐条恢复：从 `hidden[]` 撤掉这个 uid，并把条目补回清单末尾。
/// 优先用快照——置顶 / 学习计数 / 别名原样回来；没有快照再按出厂种子重建。
/// 只补这一条，不碰其它缺项（与设置页「补回缺失的出厂命令」的整批语义区分开）。
/// 既没有快照也对不上种子的孤儿 uid 不在这里动，返回 `false`。
pub fn restore_hidden(
    items: &mut Vec<LaunchItem>,
    hidden: &mut Vec<String>,
    stash: &mut Vec<LaunchItem>,
    uid: &str,
) -> bool {
    let Some(pos) = hidden.iter().position(|h| h == uid) else {
        return false;
    };
    let Some(item) = hidden_payload(stash, uid) else {
        return false;
    };
    hidden.remove(pos);
    if !items.iter().any(|i| i.uid() == uid) {
        items.push(item);
    }
    true
}

/// 全部恢复：有快照或有出厂种子的都撤掉并补回；孤儿 uid 原样留在 `hidden[]`。
/// 返回恢复了多少条。
pub fn restore_all_hidden(
    items: &mut Vec<LaunchItem>,
    hidden: &mut Vec<String>,
    stash: &mut Vec<LaunchItem>,
) -> usize {
    let mut restored = 0;
    let mut keep: Vec<String> = Vec::with_capacity(hidden.len());
    for uid in hidden.drain(..) {
        match hidden_payload(stash, &uid) {
            Some(item) => {
                if !items.iter().any(|i| i.uid() == uid) {
                    items.push(item);
                }
                restored += 1;
            }
            None => keep.push(uid),
        }
    }
    *hidden = keep;
    restored
}

/// 显式添加（添加弹窗 / SendTo）撞上 `hidden[]` 时以用户这次动作为准：撤掉 uid 与它的快照。
/// 否则界面说「已添加」，下一次启动又被 [`drop_hidden`] 静默剔掉。返回撤销了几条。
pub fn unhide_uids(hidden: &mut Vec<String>, stash: &mut Vec<LaunchItem>, uids: &[String]) -> usize {
    let mut cleared = 0;
    for uid in uids {
        if let Some(pos) = hidden.iter().position(|h| h == uid) {
            hidden.remove(pos);
            stash.retain(|i| i.uid() != *uid);
            cleared += 1;
        }
    }
    cleared
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
        let head_alpha = scheme
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false);
        if scheme.len() >= 2
            && head_alpha
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            return true; // ms-settings:、mailto: 等
        }
    }
    false
}

/// 把任意输入判成条目类型：URL/协议→`Url`，其余（命令名 / 路径 / .lnk）→`Command`。
pub fn infer_kind(input: &str) -> ItemKind {
    if is_url(input) {
        ItemKind::Url
    } else {
        ItemKind::Command
    }
}

/// FR-SRC-05：条目的「目标名」匹配键——文件取去扩展名的文件名、文件夹取路径末段
/// （尾部分隔符先剥掉）；App / Command 取目标的 exe 名（`notepad.exe` ⇒ `notepad`，
/// 2026-10-23 真机 A7）。URL、`shell:` 虚拟文件夹、含 `{query}` 的路径没有稳定目标名，
/// 一律返回 `None`（匹配层就不追加这半条键）。
pub fn target_stem(item: &LaunchItem) -> Option<String> {
    let path = item.path.trim();
    if path.is_empty()
        || is_url(path)
        || path.to_ascii_lowercase().starts_with("shell:")
        || path.contains("{query")
    {
        return None;
    }
    let stem = match item.kind {
        ItemKind::File => Path::new(path).file_stem(),
        ItemKind::Folder => Path::new(path.trim_end_matches(['\\', '/'])).file_name(),
        // `internal:<id>` 不是磁盘路径，按 Windows 路径解析只会得到一个假文件名。
        ItemKind::Url | ItemKind::Internal => None,
        _ => Path::new(path).file_stem(),
    };
    stem.and_then(|s| s.to_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
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
/// 都吃到 30 档噪声。这里要求 ≥2 字符、不等于名称、尚未出现在别名里、且不含通配符
/// `*` / `?`（FR-QRY-09：那是查询语法，不是触发词）；只攒查询**首词**
/// （FR-QRY-01 的 keyword 那半，后面的参数词不是触发码）。返回是否改变 ⇒ 调用方据此决定写盘。
pub fn learn_alias(item: &mut LaunchItem, word: &str) -> bool {
    let w = word.trim().to_lowercase();
    if w.chars().count() < 2 || w == item.name.to_lowercase() {
        return false;
    }
    // FR-QRY-09：`*` / `?` 是通配符查询的语法，不是触发词——学进别名表只会留下
    // 一条普通查询永远匹配不上、却仍参与 Tab 补全与去重的垃圾。
    if w.contains('*') || w.contains('?') {
        return false;
    }
    // 这个词本来就在名称里（`设置`→「Windows 设置」、`task`→「Task Scheduler」）⇒ 学下来
    // 一个字符都不会多认，只会往「关键字重复」表里塞一行共用词（2026-10-23 用户反馈
    // 「敲设置出来四十行」的第三个根因）。空格执行也早有同一口径：只是前缀时空格当文本。
    if item.name.to_lowercase().contains(&w) {
        return false;
    }
    if item
        .keyword
        .split_whitespace()
        .any(|a| a.to_lowercase() == w)
    {
        return false;
    }
    item.keyword = if item.keyword.trim().is_empty() {
        w
    } else {
        format!("{} {w}", item.keyword.trim())
    };
    true
}

/// 把当前输入框内容生成一条可保存条目（FR-SRC-23「添加为快捷项」）。
/// `name` 取末段并去掉 `.lnk` 后缀；启动一律走 `ShellExecuteExW`，故 `path` 原样存输入。
pub fn make_item_from_input(input: &str) -> LaunchItem {
    let trimmed = input.trim();
    let tail = trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed);
    let name = tail.trim_end_matches(".lnk");
    let name = if name.is_empty() {
        trimmed.to_string()
    } else {
        name.to_string()
    };
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
pub fn spawn_batch_import(tx: mpsc::Sender<Vec<LaunchItem>>) {
    thread::spawn(move || {
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
        let mut skipped_unresolved = 0usize;
        for file in &files {
            if let Some(app) = unsafe { resolve_lnk(file) } {
                // 自排除：target 规范化后就是本 exe（含我们自己写的启动夹快捷方式）。
                if exe_key
                    .as_deref()
                    .is_some_and(|key| is_self_target(&app.path, key))
                {
                    skipped_self += 1;
                    eprintln!(
                        "source: 批量导入自排除（target 即本 exe）：{}",
                        file.display()
                    );
                    continue;
                }
                apps.push(app);
            } else {
                // 解析失败 / 空 target：跳过不中断（约束 4）。空 target 的典型是
                // File Explorer / 控制面板 / 输入法这类 shell 快捷方式与损坏件——
                // Store 应用不在这里，它们走下面的 AppsFolder 枚举。
                skipped_unresolved += 1;
            }
        }
        let lnk_count = apps.len();
        // 批次 A9：UWP/MSIX 应用在文件系统里没有 .lnk，只能从 shell 命名空间枚举。
        let store = unsafe { scan_store_apps() };
        let store_count = store.len();
        apps.extend(store);
        eprintln!(
            "source: 批量扫描 {} 个 .lnk，解析出 {} 条、未解析 {} 条（空 target／损坏），自排除 {} 条；\
             另从 shell:AppsFolder 收 UWP/MSIX {} 条（线程 {:?}，CoInit={hr:?}）",
            total,
            lnk_count,
            skipped_unresolved,
            skipped_self,
            store_count,
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
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
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
    Some(
        std::env::current_exe()
            .ok()?
            .canonicalize()
            .ok()?
            .to_string_lossy()
            .into_owned(),
    )
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
            .GetPath(
                &mut buf,
                core::ptr::null_mut::<WIN32_FIND_DATAW>(),
                SLGP_UNCPRIORITY.0 as u32,
            )
            .map(|_| from_wide(&buf))
            .unwrap_or_default();
        if target.trim().is_empty() {
            // Store/MSIX 或损坏的快捷方式：本函数丢弃（约束 3）；Store 应用由
            // `scan_store_apps` 从 shell 命名空间补齐（批次 A9）。
            return None;
        }
        let arguments = link
            .GetArguments(&mut buf)
            .map(|_| from_wide(&buf))
            .unwrap_or_default();
        let working_dir_raw = link
            .GetWorkingDirectory(&mut buf)
            .map(|_| from_wide(&buf))
            .unwrap_or_default();

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let source_path = path.to_string_lossy().into_owned();
        let expanded_target = expand_vars(&target);
        let keyword = default_keyword(&name);

        Some(LaunchItem {
            name,
            path: expanded_target.clone(),
            arguments,
            working_dir: {
                let wd = expand_vars(&working_dir_raw);
                if wd.trim().is_empty() { None } else { Some(wd) }
            },
            // .lnk 自身作图标源（shell 解析快捷方式自定义图标），启动也走 .lnk 更保真。
            source_path: source_path.clone(),
            kind: ItemKind::App,
            keyword,
            ..Default::default()
        })
    }
}

// ---- 批次 A9：MSIX/UWP（Store）应用 ----
//
// 这些应用在文件系统里**没有** `.lnk`：开始菜单项由 AppX 包注册生成，`resolve_lnk`
// 对它们只能拿到空 target（约束 3）。唯一稳定的枚举入口是 `shell:AppsFolder` 命名空间，
// 启动入口是 `IApplicationActivationManager`（AUMID 是它的官方入参）。
// 条目路径存 `shell:AppsFolder\<AUMID>`，由 `main.rs::shell_execute` 认前缀分派。

/// 从条目路径里取出 AUMID：`shell:AppsFolder\Microsoft.WindowsCalculator_…!App`
/// ⇒ `Microsoft.WindowsCalculator_…!App`。前缀大小写不敏感（手填可能写成小写）。
pub fn uwp_aumid(path: &str) -> Option<&str> {
    const PREFIX: &str = "shell:AppsFolder\\";
    let p = path.trim();
    if !p.get(..PREFIX.len())?.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let aumid = p[PREFIX.len()..].trim();
    is_uwp_aumid(aumid).then_some(aumid)
}

/// AUMID 形态：`包族名!应用ID`。只认"含 `!`、不含路径分隔符与冒号"——
/// 同一个枚举里 Win32 项返回的是 exe 绝对路径，两者不会重叠。
fn is_uwp_aumid(s: &str) -> bool {
    !s.is_empty() && s.contains('!') && !s.contains(['\\', '/', ':'])
}

/// 把 `GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING)` 的结果归一成 AUMID；非 UWP 项返回 None。
/// 实测/文档两种形态都收：裸 AUMID，或带 `shell:AppsFolder\` 前缀的同串。
fn aumid_from_parsing_name(raw: &str) -> Option<String> {
    let s = raw.trim();
    let s = uwp_aumid(s).unwrap_or(s);
    is_uwp_aumid(s).then(|| s.to_string())
}

/// 启动 MSIX/UWP 应用。`ShellExecuteExW` 对裸 AUMID 没有解析保证，故走官方入口。
///
/// COM 公寓：调用点在 UI 线程，而本项目此前**从未在主线程常驻 `CoInitializeEx`**
/// （只有后台扫描线程与 SendTo 预填那一下）。这里补一次 STA；已初始化时返回 S_FALSE、
/// 被别的组件设成其他公寓时返回 RPC_E_CHANGED_MODE，两种情况都不影响 in-proc 的
/// `CoCreateInstance`。**因此不配对 `CoUninitialize`**——计数里可能有别人的那一份。
pub fn activate_uwp(aumid: &str) -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_INPROC_SERVER)?;
        let id = wide_null(aumid);
        let args = wide_null("");
        manager.ActivateApplication(PCWSTR(id.as_ptr()), PCWSTR(args.as_ptr()), AO_NONE)?;
    }
    Ok(())
}

/// 枚举 `shell:AppsFolder`，只收 MSIX/UWP 条目（Win32 项仍归 `.lnk` 扫描，不在这里重复收）。
/// 必须在已 `CoInitializeEx` 的线程调用（`spawn_batch_import` 的扫描线程满足）。
unsafe fn scan_store_apps() -> Vec<LaunchItem> {
    unsafe {
        let spec = wide_null("shell:AppsFolder");
        let pbc: Option<&IBindCtx> = None;
        let folder: IShellItem = match SHCreateItemFromParsingName(PCWSTR(spec.as_ptr()), pbc) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("source: shell:AppsFolder 打不开（{e}），本次不收 Store 应用");
                return Vec::new();
            }
        };
        let list: IEnumShellItems = match folder.BindToHandler(pbc, &BHID_EnumItems) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("source: 枚举 shell:AppsFolder 失败（{e}）");
                return Vec::new();
            }
        };
        let mut out: Vec<LaunchItem> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut win32 = 0usize;
        loop {
            let mut one = [None::<IShellItem>];
            let mut fetched = 0u32;
            if list.Next(&mut one, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = one[0].take() else { break };
            let aumid = match item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING) {
                Ok(p) => aumid_from_parsing_name(&pwstr_take(p)),
                Err(_) => None,
            };
            let Some(aumid) = aumid else {
                win32 += 1;
                continue;
            };
            if !seen.insert(aumid.to_lowercase()) {
                continue;
            }
            let display = match item.GetDisplayName(SIGDN_NORMALDISPLAY) {
                Ok(p) => pwstr_take(p),
                Err(_) => String::new(),
            };
            let name = if display.trim().is_empty() {
                aumid.clone()
            } else {
                display
            };
            let path = format!("shell:AppsFolder\\{aumid}");
            out.push(LaunchItem {
                name: name.clone(),
                path: path.clone(),
                // 图标源留空：`SHGetFileInfoW` 对 `shell:` 路径返回失败（本批实测
                // Settings / Store 两条均为 None）⇒ 列表里退化成首字母图标，
                // 不因图标缺失丢条目（约束 2）。真图标要走 IShellItemImageFactory，留待后续。
                source_path: String::new(),
                kind: ItemKind::App,
                keyword: default_keyword(&name),
                ..Default::default()
            });
        }
        eprintln!(
            "source: shell:AppsFolder 枚举到 UWP/MSIX {} 条（跳过 Win32 项 {} 条）",
            out.len(),
            win32
        );
        out
    }
}

/// 读走 COM 分配的宽字符串并释放——`IShellItem::GetDisplayName` 的内存归调用方。
unsafe fn pwstr_take(p: PWSTR) -> String {
    let s = unsafe { p.to_string().unwrap_or_default() };
    unsafe { CoTaskMemFree(Some(p.as_ptr() as *const core::ffi::c_void)) };
    s
}

// ---- FR-SRC-20 ① SendTo 入口安装/卸载（设置页开关的底层动作）----

const SENDTO_LINK_NAME: &str = "egui-launcher.lnk";

fn sendto_link_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        Path::new(&appdata)
            .join(r"Microsoft\Windows\SendTo")
            .join(SENDTO_LINK_NAME),
    )
}

/// 设置页开关的当前态：文件存在且归属校验通过才算「已安装」。
/// 同名文件若是别人建的（target 不是本 exe）→ 报未安装，且后续删除会拒绝碰它。
/// 旧版本程序写的 Arguments=%1 形态也算自己的（历史版本产物），判据 target==本 exe 且 args ∈ {"", "%1"}。
pub fn sendto_installed() -> bool {
    let Some(link) = sendto_link_path() else {
        return false;
    };
    if !link.exists() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
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
                link.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ));
        }
        std::fs::remove_file(&link).map_err(|e| format!("删除失败: {e}"))?;
        let witem = wide_null(&link.to_string_lossy());
        let wdir = wide_null(&dir.to_string_lossy());
        unsafe {
            SHChangeNotify(
                SHCNE_UPDATEITEM,
                SHCNF_PATHW,
                Some(witem.as_ptr().cast()),
                None,
            );
            SHChangeNotify(
                SHCNE_UPDATEDIR,
                SHCNF_PATHW,
                Some(wdir.as_ptr().cast()),
                None,
            );
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
            SHChangeNotify(
                SHCNE_UPDATEITEM,
                SHCNF_PATHW,
                Some(witem.as_ptr().cast()),
                None,
            );
            SHChangeNotify(
                SHCNE_UPDATEDIR,
                SHCNF_PATHW,
                Some(wdir.as_ptr().cast()),
                None,
            );
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
                    return Ok(format!(
                        "SendTo 入口已存在且未变化，未重写: {}",
                        link_path.display()
                    ));
                }
            }
        }
        let shell: IShellLinkW = CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("CoCreateInstance(ShellLink) 失败: {e}"))?;
        let wexe = wide_null(exe);
        shell
            .SetPath(PCWSTR(wexe.as_ptr()))
            .map_err(|e| format!("SetPath: {e}"))?;
        shell
            .SetIconLocation(PCWSTR(wexe.as_ptr()), 0)
            .map_err(|e| format!("SetIconLocation: {e}"))?;
        let wdesc = wide_null("添加到 egui-launcher 快捷项");
        shell
            .SetDescription(PCWSTR(wdesc.as_ptr()))
            .map_err(|e| format!("SetDescription: {e}"))?;
        let pf: IPersistFile = shell
            .cast()
            .map_err(|e| format!("cast IPersistFile: {e}"))?;
        let wlink = wide_null(&link_path.to_string_lossy());
        pf.Save(PCWSTR(wlink.as_ptr()), true)
            .map_err(|e| format!("Save: {e}"))?;
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
            .GetPath(
                &mut buf,
                std::ptr::null_mut::<WIN32_FIND_DATAW>(),
                SLGP_UNCPRIORITY.0 as u32,
            )
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
    let Some(link) = startup_link_path() else {
        return false;
    };
    if !link.exists() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
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
                link.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
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
        SHChangeNotify(
            SHCNE_UPDATEITEM,
            SHCNF_PATHW,
            Some(witem.as_ptr().cast()),
            None,
        );
        SHChangeNotify(
            SHCNE_UPDATEDIR,
            SHCNF_PATHW,
            Some(wdir.as_ptr().cast()),
            None,
        );
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
        shell
            .SetPath(PCWSTR(wexe.as_ptr()))
            .map_err(|e| format!("SetPath: {e}"))?;
        let wargs = wide_null(STARTUP_ARGS);
        shell
            .SetArguments(PCWSTR(wargs.as_ptr()))
            .map_err(|e| format!("SetArguments: {e}"))?;
        shell
            .SetIconLocation(PCWSTR(wexe.as_ptr()), 0)
            .map_err(|e| format!("SetIconLocation: {e}"))?;
        let wdesc = wide_null("登录时静默启动 egui-launcher（不弹窗口）");
        shell
            .SetDescription(PCWSTR(wdesc.as_ptr()))
            .map_err(|e| format!("SetDescription: {e}"))?;
        let pf: IPersistFile = shell
            .cast()
            .map_err(|e| format!("cast IPersistFile: {e}"))?;
        let wlink = wide_null(&link_path.to_string_lossy());
        pf.Save(PCWSTR(wlink.as_ptr()), true)
            .map_err(|e| format!("Save: {e}"))?;
        Ok(format!(
            "开机自启项已写入：{}（参数 {STARTUP_ARGS}，登录后托盘常驻、不弹窗口）",
            link_path.display()
        ))
    }
}

#[cfg(test)]
mod uwp_tests {
    use super::*;

    #[test]
    fn uwp_aumid_reads_own_path_prefix() {
        assert_eq!(
            uwp_aumid(r"shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
            Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App")
        );
        // 前缀大小写不敏感，尾部空白剥掉（手填/编辑器可能留空格）。
        assert_eq!(
            uwp_aumid(r"Shell:Appsfolder\Vendor.App_pfn!App  "),
            Some("Vendor.App_pfn!App")
        );
    }

    #[test]
    fn uwp_aumid_rejects_paths_that_are_not_aumid() {
        assert_eq!(uwp_aumid(r"C:\Windows\System32\notepad.exe"), None);
        assert_eq!(uwp_aumid("shell:AppsFolder"), None);
        assert_eq!(uwp_aumid(r"shell:AppsFolder\"), None);
        assert_eq!(uwp_aumid(""), None);
    }

    /// 枚举返回值有两种形态（裸 AUMID / 带前缀），且同一枚举里的 Win32 项是绝对路径——
    /// 后者必须落进"跳过"那一档，否则会与 `.lnk` 扫描重复收条目。
    #[test]
    fn aumid_from_parsing_name_accepts_both_shapes() {
        let want = Some("Microsoft.WindowsStore_8wekyb3d8bbwe!App");
        assert_eq!(
            aumid_from_parsing_name("Microsoft.WindowsStore_8wekyb3d8bbwe!App").as_deref(),
            want
        );
        assert_eq!(
            aumid_from_parsing_name(r"shell:AppsFolder\Microsoft.WindowsStore_8wekyb3d8bbwe!App")
                .as_deref(),
            want
        );
        assert_eq!(aumid_from_parsing_name(r"C:\Anki\anki.exe"), None);
        assert_eq!(aumid_from_parsing_name(""), None);
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
        let v = dedup_items(vec![
            with_args("cmd.exe", "/k ping"),
            with_args("cmd.exe", "/k ipconfig"),
        ]);
        assert_eq!(v.len(), 2, "实际只留下 {} 条", v.len());
    }

    /// 同一 path + 同一参数（含大小写差）仍是同一条，必须合并。
    #[test]
    fn same_path_same_arguments_dedups_case_insensitively() {
        let v = dedup_items(vec![
            with_args("CMD.exe", "/K PING"),
            with_args("cmd.exe", "/k ping "),
        ]);
        assert_eq!(v.len(), 1, "实际留下 {} 条", v.len());
    }

    /// §4.2.1 出厂集必须覆盖参考项目那 12 条里我们决定保留的部分。关键字是需求书点名的
    /// （`np`/`hosts`/`dl`/`docs`/`ipconfig`/`ping`/`altrun`/`maps`）——少一个就又回到
    /// 「用户得自己手加一条 ping」那天。
    #[test]
    fn builtin_seeds_cover_the_factory_set() {
        let seeds = builtin_seeds();
        let has = |kw: &str| {
            seeds
                .iter()
                .any(|s| s.keyword.split_whitespace().any(|w| w == kw))
        };
        for kw in [
            "np", "hosts", "dl", "docs", "ipconfig", "ping", "altrun", "maps",
        ] {
            assert!(has(kw), "出厂集里没有关键字 {kw}");
        }
        assert!(
            seeds.iter().any(|s| s.path == "ms-settings:windowsupdate"),
            "Windows Update 那条要能启动"
        );
        // 这两条是本批两处机制的依赖：`{query}` 模板与「启动器目录」占位符。
        assert!(seeds.iter().any(|s| s.arguments == "/k ping {query}"));
        assert!(
            seeds
                .iter()
                .any(|s| s.path == crate::source::custom::LAUNCHER_DIR_PLACEHOLDER)
        );
        // 同 uid 的两条会在 dedup_items 里被悄悄吃掉一条。
        let mut uids: Vec<String> = seeds.iter().map(|s| s.uid()).collect();
        let before = uids.len();
        uids.sort();
        uids.dedup();
        assert_eq!(uids.len(), before, "出厂集内部有重复 uid");
    }

    /// 补种只加缺的：删掉的补回来，改过路径/参数（名称没变）的不能复制第二份。
    #[test]
    fn merge_missing_seeds_only_adds_what_is_missing() {
        let mut items = builtin_seeds();
        items.retain(|i| i.name != "Ping");
        if let Some(it) = items.iter_mut().find(|i| i.name == "记事本") {
            it.path = r"C:\Windows\notepad.exe".to_string();
        }
        let before = items.len();
        assert_eq!(merge_missing_seeds(&mut items), 1, "只该补回被删掉的 Ping");
        assert_eq!(items.len(), before + 1);
        assert_eq!(
            items.iter().filter(|i| i.name == "记事本").count(),
            1,
            "改了路径的条目不复制一份"
        );
        assert_eq!(merge_missing_seeds(&mut items), 0, "补种必须幂等");
    }

    /// 批次 G1：出厂 103 条必须**条条挂标签**（不挂就掉进「恒显」池，基本档筛不掉），
    /// 标签只有「基本 / 维护」两值；白名单按名称挑 ⇒ 名称必须互不重复，
    /// 且白名单里每个名字都得真的挂在对应条目上（漏字=该条目静默掉进维护档）。
    #[test]
    fn builtin_seeds_tags_cover_every_factory_item() {
        let seeds = builtin_seeds();
        let mut names: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for s in &seeds {
            assert!(
                names.insert(s.name.as_str()),
                "出厂名称重复：{}（白名单按名挑，重名会整条串位）",
                s.name
            );
            assert_eq!(
                s.tags.len(),
                1,
                "出厂条目「{}」该恰好一个标签，实得 {:?}",
                s.name,
                s.tags
            );
            assert!(
                s.tags[0] == "基本" || s.tags[0] == "维护",
                "出厂条目「{}」标签必须是 基本/维护，实得 {:?}",
                s.name,
                s.tags
            );
        }
        let basic: std::collections::HashSet<&str> = FACTORY_TAG_BASIC.iter().copied().collect();
        assert_eq!(
            basic.len(),
            FACTORY_TAG_BASIC.len(),
            "白名单本身有重名"
        );
        let tagged_basic: Vec<&str> = seeds
            .iter()
            .filter(|s| s.tags[0] == "基本")
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(
            tagged_basic.len(),
            FACTORY_TAG_BASIC.len(),
            "「基本」实际 {} 条 ≠ 白名单 {} 条：{:?}",
            tagged_basic.len(),
            FACTORY_TAG_BASIC.len(),
            tagged_basic
        );
        for name in FACTORY_TAG_BASIC {
            assert!(
                tagged_basic.contains(name),
                "白名单里的「{name}」没挂上基本标签（可能名字改了）"
            );
        }
    }

    /// 批次 G1 回填三态：标签为空的出厂条目补上；已填（含用户改过）的不动；
    /// 对不上出厂 uid 的用户条目不动。幂等：补完再跑恒为 0。
    #[test]
    fn apply_factory_tags_fills_only_empty_tags_and_is_idempotent() {
        let mut items = builtin_seeds();
        for it in items.iter_mut() {
            it.tags.clear();
        }
        let total = items.len();
        assert_eq!(apply_factory_tags(&mut items), total, "空标签的出厂条目要整批补回");
        assert!(items.iter().all(|i| !i.tags.is_empty()), "补完不许再有空标签");

        // 用户改过的标签不受影响。
        let mut custom = builtin_seeds();
        for it in custom.iter_mut() {
            it.tags = vec!["开发".to_string()];
        }
        assert_eq!(apply_factory_tags(&mut custom), 0, "已填标签的一条都不许动");
        assert!(custom.iter().all(|i| i.tags == ["开发"]));

        // 用户条目（uid 对不上出厂种子）不补标签 ⇒ 恒显。
        let user = LaunchItem {
            name: "我的脚本".into(),
            path: r"D:\tools\go.bat".into(),
            kind: ItemKind::Command,
            ..Default::default()
        };
        let mut mixed = vec![user];
        mixed.extend(builtin_seeds().into_iter().map(|mut s| {
            s.tags.clear();
            s
        }));
        let changed = apply_factory_tags(&mut mixed);
        assert_eq!(changed, total, "只有出厂条目被补，用户条目不算");
        assert!(mixed[0].tags.is_empty(), "用户条目必须保持无标签");
        assert_eq!(apply_factory_tags(&mut mixed), 0, "回填必须幂等");
    }

    /// 批次 G1 分类计数：只数带标签的条目，按名排序；空标签/空白串不进表。
    #[test]
    fn all_tags_counts_tagged_items_only() {
        let mk = |tags: &[&str]| LaunchItem {
            name: "x".into(),
            path: "x".into(),
            kind: ItemKind::Command,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        };
        let items = vec![mk(&["维护"]), mk(&["基本", "维护"]), mk(&["基本"]), mk(&[])];
        let counts = all_tags(&items);
        assert_eq!(counts.len(), 2, "无标签条目不进表：{counts:?}");
        assert_eq!(counts.get("基本"), Some(&2));
        assert_eq!(counts.get("维护"), Some(&2));
        let keys: Vec<&String> = counts.keys().collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "输出按名排序");
    }

    /// 批次 G1 旧清单兼容：`tags` 是 `#[serde(default)]` 新增字段，
    /// G1 之前落盘的 user_items.json（没有 tags 键）必须照常解析成空标签。
    #[test]
    fn legacy_item_json_without_tags_parses_empty() {
        let json = r#"{
            "name": "记事本",
            "path": "notepad.exe",
            "arguments": "",
            "working_dir": null,
            "source_path": "notepad.exe",
            "kind": "command",
            "keyword": "jsb np"
        }"#;
        let it: LaunchItem = serde_json::from_str(json).expect("旧形状必须解析成功");
        assert!(it.tags.is_empty(), "旧清单无 tags 字段 ⇒ 空标签");
        assert_eq!(it.name, "记事本");
    }

    /// 批次 G1 前缀解析的三态与边界：单分类、多分类并集、裸 `@`、去重、
    /// 「@ 后接普通词」不算分类、标签词之后还有 `@词` 只是查询文本。
    #[test]
    fn scope_query_parses_tags_union_and_bare_at() {
        let got = parse_scope_query("@维护 磁盘");
        assert_eq!(got.tags, ["维护"], "单分类");
        assert_eq!(got.query, "磁盘", "余串是查询");

        let got = parse_scope_query("@维护 @开发 磁盘");
        assert_eq!(got.tags, ["维护", "开发"], "多分类保序");
        assert_eq!(got.query, "磁盘");

        let got = parse_scope_query("@维护 @维护 磁盘");
        assert_eq!(got.tags, ["维护"], "同分类去重");

        let got = parse_scope_query("@");
        assert!(got.tags.is_empty() && got.query.is_empty(), "裸 @ = 空查询空标签");

        let got = parse_scope_query("@ 磁盘");
        assert!(got.tags.is_empty(), "空名 @ 不算分类");
        assert_eq!(got.query, "磁盘", "空名 @ 照吃不记");

        let got = parse_scope_query("磁盘");
        assert!(got.tags.is_empty() && got.query == "磁盘", "无前缀原样通过");

        let got = parse_scope_query("邮件 我@例子.com");
        assert!(
            got.tags.is_empty() && got.query == "邮件 我@例子.com",
            "不以 @ 开头整串是查询"
        );

        let got = parse_scope_query("@维护 磁盘 @开发");
        assert_eq!(got.tags, ["维护"], "只吃开头的连续 @词");
        assert_eq!(got.query, "磁盘 @开发", "普通词之后的 @词是查询文本");

        let got = parse_scope_query("@维护@开发 磁盘");
        assert_eq!(got.tags, ["维护@开发"], "无空白的 @维护@开发 是一个分类名");
    }

    /// 批次 G1 候选池判据：显式分类 = 严格成员并集（无标签不入选）；
    /// 无显式分类时 All 全放行、Basic = 无标签 ∪ 基本。
    #[test]
    fn scope_filter_is_strict_union_or_basic_pool() {
        let mk = |tags: &[&str]| LaunchItem {
            name: "x".into(),
            path: "x".into(),
            kind: ItemKind::Command,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        };
        let basic = mk(&["基本"]);
        let maint = mk(&["维护"]);
        let both = mk(&["基本", "维护"]);
        let untagged = mk(&[]);

        let on = |explicit: &[&str]| {
            let e: Vec<String> = explicit.iter().map(|t| t.to_string()).collect();
            move |it: &LaunchItem| item_in_scope(it, &e, Scope::All)
        };
        assert!(on(&["维护"])(&maint), "显式 @维护 命中维护条目");
        assert!(on(&["维护"])(&both), "多标签条目任一命中即入选");
        assert!(!on(&["维护"])(&basic), "基本条目不冒充维护");
        assert!(!on(&["维护"])(&untagged), "无标签不属于任何分类");
        assert!(on(&["维护", "开发"])(&basic) == false, "并集不含基本");
        assert!(on(&["维护", "基本"])(&maint), "并集左支");
        assert!(on(&["维护", "基本"])(&basic), "并集右支");

        let in_scope = |it: &LaunchItem, scope: Scope| item_in_scope(it, &[], scope);
        assert!(in_scope(&maint, Scope::All) && in_scope(&untagged, Scope::All));
        assert!(in_scope(&basic, Scope::Basic), "基本档放行基本条目");
        assert!(in_scope(&untagged, Scope::Basic), "基本档放行无标签条目");
        assert!(!in_scope(&maint, Scope::Basic), "基本档拦下维护条目");
    }

    /// 批次 G1 提示行载荷：空清单给「还没有分类」，非空按名排序拼「名 数」。
    #[test]
    fn scope_tag_hint_lists_counts_sorted() {
        let empty: std::collections::BTreeMap<String, usize> = Default::default();
        assert_eq!(ScopeTagHint::from_counts(&empty).title, "还没有分类");

        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        counts.insert("维护".into(), 79);
        counts.insert("基本".into(), 24);
        let hint = ScopeTagHint::from_counts(&counts);
        assert_eq!(hint.title, "分类：基本 24 · 维护 79", "按名排序");
        assert!(!hint.subtitle.is_empty(), "副标题要教用法");
    }

    /// 批次 G1 落盘名：`Scope` 与 `ListRows` 同口径的 snake_case，序列化再读回不丢档。
    #[test]
    fn scope_serde_round_trips_both_variants() {
        for s in [Scope::All, Scope::Basic] {
            let json = serde_json::to_string(&s).unwrap();
            let back: Scope = serde_json::from_str(&json).unwrap();
            assert_eq!(back, s, "{} 往返后变档", s.as_str());
            assert_eq!(json, format!("\"{}\"", s.as_str()));
        }
        assert!(serde_json::from_str::<Scope>("\"nope\"").is_err(), "陌生名要报错");
    }

    /// FR-SRC-17 `hidden[]`：按 uid 剔除，别的条目一条不碰；空表/空白项不误伤。
    #[test]
    fn hidden_uids_are_dropped_and_nothing_else_is() {
        let mut items = vec![
            LaunchItem {
                name: "关机".into(),
                path: "internal:shutdown".into(),
                kind: ItemKind::Internal,
                ..Default::default()
            },
            LaunchItem {
                name: "记事本".into(),
                path: "notepad.exe".into(),
                kind: ItemKind::Command,
                ..Default::default()
            },
        ];
        let uid = items[0].uid();
        assert_eq!(
            drop_hidden(&mut items, &["   ".to_string()]),
            0,
            "空白项不算隐藏"
        );
        assert_eq!(
            drop_hidden(&mut items, &[uid.clone(), uid]),
            1,
            "重复 uid 只剔一条"
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "记事本");
        assert_eq!(drop_hidden(&mut items, &[]), 0);
    }

    /// 批次 F7（FR-CFG-07）：只清统计，置顶与别的字段原样；返回真变过的条数。
    #[test]
    fn reset_learning_clears_counts_and_keeps_pinned() {
        let mut items = vec![
            LaunchItem {
                name: "A".into(),
                path: "a.exe".into(),
                pinned: true,
                use_count: 9,
                last_used_ms: 123,
                keyword: "kuai".into(),
                ..Default::default()
            },
            LaunchItem {
                name: "B".into(),
                path: "b.exe".into(),
                ..Default::default()
            },
        ];
        assert_eq!(reset_learning(&mut items), 1, "只有第一条真有统计");
        assert_eq!((items[0].use_count, items[0].last_used_ms), (0, 0));
        assert!(items[0].pinned, "置顶不受重置学习影响");
        assert_eq!(items[0].keyword, "kuai", "已学别名清不掉（FR-ACT-10 偏差）");
        assert_eq!(reset_learning(&mut items), 0, "再清一次没有变动");
    }

    /// 批次 F7（FR-CFG-08）：恢复默认重置口径类字段，保留用户资产。
    #[test]
    fn restore_defaults_resets_policy_and_keeps_user_assets() {
        let mut s = AppSettings::default();
        let hot = s.toggle_hotkey;
        let typeback = s.typeback_hotkey;
        let second = s.secondary_hotkey;
        let engines = s.web_engines.len();
        s.space_to_run = SpaceRun::Always;
        s.tab_mode = TabMode::Navigate;
        s.learn_rank = LearnRank::Decay;
        s.sources_off = vec!["calc".into()];
        s.confirm_actions = false;
        s.terminal_shell = "pwsh".into();
        s.theme = "Modern".into();
        s.list_rows = ListRows::Ten;
        s.row_density = RowDensity::Dense;
        s.default_scope = Scope::Basic;
        s.secondary_action = SecondaryAction::Panel;
        s.history_size = 3;
        s.appearance.width = 820.0;
        s.appearance.remember_position = false;
        s.appearance.last_pos = Some([100, 200]);
        s.appearance.status_bar = false;
        s.appearance.fade = true;
        s.hidden = vec!["custom:app|c:\\x.exe|".into()];
        s.hidden_stash = vec![LaunchItem {
            name: "藏着".into(),
            path: "c:\\x.exe".into(),
            ..Default::default()
        }];
        s.seed_round = 99;

        restore_defaults(&mut s);

        assert_eq!(s.space_to_run, SpaceRun::default());
        assert_eq!(s.tab_mode, TabMode::default());
        assert_eq!(s.learn_rank, LearnRank::default());
        assert!(s.sources_off.is_empty(), "数据源恢复全开");
        assert!(s.confirm_actions, "危险操作确认恢复默认开");
        assert_eq!(s.terminal_shell, default_terminal_shell());
        assert_eq!(s.theme, default_theme());
        assert_eq!(s.list_rows, ListRows::default());
        assert_eq!(s.row_density, RowDensity::default());
        assert_eq!(s.default_scope, Scope::default());
        assert_eq!(
            s.secondary_action,
            SecondaryAction::default(),
            "次级动作形态回厂（批次 E3 加的，形态是口径不是资产）"
        );
        assert_eq!(
            s.history_size, HISTORY_SIZE_DEFAULT,
            "历史上限回厂：它是保留策略，跟着一起回默认"
        );
        assert_eq!(
            s.appearance,
            Appearance::default(),
            "外观段整段回厂（含记住位置与上次坐标）"
        );
        assert_eq!(s.toggle_hotkey, hot, "热键保留");
        assert_eq!(s.typeback_hotkey, typeback, "补字符热键保留");
        assert_eq!(s.secondary_hotkey, second, "次级动作键位属于热键三格，同样保留");
        assert_eq!(s.web_engines.len(), engines, "网页引擎保留");
        assert_eq!(s.hidden.len(), 1, "隐藏项保留（否则删掉的条目会复活）");
        assert_eq!(s.hidden_stash.len(), 1, "隐藏快照跟着 hidden[] 一起保留");
        assert_eq!(s.seed_round, 99, "补种轮次保留");
    }

    /// 批次 F8（FR-UIX-02）：宽度是手改得动的配置，越界要钳、怪数要落回默认。
    #[test]
    fn window_width_is_clamped_to_range() {
        assert_eq!(clamp_window_width(500.0), 500.0);
        assert_eq!(clamp_window_width(100.0), WINDOW_WIDTH_MIN);
        assert_eq!(clamp_window_width(5000.0), WINDOW_WIDTH_MAX);
        assert_eq!(clamp_window_width(0.0), WINDOW_WIDTH_MIN);
        assert_eq!(
            clamp_window_width(f32::NAN),
            Appearance::default().width,
            "非有限值不能原样进窗口"
        );
    }

    /// 批次 F8：旧 `settings.json` 没有 `appearance` 键 ⇒ 整段取默认（零迁移）。
    #[test]
    fn appearance_section_is_optional_in_old_settings() {
        let s: AppSettings = serde_json::from_str("{\"theme\":\"Modern\"}").unwrap();
        assert_eq!(s.appearance, Appearance::default());
        assert!(s.appearance.status_bar, "状态栏默认开");
        assert!(!s.appearance.fade, "淡入默认关（FR-UIX-07）");
    }

    /// 批次 F7（FR-CFG-08）：备份名带编号，同一秒内第二份也不覆盖；源文件不在就不造。
    #[test]
    fn backup_file_is_numbered_and_never_overwrites() {
        let dir = temp_log_dir("backup");
        let src = dir.join("settings.json");
        std::fs::write(&src, b"{\"theme\":\"Modern\"}").unwrap();
        let a = backup_file(&src).expect("第一份备份");
        let b = backup_file(&src).expect("第二份备份");
        assert_ne!(a, b, "同一秒内第二份要换编号，不能覆盖");
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        assert!(
            backup_file(&dir.join("missing.json")).is_none(),
            "源文件不在就不造空备份"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 批次 F7（D6）：隐藏列表能翻译出出厂名称，孤儿 uid 只显示原文。
    #[test]
    fn hidden_rows_resolve_factory_names_and_keep_orphans() {
        let seed = builtin_seeds().into_iter().next().unwrap();
        let orphan = "custom:app|z:\\none|".to_string();
        let rows = hidden_rows(&[seed.uid(), orphan.clone(), "   ".to_string()], &[]);
        assert_eq!(rows.len(), 2, "空白项不占一行");
        assert_eq!(rows[0].name.as_deref(), Some(seed.name.as_str()));
        assert_eq!(rows[1].name, None);
    }

    /// 批次 D6：扫描项没有出厂种子，名称只能从隐藏快照里读；同 uid 再藏只覆盖不堆积。
    #[test]
    fn hidden_rows_read_stashed_scan_names() {
        let scan = LaunchItem {
            name: "扫描应用".to_string(),
            path: r"C:\tools\scan.exe".to_string(),
            kind: ItemKind::App,
            ..Default::default()
        };
        let uid = scan.uid();
        let mut stash: Vec<LaunchItem> = Vec::new();
        stash_hidden(&mut stash, &scan);
        stash_hidden(&mut stash, &scan);
        assert_eq!(stash.len(), 1, "同一 uid 再藏一次只覆盖");

        let rows = hidden_rows(&[uid, "custom:app|z:\\none|".to_string()], &stash);
        assert_eq!(rows[0].name.as_deref(), Some("扫描应用"));
        assert_eq!(rows[1].name, None, "既无快照也无种子才是孤儿");
    }

    /// 批次 F7（D6）：逐条恢复只补点名那一条；孤儿留在 `hidden[]` 里；全部恢复跳过孤儿。
    #[test]
    fn restoring_hidden_puts_only_that_seed_back() {
        let seeds = builtin_seeds();
        let (seed, other) = (seeds[0].clone(), seeds[1].clone());
        let orphan = "custom:app|z:\\gone|".to_string();
        let mut items: Vec<LaunchItem> = Vec::new();
        let mut hidden = vec![seed.uid(), other.uid(), orphan.clone()];
        let mut stash: Vec<LaunchItem> = Vec::new();

        assert!(restore_hidden(
            &mut items,
            &mut hidden,
            &mut stash,
            &seed.uid()
        ));
        assert_eq!(items.len(), 1, "只补点名的这一条");
        assert_eq!(items[0].uid(), seed.uid());
        assert!(!hidden.contains(&seed.uid()));
        assert!(hidden.contains(&other.uid()), "别的隐藏项不动");

        assert!(
            !restore_hidden(&mut items, &mut hidden, &mut stash, &orphan),
            "孤儿 uid 不给恢复"
        );
        assert!(hidden.contains(&orphan));

        assert_eq!(restore_all_hidden(&mut items, &mut hidden, &mut stash), 1);
        assert_eq!(items.len(), 2);
        assert_eq!(hidden, vec![orphan], "孤儿原样留在表里");
    }

    /// 批次 D6：隐藏 → 恢复要把扫描项原样带回（置顶 / 学习计数不丢），快照用完即清。
    #[test]
    fn restore_hidden_brings_back_a_scanned_item_with_its_stats() {
        let mut scan = LaunchItem {
            name: "扫描应用".to_string(),
            path: r"C:\tools\scan.exe".to_string(),
            kind: ItemKind::App,
            ..Default::default()
        };
        scan.pinned = true;
        scan.use_count = 7;
        let uid = scan.uid();
        let mut items: Vec<LaunchItem> = Vec::new();
        let mut hidden = vec![uid.clone()];
        let mut stash: Vec<LaunchItem> = Vec::new();
        stash_hidden(&mut stash, &scan);

        assert!(restore_hidden(&mut items, &mut hidden, &mut stash, &uid));
        assert!(hidden.is_empty());
        assert!(stash.is_empty(), "恢复后快照不留在配置里");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].uid(), uid);
        assert!(items[0].pinned, "置顶跟着快照回来");
        assert_eq!(items[0].use_count, 7, "学习计数跟着快照回来");
    }

    /// 批次 D6：重新扫描导入的结果进清单前先按 `hidden[]` 剔掉，隐藏项不会复活。
    #[test]
    fn rescan_results_are_filtered_by_hidden() {
        let hidden_scan = LaunchItem {
            name: "藏起来的扫描项".to_string(),
            path: r"C:\tools\hidden.exe".to_string(),
            kind: ItemKind::App,
            ..Default::default()
        };
        let fresh_scan = LaunchItem {
            name: "新扫描到的".to_string(),
            path: r"C:\tools\fresh.exe".to_string(),
            kind: ItemKind::App,
            ..Default::default()
        };
        let mut hidden = vec![hidden_scan.uid()];
        let mut stash: Vec<LaunchItem> = Vec::new();
        stash_hidden(&mut stash, &hidden_scan);

        let mut imported = vec![hidden_scan.clone(), fresh_scan.clone()];
        assert_eq!(drop_hidden(&mut imported, &hidden), 1);
        let merged = dedup_items(imported);
        assert_eq!(merged.len(), 1, "隐藏项没被重新导入");
        assert_eq!(merged[0].uid(), fresh_scan.uid());
        assert_eq!(stash.len(), 1, "快照还在，随时能在高级页恢复");
        hidden.clear();
    }

    /// 批次 D6：显式添加（弹窗 / SendTo）撤销同名隐藏，否则下次启动会被静默剔掉。
    #[test]
    fn explicit_add_clears_the_hidden_record() {
        let scan = LaunchItem {
            name: "扫描应用".to_string(),
            path: r"C:\tools\scan.exe".to_string(),
            kind: ItemKind::App,
            ..Default::default()
        };
        let uid = scan.uid();
        let mut hidden = vec![uid.clone(), "custom:app|z:\\keep|".to_string()];
        let mut stash: Vec<LaunchItem> = Vec::new();
        stash_hidden(&mut stash, &scan);

        assert_eq!(unhide_uids(&mut hidden, &mut stash, &[uid.clone()]), 1);
        assert!(!hidden.contains(&uid));
        assert!(stash.is_empty(), "快照一起撤掉，免得留成孤儿");
        assert_eq!(hidden.len(), 1, "别的隐藏项不动");
        assert_eq!(unhide_uids(&mut hidden, &mut stash, &[uid]), 0, "重复撤销恒 0");
    }

    /// 出厂种子必须真的包含 A2 的五组（自测一遍，防止 `system_seeds()` 没接上）。
    #[test]
    fn factory_seeds_include_the_system_command_groups() {
        let seeds = builtin_seeds();
        let count = |pred: fn(&LaunchItem) -> bool| seeds.iter().filter(|s| pred(s)).count();
        assert_eq!(
            count(|s| s.kind == ItemKind::Internal && s.path == "internal:shutdown"),
            1
        );
        assert!(
            count(|s| s.path.starts_with("ms-settings:")) >= 30,
            "ms-settings 页没接进来"
        );
        assert!(
            count(|s| s.path == "devmgmt.msc") == 1,
            "Windows 工具没接进来"
        );
        assert!(
            count(|s| s.path == "internal:volume-mute") == 1,
            "音量与媒体没接进来"
        );
    }

    /// 出厂条目的关键字两两不能撞：撞了设置页一开局就报一串警告，而且触发独占时
    /// 只认排在前面的那条（A2 新增 78 条最容易踩这条）。
    #[test]
    fn factory_seed_keywords_do_not_collide() {
        let dups = crate::source::custom::duplicate_keywords(&builtin_seeds());
        assert!(dups.is_empty(), "出厂条目关键字撞车：{dups:?}");
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
        assert_eq!(
            s.learn_rank,
            LearnRank::Count,
            "旧偏好没有 learn_rank ⇒ 默认甲"
        );
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
        assert!(
            json.contains(r#""space_to_run":"unique""#),
            "实际写盘：{json}"
        );
        assert!(json.contains(r#""learn_rank":"count""#), "实际写盘：{json}");
        assert!(serde_json::from_str::<AppSettings>(r#"{"space_to_run":"wod"}"#).is_err());
        assert!(serde_json::from_str::<AppSettings>(r#"{"learn_rank":"decayy"}"#).is_err());
    }

    /// FR-SRC-16 的 `terminal_shell`：字段缺失＝`cmd`（旧 `settings.json` 照旧能读），
    /// 名字不认识也回落 `cmd` —— 与 `sources_off` 同一口径，绝不让一个陌生词把整份配置判坏。
    #[test]
    fn terminal_shell_defaults_to_cmd_and_unknown_names_do_not_break_the_file() {
        use crate::source::terminal::TermShell;
        let missing: AppSettings = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(missing.terminal_shell, "cmd");
        assert_eq!(missing.term_shell(), TermShell::Cmd);
        let known: AppSettings = serde_json::from_str(r#"{"terminal_shell":"pwsh"}"#).unwrap();
        assert_eq!(known.term_shell(), TermShell::Pwsh);
        let unknown: AppSettings = serde_json::from_str(r#"{"terminal_shell":"bash"}"#).unwrap();
        assert_eq!(
            unknown.term_shell(),
            TermShell::Cmd,
            "陌生名字回落默认，不判坏配置"
        );
        let d = AppSettings::default();
        assert_eq!(d.terminal_shell, "cmd");
    }

    #[test]
    fn default_settings_carry_the_builtin_web_engines() {
        // 2026-10-23 A6：整份配置缺失/判坏时走的是 `Default`，派生实现给的是空表 ⇒
        // 首次运行与 E1 重建后的网页搜索一条引擎都没有。手写 Default 修的就是这一刻。
        let d = AppSettings::default();
        assert_eq!(
            d.web_engines.len(),
            crate::source::websearch::builtin_engines().len()
        );
        assert!(d.web_engines.iter().any(|e| e.keyword == "g"));
        assert!(d.sources_off.is_empty());
    }

    #[test]
    fn target_stem_covers_commands_and_apps_only() {
        let cmd = LaunchItem {
            name: "记事本".to_string(),
            path: "notepad.exe".to_string(),
            kind: ItemKind::Command,
            ..Default::default()
        };
        assert_eq!(target_stem(&cmd).as_deref(), Some("notepad"));
        let app = LaunchItem {
            name: "微信".to_string(),
            path: r"C:\Program Files\Tencent\WeChat.exe".to_string(),
            kind: ItemKind::App,
            ..Default::default()
        };
        assert_eq!(target_stem(&app).as_deref(), Some("WeChat"));
        let url = LaunchItem {
            path: "https://example.com/a.html".to_string(),
            kind: ItemKind::Url,
            ..Default::default()
        };
        assert_eq!(target_stem(&url), None, "URL 没有稳定目标名");
        let uwp = LaunchItem {
            path: r"shell:AppsFolder\Microsoft.WindowsNotepad_8wekyb3d8bbwe!App".to_string(),
            kind: ItemKind::App,
            ..Default::default()
        };
        assert_eq!(target_stem(&uwp), None, "AUMID 不是文件路径");
        let tmpl = LaunchItem {
            path: r"C:\tools\{query}.exe".to_string(),
            kind: ItemKind::Command,
            ..Default::default()
        };
        assert_eq!(
            target_stem(&tmpl),
            None,
            "带 {{query}} 的路径由用户参数决定"
        );
    }

    #[test]
    fn learn_alias_gates_then_appends() {
        let mut it = LaunchItem {
            name: "计算器".to_string(),
            keyword: "jsq".to_string(),
            ..Default::default()
        };
        assert!(!learn_alias(&mut it, "jsq"), "已在别名里 ⇒ 不重复攒");
        assert!(
            !learn_alias(&mut it, "j"),
            "单字不攒：会给所有含 j 的查询喂 30 档噪声"
        );
        assert!(!learn_alias(&mut it, "计算器"), "等于名称 ⇒ 没有可学的");
        assert!(
            !learn_alias(&mut it, "计算"),
            "名称里已有的词不学：只会让重复关键字表多一行"
        );
        assert!(!learn_alias(&mut it, "  "), "空白词不攒");
        assert!(learn_alias(&mut it, "CALC"), "新的两字符以上触发词应攒下");
        assert_eq!(it.keyword, "jsq calc", "空格追加，不动默认拼音首字母");
        assert!(!learn_alias(&mut it, "calc"), "大小写去重");

        let mut bare = LaunchItem {
            name: "Notepad".to_string(),
            keyword: String::new(),
            ..Default::default()
        };
        assert!(learn_alias(&mut bare, "np"));
        assert_eq!(bare.keyword, "np", "原本无别名时直接落第一个词");
    }

    /// FR-QRY-09：通配符是**查询语法**，不是触发词。学进别名表会留下一条普通查询
    /// 永远匹配不上、却仍参与 Tab 补全与去重的垃圾。
    #[test]
    fn learn_alias_refuses_wildcard_patterns() {
        let mut it = LaunchItem {
            name: "计算器".to_string(),
            keyword: String::new(),
            ..Default::default()
        };
        assert!(!learn_alias(&mut it, "*calc"), "含 `*` 不攒");
        assert!(!learn_alias(&mut it, "c?lc"), "含 `?` 不攒");
        assert_eq!(it.keyword, "", "别名表一字不动");
        assert!(learn_alias(&mut it, "calc"), "普通触发词照旧能学");
        assert_eq!(it.keyword, "calc");
    }

    /// FR-WIN-11 开机自启的归属判据（安全规则的可测部分）：target 与参数**两个都得对**，
    /// 否则一律视为别人建的文件——既不认作"已安装"，卸载时也会拒绝碰它。
    #[test]
    fn startup_ownership_needs_both_target_and_startup_arg() {
        assert!(is_our_startup_link(
            "C:\\a\\egui-launcher.exe",
            "C:\\a\\egui-launcher.exe",
            "-Startup"
        ));
        assert!(
            is_our_startup_link("C:\\a\\e.exe", "C:\\a\\e.exe", " -Startup "),
            "启动夹里的参数串可能带首尾空格，不参与判断"
        );
        assert!(
            !is_our_startup_link("C:\\a\\e.exe", "C:\\a\\e.exe", ""),
            "无参数＝不是本项"
        );
        assert!(!is_our_startup_link(
            "C:\\a\\e.exe",
            "C:\\a\\e.exe",
            "-PREFERENCES"
        ));
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
        assert!(
            is_self_target(&plain.to_uppercase(), &key),
            "快捷方式里大小写不同也要认出来"
        );
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
        let base =
            std::env::temp_dir().join(format!("egui-launcher-startup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = std::env::var_os("APPDATA");
        // SAFETY: 本进程里只有 sendto/startup 两个前缀读 `APPDATA`，都不在这条测试之外被调用；
        // 结束即还原（含失败路径），不污染同进程其它测试线程。
        unsafe { std::env::set_var("APPDATA", &base) };
        let r = (|| -> Result<(), String> {
            let startup_dir = base.join(r"Microsoft\Windows\Start Menu\Programs\Startup");
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
            let msg = uninstall_startup()
                .err()
                .ok_or("别人的快捷方式居然被删了")?;
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

    /// 批次 P 单测的临时目录：每次调用带测试名 + pid，避免同进程并行线程互相踩。
    fn temp_log_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("egui-launcher-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 轮转的判据是「文件首行日期锚 ≠ 今天」，不是 mtime。这条同时钉住两件事：
    /// 锚是唯一真相（读不出就归「无日期」，绝不拿 mtime 猜一天），以及同日的多次启动
    /// 必须合并进同一份文件（否则自启一天重启三次就把历史冲掉三份）。
    #[test]
    fn log_rotation_keys_on_the_date_anchor_not_on_mtime() {
        let dir = temp_log_dir("rotate");
        let log = dir.join("launcher.log");

        // ① 批次 O 留下的旧格式（首行没有锚）⇒ 归成「无日期」，不假装知道它是哪天。
        std::fs::write(
            &log,
            "log: 本次启动没有控制台，stderr 已接到 X\nhotkey: ok\n",
        )
        .unwrap();
        rotate_at(&log, "2026-10-19");
        assert!(!log.exists(), "无锚的旧文件应当被归档走");
        assert!(dir.join("launcher-无日期.log").exists());

        // ② 有锚且是昨天 ⇒ 按锚里的日期归档。
        std::fs::write(&log, format!("{LOG_DATE_TAG}2026-10-18 22:07 ｜ 昨天的\n")).unwrap();
        rotate_at(&log, "2026-10-19");
        assert!(
            dir.join("launcher-2026-10-18.log").exists(),
            "跨天要按锚里的 2026-10-18 归档"
        );
        assert!(!log.exists());

        // ③ 锚就是今天 ⇒ 一个字都不动（同日重启合进同一份）。
        std::fs::write(&log, format!("{LOG_DATE_TAG}2026-10-19 08:00 ｜ 今天的\n")).unwrap();
        rotate_at(&log, "2026-10-19");
        assert!(log.exists(), "同一天的日志不该被归档");
        assert!(!dir.join("launcher-2026-10-19.log").exists());

        // ④ 锚坏了 ⇒ 走「无日期」分支，而它第一次已被占用 ⇒ 必须落到 `.2`，不能覆盖已有归档。
        std::fs::write(&log, format!("{LOG_DATE_TAG}昨天 ｜ 坏锚\n")).unwrap();
        rotate_at(&log, "2026-10-19");
        assert!(!log.exists());
        assert!(
            dir.join("launcher-无日期.2.log").exists(),
            "同名归档该加序号，不该覆盖"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 裁归档只认"完整的归档名形状"。前缀判据会把用户放在同一目录里的文件当自己的删掉，
    /// 所以这里专门放三件不该被动东西：正在写的日志、批次 O 的 `.old`、以及两个像我们但不是的名字。
    #[test]
    fn log_prune_keeps_newest_three_and_never_touches_foreign_names() {
        let dir = temp_log_dir("prune");
        let base =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        for i in 0..5u64 {
            let p = dir.join(format!("launcher-2026-01-0{}.log", i + 1));
            std::fs::write(&p, format!("第 {} 份", i + 1)).unwrap();
            let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
            f.set_modified(base + std::time::Duration::from_secs(i * 86_400))
                .unwrap();
        }
        // 01-01 最旧、01-05 最新 ⇒ 留 03/04/05。
        std::fs::write(dir.join("launcher.log"), "live").unwrap();
        std::fs::write(dir.join("launcher.log.old"), "legacy").unwrap();
        std::fs::write(dir.join("launcher-notes.log"), "foreign").unwrap();
        std::fs::write(dir.join("launcher-2026-01-0a.log"), "shaped-but-not").unwrap();

        prune_archives(&dir);

        assert!(
            !dir.join("launcher-2026-01-01.log").exists(),
            "最旧的两份该被裁掉"
        );
        assert!(!dir.join("launcher-2026-01-02.log").exists());
        for i in 3..=5 {
            assert!(
                dir.join(format!("launcher-2026-01-0{i}.log")).exists(),
                "最近 3 份要留住"
            );
        }
        for (name, body) in [
            ("launcher.log", "live"),
            ("launcher.log.old", "legacy"),
            ("launcher-notes.log", "foreign"),
            ("launcher-2026-01-0a.log", "shaped-but-not"),
        ] {
            assert_eq!(
                std::fs::read_to_string(dir.join(name)).unwrap(),
                body,
                "{name} 不该被动"
            );
        }

        // 归属判据本体的边界：名字形状。
        assert!(is_our_archive("launcher-2026-01-01.log"));
        assert!(is_our_archive("launcher-2026-01-01.7.log"));
        assert!(is_our_archive("launcher-无日期.log"));
        assert!(!is_our_archive("launcher.log"));
        assert!(!is_our_archive("launcher-notes.log"));
        assert!(!is_our_archive("launcher-2026-1-1.log"));
        assert!(!is_our_archive("launcher-2026-01-01.log.bak"));
        assert!(!is_our_archive("mylauncher-2026-01-01.log"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 批次 P 的归档名要从 `local_now_stamp()` 的前 10 个字符切出来 ⇒ 格式一变就切错，
    /// 这条把形状钉住（月/日/时/分都补零）。
    #[test]
    fn local_stamp_is_iso_prefixed() {
        let s = local_now_stamp();
        assert!(
            is_iso_date(&s[..10]),
            "前 10 字符必须是 YYYY-MM-DD，实得：{s}"
        );
        let b = s.as_bytes();
        assert!(b[10] == b' ' && b[13] == b':' && b[11..13].iter().all(|c| c.is_ascii_digit()));
        assert_eq!(s.len(), 16, "完整横幅应为 YYYY-MM-DD HH:MM，实得：{s}");
    }

    /// 批次 X 的 E1 我自己写坏的那一处，用**落盘的真形状**当场问一遍。
    /// 旧写法 `load_json(&p).unwrap_or_else(|ev| (Vec::new(), ev))` 会让推断把
    /// `unwrap_or_else` 的返回格当成 `load_json` 的 `T` ⇒ `T` 变成「(内容, 下场)」整个元组
    /// ⇒ 解析按"两元素 JSON 数组"要求清单 ⇒ **每一次启动都把好好的文件判成坏的**。
    /// 真机⑫（2026-10-22）现场：`.bad`…`.bad9` 九份全是我自己写出去的正常清单（第一份 71490
    /// 字节、181 条，python 独立验证是合法数组），第 10 次名额占满 ⇒ 留证报"没留住" ⇒ 覆盖写。
    /// 这条测试不碰真实清单目录，只在临时目录里跑，两格都验：正常文件必须 `Ok`、坏文件必须留证。
    #[test]
    fn a_real_list_file_is_an_array_not_a_two_tuple() {
        let dir = temp_log_dir("e1-roundtrip");
        let p = dir.join("items.json");

        // ① 真形状 ⇒ 必须读回来。这一格就是旧代码当场失败的那一格。
        let items = builtin_seeds();
        let json = serde_json::to_vec_pretty(&items).expect("种子序列化不该失败");
        std::fs::write(&p, &json).expect("临时目录写不了");
        let got: Vec<LaunchItem> = load_json(&p).expect("真实形状的清单必须解析成功，不许判坏");
        assert_eq!(got.len(), items.len(), "读回来的条数要与写下去的一致");

        // ② 真坏了 ⇒ 改名留证，原位置腾出来。
        std::fs::write(&p, "「这不是 JSON」".as_bytes()).expect("写坏文件失败");
        let fault = load_json::<Vec<LaunchItem>>(&p)
            .err()
            .expect("坏文件必须报 Err");
        let Corrupted::Kept(kept) = fault else {
            panic!("第一次留证该成功，实得 {fault:?}")
        };
        assert!(
            kept.exists() && !p.exists(),
            "证据该在 {kept:?}，原位置要腾出来"
        );

        // ③ `.bad` 那一族的名额被占满 ⇒ 不许就此说"没留住"，要走毫秒时间戳那一格。
        //    （真机那一次正是在这里翻车：报了"没留住"，调用方照常覆盖写。）
        for n in 0..BAD_NAME_SLOTS {
            let suffix = if n == 0 {
                "bad".to_string()
            } else {
                format!("bad{n}")
            };
            std::fs::write(dir.join(format!("items.json.{suffix}")), b"x").expect("占名额失败");
        }
        std::fs::write(&p, "「还是不是 JSON」".as_bytes()).expect("第二次写坏文件失败");
        let again = load_json::<Vec<LaunchItem>>(&p)
            .err()
            .expect("第二次坏文件必须报 Err");
        match again {
            // 时间戳那份与原文件都留着 ⇒ 这一格是"另存副本"还是"改名"都能接受，
            // 只要求：证据在、`Lost` 没出现。
            Corrupted::Kept(q) => {
                assert!(q.exists(), "时间戳留证该落下文件，实得 {q:?}");
                assert!(
                    q.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("items.json.bad")),
                    "留证名要挂在原名后面，实得 {q:?}"
                );
            }
            Corrupted::Lost => panic!("名额占满不等于留不到证：时间戳那一格该接住这一路"),
        }

        // ④ 症状本体的证据：同一份**合法数组**按"两元素元组"去读必然失败。旧写法推断出来的 `T`
        //    正是这个形状，所以真机每一次启动都在 ① 那一格报"解析失败"——判坏的不是文件，是我。
        //    （`Corrupted` 没有 `Deserialize`，这里用一个能编译的元组当替身。放在最后一格是因为
        //    这次失败照样会走留证、把 `p` 改名出去，不许它影响上面三格。）
        std::fs::write(&p, &json).expect("重写真形状失败");
        let as_tuple = load_json::<(Vec<LaunchItem>, Option<u32>)>(&p);
        assert!(
            as_tuple.is_err(),
            "按元组要求真清单必须失败，实得 Ok ⇒ 这条替身没照住旧形状"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 上一条只问到 `load_json`，**出错的其实是包装层**（`load_user_items` 里那次推断），
    /// 所以这条才是要害：走真函数、真落盘、真读回，断言"一份正常清单不许报坏"。
    /// 批次 X 那版在这里必然失败——它会报 `Some(Kept)` 且内容空，因为解析按两元素元组要求文件。
    ///
    /// 这条测试要往清单的真实落盘位置写文件，所以先自证位置安全：只允许落在**测试二进制自己
    /// 那一格目录**（便携优先那条判据下的 exe 同级）。位置不对就直接跳过，绝不拿用户的
    /// `%LOCALAPPDATA%\egui-launcher` 当真清单的耗材。原有的文件先挪开、结束还原。
    #[test]
    fn load_user_items_reads_back_a_file_it_just_wrote() {
        let list = user_list_path();
        let here = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.to_path_buf()));
        let Some(exe_dir) = here else { return };
        if list.parent() != Some(exe_dir.as_path()) {
            eprintln!(
                "跳过：清单落盘位置 {} 不在测试 exe 同级 ⇒ 不往里写",
                list.display()
            );
            return;
        }
        let settings = settings_path();
        let backup = |p: &Path| -> Option<PathBuf> {
            if !p.exists() {
                return None;
            }
            let to = p.with_file_name(format!("{}.test-backup", p.file_name()?.to_str()?));
            std::fs::rename(p, &to).ok().map(|_| to)
        };
        let saved_list = backup(&list);
        let saved_settings = backup(&settings);

        // ① 写一份真清单再读回来：不许报坏，条数要一致。这一格就是旧代码的照妖镜。
        let seeds = builtin_seeds();
        save_user_items(&seeds);
        let (got, fault) = load_user_items();
        assert!(fault.is_none(), "正常清单不许判坏，实得 {fault:?}");
        assert_eq!(got.len(), seeds.len(), "读回来的条数要与写下去的一致");

        // ② 偏好同理（真机那一条 `expected a tuple of size 2` 就是从这份文件上先冒出来的）。
        save_settings(&AppSettings::default());
        let (prefs, fault) = load_settings();
        assert!(fault.is_none(), "正常偏好不许判坏，实得 {fault:?}");
        assert_eq!(prefs.seed_round, AppSettings::default().seed_round);

        // ③ 真的坏才报坏，且要留证在原位置之外。
        std::fs::write(&list, "[1,2]".as_bytes()).expect("写坏清单失败");
        match load_user_items() {
            (empty, Some(Corrupted::Kept(kept))) => {
                assert!(empty.is_empty(), "坏文件读回来必须是空");
                assert!(
                    kept.exists() && !list.exists(),
                    "证据该在 {kept:?}，原位置该腾出来"
                );
                let _ = std::fs::remove_file(&kept);
            }
            (_, other) => panic!("坏清单该走 Kept，实得 {other:?}"),
        }

        let _ = std::fs::remove_file(&list);
        let _ = std::fs::remove_file(&settings);
        for back in [saved_list, saved_settings].into_iter().flatten() {
            let Some(name) = back
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".test-backup"))
            else {
                continue;
            };
            if let Some(dir) = back.parent() {
                let _ = std::fs::rename(&back, dir.join(name));
            }
        }
    }
}

/// 批次 Q（FR-WIN-02 热键可配置）的判据测试。刻意全部走纯函数：`RegisterHotKey` 那一半
/// 只有真机能验（占用与否取决于本机装了什么），不放进单元测试冒充已证。
#[cfg(test)]
mod hotkey_tests {
    use super::*;

    /// A–Z / 0–9 的 VK 就是 ASCII 大写；生产端由录制映射给出，这里只为测试写得动。
    fn vk(ch: char) -> u32 {
        ch.to_ascii_uppercase() as u32
    }

    /// F 键按偏移算（只定义了 `VK_F1`/`VK_F24` 两个端点）。
    fn vk_f(n: u32) -> u32 {
        VK_F1 + n - 1
    }

    #[test]
    fn default_is_the_hardcoded_ctrl_alt_space() {
        // 这条钉住"批次 Q 之前写死的那一组数"= 出厂值，改了它等于偷偷换了用户认识的键。
        assert_eq!(
            HotkeySpec::DEFAULT.mods,
            HKF_CONTROL | HKF_ALT | HKF_NOREPEAT
        );
        assert_eq!(HotkeySpec::DEFAULT.vk, VK_SPACE);
        assert_eq!(HotkeySpec::default(), HotkeySpec::DEFAULT);
        assert!(HotkeySpec::DEFAULT.check().is_ok());
        assert_eq!(HotkeySpec::DEFAULT.display(), "Ctrl+Alt+Space");
    }

    /// 2026-10-27 批次 E6：这五枚位值的**唯一出口**是 `main.rs` 里
    /// `RegisterHotKey(.., HOT_KEY_MODIFIERS(spec.mods), ..)` ⇒ 它们必须是 Win32 的那五个数。
    /// 首版 Shift 抄成 `0x0010`（`MOD_SHIFT` 其实是 `0x0004`），`check()` 放行、`display()` 也显示，
    /// 但系统回 `1004` ⇒ 带 Shift 的全局键**从来没注册成功过**。这条断言就是把那两个字节钉住：
    /// 谁再改回 `0x10`，单元测试当场变红（真机侧的对偶证据见 `target/e6/hkprobe.ps1` 的三行输出）。
    #[test]
    fn modifier_bits_are_the_win32_mod_values() {
        assert_eq!(HKF_ALT, 0x0001, "MOD_ALT");
        assert_eq!(HKF_CONTROL, 0x0002, "MOD_CONTROL");
        assert_eq!(HKF_SHIFT, 0x0004, "MOD_SHIFT —— 不是 VK_SHIFT，也不是 0x10");
        assert_eq!(HKF_WIN, 0x0008, "MOD_WIN");
        assert_eq!(HKF_NOREPEAT, 0x4000, "MOD_NOREPEAT");
        // 判据端的连带事实：Ctrl+Shift+<键> 是 `check()` 认可的组合，位值修对之后两端才第一次对上。
        assert!(
            HotkeySpec {
                mods: HKF_CONTROL | HKF_SHIFT | HKF_NOREPEAT,
                vk: VK_OEM_3
            }
            .check()
            .is_ok()
        );
        // 而且 `0x10` 现在不再是任何一位 ⇒ 旧那份值不会被误认成 Shift。
        assert_eq!(HotkeySpec { mods: 0x0010, vk: VK_A }.display(), "A");
    }

    #[test]
    fn display_orders_modifiers_and_names_vks() {
        let show = |mods, code| HotkeySpec { mods, vk: code }.display();
        assert_eq!(show(HKF_CONTROL | HKF_ALT, VK_A), "Ctrl+Alt+A");
        // NOREPEAT 是行为位，不进键名。
        assert_eq!(show(HKF_CONTROL | HKF_NOREPEAT, vk_f(5)), "Ctrl+F5");
        // 顺序恒为 Ctrl→Alt→Shift→Win，与录制端无关。
        assert_eq!(
            show(HKF_WIN | HKF_SHIFT | HKF_ALT | HKF_CONTROL, vk('7')),
            "Ctrl+Alt+Shift+Win+7"
        );
        assert_eq!(show(HKF_ALT, VK_RETURN), "Alt+Enter");
        assert_eq!(show(HKF_CONTROL, VK_F24), "Ctrl+F24");
        // 范围外的码必须显式露出来，不能显示成空白让人以为"没设键"。
        assert_eq!(show(HKF_CONTROL, VK_BACK), "Ctrl+Backspace");
        assert_eq!(show(HKF_CONTROL, 0x6A), "Ctrl+VK#6A");
        // 批次 R：`` ` `` 的展示名就是它自己，`Win+` 这个串是用户提需求时的原话。
        assert_eq!(show(0, VK_OEM_3), "`");
        assert_eq!(show(HKF_WIN | HKF_NOREPEAT, VK_OEM_3), "Win+`");
    }

    #[test]
    fn check_requires_a_strong_modifier_for_textable_keys() {
        let ok = |mods, code| HotkeySpec { mods, vk: code }.check().is_ok();
        // 字母/数字/空格/回车：带 Ctrl、Alt 或 Win 才行；Shift 单键只是打字，不收。
        assert!(ok(HKF_CONTROL, VK_A));
        assert!(ok(HKF_ALT, VK_SPACE));
        assert!(ok(HKF_WIN, vk('E')));
        assert!(!ok(HKF_SHIFT, VK_A), "Shift+A 就是打大写 A，不能当全局键");
        assert!(!ok(0, VK_A));
        assert!(!ok(HKF_NOREPEAT, vk('1')), "NOREPEAT 不算修饰键");
        // F 键有专职身份，允许单独用。
        assert!(ok(0, VK_F1));
        assert!(ok(HKF_CONTROL, vk_f(13)));
        // 不支持的码段一律拒（录制端本就不产这些，这里是第二道）。
        assert!(!ok(HKF_CONTROL, VK_BACK), "Backspace 不是可录制目标");
        assert!(!ok(HKF_CONTROL, 0x6A), "小键盘键不在本批口径内");
        assert!(!ok(HKF_CONTROL, VK_F24 + 1));
    }

    /// 批次 R 的例外集：无修饰键**只**允许 `` ` ``，且 `Shift+`` ` ```（也就是 `~`）必须被挡。
    /// 这两条都是"代价不对称"的判据：放开裸 `` ` `` 的代价是全系统打不出 `` ` ``（有补字符热键兜），
    /// 而放开 `Shift+`` ` `` 的代价是打不出 `~`，却换不回任何东西——裸 `` ` `` 那条注册已经把
    /// `` ` `` 吃掉了。真机实测过：裸 `` ` `` 注册着的时候 `Shift+` `` ` `` 仍然打得出来。
    #[test]
    fn bare_backtick_is_the_only_modifier_less_key_allowed() {
        let ok = |mods, code| HotkeySpec { mods, vk: code }.check().is_ok();
        // 裸 `` ` ``：本批唯一的新例外，必须放行。
        assert!(ok(0, VK_OEM_3), "裸 ` 是批次 R 用户点名要的单键触发");
        assert!(
            ok(HKF_NOREPEAT, VK_OEM_3),
            "NOREPEAT 是行为位，不该影响判据"
        );
        assert!(
            HotkeySpec {
                mods: HKF_NOREPEAT,
                vk: VK_OEM_3
            }
            .backtick_bare()
        );
        // 带 Ctrl / Alt / Win 的 `` ` `` 照旧收（补字符热键就走 Win+`` ` ``这条路）。
        assert!(ok(HKF_CONTROL, VK_OEM_3));
        assert!(ok(HKF_ALT, VK_OEM_3));
        assert!(ok(HKF_WIN, VK_OEM_3));
        assert!(ok(HKF_CONTROL | HKF_ALT, VK_OEM_3));
        // 单带 Shift 的 `` ` `` 必须挡，而且挡的是"只有 Shift"这一种。
        assert!(
            !ok(HKF_SHIFT, VK_OEM_3),
            "Shift+` 就是 ~，不能为了换 ` 把 ~ 也吞掉"
        );
        assert!(!ok(HKF_SHIFT | HKF_NOREPEAT, VK_OEM_3));
        assert!(
            ok(HKF_SHIFT | HKF_CONTROL, VK_OEM_3),
            "Ctrl+Shift+` 不是打字键，照收"
        );
        // 其它键的裸键照旧全部拒绝：例外只有 `` ` `` 一个。
        for code in [VK_A, VK_SPACE, VK_RETURN, vk('7'), VK_OEM_3 + 1, 0x2C] {
            assert!(!ok(0, code), "裸 0x{code:02X} 不在例外里");
        }
        assert!(
            !HotkeySpec {
                mods: 0,
                vk: VK_SPACE
            }
            .backtick_bare(),
            "裸 Space 不许被认成例外"
        );
    }

    /// 补字符热键比呼出热键多两条规矩：必须带修饰键，且不能和呼出热键撞成同一组。
    #[test]
    fn typeback_needs_a_modifier_and_must_differ_from_toggle() {
        let bt = |mods, code| HotkeySpec { mods, vk: code };
        let toggle = bt(HKF_NOREPEAT, VK_OEM_3); // 裸 `（用户设的那一种）
        assert!(
            bt(HKF_WIN | HKF_NOREPEAT, VK_OEM_3)
                .check_typeback(&toggle)
                .is_ok(),
            "Win+` 是标准答案"
        );
        assert!(bt(HKF_CONTROL, VK_OEM_3).check_typeback(&toggle).is_ok());
        assert!(bt(HKF_ALT, vk('L')).check_typeback(&toggle).is_ok());
        // 裸键当补字符键 = 把"吞字符"这件事换个地方再做一遍。
        let bare = bt(0, VK_OEM_3).check_typeback(&toggle).unwrap_err();
        assert!(bare.contains("必须带修饰键"), "实得：{bare}");
        // 和呼出键同一组：系统只会把它当呼出键，补字符永远不会发生。
        // 用出厂键当呼出侧，顺带钉住"比对不看 NOREPEAT"（一个带、一个不带，仍算同一组）。
        let strong = bt(HKF_CONTROL | HKF_ALT | HKF_NOREPEAT, VK_SPACE);
        let same = bt(HKF_CONTROL | HKF_ALT, VK_SPACE)
            .check_typeback(&strong)
            .unwrap_err();
        assert!(same.contains("同一组"), "实得：{same}");
        // 先过通用判据：不在可录集合里的码，理由要来自 check()。
        let bad = bt(HKF_CONTROL, VK_BACK)
            .check_typeback(&toggle)
            .unwrap_err();
        assert!(bad.contains("只收"), "实得：{bad}");
    }

    #[test]
    fn local_conflict_table_matches_on_mods_ignoring_norepeat() {
        // 判据表里存的 mods 带不带 NOREPEAT 都必须认得出来（注册端恒带、录制端不带）。
        let table = [
            (HKF_CONTROL | HKF_NOREPEAT, vk('I')),
            (HKF_CONTROL | HKF_SHIFT, vk('D')),
        ];
        assert!(
            HotkeySpec {
                mods: HKF_CONTROL,
                vk: vk('I')
            }
            .conflicts_local(&table)
        );
        assert!(
            HotkeySpec {
                mods: HKF_CONTROL | HKF_NOREPEAT,
                vk: vk('I')
            }
            .conflicts_local(&table)
        );
        assert!(
            HotkeySpec {
                mods: HKF_CONTROL | HKF_SHIFT,
                vk: vk('D')
            }
            .conflicts_local(&table)
        );
        // Shift 是实义修饰键，不能和 Ctrl+D 混为一谈。
        assert!(
            !HotkeySpec {
                mods: HKF_CONTROL,
                vk: vk('D')
            }
            .conflicts_local(&table)
        );
        assert!(
            !HotkeySpec {
                mods: HKF_CONTROL,
                vk: vk('J')
            }
            .conflicts_local(&table)
        );
    }

    /// 批次 E3-丙：窗口内热键（次级动作键）的判据。它管的是"按下去会不会把打字吃掉"，
    /// 与全局热键那条（裸 `` ` `` 可以、单带 Shift 不行）刻意不同源 —— 见 `check_secondary` 的注释。
    ///
    /// 证伪思路：把任意一条规则从 `check_secondary` 里删掉，下面必有一条断言变红；
    /// 反方向（把 `check` 的规则误抄进来）也有断言盯着——裸 `` ` `` 在 `check` 里是合法例外，
    /// 在这里必须是**非法**的。
    #[test]
    fn secondary_hotkey_needs_ctrl_or_alt_and_avoids_local_bindings() {
        let bt = |mods, code| HotkeySpec { mods, vk: code };
        let toggle = bt(HKF_ALT | HKF_NOREPEAT, vk_f(6)); // 出厂呼出键 Alt+F6
        let typeback = bt(HKF_CONTROL | HKF_NOREPEAT, vk('B'));
        let table = [(HKF_CONTROL | HKF_NOREPEAT, vk('I'))];
        let check = |spec: HotkeySpec| spec.check_secondary(&toggle, Some(&typeback), &table);

        // 出厂值（Ctrl+Enter）本身就是这条判据的正面样本，顺带钉住"比对不看 NOREPEAT"：
        // 出厂值带 NOREPEAT，而 toggle / typeback 这里故意不带。
        assert!(check(HotkeySpec::SECONDARY_DEFAULT).is_ok());
        assert!(check(bt(HKF_CONTROL | HKF_NOREPEAT, vk('J'))).is_ok());
        assert!(
            check(bt(HKF_CONTROL | HKF_ALT | HKF_NOREPEAT, VK_RETURN)).is_ok(),
            "两枚修饰键都按着也是合法组合"
        );
        assert!(
            check(bt(HKF_CONTROL | HKF_SHIFT, VK_RETURN)).is_ok(),
            "Shift 在这里是实义位（与呼出键同一个口径：附加 Shift 不算另一组）"
        );

        // 裸键 / 单带 Shift：窗口内抢走它就等于再也打不出来。
        let bare = check(bt(HKF_NOREPEAT, VK_RETURN)).unwrap_err();
        assert!(bare.contains("Ctrl 或 Alt"), "实得：{bare}");
        let shift = check(bt(HKF_SHIFT | HKF_NOREPEAT, VK_RETURN)).unwrap_err();
        assert!(shift.contains("Ctrl 或 Alt"), "实得：{shift}");
        // 裸 ` 在全局那条里是唯一例外，在这里不行（那颗键的代价已经在全局侧付过了）。
        assert!(check(bt(0, VK_OEM_3)).is_err());

        // Win 位：egui 在 Windows 上不把 Win 传上来，设了永远匹配不到。
        let win = check(bt(HKF_WIN | HKF_CONTROL | HKF_NOREPEAT, VK_RETURN)).unwrap_err();
        assert!(win.contains("Win"), "实得：{win}");

        // 可录集合与全局那条同一套（复用一个映射，两处判据才不会各自漂）。
        let code = check(bt(HKF_CONTROL, 0x08)).unwrap_err(); // VK_BACK
        assert!(code.contains("只收"), "实得：{code}");

        // 同组冲突三连：呼出键 / 补字符键 / 本程序固定快捷键。
        let same_toggle = check(bt(HKF_ALT, vk_f(6))).unwrap_err();
        assert!(same_toggle.contains("呼出"), "实得：{same_toggle}");
        let same_typeback = check(bt(HKF_CONTROL, vk('B'))).unwrap_err();
        assert!(same_typeback.contains("补字符"), "实得：{same_typeback}");
        let local = check(bt(HKF_CONTROL, vk('I'))).unwrap_err();
        assert!(local.contains("固定快捷键"), "实得：{local}");
    }

    #[test]
    fn settings_field_defaults_when_absent_and_round_trips() {
        // 旧 settings.json 没有这个字段 ⇒ #[serde(default)] 必须兜住，不能整份解析失败。
        let old = br#"{"space_to_run":"word","tab_mode":"navigate","learn_rank":"count"}"#;
        let s: AppSettings = serde_json::from_slice(old).expect("旧文件应能解析");
        assert_eq!(s.toggle_hotkey, HotkeySpec::DEFAULT);
        // 批次 R 新字段：老文件里连 `typeback_hotkey` 这个键都没有 ⇒ 必须是"不启用"，不是崩。
        assert_eq!(
            s.typeback_hotkey, None,
            "没有补字符热键字段的老文件应读成 None"
        );
        // 批次 E3-丙：次级动作两格同理——老文件里没有这两个键 ⇒ 快捷键版 + Ctrl+Enter。
        assert_eq!(
            s.secondary_action,
            SecondaryAction::Shortcut,
            "缺字段按用户裁定的默认：快捷键版"
        );
        assert_eq!(
            s.secondary_hotkey,
            HotkeySpec::SECONDARY_DEFAULT,
            "缺字段按 FR-ACT-01 原文的 Ctrl+Enter"
        );

        let custom = AppSettings {
            toggle_hotkey: HotkeySpec {
                mods: HKF_ALT | HKF_NOREPEAT,
                vk: vk_f(6),
            },
            typeback_hotkey: Some(HotkeySpec {
                mods: HKF_WIN | HKF_NOREPEAT,
                vk: VK_OEM_3,
            }),
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&custom).unwrap();
        // 存的是裸数（十进制），不引入枚举/字符串编码。
        assert!(
            json.contains("\"toggle_hotkey\":{\"mods\":"),
            "应为数值对，实得：{json}"
        );
        assert!(
            json.contains("\"typeback_hotkey\":{\"mods\":16392,\"vk\":192}"),
            "补字符热键要写成同样的数值对（Win+` = HKF_WIN 8 | HKF_NOREPEAT 16384 = 16392，vk 192），实得：{json}"
        );
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.toggle_hotkey, custom.toggle_hotkey);
        assert_eq!(back.typeback_hotkey, custom.typeback_hotkey);
        // 次级动作形态落盘用稳定名（批次 E3-丙）。
        assert!(
            json.contains(r#""secondary_action":"shortcut""#),
            "形态要写成可读的稳定名，实得：{json}"
        );
        assert!(
            json.contains(r#""secondary_hotkey":{"mods":16386,"vk":13}"#),
            "Ctrl+Enter = HKF_CONTROL 2 | HKF_NOREPEAT 16384 = 16386，vk 13，实得：{json}"
        );
        // `null` 也必须认（用户手改文件时写"不要这个键"最自然的写法）。
        let nul = br#"{"toggle_hotkey":{"mods":3,"vk":32},"typeback_hotkey":null}"#;
        let s: AppSettings = serde_json::from_slice(nul).expect("null 应能解析");
        assert_eq!(s.typeback_hotkey, None);
    }

    /// 2026-10-24 用户需求：主界面一屏 10 行；默认按 Delphi 口径（全部结果、滚轮翻），
    /// 「10 条」是可选截断档。缺字段必须读成默认档，而两档的名字要能原样落盘再读回
    /// （设置页改完立刻写盘，重启后得还是那一档）。
    #[test]
    fn list_rows_defaults_to_all_and_round_trips_both_names() {
        let s: AppSettings = serde_json::from_str("{}").expect("空偏好应可解析");
        assert_eq!(
            s.list_rows,
            ListRows::All,
            "缺字段 / 整份缺失都按 Delphi 口径"
        );
        assert!(
            serde_json::to_string(&s)
                .unwrap()
                .contains(r#""list_rows":"all""#),
            "落盘用稳定名 all"
        );

        let ten: AppSettings =
            serde_json::from_str(r#"{"list_rows":"ten"}"#).expect("截断档应可解析");
        assert_eq!(ten.list_rows, ListRows::Ten);
        assert_eq!(ListRows::Ten.as_str(), "ten");

        // 不认识的档位名照其余档位（space_to_run / learn_rank）的口径直接报错：
        // 静默回落会让用户以为开了「全部」，实际仍是 10 条。
        assert!(serde_json::from_str::<AppSettings>(r#"{"list_rows":"everything"}"#).is_err());
    }

    #[test]
    fn row_density_defaults_to_snug_and_round_trips_four_names() {
        // 缺字段＝甲：老 settings.json（批次 F5 那批文件里根本没有这个键）升级后观感要一致，
        // 不能因为"没写"就掉到「跟随主题」那种高行档。
        let s: AppSettings = serde_json::from_str("{}").expect("空偏好应可解析");
        assert_eq!(s.row_density, RowDensity::Snug, "缺字段 / 整份缺失都按甲档");
        assert!(
            serde_json::to_string(&s)
                .unwrap()
                .contains(r#""row_density":"snug""#),
            "落盘用稳定名 snug"
        );

        // 四个稳定名逐个回读：输入照设置页真正写出去的那些串，不造花样。
        for (name, want) in [
            ("snug", RowDensity::Snug),
            ("roomy", RowDensity::Roomy),
            ("dense", RowDensity::Dense),
            ("theme", RowDensity::Theme),
        ] {
            let json = format!(r#"{{"row_density":"{name}"}}"#);
            let got: AppSettings = serde_json::from_str(&json).expect("四个档位名都应可解析");
            assert_eq!(got.row_density, want, "稳定名 {name} 读错了档");
            assert_eq!(
                want.as_str(),
                name,
                "as_str 与 serde 名不一致：日志会说出另一个档"
            );
        }

        // 与 list_rows 同口径：不认识的档位名报错，不静默回落。
        assert!(serde_json::from_str::<AppSettings>(r#"{"row_density":"tight"}"#).is_err());
    }

    /// 批次 A10：每源开关的落盘口径。`sources_off` 存名字不存枚举值 ⇒ 将来插件源的名字
    /// 写进老配置、或某个源被删掉，都不会把整份 `settings.json` 判坏（E1 那种事故的形状）。
    #[test]
    fn source_switches_round_trip_and_ignore_unknown_ids() {
        let s: AppSettings = serde_json::from_slice(br#"{"space_to_run":"word"}"#)
            .expect("没有 sources_off 字段的老文件应能解析");
        assert!(
            SourceId::ALL.iter().all(|id| s.source_on(*id)),
            "字段缺省 ⇒ 全部启用"
        );

        let mut s = s;
        assert!(s.set_source(SourceId::Files, false), "第一次关应有变化");
        assert!(
            !s.set_source(SourceId::Files, false),
            "重复关同一个源不算变化（调用方据此决定写不写盘）"
        );
        assert!(!s.source_on(SourceId::Files));
        assert!(s.source_on(SourceId::Apps), "只关一个源，别的不受影响");
        let json = serde_json::to_string(&s).expect("应能序列化");
        assert!(
            json.contains(r#""sources_off":["files"]"#),
            "实际写盘：{json}"
        );

        let mut back: AppSettings = serde_json::from_str(&json).expect("应能读回");
        assert!(!back.source_on(SourceId::Files), "关掉的源跨启动保持关闭");
        assert!(back.set_source(SourceId::Files, true), "重新打开应有变化");
        assert!(back.sources_off.is_empty());

        // 不认识的名字（将来的插件源 / 已经删掉的源）忽略，不参与任何判据。
        let unknown: AppSettings =
            serde_json::from_str(r#"{"sources_off":["plugin-x"]}"#).expect("未知 id 不许判坏配置");
        assert!(SourceId::ALL.iter().all(|id| unknown.source_on(*id)));
    }

    /// 这个码本批写错过一次（我先填的是 1410），所以它必须由**两条独立来源**钉住，而不是照抄自己的记忆。
    #[test]
    fn already_registered_code_is_the_measured_hresult_low_word() {
        // 来源一：windows-0.62.2 绑定源 `Foundation/mod.rs:2677`。
        assert_eq!(ERROR_HOTKEY_ALREADY_REGISTERED, 1409);
        // 来源二：需求书附录 E E-01 在真机实测到的 `RegisterHotKey` 失败 HRESULT。
        assert_eq!(
            ERROR_HOTKEY_ALREADY_REGISTERED,
            0x8007_0581 & 0xFFFF,
            "低字就是 Win32 码"
        );
        // 1410 是 ERROR_CLASS_ALREADY_EXISTS：写错的后果不是崩，而是"已被系统或其它程序占用"
        // 那句提示永远不显示，退化成裸错误码——静默的错，所以这条断言留着。
        assert_ne!(ERROR_HOTKEY_ALREADY_REGISTERED, 1410);
    }
}

/// 批次 E2（FR-CFG-01 剩下的那一半：版本戳、逐版迁移、字段级容错）的判据测试。
/// 全走 [`load_settings_at`] / [`load_user_items_at`] 的路径参数化版本 ⇒ 一条都不碰用户真实
/// 数据目录（便携判定会把 `settings_path()` 指到 exe 旁边，测试不许写那儿）。
/// 【2026-10-26 批次 C3：`search_history.json` 的读写也归在这一节——同一个 `temp_dir` 夹具、
/// 同一套"纯读不动盘 / 坏了先留证"的不变量，另起一个模块只会多造一份 helper。】
#[cfg(test)]
mod config_tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("egui-launcher-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(p: &Path, text: &str) {
        std::fs::write(p, text.as_bytes()).expect("临时目录写不了");
    }

    /// 让一个目录"读得进、写不出"——`Corrupted::Lost` 的唯一入口就靠这个造出来。
    /// 用 SID `*S-1-1-0` 而不是 `Everyone`：中文系统上组名是本地化的，`icacls` 只认英文名或 SID。
    /// deny 的是 `WD`（写数据/新增数据）、`AD`（追加/建子项）、`DE`（删除）：
    /// 少了 `DE` 就还能改名（改名要的是文件的 DELETE 权限），少了 `WD/AD` 就还能另存副本，
    /// 三样缺一样都到不了 `Lost`。
    struct ReadOnlyDir {
        dir: PathBuf,
    }

    impl ReadOnlyDir {
        fn apply(dir: &Path) -> Option<Self> {
            let out = std::process::Command::new("icacls")
                .arg(dir)
                .arg("/deny")
                .arg("*S-1-1-0:(OI)(CI)(WD,AD,DE)")
                .output()
                .ok()?;
            out.status.success().then(|| Self {
                dir: dir.to_path_buf(),
            })
        }
    }

    impl Drop for ReadOnlyDir {
        fn drop(&mut self) {
            let _ = std::process::Command::new("icacls")
                .arg(&self.dir)
                .arg("/remove:d")
                .arg("*S-1-1-0")
                .output();
        }
    }

    /// 证伪实验①：某一格类型写错 ⇒ **只兜那一格**，同文件其余字段照读，且不产生 `.bad`。
    /// 判据故意选 `confirm_actions`（出厂默认 `true`）：如果我实际上还在走整份 `from_slice`，
    /// 那一格会读成 `true`（＝整份回默认），这条当场红——"整份兜默认"与"单格兜默认"分得开。
    #[test]
    fn a_wrong_typed_field_costs_only_that_field_not_the_whole_file() {
        let dir = temp_dir("e2-one-field");
        let p = dir.join("settings.json");
        write(
            &p,
            r#"{
              "schema_version": 1,
              "theme": [1, 2],
              "confirm_actions": false,
              "terminal_shell": "powershell",
              "sources_off": ["files"]
            }"#,
        );
        let (s, fault) = load_settings_at(&p);
        assert!(fault.is_none(), "单格类型不符不该把整份文件判坏：{fault:?}");
        assert_eq!(s.theme, default_theme(), "类型不符那一格该回默认");
        assert!(!s.confirm_actions, "同文件的布尔必须照读（读到 true 就是整份回默认了）");
        assert_eq!(s.terminal_shell, "powershell");
        assert_eq!(s.sources_off, vec!["files".to_string()]);
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().contains(".bad")),
            "能读出来的配置不该被留证成 .bad"
        );
        assert!(
            std::fs::read_to_string(&p)
                .unwrap()
                .contains("[1, 2]"),
            "纯读不许回写：文件原样留着，用户看得见自己写错的那一格"
        );
    }

    /// 逐格点名断言，而不是 `assert_eq!(got, x)`：漏写一行 `take_field!` 时，
    /// 报错必须说得出是哪一格漏了（整结构体比较只给一句"不相等"）。
    /// 这一条同时兜住两个漂移：加字段忘了进表、以及给容器加 `rename_all` 让表里的键名整体失效。
    #[test]
    fn every_settings_field_is_read_from_a_full_file() {
        let mut x = AppSettings::default();
        x.space_to_run = SpaceRun::Unique;
        x.tab_mode = TabMode::Navigate;
        x.learn_rank = LearnRank::Decay;
        x.toggle_hotkey = HotkeySpec {
            mods: HKF_ALT | HKF_WIN,
            vk: VK_SPACE,
        };
        x.typeback_hotkey = Some(HotkeySpec::DEFAULT);
        // 批次 E3 加的两格当时只进了 `settings_field_defaults_when_absent_and_round_trips`，
        // 而那条走的是 serde 派生（`from_slice`），**不经过 `take_field!` 这张表**——
        // 表里漏一行时的症状是"这两格永远回默认"，那条测试一点感觉都没有。
        // 【2026-10-26 批次 C3 补进本条：这是真读盘路径，漏行才报得出来。】
        x.secondary_action = SecondaryAction::Panel;
        x.secondary_hotkey = HotkeySpec {
            mods: HKF_CONTROL | HKF_ALT | HKF_NOREPEAT,
            vk: 0x78, // F9：录制端不会拒的组合，免得测试值本身是个反例
        };
        x.web_engines.truncate(2);
        x.seed_round = 7;
        x.sources_off = vec!["files".to_string(), "system".to_string()];
        x.hidden = vec!["custom:command|a.exe|".to_string()];
        x.hidden_stash = vec![LaunchItem {
            name: "藏起来的".to_string(),
            path: "C:\\x.exe".to_string(),
            ..Default::default()
        }];
        x.confirm_actions = false;
        x.terminal_shell = "pwsh".to_string();
        x.theme = "Dark".to_string();
        x.list_rows = ListRows::Ten;
        x.row_density = RowDensity::Roomy;
        x.default_scope = Scope::Basic;
        x.history_size = 7;
        // 批次 D7 的六格：同样要"特值进、特值出"，缺一行 take_field! 就得在这条上红。
        x.file_folders = vec!["C:\\probe-scope".to_string()];
        x.file_depth = 6;
        x.file_exclude = vec!["probe-skip".to_string()];
        x.file_max_entries = 100_000;
        x.file_refresh_minutes = 15;
        x.file_min_query_len = 3;
        x.appearance = Appearance {
            width: 720.0,
            status_bar: false,
            fade: true,
            ..Default::default()
        };

        let dir = temp_dir("e2-full-file");
        let p = dir.join("settings.json");
        std::fs::write(&p, &serde_json::to_vec_pretty(&x).unwrap()).unwrap();
        let (got, fault) = load_settings_at(&p);
        assert!(fault.is_none());
        assert_eq!(got.schema_version, SETTINGS_SCHEMA_VERSION, "schema_version");
        assert_eq!(got.space_to_run, SpaceRun::Unique, "space_to_run");
        assert_eq!(got.tab_mode, TabMode::Navigate, "tab_mode");
        assert_eq!(got.learn_rank, LearnRank::Decay, "learn_rank");
        assert_eq!(got.toggle_hotkey, HotkeySpec { mods: HKF_ALT | HKF_WIN, vk: VK_SPACE }, "toggle_hotkey");
        assert_eq!(got.typeback_hotkey, Some(HotkeySpec::DEFAULT), "typeback_hotkey");
        assert_eq!(got.secondary_action, SecondaryAction::Panel, "secondary_action");
        assert_eq!(
            got.secondary_hotkey,
            HotkeySpec {
                mods: HKF_CONTROL | HKF_ALT | HKF_NOREPEAT,
                vk: 0x78
            },
            "secondary_hotkey"
        );
        assert_eq!(got.web_engines.len(), 2, "web_engines");
        assert_eq!(got.seed_round, 7, "seed_round");
        assert_eq!(got.sources_off.len(), 2, "sources_off");
        assert_eq!(got.hidden.len(), 1, "hidden");
        assert_eq!(got.hidden_stash.len(), 1, "hidden_stash");
        assert_eq!(got.hidden_stash[0].name, "藏起来的", "hidden_stash 里的 name");
        assert!(!got.confirm_actions, "confirm_actions");
        assert_eq!(got.terminal_shell, "pwsh", "terminal_shell");
        assert_eq!(got.theme, "Dark", "theme");
        assert_eq!(got.list_rows, ListRows::Ten, "list_rows");
        assert_eq!(got.row_density, RowDensity::Roomy, "row_density");
        assert_eq!(got.default_scope, Scope::Basic, "default_scope");
        assert_eq!(got.appearance.width, 720.0, "appearance 递归合并：width");
        assert_eq!(got.appearance.status_bar, false, "appearance.status_bar");
        assert_eq!(got.appearance.fade, true, "appearance.fade");
        assert_eq!(got.history_size, 7, "history_size");
        assert_eq!(got.history_limit(), 7, "history_limit：配置值小于上限时原样");
        assert_eq!(
            got.file_folders,
            vec!["C:\\probe-scope".to_string()],
            "file_folders"
        );
        assert_eq!(got.file_depth, 6, "file_depth");
        assert_eq!(got.file_exclude, vec!["probe-skip".to_string()], "file_exclude");
        assert_eq!(got.file_max_entries, 100_000, "file_max_entries");
        assert_eq!(got.file_refresh_minutes, 15, "file_refresh_minutes");
        assert_eq!(got.file_min_query_len, 3, "file_min_query_len");
        assert_eq!(got.file_scan_scope().depth, 6, "file_scan_scope 汇编");
        assert_eq!(got.file_scan_scope().refresh.as_secs(), 15 * 60);
    }

    /// 批次 D7：六格 → [`filesearch::ScanScope`] 的汇编与夹子。缺省零漂移
    /// （＝老配置没有这几格时行为与 D7 之前一字不差），垃圾值夹回有效区间。
    #[test]
    fn file_scope_fields_assemble_into_a_sanitized_scan_scope() {
        let s = AppSettings::default();
        assert_eq!(
            s.file_scan_scope(),
            crate::source::filesearch::ScanScope::default(),
            "缺省六格与数据源缺省范围零漂移"
        );
        let mut bad = AppSettings::default();
        bad.file_depth = 0;
        bad.file_max_entries = 0;
        bad.file_refresh_minutes = 0;
        bad.file_min_query_len = 0;
        let g = bad.file_scan_scope();
        assert_eq!(g.depth, 1, "零深度夹到最小有效档");
        assert_eq!(g.max_entries, 1_000);
        assert_eq!(g.refresh, std::time::Duration::from_secs(60));
        assert_eq!(g.min_query_len, 1);
    }

    /// 「缺失项补默认」＋批次 J 那次的老写法：`space_to_run` 曾是布尔。
    /// 缺版本戳的文件按 v0 读 ⇒ 这里用 `schema_version: 1` 把迁移那条路隔开，两条判据别混。
    #[test]
    fn absent_fields_fall_back_and_the_legacy_bool_still_reads() {
        let dir = temp_dir("e2-absent");
        let p = dir.join("settings.json");
        write(&p, r#"{"schema_version": 1, "space_to_run": true}"#);
        let (s, fault) = load_settings_at(&p);
        assert!(fault.is_none());
        assert_eq!(s.space_to_run, SpaceRun::Always, "老文件 true＝始终执行（批次 J 的兼容口径）");
        assert_eq!(s.theme, default_theme());
        assert_eq!(
            s.web_engines.len(),
            crate::source::websearch::builtin_engines().len(),
            "缺 web_engines 要回内置那几条，不是空表（批次 X 真机 A6 的根因）"
        );
    }

    /// 「数组整体替换、不做元素级合并」：文件里写两条引擎就是两条，
    /// 不许跟内置那几条并起来；这一格默认是 9 条，所以"少了几条"看得见。
    #[test]
    fn arrays_replace_wholesale_instead_of_merging() {
        let dir = temp_dir("e2-array-replace");
        let p = dir.join("settings.json");
        let full = crate::source::websearch::builtin_engines().len();
        let mut x = AppSettings::default();
        x.web_engines.truncate(2);
        std::fs::write(&p, &serde_json::to_vec_pretty(&x).unwrap()).unwrap();
        let (got, _) = load_settings_at(&p);
        assert_eq!(got.web_engines.len(), 2, "该是文件里的两条");
        assert_ne!(got.web_engines.len(), full, "不许与内置并成 {full} 条");
    }

    /// 未知键读时忽略（不判坏）。代价明写并要求测试证实：任何一次真实写盘都会把它丢掉
    /// ⇒ "用旧版本程序打开新版本写出的文件"会丢新键。当前只有一个版本，触发不了。
    #[test]
    fn unknown_keys_are_ignored_on_read_and_dropped_on_write() {
        let dir = temp_dir("e2-unknown");
        let p = dir.join("settings.json");
        write(&p, r#"{"schema_version": 1, "theme": "Dark", "future_switch": {"a": 1}}"#);
        let (got, fault) = load_settings_at(&p);
        assert!(fault.is_none(), "多一个不认识的键不该判坏");
        assert_eq!(got.theme, "Dark");
        save_settings_at(&p, &got);
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(!after.contains("future_switch"), "这一条把'未知键会被写盘丢掉'钉成明写的事实");
    }

    /// v0 → v1：迁移前留的是**原始字节**，迁完才打戳；第二次启动不再产生第二份备份。
    #[test]
    fn a_v0_file_is_backed_up_then_stamped_once() {
        let dir = temp_dir("e2-migrate");
        let p = dir.join("settings.json");
        let raw = r#"{"theme": "Dark", "terminal_shell": "pwsh"}"#;
        write(&p, raw);
        let (s, fault) = load_settings_at(&p);
        assert!(fault.is_none());
        assert_eq!(s.schema_version, SETTINGS_SCHEMA_VERSION, "迁完该是当前版本");
        assert_eq!(s.theme, "Dark", "迁移不许丢用户已写下的值");
        let bak = dir.join("settings.v0.backup.json");
        assert_eq!(
            std::fs::read(&bak).unwrap(),
            raw.as_bytes(),
            "备份必须是原始字节，不是重新序列化的一份"
        );
        let on_disk = std::fs::read_to_string(&p).unwrap();
        assert!(on_disk.contains("\"schema_version\": 1"), "版本戳要落盘");
        let (again, _) = load_settings_at(&p);
        assert_eq!(again.schema_version, SETTINGS_SCHEMA_VERSION);
        assert!(!dir.join("settings.v0.backup-1.json").exists(), "已经 v1 就不该再迁一次");
    }

    /// 证伪实验②：备份名额全被占 ⇒ **留不到证就不迁移、也不写盘**，但内容照样读得出来。
    /// 占名用目录：`exists()` 为真就跳过，写同名的文件也必然失败——两条都踩到。
    #[test]
    fn no_backup_means_no_migration() {
        let dir = temp_dir("e2-no-backup");
        for n in 0..MIGRATE_BACKUP_SLOTS {
            let candidate = if n == 0 {
                "settings.v0.backup.json".to_string()
            } else {
                format!("settings.v0.backup-{n}.json")
            };
            std::fs::create_dir(dir.join(candidate)).expect("占名额失败");
        }
        let p = dir.join("settings.json");
        let raw = r#"{"theme": "Dark"}"#;
        write(&p, raw);
        let (s, fault) = load_settings_at(&p);
        assert!(fault.is_none(), "不迁移不等于配置坏了");
        assert_eq!(s.schema_version, 0, "没迁移成功就不能自称已经迁过");
        assert_eq!(s.theme, "Dark", "不迁移也要按已知字段读出内容");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), raw, "原文件一字不许动");
    }

    /// 版本比本程序新 ⇒ 按认识的字段读、不改它的版本戳、不回写。
    /// 反例是本程序把 v9 盖成 v1：那等于把"这份文件已经迁过"的谎写进磁盘。
    #[test]
    fn a_newer_version_is_read_but_never_rewritten() {
        let dir = temp_dir("e2-newer");
        let p = dir.join("settings.json");
        let raw = r#"{"schema_version": 9, "theme": "Dark", "only_future_knows": true}"#;
        write(&p, raw);
        let (s, fault) = load_settings_at(&p);
        assert!(fault.is_none());
        assert_eq!(s.schema_version, 9, "读到几就是几，不许降回本程序的版本");
        assert_eq!(s.theme, "Dark");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), raw, "比本程序新的文件一字不改");
    }

    /// 清单侧最实在的一条：某一条的某一格类型不对 ⇒ 只兜那一格，其余条目一字不动。
    /// 输入照上游真实形态构造（`builtin_seeds()` 序列化后改一格），不手写条目形状。
    #[test]
    fn one_bad_field_in_one_item_does_not_take_the_whole_list_down() {
        let dir = temp_dir("e2-item-field");
        let p = dir.join("user_items.json");
        let mut tree: Vec<serde_json::Value> = builtin_seeds()
            .into_iter()
            .take(3)
            .map(|it| serde_json::to_value(&it).unwrap())
            .collect();
        tree[0]["use_count"] = serde_json::json!(5);
        tree[1]["use_count"] = serde_json::json!("abc");
        std::fs::write(&p, &serde_json::to_vec_pretty(&tree).unwrap()).unwrap();
        let (got, fault) = load_user_items_at(&p);
        assert!(fault.is_none(), "一条里的一格不该把整份清单送进 .bad");
        assert_eq!(got.len(), 3, "三条都要留住");
        assert_eq!(got[0].use_count, 5, "好读的那条照读");
        assert_eq!(got[1].use_count, 0, "类型不符那一格回默认");
        assert_eq!(got[1].name, builtin_seeds()[1].name, "同一条的其余字段不许被牵连");
        assert_eq!(got[2].use_count, 0);
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().contains(".bad")),
            "读得出来的清单不该留证"
        );
    }

    /// 不是对象的元素不认作条目（丢掉 + 记下标），**不造一条空 name 空 path 的假条目**：
    /// 那种条目会带空 uid 进清单、与 `dedup_items` 和 `uid()` 的规则打架，比少一条更难查。
    #[test]
    fn a_non_object_element_is_dropped_rather_than_turned_into_a_blank_item() {
        let dir = temp_dir("e2-item-shape");
        let p = dir.join("user_items.json");
        let seeds = builtin_seeds();
        let mut tree: Vec<serde_json::Value> = vec![
            serde_json::to_value(&seeds[0]).unwrap(),
            serde_json::Value::String("我不是条目".to_string()),
            serde_json::to_value(&seeds[1]).unwrap(),
        ];
        tree[2]["name"] = serde_json::json!("改过名的");
        std::fs::write(&p, &serde_json::to_vec_pretty(&tree).unwrap()).unwrap();
        let (got, fault) = load_user_items_at(&p);
        assert!(fault.is_none());
        assert_eq!(got.len(), 2, "非对象那一格不算条目");
        assert_eq!(got[1].name, "改过名的", "被丢的是中间那条，前后两条该照位留下");
        assert!(got.iter().all(|i| !i.name.is_empty()), "不许出现空名假条目");
    }

    /// `name`/`path` 是条目唯一的必需格（E2 之前 serde 就要求这两个键存在）。逐字段容错把
    /// "缺键"从整份解析失败降成"这一条不认" ⇒ 少了名字的条目不许带空 uid 混进清单，
    /// 而**同文件其余条目照读、且不留证**（这半边是逐条容错的本职）。
    #[test]
    fn an_item_missing_a_name_or_a_path_is_not_an_item() {
        let dir = temp_dir("e2-item-required");
        let p = dir.join("user_items.json");
        let seeds = builtin_seeds();
        let mut no_name = serde_json::to_value(&seeds[0]).unwrap();
        no_name.as_object_mut().unwrap().remove("name");
        let mut no_path = serde_json::to_value(&seeds[1]).unwrap();
        no_path.as_object_mut().unwrap().remove("path");
        let tree = vec![
            no_name,
            no_path,
            serde_json::to_value(&seeds[2]).unwrap(),
        ];
        std::fs::write(&p, &serde_json::to_vec_pretty(&tree).unwrap()).unwrap();
        let (got, fault) = load_user_items_at(&p);
        assert!(fault.is_none(), "还有条目读得出来就不该把整份判坏");
        assert_eq!(got.len(), 1, "缺必需格的两条不认作条目");
        assert_eq!(got[0].name, seeds[2].name, "第三条该照位留下");
        assert!(
            got.iter().all(|i| !i.name.is_empty() && !i.path.is_empty()),
            "不许出现空名或空路径的条目（uid 会派生成 `custom:||`）"
        );
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().contains(".bad")),
            "读得出来的清单不该留证"
        );
    }

    /// 判据①的边界：数组非空却**一条都不成形** ⇒ 逐条容错会把它读成"空清单"，而调用方
    /// 紧接着按空清单播种写回 ⇒ 唯一现场被覆盖（批次 X 真机⑫同一形状的事故，换了实现不许破）。
    /// 所以这一格整体不认、走留证；对照格是 `[]`——那是用户真删光了，干净、不留证、不动盘。
    #[test]
    fn a_list_where_nothing_survives_is_quarantined_not_seeded_over() {
        let dir = temp_dir("e2-total-loss");
        let p = dir.join("user_items.json");
        write(&p, "[1, 2]");
        let (got, fault) = load_user_items_at(&p);
        assert!(got.is_empty());
        let fault = fault.expect("一条都不成形必须报下场，不许静默成空清单");
        let Corrupted::Kept(kept) = &fault else {
            panic!("第一次留证该成功，实得 {fault:?}")
        };
        assert!(kept.exists() && !p.exists(), "证据在 {kept:?}，原位置要腾出来");

        let ok = dir.join("clean.json");
        write(&ok, "[]");
        let (none, fault) = load_user_items_at(&ok);
        assert!(none.is_empty());
        assert!(fault.is_none(), "空数组＝用户删光了，不是坏");
        assert!(ok.exists(), "干净的空清单不许被改名");
    }

    /// FR-CFG-02 没退化：顶层形状不对（偏好成了数组 / 清单成了对象）与语法坏同一条路——
    /// 改名留证、按默认值启动、**不写盘**（写盘会覆盖唯一证据）。
    #[test]
    fn a_wrong_top_level_shape_still_quarantines() {
        let dir = temp_dir("e2-shape-fault");
        let s = dir.join("settings.json");
        write(&s, r#"[1, 2, 3]"#);
        let (prefs, fault) = load_settings_at(&s);
        let kept_fault = fault.expect("形状不对必须报下场");
        let Corrupted::Kept(kept) = &kept_fault else {
            panic!("第一次留证该成功，实得 {kept_fault:?}")
        };
        assert!(kept.exists() && !s.exists(), "证据在 {kept:?}，原位置要腾出来");
        assert_eq!(prefs.theme, default_theme(), "留证之后按默认值启动");
        assert_eq!(prefs.schema_version, SETTINGS_SCHEMA_VERSION, "默认值就是当前版本");

        let l = dir.join("user_items.json");
        write(&l, r#"{"not": "an array"}"#);
        let (items, fault2) = load_user_items_at(&l);
        assert!(matches!(fault2, Some(Corrupted::Kept(_))), "实得 {fault2:?}");
        assert!(items.is_empty());
        assert!(!l.exists(), "清单也该被改名留证");
    }

    /// 真·语法坏仍是老那条路（这一条是回归守卫：新增的 `Value` 中间层不许把语法错吞掉）。
    #[test]
    fn invalid_json_still_quarantines_through_the_value_layer() {
        let dir = temp_dir("e2-syntax");
        let p = dir.join("settings.json");
        write(&p, "{ 坏 JSON");
        let (prefs, fault) = load_settings_at(&p);
        assert!(matches!(fault, Some(Corrupted::Kept(_))), "实得 {fault:?}");
        assert_eq!(prefs.confirm_actions, true, "默认值启动");
    }

    /// 批次 E3：`Corrupted::Lost` 在补这条之前**没有任何一条路径走到过**——唯一提到它的是
    /// `a_real_list_file_is_an_array_not_a_two_tuple` 里那句"Lost 不该发生"的 panic 断言，
    /// 也就是说这个分支的正确性全靠读代码。这里把"改名与另存都失败"真造出来：
    /// 目录 deny 写入（读照常）⇒ 10 个 `.bad` 名额与毫秒时间戳两格都写不出去 ⇒ 必须报 `Lost`，
    /// 并且**原文件一个字节都不许被碰**——那正是 `Lost` 存在的全部理由。
    #[test]
    fn quarantine_reports_lost_when_the_directory_refuses_writes() {
        let dir = temp_dir("e3-lost");
        let p = dir.join("user_items.json");
        let original = b"[1,2]".to_vec();
        std::fs::write(&p, &original).expect("临时目录写不了");

        let guard = ReadOnlyDir::apply(&dir)
            .expect("icacls 加 deny 失败 ⇒ 这条测不到 Lost，不许算通过");
        // 控制实验：deny 真生效的判据是"新建文件失败"。没生效就说明这台机器上
        // 构造根本没做出来，此时往下跑只会得到一个与 Lost 无关的结果。
        assert!(
            std::fs::write(dir.join("acl-control.txt"), b"x").is_err(),
            "目录仍可新建文件 ⇒ deny 没生效，这条测试的结论不算数"
        );

        let outcome = quarantine(&p, &original);
        assert_eq!(outcome, Corrupted::Lost, "两条留证路都写不出时必须报 Lost");
        assert_eq!(
            std::fs::read(&p).expect("原文件该还在"),
            original,
            "Lost 场景下原文件必须原样留着（这正是调用方不许写盘的理由）"
        );
        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .expect("读不动目录")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "user_items.json")
            .collect();
        assert!(
            leftovers.is_empty(),
            "两路都没留成证，就不该有 .bad / 副本之类的残留，实得 {leftovers:?}"
        );
        drop(guard);
    }

    /// 「无 BOM」是需求书明写的：写盘首三字节不许是 EF BB BF。
    #[test]
    fn settings_json_is_written_without_a_bom() {
        let dir = temp_dir("e2-bom");
        let p = dir.join("settings.json");
        save_settings_at(&p, &AppSettings::default());
        let bytes = std::fs::read(&p).unwrap();
        assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "首三字节不该是 UTF-8 BOM");
        let (got, fault) = load_settings_at(&p);
        assert!(fault.is_none());
        assert_eq!(got.schema_version, SETTINGS_SCHEMA_VERSION);
    }

    /// 批次 C3 真机补跑（2026-10-27）抓到的一条：整份解析失败那条报文会把出问题那格的**内容**
    /// 一起打进 `launcher.log`（探针串 `"C3隐私探针ABC"` 原样出现过）。形态与行列号要留着——
    /// 将来还得靠它判坏形；只把双引号里那段抹掉。
    #[test]
    fn serde_shape_blanks_quoted_values_but_keeps_the_shape() {
        let s = r#"invalid type: string "C3隐私探针ABC", expected a sequence at line 1 column 19"#;
        assert_eq!(
            serde_shape(s),
            r#"invalid type: string "", expected a sequence at line 1 column 19"#
        );
        // 串里带转义引号：`\"` 不许提前关掉引号区，否则后半截 `b"` 会留在明面上。
        let esc = r#"invalid type: string "a\"b", expected a sequence at line 1 column 8"#;
        assert_eq!(
            serde_shape(esc),
            r#"invalid type: string "", expected a sequence at line 1 column 8"#
        );
        // 数字 / map 这类坏形原文里没有双引号 ⇒ 一字不改（行列号照留）。
        let num = "invalid type: integer `1`, expected a string at line 1 column 2";
        assert_eq!(serde_shape(num), num);
    }

    /// 批次 C3（FR-QRY-10）：记一条历史的四条规矩，每条都对应一次"用户看得出来的别扭"。
    #[test]
    fn push_history_moves_the_newest_to_the_front_and_caps_the_tail() {
        let mut h: Vec<String> = Vec::new();
        assert!(!push_history(&mut h, "", 30), "空查询不记（AHK 同口径）");
        assert!(!push_history(&mut h, "   ", 30), "只有空白也不记");
        assert!(h.is_empty());

        assert!(push_history(&mut h, "cmd", 30));
        assert!(push_history(&mut h, "notepad", 30));
        assert_eq!(h, vec!["notepad", "cmd"], "最新在前");

        assert!(push_history(&mut h, "cmd", 30));
        assert_eq!(h, vec!["cmd", "notepad"], "再敲一次是整条挪到最前，不是后面多一条");

        assert!(push_history(&mut h, " CMD ", 30));
        assert_eq!(
            h,
            vec!["CMD", "notepad"],
            "去重不分大小写，但存的是这次敲的原样（不学 AHK 一律小写）、首尾空白剥掉"
        );

        assert!(!push_history(&mut h, "CMD", 30), "位置与内容都没动 ⇒ 报没变，调用方就不写盘");
        assert_eq!(h.len(), 2);

        let mut full = vec!["3".to_string(), "2".to_string(), "1".to_string()];
        assert!(push_history(&mut full, "4", 3));
        assert_eq!(full, vec!["4", "3", "2"], "超上限从尾部丢，不是一律清空");

        let mut off = vec!["x".to_string()];
        assert!(push_history(&mut off, "y", 0), "0 档把已有的历史裁干净 ⇒ 算变化，要落盘");
        assert!(off.is_empty());
        let mut off2: Vec<String> = Vec::new();
        assert!(
            !push_history(&mut off2, "y", 0),
            "本来就没东西时 0 档不许产生一次写盘（不记＝不碰盘）"
        );
    }

    /// 批次 C3：`Ctrl+↑↓` 的走位。判据照 AHK `RecallHistory`，四条各自钉死一条边界。
    #[test]
    fn recall_history_walks_back_and_returns_the_draft() {
        let h = vec!["c".to_string(), "b".to_string(), "a".to_string()];

        let mut untouched = HistoryBrowse::default();
        assert_eq!(recall_history(&[], "", &mut untouched, 1), None, "没历史时不许动");
        assert_eq!(untouched, HistoryBrowse::default(), "不动就意味着游标一格都没改");

        let mut br = HistoryBrowse::default();
        assert_eq!(recall_history(&h, "", &mut br, 1).as_deref(), Some("c"));
        assert_eq!(br.index, 1);
        assert_eq!(br.draft, "", "起步那一下把输入框原文存成草稿");
        assert_eq!(recall_history(&h, "c", &mut br, 1).as_deref(), Some("b"));
        assert_eq!(recall_history(&h, "b", &mut br, 1).as_deref(), Some("a"));
        assert_eq!(recall_history(&h, "a", &mut br, 1), None, "已经是最早一条");
        assert_eq!(br.index, 3, "到头那一下不许把游标甩回 0（甩了就再也不能往回翻）");

        assert_eq!(recall_history(&h, "a", &mut br, -1).as_deref(), Some("b"));
        assert_eq!(recall_history(&h, "b", &mut br, -1).as_deref(), Some("c"));
        assert_eq!(br.index, 1);

        // 用户自己敲了字再翻：翻到底必须把那两个字还回去，否则"手敲的被历史吃了"。
        let mut b2 = HistoryBrowse::default();
        assert_eq!(recall_history(&h, "zz", &mut b2, 1).as_deref(), Some("c"));
        assert_eq!(b2.draft, "zz");
        assert_eq!(recall_history(&h, "c", &mut b2, -1).as_deref(), Some("zz"));
        assert_eq!(b2.index, 0, "回到草稿＝退出翻历史");
        assert_eq!(recall_history(&h, "zz", &mut b2, -1), None, "已经在草稿上，再按不动");
    }

    /// 批次 C3：一次按键必须看得见变化（AHK 那个 `while` 循环存在的唯一理由）。
    #[test]
    fn recall_history_skips_the_entry_that_is_already_typed() {
        let h = vec!["notepad".to_string(), "cmd".to_string()];
        let mut br = HistoryBrowse::default();
        assert_eq!(
            recall_history(&h, "notepad", &mut br, 1).as_deref(),
            Some("cmd"),
            "用户刚敲的正好等于最新一条 ⇒ 回填它等于没回填，要再往下一条"
        );
        assert_eq!(br.index, 2, "跳格也要把游标落在真正那一条上");

        // 草稿那一格是例外：翻回 0 就是退出翻历史，内容与当前相同也照样回去。
        let mut b = HistoryBrowse { index: 1, draft: "notepad".to_string() };
        assert_eq!(recall_history(&h, "notepad", &mut b, -1).as_deref(), Some("notepad"));
        assert_eq!(b.index, 0);
    }

    /// 批次 C3：`search_history.json` 的读写。两条 E2 不变量在这儿同样成立——
    /// **纯读不动盘**、**坏文件先留证再按空表启动**。
    #[test]
    fn history_file_starts_empty_without_touching_disk_and_round_trips() {
        let dir = temp_dir("c3-file");
        let p = dir.join("search_history.json");

        let (h, fault) = load_history_at(&p);
        assert!(h.is_empty() && fault.is_none(), "首次运行＝空表且没有事故");
        assert!(!p.exists(), "纯读不许造出一个空文件");

        save_history_at(&p, &["cmd".to_string(), "notepad".to_string()]);
        let (back, fault2) = load_history_at(&p);
        assert!(fault2.is_none());
        assert_eq!(back, vec!["cmd", "notepad"], "顺序原样回来（最新在前）");
        assert!(
            !std::fs::read(&p).unwrap().starts_with(&[0xEF, 0xBB, 0xBF]),
            "无 BOM 与偏好同口径"
        );

        save_history_at(&p, &[]);
        let (cleared, fault3) = load_history_at(&p);
        assert!(fault3.is_none());
        assert!(cleared.is_empty(), "空表是合法内容，不是坏文件（与清单 `[]` 同一判据）");
    }

    #[test]
    fn a_broken_history_file_quarantines_instead_of_being_written_over() {
        let dir = temp_dir("c3-bad");
        let p = dir.join("search_history.json");
        write(&p, "[1, 2]");
        let (h, fault) = load_history_at(&p);
        assert!(h.is_empty());
        let Some(Corrupted::Kept(kept)) = &fault else {
            panic!("第一次留证该成功，实得 {fault:?}")
        };
        assert!(kept.exists() && !p.exists(), "证据在 {kept:?}，原位置要腾出来");
        assert_eq!(
            std::fs::read(kept).unwrap(),
            b"[1, 2]",
            "留的是原文，不是我们重排过的东西"
        );

        write(&p, r#"{"a": 1}"#);
        assert!(
            matches!(load_history_at(&p).1, Some(Corrupted::Kept(_))),
            "顶层不是数组同样判坏（历史与清单、偏好共用这一条判据）"
        );
    }

    /// 批次 C3：`history_size` 这一格自身的容错，与"读盘不改写、用的时候才夹"。
    #[test]
    fn history_size_absent_is_30_and_a_huge_one_is_clamped_at_use() {
        let dir = temp_dir("c3-size");
        let p = dir.join("settings.json");

        write(&p, r#"{"schema_version": 1}"#);
        let (s, fault) = load_settings_at(&p);
        assert!(fault.is_none());
        assert_eq!(s.history_size, HISTORY_SIZE_DEFAULT, "老文件没这一格 ⇒ 30");

        write(&p, r#"{"schema_version": 1, "history_size": 9999}"#);
        let (s, _) = load_settings_at(&p);
        assert_eq!(s.history_size, 9999, "读盘不回写用户那一格（FR-CFG-01 纯读不动盘）");
        assert_eq!(s.history_limit(), HISTORY_SIZE_MAX, "取用的时候才夹，上限 200");

        write(&p, r#"{"schema_version": 1, "history_size": "很多"}"#);
        let (s, _) = load_settings_at(&p);
        assert_eq!(s.history_size, HISTORY_SIZE_DEFAULT, "类型不符只兜这一格");
    }
}


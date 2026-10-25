//! FR-ACT-04 动作集（批次 P1，2026-10-25）。
//!
//! 本文件只做三件事：**一条被选中的东西是什么**（[`Subject`]）、**它能做哪些动作**
//! （[`actions_for`]）、**面板里输入即过滤**（[`filter_actions`]）。全部纯函数——不碰 egui、
//! 不碰 Win32，所以每条判据都能在 `cargo test` 里钉住。真正执行动作的 Win32 包装在 `main.rs`。
//!
//! 口径来源：需求书 FR-ACT-03/04 的条件句（「按 `kind` 与可编辑性提供」「`shell:` 项跳过、
//! Store 应用不显示」「磁盘根目录不能删」），与 AHK 版 `Src/Core/ActionCatalog.ahk:51-111`
//! 的 `ListFor()` 逐档对照。有意不同两处：① AHK 的 `file` 类另有「复制文件 / 剪切文件 /
//! 复制到・移动到文件管理器当前目录」，依赖剪贴板 `CF_HDROP` + `DropEffect`，本批未做；
//! ② AHK 的应用条目（`default` 分支）只有「运行」一条，本实现另给「复制路径 / 复制名称 /
//! 以管理员运行」——路径本来就是 exe，藏起来没有收益。

use crate::source::application::ItemKind;

/// 面板作用对象的种类。由 [`subject_for`] 从「条目类型 + 路径」归一而来，
/// 决定 [`actions_for`] 的条件表走哪几格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// exe / `.lnk` / 命令名——应用与命令条目。
    App,
    /// `shell:AppsFolder\<AUMID>`：Store 应用。**没有属性框、不能定位、不能进回收站**。
    StoreApp,
    /// 其它 `shell:` 命名项（`shell:RecycleBinFolder` 一类）。需求书明写「`shell:` 项跳过属性」。
    Namespace,
    /// `http(s)` / `ms-settings:` / 自定义协议——非本地路径的 URL 串。
    Url,
    /// 文件系统里的文件。
    File,
    /// 文件系统里的目录。
    Folder,
    /// 启动器自己的内置动作（`internal:<id>`）。路径串对用户没有意义 ⇒ 不供复制路径。
    Internal,
}

/// 一个动作的 id。`main.rs` 的 `run_action()` 按它分派；本文件按它给标签、副标题与键位提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionId {
    Open,
    RunAs,
    OpenWith,
    Reveal,
    CopyPath,
    CopyUrl,
    CopyName,
    OpenTerminal,
    Properties,
    Recycle,
    Edit,
    AddCommand,
    Delete,
    TogglePin,
    LargeType,
}

/// 面板里的一行。`label` 是标题，`subtitle` 是弱色补充（路径、网址、代价说明），
/// `keys` 是行尾键帽提示——只在「与结果模式同一个键已经存在」时才有值，
/// 面板不另造第二套键位（Enter 与数字键本来就吃高亮行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub id: ActionId,
    pub label: String,
    pub subtitle: String,
    pub keys: &'static str,
}

/// 面板的目标：一条清单条目或一行头段结果，抽成动作表需要的最小信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelTarget {
    /// 面板标题用（「关于〈项标题〉的操作」），也是「复制名称」的内容。
    pub title: String,
    pub subject: Subject,
    /// 交给 `ShellExecuteExW` 的 `lpFile`（应用/URL/命名项）或真实路径（文件/目录）。
    pub path: String,
    pub arguments: String,
    /// 路径此刻是否真的存在（由调用方查，本文件只读这个布尔）。
    pub exists: bool,
    /// 是否清单里的真实条目 ⇒ 可编辑 / 可删除 / 可置顶。头段行（文件行、搜索行）为 false。
    pub editable: bool,
    /// `editable` 时是条目的 uid（与 `LaunchItem::uid()` 同口径）。
    pub uid: Option<String>,
    pub pinned: bool,
}

impl PanelTarget {
    /// 面板顶栏那句提示（FR-ACT-03 原文「输入框清空并显示"关于〈项标题〉的操作"」）。
    pub fn banner(&self) -> String {
        format!("关于「{}」的操作", self.title)
    }

    /// 「复制名称」的内容：标题为空时退回路径末段（头段行的标题就是文件名，一般非空）。
    pub fn name_to_copy(&self) -> String {
        let t = self.title.trim();
        if t.is_empty() {
            leaf_name(&self.path)
        } else {
            t.to_string()
        }
    }

    /// 「复制路径」的内容：应用/命令条目把参数一起带上（那是能直接贴进「运行」框的一串）。
    pub fn path_to_copy(&self) -> String {
        let args = self.arguments.trim();
        if args.is_empty() {
            self.path.clone()
        } else {
            format!("{} {}", self.path, args)
        }
    }
}

// ---------------------------------------------------------------------------
// 路径形态与归类
// ---------------------------------------------------------------------------

/// 取 RFC 3986 意义上的 scheme：`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`。
/// **盘符不算 scheme**（`C:\Windows` 的 `C` 只有一个字母）——判长度 >=2 就够，
/// 不必反向排除单字母。没有长度下限的话，整张表会把每个绝对路径当成协议 URL。
pub fn url_scheme(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    match bytes.first() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return None,
    }
    let mut i = 1;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b':' {
            break;
        }
        if !(c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.') {
            return None;
        }
        i += 1;
    }
    if bytes.get(i) != Some(&b':') {
        return None;
    }
    if i < 2 {
        return None;
    }
    Some((&text[..i], &text[i + 1..]))
}

/// 是否本地/网络文件路径（盘符绝对路径或 UNC）。`explorer /select,`、终端 cwd、回收站
/// 三档都只对该形态开放——`ms-settings:`、`shell:`、`internal:`、相对路径一律不是路径。
pub fn is_fs_path(text: &str) -> bool {
    let t = text.trim();
    let b = t.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return matches!(b[2], b'\\' | b'/');
    }
    t.starts_with("\\\\")
}

fn trim_seps(text: &str) -> &str {
    text.trim_end_matches(['\\', '/'])
}

/// 驱动器根（`C:` / `C:\`）或 UNC 共享根（`\\srv\share`）。需求书那条「磁盘根目录不能删」
/// 在 UNC 上的对应物：删共享根等于删整个网络共享的入口，同一档禁掉。
pub fn is_path_root(text: &str) -> bool {
    let t = trim_seps(text.trim());
    let b = t.as_bytes();
    if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return true;
    }
    if t.starts_with("\\\\") {
        return t[2..].split(['\\', '/']).filter(|s| !s.is_empty()).count() <= 2;
    }
    false
}

/// 路径（或 UNC 共享）的父目录，用于「在终端打开」落在文件所在目录。
/// 根目录的父目录是它自己：返回空串会让 `Command::current_dir` 直接失败。
/// 驱动器根一律带尾分隔符返回——`C:` 不是合法的 cwd，`C:\` 才是。
pub fn parent_dir(text: &str) -> String {
    let t = trim_seps(text.trim());
    if t.is_empty() {
        return String::new();
    }
    let up = if is_path_root(t) {
        t
    } else {
        match t.rfind(['\\', '/']) {
            Some(pos) if pos > 0 => trim_seps(&t[..pos]),
            _ => t,
        }
    };
    with_sep_if_drive(up)
}

/// `C:` 这种夹在中间的盘符根补回分隔符（`C:\a` 的父目录是 `C:\`，不是 `C:`）。
fn with_sep_if_drive(text: &str) -> String {
    let b = text.as_bytes();
    if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        format!("{text}\\")
    } else {
        text.to_string()
    }
}

/// 路径末段（去尾分隔符后的最后一个组件）。用于「复制名称」的兜底。
pub fn leaf_name(text: &str) -> String {
    let t = trim_seps(text.trim());
    match t.rfind(['\\', '/']) {
        Some(pos) if pos + 1 < t.len() => t[pos + 1..].to_string(),
        _ => t.to_string(),
    }
}

/// AHK `ActionCatalog.ahk:64` 认的那批「可以提权的扩展名」。指向目录或非路径串时由
/// [`is_fs_path`] 先挡住，这里只看扩展名。
pub fn can_elevate(text: &str) -> bool {
    const EXTS: [&str; 6] = ["exe", "lnk", "bat", "cmd", "msc", "ps1"];
    let t = trim_seps(text.trim());
    match t.rfind('.') {
        Some(pos) if pos + 1 < t.len() => {
            let ext = t[pos + 1..].to_ascii_lowercase();
            EXTS.contains(&ext.as_str())
        }
        _ => false,
    }
}

/// 「条目类型 + 路径 + 是否目录」→ 面板对象种类。
///
/// 次序有讲究：先看路径形态（`shell:AppsFolder\`、`shell:`、`internal:`、其它 scheme），
/// 再回落到声明的 `kind`。理由是同一个 `ItemKind::Url` 既可能是 `https://…` 也可能是
/// `ms-settings:…`，两者在动作表里其实同格（都没有属性/定位/回收站），但 `shell:` 与
/// Store 必须和它们分开——需求书为这两类各写了一句排除条件。
pub fn subject_for(kind: ItemKind, path: &str, is_dir: bool) -> Subject {
    let trimmed = path.trim();
    if let Some((scheme, rest)) = url_scheme(trimmed) {
        let scheme = scheme.to_ascii_lowercase();
        if scheme == "shell" {
            // `shell:AppsFolder\<AUMID>` 是启动器自己造的 Store 串；`shell:RecycleBinFolder`
            // 那类命名项走 Namespace。大小写照 AUMID 原样（只比前缀，不误伤别的命名项）。
            let is_store = rest
                .get(..10)
                .map(|head| head.eq_ignore_ascii_case("appsfolder"))
                .unwrap_or(false)
                && matches!(rest.get(10..11), Some("\\"));
            return if is_store {
                Subject::StoreApp
            } else {
                Subject::Namespace
            };
        }
        if scheme == "internal" {
            return Subject::Internal;
        }
        return Subject::Url;
    }
    match kind {
        ItemKind::Folder | ItemKind::File => {
            if is_dir {
                Subject::Folder
            } else {
                Subject::File
            }
        }
        ItemKind::Url => Subject::Url,
        ItemKind::Internal => Subject::Internal,
        ItemKind::App | ItemKind::Command => Subject::App,
    }
}

// ---------------------------------------------------------------------------
// 动作表
// ---------------------------------------------------------------------------

/// FR-ACT-04 的条件表。**顺序即面板顺序**，两条不变量由单测钉住：「打开」恒为第一项
/// （Enter 与数字键落下去就是它，与结果模式的默认动作同一口径），「大字显示」恒为最后一项
/// （与 AHK `ListFor()` 的收尾一致）。
pub fn actions_for(t: &PanelTarget) -> Vec<Action> {
    let fs = is_fs_path(&t.path);
    // 应用条目常常只有命令名（`calc.exe`）而非全路径；提权/定位/属性只对「盘上真有其文件」
    // 的那批有意义 ⇒ App 这一格要求 fs && exists。文件行/目录行天然是 fs。
    let file_like = t.subject == Subject::File
        || (t.subject == Subject::App && fs && t.exists && !is_path_root(&t.path));
    let dir_like = t.subject == Subject::Folder && fs;
    let on_disk = fs && (file_like || dir_like);

    let mut out = Vec::new();
    out.push(row(ActionId::Open, "启动器照常执行它", "Enter"));

    // 以管理员运行。两条通道：① 真实可执行文件（文件行、指向全路径的应用条目）；
    // ② 应用/命令条目整体——出厂命令大多是裸名（`calc.exe`、`notepad.exe`），本程序不做
    // PATH 搜索，但 `ShellExecuteExW` 自己会搜（「打开」那一条走的正是这条路），所以这里
    // 不按「是不是绝对路径」把它挡掉。目录、URL、Store、命名项不给提权。
    let runnable = !dir_like
        && !matches!(
            t.subject,
            Subject::Url | Subject::StoreApp | Subject::Namespace
        );
    if runnable && (t.subject == Subject::App || (on_disk && can_elevate(&t.path))) {
        out.push(row(ActionId::RunAs, "弹 UAC 提权后运行", ""));
    }
    // 打开方式：`OpenAs_RunDLL` 是给文件用的；AHK 同样要求此刻真的存在。
    if on_disk && !dir_like && t.exists {
        out.push(row(ActionId::OpenWith, "系统的「打开方式」对话框", ""));
    }
    if on_disk {
        out.push(row(ActionId::Reveal, &display_path(t), "Ctrl+D"));
    }
    match t.subject {
        Subject::Url => out.push(row(ActionId::CopyUrl, &t.path, "")),
        Subject::Internal => {}
        _ => out.push(row(ActionId::CopyPath, &t.path_to_copy(), "")),
    }
    out.push(row(ActionId::CopyName, &t.name_to_copy(), ""));
    // 在终端打开：目录进本目录，文件进父目录（AHK `_ParentDir` 同口径），且必须真实存在。
    if on_disk && t.exists {
        let dir = if dir_like {
            t.path.clone()
        } else {
            parent_dir(&t.path)
        };
        out.push(row(ActionId::OpenTerminal, &format!("在该目录开终端：{dir}"), ""));
        out.push(row(ActionId::Properties, "系统的属性对话框", ""));
        if !is_path_root(&t.path) {
            out.push(row(ActionId::Recycle, "移入回收站（可从回收站还原）", ""));
        }
    }

    // 尾部：编辑/新增、删除、置顶、大字显示（AHK 的收尾顺序）。
    if t.editable {
        out.push(row(ActionId::Edit, "改名称、目标与别名", "Ctrl+E"));
        out.push(row(ActionId::Delete, "删除前会再确认一次", "Ctrl+Shift+D"));
        let (label, sub) = if t.pinned {
            ("取消置顶", "回到按学习排序的位置")
        } else {
            ("置顶", "恒排在结果前列（📌）")
        };
        out.push(Action {
            id: ActionId::TogglePin,
            label: label.to_string(),
            subtitle: sub.to_string(),
            keys: "Ctrl+Shift+P",
        });
    } else if t.subject != Subject::Internal {
        // 头段行（文件行 / 搜索行）没有可编辑的条目，给用户一条「存成命令」的通道。
        out.push(row(ActionId::AddCommand, "把它存成一条快捷命令", "Ctrl+Shift+N"));
    }
    out.push(row(ActionId::LargeType, "铺满屏幕中央放大显示", "Ctrl+L"));
    out
}

fn row(id: ActionId, subtitle: &str, keys: &'static str) -> Action {
    Action {
        id,
        label: id.label().to_string(),
        subtitle: subtitle.to_string(),
        keys,
    }
}

/// 副标题里显示的路径：去掉尾部分隔符（`D:\tmp\` 显示成 `D:\tmp` 更好读），
/// 并把夹在中间的盘符根补成 `C:\`——那才是用户认得的写法。
fn display_path(t: &PanelTarget) -> String {
    with_sep_if_drive(trim_seps(&t.path))
}

impl ActionId {
    /// 面板里的标题。置顶的正反两态在 [`actions_for`] 里直接产出文案，所以这里只给
    /// 「当前未置顶」那一句。
    pub fn label(self) -> &'static str {
        match self {
            ActionId::Open => "打开",
            ActionId::RunAs => "以管理员运行",
            ActionId::OpenWith => "打开方式…",
            ActionId::Reveal => "所在位置",
            ActionId::CopyPath => "复制路径",
            ActionId::CopyUrl => "复制网址",
            ActionId::CopyName => "复制名称",
            ActionId::OpenTerminal => "在终端打开",
            ActionId::Properties => "属性",
            ActionId::Recycle => "移入回收站",
            ActionId::Edit => "编辑",
            ActionId::AddCommand => "添加为命令",
            ActionId::Delete => "删除",
            ActionId::TogglePin => "置顶",
            ActionId::LargeType => "大字显示",
        }
    }

    /// 行首徽标词（与结果行的「打开 / 复制 / 搜索」同一口径）。
    pub fn badge(self) -> &'static str {
        match self {
            ActionId::Open
            | ActionId::RunAs
            | ActionId::OpenWith
            | ActionId::Reveal
            | ActionId::OpenTerminal
            | ActionId::Properties
            | ActionId::Edit
            | ActionId::AddCommand
            | ActionId::LargeType => "执行",
            ActionId::CopyPath | ActionId::CopyUrl | ActionId::CopyName => "复制",
            ActionId::Delete => "删除",
            ActionId::Recycle => "回收",
            ActionId::TogglePin => "置顶",
        }
    }

    /// 英文别名表。FR-ACT-03 那句「输入即在动作集里模糊过滤」：本项目的动作标题全是指定
    /// 中文，所以给每个动作配一小撮**只含拉丁字母**的别名，让 `admin` / `props` / `term`
    /// 这类手打习惯能命中。**不做拼音**——与 AHK 同口径（拼音只喂应用与书签，不喂动作）。
    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            ActionId::Open => &["open", "run", "start", "enter"],
            ActionId::RunAs => &["runas", "admin", "elevate", "root"],
            ActionId::OpenWith => &["openwith", "choose", "app", "rundll"],
            ActionId::Reveal => &["reveal", "where", "location", "select"],
            ActionId::CopyPath => &["path", "copy", "cp"],
            ActionId::CopyUrl => &["url", "link", "copy", "cp"],
            ActionId::CopyName => &["name", "copy", "title"],
            ActionId::OpenTerminal => &["terminal", "term", "cmd", "shell", "console"],
            ActionId::Properties => &["properties", "props", "info", "detail"],
            ActionId::Recycle => &["recycle", "trash", "bin"],
            ActionId::Edit => &["edit", "modify", "change"],
            ActionId::AddCommand => &["add", "command", "new", "create", "save"],
            ActionId::Delete => &["delete", "del", "remove"],
            ActionId::TogglePin => &["pin", "top", "stick", "unpin"],
            ActionId::LargeType => &["large", "type", "zoom", "big", "font"],
        }
    }
}

// ---------------------------------------------------------------------------
// 过滤
// ---------------------------------------------------------------------------

/// 面板内「输入即过滤」（FR-ACT-03）。返回命中的**下标**并保持动作表原序
/// （「打开」不会因为被过滤就掉到第三位）。
///
/// 判据对照 AHK `SearchWindow._RunSearch()` 的 actions 分支（`FuzzyMatcher.Score(词, 标题) > 0`）：
/// 1. 空白查询 ⇒ 全量（面板刚进来就是全量）；
/// 2. 按空白切成若干词，**每个词都得命中**（与主列表多词同口径）；
/// 3. 一个词命中一个动作 = 标题子串 ∨ 英文别名子串 ∨ 标题字符**按序**子序列（≥2 字符）。
///    最后那条是为中文写的：「打方」要能命中「打开方式」。**不匹配副标题**——那里是路径和
///    说明文字，参与匹配会让 `D:` 这种词命中一半动作。
pub fn filter_actions(query: &str, actions: &[Action]) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return (0..actions.len()).collect();
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut hits = Vec::new();
    for (idx, a) in actions.iter().enumerate() {
        let label = a.label.to_lowercase();
        let all = a.id.keywords();
        if words.iter().all(|w| {
            label.contains(w)
                || all
                    .iter()
                    .any(|k| k.contains(w) || w.contains(*k) || is_subsequence(w, k))
                || (w.chars().count() >= 2 && is_subsequence(w, &label))
        }) {
            hits.push(idx);
        }
    }
    hits
}

/// `needle` 的字符是否按序出现在 `hay` 里（不要求连续）。两侧都按字符而非字节比较。
fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ItemKind::*;

    /// 构造口径照上游真实赋值：`title` 是条目名 / 文件行的文件名，`path` 直接取
    /// `LaunchItem::path` 或 `FileHit::path` 的原值（裸命令名、全路径、`shell:`、AUMID 都有）。
    fn target(subject: Subject, path: &str) -> PanelTarget {
        PanelTarget {
            title: leaf_name(path),
            subject,
            path: path.to_string(),
            arguments: String::new(),
            exists: true,
            editable: true,
            uid: Some(format!("custom:测试|{path}")),
            pinned: false,
        }
    }

    fn ids(t: &PanelTarget) -> Vec<ActionId> {
        actions_for(t).into_iter().map(|a| a.id).collect()
    }

    fn has(t: &PanelTarget, id: ActionId) -> bool {
        ids(t).contains(&id)
    }

    #[test]
    fn a_drive_letter_is_not_a_url_scheme() {
        // 这一条挡掉的是「每个绝对路径都被当成协议」：判据是 scheme 段长度 >=2。
        assert_eq!(url_scheme("C:\\Windows\\notepad.exe"), None);
        assert_eq!(url_scheme("D:/tmp/a.txt"), None);
        assert_eq!(url_scheme("calc.exe"), None);
        assert_eq!(url_scheme("\\\\nas\\share\\proj"), None);
        assert_eq!(url_scheme("ms-settings:display").map(|(s, _)| s), Some("ms-settings"));
        assert_eq!(url_scheme("shell:AppsFolder\\X!App").map(|(s, _)| s), Some("shell"));
        assert_eq!(url_scheme("https://a/b").map(|(_, r)| r), Some("//a/b"));
        assert!(!is_fs_path("C:a\\b.txt")); // 盘符相对路径不是绝对路径
        assert!(is_fs_path("C:\\a"));
        assert!(is_fs_path("\\\\nas\\share"));
    }

    #[test]
    fn classifies_the_stored_shapes_this_program_actually_writes() {
        // 出厂命令的 path 是裸名（`calc.exe`）；.lnk 解析后是全路径；Store 是 AUMID 串；
        // 系统命令分组是 `ms-settings:`；内置动作是 `internal:`。这五格都是本仓的真值形态。
        assert_eq!(subject_for(Command, "calc.exe", false), Subject::App);
        assert_eq!(
            subject_for(App, "C:\\Windows\\System32\\notepad.exe", false),
            Subject::App
        );
        assert_eq!(
            subject_for(App, "shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", false),
            Subject::StoreApp
        );
        assert_eq!(subject_for(App, "shell:RecycleBinFolder", false), Subject::Namespace);
        assert_eq!(subject_for(Url, "ms-settings:display", false), Subject::Url);
        assert_eq!(subject_for(Url, "https://www.google.com", false), Subject::Url);
        assert_eq!(subject_for(Internal, "internal:lock", false), Subject::Internal);
        assert_eq!(subject_for(Folder, "D:\\D10-Project", true), Subject::Folder);
        assert_eq!(subject_for(File, "D:\\tmp\\report.docx", false), Subject::File);
        // kind 说是 File、目录位说是目录 ⇒ 以目录位为准（文件行的 is_dir 来自列目录）。
        assert_eq!(subject_for(File, "D:\\tmp", true), Subject::Folder);
    }

    #[test]
    fn open_is_first_and_large_type_is_last_for_every_subject() {
        // 「Enter 落下去就是默认动作」与「大字显示垫底」两条顺序不变量（AHK ListFor 同口径）。
        for t in [
            target(Subject::App, "calc.exe"),
            target(Subject::StoreApp, "shell:AppsFolder\\Microsoft.Copilot_8wekyb3d8bbwe!App"),
            target(Subject::Namespace, "shell:RecycleBinFolder"),
            target(Subject::Url, "https://www.google.com/search?q=rust"),
            target(Subject::File, "D:\\tmp\\report.docx"),
            target(Subject::Folder, "D:\\D10-Project"),
            target(Subject::Internal, "internal:lock"),
        ] {
            let list = ids(&t);
            assert_eq!(list.first(), Some(&ActionId::Open), "{:?}", t.path);
            assert_eq!(*list.last().unwrap(), ActionId::LargeType, "{:?}", t.path);
        }
    }

    #[test]
    fn store_apps_and_shell_namespace_items_lose_the_shell_dialogs() {
        // 需求书 FR-ACT-04 那句「`shell:` 项跳过、Store 应用不显示」两条一起钉住。
        let store = target(Subject::StoreApp, "shell:AppsFolder\\Microsoft.Copilot_8wekyb3d8bbwe!App");
        for id in [ActionId::Properties, ActionId::Reveal, ActionId::Recycle, ActionId::OpenWith, ActionId::RunAs] {
            assert!(!has(&store, id), "Store 应用不该有 {id:?}");
        }
        assert!(has(&store, ActionId::Open) && has(&store, ActionId::CopyPath));

        let ns = target(Subject::Namespace, "shell:ControlPanelFolder");
        assert!(!has(&ns, ActionId::Properties));
        assert!(!has(&ns, ActionId::RunAs));
        assert!(has(&ns, ActionId::CopyPath));
    }

    #[test]
    fn urls_get_copy_url_not_copy_path() {
        let url = target(Subject::Url, "https://www.google.com/search?q=rust");
        assert!(has(&url, ActionId::CopyUrl));
        assert!(!has(&url, ActionId::CopyPath));
        for id in [ActionId::Reveal, ActionId::Properties, ActionId::Recycle, ActionId::OpenTerminal] {
            assert!(!has(&url, id));
        }
        // internal: 的 path 串对用户没有意义 ⇒ 两条复制都不给路径，只给名称。
        let int = target(Subject::Internal, "internal:lock");
        assert!(!has(&int, ActionId::CopyPath));
        assert!(has(&int, ActionId::CopyName));
    }

    #[test]
    fn bare_command_names_still_get_elevation_because_the_shell_resolves_them() {
        // 「以管理员运行」对 `calc.exe` 这种裸名是可用的：`ShellExecuteExW` 自己搜 PATH，
        // 与「打开」同一条通道 ⇒ 不因「不是绝对路径」把它挡掉。
        let calc = target(Subject::App, "calc.exe");
        assert!(has(&calc, ActionId::RunAs));
        assert!(!has(&calc, ActionId::Reveal)); // 定位需要真实路径，这一格仍按 fs 判据
        let doc = target(Subject::File, "D:\\tmp\\report.docx");
        assert!(!has(&doc, ActionId::RunAs)); // .docx 不在可提权扩展名里（AHK 同口径）
        let bat = target(Subject::File, "D:\\tmp\\build.bat");
        assert!(has(&bat, ActionId::RunAs));
    }

    #[test]
    fn a_file_row_opens_a_terminal_in_its_parent_folder() {
        let file = target(Subject::File, "D:\\tmp\\report.docx");
        let list = actions_for(&file);
        let term = list.iter().find(|a| a.id == ActionId::OpenTerminal).unwrap();
        assert!(term.subtitle.contains("D:\\tmp"), "{}", term.subtitle);
        let dir = target(Subject::Folder, "D:\\tmp");
        let term = actions_for(&dir)
            .into_iter()
            .find(|a| a.id == ActionId::OpenTerminal)
            .unwrap();
        assert!(term.subtitle.contains("D:\\tmp"), "{}", term.subtitle);
    }

    #[test]
    fn drive_and_share_roots_are_not_recyclable() {
        // 需求书 FR-ACT-04「磁盘根目录不能删」在 UNC 上的对应物也一起钉住。
        let at_root = target(Subject::Folder, "C:\\");
        assert!(!has(&at_root, ActionId::Recycle));
        let share = target(Subject::Folder, "\\\\nas\\proj");
        assert!(!has(&share, ActionId::Recycle));
        let inside = target(Subject::Folder, "\\\\nas\\proj\\docs");
        assert!(has(&inside, ActionId::Recycle));
        assert_eq!(parent_dir("C:\\"), "C:\\");
        assert_eq!(parent_dir("C:"), "C:\\");
        assert_eq!(parent_dir("C:\\a\\b"), "C:\\a");
        assert_eq!(parent_dir("C:\\a"), "C:\\");
        assert_eq!(parent_dir("\\\\nas\\proj"), "\\\\nas\\proj");
        assert_eq!(leaf_name("D:\\tmp\\"), "tmp");
    }

    #[test]
    fn a_missing_path_loses_only_the_actions_that_have_to_touch_the_disk() {
        let mut gone = target(Subject::File, "D:\\tmp\\deleted.docx");
        gone.exists = false;
        for id in [ActionId::OpenWith, ActionId::OpenTerminal, ActionId::Properties, ActionId::Recycle] {
            assert!(!has(&gone, id), "不存在的路径不该有 {id:?}");
        }
        for id in [ActionId::Open, ActionId::Reveal, ActionId::CopyPath, ActionId::CopyName] {
            assert!(has(&gone, id), "这一条不该随存在性消失：{id:?}");
        }
    }

    #[test]
    fn a_head_row_gets_add_command_instead_of_edit_delete_pin() {
        // 文件行 / 搜索行不是清单条目 ⇒ 编辑、删除、置顶三档都没有，换成「添加为命令」。
        let mut row = target(Subject::File, "D:\\tmp\\report.docx");
        row.editable = false;
        row.uid = None;
        let list = ids(&row);
        assert!(list.contains(&ActionId::AddCommand));
        for id in [ActionId::Edit, ActionId::Delete, ActionId::TogglePin] {
            assert!(!list.contains(&id), "头段行不该有 {id:?}");
        }
        // 真实条目反过来：有编辑/删除/置顶，没有「添加为命令」。
        let item = target(Subject::File, "D:\\tmp\\report.docx");
        let list = ids(&item);
        assert!(list.contains(&ActionId::Edit) && list.contains(&ActionId::Delete));
        assert!(list.contains(&ActionId::TogglePin) && !list.contains(&ActionId::AddCommand));
        assert_eq!(
            actions_for(&item)
                .into_iter()
                .find(|a| a.id == ActionId::TogglePin)
                .unwrap()
                .label,
            "置顶"
        );
        let mut pinned = item.clone();
        pinned.pinned = true;
        assert_eq!(
            actions_for(&pinned)
                .into_iter()
                .find(|a| a.id == ActionId::TogglePin)
                .unwrap()
                .label,
            "取消置顶"
        );
    }

    #[test]
    fn copy_name_and_copy_path_use_the_upstream_fields() {
        // 应用条目的 arguments 常常非空（出厂命令 `ms-settings:` 无参、`.lnk` 有参）：
        // 复制路径要把参数带上，复制名称用条目名而不是文件末段。
        let mut t = target(Subject::App, "C:\\Windows\\System32\\calc.exe");
        t.title = "计算器".to_string();
        t.arguments = "  /hidden  ".to_string();
        assert_eq!(t.name_to_copy(), "计算器");
        assert_eq!(t.path_to_copy(), "C:\\Windows\\System32\\calc.exe /hidden");
        let list = actions_for(&t);
        assert_eq!(
            list.iter().find(|a| a.id == ActionId::CopyPath).unwrap().subtitle,
            "C:\\Windows\\System32\\calc.exe /hidden"
        );
        // 标题为空时（只可能来自非条目行）退回路径末段。
        let mut blank = t.clone();
        blank.title = "  ".to_string();
        assert_eq!(blank.name_to_copy(), "calc.exe");
    }

    #[test]
    fn filter_keeps_the_table_order_and_requires_every_word() {
        let t = target(Subject::File, "D:\\tmp\\report.docx");
        let all = actions_for(&t);
        assert_eq!(filter_actions("   ", &all), (0..all.len()).collect::<Vec<_>>());

        let hit = |q: &str| -> Vec<String> {
            filter_actions(q, &all)
                .into_iter()
                .map(|i| all[i].label.clone())
                .collect()
        };
        // 子串：中文标题直接命中。
        assert_eq!(hit("复制"), ["复制路径", "复制名称"]);
        // 多词是 AND：`复制 路径` 只剩复制路径。
        assert_eq!(hit("复制 路径"), ["复制路径"]);
        assert!(hit("复制 不存在的词").is_empty());
        // 顺序跟着动作表，不跟着命中次序。
        assert_eq!(hit("位置"), ["所在位置"]);
        assert_eq!(hit("回收"), ["移入回收站"]);
        // 两个词是 AND，跨不到两个动作身上 ⇒ 命中为空（与主列表多词同一口径）。
        assert!(hit("回收 位置").is_empty());
    }

    #[test]
    fn filter_uses_english_aliases_and_char_subsequence_but_never_pinyin() {
        // 用文件行：目录没有「打开方式」那一档，而这条测的正是两个汉字按序命中它。
        let t = target(Subject::File, "D:\\tmp\\report.docx");
        let all = actions_for(&t);
        let n = |q: &str| filter_actions(q, &all).len();
        // 拉丁别名（本文件配的 keywords）与标题按序子序列都能命中。
        assert!(n("props") >= 1); // Properties 的别名
        assert!(n("term") >= 1); // OpenTerminal
        assert!(n("性") >= 1); // 单字走标题子串，命中「属性」
        assert!(n("打方") >= 1); // 两个汉字按序命中「打开方式…」
        // 拼音明确不做：这三个词一个都不该命中（与 AHK 同口径——拼音只喂应用与书签）。
        assert_eq!(n("shuxing"), 0);
        assert_eq!(n("daikai"), 0);
        assert_eq!(n("fuyizhi"), 0);
    }

    #[test]
    fn every_action_has_a_label_and_at_least_one_alias() {
        // 防止新加一档时忘了标签或别名：别名表漏一格，那一档就只能靠中文命中。
        use ActionId::*;
        for id in [
            Open, RunAs, OpenWith, Reveal, CopyPath, CopyUrl, CopyName, OpenTerminal,
            Properties, Recycle, Edit, AddCommand, Delete, TogglePin, LargeType,
        ] {
            assert!(!id.label().is_empty());
            assert!(!id.keywords().is_empty(), "{id:?} 没有英文别名");
            assert!(!id.badge().is_empty());
        }
    }
}


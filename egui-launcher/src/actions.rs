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
    /// 批次 E4（FR-ACT-05）：**一批被标记的文件/文件夹**。不是清单里的一行，而是 `Insert`
    /// 攒下来的集合 ⇒ 动作表整张换掉（[`batch_actions`]），不再走 [`actions_for`] 的单体条件表。
    Batch,
    /// 批次 E5（FR-ACT-01 第三格）：**一串算出来的文本**——计算器行的结果与换算值。
    /// 它不是磁盘上的东西、也不是协议串：`path` 里放的就是那串文本本身，所以「打开」这一格
    /// 对它没有意义（把 `14` 交给 `ShellExecute` 会去搜一个叫 14 的程序）。整张表换掉
    /// （[`text_actions`]），与 [`Subject::Batch`] 同一处理方式。
    Text,
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
    // ---- 批次 E4（FR-ACT-05）：批量动作。只在 [`Subject::Batch`] 那张表里出现 ----
    OpenAll,
    CopyFiles,
    CutFiles,
    CopyPaths,
    CopyTo,
    AddAll,
    RecycleAll,
    // ---- 批次 E5（FR-ACT-01 第三格）：文本对象。只在 [`Subject::Text`] 那张表里出现 ----
    /// 把这串文本打字打进呼出启动器之前的那个窗口（AHK `PasteText` 的同一条，注入方式有意不同）。
    PasteText,
    /// 把这串文本放进剪贴板——与结果模式按 Enter 同一条动作，所以它是这张表的第一格。
    CopyText,
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
    /// 批次 E4：`Subject::Batch` 时这一批的路径（原样，`Insert` 攒下来的次序）。
    /// 其它对象恒为空——单体动作表不看它，按键只影响表头文案里的条数。
    pub batch: Vec<String>,
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

/// 批次 E4（FR-ACT-05）：**一批**被标记项的动件表（对照 AHK `ActionCatalog.ListForMany`）。
///
/// 次序照 AHK：打开 → 复制 → 剪切 → 复制路径 → 复制到 → 加入命令 → 回收站。
/// 与 AHK 的两处有意偏离（都写进需求书 FR-ACT-05 的日期注记）：
/// ① AHK 的「移动到」依赖 QuickSwitch 里"当前文件管理器目录"这个概念，本程序没有 ⇒ 不做；
/// ② AHK 的「复制到」只在 QuickSwitch 报告了一个目录时才出现，本程序没有那个上下文 ⇒
///    恒给一条，目的地由系统的文件夹选择框当场问。
pub fn batch_actions(t: &PanelTarget) -> Vec<Action> {
    let n = t.batch.len();
    let mut out = Vec::new();
    out.push(row(ActionId::OpenAll, &format!("依次打开这 {n} 项"), "Enter"));
    out.push(row(
        ActionId::CopyFiles,
        "以文件对象放进剪贴板，可在资源管理器粘贴",
        "",
    ));
    out.push(row(
        ActionId::CutFiles,
        "粘贴时把原件移走（会写剪切标记）",
        "",
    ));
    out.push(row(
        ActionId::CopyPaths,
        &format!("{n} 条路径，每行一条"),
        "",
    ));
    out.push(row(
        ActionId::CopyTo,
        "选一个文件夹，把这一批复制过去",
        "",
    ));
    out.push(row(
        ActionId::AddAll,
        "每一项存成一条快捷命令（已有的跳过）",
        "",
    ));
    out.push(row(
        ActionId::RecycleAll,
        "移入回收站（可还原），执行前再确认一次",
        "",
    ));
    out
}

// ---------------------------------------------------------------------------
// 文本对象的动作表（批次 E5）
// ---------------------------------------------------------------------------

/// 批次 E5（FR-ACT-01 第三格「文本 ⇒ 粘贴」）：计算器行那串算出来的文本能做的三件事。
///
/// 第一格刻意不是「打开」而是「复制」，但**守的是同一条不变量的本义**——面板第一行永远等于
/// 结果模式里 Enter 落下去做的那件事，而计算器行的 Enter 本来就是复制数字（FR-SRC-10 批次 S）。
/// 给它配一个假的「打开」会把这串文本交给 `ShellExecuteExW`，等于让启动器去搜一个叫 `14` 的程序。
///
/// 「大字显示」仍垫底，与单体表、批量表同一条收尾。
fn text_actions(t: &PanelTarget) -> Vec<Action> {
    let n = t.path.chars().count();
    vec![
        row(ActionId::CopyText, "只带这一串值，与 Enter 同一条", "Enter"),
        row(
            ActionId::PasteText,
            &format!("打进呼出前的那个窗口，不碰你的剪贴板（{n} 个字符）"),
            "",
        ),
        row(ActionId::LargeType, "铺满屏幕中央放大显示", "Ctrl+L"),
    ]
}

/// FR-ACT-04 的条件表。**顺序即面板顺序**，两条不变量由单测钉住：「打开」恒为第一项
/// （Enter 与数字键落下去就是它，与结果模式的默认动作同一口径），「大字显示」恒为最后一项
/// （与 AHK `ListFor()` 的收尾一致）。
///
/// 批次 E4：`Subject::Batch` 整张换表，早退在条件表之前——批量对象没有"名称/路径/可编辑性"
/// 这些单体属性，硬套进来会让每条判据都要多一个"是不是批量"的分支。
/// 批次 E5：`Subject::Text` 同理另换一张（见 [`text_actions`]）。
pub fn actions_for(t: &PanelTarget) -> Vec<Action> {
    if t.subject == Subject::Batch {
        return batch_actions(t);
    }
    if t.subject == Subject::Text {
        return text_actions(t);
    }
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
        // 批次 E6（FR-ACT-04 原文的单行形态）：复制文件 / 剪切文件。门与 AHK `_FileActions`
        // （`ActionCatalog.ahk:312-316`）逐字对齐——「盘符或 UNC 开头」且「此刻真的存在」，
        // 就是这一整套 `on_disk && t.exists`：AHK 用 `FileExist`，对目录同样返回真。
        // 位置也照 AHK：紧跟两条复制之后、在终端打开之前。两条共用一套措辞，因为做的是
        // 同一件事的两种形态（差别只在剪不剪）。批量版（FR-ACT-05，批次 E4）用的就是
        // 同一对 `ActionId`，所以两种形态在剪贴板上写的东西一模一样。
        // AHK 的「复制到/移动到」（同函数 317-326 行）依赖 QuickSwitch 的"当前文件管理器目录"，
        // 本程序没有那个上下文 ⇒ 单行版仍只有批量面板那条「复制到…」（FR-ACT-05 偏差①已登记）。
        out.push(row(
            ActionId::CopyFiles,
            "以文件对象放进剪贴板，可在资源管理器粘贴",
            "",
        ));
        out.push(row(
            ActionId::CutFiles,
            "粘贴时把原件移走（会写剪切标记）",
            "",
        ));
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
            ("置顶", "恒排在结果前列（图标右上角角标）")
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

/// 批次 E3-丙（FR-ACT-01「`Ctrl+Enter` 走次级动作」）：快捷键版按这一条判据**直接做一件事**，
/// 不给面板、也就没有第二次选择。映射只收需求书原文点名的三格，且与面板的准入条件**逐格对齐**
/// （面板里 `Reveal` 的准入就是 `File` / `Folder` 且路径是文件系统路径，`CopyUrl` 的准入就是
/// `Url`；两边同一份 `Subject` ⇒ 不存在"面板里有、快捷键说没有"的行）：
///
/// - 文件 / 文件夹 ⇒ 所在位置（`explorer /select,`）；
/// - 网址 / 协议串 ⇒ 复制网址；
/// - 算出来的文本（计算器行）⇒ 粘贴进呼出前的那个窗口（批次 E5，2026-10-27）；
/// - 其余（应用条目、Store 应用、命名项、内置动作行）⇒ `None`，它们要做什么得在面板里选。
///
/// ~~原文第三格「文本 ⇒ 粘贴」**没做**：它与 P1 那五个粘贴 / 批量动作同一个前置（FR-ACT-05 多选），
/// 不为它开一条没有多选语义的半截实现。~~【2026-10-27 批次 E5 更正：这一格的前置**从来不是多选**，
/// 是本程序没有"呼出前的那个窗口"这份记账——AHK `App.RememberActiveWindow()` 那一格。E5 补了记账，
/// 于是它落到 [`Subject::Text`] + [`ActionId::PasteText`] 上，与多选无关。】
pub fn secondary_action_for(subject: Subject) -> Option<ActionId> {
    match subject {
        Subject::File | Subject::Folder => Some(ActionId::Reveal),
        Subject::Url => Some(ActionId::CopyUrl),
        Subject::Text => Some(ActionId::PasteText),
        _ => None,
    }
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
            ActionId::OpenAll => "全部打开",
            ActionId::CopyFiles => "复制文件",
            ActionId::CutFiles => "剪切文件",
            ActionId::CopyPaths => "复制全部路径",
            ActionId::CopyTo => "复制到…",
            ActionId::AddAll => "全部加入命令",
            ActionId::RecycleAll => "全部回收站",
            ActionId::PasteText => "粘贴到原窗口",
            ActionId::CopyText => "复制这一串",
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
            ActionId::CopyText => "复制",
            ActionId::PasteText => "粘贴",
            ActionId::Delete => "删除",
            ActionId::Recycle => "回收",
            ActionId::TogglePin => "置顶",
            ActionId::OpenAll | ActionId::AddAll => "执行",
            ActionId::CopyFiles | ActionId::CopyTo => "复制",
            ActionId::CutFiles => "剪切",
            ActionId::CopyPaths => "复制",
            ActionId::RecycleAll => "回收",
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
            ActionId::OpenAll => &["openall", "open", "run", "all", "many"],
            ActionId::CopyFiles => &["copyfiles", "copy", "files", "cf"],
            ActionId::CutFiles => &["cutfiles", "cut", "move", "files", "cutall"],
            ActionId::CopyPaths => &["copypaths", "paths", "copy", "all", "cp"],
            ActionId::CopyTo => &["copyto", "destination", "folder", "copy"],
            ActionId::AddAll => &["addall", "add", "command", "all", "save"],
            ActionId::RecycleAll => &["recycleall", "recycle", "trash", "all"],
            ActionId::PasteText => &["paste", "pastetext", "send"],
            ActionId::CopyText => &["copytext", "copy", "value", "result"],
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
            batch: Vec::new(),
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
        // 批次 E6：复制文件 / 剪切文件放进剪贴板的东西是要被**粘到别处**的，原件不在就是空话
        // ⇒ 与属性、回收站同一道门（AHK `_FileActions` 的 `FileExist`）。
        for id in [
            ActionId::OpenWith,
            ActionId::OpenTerminal,
            ActionId::Properties,
            ActionId::Recycle,
            ActionId::CopyFiles,
            ActionId::CutFiles,
        ] {
            assert!(!has(&gone, id), "不存在的路径不该有 {id:?}");
        }
        for id in [ActionId::Open, ActionId::Reveal, ActionId::CopyPath, ActionId::CopyName] {
            assert!(has(&gone, id), "这一条不该随存在性消失：{id:?}");
        }
    }

    /// 批次 E6（FR-ACT-04 原文的单行形态）：单体表里那两格文件对象动作。
    /// 钉三件事——**有**（单行版不是批量版专属）、**位置**（照 AHK 排在两条复制之后、
    /// 在终端打开之前）、**同一份载荷**（与批量表用的是同一对 `ActionId`，所以剪贴板里
    /// 的东西按条数走：一行 ⇒ `batch` 只有那一条路径）。
    #[test]
    fn a_single_row_panel_offers_copy_and_cut_of_that_one_file() {
        for (subject, path) in [
            (Subject::File, "C:\\Windows\\notepad.exe"),
            (Subject::Folder, "C:\\Windows"),
        ] {
            let t = target(subject, path);
            let got = ids(&t);
            let pos = |id: ActionId| got.iter().position(|a| *a == id).unwrap();
            assert!(got.contains(&ActionId::CopyFiles) && got.contains(&ActionId::CutFiles));
            assert!(pos(ActionId::CopyName) < pos(ActionId::CopyFiles), "{:?}：两格要排在复制名称之后", subject);
            assert!(pos(ActionId::CopyFiles) < pos(ActionId::OpenTerminal), "{:?}:要排在在终端打开之前", subject);
            // 两条相邻且顺序是"复制在前、剪切在后"（AHK `_FileActions` 315-316 行的 push 次序）。
            assert_eq!(got[pos(ActionId::CopyFiles) + 1], ActionId::CutFiles, "{:?}", subject);
        }
        // 非文件系统路径一律没有：URL / Store / 命名项 / 内置动作，以及**裸名应用条目**。
        for t in [
            target(Subject::Url, "https://example.com/?q=1"),
            target(Subject::StoreApp, "shell:AppsFolder\\Microsoft.Copilot_8wekyb3d8bbwe!App"),
            target(Subject::Namespace, "shell:RecycleBinFolder"),
            target(Subject::Internal, "internal:lock"),
            target(Subject::App, "calc.exe"),
        ] {
            for id in [ActionId::CopyFiles, ActionId::CutFiles] {
                assert!(!has(&t, id), "{} 不该有 {id:?}", t.path);
            }
        }
        // 指向真实全路径的应用条目**有**（它就是盘上的一个文件，AHK 的判据只看形状与存在）。
        let real = target(Subject::App, "C:\\Windows\\System32\\notepad.exe");
        assert!(has(&real, ActionId::CopyFiles) && has(&real, ActionId::CutFiles));
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
        // 子串：中文标题直接命中。批次 E6 起「复制」在文件行命中三格（多出「复制文件」），
        // 而「剪切文件」一格都不命中——它的标题、别名（cut/move/files/cf）里没有「复制」，
        // 这正是别名表要防的串味。
        assert_eq!(hit("复制"), ["复制路径", "复制名称", "复制文件"]);
        assert_eq!(hit("剪切"), ["剪切文件"]);
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
            OpenAll, CopyFiles, CutFiles, CopyPaths, CopyTo, AddAll, RecycleAll,
        ] {
            assert!(!id.label().is_empty());
            assert!(!id.keywords().is_empty(), "{id:?} 没有英文别名");
            assert!(!id.badge().is_empty());
        }
    }

    /// 批次 E4：批量对象走另一张表——七条、次序照 AHK `ListForMany`，且**没有**单体那些格子
    /// （编辑/删除/置顶/大字/属性/所在位置）；标着「回收」的那条必须排在最后，与 AHK 的收尾一致。
    #[test]
    fn a_batch_target_gets_the_seven_ahk_actions_in_order() {
        let mut t = target(Subject::Batch, "");
        t.title = "已标记的 3 项".to_string();
        t.editable = false;
        t.uid = None;
        t.batch = vec![
            "D:\\a.txt".to_string(),
            "D:\\b.txt".to_string(),
            "D:\\docs".to_string(),
        ];
        let list = actions_for(&t);
        let got: Vec<ActionId> = list.iter().map(|a| a.id).collect();
        assert_eq!(
            got,
            vec![
                ActionId::OpenAll,
                ActionId::CopyFiles,
                ActionId::CutFiles,
                ActionId::CopyPaths,
                ActionId::CopyTo,
                ActionId::AddAll,
                ActionId::RecycleAll,
            ]
        );
        // Enter 落在第一格（「全部打开」），与结果模式的默认动作同一口径。
        assert_eq!(list[0].keys, "Enter");
        // 表头文案里的条数来自 `batch`，不是 `path`（批量对象没有单一路径）。
        assert!(list[0].subtitle.contains('3'), "{}", list[0].subtitle);
        assert!(list[3].subtitle.contains('3'), "{}", list[3].subtitle);
        for id in [
            ActionId::Edit,
            ActionId::Delete,
            ActionId::TogglePin,
            ActionId::LargeType,
            ActionId::Properties,
            ActionId::Reveal,
            ActionId::Open,
        ] {
            assert!(!got.contains(&id), "批量表里不该有 {id:?}");
        }
    }

    /// 批次 E5：文本对象（计算器行）另有一张三格表。第一格是 Enter 本来做的那件事（复制这一串），
    /// **不是**一个假的「打开」——把这串值交给 `ShellExecuteExW` 等于让启动器去搜一个叫 `14` 的程序。
    /// 粘贴那条的副标题报字符数，供真机只看日志就能判定贴出去的是哪一串。
    #[test]
    fn a_text_target_gets_copy_paste_large_and_never_a_launch() {
        let mut t = target(Subject::Text, "1,024");
        t.title = "= 1,024".to_string();
        t.editable = false;
        t.uid = None;
        let list = actions_for(&t);
        let got = ids(&t);
        assert_eq!(
            got,
            vec![ActionId::CopyText, ActionId::PasteText, ActionId::LargeType]
        );
        assert_eq!(list[0].keys, "Enter");
        assert!(list[1].subtitle.contains('5'), "{}", list[1].subtitle);
        for id in [
            ActionId::Open,
            ActionId::CopyPath,
            ActionId::CopyUrl,
            ActionId::CopyName,
            ActionId::Reveal,
            ActionId::RunAs,
            ActionId::Properties,
            ActionId::Recycle,
            ActionId::Edit,
            ActionId::Delete,
            ActionId::TogglePin,
            ActionId::AddCommand,
        ] {
            assert!(!got.contains(&id), "文本对象不该有 {id:?}");
        }
    }

    /// 批次 E3-丙 + E5：快捷键版的映射只收需求书原文点名的三格，其余一律交给面板。
    #[test]
    fn shortcut_mapping_is_the_three_cells_the_spec_names() {
        assert_eq!(secondary_action_for(Subject::File), Some(ActionId::Reveal));
        assert_eq!(secondary_action_for(Subject::Folder), Some(ActionId::Reveal));
        assert_eq!(secondary_action_for(Subject::Url), Some(ActionId::CopyUrl));
        // FR-ACT-01 原文第三格（批次 E5）：文本 ⇒ 粘贴。
        assert_eq!(secondary_action_for(Subject::Text), Some(ActionId::PasteText));
        for s in [
            Subject::App,
            Subject::StoreApp,
            Subject::Namespace,
            Subject::Internal,
        ] {
            assert_eq!(secondary_action_for(s), None, "{s:?} 的次级动作得在面板里选");
        }
    }

    /// 更硬的一条：**快捷键给出的动作，面板里必须有**——否则两种形态说的就不是同一件事，
    /// 用户切到面板版会发现"矩阵里没有这一格"。逐 Subject 取上游真实形态的路径来构造。
    #[test]
    fn shortcut_never_offers_an_action_the_panel_does_not() {
        let cases = [
            (Subject::App, "C:\\Windows\\notepad.exe"),
            (Subject::App, "calc.exe"),
            (
                Subject::StoreApp,
                "shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
            ),
            (Subject::Namespace, "shell:RecycleBinFolder"),
            (Subject::Url, "https://example.com/?q=1"),
            (Subject::Url, "ms-settings:display"),
            (Subject::File, "C:\\Windows\\notepad.exe"),
            (Subject::Folder, "C:\\Windows"),
            (Subject::Internal, "internal:settings"),
            (Subject::Text, "1,024"),
        ];
        for (subject, path) in cases {
            let t = target(subject, path);
            if let Some(id) = secondary_action_for(subject) {
                assert!(
                    has(&t, id),
                    "{subject:?}（{path}）的快捷键动作 {id:?} 在面板里不存在"
                );
            }
        }
    }
}


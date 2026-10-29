//! FR-SRC-17 系统命令分组。
//!
//! 2026-10-23 批次 A2 落地五组：**自身命令 / 电源与会话 / Windows 工具 / ms-settings 页 /
//! 音量与媒体**；剪贴板文本转换组按用户裁定整组暂缓（见发布说明第三十一节，登记为待办）。
//!
//! 落点是**出厂条目**（用户裁定「并进出厂条目」），不是新数据源：这些命令进清单后就是普通条目，
//! Tab 补全、置顶、学习位序、编辑、删除、`hidden[]` 隐藏全部沿用既有机制。
//!
//! 只有真·内置动作用 [`ItemKind::Internal`] + `internal:<id>` 路径——`internal:` 在
//! [`crate::source::application::is_url`] 眼里是个合法协议名，于是路径四态与目标名匹配
//! 都自动跳过它；真正的分派在 `main.rs::LauncherApp::exec_internal`（ShellExecute 起不了它）。
//! Windows 工具与 ms-settings 页仍然是普通 `Command` / `Url` 条目——它们本来就该由
//! ShellExecute 打开。

use crate::source::application::{
    ItemKind, LaunchItem, command_seed, seed_keyword, system32, url_seed,
};

/// 内置动作的路径前缀。改它等于把所有已播种条目的路径换掉（`uid` 也一起变），只能增不能改。
pub const SCHEME: &str = "internal:";

/// 一个真·内置动作。「设置」带页码（1=通用 2=功能 3=热键 4=数据源，与 FR-WIN-08 的页码同口径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalAction {
    Settings(u8),
    Reload,
    RebuildAppIndex,
    RebuildFileIndex,
    About,
    OpenLog,
    Quit,
    Lock,
    Sleep,
    Hibernate,
    Shutdown,
    Restart,
    Logoff,
    EmptyRecycleBin,
    MonitorOff,
    VolumeMute,
    VolumeUp,
    VolumeDown,
    MediaPlayPause,
    MediaNext,
    MediaPrev,
    MediaStop,
}

impl InternalAction {
    /// 稳定 id（写进条目 `path`，也进日志）。**只增不改**：改了等于另一条命令。
    pub fn id(self) -> String {
        match self {
            Self::Settings(page) => format!("settings:{page}"),
            Self::Reload => "reload".into(),
            Self::RebuildAppIndex => "rebuild-app-index".into(),
            Self::RebuildFileIndex => "rebuild-file-index".into(),
            Self::About => "about".into(),
            Self::OpenLog => "open-log".into(),
            Self::Quit => "quit".into(),
            Self::Lock => "lock".into(),
            Self::Sleep => "sleep".into(),
            Self::Hibernate => "hibernate".into(),
            Self::Shutdown => "shutdown".into(),
            Self::Restart => "restart".into(),
            Self::Logoff => "logoff".into(),
            Self::EmptyRecycleBin => "empty-recycle-bin".into(),
            Self::MonitorOff => "monitor-off".into(),
            Self::VolumeMute => "volume-mute".into(),
            Self::VolumeUp => "volume-up".into(),
            Self::VolumeDown => "volume-down".into(),
            Self::MediaPlayPause => "media-play-pause".into(),
            Self::MediaNext => "media-next".into(),
            Self::MediaPrev => "media-prev".into(),
            Self::MediaStop => "media-stop".into(),
        }
    }

    pub fn path(self) -> String {
        format!("{SCHEME}{}", self.id())
    }

    pub fn from_id(id: &str) -> Option<Self> {
        if let Some(rest) = id.strip_prefix("settings:") {
            let page: u8 = rest.parse().ok()?;
            return (1..=4).contains(&page).then_some(Self::Settings(page));
        }
        Some(match id {
            "reload" => Self::Reload,
            "rebuild-app-index" => Self::RebuildAppIndex,
            "rebuild-file-index" => Self::RebuildFileIndex,
            "about" => Self::About,
            "open-log" => Self::OpenLog,
            "quit" => Self::Quit,
            "lock" => Self::Lock,
            "sleep" => Self::Sleep,
            "hibernate" => Self::Hibernate,
            "shutdown" => Self::Shutdown,
            "restart" => Self::Restart,
            "logoff" => Self::Logoff,
            "empty-recycle-bin" => Self::EmptyRecycleBin,
            "monitor-off" => Self::MonitorOff,
            "volume-mute" => Self::VolumeMute,
            "volume-up" => Self::VolumeUp,
            "volume-down" => Self::VolumeDown,
            "media-play-pause" => Self::MediaPlayPause,
            "media-next" => Self::MediaNext,
            "media-prev" => Self::MediaPrev,
            "media-stop" => Self::MediaStop,
            _ => return None,
        })
    }

    /// 从条目 `path`（`internal:<id>`）解析。不是内置动作返回 `None`。
    pub fn from_path(path: &str) -> Option<Self> {
        Self::from_id(path.trim().strip_prefix(SCHEME)?)
    }

    /// 不可撤销的那四条：默认（`confirm_actions = true`）执行前必须弹一次确认。
    /// 只说"会不会丢工作"：锁屏、关显示器、调音量都能随手撤回或重来，不在此列。
    pub fn needs_confirm(self) -> bool {
        matches!(
            self,
            Self::Shutdown | Self::Restart | Self::Logoff | Self::EmptyRecycleBin
        )
    }

    /// 批次 F4：图标规格（写进条目的 `source_path`，语法见 [`crate::source::icon`]）。
    ///
    /// 资源 id 全部照抄 AHK 版 `Src\Providers\SystemProvider.ahk` 的 `add(...)` 第三参；
    /// 音量/媒体那七条在 AHK 是 `SndVol.exe`，这里沿用同一个文件当图标源。
    pub fn icon_spec(self) -> String {
        match self {
            // AHK: `res:imageres.dll,-114`（Preferences 与每一页共用）
            Self::Settings(_) => "res:imageres.dll,-114".to_string(),
            // AHK: Reload `-5311` / RebuildIndex `-8`
            Self::Reload => "res:imageres.dll,-5311".to_string(),
            Self::RebuildAppIndex | Self::RebuildFileIndex => "res:imageres.dll,-8".to_string(),
            // AHK: About 用程序自己的图标文件，取其内置回退 `-81`
            Self::About => "res:imageres.dll,-81".to_string(),
            Self::OpenLog => "res:imageres.dll,-102".to_string(),
            Self::Quit => "res:imageres.dll,-98".to_string(),
            Self::Lock => "res:imageres.dll,-59".to_string(),
            // AHK: Sleep / Hibernate / Shutdown / Restart 都是 `res:shell32.dll,-28`
            Self::Sleep | Self::Hibernate | Self::Shutdown | Self::Restart => {
                "res:shell32.dll,-28".to_string()
            }
            Self::Logoff => "res:imageres.dll,-45".to_string(),
            Self::EmptyRecycleBin => "res:shell32.dll,-32".to_string(),
            // AHK: MonitorOff 用 `System32\DisplaySwitch.exe`，音量与媒体七条用 `SndVol.exe`
            Self::MonitorOff => system32("DisplaySwitch.exe"),
            Self::VolumeMute
            | Self::VolumeUp
            | Self::VolumeDown
            | Self::MediaPlayPause
            | Self::MediaNext
            | Self::MediaPrev
            | Self::MediaStop => system32("SndVol.exe"),
        }
    }
}

/// 36 个 ms-settings 页共用的图标：AHK 用「设置」应用本身（`shell:AppsFolder\…`），
/// 这里退成 `imageres.dll` 里的设置图标——同一个图，但不依赖 AppID 解析成不成功。
const SETTINGS_PAGE_ICON: &str = "res:imageres.dll,-114";

fn action_seed(name: &str, action: InternalAction, explicit: &str) -> LaunchItem {
    LaunchItem {
        name: name.to_string(),
        path: action.path(),
        arguments: String::new(),
        working_dir: None,
        source_path: action.icon_spec(),
        kind: ItemKind::Internal,
        keyword: seed_keyword(name, explicit),
        ..Default::default()
    }
}

/// ms-settings 页（FR-SRC-17 第三组）。只收 Win10/11 都认的稳定 URI。
///
/// 名字统一带 `Win·` 前缀（2026-10-23 从「设置·」改来）：带「设置」二字的 36 条会被
/// 裸查询「设置」的前缀档 / 包含档整批命中，又与启动器自己的设置页同形，列表里分不清。
/// 现在敲「设置」只剩两个总入口（`Windows 设置` / `启动器设置`），要哪一页直接敲页名
/// （`显示`、`声音`），或者敲 `win` 把 36 条一次列出来。
///
/// `WLAN` / `VPN` 的名字里没有汉字，`default_keyword` 会给空串 ⇒ 显式别名都以 `win` 开头，
/// 否则敲 `win` 列不全（这是改名时唯一需要手工补前缀的两条）。
const SETTINGS_PAGES: [(&str, &str, &str); 36] = [
    ("Win·显示", "ms-settings:display", "display"),
    ("Win·夜间模式", "ms-settings:nightlight", "nightlight"),
    ("Win·声音", "ms-settings:sound", "sound audio"),
    ("Win·通知", "ms-settings:notifications", "notifications"),
    ("Win·电源与睡眠", "ms-settings:powersleep", "powersleep"),
    ("Win·电池", "ms-settings:battery", "battery"),
    ("Win·存储感知", "ms-settings:storagesense", "storage"),
    (
        "Win·应用与功能",
        "ms-settings:appsfeatures",
        "appsfeatures uninstall",
    ),
    (
        "Win·可选功能",
        "ms-settings:optionalfeatures",
        "optionalfeatures",
    ),
    ("Win·默认应用", "ms-settings:defaultapps", "defaultapps"),
    ("Win·启动应用", "ms-settings:startupapps", "startupapps"),
    ("Win·打印机", "ms-settings:printers", "printers print"),
    ("Win·蓝牙", "ms-settings:bluetooth", "bluetooth"),
    (
        "Win·已连接设备",
        "ms-settings:connecteddevices",
        "connecteddevices",
    ),
    ("Win·WLAN", "ms-settings:network-wifi", "winwlan wifi wlan"),
    ("Win·网络状态", "ms-settings:network", "networkstatus"),
    ("Win·VPN", "ms-settings:network-vpn", "winvpn vpn"),
    (
        "Win·飞行模式",
        "ms-settings:network-airplanemode",
        "airplanemode",
    ),
    (
        "Win·移动热点",
        "ms-settings:network-mobilehotspot",
        "mobilehotspot",
    ),
    ("Win·鼠标", "ms-settings:mousetouchpad", "mouse"),
    ("Win·键盘", "ms-settings:keyboard", "keyboard"),
    ("Win·输入", "ms-settings:typing", "typing"),
    (
        "Win·区域与语言",
        "ms-settings:regionlanguage",
        "regionlanguage language",
    ),
    ("Win·日期与时间", "ms-settings:dateandtime", "dateandtime"),
    ("Win·颜色", "ms-settings:colors", "colors"),
    ("Win·主题", "ms-settings:themes", "themes"),
    (
        "Win·个性化",
        "ms-settings:personalization",
        "personalization",
    ),
    ("Win·任务栏", "ms-settings:taskbar", "taskbar"),
    // 不能叫「Win·隐私」：与「Win·颜色」的拼音首字母同为 `winys`，出厂关键字会撞车。
    // 也不能叫「Win·隐私总览」：`winyszl` 跨词恰好拼出 `sz`，裸 `sz` 会把它带出来
    // （2026-10-23 改名时被单测 `bare_settings_query_only_shows_the_two_entrances` 抓到）。
    ("Win·隐私概览", "ms-settings:privacy", "privacy"),
    ("Win·隐私·位置", "ms-settings:privacy-location", "location"),
    ("Win·隐私·相机", "ms-settings:privacy-camera", "camera"),
    (
        "Win·隐私·麦克风",
        "ms-settings:privacy-microphone",
        "microphone",
    ),
    ("Win·恢复", "ms-settings:recovery", "recovery reset"),
    ("Win·激活", "ms-settings:activation", "activation"),
    ("Win·关于本机", "ms-settings:about", "aboutpc sysinfo"),
    // 剪贴板历史面板本身只有 Win+V 能开，而本项目不注入 Win 键（硬约束），
    // 所以这一条打开的是「剪贴板设置页」——Win+V 的开关就在那里。
    ("Win·剪贴板", "ms-settings:clipboard", "clipboard"),
];

/// Windows 工具（FR-SRC-17 第二组）。`exe` 同时当图标源；`.msc`/`.cpl` 在 System32 下，
/// 裸命令名由 `pathcheck` 走 PATH 查得到（System32 默认在 PATH 里）。
const TOOLS: [(&str, &str, &str); 20] = [
    ("设备管理器", "devmgmt.msc", "devmgmt device"),
    ("磁盘管理", "diskmgmt.msc", "diskmgmt"),
    ("计算机管理", "compmgmt.msc", "compmgmt"),
    ("服务管理器", "services.msc", "services"),
    ("任务计划程序", "taskschd.msc", "taskschd task"),
    ("事件查看器", "eventvwr.msc", "eventvwr event"),
    ("资源监视器", "resmon.exe", "resmon"),
    ("性能监视器", "perfmon.exe", "perfmon"),
    ("系统信息", "msinfo32.exe", "msinfo"),
    ("系统配置", "msconfig.exe", "msconfig"),
    ("注册表编辑器", "regedit.exe", "regedit registry"),
    ("系统还原", "rstrui.exe", "rstrui restore"),
    ("磁盘清理", "cleanmgr.exe", "cleanmgr"),
    ("DirectX 诊断", "dxdiag.exe", "dxdiag directx"),
    ("远程桌面连接", "mstsc.exe", "mstsc rdp"),
    ("字符映射表", "charmap.exe", "charmap"),
    ("放大镜", "magnify.exe", "magnify"),
    ("屏幕键盘", "osk.exe", "osk onscreen"),
    ("网络连接", "ncpa.cpl", "ncpa"),
    ("程序和功能", "appwiz.cpl", "appwiz"),
];

/// 整组出厂条目。顺序即插进清单的次序（自身 → 电源 → 工具 → 设置页 → 音量媒体）。
pub fn system_seeds() -> Vec<LaunchItem> {
    let mut seeds = vec![
        // ---- 自身命令 ----
        // 2026-10-23：这四条从「设置·X」改名，和 Windows 的设置页分开。第 1 条就是
        // 「启动器设置」本身（打开通用页），另外三条退到「启动器·X」下，敲「设置」时
        // 只剩一个入口，不再四条平铺在 Windows 页中间。
        action_seed("启动器设置", InternalAction::Settings(1), "settings"),
        action_seed("启动器·功能", InternalAction::Settings(2), "features"),
        action_seed("启动器·热键", InternalAction::Settings(3), "hotkeys"),
        action_seed("启动器·数据源", InternalAction::Settings(4), "sources"),
        action_seed("重新加载配置", InternalAction::Reload, "reload"),
        action_seed(
            "重建应用索引",
            InternalAction::RebuildAppIndex,
            "rebuildapp reindexapp",
        ),
        action_seed(
            "重建文件索引",
            InternalAction::RebuildFileIndex,
            "rebuildfile reindexfile",
        ),
        action_seed("关于本程序", InternalAction::About, "about"),
        action_seed("打开日志", InternalAction::OpenLog, "log"),
        action_seed("退出启动器", InternalAction::Quit, "quit exit"),
        // ---- 电源与会话 ----
        action_seed("锁定屏幕", InternalAction::Lock, "lock"),
        action_seed("睡眠", InternalAction::Sleep, "sleep"),
        action_seed("休眠", InternalAction::Hibernate, "hibernate"),
        action_seed("关机", InternalAction::Shutdown, "shutdown"),
        action_seed("重启", InternalAction::Restart, "restart reboot"),
        action_seed("注销", InternalAction::Logoff, "logoff signout"),
        action_seed("清空回收站", InternalAction::EmptyRecycleBin, "recycle"),
        action_seed("关闭显示器", InternalAction::MonitorOff, "monitoroff"),
    ];

    for (name, exe, explicit) in TOOLS {
        seeds.push(command_seed(name, exe, "", exe, explicit));
    }
    // 附录 C 点名的三条带参数项。运行对话框走 shell 命令 CLSID 而不是 Win+R ——
    // 本项目不注入 Win 键；`{2559…}` 不是 `{query}` 占位符，参数原样交给 explorer。
    seeds.push(command_seed(
        "资源管理器",
        "explorer.exe",
        "",
        "explorer.exe",
        "explorer files",
    ));
    seeds.push(command_seed(
        "运行对话框",
        "explorer.exe",
        "shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}",
        "explorer.exe",
        "runbox",
    ));
    seeds.push(command_seed(
        "进程列表",
        "cmd.exe",
        "/k tasklist",
        "cmd.exe",
        "tasklist processlist",
    ));

    for (name, uri, explicit) in SETTINGS_PAGES {
        let mut it = url_seed(name, uri, explicit);
        it.source_path = SETTINGS_PAGE_ICON.to_string();
        seeds.push(it);
    }

    // ---- 音量与媒体 ----
    seeds.extend([
        action_seed("静音", InternalAction::VolumeMute, "mute"),
        action_seed("增加音量", InternalAction::VolumeUp, "volup"),
        action_seed("降低音量", InternalAction::VolumeDown, "voldown"),
        action_seed(
            "播放/暂停",
            InternalAction::MediaPlayPause,
            "playpause media",
        ),
        action_seed("下一曲", InternalAction::MediaNext, "next"),
        action_seed("上一曲", InternalAction::MediaPrev, "prev previous"),
        action_seed("停止播放", InternalAction::MediaStop, "stop"),
    ]);

    seeds
}

/// 2026-10-23 一次性改名（见 [`SETTINGS_PAGES`] 的注）：把清单里还在用旧名的出厂条目
/// 换成新名，并把关键字重算成出厂值——旧关键字里的 `sz…` 前缀正是「敲 `sz` 出来四十行」
/// 的原因，而「提交即学别名」还往其中几条塞过「设置」，一并清掉。
///
/// **只改名字仍是出厂旧名的那条**（uid 相同且名称逐字相等）：用户自己改过名的不动。
/// uid 不含名称，所以改名不会产生新条目，置顶与使用统计都留在原地。返回改了几条。
/// 幂等：改完之后旧名不再匹配，第二次调用返回 0。
pub fn rename_legacy_settings(items: &mut [LaunchItem]) -> usize {
    let mut changed = 0;
    for seed in crate::source::application::builtin_seeds() {
        let Some(old) = legacy_name(&seed.path, &seed.name) else {
            continue;
        };
        let Some(item) = items.iter_mut().find(|i| i.uid() == seed.uid()) else {
            continue;
        };
        if item.name.trim() == old {
            item.name = seed.name;
            item.keyword = seed.keyword;
            changed += 1;
        }
    }
    changed
}

/// 旧名 → 新名的对照。只有这批条目改过名，所以这里按 path 白名单而不是通用规则推导。
fn legacy_name(path: &str, new_name: &str) -> Option<String> {
    match path {
        // 总入口在 `application.rs` 的底座种子里，路径只有协议本身。
        "ms-settings:" => Some("设置".to_string()),
        "internal:settings:1" => Some("设置·通用".to_string()),
        "internal:settings:2" => Some("设置·功能".to_string()),
        "internal:settings:3" => Some("设置·热键".to_string()),
        "internal:settings:4" => Some("设置·数据源".to_string()),
        // 这一条不只换前缀还换了词（「总览」→「概览」，避 `sz` 跨词命中），旧名单独列出。
        "ms-settings:privacy" => Some("设置·隐私总览".to_string()),
        _ => new_name
            .strip_prefix("Win·")
            .map(|tail| format!("设置·{tail}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn action_ids_round_trip() {
        let all = [
            InternalAction::Settings(1),
            InternalAction::Settings(4),
            InternalAction::Reload,
            InternalAction::RebuildAppIndex,
            InternalAction::RebuildFileIndex,
            InternalAction::About,
            InternalAction::OpenLog,
            InternalAction::Quit,
            InternalAction::Lock,
            InternalAction::Shutdown,
            InternalAction::Restart,
            InternalAction::Logoff,
            InternalAction::EmptyRecycleBin,
            InternalAction::MonitorOff,
            InternalAction::VolumeMute,
            InternalAction::VolumeUp,
            InternalAction::VolumeDown,
            InternalAction::MediaPlayPause,
            InternalAction::MediaNext,
            InternalAction::MediaPrev,
            InternalAction::MediaStop,
        ];
        for a in all {
            assert_eq!(InternalAction::from_id(&a.id()), Some(a), "{}", a.id());
            assert_eq!(
                InternalAction::from_path(&a.path()),
                Some(a),
                "{}",
                a.path()
            );
        }
        assert_eq!(
            InternalAction::from_id("settings:0"),
            None,
            "页码只有 1..=4"
        );
        assert_eq!(InternalAction::from_id("settings:5"), None);
        assert_eq!(InternalAction::from_id("nope"), None);
        assert_eq!(InternalAction::from_path("internal:nope"), None);
        assert_eq!(InternalAction::from_path("notepad.exe"), None);
    }

    #[test]
    fn only_the_four_irreversible_actions_ask_first() {
        let gated: HashSet<String> = system_seeds()
            .iter()
            .filter_map(|i| InternalAction::from_path(&i.path))
            .filter(|a| a.needs_confirm())
            .map(|a| a.id())
            .collect();
        let want: HashSet<String> = ["shutdown", "restart", "logoff", "empty-recycle-bin"]
            .map(str::to_string)
            .into();
        assert_eq!(gated, want);
    }

    #[test]
    fn every_group_is_actually_present() {
        let seeds = system_seeds();
        let count = |f: &dyn Fn(&LaunchItem) -> bool| seeds.iter().filter(|i| f(i)).count();
        assert_eq!(
            count(&|i| i.kind == ItemKind::Internal && i.path.contains("settings:")),
            4,
            "四条设置页直命令"
        );
        assert!(
            count(&|i| i.kind == ItemKind::Internal) >= 20,
            "内置动作要覆盖自身/电源/媒体三组"
        );
        // 表里的 20 条 + 附录 C 点名的三条带参数项（资源管理器 / 运行对话框 / 进程列表）。
        assert_eq!(
            count(&|i| i.path.ends_with(".msc")
                || i.path.ends_with(".cpl")
                || i.path.ends_with(".exe")),
            TOOLS.len() + 3,
            "Windows 工具一条不少"
        );
        for id in [
            "internal:lock",
            "internal:sleep",
            "internal:hibernate",
            "internal:monitor-off",
        ] {
            assert!(count(&|i| i.path == id) == 1, "电源与会话组缺 {id}");
        }
        assert_eq!(
            count(&|i| i.path.starts_with("ms-settings:")),
            SETTINGS_PAGES.len(),
            "ms-settings 页一条不少"
        );
        assert!(
            count(&|i| i.path.starts_with("ms-settings:")) >= 30,
            "需求书要求至少 40 页里的稳定子集"
        );
        assert!(count(&|i| i.keyword.contains("mute")) == 1, "音量组在");
    }

    /// 2026-10-23 用户反馈「敲『设置』出来四十行」：改名之后裸「设置」/`sz` 只该出两个入口，
    /// 36 条 `Win·X` 一条都不许再被它们带出来；具体页仍然要能被页名敲到。
    #[test]
    fn bare_settings_query_only_shows_the_two_entrances() {
        let items = crate::source::application::builtin_seeds();
        let mut index = crate::matching::MatchIndex::default();
        let rank = crate::source::application::LearnRank::Count;
        let mut names = |q: &str| -> Vec<String> {
            index
                .search(q, &items, rank, 0)
                .into_iter()
                .map(|i| items[i].name.clone())
                .collect()
        };
        for q in ["设置", "sz"] {
            let got = names(q);
            assert!(
                got.contains(&"Windows 设置".to_string()),
                "{q} 该出 Windows 设置：{got:?}"
            );
            assert!(
                got.contains(&"启动器设置".to_string()),
                "{q} 该出启动器设置：{got:?}"
            );
            assert!(
                !got.iter().any(|n| n.starts_with("Win·")),
                "{q} 不该把 36 条设置页铺出来：{got:?}"
            );
        }
        // 页名本身仍然直达。
        assert!(names("显示").contains(&"Win·显示".to_string()));
        assert!(names("声音").contains(&"Win·声音".to_string()));
        // `win` 是这批页面的集合词，36 条要一次列全（WLAN/VPN 靠显式别名补上 win 前缀）。
        assert_eq!(
            names("win")
                .iter()
                .filter(|n| n.starts_with("Win·"))
                .count(),
            SETTINGS_PAGES.len(),
            "敲 win 该列全部设置页"
        );
    }

    #[test]
    fn legacy_names_are_renamed_once_and_user_renames_survive() {
        let mut items = crate::source::application::builtin_seeds();
        for it in items.iter_mut() {
            match it.path.as_str() {
                "ms-settings:" => {
                    it.name = "设置".to_string();
                    it.keyword = "sz".to_string();
                }
                // 模拟「提交即学别名」污染的旧关键字。
                "ms-settings:sound" => {
                    it.name = "设置·声音".to_string();
                    it.keyword = "szsy sound audio 设置".to_string();
                }
                "internal:settings:3" => it.name = "设置·热键".to_string(),
                _ => {}
            }
        }
        let changed = rename_legacy_settings(&mut items);
        assert_eq!(changed, 3, "总入口 + 声音页 + 启动器·热键");

        let find = |p: &str| items.iter().find(|i| i.path == p).unwrap();
        assert_eq!(find("ms-settings:").name, "Windows 设置");
        assert_eq!(find("ms-settings:sound").name, "Win·声音");
        assert_eq!(
            find("ms-settings:sound").keyword,
            "winsy sound audio",
            "旧关键字要按出厂值重算"
        );
        assert_eq!(find("internal:settings:3").name, "启动器·热键");

        assert_eq!(rename_legacy_settings(&mut items), 0, "改名必须幂等");
    }

    #[test]
    fn seeds_have_no_duplicate_uid_or_name() {
        let seeds = system_seeds();
        let mut uids = HashSet::new();
        let mut names = HashSet::new();
        for s in &seeds {
            assert!(uids.insert(s.uid()), "uid 重复：{}", s.uid());
            assert!(names.insert(s.name.clone()), "名称重复：{}", s.name);
            assert!(
                !s.keyword.trim().is_empty(),
                "每条都该有可敲的别名：{}",
                s.name
            );
        }
    }

    #[test]
    fn internal_seeds_carry_a_known_action() {
        for s in system_seeds() {
            if s.kind == ItemKind::Internal {
                assert!(
                    InternalAction::from_path(&s.path).is_some(),
                    "内置条目的路径必须能解析回动作：{} {}",
                    s.name,
                    s.path
                );
                assert!(s.path.starts_with(SCHEME));
                // 批次 F4：内置动作从这一轮起都带图标源（资源 id 照抄 AHK 版映射）。
                assert!(
                    crate::source::icon::parse_spec(&s.source_path).is_some(),
                    "内置动作必须有图标源：{}",
                    s.name
                );
            }
        }
    }

    /// 批次 F4：36 个设置页共用一条 `res:` 规格 ⇒ 共用一个纹理缓存键（AHK 同口径）。
    #[test]
    fn settings_pages_share_one_icon_key() {
        let seeds = system_seeds();
        let pages: Vec<&LaunchItem> = seeds
            .iter()
            .filter(|s| s.path.starts_with("ms-settings:"))
            .collect();
        assert_eq!(pages.len(), 36);
        let keys: std::collections::HashSet<String> = pages
            .iter()
            .filter_map(|s| crate::source::icon::cache_key(&s.source_path))
            .collect();
        assert_eq!(keys.len(), 1, "36 页只该有一个缓存键，实得 {keys:?}");
    }
}

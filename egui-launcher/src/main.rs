// 本程序是 GUI 应用 ⇒ 声明成「窗口子系统」。默认的控制台子系统会让系统在每次「没有继承到
// 控制台」的启动（登录自启、资源管理器双击）时替它新建一个控制台窗，把 `eprintln!` 日志
// 刷在一个黑窗里；关掉那个黑窗还会顺带把进程一起终止。改成窗口子系统后不再有黑窗，
// 日志改由 redirect_stderr_if_orphan() 兜到 launcher.log。
#![windows_subsystem = "windows"]

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::os::windows::io::AsRawHandle;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, GlobalFree, HANDLE, HINSTANCE, HWND, LPARAM,
    LRESULT, POINT, RECT, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, SetStdHandle};
use windows::core::PCWSTR;
// FR-SRC-10：计算器结果行 Enter ＝「把纯数字写进剪贴板」。三件套在 DataExchange，
// GlobalAlloc/GlobalLock 在 Memory（该模块没有 feature 门，已随 Win32_Foundation 白送）。
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
// 批次 A2（FR-SRC-17）「锁定屏幕」。走 API 而不是 `rundll32 user32.dll,LockWorkStation`：
// 少起一个进程，也就没有控制台窗口闪烁那一档事。
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
    WaitForSingleObject,
};
use windows::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_CHAR, MapVirtualKeyW, RegisterHotKey,
    SendInput, UnregisterHotKey, VIRTUAL_KEY, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE,
    VK_MEDIA_PREV_TRACK, VK_MEDIA_STOP, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI,
    SHERB_NOSOUND, SHEmptyRecycleBinW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, ChangeWindowMessageFilterEx, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GetClassNameW, GetCursorPos,
    GetForegroundWindow, GetMessageW, GetWindowRect, GetWindowThreadProcessId, HWND_BROADCAST,
    HWND_MESSAGE, IsIconic, KillTimer, MSG, MSGFLT_ALLOW, PostMessageW, PostQuitMessage,
    RegisterClassExW, SC_MONITORPOWER, SMTO_ABORTIFHUNG, SW_RESTORE, SW_SHOWNORMAL, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOZORDER, SendMessageTimeoutW, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COPYDATA, WM_DESTROY, WM_HOTKEY,
    WM_SYSCOMMAND, WM_TIMER, WNDCLASSEXW,
};

/// 全局呼出 / 隐藏那一组（`RegisterHotKey` 的 id）。批次 R 起系统里同时最多装着两个热键，
/// id 必须分开：`WM_HOTKEY` 的 `wparam` 就是它，靠它区分"该呼出窗口"还是"该补一个字符"。
const HOTKEY_TOGGLE: i32 = 1;
/// 补字符热键（批次 R，2026-10-20 用户追加需求）：按下它把被裸键吞掉的那个字符投给焦点窗口。
const HOTKEY_TYPEBACK: i32 = 2;

mod cli;
mod hud;
mod matching;
mod source;
mod theme;
use source::application::{
    Corrupted, ItemKind, LaunchItem, LearnRank, ListRows, RowDensity, Scope, SpaceRun, TabMode,
    dedup_items, default_keyword, is_url, learn_alias, make_item_from_input,
};
// FR-WIN-02（批次 Q）：热键的位值与 VK 码用 application 侧的那套常量，判据与展示名才能
// 在纯函数里单测；调 RegisterHotKey 时再包成 HOT_KEY_MODIFIERS。
// 批次 R 起 `HKF_WIN` 运行期也要用了：补字符键的四个复选框（照 Delphi 版 frmConfig 的形态）
// 是这台机器上唯一能把 Win 位设进去的入口——录制路径产不出它，见 `spec_from_press` 的注释。
use source::application::{
    ERROR_HOTKEY_ALREADY_REGISTERED, HKF_ALT, HKF_CONTROL, HKF_NOREPEAT, HKF_SHIFT, HKF_WIN,
    HotkeySpec, VK_0, VK_9, VK_A, VK_F1, VK_F24, VK_OEM_3, VK_RETURN, VK_SPACE, VK_Z,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// 批次 A10：行模型与列表计划搬进了数据源层；这里保留旧名，五千行 UI 代码一个字不用改。
use source::{SearchPlan as Plan, SourceRow as HeadRow};

// FR-WIN-04 失焦关闭到托盘：焦点丢失后延迟这么久再复查关闭（AHK 参考 _OnActivate 用 50ms 复查；
// 不用 0ms 是为避开右键菜单/瞬态焦点抖动导致的误关闭）。
// 术语：本程序「关闭到托盘」= ViewportCommand::Visible(false)，窗口消失、进程仍常驻托盘，
// 可用 Ctrl+Alt+Space 或托盘「显示」再呼出；只有托盘「退出」才真正结束进程。
const HIDE_DELAY: Duration = Duration::from_millis(50);

// FR-ACT-02 启动失败提示的停留时长（M1 验收「有提示」；到点自动消失，成功启动即清空）。
// 2026-10-17 泛化为通用 Toast（SendTo 安装结果也走它），故改名 TOAST_*。
const TOAST_TTL: Duration = Duration::from_secs(3);
// Toast 尾段淡出时长。
const TOAST_FADE: Duration = Duration::from_millis(400);
const TOAST_ERR: egui::Color32 = egui::Color32::from_rgb(255, 156, 146);
const TOAST_OK: egui::Color32 = egui::Color32::from_rgb(140, 220, 150);
/// FR-SRC-06：可见期内重算路径四态的最小间隔。拔插 U 盘不需要秒级响应，
/// 但用户往往"插上就呼出"，3 秒足够避开窗口刚打开那一瞬。
const PATH_RESCAN_INTERVAL: Duration = Duration::from_secs(3);

// FR-SRC-21 单实例：命名互斥体 + WM_COPYDATA 转发
const SINGLE_INSTANCE_MUTEX: &str = "Local\\XiShan.EguiLauncher.SingleInstance";
// 主实例独有的隐藏消息窗类名（热键线程建），二次进程按类名 FindWindowW 定位它。
const IPC_WINDOW_CLASS: &str = "EguiLauncherHotkeyClass";
// WM_COPYDATA 的 dwData 操作码
const IPC_OP_ACTIVATE: usize = 0;
const IPC_OP_ADD_PATH: usize = 1;
const IPC_OP_PING: usize = 2;
const IPC_OP_PREFERENCES: usize = 3;
const IPC_OP_ADD_BATCH: usize = 4;
/// 路径列表常见最多几十条；64KB 足够，同时挡住畸形消息。
const IPC_MAX_PAYLOAD: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpcSendStatus {
    Ack,
    NoWindow,
    Timeout,
}

/// 二次进程转发给主实例的消息（经静态通道进主循环）。
enum IpcMsg {
    Activate,
    AddPath(String),
    Ping,
    Preferences(u32),
    AddBatch(Vec<String>),
}

static IPC_TX: std::sync::OnceLock<mpsc::Sender<IpcMsg>> = std::sync::OnceLock::new();

/// 「发送到」递来的路径 → 弹窗预填（FR-SRC-20① 增强，2026-10-17）：
/// 存在的 `.lnk` 先解析真实目标（`resolve_lnk` 内已带 E-03 四条约束），命令行写实际 exe、
/// 图标源保留 .lnk；解析失败/非 .lnk 原样预填。CoInit 用完即 Uninit，仅此一瞬态调用。
fn dialog_for_sent_path(p: &str) -> DialogState {
    if p.to_lowercase().ends_with(".lnk") {
        let path = std::path::Path::new(p);
        if path.exists() {
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            let resolved = unsafe { source::application::resolve_lnk(path) };
            if hr.is_ok() {
                unsafe { CoUninitialize() };
            }
            if let Some(it) = resolved {
                eprintln!(
                    "FR-SRC-20①: .lnk 解析成功 {} → target={:?} args={:?} wd={:?}",
                    p, it.path, it.arguments, it.working_dir
                );
                return DialogState::from_sent(it);
            }
            eprintln!("FR-SRC-20①: .lnk 解析失败（空 target/损坏），按原路径预填 {p}");
        }
    }
    DialogState::from_input(p)
}

/// 抢命名互斥体：Some=本进程是主实例（句柄需活到进程退出）；None=已有实例。
fn acquire_single_instance() -> Option<HANDLE> {
    let name: Vec<u16> = SINGLE_INSTANCE_MUTEX
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        match CreateMutexW(None, false, PCWSTR(name.as_ptr())) {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    // 次实例只是借句柄探测存在性，必须立刻关掉：否则旧实例被杀后，
                    // 这个句柄仍会让命名互斥体活着，接管方会一直看到「已有实例」。
                    let _ = CloseHandle(h);
                    None
                } else {
                    Some(h)
                }
            }
            // API 失败不阻塞启动（按主实例继续）。
            Err(_) => Some(HANDLE::default()),
        }
    }
}

fn wait_for_mutex(timeout: Duration) -> Option<HANDLE> {
    let start = Instant::now();
    loop {
        if let Some(h) = acquire_single_instance() {
            return Some(h);
        }
        if start.elapsed() >= timeout {
            return None;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// 与系统 `COPYDATASTRUCT`（WM_COPYDATA 的 lParam）逐字段布局兼容。
/// 官方绑定在 `Win32_System_DataExchange` 模块，本地定义以免多开一个 feature 编译面。
#[repr(C)]
struct CopyDataStruct {
    dw_data: usize,
    cb_data: u32,
    lp_data: *mut core::ffi::c_void,
}

/// 从 WM_COPYDATA 载荷复制 UTF-16 字符串；畸形载荷返回 None（不信任外部进程）。
unsafe fn copy_data_payload(cds: &CopyDataStruct) -> Option<String> {
    if cds.lp_data.is_null()
        || cds.cb_data < 2
        || cds.cb_data as usize > IPC_MAX_PAYLOAD
        || cds.cb_data % 2 != 0
    {
        return None;
    }
    let units = unsafe {
        std::slice::from_raw_parts(cds.lp_data as *const u16, cds.cb_data as usize / 2 - 1)
    };
    Some(String::from_utf16_lossy(units))
}

/// 向运行中实例发一条 WM_COPYDATA；2s 超时（SMTO_ABORTIFHUNG）。
/// NoWindow=消息窗还没出现（旧实例可能在启动中）；Timeout=窗口在但没应答（按卡死处理）。
fn send_ipc_op(opcode: usize, payload: &str) -> IpcSendStatus {
    let class: Vec<u16> = IPC_WINDOW_CLASS
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let Ok(hwnd) = (unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) }) else {
        return IpcSendStatus::NoWindow;
    };
    let mut wide: Vec<u16> = payload.encode_utf16().chain(std::iter::once(0)).collect();
    let cds = CopyDataStruct {
        dw_data: opcode,
        cb_data: (wide.len() * 2) as u32,
        lp_data: wide.as_mut_ptr().cast(),
    };
    // 绑定返回 LRESULT（底层是 BOOL 语义）：0=超时/目标悬挂被弃；本方 WndProc 处理后返回 1。
    let acked = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            2000,
            None,
        )
        .0 != 0
    };
    if acked {
        IpcSendStatus::Ack
    } else {
        IpcSendStatus::Timeout
    }
}

/// FR-WIN-07：旧实例对 IPC 无应答 ⇒ 按卡死处理，结束它，供本进程接管。
/// 只认本程序的消息窗类名，避免误杀其它进程。
fn kill_hung_instance() -> bool {
    let class: Vec<u16> = IPC_WINDOW_CLASS
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let Ok(hwnd) = (unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) }) else {
        eprintln!("single instance: 找不到旧实例的消息窗，无法结束它");
        return false;
    };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 || pid == std::process::id() {
        eprintln!("single instance: 消息窗 PID 异常（{pid}），放弃结束");
        return false;
    }
    unsafe {
        let proc = match OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) {
            Ok(h) => h,
            Err(e) => {
                eprintln!(
                    "single instance: 结束旧实例 pid={pid} 失败（OpenProcess: {e}；可能权限不同）"
                );
                return false;
            }
        };
        let term = TerminateProcess(proc, 1);
        let wait = WaitForSingleObject(proc, 2000);
        let _ = CloseHandle(proc);
        if term.is_err() || wait != WAIT_OBJECT_0 {
            eprintln!("single instance: 结束旧实例 pid={pid} 失败（term={term:?}, wait={wait:?}）");
            return false;
        }
    }
    eprintln!("single instance: 旧实例 pid={pid} 对 IPC 无应答，已结束");
    true
}

/// FR-WIN-08：把命令行意图转成给运行实例的一条 IPC。
/// 1 条路径走预填弹窗协议，多条走批量；`-Preferences` 优先于裸 `-Startup`。
fn forward_cli(cli: &cli::Cli) -> IpcSendStatus {
    if cli.send_to.len() == 1 {
        send_ipc_op(IPC_OP_ADD_PATH, &cli.send_to[0])
    } else if cli.send_to.len() > 1 {
        send_ipc_op(IPC_OP_ADD_BATCH, &cli.send_to.join("\n"))
    } else if let Some(page) = cli.preferences {
        send_ipc_op(IPC_OP_PREFERENCES, &page.to_string())
    } else if cli.startup || cli.reloaded {
        // 静默启动/重启：运行实例只需知道「还有一份自己在启动」，不弹窗。
        send_ipc_op(IPC_OP_PING, "")
    } else {
        send_ipc_op(IPC_OP_ACTIVATE, "")
    }
}

// FR-WIN-10 / E-05 IME 控制常量（WM_IME_CONTROL = 0x0283）
const WM_IME_CONTROL: u32 = 0x0283;
const IMC_GETCONVERSIONMODE: usize = 0x0001;
const IMC_SETCONVERSIONMODE: usize = 0x0002;
const IMC_GETOPENSTATUS: usize = 0x0005;
const IMC_SETOPENSTATUS: usize = 0x0006;
const IME_CMODE_NATIVE: usize = 0x0001; // 中文位

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayEvent {
    ShowWindow,
    BatchImport,
    Settings, // 2026-10-17：SendTo 从托盘直连改为设置页开关，托盘入口统一为「设置…」
    OpenLog,  // 批次 P（FR-SYS-03）：需求书托盘原文「打开日志、复制系统信息、退出」里的第一格
    Exit,
}

/// 批次 F6：顶栏 ☰ 主菜单里的六个动作。
///
/// 为什么是一个"返回枚举"而不是在 Popup 的闭包里直接改 `LauncherApp`：
/// `egui::Popup::open_bool(&mut self.menu_open)` 把 `self` 可变借到了整个 `show()` 期间，
/// 闭包里再写 `self.dialog = …` 就是二次可变借用（编译不过）。所以内容闭包只读 `ui`、
/// 返回本枚举，动作在 `show()` 返回、借用结束之后再落。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    None,
    NewItem,
    BatchImport,
    Settings,
    Cheatsheet,
    OpenLog,
    Exit,
}

/// ☰ 菜单的内容（批次 F6）。文案里的快捷键与 `LOCAL_TAKEN`、`ui()` 里的 `consume_key` 同源，
/// 改键要三处一起改——所以这里刻意只写"键名"，不复述语义。
fn main_menu_content(ui: &mut egui::Ui) -> MenuAction {
    let item = |ui: &mut egui::Ui, text: &str| ui.button(text).clicked();
    if item(ui, "＋  新建快捷项        Ctrl+Shift+N") {
        MenuAction::NewItem
    } else if item(ui, "⟳  批量导入全部    Ctrl+I") {
        MenuAction::BatchImport
    } else if item(ui, "⚙  启动器设置      Ctrl+,") {
        MenuAction::Settings
    } else if item(ui, "❓  速查表          输入 ? 后点行") {
        MenuAction::Cheatsheet
    } else if item(ui, "📄  打开日志") {
        MenuAction::OpenLog
    } else {
        ui.separator();
        if item(ui, "✕  退出") {
            MenuAction::Exit
        } else {
            MenuAction::None
        }
    }
}

/// 设置页分类（对标 AHK 版设置窗口的左侧分类栏，先立当下有真实选项的分类）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Features,
    Hotkeys,
    /// 批次 A10：每个数据源一行开关。这一页的条目由注册表渲染，不在这里硬编。
    Sources,
    /// 批次 F1（FR-UIX-01）：主题选择 + 主题目录入口。第 5 页。
    Appearance,
    /// 批次 F7（D3 + D6）：重置学习排序 / 恢复默认 / 隐藏的条目。第 6 页。
    /// 这一页只有破坏性动作与低频维护项，所以排在最后。
    Advanced,
}

impl SettingsTab {
    /// FR-WIN-08：页码从 1 起（1=通用 2=功能 3=热键 4=数据源 5=外观 6=高级）；
    /// 越界返回 None，由调用方落回通用。
    fn from_page(page: u32) -> Option<Self> {
        match page {
            1 => Some(Self::General),
            2 => Some(Self::Features),
            3 => Some(Self::Hotkeys),
            4 => Some(Self::Sources),
            5 => Some(Self::Appearance),
            6 => Some(Self::Advanced),
            _ => None,
        }
    }
}

/// 批次 F7：设置页「高级」页的两个破坏性动作。
/// 与删除确认分开：`ConfirmRequest` 已被真机验过，不动它。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destructive {
    /// FR-CFG-07：清空学习排序数据（置顶保留）。
    ResetLearning,
    /// FR-CFG-08：先写 `.bak` 再把偏好恢复默认（用户资产保留）。
    RestoreDefaults,
}

// FR-SRC-23【2026-10-17 修订】：无匹配 Enter 已从「直接运行兜底」改为弹 AddDialog 预填入库，
// 原 FallbackAction 删除。

/// 添加/编辑对话框状态（FR-SRC-20 `AddDialog(prefill)`，对标 Delphi frmShortCut）。
/// `edit_uid=Some(u)` 表示编辑既有项（按派生 uid 定位替换），`None` 表示新建。
/// 身份用 uid 而非 `path`：同一 exe 不同参数是两个条目，按 path 会认错（2026-10-17）。
struct DialogState {
    edit_uid: Option<String>,
    name: String,
    keyword: String,
    path: String,
    arguments: String,
    working_dir: String,
    kind: ItemKind,
    /// 图标源（不显示在表单里）：SendTo 的 .lnk 解析后=lnk 本体（可能带自定义图标）；
    /// 空=提交时回落取命令行。编辑既有项时带入原 source_path 保真。
    source_path: String,
    /// 批次 G1 分类标签的自由文本（逗号/空白分隔），提交时由 [`source::application::parse_tags_text`] 拆表。
    tags_text: String,
}

impl DialogState {
    fn blank() -> Self {
        Self {
            edit_uid: None,
            name: String::new(),
            keyword: String::new(),
            path: String::new(),
            arguments: String::new(),
            working_dir: String::new(),
            kind: ItemKind::Command,
            source_path: String::new(),
            tags_text: String::new(),
        }
    }
    /// 用无匹配输入预填（路径/类型/名称由 make_item_from_input 推断）。
    fn from_input(input: &str) -> Self {
        let it = make_item_from_input(input);
        Self {
            edit_uid: None,
            name: it.name,
            keyword: String::new(),
            path: it.path,
            arguments: String::new(),
            working_dir: String::new(),
            kind: it.kind,
            source_path: String::new(),
            tags_text: String::new(),
        }
    }
    /// 用「发送到」转发的 .lnk 解析结果预填：命令行=真实目标，图标源=lnk 本体。
    fn from_sent(it: LaunchItem) -> Self {
        Self {
            edit_uid: None,
            name: it.name,
            keyword: String::new(),
            path: it.path,
            arguments: it.arguments,
            working_dir: it.working_dir.unwrap_or_default(),
            kind: it.kind,
            source_path: it.source_path,
            tags_text: String::new(),
        }
    }
    /// 批次 X（FR-SRC-14 → FR-SRC-20）：从文件搜索行预填「添加快捷项」。
    /// 别名留空 ⇒ 提交时 `to_item` 按名称自动生成拼音首字母，与手工新建同一条路。
    /// `source_path` 直接给文件本体：这一行的图标就是它自己的图标。
    fn from_file_hit(hit: &source::filesearch::FileHit) -> Self {
        Self {
            edit_uid: None,
            name: hit.title.clone(),
            keyword: String::new(),
            path: hit.path.clone(),
            arguments: String::new(),
            working_dir: String::new(),
            kind: if hit.is_dir {
                ItemKind::Folder
            } else {
                ItemKind::File
            },
            source_path: hit.path.clone(),
            tags_text: String::new(),
        }
    }
    /// 用既有项预填（编辑）。
    fn from_item(it: &LaunchItem) -> Self {
        Self {
            edit_uid: Some(it.uid()),
            name: it.name.clone(),
            keyword: it.keyword.clone(),
            path: it.path.clone(),
            arguments: it.arguments.clone(),
            working_dir: it.working_dir.clone().unwrap_or_default(),
            kind: it.kind,
            source_path: it.source_path.clone(),
            tags_text: it.tags.join(" "),
        }
    }
    /// 提交时生成条目；`source_path` 默认取 `path`（图标源，URL/无文件时 extract 自然失败退化无图标）。
    fn to_item(&self) -> LaunchItem {
        let name = self.name.trim().to_string();
        let name = if name.is_empty() {
            self.path
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or(&self.path)
                .trim_end_matches(".lnk")
                .to_string()
        } else {
            name
        };
        let wd = self.working_dir.trim();
        let path = self.path.trim().to_string();
        // 用户在对话框里把命令行改成 `www.…`/`…://…`/协议形态时，类型自动跟到 Url（预填推断可能判成 Command）。
        let kind = if self.kind == ItemKind::Command && is_url(&path) {
            ItemKind::Url
        } else {
            self.kind
        };
        // 别名留空时按名称自动生成拼音首字母（FR-SRC-22）；用户填了则尊重用户输入。
        let kw = {
            let k = self.keyword.trim();
            if k.is_empty() {
                default_keyword(&name)
            } else {
                k.to_string()
            }
        };
        LaunchItem {
            name,
            path,
            source_path: if self.source_path.trim().is_empty() {
                self.path.trim().to_string()
            } else {
                self.source_path.trim().to_string()
            },
            arguments: self.arguments.clone(),
            working_dir: if wd.is_empty() {
                None
            } else {
                Some(wd.to_string())
            },
            kind,
            keyword: kw,
            tags: source::application::parse_tags_text(&self.tags_text),
            // 新建默认未置顶、零统计；编辑既有项的置顶/使用统计在 DlgResult::Confirm 提交时旧值携带回填。
            ..Default::default()
        }
    }
}

/// 启动时由命令行决定的初始状态（FR-WIN-08）：窗口可见性、预填弹窗、设置页、批量 SendTo。
#[derive(Default)]
struct StartupIntent {
    visible: bool,
    dialog: Option<DialogState>,
    settings_page: Option<u32>,
    batch: Vec<String>,
}

// E-05 输入法状态快照（显示时保存，隐藏时恢复）
#[derive(Debug, Clone, Copy)]
struct ImeState {
    open: bool,
    conv: usize,
}

/// 删除确认弹窗的数据（2026-10-18 用户要求：选中「控制面板」被误删过一次）。
/// 只带展示字段与身份 uid；真正的删除动作推迟到用户按「确定」那一帧。
#[derive(Debug, Clone)]
struct ConfirmRequest {
    uid: String,
    name: String,
    path: String,
}

/// 批次 A2（FR-SRC-17）：危险内置动作的执行前确认。与删除确认同一形态——
/// 只带"要做什么"和身份，真正的动作推迟到用户按「执行」那一帧。
#[derive(Debug, Clone)]
struct InternalConfirm {
    action: source::system::InternalAction,
    name: String,
    uid: String,
}

/// 左键拖空白移动/缩放窗口的锚点。坐标一律用 Win32 权威值（物理像素）：egui 的 `outer_rect`
/// 要等 winit 把 WM_MOVE 回读进输入状态，按帧用它算位移会滞后，实测两次拖拽分别只走 82% 和反向。
#[derive(Clone, Copy)]
struct DragAnchor {
    hwnd: HWND,
    /// 起手那一刻的光标屏幕坐标。
    cursor: (i32, i32),
    /// 起手那一刻的窗口左上角。
    origin: (i32, i32),
    /// 起手那一刻的窗口宽高（缩放要用）。
    size: (i32, i32),
    /// 0=整窗移动；否则是 `EDGE_*` 位掩码的组合（哪几条边被拖着）。
    edge: u8,
}

const EDGE_MOVE: u8 = 0;
const EDGE_LEFT: u8 = 1;
const EDGE_RIGHT: u8 = 2;
const EDGE_TOP: u8 = 4;
const EDGE_BOTTOM: u8 = 8;
/// 缩放下限（逻辑像素，按帧换算成物理像素再夹）：再小结果列表和顶栏就挤得没法用了。默认窗口 500×400。
const MIN_W: i32 = 360;
const MIN_H: i32 = 240;

fn win_cursor() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p) }.ok().map(|_| (p.x, p.y))
}

fn win_origin(hwnd: HWND) -> Option<(i32, i32)> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r) }
        .ok()
        .map(|_| (r.left, r.top))
}

/// 批次 F8：这个屏幕坐标还在不在某台显示器上。`MONITOR_DEFAULTTONULL` 让不在时返回空句柄，
/// 而不是就近吸附到主屏——吸过去等于"记住了却没记住"。
fn pos_on_screen(pos: [i32; 2]) -> bool {
    !unsafe {
        MonitorFromPoint(
            POINT {
                x: pos[0],
                y: pos[1],
            },
            MONITOR_DEFAULTTONULL,
        )
    }
    .is_invalid()
}

fn win_size(hwnd: HWND) -> Option<(i32, i32)> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r) }
        .ok()
        .map(|_| (r.right - r.left, r.bottom - r.top))
}

struct LauncherApp {
    query: String,
    items: Vec<LaunchItem>,      // 当前显示项 = 内置种子 + 用户清单（去重后）
    index: matching::MatchIndex, // FR-QRY-06 匹配键缓存（名称/别名的 lower/words/initials）
    apps_tx: mpsc::Sender<Vec<LaunchItem>>, // 「批量导入」触发端（可 clone）
    apps_rx: mpsc::Receiver<Vec<LaunchItem>>, // 「批量导入」后台扫描结果通道
    clear_query_next_frame: bool,
    selected: usize,                           // FR-QRY/基线：当前键盘选中的结果行
    scroll_followed: usize, // FR-ACT-10：列表上次已跟随到位的选中行（与 selected 不等即本帧要滚动）
    focus_query: bool,      // 呼出后请求把键盘焦点给搜索框
    dialog: Option<DialogState>, // FR-SRC-20 添加/编辑对话框（Some=打开）
    confirm: Option<ConfirmRequest>, // 删除确认弹窗（Some=打开；2026-10-18 用户要求，删除不再一键生效）
    internal_confirm: Option<InternalConfirm>, // 批次 A2：关机/重启/注销/清空回收站的执行确认
    about_open: bool,                // 批次 A2：「关于本程序」弹窗（Some 的等价物，用 bool 更直白）
    help_entry: Option<source::cheatsheet::CheatHit>, // 批次 A8：速查表条目的帮助页（Some=打开）
    /// 批次 F7：设置页「高级」页的破坏性动作确认（Some=确认框开着）。
    /// 按键分支与 Esc 链都要先看它，否则开着确认框还能触发别的快捷键。
    pending_destructive: Option<Destructive>,
    /// 批次 F6：顶栏 ☰ 主菜单是否开着（2026-10-24 用户点名「换成一个三条杠的菜单，点击显示
    /// 新建按钮和其他设置按钮」）。用裸 bool 而不是 egui 的 memory：快捷键 Ctrl+M 也要能开它，
    /// `egui::Popup::open_bool` 正好吃这一个所有权，关闭（点击/Esc）由 Popup 写回 `false`。
    menu_open: bool,
    win_drag: Option<DragAnchor>, // 左键拖空白移动窗口的进行中锚点（None=没在拖）
    dialog_enter_armed: bool,     // 弹窗当帧吞掉 Enter，防"打开框的那次回车"当帧又触发对话框的确定
    settings_open: bool,          // 设置页（整页模式，替代搜索页；2026-10-17 用户要求）
    settings_tab: SettingsTab,    // 设置页当前分类
    prefs: source::application::AppSettings, // 用户偏好（settings.json，目前只有 space_to_run）
    // ---- 批次 F1：FR-UIX-01 主题（`prefs.theme` 是入口名，这里是解析后的生效形态）----
    /// 本帧渲染读的那一套尺寸 / 颜色。换主题时由 [`LauncherApp::sync_theme`] 一次性替换。
    theme: theme::Theme,
    /// 上一次真正装进 egui 的解析结果；与本次相同就不再碰 `ctx`（避免每帧重设样式）。
    theme_applied: Option<theme::Theme>,
    /// 上一次装进 egui 的字体名。字体没变就只换样式，不重建字形缓存。
    theme_font: Option<String>,
    /// 设置页下拉的选项（`System` + 内置 + 用户主题）。只在启动 / 呼出 / 换主题时重读，
    /// 避免「外观」页每帧都去扫一遍 `Themes\` 目录。
    theme_choices: Vec<String>,
    icon_rx: mpsc::Receiver<(String, Vec<u8>, u32, u32)>,
    icon_tx: mpsc::Sender<(String, Vec<u8>, u32, u32)>, // 常驻，可 clone 给每批新项再起加载器
    icons: std::collections::HashMap<String, egui::TextureHandle>, // source_path → 纹理
    // ---- 批次 U：FR-SRC-06 路径四态（后台线程算、主线程按 uid 缓存）----
    path_tx: mpsc::Sender<Vec<(String, source::pathcheck::PathState)>>,
    path_rx: mpsc::Receiver<Vec<(String, source::pathcheck::PathState)>>,
    path_states: std::collections::HashMap<String, source::pathcheck::PathState>,
    /// 有一轮检查还没回来（UNC 断线可能挂很久）⇒ 不再派新一轮，避免线程堆积。
    path_in_flight: bool,
    path_scanned_at: Instant,
    // ---- 批次 A10：数据源注册表（插件插槽）----
    /// 本帧列表由这几家拼出来，顺序即报文次序。文件源的索引/列目录状态在 `FileSource` 自己
    /// 肚子里，`LauncherApp` 不再持有它那八个字段；开关读 [`AppSettings::sources_off`]。
    sources: Vec<Box<dyn source::DataSource>>,
    /// E1（批次 X）：配置文件坏过一次的话，这里存那句说明。静默启动时 `ui()` 根本不跑，
    /// 所以它是「等窗口第一次真被看见」才转成 Toast 的那句话，不是开机就闪一下没了。
    startup_notice: Option<String>,
    tray_rx: mpsc::Receiver<TrayEvent>,
    hotkey_rx: mpsc::Receiver<()>,
    // FR-SRC-21：主实例收二次进程转发（Activate/AddPath/Ping/Preferences/AddBatch）。
    ipc_rx: mpsc::Receiver<IpcMsg>,
    ipc_pending: Vec<DialogState>,
    pending_batch: Vec<String>, // FR-WIN-08 多路径 SendTo：首帧批量入库并去重
    hide_pending: bool,         // eframe 首帧后会强制显示窗口，静默启动需在首帧末盖回隐藏
    // 托盘「安装 SendTo 入口」后台线程的回执通道
    notify_tx: mpsc::Sender<Result<String, String>>,
    notify_rx: mpsc::Receiver<Result<String, String>>,
    visible: bool,
    /// 批次 F8（FR-UIX-07）：本次显示的开始时刻；淡入进度按它算（`None` = 没在淡）。
    shown_at: Option<Instant>,
    ime_before: Option<ImeState>, // E-05 呼出前的输入法状态
    self_hwnd: Option<HWND>,      // E-05 本机顶层窗口，切英文模式时对它发 WM_IME_CONTROL
    // FR-WIN-04 失焦关闭到托盘
    hide_at: Option<Instant>,   // 到点复查仍失焦则关闭到托盘
    had_focus_since_show: bool, // 呼出后是否真正拿到过焦点（守卫：防异步 Focus 未生效那一帧误关闭）
    // 底部居中悬浮 Toast（FR-ACT-02 启动失败 + 各操作回执；deadline 到点自动消失）
    toast: Option<(String, Instant, egui::Color32)>,
    // ---- FR-UIX-09：独立提示层（HUD，`src/hud.rs`）----
    hud: hud::Hud,
    // ---- FR-WIN-02 热键可配置（批次 Q）----
    /// 设置页正在录制新组合：为真时本帧按键由录制端独占，快捷键分支与 Esc 链整体跳过。
    hotkey_recording: bool,
    /// 系统里此刻真正装着的热键（显示口径）。初值取 `prefs.toggle_hotkey`，
    /// 第一帧再从 `HOTKEY_ACTIVE` 校正——启动退回过出厂值或注册全失败时，这里跟 prefs 不一样。
    hotkey_active: HotkeySpec,
    /// 已投递给热键线程、还没拿到回执的组合（录制按钮在此期间显示"应用中…"并禁用）。
    hotkey_awaiting: Option<HotkeySpec>,
    /// 启动以来的第一条热键回执（含启动退回）。只在首帧取一次，用来 Toast 告知"你设的键没生效"。
    hotkey_synced: bool,
    // ---- 批次 R：补字符热键（把被裸键吞掉的那个字符打回去）----
    /// 系统里此刻真正装着的补字符键（显示口径）。`None` = 没启用，或启用但没注册上。
    typeback_active: Option<HotkeySpec>,
    /// 卡片上的草稿：键框 + 四个复选框改的就是它。`mods` 只放实义修饰位（不带 NOREPEAT），
    /// 投递时才补上 NOREPEAT ⇒ 复选框的状态与草稿的数值是同一个数，不用再做一次换算。
    typeback_draft: Option<HotkeySpec>,
    /// 已投递、还没拿到回执的补字符组合（这段时间禁用"应用/清除"，与主热键同一口径）。
    typeback_awaiting: Option<HotkeySpec>,
    /// 补字符的键框正在等一次按键（与主热键的"录制中"互斥，见 `hotkey_frame`）。
    typeback_armed: bool,
    /// UI 侧的镜像位：「暂停请求发出去了没有」。真值在热键线程的 `HOTKEY_SUSPENDED` 里，
    /// 这里只用来保证边沿检测不重复发消息——兜底定时器可能已经把真值改回 false 了，
    /// 那种情况下多发一条 resume 是无害的（`resume_hotkey` 幂等）。
    keypick_suspended: bool,
}

fn spawn_icon_loader(tx: mpsc::Sender<(String, Vec<u8>, u32, u32)>, source_paths: Vec<String>) {
    thread::spawn(move || {
        let mut ok = 0usize;
        let total = source_paths.len();
        let mut first_logged = false;
        for source_path in source_paths {
            if let Some((rgba, w, h)) = source::icon::extract(&source_path) {
                if !first_logged {
                    let nonzero = rgba.iter().filter(|&&v| v != 0).count();
                    eprintln!(
                        "icon probe: 首个成功 {}x{}，非零字节 {}/{}  ← {}",
                        w,
                        h,
                        nonzero,
                        rgba.len(),
                        source_path.rsplit('\\').next().unwrap_or("?")
                    );
                    first_logged = true;
                }
                ok += 1;
                if tx.send((source_path, rgba, w, h)).is_err() {
                    break;
                }
            }
        }
        eprintln!("icon loader: {ok}/{total} 图标提取成功");
    });
}

/// FR-SRC-19 学习排序的时间戳源：UNIX 毫秒，落盘后可跨进程比较。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// Ctrl+D 打开文件所在目录的选路判据：path 展开 %VAR% 后是绝对存在的文件用 path，
// 否则回落 source_path（命令类种子的 %windir%\System32\... 在这）；两者都落空＝命令名/URL/虚拟文件夹。
fn resolve_file_for_select(path: &str, source_path: &str) -> Option<String> {
    let locatable = |p: &str| -> Option<String> {
        let t = p.trim();
        if t.is_empty() {
            return None;
        }
        let expanded = source::application::expand_vars(t);
        let pb = std::path::PathBuf::from(&expanded);
        (pb.is_absolute() && pb.is_file()).then_some(expanded)
    };
    locatable(path).or_else(|| locatable(source_path))
}

/// `CF_UNICODETEXT` 的格式号。常量本身在 `Win32::System::Ole`（`CLIPBOARD_FORMAT(13u16)`，
/// 已在 windows-0.62.2 源码里核对过），但 `SetClipboardData` 收的是 `u32` ⇒ 写死 13，
/// 不为一个数字多开一条 feature。
const CF_UNICODETEXT: u32 = 13;

/// FR-SRC-10：把计算器结果写进剪贴板（需求原文的 `text` 动作＝只复制数字，不启动进程）。
///
/// 剪贴板一旦打开就必须关掉——漏关会让别的程序从此再也打不开它，所以失败路径也走同一条
/// `CloseClipboard`。句柄交给系统之后归系统所有，只有失败路径才自己 `GlobalFree`。
fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0u16)).collect();
    unsafe {
        if OpenClipboard(None).is_err() {
            return Err("打开剪贴板失败（可能正被别的程序占着）".to_string());
        }
        let done = write_text_format(&wide);
        let _ = CloseClipboard();
        done
    }
}

/// 必须在剪贴板已打开的状态下调用。UTF-16 带末尾 NUL 是 `CF_UNICODETEXT` 规定的形态。
fn write_text_format(wide: &[u16]) -> Result<(), String> {
    let len = wide.len() * std::mem::size_of::<u16>();
    unsafe {
        EmptyClipboard().map_err(|e| format!("清空剪贴板失败：{e}"))?;
        let h = GlobalAlloc(GMEM_MOVEABLE, len).map_err(|e| format!("分配剪贴板内存失败：{e}"))?;
        let p = GlobalLock(h);
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return Err("锁定剪贴板内存失败".to_string());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), p.cast::<u8>(), len);
        // 带着锁的句柄交给系统 ⇒ SetClipboardData 直接失败，必须先解锁。
        let _ = GlobalUnlock(h);
        if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(h.0))).is_err() {
            let _ = GlobalFree(Some(h));
            return Err("写入剪贴板失败".to_string());
        }
        Ok(())
    }
}

// ---- FR-ACT-10 键盘导航的纯计算部分（不碰 egui 状态，可单测）----

/// 列表可视区之外被顶栏 + 内边距 + 外框吃掉的高度，用于把窗口高换算成「一页可见几行」。
///
/// 批次 F6 起它不再是一个写死的常量：上＝`list_top_h`（含主题内边距与 [`INPUT_MIN_H`]）、
/// 下＝`list_bottom_h`（主题内边距 + 外框），两套值都由主题给，翻页这条取的是**整窗**可用高
/// （`ui.available_height()` 在 CentralPanel 之前），所以必须由调用方现量现传，
/// 否则 Compact（内边距 10）会算出比窗口实际能装的少一行。
fn list_chrome_h(theme: &theme::Theme) -> f32 {
    list_top_h(theme) + list_bottom_h(theme)
}

/// 主界面一屏的行数（2026-10-24 用户需求：「默认显示 10 条」）。
/// 两处共用：窗口默认高度按它算；[`ListRows::Ten`] 档按它截断列表（头段行算在内）。
const LIST_ROWS: usize = 10;

/// 「10 条」档下普通结果还能占几个名额：头段行（计算器 / 搜索 / 提示）算在前 10 行里。
fn result_room(head_len: usize, rows: usize) -> usize {
    rows.saturating_sub(head_len)
}

/// 「10 条」档的截断。**只裁显示与导航用到的 `results`**，`self.items` 一个字不动——
/// 换回「全部」档立刻就能看到被裁掉的那些。
fn cap_plan_rows(plan: &mut Plan, rows: usize) {
    plan.results.truncate(result_room(plan.head.len(), rows));
}

/// 主窗口默认高度：正好装下 [`LIST_ROWS`] 行。
///
/// 三段（Light 主题、150% DPI，2026-10-24 真机截图量的）：
/// 上＝外框 1 + 内边距 + 输入框 + 4（行距与分隔线）；下＝内边距 + 外框 1；
/// 中间一行＝[`row_geometry`] 给的行高 + 行距。
/// 批次 F6 起，行高不再只由主题决定（用户嫌 54+4 在 500 宽里太松），输入框也不再是
/// "字体多高就多高"（20pt 只撑出 ~29px，用户嫌小），而是钳到 [`INPUT_MIN_H`]。
fn default_window_height(theme: &theme::Theme, density: RowDensity) -> f32 {
    let (row_h, _, spacing) = row_geometry(density, theme);
    (list_top_h(theme) + LIST_ROWS as f32 * (row_h + spacing) + list_bottom_h(theme))
        .clamp(MIN_H as f32, 900.0)
}

/// 状态栏行高（FR-UIX-08 原文「高约 38」）。不随行密度缩放：甲档一行 34+2 也只比它矮 2px。
const STATUS_BAR_H: f32 = 38.0;
/// 状态栏上面的分隔线与留白，和行高一起从列表可视区里扣。
const STATUS_BAR_GAP: f32 = 6.0;
const STATUS_BAR_BLOCK: f32 = STATUS_BAR_H + STATUS_BAR_GAP;

/// 实际窗口高度 = 一屏十行 +（状态栏开着时的状态栏那一块）。
/// 分成两函数而不是给 [`default_window_height`] 加参数：它那几条绝对值单测钉的是"十行本身"，
/// 状态栏是叠加项，混在一起会让那组数随开关漂移。
fn window_height(theme: &theme::Theme, density: RowDensity, status_bar: bool) -> f32 {
    default_window_height(theme, density)
        + if status_bar { STATUS_BAR_BLOCK } else { 0.0 }
}

/// 淡入时长（FR-UIX-07 原文「入 120ms」）。淡出这一半本批不做：原文要求影子窗口，
/// 而"先淡 100ms 再隐藏"会违反同一条里的「隐藏不阻塞下一次热键唤起」。
const FADE_IN_SECS: f32 = 0.120;

/// 淡入时按进度压颜色 Alpha。`CentralPanel` 的底色与描边由面板自己画，
/// `ui.set_opacity` 管不到它们 ⇒ 面板框架这层得单独乘一次。
fn with_alpha(c: egui::Color32, alpha: f32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        c.r(),
        c.g(),
        c.b(),
        (c.a() as f32 * alpha.clamp(0.0, 1.0)) as u8,
    )
}

/// 输入框的最小高度（2026-10-24 用户正文：「输入框大一点，像 ahk 版的输入框一样，大气一点」）。
/// AHK 版是 `InputHeight := Max(editH, Win.Scale(40))`（`SearchWindow.ahk:92-96`），取 44 再高一档。
const INPUT_MIN_H: f32 = 44.0;

/// ☰ 按钮的本体宽度（不含它与输入框之间的那个列间距）。批次 F6 换掉「＋ 新建」而来，
/// 与 [`INPUT_MIN_H`] 同高 ≈ 正方形：AHK 版给它的命中宽也是"内边距 + Scale(28)"这一档。
const MENU_BTN_W: f32 = 40.0;

/// 输入框与 ☰ 之间**额外预留**的间隙。批次 F6 真机抓出来的缺陷：输入框改成定宽铺满之后，
/// 这一行里只剩 `item_spacing.x`（8px）的缝，而顶栏拖动手柄两侧各要内缩，缝被吃到 0 宽 ⇒
/// 「拖顶栏空白移动窗口」整条路静默失效（真机日志里 `搜索页顶栏空白` 零命中）。
/// 2026-10-18 用户就报过一次"顶部输入框右侧点击空白不能拖动"，这条不能又死一次。
const TOP_DRAG_GAP: f32 = 12.0;

/// 手柄从缝的两侧各让出多少：保证按下不会误判成点输入框或点 ☰。
const TOP_DRAG_INSET: f32 = 2.0;

/// 顶栏拖动手柄的实际可命中宽度 = 预留间隙 + 行间距 − 两侧内缩。
/// 它**与窗口宽度无关**（输入框是"扣掉 ☰ 与这条缝之后铺满"），所以只要 [`TOP_DRAG_GAP`]
/// 归零就会立刻退化成本次修的那个形状。渲染路径用的是量到的真边界，这里只给单测当判据。
#[cfg(test)]
fn top_drag_strip_w(item_spacing_x: f32) -> f32 {
    TOP_DRAG_GAP + item_spacing_x - 2.0 * TOP_DRAG_INSET
}

/// 搜索框的占位提示。**批次 F6 起改短**：输入框放大到 22pt 之后，原先那串十来个键名的长提示
/// 会被输入框右缘裁掉（留半句话比没有更糟）。完整键表有两处入口——☰ 菜单本身（Ctrl+M）
/// 和查询框打 `?` 的速查表（批次 A8，2026-10-24 批次 F6 起 **37 条**）。
const SEARCH_HINT: &str = "搜索…（Enter 运行 · Ctrl+M 菜单 · ? 帮助 · Esc 关闭）";

/// 状态栏开着时的占位提示（FR-UIX-08「两者耦合」的定义）：键位说明已经落在状态栏左/右两格，
/// 输入框这边只留一句「搜索…」。状态栏关掉时回落到 [`SEARCH_HINT`] 那一整句。
const SEARCH_HINT_SHORT: &str = "搜索…";

/// 列表可视区之上的占用：外框 1 + 主题内边距 + 输入框 + 4（行距与分隔线，Light 实测值）。
fn list_top_h(theme: &theme::Theme) -> f32 {
    1.0 + theme.padding + INPUT_MIN_H + 4.0
}

/// 列表可视区之下的占用：主题内边距 + 外框 1。
fn list_bottom_h(theme: &theme::Theme) -> f32 {
    theme.padding + 1.0
}

/// 一行的实际几何：（行高、图标边长、行与行的间距）。
///
/// 三档数字的来由见 [`RowDensity`] 的文档（Delphi `ItemHeight=16` 是无图标的下限参照，
/// AHK 的 54 是"行距 0 + 宽 700"才成立的上限参照）。[`RowDensity::Theme`] 把两个尺寸交还给
/// 主题，行距仍用 [`ROW_STEP_EXTRA`]——主题里没有"行距"这个键（需求书 6.2 的 24 键没有）。
pub fn row_geometry(density: RowDensity, theme: &theme::Theme) -> (f32, f32, f32) {
    match density {
        RowDensity::Snug => (34.0, 24.0, 2.0),
        RowDensity::Roomy => (40.0, 28.0, 2.0),
        RowDensity::Dense => (24.0, 16.0, 0.0),
        RowDensity::Theme => (theme.row_height, theme.icon_size, ROW_STEP_EXTRA),
    }
}

/// 一行除主题行高之外的实际增量：`item_spacing.y` 3 + 取整 1。只在 [`RowDensity::Theme`] 下用到。
const ROW_STEP_EXTRA: f32 = 4.0;

/// 一页可见行数，至少 1（窗口拉到只装得下一行时翻页仍要能动）。
/// `row_height` 是 [`row_geometry`] 给的实际步进（行高 + 行距）；`chrome` 见 [`list_chrome_h`]。
fn page_rows(available_height: f32, row_height: f32, chrome: f32) -> usize {
    ((available_height - chrome) / row_height.max(1.0))
        .floor()
        .max(1.0) as usize
}

/// 翻页落点：向下夹到最后一行，向上夹到第一行。
fn page_target(selected: usize, rows: usize, len: usize, down: bool) -> usize {
    if down {
        (selected + rows).min(len.saturating_sub(1))
    } else {
        selected.saturating_sub(rows)
    }
}

/// FR-ACT-01 数字键行号：`1..9` 指第 1..9 行，`0` 指第 10 行；越界返回 None。
fn digit_index(digit: u8, len: usize) -> Option<usize> {
    let row = if digit == 0 { 9 } else { digit as usize - 1 };
    (row < len).then_some(row)
}

/// 「提交即学别名」的取词：只取查询**首词**（FR-QRY-01 的 keyword 那半，后面的词是参数不是触发码）。
/// 数字键执行时行号数字贴在词尾（敲 `jsq5` 执行第 5 行），它不是触发词，剥掉——
/// Delphi 用 `Copy(Text, 1, Length-1)` 剥尾字符是同一个道理。
fn commit_word(query: &str, via_digit: bool) -> String {
    let w = matching::parse(query).keyword;
    if via_digit {
        w.trim_end_matches(|c: char| c.is_ascii_digit()).to_string()
    } else {
        w
    }
}

/// 行号徽标（2026-10-18 用户要求：列表里看不到 1-9/0，数字键没有可见对应物）。
/// 第 1..9 行显示 `1..9`、第 10 行显示 `0`，与 `digit_index` 同一套映射；
/// 第 11 行起不显示——那里没有按键，标了号反而误导。
fn row_num_label(idx: usize) -> Option<&'static str> {
    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]
        .get(idx)
        .copied()
}

/// 行号徽标 + 悬停说明，计算器行与条目行共用这一个画法（两行的左缘才可能对得齐）。
/// `verb` 只有两种：条目行「执行」、计算器行「复制」——第 0 行按 1 复制数字，不是跑程序。
/// 高度跟随主题行高（`interact_size.y`，列表画之前由 `theme.row_height` 赋值）。
fn row_badge(ui: &mut egui::Ui, idx: usize, verb: &str) {
    match row_num_label(idx) {
        Some(num) => {
            let h = ui.spacing().interact_size.y.max(14.0);
            ui.add_sized(
                [14.0, h],
                egui::Label::new(egui::RichText::new(num).monospace().weak()),
            )
            .on_hover_text(format!("按 {num} 直接{verb}本行"));
        }
        None => ui.add_space(14.0),
    }
}

/// FR-SRC-06：路径四态在行尾的颜色。Offline 是"介质不在"（提示性），Not found 才像"写错了"。
fn path_state_color(state: source::pathcheck::PathState) -> egui::Color32 {
    match state {
        source::pathcheck::PathState::Offline => egui::Color32::from_rgb(235, 175, 95),
        source::pathcheck::PathState::NotFound => TOAST_ERR,
        _ => egui::Color32::GRAY,
    }
}

// ---- FR-SRC-10 / FR-SRC-11 结果行并入列表的行模型（批次 S / T）----
//
// 行模型（`HeadRow`）与列表计划（`Plan`）批次 A10 起定义在 `src/source/mod.rs`，
// 分别叫 `SourceRow` / `SearchPlan`；本文件顶部用别名保留旧名。
// 头段次序照需求书 4.1.2 的分值偏移：计算器 200、`{query}` 网页搜索 150、
// 应用最高 `score+10`=110 ⇒ 两者都压在应用行之前。实际次序由 `source::default_sources()`
// 的注册顺序决定。

/// `HeadRow::Hint` 那一句"这行不能执行"的文案。批次 X 起这一格住了两种住户：
/// 引擎的空词补全提示，和「文件索引建立中」。按引擎 id 分流，而不是把提示行的副标题
/// 原样当 Toast 用——`在 'g ' 后输入，然后回车` 那种话带着触发符，复读一遍只会让人以为
/// 自己打错了。
fn hint_message(hint: &source::websearch::EngineHint) -> &'static str {
    if hint.engine_id == "filesearch" {
        "文件索引还在后台建立，稍等片刻再回车"
    } else {
        "继续输入关键词后再回车"
    }
}

/// 状态栏左侧那一格（FR-UIX-08「左 = 面板返回 / 文件模式 / 轮换提示」）。
///
/// 三档按「此刻最有用」排序，不是并列展示：目录浏览时用户下一步多半是返回，先把返回键说出来；
/// 文件模式其次；平时才轮到 Tab 轮换。「基本」分类范围只在这一格里追加——它是清单层的筛子，
/// 目录浏览/文件模式两档下说它会让人误以为是文件范围。
///
/// 目录那一档的 Esc 文案按 [`browse_parent_of`] 的真实落点说：带过滤词时先去词（不是退目录），
/// 盘根再按下去是清空查询——三处说成同一句话会教坏用户。
fn status_left_text(
    browse: Option<(&str, &str)>,
    file_mode: bool,
    tab: TabMode,
    basic_scope: bool,
) -> String {
    if let Some((dir, filter)) = browse {
        let esc = if !filter.is_empty() {
            "Esc 清除筛选"
        } else if source::filesearch::parent_query(dir).is_some() {
            "Esc 返回上级"
        } else {
            "Esc 清空"
        };
        return format!("目录 {dir} · {esc}");
    }
    if file_mode {
        return "文件模式 · Esc 清空".to_string();
    }
    let mut text = match tab {
        TabMode::Complete => "Tab 补全 / 下一项",
        TabMode::Navigate => "Tab 下一项 · Shift+Tab 上一项",
    }
    .to_string();
    if basic_scope {
        text.push_str(" · 分类：基本");
    }
    text
}

/// 状态栏右侧 `Enter` 键帽后面的动作词（FR-UIX-08「右 = Enter 行为标签」）。
///
/// 与行首徽标同口径，但这里要能直接接在键帽后面读（「Enter 打开」），所以用动词短语。
/// 四类不可执行行跟着徽标说「查看」——它们按下去只给一句话，与徽标说同一件事比换词更不容易误会。
fn enter_action_label(row: Option<&HeadRow>) -> &'static str {
    match row {
        None => "运行",
        Some(HeadRow::Calc(_)) => "复制数字",
        Some(HeadRow::Search(_)) => "搜索",
        Some(HeadRow::Hint(_))
        | Some(HeadRow::TermHint(_))
        | Some(HeadRow::CheatHint(_))
        | Some(HeadRow::ScopeTags(_)) => "查看",
        Some(HeadRow::Custom(_)) => "运行",
        Some(HeadRow::CustomHint(_)) => "补全参数",
        Some(HeadRow::File(_)) => "打开",
        Some(HeadRow::Terminal(_)) => "执行命令",
        Some(HeadRow::Cheat(_)) => "查看",
    }
}

/// 目录浏览时 Esc 的落点：先把当前的过滤词去掉（留在本目录），再逐级上溯。
/// 盘根 / UNC 共享根 / 不是浏览态一律 `None`——调用方保持"清空查询"的旧行为。
fn browse_parent_of(raw: &str) -> Option<String> {
    match source::filesearch::route(raw) {
        source::filesearch::Route::Browse { dir, filter } if !filter.is_empty() => Some(dir),
        source::filesearch::Route::Browse { dir, .. } => source::filesearch::parent_query(&dir),
        _ => None,
    }
}

/// 合并行号 → 行。`Head` 里放的是 `Plan::head` 的下标，`Item` 里放的仍是
/// [`results`](LauncherApp::results) 的名次，不是 `self.items` 的下标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Head(usize),
    Item(usize),
}

/// 合并后的总行数。导航键（↑↓ / PgUp/PgDn / Ctrl+N/P / Ctrl+J/K / Tab 的循环那一半）全在这个
/// 口径里回绕，不必换算名次。
fn row_count(results_len: usize, head_len: usize) -> usize {
    results_len + head_len
}

/// 合并行号 → 行。越界返回 `None`，调用方按「什么都不做」处理（与 `space_executes` 那侧同口径）。
fn row_at(selected: usize, results_len: usize, head_len: usize) -> Option<Row> {
    if selected < head_len {
        return Some(Row::Head(selected));
    }
    let rank = selected - head_len;
    (rank < results_len).then_some(Row::Item(rank))
}

/// 头段占掉前 `head_len` 行之后，把合并行号换算成 `space_executes` 要的「名次」。
/// 选中头段（计算器/搜索/提示）时返回 `None`——那些行不是条目，空格不替它们执行任何程序。
fn item_rank(selected: usize, head_len: usize) -> Option<usize> {
    (selected >= head_len).then(|| selected - head_len)
}

/// 取回被按下的数字键（小键盘也算：winit 把 `Numpad*` 映射到同一批 `Num*`）。
/// 用 `consume_key` 精确匹配修饰键并吃掉事件；裸数字那一带由调用方在输入框为空时才请求。
fn pressed_digit(ui: &egui::Ui, mods: egui::Modifiers) -> Option<u8> {
    const KEYS: [(egui::Key, u8); 10] = [
        (egui::Key::Num1, 1),
        (egui::Key::Num2, 2),
        (egui::Key::Num3, 3),
        (egui::Key::Num4, 4),
        (egui::Key::Num5, 5),
        (egui::Key::Num6, 6),
        (egui::Key::Num7, 7),
        (egui::Key::Num8, 8),
        (egui::Key::Num9, 9),
        (egui::Key::Num0, 0),
    ];
    KEYS.iter().find_map(|(key, digit)| {
        ui.input_mut(|i| i.consume_key(mods, *key))
            .then_some(*digit)
    })
}

/// 裸数字（不带修饰）在空输入时能否快捷执行第 N 行。
/// 2026-10-23 用户裁定：计算器源开着时让位给表达式——`1+2` 的第一个 `1`
/// 不该先启动第 1 行；`Ctrl+数字` / `Alt+数字` 不受这条影响。
fn bare_digit_allowed(query: &str, calc_on: bool) -> bool {
    query.trim().is_empty() && !calc_on
}

// ---- E-05 IME 操作（与 AHK 参考项目 Win.ImeState / Win.SetImeState 同构）----

fn ime_control(ime_wnd: HWND, cmd: usize, value: usize) -> usize {
    let mut result: usize = 0;
    unsafe {
        let _ = SendMessageTimeoutW(
            ime_wnd,
            WM_IME_CONTROL,
            WPARAM(cmd),
            LPARAM(value as isize),
            SMTO_ABORTIFHUNG,
            300,
            Some(&mut result as *mut usize as _),
        );
    }
    result
}

fn get_ime_state(ime_wnd: HWND) -> ImeState {
    let open = ime_control(ime_wnd, IMC_GETOPENSTATUS, 0) != 0;
    let conv = ime_control(ime_wnd, IMC_GETCONVERSIONMODE, 0);
    ImeState { open, conv }
}

fn set_ime_state(ime_wnd: HWND, open: bool, conv: usize) {
    // 与 AHK 参考项目 ImeSteps 同序：打开时先 open 再 conv，关闭时先 conv 再 open
    if open {
        ime_control(ime_wnd, IMC_SETOPENSTATUS, 1);
        ime_control(ime_wnd, IMC_SETCONVERSIONMODE, conv);
    } else {
        ime_control(ime_wnd, IMC_SETCONVERSIONMODE, conv);
        ime_control(ime_wnd, IMC_SETOPENSTATUS, 0);
    }
}

/// 获取当前前台窗口的 IME 窗口句柄
fn foreground_ime_wnd() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let ime_wnd = unsafe { ImmGetDefaultIMEWnd(hwnd) };
    if ime_wnd.0.is_null() {
        None
    } else {
        Some(ime_wnd)
    }
}

/// E-05 第 2 档：切当前 IME 到英文模式（去掉 IME_CMODE_NATIVE 位 + 关闭 IME）
fn switch_ime_to_english(ime_wnd: HWND, before: ImeState) {
    let target_conv = before.conv & !IME_CMODE_NATIVE;
    set_ime_state(ime_wnd, false, target_conv);
}

/// E-05 恢复输入法到呼出前的状态
fn restore_ime(ime_wnd: HWND, state: ImeState) {
    set_ime_state(ime_wnd, state.open, state.conv);
}

// ---- FR-ACT-02 启动实现 ----

fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 统一用 `ShellExecuteExW` 启动：`.lnk` 目标、命令名（PATH / App Paths 解析）、URL、
/// `shell:` 文件夹、协议（`ms-settings:`）都能覆盖 —— `std::process::Command` 覆盖不了
/// 协议/URL/Store，故 FR-ACT-02 要求一律走这里。SEE_MASK_FLAG_NO_UI 抑制系统错误框，失败以 Result 返回。
/// 批次 A9：Store/MSIX 条目（路径形如 `shell:AppsFolder\<AUMID>`）是唯一的例外——
/// 它改走 `IApplicationActivationManager`，理由见 `application::activate_uwp`。
fn shell_execute(file: &str, params: &str, dir: Option<&str>) -> windows::core::Result<()> {
    // 清单里的路径可能是 `%VAR%` 形态（出厂「启动器目录」用的是 `%LAUNCHER_DIR%`，
    // 要跟着 exe 搬家）——ShellExecuteExW 不替我们展开，所以在这里统一走一次。
    // `expand_env` 对未定义的 `%…%` 原样保留，用户写错的变量不会被悄悄吃掉。
    let file = source::custom::expand_env(file);
    if let Some(aumid) = source::application::uwp_aumid(&file) {
        return source::application::activate_uwp(aumid);
    }
    let params = source::custom::expand_env(params);
    let dir = dir.map(source::custom::expand_env);
    let wfile = wide_z(&file);
    let wparams = wide_z(&params);
    let wdir = dir.as_ref().map(|d| wide_z(d));
    let mut sei: SHELLEXECUTEINFOW = unsafe { core::mem::zeroed() };
    sei.cbSize = core::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_FLAG_NO_UI;
    sei.lpVerb = PCWSTR::null(); // 默认动词 open
    sei.lpFile = PCWSTR(wfile.as_ptr());
    sei.lpParameters = PCWSTR(wparams.as_ptr());
    sei.lpDirectory = PCWSTR(wdir.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()));
    sei.nShow = SW_SHOWNORMAL.0;
    unsafe { ShellExecuteExW(&mut sei) }
}

/// 批次 P（FR-SYS-03）「打开日志」的落点：用系统关联程序打开 `launcher.log`。
/// 走 FR-ACT-02 那套 `ShellExecuteExW`（与启动清单项同一口径），不另起 `explorer.exe`。
/// 从终端启动的会话根本不写这个文件（stderr 有人听 ⇒ 不接管，见 `redirect_stderr_if_orphan`），
/// 所以文件不存在时先留一条说明再打开——按钮点了没反应比点了有内容更糟。
fn open_log_file() {
    let path = source::application::log_path();
    if !path.exists() {
        // 首行必须是日期锚，否则这份文件将来归档时读不出自己属于哪天。
        let note = format!(
            "{}{} ｜ 本文件由「打开日志」补写：此前它不存在，说明这台机器还没发生过没有控制台的\
             启动（从终端或脚本启动时日志直接刷在控制台、不写文件）。\n",
            source::application::LOG_DATE_TAG,
            source::application::local_now_stamp()
        );
        let _ = std::fs::write(&path, note);
    }
    let file = path.display().to_string();
    match shell_execute(&file, "", None) {
        Ok(_) => eprintln!("打开日志：{file}"),
        Err(e) => eprintln!("打开日志失败：{}：{e}", path.display()),
    }
}

// ---- 批次 A2（FR-SRC-17）内置动作的两条系统调用 ----

/// 起一条控制台工具（`shutdown.exe`）。本程序是 GUI 子系统，不起控制台；
/// `CREATE_NO_WINDOW` 再挡掉子进程自己新建控制台窗——否则关机前会先闪一个黑框。
fn run_console_tool(exe: &str, args: &[&str]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

/// 音量/媒体键：按下 + 抬起各一次。与补字符那格同一个 `SendInput`，区别是这里发真 VK。
fn press_vk(vk: u16) {
    let key = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe {
        SendInput(
            &[key(false), key(true)],
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent != 2 {
        eprintln!("internal: vk 0x{vk:02X} 只注入 {sent}/2（目标程序可能忽略了它）");
    }
}

/// 危险动作确认框里那句"到底会发生什么"。只说事实，不吓人也不轻描淡写。
fn confirm_warning(action: source::system::InternalAction) -> &'static str {
    use source::system::InternalAction as A;
    match action {
        A::Shutdown => "将立即关闭这台电脑，未保存的工作会丢失。",
        A::Restart => "将立即重启这台电脑，未保存的工作会丢失。",
        A::Logoff => "将注销当前用户，所有未保存的工作会丢失。",
        A::EmptyRecycleBin => "将清空回收站，里面的文件无法再恢复（本程序不再弹系统二次确认）。",
        _ => "",
    }
}

// ---- FR-WIN-05 全局热键 ----

static HOTKEY_TX: std::sync::OnceLock<mpsc::Sender<()>> = std::sync::OnceLock::new();
static CTX: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

/// 批次 Q：换键请求走"发消息 + 共享槽"，不是在 UI 线程直接调 `RegisterHotKey`。
/// 原因有两条：① 注册必须绑在同一个 hwnd 上（注销时 Windows 按 hwnd 找），而那个消息窗只活在
/// 热键线程的闭包里；② 热键线程阻塞在 `GetMessageW`，只往 `Mutex` 里写值它永远醒不过来，
/// 所以槽必须由一条消息去踢它。UI 线程取不到回执也无所谓——`HOTKEY_RESULT` 会一直留着，
/// 下一帧读走即可。
const WM_APP_RELOAD_HOTKEY: u32 = WM_APP + 1;
/// 批次 R：设置页正在等一次按键（录制主热键 / 给补字符键选键）⇒ 临时注销主热键。
/// 不注销的话，用户按下的那个键会被系统整条吞掉，窗口一个事件都收不到 ⇒ 裸 `` ` `` 根本录不进来。
const WM_APP_SUSPEND_HOTKEY: u32 = WM_APP + 2;
/// 退出等键（成功 / 取消 / Esc / 关窗 / 隐藏）⇒ 把主热键装回去。
const WM_APP_RESUME_HOTKEY: u32 = WM_APP + 3;
/// 「清除补字符热键」。注销也得在热键线程做（`UnregisterHotKey` 按 hwnd 找登记，hwnd 归属
/// 那条老规矩对注销同样成立），所以 UI 侧同样只能投消息。
const WM_APP_CLEAR_TYPEBACK: u32 = WM_APP + 4;
/// 暂停的兜底定时器 id 与时长：注销状态一旦漏掉恢复（比如窗口隐藏后 eframe 不再跑 `ui()`，
/// UI 侧那条边沿检测就永远不执行），全局呼出会一直不可用。定时器挂在消息窗上，
/// 由热键线程自己的消息循环驱动 ⇒ 与窗口可见与否无关，这条兜底才是真兜底。
const HOTKEY_SUSPEND_TIMER: usize = 0x5EED;
const HOTKEY_SUSPEND_TIMEOUT: u32 = 30_000;

/// 系统里最多同时装着两个热键，用 id 区分。槽位这个概念只存在于本文件：
/// 落盘的是两份 `HotkeySpec`（`toggle_hotkey` / `typeback_hotkey`），没有"槽"这个字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeySlot {
    Toggle,
    Typeback,
}

impl HotkeySlot {
    fn id(self) -> i32 {
        match self {
            Self::Toggle => HOTKEY_TOGGLE,
            Self::Typeback => HOTKEY_TYPEBACK,
        }
    }

    /// 日志与 Toast 里怎么称呼这个槽。
    fn label(self) -> &'static str {
        match self {
            Self::Toggle => "全局呼出热键",
            Self::Typeback => "补字符热键",
        }
    }

    /// 系统里此刻真正装着的那一组存在哪个槽里（`None` = 这个键当前没装）。
    fn active(self) -> &'static Mutex<Option<HotkeySpec>> {
        match self {
            Self::Toggle => &HOTKEY_ACTIVE,
            Self::Typeback => &TYPEBACK_ACTIVE,
        }
    }
}

/// 消息窗句柄。`HWND` 是裸指针、不 `Sync`，这里存它的数值（进程内句柄值恒定，只在消息循环前写一次）。
static HOTKEY_HWND: AtomicUsize = AtomicUsize::new(0);
/// UI → 热键线程：待注册的新组合 + 它属于哪个槽（`take` 之后立即清空，避免重复处理）。
static HOTKEY_REQUEST: Mutex<Option<(HotkeySlot, HotkeySpec)>> = Mutex::new(None);
/// 热键线程 → UI：系统里此刻真正装着的是哪一组（`None` = 一次都没注册上，全局呼出不可用）。
/// 设置页显示它、回执也按它回滚，而不是显示"用户想要的那一份"。
static HOTKEY_ACTIVE: Mutex<Option<HotkeySpec>> = Mutex::new(None);
/// 补字符热键的同一份口径（`None` = 没启用，或启用但注册失败）。
static TYPEBACK_ACTIVE: Mutex<Option<HotkeySpec>> = Mutex::new(None);
/// 回执队列而不是单格：批次 R 之后一次启动就可能同时产出两条（主热键退回 + 补字符注册失败），
/// 单格会互相覆盖，UI 就看不见其中一条。
static HOTKEY_RESULT: Mutex<Vec<HotkeyOutcome>> = Mutex::new(Vec::new());
/// 主热键当前是否处于"录制期临时注销"状态。只有热键线程写它（在 wndproc 里），
/// UI 侧另有一个"我发过消息没有"的镜像位，两者不互相依赖。
static HOTKEY_SUSPENDED: AtomicBool = AtomicBool::new(false);

/// 一次换键的结果（回执给 UI，由 UI 决定要不要写 settings.json）。
#[derive(Debug, Clone, Copy)]
enum HotkeyOutcome {
    /// 新组合注册成功，`on` 即系统里现在装着的。
    Applied { slot: HotkeySlot, on: HotkeySpec },
    /// 新组合注册失败（`err` = `GetLastError`）。`now_active` = 尝试之后系统里真正装着的：
    /// `Some(旧值)` 表示回滚成功，`None` 表示连旧组合也没能装回去 ⇒ 全局呼出不可用，只能走托盘。
    Rejected {
        slot: HotkeySlot,
        tried: HotkeySpec,
        err: u32,
        now_active: Option<HotkeySpec>,
    },
    /// 启动时读的 `tried` 被占用，退回出厂值 `fell_back_to`（这条不改 settings.json：
    /// 冲突多半是"那会儿别的程序正开着"，不该把用户的选择冲掉）。只有主热键有出厂值可退，
    /// 补字符键失败就是失败（退回一个用户没要的键更糟），所以这条不带槽位。
    StartupFallback {
        tried: HotkeySpec,
        fell_back_to: HotkeySpec,
        err: u32,
    },
}

fn hotkey_lock<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // 这三把锁只装 Copy 的小值、临界区内不回调任何代码，能中毒的唯一路径是别处 panic；
    // 那种情况下把旧值取出来继续用，比让热键逻辑跟着一起死更好。
    slot.lock().unwrap_or_else(|e| e.into_inner())
}

/// 本程序自己占用的组合（录制端落定前的预检表）。**表里的每一条都对着 `ui()` 里一次真实的
/// `consume_key`**（Ctrl+Shift+N 新建、Ctrl+E 编辑、Ctrl+I 批量导入、Ctrl+D 打开目录、
/// Ctrl+Shift+D 删除、Ctrl+Shift+P 置顶、Ctrl+N/P 与 Ctrl+J/K 循环导航、Ctrl+M 开 ☰ 主菜单、
/// Ctrl+数字 与 Alt+数字 执行第 N 行），
/// 不多写一条：表是"避让本程序的功能键"，不是"避开所有看着眼熟的键"，多出来的条目会白白
/// 挡掉系统里其实空着的组合（Ctrl+A 这种本程序压根没绑）。
///
/// 刻意不在表里的：`Ctrl+,`、`Ctrl+=`、`Ctrl+-`（`Comma/Equals/Minus` 不在可录集合，永远撞不上）、
/// 裸 `Delete`/`Space`/`Tab`/`Enter`/`PgUp`/`PgDn`/`Esc`、`Shift+Space`/`Shift+Tab`
/// （[`HotkeySpec::check`] 挡住"不带修饰"与"单带 Shift"这两类，本程序绑的裸键因此录不进来）。
/// 【2026-10-20 批次 R 更正：上一句里"挡住不带修饰"从此**不再对 `` ` `` 成立**——裸 `` ` `` 是
/// 用户点名放开的唯一例外。表里仍然不加条目，因为本程序压根没绑过 `` ` ``：
/// `grep -rn "Backtick\|VK_OEM_3\|0xC0" src/` 的 27 处命中全在映射、判据、展示名和测试里，
/// 没有一处是 `consume_key`。】
/// 【2026-10-24 批次 F7 补一条：`Ctrl+Del`（主界面隐藏出厂条目）同样不进表——`Delete`
/// 不在可录集合里，录制端永远产不出这个组合，也就撞不上；表只收改得出来的组合。】
/// 表里的修饰键一律不带 `HKF_NOREPEAT`（比对时两边都剥）。
const LOCAL_TAKEN: &[(u32, u32)] = &[
    (HKF_CONTROL, 0x44),             // Ctrl+D 打开所在目录
    (HKF_CONTROL, 0x45),             // Ctrl+E 编辑选中项
    (HKF_CONTROL, 0x47),             // Ctrl+G 缺省分类范围 all ⇄ basic（批次 G1）
    (HKF_CONTROL, 0x49),             // Ctrl+I 批量导入全部
    (HKF_CONTROL, 0x4A),             // Ctrl+J 循环下一项
    (HKF_CONTROL, 0x4B),             // Ctrl+K 循环上一项
    (HKF_CONTROL, 0x4D),             // Ctrl+M ☰ 主菜单（批次 F6：新建按钮换成菜单后给它配的键）
    (HKF_CONTROL, 0x4E),             // Ctrl+N 循环下一项（2026-10-22 从「新建」改为导航）
    (HKF_CONTROL, 0x50),             // Ctrl+P 循环上一项（2026-10-22 从「置顶」改为导航）
    (HKF_CONTROL | HKF_SHIFT, 0x44), // Ctrl+Shift+D 删除选中项
    (HKF_CONTROL | HKF_SHIFT, 0x4E), // Ctrl+Shift+N 新建快捷项
    (HKF_CONTROL | HKF_SHIFT, 0x50), // Ctrl+Shift+P 置顶 / 取消置顶
    // 数字行：Ctrl+1..9/0 与 Alt+1..9/0 执行第 N 行（FR-ACT-10）。
    (HKF_CONTROL, 0x30),
    (HKF_CONTROL, 0x31),
    (HKF_CONTROL, 0x32),
    (HKF_CONTROL, 0x33),
    (HKF_CONTROL, 0x34),
    (HKF_CONTROL, 0x35),
    (HKF_CONTROL, 0x36),
    (HKF_CONTROL, 0x37),
    (HKF_CONTROL, 0x38),
    (HKF_CONTROL, 0x39),
    (HKF_ALT, 0x30),
    (HKF_ALT, 0x31),
    (HKF_ALT, 0x32),
    (HKF_ALT, 0x33),
    (HKF_ALT, 0x34),
    (HKF_ALT, 0x35),
    (HKF_ALT, 0x36),
    (HKF_ALT, 0x37),
    (HKF_ALT, 0x38),
    (HKF_ALT, 0x39),
];

/// `egui::Key` → Virtual-Key。**逐个列举、不做算术**：egui 的 `Key` 枚举没有显式判别值
/// （`src/data/key.rs` 按声明顺序自动编号），依赖一升级编号就可能漂，而这里产出的数要直接
/// 进 `RegisterHotKey`。范围外返回 `None` ⇒ 录制端当"没按到有效键"继续等。
fn vk_from_egui_key(k: egui::Key) -> Option<u32> {
    use egui::Key::*;
    let code = match k {
        A => 0x41,
        B => 0x42,
        C => 0x43,
        D => 0x44,
        E => 0x45,
        F => 0x46,
        G => 0x47,
        H => 0x48,
        I => 0x49,
        J => 0x4A,
        K => 0x4B,
        L => 0x4C,
        M => 0x4D,
        N => 0x4E,
        O => 0x4F,
        P => 0x50,
        Q => 0x51,
        R => 0x52,
        S => 0x53,
        T => 0x54,
        U => 0x55,
        V => 0x56,
        W => 0x57,
        X => 0x58,
        Y => 0x59,
        Z => 0x5A,
        Num0 => 0x30,
        Num1 => 0x31,
        Num2 => 0x32,
        Num3 => 0x33,
        Num4 => 0x34,
        Num5 => 0x35,
        Num6 => 0x36,
        Num7 => 0x37,
        Num8 => 0x38,
        Num9 => 0x39,
        // 空格与回车写字面量而不写 VK_SPACE / VK_RETURN：单段标识符在模式位置会被当成新绑定，
        // 而这里的每个右值都要直接进 `RegisterHotKey`，不能靠一个可能被解析成绑定的名字。
        // 同一条警告还抓出本函数的变体名写错——egui 的枚举是 `Key::Enter`，没有 `Key::Return`
        // （写成 Return 时它变成一个"匹配任意值"的绑定，把 F1–F24 全部吃掉）。
        Space => 0x20,
        Enter => 0x0D,
        // 批次 R：`` ` `` 进可录集合（egui 那边这个变体叫 `Backtick`，winit 的 `Backquote` 映射到它）。
        // 和上面两条同一个理由写数字字面量，不写 `VK_OEM_3`：模式位上的单标识符是新绑定。
        Backtick => 0xC0,
        F1 => 0x70,
        F2 => 0x71,
        F3 => 0x72,
        F4 => 0x73,
        F5 => 0x74,
        F6 => 0x75,
        F7 => 0x76,
        F8 => 0x77,
        F9 => 0x78,
        F10 => 0x79,
        F11 => 0x7A,
        F12 => 0x7B,
        F13 => 0x7C,
        F14 => 0x7D,
        F15 => 0x7E,
        F16 => 0x7F,
        F17 => 0x80,
        F18 => 0x81,
        F19 => 0x82,
        F20 => 0x83,
        F21 => 0x84,
        F22 => 0x85,
        F23 => 0x86,
        F24 => VK_F24,
        _ => return None,
    };
    // 上界断言：本批只收这几段，越界一律视为映射写错。
    if (VK_A..=VK_Z).contains(&code)
        || (VK_0..=VK_9).contains(&code)
        || (VK_F1..=VK_F24).contains(&code)
        || code == VK_SPACE
        || code == VK_RETURN
        || code == VK_OEM_3
    {
        Some(code)
    } else {
        None
    }
}

/// 录制态吃掉本帧所有按键事件并返回第一个"按下"的原始键。
/// 必须清理事件而不是只读：egui 的 `key_pressed`／`consume_key` 都查 `events`
/// （`input_state/mod.rs:753`），留着下面的快捷键分支和 Esc 链就会同时响应同一次按键。
/// 【2026-10-19 实测过再来动这里：不需要"跳过修饰键自身的按下事件"。真机上和弦的 Ctrl 按下
/// 与主键按下不会落在同一帧（`Ctrl+I`、`Ctrl+F13` 都一次落定），而录制时"按了没反应"是因为
/// 那个组合被别的全局热键整条吞掉 ⇒ 窗口收不到任何键，只能继续等（卡片文案已写明这件事）。】
fn take_record_press(ui: &mut egui::Ui) -> Option<(egui::Key, egui::Modifiers)> {
    let mut hit = None;
    ui.input_mut(|i| {
        for ev in &i.events {
            if let egui::Event::Key {
                pressed: true,
                repeat: false,
                key,
                modifiers,
                ..
            } = *ev
            {
                if hit.is_none() {
                    hit = Some((key, modifiers));
                }
            }
        }
        i.events.retain(|ev| !matches!(ev, egui::Event::Key { .. }));
    });
    hit
}

/// 原始按键 → 组合。主键不在可录集合里（`vk_from_egui_key` 返回 None）时也返回 None，
/// 调用方按"还没按到有效键"继续等，而不是报一次错。
///
/// 修饰键只读 `ctrl / alt / shift` 三个字段，**故意不读 `command`**：Windows 上它是 `ctrl` 的
/// 副本，不是 Win 键。两条独立证据：`egui-winit-0.36.2/src/lib.rs:496-500` 写的是
/// `command = if cfg!(target_os = "macos") { super_ } else { ctrl }`，`egui-0.36.2`
/// 的字段文档（`data/input/modifiers.rs:33`）原话「On Windows and Linux, set this to the same
/// value as `ctrl`」。同一处 `:495` 又把 `mac_cmd` 写成 `cfg!(macos) && super_` ⇒ 非 Mac 平台
/// 恒 false，两个字段都给不出 Win。真实的 Win 键只以 `Key::SuperLeft / SuperRight` **键事件**
/// 出现，不在本函数的输入里 ⇒ 录制路径产不出 `HKF_WIN`（手改 `settings.json` 那一串数值仍可带
/// Win，[`HotkeySpec::check`] 与 `display()` 的 Win 分支就是为那条路留的）。
/// 【2026-10-19 更正：本函数首版按「`command` 在 Windows 就是 Win 键」写了 `if m.command`，
/// 用户实测报「录 `Ctrl+I` 得到 `Ctrl+Win+I`」⇒ 该假设作废，见 RELEASE_NOTES 第十五节。】
fn spec_from_press(key: egui::Key, m: egui::Modifiers) -> Option<HotkeySpec> {
    let vk = vk_from_egui_key(key)?;
    let mut mods = HKF_NOREPEAT;
    if m.ctrl {
        mods |= HKF_CONTROL;
    }
    if m.alt {
        mods |= HKF_ALT;
    }
    if m.shift {
        mods |= HKF_SHIFT;
    }
    Some(HotkeySpec { mods, vk })
}

/// 把换键请求交给热键线程；句柄还没建立（启动极早期）时返回 false，由调用方给出"稍后再试"。
/// 槽位随 `HOTKEY_REQUEST` 一起带过去，不另用 `wparam` 传：一份数据只留一个来源。
fn post_hotkey_request(slot: HotkeySlot, spec: HotkeySpec) -> bool {
    let raw = HOTKEY_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        eprintln!("hotkey: 消息窗还没建立，本次{}换键请求作废", slot.label());
        return false;
    }
    *hotkey_lock(&HOTKEY_REQUEST) = Some((slot, spec));
    match unsafe {
        PostMessageW(
            Some(HWND(raw as *mut _)),
            WM_APP_RELOAD_HOTKEY,
            WPARAM(0),
            LPARAM(0),
        )
    } {
        Ok(()) => true,
        Err(e) => {
            *hotkey_lock(&HOTKEY_REQUEST) = None;
            eprintln!("hotkey: {}换键请求投递失败：{e}", slot.label());
            false
        }
    }
}

/// 只要"踢热键线程一下"、不带数据的消息（暂停 / 恢复）走这里。
/// 投递失败不做重试：暂停没送到 = 那个键还吞着按键，用户按了没反应，卡片文案已经写明这件事；
/// 恢复没送到 = 兜底定时器会补上。两条都不值得为它做一套状态机。
fn post_hotkey_signal(msg: u32, what: &str) {
    let raw = HOTKEY_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        eprintln!("hotkey: 消息窗还没建立，本次{what}作废");
        return;
    }
    if let Err(e) = unsafe { PostMessageW(Some(HWND(raw as *mut _)), msg, WPARAM(0), LPARAM(0)) } {
        eprintln!("hotkey: {what}投递失败：{e}");
    }
}

/// 在指定消息窗上把 `slot` 这一格换成 `spec`：先注销再注册（同一个 id、同一个 hwnd）。
/// 返回 `Err(GetLastError)`；`ERROR_HOTKEY_ALREADY_REGISTERED`（1409）= 组合已被系统或别的程序占走。
/// 做成 safe fn（内部一个 `unsafe` 块）：调用点在 `hotkey_wndproc` 的 `unsafe {}` 里，
/// 再套一层 `unsafe` 就是多余警告，本项目的口径是零警告。
fn apply_hotkey(hwnd: HWND, slot: HotkeySlot, spec: HotkeySpec) -> Result<(), u32> {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), slot.id());
        match RegisterHotKey(Some(hwnd), slot.id(), HOT_KEY_MODIFIERS(spec.mods), spec.vk) {
            Ok(()) => Ok(()),
            Err(_) => Err(GetLastError().0),
        }
    }
}

/// 批次 R：设置页要等一次真实按键 ⇒ 先把主热键从系统里摘掉。
/// 不摘的话，用户按下的那个键会被系统整条吞掉（这正是全局热键的工作方式），本程序窗口
/// 一个 `Event::Key` 都收不到 ⇒ 裸 `` ` `` 永远录不进来。只动 `Toggle` 那一格：
/// 补字符键不是"要被录进框里的键"，摘它没有收益，反而多一个装不回来的东西。
fn suspend_hotkey(hwnd: HWND) {
    unsafe {
        if !HOTKEY_SUSPENDED.swap(true, Ordering::Relaxed) {
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_TOGGLE);
            match *hotkey_lock(&HOTKEY_ACTIVE) {
                Some(k) => eprintln!(
                    "hotkey: 取键期间临时注销 {}（退出取键或 {}ms 后装回）",
                    k.display(),
                    HOTKEY_SUSPEND_TIMEOUT
                ),
                None => eprintln!("hotkey: 取键期间没有装着的全局键可注销"),
            }
        }
        // 每次都重起表：连着两次取键（录完主热键又去点补字符的框）不该让第二次的窗口期变短。
        SetTimer(
            Some(hwnd),
            HOTKEY_SUSPEND_TIMER,
            HOTKEY_SUSPEND_TIMEOUT,
            None,
        );
    }
}

/// 退出取键 ⇒ 把 `HOTKEY_ACTIVE` 里那一份装回去。幂等：没在暂停中就直接返回。
/// 装不回去是这条设计里最坏的分支（全局呼出没了），所以它必须产出回执让 UI 说给人听，
/// 而不是只在日志里留一行——窗口可能正开着设置页，用户看得见 Toast。
fn resume_hotkey(hwnd: HWND, why: &str) {
    unsafe {
        let _ = KillTimer(Some(hwnd), HOTKEY_SUSPEND_TIMER);
        if !HOTKEY_SUSPENDED.swap(false, Ordering::Relaxed) {
            return;
        }
        let Some(k) = *hotkey_lock(&HOTKEY_ACTIVE) else {
            eprintln!("hotkey: 退出取键（{why}），本来就没有装着的全局键");
            return;
        };
        match apply_hotkey(hwnd, HotkeySlot::Toggle, k) {
            Ok(()) => eprintln!("hotkey: 退出取键（{why}）⇒ 已装回 {}", k.display()),
            Err(err) => {
                *hotkey_lock(&HOTKEY_ACTIVE) = None;
                hotkey_lock(&HOTKEY_RESULT).push(HotkeyOutcome::Rejected {
                    slot: HotkeySlot::Toggle,
                    tried: k,
                    err,
                    now_active: None,
                });
                eprintln!(
                    "hotkey: 退出取键（{why}）时装回 {} 失败（{err}）⇒ 全局呼出不可用，请用托盘",
                    k.display()
                );
                if let Some(ctx) = CTX.get() {
                    ctx.request_repaint();
                }
            }
        }
    }
}

/// vk → 这个键在**当前键盘布局**下不带修饰打出的字符。用 `MapVirtualKeyW` 而不是写死一张表：
/// 用户丢掉的字符由布局决定，跟着系统走才不会补错。两种"没有字符"的返回都要挡：
/// 0 = 这个键不产生字符；最高位（0x8000）= 死键（要连按两下才出字那种）。
fn typeback_char_for(vk: u32) -> Option<char> {
    let raw = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_CHAR) };
    if raw == 0 || raw & 0x8000 != 0 {
        return None;
    }
    char::from_u32(raw & 0xFFFF)
}

/// 补字符热键按下时干的事：把主热键那个字符投给此刻的焦点窗口。
///
/// 为什么走 `KEYEVENTF_UNICODE` 而不是"注销自己 → 发 VK → 再注册"：`RegisterHotKey` 匹配的是
/// VK + 修饰键，而 Unicode 注入不带 VK（`wVk = 0`、字符放 `wScan`）⇒ 它不会被自家那条裸 `` ` ``
/// 注册再吞一次，也不用给注销/注册留一个"这期间用户真按了 `` ` ``"的空窗。
/// 【2026-10-20：这条理由是本批的设计假设，**不是实测结论**。记事本 / Edge / VS Code /
/// Windows Terminal / 中文输入法五处的真实表现要靠真机复测清单验收，见发布说明那一节。】
fn inject_typeback_char() {
    let Some(toggle) = *hotkey_lock(&HOTKEY_ACTIVE) else {
        eprintln!("typeback: 主热键当前没装着，不知道该补哪个字符");
        return;
    };
    // 只有**裸键**注册才真的吞掉打字：带修饰的呼出键（Ctrl+Alt+Space）压根不影响 Space 键，
    // 这时候再按补字符键往焦点窗口塞一个空格就是凭空多出来的动作。所以这里挡住，
    // 让这一格的语义与卡片文案「打出被呼出键吞掉的那个字符」一致。
    if toggle.mods_named() != 0 {
        eprintln!(
            "typeback: 呼出键 {} 带修饰，那个字符本来就打得出 ⇒ 本次不补（补字符键只在呼出键是裸键时才有事做）",
            toggle.display()
        );
        return;
    }
    let vk = toggle.vk;
    let Some(ch) = typeback_char_for(vk) else {
        eprintln!("typeback: vk 0x{vk:02X} 在当前键盘布局下映射不到可输入字符，本次不补");
        return;
    };
    let key = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: ch as u16,
                dwFlags: if up {
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                } else {
                    KEYEVENTF_UNICODE
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe {
        SendInput(
            &[key(false), key(true)],
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    // 日志记 vk 码而不记字符本身：批次 P 那条"不记用户敲的原文"的口径延伸——补进去的字符
    // 由配置决定、本来不是用户输入，但记成 0xC0 一样够定位问题。前台窗口只记 hwnd + 类名，
    // 不记标题：标题里经常带着用户正在编辑的正文（隐私口径同上）。
    let fg = unsafe { GetForegroundWindow() };
    let fg_info = if fg.0.is_null() {
        "前台=无".to_string()
    } else {
        let mut buf = [0u16; 256];
        let n = unsafe { GetClassNameW(fg, &mut buf) };
        let cls = String::from_utf16_lossy(&buf[..n as usize]);
        format!("前台=0x{:x}({})", fg.0 as usize, cls)
    };
    eprintln!(
        "typeback: 已向焦点窗口补入 vk 0x{vk:02X} 对应的字符（SendInput {sent}/2，{fg_info}）"
    );
}

unsafe extern "system" fn hotkey_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if msg == WM_HOTKEY {
            // 批次 R 起系统里最多装着两个键，`wparam` 就是注册时那个 id ⇒ 必须按 id 分派，
            // 否则按下补字符键会把窗口弹出来（用户要的是打一个字符）。
            match wparam.0 as i32 {
                HOTKEY_TOGGLE => {
                    if let Some(tx) = HOTKEY_TX.get() {
                        let _ = tx.send(());
                    }
                    if let Some(ctx) = CTX.get() {
                        ctx.request_repaint();
                    }
                }
                HOTKEY_TYPEBACK => inject_typeback_char(),
                _ => {}
            }
            return LRESULT(0);
        }
        // 批次 R：兜底恢复。正常情况下退出取键由 UI 投 `WM_APP_RESUME_HOTKEY`，但"窗口一隐藏
        // eframe 就不再跑 `ui()`"这条（见需求书 E 附录）会让 UI 侧的边沿检测永远等不到下一帧，
        // 全局呼出就会一直停在注销状态。定时器挂在消息窗上、由本线程驱动 ⇒ 与可见性无关。
        if msg == WM_TIMER && wparam.0 as usize == HOTKEY_SUSPEND_TIMER {
            resume_hotkey(hwnd, "兜底超时");
            return LRESULT(0);
        }
        if msg == WM_APP_SUSPEND_HOTKEY {
            suspend_hotkey(hwnd);
            return LRESULT(0);
        }
        if msg == WM_APP_RESUME_HOTKEY {
            resume_hotkey(hwnd, "退出取键");
            return LRESULT(0);
        }
        if msg == WM_APP_CLEAR_TYPEBACK {
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_TYPEBACK);
            *hotkey_lock(&TYPEBACK_ACTIVE) = None;
            eprintln!("typeback: 已注销补字符热键（设置页「清除」）");
            return LRESULT(0);
        }
        // FR-WIN-02（批次 Q）：换键。注册/注销只在本线程发生（hwnd 归属），UI 线程只投消息。
        if msg == WM_APP_RELOAD_HOTKEY {
            let Some((slot, tried)) = hotkey_lock(&HOTKEY_REQUEST).take() else {
                return LRESULT(0);
            };
            // 主热键这一格的换键做完之后系统里必然装着点什么（新键，或回滚成功的旧键），
            // 所以"取键期间暂停"的状态到此作废；补字符格的换键不动这个状态，
            // 主热键还欠着一次恢复，那条恢复只能由 resume 或定时器来做。
            if slot == HotkeySlot::Toggle {
                let _ = KillTimer(Some(hwnd), HOTKEY_SUSPEND_TIMER);
                HOTKEY_SUSPENDED.store(false, Ordering::Relaxed);
            }
            let outcome = match apply_hotkey(hwnd, slot, tried) {
                Ok(()) => {
                    *hotkey_lock(slot.active()) = Some(tried);
                    eprintln!(
                        "hotkey: {} 换键成功 → {}（原组合已注销）",
                        slot.label(),
                        tried.display()
                    );
                    HotkeyOutcome::Applied { slot, on: tried }
                }
                Err(err) => {
                    let kept = *hotkey_lock(slot.active());
                    let restored = match kept {
                        // 没有"仍在系统里的旧组合"可回滚（启动即失败过）：保持无键，照实报。
                        None => false,
                        Some(k) => apply_hotkey(hwnd, slot, k).is_ok(),
                    };
                    let now_active = if restored { kept } else { None };
                    *hotkey_lock(slot.active()) = now_active;
                    match now_active {
                        Some(k) => eprintln!(
                            "hotkey: {} {} 注册失败（{err}={}），已回到 {}",
                            slot.label(),
                            tried.display(),
                            if err == ERROR_HOTKEY_ALREADY_REGISTERED {
                                "已被占用"
                            } else {
                                "其它原因"
                            },
                            k.display()
                        ),
                        None => eprintln!(
                            "hotkey: {} {} 注册失败（{err}）且原来的键也没装回去 ⇒ 该功能当前不可用",
                            slot.label(),
                            tried.display()
                        ),
                    }
                    HotkeyOutcome::Rejected {
                        slot,
                        tried,
                        err,
                        now_active,
                    }
                }
            };
            hotkey_lock(&HOTKEY_RESULT).push(outcome);
            if let Some(ctx) = CTX.get() {
                ctx.request_repaint();
            }
            return LRESULT(0);
        }
        // FR-SRC-21 / FR-WIN-07/08：二次进程转发的呼出、路径与设置页请求，转交主循环处理。
        if msg == WM_COPYDATA {
            if lparam.0 == 0 {
                return LRESULT(0);
            }
            let cds = &*(lparam.0 as *const CopyDataStruct);
            let ipc = match cds.dw_data {
                IPC_OP_ACTIVATE => Some(IpcMsg::Activate),
                IPC_OP_PING => Some(IpcMsg::Ping),
                IPC_OP_ADD_PATH => copy_data_payload(cds).map(IpcMsg::AddPath),
                IPC_OP_PREFERENCES => copy_data_payload(cds)
                    .and_then(|p| p.trim().parse::<u32>().ok())
                    .map(IpcMsg::Preferences),
                IPC_OP_ADD_BATCH => copy_data_payload(cds).map(|p| {
                    IpcMsg::AddBatch(
                        p.split('\n')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect(),
                    )
                }),
                _ => None,
            };
            if let (Some(m), Some(tx)) = (ipc, IPC_TX.get()) {
                let _ = tx.send(m);
                if let Some(ctx) = CTX.get() {
                    ctx.request_repaint();
                }
                return LRESULT(1);
            }
        }
        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

fn spawn_hotkey_thread(tx: mpsc::Sender<()>, configured: HotkeySpec, typeback: Option<HotkeySpec>) {
    HOTKEY_TX.set(tx).ok();
    thread::spawn(move || unsafe {
        let module = GetModuleHandleW(None).expect("GetModuleHandleW");
        let class_name: Vec<u16> = "EguiLauncherHotkeyClass\0".encode_utf16().collect();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(hotkey_wndproc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            eprintln!("hotkey: RegisterClassExW failed");
            return;
        }
        let win_name: Vec<u16> = "EguiLauncherHotkey\0".encode_utf16().collect();
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(win_name.as_ptr()),
            WINDOW_STYLE(0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(HWND_MESSAGE),
            None,
            Some(module.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("hotkey: CreateWindowExW failed: {e}");
                return;
            }
        };
        // 换键要靠这条消息找到本窗，所以句柄在注册之前就公布（UI 第一帧可能已经想改键）。
        HOTKEY_HWND.store(hwnd.0 as usize, Ordering::Relaxed);
        // 批次 Q：注册的是配置里的组合。失败不再 `return`——那个 `return` 会把消息窗一起丢掉，
        // 连带 SendTo / 单实例转发全废（老版本只在写死的热键下才无害）。现在退回出厂值，
        // 出厂值也失败就保留消息窗、只是没有全局键，托盘照常用。
        match apply_hotkey(hwnd, HotkeySlot::Toggle, configured) {
            Ok(()) => {
                *hotkey_lock(&HOTKEY_ACTIVE) = Some(configured);
                eprintln!("hotkey: {} registered", configured.display());
            }
            Err(err) => {
                let fallback = HotkeySpec::DEFAULT;
                match apply_hotkey(hwnd, HotkeySlot::Toggle, fallback) {
                    Ok(()) => {
                        *hotkey_lock(&HOTKEY_ACTIVE) = Some(fallback);
                        hotkey_lock(&HOTKEY_RESULT).push(HotkeyOutcome::StartupFallback {
                            tried: configured,
                            fell_back_to: fallback,
                            err,
                        });
                        eprintln!(
                            "hotkey: {} 注册失败（{err}={}），本次退回 {}；settings.json 不改动",
                            configured.display(),
                            if err == ERROR_HOTKEY_ALREADY_REGISTERED {
                                "已被占用"
                            } else {
                                "其它原因"
                            },
                            fallback.display()
                        );
                    }
                    Err(err2) => {
                        hotkey_lock(&HOTKEY_RESULT).push(HotkeyOutcome::Rejected {
                            slot: HotkeySlot::Toggle,
                            tried: configured,
                            err: err2,
                            now_active: None,
                        });
                        eprintln!(
                            "hotkey: {} 与出厂值 {} 都注册失败（{err} / {err2}）⇒ 全局呼出不可用，消息窗仍在线（SendTo/单实例不受影响）",
                            configured.display(),
                            fallback.display()
                        );
                    }
                }
            }
        }
        // 批次 R：补字符热键。它没有"出厂值"可退——退成一个用户没要的键比没有这个键更糟，
        // 所以失败就是失败：记一行 + 推一条回执（`now_active: None` = 系统里没装着），
        // settings.json 照旧不动（下次启动还会照配置再试一次）。
        if let Some(tb) = typeback {
            match apply_hotkey(hwnd, HotkeySlot::Typeback, tb) {
                Ok(()) => {
                    *hotkey_lock(&TYPEBACK_ACTIVE) = Some(tb);
                    eprintln!(
                        "typeback: {} registered（按下补入呼出键那个字符）",
                        tb.display()
                    );
                }
                Err(err) => {
                    hotkey_lock(&HOTKEY_RESULT).push(HotkeyOutcome::Rejected {
                        slot: HotkeySlot::Typeback,
                        tried: tb,
                        err,
                        now_active: None,
                    });
                    eprintln!(
                        "typeback: {} 注册失败（{err}={}）⇒ 本次不启用补字符，字符仍然打不出来",
                        tb.display(),
                        if err == ERROR_HOTKEY_ALREADY_REGISTERED {
                            "已被占用"
                        } else {
                            "其它原因"
                        }
                    );
                }
            }
        }
        // FR-WIN-11：本实例若以管理员运行，普通权限的次进程（再双击 exe、资源管理器「发送到」）
        // 发来的 WM_COPYDATA 会被 UIPI 静默丢弃 ⇒ 只对该消息窗放行 WM_COPYDATA 这一条，
        // 不放开其它消息、不改完整性级别（放行范围＝这个 hwnd 的消息队列）。
        match ChangeWindowMessageFilterEx(hwnd, WM_COPYDATA, MSGFLT_ALLOW, None) {
            Ok(()) => {
                eprintln!("ipc: 已对本消息窗放行 WM_COPYDATA（管理员实例可收普通权限次进程）")
            }
            Err(e) => eprintln!("ipc: WM_COPYDATA 放行失败（不影响同级实例间转发）: {e}"),
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_TOGGLE);
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_TYPEBACK);
        let _ = KillTimer(Some(hwnd), HOTKEY_SUSPEND_TIMER);
        let _ = DestroyWindow(hwnd);
    });
}

// ---- LauncherApp ----

/// 搜索框的 widget `Id`。快捷键分支要在 TextEdit 之外改它的光标状态，两处必须取同一个键。
fn search_field_id() -> egui::Id {
    egui::Id::new("search_input")
}

impl LauncherApp {
    fn new(
        tray_rx: mpsc::Receiver<TrayEvent>,
        hotkey_rx: mpsc::Receiver<()>,
        ipc_rx: mpsc::Receiver<IpcMsg>,
        notify_tx: mpsc::Sender<Result<String, String>>,
        notify_rx: mpsc::Receiver<Result<String, String>>,
        self_hwnd: Option<HWND>,
        intent: StartupIntent,
        // 批次 F1 的主题由 `main` 解析一次后传进来：窗口初始高度要按主题行高算，
        // 而 `Themes\` 只该读一遍（坏文件在日志里只出现一次）。
        theme: theme::Theme,
        theme_choices: Vec<String>,
    ) -> Self {
        let (icon_tx, icon_rx) = mpsc::channel();
        let (apps_tx, apps_rx) = mpsc::channel();
        let (path_tx, path_rx) = mpsc::channel();
        // 清单文件不存在（首次运行）→ 用内置系统工具播种一次并落盘；此后它就是一张普通用户清单，
        // 全部可编辑/删除（对标 Delphi WriteDefaultShortCutList）。文件已存在（哪怕被清空）不覆盖。
        //
        // E1（批次 X）：以前"解析失败"和"文件不存在"都落回空清单，而下面那条播种/补齐路径会把
        // 空清单整张写回去 ⇒ 用户条目被静默销毁，连现场都不留。现在 `load_*` 先把坏文件改名隔离
        // （`.bad`）再回来，这里只把它当空清单用，并且**只有真的没有文件时才播种**。
        let mut broken_list: Option<Corrupted> = None;
        let mut items = if source::application::user_list_exists() {
            let (list, bad) = source::application::load_user_items();
            broken_list = bad;
            list
        } else {
            let seeds = source::application::builtin_seeds();
            source::application::save_user_items(&seeds);
            seeds
        };
        items = dedup_items(items);
        // §4.2.1 出厂命令集的补齐：老清单（settings 里的轮次落后）启动时补一次缺，补完记账。
        // 只加不删、已有条目一律不动；记账之后删掉的条目不会自己回来（找回用设置页那个按钮）。
        let (mut prefs, broken_settings) = source::application::load_settings();
        // 两种下场分开说：`Kept` 报得出留证落点，`Lost` 只能请人工处理，并且要立刻把"本轮对它
        // 只读不写"通知到下面那条补齐路径（2026-10-22 真机⑫：留证没成功时照样播种写盘，
        // 把唯一那份坏文件覆盖了——那一次日志里就写着「覆盖写会丢掉唯一证据」）。
        let mut evidence_lost = false;
        for (label, c, fallback) in [
            ("settings.json", &broken_settings, "出厂默认"),
            ("user_items.json", &broken_list, "空清单"),
        ] {
            match c {
                Some(Corrupted::Kept(p)) => eprintln!(
                    "配置损坏：{label} 解析失败，已隔离为 {}，本次按{fallback}启动（原文件未被覆盖）",
                    p.display()
                ),
                Some(Corrupted::Lost) => {
                    evidence_lost = true;
                    eprintln!(
                        "配置损坏：{label} 解析失败且留证没成功 ⇒ 本轮对它只读不写（写一次就覆盖掉唯一证据），请人工备份后删掉这个文件"
                    );
                }
                None => {}
            }
        }
        // 两句合成一句给用户看（Toast 只有一条，说两遍等于没说）。路径是他自己的文件名，
        // 出现在窗口里不是日志里 ⇒ 不违 FR-SYS-03 那条"日志不记用户原文"。
        let kept = |c: &Option<Corrupted>| match c {
            Some(Corrupted::Kept(p)) => Some(
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| p.display().to_string()),
            ),
            Some(Corrupted::Lost) | None => None,
        };
        let notice = match (kept(&broken_list), kept(&broken_settings)) {
            (Some(items_bad), Some(settings_bad)) => Some(format!(
                "⚠ 两个配置文件坏了，已分别改名留底：{items_bad} ／ {settings_bad}，本次从空清单 + 出厂默认启动"
            )),
            (Some(a), None) | (None, Some(a)) => Some(format!(
                "⚠ 配置文件坏了，已改名留底：{a}，本次从出厂默认启动（你的原文件没有被覆盖）"
            )),
            (None, None) => None,
        };
        // 留证没成功那句话不许被"已改名留底"盖掉：两种下场说成一件事就是骗用户。
        let notice = if evidence_lost {
            Some(match notice {
                Some(s) => format!("{s}；另有一份留证没成功 ⇒ 本轮对它只读不写，请人工处理"),
                None => "⚠ 配置文件坏了，但留证没成功（改名与另存副本都失败）⇒ 本轮对它只读不写，请人工备份后删掉"
                    .to_string(),
            })
        } else {
            notice
        };
        if prefs.seed_round < source::application::SEED_ROUND {
            let added = source::application::merge_missing_seeds(&mut items);
            if added > 0 {
                if evidence_lost {
                    eprintln!(
                        "source: 出厂命令集本可补 {added} 条，但坏文件留证没成功 ⇒ 这次不写清单（补齐只在内存里生效）"
                    );
                } else {
                    source::application::save_user_items(&items);
                    eprintln!(
                        "source: 出厂命令集补齐 {added} 条（第 {} 轮，老清单 {} 条）",
                        source::application::SEED_ROUND,
                        items.len() - added
                    );
                }
            }
            prefs.seed_round = source::application::SEED_ROUND;
            if evidence_lost {
                // 连记账那一格也不写：settings.json 可能正是没留住证据的那一份。
                // 代价是下一次启动会再补一轮种子（只加不删，幂等），比覆盖掉现场轻。
                eprintln!(
                    "source: 留证没成功 ⇒ 本轮不写 settings.json（seed_round 只在内存里推进）"
                );
            } else {
                source::application::save_settings(&prefs);
            }
            // 批次 G1：同一轮把出厂分类回填到老条目（uid 对得上且标签为空才补）。
            // 补标签只发生在 seed_round 进位这一次——之后用户清掉标签不会再被补回。
            let tagged = source::application::apply_factory_tags(&mut items);
            if tagged > 0 {
                if evidence_lost {
                    eprintln!(
                        "source: 出厂分类回填 {tagged} 条（坏文件留证没成功 ⇒ 这次不写清单）"
                    );
                } else {
                    source::application::save_user_items(&items);
                    eprintln!("source: 出厂分类回填 {tagged} 条（已写清单）");
                }
            }
        }
        // 2026-10-23 一次性改名：老清单里的「设置·X」→「Win·X」/「启动器·X」，
        // 总入口「设置」→「Windows 设置」（用户反馈敲「设置」出来四十行）。
        // 与 seed_round 无关——补种只加不删，改名得单独走一趟；幂等，第二轮起恒为 0。
        let renamed = source::system::rename_legacy_settings(&mut items);
        if renamed > 0 {
            if evidence_lost {
                eprintln!("source: 设置条目改名 {renamed} 条（坏文件留证没成功 ⇒ 这次不写清单）");
            } else {
                source::application::save_user_items(&items);
                eprintln!("source: 设置条目改名 {renamed} 条（已写清单）");
            }
        }
        // 2026-10-23 同目标去重：A9 从开始菜单导进过一条 Store 版「设置」，与出厂的
        // `ms-settings:` 打开的是同一个界面 ⇒ 清单一度两条同名同义。用户裁定保留协议那条、
        // 隐藏 Store 那条；走 hidden[] 而不是删除，想找回改 settings.json 就行。
        let dup_settings: Vec<String> = items
            .iter()
            .filter(|i| {
                i.kind == ItemKind::App
                    && i.path
                        .to_ascii_lowercase()
                        .contains("windows.immersivecontrolpanel")
                    && !prefs.hidden.iter().any(|h| h == &i.uid())
            })
            .map(|i| i.uid())
            .collect();
        if !dup_settings.is_empty() {
            prefs.hidden.extend(dup_settings.iter().cloned());
            if evidence_lost {
                eprintln!(
                    "source: 隐藏重复的 Store 版「设置」（坏文件留证没成功 ⇒ 这次不写 settings.json）"
                );
            } else {
                source::application::save_settings(&prefs);
                eprintln!("source: 隐藏重复的 Store 版「设置」（已写 hidden[]）");
            }
        }
        // FR-SRC-17 `hidden[]`：隐藏不是删除——补种会把条目塞回来，所以每一轮启动都再剔一次。
        // 只有真剔掉东西才写盘（平时 dropped=0，不该因为这条路径多写一次文件）。
        let dropped = source::application::drop_hidden(&mut items, &prefs.hidden);
        if dropped > 0 {
            if evidence_lost {
                eprintln!("source: hidden[] 隐藏 {dropped} 条（坏文件留证没成功 ⇒ 这次不写清单）");
            } else {
                source::application::save_user_items(&items);
                eprintln!("source: hidden[] 隐藏 {dropped} 条（已写清单）");
            }
        }
        let settings_open = intent.settings_page.is_some();
        let settings_tab = intent
            .settings_page
            .and_then(SettingsTab::from_page)
            .unwrap_or(SettingsTab::General);
        // 批次 T：`AppSettings` 不再是 `Copy`（多了一张引擎表），下面 `prefs` 要按值进结构体，
        // 所以这三样初值先取出来，不做整串 clone。
        let hotkey_active = prefs.toggle_hotkey;
        let typeback_active = prefs.typeback_hotkey;
        let typeback_draft = prefs.typeback_hotkey.map(|s| HotkeySpec {
            mods: s.mods_named(),
            vk: s.vk,
        });
        // 批次 F1：主题在 `main` 里就解析好了（窗口初始高度要按主题行高算），
        // 首帧的窗口底色 / 字号 / 行高直接读它，不留一帧"默认外观"。
        // 真正装进 `ctx` 在 `new` 末尾（那时 `CTX` 已 set）。
        let mut seed = Self {
            query: String::new(),
            clear_query_next_frame: false,
            items,
            index: matching::MatchIndex::default(),
            apps_tx,
            apps_rx,
            icon_tx: icon_tx.clone(),
            icons: std::collections::HashMap::new(),
            path_tx,
            path_rx,
            path_states: std::collections::HashMap::new(),
            path_in_flight: false,
            path_scanned_at: Instant::now(),
            sources: source::default_sources(),
            startup_notice: notice,
            selected: 0,
            scroll_followed: 0,
            focus_query: intent.visible && intent.dialog.is_none() && !settings_open,
            dialog: intent.dialog,
            dialog_enter_armed: false,
            confirm: None,
            internal_confirm: None,
            about_open: false,
            help_entry: None,
            pending_destructive: None,
            menu_open: false,
            win_drag: None,
            settings_open,
            settings_tab,
            prefs,
            theme,
            theme_applied: None,
            theme_font: None,
            theme_choices,
            icon_rx,
            tray_rx,
            hotkey_rx,
            ipc_rx,
            ipc_pending: Vec::new(),
            pending_batch: intent.batch,
            hide_pending: !intent.visible,
            notify_tx,
            notify_rx,
            visible: intent.visible,
            shown_at: None,
            ime_before: None,
            self_hwnd,
            hide_at: None,
            had_focus_since_show: false,
            toast: None,
            hud: hud::Hud::new(),
            hotkey_recording: false,
            // 先按配置值显示；第一帧再从 HOTKEY_ACTIVE 校正（启动可能退回过出厂值）。
            hotkey_active,
            hotkey_awaiting: None,
            hotkey_synced: false,
            // 批次 R：补字符键。同一套"先按配置显示、第一帧按系统真值校正"的口径。
            // 草稿把 NOREPEAT 剥掉：四个复选框读写的是实义修饰位，注册时才补回 NOREPEAT。
            typeback_active,
            typeback_draft,
            typeback_awaiting: None,
            typeback_armed: false,
            keypick_suspended: false,
        };
        // 立即为已有项起图标加载器（source_path 为空的种子会被 extract 自然跳过）。
        let source_paths: Vec<String> = seed
            .items
            .iter()
            .map(|i| i.source_path.clone())
            .filter(|p| !p.is_empty())
            .collect();
        spawn_icon_loader(icon_tx, source_paths);
        // FR-SRC-06：启动就先在后台算一轮四态，首屏列表（很快会呼出）拿到的是缓存而不是
        // 现场 I/O；之后呼出与每 3 秒各刷新一次（`refresh_path_states`）。
        seed.refresh_path_states(true);
        // 批次 A10：按注册表初始化各源。关掉的源**不初始化**——不建索引、不开后台线程。
        // 放在 `new` 而不是只靠 `ui()` 里那次兜底，是为了让静默启动（开机自启那一类）也在后台
        // 把文件索引建好；否则用户第一次呼出时看到的一定是「索引建立中」，而他要的恰恰是那一刻。
        for s in seed.sources.iter_mut() {
            if seed.prefs.source_on(s.id()) {
                s.init();
            } else {
                eprintln!("数据源 {}：已在设置里关闭，本次不初始化", s.id().as_str());
            }
        }
        // 启动即直接可见时永远走不到 `show_now()` 的「从不可见到可见」边沿，`on_show()` 里的输入法
        // 切换与失焦武装一次都不跑 ⇒ 冷启动后第一次打字被中文输入法截走（2026-10-18 用户裁定补一次）。
        // 判据取 focus_query（＝首屏就是搜索页）：弹窗/设置页启动时焦点归它们，不能让 on_show 抢回去。
        if seed.focus_query {
            seed.on_show();
        }
        // 首帧前把主题装进 egui：字体、尺寸表、控件配色一次到位。
        // 用构造时已解析好的那份，不再读一遍 `Themes\`（坏文件只该在日志里出现一次）。
        if let Some(ctx) = CTX.get() {
            let (theme, choices) = (seed.theme.clone(), seed.theme_choices.clone());
            seed.use_theme(ctx, theme, choices);
        }
        seed
    }

    /// 批次 F1（FR-UIX-01）：把 `prefs.theme` 解析成生效主题并装进 egui。
    ///
    /// 只在「启动 / 每次呼出 / 设置页换主题」这三处调用，不在 `ui()` 里逐帧解析——
    /// `load` 会读 `Themes\` 目录，而 `System` 还要读注册表；呼出时重解析是为了让
    /// 隐藏期间切换的系统明暗在下次出现时生效。
    fn sync_theme(&mut self, ctx: &egui::Context) {
        let (theme, choices) = theme::load(&self.prefs.theme);
        self.use_theme(ctx, theme, choices);
    }

    /// 装一套已解析好的主题。[`Self::sync_theme`] 与启动路径共用，避免
    /// `new()` 里为了首帧和末尾各读一遍 `Themes\`。
    fn use_theme(&mut self, ctx: &egui::Context, theme: theme::Theme, choices: Vec<String>) {
        self.theme_choices = choices;
        if self.theme_applied.as_ref() == Some(&theme) {
            self.theme = theme;
            return;
        }
        if self.theme_font.as_deref() != Some(theme.font_name.as_str()) {
            theme::install_fonts(ctx, &theme.font_name);
            self.theme_font = Some(theme.font_name.clone());
        }
        eprintln!(
            "主题：{}（字体 {}，底色 {:02X}{:02X}{:02X}，不透明度 {}）",
            theme.name,
            theme.font_name,
            theme.background.r(),
            theme.background.g(),
            theme.background.b(),
            theme.opacity
        );
        theme.apply(ctx);
        // 2026-10-24 用户裁定：换主题后窗口高度要按新几何重算，保持一屏正好 10 行。
        // 批次 F6 起行高由「行密度」决定，主题只在「跟随主题」档下供行数——但主题仍供
        // `padding`（外框内边距）与输入框字号，所以重算这件事照旧要做，只是理由换了。
        // 只在"真的换了一套主题"时动手：启动首帧由 `with_inner_size` 给同一个值；手动拖过
        // 的尺寸在呼出/隐藏之间也不动——`sync_theme` 每次呼出都会跑，但主题没变时上面那个
        // 相等判断已经先返回了。
        let first_apply = self.theme_applied.is_none();
        let height = window_height(
            &theme,
            self.prefs.row_density,
            self.prefs.appearance.status_bar,
        );
        self.theme = theme.clone();
        self.theme_applied = Some(theme);
        if !first_apply {
            self.resize_to_list(ctx, height, "换主题");
        }
        ctx.request_repaint();
    }

    /// 把窗口高度设成"正好一屏 [`LIST_ROWS`] 行"（宽度保持现状，不小于 [`MIN_W`]）。
    /// 换主题与换行密度两条路共用，`why` 只进日志。
    fn resize_to_list(&self, ctx: &egui::Context, height: f32, why: &str) {
        let size = ctx.viewport_rect().size();
        let next = egui::vec2(size.x.max(MIN_W as f32), height);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(next));
        let (row_h, _, spacing) = row_geometry(self.prefs.row_density, &self.theme);
        eprintln!(
            "{why}：行密度 {}（行高 {row_h} + 行距 {spacing}）⇒ 窗口高度重算为 {}（原 {}）",
            self.prefs.row_density.as_str(),
            height,
            size.y.round()
        );
    }

    /// 列表此刻的行几何（行高、图标边长、行距）。渲染侧每帧调一次，来源是设置项
    /// `row_density`（[`crate::row_geometry`]）；「跟随主题」档才回落到主题的 `row_height`。
    fn row_geom(&self) -> (f32, f32, f32) {
        row_geometry(self.prefs.row_density, &self.theme)
    }

    /// 批次 F8（FR-UIX-02）：宽度设置即时生效。高度保持现状——宽度不参与"一屏十行"。
    fn apply_width(&self, ctx: &egui::Context, width: f32, why: &str) {
        let w = source::application::clamp_window_width(width);
        let size = ctx.viewport_rect().size();
        if (size.x - w).abs() < 0.5 {
            return; // 已经是这个宽度：别每帧都发一条视图命令
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            w,
            size.y.max(MIN_H as f32),
        )));
        eprintln!("{why}：窗口宽度 → {w}（原 {}）", size.x.round());
    }

    /// 批次 F8（FR-UIX-07）：淡入进度 0..1。没开、已经到位、或正开着设置页都返回 `None`
    /// （设置页是低频界面，不做淡入）。
    fn fade_progress(&self) -> Option<f32> {
        if !self.prefs.appearance.fade || self.settings_open {
            return None;
        }
        let t = self.shown_at?.elapsed().as_secs_f32() / FADE_IN_SECS;
        (t < 1.0).then(|| 1.0 - (1.0 - t).powi(3))
    }

    /// FR-SRC-06：派一轮后台路径四态检查。`force` = 忽略最小间隔（呼出时总想拿最新结果）。
    /// 同一时刻只跑一波：断开的网络盘会让检查线程挂很久，攒线程没有意义。
    fn refresh_path_states(&mut self, force: bool) {
        if self.path_in_flight || self.items.is_empty() {
            return;
        }
        if !force && self.path_scanned_at.elapsed() < PATH_RESCAN_INTERVAL {
            return;
        }
        self.path_in_flight = true;
        self.path_scanned_at = Instant::now();
        source::pathcheck::spawn_scan(self.path_tx.clone(), self.items.clone());
    }

    /// 把查询框整串换成 `text`、光标放到末尾、要焦点、要重绘。
    /// `log` 由调用方给**且不得带上 `text`**：这一条路的两类来源（自定义命令补全、文件模式前缀）
    /// 都可能把用户敲进去的原文回写，而 FR-SYS-03 定的口径是日志绝不记查询原文。
    /// （批次 U 之前只有 `complete_query` 一个函数，日志里直接内插了 `text`——那时 `text`
    /// 恒来自清单关键字，所以成立；批次 X 加了 Ctrl+F 这条路之后就不成立了。）
    fn set_query(&mut self, ctx: &egui::Context, text: String, log: &str) {
        let chars = text.chars().count();
        self.query = text;
        self.selected = 0;
        self.focus_query = true;
        // 光标位置存在 TextEditState 里，程序化换掉整个字符串不会带着它走（与 Tab 补全同一个坑）。
        if let Some(mut st) = egui::widgets::text_edit::TextEditState::load(ctx, search_field_id())
        {
            st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(chars),
            )));
            st.store(ctx, search_field_id());
        }
        ctx.request_repaint();
        eprintln!("{log}");
    }

    /// 补全：查询换成清单里那条命令的关键字 + 空格。
    fn complete_query(&mut self, ctx: &egui::Context, text: &str) {
        self.set_query(
            ctx,
            text.to_string(),
            &format!("自定义命令：查询补全为「{text}」"),
        );
    }

    /// 提示行被 Enter / 数字键 / 点击激活。查询已经补好（`"ping "`）就只提示"等参数"，
    /// 否则把关键字 + 空格写进查询框、光标落末尾（需求书 FR-SRC-05 那条 `Enter` 补全）。
    fn custom_hint_activate(&mut self, ctx: &egui::Context, hint: &source::custom::CustomHint) {
        if self.query == hint.completion {
            self.toast = Some((
                "继续输入参数后再回车".to_string(),
                Instant::now() + TOAST_TTL,
                TOAST_OK,
            ));
            ctx.request_repaint_after(TOAST_TTL);
            eprintln!(
                "自定义命令：查询已在「{}」，等待用户输入参数",
                hint.completion
            );
        } else {
            self.complete_query(ctx, &hint.completion);
        }
    }

    /// 窗口在系统那边是不是"最小化"状态。`self.visible` 只是本程序自己记的旗标，
    /// `Win+D` / 任务栏「显示桌面」那类是直接改 OS 窗口状态、我们一个回调都收不到 ⇒ 旗标还停在
    /// `true`。2026-10-22 真机③：呼出路径只问旗标，于是托盘「显示」整块被跳过（只发 `Focus`，
    /// 而 `SetForegroundWindow` 不会把最小化的窗口拉起来），热键那一按更被当成"再隐藏一次"。
    /// 判据要问 OS，不能问自己的旗标。
    fn is_iconic(&self) -> bool {
        self.self_hwnd
            .map(|h| unsafe { IsIconic(h).as_bool() })
            .unwrap_or(false)
    }

    /// FR-SRC-21：IPC 触发的呼出（已显示则只重抢焦点，不再走 on_show 重复切输入法）。
    fn show_now(&mut self, ctx: &egui::Context) {
        // 批次 F1：隐藏期间系统明暗可能已经切了（选择 `System` 时要用新的），
        // 用户也可能刚改过 `Themes\*.json`。呼出是"重新读盘"的自然时机。
        self.sync_theme(ctx);
        let iconic = self.is_iconic();
        if !self.visible || iconic {
            if iconic && self.visible {
                eprintln!("呼出：旗标说「已显示」不作数（系统在最小化态）⇒ 还原并显示");
            }
            self.visible = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            // winit 的可见标志会与实际窗口脱节（被系统/全屏程序最小化后，标志仍是「可见」，
            // Visible(true) 便不发 ShowWindow）；SW_RESTORE 不看标志，直接还原并显示。
            if let Some(hwnd) = self.self_hwnd {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                };
            }
            self.on_show();
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// FR-WIN-08：打开设置页并切到指定页码（1=通用 2=功能 3=热键 4=数据源 5=外观 6=高级）。
    fn open_settings_page(&mut self, page: u32) {
        self.settings_open = true;
        self.settings_tab = match SettingsTab::from_page(page) {
            Some(t) => t,
            None => {
                eprintln!(
                    "FR-WIN-08: -Preferences {page} 无对应页（1=通用 2=功能 3=热键 4=数据源 5=外观 6=高级），落回通用"
                );
                SettingsTab::General
            }
        };
        self.focus_query = false;
    }

    /// FR-WIN-08：多路径 SendTo —— 批量入库、按 uid 去重、落盘、补图标、Toast 回执（不弹框）。
    fn add_batch(&mut self, ctx: &egui::Context, paths: Vec<String>) {
        if paths.is_empty() {
            return;
        }
        let old = self.items.clone();
        let before = self.items.len();
        let incoming: Vec<LaunchItem> = paths
            .iter()
            .map(|p| dialog_for_sent_path(p).to_item())
            .collect();
        // 批次 D6：显式 SendTo 等于用户点名要它回来 ⇒ 撤销同名隐藏，
        // 否则界面报「已添加」，下一次启动又被 hidden[] 静默剔掉。
        let incoming_uids: Vec<String> = incoming.iter().map(|i| i.uid()).collect();
        if source::application::unhide_uids(
            &mut self.prefs.hidden,
            &mut self.prefs.hidden_stash,
            &incoming_uids,
        ) > 0
        {
            source::application::save_settings(&self.prefs);
        }
        let mut missing: Vec<String> = incoming
            .iter()
            .map(|i| i.source_path.clone())
            .filter(|p| !p.is_empty() && !self.icons.contains_key(p.as_str()))
            .collect();
        self.items = dedup_items([std::mem::take(&mut self.items), incoming].concat());
        self.carry_stats(&old);
        let added = self.items.len().saturating_sub(before);
        let skipped = paths.len().saturating_sub(added);
        missing.sort();
        missing.dedup();
        if !missing.is_empty() {
            spawn_icon_loader(self.icon_tx.clone(), missing);
        }
        self.save_items();
        let skipped_text = if skipped > 0 {
            format!("，{skipped} 条重复已跳过")
        } else {
            String::new()
        };
        self.toast = Some((
            format!("✓ 已添加 {added} 条{skipped_text}"),
            Instant::now() + TOAST_TTL,
            TOAST_OK,
        ));
        ctx.request_repaint_after(TOAST_TTL);
        eprintln!(
            "FR-WIN-08: 批量 SendTo {} 条 → 新增 {added}、跳过 {skipped}",
            paths.len()
        );
    }

    /// 在 Background 层铺一块「只认领拖拽、不认领点击」的隐形手柄。
    /// 背景层只会在该点没有任何控件认领拖拽时才赢，所以只铺真空白矩形，
    /// 不可能抢走搜索框、按钮、行标签与设置页控件的点击。
    fn drag_handle(
        &mut self,
        ui: &egui::Ui,
        id: &'static str,
        rect: egui::Rect,
        edge: u8,
        cursor: Option<egui::CursorIcon>,
        tag: &'static str,
    ) {
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return; // 窗口太窄/太矮，这段空白不存在：不铺，免得负尺寸分配
        }
        let ctx = ui.ctx().clone();
        egui::Area::new(egui::Id::new(id))
            .order(egui::Order::Background)
            .fixed_pos(rect.min)
            .show(&ctx, |ui| {
                let resp = ui.allocate_response(rect.size(), egui::Sense::drag());
                // on_hover_cursor 吃掉 self 返回新 Response（它只是往 ctx 里记一笔），所以链式接回。
                let resp = match cursor {
                    Some(icon) => resp.on_hover_cursor(icon), // 悬停成 ↔ / ↕，用户才找得到这 6px 在哪
                    None => resp,
                };
                if resp.drag_started() {
                    self.begin_win_drag(ui, edge, tag);
                }
            });
    }

    /// 同层版的拖窗手柄（2026-10-24）：只给「盖在滚动列表上」的这块用。
    ///
    /// [`Self::drag_handle`] 铺在独立的 `Order::Background` 层上，而 `ScrollArea` 收滚轮的条件是
    /// 「指针所在层就是自己这层」（`is_hovering_outer_rect` → `Context::rect_contains_pointer`）。
    /// 于是指针一进列表右侧这块矩形，滚轮就被背景层吃掉、列表纹丝不动——这就是主界面滚不动的根因。
    /// 改成在中央面板同一个 `Ui` 里注册：滚轮看层不看控件，层对上了就能滚。
    ///
    /// `Sense::drag()` 不含 click ⇒ 结果行照旧优先吃点击：命中测试里点击控件在上、拖拽控件在下时
    /// 两者各归各的；而拖拽控件里我们又是**先注册**的那一个（排在滚动区域之前），
    /// 后注册的控件在上只影响点击，不影响这块空白认领拖拽。
    fn inline_drag_handle(&mut self, ui: &egui::Ui, id: &'static str, rect: egui::Rect) {
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return; // 窗口太窄/太矮，这段空白不存在
        }
        let resp = ui.interact(rect, egui::Id::new(id), egui::Sense::drag());
        if resp.drag_started() {
            self.begin_win_drag(ui, EDGE_MOVE, "列表右侧空白");
        }
    }

    /// 四边 + 四角的隐形缩放手柄（2026-10-18 用户：「程序窗口应该允许拖动窗口边缘时可以改变大小，
    /// 现在无法改变大小」）。`with_decorations(false)` 把系统可命中的调整边框（HTLEFT/HTRIGHT…）也一并
    /// 去掉了，所以只能在客户区内自己铺 6px 的手柄，动作仍走下面那套按帧 `SetWindowPos`。
    /// 手柄只贴最外圈，且四边手柄让开四角、四角手柄只占 6×6，与移动手柄的矩形互不重叠。
    fn edge_grips(&mut self, ui: &egui::Ui) {
        const T: f32 = 6.0;
        let r = ui.ctx().viewport_rect();
        let (l, t, rt, b) = (r.min.x, r.min.y, r.max.x, r.max.y);
        let rect = |a: f32, c: f32, d: f32, e: f32| {
            egui::Rect::from_min_max(egui::pos2(a, c), egui::pos2(d, e))
        };
        let grips = [
            (
                "grip_edge_left",
                EDGE_LEFT,
                rect(l, t + T, l + T, b - T),
                Some(egui::CursorIcon::ResizeWest),
                "左边缘",
            ),
            (
                "grip_edge_right",
                EDGE_RIGHT,
                rect(rt - T, t + T, rt, b - T),
                Some(egui::CursorIcon::ResizeEast),
                "右边缘",
            ),
            (
                "grip_edge_top",
                EDGE_TOP,
                rect(l + T, t, rt - T, t + T),
                Some(egui::CursorIcon::ResizeNorth),
                "上边缘",
            ),
            (
                "grip_edge_bottom",
                EDGE_BOTTOM,
                rect(l + T, b - T, rt - T, b),
                Some(egui::CursorIcon::ResizeSouth),
                "下边缘",
            ),
            (
                "grip_corner_tl",
                EDGE_TOP | EDGE_LEFT,
                rect(l, t, l + T, t + T),
                Some(egui::CursorIcon::ResizeNwSe),
                "左上角",
            ),
            (
                "grip_corner_tr",
                EDGE_TOP | EDGE_RIGHT,
                rect(rt - T, t, rt, t + T),
                Some(egui::CursorIcon::ResizeNeSw),
                "右上角",
            ),
            (
                "grip_corner_bl",
                EDGE_BOTTOM | EDGE_LEFT,
                rect(l, b - T, l + T, b),
                Some(egui::CursorIcon::ResizeNeSw),
                "左下角",
            ),
            (
                "grip_corner_br",
                EDGE_BOTTOM | EDGE_RIGHT,
                rect(rt - T, b - T, rt, b),
                Some(egui::CursorIcon::ResizeNwSe),
                "右下角",
            ),
        ];
        for (id, edge, rect, cursor, tag) in grips {
            self.drag_handle(ui, id, rect, edge, cursor, tag);
        }
    }

    /// 记录一次拖窗/缩窗口锚点。拖窗只可能发生在「本窗口刚被按下」这一刻，此刻它必然是前台窗口，
    /// 所以用 GetForegroundWindow 拿 HWND（eframe 不把 winit Window 暴露给 App::update）。
    /// 坐标一律 Win32 物理像素，交给下面的按帧 SetWindowPos 跟随。
    fn begin_win_drag(&mut self, ui: &egui::Ui, edge: u8, tag: &'static str) {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_invalid() {
            eprintln!("左键拖空白（{tag}）：拿不到前台窗口，忽略");
            return;
        }
        let (Some(cursor), Some(origin), Some(size)) =
            (win_cursor(), win_origin(hwnd), win_size(hwnd))
        else {
            eprintln!("左键拖空白（{tag}）：GetCursorPos/GetWindowRect 失败，忽略");
            return;
        };
        // 带上「本帧有没有被按下」与触屏判定：一次拖不动就能从日志分清是
        // 手势没成立、HWND 拿错，还是 ScrollArea 的拖滚（OnTouch）抢走了拖拽。
        let pressed = ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary));
        eprintln!(
            "左键拖{}（{tag}）：hwnd={hwnd:?} 按下={pressed} 起点={origin:?} 尺寸={size:?}",
            if edge == EDGE_MOVE {
                "空白移动"
            } else {
                "边缘缩放"
            }
        );
        self.win_drag = Some(DragAnchor {
            hwnd,
            cursor,
            origin,
            size,
            edge,
        });
    }

    /// FR-WIN-02（批次 Q）+ 批次 R 的每帧处理：① 取热键线程的回执（两个槽共用一条队列）；
    /// ② 首帧把显示口径对齐到"系统里真正装着的那两组"；③ 取键（录主热键 / 给补字符键选键）
    /// 独占本帧按键；④ 把"取键期间临时注销全局键"这条边沿发给热键线程。
    /// 必须排在快捷键分支之前——否则录制时按 Ctrl+I 会先被「批量导入」抢走。
    fn hotkey_frame(&mut self, ui: &mut egui::Ui) {
        // 先 take 出来再遍历：`for o in std::mem::take(&mut *hotkey_lock(..))` 会把那个 MutexGuard
        // 存活到整个循环体结束（for 的 scrutinee 临时量就是循环作用域），期间热键线程想 push 一条
        // 新回执只能等着。不至于死锁（线程从不反向等 UI 的锁），但没必要留这个窗口。
        let outcomes = std::mem::take(&mut *hotkey_lock(&HOTKEY_RESULT));
        for o in outcomes {
            match o {
                HotkeyOutcome::Applied { slot, on } => match slot {
                    HotkeySlot::Toggle => {
                        self.hotkey_active = on;
                        self.hotkey_awaiting = None;
                        if self.prefs.toggle_hotkey != on {
                            self.prefs.toggle_hotkey = on;
                            source::application::save_settings(&self.prefs);
                        }
                        self.hotkey_note(&format!("✓ 全局热键已改为 {}", on.display()), true);
                    }
                    HotkeySlot::Typeback => {
                        self.typeback_active = Some(on);
                        self.typeback_awaiting = None;
                        // 草稿跟着注册成功的那一份走：复选框显示的永远是"系统里真装着的"，
                        // 不是"用户刚才点过但没装上的"。
                        self.typeback_draft = Some(HotkeySpec {
                            mods: on.mods_named(),
                            vk: on.vk,
                        });
                        if self.prefs.typeback_hotkey != Some(on) {
                            self.prefs.typeback_hotkey = Some(on);
                            source::application::save_settings(&self.prefs);
                        }
                        self.hotkey_note(&format!("✓ 补字符热键已设为 {}", on.display()), true);
                    }
                },
                HotkeyOutcome::Rejected {
                    slot,
                    tried,
                    err,
                    now_active,
                } => {
                    let why = if err == ERROR_HOTKEY_ALREADY_REGISTERED {
                        "已被系统或其它程序占用".to_string()
                    } else {
                        format!("注册失败（错误码 {err}）")
                    };
                    match slot {
                        HotkeySlot::Toggle => {
                            self.hotkey_awaiting = None;
                            self.hotkey_active = match now_active {
                                // prefs 不动：失败的那一份从没写进 settings.json。
                                Some(k) => k,
                                None => self.prefs.toggle_hotkey,
                            };
                            let tail = match now_active {
                                Some(k) => format!("，当前仍是 {}", k.display()),
                                None => "，且原来的键也没能装回去 ⇒ 暂时只能用托盘呼出".to_string(),
                            };
                            self.hotkey_note(&format!("⚠ {} {why}{tail}", tried.display()), false);
                        }
                        HotkeySlot::Typeback => {
                            self.typeback_awaiting = None;
                            self.typeback_active = now_active;
                            let tail = match now_active {
                                Some(k) => format!("，当前仍是 {}", k.display()),
                                None => " ⇒ 这个字符还是打不出来".to_string(),
                            };
                            self.hotkey_note(
                                &format!("⚠ 补字符热键 {} {why}{tail}", tried.display()),
                                false,
                            );
                        }
                    }
                }
                HotkeyOutcome::StartupFallback {
                    tried,
                    fell_back_to,
                    err,
                } => {
                    self.hotkey_active = fell_back_to;
                    let why = if err == ERROR_HOTKEY_ALREADY_REGISTERED {
                        "本次被占用".to_string()
                    } else {
                        format!("本次注册失败（{err}）")
                    };
                    self.hotkey_note(
                        &format!(
                            "⚠ 你设的 {} {why}，这次先用 {}",
                            tried.display(),
                            fell_back_to.display()
                        ),
                        false,
                    );
                }
            }
        }

        // ② 首帧对齐。热键线程可能先于 UI 注册完（正常），也可能句柄还没建（极早期启动）；
        // 后者留待下一次有回执时再校正，这里不做轮询、不阻塞。
        if !self.hotkey_synced {
            self.hotkey_synced = true;
            if let Some(active) = *hotkey_lock(&HOTKEY_ACTIVE) {
                self.hotkey_active = active;
            }
            // 补字符这一格反过来：`None` 也是有效信息（没启用，或启用但注册失败被回执说过了），
            // 所以直接照抄，不做 `if let Some`。
            self.typeback_active = *hotkey_lock(&TYPEBACK_ACTIVE);
        }

        // ③ 取键，④ 边沿通知。
        self.take_keypick(ui);
        self.sync_keypick_suspend();
    }

    /// 取键态下吃掉本帧按键并分派给两个入口之一。互斥是结构自带的：`take_record_press` 会把
    /// 整帧的 Key 事件清空，一帧只可能有一个框拿到键，所以先判谁在等就够了。
    fn take_keypick(&mut self, ui: &mut egui::Ui) {
        if !self.hotkey_recording && !self.typeback_armed {
            return;
        }
        // 换键请求还悬着（回执未到）时不接受第二次取键，否则会覆盖 REQUEST 槽里的组合。
        if self.hotkey_awaiting.is_some() || self.typeback_awaiting.is_some() {
            return;
        }
        let Some((key, m)) = take_record_press(ui) else {
            return;
        };
        if self.typeback_armed {
            // 补字符的键框只取主键——修饰键来自那四个复选框（照 Delphi 版 frmConfig 的分工：
            // 「[键] + ☐Windows ☑Alt / ☐Ctrl ☐Shift」）。所以这里刻意无视 `m`。
            match key {
                egui::Key::Escape | egui::Key::Tab => {
                    self.typeback_armed = false;
                    eprintln!("设置页：补字符键选键取消（按 Esc / Tab）");
                }
                _ => {
                    let Some(vk) = vk_from_egui_key(key) else {
                        return; // 方向键之类：继续等，不提示
                    };
                    let shown = HotkeySpec { mods: 0, vk }.display();
                    // 已有草稿 ⇒ 只换主键，四个复选框选出来的修饰键保留；没有草稿（第一次设）
                    // ⇒ 连修饰键一起从"空"开始，由用户勾。
                    match &mut self.typeback_draft {
                        Some(d) => d.vk = vk,
                        None => self.typeback_draft = Some(HotkeySpec { mods: 0, vk }),
                    }
                    self.typeback_armed = false;
                    eprintln!("设置页：补字符键的键框收到 {shown}（修饰键看四个复选框）");
                }
            }
            return;
        }
        match key {
            egui::Key::Escape | egui::Key::Tab => {
                self.hotkey_recording = false;
                eprintln!(
                    "设置页：热键录制取消（按 {}），当前仍是 {}",
                    if key == egui::Key::Escape {
                        "Esc"
                    } else {
                        "Tab"
                    },
                    self.hotkey_active.display()
                );
            }
            egui::Key::Backspace | egui::Key::Delete => {
                self.hotkey_recording = false;
                eprintln!("设置页：热键录制收到 Backspace/Delete ⇒ 恢复出厂组合");
                self.request_hotkey(HotkeySpec::DEFAULT);
            }
            other => {
                let Some(spec) = spec_from_press(other, m) else {
                    // 主键不在可录集合（方向键、Esc 之外那些）：继续等，不报"非法"，
                    // 免得用户只是按了一下 PgDn 就被提示一次。
                    return;
                };
                // 只有真正投递出去（换键）才算这次录制落定；本地判据拒绝时**留在录制态**，
                // 因为那几条 Toast 写的是「换一个」「请再录一次」。【2026-10-20 真机实测：原来这里
                // 无条件退出录制 ⇒ 按 `Ctrl+I` 撞上本程序键表后紧接着按 `Ctrl+F13`，第二次按键被当
                // 普通按键丢掉（日志只有冲突那一行，没有投递行），用户必须再点一次「录制新热键」，
                // 提示语与行为自相矛盾。】
                self.hotkey_recording = !self.request_hotkey(spec);
            }
        }
    }

    /// 批次 R：进出取键态 ⇒ 通知热键线程注销 / 装回主热键。
    /// 为什么这件事必须由 UI 侧发边沿、线程侧动手：注销与注册只发生在热键线程（hwnd 归属），
    /// 而"什么时候算退出取键"只有 UI 知道（落定、Esc、取消按钮、切页、关窗、隐藏）。
    /// 调用点两处：`hotkey_frame` 末尾（正常路径）与 `on_hide`——窗口一隐藏 eframe 就不再跑
    /// `ui()`，边沿检测等不到下一帧，全局呼出会一直停在注销状态（真等不到还有兜底定时器）。
    fn sync_keypick_suspend(&mut self) {
        let picking = self.hotkey_recording || self.typeback_armed;
        if picking == self.keypick_suspended {
            return;
        }
        self.keypick_suspended = picking;
        post_hotkey_signal(
            if picking {
                WM_APP_SUSPEND_HOTKEY
            } else {
                WM_APP_RESUME_HOTKEY
            },
            if picking {
                "取键期注销"
            } else {
                "取键后恢复"
            },
        );
    }

    /// 录制落定：合法性 → 本程序键表预检 → 投给热键线程试注册。
    /// 三级里前两级是本地判据（不碰系统），最后一级只有 Windows 知道答案，结果走回执；
    /// **只有注册成功才写 settings.json**（用户 2026-10-19 口径「立即生效、失败回滚旧热键」）。
    /// 返回值 = "这次录制到此为止"：`true` 只给真正投递成功那一条，其余（含"与当前一致"）都是
    /// `false` ⇒ 调用方留在录制态，让用户照着提示语直接按下一个组合。
    fn request_hotkey(&mut self, spec: HotkeySpec) -> bool {
        let show = spec.display();
        if let Err(msg) = spec.check() {
            self.hotkey_note(&format!("⚠ {show}：{msg}"), false);
            eprintln!(
                "设置页：热键 {show} 未采纳（{msg}），当前仍是 {}",
                self.hotkey_active.display()
            );
            return false;
        }
        if spec.conflicts_local(LOCAL_TAKEN) {
            self.hotkey_note(&format!("⚠ {show} 是本程序自己的快捷键，换一个"), false);
            eprintln!("设置页：热键 {show} 与本程序键表冲突，未采纳");
            return false;
        }
        // 批次 R 新增的一格带来的新冲突：同一个 vk+修饰组合在同一个线程上只能注册一次，
        // 撞进补字符那一格时 `RegisterHotKey` 给的是 1409，不挡的话用户看到的是"被别的程序占用"
        // ——占用者其实是我们自己。反向那条（补字符撞主热键）由 `check_typeback` 负责。
        if let Some(tb) = self.typeback_active {
            if (tb.mods_named(), tb.vk) == (spec.mods_named(), spec.vk) {
                self.hotkey_note(
                    &format!("⚠ {show} 正被补字符热键占着，请先改掉或清除它"),
                    false,
                );
                eprintln!("设置页：热键 {show} 与补字符热键同组，未采纳");
                return false;
            }
        }
        if spec == self.hotkey_active {
            self.hotkey_note(&format!("ℹ {show} 就是当前热键，未改动"), true);
            eprintln!("设置页：热键 {show} 与当前一致，未改动");
            return false;
        }
        if !post_hotkey_request(HotkeySlot::Toggle, spec) {
            self.hotkey_note("⚠ 热键线程还没就绪，请再录一次", false);
            return false;
        }
        self.hotkey_awaiting = Some(spec);
        eprintln!("设置页：热键换键请求已投递 → {show}（等注册结果）");
        true
    }

    /// 补字符热键的"应用"。判据比主热键多两条（必须带修饰键、不许与主热键同组），
    /// 少一条（没有出厂值可退，失败就是失败）。与主热键同一口径：**只有注册成功才写盘**。
    fn request_typeback(&mut self, spec: HotkeySpec) -> bool {
        let show = spec.display();
        if let Err(msg) = spec.check_typeback(&self.hotkey_active) {
            self.hotkey_note(&format!("⚠ 补字符热键 {show}：{msg}"), false);
            eprintln!("设置页：补字符热键 {show} 未采纳（{msg}）");
            return false;
        }
        if spec.conflicts_local(LOCAL_TAKEN) {
            self.hotkey_note(&format!("⚠ {show} 是本程序自己的快捷键，换一个"), false);
            eprintln!("设置页：补字符热键 {show} 与本程序键表冲突，未采纳");
            return false;
        }
        if self.typeback_active == Some(spec) {
            self.hotkey_note(&format!("ℹ {show} 就是当前补字符热键，未改动"), true);
            eprintln!("设置页：补字符热键 {show} 与当前一致，未改动");
            return false;
        }
        if !post_hotkey_request(HotkeySlot::Typeback, spec) {
            self.hotkey_note("⚠ 热键线程还没就绪，请再点一次应用", false);
            return false;
        }
        self.typeback_awaiting = Some(spec);
        eprintln!("设置页：补字符热键请求已投递 → {show}（等注册结果）");
        true
    }

    /// 「清除」= 不要这个键了。注销动作仍然交给热键线程（hwnd 归属那条老规矩），
    /// 这里只投一条消息 + 就地把自己这三份状态与 settings.json 清干净。
    /// 这条路径不产回执：`UnregisterHotKey` 没有"失败要告诉用户"的那种失败（没装着也返回 false），
    /// 而清掉配置是本地就能做成的事。
    fn clear_typeback(&mut self) {
        post_hotkey_signal(WM_APP_CLEAR_TYPEBACK, "清除补字符热键");
        *hotkey_lock(&TYPEBACK_ACTIVE) = None;
        self.typeback_active = None;
        self.typeback_draft = None;
        self.typeback_awaiting = None;
        self.typeback_armed = false;
        if self.prefs.typeback_hotkey.take().is_some() {
            source::application::save_settings(&self.prefs);
        }
        eprintln!("设置页：补字符热键已清除（注销 + settings.json 置空）");
        self.hotkey_note("✓ 已清除补字符热键", true);
    }

    /// 热键相关的一条回执提示：统一走 Toast（沿用各操作的 TTL 与配色）。
    /// ctx 取全局 `CTX`（`run_native` 建窗时就 set 好了，比本方法任何调用点都早）。
    fn hotkey_note(&mut self, text: &str, ok: bool) {
        self.toast = Some((
            text.to_string(),
            Instant::now() + TOAST_TTL,
            if ok { TOAST_OK } else { TOAST_ERR },
        ));
        if let Some(ctx) = CTX.get() {
            ctx.request_repaint_after(TOAST_TTL);
            ctx.request_repaint();
        }
    }

    /// 设置页（2026-10-17 用户要求，参考 AHK 版设置窗口的左侧分类栏；只立当下有真实选项的分类，
    /// 不做无功能的占位开关）。入口：托盘「设置…」/ `Ctrl+,`；Esc 或「返回搜索」回搜索页。
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        // 顶栏空白也是拖动手柄（2026-10-18 用户：「顶部输入框右侧点击空白不能拖动，顶部的不是菜单栏吗？」）。
        // 右边界必须取"整行可用宽度"，不能用 `head_row.response.rect.max.x`：横向行的 response 矩形
        // 只覆盖**已用宽度**（返回按钮 + 标题 ≈ 110px），用它算出来的手柄宽度是负数、被 drag_handle
        // 直接丢弃 ⇒ 用户实测"进设置页就拖不动了"。
        let band = ui.available_rect_before_wrap();
        let mut heading_right = 0.0f32;
        let head_row = ui.horizontal(|ui| {
            if ui.button("← 返回搜索 (Esc)").clicked() {
                self.settings_open = false;
                self.focus_query = true;
            }
            heading_right = ui.heading("设置").rect.max.x;
        });
        let r = head_row.response.rect;
        self.drag_handle(
            ui,
            "window_drag_layer_settings_top",
            egui::Rect::from_min_max(
                egui::pos2(heading_right + 8.0, r.min.y),
                egui::pos2(band.max.x - 4.0, r.max.y),
            ),
            EDGE_MOVE,
            None,
            "设置页顶栏空白",
        );
        ui.separator();
        // 分类栏高度只能在 TopDown 父 Ui 上量：`Ui::horizontal` 把行高初值写死成
        // `spacing().interact_size.y`（ui.rs:2376-2379，注释"Assume there will be something
        // interactive"），所以横向行里的 `available_height()` 只有 ~28px。右侧内容区直接吃这个值
        // 就只剩一行高，用户复测「默认只看到第一个配置项，要滚鼠标才看到第二个」正是这个原因。
        let rail_h = ui.available_height().max(120.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width(84.0);
                for (label, tab) in [
                    ("通用", SettingsTab::General),
                    ("功能", SettingsTab::Features),
                    ("热键", SettingsTab::Hotkeys),
                    ("数据源", SettingsTab::Sources),
                    ("外观", SettingsTab::Appearance),
                    ("高级", SettingsTab::Advanced),
                ] {
                    if ui.selectable_label(self.settings_tab == tab, label).clicked() {
                        self.settings_tab = tab;
                    }
                }
            });
            ui.separator();
            let size = egui::vec2(ui.available_width(), rail_h);
            // 显式给内容区一块"剩余宽 × 整列高"的矩形并指定纵向布局：ScrollArea 的内容 Ui 由
            // `new_child` 建、只覆盖 max_rect 不覆盖 layout（scroll_area.rs:805-809），不指定 layout
            // 就会继承外层 horizontal 的 LeftToRight，把「SendTo 入口」「空格执行选中项」挤成同一行。
            ui.allocate_ui_with_layout(
                size,
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| match self.settings_tab {
                        SettingsTab::General => {
                            ui.label(egui::RichText::new("SendTo 入口").strong());
                            let mut enabled = source::application::sendto_installed();
                            let resp = ui.checkbox(&mut enabled, "右键文件/文件夹 → 发送到 → egui-launcher");
                            if resp.changed() {
                                if enabled {
                                    source::application::spawn_sendto_install(self.notify_tx.clone());
                                    eprintln!("设置页：SendTo 开关→开，后台安装（结果走 Toast）");
                                } else {
                                    let (text, color) = match source::application::uninstall_sendto() {
                                        Ok(s) => {
                                            eprintln!("设置页：SendTo 开关→关，{s}");
                                            (format!("✓ {s}"), TOAST_OK)
                                        }
                                        Err(s) => {
                                            eprintln!("设置页：SendTo 卸载失败：{s}");
                                            (format!("⚠ {s}"), TOAST_ERR)
                                        }
                                    };
                                    self.toast = Some((text, Instant::now() + TOAST_TTL, color));
                                    ui.ctx().request_repaint_after(TOAST_TTL);
                                }
                                ui.ctx().request_repaint(); // 开关态下一帧重读磁盘真实状态
                            }
                            ui.label(
                                egui::RichText::new(
                                    "开＝写 egui-launcher.lnk 到 SendTo 目录；关＝删除它（只删本程序创建的文件，同名他人文件不动）。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // FR-WIN-11 开机自启（批次 L，2026-10-19 用户裁定「默认关，由我手动打开」
                            // ⇒ 程序不主动往启动夹写文件；需求书原文「默认开」已按此更正）。
                            ui.label(egui::RichText::new("开机自启").strong());
                            let mut auto = source::application::startup_installed();
                            let resp = ui.checkbox(&mut auto, "登录时静默启动（托盘常驻，不弹窗口）");
                            if resp.changed() {
                                if auto {
                                    source::application::spawn_startup_install(self.notify_tx.clone());
                                    eprintln!("设置页：开机自启 开关→开，后台写启动夹（结果走 Toast）");
                                } else {
                                    let (text, color) = match source::application::uninstall_startup() {
                                        Ok(s) => {
                                            eprintln!("设置页：开机自启 开关→关，{s}");
                                            (format!("✓ {s}"), TOAST_OK)
                                        }
                                        Err(s) => {
                                            eprintln!("设置页：开机自启 卸载失败：{s}");
                                            (format!("⚠ {s}"), TOAST_ERR)
                                        }
                                    };
                                    self.toast = Some((text, Instant::now() + TOAST_TTL, color));
                                    ui.ctx().request_repaint_after(TOAST_TTL);
                                }
                                ui.ctx().request_repaint(); // 开关态下一帧重读磁盘真实状态
                            }
                            ui.label(
                                egui::RichText::new(
                                    "开＝写 egui-launcher.lnk 到「启动」文件夹、参数 -Startup（登录后不弹窗口）；关＝删除它（只删本程序创建、参数确为 -Startup 的文件）。默认关。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // FR-ACT-01 `space_to_run`（批次 J，2026-10-18 用户裁定：bool 扩成四档，
                            // 默认「打全名才执行」；旧 settings.json 里的 false/true 仍读成 off/always）。
                            ui.label(egui::RichText::new("空格执行选中项").strong());
                            let mut space = self.prefs.space_to_run;
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut space, SpaceRun::Word, "打全名才执行（默认）");
                                ui.selectable_value(&mut space, SpaceRun::Unique, "结果唯一才执行");
                            });
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut space, SpaceRun::Off, "只在框为空时");
                                ui.selectable_value(&mut space, SpaceRun::Always, "任何时候都执行");
                            });
                            if space != self.prefs.space_to_run {
                                self.prefs.space_to_run = space;
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：space_to_run = {}（已写 settings.json）",
                                    space.as_str()
                                );
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "打全名才执行（默认）＝查询整串正好等于高亮行的名称、或等于它别名表里的一个词时空格执行（jsb→记事本、Task Scheduler→自己）；只是前缀（tas、task sch）时空格仍当文本输入，想确认就 Tab 补全后按空格，或直接回车。\n结果唯一才执行＝筛选到只剩一条，空格即执行；代价是「唯一但还没打完」的查询里那个空格要用 Shift+空格 插。\n只在框为空时＝最保守，多词查询的空格完全不受影响，单手流仍要按回车。\n任何时候都执行＝空格永远是确认，代价是查询里打不出空格。\nShift+空格 在四档下都表示\"这个空格要真的打出来\"。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // FR-ACT-06 `tab_mode`（2026-10-18 用户要求：条件分流与 Delphi 专职导航
                            // 两种口径都做，默认前者；Shift+Tab 恒为上一项，不受此项影响）。
                            ui.label(egui::RichText::new("Tab 键行为").strong());
                            let mut mode = self.prefs.tab_mode;
                            ui.horizontal(|ui| {
                                ui.selectable_value(
                                    &mut mode,
                                    TabMode::Complete,
                                    "补全优先（默认）",
                                );
                                ui.selectable_value(&mut mode, TabMode::Navigate, "专职导航");
                            });
                            if mode != self.prefs.tab_mode {
                                self.prefs.tab_mode = mode;
                                source::application::save_settings(&self.prefs);
                                eprintln!("设置页：tab_mode = {mode:?}（已写 settings.json）");
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "补全优先＝只有当「能被这个查询补长的候选恰好一条」时 Tab 才补全整词（如 jsq→计算器）；候选多于一条说明歧义没消，Tab 改为循环下一项，绝不替你猜第一条。\n专职导航＝Tab 下一项、Shift+Tab 上一项（Delphi 版 ALTRun 的做法），代价是没有补全。\n两种口径都保留 ↑↓ 翻页与 PgUp/PgDn；Tab 导航到头回绕，↑↓ 仍是钳位不动。想要一条不被补全抢走的稳定导航，用 Ctrl+N / Ctrl+P（或 Ctrl+J / Ctrl+K）。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // FR-SRC-19 `learn_rank`（批次 K，2026-10-18 用户裁定：甲乙都做、默认甲）。
                            // 两种口径都只作用于同一匹配档位之内，档位本身仍是第一主键。
                            ui.label(egui::RichText::new("同档位内的学习排序").strong());
                            let mut rank = self.prefs.learn_rank;
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut rank, LearnRank::Count, "次数优先（默认）");
                                ui.selectable_value(&mut rank, LearnRank::Decay, "次数 + 时间衰减");
                            });
                            if rank != self.prefs.learn_rank {
                                self.prefs.learn_rank = rank;
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：learn_rank = {}（已写 settings.json）",
                                    rank.as_str()
                                );
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "这一层只在两条目的匹配档位相同时才决定先后，匹配更准的那条永远在前。\n次数优先（默认）＝执行次数多的在前，次数相同才看最近一次。刚用过一次的东西不会把用了几十次的老项挤下去。\n次数 + 时间衰减＝比 log2(1+次数) × 0.5^(距上次使用天数 / 7)：次数照旧计权重但被 log2 压过量级，一条项整整 7 天没用热度掉一半，所以\"上周用过一次的\"不会长期赖在前面。\n置顶（Ctrl+Shift+P）永远压过这一层；想钉住某个条目就用它，不用靠改口径绕。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // 2026-10-24 用户需求：一屏 10 行；默认按 Delphi 口径（全部结果、滚轮翻）。
                            ui.label(egui::RichText::new("主界面显示条数").strong());
                            let mut list_rows = self.prefs.list_rows;
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut list_rows, ListRows::All, "全部（默认）");
                                ui.selectable_value(&mut list_rows, ListRows::Ten, "10 条");
                            });
                            if list_rows != self.prefs.list_rows {
                                self.prefs.list_rows = list_rows;
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：list_rows = {}（已写 settings.json）",
                                    list_rows.as_str()
                                );
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "全部（默认）＝所有结果都进列表；窗口仍是一屏十行高，超出部分用滚轮翻。这一档与 Delphi 版 ALTRun 同口径：它那个「10 行」只是列表的高度，结果从不截断。\n10 条＝列表只出前 10 行，计算器 / 搜索这类提示行也占名额；窗口本来就是一屏十行高，所以不用滚轮。\n两档都只影响显示与键盘能走到哪一行：被截掉的条目不是没匹配上，换「全部」立刻可见（或把查询打得更准）。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // 批次 F6（2026-10-24 用户正文：「整体界面高度太高了，项与项之间的间距太大了」，
                            // 并点名「将以上 3 个做成可选择的配置，默认选择使用甲」）。
                            ui.label(egui::RichText::new("列表行密度").strong());
                            let mut density = self.prefs.row_density;
                            ui.horizontal_wrapped(|ui| {
                                ui.selectable_value(&mut density, RowDensity::Snug, "甲·紧凑（默认）");
                                ui.selectable_value(&mut density, RowDensity::Roomy, "乙·宽松");
                                ui.selectable_value(&mut density, RowDensity::Dense, "丙·极紧");
                                ui.selectable_value(&mut density, RowDensity::Theme, "跟随主题");
                            });
                            if density != self.prefs.row_density {
                                self.prefs.row_density = density;
                                source::application::save_settings(&self.prefs);
                                // 换档改的是"一屏正好十行"里的行高，所以窗口高度要立刻跟着重算——
                                // 与换主题走的是同一个 `resize_to_list`。
                                let h = window_height(
                                    &self.theme,
                                    density,
                                    self.prefs.appearance.status_bar,
                                );
                                self.resize_to_list(ui.ctx(), h, "换行密度");
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "甲·紧凑（默认）＝行高 34 / 图标 24 / 行距 2 ⇒ 窗口高 438。\n乙·宽松＝40 / 28 / 2 ⇒ 498。丙·极紧＝24 / 16 / 0 ⇒ 318。\n跟随主题＝行高与图标交还给主题的 `row_height` / `icon_size`（Light 54/32、Compact 44/26、Modern 56/32），窗口随之变高。\n四档都保持「一屏正好十行」：换档只改行高、窗口高度按新行高重算，不改窗口宽。\n上面那几个总高是按 Light 的内边距 14 算的，换主题只差内边距那几个像素；输入框那一行不受本项影响（高度固定 44，字号仍跟着主题的「输入字号」走）。\n参照系是那两个老版本——Delphi 版行高 16 且不带图标（纯文字才能这么紧），AHK 版行高 54 但行距 0 且窗口宽 700（松是靠宽撑起来的，不是靠行高）。这一套带图标、宽 500，所以默认取中间那一档。\n注意：不在「跟随主题」档时，主题 JSON 里的 `row_height` 与 `icon_size` 这两个键不参与渲染（其余 22 个键照常生效）。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // 批次 G1：缺省分类范围（主界面 Ctrl+G 一键轮换的就是这一格）。
                            ui.label(egui::RichText::new("缺省分类范围").strong());
                            let mut scope = self.prefs.default_scope;
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut scope, Scope::All, "全部（默认）");
                                ui.selectable_value(&mut scope, Scope::Basic, "基本");
                            });
                            if scope != self.prefs.default_scope {
                                self.prefs.default_scope = scope;
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：default_scope = {}（已写 settings.json）",
                                    scope.as_str()
                                );
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "全部（默认）＝清单里所有条目照常显示（本批之前的行为）。\n基本＝只显示出厂白名单（计算器、记事本、关机这些常用命令）＋你自己添加的条目；系统维护那一大批工具（设备管理器、注册表编辑器、36 个设置页……）平时不占列表。\n随时可以用 @分类 临时并进来：敲 @维护 把维护类整个列出，@维护 磁盘＝在维护类里搜「磁盘」，@基本 只看白名单；显式 @ 不受这一格影响。\nCtrl+G 在主界面一键轮换这一格。\n条目自己的分类在添加/编辑弹窗的「分类」格里改（逗号或空格分隔，可填多个）。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // FR-SRC-05 / §4.2.1：关键字唯一性提示。只提示、不阻断
                            // （需求书明写"不阻断启动，避免影响 S3"）；撞名时按清单先后取第一条。
                            ui.label(egui::RichText::new("关键字重复检查").strong());
                            let dups = source::custom::duplicate_keywords(&self.items);
                            if dups.is_empty() {
                                ui.label(
                                    egui::RichText::new("没有重复关键字。")
                                        .weak()
                                        .small(),
                                );
                            } else {
                                for (word, names) in dups.iter().take(6) {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "⚠ 「{word}」被 {} 条共用：{}",
                                            names.len(),
                                            names.join("、")
                                        ))
                                        .small()
                                        .color(TOAST_ERR),
                                    );
                                }
                                ui.label(
                                    egui::RichText::new(
                                        "触发独占时只认清单里先出现的那一条；学习别名也会算进这张表（同名不阻断使用）。",
                                    )
                                    .weak()
                                    .small(),
                                );
                            }
                            ui.separator();
                            // §4.2.1 / 2026-10-21 用户追加要求：出厂的系统条目被误删后要能找回。
                            ui.label(egui::RichText::new("出厂命令集").strong());
                            if ui.button("补回缺失的出厂命令").clicked() {
                                let added = source::application::merge_missing_seeds(&mut self.items);
                                // 补回来的条目里可能有用户明确隐藏的 ⇒ 立刻再剔一次，否则
                                // 「隐藏」会被这个按钮当成「删除后补回」悄悄撤销。
                                let hidden = source::application::drop_hidden(
                                    &mut self.items,
                                    &self.prefs.hidden,
                                );
                                if added > 0 || hidden > 0 {
                                    self.save_items();
                                }
                                eprintln!(
                                    "设置页：补回缺失的出厂命令 → 补 {added} 条，按 hidden[] 再剔 {hidden} 条"
                                );
                                let text = if added == 0 {
                                    "出厂命令一条不缺，清单没动".to_string()
                                } else if hidden > 0 {
                                    format!("✓ 已补回 {added} 条（其中 {hidden} 条按 hidden[] 隐藏）")
                                } else {
                                    format!("✓ 已补回 {added} 条出厂命令")
                                };
                                self.toast = Some((text, Instant::now() + TOAST_TTL, TOAST_OK));
                                ui.ctx().request_repaint_after(TOAST_TTL);
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "只补清单里缺的（按 uid / 名称认已存在），只加不删，改过的条目不动。删掉的内置条目下次启动不会自己回来，要找回就按这里。写在 settings.json 的 hidden 数组里的条目补回来后会再被剔除（隐藏 ≠ 删除）。",
                                )
                                .weak()
                                .small(),
                            );
                        }
                        SettingsTab::Features => {
                            ui.label(egui::RichText::new("批量导入").strong());
                            if ui.button("批量导入全部（Ctrl+I）").clicked() {
                                source::application::spawn_batch_import(self.apps_tx.clone());
                                self.toast = Some((
                                    "✓ 批量导入已启动，完成后自动合并".to_string(),
                                    Instant::now() + TOAST_TTL,
                                    TOAST_OK,
                                ));
                                ui.ctx().request_repaint_after(TOAST_TTL);
                                eprintln!("设置页：批量导入全部 已触发（后台扫描开始菜单/桌面）");
                            }
                            ui.label(
                                egui::RichText::new("扫描开始菜单/桌面的快捷方式并入清单（已有项不重复、不删除）。")
                                    .weak()
                                    .small(),
                            );
                            ui.separator();
                            // FR-SRC-17 `confirm_actions`（批次 A2）：四条不可撤销动作的执行确认。
                            ui.label(egui::RichText::new("危险操作先确认").strong());
                            let mut confirm_actions = self.prefs.confirm_actions;
                            if ui
                                .checkbox(
                                    &mut confirm_actions,
                                    "关机 / 重启 / 注销 / 清空回收站 执行前弹确认（默认开）",
                                )
                                .changed()
                            {
                                self.prefs.confirm_actions = confirm_actions;
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：confirm_actions = {confirm_actions}（已写 settings.json）"
                                );
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "关掉之后这四条在列表里回车即执行。它们都不可撤销，只在确定自己不会误按时关。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            ui.label(egui::RichText::new("系统命令").strong());
                            ui.label(
                                egui::RichText::new(
                                    "系统命令（关机、设置页直达、Windows 工具、ms-settings 页、音量与媒体）并进出厂条目，与普通条目一样可搜索、可置顶、可编辑、可删除。想「隐藏而不是删除」时，在主界面把查询框清空、选中一条后按 Ctrl+Del（隐藏项在「补回缺失的出厂命令」之后也不会回来），恢复入口在「高级」页。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // FR-SRC-16（批次 A3）：`>` 终端用哪个 shell。
                            ui.label(egui::RichText::new("终端 shell").strong());
                            let mut shell = self.prefs.term_shell();
                            let before = shell;
                            ui.horizontal(|ui| {
                                for s in source::terminal::TermShell::ALL {
                                    ui.selectable_value(&mut shell, s, s.label());
                                }
                            });
                            if shell != before {
                                self.prefs.terminal_shell = shell.as_str().to_string();
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：terminal_shell = {}（已写 settings.json）",
                                    shell.as_str()
                                );
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "查询框里敲 `>命令`（例：`>ipconfig /all`）会在新窗口里执行，执行完停在窗口里让你看输出；只敲 `>` 时列表给用法行。\ncmd / powershell / pwsh 各开一个新控制台窗口；Windows Terminal 那一档把同样的命令交给它的新标签（走 cmd）。选择立刻生效，不用重启。命令原文不进日志。",
                                )
                                .weak()
                                .small(),
                            );
                        }
                        SettingsTab::Hotkeys => {
                            // FR-WIN-02（批次 Q）：全局呼出热键可改。这是设置页里唯一的"输入型"控件，
                            // 按批次 K 那条例外不另配快捷键（进设置页已有 Ctrl+,）。
                            ui.label(egui::RichText::new("全局呼出 / 隐藏热键").strong());
                            ui.horizontal(|ui| {
                                let shown = if self.hotkey_recording {
                                    "按下新组合…（Esc / Tab 取消，Backspace / Delete 恢复默认）".to_string()
                                } else if let Some(w) = self.hotkey_awaiting {
                                    format!("{} 注册中…", w.display())
                                } else {
                                    self.hotkey_active.display()
                                };
                                ui.label(egui::RichText::new(shown).monospace());
                                if ui
                                    .button(if self.hotkey_recording { "取消录制" } else { "录制新热键" })
                                    .clicked()
                                {
                                    self.hotkey_recording = !self.hotkey_recording;
                                    eprintln!(
                                        "设置页：热键录制 {}（当前 {}）",
                                        if self.hotkey_recording { "开始" } else { "取消" },
                                        self.hotkey_active.display()
                                    );
                                }
                                let busy = self.hotkey_recording || self.hotkey_awaiting.is_some();
                                ui.add_enabled_ui(!busy, |ui| {
                                    if ui.button("恢复默认").clicked() {
                                        self.request_hotkey(HotkeySpec::DEFAULT);
                                    }
                                });
                            });
                            ui.label(
                                egui::RichText::new(
                                    "点「录制新热键」后直接按下想要的组合，不需要再点确认：注册成功立刻生效并写进 settings.json；被系统或其它程序占用会照实报错，并自动回到原来那一组（你原来的设置不会被冲掉）。\n可录的键：字母、数字、空格、回车、反引号 ` 和 F1–F24。非 F 键必须带 Ctrl、Alt 或 Win，因为裸 A、Shift+A 都只是打字，挂成全局键等于一路拦你输入。\n裸键只放开 ` 这一个（批次 R，2026-10-20 按你的要求加的）：代价是设成它以后全系统都打不出 ` 了，所以请照下面「补字符热键」那张卡片配一个出口；Shift+` 打出的 ~ 不受影响（本机实测）。录制期间程序会先把当前全局热键临时注销——不然裸 ` 被系统整条吞掉，本程序一个按键都收不到，永远录不成；退出录制（落定 / Esc / 取消按钮 / 关窗 / 隐藏到托盘）立刻装回去，另有一个 30 秒兜底定时器防止卡在注销状态。\nWin 组合键在这里录不了：egui 在 Windows 上不把 Win 当作修饰键传上来（你按 Ctrl 时它给的 command 位就是 Ctrl 的副本）。想要 Win+X，用它下面那张补字符卡片上的复选框。\n本程序自己的组合（Ctrl+N/E/D/I/J/K、Ctrl+Shift+N/D/P、Ctrl+数字、Alt+数字）会先在本地挡一道。全局占用只能靠\"试注册\"发现——Windows 没有枚举别人热键的 API，所以\"没报错\"只代表这一刻系统允许它注册上，不代表全机没人想用这个键。\n还有一点要知道：全局热键是被系统整条吞掉的，设成 Ctrl+A、Ctrl+C 这类常用编辑键，别的程序里那个动作就跟着没了（本程序没绑它们，所以不会在这里拦你，得自己避）。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // 批次 R：补字符热键。控件形态照 Delphi 版 frmConfig 的
                            // 「[键框] + ☐Windows / ☑Alt / ☐Ctrl / ☐Shift」（用户 2026-10-20 点名并给了
                            // 截图），分工也一样：左边那个框只管主键，修饰键全走复选框。
                            // 这一张同时是本机唯一能把 Win 位设进热键的入口——录制路径产不出它。
                            ui.label(
                                egui::RichText::new("补字符热键（按下它，打出被呼出键吞掉的那个字符）")
                                    .strong(),
                            );
                            let typed = typeback_char_for(self.hotkey_active.vk);
                            let bare_toggle = self.hotkey_active.mods_named() == 0;
                            ui.horizontal(|ui| {
                                let shown = if self.typeback_armed {
                                    "按下键…（Esc / Tab 取消）".to_string()
                                } else if let Some(a) = self.typeback_awaiting {
                                    format!("{} 注册中…", a.display())
                                } else if let Some(d) = self.typeback_draft {
                                    // 只显键名：修饰键由右边四个复选框各显自己的勾选态。
                                    HotkeySpec { mods: 0, vk: d.vk }.display()
                                } else {
                                    "（还没选键）".to_string()
                                };
                                if ui.button(shown).clicked() {
                                    self.typeback_armed = !self.typeback_armed;
                                    eprintln!(
                                        "设置页：补字符键选键 {}（当前草稿 {}）",
                                        if self.typeback_armed { "开始" } else { "取消" },
                                        self.typeback_draft
                                            .map(|d| d.display())
                                            .unwrap_or_else(|| "空".to_string())
                                    );
                                }
                                ui.label(egui::RichText::new("+").weak());
                                // 复选框读的是草稿、写的也是草稿；`mods` 里只放实义修饰位，
                                // NOREPEAT 到投递那一刻才补（见 `request_typeback`）。
                                let stored = self.typeback_draft.map(|d| d.mods).unwrap_or(0);
                                let mut bits = stored;
                                for (bit, name) in [
                                    (HKF_WIN, "Win"),
                                    (HKF_CONTROL, "Ctrl"),
                                    (HKF_ALT, "Alt"),
                                    (HKF_SHIFT, "Shift"),
                                ] {
                                    let mut on = bits & bit != 0;
                                    if ui.checkbox(&mut on, name).changed() {
                                        if on {
                                            bits |= bit;
                                        } else {
                                            bits &= !bit;
                                        }
                                    }
                                }
                                if bits != stored {
                                    // 还没选键时也允许先勾修饰键：先立一个 vk=0 的草稿，「应用」
                                    // 会被 `check()` 挡下并说明「只收字母、数字…」。
                                    match &mut self.typeback_draft {
                                        Some(d) => d.mods = bits,
                                        None => {
                                            self.typeback_draft = Some(HotkeySpec { mods: bits, vk: 0 })
                                        }
                                    }
                                }
                            });
                            ui.horizontal(|ui| {
                                let busy = self.typeback_armed || self.typeback_awaiting.is_some();
                                let draft = self.typeback_draft;
                                ui.add_enabled_ui(!busy, |ui| {
                                    if ui.button("应用").clicked() {
                                        match draft {
                                            Some(d) => {
                                                self.request_typeback(HotkeySpec {
                                                    mods: d.mods | HKF_NOREPEAT,
                                                    vk: d.vk,
                                                });
                                            }
                                            None => self.hotkey_note(
                                                "⚠ 补字符热键：先在左边点一下、再按一个键",
                                                false,
                                            ),
                                        }
                                    }
                                    if ui.button("清除").clicked() {
                                        if self.typeback_active.is_none() && draft.is_none() {
                                            self.hotkey_note("ℹ 本来就没有补字符热键", true);
                                        } else {
                                            self.clear_typeback();
                                        }
                                    }
                                });
                            });
                            ui.label(
                                egui::RichText::new(match self.typeback_active {
                                    Some(t) => {
                                        if !bare_toggle {
                                            format!(
                                                "○ 现在装着 {}，但呼出键 {} 带着修饰、不吞字符 ⇒ 按它不会有输出（改成裸键才用得上）。",
                                                t.display(),
                                                self.hotkey_active.display()
                                            )
                                        } else if let Some(c) = typed {
                                            format!("✓ 现在装着 {}：按它就把 {c} 投给此刻的焦点窗口。", t.display())
                                        } else {
                                            format!(
                                                "⚠ 现在装着 {}，但呼出键那个 VK 在当前键盘布局下反查不到字符（裸 F 键就是这种），按它不会有输出。",
                                                t.display()
                                            )
                                        }
                                    }
                                    None => match self.prefs.typeback_hotkey {
                                        Some(t) => format!("⚠ 设置里是 {}，但这次没能注册上，按了没反应。", t.display()),
                                        None => "未启用。".to_string(),
                                    },
                                })
                                .weak()
                                .small(),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "为什么需要它：呼出键设成裸 ` 之后，` 这个字符在全系统都打不出来了（Shift+` 的 ~ 不受影响），这一张卡片就是那个出口。呼出键带修饰时（出厂那组 Ctrl+Alt+Space 就是）它本来就不吞字符 ⇒ 这个键按下去什么都不做，避免凭空往你正在打的字里塞一个空格。\n要打的字符不是写死的，是按呼出键的 VK 反查当前键盘布局算出来的（MapVirtualKeyW），所以换布局不会补错键。\n必须带修饰键（Ctrl / Alt / Win 任一）：裸键挂上去只会把「吞字符的」换成它自己。也不能和呼出键是同一组——系统只会把它当成呼出键。推荐 Win+`：Win 位只有这里的复选框能设，而 Win+` 一般没被系统占。\n还没实测的那一条：注入走 SendInput 的 Unicode 通道（不带 VK，因此不会被自家那条裸 ` 再吞一次），记事本 / 浏览器 / VS Code / 终端 / 中文输入法里的实际表现要靠真机验收。"
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            let toggle_key = self.hotkey_active.display();
                            let typeback_key = self
                                .typeback_active
                                .map(|t| t.display())
                                .unwrap_or_else(|| "（补字符未启用）".to_string());
                            egui::Grid::new("hotkeys_grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                                for (key, desc) in [
                                    (toggle_key.as_str(), "全局呼出 / 隐藏（上一张卡片可改）"),
                                    (typeback_key.as_str(), "补字符：把呼出键吞掉的那个字符打回焦点窗口（上一张卡片可改）"),
                                    ("↑ / ↓", "选择条目"),
                                    ("PgUp / PgDn", "结果列表翻页（列表滚动跟随选中项）"),
                                    ("1 … 9 / 0", "执行第 1–9 行 / 第 10 行（列表前 10 行已标号）；裸键仅输入为空时生效，Ctrl+数字 / Alt+数字 不受限制"),
                                    ("Tab", "补全 / 循环下一项（按设置页「Tab 键行为」）；补全优先模式下只有「能被这个查询补长的候选恰好一条」才补全，否则导航；选中的是文件夹行时恒为进入该目录（FR-SRC-14）"),
                                    ("Shift+Tab", "循环上一项（两种 Tab 模式一致）"),
                                    ("Ctrl+N / Ctrl+P", "循环下一项 / 上一项（恒为导航，不会被补全抢走）"),
                                    ("Ctrl+J / Ctrl+K", "同上，另一对（保留给老习惯）"),
                                    ("Space", "执行选中项（默认仅输入为空时；设置页「空格直接执行」可改为始终执行）"),
                                    ("左键拖边缘/角", "改变窗口大小（四边 + 四角各 6px 隐形手柄，最小 360×240）"),
                                    ("Ctrl+= / Ctrl+-", "窗口放大 / 缩小一档（60×40 逻辑像素，键盘路径同上）"),
                                    ("左键拖空白", "移动窗口：顶栏空白（搜索框与 ☰ 之间、设置页标题右侧）或列表右侧空白；中键不再执行任何条目"),
                                    ("Ctrl+M / ☰", "主菜单：新建、批量导入、设置、速查表、打开日志、退出（批次 F6 起取代顶栏那个「＋ 新建」按钮）"),
                                    ("Enter", "运行选中项；弹窗/确认框确定；无匹配弹新增"),
                                    ("Esc", "关 ☰ 菜单 → 关确认框 → 关弹窗 → 设置页返回 → 清空输入 → 关闭到托盘（按序）"),
                                    ("Ctrl+Shift+N", "新建快捷项（选中的是文件行时预填这一行的路径与类型）"),
                                    ("Ctrl+E", "编辑选中项"),
                                    ("Delete", "删除选中项（仅输入为空时；弹确认框）"),
                                    ("Ctrl+Shift+D", "删除选中项（不受输入限制；弹确认框）"),
                                    ("Ctrl+D", "打开选中项所在目录（文件行：在资源管理器里定位这个文件）"),
                                    ("Ctrl+Shift+P", "置顶 / 取消置顶选中项（📌）"),
                                    ("Ctrl+F", "进入文件模式：在当前输入前补一个 ' 触发符；已在文件模式时只提示，不叠第二个前缀"),
                                    ("?", "打开速查表：全部语法与快捷键都在那一页（`? 词` 过滤，Enter 看某一条的帮助页）"),
                                    ("D:\\proj\\ 、 \\\\srv\\share\\", "文件模式第一式：直接打路径浏览那一层（尾部分隔符＝列整个目录；不带则是「目录 + 过滤词」）"),
                                    ("' 报告", "第二式：在索引范围（桌面 / 下载 / 文档，四级以内，3 万条封顶）里按文件名搜"),
                                    ("open|find|file 报告", "第三式：动词触发（与 ' 同一结果）"),
                                    ("pic|doc|video|audio|zip|exe|cad 报告", "第四式：按类型筛扩展名（目录不参与类型筛选）"),
                                    ("folder 项目", "第五式：只找文件夹"),
                                    (" 报告（空格开头）", "第六式：空格开头进文件模式（Shift+空格 插入的那个真空格）"),
                                    ("文件行右键", "复制路径 / 在资源管理器中显示 / 添加为快捷项（文件行不在清单里 ⇒ 编辑、删除、置顶对它不适用）"),
                                    ("Ctrl+I", "批量导入全部"),
                                    ("Ctrl+,", "打开 / 返回设置页"),
                                ] {
                                    ui.label(egui::RichText::new(key).monospace());
                                    ui.label(desc);
                                    ui.end_row();
                                }
                            });
                        }
                        SettingsTab::Sources => {
                            // 批次 A10 的验收判据就长在这一页上：这里没有硬编任何源的名字，
                            // 列表直接取自 `self.sources`；`default_sources()` 里加一行，
                            // 这一页就自己多出一行。
                            ui.label(egui::RichText::new("启用哪些数据源").strong());
                            ui.label(
                                egui::RichText::new(
                                    "关掉的源既不搜索也不出提示行，下一次启动照样保持关闭（开关写在 settings.json 的 sources_off）。改动即时生效，不用重启。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // 先把要画的行取成纯数据：`self.sources` 要可变借给 init()，
                            // 而 prefs 只读，两处借用不能在同一个迭代器里混着用。
                            let rows: Vec<(source::SourceId, &'static str, &'static str, bool)> = self
                                .sources
                                .iter()
                                .map(|s| (s.id(), s.label(), s.note(), self.prefs.source_on(s.id())))
                                .collect();
                            for (id, label, note, mut on) in rows {
                                if ui.checkbox(&mut on, label).changed()
                                    && self.prefs.set_source(id, on)
                                {
                                    source::application::save_settings(&self.prefs);
                                    eprintln!(
                                        "设置页：数据源 {} → {}（已写 settings.json）",
                                        id.as_str(),
                                        if on { "开" } else { "关" }
                                    );
                                    if on {
                                        // 重新打开时补一次初始化：文件源要重建索引，
                                        // 否则要等下一个 30 分钟周期才搜得到。
                                        if let Some(s) =
                                            self.sources.iter_mut().find(|s| s.id() == id)
                                        {
                                            s.init();
                                        }
                                    }
                                    ui.ctx().request_repaint();
                                }
                                if !note.is_empty() {
                                    ui.label(egui::RichText::new(note).weak().small());
                                    ui.add_space(6.0);
                                }
                            }
                        }
                        SettingsTab::Appearance => {
                            // 批次 F8（FR-UIX-02/03/07/08）：窗口小节。宽度即时生效；记住位置、
                            // 状态栏、淡入三项下一次呼出/隐藏时才看得出差别。
                            ui.label(egui::RichText::new("窗口").strong());
                            let mut width = self.prefs.appearance.width;
                            let wr = ui.add(
                                egui::Slider::new(
                                    &mut width,
                                    source::application::WINDOW_WIDTH_MIN
                                        ..=source::application::WINDOW_WIDTH_MAX,
                                )
                                .step_by(10.0)
                                .fixed_decimals(0)
                                .text("宽度"),
                            );
                            if wr.changed() {
                                self.prefs.appearance.width =
                                    source::application::clamp_window_width(width);
                                self.apply_width(ui.ctx(), width, "设置页：窗口宽度");
                            }
                            // 拖滑块时每帧写盘会把 settings.json 的 mtime 刷成心跳；
                            // 松手（或键盘调完）才落一次盘。
                            if wr.drag_stopped()
                                || (wr.changed() && !ui.input(|i| i.pointer.primary_down()))
                            {
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：窗口宽度 = {}（已写 settings.json）",
                                    self.prefs.appearance.width.round()
                                );
                            }
                            let mut remember = self.prefs.appearance.remember_position;
                            if ui
                                .checkbox(&mut remember, "记住窗口位置（下次呼出回到原处）")
                                .changed()
                            {
                                self.prefs.appearance.remember_position = remember;
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：remember_position = {remember}（已写 settings.json）"
                                );
                            }
                            let mut bar = self.prefs.appearance.status_bar;
                            if ui
                                .checkbox(&mut bar, "显示底部状态栏（高 38px，仅有结果时出现）")
                                .changed()
                            {
                                self.prefs.appearance.status_bar = bar;
                                // 状态栏是叠加在"一屏十行"上的 44px：窗口跟着长/缩，
                                // 列表本身的行数不变（FR-UIX-08「仅在有结果时显示」）。
                                let h =
                                    window_height(&self.theme, self.prefs.row_density, bar);
                                self.resize_to_list(ui.ctx(), h, "切换状态栏");
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：status_bar = {bar}（窗口高度 → {h}，已写 settings.json）"
                                );
                            }
                            let mut fade = self.prefs.appearance.fade;
                            if ui
                                .checkbox(&mut fade, "呼出时淡入（120ms，cubic ease-out）")
                                .changed()
                            {
                                self.prefs.appearance.fade = fade;
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：fade = {fade}（已写 settings.json；下一次呼出生效）"
                                );
                            }
                            ui.label(
                                egui::RichText::new(
                                    "宽度范围 360–900（默认 500）。淡出未做：需求书原文要求影子窗口，而「先淡再隐藏」会违反同一条里的「隐藏不阻塞下一次热键唤起」。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.add_space(8.0);
                            ui.separator();
                            // 批次 F1（FR-UIX-01）：主题选择。改成即时生效——不重启、不丢焦点。
                            ui.label(egui::RichText::new("主题").strong());
                            ui.label(
                                egui::RichText::new(
                                    "内置 19 套 + 「跟随系统」。用户主题放程序目录的 Themes 文件夹（exe 同级；不可写时退到 %LOCALAPPDATA%\\egui-launcher\\Themes），文件名即主题名，同名覆盖内置。改主题不重启，隐藏期间改的文件在下次呼出时重读。",
                                )
                                .weak()
                                .small(),
                            );
                            // 用换行布局：500px 最小宽度下「下拉 + 两个按钮」放不进一行，
                            // 原先的 `horizontal` 会把「恢复 Light」裁在窗口右边缘外。
                            ui.horizontal_wrapped(|ui| {
                                let choices = self.theme_choices.clone();
                                egui::ComboBox::from_id_salt("theme_pick")
                                    .width(200.0)
                                    .selected_text(self.prefs.theme.clone())
                                    .show_ui(ui, |ui| {
                                        for name in choices {
                                            if ui
                                                .selectable_label(self.prefs.theme == name, name.as_str())
                                                .clicked()
                                                && self.prefs.theme != name
                                            {
                                                self.prefs.theme = name.clone();
                                                source::application::save_settings(&self.prefs);
                                                eprintln!(
                                                    "设置页：主题 → {name}（已写 settings.json）"
                                                );
                                                self.sync_theme(ui.ctx());
                                            }
                                        }
                                    });
                                if ui.button("打开主题文件夹").clicked() {
                                    let dir = theme::user_dir();
                                    match std::fs::create_dir_all(&dir) {
                                        Ok(()) => {
                                            let _ = std::process::Command::new("explorer.exe")
                                                .arg(&dir)
                                                .spawn();
                                            self.internal_toast(
                                                ui.ctx(),
                                                &format!("✓ 已打开 {}", dir.display()),
                                                true,
                                            );
                                        }
                                        Err(e) => self.internal_toast(
                                            ui.ctx(),
                                            &format!("⚠ 建不了主题文件夹：{e}"),
                                            false,
                                        ),
                                    }
                                }
                                if ui.button("恢复 Light").clicked()
                                    && self.prefs.theme != theme::FALLBACK_THEME
                                {
                                    self.prefs.theme = theme::FALLBACK_THEME.to_string();
                                    source::application::save_settings(&self.prefs);
                                    eprintln!("设置页：主题 → Light（已写 settings.json）");
                                    self.sync_theme(ui.ctx());
                                }
                            });
                            ui.add_space(8.0);
                            ui.separator();
                            // 生效值摘要：文件和内置到底解析出了什么，一眼能核对。
                            let t = &self.theme;
                            ui.label(egui::RichText::new("当前生效").strong());
                            ui.label(format!(
                                "{} ｜ 字体 {} ｜ 输入 {}pt 标题 {}pt 副标题 {}pt 快捷键 {}pt",
                                t.name,
                                t.font_name,
                                t.input_font_size,
                                t.title_font_size,
                                t.subtitle_font_size,
                                t.shortcut_font_size
                            ));
                            ui.label(format!(
                                "行高 {} ｜ 图标 {} ｜ 内边距 {} ｜ 选中圆角 {} ｜ 不透明度 {}（{}/255）",
                                t.row_height, t.icon_size, t.padding, t.selected_radius, t.opacity, t.opacity
                            ));
                            // 批次 F6：这两个数字只有在「跟随主题」档下才真的决定列表行，
                            // 不写这一句就会被读成"改了主题 JSON 行高却没变＝主题坏了"。
                            if self.prefs.row_density != RowDensity::Theme {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "（当前行密度是「{}」，行高与图标不取上面这两个数——要它们生效，把通用页的「列表行密度」切到「跟随主题」。内边距照旧生效。）",
                                        self.prefs.row_density.as_str()
                                    ))
                                    .weak()
                                    .small(),
                                );
                            }
                            ui.label(
                                egui::RichText::new(
                                    "颜色与其余尺寸请直接改主题 JSON（24 个键见需求书 6.2）；颜色写 RRGGBB，写错的键保留继承值并在日志里留一行。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(format!("主题目录：{}", theme::user_dir().display()))
                                    .weak()
                                    .small(),
                            );
                        }
                        SettingsTab::Advanced => {
                            // 批次 F7（FR-CFG-07）：重置学习排序。
                            ui.label(egui::RichText::new("重置学习排序").strong());
                            ui.label(
                                egui::RichText::new(
                                    "清空所有条目的「使用次数 / 最近使用时间」，排序回到无 Boost 状态；置顶（Ctrl+Shift+P）不受影响。落盘前会先备份 user_items.json。\n提交即学到的别名清不掉——它和你在「别名」里手填的词存在同一格，事后无法区分（需求书 FR-ACT-10 的已知偏差）。",
                                )
                                .weak()
                                .small(),
                            );
                            if ui.button("重置学习排序…").clicked() {
                                self.pending_destructive = Some(Destructive::ResetLearning);
                                eprintln!("设置页：点了「重置学习排序」，等确认");
                                ui.ctx().request_repaint();
                            }
                            ui.separator();
                            // 批次 F7（FR-CFG-08）：恢复默认。
                            ui.label(egui::RichText::new("恢复默认").strong());
                            ui.label(
                                egui::RichText::new(
                                    "把「空格执行 / Tab 语义 / 学习排序 / 数据源开关 / 危险操作确认 / 终端 shell / 主题 / 显示条数 / 行密度 / 缺省分类范围」恢复出厂值，并先写一份 settings.json 备份。\n保留：全局热键、补字符热键、网页引擎、隐藏的条目、出厂命令补种轮次，以及 user_items.json 里的自定义命令、置顶、学习统计与分类标签。",
                                )
                                .weak()
                                .small(),
                            );
                            if ui.button("恢复默认…").clicked() {
                                self.pending_destructive = Some(Destructive::RestoreDefaults);
                                eprintln!("设置页：点了「恢复默认」，等确认");
                                ui.ctx().request_repaint();
                            }
                            ui.separator();
                            // 批次 F7（D6 / FR-SRC-17）：隐藏的条目。
                            ui.label(egui::RichText::new("隐藏的条目").strong());
                            let rows = source::application::hidden_rows(
                                &self.prefs.hidden,
                                &self.prefs.hidden_stash,
                            );
                            if rows.is_empty() {
                                ui.label(
                                    egui::RichText::new(
                                        "没有隐藏的条目。在主界面把查询框清空、选中一条后按 Ctrl+Del 即可隐藏——出厂命令、扫描导入的条目、自定义条目都行。",
                                    )
                                    .weak()
                                    .small(),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new(
                                        "隐藏不是删除：条目不进列表，重新扫描 /「补回缺失的出厂命令」都不会把它塞回来。恢复后置顶与学习计数一起回来。",
                                    )
                                    .weak()
                                    .small(),
                                );
                                let mut restore_uid: Option<String> = None;
                                let mut restore_all = false;
                                for row in &rows {
                                    ui.horizontal(|ui| match &row.name {
                                        Some(name) => {
                                            ui.label(name.as_str());
                                            if ui.small_button("恢复").clicked() {
                                                restore_uid = Some(row.uid.clone());
                                            }
                                        }
                                        None => {
                                            // 既没有隐藏快照、也对不上出厂种子：目前只有启动时
                                            // 自动去重写进去的 Store 版「设置」会落到这一格。
                                            ui.label(
                                                egui::RichText::new("（程序自动隐藏，不需要恢复）")
                                                    .weak()
                                                    .small(),
                                            )
                                            .on_hover_text(row.uid.as_str());
                                        }
                                    });
                                }
                                let restorable =
                                    rows.iter().filter(|r| r.name.is_some()).count();
                                if restorable > 1 && ui.button("全部恢复").clicked() {
                                    restore_all = true;
                                }
                                if let Some(uid) = restore_uid {
                                    let name = rows
                                        .iter()
                                        .find(|r| r.uid == uid)
                                        .and_then(|r| r.name.clone())
                                        .unwrap_or_default();
                                    if source::application::restore_hidden(
                                        &mut self.items,
                                        &mut self.prefs.hidden,
                                        &mut self.prefs.hidden_stash,
                                        &uid,
                                    ) {
                                        self.after_unhide(ui.ctx(), &uid);
                                        self.toast = Some((
                                            format!("✓ 已恢复「{name}」"),
                                            Instant::now() + TOAST_TTL,
                                            TOAST_OK,
                                        ));
                                        ui.ctx().request_repaint_after(TOAST_TTL);
                                    }
                                }
                                if restore_all {
                                    let n = source::application::restore_all_hidden(
                                        &mut self.items,
                                        &mut self.prefs.hidden,
                                        &mut self.prefs.hidden_stash,
                                    );
                                    if n > 0 {
                                        self.save_items();
                                        source::application::save_settings(&self.prefs);
                                        self.refresh_path_states(true);
                                        self.toast = Some((
                                            format!("✓ 已恢复 {n} 条隐藏条目"),
                                            Instant::now() + TOAST_TTL,
                                            TOAST_OK,
                                        ));
                                        eprintln!(
                                            "设置页：全部恢复隐藏条目 → {n} 条（孤儿 uid 保留）"
                                        );
                                        ui.ctx().request_repaint_after(TOAST_TTL);
                                    }
                                }
                            }
                        }
                    });
                }
            );
        });
    }

    /// 窗口显示时：对【本机窗口】的默认 IME 保存状态并切到英文模式。
    /// 与 AHK 参考项目 `_SwitchImeToEnglish` 同构：用 ImmGetDefaultIMEWnd(own_hwnd)，
    /// 而非前台窗口——per-window 输入法开启时只影响本程序，不误改别的程序。
    fn on_show(&mut self) {
        // FR-WIN-04：呼出后重置失焦关闭状态，等真正拿到焦点再武装定时器
        self.hide_at = None;
        self.had_focus_since_show = false;
        self.toast = None; // 陈旧提示不跨呼出周期存活
        // 批次 F8（FR-UIX-07）：淡入从这一刻起算。没开淡入时这个值也没人读。
        self.shown_at = Some(Instant::now());
        // 呼出后把键盘焦点给搜索框（下一帧渲染 TextEdit 时 request_focus）
        self.focus_query = true;
        self.selected = 0;
        // FR-SRC-06：每次呼出强制重算一轮（刚插上的 U 盘 / 刚恢复的网络盘要立刻反映），
        // 放在 hwnd 早退之前——没有窗口句柄时四态也该刷新。
        self.refresh_path_states(true);
        let Some(hwnd) = self.self_hwnd else { return };
        let ime_wnd = unsafe { ImmGetDefaultIMEWnd(hwnd) };
        if ime_wnd.0.is_null() {
            return;
        }
        let before = get_ime_state(ime_wnd);
        if self.ime_before.is_none() {
            self.ime_before = Some(before);
        }
        switch_ime_to_english(ime_wnd, before);
        let after = get_ime_state(ime_wnd);
        eprintln!(
            "IME→English: before open={} conv=0x{:X} | after open={} conv=0x{:X}",
            before.open, before.conv, after.open, after.conv
        );
    }

    /// 窗口隐藏时：恢复 IME 状态。与 AHK `_RestoreImeMode` 同构——
    /// 恢复到隐藏时的前台窗口（此时焦点可能已回到别的程序），共用输入法时才需要。
    fn on_hide(&mut self) {
        // FR-WIN-04：关闭到托盘时清掉待触发的失焦定时器
        self.hide_at = None;
        self.had_focus_since_show = false;
        // 批次 F8（FR-UIX-07）：这一轮显示结束，淡入进度清零；下次 on_show 重新起算。
        self.shown_at = None;
        // 批次 F8（FR-UIX-03 RememberPosition）：拖窗走的是 `SetWindowPos`，隐藏这一刻
        // `GetWindowRect` 读到的就是终值。与上次存的不同才写盘——每次隐藏都写会把
        // settings.json 的 mtime 刷得像心跳。
        if self.prefs.appearance.remember_position {
            if let Some(pos) = self.self_hwnd.and_then(win_origin) {
                let pos = [pos.0, pos.1];
                if self.prefs.appearance.last_pos != Some(pos) {
                    self.prefs.appearance.last_pos = Some(pos);
                    source::application::save_settings(&self.prefs);
                    eprintln!("窗口：记住位置 ({}, {})", pos[0], pos[1]);
                }
            }
        }
        // 批次 F6：☰ 菜单不跨隐藏保留。失焦自动隐藏时 Popup 那一帧不再运行，也就没人把
        // `menu_open` 写回 false——不在这里清掉，下次呼出会带着一个悬着的菜单出现。
        self.menu_open = false;
        // 批次 R：窗口一隐藏，eframe 就不再跑 `ui()` ⇒ `hotkey_frame` 末尾那条边沿检测等不到
        // 下一帧。取键态若还开着，全局呼出会一直停在"临时注销"状态（按了没反应），所以在这里
        // 就地退出取键并把键装回。放在这个函数最前面是因为它下面有一条 `return`。
        if self.hotkey_recording || self.typeback_armed {
            self.hotkey_recording = false;
            self.typeback_armed = false;
            eprintln!("设置页：窗口隐藏 ⇒ 退出取键（录制/选键状态不跨隐藏保留）");
            self.sync_keypick_suspend();
        }
        let Some(before) = self.ime_before.take() else {
            return;
        };
        if let Some(ime_wnd) = foreground_ime_wnd() {
            restore_ime(ime_wnd, before);
            let after = get_ime_state(ime_wnd);
            eprintln!(
                "IME restore: before open={} conv=0x{:X} | after open={} conv=0x{:X}",
                before.open, before.conv, after.open, after.conv
            );
        }
    }

    /// 运行程序后关闭到托盘（FR-ACT + 用户要求：Enter/点击运行后窗口自动关）。
    /// 与失焦关闭走同一条路径：on_hide 恢复输入法 + Visible(false)，进程仍常驻托盘。
    /// 启动一律走 ShellExecuteExW（FR-ACT-02）。arguments 目前按整串传 lpParameters。
    /// 返回是否真正启动成功——FR-SRC-19 的学习统计只在成功时累计。
    fn launch_and_hide(&mut self, ctx: &egui::Context, entry: &LaunchItem) -> bool {
        self.launch_with_log(ctx, entry, None)
    }

    /// 批次 U（FR-SRC-05）：自定义命令的启动。`{query}` 填好之后 path/arguments 里带着
    /// 用户敲的参数 ⇒ **绝不入日志**（批次 P 口径），只记清单侧身份（name + uid）与成败。
    fn launch_and_hide_masked(
        &mut self,
        ctx: &egui::Context,
        entry: &LaunchItem,
        uid: &str,
    ) -> bool {
        self.launch_with_log(ctx, entry, Some(uid))
    }

    fn launch_with_log(
        &mut self,
        ctx: &egui::Context,
        entry: &LaunchItem,
        masked_uid: Option<&str>,
    ) -> bool {
        // 批次 A2（FR-SRC-17）：内置动作（`internal:<id>`）不归 ShellExecute 管——
        // 它既不是协议也不是程序。危险动作的确认也在 `run_internal` 里拦：
        // 那里返回 false ＝ 这一帧还没执行，调用方据此不记学习统计，
        // 等确认弹窗按「执行」那一帧才补记。
        if entry.kind == ItemKind::Internal {
            return self.run_internal(ctx, entry);
        }
        match shell_execute(&entry.path, &entry.arguments, entry.working_dir.as_deref()) {
            Ok(_) => match masked_uid {
                None => eprintln!("launch: {} {}", entry.path, entry.arguments),
                Some(uid) => eprintln!(
                    "launch(custom): 已启动「{}」（{uid}，填入的路径/参数含查询词，不落日志）",
                    entry.name
                ),
            },
            Err(e) => {
                // M1 验收：启动失败不崩溃、不隐藏——窗口留在原地显示 Toast，TOAST_TTL 后自动消失。
                match masked_uid {
                    None => eprintln!("launch failed: {}: {e}", entry.path),
                    Some(uid) => eprintln!("launch(custom) failed: 「{}」{uid}: {e}", entry.name),
                }
                self.toast = Some((
                    format!("⚠ 启动失败：{}（{e}）", entry.name),
                    Instant::now() + TOAST_TTL,
                    TOAST_ERR,
                ));
                ctx.request_repaint_after(TOAST_TTL);
                return false;
            }
        }
        self.collapse_after_run(ctx);
        true
    }

    /// 执行完把窗口收回托盘（FR-ACT「运行后自动关」）。启动成功与内置动作共用一条路径，
    /// 免得两处各写一遍、将来只改一处。
    fn collapse_after_run(&mut self, ctx: &egui::Context) {
        self.toast = None;
        self.clear_query_next_frame = true;
        self.selected = 0;
        self.visible = false;
        self.on_hide();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    /// 批次 A2（FR-SRC-17）内置动作的入口：解析路径 → 危险动作先弹确认 → 交给 [`Self::exec_internal`]。
    /// 返回 `false` ＝ 这一帧什么都没做（路径不认识 / 正在等确认），调用方据此不记学习统计。
    fn run_internal(&mut self, ctx: &egui::Context, entry: &LaunchItem) -> bool {
        let Some(action) = source::system::InternalAction::from_path(&entry.path) else {
            eprintln!("internal: 不认识的系统命令路径 {}", entry.path);
            self.internal_toast(ctx, &format!("⚠ 这条系统命令不认识：{}", entry.path), false);
            return false;
        };
        if action.needs_confirm() && self.prefs.confirm_actions {
            eprintln!("internal: 「{}」执行前确认（{}）", entry.name, action.id());
            self.internal_confirm = Some(InternalConfirm {
                action,
                name: entry.name.clone(),
                uid: entry.uid(),
            });
            ctx.request_repaint();
            return false;
        }
        self.exec_internal(ctx, action)
    }

    /// 真正执行一个内置动作。返回 `false` ＝ 没做成（窗口留在原地，已给 Toast）。
    fn exec_internal(
        &mut self,
        ctx: &egui::Context,
        action: source::system::InternalAction,
    ) -> bool {
        use source::system::InternalAction as A;
        match action {
            A::Settings(page) => {
                self.settings_open = true;
                self.settings_tab =
                    SettingsTab::from_page(u32::from(page)).unwrap_or(SettingsTab::General);
                self.clear_query_next_frame = true;
                self.focus_query = false;
                eprintln!("internal: 打开设置页 {page}");
                ctx.request_repaint();
                true
            }
            A::Reload => {
                self.reload_config(ctx);
                true
            }
            A::RebuildAppIndex => {
                source::application::spawn_batch_import(self.apps_tx.clone());
                eprintln!("internal: 重建应用索引（后台扫描开始菜单/桌面）");
                self.internal_toast(ctx, "✓ 应用索引已在后台重建，完成后自动合并", true);
                true
            }
            A::RebuildFileIndex => {
                if !self.prefs.source_on(source::SourceId::Files) {
                    eprintln!("internal: 文件搜索源已关闭，拒绝重建索引");
                    self.internal_toast(ctx, "⚠ 文件搜索源已关闭，没有可重建的索引", false);
                    return false;
                }
                let mut hit = false;
                for s in self.sources.iter_mut() {
                    if s.id() == source::SourceId::Files {
                        s.rebuild();
                        hit = true;
                    }
                }
                eprintln!("internal: 文件索引重建请求已派发（找到文件源={hit}）");
                self.internal_toast(ctx, "✓ 文件索引已在后台重建", hit);
                hit
            }
            A::About => {
                self.about_open = true;
                eprintln!("internal: 打开「关于本程序」");
                ctx.request_repaint();
                true
            }
            A::OpenLog => {
                open_log_file();
                self.collapse_after_run(ctx);
                true
            }
            A::Quit => {
                eprintln!("internal: 退出启动器");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                true
            }
            A::Lock => match unsafe { LockWorkStation() } {
                Ok(()) => {
                    eprintln!("internal: 已锁定屏幕");
                    self.collapse_after_run(ctx);
                    true
                }
                Err(e) => {
                    eprintln!("internal: 锁定屏幕失败：{e}");
                    self.internal_toast(ctx, &format!("⚠ 锁定屏幕失败：{e}"), false);
                    false
                }
            },
            // 睡眠走 powrprof 的 SetSuspendState（`0,1,0` = 不休眠）——本机开了休眠时
            // Windows 可能按休眠处理，这是系统行为不是本程序的；休眠用 `shutdown /h`。
            A::Sleep => self.run_console_action(
                ctx,
                "睡眠",
                "rundll32.exe",
                &["powrprof.dll,SetSuspendState", "0,1,0"],
            ),
            A::Hibernate => self.run_console_action(ctx, "休眠", "shutdown.exe", &["/h"]),
            A::Shutdown => self.run_console_action(ctx, "关机", "shutdown.exe", &["/s", "/t", "0"]),
            A::Restart => self.run_console_action(ctx, "重启", "shutdown.exe", &["/r", "/t", "0"]),
            A::Logoff => self.run_console_action(ctx, "注销", "shutdown.exe", &["/l"]),
            A::EmptyRecycleBin => {
                let flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND;
                match unsafe { SHEmptyRecycleBinW(None, PCWSTR::null(), flags) } {
                    Ok(()) => {
                        eprintln!("internal: 回收站已清空");
                        self.collapse_after_run(ctx);
                        true
                    }
                    Err(e) => {
                        // 回收站本来就空时也可能回一个失败码；照实说，不谎报成功。
                        eprintln!("internal: 清空回收站失败：{e}");
                        self.internal_toast(ctx, &format!("⚠ 清空回收站失败：{e}"), false);
                        false
                    }
                }
            }
            A::MonitorOff => {
                let r = unsafe {
                    SendMessageTimeoutW(
                        HWND_BROADCAST,
                        WM_SYSCOMMAND,
                        WPARAM(SC_MONITORPOWER as usize),
                        LPARAM(2),
                        SMTO_ABORTIFHUNG,
                        1000,
                        None,
                    )
                };
                eprintln!(
                    "internal: 已请求关闭显示器（SendMessageTimeout 返回 {}）",
                    r.0
                );
                self.collapse_after_run(ctx);
                true
            }
            A::VolumeMute => self.send_media_key(ctx, VK_VOLUME_MUTE, "静音"),
            A::VolumeUp => self.send_media_key(ctx, VK_VOLUME_UP, "音量加"),
            A::VolumeDown => self.send_media_key(ctx, VK_VOLUME_DOWN, "音量减"),
            A::MediaPlayPause => self.send_media_key(ctx, VK_MEDIA_PLAY_PAUSE, "播放/暂停"),
            A::MediaNext => self.send_media_key(ctx, VK_MEDIA_NEXT_TRACK, "下一曲"),
            A::MediaPrev => self.send_media_key(ctx, VK_MEDIA_PREV_TRACK, "上一曲"),
            A::MediaStop => self.send_media_key(ctx, VK_MEDIA_STOP, "停止播放"),
        }
    }

    /// 音量/媒体键：走 `SendInput` 的虚拟键注入（与补字符那格同一个 API，但这里是真 VK）。
    fn send_media_key(&mut self, ctx: &egui::Context, vk: VIRTUAL_KEY, what: &str) -> bool {
        press_vk(vk.0);
        eprintln!("internal: 已发送「{what}」键（vk 0x{:02X}）", vk.0);
        self.collapse_after_run(ctx);
        true
    }

    /// 派发一条控制台动作（`shutdown.exe`）。`CREATE_NO_WINDOW` 保证不闪黑窗。
    fn run_console_action(
        &mut self,
        ctx: &egui::Context,
        what: &str,
        exe: &str,
        args: &[&str],
    ) -> bool {
        match run_console_tool(exe, args) {
            Ok(()) => {
                eprintln!("internal: {what} 已派发（{exe}）");
                self.collapse_after_run(ctx);
                true
            }
            Err(e) => {
                eprintln!("internal: {what} 失败：{e}");
                self.internal_toast(ctx, &format!("⚠ {what}失败：{e}"), false);
                false
            }
        }
    }

    fn internal_toast(&mut self, ctx: &egui::Context, text: &str, ok: bool) {
        self.toast = Some((
            text.to_string(),
            Instant::now() + TOAST_TTL,
            if ok { TOAST_OK } else { TOAST_ERR },
        ));
        ctx.request_repaint_after(TOAST_TTL);
    }

    /// FR-SRC-17「重新加载配置」：把 `settings.json` 与 `user_items.json` 重新读一遍。
    /// 这是手改配置文件后的正规入口——解析失败时**沿用当前状态**，绝不拿默认值覆盖
    /// （那会把用户手写坏一个逗号的清单变成空清单，形状与 E1 事故相同）。
    fn reload_config(&mut self, ctx: &egui::Context) {
        let off_before: Vec<source::SourceId> = self
            .sources
            .iter()
            .filter(|s| !self.prefs.source_on(s.id()))
            .map(|s| s.id())
            .collect();
        let hotkey_before = self.hotkey_active;
        let (prefs, bad_prefs) = source::application::load_settings();
        let (items, bad_items) = source::application::load_user_items();

        let mut note: Vec<String> = Vec::new();
        if bad_prefs.is_some() {
            note.push("settings.json 解析失败，沿用当前设置".to_string());
        } else {
            self.prefs = prefs;
            if self.prefs.toggle_hotkey != hotkey_before {
                self.request_hotkey(self.prefs.toggle_hotkey);
                note.push("呼出热键已按配置重装".to_string());
            }
            // 之前关着、现在打开的源要补一次初始化（文件源要重建索引，参照设置页开关）。
            for id in off_before
                .iter()
                .copied()
                .filter(|id| self.prefs.source_on(*id))
            {
                if let Some(s) = self.sources.iter_mut().find(|s| s.id() == id) {
                    s.init();
                    note.push(format!("数据源 {} 已重新初始化", id.as_str()));
                }
            }
        }

        if bad_items.is_some() {
            note.push("user_items.json 解析失败，沿用当前清单".to_string());
        } else {
            self.items = dedup_items(items);
            let dropped = source::application::drop_hidden(&mut self.items, &self.prefs.hidden);
            self.selected = 0;
            let need: Vec<String> = self
                .items
                .iter()
                .filter(|i| {
                    !i.source_path.is_empty() && !self.icons.contains_key(i.source_path.as_str())
                })
                .map(|i| i.source_path.clone())
                .collect();
            if !need.is_empty() {
                spawn_icon_loader(self.icon_tx.clone(), need);
            }
            eprintln!(
                "internal: 重新加载配置 → 清单 {} 条（hidden 剔掉 {dropped} 条）",
                self.items.len()
            );
        }

        let text = if note.is_empty() {
            format!("✓ 已重新加载配置（清单 {} 条）", self.items.len())
        } else {
            format!("⚠ 重新加载：{}", note.join("；"))
        };
        self.internal_toast(ctx, &text, note.is_empty());
    }

    /// 补全提示行不可执行（需求书把空词这一格定为 `valid=false`）：Enter/点击/数字键
    /// 落到它身上时统一给一句提示。文案由调用方给——批次 X 起 `HeadRow::Hint` 多了一格
    /// 「文件索引建立中」，那句"继续输入关键词后再回车"对它没有意义。
    fn hint_toast(&mut self, ctx: &egui::Context, msg: &str) {
        self.toast = Some((msg.to_string(), Instant::now() + TOAST_TTL, TOAST_OK));
        ctx.request_repaint_after(TOAST_TTL);
    }

    /// 批次 T：搜索行的「执行」＝用默认浏览器打开 URL，然后关到托盘。
    /// **刻意不走 `launch_and_hide`**：那条路把 `entry.arguments` 原样写日志，而这里的 URL
    /// 含用户敲的查询词（`?q=…`）——批次 P 定的口径是「绝不记查询框原文」。所以日志只记引擎 id，
    /// URL 本身也不记（它同样带着查询词）。
    fn open_search_hit(&mut self, ctx: &egui::Context, hit: &source::websearch::SearchHit) -> bool {
        match shell_execute(&hit.url, "", None) {
            Ok(_) => eprintln!(
                "websearch: 已用默认浏览器打开（引擎 {}，查询原文不落日志）",
                hit.engine_id
            ),
            Err(e) => {
                eprintln!("websearch: 打开失败（引擎 {}）：{e}", hit.engine_id);
                self.toast = Some((
                    format!("⚠ 打开失败：{e}"),
                    Instant::now() + TOAST_TTL,
                    TOAST_ERR,
                ));
                ctx.request_repaint_after(TOAST_TTL);
                return false;
            }
        }
        self.toast = None;
        self.clear_query_next_frame = true;
        self.selected = 0;
        self.visible = false;
        self.on_hide();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        true
    }

    /// 批次 X：文件行的「执行」＝用系统关联程序打开这个文件/目录，然后关到托盘。
    /// 与 [`open_search_hit`](LauncherApp::open_search_hit) 同一条理由**不走 `launch_and_hide`**：
    /// 那条路把 `path` 原样写日志，而这里的每一条路径都是用户从搜索框里现搜出来的——
    /// 可能是他从下载夹里翻出来的一个文件名。FR-SYS-03 的口径是「清单内条目才记 name」，
    /// 文件源不在清单里 ⇒ 只记动作与类型，路径与查询原文都不落日志。
    fn open_file_hit(&mut self, ctx: &egui::Context, hit: &source::filesearch::FileHit) -> bool {
        match shell_execute(&hit.path, "", None) {
            Ok(_) => eprintln!(
                "文件搜索：已用系统关联程序打开{}（路径由查询现出，不落日志）",
                if hit.is_dir { "目录" } else { "文件" }
            ),
            Err(e) => {
                eprintln!("文件搜索：打开失败：{e}");
                self.toast = Some((
                    format!("⚠ 打开失败：{e}"),
                    Instant::now() + TOAST_TTL,
                    TOAST_ERR,
                ));
                ctx.request_repaint_after(TOAST_TTL);
                return false;
            }
        }
        self.toast = None;
        self.clear_query_next_frame = true;
        self.selected = 0;
        self.visible = false;
        self.on_hide();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        true
    }

    /// 批次 A3（FR-SRC-16）：终端行的「执行」＝开一个新控制台窗口跑这条命令，随后关到托盘。
    /// **命令原文不进日志**：它就是用户敲进查询框的内容（FR-SYS-03 隐私口径），
    /// 日志只记 shell 名；失败时 Toast 也只报系统错误码，不回显命令。
    fn run_terminal_hit(&mut self, ctx: &egui::Context, hit: &source::terminal::TermHit) -> bool {
        // 命令原文不落日志，但**成败都要落一行**：这是唯一能事后看出「敲了 > 没反应」的线索。
        // 成功那句在 `spawn` 之后打，免得"已启动"和"启动失败"前后脚同时出现。
        if let Err(e) = hit.shell.spawn(&hit.command) {
            eprintln!("terminal: 启动失败（shell {}）：{e}", hit.shell.as_str());
            self.toast = Some((
                format!("⚠ 启动失败：{e}"),
                Instant::now() + TOAST_TTL,
                TOAST_ERR,
            ));
            ctx.request_repaint_after(TOAST_TTL);
            return false;
        }
        eprintln!(
            "terminal: 已启动（shell {}，命令原文不落日志）",
            hit.shell.as_str()
        );
        self.toast = None;
        self.clear_query_next_frame = true;
        self.selected = 0;
        self.visible = false;
        self.on_hide();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        true
    }

    /// 批次 X：在资源管理器里打开所在目录并选中这个文件/目录，随后关到托盘。
    /// 临时条目不进清单、不记学习统计（`launch_from_list` 为假），与清单行 Ctrl+D 那一条同形态；
    /// 日志走脱敏路径：这条路径同样来自查询。
    fn select_in_explorer(&mut self, ctx: &egui::Context, path: &str) {
        let entry = LaunchItem {
            name: "所在目录".to_string(),
            path: "explorer.exe".to_string(),
            arguments: format!("/select,\"{path}\""),
            working_dir: None,
            source_path: String::new(),
            kind: ItemKind::Command,
            keyword: String::new(),
            ..Default::default()
        };
        self.launch_and_hide_masked(ctx, &entry, "FR-SRC-14 文件行");
    }

    /// 把当前整张清单写盘（种子播种一次后即普通用户项，不再剔除）。
    fn save_items(&self) {
        source::application::save_user_items(&self.items);
        eprintln!(
            "source: 已写清单 user_items.json（{} 条）",
            self.items.len()
        );
    }

    /// FR-SRC-19 学习位序数据源：启动成功后累计次数 +1、刷新最近使用时间并落盘。
    /// 身份用派生 `uid`（见 §4.1.2）——同 exe 不同参数的两条目各算各的；
    /// 临时条目（如 Ctrl+D 的 explorer /select）不在清单里，自然匹配不到、跳过。
    /// `alias_word` 非空时顺带「提交即学别名」，与统计合并成同一次写盘。
    fn mark_used(&mut self, uid: &str, alias_word: &str) {
        let Some(pos) = self.items.iter().position(|i| i.uid() == uid) else {
            return;
        };
        self.items[pos].use_count = self.items[pos].use_count.saturating_add(1);
        self.items[pos].last_used_ms = now_ms();
        if !alias_word.is_empty() && learn_alias(&mut self.items[pos], alias_word) {
            // FR-SYS-03 隐私口径：`alias_word` 是查询框里敲的原文，绝不落日志。批次 P 之后
            // 这个文件能存好几天、还能托盘一键打开，最容易被随手发给别人看问题 ⇒ 只记清单侧的落点。
            eprintln!(
                "提交即学别名：已写入「{}」的别名字段 ⇒ {}（不记用户敲的词）",
                self.items[pos].name, self.items[pos].keyword
            );
        }
        self.save_items();
    }

    /// FR-SRC-19 编辑保统计：按派生 `uid` 把 `old` 里的 pinned/use_count/last_used_ms
    /// 回填进当前清单——批量导入 dedup 后学习数据不丢。
    /// 注意口径：uid 含命令行，所以「编辑时把参数也改了」这里必然匹配不上，
    /// 那种情况由 Confirm 分支按 edit_uid 直接携带旧统计解决，不靠本函数。
    fn carry_stats(&mut self, old: &[LaunchItem]) {
        let stats: std::collections::HashMap<String, (bool, u32, i64)> = old
            .iter()
            .map(|i| (i.uid(), (i.pinned, i.use_count, i.last_used_ms)))
            .collect();
        for it in self.items.iter_mut() {
            if let Some((pinned, use_count, last_used_ms)) = stats.get(&it.uid()) {
                it.pinned = *pinned;
                it.use_count = *use_count;
                it.last_used_ms = *last_used_ms;
            }
        }
    }

    /// 批次 F7：从「高级」页恢复一条隐藏条目之后的收尾——两份落盘、补取图标、刷新路径态。
    /// 恢复出来的条目来自隐藏快照（老记录没有快照时才按出厂种子重建），
    /// 图标纹理与路径四态都还没有缓存。
    fn after_unhide(&mut self, ctx: &egui::Context, uid: &str) {
        self.save_items();
        source::application::save_settings(&self.prefs);
        let icon_src = self
            .items
            .iter()
            .find(|i| i.uid() == uid)
            .map(|i| i.source_path.clone())
            .filter(|p| !p.is_empty())
            .filter(|p| !self.icons.contains_key(p.as_str()));
        if let Some(p) = icon_src {
            spawn_icon_loader(self.icon_tx.clone(), vec![p]);
        }
        self.refresh_path_states(true);
        ctx.request_repaint();
    }
}

impl Default for LauncherApp {
    fn default() -> Self {
        let (_t1, r1) = mpsc::channel();
        let (_t2, r2) = mpsc::channel();
        let (_t3, r3) = mpsc::channel();
        let (t4, _r4) = mpsc::channel();
        let (_t5, r5) = mpsc::channel();
        Self::new(
            r1,
            r2,
            r3,
            t4,
            r5,
            None,
            StartupIntent {
                visible: true,
                ..Default::default()
            },
            // 测试/兜底用的 `Default` 不读 `Themes\`：给基准 Light 就够了，
            // 真启动路径由 `main` 解析主题再传进来。
            theme::Theme::light(),
            Vec::new(),
        )
    }
}

impl LauncherApp {
    /// 本帧列表的完整计划（头段 + 应用结果）。批次 A10 起只剩"组上下文 → 交给注册表"。
    ///
    /// 次序全部由 [`source::default_sources`] 的注册顺序决定：自定义命令 → 网页引擎 → 文件 →
    /// 计算器 → 应用清单。几个判据原样保留在各自的源里：
    /// - 独占只在首词后出现过空格时成立（`has_rest`）——`g` 这个单字还可能是 `git` 的开头；
    /// - 用户自定义命令**先于**内置网页引擎（用户数据压过出厂数据）；
    /// - 文件模式排在网页引擎之后、普通合并之前（整张列表都是文件的独占）；
    /// - `kw` 精确等于某条需要参数的命令关键字时，头段给提示行并把同一条目从结果里摘掉；
    /// - 一条结果都没有时才问兜底（FR-QRY-08）。
    ///
    /// 关掉的源在 [`source::dispatch`] 里**连被问到的机会都没有**，不会出结果、也不会出提示行。
    fn plan(&mut self) -> Plan {
        let now = now_ms();
        // 先取值再借 `prefs.engines`：配置里存的是稳定名，认识的才留下，不认识回落 cmd。
        let term_shell = self.prefs.term_shell();
        // 速查表要按「哪些源开着」过滤条目，dispatch 也要同一份判断 ⇒ 只算一个闭包，两处共用。
        let on = |id: source::SourceId| self.prefs.source_on(id);
        // 批次 G1：`@分类` 前缀在进查询引擎前剥掉——匹配、Tab 补全、学习别名看到的都是
        // 剥剩的真查询，标签本身不会被 learn_alias 学走（它吃的是 cx.query）。
        let scope_q = source::application::parse_scope_query(&self.query);
        let bare_at = self.query.starts_with('@')
            && scope_q.tags.is_empty()
            && scope_q.query.trim().is_empty();
        let mut cx = source::SearchCtx {
            query: scope_q.query,
            items: &self.items,
            engines: &self.prefs.web_engines,
            learn_rank: self.prefs.learn_rank,
            index: &mut self.index,
            now_ms: now,
            term_shell,
            source_on: &on,
        };
        let mut plan = source::dispatch(&mut self.sources, &mut cx, &on);
        // 批次 G1 候选池：只筛清单行（results），头段（计算器/文件/网页/…）与 `>`/`?` 不受影响。
        // 显式 @ = 严格成员并集；没有显式 @ 时按缺省范围（basic = 无标签 ∪ 基本）。
        let scope = self.prefs.default_scope;
        plan.results
            .retain(|&i| source::application::item_in_scope(&self.items[i], &scope_q.tags, scope));
        if bare_at {
            let hint = source::application::ScopeTagHint::from_counts(
                &source::application::all_tags(&self.items),
            );
            plan.head.insert(0, source::SourceRow::ScopeTags(hint));
        }
        // 2026-10-24：「10 条」档在这里截断，而不是在渲染时截——显示、↑↓、PgUp/PgDn、
        // Ctrl+J/K、Tab、数字键、空格与 Enter 都读同一份 `plan()`，截在这里它们的落点才不会打架。
        if self.prefs.list_rows == ListRows::Ten {
            cap_plan_rows(&mut plan, LIST_ROWS);
        }
        plan
    }
}

impl eframe::App for LauncherApp {
    /// 批次 F1（FR-UIX-06）：清屏必须全透明。否则中央框圆角以外那一圈会被清成
    /// egui 主题的底色（不透明），`with_transparent(true)` 就只透了个寂寞。
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // eframe 为了保证首帧不出白闪，会在首帧渲染后无条件 set_visible(true)，
        // 把 with_visible(false) 覆盖掉；静默启动（-Startup）在第一帧末显式盖回隐藏。
        if self.hide_pending {
            self.hide_pending = false;
            if !self.visible {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        // FR-SRC-06：收取后台路径检查结果（整体替换缓存：一次扫描覆盖整张清单）。
        while let Ok(states) = self.path_rx.try_recv() {
            self.path_in_flight = false;
            self.path_states = states.into_iter().collect();
            ui.ctx().request_repaint();
        }
        // 窗口可见期间的兜底刷新（最小间隔见 PATH_RESCAN_INTERVAL）：用户可能开着框去插 U 盘。
        self.refresh_path_states(false);

        // 批次 A10：各数据源收自己的后台回报（文件源的索引整表替换 / 一次目录浏览，以及
        // 30 分钟重扫的兜底都在 `FileSource::poll` 里）。有变化才重绘，没变化不打扰。
        // 关掉的源连 poll 都不调：`FileSource::poll` 顺手会派下一轮索引，放进来等于开关只关了
        // 搜索、后台仍在读盘（2026-10-22 真机：`files` 关着启动，日志仍出现「本轮建成 604 条」）。
        for s in self.sources.iter_mut() {
            if self.prefs.source_on(s.id()) && s.poll() {
                ui.ctx().request_repaint();
            }
        }
        // E1：配置坏过一次的那句说明，等窗口真被看见才说（Toast 会被 `on_show` 清空，
        // 所以它必须在 `hotkey_frame` 之后、渲染之前入队）。
        if self.visible {
            if let Some(msg) = self.startup_notice.take() {
                self.toast = Some((msg, Instant::now() + TOAST_TTL, TOAST_ERR));
                ui.ctx().request_repaint_after(TOAST_TTL);
            }
        }

        // E-06 托盘事件
        while let Ok(event) = self.tray_rx.try_recv() {
            match event {
                TrayEvent::ShowWindow => {
                    self.show_now(ui.ctx());
                }
                TrayEvent::BatchImport => {
                    // 显式「批量导入全部」：后台扫 .lnk，结果经 apps_rx 回报后合并落清单。
                    source::application::spawn_batch_import(self.apps_tx.clone());
                    eprintln!("托盘：批量导入全部 已触发（后台扫描开始菜单/桌面）");
                }
                TrayEvent::Settings => {
                    self.show_now(ui.ctx());
                    self.settings_open = true;
                }
                TrayEvent::OpenLog => {
                    open_log_file();
                }
                TrayEvent::Exit => {
                    self.on_hide();
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        // FR-WIN-05 全局热键 toggle（Ctrl+Alt+Space）
        while self.hotkey_rx.try_recv().is_ok() {
            // 隐藏那一支的条件是「旗标说显示着 **且** 系统那边也显示着」：只问旗标时，`Win+D`
            // 之后第一按会走成"再隐藏一次"（用户看到的就是按了没反应，2026-10-22 真机③）。
            if self.visible && !self.is_iconic() {
                eprintln!("FR-WIN-05: 热键切换 → 隐藏");
                self.visible = false;
                self.on_hide();
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                eprintln!("FR-WIN-05: 热键切换 → 呼出");
                self.show_now(ui.ctx());
            }
        }

        // FR-WIN-08：启动时带多个 SendTo 路径 → 首帧批量入库（此时已有 ctx 可刷新）。
        if !self.pending_batch.is_empty() {
            let paths = std::mem::take(&mut self.pending_batch);
            self.add_batch(ui.ctx(), paths);
        }

        // FR-SRC-21 / FR-WIN-08 IPC 接管：Activate=呼出聚焦；AddPath=呼出并排队弹框；
        // Ping=只探活；Preferences=呼出并切设置页；AddBatch=批量入库去重。
        while let Ok(msg) = self.ipc_rx.try_recv() {
            match msg {
                IpcMsg::Activate => {
                    eprintln!("FR-SRC-21: 收到 Activate，呼出窗口");
                    self.show_now(ui.ctx());
                }
                IpcMsg::AddPath(p) => {
                    // FR-SYS-03：转发来的是用户动作数据（SendTo 的路径），与 AddBatch 分支
                    // 「只记条数」同一口径，内容不落日志；这条存在的意义是证明 IPC 收到了。
                    eprintln!("FR-SRC-21: 收到转发路径 1 条（内容不落日志）");
                    self.show_now(ui.ctx());
                    self.ipc_pending.push(dialog_for_sent_path(&p));
                }
                IpcMsg::Ping => {
                    eprintln!("FR-WIN-08: 收到 Ping（仅探活，不改 UI）");
                }
                IpcMsg::Preferences(page) => {
                    eprintln!("FR-WIN-08: 收到 -Preferences {page}");
                    self.show_now(ui.ctx());
                    self.open_settings_page(page);
                }
                IpcMsg::AddBatch(paths) => {
                    eprintln!("FR-WIN-08: 收到批量 SendTo {} 条", paths.len());
                    self.show_now(ui.ctx());
                    self.add_batch(ui.ctx(), paths);
                }
            }
        }

        // 后台操作回执（SendTo 安装结果等）→ Toast。
        while let Ok(res) = self.notify_rx.try_recv() {
            let (text, color) = match res {
                Ok(s) => (format!("✓ {s}"), TOAST_OK),
                Err(s) => (format!("⚠ {s}"), TOAST_ERR),
            };
            self.toast = Some((text, Instant::now() + TOAST_TTL, color));
            ui.ctx().request_repaint_after(TOAST_TTL);
        }

        // 当前无弹窗时从队列弹出下一个 AddDialog（预填状态在接收时已解析好）。
        if self.dialog.is_none() && !self.ipc_pending.is_empty() {
            self.dialog = Some(self.ipc_pending.remove(0));
            ui.ctx().request_repaint();
        }

        // FR-WIN-04 失焦关闭到托盘：焦点丢失后延迟 HIDE_DELAY 复查，仍失焦则关闭到托盘。
        // had_focus_since_show 守卫：呼出时 Visible(true)+Focus 是异步的，会有一帧 focused=false，
        // 没有守卫会「刚呼出就自关闭」；只有真正拿到过焦点后的失焦才武装定时器。
        if self.visible {
            // keep_open 豁免（bf07e45 登记的待办，对话框上线后补上）：
            // Modal（添加/编辑框、删除确认框）或设置页开着时失焦不关到托盘，否则半成品表单/正在看的设置随窗口一起消失；已武装的定时器一并撤销。
            if self.dialog.is_some()
                || self.confirm.is_some()
                || self.internal_confirm.is_some()
                || self.about_open
                || self.help_entry.is_some()
                || self.settings_open
            {
                self.hide_at = None;
            } else if ui.input(|i| i.focused) {
                self.had_focus_since_show = true;
                self.hide_at = None;
            } else if self.had_focus_since_show {
                match self.hide_at {
                    None => {
                        self.hide_at = Some(Instant::now() + HIDE_DELAY);
                        ui.ctx().request_repaint_after(HIDE_DELAY);
                    }
                    Some(deadline) => {
                        let now = Instant::now();
                        if now >= deadline {
                            eprintln!(
                                "FR-WIN-04: 失焦 {}ms 复查仍失焦 → 关闭到托盘",
                                HIDE_DELAY.as_millis()
                            );
                            self.visible = false;
                            self.on_hide();
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        } else {
                            ui.ctx().request_repaint_after(deadline - now);
                        }
                    }
                }
            }
        }

        if self.clear_query_next_frame {
            self.query.clear();
            self.clear_query_next_frame = false;
        }

        // Toast 到点自动消失（失焦时 egui 不产事件，靠 request_repaint_after 唤醒复查）
        if let Some((_, deadline, _)) = &self.toast {
            if Instant::now() >= *deadline {
                self.toast = None;
            }
        }

        while let Ok((source_path, rgba, w, h)) = self.icon_rx.try_recv() {
            let texture = ui.ctx().load_texture(
                &source_path,
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba),
                egui::TextureOptions::default(),
            );
            self.icons.insert(source_path, texture);
        }

        // 「批量导入全部」结果到达：合并去重 → 落 user_items.json → 为未加载过的图标源补起加载器。
        // 先收干通道到局部变量（try_iter 借用 self.apps_rx，头表达式若含循环体会把借用拖满全循环）。
        let batches: Vec<Vec<LaunchItem>> = self.apps_rx.try_iter().collect();
        let mut items_changed = false;
        for mut imported in batches {
            // 批次 D6：隐藏项即使被重新扫描到也不许复活——进清单之前先按 hidden[] 剔掉。
            let hidden_hits = source::application::drop_hidden(&mut imported, &self.prefs.hidden);
            if hidden_hits > 0 {
                eprintln!(
                    "批量导入：{hidden_hits} 条在 hidden[] 里，已跳过（设置 → 高级 可恢复）"
                );
            }
            let old = self.items.clone();
            self.items = dedup_items([std::mem::take(&mut self.items), imported].concat());
            // dedup 保留的先出现项仍持旧对象（学习统计天然在）；若重复项出现在后位被剔前，
            // 统一回填一次兜底。
            self.carry_stats(&old);
            let missing: Vec<String> = self
                .items
                .iter()
                .map(|i| i.source_path.clone())
                .filter(|p| !p.is_empty() && !self.icons.contains_key(p.as_str()))
                .collect();
            if !missing.is_empty() {
                spawn_icon_loader(self.icon_tx.clone(), missing);
            }
            self.save_items();
            items_changed = true;
        }
        if items_changed {
            ui.ctx().request_repaint();
        }

        // FR-WIN-02（批次 Q）：热键回执 + 显示对齐 + 录制接管。放在快捷键分支之前是有意的：
        // 录制态会把本帧的按键事件清空，后面那块整体收不到键；即便如此仍显式加 `!recording` 门，
        // 让"录制时不响应任何快捷键"这件事读得出来，而不是依赖清事件这个隐式效果。
        self.hotkey_frame(ui);
        // 批次 R：补字符键框选键和主热键录制是同一类状态——都在等一次按键，都不该被本程序的
        // 快捷键抢走（`take_keypick` 里的 `take_record_press` 已把整帧 Key 事件清空，这扇门是
        // 让"取键时不响应快捷键"这件事读得出来的那道显式保险）。
        let recording = self.hotkey_recording || self.typeback_armed;

        let mut launch: Option<LaunchItem> = None;
        let mut launch_from_list = false;
        let mut launch_via_digit = false; // 提交即学别名要剥掉尾数字，见 learn_alias 调用处
        // 批次 U：走「脱敏日志」的启动（自定义命令：填好的参数里有查询词）。
        let mut launch_masked = false;
        // 学习统计按**原条目** uid 记；自定义命令的 `entry` 是填过 `{query}` 的副本，
        // 它的派生 uid 与清单里那条不同，不能用。
        let mut launch_uid: Option<String> = None;
        // FR-SRC-10：计算器行的「执行」不启动进程，只把结果数字送进剪贴板（与 launch 互斥）。
        let mut copy_text: Option<String> = None;
        // FR-SRC-11：搜索行的「执行」＝打开 URL（独立函数，日志口径见 `open_search_hit`）。
        let mut open_hit: Option<source::websearch::SearchHit> = None;
        // 批次 X：文件行的三个动作各自一条道（打开 / 在资源管理器里定位 / 复制路径）。
        // 都只带路径字符串而不带 LaunchItem：这三件事都不进清单、不记学习统计。
        let mut open_file: Option<source::filesearch::FileHit> = None;
        // 批次 A3：终端行的「执行」＝在新窗口跑命令（日志不记命令原文）。
        let mut open_term: Option<source::terminal::TermHit> = None;
        let mut select_path: Option<String> = None;
        let mut copy_path: Option<String> = None;
        let mut row_actions: Vec<String> = Vec::new();
        let mut pin_toggles: Vec<String> = Vec::new();

        // 快捷键补键（2026-10-17 立规「每个功能必须配快捷键」）：窗口可见且无弹窗时生效。
        // 键位口径（用户 2026-10-17 指定）：Delete=删除选中项（仅空输入时；有输入时归 TextEdit 前向删除）、
        // Ctrl+Shift+D=删除选中项（不受输入限制）、Ctrl+D=打开选中项所在目录（explorer /select）、
        // Ctrl+,=设置页开/关（2026-10-17 追加；SendTo 已从快捷键撤下改设置页开关）、
        // Ctrl+Del=隐藏选中的出厂条目（批次 F7，仅空查询时；有字时留给 TextEdit 删词）、
        // Ctrl+Shift+N=新建快捷项、Ctrl+Shift+P=置顶/取消置顶选中项（FR-SRC-19；2026-10-22 从 Ctrl+N/P 挪来）、
        // PgUp/PgDn=翻页、Tab=补全或循环下一项（语义见设置页 tab_mode）、Shift+Tab=循环上一项、
        // Ctrl+N/Ctrl+P=恒为循环下/上一项（2026-10-22 用户点名 Emacs 的 next/prev line，
        // 正合需求书 FR-ACT-10 原文；Ctrl+J/Ctrl+K 那对 2026-10-18 加的旧键一并保留），
        // 1-9/0 与 Ctrl+1-9/0=执行第 N 行（FR-ACT-10，2026-10-17）。
        // Space=执行选中项（FR-ACT-01 的 space_to_run，四档 off/word/unique/always，默认 word；
        // 判据与各自代价见设置页「空格执行选中项」）。Shift+空格 恒为插入真空格，不受档位影响。
        // FR-SRC-10（批次 S）：查询能算出一个数时列表第 0 行是计算器行。它的「执行」＝复制纯数字
        // 再关到托盘，走 Enter / 点击 / 数字键这三条；空格与 Tab 补全在选中它时整体不接管。
        // 鼠标中键不再执行任何条目（2026-10-18 用户裁定废弃），移动窗口改由「左键拖空白」承担，见下方同名注释块。
        // consume_key 精确匹配修饰键并吃掉事件，防与 TextEdit/其他控件的键位打架。
        if self.visible
            && !recording
            && self.dialog.is_none()
            && self.confirm.is_none()
            && self.internal_confirm.is_none()
            && self.pending_destructive.is_none()
            && !self.about_open
            && self.help_entry.is_none()
        {
            // 键盘缩放（2026-10-18 立规「每个功能都要配快捷键」：拖边缘是鼠标路径，这里补键盘路径）：
            // Ctrl+= 放大一档、Ctrl+- 缩小一档，搜索页与设置页都可用。走 ViewportCommand::InnerSize
            // （逻辑像素），下限与鼠标缩放同一口径，上限 1200×900 免得连按飞出屏幕。
            let (hit_zoom_in, hit_zoom_out) = {
                let hot = |k| ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, k));
                (hot(egui::Key::Equals), hot(egui::Key::Minus))
            };
            if hit_zoom_in || hit_zoom_out {
                let (dw, dh) = if hit_zoom_in {
                    (60.0, 40.0)
                } else {
                    (-60.0, -40.0)
                };
                let cur = ui.ctx().viewport_rect().size();
                let next = egui::vec2(
                    (cur.x + dw).clamp(MIN_W as f32, 1200.0),
                    (cur.y + dh).clamp(MIN_H as f32, 900.0),
                );
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::InnerSize(next));
                eprintln!(
                    "快捷键 Ctrl+{}：窗口 {}×{} → {}×{}",
                    if hit_zoom_in { "=" } else { "-" },
                    cur.x,
                    cur.y,
                    next.x,
                    next.y
                );
            }
            // 「打开日志」**不配快捷键**（用户 2026-10-19 裁定「不必要弄一个快捷键，将来有其他需求
            // 替换掉就可以」）⇒ 只走托盘菜单那一条路，Ctrl+Shift+L 已释放。这条也是「每个功能必须
            // 配快捷键」那条立规的第二例外（第一例外是批次 K 的"设置页开关类不另配键"）。
            let hit_comma =
                ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma));
            if hit_comma {
                self.settings_open = !self.settings_open;
                self.focus_query = true;
                // 批次 F6：☰ 只在搜索页渲染 ⇒ 进设置页必须先把旗标落下，
                // 否则回到搜索页的第一帧会带着一个"没人负责关闭"的菜单。
                self.menu_open = false;
                eprintln!(
                    "快捷键 Ctrl+,：设置页 {}",
                    if self.settings_open {
                        "打开"
                    } else {
                        "返回搜索"
                    }
                );
                ui.ctx().request_repaint();
            } else if !self.settings_open {
                // Ctrl+Shift+N 新建、Ctrl+Shift+P 置顶、Ctrl+Shift+D 删除（2026-10-22 用户改键：
                // Ctrl+N/Ctrl+P 让给"下一项/上一项"，与需求书 FR-ACT-10 原文的 `Ctrl+P/N` 移动对齐；
                // 置顶挪到 Shift 层，与新建同层对称）。
                // **带 Shift 的三条必须先消费**：`consume_key` 走 `Modifiers::matches_logically`，
                // 它只看"模式要的修饰键有没有按"，忽略多出来的 Shift（egui 文档原话：先匹配更具体的
                // 组合）⇒ 先吃 COMMAND 会把 COMMAND|SHIFT 一起吞掉。原先把 Ctrl+D 排在 Ctrl+Shift+D
                // 前面，导致删除键永远走不到 —— 本次改键时发现并一并修正。
                let (hit_csn, hit_csd, hit_csp) = {
                    let hot = |k: egui::Key| {
                        ui.input_mut(|i| {
                            i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, k)
                        })
                    };
                    (hot(egui::Key::N), hot(egui::Key::D), hot(egui::Key::P))
                };
                let (hit_n, hit_p, hit_i, hit_e, hit_cd, hit_cf, hit_cm) = {
                    let hot = |mods: egui::Modifiers, k: egui::Key| {
                        ui.input_mut(|i| i.consume_key(mods, k))
                    };
                    (
                        hot(egui::Modifiers::COMMAND, egui::Key::N),
                        hot(egui::Modifiers::COMMAND, egui::Key::P),
                        hot(egui::Modifiers::COMMAND, egui::Key::I),
                        hot(egui::Modifiers::COMMAND, egui::Key::E),
                        hot(egui::Modifiers::COMMAND, egui::Key::D),
                        hot(egui::Modifiers::COMMAND, egui::Key::F),
                        hot(egui::Modifiers::COMMAND, egui::Key::M),
                    )
                };
                // 裸 Delete 只在搜索框为空时接管：有输入时它是文本编辑键，不能抢。
                let hit_del = self.query.trim().is_empty()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
                // 批次 F7（D6 / FR-SRC-17）：Ctrl+Del 隐藏选中的出厂条目。
                // 同样只在查询框为空时接管——有字时 Ctrl+Del 是 TextEdit 的「删除后一个词」。
                let hit_cdel = self.query.trim().is_empty()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Delete));

                // FR-ACT-10 补键：PgUp/PgDn 翻页；FR-ACT-06 Tab / Shift+Tab（语义见 tab_mode）。
                // Shift+Tab 必须先 consume：egui 的 consume_key 走 Modifiers::matches_logically，
                // 它对 shift 只查"模式要 shift 而实际没有"，所以 NONE 模式会把 Shift+Tab 一起吃掉。
                let (hit_pgup, hit_pgdn, hit_shift_tab, hit_tab) = {
                    let hot = |mods: egui::Modifiers, k: egui::Key| {
                        ui.input_mut(|i| i.consume_key(mods, k))
                    };
                    (
                        hot(egui::Modifiers::NONE, egui::Key::PageUp),
                        hot(egui::Modifiers::NONE, egui::Key::PageDown),
                        hot(egui::Modifiers::SHIFT, egui::Key::Tab),
                        hot(egui::Modifiers::NONE, egui::Key::Tab),
                    )
                };
                // Ctrl+J / Ctrl+K ＝ 无条件循环导航（2026-10-18 用户点名要 Emacs 的 next/prev line；
                // 当时 Ctrl+N/Ctrl+P 被「新建」「置顶」占着，2026-10-22 那两项挪到 Shift 层后
                // N/P 也接上了同一个动作，这两键作为老习惯保留）。
                let (hit_ctrl_j, hit_ctrl_k, hit_ctrl_g) = {
                    let hot = |mods: egui::Modifiers, k: egui::Key| {
                        ui.input_mut(|i| i.consume_key(mods, k))
                    };
                    (
                        hot(egui::Modifiers::COMMAND, egui::Key::J),
                        hot(egui::Modifiers::COMMAND, egui::Key::K),
                        // 批次 G1：缺省分类范围轮换键（设置页「通用」页同一格的快捷键）。
                        hot(egui::Modifiers::COMMAND, egui::Key::G),
                    )
                };
                // FR-ACT-01 把数字键列为执行手段：Ctrl+ / Alt+1..9/0 不受输入限制，
                // 裸数字只在输入框为空时接管；2026-10-23 起计算器源开着时再让一步
                //（表达式第一个字符必是数字），判据见 `bare_digit_allowed`。
                let calc_on = self.prefs.source_on(source::SourceId::Calc);
                let hit_digit = pressed_digit(ui, egui::Modifiers::COMMAND)
                    .or_else(|| pressed_digit(ui, egui::Modifiers::ALT))
                    .or_else(|| {
                        // 2026-10-24 用户报障：空输入框按 Shift+2 想打 @，却被当成裸数字执行了第 2 行。
                        // 根因＝`consume_key` 走 `matches_logically`，**多余的 Shift/Alt 会被忽略**
                        //（egui-0.36.2 input_state/mod.rs:722 文档原文）⇒ 模式 NONE 也命中 Shift+数字。
                        // 先按 SHIFT 层吃掉这一下（与 Shift+空格同一手法）；键事件被吃不挡字符事件，
                        // @ / # 照进 TextEdit。只在裸数字这一档做：Ctrl+ / Alt+数字 的原行为不动。
                        let _ = pressed_digit(ui, egui::Modifiers::SHIFT);
                        if bare_digit_allowed(&self.query, calc_on) {
                            pressed_digit(ui, egui::Modifiers::NONE)
                        } else {
                            None
                        }
                    });
                // 空格执行（FR-ACT-01 `space_to_run`，批次 J 2026-10-18 用户裁定扩成四档）：
                // 判据全在 matching::space_executes，只看当帧可观测状态（档位/查询/结果集/选中行）。
                // Shift+空格 ＝ 逃生门，恒为"插入一个真空格"：必须先按 SHIFT 消费键事件，
                // 因为 consume_key(NONE, ·) 对 shift 只查"模式要而实际没有"，会把 Shift 组合一起吃掉
                // （同一坑见批次 F 的 Shift+Tab）。键事件被吃不影响字符事件，空格照样进 TextEdit。
                let space_down = ui.input(|i| i.key_pressed(egui::Key::Space));
                let hit_shift_space = space_down
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Space));
                if hit_shift_space {
                    eprintln!(
                        "快捷键 Shift+空格：按档位 {:?} 本会执行，改判为插入真空格",
                        self.prefs.space_to_run
                    );
                }
                // `plan()` 要 `&mut self`（缓存键表），与下面 `&self.query`/`&self.items`
                // 冲突，所以先落一个局部副本；只在真的按下空格时才算这一次过滤。
                let mut space_head_len = 0usize;
                let mut space_results: Vec<usize> = Vec::new();
                if space_down && !hit_shift_space {
                    let plan = self.plan();
                    space_head_len = plan.head.len();
                    space_results = plan.results;
                }
                // 批次 S/T：选中行在头段（计算器/搜索/提示）⇒ 空格整体不接管（那些行的执行入口
                // 是 Enter/点击/数字键）。`item_rank` 同时把「合并行号」换算成 `space_executes`
                // 要的名次——头段占了前几行时名次得整体后移，否则四档判据会错位。
                let space_rank = item_rank(self.selected, space_head_len);
                let space_takes = space_down
                    && !hit_shift_space
                    && space_rank.is_some_and(|rank| {
                        matching::space_executes(
                            self.prefs.space_to_run,
                            &self.query,
                            &self.items,
                            &space_results,
                            rank,
                        )
                    });
                let hit_space = space_takes
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));
                if space_down && !hit_shift_space && !hit_space {
                    // 反向判据也要留痕：注入实测要看的就是"这一按空格到底进没进输入框"。
                    // 【2026-10-20 批次 S 顺带修掉一处隐私漏词】这行原本把 `self.query` 原样写进
                    // 日志——与批次 P 定的「绝不记查询框原文」冲突，改成只记档位/条数/选中行号。
                    eprintln!(
                        "空格未接管（档位 {}，结果 {} 条，头段 {} 行，选中第 {} 行）：键事件留给输入框当文本",
                        self.prefs.space_to_run.as_str(),
                        space_results.len(),
                        space_head_len,
                        self.selected + 1
                    );
                }

                if hit_cm {
                    // 批次 F6：☰ 主菜单的键盘入口（用户裁定「三条杠菜单 + 快捷键都留」）。
                    // 这里只翻旗标——菜单项本身在顶栏那帧由 `main_menu_content` 渲染并返回动作。
                    self.menu_open = !self.menu_open;
                    eprintln!(
                        "快捷键 Ctrl+M：☰ 主菜单 {}",
                        if self.menu_open { "打开" } else { "收起" }
                    );
                    ui.ctx().request_repaint();
                } else if hit_cf {
                    // 批次 X：Ctrl+F 进文件模式 = 在已有输入前补一个 `'` 触发符（打的词留着当过滤词，
                    // 不要用户重打一遍）。已经在文件模式里就不叠第二个前缀——`''词` 会被当成
                    // 文件名的一部分去匹配，那一下按键把用户的查询弄脏了，比没反应更糟。
                    let p = source::filesearch::route(&self.query);
                    let already = self.query.starts_with('\'')
                        || self.query.starts_with(' ')
                        || matches!(
                            p,
                            source::filesearch::Route::Search { .. }
                                | source::filesearch::Route::Browse { .. }
                        );
                    if already {
                        self.toast = Some((
                            "已在文件模式（Esc 清空回到清单；直接打路径也能浏览）".to_string(),
                            Instant::now() + TOAST_TTL,
                            TOAST_OK,
                        ));
                        ui.ctx().request_repaint_after(TOAST_TTL);
                        eprintln!("快捷键 Ctrl+F：已在文件模式，不叠第二个前缀");
                    } else {
                        let text = format!("'{}", self.query.trim());
                        self.set_query(
                            ui.ctx(),
                            text,
                            "快捷键 Ctrl+F：进入文件模式（补 ' 前缀，原输入保留为过滤词、不写日志）",
                        );
                    }
                } else if hit_csn {
                    // 批次 X：选中的是文件行时把这一行预填进添加对话框（"从眼前这一行建快捷项"），
                    // 其余情况仍是空白对话框——新建的语义不动，只是键从 Ctrl+N 挪到 Ctrl+Shift+N
                    // （2026-10-22 用户改键）。
                    // 没预填那一条也要留痕（2026-10-22 真机⑧报的就是"弹了框但两格全空"，当时日志
                    // 里没有任何一行能说明它走的是哪个分支——测量不能只记成功那一半）。
                    let (hit, head_len) = {
                        let plan = self.plan();
                        let head_len = plan.head.len();
                        (
                            plan.head
                                .get(self.selected)
                                .and_then(|r| r.file_hit())
                                .cloned(),
                            head_len,
                        )
                    };
                    self.dialog = Some(match hit {
                        Some(hit) => {
                            eprintln!(
                                "快捷键 Ctrl+Shift+N：用选中的文件行预填添加框（路径不落日志）"
                            );
                            DialogState::from_file_hit(&hit)
                        }
                        None => {
                            eprintln!(
                                "快捷键 Ctrl+Shift+N：选中的不是文件行（第 {} 行，头段 {head_len} 行）⇒ 空白添加框",
                                self.selected + 1
                            );
                            DialogState::blank()
                        }
                    });
                    self.dialog_enter_armed = true; // 开框这次 Enter 归"开框"，同帧不误提交
                } else if hit_i {
                    source::application::spawn_batch_import(self.apps_tx.clone());
                    self.toast = Some((
                        "✓ 批量导入已启动，完成后自动合并".to_string(),
                        Instant::now() + TOAST_TTL,
                        TOAST_OK,
                    ));
                    ui.ctx().request_repaint_after(TOAST_TTL);
                    eprintln!("快捷键 Ctrl+I：批量导入全部 已触发（后台扫描开始菜单/桌面）");
                } else if hit_e || hit_cd || hit_del || hit_csd || hit_csp || hit_cdel {
                    // 头段行（计算器/搜索/提示）没有条目身份 ⇒ 编辑/删除/置顶/打开目录这四档
                    // 整体不适用。判据只落在 `sel` 这一层：选中头段 ⇒ 拿不到条目，下面的动作原样跳过。
                    // 例外（批次 U）：自定义命令的提示行/独占行背后是清单里真实的一条 ⇒ 编辑与删除
                    // 按 uid 回查原条目照常可用；置顶与打开目录仍不适用（对命令无意义，旧口径不动）。
                    let plan = self.plan();
                    // 批次 X：文件行不在清单里、没有条目身份 ⇒ 编辑/删除/置顶对它无意义，
                    // 但 Ctrl+D 有真实含义（在资源管理器里定位这个文件）。下面那套按 uid 回查
                    // 的逻辑对文件行天然拿不到 `sel`，所以只需在这里单独取走 Ctrl+D 这一档。
                    let on_file_row = plan
                        .head
                        .get(self.selected)
                        .and_then(|r| r.file_hit())
                        .cloned();
                    let handled_file_cd = hit_cd && on_file_row.is_some();
                    if let Some(hit) = on_file_row.filter(|_| hit_cd) {
                        select_path = Some(hit.path.clone());
                        eprintln!(
                            "快捷键 Ctrl+D：在资源管理器中定位文件行（路径由查询现出，不落日志）"
                        );
                    }
                    let item_row = item_rank(self.selected, plan.head.len());
                    let results = plan.results;
                    // search 返回下标而不是引用：调用方接下来要改 self.items。
                    let mut sel =
                        item_row.and_then(|r| results.get(r).map(|i| self.items[*i].clone()));
                    if sel.is_none() && (hit_e || hit_del || hit_csd || hit_cdel) {
                        if let Some(Row::Head(h)) =
                            row_at(self.selected, results.len(), plan.head.len())
                        {
                            sel = plan.head[h]
                                .custom_uid()
                                .and_then(|u| self.items.iter().find(|i| i.uid() == u).cloned());
                        }
                    }
                    if item_row.is_none() && sel.is_none() && !handled_file_cd {
                        eprintln!("快捷键 编辑/删除/隐藏/置顶/开目录：选中的是头段结果行，这几档不适用");
                    }
                    // 身份一律用派生 uid（同 path 不同参数是两个条目），path 只作数据用。
                    if let Some(s) = sel {
                        let uid = s.uid();
                        let name = s.name.clone();
                        let (path, source_path) = (s.path.clone(), s.source_path.clone());
                        if hit_e {
                            self.dialog = Some(DialogState::from_item(&s));
                            self.dialog_enter_armed = true;
                        } else if hit_csp {
                            // FR-SRC-19 Ctrl+Shift+P：切换选中项置顶，立即落盘、Toast 回执。
                            // （2026-10-22 从 Ctrl+P 挪来——Ctrl+P 现在是「上一项」。）
                            // 用下标+先取副本，避免 iter_mut 借用未结束就调 &self 的 save_items。
                            if let Some(pos) = self.items.iter().position(|i| i.uid() == uid) {
                                self.items[pos].pinned = !self.items[pos].pinned;
                                let on = self.items[pos].pinned;
                                let shown = self.items[pos].name.clone();
                                self.save_items();
                                let text = if on {
                                    format!("✓ 已置顶「{shown}」📌（重启仍在）")
                                } else {
                                    format!("✓ 已取消置顶「{shown}」")
                                };
                                self.toast = Some((text, Instant::now() + TOAST_TTL, TOAST_OK));
                                ui.ctx().request_repaint_after(TOAST_TTL);
                                eprintln!("快捷键 Ctrl+Shift+P：置顶切换 {uid} → {on}");
                            }
                        } else if hit_cdel {
                            // 批次 D6：任何条目都能隐藏——出厂的、扫描导入的、自定义的都行。
                            // 隐藏前先按 uid 存一份完整快照，「高级」页靠它显示原名并原样恢复；
                            // 扫描项没有出厂种子，没有快照就永远回不来。
                            if !self.prefs.hidden.iter().any(|h| h == &uid) {
                                self.prefs.hidden.push(uid.clone());
                            }
                            source::application::stash_hidden(&mut self.prefs.hidden_stash, &s);
                            self.items.retain(|i| i.uid() != uid);
                            self.selected = 0;
                            self.save_items();
                            source::application::save_settings(&self.prefs);
                            self.refresh_path_states(true);
                            self.toast = Some((
                                format!("✓ 已隐藏「{name}」（设置 → 高级 可恢复）"),
                                Instant::now() + TOAST_TTL,
                                TOAST_OK,
                            ));
                            ui.ctx().request_repaint_after(TOAST_TTL);
                            eprintln!(
                                "快捷键 Ctrl+Del：隐藏条目 {uid}（已写 hidden[] 与快照，不删磁盘文件）"
                            );
                        } else if hit_del || hit_csd {
                            // 2026-10-18 用户要求（选中「控制面板」被误删过一次）：删除一律先弹确认，
                            // 按键不再一键生效；真正的移除在确认弹窗按「确定」那一帧才做。
                            self.confirm = Some(ConfirmRequest { uid, name, path });
                            eprintln!("快捷键 删除（Delete/Ctrl+Shift+D）：已弹确认，未删除");
                        } else if let Some(target) = resolve_file_for_select(&path, &source_path) {
                            // Ctrl+D：资源管理器打开所在目录并选中该文件，随后关到托盘（同运行语义）。
                            let entry = LaunchItem {
                                name: format!("打开目录：{name}"),
                                path: "explorer.exe".to_string(),
                                arguments: format!("/select,\"{target}\""),
                                working_dir: None,
                                source_path: String::new(),
                                kind: ItemKind::Command,
                                keyword: String::new(),
                                ..Default::default()
                            };
                            self.launch_and_hide(ui.ctx(), &entry);
                        } else {
                            self.toast = Some((
                                format!("⚠ 「{name}」没有可定位的文件（命令名/URL/虚拟文件夹）"),
                                Instant::now() + TOAST_TTL,
                                TOAST_ERR,
                            ));
                            ui.ctx().request_repaint_after(TOAST_TTL);
                            eprintln!("快捷键 Ctrl+D：{path:?} 无可定位文件，未打开目录");
                        }
                    }
                } else if hit_pgup || hit_pgdn {
                    // 翻页/循环全按「合并行号」走：头段那几行也是可停的行。
                    let plan = self.plan();
                    let len = row_count(plan.results.len(), plan.head.len());
                    if len > 0 {
                        // 批次 F6：一页几行由「行密度」决定，不再读主题的 row_height——
                        // 除了「跟随主题」档，那一档下主题才是行高的来源。
                        let (row_h, _, spacing) = self.row_geom();
                        let rows = page_rows(
                            ui.available_height(),
                            row_h + spacing,
                            list_chrome_h(&self.theme),
                        );
                        self.selected = page_target(self.selected, rows, len, hit_pgdn);
                        eprintln!(
                            "快捷键 {}：选中第 {} 行（一页 {rows} 行，共 {len} 行）",
                            if hit_pgdn { "PgDn" } else { "PgUp" },
                            self.selected + 1
                        );
                    }
                } else if hit_ctrl_j || hit_ctrl_k || hit_n || hit_p {
                    // 与 Tab/Shift+Tab 同口径（回绕循环，不是 ↑↓ 的钳位），差别只有一点：
                    // 这几对键**永远只导航**，不会被 tab_mode=Complete 的补全抢走 ——
                    // 补全优先模式下想要一条稳定的「下一条/上一条」通道就靠它们。
                    // Ctrl+N/Ctrl+P 是 2026-10-22 用户点名的一对（Emacs 的 next/prev line，也正合
                    // 需求书 FR-ACT-10 原文的 `Ctrl+P/N` 移动）；Ctrl+J/Ctrl+K 一并保留，老习惯不断。
                    let backward = hit_ctrl_k || hit_p;
                    let key = if hit_p {
                        "Ctrl+P"
                    } else if hit_ctrl_k {
                        "Ctrl+K"
                    } else if hit_n {
                        "Ctrl+N"
                    } else {
                        "Ctrl+J"
                    };
                    let plan = self.plan();
                    let len = row_count(plan.results.len(), plan.head.len());
                    if len > 0 {
                        let cur = self.selected.min(len - 1);
                        self.selected = if backward {
                            (cur + len - 1) % len
                        } else {
                            (cur + 1) % len
                        };
                        eprintln!(
                            "快捷键 {key}：循环选中第 {} 行（共 {len} 行）",
                            self.selected + 1
                        );
                    } else {
                        eprintln!("快捷键 {key}：当前查询无结果，忽略");
                    }
                } else if hit_ctrl_g {
                    // 批次 G1：缺省分类范围一键轮换（all ⇄ basic），设置页「通用」页同一格的快捷键。
                    self.prefs.default_scope = match self.prefs.default_scope {
                        Scope::All => Scope::Basic,
                        Scope::Basic => Scope::All,
                    };
                    source::application::save_settings(&self.prefs);
                    let msg = format!("缺省分类范围：{}", self.prefs.default_scope.label());
                    self.hint_toast(ui.ctx(), &msg);
                    eprintln!("快捷键 Ctrl+G：缺省分类范围 ⇄ {}（已写 settings.json）", msg);
                } else if hit_tab || hit_shift_tab {
                    // FR-ACT-06 Tab 语义由设置页 `tab_mode` 二选一（2026-10-18 用户裁定两种都做）：
                    //   Complete（默认）＝查询能把结果集消歧到只剩一条可补长的候选 ⇒ 补全成它的名称；
                    //                     否则（歧义未消 / 无可补全）⇒ 循环下一项。
                    //   Navigate＝Tab/Shift+Tab 专职下/上循环导航（Delphi 版 ALTRun 口径），补全不占键位。
                    // 补全这一半只在「无歧义」时出手（matching::unique_completion，2026-10-18 甲案）：
                    // 列表是查询的派生值，补成完整名称必然让结果集塌成一条，所以候选≥2 时不猜、走导航。
                    // 分流只看当前状态（查询词、选中项），不看按键历史——时序判据在按需重绘下
                    // 会被合帧/拆帧，同一手势在不同时刻行为不同，不可测也不好解释。
                    // Shift+Tab 恒为上一项（两种模式一致）。导航用回绕，与 ↑↓ 的钳位口径不同：
                    // ↑↓ 已复测定型不动，Tab 这一对按 Delphi 的「循环」语义走。
                    self.focus_query = true; // egui 把 Tab/Shift+Tab 判给下一个/上一个控件，焦点会跳一帧
                    let plan = self.plan();
                    let head_len = plan.head.len();
                    let results = plan.results;
                    let len = row_count(results.len(), head_len);
                    if len > 0 {
                        let cur = self.selected.min(len - 1);
                        let backward = hit_shift_tab;
                        let next = if backward {
                            (cur + len - 1) % len
                        } else {
                            (cur + 1) % len
                        };
                        // 选中头段行时补整也不接管 —— 那一下 Tab 只循环到下一行。头段正是查询串
                        // 的派生物：从计算器/搜索行补成某个应用名，眼前那一行（或整段独占结果）
                        // 会当场消失，这个"跳行"比导航更难解释。
                        let complete_to = if backward
                            || self.selected < head_len
                            || !matches!(self.prefs.tab_mode, TabMode::Complete)
                        {
                            None
                        } else {
                            matching::unique_completion(&self.query, &self.items, &results)
                                .map(|rank| self.items[results[rank]].name.clone())
                        };
                        // 批次 X（结 B4）：选中目录型文件行时 Tab ＝ 进这个目录——把路径写进查询框
                        // 并以分隔符结尾，浏览模式自己会列它。这一档排在补全之前是有意的：
                        // 头段行本来就不参与补全（上面 `self.selected < head_len` 那条判据为此存在），
                        // 现在它有了自己的 Tab 语义，不再是"什么也不做、只跳下一行"。
                        let into_dir = if backward || self.selected >= head_len {
                            None
                        } else {
                            plan.head[self.selected]
                                .file_hit()
                                .filter(|h| h.is_dir)
                                .map(|h| format!("{}\\", h.path))
                        };
                        if let Some(dir) = into_dir {
                            self.set_query(
                                ui.ctx(),
                                dir,
                                "快捷键 Tab：进入选中的目录（浏览模式，路径不写日志）",
                            );
                        } else if let Some(name) = complete_to {
                            let chars = name.chars().count();
                            self.query = name.clone();
                            self.selected = 0;
                            // 光标位置存在 TextEditState 里，程序化换掉整个字符串不会带着它走 ⇒
                            // 补完接着打字会插进词中间（`Task Sch|eduler`）。2026-10-18 用户裁定补到词尾。
                            if let Some(mut st) = egui::widgets::text_edit::TextEditState::load(
                                ui.ctx(),
                                search_field_id(),
                            ) {
                                st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                                    egui::text::CCursor::new(chars),
                                )));
                                st.store(ui.ctx(), search_field_id());
                            }
                            eprintln!("快捷键 Tab：查询补全为「{name}」（唯一可补全的候选）");
                        } else {
                            self.selected = next;
                            eprintln!(
                                "快捷键 {}：循环选中第 {} 行（共 {len} 行）",
                                if backward { "Shift+Tab" } else { "Tab" },
                                next + 1
                            );
                        }
                    } else {
                        eprintln!("快捷键 Tab：当前查询无结果，忽略");
                    }
                } else if let Some(digit) = hit_digit {
                    let plan = self.plan();
                    let head_len = plan.head.len();
                    let results = plan.results;
                    let len = row_count(results.len(), head_len);
                    match digit_index(digit, len) {
                        Some(idx) => match row_at(idx, results.len(), head_len) {
                            Some(Row::Head(h)) => match &plan.head[h] {
                                HeadRow::Calc(hit) => {
                                    // FR-SRC-10：数字键指到计算器行 ⇒ 与 Enter 同一语义（复制）。
                                    copy_text = Some(hit.copy.clone());
                                    eprintln!(
                                        "快捷键 数字键 {digit}：复制计算器结果（表达式原文不落日志）"
                                    );
                                }
                                HeadRow::Search(hit) => {
                                    // FR-SRC-11：与 Enter 同一语义（用默认浏览器打开）。
                                    eprintln!(
                                        "快捷键 数字键 {digit}：网页搜索（引擎 {}，查询原文不落日志）",
                                        hit.engine_id
                                    );
                                    open_hit = Some(hit.clone());
                                }
                                HeadRow::Hint(hint) => {
                                    // 需求书把空词这一格定为 valid=false：数字键不执行，只提示。
                                    eprintln!("快捷键 数字键 {digit}：选中的是补全提示行，不执行");
                                    self.hint_toast(ui.ctx(), hint_message(hint));
                                }
                                HeadRow::Custom(hit) => {
                                    // FR-SRC-05：与 Enter 同一语义，日志走脱敏路径。
                                    eprintln!(
                                        "快捷键 数字键 {digit}：执行自定义命令「{}」（参数不落日志）",
                                        hit.name
                                    );
                                    launch = Some(hit.entry.clone());
                                    launch_uid = Some(hit.uid.clone());
                                    launch_masked = true;
                                    launch_from_list = true;
                                    launch_via_digit = true;
                                }
                                HeadRow::CustomHint(hint) => {
                                    eprintln!(
                                        "快捷键 数字键 {digit}：选中「{}」的补全提示行",
                                        hint.title
                                    );
                                    self.custom_hint_activate(ui.ctx(), hint);
                                }
                                HeadRow::File(hit) => {
                                    eprintln!(
                                        "快捷键 数字键 {digit}：打开文件行的{}（路径由查询现出，不落日志）",
                                        if hit.is_dir { "目录" } else { "文件" }
                                    );
                                    open_file = Some(hit.clone());
                                }
                                HeadRow::Terminal(hit) => {
                                    eprintln!(
                                        "快捷键 数字键 {digit}：在终端执行（shell {}，命令原文不落日志）",
                                        hit.shell.as_str()
                                    );
                                    open_term = Some(hit.clone());
                                }
                                HeadRow::TermHint(_) => {
                                    eprintln!("快捷键 数字键 {digit}：选中的是终端用法行，不执行");
                                    self.hint_toast(ui.ctx(), source::terminal::USAGE_TOAST);
                                }
                                HeadRow::Cheat(hit) => {
                                    // FR-SRC-18：与 Enter 同一语义——打开帮助页，不关窗口。
                                    eprintln!(
                                        "快捷键 数字键 {digit}：打开速查表条目「{}」",
                                        hit.entry.title
                                    );
                                    self.help_entry = Some(*hit);
                                }
                                HeadRow::CheatHint(_) => {
                                    eprintln!(
                                        "快捷键 数字键 {digit}：选中速查表的无匹配提示行，不执行"
                                    );
                                    self.hint_toast(ui.ctx(), source::cheatsheet::NO_MATCH_TOAST);
                                }
                                HeadRow::ScopeTags(hint) => {
                                    eprintln!("快捷键 数字键 {digit}：选中的是分类提示行，不执行");
                                    self.hint_toast(ui.ctx(), hint.subtitle.as_str());
                                }
                            },
                            Some(Row::Item(rank)) => {
                                let item = self.items[results[rank]].clone();
                                eprintln!(
                                    "快捷键 数字键 {digit}：执行第 {} 行「{}」",
                                    idx + 1,
                                    item.name
                                );
                                launch = Some(item);
                                launch_from_list = true;
                                launch_via_digit = true;
                            }
                            None => {}
                        },
                        None => eprintln!(
                            "快捷键 数字键 {digit}：第 {} 行不存在（共 {len} 行），忽略",
                            if digit == 0 { 10 } else { digit as usize }
                        ),
                    }
                } else if hit_space {
                    let plan = self.plan();
                    let head_len = plan.head.len();
                    let results = plan.results;
                    let len = row_count(results.len(), head_len);
                    // 选中头段行时 `space_takes` 已为假 ⇒ 这里只可能拿到条目行；None 分支不用管。
                    if let Some(Row::Item(rank)) = row_at(
                        self.selected.min(len.saturating_sub(1)),
                        results.len(),
                        head_len,
                    ) {
                        let item = self.items[results[rank]].clone();
                        eprintln!(
                            "快捷键 空格：执行第 {} 行「{}」",
                            rank + 1 + head_len,
                            item.name
                        );
                        launch = Some(item);
                        launch_from_list = true;
                    }
                }
            }
        }

        // 左键拖空白移动窗口（2026-10-18 用户裁定：移动窗口不必非得中键，中键执行彻底废弃；
        // 无边框窗口此前没有任何移动手段，附录 A 参考项目「拖空白移动窗口」那格一直空着）。
        //
        // 不用 ViewportCommand::StartDrag：实测它把窗口交给系统拖拽循环后，抬起事件被循环吃掉，
        // egui 的左键一直卡在「按下」——用户复测的三个症状（设置页点不动、右键菜单弹不出、
        // 第二次拖不动）都是这一个根因。改成自己按帧给窗口位移，键鼠状态全程由 egui 正常收发。
        //
        // 列表那块「右侧真空白」不能在这里铺：它盖在 ScrollArea 上，必须与滚动区域同层，
        // 否则滚轮会被背景层吃掉（2026-10-24 修，见 `inline_drag_handle` 与它的调用点）。
        // 顶部栏那段空白在 CentralPanel 里量到真实坐标后再铺（见 drag_handle 的两处调用）。
        if let Some(a) = self.win_drag {
            // 只有真·抬起才结束：中途停手不动还要继续跟着，所以不能拿「本帧零位移」当结束条件。
            if !ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary)) {
                self.win_drag = None;
            } else if let Some((cx, cy)) = win_cursor() {
                let (dx, dy) = (cx - a.cursor.0, cy - a.cursor.1);
                // 下限用逻辑像素写，撞上限再按当前 DPI 换算（360 逻辑 ≈ 540 物理 @150%）。
                let ppf = ui.ctx().pixels_per_point();
                let (min_w, min_h) = ((MIN_W as f32 * ppf) as i32, (MIN_H as f32 * ppf) as i32);
                let (mut x, mut y, mut w, mut h) = if a.edge == EDGE_MOVE {
                    (a.origin.0 + dx, a.origin.1 + dy, a.size.0, a.size.1)
                } else {
                    (a.origin.0, a.origin.1, a.size.0, a.size.1)
                };
                if a.edge & EDGE_LEFT != 0 {
                    w = (a.size.0 - dx).max(min_w);
                    x = a.origin.0 + (a.size.0 - w); // 右缘钉住，撞下限时左边不再往里走
                } else if a.edge & EDGE_RIGHT != 0 {
                    w = (a.size.0 + dx).max(min_w);
                }
                if a.edge & EDGE_TOP != 0 {
                    h = (a.size.1 - dy).max(min_h);
                    y = a.origin.1 + (a.size.1 - h); // 下缘钉住
                } else if a.edge & EDGE_BOTTOM != 0 {
                    h = (a.size.1 + dy).max(min_h);
                }
                if (x, y, w, h) != (a.origin.0, a.origin.1, a.size.0, a.size.1)
                    && unsafe {
                        SetWindowPos(
                            a.hwnd,
                            None,
                            x,
                            y,
                            if a.edge == EDGE_MOVE { 0 } else { w },
                            if a.edge == EDGE_MOVE { 0 } else { h },
                            if a.edge == EDGE_MOVE {
                                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE
                            } else {
                                SWP_NOZORDER | SWP_NOACTIVATE
                            },
                        )
                    }
                    .is_err()
                {
                    self.win_drag = None; // 窗口已没了（销毁/隐藏）：别再追
                }
            }
            // 拖窗期间必须自己催帧：eframe 在「无输入」时不重绘，否则窗口跟不上手。
            if self.win_drag.is_some() {
                ui.ctx().request_repaint();
            }
        }

        // 批次 F6：☰ 菜单是否开着要在渲染前抄一份。`egui::Popup` 自己就在本帧吃掉 Escape 并把
        // `menu_open` 写回 false，等下面那段全局 Esc 分段跑到时旗标已经是"关"了——不抄这一份，
        // Esc 会同时关掉菜单和整窗（用户期望：菜单开着时 Esc 只收菜单）。
        let menu_was_open = self.menu_open;
        // 批次 F1（FR-UIX-06）：整窗底色 / 边框 / 圆角 / 内边距 / 不透明度全部走主题。
        // 圆角以外不画任何东西 ⇒ GL 后端下四角真的透出桌面（E-18）。
        // 批次 F8（FR-UIX-07）：淡入只做"入"这一半，120ms cubic ease-out。面板框架由
        // `CentralPanel` 自己画，`set_opacity` 管不到 ⇒ 底色与描边先乘一次同样的进度。
        let fade = self.fade_progress();
        let mut frame = self.theme.central_frame();
        if let Some(t) = fade {
            frame.fill = with_alpha(frame.fill, t);
            frame.stroke.color = with_alpha(frame.stroke.color, t);
            ui.ctx().request_repaint();
        }
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            if let Some(t) = fade {
                ui.set_opacity(t);
            }
            // 设置页整页模式（2026-10-17）：小窗口内独立弹窗会被裁剪，直接替换搜索页渲染。
            if self.settings_open {
                self.settings_ui(ui);
                return;
            }
            // 状态栏要钉在面板底边，列表可视区按它让出一块；先在加任何控件之前量下整块内容区。
            let content_rect = ui.max_rect();
            let search_id = search_field_id();
            // 顶栏拖动手柄要铺在「搜索框右缘 → ☰ 左缘」这段真空白上，尺寸只能运行时量
            //（2026-10-18 用户：「顶部输入框右侧点击空白不能拖动，顶部的不是菜单栏吗？」）。
            // 批次 F6：输入框与 ☰ 改成显式定宽定高（原先 ☰ 那个位置靠嵌套 right_to_left 往左溢出
            // 对齐，量到的 `pill_right` 其实是整行右缘）。现在三个 x 都是各自控件的真边界，
            // 手柄不会再铺到输入框身上，也不会在 ☰ 左边留出一段拖不动的死区。
            let mut pill_right = 0.0f32;
            let mut menu_left = f32::MAX;
            // ☰ 那一列占的宽度（含一个列间距），用来从剩余宽度里先扣掉它。
            let menu_col_w = MENU_BTN_W + ui.spacing().item_spacing.x;
            let head_row = ui.horizontal(|ui| {
                ui.label("\u{1F50D} ");
                let edit_w = (ui.available_width() - menu_col_w - TOP_DRAG_GAP).max(60.0);
                // 批次 F6（2026-10-24 用户正文：「输入框大一点，像 ahk 版的输入框一样，大气一点」）：
                // 高度钳到 [`INPUT_MIN_H`] 并让文字垂直居中——原先 TextEdit 只按字体高长到 ~29px。
                // 批次 F8（FR-UIX-08）：状态栏开着时键位说明在栏里，输入框只留一句；
                // 关掉状态栏则回落到那一整句长提示（两者的耦合口径就这一处）。
                let hint = if self.prefs.appearance.status_bar {
                    SEARCH_HINT_SHORT
                } else {
                    SEARCH_HINT
                };
                let resp = ui.add_sized(
                    [edit_w, INPUT_MIN_H],
                    egui::TextEdit::singleline(&mut self.query)
                        .id(search_id)
                        .font(egui::FontId::proportional(self.theme.input_font_size))
                        .vertical_align(egui::Align::Center)
                        .hint_text(hint),
                );
                pill_right = resp.rect.max.x;
                // 批次 F6：这条缝是给下面「搜索页顶栏空白」那个拖动手柄留的命（见 [`TOP_DRAG_GAP`]）。
                ui.add_space(TOP_DRAG_GAP);
                if resp.changed() {
                    self.selected = 0; // 输入变化时选中回到第一行
                }
                // 批次 F6：「＋ 新建」按钮撤掉，换成 ☰ 主菜单（用户点名「换成一个三条杠的菜单，
                // 点击显示新建按钮和其他设置按钮」）。新建那条路一条没少：菜单第一项 + Ctrl+Shift+N。
                let menu_btn = ui
                    .add_sized(
                        [menu_col_w, INPUT_MIN_H],
                        egui::Button::new(
                            egui::RichText::new("\u{2630}").size(self.theme.title_font_size + 5.0),
                        ),
                    )
                    .on_hover_text("主菜单（Ctrl+M）");
                menu_left = menu_btn.rect.min.x;
                if menu_btn.clicked() {
                    // 手动翻转：`open_bool` 覆盖了 `Popup::menu` 自带的 memory toggle，
                    // 而键盘那条路（Ctrl+M）本来也只能靠这个旗标进菜单。
                    self.menu_open = !self.menu_open;
                }
                let action = egui::Popup::menu(&menu_btn)
                    .id(egui::Id::new("main_menu_popup"))
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .open_bool(&mut self.menu_open)
                    .show(main_menu_content)
                    .map_or(MenuAction::None, |r| r.inner);
                // 借用在这里已经结束（`show` 返回的对象不带 `self` 的生命周期），动作落回 self。
                // 批次 F6：动作一旦落地就把旗标按下去。`Popup` 自带的 `CloseOnClick` 判据是
                // `was_open_last_frame && pointer.any_click()`（`popup.rs:608`），只有真鼠标才置
                // `any_click` ⇒ 用辅助功能（AccessKit/UIA 的 Invoke）点中条目时旗标留在 true，
                // 下一次 Esc 会跑去"收菜单"、把刚打开的对话框留在原地（2026-10-24 真机抓到）。
                if !matches!(action, MenuAction::None) {
                    self.menu_open = false;
                }
                match action {
                    MenuAction::None => {}
                    MenuAction::NewItem => self.dialog = Some(DialogState::blank()),
                    MenuAction::BatchImport => {
                        source::application::spawn_batch_import(self.apps_tx.clone());
                        self.toast = Some((
                            "✓ 批量导入已启动，完成后自动合并".to_string(),
                            Instant::now() + TOAST_TTL,
                            TOAST_OK,
                        ));
                        ui.ctx().request_repaint_after(TOAST_TTL);
                        eprintln!("☰ 菜单：批量导入全部 已触发（后台扫描开始菜单/桌面）");
                    }
                    MenuAction::Settings => {
                        self.settings_open = true;
                        eprintln!("☰ 菜单：进设置页");
                    }
                    MenuAction::Cheatsheet => {
                        // 速查表在列表里，入口就是查询框的那个 `?`（批次 A8）：代打字，不另开一套渲染。
                        self.set_query(ui.ctx(), "?".to_string(), "☰ 菜单：打开速查表");
                    }
                    MenuAction::OpenLog => {
                        open_log_file();
                        eprintln!("☰ 菜单：打开日志");
                    }
                    MenuAction::Exit => {
                        self.on_hide();
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        eprintln!("☰ 菜单：退出");
                    }
                }
            });
            if self.dialog.is_none()
                && self.confirm.is_none()
                && self.internal_confirm.is_none()
                && !self.about_open
                && self.help_entry.is_none()
            {
                let r = head_row.response.rect;
                self.drag_handle(
                    ui,
                    "window_drag_layer_search_top",
                    egui::Rect::from_min_max(
                        egui::pos2(pill_right + TOP_DRAG_INSET, r.min.y),
                        egui::pos2(menu_left - TOP_DRAG_INSET, r.max.y),
                    ),
                    EDGE_MOVE,
                    None,
                    "搜索页顶栏空白",
                );
            }
            // 呼出后把键盘焦点交给搜索框（widget 焦点，独立于 ViewportCommand::Focus 的原生窗口焦点）
            if self.focus_query {
                ui.memory_mut(|m| m.request_focus(search_id));
                self.focus_query = false;
                ui.ctx().request_repaint();
            }
            ui.separator();

            // 批次 S/T：列表 = 头段（计算器/搜索/提示）+ 应用结果，`self.selected` 按「合并行号」计
            //（换算全走 `row_at`/`item_rank` 这两个纯函数，见它们各自的判据注释）。
            // 批次 F8：计划要早于拖窗手柄算——状态栏只在有结果时占那 44px，
            // 手柄底边得跟着让，不然它盖住状态栏的右半边、点上去是拖窗而不是点键帽。
            let q = self.query.trim().to_lowercase();
            let plan = self.plan();
            let head_len = plan.head.len();
            let results = plan.results;
            let total = row_count(results.len(), head_len);
            let status_shown = self.prefs.appearance.status_bar && total > 0;
            let file_route = source::filesearch::route(&self.query);
            let in_file_mode = !matches!(file_route, source::filesearch::Route::NotOurs);

            // 2026-10-24 修滚轮：列表右侧那块「拖空白移动窗口」的手柄必须与 ScrollArea 同层。
            // 原先它铺在独立的 Background `Area` 上（`drag_handle`），指针一进去，
            // `Context::rect_contains_pointer` 认到的层就不是滚动区域那一层，滚轮整段被吞。
            // 同层注册后：滚轮照常滚，点击仍归结果行（行是 click，这里是 drag-only，且行在本行之后注册）。
            // 弹窗/确认框/帮助页打开时整块撤掉，与原来的让位规则一致。
            if !(self.dialog.is_some()
                || self.confirm.is_some()
                || self.internal_confirm.is_some()
                || self.about_open
                || self.help_entry.is_some())
            {
                let root = ui.ctx().viewport_rect();
                let plate = egui::Rect::from_min_max(
                    egui::pos2(root.min.x + 260.0, root.min.y + 40.0),
                    root.max
                        - egui::vec2(
                            12.0,
                            6.0 + if status_shown { STATUS_BAR_BLOCK } else { 0.0 },
                        ),
                );
                self.inline_drag_handle(ui, "window_drag_layer_list", plate);
            }

            if total == 0 {
                // 批次 X：文件模式下"未找到"这三行字会说错两件事——范围（不是"清单里没有"，
                // 是"索引范围里没有"或"这个目录空/读不到"）与预填（`'报告`、`pic 报告` 这种带
                // 触发符的串喂给 `from_input` 会生成一条命令行是乱码的条目）。所以另起一格。
                if q.is_empty() {
                    ui.label("清单为空——按 Ctrl+Shift+N、或点右上 ☰ 里的「新建快捷项」，也可按 Ctrl+I 批量导入。");
                } else if in_file_mode {
                    if self.prefs.source_on(source::SourceId::Files) {
                        ui.label(match &file_route {
                            source::filesearch::Route::Browse { dir, .. } =>
                                format!("这个目录为空或读不到：{dir}"),
                            _ => "文件模式：索引范围（桌面 / 下载 / 文档，四级以内）内没有匹配的文件。"
                                .to_string(),
                        });
                    } else {
                        // 2026-10-23 A2：源关着的时候说「索引范围内没有匹配」会把用户
                        // 引向错的地方——此刻根本没有索引，开关就在设置页第 4 页。
                        ui.label(
                            "文件搜索已关闭——按 Ctrl+, 进设置，在「数据源」页把「文件搜索」打开后再试。",
                        );
                    }
                    if ui.button("＋ 新建快捷项").clicked() {
                        self.dialog = Some(DialogState::blank());
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.dialog = Some(DialogState::blank());
                        self.dialog_enter_armed = true;
                    }
                } else {
                    ui.label(format!("未找到「{}」——按 Enter 或点下方按钮弹出新增窗口", self.query.trim()));
                    // FR-SRC-23（2026-10-17 修订）：无匹配 Enter 不再直接运行，而是弹出预填当前输入的 AddDialog，确定即入库。
                    if ui.button("＋ 添加为快捷项").clicked() {
                        let dq = DialogState::from_input(&self.query);
                        self.dialog = Some(dq);
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let dq = DialogState::from_input(&self.query);
                        self.dialog = Some(dq);
                        // 本次 Enter 归"开框"所有；同帧对话框若立即监听 Enter 会误触发确定，置 armed 吞掉。
                        self.dialog_enter_armed = true;
                    }
                }
            } else {
                if self.selected >= total {
                    self.selected = 0;
                }
                // 键盘导航：↑↓ 移动选中，Enter 运行选中项。对话框/确认框开着时让位给它们（Enter=确定）。
                if self.dialog.is_none()
                    && self.confirm.is_none()
                    && self.internal_confirm.is_none()
                    && !self.about_open
                    && self.help_entry.is_none()
                {
                    if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                        self.selected = (self.selected + 1).min(total - 1);
                    } else if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                        self.selected = self.selected.saturating_sub(1);
                    } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        match row_at(self.selected, results.len(), head_len) {
                            // FR-SRC-10 的 `text` 动作：Enter 只把纯数字放进剪贴板，然后关到托盘。
                            Some(Row::Head(h)) => match &plan.head[h] {
                                HeadRow::Calc(hit) => copy_text = Some(hit.copy.clone()),
                                HeadRow::Search(hit) => open_hit = Some(hit.clone()),
                                HeadRow::Hint(hint) => self.hint_toast(ui.ctx(), hint_message(hint)),
                                HeadRow::Custom(hit) => {
                                    launch = Some(hit.entry.clone());
                                    launch_uid = Some(hit.uid.clone());
                                    launch_masked = true;
                                    launch_from_list = true;
                                }
                                HeadRow::CustomHint(hint) => {
                                    self.custom_hint_activate(ui.ctx(), hint);
                                }
                                HeadRow::File(hit) => open_file = Some(hit.clone()),
                                HeadRow::Terminal(hit) => open_term = Some(hit.clone()),
                                HeadRow::TermHint(_) => {
                                    self.hint_toast(ui.ctx(), source::terminal::USAGE_TOAST)
                                }
                                HeadRow::Cheat(hit) => self.help_entry = Some(*hit),
                                HeadRow::CheatHint(_) => {
                                    self.hint_toast(ui.ctx(), source::cheatsheet::NO_MATCH_TOAST)
                                }
                                HeadRow::ScopeTags(hint) => {
                                    self.hint_toast(ui.ctx(), hint.subtitle.as_str())
                                }
                            },
                            Some(Row::Item(rank)) => {
                                launch = Some(self.items[results[rank]].clone());
                                launch_from_list = true;
                            }
                            None => {}
                        }
                    }
                }
                // 列表滚动跟随选中项（FR-ACT-10 验收项）：只在选中变化的那一帧把它居中，
                // 其余帧不干预用户自己的滚轮位置。
                let follow_selection = self.selected != self.scroll_followed;
                // 批次 F6：行高/图标由「行密度」给（默认甲 34/24），只有「跟随主题」档才回落到
                // 主题的 `row_height` / `icon_size`。`Ui::horizontal` 的行高初值取自
                // `spacing.interact_size.y` ⇒ 这一处赋值让所有结果行同高（含头段行）；
                // `item_spacing.y` 就是行与行的间距，窗口高度公式用的是同一对数。
                let (row_h, icon, row_spacing) = self.row_geom();
                let sub_size = self.theme.subtitle_font_size;
                let glyph = (icon * 0.6).max(10.0);
                // results 是下标（见 MatchIndex::search），逐项回指 self.items。
                // 套 ScrollArea 才谈得上翻页与滚动跟随：窗口只装得下约十行，清单可达上百行。
                // 批次 F8：状态栏要占底部 38 + 6，滚动区先把它让出来（否则列表会垫在栏下面）。
                let list_h = (ui.available_height()
                    - if status_shown { STATUS_BAR_BLOCK } else { 0.0 })
                .max(1.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(list_h)
                    .show(ui, |ui| {
                        ui.spacing_mut().interact_size.y = row_h;
                        ui.spacing_mut().item_spacing.y = row_spacing;
                        for (h, row) in plan.head.iter().enumerate() {
                            let is_sel = self.selected == h;
                            match row {
                                HeadRow::Calc(hit) => {
                                    // 标题只有数字（`= 600`），单位/进制/来源同一行弱色副标题；
                                    // 复制走 `hit.copy`——不带千分位、不带单位，粘进别的程序就是一串数字。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, "复制");
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new("=")
                                                    .monospace()
                                                    .size(glyph)
                                                    .weak(),
                                            ),
                                        )
                                        .on_hover_text(source::calc::ROW_TAG);
                                        let sel = ui.selectable_label(is_sel, hit.title.clone());
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        ui.label(
                                            egui::RichText::new(hit.subtitle.as_str()).weak().size(sub_size),
                                        );
                                        if sel.clicked() {
                                            copy_text = Some(hit.copy.clone());
                                        }
                                        sel.context_menu(|ui| {
                                            if ui.button("复制结果").clicked() {
                                                copy_text = Some(hit.copy.clone());
                                                ui.close();
                                            }
                                        });
                                    });
                                }
                                HeadRow::Search(hit) => {
                                    // FR-SRC-11：标题是「Google「词」」，副标题给域名与执行提示。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, "搜索");
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new("\u{1F50D}").size(glyph).weak(),
                                            ),
                                        )
                                        .on_hover_text("网页搜索（FR-SRC-11）");
                                        let sel = ui.selectable_label(is_sel, hit.title.clone());
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        ui.label(
                                            egui::RichText::new(hit.subtitle.as_str()).weak().size(sub_size),
                                        );
                                        if sel.clicked() {
                                            open_hit = Some(hit.clone());
                                        }
                                    });
                                }
                                HeadRow::Hint(hint) => {
                                    // 不可执行行：弱色显示，点/按只给一句 Toast（`hint_toast`）。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, "查看");
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new("…").monospace().size(glyph).weak(),
                                            ),
                                        );
                                        let sel = ui.selectable_label(
                                            is_sel,
                                            egui::RichText::new(hint.title.as_str()).weak(),
                                        );
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        ui.label(
                                            egui::RichText::new(hint.subtitle.as_str()).weak().size(sub_size),
                                        );
                                        if sel.clicked() {
                                            self.hint_toast(ui.ctx(), hint_message(hint));
                                        }
                                    });
                                }
                                HeadRow::Custom(hit) => {
                                    // FR-SRC-05：`{query}` 已填好的独占行；路径四态跟条目一起显示。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, "运行");
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new("▸").monospace().size(glyph).weak(),
                                            ),
                                        )
                                        .on_hover_text("自定义命令（FR-SRC-05）");
                                        let sel = ui.selectable_label(is_sel, hit.name.clone());
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        let state = self
                                            .path_states
                                            .get(&hit.uid)
                                            .copied()
                                            .unwrap_or_default();
                                        if state.is_problem() {
                                            ui.label(
                                                egui::RichText::new(state.label())
                                                    .small()
                                                    .color(path_state_color(state)),
                                            );
                                        }
                                        ui.label(
                                            egui::RichText::new(hit.subtitle.as_str()).weak().size(sub_size),
                                        );
                                        if sel.clicked() {
                                            launch = Some(hit.entry.clone());
                                            launch_uid = Some(hit.uid.clone());
                                            launch_masked = true;
                                            launch_from_list = true;
                                        }
                                    });
                                }
                                HeadRow::CustomHint(hint) => {
                                    // 不可执行行（valid=false）：点/按都是"补全成 `kw `"。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, "补全");
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new("…").monospace().size(glyph).weak(),
                                            ),
                                        );
                                        let sel = ui.selectable_label(
                                            is_sel,
                                            egui::RichText::new(hint.title.as_str()).weak(),
                                        );
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        // 把"我是清单里哪一条"写在脸上：只报「需要参数」会被当成
                                        // 系统自带的另一条同名命令（2026-10-21 真人反馈）。
                                        if !hint.command.is_empty() {
                                            ui.label(
                                                egui::RichText::new(hint.command.as_str())
                                                    .monospace()
                                                    .weak()
                                                    .size(sub_size),
                                            );
                                        }
                                        ui.label(
                                            egui::RichText::new(hint.subtitle.as_str()).weak().size(sub_size),
                                        );
                                        if sel.clicked() {
                                            self.custom_hint_activate(ui.ctx(), hint);
                                        }
                                        // 头段行没有名次，但这一类有条目身份 ⇒ 右键照样编辑/删除（按 uid 回查）。
                                        sel.context_menu(|ui| {
                                            if ui.button("编辑…").clicked() {
                                                if let Some(it) = self
                                                    .items
                                                    .iter()
                                                    .find(|i| i.uid() == hint.uid)
                                                    .cloned()
                                                {
                                                    self.dialog = Some(DialogState::from_item(&it));
                                                }
                                                ui.close();
                                            }
                                            if ui.button("删除（不删磁盘文件）").clicked() {
                                                row_actions.push(hint.uid.clone());
                                                ui.close();
                                            }
                                        });
                                    });
                                }
                                HeadRow::File(hit) => {
                                    // 批次 X（FR-SRC-14）：标题是文件名，副标题是所在目录——同一屏里
                                    // 好几个同名文件全靠后半句认出哪一个才是要找的那个。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, "打开");
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new(if hit.is_dir {
                                                    "\u{1F4C1}"
                                                } else {
                                                    "\u{1F4C4}"
                                                })
                                                .size(glyph)
                                                .weak(),
                                            ),
                                        )
                                        .on_hover_text(if hit.is_dir {
                                            "文件夹（FR-SRC-14，选中按 Tab 进入）"
                                        } else {
                                            "文件（FR-SRC-14）"
                                        });
                                        let sel = ui.selectable_label(is_sel, hit.title.clone());
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        ui.label(
                                            egui::RichText::new(hit.subtitle.as_str()).weak().size(sub_size),
                                        );
                                        if sel.clicked() {
                                            open_file = Some(hit.clone());
                                        }
                                        // 文件行不在清单里 ⇒ 没有"编辑/删除清单条目"可言，右键给三件
                                        // 对这个文件本身有意义的事（复制 / 定位 / 收进清单）。
                                        sel.context_menu(|ui| {
                                            if ui.button("复制路径").clicked() {
                                                copy_path = Some(hit.path.clone());
                                                ui.close();
                                            }
                                            if ui.button("在资源管理器中显示").clicked() {
                                                select_path = Some(hit.path.clone());
                                                ui.close();
                                            }
                                            if ui.button("添加为快捷项…").clicked() {
                                                self.dialog = Some(DialogState::from_file_hit(hit));
                                                ui.close();
                                            }
                                        });
                                    });
                                }
                                HeadRow::Terminal(hit) => {
                                    // FR-SRC-16：整张列表就这一行，标题原样是命令（等宽字体，
                                    // 让 `|`、`>` 这类符号对得齐）。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, "执行");
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new(">").monospace().size(glyph).weak(),
                                            ),
                                        )
                                        .on_hover_text("终端命令（FR-SRC-16）");
                                        let sel = ui.selectable_label(
                                            is_sel,
                                            egui::RichText::new(hit.title.as_str()).monospace(),
                                        );
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        ui.label(
                                            egui::RichText::new(hit.subtitle.as_str()).weak().size(sub_size),
                                        );
                                        if sel.clicked() {
                                            open_term = Some(hit.clone());
                                        }
                                    });
                                }
                                HeadRow::TermHint(hint) => {
                                    // 不可执行行：点/按只给一句「继续输入命令」。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, "查看");
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new(">").monospace().size(glyph).weak(),
                                            ),
                                        );
                                        let sel = ui.selectable_label(
                                            is_sel,
                                            egui::RichText::new(hint.title.as_str()).weak(),
                                        );
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        ui.label(
                                            egui::RichText::new(hint.subtitle.as_str()).weak().size(sub_size),
                                        );
                                        if sel.clicked() {
                                            self.hint_toast(ui.ctx(), source::terminal::USAGE_TOAST);
                                        }
                                    });
                                }
                                HeadRow::Cheat(hit) => {
                                    // FR-SRC-18：一行一条语法/快捷键；点击＝打开帮助页（窗口不关）。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, "查看");
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new("?").monospace().size(glyph).weak(),
                                            ),
                                        )
                                        .on_hover_text(format!(
                                            "速查表 · {}",
                                            hit.entry.group.label()
                                        ));
                                        let sel = ui.selectable_label(
                                            is_sel,
                                            egui::RichText::new(hit.entry.key).monospace(),
                                        );
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        ui.label(
                                            egui::RichText::new(hit.entry.group.label())
                                                .weak()
                                                .size(11.0),
                                        );
                                        ui.label(
                                            egui::RichText::new(hit.entry.summary).weak().size(sub_size),
                                        );
                                        if sel.clicked() {
                                            self.help_entry = Some(*hit);
                                        }
                                    });
                                }
                                HeadRow::CheatHint(hint) => {
                                    // 过滤词一条也没命中的提示行：点/按只给一句换词建议。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, "查看");
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new("?").monospace().size(glyph).weak(),
                                            ),
                                        );
                                        let sel = ui.selectable_label(
                                            is_sel,
                                            egui::RichText::new(hint.title.as_str()).weak(),
                                        );
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        ui.label(
                                            egui::RichText::new(hint.subtitle).weak().size(sub_size),
                                        );
                                        if sel.clicked() {
                                            self.hint_toast(
                                                ui.ctx(),
                                                source::cheatsheet::NO_MATCH_TOAST,
                                            );
                                        }
                                    });
                                }
                                HeadRow::ScopeTags(hint) => {
                                    // 批次 G1：裸 `@` 的分类提示行，不可执行；点/按只给用法 Toast。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, "查看");
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new("@").monospace().size(glyph).weak(),
                                            ),
                                        );
                                        let sel = ui.selectable_label(
                                            is_sel,
                                            egui::RichText::new(hint.title.as_str()).weak(),
                                        );
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        ui.label(
                                            egui::RichText::new(hint.subtitle.as_str())
                                                .weak()
                                                .size(sub_size),
                                        );
                                        if sel.clicked() {
                                            self.hint_toast(ui.ctx(), hint.subtitle.as_str());
                                        }
                                    });
                                }
                            }
                        }
                        for (rank, &hit) in results.iter().enumerate() {
                            let idx = rank + head_len;
                            let item = &self.items[hit];
                            let is_sel = idx == self.selected;
                            ui.horizontal(|ui| {
                                // 行号徽标（2026-10-18 用户要求：数字键没有可见对应物）：
                                // 前 10 行标 1-9/0，第 11 行起留白——那里没有按键，标了反而误导。
                                row_badge(ui, idx, "执行");
                                if let Some(texture) = self.icons.get(&item.source_path) {
                                    ui.image(egui::load::SizedTexture::new(texture.id(), egui::vec2(icon, icon)));
                                } else {
                                    ui.add_space(icon);
                                }
                                // FR-SRC-19：置顶项名称前加 📌 角标（免鼠标可辨）。
                                let shown = if item.pinned { format!("📌 {}", item.name) } else { item.name.clone() };
                                let sel = ui.selectable_label(is_sel, shown);
                                if is_sel && follow_selection {
                                    sel.scroll_to_me(Some(egui::Align::Center));
                                }
                                // FR-SRC-06：路径四态只在异常时露出来（OK/- 不占版面）。
                                let uid = item.uid();
                                if let Some(state) = self.path_states.get(&uid) {
                                    if state.is_problem() {
                                        ui.label(
                                            egui::RichText::new(state.label())
                                                .small()
                                                .color(path_state_color(*state)),
                                        );
                                    }
                                }
                                if sel.clicked() {
                                    launch = Some(item.clone());
                                    launch_from_list = true;
                                }
                                // FR-SRC-22：种子已并入普通清单，每项都能编辑/删除
                                sel.context_menu(|ui| {
                                    if ui.button("编辑…").clicked() {
                                        let dq = DialogState::from_item(item);
                                        self.dialog = Some(dq);
                                        ui.close();
                                    }
                                    // FR-SRC-19：置顶开关入口（右键），与 Ctrl+Shift+P 同语义。
                                    let pin_label = if item.pinned { "取消置顶" } else { "置顶" };
                                    if ui.button(pin_label).clicked() {
                                        pin_toggles.push(uid.clone());
                                        ui.close();
                                    }
                                    // 与 Delete/Ctrl+Shift+D 同一条确认路径（2026-10-18 用户要求）。
                                    if ui.button("删除（不删磁盘文件）").clicked() {
                                        row_actions.push(uid.clone());
                                        ui.close();
                                    }
                                });
                            });
                        }
                    });
                self.scroll_followed = self.selected;
            }

            // FR-UIX-08：底部状态栏。只在有结果时出现；列表已在上面让出这一块。
            // 左格说"此刻按下去最有用的事"（返回 / 模式 / Tab），右格是 Enter 的行为标签。
            // 不填底色：整窗底色与透明都交给主题框架，状态栏只画一条分隔线 + 两段文字。
            if status_shown {
                let bar = egui::Rect::from_min_max(
                    egui::pos2(content_rect.min.x, content_rect.max.y - STATUS_BAR_H),
                    content_rect.max,
                );
                ui.painter().hline(
                    bar.x_range(),
                    bar.min.y,
                    egui::Stroke::new(1.0, self.theme.separator),
                );
                let browse = match &file_route {
                    source::filesearch::Route::Browse { dir, filter } => {
                        Some((dir.as_str(), filter.as_str()))
                    }
                    _ => None,
                };
                let left = status_left_text(
                    browse,
                    in_file_mode,
                    self.prefs.tab_mode,
                    matches!(self.prefs.default_scope, Scope::Basic),
                );
                // 左格剪到右格之前：360 宽 + 长路径时宁可截断，也不让两段字叠在一起。
                let left_rect = egui::Rect::from_min_max(
                    bar.min,
                    egui::pos2((bar.max.x - 170.0).max(bar.min.x + 40.0), bar.max.y),
                );
                ui.painter().with_clip_rect(left_rect).text(
                    egui::pos2(bar.min.x, bar.center().y),
                    egui::Align2::LEFT_CENTER,
                    left,
                    egui::FontId::proportional(self.theme.title_font_size),
                    self.theme.subtitle,
                );
                let action = match row_at(self.selected, total - head_len, head_len) {
                    Some(Row::Head(h)) => enter_action_label(plan.head.get(h)),
                    _ => enter_action_label(None),
                };
                let mut right = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(bar.shrink2(egui::vec2(0.0, 4.0)))
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                right.label(
                    egui::RichText::new(action)
                        .size(self.theme.title_font_size)
                        .color(self.theme.title),
                );
                egui::Frame::new()
                    .fill(self.theme.selected_background)
                    .stroke(egui::Stroke::new(1.0, self.theme.border))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(7, 2))
                    .show(&mut right, |ui| {
                        ui.label(
                            egui::RichText::new("Enter")
                                .monospace()
                                .size(self.theme.shortcut_font_size + 1.0)
                                .color(self.theme.input_text),
                        );
                    });
            }
        });

        // 通用 Toast：底部居中悬浮圆角胶囊，TTL 尾段淡出（2026-10-17 用户反馈内联红字不优雅）。
        if let Some((msg, deadline, text_c)) = &self.toast {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let alpha = if remaining < TOAST_FADE {
                (remaining.as_secs_f32() / TOAST_FADE.as_secs_f32()).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let fade = |c: egui::Color32| {
                egui::Color32::from_rgba_unmultiplied(
                    c.r(),
                    c.g(),
                    c.b(),
                    (c.a() as f32 * alpha) as u8,
                )
            };
            // 底色/描边从文字色派生：描边=文字色，底=文字色的 ~22%（红/绿两种 Toast 共用一套画法）。
            let fill = egui::Color32::from_rgb(text_c.r() / 5, text_c.g() / 5, text_c.b() / 5);
            egui::Area::new(egui::Id::new("toast"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -28.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(fade(fill))
                        .stroke(egui::Stroke::new(1.0_f32, fade(*text_c)))
                        .corner_radius(10.0)
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(msg).size(13.0).color(fade(*text_c)));
                        });
                });
            ui.ctx().request_repaint_after(remaining.min(TOAST_FADE));
        }

        // 右键「删除」→ 弹确认（与 Delete/Ctrl+Shift+D 同一条路，2026-10-18 用户要求）；
        // 右键置顶切换 → 立即应用。身份键是派生 uid（同 path 不同参数的条目靠它区分），不是 path。
        if let Some(u) = row_actions.first() {
            if let Some(it) = self.items.iter().find(|i| i.uid() == *u) {
                self.confirm = Some(ConfirmRequest {
                    uid: it.uid(),
                    name: it.name.clone(),
                    path: it.path.clone(),
                });
                eprintln!("右键「删除」：已弹确认 {u}（未删除）");
            }
        }
        if !pin_toggles.is_empty() {
            for u in &pin_toggles {
                if let Some(it) = self.items.iter_mut().find(|i| i.uid() == *u) {
                    it.pinned = !it.pinned;
                    eprintln!("右键：置顶切换 {u} → {}", it.pinned);
                }
            }
            self.save_items();
            ui.ctx().request_repaint();
        }

        // ---- FR-SRC-20 添加/编辑对话框（模态，对标 Delphi frmShortCut）----
        if self.dialog.is_some() {
            let editing = self.dialog.as_ref().unwrap().edit_uid.is_some();
            let title = if editing {
                "编辑快捷项"
            } else {
                "添加快捷项"
            };

            enum DlgResult {
                Keep,
                Confirm,
                Cancel,
            }

            let modal_resp =
                egui::Modal::new(egui::Id::new("shortcut_dialog")).show(ui.ctx(), |ui| {
                    // 尺寸（2026-10-22 用户要求）：原来内容只有 ~300 宽、且不可拉伸。
                    // `Resize` 给一块能拖右下角的区域，egui 按 id 记住用户拉到的尺寸（本次运行内）。
                    // 上限随主窗口走：主窗口本身最小 360×240，固定 640 宽会溢出到窗外。
                    let avail = ui.ctx().viewport_rect().size();
                    let max_w = (avail.x - 48.0).clamp(320.0, 1200.0);
                    let max_h = (avail.y - 48.0).clamp(260.0, 900.0);
                    egui::Resize::default()
                        .id_salt("shortcut_dialog_size")
                        .default_size([640.0_f32.min(max_w), 420.0_f32.min(max_h)])
                        .min_size([460.0_f32.min(max_w), 300.0_f32.min(max_h)])
                        .max_size([max_w, max_h])
                        .show(ui, |ui| {
                            let mut result = DlgResult::Keep;
                            // Enter＝确定（FR-SRC-20 快捷键）：开框当帧被 armed 吞掉；类型下拉展开时 Enter 归选项，不抢。
                            let armed = self.dialog_enter_armed;
                            self.dialog_enter_armed = false;
                            if !armed
                                && !egui::Popup::is_any_open(ui.ctx())
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                result = DlgResult::Confirm;
                            }
                            ui.heading(title);
                            ui.separator();
                            // 每行＝「标签 + 占满剩余宽度的输入框」，输入框跟着窗口一起变宽。
                            let label_w = 72.0;
                            let row =
                                |ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str| {
                                    ui.horizontal(|ui| {
                                        ui.add_sized([label_w, 20.0], egui::Label::new(label));
                                        let w = ui.available_width().max(160.0);
                                        ui.add_sized(
                                            [w, 22.0],
                                            egui::TextEdit::singleline(value).hint_text(hint),
                                        );
                                    });
                                };
                            {
                                let dg = self.dialog.as_mut().unwrap();
                                row(ui, "别名", &mut dg.keyword, "触发码，如 calc");
                                row(ui, "名称", &mut dg.name, "显示名，如 计算器");
                                let path_hint = if dg.kind == ItemKind::Internal {
                                    "内置动作标记 internal:<id>，改了这条就执行不了"
                                } else {
                                    "exe / 路径 / URL / shell: 目标"
                                };
                                row(ui, "命令行", &mut dg.path, path_hint);
                                row(ui, "参数", &mut dg.arguments, "");
                                row(ui, "工作目录", &mut dg.working_dir, "");
                                row(
                                    ui,
                                    "分类",
                                    &mut dg.tags_text,
                                    "逗号/空格分隔，可多个，如：开发, 工具",
                                );
                                ui.horizontal(|ui| {
                                    ui.add_sized([label_w, 20.0], egui::Label::new("类型"));
                                    if dg.kind == ItemKind::Internal {
                                        // 内置动作（批次 A2）不在下拉里：`ALL` 不含 Internal，
                                        // 硬套下拉会 `unwrap_or(0)` 把类型静默改成「应用」，
                                        // 存盘后这条系统命令就再也执行不了。这里只读展示。
                                        ui.label(
                                            egui::RichText::new("系统命令（内置动作，类型不可改）")
                                                .weak(),
                                        );
                                    } else {
                                        let mut idx = ItemKind::ALL
                                            .iter()
                                            .position(|k| *k == dg.kind)
                                            .unwrap_or(0);
                                        egui::ComboBox::from_id_salt("kind_combo")
                                            .selected_text(dg.kind.label())
                                            .show_index(ui, &mut idx, ItemKind::ALL.len(), |i| {
                                                ItemKind::ALL[i].label().to_string()
                                            });
                                        dg.kind = ItemKind::ALL[idx];
                                    }
                                });
                            }
                            // 按钮贴底：窗口被拉高后不会孤零零吊在表单下面。
                            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                                ui.horizontal(|ui| {
                                    if ui.button("确定").clicked() {
                                        result = DlgResult::Confirm;
                                    }
                                    if ui.button("取消").clicked() {
                                        result = DlgResult::Cancel;
                                    }
                                });
                                ui.label(
                                    egui::RichText::new("拖右下角可改大小（本次运行内记住）")
                                        .weak()
                                        .small(),
                                );
                            });
                            result
                        })
                });

            match modal_resp.inner {
                DlgResult::Confirm => {
                    if let Some(dg) = self.dialog.take() {
                        let item = dg.to_item();
                        if item.path.is_empty() {
                            self.dialog = Some(dg); // 目标为空，保持打开不提交
                        } else {
                            // FR-SRC-19：编辑/提交不得丢置顶与使用统计。
                            // 编辑走"按 edit_uid 直接携带"——用户连命令行一起改时 uid 变了，
                            // 事后按 uid 回填会认不出，所以先从旧项整体搬过来；
                            // 快照回填只兜新建项撞上既有 uid（dedup 留旧剔新）的情况。
                            let old = self.items.clone();
                            let mut item = item;
                            // 批次 D6：显式添加撞上 hidden[] ⇒ 以这次添加为准撤销隐藏。
                            if source::application::unhide_uids(
                                &mut self.prefs.hidden,
                                &mut self.prefs.hidden_stash,
                                &[item.uid()],
                            ) > 0
                            {
                                source::application::save_settings(&self.prefs);
                            }
                            match dg.edit_uid {
                                Some(u) => {
                                    if let Some(prev) = old.iter().find(|i| i.uid() == u) {
                                        item.pinned = prev.pinned;
                                        item.use_count = prev.use_count;
                                        item.last_used_ms = prev.last_used_ms;
                                    }
                                    if let Some(existing) =
                                        self.items.iter_mut().find(|i| i.uid() == u)
                                    {
                                        *existing = item.clone();
                                    } else {
                                        self.items.push(item.clone());
                                    }
                                }
                                None => self.items.push(item.clone()),
                            }
                            self.items = dedup_items(std::mem::take(&mut self.items));
                            // uid = 类型+命令行+参数（不含名称，见 application.rs `uid()`）⇒ 与既有条目
                            // 命令行与参数完全相同的新条目会被 dedup 留旧剔新、静默丢掉。批量导入路径
                            // 有「N 条重复已跳过」回执，弹窗这条路原来没有 ⇒ 2026-10-24 G1 真机抓到后补 Toast。
                            let survived = self
                                .items
                                .iter()
                                .any(|i| i.uid() == item.uid() && i.name == item.name);
                            if !survived {
                                self.toast = Some((
                                    format!(
                                        "未添加「{}」：命令行与参数和既有条目重复（同类型才算重复）",
                                        item.name
                                    ),
                                    Instant::now() + TOAST_TTL,
                                    TOAST_ERR,
                                ));
                            }
                            self.carry_stats(&old);
                            if !item.source_path.is_empty()
                                && !self.icons.contains_key(item.source_path.as_str())
                            {
                                spawn_icon_loader(
                                    self.icon_tx.clone(),
                                    vec![item.source_path.clone()],
                                );
                            }
                            self.save_items();
                            // 入库/保存后清空搜索框，呼出即见全清单（FR-SRC-20 添加流一致）。
                            self.clear_query_next_frame = true;
                            ui.ctx().request_repaint();
                        }
                    }
                }
                DlgResult::Cancel => {
                    self.dialog = None;
                    ui.ctx().request_repaint();
                }
                DlgResult::Keep => {}
            }

            // 对话框本帧关闭（确定成功/取消/空目标外全路径）→ 焦点还给搜索框。
            if self.dialog.is_none() {
                self.focus_query = true;
            }
        }

        // ---- 删除确认弹窗（2026-10-18 用户要求：选中「控制面板」被误删过一次）----
        // 三条删除入口（Delete / Ctrl+Shift+D / 右键「删除」）都汇到这里；
        // 移除动作只在本帧按下「确定」时执行，Esc/取消只关框。
        if self.confirm.is_some() {
            enum ConfirmResult {
                Keep,
                Yes,
                No,
            }
            let modal_resp = egui::Modal::new(egui::Id::new("delete_confirm")).show(
                ui.ctx(),
                |ui| {
                    let c = self.confirm.as_ref().unwrap();
                    ui.heading(format!("从清单删除「{}」？", c.name));
                    ui.separator();
                    ui.label(egui::RichText::new(format!("目标：{}", c.path)).small());
                    ui.label(
                        egui::RichText::new(
                            "只从启动器清单移除，不删磁盘上的程序或文件；需要时可再按 Ctrl+Shift+N 新建（或点 ☰ 菜单第一项），也可 Ctrl+I 批量导入。",
                        )
                        .weak()
                        .small(),
                    );
                    let mut result = ConfirmResult::Keep;
                    // Enter=删除（与添加对话框同判据）；Esc 由下方全局分段链处理。
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        result = ConfirmResult::Yes;
                    }
                    ui.horizontal(|ui| {
                        if ui.button("删除 (Enter)").clicked() {
                            result = ConfirmResult::Yes;
                        }
                        if ui.button("取消 (Esc)").clicked() {
                            result = ConfirmResult::No;
                        }
                    });
                    result
                },
            );
            match modal_resp.inner {
                ConfirmResult::Yes => {
                    if let Some(c) = self.confirm.take() {
                        self.items.retain(|i| i.uid() != c.uid);
                        self.selected = 0;
                        self.save_items();
                        self.toast = Some((
                            format!("✓ 已删除「{}」（未删磁盘文件）", c.name),
                            Instant::now() + TOAST_TTL,
                            TOAST_OK,
                        ));
                        ui.ctx().request_repaint_after(TOAST_TTL);
                        eprintln!("删除确认：按「确定」删除 {}（不删磁盘文件）", c.uid);
                        self.focus_query = true;
                    }
                }
                ConfirmResult::No => {
                    eprintln!("删除确认：按「取消」，未删除");
                    self.confirm = None;
                    self.focus_query = true;
                    ui.ctx().request_repaint();
                }
                ConfirmResult::Keep => {}
            }
        }

        // ---- 批次 F7：「高级」页两个破坏性动作的确认 ----
        // 与删除确认同形态：这一帧只问，按「确定」那一帧才真的做；Esc 由下方全局分段链处理。
        if let Some(action) = self.pending_destructive {
            enum DestructiveResult {
                Keep,
                Yes,
                No,
            }
            let modal = egui::Modal::new(egui::Id::new("destructive_confirm")).show(
                ui.ctx(),
                |ui| {
                    let (title, confirm_label, warning) = match action {
                        Destructive::ResetLearning => (
                            "重置学习排序？",
                            "重置 (Enter)",
                            "清空所有条目的使用次数与最近使用时间：排序回到无 Boost 状态，置顶保留。已学到的别名会留下（与手填别名不可区分）。落盘前先备份 user_items.json。",
                        ),
                        Destructive::RestoreDefaults => (
                            "恢复默认？",
                            "恢复 (Enter)",
                            "重置空格执行 / Tab 语义 / 学习排序 / 数据源开关 / 危险操作确认 / 终端 shell / 主题 / 显示条数 / 行密度 / 缺省分类范围。热键、网页引擎、隐藏条目、自定义命令与置顶都会保留。落盘前先备份 settings.json。",
                        ),
                    };
                    ui.heading(title);
                    ui.separator();
                    ui.label(egui::RichText::new(warning).weak().small());
                    let mut result = DestructiveResult::Keep;
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        result = DestructiveResult::Yes;
                    }
                    ui.horizontal(|ui| {
                        if ui.button(confirm_label).clicked() {
                            result = DestructiveResult::Yes;
                        }
                        if ui.button("取消 (Esc)").clicked() {
                            result = DestructiveResult::No;
                        }
                    });
                    result
                },
            );
            match modal.inner {
                DestructiveResult::Yes => {
                    self.pending_destructive = None;
                    match action {
                        Destructive::ResetLearning => {
                            let bak = source::application::backup_file(
                                &source::application::data_dir().join("user_items.json"),
                            );
                            let n = source::application::reset_learning(&mut self.items);
                            self.save_items();
                            let bak_note = match &bak {
                                Some(p) => format!("已备份到 {}", p.display()),
                                None => "清单文件不存在，未写备份".to_string(),
                            };
                            self.toast = Some((
                                format!("✓ 已重置学习排序（{n} 条有统计，置顶保留）"),
                                Instant::now() + TOAST_TTL,
                                TOAST_OK,
                            ));
                            ui.ctx().request_repaint_after(TOAST_TTL);
                            eprintln!("高级页：重置学习排序 → {n} 条统计清零（{bak_note}）");
                        }
                        Destructive::RestoreDefaults => {
                            let bak = source::application::backup_file(
                                &source::application::data_dir().join("settings.json"),
                            );
                            source::application::restore_defaults(&mut self.prefs);
                            source::application::save_settings(&self.prefs);
                            // 立即生效：数据源全开 ⇒ 每一家都补一次初始化（文件源要重建索引）；
                            // 主题走 sync_theme，随后无条件按新行密度重算窗口高度。
                            for s in self.sources.iter_mut() {
                                s.init();
                            }
                            self.sync_theme(ui.ctx());
                            let h = window_height(
                                &self.theme,
                                self.prefs.row_density,
                                self.prefs.appearance.status_bar,
                            );
                            self.resize_to_list(ui.ctx(), h, "恢复默认");
                            // 宽度也回厂（设置页那一格恢复默认后要立刻看得见）。
                            self.apply_width(ui.ctx(), self.prefs.appearance.width, "恢复默认");
                            let bak_note = match &bak {
                                Some(p) => format!("已备份到 {}", p.display()),
                                None => "settings.json 不存在，未写备份".to_string(),
                            };
                            self.toast = Some((
                                "✓ 已恢复默认（热键 / 引擎 / 隐藏项 / 自定义命令保留）".to_string(),
                                Instant::now() + TOAST_TTL,
                                TOAST_OK,
                            ));
                            ui.ctx().request_repaint_after(TOAST_TTL);
                            eprintln!("高级页：恢复默认 → 偏好已重置（{bak_note}）");
                        }
                    }
                }
                DestructiveResult::No => {
                    eprintln!("高级页：破坏性动作按「取消」，未执行");
                    self.pending_destructive = None;
                    ui.ctx().request_repaint();
                }
                DestructiveResult::Keep => {}
            }
        }

        // ---- 批次 A2：危险内置动作的执行确认（关机 / 重启 / 注销 / 清空回收站）----
        // 与删除确认同形态：这一帧只问，按「执行」那一帧才真的做；Esc 由下方全局分段链处理。
        // 别在这里再调 `should_close()`：它不消费按键，Esc 会同一帧被处理两遍——
        // 取消完确认框还会顺着链再清一次查询框（空查询时直接收窗）。
        if self.internal_confirm.is_some() {
            enum InternalResult {
                Keep,
                Yes,
                No,
            }
            let modal = egui::Modal::new(egui::Id::new("internal_confirm")).show(
                ui.ctx(),
                |ui| {
                    let c = self.internal_confirm.as_ref().unwrap();
                    ui.heading(format!("执行「{}」？", c.name));
                    ui.separator();
                    ui.label(egui::RichText::new(confirm_warning(c.action)).strong());
                    ui.label(
                        egui::RichText::new(
                            "这条操作不可撤销。确认前请先保存手头的工作；关掉「危险操作先确认」可跳过这一步。",
                        )
                        .weak()
                        .small(),
                    );
                    let mut result = InternalResult::Keep;
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        result = InternalResult::Yes;
                    }
                    ui.horizontal(|ui| {
                        if ui.button("执行 (Enter)").clicked() {
                            result = InternalResult::Yes;
                        }
                        if ui.button("取消 (Esc)").clicked() {
                            result = InternalResult::No;
                        }
                    });
                    result
                },
            );
            match modal.inner {
                InternalResult::Yes => {
                    if let Some(c) = self.internal_confirm.take() {
                        let ok = self.exec_internal(ui.ctx(), c.action);
                        if ok {
                            // 与「启动成功才累计」同一口径：确认后真的执行了才记一次。
                            self.mark_used(&c.uid, "");
                        }
                        eprintln!(
                            "内部确认：按「执行」→ {}（{}）",
                            c.action.id(),
                            if ok { "成功" } else { "失败" }
                        );
                        self.focus_query = true;
                    }
                }
                InternalResult::No => {
                    eprintln!("内部确认：按「取消」，未执行");
                    self.internal_confirm = None;
                    self.focus_query = true;
                    ui.ctx().request_repaint();
                }
                InternalResult::Keep => {}
            }
        }

        // ---- 批次 A8：速查表条目的帮助页（「对应帮助页」在本程序内，不依赖 docs/）----
        if let Some(hit) = self.help_entry {
            let entry = hit.entry;
            let modal = egui::Modal::new(egui::Id::new("cheatsheet_help")).show(ui.ctx(), |ui| {
                // 模态是居中的胶囊，宽度由内容决定；窗口只有 640 宽时必须主动收窄，
                // 否则 520 的定宽会顶出窗口左右两边（2026-10-23 真机截图裁边）。
                ui.set_max_width((ui.ctx().content_rect().width() - 64.0).clamp(240.0, 520.0));
                ui.heading(entry.title);
                ui.label(
                    egui::RichText::new(format!("速查表 · {}", entry.group.label()))
                        .weak()
                        .small(),
                );
                ui.add_space(4.0);
                ui.label(egui::RichText::new(entry.key).monospace().strong());
                ui.separator();
                ui.label(entry.detail);
                ui.add_space(10.0);
                let mut go_page = None;
                let mut close = false;
                ui.horizontal(|ui| {
                    if let Some(page) = entry.settings_page {
                        let name = match page {
                            1 => "通用",
                            2 => "功能",
                            3 => "热键",
                            4 => "数据源",
                            _ => "设置",
                        };
                        if ui.button(format!("打开设置页（{name}）")).clicked() {
                            go_page = Some(page);
                        }
                    }
                    if ui.button("关闭 (Esc)").clicked() {
                        close = true;
                    }
                });
                (go_page, close)
            });
            // Esc 由下方全局分段链处理，模态里不重复。
            let (go_page, close) = modal.inner;
            if let Some(page) = go_page {
                self.help_entry = None;
                self.open_settings_page(page);
                ui.ctx().request_repaint();
            } else if close {
                self.help_entry = None;
                self.focus_query = true;
                ui.ctx().request_repaint();
            }
        }

        // ---- 批次 A2：「关于本程序」----
        if self.about_open {
            let modal = egui::Modal::new(egui::Id::new("about_program")).show(ui.ctx(), |ui| {
                ui.heading("关于本程序");
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("egui-launcher v{}", env!("CARGO_PKG_VERSION")))
                        .strong(),
                );
                ui.label("Rust + egui/eframe 重写的 ALTRun 启动器。");
                ui.add_space(6.0);
                let exe = std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "（取不到程序路径）".to_string());
                ui.label(egui::RichText::new(format!("程序位置：{exe}")).small());
                ui.label(egui::RichText::new(format!("清单条目：{} 条", self.items.len())).small());
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "日志只记动作和清单内条目的名称 / uid，不记查询框原文，也不记窗口标题。",
                    )
                    .weak()
                    .small(),
                );
                ui.add_space(8.0);
                ui.button("关闭 (Esc)").clicked()
            });
            // Esc 同样交给下方全局分段链，不在模态里重复处理。
            if modal.inner {
                self.about_open = false;
                self.focus_query = true;
                ui.ctx().request_repaint();
            }
        }

        // 运行选中的程序并关闭到托盘（FR-ACT + 用户要求：Enter/点击运行后窗口自动关）。
        // FR-SRC-19：清单项成功启动才累计使用统计（launch_and_hide 失败分支提前 return，走不到这里）。
        if let Some(entry) = launch {
            // 自定义命令的 uid 取原条目（`entry` 是填过 `{query}` 的副本，派生 uid 已变）。
            let uid = launch_uid.clone().unwrap_or_else(|| entry.uid());
            let started = if launch_masked {
                self.launch_and_hide_masked(ui.ctx(), &entry, &uid)
            } else {
                self.launch_and_hide(ui.ctx(), &entry)
            };
            if started && launch_from_list {
                // FR-SRC-19 补全「提交即学别名」（口径与门槛见 learn_alias）。
                self.mark_used(&uid, &commit_word(&self.query, launch_via_digit));
            }
        }

        // FR-SRC-11：搜索行的「执行」＝用默认浏览器打开 + 关到托盘。与上两段互斥：
        // 头段一次只会产生其中一个动作（计算器复制 / 搜索打开 / 条目启动）。
        if let Some(hit) = open_hit {
            self.open_search_hit(ui.ctx(), &hit);
        }

        // 批次 X：文件行的三个动作（打开 / 在资源管理器中定位 / 复制路径）。与上面三段互斥：
        // 一次按键或点击只会产出其中一个。
        if let Some(hit) = open_file {
            self.open_file_hit(ui.ctx(), &hit);
        }
        if let Some(hit) = open_term {
            self.run_terminal_hit(ui.ctx(), &hit);
        }
        if let Some(path) = select_path {
            self.select_in_explorer(ui.ctx(), &path);
        }
        if let Some(path) = copy_path {
            // 复制这一档**不关到托盘**（与计算器那格刻意不同）：右键复制路径通常发生在浏览途中，
            // 用户下一步是去别处粘贴，不是收工。窗口留在原地，只给一句回执。
            match copy_to_clipboard(&path) {
                Ok(()) => {
                    // 路径由查询现出 ⇒ 日志只记长度（FR-SYS-03 隐私口径）。
                    eprintln!(
                        "文件搜索：路径已复制（{} 个字符，路径原文不落日志）",
                        path.chars().count()
                    );
                    self.toast = Some((
                        format!("✓ 已复制路径：{path}"),
                        Instant::now() + TOAST_TTL,
                        TOAST_OK,
                    ));
                }
                Err(e) => {
                    self.toast = Some((
                        format!("⚠ 复制失败：{e}"),
                        Instant::now() + TOAST_TTL,
                        TOAST_ERR,
                    ));
                    eprintln!("文件搜索：复制路径失败（{e}）");
                }
            }
            ui.ctx().request_repaint_after(TOAST_TTL);
        }

        // FR-SRC-10：计算器行的「执行」＝复制纯数字 + 关到托盘（需求原文把它列成 `text` 动作，
        // 不是 `run`——算式结果不该启动任何进程）。与上面那段互斥：选中计算器行时不会产生 LaunchItem。
        // 复制失败不隐藏，与 FR-ACT-02 启动失败同一口径：窗口留在原地把原因显示出来。
        if let Some(text) = copy_text {
            match copy_to_clipboard(&text) {
                Ok(()) => {
                    // 表达式原文不落日志（FR-SYS-03 隐私口径）⇒ 只记长度与成败。
                    eprintln!(
                        "计算器：结果已复制（{} 个字符，表达式原文不落日志）→ 关闭到托盘",
                        text.chars().count()
                    );
                    // FR-UIX-09：主窗随即关到托盘，所以回执只能靠 HUD 送到窗口外。
                    let msg = format!("✓ 已复制 {text}");
                    self.toast = Some((msg.clone(), Instant::now() + TOAST_TTL, TOAST_OK));
                    self.hud
                        .notify(ui.ctx(), msg, &self.theme, hud::HOLD_FOREGROUND);
                    self.clear_query_next_frame = true;
                    self.selected = 0;
                    self.visible = false;
                    self.on_hide();
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    ui.ctx().request_repaint_after(TOAST_TTL);
                }
                Err(e) => {
                    self.toast = Some((
                        format!("⚠ 复制失败：{e}"),
                        Instant::now() + TOAST_TTL,
                        TOAST_ERR,
                    ));
                    ui.ctx().request_repaint_after(TOAST_TTL);
                    eprintln!("计算器：复制失败（{e}）");
                }
            }
        }

        // Esc 分段（FR-ACT-10「清空后隐藏」+2026-10-17 设置页一档 +2026-10-18 删除确认一档
        // +2026-10-24 批次 F6 菜单一档）：
        // 菜单→只收菜单；确认框→关框；对话框→关框；设置页→回搜索；有字→清空；全空→关闭到托盘。
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if menu_was_open {
                // Popup 在本帧已经把旗标写回 false，这里只补一条日志并接住剩下的分支——
                // 不接住的话这一次 Esc 会顺路把查询清空、甚至把窗口收进托盘。
                self.menu_open = false;
                eprintln!("☰ 菜单：按 Esc 收起（窗口与查询都不动）");
            } else if self.pending_destructive.is_some() {
                eprintln!("高级页确认：按 Esc 取消，未执行");
                self.pending_destructive = None;
                self.focus_query = true;
                ui.ctx().request_repaint();
            } else if self.internal_confirm.is_some() {
                eprintln!("内部确认：按 Esc 取消，未执行");
                self.internal_confirm = None;
                self.focus_query = true;
                ui.ctx().request_repaint();
            } else if self.help_entry.is_some() {
                self.help_entry = None;
                self.focus_query = true;
                ui.ctx().request_repaint();
            } else if self.about_open {
                self.about_open = false;
                self.focus_query = true;
                ui.ctx().request_repaint();
            } else if self.confirm.is_some() {
                eprintln!("删除确认：按 Esc 取消，未删除");
                self.confirm = None;
                self.focus_query = true;
                ui.ctx().request_repaint();
            } else if self.dialog.is_some() {
                self.dialog = None;
                self.focus_query = true; // Esc 关框后焦点同样还给搜索框
                ui.ctx().request_repaint();
            } else if self.settings_open {
                self.settings_open = false;
                self.focus_query = true;
                ui.ctx().request_repaint();
            } else if let Some(parent) = browse_parent_of(&self.query) {
                // 批次 F8（FR-SRC-14 附带）：目录浏览里 Esc 逐级返回；先去过滤词再上溯。
                // 盘根返回 None ⇒ 落到下面"清空查询"那一档，行为与以前一致。
                // 日志只记动作，路径原文不落盘（FR-SYS-03）。
                self.set_query(ui.ctx(), parent, "Esc：目录浏览返回上级");
            } else if !self.query.is_empty() {
                self.query.clear();
                self.selected = 0;
                self.focus_query = true; // egui TextEdit 在编辑态按 Esc 会自弃焦点，主动要回
                ui.ctx().request_repaint();
            } else {
                self.visible = false;
                self.on_hide();
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        // 边缘缩放手柄（2026-10-18 用户：「窗口应该允许拖动边缘改变大小，现在无法改变大小」）。
        // 放在本帧最后铺设：同为 Background 层时后铺者赢得命中，万一和顶栏/列表空白手柄贴上，
        // 最外圈那 6px 归缩放。
        self.edge_grips(ui);

        // FR-UIX-09：本帧仍在显示期的 HUD 子视口。主窗隐藏后这里不再跑，但已声明的视口
        // 由 HUD 自己的回调续命（E-08 实测）。
        self.hud.render(ui.ctx());
    }
}

fn create_tray_icon() -> tray_icon::Icon {
    let (w, h) = (32u32, 32u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        rgba.extend_from_slice(&[0, 120, 215, 255]);
    }
    tray_icon::Icon::from_rgba(rgba, w, h).expect("Failed to create tray icon")
}

fn init_tray() -> (TrayIcon, mpsc::Receiver<TrayEvent>) {
    let (tx, rx) = mpsc::channel();
    let show_id = muda::MenuId("show".to_string());
    let import_id = muda::MenuId("import".to_string());
    let settings_id = muda::MenuId("settings".to_string());
    let log_id = muda::MenuId("openlog".to_string());
    let exit_id = muda::MenuId("exit".to_string());
    let show_item = MenuItem::with_id(show_id.clone(), "显示", true, None);
    let import_item = MenuItem::with_id(import_id.clone(), "批量导入全部", true, None);
    let settings_item = MenuItem::with_id(settings_id.clone(), "设置…", true, None);
    let log_item = MenuItem::with_id(log_id.clone(), "打开日志", true, None);
    let exit_item = MenuItem::with_id(exit_id.clone(), "退出", true, None);
    let menu = Menu::new();
    menu.append(&show_item).unwrap();
    menu.append(&import_item).unwrap();
    menu.append(&settings_item).unwrap();
    menu.append(&log_item).unwrap();
    menu.append(&exit_item).unwrap();
    let menu_channel = MenuEvent::receiver();
    thread::spawn(move || {
        loop {
            match menu_channel.recv() {
                Ok(event) => {
                    if event.id == show_id {
                        let _ = tx.send(TrayEvent::ShowWindow);
                    } else if event.id == import_id {
                        let _ = tx.send(TrayEvent::BatchImport);
                    } else if event.id == settings_id {
                        let _ = tx.send(TrayEvent::Settings);
                    } else if event.id == log_id {
                        let _ = tx.send(TrayEvent::OpenLog);
                    } else if event.id == exit_id {
                        let _ = tx.send(TrayEvent::Exit);
                    }
                    if let Some(ctx) = CTX.get() {
                        ctx.request_repaint();
                    }
                }
                Err(_) => break,
            }
        }
    });
    let icon = create_tray_icon();
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("egui Launcher (Ctrl+Alt+Space)")
        .with_icon(icon)
        .with_menu_on_left_click(true)
        .build()
        .expect("Failed to create tray icon");
    (tray, rx)
}

/// 句柄的生命周期：`SetStdHandle` 只换指针、不接管所有权，`File` 一 drop 句柄就悬空，
/// 所以文件必须活到进程结束——放进 `static` 即可，不必 `mem::forget`。
static LOG_FILE: OnceLock<std::fs::File> = OnceLock::new();

/// 没有控制台可看日志时，把 stderr 接到 `launcher.log`（与清单同目录，便携优先）。
///
/// 只换标准错误句柄 ⇒ 全部 `eprintln!` 调用点一行不动，panic 信息也一起进文件。
/// 判据是「没人听」：GUI 子系统进程从登录自启/资源管理器启动时 `GetStdHandle` 直接报
/// 错（句柄为 NULL/INVALID），日志会全部落空 ⇒ 这时才接管；从终端启动或被外部重定向
/// 捕获时句柄有效 ⇒ 一个字都不动，保住实时日志这条取证通道。两条分支均在本机实测。
///
/// 批次 P 起，落笔前先把上一轮的内容归档（跨天或超 `LOG_ROTATE_BYTES` ⇒
/// `launcher-<内容日期>.log`，只留 `LOG_KEEP_ARCHIVES` 份），并在新文件首行留日期锚——
/// 归档命名读的就是那个锚，不是 mtime（换算函数在现有 feature 下不可达，见 `application.rs`）。
fn redirect_stderr_if_orphan() {
    if unsafe { GetStdHandle(STD_ERROR_HANDLE) }.is_ok() {
        return;
    }
    let stamp = source::application::rotate_log_and_stamp();
    let path = source::application::log_path();
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return; // 日志文件打不开也要照常启动，不因记不了日志而退出
    };
    let file = LOG_FILE.get_or_init(|| file);
    if unsafe { SetStdHandle(STD_ERROR_HANDLE, HANDLE(file.as_raw_handle())) }.is_ok() {
        eprintln!(
            "{}{stamp} ｜ 本次启动没有控制台，stderr 接到 {}",
            source::application::LOG_DATE_TAG,
            path.display()
        );
    }
}

fn main() -> eframe::Result {
    redirect_stderr_if_orphan(); // 必须在第一条日志之前：换句柄只管之后的写
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = cli::parse(&args);

    // FR-SRC-21 / FR-WIN-07 单实例：命名互斥体判定。次实例把命令行意图经 WM_COPYDATA
    // 交给运行中实例后自身退出（不产生第二个托盘图标）。
    // -Reloaded：旧实例正在退出，先等它释放互斥体（最多 2s）再接管。
    let primary = if cli.reloaded {
        wait_for_mutex(Duration::from_secs(2))
    } else {
        acquire_single_instance()
    };
    let primary = match primary {
        Some(h) => Some(h),
        None => {
            let mut status = forward_cli(&cli);
            // 旧实例可能还在启动、消息窗尚未创建：短暂重试，避免把「启动中」误判成「卡死」。
            for _ in 0..20 {
                if status != IpcSendStatus::NoWindow {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
                status = forward_cli(&cli);
            }
            match status {
                IpcSendStatus::Ack => {
                    eprintln!("single instance: 已转发给运行实例，本进程退出");
                    return Ok(());
                }
                IpcSendStatus::Timeout => {
                    // 消息窗在但 2s 无应答 ⇒ 按 FR-WIN-07 认定卡死：结束旧实例后接管。
                    if kill_hung_instance() {
                        match wait_for_mutex(Duration::from_secs(2)) {
                            Some(h) => {
                                eprintln!("single instance: 已接管卡死的旧实例");
                                Some(h)
                            }
                            None => {
                                eprintln!(
                                    "single instance: 旧实例已结束但互斥体未释放，按第二实例启动（降级）"
                                );
                                None
                            }
                        }
                    } else {
                        eprintln!("single instance: 无法结束卡死的旧实例，按第二实例启动（降级）");
                        None
                    }
                }
                IpcSendStatus::NoWindow => {
                    // 互斥体在但找不到消息窗：无法安全定位旧进程，不做无依据的杀进程。
                    if cli.startup {
                        eprintln!("single instance: 已有实例但找不到消息窗，-Startup 本进程退出");
                        return Ok(());
                    }
                    eprintln!("single instance: 已有实例但找不到消息窗，按第二实例启动（降级）");
                    None
                }
            }
        }
    };
    // 主实例的互斥体句柄不显式 Close：windows 的 HANDLE 无 Drop，进程退出时由系统回收。
    let _primary_handle = primary;

    // FR-WIN-08：命令行意图 → 首帧初始状态。
    // 静默自启不弹窗；SendTo / Preferences 本身就是要用户看见的，故强制可见。
    let intent = StartupIntent {
        visible: !cli.startup || !cli.send_to.is_empty() || cli.preferences.is_some(),
        dialog: if cli.send_to.len() == 1 {
            Some(dialog_for_sent_path(&cli.send_to[0]))
        } else {
            None
        },
        settings_page: cli.preferences,
        batch: if cli.send_to.len() > 1 {
            cli.send_to.clone()
        } else {
            Vec::new()
        },
    };
    let start_visible = intent.visible;

    let (_tray, tray_rx) = init_tray();
    let (hotkey_tx, hotkey_rx) = mpsc::channel();
    let (ipc_tx, ipc_rx) = mpsc::channel();
    IPC_TX.set(ipc_tx).ok();
    let (notify_tx, notify_rx) = mpsc::channel();
    // 批次 Q：热键在设置里可改，而注册发生在 `LauncherApp::new` 之前（热键线程自己那条消息循环），
    // 所以这里先读一次 settings.json。多读一次这个几十十字节的文件，换掉一个"配置要穿过
    // intent 才能到线程"的接线，值得。批次 R 起这一次读还要带出补字符热键。
    let (configured, broken) = source::application::load_settings();
    match &broken {
        Some(Corrupted::Kept(p)) => eprintln!(
            "配置损坏：热键线程用的 settings.json 解析失败，已隔离为 {}，本轮按出厂热键注册",
            p.display()
        ),
        // 这一次读只喂热键线程、不写盘，所以 `Lost` 在这里不需要停手；`LauncherApp::new`
        // 里那次读才挂着"留证没成功就不写"的门禁。
        Some(Corrupted::Lost) => {
            eprintln!(
                "配置损坏：settings.json 解析失败且留证没成功 ⇒ 热键线程按出厂热键注册（这一次只读不写）"
            )
        }
        None => {}
    }
    spawn_hotkey_thread(
        hotkey_tx,
        configured.toggle_hotkey,
        configured.typeback_hotkey,
    );

    // 批次 F1 的主题在这里解析一次：默认窗口高度要按主题行高算（一屏正好 10 行），
    // 同一份再传进 `LauncherApp`——`Themes\` 只读一遍，坏主题文件在日志里也只记一次。
    // 2026-10-24 用户需求：旧默认 400px 只装得下 6 行，现在按 `LIST_ROWS` 算。
    let (start_theme, start_theme_choices) = theme::load(&configured.theme);
    let start_height = window_height(
        &start_theme,
        configured.row_density,
        configured.appearance.status_bar,
    );
    let start_width = source::application::clamp_window_width(configured.appearance.width);

    let mut wgpu_options = eframe::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(create_new) = &mut wgpu_options.wgpu_setup {
        // E-18 阶段二硬前置（2026-10-24）：只留 GL，不再带 DX12 兜底。
        // 透明与否的决定性变量是呈现路径，不是 winit 样式：DX12 的 DXGI 交换链
        // 在 HWND 上不带 alpha，GL 走 WGL 像素格式 + GDI 重定向面，DWM 认它的 alpha。
        // 本机实测（`winprobe transparent gl`）：GL 不补任何样式也透、圆角干净。
        // 代价：WGL 上下文建不起来就没有后备（本机 Intel HD 630 可用）。
        create_new.instance_descriptor.backends = eframe::wgpu::Backends::GL;
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([start_width, start_height])
        .with_title("")
        .with_decorations(false)
        .with_always_on_top()
        .with_transparent(true)
        .with_visible(start_visible);
    // 批次 F8（FR-UIX-03 RememberPosition）：上次的位置还要在显示器上才用。
    // 拔掉副屏、改分辨率之后，旧坐标可能整块落在不存在的区域——那样窗口会"启动成功但看不见"。
    if configured.appearance.remember_position {
        match configured.appearance.last_pos {
            Some([x, y]) if pos_on_screen([x, y]) => {
                viewport = viewport.with_position([x as f32, y as f32]);
                eprintln!("窗口：使用上次位置 ({x}, {y})");
            }
            Some([x, y]) => {
                eprintln!("窗口：上次位置 ({x}, {y}) 已不在任何显示器上 ⇒ 落回系统默认位置");
            }
            None => {}
        }
    }

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };

    eframe::run_native(
        "egui Launcher",
        options,
        Box::new(move |cc: &eframe::CreationContext<'_>| {
            // 字体不再在这里装：批次 F1 起由 `sync_theme` 按主题的 `font_name` 一次装好
            // （`LauncherApp::new` 末尾调用，`CTX` 已经 set）。CJK 回退仍在同一条路径里。
            CTX.set(cc.egui_ctx.clone()).ok();
            // E-05：取本机顶层 HWND，on_show 对它发 WM_IME_CONTROL 切英文模式
            let self_hwnd = cc.window_handle().ok().and_then(|h| match h.as_raw() {
                RawWindowHandle::Win32(w) => Some(HWND(w.hwnd.get() as *mut _)),
                _ => None,
            });
            if self_hwnd.is_none() {
                eprintln!("IME: 无法获取本机 HWND，E-05 英文模式切换将跳过");
            }
            Ok(Box::new(LauncherApp::new(
                tray_rx,
                hotkey_rx,
                ipc_rx,
                notify_tx,
                notify_rx,
                self_hwnd,
                intent,
                start_theme,
                start_theme_choices,
            )))
        }),
    )
}

#[cfg(test)]
mod nav_tests {
    use super::*;
    // 批次 Q 这里有一句单独的 `use crate::source::application::HKF_WIN;`，理由是"只有测试断言
    // 录制路径不许产出 Win 位，运行期没人读它"。批次 R 把这个前提推翻了（补字符卡片上的复选框
    // 就在运行期读它），所以改成靠上面那条 glob 带进来。

    #[test]
    fn bare_digit_yields_to_calculator_when_calc_source_is_on() {
        assert!(bare_digit_allowed("", false));
        assert!(
            !bare_digit_allowed("", true),
            "计算器开着时第一个数字要进表达式"
        );
        assert!(
            !bare_digit_allowed("1+2", false),
            "有输入时数字本来就是查询文本"
        );
        assert!(!bare_digit_allowed("   ", true));
    }

    /// 2026-10-24 用户报障：空输入框按 Shift+2 想打 @，被裸数字那档当成 `2` 执行了。
    /// 根因是 egui 的 `consume_key` 走 `matches_logically`，**多余的 Shift 会被忽略**
    ///（egui-0.36.2 input_state/mod.rs:722）⇒ 裸数字分支必须先吃掉 SHIFT 层。
    /// 这条同时钉两件事：① egui 确实有这个语义（将来升级变了，闸口是否还有必要就该重估）；
    /// ② 「先吃 SHIFT 层、再判裸数字」的次序确实把 Shift+2 漏给输入框。
    #[test]
    fn shift_digit_is_left_for_text_instead_of_bare_digit() {
        use egui::{Event, Modifiers};
        fn key(key: egui::Key, modifiers: Modifiers) -> Event {
            Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }
        }
        let run = |events: Vec<Event>| {
            let ctx = egui::Context::default();
            let mut seen: (Option<u8>, Option<u8>) = (None, None);
            let mut out =
                ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                    let shift_layer = pressed_digit(ui, Modifiers::SHIFT);
                    let bare = if bare_digit_allowed("", false) {
                        pressed_digit(ui, Modifiers::NONE)
                    } else {
                        None
                    };
                    seen = (shift_layer, bare);
                });
            // FullOutput 的纹理增量不落地就会在 drop 时 panic（epaint textures.rs:337），单测里清掉。
            out.textures_delta.clear();
            seen
        };
        assert_eq!(
            run(vec![key(egui::Key::Num2, Modifiers::SHIFT)]),
            (Some(2), None),
            "Shift+2（@）必须被 SHIFT 层吃掉，裸数字档不许再取走"
        );
        assert_eq!(
            run(vec![key(egui::Key::Num2, Modifiers::NONE)]),
            (None, Some(2)),
            "对照：裸 2 仍归裸数字档（原行为不变）"
        );
        assert!(
            Modifiers::SHIFT.matches_logically(Modifiers::NONE),
            "egui 语义：模式 NONE 忽略多余 Shift —— 这就是必须显式闸的原因"
        );
    }

    #[test]
    fn digit_zero_means_tenth_row_and_one_means_first() {
        assert_eq!(digit_index(1, 20), Some(0));
        assert_eq!(digit_index(9, 20), Some(8));
        assert_eq!(digit_index(0, 20), Some(9), "规格把 0 定为第 10 行");
    }

    #[test]
    fn digit_out_of_range_is_none() {
        assert_eq!(digit_index(3, 2), None, "只有两行时 3 不该指到任何行");
        assert_eq!(digit_index(0, 9), None, "第 10 行不存在");
        assert_eq!(digit_index(1, 0), None, "空结果不执行");
    }

    #[test]
    fn commit_word_takes_first_word_and_strips_row_digit() {
        assert_eq!(commit_word("jsq", false), "jsq");
        assert_eq!(
            commit_word("  JSQ ", false),
            "jsq",
            "首尾空白与大小写不进别名"
        );
        assert_eq!(
            commit_word("jsq5", true),
            "jsq",
            "数字键执行时行号不是触发词"
        );
        assert_eq!(
            commit_word("jsq5", false),
            "jsq5",
            "非数字键执行不剥——那可能就是用户要的词"
        );
        assert_eq!(
            commit_word("visual studio", false),
            "visual",
            "参数词不进别名"
        );
        assert_eq!(commit_word("", false), "");
    }

    #[test]
    fn page_target_clamps_at_both_ends() {
        assert_eq!(page_target(0, 9, 100, true), 9);
        assert_eq!(
            page_target(95, 9, 100, true),
            99,
            "向下夹到最后一行，不越过"
        );
        assert_eq!(page_target(3, 9, 100, false), 0, "向上夹到第一行");
        assert_eq!(
            page_target(0, 9, 0, true),
            0,
            "空结果不塌陷（len-1 的减法不许下溢）"
        );
    }

    // ---- 头段（计算器/搜索/提示）并入列表后的行模型（批次 S / T）----

    #[test]
    fn row_count_adds_the_head_rows_only_when_present() {
        assert_eq!(row_count(0, 0), 0, "空清单空查询：一行都没有");
        assert_eq!(row_count(3, 0), 3);
        assert_eq!(
            row_count(0, 1),
            1,
            "算式没有条目命中时，列表里就只剩计算器那一行"
        );
        assert_eq!(row_count(3, 1), 4);
        assert_eq!(row_count(0, 2), 2, "头段可以不止一行（未来多源并列）");
        assert_eq!(row_count(3, 2), 5);
    }

    #[test]
    fn row_at_puts_the_head_rows_first_and_offsets_the_items() {
        // 下标表照 MatchIndex::search 的真实产出构造（名次 → self.items 下标）。
        let results = [7, 2, 9];
        assert_eq!(row_at(0, results.len(), 1), Some(Row::Head(0)));
        assert_eq!(
            row_at(1, results.len(), 1),
            Some(Row::Item(0)),
            "第 1 行是第 0 名"
        );
        assert_eq!(row_at(3, results.len(), 1), Some(Row::Item(2)));
        assert_eq!(
            row_at(4, results.len(), 1),
            None,
            "越界不给行，调用方什么都不做"
        );
        // 头段不在场时，合并行号就是名次——这条是"批次 S 之前的行为一个字没变"的证据。
        assert_eq!(row_at(0, results.len(), 0), Some(Row::Item(0)));
        assert_eq!(row_at(2, results.len(), 0), Some(Row::Item(2)));
        assert_eq!(row_at(3, results.len(), 0), None);
        assert_eq!(
            row_at(0, 0, 1),
            Some(Row::Head(0)),
            "只有计算器一行时第 0 行仍在"
        );
        // 头段两行（假想的多源并列）：第 0/1 行归头段，应用从第 2 行起。
        assert_eq!(row_at(1, results.len(), 2), Some(Row::Head(1)));
        assert_eq!(row_at(2, results.len(), 2), Some(Row::Item(0)));
        assert_eq!(row_at(4, results.len(), 2), Some(Row::Item(2)));
    }

    // 批次 A10：`drop_hinted` 的判据已并入 `source::dispatch` 第 4 段，
    // 对应单测 `a_custom_hint_absorbs_the_same_item_from_results` 跟着搬到 `src/source/mod.rs`。

    #[test]
    fn only_custom_head_rows_carry_an_item_identity() {
        use crate::source::application::{ItemKind, LaunchItem};
        use crate::source::calc::CalcHit;
        use crate::source::custom::{CustomHint, CustomHit};
        use crate::source::websearch::EngineHint;
        let item = LaunchItem {
            name: "ping".into(),
            keyword: "ping".into(),
            kind: ItemKind::Command,
            path: "cmd.exe".into(),
            arguments: "/k ping {query}".into(),
            ..Default::default()
        };
        let uid = item.uid();
        let s = String::new();
        let hint = CustomHint {
            uid: uid.clone(),
            title: s.clone(),
            subtitle: s.clone(),
            completion: s.clone(),
            command: s.clone(),
        };
        let hit = CustomHit {
            uid: uid.clone(),
            name: item.name.clone(),
            entry: item.clone(),
            subtitle: s.clone(),
        };
        assert_eq!(HeadRow::CustomHint(hint).custom_uid(), Some(uid.as_str()));
        assert_eq!(HeadRow::Custom(hit).custom_uid(), Some(uid.as_str()));
        // 没有条目身份的头段不许被编辑/删除接管——旧口径，漂了就等于给计算器行挂上删除键。
        let calc = CalcHit {
            title: s.clone(),
            subtitle: s.clone(),
            copy: s.clone(),
        };
        assert_eq!(HeadRow::Calc(calc).custom_uid(), None);
        let eng = EngineHint {
            engine_id: s.clone(),
            title: s.clone(),
            subtitle: s.clone(),
            completion: s,
        };
        assert_eq!(HeadRow::Hint(eng).custom_uid(), None);
        // 文件行是从磁盘现出来的，没有清单身份 ⇒ 编辑/删除/置顶那几档不许接管它；
        // 但 `file_hit()` 必须是它唯一的取行途径（Tab 进目录、Ctrl+D 定位靠这个闸门）。
        let file = source::filesearch::FileHit {
            title: "报告.docx".into(),
            path: r"C:\u\报告.docx".into(),
            is_dir: false,
            subtitle: r"C:\u".into(),
        };
        let file_row = HeadRow::File(file.clone());
        let custom_row = HeadRow::Custom(CustomHit {
            uid,
            name: "ping".into(),
            entry: item,
            subtitle: String::new(),
        });
        assert_eq!(file_row.custom_uid(), None);
        assert_eq!(file_row.file_hit(), Some(&file));
        assert_eq!(custom_row.file_hit(), None, "只有文件行能被 file_hit 取走");
    }

    #[test]
    fn item_rank_refuses_head_rows_and_renumbers_the_rest() {
        // 空格四档与"编辑/删除/置顶/开目录"这四档都要它：选中头段 ⇒ None（不接管）。
        assert_eq!(item_rank(0, 1), None);
        assert_eq!(item_rank(0, 0), Some(0), "头段不在场，第 0 行就是第 0 名");
        assert_eq!(item_rank(1, 1), Some(0));
        assert_eq!(item_rank(5, 1), Some(4));
        // 关键一行：头段两行时 selected=1 也算头段，减法不许下溢。
        assert_eq!(item_rank(1, 2), None);
        assert_eq!(item_rank(2, 2), Some(0));
    }

    #[test]
    fn exclusive_needs_a_space_after_the_first_word() {
        use source::websearch::Exclusive;
        use source::websearch::exclusive_for;
        let engines = source::websearch::builtin_engines();
        // 首词后没空格 ⇒ 普通搜索（`g` 可能是 `git` 的开头）。
        assert_eq!(exclusive_for(&engines, "g"), Exclusive::NotOurs);
        assert_eq!(exclusive_for(&engines, "G"), Exclusive::NotOurs);
        // 首词 + 空格 = 独占：没词只剩补全提示，有词就是可执行行。
        assert!(matches!(exclusive_for(&engines, "g "), Exclusive::Hints(_)));
        assert!(matches!(
            exclusive_for(&engines, "G foo"),
            Exclusive::Rows(_)
        ));
        // 前缀/别的词不是引擎关键字。
        assert_eq!(exclusive_for(&engines, "goo x"), Exclusive::NotOurs);
        assert_eq!(exclusive_for(&engines, "zzz "), Exclusive::NotOurs);
    }

    #[test]
    fn digit_rows_land_on_the_head_rows_first() {
        // 数字键走的是「合并行号」：计算器在场时按 1 复制数字，按 2 才是原来的第一条。
        let results = [7, 2];
        let head_len = 1;
        let len = row_count(results.len(), head_len);
        assert_eq!(
            row_at(digit_index(1, len).unwrap(), results.len(), head_len),
            Some(Row::Head(0))
        );
        assert_eq!(
            row_at(digit_index(2, len).unwrap(), results.len(), head_len),
            Some(Row::Item(0))
        );
        assert_eq!(
            row_at(digit_index(3, len).unwrap(), results.len(), head_len),
            Some(Row::Item(1))
        );
        assert_eq!(digit_index(4, len), None, "共三行时按 4 不该指到任何行");
    }

    #[test]
    fn page_rows_uses_window_height_but_never_below_one() {
        // 批次 F6：Light（内边距 14）的顶栏 + 上下内边距 = 63 + 15 = 78，页高由窗口高现算。
        let chrome = list_chrome_h(&theme::Theme::light());
        assert_eq!(chrome, 78.0, "Light 的列表外框占用：顶 63 + 底 15");
        // 甲档（默认）步进 34 + 2 = 36：438 高的窗口正好十行，手动缩到 400 就只剩八行。
        assert_eq!(page_rows(438.0, 36.0, chrome), 10);
        assert_eq!(
            page_rows(400.0, 36.0, chrome),
            8,
            "窗口比默认矮时按实际高度给行数"
        );
        assert_eq!(
            page_rows(20.0, 36.0, chrome),
            1,
            "拉到比列表还矮时仍要能动一行"
        );
        assert_eq!(
            page_rows(400.0, 0.0, chrome),
            322,
            "行距为 0 时按 1px 兜底，不许除零"
        );
    }

    /// 批次 F6 真机抓到的缺陷钉住：输入框改成定宽铺满之后，顶栏那条缝只剩 `item_spacing.x`，
    /// 手柄两侧一内缩就归零 ⇒ 「拖顶栏空白移动窗口」静默失效（日志里 `搜索页顶栏空白` 零命中）。
    /// 这一条不测渲染，只测"缝有没有留够"——`TOP_DRAG_GAP` 哪天被删掉，这里就该红。
    #[test]
    fn top_drag_strip_stays_wide_enough_to_hit() {
        // egui 默认行间距 8；主题把 spacing 调紧到 4 时也要还剩 12px 可命中。
        assert!(
            top_drag_strip_w(8.0) >= 8.0,
            "默认行间距下顶栏手柄只有 {} px",
            top_drag_strip_w(8.0)
        );
        assert!(
            top_drag_strip_w(4.0) >= 8.0,
            "紧行间距下顶栏手柄只有 {} px",
            top_drag_strip_w(4.0)
        );
        // 反证：没有预留间隙时，8px 的行间距被两侧内缩吃到只剩 4px——就是本次修掉的形状。
        assert!(
            8.0 - 2.0 * TOP_DRAG_INSET < 8.0,
            "内缩若小到让「零预留」也够用，这条反证就该换掉（当前内缩 {}）",
            TOP_DRAG_INSET
        );
    }

    #[test]
    fn ten_rows_mode_caps_results_and_counts_head_rows() {
        let mut plan = Plan::default();
        plan.results = (0..30).collect();
        cap_plan_rows(&mut plan, LIST_ROWS);
        assert_eq!(
            plan.results,
            (0..10).collect::<Vec<_>>(),
            "没有头段时列前 10 条"
        );
        // 头段行与普通结果共用这 10 行名额。
        assert_eq!(result_room(2, LIST_ROWS), 8);
        assert_eq!(
            result_room(LIST_ROWS, LIST_ROWS),
            0,
            "头段自己占满就不再放结果"
        );
        assert_eq!(result_room(LIST_ROWS + 2, LIST_ROWS), 0, "头段超了也不下溢");
    }

    #[test]
    fn default_window_height_gives_a_full_ten_row_page() {
        let light = theme::Theme::light();
        let chrome = list_chrome_h(&light);
        // 四档各自的窗口高，且每一档都要正好装下 LIST_ROWS 行（2026-10-24 用户的「一屏十行」）。
        for (density, height, step) in [
            (RowDensity::Snug, 438.0, 36.0),
            (RowDensity::Roomy, 498.0, 42.0),
            (RowDensity::Dense, 318.0, 24.0),
            (RowDensity::Theme, 658.0, 58.0),
        ] {
            let h = default_window_height(&light, density);
            assert_eq!(h, height, "{} 档窗口高", density.as_str());
            assert_eq!(
                page_rows(h, step, chrome),
                LIST_ROWS,
                "{} 档默认高度 {h} 要正好装下 {LIST_ROWS} 行（步进 {step}）",
                density.as_str()
            );
            assert!(
                h >= MIN_H as f32 && h <= 900.0,
                "夹在缩放下限与上限之间：{h}"
            );
        }
        // 行高被主题写成 0（只有「跟随主题」档会读它）时按最小窗口兜底，不许算出比 MIN_H 还矮的窗口。
        let mut flat = theme::Theme::light();
        flat.row_height = 0.0;
        assert_eq!(
            default_window_height(&flat, RowDensity::Theme),
            MIN_H as f32
        );
        // 换主题仍要重算：主题供 `padding`（内边距）与「跟随主题」档的行高。
        // 走纯解析那条路（`resolve_with` + 空表），单测不碰磁盘与注册表。
        let none = std::collections::HashMap::new();
        // Compact 内边距 10、行高 44 ⇒ 跟随主题 59 + 10×48 + 11 = 550；甲档 59 + 10×36 + 11 = 430。
        let compact = theme::resolve_with("Compact", &none, false);
        assert_eq!(default_window_height(&compact, RowDensity::Theme), 550.0);
        assert_eq!(default_window_height(&compact, RowDensity::Snug), 430.0);
        // Modern 内边距 16、行高 56 ⇒ 65 + 10×60 + 17 = 682。
        let modern = theme::resolve_with("Modern", &none, false);
        assert_eq!(default_window_height(&modern, RowDensity::Theme), 682.0);
    }

    #[test]
    fn window_height_stacks_the_status_bar_on_top_of_ten_rows() {
        let light = theme::Theme::light();
        // 状态栏是叠加项，不是从十行里扣：开关只在 438 上加/减一整块。
        assert_eq!(window_height(&light, RowDensity::Snug, false), 438.0);
        assert_eq!(
            window_height(&light, RowDensity::Snug, true),
            438.0 + STATUS_BAR_BLOCK
        );
        assert_eq!(
            window_height(&light, RowDensity::Theme, true),
            default_window_height(&light, RowDensity::Theme) + STATUS_BAR_BLOCK
        );
        assert_eq!(STATUS_BAR_BLOCK, STATUS_BAR_H + STATUS_BAR_GAP);
    }

    #[test]
    fn status_left_text_puts_the_next_escape_first() {
        // 目录浏览三档 Esc 文案各说各的真实落点：退目录 / 去过滤词 / 盘根清空。
        assert_eq!(
            status_left_text(
                Some(("D:\\a\\b\\", "")),
                false,
                TabMode::Navigate,
                true
            ),
            "目录 D:\\a\\b\\ · Esc 返回上级"
        );
        assert_eq!(
            status_left_text(Some(("D:\\a\\b\\", "re")), false, TabMode::Complete, false),
            "目录 D:\\a\\b\\ · Esc 清除筛选"
        );
        assert_eq!(
            status_left_text(Some(("D:\\", "")), false, TabMode::Complete, false),
            "目录 D:\\ · Esc 清空"
        );
        assert_eq!(
            status_left_text(None, true, TabMode::Complete, true),
            "文件模式 · Esc 清空"
        );
        // 平时两档 Tab 口径，各自原样出场；「基本」只在平时那一档追加。
        assert_eq!(
            status_left_text(None, false, TabMode::Complete, false),
            "Tab 补全 / 下一项"
        );
        assert_eq!(
            status_left_text(None, false, TabMode::Complete, true),
            "Tab 补全 / 下一项 · 分类：基本"
        );
        assert_eq!(
            status_left_text(None, false, TabMode::Navigate, false),
            "Tab 下一项 · Shift+Tab 上一项"
        );
    }

    #[test]
    fn enter_action_labels_say_what_enter_will_do() {
        assert_eq!(enter_action_label(None), "运行");
        let calc = HeadRow::Calc(source::calc::CalcHit {
            title: "= 2".into(),
            subtitle: String::new(),
            copy: "2".into(),
        });
        assert_eq!(enter_action_label(Some(&calc)), "复制数字");
        let search = HeadRow::Search(source::websearch::SearchHit {
            engine_id: "g".into(),
            title: "Google「x」".into(),
            subtitle: String::new(),
            url: "https://example.com/?q=x".into(),
        });
        assert_eq!(enter_action_label(Some(&search)), "搜索");
        let hint = HeadRow::Hint(source::websearch::EngineHint {
            engine_id: "g".into(),
            title: String::new(),
            subtitle: String::new(),
            completion: String::new(),
        });
        assert_eq!(enter_action_label(Some(&hint)), "查看");
        let custom = HeadRow::Custom(source::custom::CustomHit {
            uid: "u".into(),
            name: "ping".into(),
            entry: LaunchItem::default(),
            subtitle: String::new(),
        });
        assert_eq!(enter_action_label(Some(&custom)), "运行");
        let custom_hint = HeadRow::CustomHint(source::custom::CustomHint {
            uid: "u".into(),
            title: String::new(),
            subtitle: String::new(),
            completion: "ping ".into(),
            command: String::new(),
        });
        assert_eq!(enter_action_label(Some(&custom_hint)), "补全参数");
        let file = HeadRow::File(source::filesearch::FileHit {
            title: "a.txt".into(),
            path: "D:\\a.txt".into(),
            is_dir: false,
            subtitle: String::new(),
        });
        assert_eq!(enter_action_label(Some(&file)), "打开");
        let term = HeadRow::Terminal(source::terminal::TermHit {
            shell: source::terminal::TermShell::Cmd,
            command: "ipconfig".into(),
            title: "ipconfig".into(),
            subtitle: String::new(),
        });
        assert_eq!(enter_action_label(Some(&term)), "执行命令");
        let term_hint = HeadRow::TermHint(source::terminal::TermHint {
            title: String::new(),
            subtitle: String::new(),
        });
        assert_eq!(enter_action_label(Some(&term_hint)), "查看");
        let cheat = HeadRow::Cheat(source::cheatsheet::CheatHit {
            entry: &source::cheatsheet::ENTRIES[0],
        });
        assert_eq!(enter_action_label(Some(&cheat)), "查看");
        let cheat_hint = HeadRow::CheatHint(source::cheatsheet::CheatHint {
            title: String::new(),
            subtitle: source::cheatsheet::NO_MATCH_TOAST,
        });
        assert_eq!(enter_action_label(Some(&cheat_hint)), "查看");
        let scope = HeadRow::ScopeTags(source::application::ScopeTagHint {
            title: String::new(),
            subtitle: String::new(),
        });
        assert_eq!(enter_action_label(Some(&scope)), "查看");
    }

    #[test]
    fn browse_esc_strips_the_filter_then_walks_up() {
        // 先去掉过滤词（留在本目录），再逐级上溯；盘根交给调用方的"清空查询"。
        assert_eq!(
            browse_parent_of("D:\\a\\b\\re").as_deref(),
            Some("D:\\a\\b\\")
        );
        assert_eq!(browse_parent_of("D:\\a\\b\\").as_deref(), Some("D:\\a\\"));
        assert_eq!(browse_parent_of("D:\\a\\").as_deref(), Some("D:\\"));
        assert_eq!(browse_parent_of("D:\\").as_deref(), None);
        // 不是浏览态（索引搜索 / 普通查询）：Esc 不该把查询换成一条路径。
        assert_eq!(browse_parent_of("'报告"), None);
        assert_eq!(browse_parent_of("note"), None);
    }

    #[test]
    fn row_badges_agree_with_digit_keys() {
        // 徽标和 digit_index 必须说的是同一件事：按下徽标上的数字，落点就是那一行。
        for idx in 0..10 {
            let label = row_num_label(idx).unwrap_or_else(|| panic!("第 {} 行该有号", idx + 1));
            let digit: u8 = label.parse().unwrap();
            assert_eq!(
                digit_index(digit, 10),
                Some(idx),
                "徽标 {label} 与数字键落点不一致"
            );
        }
        assert_eq!(row_num_label(10), None, "第 11 行没有对应按键，不该标号");
    }

    /// 批次 Q 的映射表。这条 test 是被一条真警告逼出来的：egui 的枚举叫 `Key::Enter` 不叫
    /// `Key::Return`，写成 `Return` 时 rustc 把它当成"匹配任意值"的新绑定，F1–F24 全部失效，
    /// 而编译照样通过 ⇒ 只能靠断言钉住每一段的落点。
    #[test]
    fn vk_map_covers_exactly_the_allowed_keys() {
        use egui::Key::*;
        assert_eq!(vk_from_egui_key(A), Some(0x41));
        assert_eq!(vk_from_egui_key(Z), Some(0x5A));
        assert_eq!(vk_from_egui_key(Num0), Some(0x30));
        assert_eq!(vk_from_egui_key(Num9), Some(0x39));
        assert_eq!(vk_from_egui_key(Space), Some(0x20));
        assert_eq!(vk_from_egui_key(Enter), Some(0x0D), "回车是 Key::Enter");
        // 批次 R：` 是这套键表里唯一被允许裸按的键，录制端必须认得它（winit 那边叫 Backquote）。
        assert_eq!(vk_from_egui_key(Backtick), Some(0xC0), "` = VK_OEM_3");
        assert_eq!(vk_from_egui_key(F1), Some(0x70));
        assert_eq!(vk_from_egui_key(F12), Some(0x7B));
        assert_eq!(vk_from_egui_key(F13), Some(0x7C));
        assert_eq!(vk_from_egui_key(F24), Some(0x87));
        // 范围外一律 None：录制端把这种按键当"还没按到有效键"继续等。
        for k in [
            Escape, Tab, Backspace, Delete, ArrowUp, ArrowDown, ArrowLeft, ArrowRight, PageUp,
            PageDown, Home, End, Insert, SuperLeft, SuperRight,
        ] {
            assert_eq!(vk_from_egui_key(k), None, "{k:?} 不在可录集合");
        }
        // 后两条是"按 Win 键本身不许被当成主键"：录制时先按 Win 再按 I，走的是
        // "还没按到有效键，继续等"那条分支（`hotkey_frame` 的 `other =>` 里 `return`）。
    }

    /// 修饰键只取 `ctrl/alt/shift`。输入的构造严格照上游在 Windows 上实际给的东西：`command`
    /// 恒等于 `ctrl`（`egui-winit-0.36.2/src/lib.rs:496-500`）、`mac_cmd` 在非 Mac 恒 false（`:495`）。
    /// ~~本测试首版写成「`command` 在 Windows 上就是 Win 键」，按那个虚构语义造输入并断言结果含
    /// `HKF_WIN`~~【2026-10-19 更正：那条断言把"录 Ctrl 白送一个 Win"的缺陷钉成了期望值，
    /// 58/58 全绿也照不出来 ⇒ 它验证的是我的假设，不是库的行为】。
    #[test]
    fn spec_from_press_builds_mods_from_the_three_real_modifier_names() {
        let m = |ctrl, alt, shift| egui::Modifiers {
            ctrl,
            alt,
            shift,
            command: ctrl, // 上游就是这么镜像的，不是我在这里替它编
            mac_cmd: false,
        };
        let plain = spec_from_press(egui::Key::A, m(false, false, false)).unwrap();
        assert_eq!(
            plain.mods, HKF_NOREPEAT,
            "无修饰键时只剩 NOREPEAT ⇒ check() 会拒"
        );
        let all = spec_from_press(egui::Key::I, m(true, true, true)).unwrap();
        assert_eq!(
            all.mods,
            HKF_NOREPEAT | HKF_CONTROL | HKF_ALT | HKF_SHIFT,
            "Ctrl+Alt+Shift+I 不许再多 Win"
        );
        assert_eq!(
            spec_from_press(egui::Key::I, m(true, false, false))
                .unwrap()
                .mods,
            HKF_NOREPEAT | HKF_CONTROL,
            "用户实测点名的就是这一条：录 Ctrl+I 得 Ctrl+I"
        );
        assert!(spec_from_press(egui::Key::Tab, m(true, false, false)).is_none());
    }

    /// 穷举 `ctrl/alt/shift/command/mac_cmd` 的全部形态：**没有任何一种**能让录制路径产出
    /// `HKF_WIN`。判据刻意不依赖"`command` 等于 `ctrl`"这条上游行为——万一哪天 egui 改了镜像
    /// 规则（把 Win 真的塞进 `command`），这里也不会变成"缺陷被重新钉成期望值"。
    #[test]
    fn spec_from_press_can_never_produce_win_mod_bit() {
        for ctrl in [false, true] {
            for alt in [false, true] {
                for shift in [false, true] {
                    for command in [false, true] {
                        for mac_cmd in [false, true] {
                            let m = egui::Modifiers {
                                ctrl,
                                alt,
                                shift,
                                command,
                                mac_cmd,
                            };
                            let s = spec_from_press(egui::Key::A, m).unwrap();
                            assert_eq!(
                                s.mods & HKF_WIN,
                                0,
                                "Modifiers {m:?} 不该推出 Win，实得 {}",
                                s.display()
                            );
                            let want = HKF_NOREPEAT
                                | if ctrl { HKF_CONTROL } else { 0 }
                                | if alt { HKF_ALT } else { 0 }
                                | if shift { HKF_SHIFT } else { 0 };
                            assert_eq!(s.mods, want, "修饰位只许由 ctrl/alt/shift 决定（{m:?}）");
                        }
                    }
                }
            }
        }
    }

    /// 冲突表必须是"用户真能录到的组合"，且绝不能把出厂键算成自冲突——否则设置页点
    /// 「恢复默认」会被自己的预检挡掉。
    #[test]
    fn local_table_holds_only_recordable_combos_and_not_the_default() {
        for (mods, vk) in LOCAL_TAKEN {
            let spec = HotkeySpec {
                mods: *mods | HKF_NOREPEAT,
                vk: *vk,
            };
            assert!(
                spec.check().is_ok(),
                "表里 {}/0x{vk:02X} 根本不是可录组合",
                spec.display()
            );
            assert!(
                !spec.conflicts_local(&[(HotkeySpec::DEFAULT.mods, HotkeySpec::DEFAULT.vk)]),
                "出厂组合不许出现在自己的冲突表里"
            );
        }
        // 抽查几条必须挡得住的（这些功能都有 Ctrl 版本，热键不能同名）。
        for vk in [0x4E, 0x45, 0x49, 0x44, 0x50, 0x4A, 0x4B] {
            assert!(
                HotkeySpec {
                    mods: HKF_CONTROL | HKF_NOREPEAT,
                    vk
                }
                .conflicts_local(LOCAL_TAKEN),
                "Ctrl+{} 应当被本地表挡住",
                char::from_u32(vk).unwrap()
            );
        }
        assert!(
            HotkeySpec {
                mods: HKF_ALT | HKF_NOREPEAT,
                vk: 0x35
            }
            .conflicts_local(LOCAL_TAKEN),
            "Alt+5 执行第 5 行"
        );
        assert!(
            !HotkeySpec {
                mods: HKF_ALT | HKF_NOREPEAT,
                vk: 0x49
            }
            .conflicts_local(LOCAL_TAKEN),
            "Alt+I 本程序没用"
        );
        assert!(!HotkeySpec::DEFAULT.conflicts_local(LOCAL_TAKEN));
        // 反向也要钉住：这张表只挡"本程序真绑了的键"，不挡一切眼熟的编辑键。
        // Ctrl+A/Ctrl+Z 之类在本程序里没绑（表里当初多写过四条，全是凭印象），系统空着就该让人用。
        for vk in [0x41u32, 0x42, 0x48, 0x5A] {
            assert!(
                !HotkeySpec {
                    mods: HKF_CONTROL | HKF_NOREPEAT,
                    vk
                }
                .conflicts_local(LOCAL_TAKEN),
                "Ctrl+{} 本程序没绑，不该出现在表里",
                char::from_u32(vk).unwrap()
            );
        }
        // 【2026-10-24 批次 F6】这一句原本连着上面那个循环里的 `0x4D` 一起写："Ctrl+M 本程序
        // 没绑"。批次 F6 把顶栏「＋ 新建」换成 ☰ 菜单并给菜单配了 Ctrl+M，那句从此不成立 ⇒
        // 0x4D 从"没绑"名单挪到这里，改成钉住"绑了就必须进表"（表与 `consume_key` 一一对应）。
        assert!(
            HotkeySpec {
                mods: HKF_CONTROL | HKF_NOREPEAT,
                vk: 0x4D
            }
            .conflicts_local(LOCAL_TAKEN),
            "Ctrl+M 开 ☰ 主菜单，必须被本地表挡住"
        );
    }
}

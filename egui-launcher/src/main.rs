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
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND,
    LPARAM, LRESULT, POINT, RECT, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, SetStdHandle};
use windows::core::PCWSTR;
// FR-SRC-10：计算器结果行 Enter ＝「把纯数字写进剪贴板」。三件套在 DataExchange，
// GlobalAlloc/GlobalLock 在 Memory（该模块没有 feature 门，已随 Win32_Foundation 白送）。
// 批次 E4（FR-ACT-05）：「Preferred DropEffect」是**运行时注册的自定义格式**，
// 名字要交给 RegisterClipboardFormatW 换 id，没有现成常量。
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
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
    DROPFILES, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT, FOF_WANTNUKEWARNING,
    FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FO_COPY, FO_DELETE, FileOpenDialog,
    IFileOpenDialog, SEE_MASK_FLAG_NO_UI,
    SHFILEOPSTRUCTW, SHFileOperationW, SHObjectProperties, SHOP_FILEPATH, SHELLEXECUTEINFOW,
    SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW,
    SIGDN_FILESYSPATH, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CallNextHookEx, ChangeWindowMessageFilterEx,
    CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GetClassNameW, GetCursorPos,
    GetForegroundWindow, GetMessageW, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, HHOOK, HWND_BROADCAST,
    HWND_MESSAGE, IsIconic, IsWindow, KBDLLHOOKSTRUCT, KillTimer, MSG, MSGFLT_ALLOW, PostMessageW, PostQuitMessage,
    RegisterClassExW, SC_MONITORPOWER, SMTO_ABORTIFHUNG, SW_HIDE, SW_RESTORE,
    SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED,
    SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SendMessageTimeoutW, SetForegroundWindow,
    SetTimer, SetWindowPos, SetWindowsHookExW, ShowWindow, TranslateMessage, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_COPYDATA, WM_DESTROY, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_SYSCOMMAND, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WM_TIMER, WNDCLASSEXW,
};

/// 全局呼出 / 隐藏那一组（`RegisterHotKey` 的 id）。批次 R 起系统里同时最多装着两个热键，
/// id 必须分开：`WM_HOTKEY` 的 `wparam` 就是它，靠它区分"该呼出窗口"还是"该补一个字符"。
const HOTKEY_TOGGLE: i32 = 1;
/// 补字符热键（批次 R，2026-10-20 用户追加需求）：按下它把被裸键吞掉的那个字符投给焦点窗口。
const HOTKEY_TYPEBACK: i32 = 2;
/// 副呼出热键（批次 G1，FR-WIN-02 的「次热键」）：与 `HOTKEY_TOGGLE` **做同一件事**，
/// 只是换了一颗键（出厂 `Alt+R`）。单独占一个 id 是因为 `WM_HOTKEY` 的 `wparam` 只带 id。
const HOTKEY_EXTRA: i32 = 3;

mod actions;
mod cli;
mod dbltap;
mod hud;
mod large_type;
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
    HotkeySpec, SecondaryAction, VK_0, VK_9, VK_A, VK_F1, VK_F24, VK_OEM_3, VK_RETURN, VK_SPACE,
    VK_Z,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// 批次 G1（FR-WIN-03）：双击 Ctrl / Shift 的判定是纯函数（`src/dbltap.rs`），
// 钩子装卸、时间戳与"切一次显隐"的投递在这里。
use dbltap::{DblTap, TapDecision, TapTracker};

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
// 配置损坏/留证失败那句说明要长得多（含"先手动备份再删掉"这种要照着做的话），
// 3 秒读不完两条换行的句子——批次 E3 起这类启动提示单独给 10 秒。
const TOAST_TTL_STARTUP: Duration = Duration::from_secs(10);
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
    /// 批次 D7（FR-SRC-02 / FR-SRC-14）：文件索引的扫描目录与时机。第 5 页。
    /// 插在「数据源」后面：它本来就是文件源那一行的展开。
    /// 【2026-10-27 批次 D7：插入本页，外观/高级的页号顺延为 6/7。】
    FileSearch,
    /// 批次 F1（FR-UIX-01）：主题选择 + 主题目录入口。~~第 5 页~~【批次 D7 起第 6 页】。
    Appearance,
    /// 批次 F7（D3 + D6）：重置学习排序 / 恢复默认 / 隐藏的条目。~~第 6 页~~【批次 D7 起第 7 页】。
    /// 这一页只有破坏性动作与低频维护项，所以排在最后。
    Advanced,
    /// 批次 D5（FR-ACT-09）：快捷项批量管理——勾选批量删、失效项校验、绝对 ↔ 相对互转。第 8 页。
    /// 与「高级」同为低频维护，但它是"对整张清单动刀"，单开一页免得混进设置开关里。
    Manage,
    /// 批次 D8（FR-SYS-06）：使用统计——合计 / 近 30 天柱状 / 按功能次数 / 按条目前 10。第 9 页。
    /// ~~排在最后~~【2026-10-29 批次 D2：第 10 页「网页引擎」追加在它后面，本页不再是末项】：
    /// 它是只读看板（唯一的写动作是「清除统计」），与上面那些真要动配置的页放一起
    /// 反而让分类栏显得杂乱。
    Usage,
    /// 批次 D2（FR-SRC-11 的缺口）：网页搜索引擎表——列出 / 增删改 / 恢复出厂 / 兜底引擎。第 10 页。
    /// 排在「使用统计」后面：同属低频维护页，且不打断前九页的页号（`-Preferences` 是线性协议）。
    WebEngines,
}

impl SettingsTab {
    /// FR-WIN-08：页码从 1 起（1=通用 2=功能 3=热键 4=数据源 5=文件搜索 6=外观 7=高级
    /// 8=批量管理 9=使用统计）；越界返回 None，由调用方落回通用。批次 D7 起第 5 页由「外观」
    /// 改为「文件搜索」，原第 5/6 页顺延——`-Preferences 5/6` 的旧语义随之变，与设置页分类栏一起改口径。
    /// 【批次 D5：第 8 页追加在末尾，1–7 的语义与 `-Preferences` 参数一格不动。】
    /// 【批次 D8：第 9 页同样追加在末尾，1–8 一格不动。】
    /// 【批次 D2（2026-10-29）：第 10 页「网页引擎」同样追加在末尾，1–9 一格不动。】
    fn from_page(page: u32) -> Option<Self> {
        match page {
            1 => Some(Self::General),
            2 => Some(Self::Features),
            3 => Some(Self::Hotkeys),
            4 => Some(Self::Sources),
            5 => Some(Self::FileSearch),
            6 => Some(Self::Appearance),
            7 => Some(Self::Advanced),
            8 => Some(Self::Manage),
            9 => Some(Self::Usage),
            10 => Some(Self::WebEngines),
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
    /// FR-QRY-10（批次 C3）：把 search_history.json 清成空表，上限那一格不动。
    /// 这一个**不写备份**——另两个清的是统计，这张表清的是用户敲过的原文。
    ClearHistory,
    /// FR-SYS-06（批次 D8）：把 usage.json 清成空表。条目上的 `use_count` 不动（那是另一本账）。
    /// 与 [`Self::ClearHistory`] 一样不写备份——它清的是统计，留一份副本正好是「清除」二字的反面。
    ClearUsage,
    /// FR-ACT-07（批次 B6）：把 param_history.json 清成空表（参数框下面那列最近用过的值）。
    /// 与上面两条"清原文"的同一个理由：不写备份。
    ClearParamHistory,
}

/// 批次 D5（FR-ACT-09）：批量管理页的两种确认（`Some`＝确认框开着）。
///
/// 转换把**算好的计划**一起存进来：确认框开着的那一帧清单不许再动，落地时按存下来的那份执行，
/// 报出来的条数就是用户刚才看到的那一份（与 F7 两个破坏性动作"只问不做"的形态同一条口径）。
enum ManageConfirm {
    Convert {
        to_relative: bool,
        changes: Vec<(String, String)>,
        skipped: usize,
    },
    Delete {
        uids: Vec<String>,
    },
}

// FR-SRC-23【2026-10-17 修订】：无匹配 Enter 已从「直接运行兜底」改为弹 AddDialog 预填入库，
// 原 FallbackAction 删除。

/// 添加/编辑对话框状态（FR-SRC-20 `AddDialog(prefill)`，对标 Delphi frmShortCut）。
/// `edit_uid=Some(u)` 表示编辑既有项（按派生 uid 定位替换），`None` 表示新建。
/// 身份用 uid 而非 `path`：同一 exe 不同参数是两个条目，按 path 会认错（2026-10-17）。
/// 批次 D4（FR-CFG-06）：一次「选文件 / 选目录 / 拖入」算出来的值，以及它和表单现值的冲突。
///
/// Delphi 在 `btnFileClick` / `btnDirClick` 里对"别名"和"名称"各弹一次 `MessageBox` 问要不要
/// 替换；这里合成一条问一次（两个字段一起答）。`*_old` 只在真有冲突时是 `Some`，文案要用。
struct PickPrompt {
    fill: source::import::ImportFill,
    keyword_old: Option<String>,
    name_old: Option<String>,
}

/// 这个 `fill` 要不要先问一句：别名/名称已经填了别的值 ⇒ 问；否则直接填。
fn make_fill_prompt(dg: &DialogState, fill: &source::import::ImportFill) -> Option<PickPrompt> {
    let keyword_old = dg.keyword.trim();
    let name_old = dg.name.trim();
    let keyword_old = (!keyword_old.is_empty() && keyword_old != fill.keyword)
        .then(|| keyword_old.to_string());
    let name_old =
        (!name_old.is_empty() && name_old != fill.name).then(|| name_old.to_string());
    (keyword_old.is_some() || name_old.is_some()).then(|| PickPrompt {
        fill: fill.clone(),
        keyword_old,
        name_old,
    })
}

/// 把 `fill` 写进表单。`replace_names = false`（用户答「保留」）时只补空着的两格，
/// 命令行/参数/工作目录照填——那三格是"这条命令到底是什么"，不跟着名字走。
fn apply_fill(dg: &mut DialogState, fill: &source::import::ImportFill, replace_names: bool) {
    dg.path = fill.path.clone();
    dg.arguments = fill.arguments.clone();
    dg.working_dir = fill.working_dir.clone();
    if replace_names || dg.keyword.trim().is_empty() {
        dg.keyword = fill.keyword.clone();
    }
    if replace_names || dg.name.trim().is_empty() {
        dg.name = fill.name.clone();
    }
    // 系统命令的 `internal:<id>` 不容许被类型推断改掉（同下方类型下拉只读那一条理由）。
    if dg.kind != ItemKind::Internal {
        dg.kind = fill.kind;
    }
}

/// 批次 D4：把拖进来/选进来的路径按 Delphi 口径解出要填的整组值。
///
/// `.lnk` 解引用走本仓既有的 `resolve_lnk`（它的 `path` 已展开过环境变量）。Delphi 的
/// `ResolveLink` 把目标与参数拼在同一行里，这里分开存（我们的条目本来就是 `path`/`arguments`
/// 两格），但命名仍按"目标 + 参数"一整行算，否则 Chrome 的 `--app=` 特例不成立。
///
/// 解引用失败、或解析到 `%windir%\Installer\{…}` 里 ⇒ 照 Delphi 的做法**仍按 `.lnk` 本身**导入。
fn import_from_path(path: &str) -> source::import::ImportFill {
    let original = path.trim().trim_matches('"').to_string();
    let mut target = original.clone();
    let mut arguments = String::new();
    let mut working_dir = String::new();
    if original.to_ascii_lowercase().ends_with(".lnk") {
        if let Some(it) = unsafe { source::application::resolve_lnk(std::path::Path::new(&original)) } {
            let windir = std::env::var_os("windir")
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !it.path.trim().is_empty()
                && !source::import::is_installer_target(&it.path, &windir)
            {
                target = it.path;
                arguments = it.arguments;
                working_dir = it.working_dir.unwrap_or_default();
            }
        }
    }
    let is_dir = std::path::Path::new(source::import::unquote(&target)).is_dir();
    source::import::fill_from_import(&target, &arguments, &working_dir, is_dir)
}

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
    /// 批次 B6（FR-ACT-07）：参数编码四档。`None`＝不展开标记、不弹参数框（旧条目的缺省）。
    param_type: source::params::ParamType,
    /// 批次 B6：本条已经露过一次「有标记但编码=不替换」的追问。存进弹窗自己身上——
    /// 与 `fill_prompt` 同一条理由：框一关它就该消失，挂在 App 上要挨个关框路径去清。
    marker_warn: bool,
    /// 批次 D4（FR-CFG-06）：选文件/选目录之后的"要不要覆盖已填的别名/名称"询问。
    /// 挂在弹窗自己身上，是为了跟着弹窗一起消失——关框的路径有五六条，挨个清必漏一条。
    fill_prompt: Option<PickPrompt>,
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
            param_type: source::params::ParamType::None,
            marker_warn: false,
            fill_prompt: None,
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
            param_type: source::params::ParamType::None,
            marker_warn: false,
            fill_prompt: None,
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
            param_type: source::params::ParamType::None,
            marker_warn: false,
            fill_prompt: None,
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
            param_type: source::params::ParamType::None,
            marker_warn: false,
            fill_prompt: None,
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
            param_type: it.param_type,
            marker_warn: false,
            fill_prompt: None,
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
            param_type: self.param_type,
            // 新建默认未置顶、零统计；编辑既有项的置顶/使用统计在 DlgResult::Confirm 提交时旧值携带回填。
            ..Default::default()
        }
    }
    /// 批次 B6（FR-ACT-07）：保存前要不要追问「有标记但编码=不替换」。
    ///
    /// 判据只看**两个命令行格**：工作目录不参与展开（Delphi 也只处理命令行两格），
    /// 把标记写在工作目录里本来就换不了，提醒它对不上号。
    fn marker_warn_needed(&self) -> bool {
        self.param_type == source::params::ParamType::None
            && (source::params::has_marker(&self.path)
                || source::params::has_marker(&self.arguments))
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

/// 批次 B6（FR-ACT-07）：参数框打开时扣下的那一份现场。
///
/// 只带"要启动谁"和身份；展开（标记 → 真实值）推迟到用户按「确定」那一帧——
/// 打开框这一刻剪贴板可能还会被用户改掉，参数也该以他最后一次敲的为准。
#[derive(Debug, Clone)]
struct ParamPrompt {
    /// 原条目（未展开），确定那一帧才按 [`LauncherApp::expand_for_launch`] 出副本。
    entry: LaunchItem,
    /// 学习统计 / 使用统计用的身份（原条目 uid，不是展开副本的派生 uid）。
    uid: String,
    /// 输入框里的参数（跨帧活着）。
    text: String,
    /// 这一按来自清单行 ⇒ 成功才累计使用次数并学别名（与 `<Row::Item>` 那两支同口径）。
    from_list: bool,
    /// 学别名用的那串词（清单进来是查询框，面板进来是面板存下的原查询）。
    alias_word: String,
    /// 来自自定义命令源（`HeadRow::Custom`）：成功要记一次 `Bucket::Custom`。
    from_custom: bool,
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

/// 批次 F3（FR-WIN-09）：工作区千分比 → 窗口左上角（物理像素）。
/// 与 AHK `SearchWindow.Place`（`SearchWindow.ahk:691-701`）逐式对译：X 按"可用宽度"摊、
/// Y 按"整块高度"摊，然后各自夹回工作区内。
fn place_in_work(work: egui::Rect, size: (i32, i32), rel: [u16; 2]) -> (i32, i32) {
    let (l, t, r, b) = (
        work.min.x.round() as i32,
        work.min.y.round() as i32,
        work.max.x.round() as i32,
        work.max.y.round() as i32,
    );
    let (w, h) = size;
    let fx = rel[0].min(1000) as f32 / 1000.0;
    let fy = rel[1].min(1000) as f32 / 1000.0;
    let x = l + ((r - l - w) as f32 * fx).round() as i32;
    let y = t + ((b - t) as f32 * fy).round() as i32;
    (x.clamp(l, (r - w).max(l)), y.clamp(t, (b - h).max(t)))
}

/// [`place_in_work`] 的反函数：窗口左上角 → 工作区千分比（AHK `RelativePosition` 同式）。
fn relative_in_work(work: egui::Rect, size: (i32, i32), pos: (i32, i32)) -> [u16; 2] {
    let (l, t, r, b) = (
        work.min.x.round() as i32,
        work.min.y.round() as i32,
        work.max.x.round() as i32,
        work.max.y.round() as i32,
    );
    let (w, _h) = size;
    let fx = if r - l > w {
        ((pos.0 - l) as f32 * 1000.0 / (r - l - w) as f32).round()
    } else {
        // 窗口比工作区还宽：没有可摊的余量，记成"居中"，下次摆出来仍是对齐左缘。
        500.0
    };
    let fy = if b - t > 0 {
        ((pos.1 - t) as f32 * 1000.0 / (b - t) as f32).round()
    } else {
        200.0
    };
    [
        fx.clamp(0.0, 1000.0) as u16,
        fy.clamp(0.0, 1000.0) as u16,
    ]
}

/// 「呼出前那个窗口」的中心点。最小化的窗口 `GetWindowRect` 给的是屏幕外的假坐标，
/// 这里直接当没有（AHK 的 `WorkAreaOfWindow` 同样读不对最小化窗口）。
fn previous_window_center() -> Option<(i32, i32)> {
    let raw = PREVIOUS_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return None;
    }
    let hwnd = HWND(raw as *mut _);
    if unsafe { IsIconic(hwnd).as_bool() } {
        return None;
    }
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
    if r.right <= r.left || r.bottom <= r.top {
        return None;
    }
    Some(((r.left + r.right) / 2, (r.top + r.bottom) / 2))
}

/// 批次 F3（FR-WIN-09）：按 `show_on` 选一块屏的工作区（物理像素）。
/// `Primary` 用坐标 `(0, 0)`——Windows 约定主屏左上角恒为虚拟坐标原点。
/// `Active` 没有「呼出前窗口」（最小化 / 已关闭 / 启动那一刻）时退回鼠标所在屏，与 AHK 同。
fn work_for(show_on: source::application::ShowOn) -> Option<egui::Rect> {
    use source::application::ShowOn;
    let pt = match show_on {
        ShowOn::Primary => (0, 0),
        ShowOn::Active => previous_window_center().or_else(win_cursor)?,
        ShowOn::Mouse => win_cursor()?,
    };
    hud::monitor_work_at(pt)
}

/// 窗口中心点所在屏的工作区——隐藏时把当前位置折算成千分比要用它。
fn work_at_window(hwnd: HWND) -> Option<egui::Rect> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
    if r.right <= r.left || r.bottom <= r.top {
        return None;
    }
    hud::monitor_work_at(((r.left + r.right) / 2, (r.top + r.bottom) / 2))
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
    /// 批次 B6（FR-ACT-07）：参数框（Some=打开）。
    param_prompt: Option<ParamPrompt>,
    /// 参数框当帧吞掉 Enter（与 [`Self::dialog_enter_armed`] 同一条理由）。
    param_enter_armed: bool,
    /// 批次 B6：呼出瞬间的快照（剪贴板 / 前台窗口），只有带标记的条目会读它。
    summon: source::params::Snapshot,
    /// 批次 B6：参数历史的内存副本。确认一次写一次盘，读盘在启动那一次。
    param_history: source::params::ParamHistory,
    /// 参数历史文件坏过（`Corrupted::Lost`）⇒ 这一轮别把用户的旧文件覆盖掉
    /// （与 `usage_blocked` / `history_blocked` 同一口径）。
    param_history_blocked: bool,
    about_open: bool,                // 批次 A2：「关于本程序」弹窗（Some 的等价物，用 bool 更直白）
    help_entry: Option<source::cheatsheet::CheatHit>, // 批次 A8：速查表条目的帮助页（Some=打开）
    /// 批次 F7：设置页「高级」页的破坏性动作确认（Some=确认框开着）。
    /// 按键分支与 Esc 链都要先看它，否则开着确认框还能触发别的快捷键。
    pending_destructive: Option<Destructive>,
    /// 批次 D5（FR-ACT-09）：「批量管理」页勾选的条目 uid（派生值，不落盘）。
    /// 转换会改路径 ⇒ uid 跟着变，所以每次转换/删除之后都要清空这份勾选。
    manage_checked: std::collections::BTreeSet<String>,
    /// 批次 D5：「批量管理」页的确认框（转换 / 删除，Some=开着）。
    manage_confirm: Option<ManageConfirm>,
    /// 批次 F6：顶栏 ☰ 主菜单是否开着（2026-10-24 用户点名「换成一个三条杠的菜单，点击显示
    /// 新建按钮和其他设置按钮」）。用裸 bool 而不是 egui 的 memory：快捷键 Ctrl+M 也要能开它，
    /// `egui::Popup::open_bool` 正好吃这一个所有权，关闭（点击/Esc）由 Popup 写回 `false`。
    menu_open: bool,
    win_drag: Option<DragAnchor>, // 左键拖空白移动窗口的进行中锚点（None=没在拖）
    dialog_enter_armed: bool,     // 弹窗当帧吞掉 Enter，防"打开框的那次回车"当帧又触发对话框的确定
    settings_open: bool,          // 设置页（整页模式，替代搜索页；2026-10-17 用户要求）
    settings_tab: SettingsTab,    // 设置页当前分类
    /// 批次 D7 修正（2026-10-28 真机）：设置页「文件搜索」页两串输入框的编辑缓冲。
    /// 输入框的文本源必须**跨帧**活着：每帧从 `prefs` 克隆一份交出去，键进来的那一帧写进克隆，
    /// 下一帧又被新的克隆盖回去 ⇒ 真机表现就是"打不进字"（当时 5 条目录 + 8 条排除行全中）。
    /// `None`＝这一页没在编辑；进页时按 prefs 取一份，离开页/关设置页时回写并丢掉。
    file_folders_edit: Option<Vec<String>>,
    file_exclude_edit: Option<Vec<String>>,
    /// 批次 D2：设置页「网页引擎」页的编辑缓冲，跨帧理由与上面两条相同（每帧克隆会把键吞掉）。
    /// `None`＝这一页没在编辑；进页时按 prefs 取一份，离开页/关设置页时整理后回写并丢掉。
    web_engines_edit: Option<Vec<source::websearch::WebEngine>>,
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
    /// 回执：缓存键 + `None`＝这条取不到图标（失败键不再重试）。
    icon_rx: mpsc::Receiver<(String, Option<(Vec<u8>, u32, u32)>)>,
    icon_tx: mpsc::Sender<(String, Option<(Vec<u8>, u32, u32)>)>, // 常驻，可 clone 给每批新项再起加载器
    /// 纹理缓存：`icon::cache_key` → 纹理 + 最近一次被画出的序号（LRU）。
    icons: std::collections::HashMap<String, IconEntry>,
    /// 已派给加载线程、还没回执的键（防同一键重复起线程）。
    icon_pending: std::collections::HashSet<String>,
    /// 提取失败过的键——失败不重试，否则可见行每帧都会重开线程。
    icon_failed: std::collections::HashSet<String>,
    /// 行渲染的单调序号：每帧 +1，写进 `IconEntry.last_use` 做 LRU 排序。
    icon_tick: std::cell::Cell<u64>,
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
    // ---- FR-UIX-10：大字显示（`src/large_type.rs`）----
    large_type: large_type::LargeType,
    // ---- 批次 P1：操作面板（FR-ACT-03 / FR-ACT-04，`src/actions.rs` 出表）----
    /// 面板现场（`None` = 结果模式）。面板开着时查询框里那几个字是**过滤词**，
    /// 用户原来的查询原文存在 `saved_query` 里，退出时原样放回去。
    panel: Option<PanelState>,
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
    // ---- 批次 G1：FR-WIN-02 的"次热键"（第二个全局呼出键，出厂 `Alt+R`）----
    /// 系统里此刻真正装着的副呼出键（显示口径）。`None` = 用户清掉了，或注册失败。
    extra_active: Option<HotkeySpec>,
    /// 已投递、还没拿到回执的副呼出组合（这段时间禁用"应用/清除"，与另两格同一口径）。
    extra_awaiting: Option<HotkeySpec>,
    /// 副呼出的键框正在等一次按键（三格互斥，见 `take_keypick`）。
    extra_recording: bool,
    // ---- 批次 E3-丙：次级动作键（`Ctrl+Enter` 一类窗口内热键）----
    /// 设置页「次级动作」卡片的键框正在等一次按键。这颗键**不注册给系统**（不占别人的键、
    /// 也不进热键线程），所以不需要 `*_awaiting` / `*_active` 那一套回执；录制完直接落配置。
    secondary_recording: bool,
    // ---- 批次 E4：FR-ACT-05 多标记与批量 ----
    /// 已标记的路径（原样保存；比较不分大小写，与 Windows 路径口径一致）。
    /// 每次呼出清空——AHK `SearchWindow.Marked := Map()` 也是这条口径，标记不跨呼出周期存活。
    marked: Vec<String>,
    /// 「全部回收站」的执行前确认（Some=确认框开着，里面是待回收的路径）。
    /// 与删除确认同一形态：这一帧只问，按「确定」那一帧才逐个进回收站。
    batch_confirm: Option<Vec<String>>,
    // ---- 批次 C3：FR-QRY-10 搜索历史（`search_history.json`）----
    /// 执行过的查询原文，最新在前（与 [`source::application::push_history`] 那张表同一口径：
    /// 只存文本、不存结果，去重不分大小写、回填按原样）。落盘在 `remember_history` 里按需做。
    history: Vec<String>,
    /// 翻历史的游标（`index = 0` ＝ 没在翻，框里是用户自己敲的）。框里的字被用户改动时归零，
    /// 与 AHK `_OnInputChange` 清 `HistoryIndex` 同一口径——不清就成了"改完字再按 ↑ 跳过一条"。
    history_browse: source::application::HistoryBrowse,
    /// `search_history.json` 坏了且留证没成功 ⇒ 本次启动不写它（写一次就覆盖掉唯一证据）。
    /// 只管历史这一份文件：设置与清单另有各自的门槛，见 `writes_blocked`。
    history_blocked: bool,
    // ---- 批次 D8：FR-SYS-06 使用统计（`usage.json`）----
    /// 按天、按功能桶的计数。**只记次数不记内容**（形态见 [`source::usage`]）。
    /// 记账点在 `record_usage`：清单项与系统命令走 `mark_used`，其余四类走各自执行点。
    usage: source::usage::UsageStats,
    /// `usage.json` 坏了且留证没成功 ⇒ 本次启动不写它（与 `history_blocked` 同一条门槛，
    /// 各管各的文件）。
    usage_blocked: bool,
}

/// 一条待取图标的作业：缓存键 + 图标规格（条目 `source_path` 原文）。
type IconJob = (String, String);

/// 缓存里的一条纹理。`last_use` 用 `Cell`，是为了行渲染只有 `&self` 时也能刷新 LRU 时刻。
struct IconEntry {
    texture: egui::TextureHandle,
    last_use: std::cell::Cell<u64>,
}

/// 条目实际要取的图标规格。`source_path` 为空时按类别兜一个通用图标：
/// 文件夹 ⇒ `folder:`，网址 ⇒ `url:`（需求书 FR-SRC-03 只管"有图标"，没规定出厂条目必须自带规格）。
/// 其余类别（应用 / 命令 / 文件 / 内置动作）空着就是没有，不臆造。
fn icon_spec_of(it: &source::application::LaunchItem) -> &str {
    if !it.source_path.is_empty() {
        return &it.source_path;
    }
    match it.kind {
        source::application::ItemKind::Folder => "folder:",
        source::application::ItemKind::Url => "url:",
        _ => "",
    }
}

/// 从条目里收「键 + 规格」，按缓存键去重、保序。规格为空 ⇒ `cache_key` 给 None，跳过。
fn icon_jobs(items: &[source::application::LaunchItem]) -> Vec<IconJob> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for it in items {
        let spec = icon_spec_of(it);
        if let Some(key) = source::icon::cache_key(spec) {
            if seen.insert(key.clone()) {
                out.push((key, spec.to_string()));
            }
        }
    }
    out
}

fn spawn_icon_loader(
    tx: mpsc::Sender<(String, Option<(Vec<u8>, u32, u32)>)>,
    jobs: Vec<IconJob>,
) {
    thread::spawn(move || {
        let mut ok = 0usize;
        let total = jobs.len();
        let mut first_logged = false;
        for (key, spec) in jobs {
            let hit = source::icon::extract(&spec);
            if let Some((rgba, w, h)) = &hit {
                if !first_logged {
                    let nonzero = rgba.iter().filter(|&&v| v != 0).count();
                    eprintln!(
                        "icon probe: 首个成功 {}x{}，非零字节 {}/{}  ← {}",
                        w,
                        h,
                        nonzero,
                        rgba.len(),
                        source::icon::log_label(&spec)
                    );
                    first_logged = true;
                }
                ok += 1;
            }
            if tx.send((key, hit)).is_err() {
                break;
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
        let done = EmptyClipboard()
            .map_err(|e| format!("清空剪贴板失败：{e}"))
            .and_then(|_| write_text_format(&wide));
        let _ = CloseClipboard();
        done
    }
}

/// 必须在剪贴板已打开、且已 `EmptyClipboard` 之后调用。UTF-16 带末尾 NUL 是 `CF_UNICODETEXT`
/// 规定的形态。
fn write_text_format(wide: &[u16]) -> Result<(), String> {
    // SAFETY：`u16` 的字节视图只在这一个调用里用，`wide` 活到函数返回。
    let bytes = unsafe {
        std::slice::from_raw_parts(
            wide.as_ptr().cast::<u8>(),
            wide.len() * std::mem::size_of::<u16>(),
        )
    };
    write_clipboard_bytes(CF_UNICODETEXT, bytes)
}

/// 批次 E4（FR-ACT-05）：`CF_HDROP` 的格式号。与 `CF_UNICODETEXT` 同一口径——常量在
/// `Win32::System::Ole`，但 `SetClipboardData` 收 `u32` ⇒ 写死 15，不为一个数字多开一条 feature。
const CF_HDROP: u32 = 15;

/// 资源管理器读的那条"复制还是剪切"标记。它不是一个数字格式号，而是注册格式
/// `Preferred DropEffect`，值是 4 字节 `DWORD`。
const PREFERRED_DROPEFFECT: &str = "Preferred DropEffect";
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

/// `CF_HDROP` 的载荷：`DROPFILES` 头 + 一串 UTF-16 路径（每条以 NUL 结尾）+ 末尾再一个 NUL。
///
/// `pFiles` 是"文件表在整块内存里的字节偏移"，必须等于结构体长度；`fWide=TRUE` 才是宽字符表
///（资源管理器两种都认，但中文路径只有宽字符不会丢）。
/// 抽成纯函数是为了能钉字节布局——真机验证一次要动资源管理器，单测每次都能跑。
fn build_dropfiles(paths: &[String]) -> Vec<u8> {
    let header = std::mem::size_of::<DROPFILES>();
    let mut data = vec![0u8; header];
    let head = DROPFILES {
        pFiles: header as u32,
        pt: POINT { x: 0, y: 0 },
        fNC: false.into(),
        fWide: true.into(),
    };
    // SAFETY：`head` 是一块就地初始化的 `#[repr(C)]` 结构，按字节拷进同样大小的缓冲区。
    unsafe {
        std::ptr::copy_nonoverlapping(
            (&head as *const DROPFILES).cast::<u8>(),
            data.as_mut_ptr(),
            header,
        );
    }
    for path in paths {
        for unit in path.encode_utf16() {
            data.extend_from_slice(&unit.to_le_bytes());
        }
        data.extend_from_slice(&0u16.to_le_bytes());
    }
    data.extend_from_slice(&0u16.to_le_bytes());
    data
}

/// 把一批文件/目录放进剪贴板，并标明这一批是复制还是剪切（`cut=true` 写
/// `DROPEFFECT_MOVE`，资源管理器粘贴时就会把原件移走）。
///
/// 「剪切」全部靠这个 `DropEffect` 成立：只写 `CF_HDROP` 的话资源管理器一律按复制处理，
/// 这正是需求书那句「剪切须写 `DropEffect` 才能被资源管理器识别」的意思。
fn copy_files_to_clipboard(paths: &[String], cut: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Err("没有可放进剪贴板的路径".to_string());
    }
    let data = build_dropfiles(paths);
    let effect = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY };
    let name: Vec<u16> = PREFERRED_DROPEFFECT
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .collect();
    // SAFETY：`name` 以 NUL 结尾且活到调用返回。
    let fmt = unsafe { RegisterClipboardFormatW(PCWSTR(name.as_ptr())) };
    if fmt == 0 {
        return Err("注册剪贴板格式 Preferred DropEffect 失败".to_string());
    }
    unsafe {
        if OpenClipboard(None).is_err() {
            return Err("打开剪贴板失败（可能正被别的程序占着）".to_string());
        }
        let done = EmptyClipboard()
            .map_err(|e| format!("清空剪贴板失败：{e}"))
            .and_then(|_| {
                write_clipboard_bytes(CF_HDROP, &data)?;
                // 文件已经进剪贴板之后这一步才失败：不谎报"整体失败"，但必须说清后果——
                // 资源管理器会按"复制"处理，用户以为剪走了、其实原件还在。
                write_clipboard_bytes(fmt, &effect.to_le_bytes()).map_err(|e| {
                    format!("文件已放进剪贴板，但剪贴标记没写成功（会按复制处理）：{e}")
                })
            });
        let _ = CloseClipboard();
        done
    }
}

/// 把一段字节装进 `GMEM_MOVEABLE` 全局内存并交给剪贴板。**必须在剪贴板已打开、
/// 且已 `EmptyClipboard` 之后调用**；`GlobalUnlock` 不能省——带着锁的句柄交给系统会让
/// `SetClipboardData` 直接失败。句柄一旦交出去就归系统所有，只有失败路径才自己 `GlobalFree`。
fn write_clipboard_bytes(format: u32, bytes: &[u8]) -> Result<(), String> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))
            .map_err(|e| format!("分配剪贴板内存失败：{e}"))?;
        let p = GlobalLock(h);
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return Err("锁定剪贴板内存失败".to_string());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(h);
        if SetClipboardData(format, Some(HANDLE(h.0))).is_err() {
            let _ = GlobalFree(Some(h));
            return Err("写入剪贴板失败".to_string());
        }
        Ok(())
    }
}

// ---- 批次 E5（FR-ACT-01 第三格「文本 ⇒ 粘贴」）：呼出前那个窗口的记账与注入 ----

/// 呼出之前那个窗口的句柄数值；`0` = 这次没记下（呼出时系统前台本来就是空的）。
/// `HWND` 是裸指针、不 `Sync`，与 [`HOTKEY_HWND`] 同一处理：存数值、用时还原。
static PREVIOUS_HWND: AtomicUsize = AtomicUsize::new(0);

/// 一批最多投多少个 UTF-16 单元。`SendInput` 对单次调用的事件数有系统上限，
/// 计算器行那串值远用不到分块，留分块是因为这张表的文本格以后可能吃到换算长串。
const PASTE_CHUNK_UNITS: usize = 64;

/// 投字之前等焦点落到目标窗口的上限与步进。见 [`paste_text_into_previous_window`] 的闸 ③：
/// `SetForegroundWindow` 报 TRUE 之后，前台换人可能还要几毫秒才在 `GetForegroundWindow` 上看得见。
/// 上限取 300ms 而不是 AHK 的 1s——这条循环堵在 UI 线程上，再长就是用户能感觉到的卡顿。
const FOCUS_SETTLE_TIMEOUT: Duration = Duration::from_millis(300);
const FOCUS_SETTLE_POLL: Duration = Duration::from_millis(10);

/// 只取窗口类名。**标题一个字不取**：标题里经常带着用户正在编辑的正文，
/// 与 [`inject_typeback_char`] 那条"前台只记 hwnd + 类名"的隐私口径同一条。
fn window_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    let n = (n.max(0) as usize).min(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// 读一个窗口的标题。**只有 `{%wt}` 那一格用**：标题里经常带着用户正在编辑的正文，
/// 取到之后直接交给呼出快照，绝不进日志（与 [`window_class_name`] 那条隐私口径同源）。
fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    let n = (n.max(0) as usize).min(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// 批次 B6（FR-ACT-07）：呼出瞬间的剪贴板文本（`{%c}` 的来源）。
///
/// 打不开剪贴板（正被别的程序占着）＝ `None` ⇒ 展开成空串。宁可让用户看见"参数少了"，
/// 也不拿上一次呼出的旧内容冒充这一次的剪贴板。
///
/// 读的时候按 `CF_UNICODETEXT` 的约定走 NUL 结尾；1Mi 个 UTF-16 单元的上限只是防一份
/// 没写结尾的坏数据把 UI 线程挂住（正常剪贴板远到不了这个量）。
fn read_clipboard_text() -> Option<String> {
    unsafe {
        if OpenClipboard(None).is_err() {
            eprintln!("参数标记：剪贴板打不开（可能正被别的程序占着）⇒ {{%c}} 这次是空值");
            return None;
        }
        let text = match GetClipboardData(CF_UNICODETEXT) {
            Ok(handle) if !handle.is_invalid() => {
                let h = HGLOBAL(handle.0);
                let p = GlobalLock(h);
                let s = if p.is_null() {
                    None
                } else {
                    let base = p.cast::<u16>();
                    let mut units: Vec<u16> = Vec::new();
                    for i in 0..(1usize << 20) {
                        let u = *base.add(i);
                        if u == 0 {
                            break;
                        }
                        units.push(u);
                    }
                    let _ = GlobalUnlock(h);
                    Some(String::from_utf16_lossy(&units))
                };
                s
            }
            // 剪贴板里没有文本（只有文件 / 图片，或者干脆是空的）⇒ 空值，不报错。
            _ => None,
        };
        let _ = CloseClipboard();
        if let Some(t) = &text {
            // 长度可以入日志，原文不行（FR-SYS-03）。
            eprintln!(
                "参数标记：呼出快照读到剪贴板 {} 字（原文不落日志）",
                t.chars().count()
            );
        }
        text
    }
}

/// 呼出快照的初值。构造那一刻窗口还没建出来（`self_hwnd` 为 `None`），所以不看"前台是不是自己"。
/// 少了它，程序启动后**第一次**按回车运行的带标记条目会拿到一份空快照。
fn initial_summon_snapshot() -> source::params::Snapshot {
    let mut snap = source::params::Snapshot {
        clipboard: read_clipboard_text().unwrap_or_default(),
        ..Default::default()
    };
    let fg = unsafe { GetForegroundWindow() };
    if !fg.0.is_null() {
        snap.window_id = (fg.0 as usize).to_string();
        snap.window_title = window_title(fg);
        snap.window_class = window_class_name(fg);
    }
    snap
}

/// 记一次"呼出之前的前台窗口"。AHK 版同一格是 `App.RememberActiveWindow()`——
/// 本程序此前**没有这份记账**，这就是 FR-ACT-01 第三格一直做不下去的真正前置
/// （不是 FR-ACT-05 多选，那个批次 E4 就交付了）。
///
/// 三条规矩：
/// · 前台为空 ⇒ 清零。宁可以说"没记下"然后拒绝粘贴，不能留着上一次的窗口当这次的目标。
/// · 前台就是本程序自己 ⇒ **保留上一次那一个**，不覆盖。这是 AHK 的同一条判据：重复呼出、
///   托盘点自己、大字窗把焦点借走又还回来，这些时刻的前台都不是用户要贴回去的窗口。
///   可见却失焦的那几种情况（弹窗／设置页／大字窗开着，FR-WIN-04 不关窗）也走这一格，
///   所以这个函数在 `show_now` 里**每次呼出都调**，不只调"隐藏→显示"那条边沿。
/// · 其余 ⇒ 记下，日志报 hwnd + 类名。
fn remember_previous_window(own: Option<HWND>) {
    let fg = unsafe { GetForegroundWindow() };
    if fg.0.is_null() {
        PREVIOUS_HWND.store(0, Ordering::Relaxed);
        eprintln!("paste: 呼出时系统前台为空 ⇒ 这次没有可粘贴的目标");
        return;
    }
    if let Some(me) = own {
        if me.0 == fg.0 {
            return;
        }
    }
    let raw = fg.0 as usize;
    let changed = PREVIOUS_HWND.swap(raw, Ordering::Relaxed) != raw;
    if changed {
        eprintln!(
            "paste: 记下「呼出前的窗口」= 0x{raw:x}({})",
            window_class_name(fg)
        );
    }
}

/// 把一串文本**逐字符打字**进 [`PREVIOUS_HWND`] 那个窗口。
/// 成功返回落点描述（hwnd + 类名，不含内容），失败返回"为什么一个字都没投"。
///
/// 三道闸，任何一道不过都还没碰过键盘：
/// 1. 记到了句柄，且 [`IsWindow`] 说它还在（原窗口已关 ⇒ 清空记账，避免下次还往死句柄上撞）；
/// 2. [`SetForegroundWindow`] 报 TRUE —— 系统可以整条拒绝（前台锁），只看返回值不够；
/// 3. 再问 [`GetForegroundWindow`] 是不是它——这一条才是"焦点真回来了"的事实；缺了它，
///    投出去的字符会落进还拿着焦点的本程序输入框里，用户看到一串凭空多出来的数字。
///    不等一帧而是**最多等 [`FOCUS_SETTLE_TIMEOUT`]**：换人是异步的，同帧看一眼会把该贴成功的
///    那次拒掉（AHK 版在同一个位置等 1 秒，见 `App.FocusPreviousWindow()`）。
///
/// 走 `KEYEVENTF_UNICODE` 而不是 AHK 的"借剪贴板 → `^v` → 还原"：还原之前那一段里用户按
/// `Ctrl+V` 会贴出错东西，剪贴板内容还会被监听型程序拿走；计算器那串值是短的纯文本，
/// Unicode 通道 `wVk = 0` ⇒ `Ctrl+Enter` 那颗还按着的 `Ctrl` 不会被折成新的快捷键
/// （与 [`inject_typeback_char`] 同一套理由，那一条已真机验过记事本与微软拼音）。
///
/// 与那条的**唯一区别**：typeback 打的是本程序自己的输入框（呼出时已按 E-05 切过英文），
/// 这里打的是别人家的窗口，它的输入法状态我不知道 ⇒ 投字那一小段要自己按快照临时切英文、
/// 投完还回去。少了这一步，中文 IME 会把事件吃掉而 `SendInput` 仍报全部成功（批次 E5 真机
/// 抓到的，见下面那段注）。
fn paste_text_into_previous_window(text: &str) -> Result<String, String> {
    if text.is_empty() {
        return Err("这一行没有可粘贴的内容".to_string());
    }
    let raw = PREVIOUS_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return Err("这次呼出没记下「之前的窗口」⇒ 一个字都没投".to_string());
    }
    let hwnd = HWND(raw as *mut _);
    if unsafe { !IsWindow(Some(hwnd)).as_bool() } {
        PREVIOUS_HWND.store(0, Ordering::Relaxed);
        return Err(format!(
            "原来那个窗口（0x{raw:x}）已经关掉了 ⇒ 一个字都没投"
        ));
    }
    let target = window_class_name(hwnd);
    if unsafe { !SetForegroundWindow(hwnd).as_bool() } {
        return Err(format!(
            "系统没让 0x{raw:x}({target}) 到前台（前台锁）⇒ 一个字都没投"
        ));
    }
    // ---- 闸 ③：等焦点真的落到目标窗口，最多 FOCUS_SETTLE_TIMEOUT ----
    // 只看一帧会误拒：调用这一条的时候本程序的窗还盖在上面（`do_paste` 是投完字才收窗的），
    // `SetForegroundWindow` 说 TRUE 只是"允许切"，前台换人可能晚几毫秒才在 `GetForegroundWindow`
    // 上看得见。AHK 版同样是等的（`App.FocusPreviousWindow()` = `WinActivate` +
    // `WinWaitActive(..., , 1)`），这里把上限收短。等不到才拒——宁可一个字都不投，
    // 也不往此刻真正拿着焦点的那个窗口里打。
    let mut settle_polls = 0usize;
    let settle_deadline = Instant::now() + FOCUS_SETTLE_TIMEOUT;
    loop {
        let fg = unsafe { GetForegroundWindow() };
        if fg.0 == hwnd.0 {
            break;
        }
        if Instant::now() >= settle_deadline {
            let now_cls = if fg.0.is_null() {
                "无".to_string()
            } else {
                window_class_name(fg)
            };
            return Err(format!(
                "焦点没回到原窗口（现在前台是 0x{:x}({now_cls})，等了 {settle_polls} 次）⇒ 一个字都没投",
                fg.0 as usize
            ));
        }
        thread::sleep(FOCUS_SETTLE_POLL);
        settle_polls += 1;
    }
    if settle_polls > 0 {
        eprintln!("paste: 焦点等了 {settle_polls} 次才落到 0x{raw:x}({target})");
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    // ---- 批次 E5 真机抓到的根因：`KEYEVENTF_UNICODE` 的事件**先过一遍目标那边的输入法** ----
    // 目标挂着中文模式的 IME 时，`SendInput` 照样返回"全部收下"，字却一个字都不落地——
    // 报成功的假回执，比报错难查得多。第一轮之所以看着是好的，是本程序自己"呼出时切英文"
    // 那一次恰好切在了记事本上（那一刻前台还是它），关窗时又按 E-05 原样还回中文
    // ⇒ 第二轮起投进去的全被 IME 吃掉。所以打字这一小段要**自己**把目标的输入法按住，
    // 投完立刻按快照还回去（含中途失败那条路），不改用户的输入法设置。
    let ime_wnd = ime_wnd_of(hwnd);
    let ime_before = ime_wnd.map(|w| get_ime_state(w));
    if let (Some(w), Some(before)) = (ime_wnd, ime_before) {
        switch_ime_to_english(w, before);
    }
    let mut sent = 0usize;
    let mut failed: Option<String> = None;
    for chunk in units.chunks(PASTE_CHUNK_UNITS) {
        let input = |unit: u16, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
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
        let events: Vec<INPUT> = chunk
            .iter()
            .flat_map(|u| [input(*u, false), input(*u, true)])
            .collect();
        let n = unsafe { SendInput(&events, std::mem::size_of::<INPUT>() as i32) } as usize;
        sent += n;
        if n != events.len() {
            // 已经投出去的那一部分收不回来，所以这条要说清"贴了一半"，
            // 而不是给一个看起来像"什么都没做"的失败。字符数不是内容，可落日志。
            failed = Some(format!(
                "投到 0x{raw:x}({target}) 时系统只收了 {sent}/{} 个事件 ⇒ 这串可能只贴了一半",
                units.len() * 2
            ));
            break;
        }
    }
    if let (Some(w), Some(before)) = (ime_wnd, ime_before) {
        restore_ime(w, before);
    }
    if let Some(e) = failed {
        return Err(e);
    }
    // 输入法快照是状态不是内容，可以落日志——这一行同时是上面那条根因的取证口：
    // 若哪天又出现"报成功却没字"，先看这里的 before 是不是 open=true/conv 带 NATIVE。
    let ime_note = match ime_before {
        Some(s) => format!(
            "，目标输入法 before open={} conv=0x{:X} ⇒ 投字期间临时切英文",
            s.open, s.conv
        ),
        None => "，目标无 IME 窗口 ⇒ 没动输入法".to_string(),
    };
    eprintln!(
        "paste: 已向 0x{raw:x}({target}) 投出 {} 个字符（SendInput {sent} 个事件{ime_note}）；内容不落日志",
        units.len()
    );
    Ok(format!("0x{raw:x}({target})"))
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
/// 和查询框打 `?` 的速查表（批次 A8 起有；批次 F6 那阵注释里写的是 37 条，
/// 2026-10-26 批次 B8 之后 52 条、同批次 C3 加「翻搜索历史」一条后现量 53 条
/// ——`grep -c '^    CheatEntry {' src/source/cheatsheet.rs` 随时可复量）。
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

/// 批次 F4：置顶角标——在图标槽右上角画一个琥珀色小三角。
///
/// 2026-10-29 之前是名称前加 `📌 `，前缀会把名称挤偏、也让「名称→执行」的列位置随置顶状态跳动；
/// 角标贴在图标上，宽度不变。没有图标（空槽）时三角照样画在那个空槽的右上角。
fn paint_pin_badge(ui: &egui::Ui, slot: egui::Rect) {
    let side = (slot.width() * 0.45).clamp(6.0, 14.0);
    let (r, t) = (slot.right(), slot.top());
    let pts = vec![
        egui::pos2(r - side, t),
        egui::pos2(r, t),
        egui::pos2(r, t + side),
    ];
    ui.painter().add(egui::Shape::convex_polygon(
        pts,
        egui::Color32::from_rgb(255, 176, 32),
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(140)),
    ));
}

/// 批次 B6（FR-ACT-07）：带参项在列表行上的 `*` 前缀（继承 Delphi 的 `*` 前缀语义）。
///
/// 判据取 `ParamType ≠ none`：那才是"启动前会替换标记 / 追问参数"的一档。
/// 编码为空时命令行里即使写着标记也不会展开（Delphi 同口径），标 `*` 反而误导。
fn param_row_mark(item: &LaunchItem) -> &'static str {
    if item.param_type == source::params::ParamType::None {
        ""
    } else {
        "* "
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

/// 批次 D5：批量管理页的行内路径按字符截断（含省略号）。
///
/// 不用 `Label::truncate`：那一套靠布局决定刀口，宽窄一变"哪几行被省略"就跟着变；
/// 这里要的是"一屏扫下来都一样长"的稳定观感，完整路径走悬停提示。
fn shorten_path(path: &str, max_chars: usize) -> String {
    if path.chars().count() <= max_chars {
        return path.to_string();
    }
    let mut out: String = path.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
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
/// 四档按「此刻最有用」排序，不是并列展示：操作面板开着先说怎么回去；目录浏览时用户下一步
/// 多半是返回，把返回键说出来；文件模式其次；平时才轮到 Tab 轮换。「基本」分类范围只在
/// 平时这一档追加——它是清单层的筛子，面板/目录/文件模式三档下说它会让人误以为是文件范围。
///
/// 目录那一档的 Esc 文案按 [`browse_parent_of`] 的真实落点说：带过滤词时先去词（不是退目录），
/// 盘根再按下去是清空查询——三处说成同一句话会教坏用户。
fn status_left_text(
    panel: bool,
    browse: Option<(&str, &str)>,
    file_mode: bool,
    tab: TabMode,
    basic_scope: bool,
) -> String {
    // 批次 P1（FR-ACT-03）：面板模式下这一格改说"怎么回去"。左格排在目录 / 文件模式之前：
    // 面板里的过滤词是随便打的字，`route()` 认不出浏览态，但**就算**认出也不该拿目录那套
    // Esc 文案骗人——此刻 Esc 的唯一落点是退出面板。
    if panel {
        return "操作面板 · Esc 或 ← 返回结果".to_string();
    }
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

/// 状态栏右格的圆角键帽（FR-UIX-08）。调用方在 `right_to_left` 的子 Ui 里按"先右后左"的顺序
/// 塞：先动作词、再 `Enter`、最后才是 `→ 操作` 那一组。
fn status_key_cap(ui: &mut egui::Ui, text: &str, theme: &theme::Theme) {
    egui::Frame::new()
        .fill(theme.selected_background)
        .stroke(egui::Stroke::new(1.0, theme.border))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .monospace()
                    .size(theme.shortcut_font_size + 1.0)
                    .color(theme.input_text),
            );
        });
}

/// FR-UIX-10 大字显示的文本。口径对齐 AHK 版 `ResultItem.DisplayText()`：
/// 「选中行的展示文本」优先，没有选中行时退回输入框原文（AHK `_ShowLargeType` 是
/// `item.DisplayText()` 否则 `Input.Value`）。条目层与 AHK `CopyText()` 同一条分派：
/// 文件 / 目录 / URL 取 `path`，其余取名称。
///
/// 头段行的「展示文本」没有 AHK 对应物（那些行在 AHK 里各自是独立控件），按各自最像
/// 「这段文字本身」的字段取：计算器取结果、网页搜索取 URL、提示行取标题、终端取命令。
fn large_text_of(
    row: Option<&HeadRow>,
    item: Option<&LaunchItem>,
    query: &str,
) -> Option<String> {
    let text = match (item, row) {
        (Some(item), _) => match item.kind {
            ItemKind::Url | ItemKind::File | ItemKind::Folder => item.path.clone(),
            _ => item.name.clone(),
        },
        (None, Some(row)) => match row {
            HeadRow::Calc(hit) => hit.copy.clone(),
            HeadRow::Search(hit) => hit.url.clone(),
            HeadRow::Hint(hit) => hit.title.clone(),
            HeadRow::Custom(hit) => hit.name.clone(),
            // 提示行的标题是「某命令（需要参数）」这种说明文，命令原文才是可展示的内容。
            HeadRow::CustomHint(hit) => hit.command.clone(),
            HeadRow::File(hit) => hit.path.clone(),
            HeadRow::Terminal(hit) => hit.command.clone(),
            HeadRow::TermHint(hit) => hit.title.clone(),
            HeadRow::Cheat(hit) => hit.entry.title.to_string(),
            HeadRow::CheatHint(hit) => hit.title.clone(),
            HeadRow::ScopeTags(hit) => hit.title.clone(),
            // 批次 P1：面板行的"这段文字本身"就是动作名——大字要的是"我现在选的是哪一条"。
            HeadRow::Action(hit) => hit.label.clone(),
        },
        (None, None) => query.to_string(),
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
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
        // 批次 P1（FR-ACT-03）：面板行的 Enter 落点＝执行高亮的那一条动作，词跟着行首徽标走
        // （「Enter 复制」「Enter 删除」——与结果模式同一套"徽标即动作类别"的口径）。
        Some(HeadRow::Action(hit)) => hit.id.badge(),
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

// ---- 批次 P1：操作面板（FR-ACT-03 进出 + FR-ACT-04 动作表）----

/// 批次 P1（FR-ACT-03）：操作面板的现场。`None` = 结果模式（平常那一屏）。
///
/// **面板不另开输入框**：过滤词就住在 [`LauncherApp::query`] 里——AHK 版同口径
/// （`_SetInput("")` 清空 + `Win.SetCueBanner` 把「关于〈项标题〉的操作」做到占位符上）。
/// 于是"恢复原输入文字"＝把进来时那一串原样写回，焦点、光标、TextEdit 的绑定一个都不碰。
struct PanelState {
    /// 进来时的查询原文。退出时一字不改写回（验收：进出面板后 `chr` 与选中的 Chrome 行都不丢）。
    saved_query: String,
    /// 进来时的高亮行（合并行号），退出时原样放回。
    saved_selected: usize,
    /// 动作表的输入：标题 / 种类 / 路径 / 能不能编辑。进面板时算一次，之后不再随帧变。
    target: actions::PanelTarget,
    /// 完整动作表（未按过滤词裁剪）。面板行序就是这里的序，`filter_actions` 只删不改序。
    actions: Vec<actions::Action>,
    /// 「打开」那一条具体怎么打开。**必须在进面板这一刻就定下来**：面板开着时查询框是过滤词，
    /// `{query}` 型自定义命令再按 `self.query` 去填就填错了东西。
    open: PanelOpen,
}

/// [`PanelState::open`] 此刻四种形态。批次 P1 写下"两种形态，差别只有一件事：**有没有清单身份**
/// （学习统计与脱敏日志都挂在身份上）"——那一句到批次 E5 只算部分成立：
/// `Entry` / `Path` / `Batch` 三档的差别确实是身份，而新加的 [`PanelOpen::Value`] 差的是**对象种类**
/// （那一档根本没有"打开"这个动作可做）。
enum PanelOpen {
    /// 清单里的条目：交给与 Enter 完全同一条 [`LauncherApp::launch_with_log`]
    /// （内置动作 `internal:` 的分派、自定义命令的脱敏日志都在里面），只是学习用的词换成进来时那串。
    Entry {
        /// 真正交给 `ShellExecuteExW` 的那份（自定义命令是已填好 `{query}` 的副本）。
        entry: LaunchItem,
        /// 清单侧身份。副本的派生 uid 与它不同，所以单列一份，不能用 `entry.uid()`。
        uid: String,
        /// 自定义命令：path/arguments 里带着用户敲的词 ⇒ 日志走脱敏那一条。
        masked: bool,
    },
    /// 只有路径 / URL（文件行、网页搜索行）：直接 `ShellExecuteExW`。
    /// 不进清单、不记学习——FR-SYS-03：这些串都是从查询框现搜出来的。
    Path,
    /// 批次 E4（FR-ACT-05）：一批被标记的路径。没有清单身份、没有"当前打开的那一个"——
    /// 表里七条动作各自对整个 `target.batch` 动手。
    Batch,
    /// 批次 E5（FR-ACT-01 第三格）：**一串文本**（计算器行的结果 / 换算值）。
    /// `target.path` 里放的就是那一串本身，所以这一档**没有"打开"**：把它交给
    /// `ShellExecuteExW` 等于让启动器去搜一个叫 `14` 的程序。表里只有复制 / 粘贴 / 大字三格，
    /// 都不会走到这里执行——`ActionId::Open` 明确拒绝这一档（见 [`LauncherApp::run_panel_action`]）。
    Value,
}

/// 面板里执行完一条动作之后的现场处置。四条出路语义上互斥，所以做成枚举而不是两个 bool。
enum PanelOutcome {
    /// 原样保留（复制 / 属性 / 打开方式 / 回收站 / 大字——用户下一步还在面板里）。
    Stay,
    /// 保留现场但换一份 target（置顶切换后「置顶 / 取消置顶」那条文案要跟着改，动作表要重建）。
    Reland(actions::PanelTarget),
    /// 关面板回结果模式（编辑 / 删除确认 / 添加为命令——现场交给了我们自己的弹窗）。
    Close,
    /// 窗口已经收到托盘（打开 / 提权 / 终端 / 所在位置），面板没资格留下：
    /// 下次呼出必须从结果模式重新开始。
    Gone,
}

/// FR-ACT-03 那句「仅"新按"，须过滤键盘自重复」。
///
/// egui 的 `key_pressed` / `num_presses` 把操作系统的自重复也算成"本帧按下了"，按住 `→`
/// 就会连跳进面板、再连执行动作——验收点名的正是这条，只能读原始事件里的 `repeat: false`。
/// 命中后还要把这些键事件**收走**：`→`/`←` 在这里做的是模式切换，同一帧不能再让 TextEdit
/// 拿去移光标（退出时 `set_query` 把光标放到末尾，TextEdit 若又吃到一条 `←` 就差一个字符，
/// 下一次 `→` 白按）。只 `retain` 掉目标键那一格，手法与 [`take_record_press`] 一致——
/// 清空整帧事件会让同一帧里的 Esc 和所有快捷键一起变聋（批次 R 踩过）。
fn take_fresh_key(ctx: &egui::Context, wanted: egui::Key) -> bool {
    let ours = |ev: &egui::Event| {
        matches!(ev,
            egui::Event::Key { key, modifiers, .. }
            if *key == wanted && *modifiers == egui::Modifiers::NONE
        )
    };
    let mut fresh = false;
    ctx.input_mut(|i| {
        fresh = i.events.iter().any(|ev| {
            ours(ev)
                && matches!(ev, egui::Event::Key { pressed: true, repeat: false, .. })
        });
        if fresh {
            i.events.retain(|ev| !ours(ev));
        }
    });
    fresh
}

/// 本帧有没有「不带任何修饰键的 Backspace 按下」——**只读不收**，这是它与 [`take_fresh_key`] 的分工。
///
/// 批次 B8（2026-10-26）用它换掉 `i.key_pressed(Backspace)`：`key_pressed` 不看修饰键，于是面板里
/// 过滤词为空时按 `Ctrl+Backspace`（egui 原生「删前一个词」）会**同时**被判成「退出面板」，
/// 一次按键干两件事，且删词那一半根本没字可删（真机抓到的正是这一格）。
/// 刻意不 `retain`：退格事件还要留给 TextEdit 做真正的删词/删字符，收走它等于把输入框的退格弄聋。
fn fresh_plain_backspace(ctx: &egui::Context) -> bool {
    ctx.input(|i| {
        i.events.iter().any(|ev| {
            matches!(ev,
                egui::Event::Key { key: egui::Key::Backspace, pressed: true, modifiers, .. }
                if *modifiers == egui::Modifiers::NONE)
        })
    })
}

// ---- 批次 E4（FR-ACT-05）：多标记与批量 ----

/// 这一行可标记的路径（`None` = 这一行不是文件/文件夹）。
///
/// 判据照需求书原文「仅 `file`/`folder` 且 `arg≠""`」：`arg` 在 AHK 版里就是文件/目录条目的
/// 路径字段（`ResultItem.Arg`），本程序对应 `LaunchItem::path` 与 `FileHit::path`。
/// 应用/命令/URL/命名项/提示行一律不可标记——批量动作全是文件系统动作，对它们没有意义。
fn markable_path(
    row: Row,
    head: &[HeadRow],
    results: &[usize],
    items: &[LaunchItem],
) -> Option<String> {
    match row {
        Row::Head(h) => match head.get(h)? {
            HeadRow::File(hit) => {
                let p = hit.path.trim();
                (!p.is_empty()).then(|| p.to_string())
            }
            _ => None,
        },
        Row::Item(rank) => {
            let it = items.get(*results.get(rank)?)?;
            let file_like = matches!(it.kind, ItemKind::File | ItemKind::Folder);
            let p = it.path.trim();
            (file_like && !p.is_empty()).then(|| p.to_string())
        }
    }
}

/// 标记集合的判定：Windows 路径不分大小写，`C:\A` 与 `c:\a` 是同一条。
fn is_marked(marked: &[String], path: &str) -> bool {
    marked.iter().any(|p| p.eq_ignore_ascii_case(path))
}

/// `Insert` 的切换。返回 `true` = 这一下是**加上**标记，`false` = 取消。
/// 保留首次加入的次序：批量动作按这个顺序执行（AHK 的 `Map` 也是插入序）。
fn toggle_mark(marked: &mut Vec<String>, path: &str) -> bool {
    match marked.iter().position(|p| p.eq_ignore_ascii_case(path)) {
        Some(pos) => {
            marked.remove(pos);
            false
        }
        None => {
            marked.push(path.to_string());
            true
        }
    }
}

/// 状态栏左格在有标记时的说法：先说清"现在攒着什么"，再说怎么取消、怎么动手。
fn marked_hint(count: usize) -> String {
    format!("已标记 {count} 项 · Insert 取消 · → 批量操作")
}

/// 一次按键是否命中某个**窗口内**热键（批次 E3-丙）。
///
/// 修饰键按"逐位相等"比：`Ctrl+Enter` 不该被 `Ctrl+Shift+Enter` 触发，也不该被裸 `Enter` 触发。
/// egui 的 `matches_logically` 恰恰相反——它忽略多出来的 Shift/Alt，[`take_fresh_key`] 为箭头键
/// 用的也是 `== NONE`，这里是同一条口径的显式写法。
///
/// `HKF_WIN` 一旦出现在配置里就直接不命中：egui 在 Windows 上根本不把 Win 当修饰键传上来
///（`Modifiers` 只有 `command`，那是 Ctrl 的副本），所以带 Win 的组合在这个入口永远等不到自己的
/// 那一帧。设置页的 [`HotkeySpec::check_secondary`] 已经挡住了这条路，这里只是让手改
/// `settings.json` 的人也不会得到一个"按什么都触发"的键。
fn local_hotkey_hits(spec: HotkeySpec, key: egui::Key, m: egui::Modifiers) -> bool {
    let named = spec.mods_named();
    if named & HKF_WIN != 0 || vk_from_egui_key(key) != Some(spec.vk) {
        return false;
    }
    m.ctrl == (named & HKF_CONTROL != 0)
        && m.alt == (named & HKF_ALT != 0)
        && m.shift == (named & HKF_SHIFT != 0)
}

/// 窗口内热键的取用：命中就**只收走这一个组合**的键事件并返回 `true`。
///
/// 为什么必须收走：`Enter` 那一档用的是 `key_pressed`，它不看修饰键 ⇒ 不收走的话
/// `Ctrl+Enter` 会同时跑次级动作和选中项，一次按键干两件事。为什么只 retain 掉自己那一格而不清空
/// 整帧：清空会把同一帧的 Esc 和所有固定快捷键一起变聋（批次 R 踩过这个坑，见 [`take_record_press`]）。
///
/// **自重复那一条也收走，但不回报命中**（批次 E6 修的缺陷，2026-10-27）。`repeat` 这一位是 egui
/// 自己按跨帧的 `keys_down` 改写的（`input_state/mod.rs:393` + `:400-413`
/// `let first_press = keys_down.insert(*key); *repeat = !first_press`），而 `num_presses` 数事件时
/// **不看** `repeat`（`:753-764`，文档原话 "Includes key-repeat events"）。原先只在 `fresh` 时
/// 才 retain ⇒ 第一帧之后那颗键的重复投递原样留在这一帧里，`key_pressed(Enter)` 就为真：
/// **按住 `Ctrl+Enter` 不放，第一次是定位、之后每一帧都在"启动选中项"**——一次按键先做一件事、
/// 接着连做另一件事。改成"组合是我的就收，只有新按才回报"，按住不放于是只有一次动作，
/// 与批次 P1 给 `→`/`←` 定的那条产品口径同型（「按住不放不会连跳」，见 [`take_fresh_key`]）。
fn take_local_hotkey(ui: &mut egui::Ui, spec: HotkeySpec) -> bool {
    let ours = |ev: &egui::Event| {
        matches!(ev,
            egui::Event::Key { key, modifiers, .. }
            if local_hotkey_hits(spec, *key, *modifiers)
        )
    };
    let mut fresh = false;
    ui.input_mut(|i| {
        fresh = i.events.iter().any(|ev| {
            ours(ev) && matches!(ev, egui::Event::Key { pressed: true, repeat: false, .. })
        });
        // 无条件 retain：见上面「自重复那一条也收走」。`ours` 只认这一组键 + 逐位相等的修饰键，
        // 所以收走的范围严格等于"这颗键自己的事件（含抬起）"，不会碰到裸 Enter 或别的组合。
        i.events.retain(|ev| !ours(ev));
    });
    fresh
}

/// FR-ACT-03 那句「且光标在末尾」：插入点不在查询末尾时 `→` 是 TextEdit 的"右移光标"，
/// 抢走它用户就没法从词中间往右翻。`CCursor::index` 是**字符**下标（与 [`set_query`] 里
/// `CCursor::new(chars)` 同一口径），所以跟 `chars().count()` 比，不跟字节长度比。
/// 读不到 `TextEditState`（输入框还没渲染过）按"不在末尾"处理——那种情况本来也不该进面板。
fn caret_at_end(ctx: &egui::Context, query: &str) -> bool {
    let Some(st) = egui::widgets::text_edit::TextEditState::load(ctx, search_field_id()) else {
        return false;
    };
    // 0.36 把插入点收在 `TextCursorState::char_range()` 里（返回 `Option`：从未点过输入框
    // 就没有 range），不是更早版本那个直接摆在外面的 `CCursorRange` 字段。
    let Some(range) = st.cursor.char_range() else {
        return false;
    };
    // `index` 是 `CharIndex`（0.36 的字符下标新类型，`.0` 才是那个 `usize`），
    // 所以右边也得按"字符数"给，按字节比会在中文查询上判错。
    range.primary.index.0 >= query.chars().count()
}

/// `Ctrl+C` 这一格让不让给输入框（批次 B8，FR-ACT-10 附录 A）。
///
/// 让出的条件与 egui 的 TextEdit **读同一份状态**：焦点在查询框且选区非空。
/// `builder.rs:1114` 那句就是 `Event::Copy => if !cursor_range.is_empty() { 复制选区 }`，
/// 而它用的 `state.cursor` 正是这里 `load` 出来的那一份（折叠选区在 `builder.rs:556-566`
/// 就地写回）⇒ 判据不可能和控件自己的判断错开。没有选区时 TextEdit 对 `Copy` 什么都不做，
/// 这一格才归本程序的「复制这一行」——与 AHK 版
/// `SearchWindow.ahk:1408` 的 `if (shift || _HasTextSelection()) return;` 同口径。
fn query_box_owns_copy(ctx: &egui::Context) -> bool {
    if !ctx.memory(|mem| mem.has_focus(search_field_id())) {
        return false;
    }
    let Some(st) = egui::widgets::text_edit::TextEditState::load(ctx, search_field_id()) else {
        return false;
    };
    // `char_range()` 为 `None` = 从没点过输入框；`is_empty()` = 只有一个插入点、没有选区。
    st.cursor.char_range().is_some_and(|r| !r.is_empty())
}

/// 这一帧有没有 `Ctrl+C`（批次 B8）。
///
/// **不能**写成 `consume_key(MODIFIERS_COMMAND, Key::C)`：egui-winit 在源头就把 Ctrl+C 折成
/// 一条 `Event::Copy` 然后 `return`（`egui-winit-0.36.2/src/lib.rs:1019-1027`），
/// `Event::Key{ C, ctrl }` 根本不会进队列 ⇒ 那条分支编译通过、单测只要不投 `Event::Copy`
/// 也照样"通过"，真机上却是一颗永远不响的死键（`consume_key` 那一类"绿检 ≠ 对测"的老坑，
/// 见 [[feedback-workflow-rules]]）。命中后把这条 `Copy` 摘掉：不摘的话同帧稍后渲染的
/// TextEdit 也会吃到它，一次按键往剪贴板写两遍。
fn take_copy_press(events: &mut Vec<egui::Event>, yield_to_text_edit: bool) -> bool {
    if yield_to_text_edit {
        return false;
    }
    let Some(pos) = events.iter().position(|e| matches!(e, egui::Event::Copy)) else {
        return false;
    };
    events.remove(pos);
    true
}

/// 滚轮 → 本帧的选中位移（`-1` / `0` / `+1`），批次 B8（FR-ACT-10 附录 A「滚轮移动选中」）。
///
/// 只看纵向净增量的**符号**、不看量：一格滚轮移一行是 AHK `_OnMouseWheel`
/// （`SearchWindow.ahk:1542` `MoveSelection(delta >= 0x8000 ? 1 : -1)`）的口径；照像素走会因为
/// 鼠标不同、`line_scroll_speed` 不同而"一格跳好几行"。同一帧内先求和 ⇒ 上下各来一条等于没动。
///
/// 符号两头都对过源码：`delta.y > 0` 是"往上滚"（`event.rs:157-161` 说正向=内容向下移、露出前面的行，
/// `scroll_area.rs:1234` 也正是拿正向增量去减 `offset`）⇒ 选中往**上**走 `-1`，与 AHK 反向那半格一致。
///
/// 读原始 `Event::MouseWheel` 而不是 `i.smooth_scroll_delta()`：后者被 egui 的滚轮平滑器拆成几帧
/// 吐出（`input_state/wheel_state.rs:138-143` 对 `Unit::Line` 一律先存进 `unprocessed_wheel_delta`，
/// 再由 `after_events` 逐帧分账），按住一格滚轮会让选中连跳好几行；而那个状态机是
/// `InputState` 的**私有字段**（`mod.rs:227`），外面清不掉 ⇒ 只能不去读它。
/// 配套的"这一帧别让列表自己吃滚轮"在渲染处用 `ScrollSource { mouse_wheel: false }` 关。
/// 带任何修饰键的滚轮不接管（Shift=横向、Alt=纵向覆盖、Ctrl=缩放，见 `mod.rs:117-118`），留给 egui。
fn wheel_selection_step(events: &[egui::Event]) -> i32 {
    let net: f32 = events
        .iter()
        .filter_map(|e| match e {
            egui::Event::MouseWheel { delta, modifiers, .. }
                if *modifiers == egui::Modifiers::NONE =>
            {
                Some(delta.y)
            }
            _ => None,
        })
        .sum();
    if net > 0.0 {
        -1
    } else if net < 0.0 {
        1
    } else {
        0
    }
}

/// 批次 C3（FR-QRY-10）：`Ctrl+↑↓` 这一刻该不该归"翻搜索历史"。判据单独成函数是为了**能测**
/// ——`ui()` 里那份现场（egui 上下文、弹窗状态）打不开，而这三条让位本身是纯的。
///
/// 三条与 AHK `SearchWindow.RecallHistory` 开头那两句一一对上（`SearchWindow.ahk:1429-1431`），
/// 少一条就是"在别的地方按 ↑↓ 会跳走搜索历史"：
/// - **面板模式**（AHK 的 `Mode = "actions"`）：框里那几个字是过滤词，↑↓ 属于那张动作表；
///   回填一串查询进面板的过滤框，得到的是"动作列表被过滤成空"，不是用户要的东西。
/// - **文件模式**（AHK 的 `FileMode`）：↑↓ 属于目录浏览/文件结果，同理让位。
/// - **弹窗类现场开着**：Enter 都归"确定"了，翻历史更靠后（[`LauncherApp::keys_live`]）。
///
/// 设置页与取键录制态不写在参数里：调用点外面那道总闸（`!recording` / `!settings_open`）
/// 已经把它们挡了，这里再列一遍只会让人以为这两条是可选的。
fn recall_history_allowed(panel_open: bool, in_file_mode: bool, keys_live: bool) -> bool {
    !panel_open && !in_file_mode && keys_live
}

/// 「这一串此刻在不在盘上」——只有对该形态是路径的串才去 `stat`，
/// URL / `shell:` / `internal:` 一律 false（`actions_for` 里那几格本来就不看 `exists`）。
/// 清单条目不走这里，走 FR-SRC-06 那份后台缓存：离线 UNC 在主线程 `stat` 会把整帧挂住。
fn fs_exists(path: &str) -> bool {
    actions::is_fs_path(path) && std::path::Path::new(path).exists()
}

/// 批次 E3：启动期写盘门禁的判据。单独成函数是为了**能测**——`LauncherApp::new` 里那份
/// 现场（通道、egui 上下文）测不起来，而"哪份配置的留证没成功"这个决定本身是纯的。
/// 任一份是 `Lost`（改名与另存副本都没成、坏文件还挡在原位）⇒ 本轮启动的六处补齐/去重
/// 一律不落盘：写一次就覆盖掉唯一证据。
///
/// 批次 C3 起改成切片入参：可有可无的文件再加一份（`search_history.json`）不该逼着
/// 每条调用点和每道形参再长一格。
fn lost_evidence_labels<'a>(
    faults: &[(&'a str, &'a Option<Corrupted>)],
) -> Vec<&'a str> {
    faults
        .iter()
        .filter(|(_, c)| matches!(c, Some(Corrupted::Lost)))
        .map(|(label, _)| *label)
        .collect()
}

/// 「所在位置」的落点。先按 Ctrl+D 那套找**文件**（[`resolve_file_for_select`] 的判据是
/// `is_file()`），落空了再认**目录**——FR-ACT-04 的验收点名的正是文件夹那一格
/// （「对文件夹『所在位置』打开父目录并选中」），只复用文件判据的话文件夹行永远定不了位，
/// 而 `explorer /select,` 对目录本来就是有效的。
fn reveal_target(path: &str, source_path: &str) -> Option<String> {
    let pick = |p: &str| -> Option<String> {
        let t = p.trim();
        if t.is_empty() {
            return None;
        }
        let expanded = source::application::expand_vars(t);
        let pb = std::path::PathBuf::from(&expanded);
        (pb.is_absolute() && pb.is_dir()).then_some(expanded)
    };
    resolve_file_for_select(path, source_path).or_else(|| pick(path).or_else(|| pick(source_path)))
}

/// 「用户在系统那句警告里点了否」：`ERROR_CANCELLED(1223)` 经 `HRESULT_FROM_WIN32` 变成
/// `0x800704C7`。UAC 拒绝提权、`FOF_WANTNUKEWARNING` 那句"永久删除"被驳回，都是这一个码。
/// 它不是故障 ⇒ Toast 要说「已取消」，不能写「失败：…」。
/// （`recycle_path` 那边拿到的是裸 Win32 码 1223，`SHFileOperationW` 不返回 HRESULT。）
fn user_cancelled(e: &windows::core::Error) -> bool {
    e.code() == windows::core::HRESULT::from_win32(1223)
}

/// 把「当前高亮行」翻译成面板需要的两份数据（动作表输入 + 「打开」的可执行形态）。
/// 返回 `None` = 这一行没有可处置的对象 ⇒ `→` 保持 TextEdit 的原义，不抢键。
///
/// 「哪些行有面板」与「Enter 按下去干什么」是两件事，判据**刻意不同源**：~~计算器那一串数字~~
/// 没有路径可复制、终端行是一条命令而不是一个东西、`valid=false` 的提示行压根不该被执行——
/// 硬给它们一张动作表只会供出一排按了报错的按钮。
/// 【2026-10-27 批次 E5 更正：计算器行**已经进了**这张判据——但它用的不是路径那张表，
/// 是 [`actions::Subject::Text`] 那张三格表（复制这一串 / 粘贴到原窗口 / 大字显示），
/// 一条会走到 `ShellExecuteExW` 的动作都没有，所以"供出一排按了报错的按钮"这个担心
/// 对文本对象不成立。终端行与提示行仍然按原话排除。】
///
/// 参数拆成 `head` / `results` 两片而不是 `&Plan`：渲染那一帧把 `plan.results` 搬出来当局部量
/// 用了（条目循环要按下标取），整块 `&plan` 在那里是"部分移动后再借用"，编译不过。
fn panel_for_row(
    row: Row,
    head: &[HeadRow],
    results: &[usize],
    items: &[LaunchItem],
    path_states: &std::collections::HashMap<String, source::pathcheck::PathState>,
) -> Option<(actions::PanelTarget, PanelOpen)> {
    // 清单那一格：uid / 可编辑 / exists（四态缓存）都跟着身份走。
    // `is_dir` 用**声明的 kind**，不新起一次 `stat`：目录型条目入库时就是 Folder。
    let target_of = |it: &LaunchItem| -> actions::PanelTarget {
        let exists = match path_states.get(&it.uid()) {
            // 缓存里没这一条（刚入库 / 还没轮到扫它）⇒ 当作在：四态只在"确认不在"时才说不在。
            Some(s) => !s.is_problem(),
            None => true,
        };
        actions::PanelTarget {
            title: it.name.clone(),
            subject: actions::subject_for(it.kind, &it.path, it.kind == ItemKind::Folder),
            path: it.path.clone(),
            arguments: it.arguments.clone(),
            exists,
            editable: true,
            uid: Some(it.uid()),
            pinned: it.pinned,
            batch: Vec::new(),
        }
    };
    match row {
        Row::Item(rank) => {
            let it = items.get(*results.get(rank)?)?;
            let uid = it.uid();
            Some((target_of(it), PanelOpen::Entry { entry: it.clone(), uid, masked: false }))
        }
        Row::Head(h) => match head.get(h)? {
            // 自定义命令的独占行：`hit.entry` 是已把 `{query}` 填好的副本，uid 仍是清单里那一条。
            HeadRow::Custom(hit) => {
                let it = items.iter().find(|i| i.uid() == hit.uid)?;
                Some((
                    target_of(it),
                    PanelOpen::Entry {
                        entry: hit.entry.clone(),
                        uid: hit.uid.clone(),
                        masked: true,
                    },
                ))
            }
            HeadRow::File(hit) => Some((
                actions::PanelTarget {
                    title: hit.title.clone(),
                    subject: actions::subject_for(ItemKind::File, &hit.path, hit.is_dir),
                    path: hit.path.clone(),
                    arguments: String::new(),
                    // 这一行是刚列出来 / 查出来的，不存在就不会出现在这里。
                    exists: true,
                    editable: false,
                    uid: None,
                    pinned: false,
                    batch: Vec::new(),
                },
                PanelOpen::Path,
            )),
            HeadRow::Search(hit) => Some((
                actions::PanelTarget {
                    title: hit.title.clone(),
                    subject: actions::subject_for(ItemKind::Url, &hit.url, false),
                    path: hit.url.clone(),
                    arguments: String::new(),
                    exists: fs_exists(&hit.url),
                    editable: false,
                    uid: None,
                    pinned: false,
                    batch: Vec::new(),
                },
                PanelOpen::Path,
            )),
            // 批次 E5（FR-ACT-01 第三格）：计算器行第一次有动作表。
            // 对象是**那一串文本**，不是路径：`path` 里放 `hit.copy`（不带千分位、不带单位的那一份，
            // 与 Enter 复制的是同一个串），`title` 用行上显示的那一句（`= 1,024`）只为大字显示用。
            // `editable: false` + `uid: None` ⇒ 表里没有编辑 / 删除 / 置顶；`exists: true`
            // 是因为"这一串刚算出来"不存在失效一说（四态只管磁盘上的东西）。
            HeadRow::Calc(hit) => Some((
                actions::PanelTarget {
                    title: hit.title.clone(),
                    subject: actions::Subject::Text,
                    path: hit.copy.clone(),
                    arguments: String::new(),
                    exists: true,
                    editable: false,
                    uid: None,
                    pinned: false,
                    batch: Vec::new(),
                },
                PanelOpen::Value,
            )),
            // 终端 / 各类提示行（`valid=false`）/ 分类行：没有可处置的对象。
            _ => None,
        },
    }
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

/// 取**任意**窗口的 IME 窗口句柄（跨进程可用：下面全是 `SendMessageTimeout`，
/// 与 AHK 版 `Win.SetImeState` 同构）。批次 E5 往原窗口打字前要先问它这一句。
fn ime_wnd_of(hwnd: HWND) -> Option<HWND> {
    let ime_wnd = unsafe { ImmGetDefaultIMEWnd(hwnd) };
    if ime_wnd.0.is_null() {
        None
    } else {
        Some(ime_wnd)
    }
}

/// 获取当前前台窗口的 IME 窗口句柄
fn foreground_ime_wnd() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    ime_wnd_of(hwnd)
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
    shell_execute_verb(file, None, params, dir, SW_SHOWNORMAL.0)
}

/// 批次 B7（FR-ACT-08）：运行标志改的就是这一格 `nShow` —— Delphi 也把 `ShowCmd` 交给
/// `ShellExecute` 的最后一个参数（`untShortCutMan.pas:341`），两条路径同一个位置。
fn shell_execute_show(
    file: &str,
    params: &str,
    dir: Option<&str>,
    nshow: i32,
) -> windows::core::Result<()> {
    shell_execute_verb(file, None, params, dir, nshow)
}

/// [`source::runflags::ShowFlag`] → `nShow`。四档都取 Win32 具名常量，不手抄数字：
/// 最大化是 3、最小化是 2，抄反了界面上一眼看不出，只会"按了却变成另一种"。
fn nshow(flag: source::runflags::ShowFlag) -> i32 {
    match flag {
        source::runflags::ShowFlag::Normal => SW_SHOWNORMAL.0,
        source::runflags::ShowFlag::Maximized => SW_SHOWMAXIMIZED.0,
        source::runflags::ShowFlag::Minimized => SW_SHOWMINIMIZED.0,
        source::runflags::ShowFlag::Hidden => SW_HIDE.0,
    }
}

/// [`shell_execute`] 的带动词版本（批次 P1，FR-ACT-04）。动词有两处用到：
/// `runas`（以管理员运行）与 `properties`（属性框的兜底，见 [`show_properties`]）。
///
/// 两点与默认动词不同：① Store 的 AUMID 串**不走** `activate_uwp`——那个入口只认"打开"，
/// 提权/属性对它没有意义（面板里那两档本来就不给 Store 应用）；② `SEE_MASK_FLAG_NO_UI`
/// 挡的是"没有关联程序"那类系统错误框，**不挡 UAC**：`runas` 照样弹提权，用户拒绝后
/// 返回 `ERROR_CANCELLED(1223)`，调用方据此说"已取消"而不是"失败"。
fn shell_execute_verb(
    file: &str,
    verb: Option<&str>,
    params: &str,
    dir: Option<&str>,
    nshow: i32,
) -> windows::core::Result<()> {
    // 清单里的路径可能是 `%VAR%` 形态（出厂「启动器目录」用的是 `%LAUNCHER_DIR%`，
    // 要跟着 exe 搬家）——ShellExecuteExW 不替我们展开，所以在这里统一走一次。
    // `expand_env` 对未定义的 `%…%` 原样保留，用户写错的变量不会被悄悄吃掉。
    let file = source::custom::expand_env(file);
    if verb.is_none() {
        if let Some(aumid) = source::application::uwp_aumid(&file) {
            return source::application::activate_uwp(aumid);
        }
    }
    let params = source::custom::expand_env(params);
    let dir = dir.as_ref().map(|d| source::custom::expand_env(d));
    let wfile = wide_z(&file);
    let wverb = verb.map(wide_z);
    let wparams = wide_z(&params);
    let wdir = dir.as_ref().map(|d| wide_z(d));
    let mut sei: SHELLEXECUTEINFOW = unsafe { core::mem::zeroed() };
    sei.cbSize = core::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_FLAG_NO_UI;
    sei.lpVerb = wverb.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    sei.lpFile = PCWSTR(wfile.as_ptr());
    sei.lpParameters = PCWSTR(wparams.as_ptr());
    sei.lpDirectory = PCWSTR(wdir.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()));
    sei.nShow = nshow;
    unsafe { ShellExecuteExW(&mut sei) }
}

/// FR-ACT-04「以管理员运行」。参数与「打开」同一条通道，只多一个 `runas` 动词。
/// 工作目录跟着条目的 `working_dir` 走（与不提权那一档对齐，不额外加限制）。
fn shell_execute_runas(file: &str, params: &str, dir: Option<&str>) -> windows::core::Result<()> {
    shell_execute_verb(file, Some("runas"), params, dir, SW_SHOWNORMAL.0)
}

/// FR-ACT-04「打开方式…」。走系统自己的 `OpenAs_RunDLL`，不自己列候选程序。
///
/// 路径**不加引号**（与 AHK `ActionCatalog.OpenWith` 同一口径）：`OpenAs_RunDLL` 把剩下的
/// 整条命令行当作目标名，加了引号反而把引号算进文件名。
fn open_with_dialog(path: &str) -> windows::core::Result<()> {
    shell_execute(
        "rundll32.exe",
        &format!("shell32.dll,OpenAs_RunDLL {path}"),
        None,
    )
}

/// FR-ACT-04「属性」。`SHObjectProperties(0, SHOP_FILEPATH, path, 0)` 与资源管理器里
/// 右键→属性是同一条调用；它对某些形态（网络盘上的文件夹一类）返回 FALSE，
/// 这时回落到 `properties` 动词——需求书那条验收「对网络盘文件夹『属性』不报错」靠的就是
/// 这一对兜底，只留第一道会静默什么都不发生。兜底的返回码不当成功判据：能起起来就算成功。
fn show_properties(path: &str) -> bool {
    let by_api = unsafe {
        // 主线程此前不常驻 COM 公寓（只有后台扫描线程和 UWP 启动那一下会初始化）。
        // 这里补一次 STA：已初始化返回 S_FALSE、被别的组件设成别的公寓返回 RPC_E_CHANGED_MODE，
        // 两种都不影响这次调用，所以**不配对 CoUninitialize**（计数里可能有别人的那一份）。
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let w = wide_z(path);
        SHObjectProperties(None, SHOP_FILEPATH, PCWSTR(w.as_ptr()), PCWSTR::null()).as_bool()
    };
    if by_api {
        return true;
    }
    shell_execute_verb(path, Some("properties"), "", None, SW_SHOWNORMAL.0).is_ok()
}

/// FR-ACT-04「移入回收站」。用 Windows 自己的删除（`SHFileOperationW` + `FOF_ALLOWUNDO`），
/// 于是进度框、重名询问、回收站还原这些系统行为全都免费。
///
/// 标志里刻意带上 `FOF_WANTNUKEWARNING`：`ALLOWUNDO` 只保证"能进回收站的时候"进回收站，
/// 网络盘、超大文件、被策略禁止的卷上它会**永久删除**——那一句警告必须由系统问，不能替用户跳过。
/// `pFrom` 要求「双 NUL 结尾」：`wide_z` 补的是数组末尾那一个，所以字符串本身也得带一个 `\0`。
fn recycle_path(path: &str) -> Result<(), u16> {
    let from = wide_z(&format!("{path}\0"));
    let mut op = SHFILEOPSTRUCTW::default();
    op.wFunc = FO_DELETE;
    op.pFrom = PCWSTR(from.as_ptr());
    op.fFlags = (FOF_ALLOWUNDO | FOF_WANTNUKEWARNING | FOF_NOCONFIRMATION | FOF_SILENT).0 as u16;
    let code = unsafe { SHFileOperationW(&mut op) };
    if code == 0 {
        Ok(())
    } else {
        // 用户在那句"永久删除"警告里点了否，系统返回 1223（ERROR_CANCELLED）——
        // 这不是故障，调用方要按"取消"说。
        Err(code as u16)
    }
}

/// 批次 D4（FR-CFG-06）：「选文件…」「选目录…」共用的系统选择框。
/// `want_dir = true` ⇒ `FOS_PICKFOLDERS`（批次 E4「复制到」原来那条路）；
/// `false` ⇒ `FOS_FILEMUSTEXIST`（只能挑已存在的文件，防止手打一个不存在的名字）。
/// 返回 `Ok(None)` = 用户取消（这不是故障，调用方要说「已取消」而不是「失败」）。
///
/// 走 COM 而不是 `SHBrowseForFolder`：后者是 IE 时代的老对话框，没有地址栏、不能贴路径，
/// 用户要挑一个深层目录时几乎没法用。COM 公寓在这里补一次 STA（与 `show_properties` 同一口径，
/// 已初始化/被别人设成别的公寓都不影响 in-proc，所以**不配对 `CoUninitialize`**）。
fn pick_path(want_dir: bool) -> Result<Option<String>, String> {
    let what = if want_dir { "文件夹" } else { "文件" };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("{what}选择框创建失败：{e}"))?;
        let mut opts = dialog
            .GetOptions()
            .map_err(|e| format!("读取对话框选项失败：{e}"))?;
        opts |= FOS_FORCEFILESYSTEM;
        if want_dir {
            opts |= FOS_PICKFOLDERS;
        } else {
            opts |= FOS_FILEMUSTEXIST;
        }
        dialog
            .SetOptions(opts)
            .map_err(|e| format!("设置对话框选项失败：{e}"))?;
        // 1223 = 用户点「取消」。它不是错误 ⇒ 返回 None，让上层说「已取消」。
        if let Err(e) = dialog.Show(None) {
            return if e.code() == windows::core::HRESULT::from_win32(1223) {
                Ok(None)
            } else {
                Err(format!("{what}选择框失败：{e}"))
            };
        }
        let item = dialog
            .GetResult()
            .map_err(|e| format!("取{what}选择结果失败：{e}"))?;
        let raw = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|e| format!("取{what}路径失败：{e}"))?;
        let text = raw.to_string().unwrap_or_default();
        let _ = CoTaskMemFree(Some(raw.as_ptr() as *const core::ffi::c_void));
        Ok(Some(text).filter(|s| !s.trim().is_empty()))
    }
}

/// 批次 E4（FR-ACT-05）「复制到」的入口：只要目录。
fn pick_folder() -> Result<Option<String>, String> {
    pick_path(true)
}

/// 批次 D4（FR-CFG-06）「选文件…」的入口：只要文件。
fn pick_file() -> Result<Option<String>, String> {
    pick_path(false)
}

/// `path` 是否就是 `dest` 或位于 `dest` 之下（不分大小写，尾分隔符先剥掉）。
/// 批次 E4「复制到」的第一道闸：把文件夹复制进它自己（或它的子孙）会被系统报错，
/// 更糟的是资源管理器会真的开始无限递归复制。AHK 版同一道闸（`CopyMany` 里那个 `InStr = 1`）。
fn same_or_under(path: &str, dest: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches(['\\', '/']).to_ascii_lowercase();
    let p = norm(path);
    let d = norm(dest);
    if p.is_empty() || d.is_empty() {
        return false;
    }
    p == d || {
        // 逐段比前缀：`D:\a` 不能把 `D:\ab` 当成自己的子目录。
        let mut prefix = d.clone();
        prefix.push('\\');
        p.starts_with(&prefix)
    }
}

/// 「复制到」的执行：`SHFileOperationW` + `FO_COPY`，多个来源、一个目标目录。
/// 用系统自己的复制（有进度条、有重名询问、可取消），返回实际参与复制的条数。
///
/// 与 `recycle_path` 同一条错误口径：`Err(u16)` 是裸 Win32 码，1223 = 用户取消。
/// `pFrom` 要「双 NUL 结尾」，所以每条路径自己带一个 `\0`，末尾再补一个（`wide_z` 又补一个）。
fn copy_many_to(paths: &[String], dest: &str) -> Result<usize, u16> {
    let usable: Vec<&str> = paths
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty() && !same_or_under(p, dest))
        .collect();
    if usable.is_empty() {
        return Ok(0);
    }
    let from: String = usable
        .iter()
        .map(|p| format!("{p}\0"))
        .collect::<String>()
        + "\0";
    let from = wide_z(&from);
    let to = wide_z(&format!("{dest}\0"));
    let mut op = SHFILEOPSTRUCTW::default();
    op.wFunc = FO_COPY;
    op.pFrom = PCWSTR(from.as_ptr());
    op.pTo = PCWSTR(to.as_ptr());
    // 不给 `FOF_NOCONFIRMATION`：重名要不要覆盖必须由系统问用户。
    // 也不给 `FOF_SILENT`：复制大文件时那根进度条是"还在干活"的唯一凭据。
    op.fFlags = 0;
    let code = unsafe { SHFileOperationW(&mut op) };
    if code == 0 {
        Ok(usable.len())
    } else {
        Err(code as u16)
    }
}

/// 两条路径是否指向同一个目标（不分大小写、尾分隔符不计）。
/// 「全部加入命令」的去重判据：AHK `CustomCommandProvider.FindByTarget` 同口径。
fn same_target_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches(['\\', '/']).to_ascii_lowercase();
    !a.trim().is_empty() && norm(a) == norm(b)
}

/// 批次 E4「全部加入命令」：一条路径 → 一条清单条目，形态照单条的「添加为命令」
/// （`DialogState::from_input` → `to_item`）：名称取末段、别名按拼音首字母生成、图标源就是它自己。
/// 目录与文件的 `kind` 当场 `is_dir` 判——收进来的路径都是从结果列表里挑的，盘上基本都在。
fn command_item_from_path(path: &str) -> LaunchItem {
    let trimmed = path.trim();
    let bare = trimmed.trim_end_matches(['\\', '/']);
    let is_dir = std::path::Path::new(bare).is_dir();
    let leaf = bare.rsplit(['\\', '/']).next().unwrap_or(bare);
    let leaf = leaf.trim_end_matches(".lnk");
    let name = if leaf.is_empty() {
        trimmed.to_string()
    } else {
        leaf.to_string()
    };
    let keyword = source::application::default_keyword(&name);
    LaunchItem {
        name,
        path: trimmed.to_string(),
        arguments: String::new(),
        working_dir: None,
        source_path: trimmed.to_string(),
        kind: if is_dir {
            ItemKind::Folder
        } else {
            ItemKind::File
        },
        keyword,
        ..Default::default()
    }
}

/// 把搜索框的插入点放到末尾。程序化换掉 `self.query` 不会带着 `TextEditState` 里的光标走
/// ⇒ 接着打字会插进词中间（批次 F 的 Tab 补全踩过，[`LauncherApp::set_query`] 里内联做了同一件事）。
/// 这个函数只给"拿不到 `ctx` 的那条改写路径"用：`on_hide` 是被隐藏流程调起来的，
/// 只能取 `CTX` 那个静态句柄。
fn caret_to_end(ctx: &egui::Context, chars: usize) {
    if let Some(mut st) = egui::widgets::text_edit::TextEditState::load(ctx, search_field_id()) {
        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(chars),
        )));
        st.store(ctx, search_field_id());
    }
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

/// F4（批次 B8，FR-ACT-10 附录 A「打开配置文件」）：打开配置所在的目录。
///
/// 口径偏差入档：需求书这一句在 AHK 版对着"打开 `settings.ini` 那一个文件"，本程序的现场是
/// 同一个数据目录里的**两份 JSON 加一份日志**（`settings.json` / `user_items.json` /
/// `launcher.log`）⇒ 没有"那一个文件"可开，开目录让用户自己在资源管理器里挑。
/// 走 [`shell_execute`] 的系统关联程序（与「打开日志」同一条路），不另起 `explorer.exe`。
/// 返回 `Err(原因)` 给调用方出 Toast：点了没反应比点了有话说更糟。
fn open_config_folder() -> Result<(), String> {
    let dir = source::application::data_dir();
    if !dir.exists() {
        return Err(format!("配置目录还不存在：{}", dir.display()));
    }
    let path = dir.display().to_string();
    shell_execute(&path, "", None).map_err(|e| format!("打开配置目录失败：{e}"))?;
    eprintln!("快捷键 F4：打开配置目录（{path}）");
    Ok(())
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
/// 「清除副呼出热键」（批次 G1）。与上面那条同一个理由：注销只能在热键线程做。
const WM_APP_CLEAR_EXTRA: u32 = WM_APP + 5;
/// 「双击呼出换档」（批次 G1）。`wparam` = [`dbltap::DblTap::code`]（0 关 / 1 Ctrl / 2 Shift）。
/// 为什么这件事也要投消息：低级键盘钩子`WH_KEYBOARD_LL` 的回调**跑在装钩子的那条线程**
/// （系统往它的消息队列里投），所以钩子只能在热键线程装、也只能在热键线程卸。
const WM_APP_SET_DOUBLE_TAP: u32 = WM_APP + 6;
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
    /// 批次 G1：副呼出键（`extra_hotkey`）。
    Extra,
}

impl HotkeySlot {
    fn id(self) -> i32 {
        match self {
            Self::Toggle => HOTKEY_TOGGLE,
            Self::Typeback => HOTKEY_TYPEBACK,
            Self::Extra => HOTKEY_EXTRA,
        }
    }

    /// 日志与 Toast 里怎么称呼这个槽。
    fn label(self) -> &'static str {
        match self {
            Self::Toggle => "全局呼出热键",
            Self::Typeback => "补字符热键",
            Self::Extra => "副呼出热键",
        }
    }

    /// 系统里此刻真正装着的那一组存在哪个槽里（`None` = 这个键当前没装）。
    fn active(self) -> &'static Mutex<Option<HotkeySpec>> {
        match self {
            Self::Toggle => &HOTKEY_ACTIVE,
            Self::Typeback => &TYPEBACK_ACTIVE,
            Self::Extra => &EXTRA_ACTIVE,
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
/// 副呼出键的同一份口径（批次 G1；`None` = 不用，或用了但注册失败）。
static EXTRA_ACTIVE: Mutex<Option<HotkeySpec>> = Mutex::new(None);
/// 此刻装着的低级键盘钩子句柄（0 = 没装）。`HHOOK` 是裸指针、不 `Sync`，存数值。
/// 只有热键线程读写它（装/卸都在 `set_tap_hook` 里，而它只在热键线程被调用）。
static TAP_HOOK: AtomicUsize = AtomicUsize::new(0);
/// 双击判定状态。**只有 `keyboard_hook_proc` 与 `set_tap_hook` 在锁它**，两者都跑在热键线程
/// ⇒ 没有真正的并发等待；写成 `Mutex` 只是因为 `static` 要的是 `Sync`，而 `TapTracker` 有可变状态。
static TAP_TRACKER: Mutex<TapTracker> = Mutex::new(TapTracker::new(DblTap::Off));
/// 判定要的是"毫秒数"，钩子回调里每次 `Instant::now()` 差一个共同零点即可。
static TAP_EPOCH: OnceLock<Instant> = OnceLock::new();
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

/// 本程序自己占用的组合（录制端落定前的预检表）。**表里的每一条都对着 `ui()` 里一次真实的取键**
/// （`consume_key`、`take_fresh_key`，以及批次 B8 那一下走 `take_copy_press` 的复制）
/// （Ctrl+Shift+N 新建、Ctrl+E 编辑、Ctrl+I 批量导入、Ctrl+D 打开目录、
/// Ctrl+Shift+D 删除、Ctrl+Shift+P 置顶、Ctrl+N/P 与 Ctrl+J/K 循环导航、Ctrl+M 开 ☰ 主菜单、
/// Ctrl+L 大字显示、Ctrl+数字 与 Alt+数字 执行第 N 行），
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
/// 【2026-10-26 批次 B8 破一处先例：上面那句"裸键录不进来"**对 F1–F4 不成立**——
/// [`HotkeySpec::check`] 对功能键放行不带修饰（`application.rs` 里 `if function { return Ok(()) }`
/// 那一格，判据在 `check` 开头），
/// 所以裸 `F1`…`F4` 是录得出来的组合，本程序绑了它们就必须进表。同批的 `Ctrl+C` 更硬：
/// egui-winit 把 Ctrl+C 折成 `Event::Copy` 后**不再投递 `Event::Key`**
/// （`egui-winit-0.36.2/src/lib.rs:1019-1027`），窗口内热键录成这一组会永远等不到自己那一帧。】
/// 表里的修饰键一律不带 `HKF_NOREPEAT`（比对时两边都剥）。
const LOCAL_TAKEN: &[(u32, u32)] = &[
    // 批次 C3（FR-QRY-10）的 `Ctrl+↑` / `Ctrl+↓` **不列在这里**，是单测挡下来的（下表第一条断言
    // 就是"表里每一格必须真录得出来"）：`HotkeySpec::check` 只收字母、数字、空格、回车、反引号
    // 与 F1–F24，方向键 0x26/0x28 过不了它，`vk_from_egui_key` 也不认这两个变体（录制端对方向键
    // 的口径是"继续等，不提示"）⇒ 用户把全局键和次级动作键都录不成这一颗，拦它等于白占一行。
    // 窗口内取键不经过这张表：`ui()` 里在查询框渲染之前直接 `consume_key(COMMAND, ArrowUp/Down)`。
    (HKF_CONTROL, 0x43),             // Ctrl+C 复制这一行（批次 B8；复制的不是选中文本，见 take_copy_press）
    (HKF_CONTROL, 0x44),             // Ctrl+D 打开所在目录
    (HKF_CONTROL, 0x45),             // Ctrl+E 编辑选中项
    (HKF_CONTROL, 0x47),             // Ctrl+G 缺省分类范围 all ⇄ basic（批次 G1）
    (HKF_CONTROL, 0x49),             // Ctrl+I 批量导入全部
    (HKF_CONTROL, 0x4A),             // Ctrl+J 循环下一项
    (HKF_CONTROL, 0x4B),             // Ctrl+K 循环上一项
    (HKF_CONTROL, 0x4C),             // Ctrl+L 大字显示（FR-UIX-10）
    (HKF_CONTROL, 0x4D),             // Ctrl+M ☰ 主菜单（批次 F6：新建按钮换成菜单后给它配的键）
    (HKF_CONTROL, 0x4E),             // Ctrl+N 循环下一项（2026-10-22 从「新建」改为导航）
    (HKF_CONTROL, 0x50),             // Ctrl+P 循环上一项（2026-10-22 从「置顶」改为导航）
    // 批次 D5（FR-ACT-09）：`Ctrl+B` 进设置页「批量管理」、`Ctrl+R` 在该页重新校验路径。
    // 两颗都是"窗口内绑了就必须进表"的口径：`RegisterHotKey` 一吞，窗口里那颗键就此失聪。
    (HKF_CONTROL, 0x42),             // Ctrl+B 设置页·批量管理（批次 D5）
    (HKF_CONTROL, 0x52),             // Ctrl+R 批量管理·重新校验路径（批次 D5）
    (HKF_CONTROL, 0x54),             // Ctrl+T 设置页·使用统计（批次 D8）
    (HKF_CONTROL, 0x57),             // Ctrl+W 设置页·网页引擎（批次 D2）
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
    // 批次 E6（FR-ACT-01 最后一格「`Alt+Enter` ⇒ 复制这一行」）。0x0D = `VK_RETURN`
    // （`application.rs` 的 `SECONDARY_DEFAULT` 用的就是它），而 `HotkeySpec::check` **收**回车
    // ⇒ 这一格跟 `Ctrl+Enter` 一样是录得出来的组合：不登记就能把它绑成全局键，
    // 代价与批次 R 裸 `` ` `` 同型——`RegisterHotKey` 吞掉之后窗口里那颗 `Alt+Enter` 就此失聪，
    // 而且 `Alt+Enter` 是系统里最常见的"属性对话框"与终端换行，抢走还顺手抹掉别人的。
    (HKF_ALT, 0x0D),                  // Alt+Enter 复制这一行（与 Ctrl+C 同一份载荷）
    // 批次 B8（FR-ACT-10）：裸 F1–F4。0x70..=0x73 就是 VK_F1..VK_F4，`take_fresh_key` 只认
    // 不带修饰的那一层，所以 `Ctrl+F1` / `Alt+F1` 仍然留给用户录；把这四格裸键登记进表，
    // 挡的是"录成全局呼出键 ⇒ 系统整条吞掉，窗口里那颗 F1 就此失聪"（同批次 R 裸 `` ` `` 那一层代价）。
    (0, 0x70), // F1 关于本程序
    (0, 0x71), // F2 打开 / 返回设置页（Ctrl+, 的别名）
    (0, 0x72), // F3 编辑选中项（Ctrl+E 的别名）
    (0, 0x73), // F4 打开配置文件所在目录
];

/// 批次 E6（FR-ACT-01「`Alt+Enter` 复制这一行」）：窗口内热键的比对值。
///
/// 与 [`LOCAL_TAKEN`] 里那一格必须是同一份数值——那边挡的是"用户把它录成全局键"，
/// 这边做的是"窗口里按它要能命中"。写成 const 而不现造：`HotkeySpec` 的两个字段要直接
/// 对上 `RegisterHotKey` 的口径（`HKF_ALT` / `VK_RETURN`），漂一位就变成另一颗键。
/// 它**不进** `RegisterHotKey`：AHK 那一颗也只在搜索窗内有效——`SearchWindow.ahk:1284-1286`
/// 的 `case 0x0D: _Execute(ctrl ? "ctrl" : alt ? "alt" : "")` 读的是自己窗口里的
/// `WM_KEYDOWN`/`WM_SYSKEYDOWN`，全局按 Alt+Enter 去抢别人的属性框不是需求。
const ALT_ENTER: HotkeySpec = HotkeySpec {
    mods: HKF_ALT,
    vk: VK_RETURN,
};

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
            // 批次 G1：副呼出键一起摘。理由同主热键——不摘的话录副呼出时按下它自己那颗键
            // （`Alt+R` 就在出厂值上）会被系统整条吞掉，框里永远收不到一次按键。
            if hotkey_lock(&EXTRA_ACTIVE).is_some() {
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_EXTRA);
                eprintln!("hotkey: 取键期间临时注销副呼出键（退出取键时装回）");
            }
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
        // 批次 G1：副呼出键跟着装回。放在主热键那格之前——主热键可能是空的（启动即失败过），
        // 那时候也得把副呼出装回去，不能被下面那条 `else { return }` 连累。
        if let Some(e) = *hotkey_lock(&EXTRA_ACTIVE) {
            match apply_hotkey(hwnd, HotkeySlot::Extra, e) {
                Ok(()) => {
                    eprintln!("hotkey: 退出取键（{why}）⇒ 已装回副呼出键 {}", e.display())
                }
                Err(err) => {
                    *hotkey_lock(&EXTRA_ACTIVE) = None;
                    hotkey_lock(&HOTKEY_RESULT).push(HotkeyOutcome::Rejected {
                        slot: HotkeySlot::Extra,
                        tried: e,
                        err,
                        now_active: None,
                    });
                    eprintln!(
                        "hotkey: 退出取键（{why}）时装回副呼出键 {} 失败（{err}）",
                        e.display()
                    );
                    if let Some(ctx) = CTX.get() {
                        ctx.request_repaint();
                    }
                }
            }
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

// ---- 批次 G1：双击 Ctrl / Shift 呼出（FR-WIN-03）----

fn tap_lock() -> std::sync::MutexGuard<'static, TapTracker> {
    // 与 `hotkey_lock` 同一条理由：这把锁只装几个 Copy 小值，临界区内不回调任何代码；
    // 钩子回调里更不能 panic（那条路径上没有谁来接住它）。
    TAP_TRACKER.lock().unwrap_or_else(|e| e.into_inner())
}

/// 钩子回调里用的毫秒数（进程内单调）。判定只比较两次调用的差，零点在哪无所谓。
fn tap_now_ms() -> u32 {
    TAP_EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u32
}

/// `WH_KEYBOARD_LL` 的回调。**跑在装钩子的那条线程**（系统把事件投给那条线程的消息队列），
/// 而系统对它有硬超时（回得太慢会被静默摘钩）⇒ 这里只做三件事：取 vk、喂状态机、
/// 判到"双击"时走既有的 `HOTKEY_TX` 漏斗。
///
/// **绝不在回调里写日志**：那等于把用户的每一次按键都过一遍磁盘。
/// **也不滤 `LLKHF_INJECTED`**：远控软件与自动化测试发的都是注入键，滤掉就等于把
/// FR-WIN-03「忽略远控软件的同步抖动」反过来实现（抖动由 25ms 最小间隔那条挡）。
unsafe extern "system" fn keyboard_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        // 负数 = 系统要求原样往下传，此时 `lparam` 不是 `KBDLLHOOKSTRUCT`。
        if code < 0 || lparam.0 == 0 {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let is_up = match wparam.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => false,
            WM_KEYUP | WM_SYSKEYUP => true,
            _ => return CallNextHookEx(None, code, wparam, lparam),
        };
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let decision = {
            let mut t = tap_lock();
            t.on_key(kb.vkCode, is_up, tap_now_ms())
        };
        // 取键期（设置页正在录主/副热键）不认双击：那会儿用户按 Ctrl 是在**录组合键**，
        // 连着按两下不该把窗口藏起来把录制打断。`HOTKEY_SUSPENDED` 也就跑在本线程，读它免费。
        if decision == TapDecision::Show && !HOTKEY_SUSPENDED.load(Ordering::Relaxed) {
            // 与主热键**同一条路**（AHK 版双击也是 `SearchWindow.Toggle()`）：
            // 一边是 `HOTKEY_TX` 的"切一次显隐"，一边要唤醒可能已经隐藏的 UI。
            if let Some(tx) = HOTKEY_TX.get() {
                let _ = tx.send(());
            }
            if let Some(ctx) = CTX.get() {
                ctx.request_repaint();
            }
        }
        // 一律放行：这是"旁听"钩子，吞掉任何键都等于把 Ctrl / Shift 从系统里抢走。
        CallNextHookEx(None, code, wparam, lparam)
    }
}

/// 装 / 卸低级键盘钩子，并把判定状态重置到 `mode`。**只能在热键线程调**。
///
/// 只在 `mode != Off` 时装：不装就一个按键事件都看不见，关闭档的代价为零。
/// 换档但钩子已在时只重置状态、不重装——半下没抬的那颗键因此不可能跨档配对。
fn set_tap_hook(mode: DblTap) {
    unsafe {
        let raw = TAP_HOOK.load(Ordering::Relaxed);
        if !mode.needs_hook() {
            if raw != 0 {
                match UnhookWindowsHookEx(HHOOK(raw as *mut _)) {
                    Ok(()) => eprintln!("dbltap: 已卸下键盘钩子（双击呼出改为「关闭」）"),
                    Err(e) => eprintln!("dbltap: 卸下键盘钩子失败：{e}"),
                }
                TAP_HOOK.store(0, Ordering::Relaxed);
            }
            tap_lock().set_mode(DblTap::Off);
            return;
        }
        tap_lock().set_mode(mode);
        if raw != 0 {
            eprintln!(
                "dbltap: 双击档改为「{}」（钩子已在，判定状态已重置）",
                mode.label()
            );
            return;
        }
        let module = match GetModuleHandleW(None) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("dbltap: GetModuleHandleW 失败：{e} ⇒ 双击呼出不可用");
                return;
            }
        };
        // `dwThreadId = 0` 的全局钩子必须给 hMod（否则 ERROR_HOOK_NEEDS_HMOD），
        // 给的就是本 exe 的模块句柄——低级钩子的回调在本进程里跑，所以模块要一直活着。
        match SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_hook_proc),
            Some(HINSTANCE(module.0)),
            0,
        ) {
            Ok(h) => {
                TAP_HOOK.store(h.0 as usize, Ordering::Relaxed);
                eprintln!(
                    "dbltap: 已装键盘钩子（{}；连按两下切一次显隐）",
                    mode.label()
                );
            }
            Err(e) => eprintln!("dbltap: 装键盘钩子失败（{e}）⇒ 双击呼出本次不可用"),
        }
    }
}

/// UI → 热键线程：换双击档。消息窗还没建好时丢弃（用户再点一次会重投）。
fn post_double_tap(mode: DblTap) {
    let raw = HOTKEY_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        eprintln!("dbltap: 消息窗还没建立，本次换档作废");
        return;
    }
    if let Err(e) = unsafe {
        PostMessageW(
            Some(HWND(raw as *mut _)),
            WM_APP_SET_DOUBLE_TAP,
            WPARAM(mode.code() as usize),
            LPARAM(0),
        )
    } {
        eprintln!("dbltap: 换档消息投递失败：{e}");
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
                // 批次 G1：「副呼出热键」与主呼出键是同一件事（AHK 版把两个键绑在同一个
                // `SearchWindow.Toggle()` 上），所以走同一个分支。
                HOTKEY_TOGGLE | HOTKEY_EXTRA => {
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
        // 批次 G1：清除副呼出键 / 换双击档。两件都只能在本线程做——
        // 前者是 `UnregisterHotKey` 的 hwnd 归属，后者是低级钩子的线程归属。
        if msg == WM_APP_CLEAR_EXTRA {
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_EXTRA);
            *hotkey_lock(&EXTRA_ACTIVE) = None;
            eprintln!("hotkey: 已注销副呼出热键（设置页「清除」）");
            return LRESULT(0);
        }
        if msg == WM_APP_SET_DOUBLE_TAP {
            match DblTap::from_code(wparam.0 as u32) {
                Some(mode) => set_tap_hook(mode),
                None => eprintln!("dbltap: 收到不认识的档位码 {}", wparam.0),
            }
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

fn spawn_hotkey_thread(
    tx: mpsc::Sender<()>,
    configured: HotkeySpec,
    typeback: Option<HotkeySpec>,
    extra: Option<HotkeySpec>,
    dbl_tap: DblTap,
) {
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
        // 批次 G1：副呼出热键（`Alt+R`）。与主热键同一件事，但没有"出厂值可退"这一说——
        // 退了就是又一颗呼出键；失败照实报一条回执（`now_active: None`）。
        // 与已装着的两颗撞组时**不试注册**：同线程同 vk+修饰只能注册一次，撞出来的是 1409，
        // 而 1409 在本程序里的含义是"被别的程序占用"——占用者其实是我们自己（手改
        // `settings.json` 才做得出这种配置）。跳过时只记一行，日志里看得见。
        if let Some(extra) = extra {
            let clash = |other: Option<HotkeySpec>| {
                other.is_some_and(|k| (k.mods_named(), k.vk) == (extra.mods_named(), extra.vk))
            };
            let toggle_now = *hotkey_lock(&HOTKEY_ACTIVE);
            let typeback_now = *hotkey_lock(&TYPEBACK_ACTIVE);
            if clash(toggle_now) || clash(typeback_now) {
                eprintln!(
                    "hotkey: 副呼出热键 {} 与已装着的那颗同组（多半是手改过 settings.json）⇒ 本次跳过，不改配置",
                    extra.display()
                );
            } else {
                match apply_hotkey(hwnd, HotkeySlot::Extra, extra) {
                    Ok(()) => {
                        *hotkey_lock(&EXTRA_ACTIVE) = Some(extra);
                        eprintln!(
                            "hotkey: 副呼出热键 {} registered（与主呼出键同效）",
                            extra.display()
                        );
                    }
                    Err(err) => {
                        hotkey_lock(&HOTKEY_RESULT).push(HotkeyOutcome::Rejected {
                            slot: HotkeySlot::Extra,
                            tried: extra,
                            err,
                            now_active: None,
                        });
                        eprintln!(
                            "hotkey: 副呼出热键 {} 注册失败（{err}={}）⇒ 本次不可用，主呼出键不受影响",
                            extra.display(),
                            if err == ERROR_HOTKEY_ALREADY_REGISTERED {
                                "已被占用"
                            } else {
                                "其它原因"
                            }
                        );
                    }
                }
            }
        }
        // 批次 G1：双击呼出。钩子必须在**本线程**装（回调跑在装钩子的线程），
        // 而本线程此刻正好要进 `GetMessageW` 循环 ⇒ 时机就是这里。关闭档不装。
        set_tap_hook(dbl_tap);
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
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_EXTRA);
        // 钩子不摘的话会一直挂在一条已经退出的线程上（系统只在进程结束时收拾）。
        set_tap_hook(DblTap::Off);
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
        // 批次 E2：启动路径上的设置只由 `main` 读这一次，结果原样递进来（`reload_config` 那条
        // 用户手动重载除外）。以前这里读第二遍，而第一遍已经把
        // 坏文件改名隔离掉了 ⇒ 第二遍看到的是"文件不存在、没有故障"，那句界面提示永远轮不到出场
        // （2026-10-25 真机：只有 settings.json 坏时屏幕上什么都不提示）。顺带一条：类型不符的
        // 降级日志从前一次启动打两遍，也是这第二次读造成的。
        settings: source::application::AppSettings,
        broken_settings: Option<Corrupted>,
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
        // 批次 C3（FR-QRY-10）：搜索历史。**文件不在＝空表，并且不写盘**——用户还没执行过任何东西，
        // 这时候在盘上落一个 `[]` 是程序自己造的文件，不是用户要的（与清单那条"只有真没有才播种"同一口径）。
        let (history, broken_history) = source::application::load_history();
        // 批次 D8（FR-SYS-06）：使用统计。文件不在＝空表且**不写盘**——用户还没执行过任何东西，
        // 这时候落一个空 usage.json 是程序自己造的文件（与历史那条同一口径）。
        let (usage, broken_usage) = source::application::load_usage();
        // 批次 B6（FR-ACT-07）：参数历史是第五份文件，门禁同样各管各的（只挡它自己的写）。
        let (param_history, broken_param) = source::application::load_param_history();
        // §4.2.1 出厂命令集的补齐：老清单（settings 里的轮次落后）启动时补一次缺，补完记账。
        // 只加不删、已有条目一律不动；记账之后删掉的条目不会自己回来（找回用设置页那个按钮）。
        let mut prefs = settings;
        // 两种下场分开说：`Kept` 报得出留证落点，`Lost` 只能请人工处理，并且要立刻把"本轮对它
        // 只读不写"通知到下面那条补齐路径（2026-10-22 真机⑫：留证没成功时照样播种写盘，
        // 把唯一那份坏文件覆盖了——那一次日志里就写着「覆盖写会丢掉唯一证据」）。
        let lost_cfg = lost_evidence_labels(&[
            ("settings.json", &broken_settings),
            ("user_items.json", &broken_list),
        ]);
        let writes_blocked = !lost_cfg.is_empty();
        // 历史那份是第三个文件，门禁也各管各的：它没留住证只挡它自己的写，
        // 不连带挡设置与清单（那是另外两份文件、另外两回事）；反过来也一样。
        let history_blocked = matches!(broken_history, Some(Corrupted::Lost));
        // 批次 D8：统计是第四份文件，门禁同样各管各的（它没留住证只挡它自己的写）。
        let usage_blocked = matches!(broken_usage, Some(Corrupted::Lost));
        // 批次 B6：参数历史同一条口径。
        let param_history_blocked = matches!(broken_param, Some(Corrupted::Lost));
        // 给用户看的那一句把四份并进来：坏在哪个文件上他就得知道哪个，别说漏了让他以为没事。
        let lost = if history_blocked || usage_blocked || param_history_blocked {
            let mut v: Vec<&str> = lost_cfg;
            if history_blocked {
                v.push("search_history.json");
            }
            if usage_blocked {
                v.push("usage.json");
            }
            if param_history_blocked {
                v.push("param_history.json");
            }
            v
        } else {
            lost_cfg
        };
        for (label, c, fallback) in [
            ("settings.json", &broken_settings, "出厂默认"),
            ("user_items.json", &broken_list, "空清单"),
            ("search_history.json", &broken_history, "空历史"),
            ("usage.json", &broken_usage, "空统计"),
            ("param_history.json", &broken_param, "空参数历史"),
        ] {
            match c {
                Some(Corrupted::Kept(p)) => eprintln!(
                    "配置损坏：{label} 解析失败，已隔离为 {}，本次按{fallback}启动（原文件未被覆盖）",
                    p.display()
                ),
                Some(Corrupted::Lost) => eprintln!(
                    "配置损坏：{label} 解析失败且留证没成功 ⇒ 本次启动不写它（写一次就覆盖掉唯一证据；之后手动保存仍会覆盖），请人工备份后删掉这个文件"
                ),
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
            // 两句分开写：只有清单坏的时候说"从出厂默认启动"等于告诉用户他的设置被重置了，
            // 而实际是设置一切照常、只有清单按空的处理（2026-10-25 真机截屏抓到）。
            (Some(a), None) => Some(format!(
                "⚠ 配置文件坏了，已改名留底：{a}，本次按空清单启动（你的设置照常，原文件没有被覆盖）"
            )),
            (None, Some(a)) => Some(format!(
                "⚠ 配置文件坏了，已改名留底：{a}，本次按出厂默认启动（你的清单照常，原文件没有被覆盖）"
            )),
            (None, None) => None,
        };
        // 批次 C3：历史那份坏了的说法单独追加，不改上面三条臂。
        // 理由不是省事——那三条是 E2 真机逐条对过屏幕文案的（`⚠ 两个配置文件坏了…` 那句
        // 就是当时截屏回读定下来的），为第三个文件重写它们等于作废一次实测记录。
        let notice = match (notice, kept(&broken_history)) {
            (Some(s), Some(h)) => Some(format!("{s}；搜索历史也坏了，已改名留底：{h}")),
            (None, Some(h)) => Some(format!(
                "⚠ 搜索历史坏了，已改名留底：{h}，本次按空历史启动（原文件没有被覆盖）"
            )),
            (n, None) => n,
        };
        // 批次 D8：统计那份坏了的说法单独追加，同样不改上面那几条已对过屏幕的臂。
        let notice = match (notice, kept(&broken_usage)) {
            (Some(s), Some(u)) => Some(format!("{s}；使用统计也坏了，已改名留底：{u}")),
            (None, Some(u)) => Some(format!(
                "⚠ 使用统计坏了，已改名留底：{u}，本次按空统计启动（原文件没有被覆盖）"
            )),
            (n, None) => n,
        };
        // 批次 B6：参数历史同一条写法（同样不改上面几条已对过屏幕的臂）。
        let notice = match (notice, kept(&broken_param)) {
            (Some(s), Some(p)) => Some(format!("{s}；参数历史也坏了，已改名留底：{p}")),
            (None, Some(p)) => Some(format!(
                "⚠ 参数历史坏了，已改名留底：{p}，本次按空参数历史启动（原文件没有被覆盖）"
            )),
            (n, None) => n,
        };
        // 留证没成功那句话不许被"已改名留底"盖掉：两种下场说成一件事就是骗用户。
        // 措辞按事实收窄（2026-10-26 批次 E3）：门禁只管启动期那六处补齐/去重，
        // 用户之后手动增删改条目照样会写回，所以只能说"本次启动不会写它"。
        let lost_tail = match lost.as_slice() {
            [] => None,
            [one] => Some(format!(
                "{one} 坏了且留证没成功（原名与副本都写不出来）⇒ 本次启动不会写它，你之后手动保存仍会覆盖；请先手动备份再删掉"
            )),
            many => Some(format!(
                "{} 都坏了且留证都没成功（原名与副本都写不出来）⇒ 本次启动不会写它们，你之后手动保存仍会覆盖；请先手动备份再删掉",
                many.join(" 与 ")
            )),
        };
        let notice = match (notice, lost_tail) {
            (None, None) => None,
            (Some(s), None) => Some(s),
            (None, Some(t)) => Some(format!("⚠ {t}")),
            (Some(s), Some(t)) => Some(format!("{s}；{t}")),
        };
        if prefs.seed_round < source::application::SEED_ROUND {
            let added = source::application::merge_missing_seeds(&mut items);
            if added > 0 {
                if writes_blocked {
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
            if writes_blocked {
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
                if writes_blocked {
                    eprintln!(
                        "source: 出厂分类回填 {tagged} 条（坏文件留证没成功 ⇒ 这次不写清单）"
                    );
                } else {
                    source::application::save_user_items(&items);
                    eprintln!("source: 出厂分类回填 {tagged} 条（已写清单）");
                }
            }
            // 批次 F4：同一轮把内置动作的图标规格回填到老条目（uid 对得上且 `source_path` 为空才补）。
            let iconized = source::application::apply_factory_icons(&mut items);
            if iconized > 0 {
                if writes_blocked {
                    eprintln!(
                        "source: 出厂图标回填 {iconized} 条（坏文件留证没成功 ⇒ 这次不写清单）"
                    );
                } else {
                    source::application::save_user_items(&items);
                    eprintln!("source: 出厂图标回填 {iconized} 条（已写清单）");
                }
            }
        }
        // 2026-10-23 一次性改名：老清单里的「设置·X」→「Win·X」/「启动器·X」，
        // 总入口「设置」→「Windows 设置」（用户反馈敲「设置」出来四十行）。
        // 与 seed_round 无关——补种只加不删，改名得单独走一趟；幂等，第二轮起恒为 0。
        let renamed = source::system::rename_legacy_settings(&mut items);
        if renamed > 0 {
            if writes_blocked {
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
            if writes_blocked {
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
            if writes_blocked {
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
        let extra_active = prefs.extra_hotkey;
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
            icon_pending: std::collections::HashSet::new(),
            icon_failed: std::collections::HashSet::new(),
            icon_tick: std::cell::Cell::new(0),
            path_tx,
            path_rx,
            path_states: std::collections::HashMap::new(),
            path_in_flight: false,
            path_scanned_at: Instant::now(),
            sources: source::default_sources_with(&prefs),
            startup_notice: notice,
            selected: 0,
            scroll_followed: 0,
            focus_query: intent.visible && intent.dialog.is_none() && !settings_open,
            dialog: intent.dialog,
            dialog_enter_armed: false,
            confirm: None,
            internal_confirm: None,
            param_prompt: None,
            param_enter_armed: false,
            summon: initial_summon_snapshot(),
            param_history,
            param_history_blocked,
            about_open: false,
            help_entry: None,
            pending_destructive: None,
            manage_checked: std::collections::BTreeSet::new(),
            manage_confirm: None,
            menu_open: false,
            win_drag: None,
            settings_open,
            settings_tab,
            file_folders_edit: None,
            file_exclude_edit: None,
            web_engines_edit: None,
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
            large_type: large_type::LargeType::new(self_hwnd),
            // 批次 P1：面板不是启动态能带进来的东西（呼出永远从结果模式开始）。
            panel: None,
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
            // 批次 G1：副呼出键同一套口径（先按配置显示，首帧按 EXTRA_ACTIVE 校正）。
            extra_active,
            extra_awaiting: None,
            extra_recording: false,
            secondary_recording: false,
            marked: Vec::new(),
            batch_confirm: None,
            history,
            history_browse: source::application::HistoryBrowse::default(),
            history_blocked,
            usage,
            usage_blocked,
        };
        // 立即为已有项起图标加载器（`source_path` 为空 ⇒ `cache_key` 给 None，自然跳过）。
        let jobs = icon_jobs(&seed.items);
        seed.spawn_missing_icons(jobs);
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
        // 批次 F3（FR-WIN-09）：老 `last_pos` 先折算成千分比，再按 `show_on` 做首次落点。
        // 顺序不能反——先摆位后折算，摆出来的默认位置会被一并记成"用户记住的位置"。
        seed.migrate_legacy_position(writes_blocked);
        if let Some(hwnd) = seed.self_hwnd {
            seed.place_on_summon(hwnd);
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
        // 批次 E5：这一行必须在**动窗口之前**。此刻系统前台还是用户待的那个窗口，
        // `ShowWindow` / `Focus` 之后就变成我们自己的了——FR-ACT-01 第三格「文本 ⇒ 粘贴」
        // 要的"贴回哪儿"只有这一帧问得出来。每次呼出都问一遍，不只隐藏→显示的边沿：
        // 弹窗／设置页／大字窗开着时失焦不关窗（FR-WIN-04），那种状态下前台已经不是我们。
        remember_previous_window(self.self_hwnd);
        // 批次 B6（FR-ACT-07）：同一时刻抓参数快照（`{%c}` / `{%wd}` / `{%wt}` / `{%wc}`）。
        // 与上一行同样必须在动窗口之前——`{%wt}` 验收的就是"呼出之前那个窗口的标题"。
        self.capture_summon_snapshot();
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
                // 批次 F3（FR-WIN-09）：每次呼出都重摆一次落点（AHK `SearchWindow.ahk:589`
                // 同样在 `_Show` 里每次调 `Place`）。放在 `SW_RESTORE` 之后：最小化态下
                // `SetWindowPos` 改的是「还原后的坐标」，先摆会被 `ShowWindow` 覆盖回去。
                self.place_on_summon(hwnd);
            }
            self.on_show();
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// 批次 F3（FR-WIN-09）：按 `show_on` 选屏、按工作区千分比摆窗口。
    ///
    /// 「记住位置」关掉时永远回默认 `[500, 200]`（水平居中、离顶 20%）——AHK `Place` 那一路
    /// 也是把空 position 当默认值，拖动只在这一次显示期间有效；打开时才读 `pos_rel`。
    fn place_on_summon(&self, hwnd: HWND) {
        let appearance = &self.prefs.appearance;
        let rel = if appearance.remember_position {
            appearance.pos_rel.unwrap_or(source::application::POS_REL_DEFAULT)
        } else {
            source::application::POS_REL_DEFAULT
        };
        let (Some(size), Some(work)) = (win_size(hwnd), work_for(appearance.show_on)) else {
            eprintln!("窗口：落点算不出来（读不到窗口尺寸或工作区）⇒ 保持原位");
            return;
        };
        let (x, y) = place_in_work(work, size, rel);
        let moved = win_origin(hwnd).is_none_or(|(ox, oy)| (ox, oy) != (x, y));
        if moved {
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    x,
                    y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        // 与 F8 那条同口径：只记坐标与档位，不记任何用户内容。
        eprintln!(
            "窗口：落点 {}（{}/{}{}）→ ({x}, {y})",
            appearance.show_on.as_str(),
            rel[0],
            rel[1],
            if appearance.remember_position && appearance.pos_rel.is_some() {
                "，记住的位置"
            } else {
                "，默认落点"
            }
        );
    }

    /// 批次 F3（FR-WIN-09）：批次 F8 的 `last_pos` 是绝对坐标，按新口径折算成工作区千分比。
    ///
    /// 只做一次：折算完清空 `last_pos` 并落盘，之后 `settings.json` 里就只剩 `pos_rel`。
    /// `settings.json` 坏到留证没成功时不折算也不写盘（那一轮对它是只读的），下次启动再来。
    fn migrate_legacy_position(&mut self, writes_blocked: bool) {
        let Some(old) = self.prefs.appearance.last_pos else {
            return;
        };
        if writes_blocked {
            eprintln!("窗口：settings.json 本轮只读 ⇒ 旧位置字段留到下次启动折算");
            return;
        }
        let converted = self.self_hwnd.and_then(|hwnd| {
            if !self.prefs.appearance.remember_position || !pos_on_screen(old) {
                return None;
            }
            let size = win_size(hwnd)?;
            let work = hud::monitor_work_at((old[0], old[1]))?;
            Some(relative_in_work(work, size, (old[0], old[1])))
        });
        if let Some(rel) = converted {
            self.prefs.appearance.pos_rel = Some(rel);
        }
        self.prefs.appearance.last_pos = None;
        source::application::save_settings(&self.prefs);
        match converted {
            Some(rel) => eprintln!(
                "窗口：旧位置字段（绝对坐标）已折算为工作区 {}/1000、{}/1000",
                rel[0], rel[1]
            ),
            None => eprintln!("窗口：旧位置字段作废（不再记住或已不在任何显示器上）"),
        }
    }

    /// 批次 B6（FR-ACT-07）：每次呼出抓一份现场。三条规矩与 [`remember_previous_window`] 对齐：
    /// · 剪贴板每次重读——`{%c}` 的语义就是"按下热键那一刻的剪贴板"；
    /// · 前台为空 ⇒ 窗口三格清空（宁可不填，不拿上一次的冒充）；
    /// · 前台是自己 ⇒ 保留上一次那一个（重复呼出 / 托盘点自己 / 大字窗还回来）。
    /// 标题与剪贴板原文都不进日志（FR-SYS-03），只记长度与类名。
    fn capture_summon_snapshot(&mut self) {
        self.summon.clipboard = read_clipboard_text().unwrap_or_default();
        let fg = unsafe { GetForegroundWindow() };
        if fg.0.is_null() {
            self.summon.window_id.clear();
            self.summon.window_title.clear();
            self.summon.window_class.clear();
            return;
        }
        if self.self_hwnd.is_some_and(|me| me.0 == fg.0) {
            return;
        }
        self.summon.window_id = (fg.0 as usize).to_string();
        self.summon.window_title = window_title(fg);
        self.summon.window_class = window_class_name(fg);
        eprintln!(
            "参数标记：呼出快照 = 前台 0x{:x}（类名 {}；标题不落日志）、剪贴板 {} 字",
            fg.0 as usize,
            self.summon.window_class,
            self.summon.clipboard.chars().count()
        );
    }

    /// 批次 B6（FR-ACT-07）：这一条要不要先问参数。`true` = 已弹框，调用方别再启动。
    ///
    /// 判据全在 [`source::params::needs_prompt`] 里（写着 `{%p}`，或者一个标记都没写但
    /// `ParamType≠none` 要追加参数），这里只负责扣下现场。
    fn param_prompt_if_needed(
        &mut self,
        entry: &LaunchItem,
        uid: &str,
        from_list: bool,
        alias_word: String,
        from_custom: bool,
    ) -> bool {
        if !source::params::needs_prompt(&entry.path, &entry.arguments, entry.param_type) {
            return false;
        }
        eprintln!(
            "参数标记：条目「{}」要先填参数 ⇒ 弹参数框（{uid}）",
            entry.name
        );
        self.param_prompt = Some(ParamPrompt {
            entry: entry.clone(),
            uid: uid.to_string(),
            text: String::new(),
            from_list,
            alias_word,
            from_custom,
        });
        self.param_enter_armed = true;
        true
    }

    /// 批次 B6：把条目里的标记换成真实值。返回（展开后的副本, 是否做过替换）。
    ///
    /// **做过替换 ⇒ 日志必须走脱敏路径**：填进去的是剪贴板原文 / 窗口标题 / 用户参数。
    /// 工作目录不展开（Delphi 也只处理命令行两格）：里面的标记原样交给系统，看得见比静默吞掉好。
    ///
    /// 2026-10-28 真机④：这里原先在「一个标记都没写」时直接短路返回，`expand_command` 里
    /// 那条「没标记就按编码接在命令行后面」的分支因此永远走不到——参数框提示写着「这段会按
    /// 编码接在命令行后面」，实际只启动了 `--base`。判据改成比一比展开前后变没变，接不接
    /// 由 `expand_command` 自己定。
    fn expand_for_launch(&self, entry: &LaunchItem, param: &str) -> (LaunchItem, bool) {
        let ty = entry.param_type;
        if ty == source::params::ParamType::None {
            return (entry.clone(), false);
        }
        let (path, arguments) =
            source::params::expand_command(&entry.path, &entry.arguments, ty, &self.summon, param);
        let changed = path != entry.path || arguments != entry.arguments;
        let mut out = entry.clone();
        out.path = path;
        out.arguments = arguments;
        (out, changed)
    }

    /// 批次 B6（FR-ACT-07）：参数框按「确定」那一帧——展开、启动、记账，三件事与列表直接
    /// 启动那一按同一口径（成功才累计统计；`launch_and_hide_masked` 保证日志不落真实参数）。
    fn confirm_param_prompt(&mut self, ctx: &egui::Context) {
        let Some(p) = self.param_prompt.take() else {
            return;
        };
        let param = p.text.trim().to_string();
        let (entry, expanded) = self.expand_for_launch(&p.entry, &param);
        if expanded {
            eprintln!(
                "参数标记：「{}」展开完成（参数 {} 字，原文不落日志）",
                p.entry.name,
                param.chars().count()
            );
        }
        // 历史只记非空的参数（空值没有复用价值）；写盘失败不影响启动。
        if !param.is_empty() && self.param_history.touch(&param) {
            if self.param_history_blocked {
                eprintln!("参数历史：文件坏且留证没成功 ⇒ 这次不写盘（只在内存里记一条）");
            } else {
                source::application::save_param_history(&self.param_history);
            }
        }
        self.remember_history(self.query.clone());
        let started = self.launch_and_hide_masked(ctx, &entry, &p.uid);
        if started && p.from_list {
            self.mark_used(&p.uid, &p.alias_word);
        }
        if started && p.from_custom {
            self.record_usage(source::usage::Bucket::Custom);
        }
    }

    /// FR-WIN-08：打开设置页并切到指定页码（1=通用 2=功能 3=热键 4=数据源 5=文件搜索 6=外观
    /// 7=高级 8=批量管理 9=使用统计 10=网页引擎）。
    fn open_settings_page(&mut self, page: u32) {
        self.settings_open = true;
        self.settings_tab = match SettingsTab::from_page(page) {
            Some(t) => t,
            None => {
                eprintln!(
                    "FR-WIN-08: -Preferences {page} 无对应页（1=通用 2=功能 3=热键 4=数据源 5=文件搜索 6=外观 7=高级 8=批量管理 9=使用统计 10=网页引擎），落回通用"
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
        let jobs = icon_jobs(&incoming);
        self.items = dedup_items([std::mem::take(&mut self.items), incoming].concat());
        self.carry_stats(&old);
        let added = self.items.len().saturating_sub(before);
        let skipped = paths.len().saturating_sub(added);
        self.spawn_missing_icons(jobs);
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
                    HotkeySlot::Extra => {
                        self.extra_active = Some(on);
                        self.extra_awaiting = None;
                        if self.prefs.extra_hotkey != Some(on) {
                            self.prefs.extra_hotkey = Some(on);
                            source::application::save_settings(&self.prefs);
                        }
                        self.hotkey_note(
                            &format!("✓ 副呼出热键已设为 {}", on.display()),
                            true,
                        );
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
                        HotkeySlot::Extra => {
                            self.extra_awaiting = None;
                            // 与主热键那格不同，这里 `now_active` 就是真值：副呼出没有"出厂值可退"，
                            // 线程侧换键失败时已经把原来那颗装回去了（装不回去才是 None）。
                            self.extra_active = now_active;
                            let tail = match now_active {
                                Some(k) => format!("，当前仍是 {}", k.display()),
                                None => " ⇒ 现在只剩主呼出键".to_string(),
                            };
                            self.hotkey_note(
                                &format!("⚠ 副呼出热键 {} {why}{tail}", tried.display()),
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
            // 批次 G1：副呼出同理，`None` 也是有效信息（清掉了 / 撞组被跳过 / 注册失败）。
            self.extra_active = *hotkey_lock(&EXTRA_ACTIVE);
        }

        // ③ 取键，④ 边沿通知。
        self.take_keypick(ui);
        self.sync_keypick_suspend();
    }

    /// 取键态下吃掉本帧按键并分派给两个入口之一。互斥是结构自带的：`take_record_press` 会把
    /// 整帧的 Key 事件清空，一帧只可能有一个框拿到键，所以先判谁在等就够了。
    fn take_keypick(&mut self, ui: &mut egui::Ui) {
        if !self.hotkey_recording
            && !self.typeback_armed
            && !self.secondary_recording
            && !self.extra_recording
        {
            return;
        }
        // 换键请求还悬着（回执未到）时不接受第二次取键，否则会覆盖 REQUEST 槽里的组合。
        if self.hotkey_awaiting.is_some()
            || self.typeback_awaiting.is_some()
            || self.extra_awaiting.is_some()
        {
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
        if self.secondary_recording {
            match key {
                egui::Key::Escape | egui::Key::Tab => {
                    self.secondary_recording = false;
                    eprintln!(
                        "设置页：次级动作键录制取消（按 {}），当前仍是 {}",
                        if key == egui::Key::Escape { "Esc" } else { "Tab" },
                        self.prefs.secondary_hotkey.display()
                    );
                }
                egui::Key::Backspace | egui::Key::Delete => {
                    self.secondary_recording = false;
                    self.prefs.secondary_hotkey = HotkeySpec::SECONDARY_DEFAULT;
                    source::application::save_settings(&self.prefs);
                    let show = HotkeySpec::SECONDARY_DEFAULT.display();
                    eprintln!("设置页：次级动作键收到 Backspace/Delete ⇒ 恢复默认 {show}（已写 settings.json）");
                    self.hotkey_note(&format!("✓ 次级动作键恢复默认：{show}"), true);
                }
                other => {
                    let Some(spec) = spec_from_press(other, m) else {
                        return; // 方向键之类：继续等，不提示（与主热键同一口径）
                    };
                    let show = spec.display();
                    // 这颗键不进系统，所以不需要"试注册"那一级；判据全在本地，落定即生效。
                    // 被拒时**留在录制态**：提示语写的是"换一个"，退出录制就会自相矛盾
                    //（主热键那条 2026-10-20 真机踩过，见上面的长注释）。
                    if let Err(msg) = spec.check_secondary(
                        &self.hotkey_active,
                        self.typeback_active.as_ref(),
                        LOCAL_TAKEN,
                    ) {
                        self.hotkey_note(&format!("⚠ {show}：{msg}"), false);
                        eprintln!(
                            "设置页：次级动作键 {show} 未采纳（{msg}），当前仍是 {}",
                            self.prefs.secondary_hotkey.display()
                        );
                        return;
                    }
                    self.secondary_recording = false;
                    self.prefs.secondary_hotkey = spec;
                    source::application::save_settings(&self.prefs);
                    eprintln!("设置页：次级动作键 → {show}（已写 settings.json）");
                    self.hotkey_note(&format!("✓ 次级动作键：{show}"), true);
                }
            }
            return;
        }
        if self.extra_recording {
            match key {
                egui::Key::Escape | egui::Key::Tab => {
                    self.extra_recording = false;
                    eprintln!(
                        "设置页：副呼出键录制取消（按 {}），当前仍是 {}",
                        if key == egui::Key::Escape { "Esc" } else { "Tab" },
                        match self.extra_active {
                            Some(k) => k.display().to_string(),
                            None => "无".to_string(),
                        }
                    );
                }
                egui::Key::Backspace | egui::Key::Delete => {
                    self.extra_recording = false;
                    eprintln!("设置页：副呼出键收到 Backspace/Delete ⇒ 恢复出厂组合");
                    self.request_extra(HotkeySpec::EXTRA_DEFAULT);
                }
                other => {
                    let Some(spec) = spec_from_press(other, m) else {
                        return; // 方向键之类：继续等，不提示（另两格同一口径）
                    };
                    self.extra_recording = !self.request_extra(spec);
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
        // 批次 G1：副呼出的录制也进暂停态（`suspend_hotkey` 会把主/副两颗都摘掉）。
        // `secondary_recording` 不在内：那颗键不注册给系统，摘了也没有意义。
        let picking = self.hotkey_recording || self.typeback_armed || self.extra_recording;
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
        // 批次 G1：第三格同理。反向几条（另两格撞副呼出）在各自的 request_* 里。
        if let Some(ex) = self.extra_active {
            if (ex.mods_named(), ex.vk) == (spec.mods_named(), spec.vk) {
                self.hotkey_note(
                    &format!("⚠ {show} 正被副呼出热键占着，请先改掉或清除它"),
                    false,
                );
                eprintln!("设置页：热键 {show} 与副呼出热键同组，未采纳");
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
        // 批次 G1：`check_typeback` 管不到第三格，这里补上（同线程同组只能注册一次）。
        if let Some(ex) = self.extra_active {
            if (ex.mods_named(), ex.vk) == (spec.mods_named(), spec.vk) {
                self.hotkey_note(
                    &format!("⚠ 补字符热键 {show} 正被副呼出热键占着，请先改掉或清除它"),
                    false,
                );
                eprintln!("设置页：补字符热键 {show} 与副呼出热键同组，未采纳");
                return false;
            }
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

    /// 批次 G1：副呼出热键的"应用"。判据在 `check_extra` 里（带修饰键 + 不许与另两格同组），
    /// 其余与主热键同一口径：键表预检、同值不动、**只有注册成功才写盘**。
    fn request_extra(&mut self, spec: HotkeySpec) -> bool {
        let show = spec.display();
        if let Err(msg) = spec.check_extra(&self.hotkey_active, self.typeback_active.as_ref()) {
            self.hotkey_note(&format!("⚠ 副呼出热键 {show}：{msg}"), false);
            eprintln!("设置页：副呼出热键 {show} 未采纳（{msg}）");
            return false;
        }
        if spec.conflicts_local(LOCAL_TAKEN) {
            self.hotkey_note(&format!("⚠ {show} 是本程序自己的快捷键，换一个"), false);
            eprintln!("设置页：副呼出热键 {show} 与本程序键表冲突，未采纳");
            return false;
        }
        if self.extra_active == Some(spec) {
            self.hotkey_note(&format!("ℹ {show} 就是当前副呼出热键，未改动"), true);
            eprintln!("设置页：副呼出热键 {show} 与当前一致，未改动");
            return false;
        }
        if !post_hotkey_request(HotkeySlot::Extra, spec) {
            self.hotkey_note("⚠ 热键线程还没就绪，请再录一次", false);
            return false;
        }
        self.extra_awaiting = Some(spec);
        eprintln!("设置页：副呼出热键请求已投递 → {show}（等注册结果）");
        true
    }

    /// 批次 G1：「清除」副呼出键 = 只剩主呼出键（与 `clear_typeback` 同形态、不发回执）。
    fn clear_extra(&mut self) {
        post_hotkey_signal(WM_APP_CLEAR_EXTRA, "清除副呼出热键");
        *hotkey_lock(&EXTRA_ACTIVE) = None;
        self.extra_active = None;
        self.extra_awaiting = None;
        self.extra_recording = false;
        if self.prefs.extra_hotkey.take().is_some() {
            source::application::save_settings(&self.prefs);
        }
        eprintln!("设置页：副呼出热键已清除（注销 + settings.json 置空）");
        self.hotkey_note("✓ 已清除副呼出热键", true);
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

    /// 批次 D7 修正（2026-10-28 真机「打不进字」）：把「文件搜索」页**目录清单**的编辑缓冲
    /// 回写 prefs。有改动才落盘、才报日志 —— 「点别处但没改过」不该白写一次盘、白扫一轮索引。
    fn commit_file_folders_edit(&mut self) -> bool {
        let Some(buf) = self.file_folders_edit.as_ref() else {
            return false;
        };
        if *buf == self.prefs.file_folders {
            return false;
        }
        self.prefs.file_folders = buf.clone();
        source::application::save_settings(&self.prefs);
        eprintln!(
            "设置页：文件扫描目录已改（现 {} 个条目，路径不落日志）",
            self.prefs.file_folders.len()
        );
        true
    }

    /// 「文件搜索」页**排除表**的同一条回写路径（与目录清单分开提交：两者各自失焦）。
    fn commit_file_exclude_edit(&mut self) -> bool {
        let Some(buf) = self.file_exclude_edit.as_ref() else {
            return false;
        };
        if *buf == self.prefs.file_exclude {
            return false;
        }
        self.prefs.file_exclude = buf.clone();
        source::application::save_settings(&self.prefs);
        eprintln!(
            "设置页：文件排除表已改（现 {} 个条目）",
            self.prefs.file_exclude.len()
        );
        true
    }

    /// 离开「文件搜索」页（换分类 / 关设置页）时收尾：先把"没失焦就走了"的编辑回写掉，
    /// 再丢缓冲 —— 下次进页按 prefs 重新取（`-Preferences 5` 从别处改过的值也能看到）。
    fn finish_file_scope_edits(&mut self) {
        if self.file_folders_edit.is_none() && self.file_exclude_edit.is_none() {
            return;
        }
        self.commit_file_folders_edit();
        self.commit_file_exclude_edit();
        self.file_folders_edit = None;
        self.file_exclude_edit = None;
    }

    /// 关设置页 / 切页时的统一收尾：把**所有**页的编辑缓冲回写掉。
    /// 只在 `settings_open = false` 的那几条路上调用——`settings_ui` 里的"换分类"那条分支
    /// 管不到"整页关掉"，漏了它缓冲会留着旧值：下次进页时 `is_none()` 判假，页面拿到的
    /// 是不再刷新的旧表（2026-10-29 批次 D2 接线时发现的既有漏洞，文件搜索页同样中招）。
    fn finish_settings_edits(&mut self) {
        self.finish_file_scope_edits();
        self.finish_web_engine_edits();
    }

    /// 批次 D2：把「网页引擎」页的编辑缓冲**原样**写回 prefs（不丢行、不去重）。
    /// 整理留给 [`Self::finish_web_engine_edits`]：编辑途中"关键字已填、网址还没填"是正常中间态，
    /// 这时候丢行等于把用户刚敲的字吃掉。
    fn save_web_engines_edit(&mut self) -> bool {
        let Some(buf) = self.web_engines_edit.as_ref() else {
            return false;
        };
        if *buf == self.prefs.web_engines {
            return false;
        }
        self.prefs.web_engines = buf.clone();
        source::application::save_settings(&self.prefs);
        eprintln!(
            "设置页：网页引擎表已改（现 {} 条；关键字与网址不落日志）",
            self.prefs.web_engines.len()
        );
        true
    }

    /// 离开「网页引擎」页（换分类 / 关设置页）时收尾：整理 → 写盘 → 丢缓冲。
    /// 丢掉的条数走 Toast 说清（残缺 / 关键字重复），不静默吞行。
    fn finish_web_engine_edits(&mut self) {
        let Some(mut buf) = self.web_engines_edit.take() else {
            return;
        };
        let cleaned = source::websearch::sanitize_engines(&mut buf);
        if cleaned.incomplete > 0 || cleaned.duplicate > 0 {
            let mut parts: Vec<String> = Vec::new();
            if cleaned.incomplete > 0 {
                parts.push(format!("{} 条没填关键字或网址", cleaned.incomplete));
            }
            if cleaned.duplicate > 0 {
                parts.push(format!("{} 条关键字重复", cleaned.duplicate));
            }
            self.hotkey_note(
                &format!("网页引擎：{}，这些没保存", parts.join("、")),
                false,
            );
            eprintln!(
                "设置页：网页引擎表整理丢弃 {} 条（残缺）+ {} 条（重复）",
                cleaned.incomplete, cleaned.duplicate
            );
        }
        let mut changed = false;
        // 兜底序列不能指着已经被删掉的引擎（自定义 id 被删、或那条本身就被整理丢了）。
        if source::websearch::prune_fallback(&mut self.prefs.web_fallback, &buf) {
            eprintln!("设置页：兜底序列已按现表整理（不在表里的 id 去掉）");
            changed = true;
        }
        if buf != self.prefs.web_engines {
            eprintln!(
                "设置页：网页引擎表已写入 settings.json（{} 条）",
                buf.len()
            );
            self.prefs.web_engines = buf;
            changed = true;
        }
        // 「进来看看又出去」不该白写一次盘（与文件搜索页同一条口径）。
        if changed {
            source::application::save_settings(&self.prefs);
        }
    }

    /// 批次 D5（FR-ACT-09）：设置页第 8 页「批量管理」。
    ///
    /// 三件事：勾选批量删（只删清单条目，不动磁盘文件）、失效项校验（复用 FR-SRC-06 的四态）
    /// 生成勾选清单、exe 目录内的绝对路径 ↔ `%LAUNCHER_DIR%` 批量互转（先确认、报条数）。
    /// 转换只改 `path` 一格——参数与工作目录说的是"这条命令怎么跑"，换台机器不该跟着变。
    ///
    /// 两处与需求书原文的偏差登记在这里：原文的便携写法是 `.\`，本仓写成 `%LAUNCHER_DIR%`
    /// （`.\` 认的是进程当前目录，而本程序从不 `SetCurrentDirectory`，同一份清单会时灵时不灵，
    /// 详见 `src/source/manage.rs` 顶部）；反向转换两种写法都认。
    fn manage_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("批量管理").strong());
        ui.label(
            egui::RichText::new(
                "勾选任意条目 → 转换路径或删除。删除只从清单里移除，不删磁盘文件。\n\
                 状态列沿用主界面那套四态：`-`＝不检查（网址 / 内置动作 / 含 {query}），\
                 `Offline`＝盘或网络位置不可达（拔掉的 U 盘、断线的共享），它不等于「文件找不到」，\
                 「只选失效项」也不会勾它。",
            )
            .weak()
            .small(),
        );
        ui.add_space(4.0);

        // Ctrl+R：重跑一遍四态检查。这一页的状态列就是现场，等三秒的兜底间隔不够用。
        // 确认框开着时不取键：那会儿 Enter/Esc 归它，重跑一轮只会让确认框里的计划与现实脱节。
        let hit_recheck = self.manage_confirm.is_none()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::R));
        let mut select_all = false;
        let mut clear_sel = false;
        let mut select_broken = false;
        let mut recheck = hit_recheck;
        ui.horizontal(|ui| {
            if ui.button("全选").clicked() {
                select_all = true;
            }
            if ui.add_enabled(!self.manage_checked.is_empty(), egui::Button::new("清空选择")).clicked() {
                clear_sel = true;
            }
            if ui
                .button("只选失效项")
                .on_hover_text("只勾「Not found」的行；Offline 不算——那是介质不在，条目本身没写错")
                .clicked()
            {
                select_broken = true;
            }
            if ui
                .button("重新校验 (Ctrl+R)")
                .on_hover_text("重跑一遍四态检查；断开的网络盘可能要等几秒")
                .clicked()
            {
                recheck = true;
            }
            ui.label(
                egui::RichText::new(format!("已勾选 {} 条", self.manage_checked.len()))
                    .weak()
                    .small(),
            );
        });

        if select_all {
            for it in &self.items {
                self.manage_checked.insert(it.uid());
            }
        }
        if clear_sel {
            self.manage_checked.clear();
        }
        if select_broken {
            let mut hit = 0usize;
            for it in &self.items {
                let uid = it.uid();
                if self.path_states.get(&uid) == Some(&source::pathcheck::PathState::NotFound) {
                    self.manage_checked.insert(uid);
                    hit += 1;
                }
            }
            if hit == 0 {
                self.toast = Some((
                    "没有查到失效项（Offline 不算失效）".to_string(),
                    Instant::now() + TOAST_TTL,
                    TOAST_OK,
                ));
                ui.ctx().request_repaint_after(TOAST_TTL);
            }
            eprintln!("批量管理：只选失效项 → 勾上 {hit} 条");
        }
        if recheck {
            if self.path_in_flight {
                // 上一轮还没回来（UNC 断线能把检查线程挂很久）：不攒线程，也不假装在跑。
                self.toast = Some((
                    "上一轮校验还没回来，稍等".to_string(),
                    Instant::now() + TOAST_TTL,
                    TOAST_ERR,
                ));
                ui.ctx().request_repaint_after(TOAST_TTL);
            } else {
                self.refresh_path_states(true);
                eprintln!("批量管理：重新校验路径（{} 条）", self.items.len());
            }
        }

        if self.items.is_empty() {
            ui.separator();
            ui.label(
                egui::RichText::new("清单是空的：先在主界面按 Ctrl+Shift+N 新建，或 Ctrl+I 批量导入。")
                    .weak()
                    .small(),
            );
            return;
        }

        // 快照出行（uid + 显示用字段 + 四态）。下面循环里只动局部变量，回填放循环之后——
        // 边迭代 `self.items` 边改 `self.manage_checked` 借不过去。
        let rows: Vec<(String, String, String, source::pathcheck::PathState)> = self
            .items
            .iter()
            .map(|it| {
                let uid = it.uid();
                let state = self.path_states.get(&uid).copied().unwrap_or_default();
                (uid, it.name.clone(), it.path.clone(), state)
            })
            .collect();
        let mut toggles: Vec<(String, bool)> = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("manage_rows")
            .max_height(260.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (uid, name, path, state) in &rows {
                    ui.horizontal(|ui| {
                        let mut on = self.manage_checked.contains(uid);
                        if ui.checkbox(&mut on, "").changed() {
                            toggles.push((uid.clone(), on));
                        }
                        ui.allocate_ui_with_layout(
                            egui::vec2(64.0, 18.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(state.label())
                                        .small()
                                        .color(path_state_color(*state)),
                                );
                            },
                        );
                        ui.label(name.as_str());
                        ui.add(egui::Label::new(
                            egui::RichText::new(shorten_path(path, 64)).weak().small(),
                        ))
                        .on_hover_text(path.as_str());
                    });
                }
            });
        for (uid, on) in toggles {
            if on {
                self.manage_checked.insert(uid);
            } else {
                self.manage_checked.remove(&uid);
            }
        }

        ui.separator();
        let checked_n = self.manage_checked.len();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.display().to_string()))
            .unwrap_or_default();
        let mut convert: Option<bool> = None;
        let mut ask_delete = false;
        ui.horizontal(|ui| {
            let some = checked_n > 0;
            if ui
                .add_enabled(some, egui::Button::new("转为相对路径…"))
                .on_hover_text(format!(
                    "把 {exe_dir} 下的绝对路径改写成 {}\\…；参数与工作目录不动",
                    source::manage::LAUNCHER_DIR
                ))
                .clicked()
            {
                convert = Some(true);
            }
            if ui
                .add_enabled(some, egui::Button::new("转为绝对路径…"))
                .on_hover_text("把 %LAUNCHER_DIR%\\… 或 .\\… 还原成当前程序目录下的绝对路径")
                .clicked()
            {
                convert = Some(false);
            }
            if ui
                .add_enabled(some, egui::Button::new(format!("删除选中…（{checked_n}）")))
                .on_hover_text("只从清单里移除这些条目，不删磁盘文件")
                .clicked()
            {
                ask_delete = true;
            }
            ui.label(
                egui::RichText::new(format!("共 {} 条", self.items.len()))
                    .weak()
                    .small(),
            );
        });

        if let Some(to_relative) = convert {
            let plan =
                source::manage::plan(&self.items, &self.manage_checked, &exe_dir, to_relative);
            if plan.changed() == 0 {
                self.toast = Some((
                    format!(
                        "选中的 {checked_n} 条都不适用{}",
                        if to_relative { "（不在本程序目录下或已是相对路径）" } else { "（不是相对路径写法）" }
                    ),
                    Instant::now() + TOAST_TTL,
                    TOAST_ERR,
                ));
                ui.ctx().request_repaint_after(TOAST_TTL);
                eprintln!("批量管理：转换请求落空（{checked_n} 条全部跳过）");
            } else {
                self.manage_confirm = Some(ManageConfirm::Convert {
                    to_relative,
                    changes: plan.changes,
                    skipped: plan.skipped,
                });
                ui.ctx().request_repaint();
            }
        }
        if ask_delete {
            self.manage_confirm = Some(ManageConfirm::Delete {
                uids: self.manage_checked.iter().cloned().collect(),
            });
            ui.ctx().request_repaint();
        }
    }

    /// 批次 D8（FR-SYS-06）：设置页第 9 页「使用统计」。
    ///
    /// 三段数据两种来源，刻意不混：
    /// - 合计 / 近 30 天 / 按功能次数 ← `usage.json`（本批新增的按天账本）；
    /// - 按条目前 10 ← 清单里现成的 `use_count`（只喂排序、以前从不给用户看）。
    /// 两者不互相推导：条目删掉之后它贡献过的次数仍留在 `usage.json` 里——"使用统计"
    /// 记的是功能被用了多少次，不是"现在清单里还剩哪些条目"。
    fn usage_ui(&mut self, ui: &mut egui::Ui) {
        use source::usage::Bucket;

        ui.label(egui::RichText::new("使用统计").strong());
        ui.label(
            egui::RichText::new(
                "只记次数：哪一类功能、哪天、几次。查询词、条目名、路径都不会进 usage.json，也不会进 launcher.log。",
            )
            .weak()
            .small(),
        );
        ui.add_space(4.0);

        let today = source::application::today_iso();
        // 保留期落到一个具体日期给用户看：只写"400 天"看不出这条线现在在哪儿。
        let oldest_kept =
            source::usage::add_days(&today, -(source::usage::KEEP_DAYS - 1)).unwrap_or_default();
        ui.label(
            egui::RichText::new(format!(
                "保留 {} 天：当前最早保留到 {oldest_kept}，更早的记录在每次记账时裁掉（裁的是「今天往前 {} 天」，不是「留最新 {} 条」）。",
                source::usage::KEEP_DAYS,
                source::usage::KEEP_DAYS,
                source::usage::KEEP_DAYS
            ))
            .weak()
            .small(),
        );
        ui.add_space(2.0);

        let total = self.usage.total();
        let days = self.usage.days_recorded();
        let first = self.usage.first_day().unwrap_or("—").to_string();
        if total == 0 {
            ui.label(egui::RichText::new("还没有任何记录：执行一次清单项、算式、网页搜索或文件行之后，这里就有数了。").weak());
        } else {
            ui.label(format!("合计 {total} 次 · 有记录 {days} 天 · 最早 {first}"));
        }
        ui.separator();

        // ---- 近 30 天柱状 ----
        ui.label(egui::RichText::new("近 30 天").strong());
        let window = self.usage.last_days(&today, 30);
        let max = window.iter().map(|(_, n)| *n).max().unwrap_or(0).max(1);
        let width = ui.available_width().max(160.0);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 72.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let baseline = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let bar_color = ui.visuals().selection.bg_fill;
        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.bottom() - 0.5),
                egui::pos2(rect.right(), rect.bottom() - 0.5),
            ],
            egui::Stroke::new(1.0, baseline),
        );
        let n = window.len().max(1) as f32;
        let gap = 2.0f32;
        let bar_w = ((rect.width() - gap * (n - 1.0)) / n).max(1.0);
        for (i, (_, count)) in window.iter().enumerate() {
            if *count == 0 {
                continue;
            }
            let h = ((rect.height() - 6.0) * (*count as f32 / max as f32)).max(2.0);
            let x = rect.left() + i as f32 * (bar_w + gap);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x, rect.bottom() - h),
                    egui::pos2(x + bar_w, rect.bottom()),
                ),
                egui::CornerRadius::ZERO,
                bar_color,
            );
        }
        // 悬停哪一格说哪一天：零次的日子也要能问（柱高为 0，只能按列命中）。
        if let Some(p) = resp.clone().hover_pos() {
            let idx = (((p.x - rect.left()) / (bar_w + gap)).floor() as isize)
                .clamp(0, window.len() as isize - 1) as usize;
            if let Some((day, count)) = window.get(idx) {
                resp.on_hover_text(format!("{day}：{count} 次"));
            }
        }
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(window.first().map(|(d, _)| d.as_str()).unwrap_or("—"))
                    .weak()
                    .small(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(format!("今天 {today}")).weak().small());
            });
        });
        ui.separator();

        // ---- 按功能次数 + 占比 ----
        ui.label(egui::RichText::new("按功能次数").strong());
        let denom = total.max(1) as f32;
        for b in Bucket::ALL {
            let count = self.usage.bucket_total(b);
            let share = count as f32 / denom * 100.0;
            ui.horizontal(|ui| {
                ui.add_sized(
                    [84.0, 18.0],
                    egui::Label::new(b.label()).truncate(),
                );
                ui.add_sized(
                    [64.0, 18.0],
                    egui::Label::new(format!("{count} 次")).truncate(),
                );
                let (bar, _) =
                    ui.allocate_exact_size(egui::vec2(120.0, 8.0), egui::Sense::hover());
                let p = ui.painter_at(bar);
                p.rect_filled(bar, egui::CornerRadius::ZERO, ui.visuals().faint_bg_color);
                if count > 0 {
                    p.rect_filled(
                        egui::Rect::from_min_max(
                            bar.min,
                            egui::pos2(
                                bar.left() + bar.width() * (count as f32 / denom),
                                bar.bottom(),
                            ),
                        ),
                        egui::CornerRadius::ZERO,
                        ui.visuals().selection.bg_fill,
                    );
                }
                ui.label(
                    egui::RichText::new(format!("{share:.1}%"))
                        .weak()
                        .small(),
                );
            });
        }
        let unknown = self.usage.unknown_total();
        if unknown > 0 {
            ui.label(
                egui::RichText::new(format!(
                    "其他（本版本认不出的桶）{unknown} 次——它算进合计，但不参与上面的分桶"
                ))
                .weak()
                .small(),
            );
        }
        // 2026-10-29 用户裁定（§六十三 记的那条「双记」）：两个桶语义各自成立，不去重，
        // 但必须写在这儿——否则下一个人（或下一轮的我）会把它当成重复计错的 bug 再查一遍。
        ui.label(
            egui::RichText::new(
                "一次启动可能落两个桶：清单里那些自定义命令既算「清单项」也算「自定义命令」\
                 （前者＝这条清单被启动，后者＝一条自定义命令被执行）⇒「合计」会比实际启动次数多。\
                 这是有意的口径，不是重复计错。",
            )
            .weak()
            .small(),
        );
        ui.separator();

        // ---- 按条目前 10（现成 use_count，只喂排序、从不给用户看的那一格）----
        ui.label(egui::RichText::new("按条目前 10").strong());
        let mut top: Vec<(String, u32)> = self
            .items
            .iter()
            .filter(|i| i.use_count > 0)
            .map(|i| (i.name.clone(), i.use_count))
            .collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if top.is_empty() {
            ui.label(egui::RichText::new("还没有成功启动过清单里的任何条目。").weak());
        } else {
            for (rank, (name, count)) in top.iter().take(10).enumerate() {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [28.0, 18.0],
                        egui::Label::new(egui::RichText::new(format!("{}.", rank + 1)).weak()),
                    );
                    ui.add_sized([240.0, 18.0], egui::Label::new(name.as_str()).truncate())
                        .on_hover_text(name.as_str());
                    ui.label(egui::RichText::new(format!("{count} 次")).weak().small());
                });
            }
            ui.label(
                egui::RichText::new("这一格统计的是清单条目本身（删掉条目这里就没了），与上面按功能的账本各算各的。")
                    .weak()
                    .small(),
            );
        }
        ui.separator();

        // ---- 清除 ----
        ui.label(egui::RichText::new("清除统计").strong());
        ui.label(
            egui::RichText::new(
                "把 usage.json 清成空表：合计、近 30 天柱状与「按功能次数」全部归零。\n条目上的累计次数与最近使用时间不动——那是排序用的另一本账，要清它去「高级」页的「重置学习排序」。这一条不写备份。",
            )
            .weak()
            .small(),
        );
        if ui.button("清除统计…").clicked() {
            self.pending_destructive = Some(Destructive::ClearUsage);
            eprintln!("设置页：点了「清除统计」，等确认");
            ui.ctx().request_repaint();
        }
    }

    /// 批次 D2（FR-SRC-11 的偏差收口）：设置页第 10 页「网页引擎」——列出 / 增删改 /
    /// 恢复出厂 / 选兜底引擎。在此之前增删改只能手改 `settings.json`。
    ///
    /// 三条口径：
    /// - 表就是 `settings.json` 的 `web_engines`（手改文件仍然有效）；`id` 与 `icon` 不在界面上：
    ///   前者是兜底与日志的锚、后者还没有显示入口（既有偏差，本批不碰）；
    /// - 编辑走跨帧缓冲（理由同文件搜索页），**改一格就写盘**（原样、不丢行）；
    ///   整理（残缺行丢弃、关键字去重）只在离开这一页时做，丢了几条走 Toast 说清；
    /// - 「兜底」是单选：出厂序列 Google → Bing，或某一条引擎。
    fn web_engines_ui(&mut self, ui: &mut egui::Ui) {
        use source::websearch;

        ui.label(egui::RichText::new("网页搜索引擎").strong());
        ui.label(
            egui::RichText::new(
                "「关键字 + 空格 + 词」独占整张列表（例：g 天气）。网址模板里的 {query} 换成词、\
                 {query|8000} 是空词时的默认值；模板里一个占位符都没有，那就是一条网址书签。\
                 关键字只在首词精确相等时独占（不分大小写），打一半（goo）不算。",
            )
            .weak()
            .small(),
        );
        ui.separator();

        if self.web_engines_edit.is_none() {
            self.web_engines_edit = Some(self.prefs.web_engines.clone());
        }
        let mut commit = false;
        let mut remove_row: Option<usize> = None;
        let mut add_row = false;
        let mut restore_default = false;
        {
            let list = self.web_engines_edit.as_mut().expect("上面刚填过");
            for (i, e) in list.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label("关键字");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut e.keyword)
                            .hint_text("g")
                            .desired_width(88.0),
                    );
                    if resp.lost_focus() {
                        commit = true;
                    }
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut e.title)
                            .hint_text("Google")
                            .desired_width((ui.available_width() - 60.0).max(60.0)),
                    );
                    if resp.lost_focus() {
                        commit = true;
                    }
                    if ui.button("删除").clicked() {
                        remove_row = Some(i);
                    }
                });
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut e.url)
                        .hint_text("https://www.google.com/search?q={query}")
                        .desired_width(ui.available_width()),
                );
                if resp.lost_focus() {
                    commit = true;
                }
                ui.add_space(2.0);
            }
        }
        // 现场体检：整理发生在离开这一页时，先在这儿说清楚"你看到的这些将会发生什么"。
        let (dups, incomplete) = self
            .web_engines_edit
            .as_ref()
            .map(|list| {
                let mut seen: Vec<String> = Vec::new();
                let mut dups = 0usize;
                let mut incomplete = 0usize;
                for e in list.iter() {
                    let kw = e.keyword.trim().to_lowercase();
                    if kw.is_empty() || e.url.trim().is_empty() {
                        incomplete += 1;
                        continue;
                    }
                    if seen.contains(&kw) {
                        dups += 1;
                    } else {
                        seen.push(kw);
                    }
                }
                (dups, incomplete)
            })
            .unwrap_or((0, 0));
        if incomplete > 0 || dups > 0 {
            let mut parts: Vec<String> = Vec::new();
            if incomplete > 0 {
                parts.push(format!("{incomplete} 条还没填关键字或网址"));
            }
            if dups > 0 {
                parts.push(format!("{dups} 条关键字与前面重复"));
            }
            ui.label(
                egui::RichText::new(format!(
                    "{}——离开这一页时不保存（重复的只留靠前那条）。",
                    parts.join("、")
                ))
                .small()
                .color(TOAST_ERR),
            );
        }
        ui.horizontal(|ui| {
            if ui.button("＋ 添加引擎").clicked() {
                add_row = true;
            }
            if ui.button("恢复出厂九条").clicked() {
                restore_default = true;
            }
        });
        if let Some(i) = remove_row {
            if let Some(list) = self.web_engines_edit.as_mut() {
                if i < list.len() {
                    list.remove(i);
                }
            }
            commit = true;
        }
        if add_row {
            let id = self
                .web_engines_edit
                .as_ref()
                .map(|l| websearch::next_custom_id(l))
                .unwrap_or_else(|| "custom-1".to_string());
            if let Some(list) = self.web_engines_edit.as_mut() {
                list.push(websearch::WebEngine {
                    id,
                    keyword: String::new(),
                    title: String::new(),
                    url: String::new(),
                    icon: String::new(),
                });
            }
            eprintln!("设置页：网页引擎表加了一条空行（填完关键字与网址才算数）");
            ui.ctx().request_repaint();
        }
        if restore_default {
            self.web_engines_edit = Some(websearch::builtin_engines());
            commit = true;
            eprintln!("设置页：网页引擎表恢复出厂九条（自定义过的条目被替换）");
        }
        if commit && self.save_web_engines_edit() {
            ui.ctx().request_repaint();
        }
        ui.label(
            egui::RichText::new(
                "改一格、点别处就保存。「恢复出厂九条」会把整张表换成出厂那九条，自己加的条目会被清掉。\
                 引擎的图标字段暂时不显示（既有偏差）。手改 settings.json 与本页等价。",
            )
            .weak()
            .small(),
        );
        ui.separator();

        // ---- 兜底（没有结果时的那几行）----
        ui.label(egui::RichText::new("兜底搜索").strong());
        ui.label(
            egui::RichText::new(
                "一条结果都没有时，列表末尾按这个序列给搜索行。出厂＝Google → Bing（表里没有的跳过）。\
                 选某一条引擎就只出它那一行。",
            )
            .weak()
            .small(),
        );
        let factory = websearch::default_fallback();
        let list = self
            .web_engines_edit
            .as_ref()
            .cloned()
            .unwrap_or_else(|| self.prefs.web_engines.clone());
        let current = if self.prefs.web_fallback == factory {
            "出厂：Google → Bing".to_string()
        } else if self.prefs.web_fallback.len() == 1 {
            let id = &self.prefs.web_fallback[0];
            match list.iter().find(|e| &e.id == id) {
                Some(e) if !e.title.is_empty() => format!("{}（{}）", e.title, e.keyword),
                Some(e) => e.keyword.clone(),
                None => "已删除的引擎（离开本页时改回出厂）".to_string(),
            }
        } else {
            "自定义序列".to_string()
        };
        let mut picked: Option<Vec<String>> = None;
        egui::ComboBox::from_id_salt("web_fallback_pick")
            .selected_text(current)
            .width(240.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(self.prefs.web_fallback == factory, "出厂：Google → Bing")
                    .clicked()
                {
                    picked = Some(factory.clone());
                }
                for e in list.iter() {
                    let one = vec![e.id.clone()];
                    let label = if e.title.is_empty() {
                        format!("（{}）", e.keyword)
                    } else {
                        format!("{}（{}）", e.title, e.keyword)
                    };
                    if ui
                        .selectable_label(self.prefs.web_fallback == one, label)
                        .clicked()
                    {
                        picked = Some(one);
                    }
                }
            });
        if let Some(v) = picked {
            self.prefs.web_fallback = v;
            source::application::save_settings(&self.prefs);
            eprintln!(
                "设置页：兜底序列已改（现 {} 条，id 不落日志）",
                self.prefs.web_fallback.len()
            );
            ui.ctx().request_repaint();
        }
    }


    /// 设置页（2026-10-17 用户要求，参考 AHK 版设置窗口的左侧分类栏；只立当下有真实选项的分类，
    /// 不做无功能的占位开关）。入口：托盘「设置…」/ `Ctrl+,`；Esc 或「返回搜索」回搜索页。
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        // 换到别的分类 ⇒ 这一页的编辑缓冲作废（回写后丢掉，见 [`finish_file_scope_edits`]）。
        if self.settings_tab != SettingsTab::FileSearch {
            self.finish_file_scope_edits();
        }
        // 批次 D5：不在「批量管理」页就把勾选丢掉。uid 派生自路径，别处（IPC 导入、编辑弹窗）
        // 改过清单之后旧 uid 会变成幽灵——留着它们，转换与删除的条数就对不上了。
        if self.settings_tab != SettingsTab::Manage {
            self.manage_checked.clear();
        }
        // 批次 D2：换到别的分类 ⇒ 引擎表的编辑缓冲回写后丢掉（见 [`finish_web_engine_edits`]）。
        if self.settings_tab != SettingsTab::WebEngines {
            self.finish_web_engine_edits();
        }
        // 顶栏空白也是拖动手柄（2026-10-18 用户：「顶部输入框右侧点击空白不能拖动，顶部的不是菜单栏吗？」）。
        // 右边界必须取"整行可用宽度"，不能用 `head_row.response.rect.max.x`：横向行的 response 矩形
        // 只覆盖**已用宽度**（返回按钮 + 标题 ≈ 110px），用它算出来的手柄宽度是负数、被 drag_handle
        // 直接丢弃 ⇒ 用户实测"进设置页就拖不动了"。
        let band = ui.available_rect_before_wrap();
        let mut heading_right = 0.0f32;
        let head_row = ui.horizontal(|ui| {
            if ui.button("← 返回搜索 (Esc)").clicked() {
                self.settings_open = false;
                self.manage_checked.clear();
                // 批次 D2：整页关掉时把所有编辑缓冲回写（这条路上 `settings_ui` 已经不再跑了，
                // 页内"换分类"那条 finish 分支盖不到）。
                self.finish_settings_edits();
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
                    ("文件搜索", SettingsTab::FileSearch),
                    ("外观", SettingsTab::Appearance),
                    ("高级", SettingsTab::Advanced),
                    ("批量管理", SettingsTab::Manage),
                    ("使用统计", SettingsTab::Usage),
                    ("网页引擎", SettingsTab::WebEngines),
                ] {
                    if ui.selectable_label(self.settings_tab == tab, label).clicked() {
                        // 批次 D5：进「批量管理」页时强制重跑一轮路径四态——这一页的状态列就是
                        // 要看的现场，等 3 秒的兜底间隔会让人对着旧结论勾"失效项"。
                        if tab == SettingsTab::Manage && self.settings_tab != tab {
                            self.refresh_path_states(true);
                        }
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
                            ui.separator();
                            // FR-QRY-10（批次 C3）：搜索历史上限。落盘的只有文本，不存结果、不存 uid。
                            ui.label(egui::RichText::new("搜索历史").strong());
                            let mut limit = self.prefs.history_size as f32;
                            let hr = ui.add(
                                egui::Slider::new(
                                    &mut limit,
                                    0.0..=source::application::HISTORY_SIZE_MAX as f32,
                                )
                                .step_by(1.0)
                                .fixed_decimals(0)
                                .text("记住最近"),
                            );
                            if hr.changed() {
                                // 滑块只走 `0..=HISTORY_SIZE_MAX`，`round()` 后必在区间内；
                                // `max(0.0)` 是给手写过负数的那份 settings.json 兜底（用的时候还有一道夹）。
                                self.prefs.history_size = limit.round().max(0.0) as usize;
                            }
                            // 拖动中每帧都在变 ⇒ 只有松手那一帧（或键盘/点击步进改的）才写盘，
                            // 否则一次拖动就把 settings.json 的 mtime 刷成心跳（与外观页「窗口宽度」同一口径）。
                            if hr.drag_stopped()
                                || (hr.changed() && !ui.input(|i| i.pointer.primary_down()))
                            {
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：history_size = {}（已写 settings.json）",
                                    self.prefs.history_size
                                );
                            }
                            ui.label(
                                egui::RichText::new(format!(
                                    "记住最近 {} 条：每次执行（回车启动、点开文件行、> 开头的终端命令、计算器复制、面板里选一条走掉）把输入框那串存进 search_history.json。Ctrl+↑ 往更早翻，Ctrl+↓ 翻回来；翻到最末再按一次就回到你自己敲的那串。\n只存文本、不存结果：回填进来的串不会替你启动任何东西，还是按回车才执行。重复的按小写认（C:\\Foo 与 c:\\foo 算同一条），存的时候按原样存。\n调到 0＝不再记；下一次执行会把已有的那批一起清掉并落盘。想现在就清干净、又不改上限，用「高级」页那个「清空搜索历史」。\n这张表里的原文只进 search_history.json，绝不进 launcher.log。",
                                    self.prefs.history_size
                                ))
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
                                    "点「录制新热键」后直接按下想要的组合，不需要再点确认：注册成功立刻生效并写进 settings.json；被系统或其它程序占用会照实报错，并自动回到原来那一组（你原来的设置不会被冲掉）。\n可录的键：字母、数字、空格、回车、反引号 ` 和 F1–F24。非 F 键必须带 Ctrl、Alt 或 Win，因为裸 A、Shift+A 都只是打字，挂成全局键等于一路拦你输入。\n裸键只放开 ` 这一个（批次 R，2026-10-20 按你的要求加的）：代价是设成它以后全系统都打不出 ` 了，所以请照下面「补字符热键」那张卡片配一个出口；Shift+` 打出的 ~ 不受影响（本机实测）。录制期间程序会先把当前全局热键临时注销——不然裸 ` 被系统整条吞掉，本程序一个按键都收不到，永远录不成；退出录制（落定 / Esc / 取消按钮 / 关窗 / 隐藏到托盘）立刻装回去，另有一个 30 秒兜底定时器防止卡在注销状态。\nWin 组合键在这里录不了：egui 在 Windows 上不把 Win 当作修饰键传上来（你按 Ctrl 时它给的 command 位就是 Ctrl 的副本）。想要 Win+X，用它下面那张补字符卡片上的复选框。\n本程序自己的组合（Ctrl+N/E/D/I/J/K/M/L/B/R、Ctrl+Shift+N/D/P、Ctrl+数字、Alt+数字、F1–F4、Alt+Enter）会先在本地挡一道；这张名单不穷尽，以真被挡下来的那一刻的提示为准。全局占用只能靠\"试注册\"发现——Windows 没有枚举别人热键的 API，所以\"没报错\"只代表这一刻系统允许它注册上，不代表全机没人想用这个键。\n还有一点要知道：全局热键是被系统整条吞掉的，设成 Ctrl+A、Ctrl+C 这类常用编辑键，别的程序里那个动作就跟着没了（本程序没绑它们，所以不会在这里拦你，得自己避）。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // 批次 G1（FR-WIN-02 / 开发计划 §8.7）：第二颗呼出键。形态照上面那张，
                            // 差别只有"没有出厂值可退"——它出厂就是 Alt+R，清除之后是真的没有。
                            ui.label(egui::RichText::new("副呼出热键（第二颗呼出键）").strong());
                            ui.horizontal(|ui| {
                                let shown = if self.extra_recording {
                                    "按下新组合…（Esc / Tab 取消，Backspace / Delete 恢复默认）"
                                        .to_string()
                                } else if let Some(w) = self.extra_awaiting {
                                    format!("{} 注册中…", w.display())
                                } else {
                                    match self.extra_active {
                                        Some(k) => k.display().to_string(),
                                        None => "（未启用）".to_string(),
                                    }
                                };
                                ui.label(egui::RichText::new(shown).monospace());
                                if ui
                                    .button(if self.extra_recording {
                                        "取消录制"
                                    } else {
                                        "录制新热键"
                                    })
                                    .clicked()
                                {
                                    self.extra_recording = !self.extra_recording;
                                    eprintln!(
                                        "设置页：副呼出键录制 {}（当前 {}）",
                                        if self.extra_recording { "开始" } else { "取消" },
                                        match self.extra_active {
                                            Some(k) => k.display().to_string(),
                                            None => "未启用".to_string(),
                                        }
                                    );
                                }
                                let busy =
                                    self.extra_recording || self.extra_awaiting.is_some();
                                ui.add_enabled_ui(!busy, |ui| {
                                    if ui.button("恢复默认").clicked() {
                                        self.request_extra(HotkeySpec::EXTRA_DEFAULT);
                                    }
                                    if ui.button("清除").clicked() {
                                        if self.extra_active.is_none()
                                            && self.prefs.extra_hotkey.is_none()
                                        {
                                            self.hotkey_note("ℹ 本来就没有副呼出热键", true);
                                        } else {
                                            self.clear_extra();
                                        }
                                    }
                                });
                            });
                            ui.label(
                                egui::RichText::new(
                                    "出厂是 Alt+R，与你上面那颗主热键一样：按一下就呼出 / 隐藏（AHK 版的「次热键」就是这个）。\n和主热键同一套规矩：必须带修饰键、不能和主热键或补字符热键是同一组；注册成功才写 settings.json，失败会照实报错并把原来那颗装回去。「清除」之后只剩主呼出键，想再要回来点「恢复默认」。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            // 批次 G1（FR-WIN-03）：双击修饰键呼出。AHK `App.ahk` 的同名功能：
                            // 400ms 窗口、300ms 长按阈值；另按需求书加了 25ms 最小间隔挡远控抖动。
                            ui.label(egui::RichText::new("双击呼出（连按两下同一个修饰键）").strong());
                            let mut tap_mode = self.prefs.dbl_tap;
                            ui.horizontal(|ui| {
                                for m in DblTap::ALL {
                                    ui.selectable_value(&mut tap_mode, m, m.label());
                                }
                            });
                            if tap_mode != self.prefs.dbl_tap {
                                self.prefs.dbl_tap = tap_mode;
                                source::application::save_settings(&self.prefs);
                                // 钩子只活在热键线程里（低级键盘钩子的回调跑在装它的线程上），
                                // 所以这里只投消息，换档与重置判定状态都在那边做。
                                post_double_tap(tap_mode);
                                eprintln!(
                                    "设置页：双击呼出 → {}（已写 settings.json，钩子已换档）",
                                    tap_mode.as_str()
                                );
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "连按两下同一个修饰键切一次显隐（左右 Ctrl / Shift 都算，两下可以是不同的那一边）。\n判定口径：两下之间 400ms 以内才算双击；单次按住超过 300ms 不算（那是组合键的起手，例如 Ctrl+C）；两次抬起间隔小于 25ms 的当成远控软件 / 自动化的同步抖动丢掉。Shift+CapsLock 这类按下修饰键再按别的键照常工作，不会被当成双击。\n默认关闭。开启时本程序会挂一个「只旁听、不拦键」的键盘钩子：所有按键照常送到你正在用的程序，它只负责数那两下。"
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
                            // 批次 E3-丙（FR-ACT-01「Ctrl+Enter 走次级动作」/ 开发计划 B3）：
                            // 用户 2026-10-26 裁定两种形态都做、默认快捷键版、可切换、键也可自定义。
                            ui.label(egui::RichText::new("次级动作键（选中一行后按它）").strong());
                            let mut mode = self.prefs.secondary_action;
                            ui.horizontal(|ui| {
                                for m in [SecondaryAction::Shortcut, SecondaryAction::Panel] {
                                    ui.selectable_value(&mut mode, m, m.label());
                                }
                            });
                            if mode != self.prefs.secondary_action {
                                self.prefs.secondary_action = mode;
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：次级动作形态 → {}（已写 settings.json）",
                                    mode.as_str()
                                );
                                ui.ctx().request_repaint();
                            }
                            ui.horizontal(|ui| {
                                let shown = if self.secondary_recording {
                                    "按下新组合…（Esc / Tab 取消，Backspace / Delete 恢复默认）"
                                        .to_string()
                                } else {
                                    self.prefs.secondary_hotkey.display()
                                };
                                ui.label(egui::RichText::new(shown).monospace());
                                if ui
                                    .button(if self.secondary_recording {
                                        "取消录制"
                                    } else {
                                        "录制新键"
                                    })
                                    .clicked()
                                {
                                    self.secondary_recording = !self.secondary_recording;
                                    eprintln!(
                                        "设置页：次级动作键录制 {}（当前 {}）",
                                        if self.secondary_recording { "开始" } else { "取消" },
                                        self.prefs.secondary_hotkey.display()
                                    );
                                }
                                ui.add_enabled_ui(!self.secondary_recording, |ui| {
                                    if ui.button("恢复默认").clicked() {
                                        self.prefs.secondary_hotkey = HotkeySpec::SECONDARY_DEFAULT;
                                        source::application::save_settings(&self.prefs);
                                        eprintln!(
                                            "设置页：次级动作键恢复默认 {}（已写 settings.json）",
                                            HotkeySpec::SECONDARY_DEFAULT.display()
                                        );
                                        ui.ctx().request_repaint();
                                    }
                                });
                            });
                            ui.label(
                                egui::RichText::new(format!(
                                    "「快捷键版」＝按 {} 直接做一条：文件行 / 目录行「在资源管理器里定位」，网页搜索行「复制网址」，计算器 / 文本行「粘贴到原窗口」（2026-10-27 批次 E5 起；这一条要求呼出时记下了\"之前的窗口\"，没记下就拒投并明说，不会假装投过了）；「面板版」＝跟 → 一样进操作面板，动作进去再选。\n应用条目、系统命令、自定义命令没有\"一眼就是它\"的次级动作 —— 快捷键版会提示你按 → 看全部，不替你猜（Enter 本来就是打开，再配一个别名只会让两个键争同一件事）。\n这颗键只在搜索页、没有弹窗、没开面板时生效，而且不注册给系统：不会抢别的程序的键，也不会像全局呼出键那样被系统整条吞掉。必须带 Ctrl 或 Alt（裸键与单带 Shift 的键都只是打字），Win 组合设不了（egui 在 Windows 上不把 Win 传上来）。\n改完立刻生效并直接写 settings.json —— 系统里没有它，不需要\"试注册\"那一级，也就没有\"被别的程序占用\"这回事。",
                                    self.prefs.secondary_hotkey.display()
                                ))
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            let toggle_key = self.hotkey_active.display();
                            let extra_key = self
                                .extra_active
                                .map(|k| k.display())
                                .unwrap_or_else(|| "（副呼出未启用）".to_string());
                            let tap_key = self.prefs.dbl_tap.label();
                            let typeback_key = self
                                .typeback_active
                                .map(|t| t.display())
                                .unwrap_or_else(|| "（补字符未启用）".to_string());
                            let secondary_key = self.prefs.secondary_hotkey.display();
                            egui::Grid::new("hotkeys_grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                                for (key, desc) in [
                                    (toggle_key.as_str(), "全局呼出 / 隐藏（上一张卡片可改）"),
                                    (extra_key.as_str(), "副呼出：第二颗呼出键，效果同上（上一张卡片可改、可清除；批次 G1）"),
                                    (tap_key, "双击呼出：连按两下 Ctrl 或 Shift 切一次显隐（上一张卡片可改；默认关闭）"),
                                    (secondary_key.as_str(), "次级动作：快捷键版直接做一条（文件行定位 / 网页行复制网址 / 文本行粘贴到原窗口），面板版等于 →（上一张卡片可改）"),
                                    (typeback_key.as_str(), "补字符：把呼出键吞掉的那个字符打回焦点窗口（上一张卡片可改）"),
                                    ("↑ / ↓", "选择条目"),
                                    ("Ctrl+↑ / Ctrl+↓", "翻搜索历史：往更早一条 / 往更新一条，翻到最末再按 Ctrl+↓ 回到你自己敲的那串（批次 C3；上限见「通用」页「搜索历史」，面板与文件模式下不接管）"),
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
                                    ("Ctrl+E / F3", "编辑选中项（F3 是批次 B8 加的别名，同一条路；面板开着时都不接管）"),
                                    ("Delete", "删除选中项（仅输入为空时；弹确认框）"),
                                    ("Ctrl+Shift+D", "删除选中项（不受输入限制；弹确认框）"),
                                    ("Ctrl+D", "打开选中项所在目录（文件行：在资源管理器里定位这个文件）"),
                                    ("Ctrl+Shift+P", "置顶 / 取消置顶选中项（图标右上角角标）"),
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
                                    ("Ctrl+, / F2", "打开 / 返回设置页（F2 是批次 B8 加的别名；两颗键在设置页上也照样有效）"),
                                    ("F1", "关于本程序：版本、程序位置、清单条数（Esc 关闭；批次 B8）"),
                                    ("F4", "打开配置文件所在的目录（settings.json / user_items.json / launcher.log 都在那一层；批次 B8）"),
                                    ("Ctrl+C", "复制当前这一行：文件取路径、计算器取数字、应用与命令取名称（批次 B8）；输入框里有选区时先复制选中的文字；不关窗"),
                                    ("鼠标滚轮", "在结果列表上滚一格＝选中上/下一行，两端夹住不回绕；列表位置仍由「滚动跟随选中」负责（批次 B8）"),
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
                        SettingsTab::FileSearch => {
                            // 批次 D7（FR-SRC-02 / FR-SRC-14）：文件索引的范围与时机。
                            // 所有改动走同一条路：改 prefs → 写 settings.json；下一帧 poll 循环里
                            // `apply_settings` 把新范围喂给文件源，`set_scope` 比对不同才重扫
                            // ⇒ 「保存即重扫」是自动的，这一页不用自己碰 sources。
                            ui.label(egui::RichText::new("扫描目录").strong());
                            // 编辑缓冲必须跨帧（见 `file_folders_edit` 字段注释）。
                            if self.file_folders_edit.is_none() {
                                self.file_folders_edit = Some(self.prefs.file_folders.clone());
                            }
                            let mut commit_folders = false;
                            let mut remove_row: Option<usize> = None;
                            {
                                let folders = self.file_folders_edit.as_mut().expect("上面刚填过");
                                for (i, f) in folders.iter_mut().enumerate() {
                                    ui.horizontal(|ui| {
                                        let resp = ui
                                            .add(
                                                egui::TextEdit::singleline(f)
                                                    .hint_text(
                                                        "%USERPROFILE%\\Documents 或 D:\\data",
                                                    )
                                                    .desired_width(ui.available_width() - 44.0),
                                            )
                                            .on_hover_text(
                                                "支持 %环境变量%；目录不存在时这一行不参与扫描",
                                            );
                                        // 路径是打字输入：提交在失焦那刻，不是每个键一写盘一重扫。
                                        // 判据只用 `lost_focus`——`changed()` 只在那帧真有编辑时为真，
                                        // 而失焦落在后来的点击帧，`changed() && lost_focus()` 几乎
                                        // 不可能同帧成立（2026-10-28 真机：那一版等于永不提交）。
                                        if resp.lost_focus() {
                                            *f = f.trim().to_string();
                                            commit_folders = true;
                                        }
                                        if ui.button("删除").clicked() {
                                            remove_row = Some(i);
                                        }
                                    });
                                }
                                if let Some(i) = remove_row {
                                    folders.remove(i);
                                    commit_folders = true;
                                }
                                if ui.button("添加目录").clicked() {
                                    folders.push(String::new());
                                    commit_folders = true;
                                }
                            }
                            if commit_folders && self.commit_file_folders_edit() {
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "改完点别处（离开输入框）生效；添加/删除立刻生效。生效后自动在后台重建索引，\
                                     旧索引在建成前继续可用，不卡界面。目录不存在的行跳过。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            ui.label(egui::RichText::new("排除的目录名").strong());
                            // 与目录清单同一条跨帧缓冲（见 `file_exclude_edit` 字段注释）。
                            if self.file_exclude_edit.is_none() {
                                self.file_exclude_edit = Some(self.prefs.file_exclude.clone());
                            }
                            let mut commit_exclude = false;
                            let mut remove_ex: Option<usize> = None;
                            {
                                let exclude = self.file_exclude_edit.as_mut().expect("上面刚填过");
                                for (i, x) in exclude.iter_mut().enumerate() {
                                    ui.horizontal(|ui| {
                                        let resp = ui.add(
                                            egui::TextEdit::singleline(x)
                                                .hint_text("node_modules")
                                                .desired_width(ui.available_width() - 44.0),
                                        );
                                        // 按目录名整段跳过（大小写不敏感），连它下面的文件一起不进索引。
                                        if resp.lost_focus() {
                                            *x = x.trim().to_string();
                                            commit_exclude = true;
                                        }
                                        if ui.button("删除").clicked() {
                                            remove_ex = Some(i);
                                        }
                                    });
                                }
                                if let Some(i) = remove_ex {
                                    exclude.remove(i);
                                    commit_exclude = true;
                                }
                                ui.horizontal(|ui| {
                                    if ui.button("添加目录名").clicked() {
                                        exclude.push(String::new());
                                        commit_exclude = true;
                                    }
                                    if ui.button("恢复默认排除").clicked() {
                                        *exclude =
                                            source::filesearch::default_scope_exclude();
                                        commit_exclude = true;
                                    }
                                });
                            }
                            if commit_exclude && self.commit_file_exclude_edit() {
                                ui.ctx().request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "排除按目录名整段跳过（大小写不敏感），连它下面的文件一起不进索引。\
                                     「恢复默认排除」把整张表写回出厂七条（node_modules / .git 等）——\
                                     自己加的条目会被清掉，误删可以再加回来。",
                                )
                                .weak()
                                .small(),
                            );
                            ui.separator();
                            ui.label(egui::RichText::new("档位").strong());
                            ui.horizontal(|ui| {
                                ui.label("递归深度");
                                let mut depth = self.prefs.file_depth;
                                let before = depth;
                                for (v, l) in [(2usize, "2"), (4, "4（默认）"), (6, "6"), (8, "8")] {
                                    ui.selectable_value(&mut depth, v, l);
                                }
                                if depth != before {
                                    self.prefs.file_depth = depth;
                                    source::application::save_settings(&self.prefs);
                                    eprintln!("设置页：file_depth = {depth}（已写 settings.json）");
                                    ui.ctx().request_repaint();
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("条目上限");
                                let mut cap = self.prefs.file_max_entries;
                                let before = cap;
                                for (v, l) in [
                                    (10_000usize, "1 万"),
                                    (30_000, "3 万（默认）"),
                                    (100_000, "10 万"),
                                ] {
                                    ui.selectable_value(&mut cap, v, l);
                                }
                                if cap != before {
                                    self.prefs.file_max_entries = cap;
                                    source::application::save_settings(&self.prefs);
                                    eprintln!("设置页：file_max_entries = {cap}（已写 settings.json）");
                                    ui.ctx().request_repaint();
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("重扫间隔");
                                let mut mins = self.prefs.file_refresh_minutes;
                                let before = mins;
                                for (v, l) in [
                                    (5u64, "5 分钟"),
                                    (15, "15 分钟"),
                                    (30, "30 分钟（默认）"),
                                    (60, "60 分钟"),
                                    (180, "180 分钟"),
                                ] {
                                    ui.selectable_value(&mut mins, v, l);
                                }
                                if mins != before {
                                    self.prefs.file_refresh_minutes = mins;
                                    source::application::save_settings(&self.prefs);
                                    eprintln!(
                                        "设置页：file_refresh_minutes = {mins}（已写 settings.json）"
                                    );
                                    ui.ctx().request_repaint();
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("最小字数");
                                let mut mql = self.prefs.file_min_query_len;
                                let before = mql;
                                for (v, l) in [(1usize, "1"), (2, "2（默认）"), (3, "3")] {
                                    ui.selectable_value(&mut mql, v, l);
                                }
                                if mql != before {
                                    self.prefs.file_min_query_len = mql;
                                    source::application::save_settings(&self.prefs);
                                    eprintln!(
                                        "设置页：file_min_query_len = {mql}（已写 settings.json）"
                                    );
                                    ui.ctx().request_repaint();
                                }
                            });
                            ui.label(
                                egui::RichText::new(
                                    "「（默认）」标的是出厂档。手改 settings.json 越过档位的值照常生效，\
                                     界面上不选中任何档，点任意一档即覆盖。最小字数管的是进文件模式（'词 / 空格开头）\
                                     的门槛，与普通搜索无关。",
                                )
                                .weak()
                                .small(),
                            );
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
                            // 批次 F3（FR-WIN-09 ShowOn）：只在多显示器下看得出差别，单屏三档等价。
                            // 选中即刻重摆一次（挪到别的屏就是反馈），下次呼出照样按新档位落点。
                            ui.label("呼出落点");
                            let mut show_on = self.prefs.appearance.show_on;
                            egui::ComboBox::from_id_salt("show_on")
                                .selected_text(show_on.label())
                                .show_ui(ui, |ui| {
                                    use source::application::ShowOn;
                                    for opt in [ShowOn::Mouse, ShowOn::Primary, ShowOn::Active] {
                                        ui.selectable_value(&mut show_on, opt, opt.label());
                                    }
                                });
                            if show_on != self.prefs.appearance.show_on {
                                self.prefs.appearance.show_on = show_on;
                                source::application::save_settings(&self.prefs);
                                eprintln!(
                                    "设置页：show_on = {}（已写 settings.json）",
                                    show_on.as_str()
                                );
                                if let Some(hwnd) = self.self_hwnd {
                                    self.place_on_summon(hwnd);
                                }
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
                            // FR-QRY-10（批次 C3）：清空搜索历史。只清表，「通用」页那个上限不动。
                            ui.label(egui::RichText::new("清空搜索历史").strong());
                            ui.label(
                                egui::RichText::new(format!(
                                    "现在存着 {} 条（上限 {}）。按这个把 search_history.json 清成空表，Ctrl+↑ 从空表翻起；「通用」页那个上限不受影响。\n这一条不写备份：上面两个按钮都先备份，因为它们清的是统计；这张表存的是你亲手敲过的原文，留一份副本正好是「清空」二字的反面。想让它以后一条都不记，把上限调到 0。",
                                    self.history.len(),
                                    self.prefs.history_size
                                ))
                                .weak()
                                .small(),
                            );
                            if ui.button("清空搜索历史…").clicked() {
                                self.pending_destructive = Some(Destructive::ClearHistory);
                                eprintln!("设置页：点了「清空搜索历史」，等确认");
                                ui.ctx().request_repaint();
                            }
                            ui.separator();
                            // 批次 B6（FR-ACT-07）：清空参数历史。与上面那条同口径——只清表、不备份。
                            ui.label(egui::RichText::new("清空参数历史").strong());
                            if self.param_history.is_empty() {
                                ui.label(
                                    egui::RichText::new(
                                        "现在一条都没存：带参项的参数框里填过值之后，这里才会有东西可以清。",
                                    )
                                    .weak()
                                    .small(),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "现在存着 {} 条（上限 {}；顺序是新的在前，复用的只加计数、不挪位置）。\n参数框下面那列「最近用过的参数」就是它；清空后你得重新一个一个敲。\n这一条同样不写备份：清的就是你亲手敲过的原文。",
                                        self.param_history.len(),
                                        source::params::HISTORY_LIMIT
                                    ))
                                    .weak()
                                    .small(),
                                );
                                if ui.button("清空参数历史…").clicked() {
                                    self.pending_destructive = Some(Destructive::ClearParamHistory);
                                    eprintln!("设置页：点了「清空参数历史」，等确认");
                                    ui.ctx().request_repaint();
                                }
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
                        SettingsTab::Manage => self.manage_ui(ui),
                        SettingsTab::Usage => self.usage_ui(ui),
                        SettingsTab::WebEngines => self.web_engines_ui(ui),
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
        // 批次 E4（FR-ACT-05）：标记不跨呼出周期存活（AHK `SearchWindow.Marked := Map()` 同口径）。
        // 留着上一轮的标记，下次呼出一按 `→` 就会对用户早已忘记的一组文件动手。
        self.marked.clear();
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
        // 批次 F3（FR-WIN-09）：存的是**在工作区里的千分比**（AHK `_OnMoved` 同口径），
        // 换分辨率 / 拔插显示器之后仍落在差不多的地方；窗口中心所在的那块屏的工作区为准。
        if self.prefs.appearance.remember_position {
            let rel = self.self_hwnd.and_then(|hwnd| {
                // 最小化态的 `GetWindowRect` 给的是屏幕外的假坐标（-32000 那一带），
                // 折成千分比会记成 [0, 0]，下次呼出就贴到左上角去了。
                if unsafe { IsIconic(hwnd).as_bool() } {
                    return None;
                }
                let size = win_size(hwnd)?;
                let pos = win_origin(hwnd)?;
                let work = work_at_window(hwnd)?;
                Some(relative_in_work(work, size, pos))
            });
            if let Some(rel) = rel {
                if self.prefs.appearance.pos_rel != Some(rel) {
                    self.prefs.appearance.pos_rel = Some(rel);
                    source::application::save_settings(&self.prefs);
                    eprintln!(
                        "窗口：记住位置（工作区 {}/1000、{}/1000）",
                        rel[0], rel[1]
                    );
                }
            }
        }
        // 批次 F6：☰ 菜单不跨隐藏保留。失焦自动隐藏时 Popup 那一帧不再运行，也就没人把
        // `menu_open` 写回 false——不在这里清掉，下次呼出会带着一个悬着的菜单出现。
        self.menu_open = false;
        // 批次 P1（FR-ACT-03）：面板同样不跨隐藏保留。失焦自动隐藏时不会有第二帧来退出它——
        // 留着的话下次呼出直接落在"框里是过滤词、列表只有动作"那一屏（与 ☰ 菜单同一条处理）。
        // 原查询与高亮行在这里放回：不放回就是把用户没打过的过滤词当成他打的词。
        // 光标也要一起放到末尾（`query` 是程序化改写的，见 [`caret_to_end`]）。
        if let Some(p) = self.panel.take() {
            let chars = p.saved_query.chars().count();
            eprintln!(
                "操作面板：窗口隐藏 ⇒ 退出面板，原查询（{chars} 个字符，不落原文）与第 {} 行已放回",
                p.saved_selected + 1
            );
            self.query = p.saved_query;
            self.selected = p.saved_selected;
            if let Some(ctx) = CTX.get() {
                caret_to_end(ctx, chars);
            }
        }
        // 批次 E4：批量回收站的确认框同样不跨隐藏保留——留着的话下次呼出会直接看见一句
        // 「把这 N 项移到回收站？」，而用户早已忘了自己标过什么。
        if self.batch_confirm.take().is_some() {
            eprintln!("全部回收站：窗口隐藏 ⇒ 取消确认（未删除）");
        }
        // 批次 C3：翻历史的位置同样不跨隐藏保留。不归零的症状最扎手：执行一次之后那条被
        // `push_history` 挪到了第 1 格，而 `index` 还停在上一轮的 3 ⇒ 下次呼出按 `Ctrl+↑`
        // 从第 4 格起跳，前三条全被跳过。草稿也一起丢（那串属于上一轮，不是这一轮要回来的东西）。
        if self.history_browse.index != 0 {
            self.history_browse = Default::default();
            eprintln!("搜索历史：窗口隐藏 ⇒ 翻页位置归零");
        }
        // 批次 R：窗口一隐藏，eframe 就不再跑 `ui()` ⇒ `hotkey_frame` 末尾那条边沿检测等不到
        // 下一帧。取键态若还开着，全局呼出会一直停在"临时注销"状态（按了没反应），所以在这里
        // 就地退出取键并把键装回。放在这个函数最前面是因为它下面有一条 `return`。
        if self.hotkey_recording || self.typeback_armed || self.extra_recording {
            self.hotkey_recording = false;
            self.typeback_armed = false;
            self.extra_recording = false;
            eprintln!("设置页：窗口隐藏 ⇒ 退出取键（录制/选键状态不跨隐藏保留）");
            self.sync_keypick_suspend();
        }
        // 批次 E3-丙：次级动作键框同理，但它没进 `sync_keypick_suspend`（那颗键不在系统里，
        // 没有"装回去"这回事），所以只落自己的旗标。
        if self.secondary_recording {
            self.secondary_recording = false;
            eprintln!("设置页：窗口隐藏 ⇒ 退出次级动作键录制");
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

    /// 批次 D4（FR-CFG-06）：把一组算好的值填进弹窗表单；别名/名称已有别的值就先挂询问。
    /// 「选文件」「选目录」「拖入」三条路共用这一处收尾，免得三份冲突判定各写一遍走样。
    /// 日志只记来源与"问没问"，**路径与别名不进日志**（批次 P 口径）。
    fn fill_dialog(
        &mut self,
        ctx: &egui::Context,
        fill: source::import::ImportFill,
        origin: &str,
    ) {
        let Some(dg) = self.dialog.as_mut() else {
            return;
        };
        match make_fill_prompt(dg, &fill) {
            Some(p) => {
                dg.fill_prompt = Some(p);
                eprintln!("弹窗：{origin}与已填的别名/名称不同，等用户选「替换」还是「保留」（内容不落日志）");
            }
            None => {
                apply_fill(dg, &fill, true);
                eprintln!("弹窗：{origin}已填入表单（内容不落日志）");
            }
        }
        ctx.request_repaint();
    }

    /// 批次 D4（FR-CFG-06）：拖动文件/文件夹到弹窗上（Delphi `frmShortCut` 的 `WM_DropFiles`）。
    /// 只取第一个——`DroppedFileHandle` 里多选没走通（`DragAcceptFiles` 那条路 Delphi 也只取一个）。
    fn dialog_drop(&mut self, ctx: &egui::Context, path: &str) {
        let Some(kind) = self.dialog.as_ref().map(|d| d.kind) else {
            return;
        };
        // 系统命令的命令行就是 `internal:<id>`，被外部路径覆盖后这条就执行不了了。
        if kind == ItemKind::Internal {
            self.internal_toast(ctx, "⚠ 系统命令不接受外部路径（命令行是 internal:<id>）", false);
            return;
        }
        let fill = import_from_path(path);
        let what = if fill.kind == ItemKind::Folder {
            "拖入的文件夹"
        } else {
            "拖入的文件"
        };
        self.fill_dialog(ctx, fill, what);
    }

    /// 批次 D4（FR-CFG-06）：「选文件…」/「选目录…」。用户取消不算失败，只落一行日志。
    fn dialog_pick(&mut self, ctx: &egui::Context, want_dir: bool) {
        let what = if want_dir { "文件夹" } else { "文件" };
        let picked = if want_dir { pick_folder() } else { pick_file() };
        match picked {
            Ok(Some(path)) => {
                let fill = source::import::fill_from_pick(&path, want_dir);
                let origin = if want_dir {
                    "所选文件夹"
                } else {
                    "所选文件"
                };
                self.fill_dialog(ctx, fill, origin);
            }
            Ok(None) => {
                eprintln!("弹窗：{what}选择已取消");
                ctx.request_repaint();
            }
            Err(e) => self.internal_toast(ctx, &format!("⚠ {e}"), false),
        }
    }

    /// 批次 D4（FR-CFG-06）「测试执行」：照当前表单启动一次，看能不能跑起来。
    ///
    /// 与列表里按 Enter 的区别：**不收窗、不记学习统计、不动使用统计**。用户还在编辑，
    /// 收窗等于把没保存的内容丢了；统计只认真正入库的条目。
    fn test_dialog_entry(&mut self, ctx: &egui::Context) {
        // 先把要用的值抠出来：下面要借 `&mut self` 弹 Toast，不能同时握着 `self.dialog` 的借用。
        let Some((kind, path, args, wd, name, ty)) = self.dialog.as_ref().map(|dg| {
            let wd = dg.working_dir.trim();
            (
                dg.kind,
                dg.path.trim().to_string(),
                dg.arguments.clone(),
                (!wd.is_empty()).then(|| wd.to_string()),
                dg.name.clone(),
                dg.param_type,
            )
        }) else {
            return;
        };
        // 内置动作不给测：`internal:<id>` 走 ShellExecute 只会失败，真跑起来则是直接执行危险动作。
        if kind == ItemKind::Internal {
            self.internal_toast(ctx, "⚠ 系统命令不提供测试执行（会直接触发内置动作）", false);
            return;
        }
        if path.is_empty() {
            self.internal_toast(ctx, "⚠ 命令行为空，没东西可测", false);
            return;
        }
        // 批次 B7（FR-ACT-08）：测试执行与真启动同口径——标志先剥、窗口状态照样生效。
        // 顺序必须在标记展开之前：编码非空却没有标记时，参数是拼在"命令尾"的，
        // 留着 `@+` 的话会拼到标志后面去，变成 `notepad.exe@+ 值`。
        let (path, show_flag) = if kind == ItemKind::Url {
            (path, source::runflags::ShowFlag::Normal)
        } else {
            let (stripped, flag) = source::runflags::split(&path);
            (stripped.to_string(), flag)
        };
        if path.is_empty() {
            self.internal_toast(ctx, "⚠ 运行标志后面没有命令，没东西可测", false);
            return;
        }
        // 批次 B6（FR-ACT-07）：带参项按"呼出那一刻的现场"展开标记再试跑。
        // 弹窗里没法再套一层参数框 ⇒ `{%p}` 按空串代入；这比把字面量 `{%p}` 交给程序强，
        // 后者只会换回一个"找不到文件"。日志本来就只记名称（"名称/路径/参数不落日志"），
        // 展开后的真实值（剪贴板 / 窗口标题）不会进 launcher.log。
        let asked = source::params::needs_prompt(&path, &args, ty);
        let (path, args) = if ty == source::params::ParamType::None {
            (path, args)
        } else {
            source::params::expand_command(&path, &args, ty, &self.summon, "")
        };
        let name = if name.trim().is_empty() { path.clone() } else { name };
        match shell_execute_show(&path, &args, wd.as_deref(), nshow(show_flag)) {
            Ok(_) => {
                eprintln!("弹窗测试执行：已启动（名称/路径/参数不落日志）");
                let note = if asked { "（标记已按当前现场展开，{%p} 为空）" } else { "" };
                self.internal_toast(ctx, &format!("✓ 已测试启动「{name}」{note}"), true);
            }
            Err(e) => {
                eprintln!("弹窗测试执行失败：{e}");
                self.internal_toast(ctx, &format!("⚠ 测试启动失败：{e}"), false);
            }
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
        // 批次 B7（FR-ACT-08）：`@+` / `@-` / `@` 改的是"这条命令拉起来的那个窗口"。
        // URL 档不认：浏览器自己开窗，剥掉尾巴上的 `@` 只会把网址啃坏。
        let (file, flag) = match entry.kind {
            ItemKind::Url => (entry.path.as_str(), source::runflags::ShowFlag::Normal),
            _ => source::runflags::split(&entry.path),
        };
        // 整条命令只写了一个 `@` ⇒ 剥完没东西可启动。不交给 ShellExecute 换一个
        // "找不到文件"的系统错误，直接说清楚是标志后面缺命令。
        if file.is_empty() {
            eprintln!(
                "launch: 运行标志后面没有命令，未启动（{}）",
                entry.kind.label()
            );
            self.internal_toast(ctx, "⚠ 运行标志后面没有命令", false);
            return false;
        }
        match shell_execute_show(
            file,
            &entry.arguments,
            entry.working_dir.as_deref(),
            nshow(flag),
        ) {
            Ok(_) => match masked_uid {
                None => eprintln!("launch: {} {}", file, entry.arguments),
                Some(uid) => eprintln!(
                    "launch(custom): 已启动「{}」（{uid}，填入的路径/参数含用户内容，不落日志）",
                    entry.name
                ),
            },
            Err(e) => {
                // M1 验收：启动失败不崩溃、不隐藏——窗口留在原地显示 Toast，TOAST_TTL 后自动消失。
                match masked_uid {
                    None => eprintln!("launch failed: {}: {e}", file),
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
            let jobs = icon_jobs(&self.items);
            self.spawn_missing_icons(jobs);
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

    /// 批次 P1（FR-ACT-03）：`→` / 右键「操作…」进入操作面板。
    /// 返回 `false` = 这一行没有可处置的对象（计算器 / 终端 / 各类 `valid=false` 提示行）⇒ 现场一字不动。
    ///
    /// 过滤词就住在 `query` 里，所以"进入"＝存现场 + 清空。**清空必须走 [`Self::set_query`]**：
    /// 它同时把光标放到末尾并要回焦点——少了这两样，进面板打的第一个字会插到词中间或落空
    /// （批次 F 补全后光标插错位是同一个坑）。
    fn open_panel(&mut self, ctx: &egui::Context, row: Row) -> bool {
        let plan = self.plan();
        let Some((target, open)) =
            panel_for_row(row, &plan.head, &plan.results, &self.items, &self.path_states)
        else {
            eprintln!("操作面板：这一行没有可处置的对象（终端 / 提示 / 分类行），不进面板");
            return false;
        };
        // 「恢复选中行」按**这一行本身**记，不按此刻的选中记：键盘（`→`）进来时两者相等——
        // 行本来就是从 `self.selected` 取的；右键进来时用户点的是哪一行，退出就该落回哪一行。
        let merged = match row {
            Row::Head(h) => h,
            Row::Item(rank) => plan.head.len() + rank,
        };
        let panel_actions = actions::actions_for(&target);
        // FR-SYS-03：只有清单里真实存在的条目才报名字——文件行 / 搜索行的标题是从查询现出来的。
        if target.editable {
            eprintln!(
                "操作面板：进入（{:?}，关于「{}」，动作 {} 条）",
                target.subject,
                target.title,
                panel_actions.len()
            );
        } else {
            eprintln!(
                "操作面板：进入（{:?}，动作 {} 条；对象来自查询 ⇒ 标题不落日志）",
                target.subject,
                panel_actions.len()
            );
        }
        self.panel = Some(PanelState {
            saved_query: self.query.clone(),
            saved_selected: merged,
            target,
            actions: panel_actions,
            open,
        });
        self.set_query(ctx, String::new(), "操作面板：查询框改作过滤词（原查询已存，原文不落日志）");
        true
    }

    /// 批次 E4（FR-ACT-05）：`→` 的**批量入口**——有标记时开的是这一张表，与当前选中哪一行无关
    /// （AHK `_OpenActions()` 同口径：`Marked.Count` 非零就对自己的集合动手）。
    ///
    /// 现场处理与单条目面板一致：存查询原文、存选中行、清空查询当过滤词。
    /// 「恢复选中行」按此刻的选中记——批量面板不对应某一行，退出时落在原处最不意外。
    fn open_batch_panel(&mut self, ctx: &egui::Context) -> bool {
        if self.marked.is_empty() {
            return false;
        }
        let paths = self.marked.clone();
        let target = actions::PanelTarget {
            title: format!("已标记的 {} 项", paths.len()),
            subject: actions::Subject::Batch,
            path: String::new(),
            arguments: String::new(),
            // 批量对象不靠「存在」开关任何一格：批量的七条都对路径本身动手，
            // 中途删掉的文件由各条动作自己报失败并计数。
            exists: true,
            editable: false,
            uid: None,
            pinned: false,
            batch: paths,
        };
        let panel_actions = actions::actions_for(&target);
        eprintln!(
            "批量面板：进入（已标记 {} 项，动作 {} 条；路径不落日志）",
            target.batch.len(),
            panel_actions.len()
        );
        self.panel = Some(PanelState {
            saved_query: self.query.clone(),
            saved_selected: self.selected,
            target,
            actions: panel_actions,
            open: PanelOpen::Batch,
        });
        self.set_query(
            ctx,
            String::new(),
            "批量面板：查询框改作过滤词（原查询已存，原文不落日志）",
        );
        true
    }

    /// 批次 P1（FR-ACT-03）：退出面板回结果模式（`Esc` / `←` / 空输入 `Backspace` / 执行完一条换屏动作）。
    /// 恢复**原输入文字与选中行**是写进验收的两句，顺序有讲究：`set_query` 内部带着
    /// `selected = 0`，所以放回行号必须排在它**之后**。
    fn close_panel(&mut self, ctx: &egui::Context, by: &str, focus_search: bool) {
        let Some(p) = self.panel.take() else { return };
        let chars = p.saved_query.chars().count();
        let row = p.saved_selected + 1;
        let saved = p.saved_query;
        self.set_query(
            ctx,
            saved,
            &format!(
                "操作面板：由{by}退出，原查询（{chars} 个字符，不落原文）与第 {row} 行已恢复"
            ),
        );
        self.selected = p.saved_selected;
        if !focus_search {
            // 现场交给了自己的弹窗（编辑 / 删除确认）⇒ 焦点不许被抢回搜索框。
            self.focus_query = false;
        }
        ctx.request_repaint();
    }

    /// 批次 E3-丙：「所在位置」做成面板版与快捷键版共用的一条。`via` 是日志前缀——
    /// 面板那一路传「操作面板」，日志文本与批次 P1 逐字一致（真机判据直接 grep 那个前缀）。
    fn do_reveal(
        &mut self,
        ctx: &egui::Context,
        via: &str,
        target: &actions::PanelTarget,
        source_path: &str,
    ) -> PanelOutcome {
        match reveal_target(&target.path, source_path) {
            Some(p) => {
                eprintln!(
                    "{via}：所在位置（{} 个字符的路径不落日志）→ 关闭到托盘",
                    p.chars().count()
                );
                self.select_in_explorer(ctx, &p);
                PanelOutcome::Gone
            }
            None => {
                eprintln!("{via}：所在位置落空（{:?}：没有可定位的本地路径）", target.subject);
                self.toast = Some((
                    "⚠ 这一条没有能在资源管理器里定位的本地路径".to_string(),
                    Instant::now() + TOAST_TTL,
                    TOAST_ERR,
                ));
                ctx.request_repaint_after(TOAST_TTL);
                PanelOutcome::Stay
            }
        }
    }

    /// 批次 E3-丙：剪贴板三格共用的一条（面板的复制路径 / 复制网址 / 复制名称）。
    /// 内容多半是从查询现出来的 ⇒ 日志只记长度与是哪一格（FR-SYS-03）。
    /// 批次 E5 起是四格：多一条「复制这一串」（计算器行的值）。
    fn do_copy(
        &mut self,
        ctx: &egui::Context,
        via: &str,
        target: &actions::PanelTarget,
        id: actions::ActionId,
    ) -> PanelOutcome {
        let text = match id {
            actions::ActionId::CopyName => target.name_to_copy(),
            actions::ActionId::CopyUrl => target.path.clone(),
            // 批次 E5：文本对象的 `path` 里就是那一串值，与 Enter 复制的同一份。
            actions::ActionId::CopyText => target.path.clone(),
            _ => target.path_to_copy(),
        };
        let verb = match id {
            actions::ActionId::CopyName => "名称",
            actions::ActionId::CopyUrl => "网址",
            actions::ActionId::CopyText => "这一串",
            _ => "路径",
        };
        match copy_to_clipboard(&text) {
            Ok(()) => {
                eprintln!(
                    "{via}：已复制{verb}（{} 个字符，内容不落日志）",
                    text.chars().count()
                );
                self.toast = Some((
                    format!("✓ 已复制{verb}：{text}"),
                    Instant::now() + TOAST_TTL,
                    TOAST_OK,
                ));
            }
            Err(e) => {
                eprintln!("{via}：复制{verb}失败：{e}");
                self.toast = Some((
                    format!("⚠ 复制失败：{e}"),
                    Instant::now() + TOAST_TTL,
                    TOAST_ERR,
                ));
            }
        }
        ctx.request_repaint_after(TOAST_TTL);
        PanelOutcome::Stay
    }

    /// 批次 E5（FR-ACT-01 第三格）：「粘贴到原窗口」做成面板版与快捷键版共用的一条。
    /// `via` 是日志前缀（与 [`Self::do_reveal`] 同一手法，真机判据直接 grep 前缀分辨哪一路）。
    ///
    /// 成功 ⇒ `Gone`：用户的下一步是在**那个窗口**里接着干活，把我们留在原地只会让他再多按一次
    /// `Esc`；所以先注入、再收窗，回执用 HUD 送到窗口外面（FR-UIX-09 那条口径）。
    /// 失败 ⇒ `Stay` 且一个字都没投：[`paste_text_into_previous_window`] 的三道闸任何一道没过
    /// 都在"碰键盘之前"，Toast 报的是哪一道挡的——不猜用户想贴哪儿，也不贴到别处去。
    fn do_paste(
        &mut self,
        ctx: &egui::Context,
        via: &str,
        target: &actions::PanelTarget,
    ) -> PanelOutcome {
        let chars = target.path.chars().count();
        match paste_text_into_previous_window(&target.path) {
            Ok(where_) => {
                eprintln!("{via}：已把 {chars} 个字符投给 {where_}（内容不落日志）");
                self.hud.notify(
                    ctx,
                    format!("✓ 已粘贴 {chars} 个字符到原来的窗口"),
                    &self.theme,
                    hud::HOLD_FOREGROUND,
                    work_for(self.prefs.appearance.show_on),
                );
                self.collapse_after_run(ctx);
                PanelOutcome::Gone
            }
            Err(e) => {
                eprintln!("{via}：粘贴未执行：{e}");
                self.toast = Some((format!("⚠ {e}"), Instant::now() + TOAST_TTL, TOAST_ERR));
                ctx.request_repaint_after(TOAST_TTL);
                PanelOutcome::Stay
            }
        }
    }

    /// 批次 E3-丙（FR-ACT-01「`Ctrl+Enter` 走次级动作」/ 开发计划 B3）：那颗可配的次级动作键。
    ///
    /// 两种形态由 `prefs.secondary_action` 决定，作用对象都是"当前选中行"：
    /// - [`SecondaryAction::Panel`] ＝ `→` 那个入口（同一个 [`Self::open_panel`]，同一套现场保存）；
    /// - [`SecondaryAction::Shortcut`] 执行 [`actions::secondary_action_for`] 给的那**一条**动作
    ///   （文件行 / 目录行 → 所在位置，网页搜索行 → 复制网址，计算器行 → 粘贴到原窗口）。
    ///
    /// 快捷键版刻意不包含"打开"：Enter 本来就是打开，再给它配一个别名只会让两个键争同一件事。
    /// 不在那三格里的对象（应用条目、系统命令、自定义命令…）不猜也不静默——明说"按 → 看全部动作"。
    fn run_secondary_action(&mut self, ctx: &egui::Context, row: Row) {
        let key = self.prefs.secondary_hotkey.display();
        let plan = self.plan();
        let Some((target, open)) =
            panel_for_row(row, &plan.head, &plan.results, &self.items, &self.path_states)
        else {
            eprintln!("次级动作 {key}：这一行没有可处置的对象（终端 / 提示 / 分类行）");
            self.toast = Some((
                "这一行没有可处置的对象".to_string(),
                Instant::now() + TOAST_TTL,
                TOAST_OK,
            ));
            ctx.request_repaint_after(TOAST_TTL);
            return;
        };
        if self.prefs.secondary_action == SecondaryAction::Panel {
            eprintln!("次级动作 {key}：面板版 ⇒ 进操作面板（{:?}）", target.subject);
            self.open_panel(ctx, row);
            return;
        }
        let Some(id) = actions::secondary_action_for(target.subject) else {
            eprintln!(
                "次级动作 {key}：快捷键版对这一行没有默认动作（{:?}）⇒ 提示按 → 看全部",
                target.subject
            );
            self.toast = Some((
                format!("这一行没有 {key} 的默认动作，按 → 打开操作面板"),
                Instant::now() + TOAST_TTL,
                TOAST_OK,
            ));
            ctx.request_repaint_after(TOAST_TTL);
            return;
        };
        let source_path = match &open {
            PanelOpen::Entry { entry, .. } => entry.source_path.as_str(),
            // 这条路径只从结果模式进来（`self.panel.is_none()` 才取键），批量面板不在其中。
            // 批次 E5：`Value` 同样没有"源路径"——计算器行不在清单里，它只是一个算出来的串。
            PanelOpen::Path | PanelOpen::Batch | PanelOpen::Value => "",
        };
        eprintln!(
            "次级动作 {key}：快捷键版 ⇒ {}（{:?}）",
            id.label(),
            target.subject
        );
        match id {
            actions::ActionId::Reveal => {
                self.do_reveal(ctx, "次级动作", &target, source_path);
            }
            actions::ActionId::CopyUrl => {
                self.do_copy(ctx, "次级动作", &target, id);
            }
            // 批次 E5：第三格「文本 ⇒ 粘贴」。这条是唯一会**动到别人窗口**的，
            // 所以 `do_paste` 里那三道闸一个字都不能省；失败它自己弹 Toast、窗口留在原地。
            actions::ActionId::PasteText => {
                self.do_paste(ctx, "次级动作", &target);
            }
            actions::ActionId::CopyText => {
                self.do_copy(ctx, "次级动作", &target, id);
            }
            other => {
                // 动作表以后扩了映射项、而这里还没接它的执行路径 ⇒ 退回面板，别静默什么都不做。
                eprintln!(
                    "次级动作 {key}：{} 尚未接入快捷键版，退回操作面板",
                    other.label()
                );
                self.open_panel(ctx, row);
            }
        }
    }

    /// 批次 P1（FR-ACT-03 / FR-ACT-04）：执行面板里选中的那条动作。
    ///
    /// **学习统计以原查询记**，这是 FR-ACT-03 定这条流程的理由：用户打的是 `chr`，学会的就该是
    /// `chr`→Chrome；把动作名「打开」喂进别名表，下次打 `open` 就能唤出 Chrome。所以 `alias_word`
    /// 取 `saved_query`，而且只有真的启动了程序的两档（打开 / 提权）才记。
    ///
    /// 每条动作的现场处置见 [`PanelOutcome`]：复制 / 属性 / 打开方式 / 回收站 / 大字**不关窗**
    /// （用户下一步还在面板里，或要去别处粘贴）；打开 / 提权 / 终端 / 所在位置交给系统后收进托盘。
    fn run_panel_action(&mut self, ctx: &egui::Context, id: actions::ActionId) {
        let Some(state) = self.panel.take() else { return };
        let alias_word = commit_word(&state.saved_query, false);
        let PanelState {
            saved_query,
            saved_selected,
            target,
            actions: panel_actions,
            open,
        } = state;
        let label = id.label();
        let outcome: PanelOutcome = match id {
            // ---- 三类"启动" ----
            actions::ActionId::Open => match &open {
                PanelOpen::Entry {
                    entry,
                    uid,
                    masked,
                } => {
                    // 批次 B6（FR-ACT-07）：要参数的那一档在这里也先弹框。面板已经出栈，
                    // 取消之后回到搜索结果页（面板存的原查询不再放回），这与"面板里启动失败"
                    // 那一格刻意不同：那里是没得选，这里是用户自己撤的。
                    // `from_list = true`：面板里这条的非参数路径成功就 `mark_used`（见下方分支），
                    // 填完参数之后同样要记——不然同一条命令"要不要填参数"决定它算不算用过。
                    if self.param_prompt_if_needed(entry, uid, true, alias_word.clone(), false) {
                        PanelOutcome::Stay
                    } else {
                        let (entry, expanded) = self.expand_for_launch(entry, "");
                        let started = if *masked || expanded {
                            self.launch_and_hide_masked(ctx, &entry, uid)
                        } else {
                            self.launch_and_hide(ctx, &entry)
                        };
                        if started {
                            self.mark_used(uid, &alias_word);
                            PanelOutcome::Gone
                        } else {
                            // 启动失败：`launch_with_log` 已经把窗口留在原地给了原因。
                            // 面板不撤——撤了查询框里剩的就是过滤词，那一屏结果会被过滤词误导。
                            PanelOutcome::Stay
                        }
                    }
                }
                PanelOpen::Path => {
                    // 文件行 / 搜索行：串从查询现出 ⇒ 与 `open_file_hit` 同一口径，路径不落日志。
                    match shell_execute(&target.path, &target.arguments, None) {
                        Ok(()) => {
                            eprintln!(
                                "操作面板：打开（{:?}，路径不落日志）→ 关闭到托盘",
                                target.subject
                            );
                            self.collapse_after_run(ctx);
                            PanelOutcome::Gone
                        }
                        Err(e) => {
                            eprintln!("操作面板：打开失败（{:?}）：{e}", target.subject);
                            self.toast = Some((
                                format!("⚠ 打开失败：{e}"),
                                Instant::now() + TOAST_TTL,
                                TOAST_ERR,
                            ));
                            ctx.request_repaint_after(TOAST_TTL);
                            PanelOutcome::Stay
                        }
                    }
                }
                // 批量面板的表里没有「打开」这一格（第一格是 `OpenAll`）；
                // 真走到这里说明动作表与这里的分派脱节了，别静默丢掉这一按。
                PanelOpen::Batch => {
                    eprintln!("操作面板：批量面板里没有单体的「打开」，这一按未处理");
                    PanelOutcome::Stay
                }
                // 批次 E5：文本对象**刻意没有**「打开」——把 `14` 交给 `ShellExecuteExW`
                // 是去搜一个叫 14 的程序。这里拒绝而不是回落 `Path` 那一条：面板的表已经不给
                // 这一格了，走到这里只可能是两边分派脱节，宁可留人原地问一句。
                PanelOpen::Value => {
                    eprintln!("操作面板：文本对象没有「打开」这一说（那一串不是路径也不是协议）");
                    self.toast = Some((
                        "这一行是一串文本，没有能打开的东西".to_string(),
                        Instant::now() + TOAST_TTL,
                        TOAST_OK,
                    ));
                    ctx.request_repaint_after(TOAST_TTL);
                    PanelOutcome::Stay
                }
            },
            actions::ActionId::RunAs => 'run_as: {
                // 批次 B6（FR-ACT-07）：提权这一格先没法问参数，也绝不能把字面量 `{%p}`
                // 带着 UAC 送给系统 ⇒ 明说，让用户改走「打开」（那条会弹参数框）。
                if let PanelOpen::Entry { entry, .. } = &open {
                    if source::params::needs_prompt(
                        &entry.path,
                        &entry.arguments,
                        entry.param_type,
                    ) {
                        eprintln!("操作面板：这条要先填参数，提权版不支持 ⇒ 请改用「打开」");
                        self.toast = Some((
                            "这条带参数标记：请用「打开」填参数后再运行".to_string(),
                            Instant::now() + TOAST_TTL,
                            TOAST_ERR,
                        ));
                        ctx.request_repaint_after(TOAST_TTL);
                        break 'run_as PanelOutcome::Stay;
                    }
                }
                let dir = match &open {
                    PanelOpen::Entry { entry, .. } => entry.working_dir.as_deref(),
                    PanelOpen::Path | PanelOpen::Batch | PanelOpen::Value => None,
                };
                match shell_execute_runas(&target.path, &target.arguments, dir) {
                    Ok(()) => {
                        if let PanelOpen::Entry { uid, .. } = &open {
                            self.mark_used(uid, &alias_word);
                        }
                        eprintln!(
                            "操作面板：以管理员运行（{:?}）已交给 UAC → 关闭到托盘",
                            target.subject
                        );
                        self.collapse_after_run(ctx);
                        PanelOutcome::Gone
                    }
                    Err(e) if user_cancelled(&e) => {
                        eprintln!("操作面板：提权被用户取消（1223），未运行");
                        self.toast = Some((
                            "已取消提权，未运行".to_string(),
                            Instant::now() + TOAST_TTL,
                            TOAST_OK,
                        ));
                        ctx.request_repaint_after(TOAST_TTL);
                        PanelOutcome::Stay
                    }
                    Err(e) => {
                        eprintln!("操作面板：以管理员运行失败（{:?}）：{e}", target.subject);
                        self.toast = Some((
                            format!("⚠ 提权启动失败：{e}"),
                            Instant::now() + TOAST_TTL,
                            TOAST_ERR,
                        ));
                        ctx.request_repaint_after(TOAST_TTL);
                        PanelOutcome::Stay
                    }
                }
            }
            actions::ActionId::OpenTerminal => {
                // 目录进本目录、文件进所在目录（AHK `_ParentDir` 同口径）。用 `spawn_at` 而不是
                // 拼一条 `cd /d` 命令：那玩意在 pwsh 里写法不同，四档 shell 就要分四支。
                let dir = if matches!(target.subject, actions::Subject::Folder) {
                    target.path.clone()
                } else {
                    actions::parent_dir(&target.path)
                };
                let shell = self.prefs.term_shell();
                match shell.spawn_at(&dir) {
                    Ok(()) => {
                        eprintln!(
                            "操作面板：在终端打开（shell {}，目录不落日志）→ 关闭到托盘",
                            shell.as_str()
                        );
                        self.collapse_after_run(ctx);
                        PanelOutcome::Gone
                    }
                    Err(e) => {
                        eprintln!("操作面板：终端启动失败（shell {}）：{e}", shell.as_str());
                        self.toast = Some((
                            format!("⚠ 启动失败：{e}"),
                            Instant::now() + TOAST_TTL,
                            TOAST_ERR,
                        ));
                        ctx.request_repaint_after(TOAST_TTL);
                        PanelOutcome::Stay
                    }
                }
            }
            // ---- 交给系统的对话框 ----
            actions::ActionId::OpenWith => match open_with_dialog(&target.path) {
                Ok(()) => {
                    eprintln!("操作面板：已交给系统「打开方式」（对象来自查询 ⇒ 路径不落日志）");
                    PanelOutcome::Stay
                }
                Err(e) => {
                    eprintln!("操作面板：「打开方式」调用失败：{e}");
                    self.toast = Some((
                        format!("⚠ 打开方式失败：{e}"),
                        Instant::now() + TOAST_TTL,
                        TOAST_ERR,
                    ));
                    ctx.request_repaint_after(TOAST_TTL);
                    PanelOutcome::Stay
                }
            },
            actions::ActionId::Properties => {
                if show_properties(&target.path) {
                    eprintln!("操作面板：属性框已交给系统（{:?}，路径不落日志）", target.subject);
                } else {
                    // `SHObjectProperties` 与 `properties` 动词两道都落空——网络盘上的某些形态会这样。
                    eprintln!("操作面板：属性框起不来（{:?}）", target.subject);
                    self.toast = Some((
                        "⚠ 这个对象没有属性框".to_string(),
                        Instant::now() + TOAST_TTL,
                        TOAST_ERR,
                    ));
                    ctx.request_repaint_after(TOAST_TTL);
                }
                PanelOutcome::Stay
            }
            actions::ActionId::Reveal => {
                let source_path = match &open {
                    PanelOpen::Entry { entry, .. } => entry.source_path.as_str(),
                    PanelOpen::Path | PanelOpen::Batch | PanelOpen::Value => "",
                };
                self.do_reveal(ctx, "操作面板", &target, source_path)
            }
            // ---- 剪贴板三格：都不关窗（下一步是去别处粘贴，不是收工）。批次 E5 起多两格 ----
            actions::ActionId::CopyPath
            | actions::ActionId::CopyUrl
            | actions::ActionId::CopyName => self.do_copy(ctx, "操作面板", &target, id),
            // 批次 E5：文本对象的「复制这一串」走同一条（只是取的是值不是路径）。
            actions::ActionId::CopyText => self.do_copy(ctx, "操作面板", &target, id),
            // 批次 E5：「粘贴到原窗口」是这张表里**唯一会把窗口收进托盘**的剪贴类动作——
            // 焦点已经交还给用户原来那个窗口，把我们留在原地没有意义。
            actions::ActionId::PasteText => self.do_paste(ctx, "操作面板", &target),
            actions::ActionId::Recycle => {
                // `recycle_path` 给的是裸 Win32 码（`SHFileOperationW` 不按 HRESULT 说话）：
                // 1223 = 用户在那句「永久删除？」警告里点了"否"，其余非零才是真失败。
                match recycle_path(&target.path) {
                    Ok(()) => {
                        self.refresh_path_states(true);
                        eprintln!(
                            "操作面板：已移入回收站（{} 个字符的路径不落日志）",
                            target.path.chars().count()
                        );
                        self.toast = Some((
                            "✓ 已移入回收站（可从回收站还原）".to_string(),
                            Instant::now() + TOAST_TTL,
                            TOAST_OK,
                        ));
                        ctx.request_repaint_after(TOAST_TTL);
                        // 对象已经不在了 ⇒ 面板里没有第二条动作还值得对它做，退回结果模式。
                        // 清单条目**不跟着删**（进回收站 ≠ 从清单消失），路径四态会把它标成失效。
                        PanelOutcome::Close
                    }
                    Err(1223) => {
                        eprintln!("操作面板：那句「永久删除」警告被用户驳回，未删除");
                        self.toast = Some((
                            "已取消，未删除".to_string(),
                            Instant::now() + TOAST_TTL,
                            TOAST_OK,
                        ));
                        ctx.request_repaint_after(TOAST_TTL);
                        PanelOutcome::Stay
                    }
                    Err(c) => {
                        eprintln!("操作面板：移入回收站失败（系统返回 {c}）");
                        self.toast = Some((
                            format!("⚠ 移入回收站失败（系统返回 {c}）"),
                            Instant::now() + TOAST_TTL,
                            TOAST_ERR,
                        ));
                        ctx.request_repaint_after(TOAST_TTL);
                        PanelOutcome::Stay
                    }
                }
            }
            // ---- 交给我们自己的弹窗与确认框 ----
            actions::ActionId::Edit => {
                if let Some(uid) = &target.uid {
                    if let Some(it) = self.items.iter().find(|i| i.uid() == *uid).cloned() {
                        self.dialog = Some(DialogState::from_item(&it));
                        // 打开面板这一下是 Enter 按的：同一帧不能让刚出生的对话框把它当成「确定」。
                        self.dialog_enter_armed = true;
                    }
                }
                PanelOutcome::Close
            }
            actions::ActionId::AddCommand => {
                self.dialog = Some(DialogState::from_input(&target.path));
                self.dialog_enter_armed = true;
                PanelOutcome::Close
            }
            actions::ActionId::Delete => {
                if let Some(uid) = &target.uid {
                    if let Some(it) = self.items.iter().find(|i| i.uid() == *uid).cloned() {
                        self.confirm = Some(ConfirmRequest {
                            uid: it.uid(),
                            name: it.name.clone(),
                            path: it.path.clone(),
                        });
                        eprintln!("操作面板：删除 {uid} 前先弹确认（未删除）");
                    }
                }
                PanelOutcome::Close
            }
            actions::ActionId::TogglePin => {
                // 只改这一份快照里的 `pinned`，让面板原地换行文案（「置顶」⇄「取消置顶」）。
                // 不能搬走 `target`：`PanelOutcome::Stay`/`Close` 那条路还要用它把现场放回。
                let mut new_target = target.clone();
                if let Some(uid) = &new_target.uid {
                    if let Some(pos) = self.items.iter().position(|i| i.uid() == *uid) {
                        self.items[pos].pinned = !self.items[pos].pinned;
                        let on = self.items[pos].pinned;
                        let name = self.items[pos].name.clone();
                        self.save_items();
                        new_target.pinned = on;
                        self.toast = Some((
                            format!("✓ 已{}", if on { "置顶" } else { "取消置顶" }),
                            Instant::now() + TOAST_TTL,
                            TOAST_OK,
                        ));
                        eprintln!("操作面板：置顶切换 {uid} → {on}（「{name}」）");
                    }
                }
                PanelOutcome::Reland(new_target)
            }
            // ---- 批次 E4（FR-ACT-05）：批量七条。只可能出现在 `Subject::Batch` 的面板里 ----
            actions::ActionId::OpenAll => {
                let total = target.batch.len();
                let mut failed = 0usize;
                for p in &target.batch {
                    if let Err(e) = shell_execute(p, "", None) {
                        failed += 1;
                        eprintln!("批量面板：全部打开失败一项（系统错误）：{e}");
                    }
                }
                if failed == total {
                    eprintln!("批量面板：全部打开 {total} 项全部失败，窗口留在原地");
                    self.toast = Some((
                        format!("⚠ {total} 项都没能打开"),
                        Instant::now() + TOAST_TTL,
                        TOAST_ERR,
                    ));
                    ctx.request_repaint_after(TOAST_TTL);
                    PanelOutcome::Stay
                } else {
                    eprintln!(
                        "批量面板：全部打开 {total} 项（失败 {failed} 项；路径不落日志）→ 关闭到托盘"
                    );
                    self.collapse_after_run(ctx);
                    PanelOutcome::Gone
                }
            }
            actions::ActionId::CopyFiles | actions::ActionId::CutFiles => {
                // 批次 E6（FR-ACT-04 单行形态）：这一对 `ActionId` 现在两张表都有。单体面板的
                // `batch` 恒空 ⇒ 取它自己那一条路径，于是两条通道写进剪贴板的载荷只差条数，
                // `CF_HDROP` / `Preferred DropEffect` 的构造完全同一份代码。
                let cut = id == actions::ActionId::CutFiles;
                let paths: Vec<String> = if target.batch.is_empty() {
                    vec![target.path.clone()]
                } else {
                    target.batch.clone()
                };
                let n = paths.len();
                let via = if target.batch.is_empty() {
                    "操作面板"
                } else {
                    "批量面板"
                };
                let verb = if cut { "剪切" } else { "复制" };
                match copy_files_to_clipboard(&paths, cut) {
                    Ok(()) => {
                        eprintln!("{via}：{n} 项已{verb}进剪贴板（路径不落日志）");
                        self.toast = Some((
                            format!("✓ 已{verb} {n} 项，去资源管理器粘贴"),
                            Instant::now() + TOAST_TTL,
                            TOAST_OK,
                        ));
                    }
                    Err(e) => {
                        eprintln!("{via}：{verb}文件失败：{e}");
                        self.toast = Some((
                            format!("⚠ {verb}文件失败：{e}"),
                            Instant::now() + TOAST_TTL,
                            TOAST_ERR,
                        ));
                    }
                }
                ctx.request_repaint_after(TOAST_TTL);
                PanelOutcome::Stay
            }
            actions::ActionId::CopyPaths => {
                let n = target.batch.len();
                // Windows 换行：贴进记事本/终端都是一行一条（AHK `CopyPaths` 同口径）。
                let joined = target.batch.join("\r\n");
                match copy_to_clipboard(&joined) {
                    Ok(()) => {
                        eprintln!("批量面板：已复制 {n} 条路径（内容不落日志）");
                        self.toast = Some((
                            format!("✓ 已复制 {n} 条路径（每行一条）"),
                            Instant::now() + TOAST_TTL,
                            TOAST_OK,
                        ));
                    }
                    Err(e) => {
                        eprintln!("批量面板：复制路径失败：{e}");
                        self.toast = Some((
                            format!("⚠ 复制失败：{e}"),
                            Instant::now() + TOAST_TTL,
                            TOAST_ERR,
                        ));
                    }
                }
                ctx.request_repaint_after(TOAST_TTL);
                PanelOutcome::Stay
            }
            actions::ActionId::CopyTo => {
                let n = target.batch.len();
                let outcome = match pick_folder() {
                    Ok(None) => {
                        eprintln!("批量面板：复制到 — 用户在文件夹选择框里取消，未复制");
                        ("已取消，未复制".to_string(), TOAST_OK)
                    }
                    Ok(Some(dest)) => match copy_many_to(&target.batch, &dest) {
                        Ok(0) => {
                            eprintln!("批量面板：复制到 — 全部来源都在目标目录里，没有可复制的项");
                            ("这些项都在目标文件夹里，无需复制".to_string(), TOAST_OK)
                        }
                        Ok(done) => {
                            eprintln!(
                                "批量面板：复制到完成 {done}/{n} 项（目标目录名不落日志）"
                            );
                            (
                                format!(
                                    "✓ 已复制 {done} 项到「{}」",
                                    actions::leaf_name(&dest)
                                ),
                                TOAST_OK,
                            )
                        }
                        Err(1223) => {
                            eprintln!("批量面板：复制到 — 用户在系统复制里取消");
                            ("已取消，未复制".to_string(), TOAST_OK)
                        }
                        Err(code) => {
                            eprintln!("批量面板：复制到失败（系统返回 {code}）");
                            (format!("⚠ 复制到失败（系统返回 {code}）"), TOAST_ERR)
                        }
                    },
                    Err(e) => {
                        eprintln!("批量面板：文件夹选择框失败：{e}");
                        (format!("⚠ {e}"), TOAST_ERR)
                    }
                };
                self.toast = Some((outcome.0, Instant::now() + TOAST_TTL, outcome.1));
                ctx.request_repaint_after(TOAST_TTL);
                PanelOutcome::Stay
            }
            actions::ActionId::AddAll => {
                let mut added: Vec<LaunchItem> = Vec::new();
                let mut skipped = 0usize;
                for p in &target.batch {
                    if self.items.iter().any(|it| same_target_path(&it.path, p)) {
                        skipped += 1;
                        continue;
                    }
                    added.push(command_item_from_path(p));
                }
                if added.is_empty() {
                    eprintln!("批量面板：全部加入命令 — {skipped} 项都已在清单里，未新增");
                    self.toast = Some((
                        "这些项都已在清单里（未新增）".to_string(),
                        Instant::now() + TOAST_TTL,
                        TOAST_OK,
                    ));
                } else {
                    let before = self.items.len();
                    let old = self.items.clone();
                    self.items =
                        dedup_items([std::mem::take(&mut self.items), added.clone()].concat());
                    self.carry_stats(&old);
                    let grew = self.items.len().saturating_sub(before);
                    let jobs = icon_jobs(&added);
                    self.spawn_missing_icons(jobs);
                    self.save_items();
                    eprintln!(
                        "批量面板：全部加入命令 — 新增 {grew} 条，跳过 {skipped} 条已存在（名称不落日志）"
                    );
                    let tail = if skipped > 0 {
                        format!("（跳过 {skipped} 条已存在）")
                    } else {
                        String::new()
                    };
                    self.toast = Some((
                        format!("✓ 已加入 {grew} 条命令{tail}"),
                        Instant::now() + TOAST_TTL,
                        TOAST_OK,
                    ));
                }
                ctx.request_repaint_after(TOAST_TTL);
                PanelOutcome::Stay
            }
            actions::ActionId::RecycleAll => {
                let paths = target.batch.clone();
                eprintln!(
                    "批量面板：全部回收站 — 先弹确认（{} 项，未删除）",
                    paths.len()
                );
                self.batch_confirm = Some(paths);
                PanelOutcome::Stay
            }
            actions::ActionId::LargeType => {
                // 复用 FR-UIX-10 的大字视口：铺的就是这一项的名字（`target.title`）。
                // 批次 E5 的文本对象例外——铺的是**那一串值**：计算器行的"这段文字本身"就是
                // 数字（与 [`large_text_of`] 里 `HeadRow::Calc(hit) => hit.copy` 同一条口径），
                // 标题那一串是 `= 1,024`，把等号铺满屏幕没有意义。
                let text = if target.subject == actions::Subject::Text {
                    target.path.clone()
                } else {
                    target.title.clone()
                };
                self.large_type.show(ctx, text);
                PanelOutcome::Stay
            }
        };
        // 现场怎么处置见 [`PanelOutcome`]。`Gone` 是唯一不回去的：那条路上窗口已经收到托盘，
        // 下次呼出必须从结果模式重新开始（`on_hide` 里也兜了一遍，两处口径一致）。
        match outcome {
            PanelOutcome::Stay => {
                self.panel = Some(PanelState {
                    saved_query,
                    saved_selected,
                    target,
                    actions: panel_actions,
                    open,
                });
            }
            PanelOutcome::Reland(new_target) => {
                let rebuilt = actions::actions_for(&new_target);
                self.panel = Some(PanelState {
                    saved_query,
                    saved_selected,
                    target: new_target,
                    actions: rebuilt,
                    open,
                });
                ctx.request_repaint();
            }
            PanelOutcome::Close => {
                self.panel = Some(PanelState {
                    saved_query,
                    saved_selected,
                    target,
                    actions: panel_actions,
                    open,
                });
                self.close_panel(ctx, label, self.dialog.is_none() && self.confirm.is_none());
            }
            // 批次 C3：`Gone` 是唯一"这条查询已经交付出去"的下场（打开 / 提权 / 终端 /
            // 所在位置 / 全部打开，窗口收到托盘）。历史取 `saved_query` 不取 `self.query`：
            // 此刻框里是**过滤词**，记进历史就会在下次 `Ctrl+↑` 时回填出一个没敲过的词。
            // `Stay` / `Close` 那两档不记：复制、属性、大字这些不关窗，用户还在这条查询里。
            PanelOutcome::Gone => self.remember_history(saved_query),
        }
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
        // 批次 D8：功能桶按条目的类型分——内置动作（`internal:`）自成一桶，
        // 其余（应用/命令/网址/文件夹/文件）都算「清单项」。
        let bucket = if self.items[pos].kind == ItemKind::Internal {
            source::usage::Bucket::Internal
        } else {
            source::usage::Bucket::Item
        };
        if !alias_word.is_empty() && learn_alias(&mut self.items[pos], alias_word) {
            // FR-SYS-03 隐私口径：别名字段本身就是"用户敲过的词的集合"——把 `keyword` 打进日志，
            // 等于把刚学到的那个词（以及历史上所有被学进去的查询原文）原样抄进一份能存好几天、
            // 还能托盘一键打开的文件里。所以只记"写过了"，字段内容一个字都不出。
            eprintln!(
                "提交即学别名：已写入「{}」的别名字段（学到的词与既有别名都不落日志）",
                self.items[pos].name
            );
        }
        self.save_items();
        // 批次 D8（FR-SYS-06）：同一次"用过了"顺手记进使用统计。口径与 `use_count` 一致
        // ——**真的启动了才走到这里**（四个调用点都在成功分支上）。
        self.record_usage(bucket);
    }

    /// 批次 D8（FR-SYS-06）：记一次使用统计。
    ///
    /// - 日期取**本地今天**（`today_iso`，来源就是日志那条日期锚），顺手按 400 天裁一次；
    /// - 记的只有"哪一类功能、哪天、几次"三样——桶名是固定常量、日期是日历，
    ///   **任何用户原文都不会进这个文件**（单测 `json_carries_only_dates_buckets_and_counts` 钉着）；
    /// - 日志也只报桶名与次数（FR-SYS-03 同一口径）。
    fn record_usage(&mut self, bucket: source::usage::Bucket) {
        let today = source::application::today_iso();
        let n = self.usage.record(&today, bucket);
        self.usage.trim(&today, source::usage::KEEP_DAYS);
        if self.usage_blocked {
            eprintln!(
                "使用统计：{} 今日第 {n} 次（usage.json 坏且留证没成功 ⇒ 本次不写它）",
                bucket.label()
            );
            return;
        }
        source::application::save_usage(&self.usage);
        eprintln!(
            "使用统计：+1 {}（今日 {n} 次，累计 {} 次）",
            bucket.label(),
            self.usage.total()
        );
    }

    /// 列表区此刻吃不吃键盘。三处共用同一个判据（列表按键与滚轮、搜索页顶栏的拖窗缝、
    /// 列表右侧那块拖窗手柄）：弹窗/参数框/确认框/关于页/帮助页任一开着 ⇒ 让位，
    /// Enter 归"确定"按钮、拖动手柄整块撤掉。
    /// 批次 C3 之前这三处各抄了一遍同样的六个条件，加一档确认框就得记着改三处——
    /// 漏一处的症状是"开着确认框还能用 ↑↓ 移动选中"。
    /// 批次 B6 补 `param_prompt`（2026-10-28 真机①）：漏了它时回车在参数框里按一次会
    /// "确认并启动"＋"列表再启动选中行"各跑一遍，后者又开出一个空参数框（幽灵框）。
    fn keys_live(&self) -> bool {
        self.dialog.is_none()
            && self.param_prompt.is_none()
            && self.confirm.is_none()
            && self.batch_confirm.is_none()
            && self.internal_confirm.is_none()
            && !self.about_open
            && self.help_entry.is_none()
    }

    /// 本查询在不在文件模式（批次 D7：最小字数可配了）。判据与 `FileSource::plan` 必须同一档：
    /// 两者都读 `settings.json` 那一格、都过 [`ScanScope::clamp_min_query_len`] 这道夹子——
    /// 以前 `route()` 全局一个缺省值，无所谓；现在界面这边的"在不在文件模式"若用缺省 2 字、
    /// 源那边用配置值，配成 1 或 3 时两边的答案就会分叉。
    fn file_route(&self, raw: &str) -> source::filesearch::Route {
        source::filesearch::route_at(
            raw,
            source::filesearch::ScanScope::clamp_min_query_len(self.prefs.file_min_query_len),
        )
    }

    /// FR-QRY-10（批次 C3）：一次成功执行 = 记一条搜索历史。
    ///
    /// 文本由调用方给，不读 `self.query`：操作面板那一路此刻框里是**过滤词**，用户真正的查询
    /// 存在 `PanelState::saved_query` 里（`mark_used` 的 `alias_word` 同源、同一口径）。
    /// 也不挂在 `mark_used` 上——那个函数第一件事就是"这条不在清单里就返回"，而历史上恰恰
    /// 最该留住那种条目：`>git status`、一个敲了半天的文件路径、`g:某词`，它们都不进清单。
    ///
    /// 只在表真的变了的时候写盘（`source::application::AppSettings::set_source` 那条
    /// "没变就别多一次写"的口径）。
    /// 日志只报条数、绝不报文本：这张表存的就是用户敲过的原文，FR-SYS-03 那条口径对它同样成立
    /// （`launcher.log` 能存好几天，托盘一键就能打开，抄进去等于把历史明文另存一份）。
    ///
    /// 形参收 `String` 不是 `&str`：调用点全在 `&mut self` 的界面帧里，喂 `&self.query`
    /// 会撞上"边改边借"（`remember_history` 要动 `self.history` 与 `self.prefs`）。
    fn remember_history(&mut self, text: String) {
        if !source::application::push_history(&mut self.history, &text, self.prefs.history_limit())
        {
            return;
        }
        let n = self.history.len();
        if self.history_blocked {
            eprintln!(
                "source: 搜索历史已在内存里更新（现 {n} 条），但留证没成功 ⇒ 本次启动不写 search_history.json"
            );
            return;
        }
        source::application::save_history(&self.history);
        eprintln!("source: 已写搜索历史 search_history.json（{n} 条，文本不落日志）");
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

    /// 批次 F4：把一批作业里「还没纹理、没在飞、也没失败过」的派给加载线程。
    /// 所有补图标的地方都走这一个口子，`pending` / `failed` 两张表才有一致的含义。
    fn spawn_missing_icons(&mut self, jobs: Vec<IconJob>) {
        let missing: Vec<IconJob> = jobs
            .into_iter()
            .filter(|(k, _)| {
                !self.icons.contains_key(k)
                    && !self.icon_pending.contains(k)
                    && !self.icon_failed.contains(k)
            })
            .collect();
        if missing.is_empty() {
            return;
        }
        for (k, _) in &missing {
            self.icon_pending.insert(k.clone());
        }
        spawn_icon_loader(self.icon_tx.clone(), missing);
    }

    /// 批次 F7：从「高级」页恢复一条隐藏条目之后的收尾——两份落盘、补取图标、刷新路径态。
    /// 恢复出来的条目来自隐藏快照（老记录没有快照时才按出厂种子重建），
    /// 图标纹理与路径四态都还没有缓存。
    fn after_unhide(&mut self, ctx: &egui::Context, uid: &str) {
        self.save_items();
        source::application::save_settings(&self.prefs);
        let jobs = self
            .items
            .iter()
            .find(|i| i.uid() == uid)
            .map(|i| icon_jobs(std::slice::from_ref(i)))
            .unwrap_or_default();
        self.spawn_missing_icons(jobs);
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
            // 同一条取向：兜底构造不碰盘上的设置文件，出厂默认 + "没有故障"就够测试用。
            source::application::AppSettings::default(),
            None,
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
        // 批次 P1（FR-ACT-03）：面板开着时整张列表就是那张动作表，此刻查询框里那几个字是**过滤词**。
        // 走头段通道（`SourceRow::Action`）而不是 `results`：动作行没有"名次"可言，而 ↑↓／PgUp／
        // Ctrl+J/K／Tab／数字键／点击／徽标／状态栏全都读同一份 `plan`，借道头段就等于一次到位。
        // 早退必须在 `cap_plan_rows` 之前——那一刀是给「10 条」档的结果列表用的，动作表只有十几行，
        // 被截掉后半张就成了「进面板看不见「删除」」。
        if let Some(p) = &self.panel {
            let hits = actions::filter_actions(&self.query, &p.actions);
            return Plan {
                head: hits
                    .iter()
                    .map(|&i| source::SourceRow::Action(p.actions[i].clone()))
                    .collect(),
                results: Vec::new(),
            };
        }
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
            web_fallback: &self.prefs.web_fallback,
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
            if self.prefs.source_on(s.id()) {
                // 批次 D7「保存即重扫」：设置里的扫描范围与源手里的不同 ⇒ 这里换给源
                //（`set_scope` 相等去重，同值零成本；文件源之外全部默认空操作）。
                s.apply_settings(&self.prefs);
                if s.poll() {
                    ui.ctx().request_repaint();
                }
            }
        }
        // E1：配置坏过一次的那句说明，等窗口真被看见才说（Toast 会被 `on_show` 清空，
        // 所以它必须在 `hotkey_frame` 之后、渲染之前入队）。
        if self.visible {
            if let Some(msg) = self.startup_notice.take() {
                self.toast = Some((msg, Instant::now() + TOAST_TTL_STARTUP, TOAST_ERR));
                ui.ctx().request_repaint_after(TOAST_TTL_STARTUP);
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
                || self.param_prompt.is_some()
                || self.confirm.is_some()
                || self.batch_confirm.is_some()
                || self.internal_confirm.is_some()
                || self.about_open
                || self.help_entry.is_some()
                || self.settings_open
                // FR-UIX-10：大字窗要抢焦点才收得到 Esc，主窗必然"失焦"。
                // 这一档不藏：主窗留着，关闭大字时才有一个可激活的窗口把焦点还回来
                // （真机 2026-10-24：藏掉之后 SetForegroundWindow 激活的是隐藏窗，
                // 焦点落到外部程序，后续按键全漏）。
                || self.large_type.showing()
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

        // 批次 F4：回执是「缓存键 + 可选像素」。取不到的键记进 failed，免得可见行每帧重开线程。
        while let Ok((key, hit)) = self.icon_rx.try_recv() {
            self.icon_pending.remove(&key);
            match hit {
                Some((rgba, w, h)) => {
                    let texture = ui.ctx().load_texture(
                        &key,
                        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba),
                        egui::TextureOptions::default(),
                    );
                    let entry = IconEntry {
                        texture,
                        last_use: std::cell::Cell::new(self.icon_tick.get()),
                    };
                    self.icons.insert(key, entry);
                }
                None => {
                    self.icon_failed.insert(key);
                }
            }
        }
        // 批次 F4：LRU 修剪（AHK `MaxIcons` 同口径：超 1500 剪到 1200）。
        // 被剪掉的键不进 failed，可见行下一帧会重新要一次。
        if self.icons.len() > source::icon::ICON_CACHE_MAX {
            let last_use: std::collections::HashMap<String, u64> = self
                .icons
                .iter()
                .map(|(k, v)| (k.clone(), v.last_use.get()))
                .collect();
            let doomed = source::icon::evict_keys(&last_use);
            if !doomed.is_empty() {
                for k in &doomed {
                    self.icons.remove(k);
                }
                eprintln!(
                    "icon cache: 超上限 {} 条，淘汰最久没用的 {} 条（剩 {}）",
                    last_use.len(),
                    doomed.len(),
                    self.icons.len()
                );
            }
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
            let jobs = icon_jobs(&self.items);
            self.spawn_missing_icons(jobs);
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
        // 批次 E3-丙的次级动作键框同理：它录的恰好也是窗口内热键，不挡就会"按什么键都先执行一次"。
        let recording =
            self.hotkey_recording || self.typeback_armed || self.secondary_recording || self.extra_recording;

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
        // 批次 P1（FR-ACT-03）：面板里被选中的动作（Enter / 点击 / 数字键三条入口汇成这一个槽）。
        // 和其它槽一样"当帧只记、帧末才做"：动作会弹系统对话框、会改清单、甚至会把窗口收进托盘，
        // 在渲染列表的借用里做这些事，下一行就要读到改过的 `self.panel`。
        let mut panel_action: Option<actions::ActionId> = None;
        // FR-ACT-03 的右键入口「操作…」。同样必须帧末才做：`open_panel` 要 `&mut self`，
        // 而右键闭包里正握着 `&self.items[..]` 借出来的那一行（2021 的按字段捕获让它过了编译，
        // 换成整块 `&mut self` 就冲突了）。
        let mut panel_request: Option<Row> = None;

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
        // 批次 P1（FR-ACT-03）：`→` 进操作面板（只认"新按"那一下 + 光标必须在末尾，所以按住不放
        // 不会连跳），`←` / `Esc` / 输入为空时的 `Backspace` 退出并把原查询与选中行放回去。
        // 判据与落点见下面的 `panel_exit` / `panel_enter`：这两个旗标是那条长派发链的第一档，
        // 排在 Ctrl+M / Ctrl+F / 数字键 / Tab 之前；但上面那批 `consume_key` 已经先一步把
        // Ctrl 组合的事件吃掉了——箭头键与它们不同键，所以先后顺序不影响结果。
        // Esc 那一档不在这里，在下方 Esc 阶梯里（面板先于"清空输入"和"关闭到托盘"）。
        // 鼠标中键不再执行任何条目（2026-10-18 用户裁定废弃），移动窗口改由「左键拖空白」承担，见下方同名注释块。
        // consume_key 精确匹配修饰键并吃掉事件，防与 TextEdit/其他控件的键位打架。
        if self.visible
            && !recording
            && self.dialog.is_none()
            && self.param_prompt.is_none()
            && self.confirm.is_none()
            && self.batch_confirm.is_none()
            && self.internal_confirm.is_none()
            && self.pending_destructive.is_none()
            && self.manage_confirm.is_none()
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
            // ---- 批次 B8（FR-ACT-10 附录 A）：F1 关于 / F2 设置 ----
            // 这两颗开的是"整窗口级"的界面（关于弹窗、设置页），搜索页与设置页都该有效，所以在
            // 下面那个 `!settings_open` 分岔**之前**取键。裸功能键走 `take_fresh_key`：只认不带修饰
            // 那一层（`Ctrl+F1`、`Alt+F1` 仍留给用户录），手法与 `→`/`←`/`Insert` 同一条口径。
            let hit_f1 = take_fresh_key(ui.ctx(), egui::Key::F1);
            let hit_f2 = take_fresh_key(ui.ctx(), egui::Key::F2);
            if hit_f1 {
                self.about_open = true;
                // 与 Ctrl+, 同一条理由：☰ 只在搜索页渲染，键盘打开别的界面时必须把旗标落下，
                // 否则回到搜索页那一帧会带着一个"没人负责关闭"的菜单。
                self.menu_open = false;
                eprintln!("快捷键 F1：打开「关于本程序」");
                ui.ctx().request_repaint();
            }
            let hit_comma =
                ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma));
            // 批次 D5（FR-ACT-09）：`Ctrl+B` 直达设置页「批量管理」。与 F2 同一层取键——设置页
            // 开着时它也生效（在「外观」页想看一眼失效项，不必先回搜索页）。
            let hit_ctrl_b =
                ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::B));
            // 批次 D8（FR-SYS-06）：`Ctrl+T` 直达设置页「使用统计」。与 `Ctrl+B` 同一层、
            // 同一口径（设置页开着时也生效）。
            let hit_ctrl_t =
                ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::T));
            // 批次 D2（FR-SRC-11）：`Ctrl+W` 直达设置页「网页引擎」——三颗"直达设置页"的键
            // （B/T/W）同一层取，设置页开着时也跳。
            let hit_ctrl_w =
                ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::W));
            if hit_comma || hit_f2 {
                self.settings_open = !self.settings_open;
                // 批次 D2：这一下是"关掉设置页"时也要回写编辑缓冲。
                if !self.settings_open {
                    self.finish_settings_edits();
                }
                self.focus_query = true;
                // 批次 F6：☰ 只在搜索页渲染 ⇒ 进设置页必须先把旗标落下，
                // 否则回到搜索页的第一帧会带着一个"没人负责关闭"的菜单。
                self.menu_open = false;
                eprintln!(
                    "快捷键 {}：设置页 {}",
                    if hit_f2 { "F2" } else { "Ctrl+," },
                    if self.settings_open {
                        "打开"
                    } else {
                        "返回搜索"
                    }
                );
                ui.ctx().request_repaint();
            } else if hit_ctrl_b {
                // 批次 D5：直达「批量管理」。不进 `!settings_open` 那一档：设置页开着时跳页也要灵。
                self.settings_open = true;
                self.settings_tab = SettingsTab::Manage;
                self.focus_query = true;
                self.menu_open = false;
                self.refresh_path_states(true);
                eprintln!("快捷键 Ctrl+B：设置页「批量管理」");
                ui.ctx().request_repaint();
            } else if hit_ctrl_t {
                // 批次 D8：直达「使用统计」。与 Ctrl+B 同形态——设置页开着时也跳。
                self.settings_open = true;
                self.settings_tab = SettingsTab::Usage;
                self.focus_query = true;
                self.menu_open = false;
                eprintln!("快捷键 Ctrl+T：设置页「使用统计」");
                ui.ctx().request_repaint();
            } else if hit_ctrl_w {
                // 批次 D2：直达「网页引擎」。与 Ctrl+B / Ctrl+T 同形态——设置页开着时也跳。
                self.settings_open = true;
                self.settings_tab = SettingsTab::WebEngines;
                self.focus_query = true;
                self.menu_open = false;
                eprintln!("快捷键 Ctrl+W：设置页「网页引擎」");
                ui.ctx().request_repaint();
            } else if !self.settings_open {
                // ---- 批次 B8（FR-ACT-10 附录 A）：F3 编辑 · F4 打开配置文件 · Ctrl+C 复制这一行 ----
                // `F3` 与 `Ctrl+C` 的对象都是"当前高亮的那一行"⇒ 面板开着时不接管（那一屏的"这一行"
                // 是一条动作，复制/编辑它反而说不清指谁；`Insert` 在批次 E4 同口径让位）。
                // `F4` 开的是配置目录，与选中行无关⇒ 面板里也照常有效，跟 F1/F2 一样不设这道闸。
                let hit_f3 = self.panel.is_none() && take_fresh_key(ui.ctx(), egui::Key::F3);
                let hit_f4 = take_fresh_key(ui.ctx(), egui::Key::F4);
                // `Ctrl+C` 先问输入框有没有活要干：焦点在查询框且选区非空 ⇒ 让给 egui 复制选中的文字
                //（判据与 TextEdit 读同一份 `TextEditState`，见 [`query_box_owns_copy`]）。
                // 不能用 `consume_key(COMMAND, C)`——那颗键在 egui-winit 就被折成 `Event::Copy` 了。
                let yield_copy = query_box_owns_copy(ui.ctx());
                let hit_copy = self.panel.is_none()
                    && ui.input_mut(|i| take_copy_press(&mut i.events, yield_copy));
                // ---- 批次 C3（FR-QRY-10）：`Ctrl+↑` 更早一条 / `Ctrl+↓` 更新一条 ----
                // 放在查询框渲染**之前**取键是有意的：`consume_key` 会把这一格事件删掉，
                // 后面的 TextEdit 就看不到它（否则单行框先把它当"光标到行首"动一遍，回填再盖回来）。
                // 与裸 `↑↓`（移动选中）不需要再写一条互斥判据：事件已被删，下面列表那一段的
                // `key_pressed(ArrowUp)` 同帧恒为 `false`——`num_presses` 读的就是同一份 `events`
                // （`input_state/mod.rs:691-711` / `:746-764`，egui 0.36.2）。
                // `Shift+↑` / `Alt+↑` 仍然归移动选中：`Modifiers::matches_logically` 对 ctrl 是
                // 精确比（`:211-220`），少了 ctrl 就匹配不上 `COMMAND` 这一层——与 AHK
                // `if ctrl → RecallHistory / else MoveSelection` 分得一样。
                let hist_step = if recall_history_allowed(
                    self.panel.is_some(),
                    !matches!(
                        self.file_route(&self.query),
                        source::filesearch::Route::NotOurs
                    ),
                    self.keys_live(),
                ) {
                    let up = ui.input_mut(|i| {
                        i.consume_key(egui::Modifiers::COMMAND, egui::Key::ArrowUp)
                    });
                    let down = ui.input_mut(|i| {
                        i.consume_key(egui::Modifiers::COMMAND, egui::Key::ArrowDown)
                    });
                    match (up, down) {
                        (true, _) => Some(1),
                        (false, true) => Some(-1),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(step) = hist_step {
                    let key_name = if step > 0 { "↑" } else { "↓" };
                    let next = source::application::recall_history(
                        &self.history,
                        &self.query,
                        &mut self.history_browse,
                        step,
                    );
                    match next {
                        Some(text) => {
                            // 日志里不许出现回填的那串：它就是用户敲过的原文（FR-SYS-03）。
                            let where_ = if self.history_browse.index == 0 {
                                "回到自己敲的那串".to_string()
                            } else {
                                format!("第 {} 条", self.history_browse.index)
                            };
                            self.set_query(
                                ui.ctx(),
                                text,
                                &format!(
                                    "搜索历史 Ctrl+{key_name}：{where_}（共 {} 条，原文不落日志）",
                                    self.history.len()
                                ),
                            );
                        }
                        None => eprintln!(
                            "搜索历史 Ctrl+{key_name}：{}那头已经到底（共 {} 条），输入框不动",
                            if step > 0 { "更早" } else { "更新" },
                            self.history.len()
                        ),
                    }
                }
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
                let (hit_n, hit_p, hit_i, hit_e, hit_cd, hit_cf, hit_cm, hit_cl) = {
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
                        hot(egui::Modifiers::COMMAND, egui::Key::L),
                    )
                };
                // 批次 B8：`F3` 就是 `Ctrl+E` 的别名（需求书附录 A 那两格写的是同一个动作）⇒ 直接
                // 并进去，下面编辑那三处判据一处都不用改。`edit_via` 只为日志能分清这一下是哪颗键
                // （`Ctrl+E` 原本在现场不留一行日志，真机取证时"F3 到底走没走到"就没有可读的凭据）。
                let edit_via: &'static str = if hit_f3 { "F3" } else { "Ctrl+E" };
                let hit_e = hit_e || hit_f3;
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

                // ---- 批次 P1（FR-ACT-03）：`→` 进面板 / `←`・空输入 `Backspace` 出面板 ----
                // 判据放在这里（渲染列表之前）而不是列表那一坨里：退出要恢复查询，
                // 同帧的 TextEdit 若已经按"过滤词"渲染过一遍，恢复就得等下一帧、光标还会差一格。
                // 两个旗标都只在**搜索页**成立：设置页整页替换了搜索页（`settings_ui` 里 `return`），
                // 那一屏没有列表可切。
                // `take_fresh_key` 排最后：只有真的要进出才把键收走，不命中就原样留给输入框。
                let panel_exit: Option<&'static str> = if self.panel.is_none()
                    || self.settings_open
                {
                    None
                } else if take_fresh_key(ui.ctx(), egui::Key::ArrowLeft) {
                    Some("←")
                } else if self.query.is_empty() && fresh_plain_backspace(ui.ctx()) {
                    Some("空输入的 Backspace")
                } else {
                    None
                };
                let panel_enter = !self.settings_open
                    && self.panel.is_none()
                    && caret_at_end(ui.ctx(), &self.query)
                    && take_fresh_key(ui.ctx(), egui::Key::ArrowRight);

                // ---- 批次 E3-丙：次级动作键（`Ctrl+Enter` 一类**窗口内**热键）----
                // 位置有讲究：必须在这里把事件收走。下面 Enter 那一档用的是 `key_pressed`，
                // 它不看修饰键 ⇒ 不收走的话 `Ctrl+Enter` 会先跑次级动作、再把选中项也跑一遍，
                // 一次按键干两件事（真机最容易撞见的正是这一条）。面板开着时不接管：
                // 面板里 `Ctrl+Enter` 就是 `Enter`，仍是"执行高亮动作"，与批次 P1 的口径一致。
                let secondary_hit = !self.settings_open
                    && self.panel.is_none()
                    && take_local_hotkey(ui, self.prefs.secondary_hotkey);

                // ---- 批次 E6（FR-ACT-01 最后一格）：`Alt+Enter` 复制这一行 ----
                // 与 `Ctrl+Enter` 同一手法、同一条理由：下面 Enter 那档用 `key_pressed`，不看修饰键
                // ⇒ 不在这里把事件收走，`Alt+Enter` 就会"复制完再把选中项执行一遍"（启动一个程序
                // 去复制一行文本，是这一格最容易撞出来的事故）。排在次级动作键**之后**取：用户把那
                // 颗键自定义成别的组合时，`Alt+Enter` 仍归本条（`check_secondary` 现在也会用
                // `LOCAL_TAKEN` 挡下"把次级键录成 Alt+Enter"，所以两者不会争同一帧）。
                // 面板开着时**不接管**，于是事件原样落到面板的 Enter 那档 ⇒ `Alt+Enter` ＝执行高亮
                // 动作，与 AHK 一致（`_Execute` 在 `Mode="actions"` 分支整个忽略 modifier，
                // `SearchWindow.ahk:1029-1038`）⇒ 这一格连偏差都不算。
                let alt_enter_hit = self.panel.is_none() && take_local_hotkey(ui, ALT_ENTER);

                // ---- 批次 E4（FR-ACT-05）：`Insert` 标记 / 取消标记 ----
                // 只认"新按"、只认无修饰（与 `take_fresh_key` 同一口径）。面板开着时不接管：
                // 面板里的 `Insert` 没有动作表条目，抢走它只会让过滤词框少一个键。
                let hit_insert =
                    self.panel.is_none() && take_fresh_key(ui.ctx(), egui::Key::Insert);

                if let Some(by) = panel_exit {
                    self.close_panel(ui.ctx(), by, true);
                } else if panel_enter {
                    // 批次 E4：有标记时 `→` 进的是批量面板——与当前选中哪一行无关
                    // （AHK `_OpenActions()`：`Marked.Count` 非零就对自己的集合动手）。
                    if !self.marked.is_empty() {
                        self.open_batch_panel(ui.ctx());
                    } else {
                        let plan = self.plan();
                        let row = row_at(self.selected, plan.results.len(), plan.head.len());
                        if let Some(row) = row {
                            self.open_panel(ui.ctx(), row);
                        }
                    }
                } else if secondary_hit {
                    let plan = self.plan();
                    let row = row_at(self.selected, plan.results.len(), plan.head.len());
                    if let Some(row) = row {
                        self.run_secondary_action(ui.ctx(), row);
                    }
                } else if hit_insert {
                    // 标记后选中自动下移一行（AHK `ToggleMark` 同口径；加到最后一行就停在原地，
                    // 不绕回开头——用户按 `Insert` 是"接着标下一个"，不是"从头再来"）。
                    // 路径不落日志（FR-SYS-03：这些路径来自查询框现搜出来的结果）。
                    let plan = self.plan();
                    let row = row_at(self.selected, plan.results.len(), plan.head.len());
                    let total = row_count(plan.results.len(), plan.head.len());
                    let here = self.selected;
                    let path = row.and_then(|r| {
                        markable_path(r, &plan.head, &plan.results, &self.items)
                    });
                    match path {
                        Some(path) => {
                            let added = toggle_mark(&mut self.marked, &path);
                            if self.selected + 1 < total {
                                self.selected += 1;
                            }
                            eprintln!(
                                "快捷键 Insert：第 {} 行{}标记（共 {} 项已标记；路径不落日志）",
                                here + 1,
                                if added { "已" } else { "已取消" },
                                self.marked.len()
                            );
                            ui.ctx().request_repaint();
                        }
                        None => {
                            self.toast = Some((
                                "这一行不是文件/文件夹，不能标记".to_string(),
                                Instant::now() + TOAST_TTL,
                                TOAST_ERR,
                            ));
                            ui.ctx().request_repaint_after(TOAST_TTL);
                            eprintln!("快捷键 Insert：第 {} 行不是文件/文件夹，未标记", here + 1);
                        }
                    }
                } else if hit_copy || alt_enter_hit {
                    // FR-ACT-10 附录 A「`Ctrl+C` 复制」与 FR-ACT-01「`Alt+Enter` 复制这一行」：
                    // 两颗键在 AHK 里就是同一份载荷——`ActionCatalog.RunModifier(item,"alt")`
                    // 调的 `CopyText(item.CopyText())`（`ActionCatalog.ahk:37-39`）与 `_CopySelected()`
                    //（`SearchWindow.ahk:1480-1486`）取的是同一个 `ResultItem.CopyText()`
                    //（`ResultItem.ahk:55-59`）⇒ 本程序也共用这一坨，只把键名分开记进日志。
                    // 内容 = FR-UIX-10 大字
                    // 显示那一串（同一条 [`large_text_of`]，也就是 AHK `ResultItem.CopyText()` 的口径：
                    // 文件/目录/网址取路径、计算器取纯数字、应用与命令取名称）。
                    // 查询框里选中的文字在上面已经让出去了，这一格碰不到它。
                    // **与 AHK 的一处有意偏差**：AHK `_CopySelected()` 复制前把搜索窗 `Hide()`
                    //（`SearchWindow.ahk:1481`），这里让窗口留在原地——复制完通常紧接着要粘到别处，
                    // 关窗会让用户以为按键没生效；与本程序面板里那三条复制同口径（复制不关窗）。
                    let key_name = if alt_enter_hit { "Alt+Enter" } else { "Ctrl+C" };
                    let text = {
                        let plan = self.plan();
                        let item = item_rank(self.selected, plan.head.len())
                            .and_then(|r| plan.results.get(r))
                            .map(|i| &self.items[*i]);
                        // 第三个参数给空串：`large_text_of` 的"没有选中行"兜底是回落到查询原文，
                        // 那是 Ctrl+L 大字的语义（AHK `_ShowLargeType` 的 `Input.Value`）；
                        // 复制要的是"这一行"，一行都没有时必须什么都不复制（AHK `_CopySelected` 同）。
                        large_text_of(plan.head.get(self.selected), item, "")
                    };
                    match text {
                        Some(text) => match copy_to_clipboard(&text) {
                            Ok(()) => {
                                eprintln!(
                                    "快捷键 {key_name}：已复制这一行（{} 个字符，内容不落日志）",
                                    text.chars().count()
                                );
                                self.toast = Some((
                                    format!("✓ 已复制：{text}"),
                                    Instant::now() + TOAST_TTL,
                                    TOAST_OK,
                                ));
                                ui.ctx().request_repaint_after(TOAST_TTL);
                            }
                            Err(e) => {
                                eprintln!("快捷键 {key_name}：复制失败：{e}");
                                self.toast = Some((
                                    format!("⚠ 复制失败：{e}"),
                                    Instant::now() + TOAST_TTL,
                                    TOAST_ERR,
                                ));
                                ui.ctx().request_repaint_after(TOAST_TTL);
                            }
                        },
                        None => {
                            eprintln!("快捷键 {key_name}：这一屏没有可复制的行（选中第 {} 行）", self.selected + 1);
                            self.toast = Some((
                                "⚠ 这一屏没有可复制的行".to_string(),
                                Instant::now() + TOAST_TTL,
                                TOAST_ERR,
                            ));
                            ui.ctx().request_repaint_after(TOAST_TTL);
                        }
                    }
                } else if hit_f4 {
                    match open_config_folder() {
                        Ok(()) => {
                            self.menu_open = false;
                            ui.ctx().request_repaint();
                        }
                        Err(e) => {
                            eprintln!("快捷键 F4：{e}");
                            self.toast = Some((
                                format!("⚠ {e}"),
                                Instant::now() + TOAST_TTL,
                                TOAST_ERR,
                            ));
                            ui.ctx().request_repaint_after(TOAST_TTL);
                        }
                    }
                } else if hit_cm {
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
                    let p = self.file_route(&self.query);
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
                } else if hit_cl {
                    // FR-UIX-10（2026-10-24）：大字显示。内容口径对齐 AHK 版
                    // `_ShowLargeType`——有选中行就用它的展示文本，没有就用输入框原文。
                    // 原文不落日志，只记字符数与字号（FR-SYS-03）。
                    //
                    // 与 AHK 版有一处**有意偏离**：AHK 在弹大字前把搜索窗 Hide 掉，这里让主窗
                    // 留在原位。真机实测（2026-10-24）Hide 之后大字一关，焦点会落到本进程那个
                    // 隐藏的事件窗上（按键全部丢失），不如让主窗留在下面接回焦点。
                    // 配套改动：FR-WIN-04 在大字窗开着期间不自动隐藏主窗（见下方 keep_open
                    // 里的 `large_type.showing()`），否则焦点一被大字窗拿走，主窗 50ms 就没了。
                    let text = {
                        let plan = self.plan();
                        let item = item_rank(self.selected, plan.head.len())
                            .and_then(|r| plan.results.get(r))
                            .map(|i| self.items[*i].clone());
                        large_text_of(plan.head.get(self.selected), item.as_ref(), &self.query)
                    };
                    match text {
                        Some(text) => {
                            self.toast = None;
                            self.large_type.show(ui.ctx(), text);
                        }
                        None => eprintln!("快捷键 Ctrl+L：没有可显示的文字（空输入且没有选中行）"),
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
                            eprintln!(
                                "快捷键 {edit_via}：弹出编辑对话框（第 {} 行，名称不落日志）",
                                self.selected + 1
                            );
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
                                    format!("✓ 已置顶「{shown}」（图标右上角角标，重启仍在）")
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
                                HeadRow::Action(hit) => {
                                    // 批次 P1：面板那十几行也占前 10 个数字键位（口径与 Enter 一致）。
                                    eprintln!(
                                        "快捷键 数字键 {digit}：执行操作面板动作「{}」",
                                        hit.id.label()
                                    );
                                    panel_action = Some(hit.id);
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
            // 不在设置页 ⇒ 「文件搜索」页的编辑缓冲收尾（把没失焦就走的编辑回写掉再丢掉）。
            self.finish_file_scope_edits();
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
                // 批次 P1（FR-ACT-03）：面板借用的是同一个输入框（`query` 当过滤词），
                // 于是那句「关于〈项标题〉的操作」就落在占位符上——与 AHK 的 `SetCueBanner` 同形态，
                // 而且只在过滤词为空（刚进面板）时露出来，正是需要它的那一刻。
                let hint = match &self.panel {
                    Some(p) => egui::RichText::new(p.target.banner()),
                    None if self.prefs.appearance.status_bar => {
                        egui::RichText::new(SEARCH_HINT_SHORT)
                    }
                    None => egui::RichText::new(SEARCH_HINT),
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
                    // 批次 C3：框里的字被用户改动 ⇒ 翻历史的位置归零（AHK 在 `_OnInputChange`
                    // 里清 `HistoryIndex`，同一口径）。不清的症状：翻到第 3 条、改成别的字、
                    // 再按 `Ctrl+↑` 会跳到第 4 条——中间那条永远看不见。
                    // 程序化回填不会走到这里：egui 的 `changed` 只由 `events()` 那一路置起
                    // （`text_edit/builder.rs:568-602`，0.36.2），换个字符串进 `self.query`
                    // 不算用户改动。真机第 4 条专门验这一点（连按三次应依次回填三条）。
                    if self.history_browse.index != 0 {
                        self.history_browse = Default::default();
                        eprintln!("搜索历史：输入已改 ⇒ 翻页位置归零");
                    }
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
            if self.keys_live() {
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
            let file_route = self.file_route(&self.query);
            let in_file_mode = !matches!(file_route, source::filesearch::Route::NotOurs);

            // 2026-10-24 修滚轮：列表右侧那块「拖空白移动窗口」的手柄必须与 ScrollArea 同层。
            // 原先它铺在独立的 Background `Area` 上（`drag_handle`），指针一进去，
            // `Context::rect_contains_pointer` 认到的层就不是滚动区域那一层，滚轮整段被吞。
            // 同层注册后：滚轮照常滚，点击仍归结果行（行是 click，这里是 drag-only，且行在本行之后注册）。
            // 弹窗/确认框/帮助页打开时整块撤掉，与原来的让位规则一致。
            if !self.keys_live() {
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
                // FR-ACT-03：面板模式的过滤词没命中任何动作。必须排在最前——此刻查询框里是
                // 过滤词，`in_file_mode` / `q.is_empty()` 那两格说的是"搜索/清单"，全是错的话；
                // 更要短路 FR-SRC-23 的「Enter 弹出新增」，否则打一个不存在的动作名按 Enter
                // 会拿过滤词去建一条快捷项。
                if self.panel.is_some() {
                    ui.label("没有匹配的动作——改字再筛，或按 Esc / ← 返回结果列表。");
                } else if q.is_empty() {
                    ui.label("清单为空——按 Ctrl+Shift+N、或点右上 ☰ 里的「新建快捷项」，也可按 Ctrl+I 批量导入。");
                // 批次 X：文件模式下"未找到"这三行字会说错两件事——范围（不是"清单里没有"，
                // 是"索引范围里没有"或"这个目录空/读不到"）与预填（`'报告`、`pic 报告` 这种带
                // 触发符的串喂给 `from_input` 会生成一条命令行是乱码的条目）。所以另起一格。
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
                    if ui.button("＋ 新建快捷项 (Ctrl+Shift+N)").clicked() {
                        self.dialog = Some(DialogState::blank());
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.dialog = Some(DialogState::blank());
                        self.dialog_enter_armed = true;
                    }
                } else {
                    ui.label(format!("未找到「{}」——按 Enter 或点下方按钮弹出新增窗口", self.query.trim()));
                    // FR-SRC-23（2026-10-17 修订）：无匹配 Enter 不再直接运行，而是弹出预填当前输入的 AddDialog，确定即入库。
                    if ui.button("＋ 添加为快捷项 (Ctrl+Shift+N)").clicked() {
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
                let keys_live = self.keys_live();
                // 批次 B8（FR-ACT-10 附录 A「滚轮移动选中」，口径同 AHK `_OnMouseWheel`）：
                // 一格滚轮 = 选中上/下一行，两端夹住不回绕（与 ↑↓ 同一条边界）。列表位置不另外滚，
                // 交给下面既有的"选中跟随"（`follow_selection` → `scroll_to_me(Align::Center)`），
                // 所以滚轮仍然看得见后面的行，只是每格走一行——要快走有 PgUp/PgDn 一整页。
                // 配套的"这段列表不直接吃滚轮"在 `ScrollArea::scroll_source` 那一句，判据同 `keys_live`：
                // 两头必须用同一个条件，否则会出现"移了选中又按平滑增量滚了一截列表"的两倍位移。
                if keys_live {
                    let wheel = ui.input(|i| wheel_selection_step(&i.events));
                    if wheel != 0 {
                        let before = self.selected;
                        if wheel > 0 {
                            self.selected = (self.selected + 1).min(total - 1);
                        } else {
                            self.selected = self.selected.saturating_sub(1);
                        }
                        if self.selected != before {
                            eprintln!("滚轮：选中移到第 {} 行（共 {total} 行）", self.selected + 1);
                        }
                    }
                }
                if keys_live {
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
                                HeadRow::Action(hit) => {
                                    // 批次 P1（FR-ACT-03）：Enter 执行选中的动作，学习统计以
                                    // **原查询**记录（在 `run_panel_action` 里取 `saved_query`）。
                                    panel_action = Some(hit.id);
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
                    // 批次 B8：滚轮这一格归"移动选中"（上面那段），列表不再直接吃滚轮；
                    // 滚动条拖拽与触摸拖滚照常留着，键盘（↑↓/PgUp）也照常。
                    // `!keys_live` 时（弹窗/确认框/关于页盖在上面）恢复默认行为，那时选中不跟着动。
                    .scroll_source(egui::containers::scroll_area::ScrollSource {
                        mouse_wheel: !keys_live,
                        ..Default::default()
                    })
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
                                        // FR-ACT-03 的右键入口。搜索行的对象就是那条 URL ⇒
                                        // 面板给「打开 / 复制网址 / 复制名称 / 添加为命令 / 大字显示」五条
                                        // （`on_disk` 对 URL 恒假，提权、打开方式、定位、终端、属性都不出现）。
                                        sel.context_menu(|ui| {
                                            if ui.button("操作…").clicked() {
                                                panel_request = Some(Row::Head(h));
                                                ui.close();
                                            }
                                        });
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
                                        // FR-ACT-03：独占的自定义命令行也是条目，右键给面板。
                                        // 「打开」这一条用面板里的 `PanelOpen::Entry`（已把
                                        // `{query}` 填好的那份），所以按面板的"打开"和按 Enter 同结果。
                                        sel.context_menu(|ui| {
                                            if ui.button("操作…").clicked() {
                                                panel_request = Some(Row::Head(h));
                                                ui.close();
                                            }
                                        });
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
                                        // 批次 E4：标记的落款用标题前缀 `✓`（不另开列宽），
                                        // 不另开列宽——行号徽标与图标在标记前后保持同一 x 坐标。
                                        let shown = if is_marked(&self.marked, &hit.path) {
                                            format!("✓ {}", hit.title)
                                        } else {
                                            hit.title.clone()
                                        };
                                        let sel = ui.selectable_label(is_sel, shown);
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        ui.label(
                                            egui::RichText::new(hit.subtitle.as_str()).weak().size(sub_size),
                                        );
                                        if sel.clicked() {
                                            open_file = Some(hit.clone());
                                        }
                                        // 文件行不在清单里 ⇒ 没有"编辑/删除清单条目"可言，右键给
                                        // 「操作…」面板（FR-ACT-03/04）加三件对这个文件本身有意义的事
                                        // （复制 / 定位 / 收进清单）。后三条是面板的子集，留着是因为
                                        // 它们各自一键直达、不必进面板再挑。
                                        sel.context_menu(|ui| {
                                            if ui.button("操作…").clicked() {
                                                panel_request = Some(Row::Head(h));
                                                ui.close();
                                            }
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
                                HeadRow::Action(hit) => {
                                    // 批次 P1（FR-ACT-04）：标题 + 键位 + 一句"这一条会碰什么"。
                                    // 副标题在 `actions_for` 里就是照这个目的填的（复制动作给要复制的
                                    // 那串、定位动作给路径、终端动作给目录），所以原样显示、不再加工。
                                    ui.horizontal(|ui| {
                                        row_badge(ui, h, hit.id.badge());
                                        ui.add_sized(
                                            [icon, icon],
                                            egui::Label::new(
                                                egui::RichText::new("\u{2699}")
                                                    .size(glyph)
                                                    .weak(),
                                            ),
                                        )
                                        .on_hover_text("操作面板（FR-ACT-03 / FR-ACT-04）");
                                        let sel = ui.selectable_label(is_sel, hit.label.clone());
                                        if is_sel && follow_selection {
                                            sel.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        if !hit.keys.is_empty() {
                                            ui.label(
                                                egui::RichText::new(hit.keys)
                                                    .monospace()
                                                    .small()
                                                    .weak(),
                                            );
                                        }
                                        ui.label(
                                            egui::RichText::new(hit.subtitle.as_str())
                                                .weak()
                                                .size(sub_size),
                                        );
                                        if sel.clicked() {
                                            panel_action = Some(hit.id);
                                        }
                                    });
                                }
                            }
                        }
                        // 批次 F4：可见行缺图标（含被 LRU 淘汰掉的）就地补一次，
                        // 不指望别的事件来兜；`spawn_missing_icons` 会把已在飞/已失败过的滤掉。
                        let tick = {
                            let t = self.icon_tick.get() + 1;
                            self.icon_tick.set(t);
                            t
                        };
                        let visible_jobs: Vec<IconJob> = results
                            .iter()
                            .filter_map(|&hit| {
                                let it = &self.items[hit];
                                let spec = icon_spec_of(it);
                                let key = source::icon::cache_key(spec)?;
                                Some((key, spec.to_string()))
                            })
                            .collect();
                        self.spawn_missing_icons(visible_jobs);
                        for (rank, &hit) in results.iter().enumerate() {
                            let idx = rank + head_len;
                            let item = &self.items[hit];
                            let is_sel = idx == self.selected;
                            ui.horizontal(|ui| {
                                // 行号徽标（2026-10-18 用户要求：数字键没有可见对应物）：
                                // 前 10 行标 1-9/0，第 11 行起留白——那里没有按键，标了反而误导。
                                row_badge(ui, idx, "执行");
                                // 批次 F4：图标按缓存键取（一堆 .pdf 共用一张纹理），
                                // 画出来的同时把 LRU 时刻刷新成本帧的 tick。
                                let slot = match source::icon::cache_key(icon_spec_of(item))
                                    .as_ref()
                                    .and_then(|k| self.icons.get(k))
                                {
                                    Some(entry) => {
                                        entry.last_use.set(tick);
                                        ui.image(egui::load::SizedTexture::new(
                                            entry.texture.id(),
                                            egui::vec2(icon, icon),
                                        ))
                                    }
                                    None => ui.allocate_exact_size(
                                        egui::vec2(icon, icon),
                                        egui::Sense::hover(),
                                    )
                                    .1,
                                };
                                // FR-SRC-19：置顶项在图标右上角画角标（2026-10-29 批次 F4
                                // 从「名称前 📌」改来——前缀会把名称挤偏，角标贴图标不占字宽）。
                                if item.pinned {
                                    paint_pin_badge(ui, slot.rect);
                                }
                                // 批次 E4：标记项用名称前 `✓` 前缀，不另开列宽。
                                // 批次 B6：带参项在名称前加 `*`（最近名字，先于 ✓）。
                                let marked = is_marked(&self.marked, &item.path);
                                let mut shown = format!("{}{}", param_row_mark(item), item.name);
                                if marked {
                                    shown = format!("✓ {shown}");
                                }
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
                                    // FR-ACT-03：右键「操作…」进面板。放在最前——它是"这一行还能做
                                    // 什么"的总入口，编辑/置顶/删除是其中三条的快捷版。
                                    if ui.button("操作…").clicked() {
                                        panel_request = Some(Row::Item(rank));
                                        ui.close();
                                    }
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
                // 批次 E4（FR-ACT-05）：有标记时左格改说"攒着什么、怎么取消、怎么动手"。
                // 面板开着仍说怎么回结果——那一格是退出口，比标记数更要紧。
                let marks = self.marked.len();
                let left = if self.panel.is_none() && marks > 0 {
                    marked_hint(marks)
                } else {
                    status_left_text(
                        self.panel.is_some(),
                        browse,
                        in_file_mode,
                        self.prefs.tab_mode,
                        matches!(self.prefs.default_scope, Scope::Basic),
                    )
                };
                // 左格剪到右格之前：360 宽 + 长路径时宁可截断，也不让两段字叠在一起。
                // 右格占位按面板开合给两档：结果模式多一组「→ 操作」（键帽 + 词），留到 232；
                // 面板模式只有 Enter 那一枚，回到原来的 170。
                let right_reserve = if self.panel.is_some() { 170.0 } else { 232.0 };
                let left_rect = egui::Rect::from_min_max(
                    bar.min,
                    egui::pos2((bar.max.x - right_reserve).max(bar.min.x + 40.0), bar.max.y),
                );
                ui.painter().with_clip_rect(left_rect).text(
                    egui::pos2(bar.min.x, bar.center().y),
                    egui::Align2::LEFT_CENTER,
                    left,
                    egui::FontId::proportional(self.theme.title_font_size),
                    self.theme.subtitle,
                );
                let selected_row = row_at(self.selected, total - head_len, head_len);
                let action = match selected_row {
                    Some(Row::Head(h)) => enter_action_label(plan.head.get(h)),
                    _ => enter_action_label(None),
                };
                // FR-UIX-08 右格原文的后半句「+ `→` 动作」。批次 F8 那颗键帽没画，理由是
                // 「还没有行动作面板，画个死键帽反而误导」；面板有了，判据也照面板的真实入口
                // 取——只有 `panel_for_row` 认得的行（条目 / 自定义命令 / 文件 / 搜索）才亮，
                // ~~计算器、终端、提示行按 `→` 是什么都不做的，就不给它们许诺。~~
                // 【2026-10-27 批次 E5：计算器行现在认得了（它给的是文本那张三格表），
                // 这一格的判据本来就是现取 `panel_for_row`，所以键帽自己会跟着亮；
                // 终端与各类提示行仍按原话不亮。】
                // 批次 E4：有标记时 `→` 恒可进——开的是**批量面板**，与当前选中哪一行无关
                // （AHK `_OpenActions()` 同口径：`Marked.Count` 非零就对自己的集合动手）。
                let panel_ready = self.panel.is_none()
                    && (marks > 0
                        || match selected_row {
                            Some(row) => panel_for_row(
                                row,
                                &plan.head,
                                &results,
                                &self.items,
                                &self.path_states,
                            )
                            .is_some(),
                            None => false,
                        });
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
                status_key_cap(&mut right, "Enter", &self.theme);
                if panel_ready {
                    right.label(
                        egui::RichText::new(if marks > 0 { "批量" } else { "操作" })
                            .size(self.theme.title_font_size)
                            .color(self.theme.title),
                    );
                    status_key_cap(&mut right, "→", &self.theme);
                }
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
                            // Area 里的 label 默认不限宽 ⇒ 长句会画成一整行、越过窗口被裁掉。
                            // 与速查弹窗同口径按窗口宽度收（留出胶囊内边距），让它自己换行。
                            ui.set_max_width((ui.ctx().content_rect().width() - 72.0).max(160.0));
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

            // 批次 D4（FR-CFG-06）：拖进来的文件只在这里接——弹窗关着时窗口失焦 50ms 就自藏
            //（FR-WIN-04 的 HIDE_DELAY），根本拖不到；Delphi 的 `WM_DropFiles` 也只挂在
            // `frmShortCut` 这一扇窗上。`dropped_files` 是**当帧**输入，这一帧不取就丢了。
            let dropped = ui.ctx().input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|f| f.path().to_str().map(str::to_string))
                    .next()
            });
            if let Some(path) = dropped {
                self.dialog_drop(ui.ctx(), &path);
            }

            // 批次 D4：三个只在弹窗里有效的热键。主界面那一整块快捷键以 `dialog.is_none()`
            // 为门，所以不会撞车；`Ctrl+Enter` 在那里是次级动作键，此刻同样是关着的。
            // 取键必须排在下面的 TextEdit 与 Enter 判据之前：`consume_key` 会把事件删掉，
            // 否则这一下 `Ctrl+Enter` 之后还会被 `key_pressed(Enter)` 当成"确定"再跑一遍。
            // `Ctrl+Shift+O` 要先吃：`consume_key(COMMAND, ·)` 对多余的 Shift 是放行的
            //（与 Shift+Tab / Shift+空格同一个坑）。
            let prompt_open = self.dialog.as_ref().is_some_and(|d| d.fill_prompt.is_some());
            // 批次 B6：「标记但编码=无参数」的追问露着时，Enter 不再是"确定"——
            // 那一屏是在问他"还要不要存"，再按一次回车就替他答了。
            // 条件里再查一遍 `marker_warn_needed`：用户把编码改成非「无参数」之后，
            // 这条追问就过期了，不该继续扣着回车。
            let marker_warn_now = self
                .dialog
                .as_ref()
                .is_some_and(|d| d.marker_warn && d.marker_warn_needed());
            let (want_file, want_dir, want_test) = if prompt_open {
                (false, false, false)
            } else {
                let hot =
                    |mods: egui::Modifiers, k: egui::Key| ui.input_mut(|i| i.consume_key(mods, k));
                let dir = hot(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::O);
                let file = hot(egui::Modifiers::COMMAND, egui::Key::O);
                let test = hot(egui::Modifiers::COMMAND, egui::Key::Enter);
                (file, dir, test)
            };
            // 闭包里有系统对话框要开、有 `self.dialog` 的借用要握 ⇒ 三件事都先记旗标，
            // 模态闭包返回之后再落（与列表那边"当帧只记、帧末才做"同一条口径）。
            let mut pick_request: Option<bool> = if want_dir {
                Some(true)
            } else if want_file {
                Some(false)
            } else {
                None
            };
            let mut test_request = want_test;
            let mut prompt_answer: Option<bool> = None;
            let mut marker_warn_off = false;

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
                            // 「要不要替换」那条询问开着时 Enter 不提交：那一屏只有两个按钮，
                            // 把"回车"当确定会连询问一起跳过（用户以为还没答）。
                            if !armed
                                && !prompt_open
                                && !marker_warn_now
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
                                // 批次 D4（FR-CFG-06）三个辅助按钮。系统命令那一档整组禁用：
                                // 它的命令行是 `internal:<id>`，被真实路径覆盖就再也执行不了。
                                ui.horizontal(|ui| {
                                    ui.add_sized([label_w, 20.0], egui::Label::new(""));
                                    let internal = dg.kind == ItemKind::Internal;
                                    ui.add_enabled_ui(!internal, |ui| {
                                        if ui
                                            .button("选文件… (Ctrl+O)")
                                            .on_hover_text(
                                                "Ctrl+O：命令行＝所选文件，别名/名称按文件名去扩展名",
                                            )
                                            .clicked()
                                        {
                                            pick_request = Some(false);
                                        }
                                        if ui
                                            .button("选目录… (Ctrl+Shift+O)")
                                            .on_hover_text(
                                                "Ctrl+Shift+O：命令行＝所选目录，别名/名称按目录末段",
                                            )
                                            .clicked()
                                        {
                                            pick_request = Some(true);
                                        }
                                        if ui
                                            .button("测试执行 (Ctrl+Enter)")
                                            .on_hover_text(
                                                "Ctrl+Enter：照当前表单启动一次；窗口不收、不记使用统计",
                                            )
                                            .clicked()
                                        {
                                            test_request = true;
                                        }
                                    });
                                    ui.label(
                                        egui::RichText::new("也可把 .lnk / 程序 / 文件夹拖进来")
                                            .weak()
                                            .small(),
                                    );
                                });
                                row(ui, "参数", &mut dg.arguments, "");
                                // 批次 B6（FR-ACT-07）：编码四档。紧贴「参数」——它决定的正是
                                // "参数怎么代进去"。`无参数` 时标记不展开也不弹框（Delphi 同口径）。
                                ui.horizontal(|ui| {
                                    ui.add_sized([label_w, 20.0], egui::Label::new("编码"));
                                    let mut idx = source::params::ParamType::ALL
                                        .iter()
                                        .position(|t| *t == dg.param_type)
                                        .unwrap_or(0);
                                    egui::ComboBox::from_id_salt("param_type_combo")
                                        .selected_text(dg.param_type.label())
                                        .show_index(
                                            ui,
                                            &mut idx,
                                            source::params::ParamType::ALL.len(),
                                            |i| {
                                                source::params::ParamType::ALL[i]
                                                    .label()
                                                    .to_string()
                                            },
                                        );
                                    dg.param_type = source::params::ParamType::ALL[idx];
                                    ui.label(
                                        egui::RichText::new(dg.param_type.hint()).weak().small(),
                                    );
                                });
                                ui.horizontal_wrapped(|ui| {
                                    ui.add_sized([label_w, 18.0], egui::Label::new(""));
                                    ui.label(
                                        egui::RichText::new(
                                            "标记：{%p} 你填的参数 · {%c} 呼出那一刻的剪贴板 · {%wd} 前台窗口句柄 · {%wt} 它的标题 · {%wc} 它的类名（编码=无参数时都不替换）",
                                        )
                                        .weak()
                                        .small(),
                                    );
                                });
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
                            // 批次 D4：别名/名称已经有别的值 ⇒ 先问一句（Delphi 对两个字段各弹一次
                            // `MessageBox`，这里合成一条）。按「保留」只补空着的两格，
                            // 命令行/参数/工作目录照填——那三格是"这条命令到底是什么"。
                            if let Some(p) = self.dialog.as_ref().and_then(|d| d.fill_prompt.as_ref())
                            {
                                ui.separator();
                                let mut parts: Vec<String> = Vec::new();
                                if let Some(old) = &p.keyword_old {
                                    parts.push(format!("别名「{old}」→「{}」", p.fill.keyword));
                                }
                                if let Some(old) = &p.name_old {
                                    parts.push(format!("名称「{old}」→「{}」", p.fill.name));
                                }
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{}，要替换吗？",
                                            parts.join("，")
                                        ))
                                        .strong(),
                                    );
                                    if ui.button("替换").clicked() {
                                        prompt_answer = Some(true);
                                    }
                                    if ui.button("保留").clicked() {
                                        prompt_answer = Some(false);
                                    }
                                });
                            }
                            // 批次 B6（FR-ACT-07）：有标记但编码=无参数 ⇒ 保存前问一句。
                            // 「仍然保存」把这一按再交回 Confirm（此时弹窗上已带 `marker_warn`，
                            // 外层不再追问）；「返回修改」只收提示，框和内容都不动。
                            if self
                                .dialog
                                .as_ref()
                                .is_some_and(|d| d.marker_warn && d.marker_warn_needed())
                            {
                                ui.separator();
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        egui::RichText::new(
                                            "命令行/参数里有 {%…} 标记，但编码是「无参数」：这些标记会原样交给程序，不会被替换。仍然保存吗？",
                                        )
                                        .strong(),
                                    );
                                    if ui.button("仍然保存").clicked() {
                                        result = DlgResult::Confirm;
                                    }
                                    if ui.button("返回修改").clicked() {
                                        marker_warn_off = true;
                                    }
                                });
                            }
                            // 按钮贴底：窗口被拉高后不会孤零零吊在表单下面。
                            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                                ui.horizontal(|ui| {
                                    if ui.button("确定 (Enter)").clicked() {
                                        result = DlgResult::Confirm;
                                    }
                                    if ui.button("取消 (Esc)").clicked() {
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
                    // 批次 B6（FR-ACT-07）：有标记但编码=无参数 ⇒ 先在这一帧把追问露出来，
                    // 不提交。「仍然保存」会把同一按再交回来，那时 `marker_warn` 已置位就不再追问。
                    let warn_now = self
                        .dialog
                        .as_ref()
                        .is_some_and(|d| d.marker_warn_needed() && !d.marker_warn);
                    if warn_now {
                        if let Some(dg) = self.dialog.as_mut() {
                            dg.marker_warn = true;
                        }
                        eprintln!("弹窗：命令行/参数含参数标记、编码是「无参数」⇒ 保存前追问一次");
                        ui.ctx().request_repaint();
                    } else if let Some(dg) = self.dialog.take() {
                        let item = dg.to_item();
                        // FR-ACT-07「名称不得含 `,`」：Delphi 版清单是 `F|编码|别名|名称|命令行`
                        // 这种逗号/竖线定界的行格式，名字里带逗号在那边会被拆成两列。我们存 JSON
                        // 本不需要这条，但需求书把它列为规则，照做——两份清单互相导时不至于分岔。
                        if item.name.contains(',') {
                            eprintln!("弹窗：名称含英文逗号 ⇒ 不提交（改名后再存）");
                            self.toast = Some((
                                format!(
                                    "名称里不能有英文逗号：「{}」请改一下（分类、参数各有自己的格子）",
                                    item.name
                                ),
                                Instant::now() + TOAST_TTL,
                                TOAST_ERR,
                            ));
                            self.dialog = Some(dg);
                            ui.ctx().request_repaint_after(TOAST_TTL);
                        } else if item.path.is_empty() {
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
                            let jobs = icon_jobs(std::slice::from_ref(&item));
                            self.spawn_missing_icons(jobs);
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

            // 批次 D4：三个辅助动作的落点。选文件/选目录会开阻塞的系统对话框，模态闭包里
            // 正握着 `self.dialog` 的可变借用 ⇒ 一律挪到闭包返回之后再做。
            if marker_warn_off {
                if let Some(dg) = self.dialog.as_mut() {
                    dg.marker_warn = false;
                }
                ui.ctx().request_repaint();
            }
            if let Some(dir) = pick_request {
                self.dialog_pick(ui.ctx(), dir);
            }
            if test_request {
                self.test_dialog_entry(ui.ctx());
            }
            if let Some(replace) = prompt_answer {                if let Some(dg) = self.dialog.as_mut() {
                    if let Some(p) = dg.fill_prompt.take() {
                        apply_fill(dg, &p.fill, replace);
                        eprintln!(
                            "弹窗：导入值的别名/名称{}，命令行已填（内容不落日志）",
                            if replace { "已替换" } else { "保留原值" }
                        );
                    }
                }
                ui.ctx().request_repaint();
            }

            // 对话框本帧关闭（确定成功/取消/空目标外全路径）→ 焦点还给搜索框。
            if self.dialog.is_none() {
                self.focus_query = true;
            }
        }

        // ---- 批次 B6（FR-ACT-07）：参数框 ----
        // 只在条目写着 `{%p}`（或 `ParamType≠none` 且没有任何标记）时出现；「确定」那一帧
        // 才展开并启动（见 [`Self::confirm_param_prompt`]）。
        if self.param_prompt.is_some() {
            enum ParamResult {
                Keep,
                Confirm,
                Cancel,
            }
            let mut result = ParamResult::Keep;
            let mut picked: Option<String> = None;
            let history: Vec<String> = self.param_history.texts().map(str::to_string).collect();
            let _ = egui::Modal::new(egui::Id::new("param_prompt")).show(ui.ctx(), |ui| {
                let opening = self.param_enter_armed;
                self.param_enter_armed = false;
                // Enter＝确定（开框那一帧不算，与添加/编辑框同一条）。
                if !opening
                    && !egui::Popup::is_any_open(ui.ctx())
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    result = ParamResult::Confirm;
                }
                // 模态是居中的胶囊，宽度由内容决定；窗口只有 640 宽时要主动收窄（与帮助页同口径）。
                ui.set_max_width((ui.ctx().content_rect().width() - 64.0).clamp(240.0, 520.0));
                let (name, ty, has_p) = self
                    .param_prompt
                    .as_ref()
                    .map(|p| {
                        (
                            p.entry.name.clone(),
                            p.entry.param_type,
                            source::params::has_param_marker(&p.entry.path)
                                || source::params::has_param_marker(&p.entry.arguments),
                        )
                    })
                    .unwrap_or_default();
                ui.heading(format!("参数：{name}"));
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "回车运行、Esc 取消。编码：{}（{}）",
                        ty.label(),
                        ty.hint()
                    ))
                    .weak()
                    .small(),
                );
                // 没有 `{%p}` 但它仍要参数（「编码≠无参数 + 一个标记都没有」那一档）：
                // 填的值是接在命令行尾/并进参数里的，提一句省得他找 `{%p}` 在哪。
                let param_hint = if has_p {
                    "要填进 {%p} 的值"
                } else {
                    "这条没有 {%p}：这段会按编码接在命令行后面"
                };
                let resp = {
                    let p = self.param_prompt.as_mut().unwrap();
                    ui.add(
                        egui::TextEdit::singleline(&mut p.text)
                            .hint_text(param_hint)
                            .desired_width(f32::INFINITY),
                    )
                };
                if opening {
                    resp.request_focus();
                }
                if !history.is_empty() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("最近用过的参数（点一条填进框里）")
                            .weak()
                            .small(),
                    );
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .show(ui, |ui| {
                            for h in &history {
                                if ui.selectable_label(false, h).clicked() {
                                    picked = Some(h.clone());
                                }
                            }
                        });
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("确定 (Enter)").clicked() {
                        result = ParamResult::Confirm;
                    }
                    if ui.button("取消 (Esc)").clicked() {
                        result = ParamResult::Cancel;
                    }
                });
            });
            if let Some(h) = picked {
                if let Some(p) = self.param_prompt.as_mut() {
                    p.text = h;
                }
                ui.ctx().request_repaint();
            }
            match result {
                ParamResult::Confirm => self.confirm_param_prompt(ui.ctx()),
                ParamResult::Cancel => {
                    self.param_prompt = None;
                    self.focus_query = true;
                    eprintln!("参数框：取消，未启动（参数原文不落日志）");
                    ui.ctx().request_repaint();
                }
                ParamResult::Keep => {}
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

        // ---- 批次 E4（FR-ACT-05）：批量回收站的执行确认 ----
        // 与删除确认同形态：这一帧只问，按「确定」那一帧才逐个进回收站；Esc 由下方全局分段链处理。
        // 确认框撤掉之后**面板还开着**——用户取消时不希望连现场一起丢。
        if let Some(paths) = self.batch_confirm.clone() {
            enum BatchRecycleResult {
                Keep,
                Yes,
                No,
            }
            let modal = egui::Modal::new(egui::Id::new("batch_recycle_confirm")).show(
                ui.ctx(),
                |ui| {
                    ui.heading(format!("把这 {} 项移到回收站？", paths.len()));
                    ui.separator();
                    ui.label(
                        egui::RichText::new(
                            "可以从回收站还原。网络盘、超出回收站容量的文件会被永久删除——那种情况下系统会先问一次。",
                        )
                        .strong(),
                    );
                    ui.label(
                        egui::RichText::new("执行后这一批标记会清空，面板也会收起。")
                            .weak()
                            .small(),
                    );
                    let mut result = BatchRecycleResult::Keep;
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        result = BatchRecycleResult::Yes;
                    }
                    ui.horizontal(|ui| {
                        if ui.button("移到回收站 (Enter)").clicked() {
                            result = BatchRecycleResult::Yes;
                        }
                        if ui.button("取消 (Esc)").clicked() {
                            result = BatchRecycleResult::No;
                        }
                    });
                    result
                },
            );
            match modal.inner {
                BatchRecycleResult::Yes => {
                    if let Some(paths) = self.batch_confirm.take() {
                        let total = paths.len();
                        let (mut done, mut failed, mut cancelled) = (0usize, 0usize, 0usize);
                        for p in &paths {
                            match recycle_path(p) {
                                Ok(()) => done += 1,
                                // 1223 = 用户在那句「永久删除」警告里点了"否"。
                                Err(1223) => cancelled += 1,
                                Err(c) => {
                                    failed += 1;
                                    eprintln!("批量面板：回收一项失败（系统返回 {c}）");
                                }
                            }
                        }
                        self.refresh_path_states(true);
                        self.marked.clear();
                        eprintln!(
                            "全部回收站：{total} 项 → 成功 {done}、失败 {failed}、取消 {cancelled}（路径不落日志）"
                        );
                        self.close_panel(ui.ctx(), "全部回收站", true);
                        let (msg, color) = if failed > 0 {
                            (
                                format!("⚠ {total} 项里 {failed} 项没能移入回收站"),
                                TOAST_ERR,
                            )
                        } else if cancelled > 0 {
                            (
                                format!("已移入回收站 {done} 项，{cancelled} 项被取消"),
                                TOAST_OK,
                            )
                        } else {
                            (
                                format!("✓ 已把 {done} 项移入回收站（可从回收站还原）"),
                                TOAST_OK,
                            )
                        };
                        self.toast = Some((msg, Instant::now() + TOAST_TTL, color));
                        ui.ctx().request_repaint_after(TOAST_TTL);
                        self.focus_query = true;
                    }
                }
                BatchRecycleResult::No => {
                    eprintln!("全部回收站：按「取消」，未删除");
                    self.batch_confirm = None;
                    self.focus_query = true;
                    ui.ctx().request_repaint();
                }
                BatchRecycleResult::Keep => {}
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
                            "重置空格执行 / Tab 语义 / 学习排序 / 数据源开关 / 危险操作确认 / 终端 shell / 主题 / 显示条数 / 行密度 / 缺省分类范围 / 历史上限。热键、网页引擎、隐藏条目、自定义命令与置顶都会保留。落盘前先备份 settings.json。",
                        ),
                        Destructive::ClearHistory => (
                            "清空搜索历史？",
                            "清空 (Enter)",
                            "把 search_history.json 清成空表：Ctrl+↑ 从空表翻起，你敲过的那些串不再能回填。历史上限那一格不动。这一条不写备份（另两个按钮先备份，是因为它们清的是统计；这张表要清的是原文）。",
                        ),
                        Destructive::ClearUsage => (
                            "清除使用统计？",
                            "清除 (Enter)",
                            "把 usage.json 清成空表：合计、近 30 天柱状与「按功能次数」全部归零。条目上的累计次数与最近使用时间不动（那是排序用的，清它去「重置学习排序」）。这一条不写备份。",
                        ),
                        Destructive::ClearParamHistory => (
                            "清空参数历史？",
                            "清空 (Enter)",
                            "把 param_history.json 清成空表：带参项的参数框下面那列「最近用过的参数」会变空。这一条不写备份。",
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
                        Destructive::ClearHistory => {
                            let n = self.history.len();
                            self.history.clear();
                            // 翻页索引必须跟着归零：表空了还留着"正在第 3 条"，
                            // 下一次 Ctrl+↑ 就会拿越界的下标去问一张空表。
                            self.history_browse = Default::default();
                            if self.history_blocked {
                                // 与 remember_history 同一道闸：那份坏文件还挡在原位，
                                // 清空的写盘正好会把唯一证据覆盖掉——宁可只在内存里清。
                                self.toast = Some((
                                    format!(
                                        "⚠ 内存里 {n} 条已清，但那份坏文件没动（留证没成功 ⇒ 本次启动不写 search_history.json）"
                                    ),
                                    Instant::now() + TOAST_TTL,
                                    TOAST_ERR,
                                ));
                                eprintln!(
                                    "高级页：清空搜索历史 → 内存里 {n} 条已清；坏文件留证没成功 ⇒ 不写盘"
                                );
                            } else {
                                source::application::save_history(&self.history);
                                self.toast = Some((
                                    format!("✓ 已清空搜索历史（{n} 条）"),
                                    Instant::now() + TOAST_TTL,
                                    TOAST_OK,
                                ));
                                eprintln!(
                                    "高级页：清空搜索历史 → {n} 条已清（search_history.json 写成空表，不备份）"
                                );
                            }
                            ui.ctx().request_repaint_after(TOAST_TTL);
                        }
                        Destructive::ClearUsage => {
                            let n = self.usage.total();
                            let days = self.usage.days_recorded();
                            self.usage.clear();
                            if self.usage_blocked {
                                // 与 `record_usage` 同一道闸：坏文件还挡在原位，清空的写盘
                                // 正好会把唯一证据覆盖掉 ⇒ 只在内存里清。
                                self.toast = Some((
                                    format!(
                                        "⚠ 内存里已清（{n} 次 / {days} 天），但那份坏文件没动（留证没成功 ⇒ 本次启动不写 usage.json）"
                                    ),
                                    Instant::now() + TOAST_TTL,
                                    TOAST_ERR,
                                ));
                                eprintln!(
                                    "设置页：清除使用统计 → 内存里已清（{n} 次 / {days} 天）；坏文件留证没成功 ⇒ 不写盘"
                                );
                            } else {
                                source::application::save_usage(&self.usage);
                                self.toast = Some((
                                    format!("✓ 已清除使用统计（{n} 次 / {days} 天）"),
                                    Instant::now() + TOAST_TTL,
                                    TOAST_OK,
                                ));
                                eprintln!(
                                    "设置页：清除使用统计 → {n} 次 / {days} 天已清（usage.json 写成空表，不备份；条目 use_count 未动）"
                                );
                            }
                            ui.ctx().request_repaint_after(TOAST_TTL);
                        }
                        Destructive::ClearParamHistory => {
                            let n = self.param_history.len();
                            self.param_history.clear();
                            if self.param_history_blocked {
                                // 与 `confirm_param_prompt` 的写盘同一道闸：坏文件还挡在原位，
                                // 清空的写盘正好会把唯一证据覆盖掉 ⇒ 只在内存里清。
                                self.toast = Some((
                                    format!(
                                        "⚠ 内存里 {n} 条已清，但那份坏文件没动（留证没成功 ⇒ 本次启动不写 param_history.json）"
                                    ),
                                    Instant::now() + TOAST_TTL,
                                    TOAST_ERR,
                                ));
                                eprintln!(
                                    "高级页：清空参数历史 → 内存里 {n} 条已清；坏文件留证没成功 ⇒ 不写盘"
                                );
                            } else {
                                source::application::save_param_history(&self.param_history);
                                self.toast = Some((
                                    format!("✓ 已清空参数历史（{n} 条）"),
                                    Instant::now() + TOAST_TTL,
                                    TOAST_OK,
                                ));
                                eprintln!(
                                    "高级页：清空参数历史 → {n} 条已清（param_history.json 写成空表，不备份）"
                                );
                            }
                            ui.ctx().request_repaint_after(TOAST_TTL);
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

        // ---- 批次 D5（FR-ACT-09）：「批量管理」页的两种确认（转换 / 删除）----
        // 与 F7 那两个破坏性动作同形态：这一帧只问，按「确定」那一帧才落盘；Esc 由下方全局分段链处理。
        // 计划在点按钮那一刻就算好存进来了，所以这里不必重算——报出去的条数就是确认框上那一份。
        if let Some(pending) = self.manage_confirm.take() {
            enum ManageResult {
                Keep,
                Yes,
                No,
            }
            let (title, confirm_label, warning) = match &pending {
                ManageConfirm::Convert {
                    to_relative,
                    changes,
                    skipped,
                } => {
                    let exe_dir = std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|d| d.display().to_string()))
                        .unwrap_or_default();
                    let how = if *to_relative {
                        format!(
                            "把命令行写法改成 `{}\\…`（本程序目录：{exe_dir}）",
                            source::manage::LAUNCHER_DIR
                        )
                    } else {
                        format!("把 `{}\\…` 还原成 {exe_dir} 下的绝对路径", source::manage::LAUNCHER_DIR)
                    };
                    (
                        format!(
                            "把 {} 条路径改成{}？",
                            changes.len(),
                            if *to_relative { "相对写法" } else { "绝对路径" }
                        ),
                        "转换 (Enter)".to_string(),
                        format!(
                            "{how}。\n另有 {skipped} 条选中但不动：网址 / 内置动作 / 含 {{query}} / 不在本程序目录下 / 已经是目标形态。\n只改命令行这一格（路径），参数、工作目录、别名、置顶与学习统计都原样保留；提交后立刻写盘。"
                        ),
                    )
                }
                ManageConfirm::Delete { uids } => (
                    format!("从清单里删除这 {} 条？", uids.len()),
                    format!("删除 {} 条 (Enter)", uids.len()),
                    "只把它们从 user_items.json 里移除，不碰磁盘上的文件。\n提交即写盘；要找回只能手工编辑清单或重新导入。"
                        .to_string(),
                ),
            };
            let modal = egui::Modal::new(egui::Id::new("manage_confirm")).show(
                ui.ctx(),
                |ui| {
                    ui.heading(title);
                    ui.separator();
                    ui.label(egui::RichText::new(warning).weak().small());
                    let mut result = ManageResult::Keep;
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        result = ManageResult::Yes;
                    }
                    ui.horizontal(|ui| {
                        if ui.button(confirm_label).clicked() {
                            result = ManageResult::Yes;
                        }
                        if ui.button("取消 (Esc)").clicked() {
                            result = ManageResult::No;
                        }
                    });
                    result
                },
            );
            match modal.inner {
                ManageResult::Yes => match pending {
                    ManageConfirm::Convert {
                        to_relative,
                        changes,
                        skipped,
                    } => {
                        let mut applied = 0usize;
                        for (uid, new_path) in &changes {
                            if let Some(pos) = self.items.iter().position(|it| it.uid() == *uid) {
                                self.items[pos].path = new_path.clone();
                                applied += 1;
                            }
                        }
                        if applied > 0 {
                            self.save_items();
                            self.selected = 0;
                            self.manage_checked.clear();
                            self.refresh_path_states(true);
                            self.toast = Some((
                                format!(
                                    "✓ {applied} 条改为{}（{} 条跳过）",
                                    if to_relative { "相对写法" } else { "绝对路径" },
                                    skipped
                                ),
                                Instant::now() + TOAST_TTL,
                                TOAST_OK,
                            ));
                            ui.ctx().request_repaint_after(TOAST_TTL);
                            eprintln!(
                                "批量管理：转{} → {applied} 条，跳过 {skipped}（路径不落日志）",
                                if to_relative { "相对写法" } else { "绝对路径" }
                            );
                        } else {
                            // 计划落好之后清单又被别处改过（IPC 导入 / 编辑）。宁可报"没改成"，
                            // 也不把"改了多少"说成一个自己没验证过的数。
                            self.toast = Some((
                                "⚠ 清单已变，这次一条也没改成".to_string(),
                                Instant::now() + TOAST_TTL,
                                TOAST_ERR,
                            ));
                            ui.ctx().request_repaint_after(TOAST_TTL);
                            eprintln!("批量管理：转换落空（计划里的条目已不在清单，未写盘）");
                        }
                    }
                    ManageConfirm::Delete { uids } => {
                        let before = self.items.len();
                        self.items.retain(|it| !uids.contains(&it.uid()));
                        let removed = before - self.items.len();
                        if removed > 0 {
                            self.selected = 0;
                            self.save_items();
                            self.manage_checked.clear();
                            self.refresh_path_states(true);
                            self.toast = Some((
                                format!("✓ 已从清单删除 {removed} 条（未删磁盘文件）"),
                                Instant::now() + TOAST_TTL,
                                TOAST_OK,
                            ));
                            ui.ctx().request_repaint_after(TOAST_TTL);
                            eprintln!("批量管理：删除 → {removed} 条条目（不删磁盘文件，uid 不落日志）");
                        } else {
                            self.toast = Some((
                                "⚠ 清单已变，这次一条也没删".to_string(),
                                Instant::now() + TOAST_TTL,
                                TOAST_ERR,
                            ));
                            ui.ctx().request_repaint_after(TOAST_TTL);
                            eprintln!("批量管理：删除落空（计划里的条目已不在清单）");
                        }
                    }
                },
                ManageResult::No => {
                    eprintln!("批量管理确认：按「取消」，未执行");
                    ui.ctx().request_repaint();
                }
                ManageResult::Keep => self.manage_confirm = Some(pending),
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
            // 批次 D8：自定义命令源（`{query}`）的条目不在清单里，`mark_used` 会在
            // "这条不在清单里就返回"那一步跳过 ⇒ 它的使用次数得在这儿单独记。
            // 判据用 `launch_uid` 是否被设过：三个分派点（数字键 / 回车 / 点击）只有
            // `HeadRow::Custom` 那一支会设它；清单行走的是 `entry.uid()` 现算。
            let from_custom = launch_uid.is_some();
            let alias_word = commit_word(&self.query, launch_via_digit);
            // 批次 B6（FR-ACT-07）：要参数的那一档**先弹框**，确认那一帧才启动
            //（`confirm_param_prompt` 里再记账）。搜索历史也顺延到确认那一帧——
            // Esc 取消掉的那一按不该算"提交过"。
            if !self.param_prompt_if_needed(
                &entry,
                &uid,
                launch_from_list,
                alias_word.clone(),
                from_custom,
            ) {
                // FR-QRY-10（批次 C3）：提交了就记一条，**不等启动结果**——AHK `Knowledge.Record`
                // 也是先记账再 `_SafeRun`。理由在用户那侧：启动失败时他要的是改两个字再试一次，
                // 框里那串正是得留着的东西；而记完不会挡任何事（不写学习统计、不改清单）。
                // 空查询（呼出后直接点列表）由 `push_history` 自己跳过。
                self.remember_history(self.query.clone());
                // 批次 B6：写了标记的条目先展开（`{%c}` / `{%wt}` …），展开过就走脱敏日志。
                let (entry, expanded) = self.expand_for_launch(&entry, "");
                let started = if expanded || launch_masked {
                    self.launch_and_hide_masked(ui.ctx(), &entry, &uid)
                } else {
                    self.launch_and_hide(ui.ctx(), &entry)
                };
                if started && launch_from_list {
                    // FR-SRC-19 补全「提交即学别名」（口径与门槛见 learn_alias）。
                    self.mark_used(&uid, &alias_word);
                }
                if started && from_custom {
                    self.record_usage(source::usage::Bucket::Custom);
                }
            }
        }

        // FR-SRC-11：搜索行的「执行」＝用默认浏览器打开 + 关到托盘。与上两段互斥：
        // 头段一次只会产生其中一个动作（计算器复制 / 搜索打开 / 条目启动）。
        if let Some(hit) = open_hit {
            self.remember_history(self.query.clone());
            // 批次 D8：真打开了才记一次（与 `mark_used` 的"成功才累计"同一口径）。
            if self.open_search_hit(ui.ctx(), &hit) {
                self.record_usage(source::usage::Bucket::Web);
            }
        }

        // 批次 X：文件行的三个动作（打开 / 在资源管理器中定位 / 复制路径）。与上面三段互斥：
        // 一次按键或点击只会产出其中一个。
        if let Some(hit) = open_file {
            // 批次 C3：这一条最该进历史——他能敲到这里，说明这串路径是现打的。
            self.remember_history(self.query.clone());
            if self.open_file_hit(ui.ctx(), &hit) {
                self.record_usage(source::usage::Bucket::File);
            }
        }
        if let Some(hit) = open_term {
            // 批次 C3：`>` 那条命令同样不进清单（没有 uid 可学），历史上却是他最想找回的一串。
            self.remember_history(self.query.clone());
            if self.run_terminal_hit(ui.ctx(), &hit) {
                self.record_usage(source::usage::Bucket::Terminal);
            }
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
            // 批次 C3：算式也是"敲进去又提交出去的一串"（AHK 给 `text` 动作照样记账，同口径）。
            self.remember_history(self.query.clone());
            match copy_to_clipboard(&text) {
                Ok(()) => {
                    // 表达式原文不落日志（FR-SYS-03 隐私口径）⇒ 只记长度与成败。
                    eprintln!(
                        "计算器：结果已复制（{} 个字符，表达式原文不落日志）→ 关闭到托盘",
                        text.chars().count()
                    );
                    // 批次 D8：复制成功才算用了一次（与另外三类执行同一口径）。
                    self.record_usage(source::usage::Bucket::Calc);
                    // FR-UIX-09：主窗随即关到托盘，所以回执只能靠 HUD 送到窗口外。
                    let msg = format!("✓ 已复制 {text}");
                    self.toast = Some((msg.clone(), Instant::now() + TOAST_TTL, TOAST_OK));
                    self.hud.notify(
                        ui.ctx(),
                        msg,
                        &self.theme,
                        hud::HOLD_FOREGROUND,
                        work_for(self.prefs.appearance.show_on),
                    );
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

        // FR-ACT-03：右键「操作…」入口。放在面板动作之前：同一帧里两件事不会同时发生
        // （结果模式的右键槽和面板里的动作槽互斥），这个顺序只是让"先开框、后动作"读起来顺。
        if let Some(row) = panel_request {
            self.open_panel(ui.ctx(), row);
        }

        // 批次 P1（FR-ACT-03 / FR-ACT-04）：执行面板里选中的那条动作。放在其它帧末槽之后：
        // 它会改 `self.panel` / `self.query` / `self.dialog`，而这些下面 Esc 那一串都要读——
        // 顺序颠倒的话，同帧的 Esc 会对着一个已经变化一次的现场再做一次"退出"。
        if let Some(id) = panel_action {
            self.run_panel_action(ui.ctx(), id);
        }

        // Esc 分段（FR-ACT-10「清空后隐藏」+2026-10-17 设置页一档 +2026-10-18 删除确认一档
        // +2026-10-24 批次 F6 菜单一档 +2026-10-25 批次 P1 操作面板一档）：
        // 菜单→只收菜单；确认框→关框；对话框→关框；设置页→回搜索；面板→回结果并还原现场；
        // 目录→上一级；有字→清空；全空→关闭到托盘。
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
            } else if self.manage_confirm.is_some() {
                // 批次 D5：批量管理的确认框。只收框，设置页与勾选都留着（用户想改选再试）。
                eprintln!("批量管理确认：按 Esc 取消，未执行");
                self.manage_confirm = None;
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
            } else if self.batch_confirm.is_some() {
                eprintln!("全部回收站确认：按 Esc 取消，未删除");
                self.batch_confirm = None;
                self.focus_query = true;
                ui.ctx().request_repaint();
            } else if self.confirm.is_some() {
                eprintln!("删除确认：按 Esc 取消，未删除");
                self.confirm = None;
                self.focus_query = true;
                ui.ctx().request_repaint();
            } else if self.param_prompt.is_some() {
                // 批次 B6：只收参数框，查询与选中都不动（用户可能想换个参数再来一次）。
                self.param_prompt = None;
                self.focus_query = true;
                eprintln!("参数框：按 Esc 取消，未启动（参数原文不落日志）");
                ui.ctx().request_repaint();
            } else if self.dialog.as_ref().is_some_and(|d| d.fill_prompt.is_some()) {
                // 批次 D4：替换询问那一屏，Esc 只收询问、不关弹窗——这里按 Esc 的意思是
                // "这次导入别换别名"，连整框一起关就把刚选的值和已填的内容全丢了。
                if let Some(dg) = self.dialog.as_mut() {
                    dg.fill_prompt = None;
                }
                eprintln!("弹窗：按 Esc 收起「要不要替换」询问，弹窗保留（内容不落日志）");
                ui.ctx().request_repaint();
            } else if self.dialog.is_some() {
                self.dialog = None;
                self.focus_query = true; // Esc 关框后焦点同样还给搜索框
                ui.ctx().request_repaint();
            } else if self.settings_open {
                self.settings_open = false;
                self.manage_checked.clear();
                // 批次 D2：Esc 关设置页同样要把编辑缓冲回写（见 [`finish_settings_edits`]）。
                self.finish_settings_edits();
                self.focus_query = true;
                ui.ctx().request_repaint();
            } else if self.panel.is_some() {
                // 批次 P1（FR-ACT-03）：面板开着时 Esc 关面板并把原查询与选中行放回去。
                // 排在「Esc 返回上级目录」与「有字→清空」**之前**是有意的：过滤词长得像路径
                // （`D:\temp`）时那两档会抢走这一下，结果是面板还开着、查询已被换成父目录——
                // 两个模式同时被搅，比没反应更糟。
                self.close_panel(ui.ctx(), "Esc", true);
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
        // FR-UIX-10：大字显示同理——声明在、回调活着；主窗即使被热键藏起来，
        // 已声明的大字视口也由它自己的回调续命。
        self.large_type.render(ui.ctx());
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
    // 所以这里读一次 settings.json。批次 R 起这一次读还要带出补字符热键。
    // 批次 E2 起这是**启动路径上唯一一次**读设置（另一处 `reload_config` 是用户手动"重新加载配置"，
    // 不在启动链上）：读到的值与故障一起传进 `LauncherApp::new`。
    // 以前下面还会读第二遍，而这一次读顺手把坏文件改名隔离了 ⇒ 第二遍只看见"文件不存在"，
    // FR-CFG-02 那句界面提示因此从不出现（类型不符的降级日志反倒一次启动打两遍）。
    let (configured, broken) = source::application::load_settings();
    match &broken {
        Some(Corrupted::Kept(p)) => eprintln!(
            "配置损坏：热键线程用的 settings.json 解析失败，已隔离为 {}，本轮按出厂热键注册",
            p.display()
        ),
        // 这一次读自己不写盘，所以 `Lost` 在这里不需要停手；"留证没成功就不写"那道门禁吃的是
        // 下面传进 `LauncherApp::new` 的这份 `broken`，门禁本身没有因为少了一次读而变空。
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
        configured.extra_hotkey,
        configured.dbl_tap,
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

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([start_width, start_height])
        .with_title("")
        .with_decorations(false)
        .with_always_on_top()
        .with_transparent(true)
        .with_visible(start_visible);
    // 批次 F8（FR-UIX-03 RememberPosition）的位置改由批次 F3 接管：`settings.json` 里存的是
    // 工作区千分比，得等窗口真的存在才能按 `show_on` 选屏摆位（见 `LauncherApp::new` 末尾
    // 的首次落点）。这里不再给 `with_position`——先用系统默认位置建窗，首帧前就被摆好。

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
                configured,
                broken,
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

    /// 批次 D5：行内路径截断按字符算——按字节切会在多字节字符中间下刀，
    /// 而且"多少字"对用户来说就是字符数，不是字节数。
    #[test]
    fn shorten_path_cuts_on_char_boundaries() {
        assert_eq!(shorten_path(r"C:\a.exe", 64), r"C:\a.exe");
        let long = format!(r"C:\{}\x.exe", "工具".repeat(40));
        let got = shorten_path(&long, 10);
        assert_eq!(got.chars().count(), 10);
        assert!(got.ends_with('…'), "截断了要看得出来：{got}");
        assert!(got.starts_with(r"C:\工"));
    }

    /// 批次 B6（FR-ACT-07）：编辑弹窗把「编码」带进带出。用户改的是那个下拉，
    /// 提交时必须落回 `LaunchItem.param_type`——`to_item` 漏这一格的话，
    /// 界面上改完、存盘回来还是「无参数」，光看界面看不出来。
    #[test]
    fn dialog_round_trips_param_type() {
        let mut it = LaunchItem {
            name: "百度".to_string(),
            path: "https://www.baidu.com/s?wd=".to_string(),
            ..Default::default()
        };
        it.param_type = source::params::ParamType::UrlQuery;
        let dg = DialogState::from_item(&it);
        assert_eq!(dg.param_type, source::params::ParamType::UrlQuery);
        assert_eq!(dg.to_item().param_type, source::params::ParamType::UrlQuery);
    }

    /// 批次 B6：「有标记但编码=无参数」只在**真的不会展开**时追问。
    #[test]
    fn marker_warning_only_for_literal_markers_without_encoding() {
        let mut dg = DialogState::blank();
        dg.path = "notepad.exe".to_string();
        dg.arguments = "{%c}".to_string();
        assert!(dg.marker_warn_needed(), "有标记 + 编码=无参数 ⇒ 要追问");
        dg.param_type = source::params::ParamType::NoEncoding;
        assert!(!dg.marker_warn_needed(), "编码非空会真的替换 ⇒ 不追问");
        dg.param_type = source::params::ParamType::None;
        dg.arguments.clear();
        assert!(!dg.marker_warn_needed(), "没有标记 ⇒ 不追问");
    }

    /// 批次 B6：列表行的 `*` 只标"会替换参数"的那一档（编码非空）。
    /// 编码为空时标记根本不展开，标 `*` 反而是误导。
    #[test]
    fn row_mark_follows_param_type_not_markers() {
        let mut it = LaunchItem {
            name: "x".to_string(),
            path: "p".to_string(),
            ..Default::default()
        };
        it.arguments = "{%c}".to_string();
        assert_eq!(param_row_mark(&it), "");
        it.param_type = source::params::ParamType::Utf8Query;
        assert_eq!(param_row_mark(&it), "* ");
    }

    /// 批次 D4（FR-CFG-06）：两条最容易走样的规则钉在这里——①只有"已填了别的值"才问；
    /// ②答「保留」时只保别名/名称，命令行那三格照换（它们是"这条命令到底是什么"）。
    fn d4_fill(path: &str, keyword: &str, name: &str, kind: ItemKind) -> source::import::ImportFill {
        source::import::ImportFill {
            path: path.to_string(),
            arguments: "arg".to_string(),
            working_dir: r"C:\wd".to_string(),
            keyword: keyword.to_string(),
            name: name.to_string(),
            kind,
        }
    }

    #[test]
    fn fill_prompt_only_asks_when_names_differ() {
        let mut dg = DialogState::blank();
        assert!(make_fill_prompt(&dg, &d4_fill("a", "k", "n", ItemKind::App)).is_none());

        dg.keyword = "old".to_string();
        dg.name = "n".to_string();
        let p = make_fill_prompt(&dg, &d4_fill("a", "k", "n", ItemKind::App))
            .expect("别名已填别的值，必须问");
        assert_eq!(p.keyword_old.as_deref(), Some("old"));
        assert!(p.name_old.is_none(), "名称一样就不该出现在询问里");

        // 值相同（含前后空白）不算冲突。
        dg.keyword = "  k ".to_string();
        assert!(make_fill_prompt(&dg, &d4_fill("a", "k", "n", ItemKind::App)).is_none());
    }

    #[test]
    fn apply_fill_keep_only_spares_the_names() {
        let mut dg = DialogState::blank();
        dg.keyword = "old".to_string();
        dg.name = "oldname".to_string();
        let fill = d4_fill(r"C:\x.exe", "k", "n", ItemKind::App);
        apply_fill(&mut dg, &fill, false);
        assert_eq!((dg.keyword.as_str(), dg.name.as_str()), ("old", "oldname"));
        assert_eq!(dg.path, r"C:\x.exe", "命令行不跟着名字走");
        assert_eq!(dg.arguments, "arg");
        assert_eq!(dg.working_dir, r"C:\wd");

        // 答「替换」才动名字；空着的格无论答什么都要补上。
        let mut dg2 = DialogState::blank();
        apply_fill(&mut dg2, &fill, false);
        assert_eq!((dg2.keyword.as_str(), dg2.name.as_str()), ("k", "n"));
        let mut dg3 = DialogState::blank();
        dg3.keyword = "old".to_string();
        apply_fill(&mut dg3, &fill, true);
        assert_eq!(dg3.keyword, "k");
    }

    #[test]
    fn apply_fill_never_rewrites_internal_kind() {
        let mut dg = DialogState::blank();
        dg.kind = ItemKind::Internal;
        apply_fill(&mut dg, &d4_fill("internal:lock", "k", "n", ItemKind::App), true);
        assert_eq!(
            dg.kind,
            ItemKind::Internal,
            "系统命令的类型被改掉，这条就再也执行不了了"
        );
    }

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

    /// 批次 D7：插入「文件搜索」第 5 页后外观/高级顺延，`-Preferences 5/6` 语义随之变。
    /// 批次 D5：第 8 页「批量管理」追加在末尾，1–7 一格不动。
    /// 批次 D8：第 9 页「使用统计」同样追加在末尾，1–8 一格不动。
    /// 批次 D2：第 10 页「网页引擎」同样追加在末尾，1–9 一格不动。
    /// 这条钉住 1..=10 的整张映射——只测旧值漂没漂是不够的，页号是线性协议，两端都得钉。
    #[test]
    fn settings_tab_page_mapping_covers_one_through_ten() {
        assert_eq!(SettingsTab::from_page(1), Some(SettingsTab::General));
        assert_eq!(SettingsTab::from_page(2), Some(SettingsTab::Features));
        assert_eq!(SettingsTab::from_page(3), Some(SettingsTab::Hotkeys));
        assert_eq!(SettingsTab::from_page(4), Some(SettingsTab::Sources));
        assert_eq!(SettingsTab::from_page(5), Some(SettingsTab::FileSearch));
        assert_eq!(SettingsTab::from_page(6), Some(SettingsTab::Appearance));
        assert_eq!(SettingsTab::from_page(7), Some(SettingsTab::Advanced));
        assert_eq!(
            SettingsTab::from_page(8),
            Some(SettingsTab::Manage),
            "批次 D5：第 8 页是「批量管理」"
        );
        assert_eq!(
            SettingsTab::from_page(9),
            Some(SettingsTab::Usage),
            "批次 D8：第 9 页是「使用统计」"
        );
        assert_eq!(
            SettingsTab::from_page(10),
            Some(SettingsTab::WebEngines),
            "批次 D2：第 10 页是「网页引擎」"
        );
        assert_eq!(SettingsTab::from_page(11), None, "越界要给 None，调用方落回通用");
        assert_eq!(SettingsTab::from_page(0), None);
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
        // 批次 P1（FR-ACT-03）新加的那一档排在最前：面板开着时，就算过滤词长得像目录、
        // 也像文件模式，这一格说的仍然是「怎么退出面板」——Esc 的真实落点只有这一个。
        assert_eq!(
            status_left_text(true, Some(("D:\\a\\b\\", "")), true, TabMode::Complete, true),
            "操作面板 · Esc 或 ← 返回结果"
        );
        // 目录浏览三档 Esc 文案各说各的真实落点：退目录 / 去过滤词 / 盘根清空。
        assert_eq!(
            status_left_text(
                false,
                Some(("D:\\a\\b\\", "")),
                false,
                TabMode::Navigate,
                true
            ),
            "目录 D:\\a\\b\\ · Esc 返回上级"
        );
        assert_eq!(
            status_left_text(false, Some(("D:\\a\\b\\", "re")), false, TabMode::Complete, false),
            "目录 D:\\a\\b\\ · Esc 清除筛选"
        );
        assert_eq!(
            status_left_text(false, Some(("D:\\", "")), false, TabMode::Complete, false),
            "目录 D:\\ · Esc 清空"
        );
        assert_eq!(
            status_left_text(false, None, true, TabMode::Complete, true),
            "文件模式 · Esc 清空"
        );
        // 平时两档 Tab 口径，各自原样出场；「基本」只在平时那一档追加。
        assert_eq!(
            status_left_text(false, None, false, TabMode::Complete, false),
            "Tab 补全 / 下一项"
        );
        assert_eq!(
            status_left_text(false, None, false, TabMode::Complete, true),
            "Tab 补全 / 下一项 · 分类：基本"
        );
        assert_eq!(
            status_left_text(false, None, false, TabMode::Navigate, false),
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
        // 批次 P1：面板行的 Enter 词 = 那条动作的行首徽标（「Enter 复制」「Enter 回收」）。
        let action = HeadRow::Action(actions::Action {
            id: actions::ActionId::Recycle,
            label: actions::ActionId::Recycle.label().to_string(),
            subtitle: String::new(),
            keys: "",
        });
        assert_eq!(enter_action_label(Some(&action)), "回收");
    }

    /// FR-ACT-05 的可标记判据：需求书写「仅 `file`/`folder` 且 `arg≠""`」。
    /// `arg` 在 AHK 里是文件/目录条目的路径，本程序就是 `path`；所以判据＝
    /// 「是文件/目录 且 路径非空」。**别把应用/命令条目放进来**——批量动作全是文件系统动作。
    #[test]
    fn insert_marks_only_rows_that_name_a_file_or_folder() {
        let items = [
            // 0：文件条目
            LaunchItem {
                name: "a.txt".into(),
                path: "D:\\a.txt".into(),
                kind: ItemKind::File,
                ..Default::default()
            },
            // 1：目录条目
            LaunchItem {
                name: "docs".into(),
                path: "D:\\docs".into(),
                kind: ItemKind::Folder,
                ..Default::default()
            },
            // 2：应用条目——路径是 exe，但不是"文件/目录"这一类的清单条目
            LaunchItem {
                name: "记事本".into(),
                path: "notepad.exe".into(),
                kind: ItemKind::App,
                ..Default::default()
            },
            // 3：文件条目但路径为空（畸形数据）⇒ 不能标记
            LaunchItem {
                name: "空路径".into(),
                path: "   ".into(),
                kind: ItemKind::File,
                ..Default::default()
            },
        ];
        let results = [0usize, 1, 2, 3];
        let head = [
            HeadRow::File(source::filesearch::FileHit {
                title: "b.txt".into(),
                path: "D:\\b.txt".into(),
                is_dir: false,
                subtitle: String::new(),
            }),
            HeadRow::Calc(source::calc::CalcHit {
                title: "= 2".into(),
                subtitle: String::new(),
                copy: "2".into(),
            }),
        ];
        // 文件行按原路径返回（不带引号、不归一）。
        assert_eq!(
            markable_path(Row::Head(0), &head, &results, &items).as_deref(),
            Some("D:\\b.txt")
        );
        // 计算器行不是文件/目录。
        assert_eq!(markable_path(Row::Head(1), &head, &results, &items), None);
        assert_eq!(
            markable_path(Row::Item(0), &head, &results, &items).as_deref(),
            Some("D:\\a.txt")
        );
        assert_eq!(
            markable_path(Row::Item(1), &head, &results, &items).as_deref(),
            Some("D:\\docs")
        );
        assert_eq!(markable_path(Row::Item(2), &head, &results, &items), None);
        assert_eq!(markable_path(Row::Item(3), &head, &results, &items), None);
        // 名次越界 ⇒ None，不 panic（选中行与计划不同帧时宁可什么都不做）。
        assert_eq!(markable_path(Row::Item(9), &head, &results, &items), None);
        assert_eq!(markable_path(Row::Head(9), &head, &results, &items), None);
    }

    /// `Insert` 的切换口径：大小写不敏感（Windows 路径）、重复标记＝取消、次序按首次加入。
    #[test]
    fn marking_toggles_case_insensitively_and_keeps_insertion_order() {
        let mut marked: Vec<String> = Vec::new();
        assert!(toggle_mark(&mut marked, "D:\\A.txt"));
        assert!(is_marked(&marked, "d:\\a.txt"));
        assert!(toggle_mark(&mut marked, "D:\\b.txt"));
        assert!(is_marked(&marked, "d:\\A.TXT"));
        assert_eq!(marked, vec!["D:\\A.txt".to_string(), "D:\\b.txt".to_string()]);
        // 同一条路径换个大小写再按一次 Insert ＝ 取消，且不改变其余条目的次序。
        assert!(!toggle_mark(&mut marked, "d:\\a.TXT"));
        assert!(!is_marked(&marked, "D:\\A.txt"));
        assert_eq!(marked, vec!["D:\\b.txt".to_string()]);
        assert_eq!(marked_hint(1), "已标记 1 项 · Insert 取消 · → 批量操作");
    }

    /// `CF_HDROP` 的字节布局。资源管理器对这块缓冲区没有任何容错余地——`pFiles` 差 1
    /// 就整片读空、`fWide` 错就按 ANSI 解释中文路径、末尾少一个 NUL 就把后面那块内存当路径，
    /// 所以逐段钉住：头长、偏移、`fWide`、UTF-16 小端、每条路径各自 NUL 尾 + 整体再补一个。
    #[test]
    fn dropfiles_layout_matches_the_cf_hdrop_contract() {
        let paths = vec!["D:\\a.txt".to_string(), "D:\\中文 目录".to_string()];
        let data = build_dropfiles(&paths);
        let header = std::mem::size_of::<DROPFILES>();

        // ① 头：pFiles 必须指向文件表起点（＝头长），pt 归零，fNC=0，fWide=1。
        assert_eq!(
            u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize,
            header
        );
        let f_wide_off = header - 4;
        assert_eq!(
            u32::from_le_bytes(data[f_wide_off..f_wide_off + 4].try_into().unwrap()),
            1,
            "fWide 必须是 1，否则中文路径按 ANSI 解释"
        );
        let f_nc_off = header - 8;
        assert_eq!(
            u32::from_le_bytes(data[f_nc_off..f_nc_off + 4].try_into().unwrap()),
            0
        );
        let pt_y_off = header - 8 - 8;
        for off in [pt_y_off, pt_y_off + 4] {
            assert_eq!(
                i32::from_le_bytes(data[off..off + 4].try_into().unwrap()),
                0
            );
        }
        // 结构体自己也得认同一套偏移（下面用真实结构体再对比一次，改 DROPFILES 导入不会静默错位）。
        let head = DROPFILES {
            pFiles: header as u32,
            pt: POINT { x: 0, y: 0 },
            fNC: false.into(),
            fWide: true.into(),
        };
        // SAFETY：`head` 是就地初始化的 `#[repr(C)]` 结构，只按字节读。
        let head_bytes = unsafe {
            std::slice::from_raw_parts((&head as *const DROPFILES).cast::<u8>(), header)
        };
        assert_eq!(&data[..header], head_bytes);

        // ② 文件表：UTF-16LE，每条路径各自以 NUL 结尾，整块末尾再一个 NUL。
        let mut expect: Vec<u8> = Vec::new();
        for p in &paths {
            for unit in p.encode_utf16() {
                expect.extend_from_slice(&unit.to_le_bytes());
            }
            expect.extend_from_slice(&0u16.to_le_bytes());
        }
        expect.extend_from_slice(&0u16.to_le_bytes());
        assert_eq!(&data[header..], &expect[..]);

        // ③ 反解一遍：按 `pFiles` 偏移读双 NUL 结尾的宽串表，得回原路径与原次序。
        let table = &data[header..];
        let units: Vec<u16> = table
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let mut got: Vec<String> = Vec::new();
        let mut cur = String::new();
        for u in units {
            if u == 0 {
                if cur.is_empty() {
                    break;
                }
                got.push(std::mem::take(&mut cur));
            } else {
                cur.push(char::from_u32(u as u32).unwrap());
            }
        }
        assert_eq!(got, paths);

        // ④ 空清单：头 + 一个 NUL 结尾符，仍然是一块结构上合法的缓冲区（不会越界读）。
        let empty = build_dropfiles(&[]);
        assert_eq!(empty.len(), header + 2);
        assert_eq!(&empty[header..], &[0u8, 0u8]);
    }

    /// 「复制到」的第一道闸：把文件夹复制进它自己（或它的子孙）会被系统拒绝，更糟的是
    /// 资源管理器可能开始递归复制。逐段比前缀是为了不把 `D:\ab` 当成 `D:\a` 的子目录。
    #[test]
    fn copy_to_skips_sources_that_live_at_or_under_the_destination() {
        assert!(same_or_under("D:\\a", "D:\\a"));
        assert!(same_or_under("D:\\a", "D:\\A\\")); // 尾分隔符与大小写都不计
        assert!(same_or_under("D:\\a\\b", "D:\\a"));
        assert!(same_or_under("\\\\nas\\share\\proj\\x", "\\\\nas\\share\\proj"));
        assert!(!same_or_under("D:\\ab", "D:\\a"));
        assert!(!same_or_under("C:\\x", "D:\\a"));
        assert!(!same_or_under("", "D:\\a"));
        assert!(!same_or_under("D:\\a", ""));
    }

    /// 「全部加入命令」的去重判据与条目生成。判据照 AHK `FindByTarget`：不分大小写、尾分隔符不计；
    /// 条目名称取末段（`.lnk` 后缀去掉），目录按 `is_dir` 归一成 `Folder`。
    #[test]
    fn add_all_matches_targets_and_builds_command_items() {
        assert!(same_target_path("D:\\a.txt", "d:\\A.TXT"));
        assert!(same_target_path("D:\\docs\\", "D:\\docs"));
        assert!(!same_target_path("D:\\a.txt", "D:\\b.txt"));
        // 空路径不参与"已存在"判定（畸形数据不该把正常条目挡掉）。
        assert!(!same_target_path("", ""));
        assert!(!same_target_path("   ", "D:\\a"));

        // 不存在的路径 ⇒ 文件型条目，名称取末段、扩展名保留（与单条「添加为命令」同口径）。
        let ghost = command_item_from_path("D:\\tmp\\报告.docx");
        assert_eq!(ghost.name, "报告.docx");
        assert_eq!(ghost.path, "D:\\tmp\\报告.docx");
        assert_eq!(ghost.kind, ItemKind::File);
        assert_eq!(ghost.source_path, "D:\\tmp\\报告.docx");
        // `.lnk` 后缀去掉（与 `make_item_from_input` 同一格）。
        assert_eq!(
            command_item_from_path("D:\\tmp\\Chrome.lnk").name,
            "Chrome"
        );

        // 真目录 ⇒ 目录型条目（用临时目录，不碰用户数据）。
        let dir = std::env::temp_dir().join("egui-launcher-e4-add-all");
        let _ = std::fs::create_dir_all(&dir);
        let item = command_item_from_path(&dir.display().to_string());
        assert_eq!(item.kind, ItemKind::Folder, "{}", dir.display());
        assert_eq!(item.name, "egui-launcher-e4-add-all");
        let _ = std::fs::remove_dir(&dir);
    }

    /// `→` 进面板、状态栏那枚 `→` 键帽、右键「操作…」三个入口共用这一条判据。
    /// 键帽按了会没反应的话就是误导 ⇒ 「有没有面板可进」必须与 `panel_for_row` 同源，
    /// 所以这里直接测那个函数，不再另写一份"看起来一样"的规则。
    #[test]
    fn only_rows_that_hold_an_object_offer_the_action_panel() {
        let items = [LaunchItem {
            name: "记事本".into(),
            path: "notepad.exe".into(),
            kind: ItemKind::Command,
            ..Default::default()
        }];
        let states = std::collections::HashMap::new();
        let no_panel = |row: Row, head: &[HeadRow]| {
            panel_for_row(row, head, &[0], &items, &states).is_some()
        };
        // 条目行永远有面板（哪怕路径是裸命令名，`exists` 走的是"没扫到当作在"那一格）。
        assert!(no_panel(Row::Item(0), &[]));
        // 名次越界 ⇒ None，不 panic：选中行与计划不同帧时宁可什么都不做。
        assert!(!no_panel(Row::Item(3), &[]));
        for head in [
            HeadRow::Terminal(source::terminal::TermHit {
                shell: source::terminal::TermShell::Cmd,
                command: "ipconfig".into(),
                title: "ipconfig".into(),
                subtitle: String::new(),
            }),
            HeadRow::Hint(source::websearch::EngineHint {
                engine_id: "g".into(),
                title: "搜索「x」".into(),
                subtitle: String::new(),
                completion: "g x".into(),
            }),
            HeadRow::ScopeTags(source::application::ScopeTagHint {
                title: "分类".into(),
                subtitle: String::new(),
            }),
        ] {
            assert!(!no_panel(Row::Head(0), &[head.clone()]), "{head:?} 这一行不该有面板");
        }
        // 文件行与搜索行是"查询现出来的对象"，面板给得了复制/打开，uid 为空 ⇒ 不谈编辑置顶。
        let file = HeadRow::File(source::filesearch::FileHit {
            title: "a.txt".into(),
            path: "D:\\a.txt".into(),
            is_dir: false,
            subtitle: String::new(),
        });
        let (target, open) = panel_for_row(Row::Head(0), &[file], &[], &items, &states)
            .expect("文件行应有面板");
        assert_eq!(target.subject, actions::Subject::File);
        assert!(!target.editable);
        assert!(target.uid.is_none());
        assert!(matches!(open, PanelOpen::Path));

        // 批次 E5：计算器行从上面那份"不该有面板"的名单里搬出来了
        // （原先它是第一项，`title: "= 2"、copy: "2"`）。搬出来的理由不是"它现在能打开了"——
        // 它**还是**打不开，而是它终于有了一张不含「打开」的表：复制这一串 / 粘贴到原窗口 / 大字。
        let calc = HeadRow::Calc(source::calc::CalcHit {
            title: "= 1,024".into(),
            subtitle: String::new(),
            copy: "1,024".into(),
        });
        let (target, open) = panel_for_row(Row::Head(0), &[calc], &[], &items, &states)
            .expect("计算器行应有文本那张表");
        assert_eq!(target.subject, actions::Subject::Text);
        // `path` 里放的是**那一串值**（`copy`，不带等号），标题原样留着只给大字与面板抬头看。
        assert_eq!(target.path, "1,024");
        assert_eq!(target.title, "= 1,024");
        assert!(!target.editable);
        assert!(target.uid.is_none());
        assert!(matches!(open, PanelOpen::Value));
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
        // 【批次 D5】`0x42`（Ctrl+B）原本在这份"没绑"名单里；本批把它绑成"进设置页「批量管理」"
        // 之后从名单里挪走，改成下面的正向断言（与批次 F6 的 Ctrl+M 同一条路径）。
        for vk in [0x41u32, 0x48, 0x5A] {
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
        // 批次 D5（FR-ACT-09）：窗口内绑了就进表，否则用户能把它录成全局键 ⇒ `RegisterHotKey`
        // 整条吞掉 ⇒ 窗口里那颗键静默失效（与批次 B8 的 Ctrl+C 同型代价）。
        for vk in [0x42u32, 0x52] {
            assert!(
                HotkeySpec {
                    mods: HKF_CONTROL | HKF_NOREPEAT,
                    vk
                }
                .conflicts_local(LOCAL_TAKEN),
                "Ctrl+{} 批次 D5 已绑定，必须被本地表挡住",
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
        // FR-UIX-10：Ctrl+L 现在绑了大字显示，与 Ctrl+M 同一口径进表。
        assert!(
            HotkeySpec {
                mods: HKF_CONTROL | HKF_NOREPEAT,
                vk: 0x4C
            }
            .conflicts_local(LOCAL_TAKEN),
            "Ctrl+L 大字显示，必须被本地表挡住"
        );
        // ---- 批次 B8（FR-ACT-10）新增的五格 ----
        // `Ctrl+C`：本程序绑了"复制这一行"，必须挡。不挡的代价不是崩，是**静默降级**——
        // 窗口内热键录成 Ctrl+C 后 `local_hotkey_hits` 永远等不到那条 `Event::Key`
        //（egui-winit 在源头就把它折成 `Event::Copy` 了），用户改完键得到一颗死键。
        assert!(
            HotkeySpec {
                mods: HKF_CONTROL | HKF_NOREPEAT,
                vk: 0x43
            }
            .conflicts_local(LOCAL_TAKEN),
            "Ctrl+C 复制这一行，必须被本地表挡住"
        );
        // ---- 批次 E6（FR-ACT-01 最后一格）：`Alt+Enter` 复制这一行 ----
        // 这一格与 `Ctrl+Enter`（`HotkeySpec::SECONDARY_DEFAULT`）是同一个 VK，只差修饰键 ⇒
        // `check` 收它（回车在可录名单里），不挡就能把它绑成全局呼出键或次级键。
        // 代价与批次 R 裸 `` ` `` 同型但更宽：`Alt+Enter` 在系统里是"属性对话框"（资源管理器、
        // 任务计划程序…）与终端确认换行的常用键，`RegisterHotKey` 一吞，别人那颗也没了。
        let alt_enter = HotkeySpec {
            mods: HKF_ALT | HKF_NOREPEAT,
            vk: 0x0D,
        };
        assert!(
            alt_enter.conflicts_local(LOCAL_TAKEN),
            "Alt+Enter 复制这一行，必须被本地表挡住"
        );
        // 挡住的是**精确那一格**：多加一层修饰键的 `Ctrl+Alt+Enter` 本程序没绑，仍留给用户
        // （与上面 `Ctrl+F1` 那条同一口径：表越宽，用户能录的组合越少）。
        assert!(
            !HotkeySpec {
                mods: HKF_ALT | HKF_CONTROL | HKF_NOREPEAT,
                vk: 0x0D
            }
            .conflicts_local(LOCAL_TAKEN),
            "Ctrl+Alt+Enter 本程序没绑，不该出现在表里"
        );
        // 次级动作键走的是另一道判据（`check_secondary` 里也读这张表）⇒ 单独钉一次：
        // 用户不能把 `Ctrl+Enter` 换成 `Alt+Enter`，否则两颗键争同一帧，本批那条就得重新排次序。
        assert!(
            HotkeySpec {
                mods: HKF_ALT,
                vk: VK_RETURN
            }
            .check_secondary(&HotkeySpec::DEFAULT, None, LOCAL_TAKEN)
            .is_err(),
            "次级动作键不许录成 Alt+Enter（已被窗口内热键占用）"
        );
        // 出厂那颗 `Ctrl+Enter` 仍然合法——上面那句挡的是"改成 Alt+Enter"，不是"回车类全禁"。
        assert!(
            HotkeySpec::SECONDARY_DEFAULT
                .check_secondary(&HotkeySpec::DEFAULT, None, LOCAL_TAKEN)
                .is_ok(),
            "出厂的 Ctrl+Enter 次级键不许被自己这张表挡掉"
        );
        // 裸 F1–F4：`HotkeySpec::check` 对功能键放行不带修饰 ⇒ 这四颗是**真录得出来**的裸键
        //（与"裸键录不进来"那条先例不同，见 `LOCAL_TAKEN` 上方的 2026-10-26 注记）。
        for vk in [0x70u32, 0x71, 0x72, 0x73] {
            let bare = HotkeySpec {
                mods: HKF_NOREPEAT,
                vk,
            };
            assert!(bare.check().is_ok(), "裸 F 键必须录得进来，否则表里这条是空的");
            assert!(
                bare.conflicts_local(LOCAL_TAKEN),
                "裸 0x{vk:02X} 本程序绑了，必须被本地表挡住"
            );
            // 带修饰那一层仍然留给用户（`take_fresh_key` 只认 `Modifiers::NONE`）。
            assert!(
                !HotkeySpec {
                    mods: HKF_CONTROL | HKF_NOREPEAT,
                    vk
                }
                .conflicts_local(LOCAL_TAKEN),
                "Ctrl+F(0x{vk:02X}) 本程序没绑，该让用户录"
            );
        }
        // 只绑到 F4：F5 往后没占用，不许顺手多挡一条（表越宽，用户能录的组合越少）。
        assert!(
            !HotkeySpec {
                mods: HKF_NOREPEAT,
                vk: 0x74
            }
            .conflicts_local(LOCAL_TAKEN),
            "F5 本程序没绑，不该出现在表里"
        );
        // ---- 批次 C3：`Ctrl+↑` / `Ctrl+↓` 绑了但**不进表** ----
        // 上面那条 for 会把"表里有但录不进来"的格子判红，这两颗正是被它挡回去的：
        // 方向键既过不了 `check`（只收字母数字空格回车反引号与 F1–F24），`vk_from_egui_key`
        // 也不认这两个变体 ⇒ 全局键与次级动作键都录不成它，表里拦一颗录不出来的键只是白占一行。
        // 这一句钉的是"下一个人看到窗口内绑了 ↑↓，顺手往表里补两行"——补了这里就红。
        for vk in [0x26u32, 0x28] {
            let arrow = HotkeySpec {
                mods: HKF_CONTROL | HKF_NOREPEAT,
                vk,
            };
            assert!(
                arrow.check().is_err(),
                "0x{vk:02X} 要是哪天录得进来了，这条让位规则就得重新想"
            );
            assert!(
                !arrow.conflicts_local(LOCAL_TAKEN),
                "录不进来的键不许占本地表一行"
            );
        }
        // 录制端也得自己认这同一件事：`vk_from_egui_key` 不认方向键 ⇒ `spec_from_press` 直接
        // 返回 `None`，那颗键在录制路径上连"非法组合"的提示都走不到（口径是"继续等，不提示"）。
        // 两边判据分开钉：`check` 是"值不值得注册"，这里是"能不能被录出来"，缺一个都会骗人。
        let ctrl = egui::Modifiers {
            ctrl: true,
            command: true, // Windows 上 egui-winit 把 command 直接抄 ctrl（lib.rs:492-500）
            ..egui::Modifiers::NONE
        };
        for key in [egui::Key::ArrowUp, egui::Key::ArrowDown] {
            assert_eq!(
                vk_from_egui_key(key),
                None,
                "{key:?} 要是哪天进了可录集合，上面那条让位就得重判"
            );
            assert!(
                spec_from_press(key, ctrl).is_none(),
                "{key:?} + Ctrl 录不出 spec"
            );
        }
    }

    /// 批次 C3：`Ctrl+↑↓` 的取键口径。**不自己复刻判据**——把上游真会投进队列的键帧
    /// （egui-winit 0.36.2 `lib.rs:492-500`：Windows 上按住 Ctrl 时 `ctrl` 与 `command` 一起为真）
    /// 喂给 egui 自己的 `consume_key`，让它说清"这一帧还剩什么"。
    ///
    /// 三条都是真机上会撞的：
    /// ① `Ctrl+↑` 取走之后裸 `↑` 必须还在——同一帧的列表导航读的就是 `key_pressed(↑)`，
    ///    把裸键一起吞掉等于按一次历史选下一行跟着跳；
    /// ② `Ctrl+Shift+↑` 一起被取走（`matches_logically` 只在模式**要求** shift 时才查它）
    ///    ⇒ 多按一个 Shift 仍翻历史，与 AHK 那句 `if ctrl` 同一口径；
    /// ③ `Alt+↑` 不归本程序：模式要 `command`，事件里 `command` 为假 ⇒ 原样留给输入框。
    #[test]
    fn ctrl_arrow_take_leaves_bare_and_alt_arrows() {
        use egui::{Event, InputState, Key, Modifiers};
        let none = Modifiers::NONE;
        let ctrl = Modifiers {
            ctrl: true,
            command: true,
            ..Modifiers::NONE
        };
        let ctrl_shift = Modifiers {
            shift: true,
            ..ctrl
        };
        let alt = Modifiers {
            alt: true,
            ..Modifiers::NONE
        };
        let press = |key, m| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: m,
        };

        let mut up = InputState::default();
        up.events = vec![press(Key::ArrowUp, ctrl), press(Key::ArrowUp, none)];
        assert!(
            up.consume_key(Modifiers::COMMAND, Key::ArrowUp),
            "Ctrl+↑ 该被取走（翻历史）"
        );
        assert!(
            up.key_pressed(Key::ArrowUp),
            "裸 ↑ 必须留在同一帧里 ⇒ 列表导航照旧，不需要另写一道修饰键判据"
        );
        assert!(
            !up.consume_key(Modifiers::COMMAND, Key::ArrowUp),
            "同一帧只取一次：裸 ↑ 不许被第二下当成 Ctrl+↑"
        );

        let mut down = InputState::default();
        down.events = vec![press(Key::ArrowDown, ctrl_shift), press(Key::ArrowDown, alt)];
        assert!(
            down.consume_key(Modifiers::COMMAND, Key::ArrowDown),
            "多按一个 Shift 仍算翻历史（matches_logically 不查模式没要求的 Shift）"
        );
        assert_eq!(
            down.num_presses(Key::ArrowDown),
            1,
            "Alt+↓ 不是本程序的键，必须整条留下"
        );
        // 刻意不再断言"Alt+↓ 取不走"：`consume_key(ALT, ↓)` 本身会命中那条事件
        //（模式没要求 ctrl，`cmd_ctrl_matches` 只查"别按 ctrl/command"）——
        // 这条键不归本程序管是靠**我们不去调它**保证的，不是靠取键函数的判据。
    }

    /// 批次 C3：`Ctrl+↑↓` 在哪些情况下让位。判据单列成 `recall_history_allowed` 就是为了这条能测——
    /// 界面帧里那三样状态（面板 / 文件模式 / 弹窗）凑不出真实现场。
    #[test]
    fn history_recall_yields_to_panel_file_mode_and_modals() {
        assert!(
            recall_history_allowed(false, false, true),
            "搜索页、没开面板、不在文件模式 ⇒ 才归历史"
        );
        assert!(
            !recall_history_allowed(true, false, true),
            "面板开着：↑↓ 是面板自己的导航键，翻历史会把选中项改掉"
        );
        assert!(
            !recall_history_allowed(false, true, true),
            "文件模式：AHK 的 RecallHistory 在这一档直接 return（回填进来的串会被当成路径续打）"
        );
        assert!(
            !recall_history_allowed(false, false, false),
            "弹窗 / 确认框 / 关于 / 速查表活着：按键不归搜索页（keys_live 那六条门）"
        );
    }

    #[test]
    fn large_text_prefers_the_selected_row_then_the_query() {
        use crate::source::calc::CalcHit;
        use crate::source::custom::CustomHint;
        use crate::source::filesearch::FileHit;
        use crate::source::websearch::SearchHit;

        // AHK `_ShowLargeType`：没有选中行就用输入框原文（且两侧空白不进大字窗）。
        assert_eq!(
            large_text_of(None, None, "  138 0013 8000 "),
            Some("138 0013 8000".to_string())
        );
        assert_eq!(large_text_of(None, None, "   "), None, "全空白不弹窗");

        // 条目层照 AHK `CopyText()`：URL / 文件 / 目录取 path，其余取名称。
        let url = LaunchItem {
            name: "示例".into(),
            kind: ItemKind::Url,
            path: "https://example.com/?q=1".into(),
            ..Default::default()
        };
        assert_eq!(
            large_text_of(None, Some(&url), "被忽略"),
            Some("https://example.com/?q=1".to_string())
        );
        let app = LaunchItem {
            name: "记事本".into(),
            kind: ItemKind::App,
            path: "notepad.exe".into(),
            ..Default::default()
        };
        assert_eq!(large_text_of(None, Some(&app), ""), Some("记事本".to_string()));

        // 头段行：计算器取结果、网页搜索取 URL、文件取路径、参数提示取命令原文。
        let calc = HeadRow::Calc(CalcHit {
            title: "7+7".into(),
            subtitle: String::new(),
            copy: "14".into(),
        });
        assert_eq!(large_text_of(Some(&calc), None, ""), Some("14".to_string()));
        let search = HeadRow::Search(SearchHit {
            engine_id: "g".into(),
            title: "g 词".into(),
            subtitle: String::new(),
            url: "https://example.com/search?q=词".into(),
        });
        assert_eq!(
            large_text_of(Some(&search), None, ""),
            Some("https://example.com/search?q=词".to_string())
        );
        let file = HeadRow::File(FileHit {
            title: "a.txt".into(),
            path: "C:\\x\\a.txt".into(),
            is_dir: false,
            subtitle: String::new(),
        });
        assert_eq!(
            large_text_of(Some(&file), None, ""),
            Some("C:\\x\\a.txt".to_string())
        );
        let hint = HeadRow::CustomHint(CustomHint {
            uid: "u".into(),
            title: "ping（需要参数）".into(),
            subtitle: String::new(),
            completion: "ping ".into(),
            command: "cmd.exe /k ping {query}".into(),
        });
        assert_eq!(
            large_text_of(Some(&hint), None, ""),
            Some("cmd.exe /k ping {query}".to_string()),
            "提示行的标题是说明文，命令原文才是可展示内容"
        );

        // 有选中条目时条目优先（真实调用里两者互斥，这里钉住优先级）。
        assert_eq!(
            large_text_of(Some(&calc), Some(&url), ""),
            Some("https://example.com/?q=1".to_string())
        );
    }

    /// 批次 B8-甲：`Ctrl+C` 的取键通道。
    ///
    /// **输入形态照上游真实赋值构造**（不是照"我以为 winit 会投什么"）：
    /// `egui-winit-0.36.2/src/lib.rs:1019-1027` 在 Ctrl+C 上只 push 一条 `Event::Copy`
    /// 然后 `return`，一条 `Event::Key{ C, ctrl }` 都不会进队列。所以这里既要钉住
    /// "`Event::Copy` 才算命中"，也要钉住"光有 `Key{C}` 不算"——后者正是那条会编译、
    /// 会绿、真机上永远是死键的写法（`consume_key(COMMAND, C)`）。
    #[test]
    fn copy_press_reads_the_copy_event_and_not_the_c_key() {
        // ① 真机形态：只有 Event::Copy ⇒ 命中，且事件被摘走（同帧 TextEdit 不能再复制一遍）。
        let mut events = vec![egui::Event::Copy, egui::Event::Copy];
        assert!(take_copy_press(&mut events, false));
        assert_eq!(events.len(), 1, "只摘走一条，另一条留给下一帧的同一判据");
        assert!(take_copy_press(&mut events, false));
        assert!(events.is_empty(), "命中过的那条必须从队列里消失");

        // ② 只有 Key{C, ctrl}：不算命中（这正是 consume_key 那条死键的现场）。
        let mut events = vec![egui::Event::Key {
            key: egui::Key::C,
            physical_key: Some(egui::Key::C),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }];
        assert!(
            !take_copy_press(&mut events, false),
            "`Event::Key{{C, ctrl}}` 在本机永远不会出现，不许当成命中"
        );
        assert_eq!(events.len(), 1);

        // ③ 输入框有选区 ⇒ 让位：返回 false，且**不许**摘走那条 Copy（否则选中文本复制就死了）。
        let mut events = vec![egui::Event::Copy];
        assert!(!take_copy_press(&mut events, true));
        assert_eq!(events.len(), 1, "让位时事件必须原样留给 TextEdit");

        // ④ 剪切板上的邻居不许混进来：Cut / Paste 都不算复制这一行。
        let mut events = vec![egui::Event::Cut, egui::Event::Paste("x".into())];
        assert!(!take_copy_press(&mut events, false));
        assert_eq!(events.len(), 2);
    }

    /// 批次 B8-丙：滚轮 → 选中位移。符号口径两头都对过源码
    /// （`data/input/event.rs:157-161` 正向=内容向下移=往上翻；`scroll_area.rs:1234` 拿正向去减
    /// `offset`；AHK `_OnMouseWheel` 反向上滚给 `MoveSelection(-1)`）⇒ 向上滚 = `-1`。
    #[test]
    fn wheel_moves_selection_one_row_per_notch() {
        let wheel = |y: f32, mods: egui::Modifiers| {
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, y),
                phase: egui::TouchPhase::Move,
                modifiers: mods,
            }
        };
        let none = egui::Modifiers::NONE;
        // 向下滚（负向）= 下一行；向上滚 = 上一行。
        assert_eq!(wheel_selection_step(&[wheel(-1.0, none)]), 1);
        assert_eq!(wheel_selection_step(&[wheel(1.0, none)]), -1);
        // 一格一行，不看量：触控板一条大增量、`Unit::Point` 的连续滚动都只走一行。
        assert_eq!(wheel_selection_step(&[wheel(-14.0, none)]), 1);
        assert_eq!(
            wheel_selection_step(&[wheel(-1.0, none), wheel(-1.0, none), wheel(-1.0, none)]),
            1,
            "同帧三条也只移一行（连续滚由每一帧各管一次）"
        );
        // 一帧内上下各一条 = 抵消，不动选中。
        assert_eq!(wheel_selection_step(&[wheel(-1.0, none), wheel(1.0, none)]), 0);
        // 带任何修饰键不接管：Ctrl=缩放、Shift=横向、Alt=egui 的纵向覆盖，都留给 egui。
        assert_eq!(wheel_selection_step(&[wheel(-1.0, egui::Modifiers::COMMAND)]), 0);
        assert_eq!(wheel_selection_step(&[wheel(-1.0, egui::Modifiers::SHIFT)]), 0);
        assert_eq!(wheel_selection_step(&[wheel(-1.0, egui::Modifiers::ALT)]), 0);
        // 别的键事件不算滚轮。
        assert_eq!(
            wheel_selection_step(&[egui::Event::Key {
                key: egui::Key::ArrowDown,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: none,
            }]),
            0
        );
        assert_eq!(wheel_selection_step(&[]), 0);
    }

    /// 批次 B8-丙：`Ctrl+C` 复制的"这一行"就是大字那一串，但**没有选中行时必须什么都不是**。
    /// 第三个参数传空串是这条与 `Ctrl+L` 唯一的差别：大字按 AHK `_ShowLargeType` 回落到输入框原文，
    /// 复制按 AHK `_CopySelected`（`item` 取不到就 `return`）——把查询原文复制走是另一件事。
    #[test]
    fn copy_line_of_row_uses_the_same_text_but_never_the_query() {
        use crate::source::calc::CalcHit;

        let calc = HeadRow::Calc(CalcHit {
            title: "= 14".into(),
            subtitle: String::new(),
            copy: "14".into(),
        });
        assert_eq!(large_text_of(Some(&calc), None, ""), Some("14".to_string()));
        let app = LaunchItem {
            name: "记事本".into(),
            kind: ItemKind::App,
            path: "C:\\Windows\\System32\\notepad.exe".into(),
            ..Default::default()
        };
        assert_eq!(large_text_of(None, Some(&app), ""), Some("记事本".to_string()));
        assert_eq!(
            large_text_of(None, None, "jsq"),
            Some("jsq".to_string()),
            "大字那侧：没有选中行就用输入框原文（这条不许被复制改掉）"
        );
        assert_eq!(
            large_text_of(None, None, ""),
            None,
            "复制这侧：一行都没有 ⇒ 什么都不复制（空串喂的是查询回落那一格）"
        );
    }

    /// 批次 E3：启动期写盘门禁的四种组合。`Lost` 出现在任一份上就必须挡写盘——
    /// 只盯清单、或把 `Lost` 折进 `Option` 都漏得掉"设置坏了但清单好"那一格。
    /// 名字也要跟着回来：Toast 与日志得说清是哪一份文件，含糊成"某个配置"等于没说。
    #[test]
    fn lost_evidence_blocks_writes_for_either_file() {
        let none: Option<Corrupted> = None;
        let kept: Option<Corrupted> = Some(Corrupted::Kept(std::path::PathBuf::from(
            "user_items.json.bad",
        )));

        assert!(
            lost_evidence_labels(&[("settings.json", &none), ("user_items.json", &none)])
                .is_empty(),
            "两份都没坏 ⇒ 不许挡写盘"
        );
        assert_eq!(
            lost_evidence_labels(&[
                ("settings.json", &Some(Corrupted::Lost)),
                ("user_items.json", &kept),
            ]),
            vec!["settings.json"],
            "留证成功的 Kept 不该被当成事故"
        );
        assert_eq!(
            lost_evidence_labels(&[
                ("settings.json", &none),
                ("user_items.json", &Some(Corrupted::Lost)),
            ]),
            vec!["user_items.json"],
        );
        assert_eq!(
            lost_evidence_labels(&[
                ("settings.json", &Some(Corrupted::Lost)),
                ("user_items.json", &Some(Corrupted::Lost)),
            ]),
            vec!["settings.json", "user_items.json"],
            "两份都没留住 ⇒ 两份都要报出来"
        );
        // 批次 C3 起入参是切片：第三份文件（`search_history.json`）挂进来不必让形参再长一格。
        // 这两条只钉函数本身——每一份 `Lost` 都照实报出、`Kept` 照旧不算事故、顺序照旧按传入。
        // 「历史坏了只挡历史写盘、配置照写」**不是**这个函数的性质，是调用方的：
        // `LauncherApp::new` 只把配置那两份喂进这道闸，历史那份另走 `history_blocked`。
        assert_eq!(
            lost_evidence_labels(&[
                ("settings.json", &none),
                ("user_items.json", &none),
                ("search_history.json", &Some(Corrupted::Lost)),
            ]),
            vec!["search_history.json"],
            "三份里只有历史丢证 ⇒ 只报它一个"
        );
        assert_eq!(
            lost_evidence_labels(&[
                ("settings.json", &Some(Corrupted::Lost)),
                ("user_items.json", &none),
                ("search_history.json", &kept),
            ]),
            vec!["settings.json"],
            "三份混着传：Kept 依旧不算事故、顺序依旧按传入"
        );
    }

    /// 批次 E3-丙：窗口内热键（次级动作键）的匹配。
    ///
    /// 真机最怕的两条在这里钉住：① 裸 Enter **不算**命中——算的话我们会在 Enter 那一档之前
    /// 把"运行选中项"吃掉；② 多按一个 Shift 不算命中——`Modifiers::matches_logically` 恰好相反
    ///（`consume_key` 就是被它坑过两次：Shift+Tab、Shift+数字），所以这里必须逐位相等。
    #[test]
    fn window_hotkey_matches_modifiers_exactly() {
        let bt = |mods, code| HotkeySpec { mods, vk: code };
        let m = |ctrl: bool, alt: bool, shift: bool| egui::Modifiers {
            ctrl,
            alt,
            shift,
            ..Default::default()
        };
        let ctrl_enter = HotkeySpec::SECONDARY_DEFAULT;

        assert!(local_hotkey_hits(
            ctrl_enter,
            egui::Key::Enter,
            m(true, false, false)
        ));
        assert!(
            !local_hotkey_hits(ctrl_enter, egui::Key::Enter, m(false, false, false)),
            "裸 Enter 归「运行选中项」，不是次级动作"
        );
        assert!(
            !local_hotkey_hits(ctrl_enter, egui::Key::Enter, m(true, false, true)),
            "多带 Shift 是另一组"
        );
        assert!(!local_hotkey_hits(
            ctrl_enter,
            egui::Key::Enter,
            m(false, true, false)
        ));
        assert!(
            !local_hotkey_hits(ctrl_enter, egui::Key::Space, m(true, false, false)),
            "同一个修饰键、换成别的键不算"
        );
        assert!(
            !local_hotkey_hits(
                bt(HKF_CONTROL, VK_RETURN),
                egui::Key::ArrowLeft,
                m(true, false, false)
            ),
            "映射不出 VK 的键永远不命中"
        );

        // NOREPEAT 只是注册位：带不带都不影响比对（设置页存的草稿恒不带）。
        let alt_f6 = bt(HKF_ALT | HKF_NOREPEAT, 0x75);
        assert!(local_hotkey_hits(alt_f6, egui::Key::F6, m(false, true, false)));
        assert!(!local_hotkey_hits(alt_f6, egui::Key::F6, m(true, false, false)));

        // 手改 settings.json 塞进来的 Win 位：egui 在 Windows 上传不上来 ⇒ 恒不命中。
        // 这一条防的是"把 Win 当成没按修饰键"⇒ 裸 Enter 变成次级动作键。
        assert!(!local_hotkey_hits(
            bt(HKF_WIN | HKF_CONTROL, VK_RETURN),
            egui::Key::Enter,
            m(true, false, false)
        ));
        assert!(!local_hotkey_hits(
            bt(HKF_WIN, VK_RETURN),
            egui::Key::Enter,
            m(false, false, false)
        ));
    }

    /// 批次 E6（FR-ACT-01「`Alt+Enter` 复制这一行」）：这颗键的取用。
    ///
    /// 用真 `Context` + 真 `Ui`（手法照 `shift_digit_is_left_for_text_instead_of_bare_digit`）而不
    /// 只测 [`local_hotkey_hits`]，是因为本批要钉的是**两件事**：① 命中只认逐位相等那一种组合；
    /// ② 命中之后那一条 `Event::Key` 必须从这一帧消失。②才是"一次按键干两件事"的发生现场——
    /// 下面 Enter 那一档读 `key_pressed`，而 `num_presses` 数的就是同一份 `events`（egui 0.36.2
    /// `input_state/mod.rs:691-711`），事件不删就会"复制完又把选中项启动一遍"。
    /// 键帧按上游真实赋值构造：Windows 上 egui-winit 把 `command` 直接抄成 `ctrl`
    ///（`egui-winit-0.36.2/src/lib.rs:492-500`），`Alt` 没有这种副本。
    #[test]
    fn alt_enter_is_taken_and_leaves_the_other_enter_frames_alone() {
        use egui::{Event, Modifiers, RawInput};
        fn key(k: egui::Key, modifiers: Modifiers) -> Event {
            Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }
        }
        let alt = Modifiers {
            alt: true,
            ..Modifiers::NONE
        };
        // 一次跑完返回：(本批那颗键命中没有, 这一帧还剩几条 Enter 键事件)
        let run = |events: Vec<Event>| {
            let ctx = egui::Context::default();
            let mut hit = false;
            let mut left = 0usize;
            let mut out = ctx.run_ui(RawInput { events, ..Default::default() }, |ui| {
                hit = take_local_hotkey(ui, ALT_ENTER);
                left = ui.input(|i| {
                    i.events
                        .iter()
                        .filter(|e| {
                            matches!(e, Event::Key {
                                key: egui::Key::Enter,
                                pressed: true,
                                ..
                            })
                        })
                        .count()
                });
            });
            // FullOutput 的纹理增量不落地就会在 drop 时 panic（epaint textures.rs:337），单测里清掉。
            out.textures_delta.clear();
            (hit, left)
        };
        assert_eq!(
            run(vec![key(egui::Key::Enter, alt)]),
            (true, 0),
            "Alt+Enter 命中 ⇒ 自己那一格必须消失，否则同帧的 Enter 档会再执行一次选中项"
        );
        assert_eq!(
            run(vec![key(egui::Key::Enter, Modifiers::NONE)]),
            (false, 1),
            "裸 Enter 归「运行选中项」，一颗事件都不许动"
        );
        assert_eq!(
            run(vec![key(egui::Key::Enter, Modifiers { ctrl: true, command: true, ..Modifiers::NONE })]),
            (false, 1),
            "Ctrl+Enter 是出厂的次级动作键，本批不许把它一起收走"
        );
        assert_eq!(
            run(vec![key(egui::Key::Enter, Modifiers { alt: true, shift: true, ..Modifiers::NONE })]),
            (false, 1),
            "多按一个 Shift 是另一组键（`consume_key` 的 matches_logically 恰恰相反，见批次 E3-丙 那条坑）"
        );
        // 同一帧里两条并存（按住 Alt 又松了一次裸 Enter 这种注入实测会造出来的形状）：
        // 只准收自己那一格，第二格必须留着——批次 R 清空整帧那个坑的判据（见 [`take_record_press`]）。
        assert_eq!(
            run(vec![
                key(egui::Key::Enter, alt),
                key(egui::Key::Enter, Modifiers::NONE)
            ]),
            (true, 1),
            "只 retain 掉 Alt+Enter 那一格，同帧的裸 Enter 必须还在"
        );
        // 长按自动重复**不接管**，而且重复那一条要连事件一起收走——这是本批抓到的真缺陷：
        // 原先只在命中时才 retain，于是第一帧之后 egui 自己生成的重复投递原样留在这一帧，
        // 而下游那一档用的 `key_pressed` 数的是 `pressed: true`、**不看** `repeat`（`:753-764`）
        // ⇒ 按住不放＝复制一次，之后每帧"运行选中项"一次。
        // 这一格差点被写成假测试，同样是 egui 源码挡下来的：`repeat` 不是调用方填的那个值——
        // `begin_pass` 按**跨帧的** `keys_down` 把它改写（`input_state/mod.rs:393`+`:400-413`：
        // `let first_press = keys_down.insert(*key); *repeat = !first_press`），
        // 单帧里手工塞一条 `repeat: true` 会被改回 `false`，于是"命中"。
        // ⇒ 真形状只能用同一个 `Context` 连跑两帧来造：第二帧那颗键才真的是 repeat。
        let ctx = egui::Context::default();
        // 一次跑完返回：(命中没有, 取键前这条事件的 repeat 位, 取键后 `key_pressed(Enter)`)
        let frame = |ev: Event| {
            let mut hit = false;
            let mut marked_repeat = false;
            let mut enter_pressed = false;
            let mut out = ctx.run_ui(RawInput { events: vec![ev], ..Default::default() }, |ui| {
                // 取键**之前**读 `repeat`：这一位是"egui 改写了调用方填的值"那条事实的现量。
                marked_repeat = ui.input(|i| {
                    i.events.iter().any(|e| {
                        matches!(e, Event::Key { key: egui::Key::Enter, repeat: true, .. })
                    })
                });
                hit = take_local_hotkey(ui, ALT_ENTER);
                // 取键**之后**问下游真正用的那条判据，而不是自己数事件：钉的是症状本身。
                enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
            });
            out.textures_delta.clear();
            (hit, marked_repeat, enter_pressed)
        };
        let press = key(egui::Key::Enter, alt);
        assert_eq!(
            frame(press.clone()),
            (true, false, false),
            "第一帧是真按：命中，egui 不把它标成重复，且同帧的「运行选中项」判据为假"
        );
        assert_eq!(
            frame(press),
            (false, true, false),
            "同一个 pressed 事件再来一帧 ⇒ egui 自己标成 repeat ⇒ 本批那颗键不命中；\
             这一条也必须收走 ⇒ `key_pressed(Enter)` 为假（长按不连发选中项）"
        );
    }

    /// 批次 E5：「粘贴到原窗口」三道闸里有两道发生在**碰键盘之前**，所以单测盖得到；
    /// 第三道（焦点当场真的回到原窗口）只有真机测得出。这里钉的是判据的**次序**：
    /// 空串先挡、死句柄次之、没记账最后，三条都返回 `Err` 且一条 `SendInput` 都不发。
    /// 防的是以后有人把这几道挪到注入之后——症状是往此刻拿着焦点的本程序输入框里打进一串数字。
    #[test]
    fn the_paste_gates_refuse_before_anything_is_typed() {
        let dead = HWND((usize::MAX >> 1) as *mut _);
        let dead_raw = dead.0 as usize;
        // ① 空串在句柄判据之前：记着一个死句柄也不该走到"投字符"那一步，也不该顺手清掉记账
        //    （清空是"原窗口已关"那条分支的活儿，见 ②）。
        PREVIOUS_HWND.store(dead_raw, Ordering::Relaxed);
        assert!(
            paste_text_into_previous_window("")
                .unwrap_err()
                .contains("没有可粘贴"),
            "空串要先被挡下，而不是先去问句柄"
        );
        assert_eq!(PREVIOUS_HWND.load(Ordering::Relaxed), dead_raw);
        // ② 原窗口已经关了 ⇒ 拒绝 + 清零记账，下一次不会再往同一个死句柄上撞。
        //    `usize::MAX >> 1` 在 x64 上不可能是句柄值（内核句柄只占低 32 位）⇒ `IsWindow` 必 FALSE。
        let e = paste_text_into_previous_window("42").unwrap_err();
        assert!(e.contains("关掉了"), "{e}");
        assert_eq!(PREVIOUS_HWND.load(Ordering::Relaxed), 0, "死句柄要清零");
        // ③ 压根没记下架（这次呼出的前台是空的）⇒ 说的是"没记下"，不是"关掉了"。
        let e = paste_text_into_previous_window("42").unwrap_err();
        assert!(e.contains("没记下"), "{e}");
    }

    fn work_rect(l: f32, t: f32, r: f32, b: f32) -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(l, t), egui::pos2(r, b))
    }

    /// 批次 F3（FR-WIN-09）：默认档 `[500, 200]` ＝ 水平居中、离工作区顶 20%。
    /// 要害在 Y 那条**不做**「减去窗口高」（AHK `Place` 就是这么写的：X 摊剩余宽、Y 摊整块高），
    /// 写成 `(height - win_h) * fy` 的话 200 会落到 76，肉眼就是"贴在顶上"。
    #[test]
    fn default_ratio_is_centered_horizontally_and_a_fifth_down() {
        let w = work_rect(0.0, 0.0, 1920.0, 1040.0);
        let (x, y) = place_in_work(w, (500, 658), source::application::POS_REL_DEFAULT);
        assert_eq!(x, 710, "（1920-500）×0.5");
        assert_eq!(y, 208, "1040×0.2，不扣窗口高");
    }

    /// 摆好再折回来要回到同一组千分比（±1 是两次取整的余量）。
    #[test]
    fn place_and_relative_round_trip() {
        let w = work_rect(-1920.0, 0.0, 0.0, 1040.0);
        let size = (500, 658);
        for fx in [0u16, 1, 250, 500, 750, 999, 1000] {
            // Y 的上限是「还能整个塞进工作区」那条（(1040-658)/1040 ≈ 367‰）；再大就会被夹住，
            // 夹住之后折回来当然对不上——那属于「超界」而不是往返失真。
            for fy in [0u16, 100, 200, 300, 367] {
                let rel = [fx, fy];
                let pos = place_in_work(w, size, rel);
                let back = relative_in_work(w, size, pos);
                assert!(
                    back[0].abs_diff(fx) <= 1 && back[1].abs_diff(fy) <= 1,
                    "{rel:?} → {pos:?} → {back:?}"
                );
            }
        }
    }

    /// 窗口比工作区还宽 / 还高时夹到左缘上缘，不把窗口推出屏幕；折回来记成居中。
    #[test]
    fn oversized_window_is_clamped_and_recorded_as_centered() {
        let w = work_rect(0.0, 0.0, 400.0, 300.0);
        let size = (500, 658);
        assert_eq!(place_in_work(w, size, [1000, 1000]), (0, 0));
        assert_eq!(relative_in_work(w, size, (0, 0)), [500, 0]);
    }

    /// 主屏在右边、副屏在左边（负坐标）时同样按那块屏的工作区摊。
    #[test]
    fn negative_origin_work_area_is_honoured() {
        let w = work_rect(-1920.0, 0.0, 0.0, 1040.0);
        let (x, y) = place_in_work(w, (500, 658), [0, 1000]);
        assert_eq!(x, -1920);
        assert_eq!(y, 1040 - 658, "贴底要夹到工作区底边减窗口高");
    }

    /// F4：文件夹 / 网址条目没写图标源时按类别兜通用图标（AHK `IconCache` 同口径），
    /// 其余类别空着就是没有；显式写了规格的照原文用，不被类别兜底顶掉。
    #[test]
    fn folder_and_url_items_fall_back_to_a_generic_icon() {
        let folder = LaunchItem {
            name: "此电脑".into(),
            kind: ItemKind::Folder,
            path: "shell:MyComputerFolder".into(),
            ..Default::default()
        };
        assert_eq!(icon_spec_of(&folder), "folder:");
        let url = LaunchItem {
            kind: ItemKind::Url,
            path: "ms-settings:".into(),
            ..Default::default()
        };
        assert_eq!(icon_spec_of(&url), "url:");
        let cmd = LaunchItem {
            kind: ItemKind::Command,
            ..Default::default()
        };
        assert_eq!(icon_spec_of(&cmd), "");
        let explicit = LaunchItem {
            kind: ItemKind::Folder,
            source_path: "ext:.txt".into(),
            ..Default::default()
        };
        assert_eq!(icon_spec_of(&explicit), "ext:.txt");

        // 二十个文件夹 + 二十个网址只该派两个取图标任务（同一张纹理）。
        let many: Vec<LaunchItem> = (0..20)
            .flat_map(|_| {
                [
                    LaunchItem {
                        kind: ItemKind::Folder,
                        ..Default::default()
                    },
                    LaunchItem {
                        kind: ItemKind::Url,
                        ..Default::default()
                    },
                ]
            })
            .collect();
        let specs: Vec<String> = icon_jobs(&many).into_iter().map(|(_, s)| s).collect();
        assert_eq!(specs, vec!["folder:".to_string(), "url:".to_string()]);
    }
}

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, CloseHandle, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT,
    POINT, RECT, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
    WaitForSingleObject,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_SPACE,
};
use windows::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, ShellExecuteExW, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos,
    GetForegroundWindow, GetMessageW, GetWindowRect, GetWindowThreadProcessId, PostQuitMessage,
    RegisterClassExW, SendMessageTimeoutW, SetWindowPos, ShowWindow, TranslateMessage,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, HWND_MESSAGE, MSG, SMTO_ABORTIFHUNG, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOZORDER, SW_RESTORE, SW_SHOWNORMAL, WM_COPYDATA, WM_DESTROY, WM_HOTKEY,
    WNDCLASSEXW, WINDOW_EX_STYLE, WINDOW_STYLE,
};

const HOTKEY_TOGGLE: i32 = 1;

mod cli;
mod matching;
mod source;
use source::application::{
    default_keyword, dedup_items, is_url, learn_alias, make_item_from_input, ItemKind, LaunchItem,
    LearnRank, SpaceRun, TabMode,
};
use std::time::{SystemTime, UNIX_EPOCH};

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
    let name: Vec<u16> = SINGLE_INSTANCE_MUTEX.encode_utf16().chain(std::iter::once(0)).collect();
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
    let class: Vec<u16> = IPC_WINDOW_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
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
        .0
            != 0
    };
    if acked { IpcSendStatus::Ack } else { IpcSendStatus::Timeout }
}

/// FR-WIN-07：旧实例对 IPC 无应答 ⇒ 按卡死处理，结束它，供本进程接管。
/// 只认本程序的消息窗类名，避免误杀其它进程。
fn kill_hung_instance() -> bool {
    let class: Vec<u16> = IPC_WINDOW_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
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
    Exit,
}

/// 设置页分类（对标 AHK 版设置窗口的左侧分类栏，先立当下有真实选项的三类）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Features,
    Hotkeys,
}

impl SettingsTab {
    /// FR-WIN-08：页码从 1 起（1=通用 2=功能 3=热键）；越界返回 None，由调用方落回通用。
    fn from_page(page: u32) -> Option<Self> {
        match page {
            1 => Some(Self::General),
            2 => Some(Self::Features),
            3 => Some(Self::Hotkeys),
            _ => None,
        }
    }
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
        }
    }
    /// 提交时生成条目；`source_path` 默认取 `path`（图标源，URL/无文件时 extract 自然失败退化无图标）。
    fn to_item(&self) -> LaunchItem {
        let name = self.name.trim().to_string();
        let name = if name.is_empty() {
            self
                .path
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
        let kind =
            if self.kind == ItemKind::Command && is_url(&path) { ItemKind::Url } else { self.kind };
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
            source_path: if self.source_path.trim().is_empty() { self.path.trim().to_string() } else { self.source_path.trim().to_string() },
            arguments: self.arguments.clone(),
            working_dir: if wd.is_empty() { None } else { Some(wd.to_string()) },
            kind,
            keyword: kw,
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
    unsafe { GetWindowRect(hwnd, &mut r) }.ok().map(|_| (r.left, r.top))
}

fn win_size(hwnd: HWND) -> Option<(i32, i32)> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r) }
        .ok()
        .map(|_| (r.right - r.left, r.bottom - r.top))
}

struct LauncherApp {
    query: String,
    items: Vec<LaunchItem>,           // 当前显示项 = 内置种子 + 用户清单（去重后）
    index: matching::MatchIndex,      // FR-QRY-06 匹配键缓存（名称/别名的 lower/words/initials）
    apps_tx: mpsc::Sender<Vec<LaunchItem>>, // 「批量导入」触发端（可 clone）
    apps_rx: mpsc::Receiver<Vec<LaunchItem>>, // 「批量导入」后台扫描结果通道
    clear_query_next_frame: bool,
    selected: usize,              // FR-QRY/基线：当前键盘选中的结果行
    scroll_followed: usize,       // FR-ACT-10：列表上次已跟随到位的选中行（与 selected 不等即本帧要滚动）
    focus_query: bool,            // 呼出后请求把键盘焦点给搜索框
    dialog: Option<DialogState>,  // FR-SRC-20 添加/编辑对话框（Some=打开）
    confirm: Option<ConfirmRequest>, // 删除确认弹窗（Some=打开；2026-10-18 用户要求，删除不再一键生效）
    win_drag: Option<DragAnchor>,    // 左键拖空白移动窗口的进行中锚点（None=没在拖）
    dialog_enter_armed: bool,     // 弹窗当帧吞掉 Enter，防"打开框的那次回车"当帧又触发对话框的确定
    settings_open: bool,          // 设置页（整页模式，替代搜索页；2026-10-17 用户要求）
    settings_tab: SettingsTab,    // 设置页当前分类
    prefs: source::application::AppSettings, // 用户偏好（settings.json，目前只有 space_to_run）
    icon_rx: mpsc::Receiver<(String, Vec<u8>, u32, u32)>,
    icon_tx: mpsc::Sender<(String, Vec<u8>, u32, u32)>, // 常驻，可 clone 给每批新项再起加载器
    icons: std::collections::HashMap<String, egui::TextureHandle>, // source_path → 纹理
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
    ime_before: Option<ImeState>, // E-05 呼出前的输入法状态
    self_hwnd: Option<HWND>,      // E-05 本机顶层窗口，切英文模式时对它发 WM_IME_CONTROL
    // FR-WIN-04 失焦关闭到托盘
    hide_at: Option<Instant>,     // 到点复查仍失焦则关闭到托盘
    had_focus_since_show: bool,   // 呼出后是否真正拿到过焦点（守卫：防异步 Focus 未生效那一帧误关闭）
    // 底部居中悬浮 Toast（FR-ACT-02 启动失败 + 各操作回执；deadline 到点自动消失）
    toast: Option<(String, Instant, egui::Color32)>,
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
                        w, h, nonzero, rgba.len(),
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
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
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

// ---- FR-ACT-10 键盘导航的纯计算部分（不碰 egui 状态，可单测）----

/// 结果行近似高度：32px 图标 + egui 默认行间距。
const ROW_H: f32 = 36.0;
/// 搜索行与分隔线占掉的高度，用于把窗口高换算成「一页可见几行」。
const LIST_CHROME_H: f32 = 44.0;

/// 一页可见行数，至少 1（窗口拉到只装得下一行时翻页仍要能动）。
fn page_rows(available_height: f32) -> usize {
    ((available_height - LIST_CHROME_H) / ROW_H).floor().max(1.0) as usize
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
    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"].get(idx).copied()
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
    KEYS.iter()
        .find_map(|(key, digit)| ui.input_mut(|i| i.consume_key(mods, *key)).then_some(*digit))
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
    if hwnd.0.is_null() { return None; }
    let ime_wnd = unsafe { ImmGetDefaultIMEWnd(hwnd) };
    if ime_wnd.0.is_null() { None } else { Some(ime_wnd) }
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
fn shell_execute(file: &str, params: &str, dir: Option<&str>) -> windows::core::Result<()> {
    let wfile = wide_z(file);
    let wparams = wide_z(params);
    let wdir = dir.map(|d| wide_z(d));
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

// ---- FR-WIN-05 全局热键 ----

static HOTKEY_TX: std::sync::OnceLock<mpsc::Sender<()>> = std::sync::OnceLock::new();
static CTX: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

unsafe extern "system" fn hotkey_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_HOTKEY {
            if wparam.0 as i32 == HOTKEY_TOGGLE {
                if let Some(tx) = HOTKEY_TX.get() {
                    let _ = tx.send(());
                }
                if let Some(ctx) = CTX.get() {
                    ctx.request_repaint();
                }
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

fn spawn_hotkey_thread(tx: mpsc::Sender<()>) {
    HOTKEY_TX.set(tx).ok();
    thread::spawn(|| unsafe {
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
            CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT,
            Some(HWND_MESSAGE),
            None,
            Some(module.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => { eprintln!("hotkey: CreateWindowExW failed: {e}"); return; }
        };
        let mods = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
        if let Err(e) = RegisterHotKey(Some(hwnd), HOTKEY_TOGGLE, mods, VK_SPACE.0 as u32) {
            eprintln!("hotkey: RegisterHotKey failed: {e}");
            return;
        }
        eprintln!("hotkey: Ctrl+Alt+Space registered");
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_TOGGLE);
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
    ) -> Self {
        let (icon_tx, icon_rx) = mpsc::channel();
        let (apps_tx, apps_rx) = mpsc::channel();
        // 清单文件不存在（首次运行）→ 用内置系统工具播种一次并落盘；此后它就是一张普通用户清单，
        // 全部可编辑/删除（对标 Delphi WriteDefaultShortCutList）。文件已存在（哪怕被清空）不覆盖。
        let mut items = if source::application::user_list_exists() {
            source::application::load_user_items()
        } else {
            let seeds = source::application::builtin_seeds();
            source::application::save_user_items(&seeds);
            seeds
        };
        items = dedup_items(items);
        let settings_open = intent.settings_page.is_some();
        let settings_tab =
            intent.settings_page.and_then(SettingsTab::from_page).unwrap_or(SettingsTab::General);
        let mut seed = Self {
            query: String::new(), clear_query_next_frame: false, items,
            index: matching::MatchIndex::default(),
            apps_tx, apps_rx, icon_tx: icon_tx.clone(),
            icons: std::collections::HashMap::new(),
            selected: 0,
            scroll_followed: 0,
            focus_query: intent.visible && intent.dialog.is_none() && !settings_open,
            dialog: intent.dialog, dialog_enter_armed: false,
            confirm: None,
            win_drag: None,
            settings_open, settings_tab,
            prefs: source::application::load_settings(),
            icon_rx, tray_rx, hotkey_rx,
            ipc_rx, ipc_pending: Vec::new(),
            pending_batch: intent.batch,
            hide_pending: !intent.visible,
            notify_tx, notify_rx,
            visible: intent.visible, ime_before: None, self_hwnd,
            hide_at: None, had_focus_since_show: false,
            toast: None,
        };
        // 立即为已有项起图标加载器（source_path 为空的种子会被 extract 自然跳过）。
        let source_paths: Vec<String> =
            seed.items.iter().map(|i| i.source_path.clone()).filter(|p| !p.is_empty()).collect();
        spawn_icon_loader(icon_tx, source_paths);
        // 启动即直接可见时永远走不到 `show_now()` 的「从不可见到可见」边沿，`on_show()` 里的输入法
        // 切换与失焦武装一次都不跑 ⇒ 冷启动后第一次打字被中文输入法截走（2026-10-18 用户裁定补一次）。
        // 判据取 focus_query（＝首屏就是搜索页）：弹窗/设置页启动时焦点归它们，不能让 on_show 抢回去。
        if seed.focus_query {
            seed.on_show();
        }
        seed
    }

    /// FR-SRC-21：IPC 触发的呼出（已显示则只重抢焦点，不再走 on_show 重复切输入法）。
    fn show_now(&mut self, ctx: &egui::Context) {
        if !self.visible {
            self.visible = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            // winit 的可见标志会与实际窗口脱节（被系统/全屏程序最小化后，标志仍是「可见」，
            // Visible(true) 便不发 ShowWindow）；SW_RESTORE 不看标志，直接还原并显示。
            if let Some(hwnd) = self.self_hwnd {
                unsafe { let _ = ShowWindow(hwnd, SW_RESTORE); };
            }
            self.on_show();
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// FR-WIN-08：打开设置页并切到指定页码（1=通用 2=功能 3=热键）。
    fn open_settings_page(&mut self, page: u32) {
        self.settings_open = true;
        self.settings_tab = match SettingsTab::from_page(page) {
            Some(t) => t,
            None => {
                eprintln!(
                    "FR-WIN-08: -Preferences {page} 无对应页（1=通用 2=功能 3=热键），落回通用"
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
        let incoming: Vec<LaunchItem> =
            paths.iter().map(|p| dialog_for_sent_path(p).to_item()).collect();
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
        let skipped_text =
            if skipped > 0 { format!("，{skipped} 条重复已跳过") } else { String::new() };
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
            if edge == EDGE_MOVE { "空白移动" } else { "边缘缩放" }
        );
        self.win_drag = Some(DragAnchor { hwnd, cursor, origin, size, edge });
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
                                    "补全优先＝只有当「能被这个查询补长的候选恰好一条」时 Tab 才补全整词（如 jsq→计算器）；候选多于一条说明歧义没消，Tab 改为循环下一项，绝不替你猜第一条。\n专职导航＝Tab 下一项、Shift+Tab 上一项（Delphi 版 ALTRun 的做法），代价是没有补全。\n两种口径都保留 ↑↓ 翻页与 PgUp/PgDn；Tab 导航到头回绕，↑↓ 仍是钳位不动。想要一条不被补全抢走的稳定导航，用 Ctrl+J / Ctrl+K。",
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
                                    "这一层只在两条目的匹配档位相同时才决定先后，匹配更准的那条永远在前。\n次数优先（默认）＝执行次数多的在前，次数相同才看最近一次。刚用过一次的东西不会把用了几十次的老项挤下去。\n次数 + 时间衰减＝比 log2(1+次数) × 0.5^(距上次使用天数 / 7)：次数照旧计权重但被 log2 压过量级，一条项整整 7 天没用热度掉一半，所以\"上周用过一次的\"不会长期赖在前面。\n置顶（Ctrl+P）永远压过这一层；想钉住某个条目就用它，不用靠改口径绕。",
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
                        }
                        SettingsTab::Hotkeys => {
                            egui::Grid::new("hotkeys_grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                                for (key, desc) in [
                                    ("Ctrl+Alt+Space", "全局呼出 / 隐藏"),
                                    ("↑ / ↓", "选择条目"),
                                    ("PgUp / PgDn", "结果列表翻页（列表滚动跟随选中项）"),
                                    ("1 … 9 / 0", "执行第 1–9 行 / 第 10 行（列表前 10 行已标号）；裸键仅输入为空时生效，Ctrl+数字 / Alt+数字 不受限制"),
                                    ("Tab", "补全 / 循环下一项（按设置页「Tab 键行为」）；补全优先模式下只有「能被这个查询补长的候选恰好一条」才补全，否则导航"),
                                    ("Shift+Tab", "循环上一项（两种 Tab 模式一致）"),
                                    ("Ctrl+J / Ctrl+K", "循环下一项 / 上一项（恒为导航，不会被补全抢走）"),
                                    ("Space", "执行选中项（默认仅输入为空时；设置页「空格直接执行」可改为始终执行）"),
                                    ("左键拖边缘/角", "改变窗口大小（四边 + 四角各 6px 隐形手柄，最小 360×240）"),
                                    ("Ctrl+= / Ctrl+-", "窗口放大 / 缩小一档（60×40 逻辑像素，键盘路径同上）"),
                                    ("左键拖空白", "移动窗口：顶栏空白（搜索框与「＋ 新建」之间、设置页标题右侧）或列表右侧空白；中键不再执行任何条目"),
                                    ("Enter", "运行选中项；弹窗/确认框确定；无匹配弹新增"),
                                    ("Esc", "关确认框 → 关弹窗 → 设置页返回 → 清空输入 → 关闭到托盘（按序）"),
                                    ("Ctrl+N", "新建快捷项"),
                                    ("Ctrl+E", "编辑选中项"),
                                    ("Delete", "删除选中项（仅输入为空时；弹确认框）"),
                                    ("Ctrl+Shift+D", "删除选中项（不受输入限制；弹确认框）"),
                                    ("Ctrl+D", "打开选中项所在目录"),
                                    ("Ctrl+P", "置顶 / 取消置顶选中项（📌）"),
                                    ("Ctrl+I", "批量导入全部"),
                                    ("Ctrl+,", "打开 / 返回设置页"),
                                ] {
                                    ui.label(egui::RichText::new(key).monospace());
                                    ui.label(desc);
                                    ui.end_row();
                                }
                            });
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
        // 呼出后把键盘焦点给搜索框（下一帧渲染 TextEdit 时 request_focus）
        self.focus_query = true;
        self.selected = 0;
        let Some(hwnd) = self.self_hwnd else { return };
        let ime_wnd = unsafe { ImmGetDefaultIMEWnd(hwnd) };
        if ime_wnd.0.is_null() { return; }
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
        let Some(before) = self.ime_before.take() else { return };
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
        match shell_execute(&entry.path, &entry.arguments, entry.working_dir.as_deref()) {
            Ok(_) => eprintln!("launch: {} {}", entry.path, entry.arguments),
            Err(e) => {
                // M1 验收：启动失败不崩溃、不隐藏——窗口留在原地显示 Toast，TOAST_TTL 后自动消失。
                eprintln!("launch failed: {}: {e}", entry.path);
                self.toast = Some((
                    format!("⚠ 启动失败：{}（{e}）", entry.name),
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

    /// 把当前整张清单写盘（种子播种一次后即普通用户项，不再剔除）。
    fn save_items(&self) {
        source::application::save_user_items(&self.items);
        eprintln!("source: 已写清单 user_items.json（{} 条）", self.items.len());
    }

    /// FR-SRC-19 学习位序数据源：启动成功后累计次数 +1、刷新最近使用时间并落盘。
    /// 身份用派生 `uid`（见 §4.1.2）——同 exe 不同参数的两条目各算各的；
    /// 临时条目（如 Ctrl+D 的 explorer /select）不在清单里，自然匹配不到、跳过。
    /// `alias_word` 非空时顺带「提交即学别名」，与统计合并成同一次写盘。
    fn mark_used(&mut self, uid: &str, alias_word: &str) {
        let Some(pos) = self.items.iter().position(|i| i.uid() == uid) else { return };
        self.items[pos].use_count = self.items[pos].use_count.saturating_add(1);
        self.items[pos].last_used_ms = now_ms();
        if !alias_word.is_empty() && learn_alias(&mut self.items[pos], alias_word) {
            eprintln!(
                "提交即学别名：「{alias_word}」已写入「{}」的别名字段 ⇒ {}",
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
}

impl Default for LauncherApp {
    fn default() -> Self {
        let (_t1, r1) = mpsc::channel();
        let (_t2, r2) = mpsc::channel();
        let (_t3, r3) = mpsc::channel();
        let (t4, _r4) = mpsc::channel();
        let (_t5, r5) = mpsc::channel();
        Self::new(r1, r2, r3, t4, r5, None, StartupIntent { visible: true, ..Default::default() })
    }
}

impl LauncherApp {
    /// 当前查询的结果次序：匹配档位是第一主键，同档位内按设置页 `learn_rank` 选的口径比
    /// （甲＝累计次数→最近一次，乙＝热度 log2(1+次数)×半衰期衰减）。
    /// 时间戳只喂学习位序这一层，不参与打分、也不参与档位闸门。
    fn results(&mut self) -> Vec<usize> {
        self.index.search(&self.query, &self.items, self.prefs.learn_rank, now_ms())
    }
}

impl eframe::App for LauncherApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // eframe 为了保证首帧不出白闪，会在首帧渲染后无条件 set_visible(true)，
        // 把 with_visible(false) 覆盖掉；静默启动（-Startup）在第一帧末显式盖回隐藏。
        if self.hide_pending {
            self.hide_pending = false;
            if !self.visible {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Visible(false));
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
                TrayEvent::Exit => {
                    self.on_hide();
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        // FR-WIN-05 全局热键 toggle（Ctrl+Alt+Space）
        while self.hotkey_rx.try_recv().is_ok() {
            if self.visible {
                eprintln!("FR-WIN-05: 热键切换 → 隐藏");
                self.visible = false;
                self.on_hide();
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Visible(false));
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
                    eprintln!("FR-SRC-21: 收到转发路径 {p:?}");
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
            if self.dialog.is_some() || self.confirm.is_some() || self.settings_open {
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
                            eprintln!("FR-WIN-04: 失焦 {}ms 复查仍失焦 → 关闭到托盘", HIDE_DELAY.as_millis());
                            self.visible = false;
                            self.on_hide();
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Visible(false));
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
        for imported in batches {
            let old = self.items.clone();
            self.items = dedup_items([std::mem::take(&mut self.items), imported].concat());
            // dedup 保留的先出现项仍持旧对象（学习统计天然在）；若重复项出现在后位被剔前，
            // 统一回填一次兜底。
            self.carry_stats(&old);
            let missing: Vec<String> = self.items
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

        let mut launch: Option<LaunchItem> = None;
        let mut launch_from_list = false;
        let mut launch_via_digit = false; // 提交即学别名要剥掉尾数字，见 learn_alias 调用处
        let mut row_actions: Vec<String> = Vec::new();
        let mut pin_toggles: Vec<String> = Vec::new();

        // 快捷键补键（2026-10-17 立规「每个功能必须配快捷键」）：窗口可见且无弹窗时生效。
        // 键位口径（用户 2026-10-17 指定）：Delete=删除选中项（仅空输入时；有输入时归 TextEdit 前向删除）、
        // Ctrl+Shift+D=删除选中项（不受输入限制）、Ctrl+D=打开选中项所在目录（explorer /select）、
        // Ctrl+,=设置页开/关（2026-10-17 追加；SendTo 已从快捷键撤下改设置页开关）、
        // Ctrl+P=置顶/取消置顶选中项（FR-SRC-19，2026-10-17）、
        // PgUp/PgDn=翻页、Tab=补全或循环下一项（语义见设置页 tab_mode）、Shift+Tab=循环上一项、
        // Ctrl+J/Ctrl+K=恒为循环下/上一项（2026-10-18 用户点名 Emacs 的 next/prev line；
        // 因 Ctrl+N=新建、Ctrl+P=置顶 已占用，用户改选这一对空闲键），
        // 1-9/0 与 Ctrl+1-9/0=执行第 N 行（FR-ACT-10，2026-10-17）。
        // Space=执行选中项（FR-ACT-01 的 space_to_run，四档 off/word/unique/always，默认 word；
        // 判据与各自代价见设置页「空格执行选中项」）。Shift+空格 恒为插入真空格，不受档位影响。
        // 鼠标中键不再执行任何条目（2026-10-18 用户裁定废弃），移动窗口改由「左键拖空白」承担，见下方同名注释块。
        // consume_key 精确匹配修饰键并吃掉事件，防与 TextEdit/其他控件的键位打架。
        if self.visible && self.dialog.is_none() && self.confirm.is_none() {
            // 键盘缩放（2026-10-18 立规「每个功能都要配快捷键」：拖边缘是鼠标路径，这里补键盘路径）：
            // Ctrl+= 放大一档、Ctrl+- 缩小一档，搜索页与设置页都可用。走 ViewportCommand::InnerSize
            // （逻辑像素），下限与鼠标缩放同一口径，上限 1200×900 免得连按飞出屏幕。
            let (hit_zoom_in, hit_zoom_out) = {
                let hot = |k| ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, k));
                (hot(egui::Key::Equals), hot(egui::Key::Minus))
            };
            if hit_zoom_in || hit_zoom_out {
                let (dw, dh) = if hit_zoom_in { (60.0, 40.0) } else { (-60.0, -40.0) };
                let cur = ui.ctx().viewport_rect().size();
                let next = egui::vec2(
                    (cur.x + dw).clamp(MIN_W as f32, 1200.0),
                    (cur.y + dh).clamp(MIN_H as f32, 900.0),
                );
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(next));
                eprintln!(
                    "快捷键 Ctrl+{}：窗口 {}×{} → {}×{}",
                    if hit_zoom_in { "=" } else { "-" },
                    cur.x,
                    cur.y,
                    next.x,
                    next.y
                );
            }
            let hit_comma = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma));
            if hit_comma {
                self.settings_open = !self.settings_open;
                self.focus_query = true;
                eprintln!("快捷键 Ctrl+,：设置页 {}", if self.settings_open { "打开" } else { "返回搜索" });
                ui.ctx().request_repaint();
            } else if !self.settings_open {
                let (hit_n, hit_i, hit_e, hit_cd, hit_csd, hit_cp) = {
                    let hot = |mods: egui::Modifiers, k: egui::Key| ui.input_mut(|i| i.consume_key(mods, k));
                    (
                        hot(egui::Modifiers::COMMAND, egui::Key::N),
                        hot(egui::Modifiers::COMMAND, egui::Key::I),
                        hot(egui::Modifiers::COMMAND, egui::Key::E),
                        hot(egui::Modifiers::COMMAND, egui::Key::D),
                        hot(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::D),
                        hot(egui::Modifiers::COMMAND, egui::Key::P),
                    )
                };
                // 裸 Delete 只在搜索框为空时接管：有输入时它是文本编辑键，不能抢。
                let hit_del = self.query.trim().is_empty()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));

                // FR-ACT-10 补键：PgUp/PgDn 翻页；FR-ACT-06 Tab / Shift+Tab（语义见 tab_mode）。
                // Shift+Tab 必须先 consume：egui 的 consume_key 走 Modifiers::matches_logically，
                // 它对 shift 只查"模式要 shift 而实际没有"，所以 NONE 模式会把 Shift+Tab 一起吃掉。
                let (hit_pgup, hit_pgdn, hit_shift_tab, hit_tab) = {
                    let hot = |mods: egui::Modifiers, k: egui::Key| ui.input_mut(|i| i.consume_key(mods, k));
                    (
                        hot(egui::Modifiers::NONE, egui::Key::PageUp),
                        hot(egui::Modifiers::NONE, egui::Key::PageDown),
                        hot(egui::Modifiers::SHIFT, egui::Key::Tab),
                        hot(egui::Modifiers::NONE, egui::Key::Tab),
                    )
                };
                // Ctrl+J / Ctrl+K ＝ 无条件循环导航（2026-10-18 用户点名要 Emacs 的 next/prev line；
                // Ctrl+N/Ctrl+P 已被「新建」「置顶」占用，用户改选这一对空闲键）。
                let (hit_ctrl_j, hit_ctrl_k) = {
                    let hot = |mods: egui::Modifiers, k: egui::Key| ui.input_mut(|i| i.consume_key(mods, k));
                    (
                        hot(egui::Modifiers::COMMAND, egui::Key::J),
                        hot(egui::Modifiers::COMMAND, egui::Key::K),
                    )
                };
                // FR-ACT-01 把数字键列为执行手段：Ctrl+ / Alt+1..9/0 不受输入限制，
                // 裸数字沿用裸 Delete 的口径——只在输入框为空时接管（有输入时数字是查询文本）。
                let hit_digit = pressed_digit(ui, egui::Modifiers::COMMAND)
                    .or_else(|| pressed_digit(ui, egui::Modifiers::ALT))
                    .or_else(|| {
                        if self.query.trim().is_empty() {
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
                let hit_shift_space =
                    space_down && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Space));
                if hit_shift_space {
                    eprintln!(
                        "快捷键 Shift+空格：按档位 {:?} 本会执行，改判为插入真空格",
                        self.prefs.space_to_run
                    );
                }
                // `results()` 要 `&mut self`（缓存键表），与下面 `&self.query`/`&self.items`
                // 冲突，所以先落一个局部副本；只在真的按下空格时才算这一次过滤。
                let space_results = if space_down && !hit_shift_space {
                    self.results()
                } else {
                    Vec::new()
                };
                let space_takes = space_down
                    && !hit_shift_space
                    && matching::space_executes(
                        self.prefs.space_to_run,
                        &self.query,
                        &self.items,
                        &space_results,
                        self.selected,
                    );
                let hit_space = space_takes
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));
                if space_down && !hit_shift_space && !hit_space {
                    // 反向判据也要留痕：注入实测要看的就是"这一按空格到底进没进输入框"。
                    let n = self.results().len();
                    eprintln!(
                        "空格未接管（档位 {}，查询 {:?}，结果 {n} 条）：键事件留给输入框当文本",
                        self.prefs.space_to_run.as_str(),
                        self.query
                    );
                }

                if hit_n {
                    self.dialog = Some(DialogState::blank());
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
                } else if hit_e || hit_cd || hit_del || hit_csd || hit_cp {
                    let results = self.results();
                    let idx = self.selected.min(results.len().saturating_sub(1));
                    // search 返回下标而不是引用：调用方接下来要改 self.items。
                    let sel = results.get(idx).map(|i| self.items[*i].clone());
                    // 身份一律用派生 uid（同 path 不同参数是两个条目），path 只作数据用。
                    if let Some(s) = sel {
                        let uid = s.uid();
                        let name = s.name.clone();
                        let (path, source_path) = (s.path.clone(), s.source_path.clone());
                        if hit_e {
                            self.dialog = Some(DialogState::from_item(&s));
                            self.dialog_enter_armed = true;
                        } else if hit_cp {
                            // FR-SRC-19 Ctrl+P：切换选中项置顶，立即落盘、Toast 回执。
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
                                eprintln!("快捷键 Ctrl+P：置顶切换 {uid} → {on}");
                            }
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
                    let len = self.results().len();
                    if len > 0 {
                        let rows = page_rows(ui.available_height());
                        self.selected = page_target(self.selected, rows, len, hit_pgdn);
                        eprintln!(
                            "快捷键 {}：选中第 {} 行（一页 {rows} 行，共 {len} 行）",
                            if hit_pgdn { "PgDn" } else { "PgUp" },
                            self.selected + 1
                        );
                    }
                } else if hit_ctrl_j || hit_ctrl_k {
                    // 与 Tab/Shift+Tab 同口径（回绕循环，不是 ↑↓ 的钳位），差别只有一点：
                    // 这对键**永远只导航**，不会被 tab_mode=Complete 的补全抢走 ——
                    // 补全优先模式下想要一条稳定的「下一条/上一条」通道就靠它。
                    let len = self.results().len();
                    if len > 0 {
                        let cur = self.selected.min(len - 1);
                        self.selected = if hit_ctrl_k {
                            (cur + len - 1) % len
                        } else {
                            (cur + 1) % len
                        };
                        eprintln!(
                            "快捷键 {}：循环选中第 {} 行（共 {len} 行）",
                            if hit_ctrl_k { "Ctrl+K" } else { "Ctrl+J" },
                            self.selected + 1
                        );
                    } else {
                        eprintln!("快捷键 Ctrl+J/K：当前查询无结果，忽略");
                    }
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
                    let results = self.results();
                    let len = results.len();
                    if len > 0 {
                        let cur = self.selected.min(len - 1);
                        let backward = hit_shift_tab;
                        let next = if backward {
                            (cur + len - 1) % len
                        } else {
                            (cur + 1) % len
                        };
                        let complete_to = if backward
                            || !matches!(self.prefs.tab_mode, TabMode::Complete)
                        {
                            None
                        } else {
                            matching::unique_completion(&self.query, &self.items, &results)
                                .map(|rank| self.items[results[rank]].name.clone())
                        };
                        if let Some(name) = complete_to {
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
                    let results = self.results();
                    match digit_index(digit, results.len()) {
                        Some(idx) => {
                            let item = self.items[results[idx]].clone();
                            eprintln!("快捷键 数字键 {digit}：执行第 {} 行「{}」", idx + 1, item.name);
                            launch = Some(item);
                            launch_from_list = true;
                            launch_via_digit = true;
                        }
                        None => eprintln!(
                            "快捷键 数字键 {digit}：第 {} 行不存在（共 {} 行），忽略",
                            if digit == 0 { 10 } else { digit as usize },
                            results.len()
                        ),
                    }
                } else if hit_space {
                    let results = self.results();
                    let idx = self.selected.min(results.len().saturating_sub(1));
                    if let Some(&h) = results.get(idx) {
                        let item = self.items[h].clone();
                        eprintln!("快捷键 空格：执行第 {} 行「{}」", idx + 1, item.name);
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
        // 这里先铺「列表右侧真空白」（x 从 260 起、y 从搜索行以下，右缘留 12px 给滚动条）；
        // 顶部栏那段空白在 CentralPanel 里量到真实坐标后再铺（见 drag_handle 的两处调用）。
        // 添加框/确认框打开时整块撤掉：背景层只会在「该点没有任何控件认领拖拽」时赢，
        // 缩小矩形 + 让开弹窗，就不可能抢走搜索框、新建按钮与行标签的点击。
        if !(self.dialog.is_some() || self.confirm.is_some()) {
            let root = ui.max_rect();
            let plate = egui::Rect::from_min_max(
                egui::pos2(root.min.x + 260.0, root.min.y + 40.0),
                root.max - egui::vec2(12.0, 6.0),
            );
            // 设置页整页模式下这块矩形盖的是设置内容，必须让开。
            if !self.settings_open {
                self.drag_handle(
                    ui,
                    "window_drag_layer_list",
                    plate,
                    EDGE_MOVE,
                    None,
                    "列表右侧空白",
                );
            }
        }
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

        egui::CentralPanel::default().show(ui, |ui| {
            // 设置页整页模式（2026-10-17）：小窗口内独立弹窗会被裁剪，直接替换搜索页渲染。
            if self.settings_open {
                self.settings_ui(ui);
                return;
            }
            let search_id = search_field_id();
            // 顶栏拖动手柄要铺在「搜索框右缘 → ＋新建左缘」这段真空白上，尺寸只能运行时量
            //（2026-10-18 用户：「顶部输入框右侧点击空白不能拖动，顶部的不是菜单栏吗？」）。
            let mut pill_right = 0.0f32;
            let mut new_btn_left = f32::MAX;
            let head_row = ui.horizontal(|ui| {
                ui.label("\u{1F50D} ");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(search_id)
                        .hint_text("搜索…（↑↓/PgUp/PgDn 选择，Enter/空格/1-9 运行，Tab 补全，Ctrl+N 新建，Ctrl+E 编辑，Delete 删除，Ctrl+D 开目录，Ctrl+P 置顶，Ctrl+, 设置，Esc 清空/关闭）"),
                );
                pill_right = resp.rect.max.x;
                if resp.changed() {
                    self.selected = 0; // 输入变化时选中回到第一行
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 通用「＋ 新建」入口（FR-SRC-20）：打开空白添加对话框
                    let btn = ui.button("＋ 新建");
                    new_btn_left = btn.rect.min.x;
                    if btn.clicked() {
                        self.dialog = Some(DialogState::blank());
                    }
                });
            });
            if self.dialog.is_none() && self.confirm.is_none() {
                let r = head_row.response.rect;
                self.drag_handle(
                    ui,
                    "window_drag_layer_search_top",
                    egui::Rect::from_min_max(
                        egui::pos2(pill_right + 4.0, r.min.y),
                        egui::pos2(new_btn_left - 4.0, r.max.y),
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

            let q = self.query.trim().to_lowercase();
            let results = self.results();
            if results.is_empty() {
                if q.is_empty() {
                    ui.label("清单为空——按 Ctrl+N 或点右上「＋」新建，或按 Ctrl+I 批量导入。");
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
                if self.selected >= results.len() {
                    self.selected = 0;
                }
                // 键盘导航：↑↓ 移动选中，Enter 运行选中项。对话框/确认框开着时让位给它们（Enter=确定）。
                if self.dialog.is_none() && self.confirm.is_none() {
                    if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                        self.selected = (self.selected + 1).min(results.len() - 1);
                    } else if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                        self.selected = self.selected.saturating_sub(1);
                    } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        launch = Some(self.items[results[self.selected]].clone());
                        launch_from_list = true;
                    }
                }
                // 列表滚动跟随选中项（FR-ACT-10 验收项）：只在选中变化的那一帧把它居中，
                // 其余帧不干预用户自己的滚轮位置。
                let follow_selection = self.selected != self.scroll_followed;
                // results 是下标（见 MatchIndex::search），逐项回指 self.items。
                // 套 ScrollArea 才谈得上翻页与滚动跟随：窗口只装得下约十行，清单可达上百行。
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (idx, &hit) in results.iter().enumerate() {
                            let item = &self.items[hit];
                            let is_sel = idx == self.selected;
                            ui.horizontal(|ui| {
                                // 行号徽标（2026-10-18 用户要求：数字键没有可见对应物）：
                                // 前 10 行标 1-9/0，第 11 行起留白——那里没有按键，标了反而误导。
                                match row_num_label(idx) {
                                    Some(num) => {
                                        ui.add_sized(
                                            [14.0, 32.0],
                                            egui::Label::new(
                                                egui::RichText::new(num).monospace().weak(),
                                            ),
                                        )
                                        .on_hover_text(format!("按 {num} 直接执行本行"));
                                    }
                                    None => ui.add_space(14.0),
                                }
                                if let Some(texture) = self.icons.get(&item.source_path) {
                                    ui.image(egui::load::SizedTexture::new(texture.id(), egui::vec2(32.0, 32.0)));
                                } else {
                                    ui.add_space(32.0);
                                }
                                // FR-SRC-19：置顶项名称前加 📌 角标（免鼠标可辨）。
                                let shown = if item.pinned { format!("📌 {}", item.name) } else { item.name.clone() };
                                let sel = ui.selectable_label(is_sel, shown);
                                if is_sel && follow_selection {
                                    sel.scroll_to_me(Some(egui::Align::Center));
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
                                    // FR-SRC-19：置顶开关入口（右键），与 Ctrl+P 同语义。
                                    let pin_label = if item.pinned { "取消置顶" } else { "置顶" };
                                    if ui.button(pin_label).clicked() {
                                        pin_toggles.push(item.uid());
                                        ui.close();
                                    }
                                    // 与 Delete/Ctrl+Shift+D 同一条确认路径（2026-10-18 用户要求）。
                                    if ui.button("删除（不删磁盘文件）").clicked() {
                                        row_actions.push(item.uid());
                                        ui.close();
                                    }
                                });
                            });
                        }
                    });
                self.scroll_followed = self.selected;
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
                egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (c.a() as f32 * alpha) as u8)
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
                            ui.label(
                                egui::RichText::new(msg)
                                    .size(13.0)
                                    .color(fade(*text_c)),
                            );
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
            let title = if editing { "编辑快捷项" } else { "添加快捷项" };

            enum DlgResult {
                Keep,
                Confirm,
                Cancel,
            }

            let modal_resp = egui::Modal::new(egui::Id::new("shortcut_dialog")).show(
                ui.ctx(),
                |ui| {
                    ui.heading(title);
                    ui.separator();
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
                    egui::Grid::new("add_dialog_grid")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            let dg = self.dialog.as_mut().unwrap();
                            ui.label("别名");
                            ui.add(
                                egui::TextEdit::singleline(&mut dg.keyword)
                                    .hint_text("触发码，如 calc")
                                    .desired_width(240.0),
                            );
                            ui.end_row();
                            ui.label("名称");
                            ui.add(
                                egui::TextEdit::singleline(&mut dg.name)
                                    .hint_text("显示名，如 计算器")
                                    .desired_width(240.0),
                            );
                            ui.end_row();
                            ui.label("命令行");
                            ui.add(
                                egui::TextEdit::singleline(&mut dg.path)
                                    .hint_text("exe / 路径 / URL / shell: 目标")
                                    .desired_width(240.0),
                            );
                            ui.end_row();
                            ui.label("参数");
                            ui.add(egui::TextEdit::singleline(&mut dg.arguments).desired_width(240.0));
                            ui.end_row();
                            ui.label("工作目录");
                            ui.add(egui::TextEdit::singleline(&mut dg.working_dir).desired_width(240.0));
                            ui.end_row();
                            ui.label("类型");
                            let mut idx =
                                ItemKind::ALL.iter().position(|k| *k == dg.kind).unwrap_or(0);
                            egui::ComboBox::from_id_salt("kind_combo")
                                .selected_text(dg.kind.label())
                                .show_index(ui, &mut idx, ItemKind::ALL.len(), |i| {
                                    ItemKind::ALL[i].label().to_string()
                                });
                            dg.kind = ItemKind::ALL[idx];
                            ui.end_row();
                        });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("确定").clicked() {
                            result = DlgResult::Confirm;
                        }
                        if ui.button("取消").clicked() {
                            result = DlgResult::Cancel;
                        }
                    });
                    result
                },
            );

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
                            self.carry_stats(&old);
                            if !item.source_path.is_empty()
                                && !self.icons.contains_key(item.source_path.as_str())
                            {
                                spawn_icon_loader(self.icon_tx.clone(), vec![item.source_path.clone()]);
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
                            "只从启动器清单移除，不删磁盘上的程序或文件；需要时可再「＋ 新建」或 Ctrl+I 批量导入。",
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

        // 运行选中的程序并关闭到托盘（FR-ACT + 用户要求：Enter/点击运行后窗口自动关）。
        // FR-SRC-19：清单项成功启动才累计使用统计（launch_and_hide 失败分支提前 return，走不到这里）。
        if let Some(entry) = launch {
            let started = self.launch_and_hide(ui.ctx(), &entry);
            if started && launch_from_list {
                // FR-SRC-19 补全「提交即学别名」（口径与门槛见 learn_alias）。
                self.mark_used(&entry.uid(), &commit_word(&self.query, launch_via_digit));
            }
        }

        // Esc 分段（FR-ACT-10「清空后隐藏」+2026-10-17 设置页一档 +2026-10-18 删除确认一档）：
        // 确认框→关框；对话框→关框；设置页→回搜索；有字→清空；全空→关闭到托盘。
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.confirm.is_some() {
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
            } else if !self.query.is_empty() {
                self.query.clear();
                self.selected = 0;
                self.focus_query = true; // egui TextEdit 在编辑态按 Esc 会自弃焦点，主动要回
                ui.ctx().request_repaint();
            } else {
                self.visible = false;
                self.on_hide();
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        // 边缘缩放手柄（2026-10-18 用户：「窗口应该允许拖动边缘改变大小，现在无法改变大小」）。
        // 放在本帧最后铺设：同为 Background 层时后铺者赢得命中，万一和顶栏/列表空白手柄贴上，
        // 最外圈那 6px 归缩放。
        self.edge_grips(ui);
    }
}

fn install_cjk_fallback(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (file, index) in [("msyh.ttc", 0u32), ("simhei.ttf", 0), ("STXIHEI.TTF", 0)] {
        let Ok(bytes) = std::fs::read(format!(r"C:\Windows\Fonts\{file}")) else { continue };
        if skrifa::FontRef::from_index(&bytes, index).is_err() { continue; }
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        fonts.font_data.insert("cjk".to_owned(), std::sync::Arc::new(data));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("cjk".to_owned());
        }
        ctx.set_fonts(fonts);
        return;
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
    let exit_id = muda::MenuId("exit".to_string());
    let show_item = MenuItem::with_id(show_id.clone(), "显示", true, None);
    let import_item = MenuItem::with_id(import_id.clone(), "批量导入全部", true, None);
    let settings_item = MenuItem::with_id(settings_id.clone(), "设置…", true, None);
    let exit_item = MenuItem::with_id(exit_id.clone(), "退出", true, None);
    let menu = Menu::new();
    menu.append(&show_item).unwrap();
    menu.append(&import_item).unwrap();
    menu.append(&settings_item).unwrap();
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

fn main() -> eframe::Result {
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
        batch: if cli.send_to.len() > 1 { cli.send_to.clone() } else { Vec::new() },
    };
    let start_visible = intent.visible;

    let (_tray, tray_rx) = init_tray();
    let (hotkey_tx, hotkey_rx) = mpsc::channel();
    let (ipc_tx, ipc_rx) = mpsc::channel();
    IPC_TX.set(ipc_tx).ok();
    let (notify_tx, notify_rx) = mpsc::channel();
    spawn_hotkey_thread(hotkey_tx);

    let mut wgpu_options = eframe::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(create_new) = &mut wgpu_options.wgpu_setup {
        create_new.instance_descriptor.backends = eframe::wgpu::Backends::DX12 | eframe::wgpu::Backends::GL;
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([500.0, 400.0])
            .with_title("")
            .with_decorations(false)
            .with_always_on_top()
            .with_visible(start_visible),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };

    eframe::run_native(
        "egui Launcher",
        options,
        Box::new(move |cc: &eframe::CreationContext<'_>| {
            install_cjk_fallback(&cc.egui_ctx);
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
                tray_rx, hotkey_rx, ipc_rx, notify_tx, notify_rx, self_hwnd, intent,
            )))
        }),
    )
}

#[cfg(test)]
mod nav_tests {
    use super::*;

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
        assert_eq!(commit_word("  JSQ ", false), "jsq", "首尾空白与大小写不进别名");
        assert_eq!(commit_word("jsq5", true), "jsq", "数字键执行时行号不是触发词");
        assert_eq!(commit_word("jsq5", false), "jsq5", "非数字键执行不剥——那可能就是用户要的词");
        assert_eq!(commit_word("visual studio", false), "visual", "参数词不进别名");
        assert_eq!(commit_word("", false), "");
    }

    #[test]
    fn page_target_clamps_at_both_ends() {
        assert_eq!(page_target(0, 9, 100, true), 9);
        assert_eq!(page_target(95, 9, 100, true), 99, "向下夹到最后一行，不越过");
        assert_eq!(page_target(3, 9, 100, false), 0, "向上夹到第一行");
        assert_eq!(page_target(0, 9, 0, true), 0, "空结果不塌陷（len-1 的减法不许下溢）");
    }

    #[test]
    fn page_rows_uses_window_height_but_never_below_one() {
        assert_eq!(page_rows(400.0), 9, "默认 400px 窗口约九行一页");
        assert_eq!(page_rows(20.0), 1, "窗口拉到比列表还矮时仍要能动一行");
    }

    #[test]
    fn row_badges_agree_with_digit_keys() {
        // 徽标和 digit_index 必须说的是同一件事：按下徽标上的数字，落点就是那一行。
        for idx in 0..10 {
            let label = row_num_label(idx).unwrap_or_else(|| panic!("第 {} 行该有号", idx + 1));
            let digit: u8 = label.parse().unwrap();
            assert_eq!(digit_index(digit, 10), Some(idx), "徽标 {label} 与数字键落点不一致");
        }
        assert_eq!(row_num_label(10), None, "第 11 行没有对应按键，不该标号");
    }
}

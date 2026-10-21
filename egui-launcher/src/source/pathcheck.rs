//! FR-SRC-06 路径四态校验：`OK` / `Not found` / `Offline` / `-`。
//!
//! 核心判据只有一条：**「盘/共享不可达」与「文件确实不存在」必须分开**。拔掉 U 盘之后
//! `Z:\tools\x.exe` 的失败原因是"介质不在"，不是"这条命令写错了"；报告成 `Not found`
//! 会让用户去删一条其实没问题的条目（需求书验收例）。
//!
//! 检查范围（需求书原文 + 本模块的实现口径）：
//! - `url` 类型、任何 URL/协议、`shell:` 虚拟文件夹 ⇒ `-`（不检查）。
//! - 路径里含 `{query}` ⇒ `-`：填充前它不是一个真实路径，检查它只会得到假阴性。
//! - `command` 类型且只写了程序名 ⇒ `PATH`（配 `PATHEXT`）→ `App Paths`（HKCU/HKLM）。
//! - 其余按文件系统看：存在 ⇒ `OK`；盘根或 UNC 共享根不可达 ⇒ `Offline`；否则 `Not found`。
//!
//! **本模块会碰磁盘与注册表**（UNC/断开的映射盘可能阻塞数秒）⇒ 只允许在后台线程里调用
//! [`spawn_scan`]，主线程只读缓存结果。

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
};

use crate::source::application::{is_url, ItemKind, LaunchItem};
use crate::source::custom;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PathState {
    /// `-`：url / `shell:` / 含 `{query}` / 空路径 —— 不检查。
    #[default]
    NotChecked,
    Ok,
    NotFound,
    /// 盘或网络位置不可达。**不得与 `NotFound` 混报**。
    Offline,
}

impl PathState {
    pub fn label(self) -> &'static str {
        match self {
            PathState::NotChecked => "-",
            PathState::Ok => "OK",
            PathState::NotFound => "Not found",
            PathState::Offline => "Offline",
        }
    }

    /// 结果行只在异常两态加副标题——`OK`/`-` 占版面没有信息量。
    pub fn is_problem(self) -> bool {
        matches!(self, PathState::NotFound | PathState::Offline)
    }
}

pub fn check_item(item: &LaunchItem) -> PathState {
    check(&item.path, item.kind)
}

pub fn check(path: &str, kind: ItemKind) -> PathState {
    let raw = path.trim();
    if raw.is_empty() {
        return PathState::NotChecked;
    }
    if kind == ItemKind::Url
        || is_url(raw)
        || raw.to_ascii_lowercase().starts_with("shell:")
        || custom::has_query_placeholder(raw)
    {
        return PathState::NotChecked;
    }
    let expanded = custom::expand_env(raw);

    // 裸程序名先走 PATH/App Paths：`Path::exists()` 会看当前工作目录，
    // 把一条「其实找不到的命令」误判成存在。
    if kind == ItemKind::Command && is_bare_name(&expanded) {
        return if lookup_program(&expanded) { PathState::Ok } else { PathState::NotFound };
    }

    let p = PathBuf::from(&expanded);
    if p.exists() {
        return PathState::Ok;
    }
    match root_of(&p) {
        RootKind::Drive(root) => {
            if root.exists() { PathState::NotFound } else { PathState::Offline }
        }
        RootKind::Unc(root) => {
            if root.exists() { PathState::NotFound } else { PathState::Offline }
        }
        RootKind::None => PathState::NotFound,
    }
}

/// 后台批量检查。结果按派生 uid 回传，主线程按 uid 建缓存（同 path 不同参数是两条）。
/// 调用方负责"同一时刻只跑一个"——UNC 断线可能让这个线程挂很久，堆线程没有意义。
pub fn spawn_scan(tx: mpsc::Sender<Vec<(String, PathState)>>, items: Vec<LaunchItem>) {
    thread::spawn(move || {
        let out: Vec<(String, PathState)> =
            items.iter().map(|it| (it.uid(), check_item(it))).collect();
        let _ = tx.send(out);
    });
}

/// 只有程序名、没有目录也没有盘符（`ping`、`notepad.exe`）。
fn is_bare_name(s: &str) -> bool {
    !s.contains(['\\', '/', ':'])
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default()
}

fn path_exts() -> Vec<String> {
    let raw = std::env::var_os("PATHEXT")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut exts: Vec<String> = raw
        .split(';')
        .filter(|e| !e.trim().is_empty())
        .map(|e| {
            let e = e.trim().to_ascii_lowercase();
            if e.starts_with('.') { e } else { format!(".{e}") }
        })
        .collect();
    if exts.is_empty() {
        exts = vec![".com".into(), ".exe".into(), ".bat".into(), ".cmd".into()];
    }
    exts
}

/// `PATH` + `PATHEXT` + `App Paths` 三处查。找不到返回 false ⇒ 调用方判 `Not found`。
fn lookup_program(name: &str) -> bool {
    if app_paths_lookup(name) {
        return true;
    }
    let has_ext = Path::new(name).extension().is_some();
    let exts = path_exts();
    for dir in path_dirs() {
        let dir = Path::new(&dir);
        if dir.join(name).is_file() {
            return true;
        }
        if !has_ext {
            for ext in &exts {
                if dir.join(format!("{name}{ext}")).is_file() {
                    return true;
                }
            }
        }
    }
    false
}

/// `App Paths` 下登记的默认值就是 exe 全路径。HKCU 优先于 HKLM（用户级覆盖机器级）。
/// 只认 `REG_SZ`；键不存在/读失败一律当作没登记，不影响后面的 PATH 判定。
fn app_paths_lookup(name: &str) -> bool {
    let mut candidates = vec![name.to_string()];
    if Path::new(name).extension().is_none() {
        candidates.push(format!("{name}.exe"));
    }
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for cand in &candidates {
            let sub = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{cand}");
            if reg_default_string(root, &sub).is_some() {
                return true;
            }
        }
    }
    false
}

fn reg_default_string(root: HKEY, subkey: &str) -> Option<String> {
    let subkey_w = wide(subkey);
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * std::mem::size_of::<u16>()) as u32;
    let rc = unsafe {
        RegGetValueW(
            root,
            PCWSTR(subkey_w.as_ptr()),
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    // size 是含结尾 NUL 的字节数。
    let len = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

enum RootKind {
    Drive(PathBuf),
    Unc(PathBuf),
    None,
}

/// 取出「可达性判据」的那个根：`Z:\a\b` → `Z:\`，`\\srv\share\a` → `\\srv\share`。
fn root_of(p: &Path) -> RootKind {
    let s = p.to_string_lossy();
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return RootKind::Drive(PathBuf::from(format!("{}\\", &s[..2])));
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        let parts: Vec<&str> = rest.split(['\\', '/']).filter(|x| !x.is_empty()).collect();
        return match parts.as_slice() {
            [server, share, ..] => RootKind::Unc(PathBuf::from(format!(r"\\{server}\{share}"))),
            [server] => RootKind::Unc(PathBuf::from(format!(r"\\{server}"))),
            _ => RootKind::None,
        };
    }
    RootKind::None
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_and_shell_targets_are_not_checked() {
        for path in ["https://example.com/x", "ms-settings:windowsupdate", "shell:MyComputerFolder"] {
            assert_eq!(check(path, ItemKind::Url), PathState::NotChecked, "{path}");
            assert_eq!(check(path, ItemKind::Command), PathState::NotChecked, "{path}");
        }
        assert_eq!(check("   ", ItemKind::File), PathState::NotChecked);
    }

    #[test]
    fn placeholder_paths_are_not_checked() {
        assert_eq!(check(r"%USERPROFILE%\Documents\{query}", ItemKind::Folder), PathState::NotChecked);
        assert_eq!(check("C:\\x\\{query|默认}.txt", ItemKind::File), PathState::NotChecked);
    }

    /// 「启动器目录」把路径存成占位符：校验前必须解析成当前 exe 所在目录，
    /// 否则这条出厂条目会常年显示 `Not found`。
    #[test]
    fn launcher_dir_placeholder_resolves() {
        assert_eq!(
            check(crate::source::custom::LAUNCHER_DIR_PLACEHOLDER, ItemKind::Folder),
            PathState::Ok
        );
    }

    #[test]
    fn bare_program_name_uses_path_lookup() {
        // cmd.exe 在 %SystemRoot%\System32，必在 PATH 里；不带扩展名时靠 PATHEXT 补齐。
        assert_eq!(check("cmd.exe", ItemKind::Command), PathState::Ok);
        assert_eq!(check("cmd", ItemKind::Command), PathState::Ok);
        assert_eq!(
            check("definitely-not-a-real-program-xyz", ItemKind::Command),
            PathState::NotFound
        );
    }

    #[test]
    fn missing_local_file_is_not_found_not_offline() {
        let missing = std::env::temp_dir().join("egui-launcher-no-such-file.txt");
        let state = check(&missing.to_string_lossy(), ItemKind::File);
        assert_eq!(state, PathState::NotFound, "盘还在、文件不在 ⇒ Not found");
    }

    #[test]
    fn existing_file_is_ok() {
        let path = std::env::temp_dir().join("egui-launcher-pathcheck-probe.txt");
        std::fs::write(&path, b"x").expect("写临时文件");
        let state = check(&path.to_string_lossy(), ItemKind::File);
        let _ = std::fs::remove_file(&path);
        assert_eq!(state, PathState::Ok);
    }

    #[test]
    fn root_classification() {
        assert!(matches!(root_of(Path::new(r"C:\a\b")), RootKind::Drive(r) if r == Path::new(r"C:\")));
        assert!(matches!(root_of(Path::new(r"\\srv\share\a\b")), RootKind::Unc(r) if r == Path::new(r"\\srv\share")));
        assert!(matches!(root_of(Path::new(r"relative\a")), RootKind::None));
    }

    #[test]
    fn labels_match_the_requirement() {
        assert_eq!(PathState::NotChecked.label(), "-");
        assert_eq!(PathState::Ok.label(), "OK");
        assert_eq!(PathState::NotFound.label(), "Not found");
        assert_eq!(PathState::Offline.label(), "Offline");
        assert!(PathState::Offline.is_problem());
        assert!(!PathState::Ok.is_problem());
    }
}

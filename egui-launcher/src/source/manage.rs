//! FR-ACT-09（批次 D5）：快捷项批量管理——绝对路径 ↔ 相对路径互转。
//!
//! **本仓说的"相对"是 `%LAUNCHER_DIR%`，不是 Delphi 的 `.\`**。程序从不
//! `SetCurrentDirectory`，`.\x.exe` 认的是"启动它时的当前目录"——从资源管理器起是 exe 目录，
//! 从自启动项起是 `System32`，同一个 `user_items.json` 会时灵时不灵。`%LAUNCHER_DIR%` 在
//! `custom::expand_env`（启动、路径校验、文件索引三处共用）里每次都解析成当前 exe 目录，
//! 搬家不用改清单。需求书原文的 `.\` 属于 Delphi 的实现细节，登记为有意偏差。
//!
//! 反向转换两种写法都认：`%LAUNCHER_DIR%`（本仓产出）与 `.\`（用户手抄 Delphi 或 AHK 的写法）。
//!
//! 纯函数、不碰磁盘；哪条该跳过由 [`is_convertible`] 判，统计口径也在这里统一。

use std::collections::BTreeSet;

use crate::source::application::{ItemKind, LaunchItem, is_url};
use crate::source::custom;

/// 相对路径前缀，与出厂命令「启动器目录」用的是同一格（`custom::LAUNCHER_DIR_PLACEHOLDER`）。
pub const LAUNCHER_DIR: &str = custom::LAUNCHER_DIR_PLACEHOLDER;

/// 一次批量转换的结果：真正改了哪些行 + 选中却跳过了几条。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConvertPlan {
    /// `(uid, 新路径)`。只收**真的变了**的行——已经是目标形态的不算。
    pub changes: Vec<(String, String)>,
    /// 选中但没动：URL / `internal:` / `shell:` / 含 `{query}` / 不在 exe 目录下 / 已经是相对。
    pub skipped: usize,
}

impl ConvertPlan {
    pub fn changed(&self) -> usize {
        self.changes.len()
    }
}

/// 参与转换的门槛：是文件系统路径，且不是"填充前还不是真实路径"的那种。
pub fn is_convertible(item: &LaunchItem) -> bool {
    let raw = item.path.trim();
    !raw.is_empty()
        && item.kind != ItemKind::Internal
        && item.kind != ItemKind::Url
        && !is_url(raw)
        && !raw.to_ascii_lowercase().starts_with("shell:")
        && !custom::has_query_placeholder(raw)
}

/// 对勾选的行算一遍转换，不改数据。
pub fn plan(
    items: &[LaunchItem],
    checked: &BTreeSet<String>,
    exe_dir: &str,
    to_relative: bool,
) -> ConvertPlan {
    let mut out = ConvertPlan::default();
    for item in items {
        let uid = item.uid();
        if !checked.contains(&uid) {
            continue;
        }
        let next = if !is_convertible(item) {
            None
        } else if to_relative {
            relativize(&item.path, exe_dir)
        } else {
            absolutize(&item.path, exe_dir)
        };
        match next {
            Some(p) => out.changes.push((uid, p)),
            None => out.skipped += 1,
        }
    }
    out
}

/// 绝对 ⇒ `%LAUNCHER_DIR%\…`。不在 exe 目录下（或只是前缀相同、不是同一个目录）返回 `None`。
pub fn relativize(path: &str, exe_dir: &str) -> Option<String> {
    let path = strip_verbatim(path.trim());
    let dir = strip_verbatim(exe_dir.trim());
    let dir = trim_dir(dir);
    if dir.is_empty() || path.is_empty() {
        return None;
    }
    let rest = strip_dir_prefix(path, dir)?;
    // 目录边界：`C:\Tools\Launcher2` 不能被 `C:\Tools\Launcher` 吃掉。
    // 例外是 `dir` 本身以分隔符结尾（盘根 `C:\`），那时后缀直接接内容。
    if !rest.is_empty() && !dir.ends_with(['\\', '/']) && !rest.starts_with(['\\', '/']) {
        return None;
    }
    let rest = rest.trim_start_matches(['\\', '/']);
    if rest.is_empty() {
        Some(LAUNCHER_DIR.to_string())
    } else {
        Some(format!("{LAUNCHER_DIR}\\{rest}"))
    }
}

/// `%LAUNCHER_DIR%\…` 或 `.\…` ⇒ 绝对路径。别的写法返回 `None`（不猜）。
pub fn absolutize(path: &str, exe_dir: &str) -> Option<String> {
    let raw = path.trim();
    let dir = trim_dir(strip_verbatim(exe_dir.trim()));
    if dir.is_empty() || raw.is_empty() {
        return None;
    }
    let rest = if let Some(tail) = strip_dir_prefix(raw, LAUNCHER_DIR) {
        // `%LAUNCHER_DIR%x` 不是本仓的写法：只认"整串就是前缀"或"前缀后紧跟分隔符"。
        if !tail.is_empty() && !tail.starts_with(['\\', '/']) {
            return None;
        }
        tail
    } else if let Some(tail) = raw.strip_prefix(".\\").or_else(|| raw.strip_prefix("./")) {
        tail
    } else if raw == "." {
        ""
    } else {
        return None;
    };
    let rest = rest.trim_start_matches(['\\', '/']);
    if rest.is_empty() {
        Some(dir.to_string())
    } else {
        Some(format!("{dir}\\{rest}"))
    }
}

/// `\\?\C:\…` ⇒ `C:\…`：`current_exe()` 在 Windows 上会带这个前缀，用户清单里不会有。
fn strip_verbatim(s: &str) -> &str {
    s.strip_prefix(r"\\?\").unwrap_or(s)
}

/// 逐字符比对前缀，返回前缀之后的剩余部分。
///
/// 不用字节切片：`C:/Tools/Launcher` 的正斜杠写法要能认（`/` 与 `\` 等价），
/// 而非 ASCII 路径按字节切会在字符中间刀下去——`&s[..n]` 直接 panic。
fn strip_dir_prefix<'a>(path: &'a str, dir: &str) -> Option<&'a str> {
    let mut it = path.char_indices();
    for dc in dir.chars() {
        let (_, pc) = it.next()?;
        if !same_char(pc, dc) {
            return None;
        }
    }
    let idx = it.next().map(|(i, _)| i).unwrap_or(path.len());
    Some(&path[idx..])
}

fn same_char(a: char, b: char) -> bool {
    let norm = |c: char| if c == '/' { '\\' } else { c.to_ascii_lowercase() };
    norm(a) == norm(b)
}

/// 去掉尾部隔符；盘根（`C:`）补回一个，否则 `C:` 会当普通前缀匹配掉整个盘。
fn trim_dir(dir: &str) -> &str {
    let t = dir.trim_end_matches(['\\', '/']);
    if t.len() == 2 && t.as_bytes()[1] == b':' {
        // 盘根只有 26 个，用静态常量返回（函数签名借的是入参，拼不出临时串）。
        const ROOTS: [&str; 26] = [
            r"A:\", r"B:\", r"C:\", r"D:\", r"E:\", r"F:\", r"G:\", r"H:\", r"I:\", r"J:\",
            r"K:\", r"L:\", r"M:\", r"N:\", r"O:\", r"P:\", r"Q:\", r"R:\", r"S:\", r"T:\",
            r"U:\", r"V:\", r"W:\", r"X:\", r"Y:\", r"Z:\",
        ];
        let idx = t.as_bytes()[0].to_ascii_uppercase().wrapping_sub(b'A');
        if (idx as usize) < ROOTS.len() {
            return ROOTS[idx as usize];
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE_DIR: &str = r"C:\Tools\Launcher";

    fn item(path: &str, kind: ItemKind) -> LaunchItem {
        LaunchItem {
            name: "x".into(),
            path: path.into(),
            kind,
            ..Default::default()
        }
    }

    /// 证伪目标：若"前缀相同即可"，`Launcher2` 会被误当成 exe 目录的兄弟 ⇒ 这条会红。
    #[test]
    fn relativize_needs_a_directory_boundary() {
        assert_eq!(
            relativize(r"C:\Tools\Launcher\bin\app.exe", EXE_DIR).as_deref(),
            Some(r"%LAUNCHER_DIR%\bin\app.exe")
        );
        assert_eq!(relativize(r"C:\Tools\Launcher2\app.exe", EXE_DIR), None);
        assert_eq!(relativize(r"C:\Tools\LauncherX", EXE_DIR), None);
        assert_eq!(relativize(r"D:\Tools\Launcher\app.exe", EXE_DIR), None);
    }

    #[test]
    fn relativize_is_case_insensitive_and_handles_separators() {
        assert_eq!(
            relativize(r"c:\tools\launcher\App.EXE", EXE_DIR).as_deref(),
            Some(r"%LAUNCHER_DIR%\App.EXE")
        );
        // 尾部隔符、正斜杠、verbatim 前缀都不该影响判断。
        assert_eq!(
            relativize(r"\\?\C:\Tools\Launcher\bin\app.exe", r"C:\Tools\Launcher\").as_deref(),
            Some(r"%LAUNCHER_DIR%\bin\app.exe")
        );
        assert_eq!(
            relativize("C:/Tools/Launcher/bin/app.exe", EXE_DIR).as_deref(),
            Some(r"%LAUNCHER_DIR%\bin/app.exe")
        );
    }

    /// exe 目录自己也是合法值（出厂「启动器目录」那条就指向它）。
    #[test]
    fn relativize_keeps_the_launcher_dir_itself() {
        assert_eq!(relativize(EXE_DIR, EXE_DIR).as_deref(), Some(LAUNCHER_DIR));
        assert_eq!(
            relativize(r"C:\Tools\Launcher\", EXE_DIR).as_deref(),
            Some(LAUNCHER_DIR)
        );
    }

    /// 中文目录名：按字节切前缀会在字符中间落刀（`&s[..n]` panic），这条钉住逐字符比对。
    #[test]
    fn relativize_handles_non_ascii_directories() {
        assert_eq!(
            relativize(r"C:\工具\启动器\bin\app.exe", r"C:\工具\启动器").as_deref(),
            Some(r"%LAUNCHER_DIR%\bin\app.exe")
        );
        assert_eq!(relativize(r"C:\工具\启动器2\app.exe", r"C:\工具\启动器"), None);
    }

    #[test]
    fn absolutize_understands_both_spellings() {
        assert_eq!(
            absolutize(r"%LAUNCHER_DIR%\bin\app.exe", EXE_DIR).as_deref(),
            Some(r"C:\Tools\Launcher\bin\app.exe")
        );
        assert_eq!(
            absolutize(r"%launcher_dir%\bin\app.exe", EXE_DIR).as_deref(),
            Some(r"C:\Tools\Launcher\bin\app.exe")
        );
        assert_eq!(
            absolutize(".\\bin\\app.exe", EXE_DIR).as_deref(),
            Some(r"C:\Tools\Launcher\bin\app.exe")
        );
        assert_eq!(
            absolutize("%LAUNCHER_DIR%", EXE_DIR).as_deref(),
            Some(EXE_DIR)
        );
        // 不是本仓写法的一律不猜。
        assert_eq!(absolutize(r"%LAUNCHER_DIR%x\app.exe", EXE_DIR), None);
        assert_eq!(absolutize(r"C:\Windows\notepad.exe", EXE_DIR), None);
        assert_eq!(absolutize(r"..\up\app.exe", EXE_DIR), None);
    }

    #[test]
    fn round_trip_returns_the_original_string() {
        let abs = r"C:\Tools\Launcher\bin\app.exe";
        let rel = relativize(abs, EXE_DIR).expect("exe 目录下必须能转相对");
        assert_eq!(absolutize(&rel, EXE_DIR).as_deref(), Some(abs));
    }

    /// 已经是对应形态的行不该算进"改了 N 条"，否则报数会虚高。
    #[test]
    fn converting_twice_changes_nothing() {
        let rel = r"%LAUNCHER_DIR%\bin\app.exe";
        assert_eq!(relativize(rel, EXE_DIR), None);
        assert_eq!(relativize(r".\bin\app.exe", EXE_DIR), None);
        assert_eq!(absolutize(EXE_DIR, EXE_DIR), None);
    }

    #[test]
    fn non_filesystem_rows_are_never_converted() {
        assert!(is_convertible(&item(r"C:\Tools\Launcher\x", ItemKind::App)));
        assert!(is_convertible(&item(r"C:\Tools\Launcher\x", ItemKind::Command)));
        assert!(is_convertible(&item(r"C:\Tools\Launcher\x", ItemKind::Folder)));
        assert!(!is_convertible(&item(r"C:\Tools\Launcher\x", ItemKind::Url)));
        assert!(!is_convertible(&item(r"C:\Tools\Launcher\x", ItemKind::Internal)));
        assert!(!is_convertible(&item("https://example.com/a", ItemKind::App)));
        assert!(!is_convertible(&item(
            r"shell:AppsFolder\Microsoft.X_8wekyb3d8bbwe!App",
            ItemKind::App
        )));
        assert!(!is_convertible(&item(r"C:\Tools\Launcher\{query}.exe", ItemKind::App)));
        assert!(!is_convertible(&item("   ", ItemKind::App)));
    }

    /// 计划层：勾了三条（一条 URL、一条已在 exe 目录、一条普通文件），只该改最后一条。
    #[test]
    fn plan_counts_changes_and_skips_separately() {
        let items = vec![
            item(r"C:\Tools\Launcher\bin\app.exe", ItemKind::App),
            item("https://example.com", ItemKind::Url),
            item(r"%LAUNCHER_DIR%\bin\app.exe", ItemKind::App),
        ];
        let checked: BTreeSet<String> = items.iter().map(|i| i.uid()).collect();
        let rel = plan(&items, &checked, EXE_DIR, true);
        assert_eq!(rel.changed(), 1);
        assert_eq!(rel.skipped, 2);
        assert_eq!(rel.changes[0].1, r"%LAUNCHER_DIR%\bin\app.exe");

        let abs = plan(&items, &checked, EXE_DIR, false);
        assert_eq!(abs.changed(), 1);
        assert_eq!(abs.skipped, 2);
        assert_eq!(abs.changes[0].1, r"C:\Tools\Launcher\bin\app.exe");
    }

    #[test]
    fn plan_ignores_unchecked_rows() {
        let items = vec![item(r"C:\Tools\Launcher\bin\app.exe", ItemKind::App)];
        let empty = BTreeSet::new();
        assert_eq!(plan(&items, &empty, EXE_DIR, true), ConvertPlan::default());
    }
}

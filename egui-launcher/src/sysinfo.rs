//! 批次 G5（FR-SYS-03）：一键「复制系统信息」。
//!
//! 三段：`SystemFacts`（要报的字段，纯数据）→ [`build_report`]（排版成一段纯文本）→
//! 调用方把这段文本丢进剪贴板。Win32 只碰 [`os_version`] 一处（读注册表那几格版本号），
//! 其余字段由 `main.rs` 从自己肚子里取——这里不认识 `LauncherApp`，也不认识 egui。
//!
//! **隐私口径**：这份文本是给用户贴进 issue / 聊天框的，比日志更该少认人。
//! 路径一律先过 [`redact_path`] 把 `C:\Users\<用户名>` 换成 `%USERPROFILE%`；
//! 条目名称、查询原文、窗口标题一概不进（这里根本没有对应字段，是结构上做不到，不是"记得别写"）。

use std::path::Path;

/// 一份系统信息报告的全部字段。**全是已经算好、已经脱敏的展示串**——
/// 这样 [`build_report`] 与 [`redact_path`] 都能纯函数单测，Win32 与 egui 的取值
/// 留在 `main.rs`（那边本来就有这些值）。
#[derive(Debug, Clone, Default)]
pub struct SystemFacts {
    /// 程序版本（`CARGO_PKG_VERSION`）。
    pub version: String,
    /// `debug` / `release`。
    pub build_kind: String,
    /// 生效的日志级别 + 它是谁定的（例：`详细（跟随构建）`）。
    pub log_level: String,
    /// 系统版本（[`os_version`] 的产物）。
    pub os: String,
    /// 进程架构。
    pub arch: String,
    /// 渲染后端。程序固定只留 GL，见 `main.rs` 的 `Backends::GL`。
    pub backend: String,
    /// 缩放（每逻辑点几个像素）。
    pub dpi: String,
    /// 主题 / 字体 / 行高 / 行密度 / 窗口尺寸，一行说完。
    pub appearance: String,
    /// 落盘位置。键是短标签（`程序` / `数据` / `日志`），值**必须**已经过 [`redact_path`]。
    pub paths: Vec<(&'static str, String)>,
    /// 各类条目数。顺序即排版顺序。
    pub counts: Vec<(&'static str, usize)>,
    /// 数据源开 / 关（已经拼好的 `开 x、y；关 —`）。
    pub sources: String,
    /// 热键槽位与当前键位（`呼出` / `次呼出` / …）。`None` 的槽在调用方拼成 `—`。
    pub hotkeys: Vec<(&'static str, String)>,
}

/// 排版成一段纯文本。行序固定、每行 `标签：值`，用户整段粘出去即可。
pub fn build_report(f: &SystemFacts) -> String {
    use crate::i18n::{t, tfmt};
    let mut out = String::new();
    out.push_str(&tfmt("sysinfo.header", &[&f.version, &f.build_kind]));
    out.push('\n');
    for (key, value) in [
        ("sysinfo.log_level", &f.log_level),
        ("sysinfo.os", &f.os),
        ("sysinfo.arch", &f.arch),
        ("sysinfo.backend_line", &f.backend),
        ("sysinfo.dpi", &f.dpi),
        ("sysinfo.appearance", &f.appearance),
    ] {
        out.push_str(&tfmt(key, &[value]));
        out.push('\n');
    }
    out.push_str(t("sysinfo.paths"));
    out.push('\n');
    for (label, value) in &f.paths {
        out.push_str(&format!("  {label} {value}\n"));
    }
    let sep = t("sysinfo.sep");
    let counts = f
        .counts
        .iter()
        .map(|(label, n)| format!("{label} {n}"))
        .collect::<Vec<_>>()
        .join(sep);
    out.push_str(&tfmt("sysinfo.counts", &[&counts]));
    out.push('\n');
    out.push_str(&tfmt("sysinfo.sources", &[&f.sources]));
    out.push('\n');
    let hotkeys = f
        .hotkeys
        .iter()
        .map(|(label, key)| format!("{label} {key}"))
        .collect::<Vec<_>>()
        .join(sep);
    out.push_str(&tfmt("sysinfo.hotkeys", &[&hotkeys]));
    out.push('\n');
    out
}

/// 把文本里出现的内置路径（`C:\Users\<用户名>`）换成 `%USERPROFILE%`。
///
/// 判据细节：
/// - 大小写不敏感（Windows 路径本来就大小写不敏感，`c:\users\x` 与 `C:\Users\X` 是同一处）；
/// - 匹配后面必须是边界（结尾或非字母数字）——否则 `C:\Users\Bo` 会把 `C:\Users\Bobby\…`
///   削成 `%USERPROFILE%bby\…`，脱敏变成造错；
/// - `profile` 为空时原样返回（取不到用户名不是"文本整段该消失"的理由）。
pub fn redact_path(text: &str, profile: &str) -> String {
    if profile.is_empty() {
        return text.to_string();
    }
    let n = profile.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if rest.len() >= n {
            if let Some(head) = rest.get(..n) {
                if head.eq_ignore_ascii_case(profile) {
                    let after = &rest[n..];
                    let boundary = after
                        .chars()
                        .next()
                        .is_none_or(|c| !c.is_alphanumeric());
                    if boundary {
                        out.push_str("%USERPROFILE%");
                        i += n;
                        continue;
                    }
                }
            }
        }
        let ch = rest.chars().next().expect("i 在 len 之内，至少还有一个字符");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// 从注册表读系统名与版本号：`Windows 11 专业版 24H2（构建 26100.1742）`。
///
/// 三格都读不到 ⇒ `Windows（版本未知）`——**不臆造**一个版本号：报告里写错版本比写"不知道"更误事。
/// `UBR` 是 `CurrentBuildNumber` 的小号（`26100.1742` 里那个 1742），单独一格 `REG_DWORD`。
pub fn os_version() -> String {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };

    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

    fn reg_string(name: &str) -> Option<String> {
        let key_w: Vec<u16> = KEY.encode_utf16().chain(std::iter::once(0)).collect();
        let name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buf = [0u16; 256];
        let mut size = (buf.len() * std::mem::size_of::<u16>()) as u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key_w.as_ptr()),
                PCWSTR(name_w.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
                Some(&mut size),
            )
        };
        if rc != windows::Win32::Foundation::ERROR_SUCCESS {
            return None;
        }
        // size 是含结尾 NUL 的字节数。
        let len = (size as usize / 2).saturating_sub(1).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..len]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    fn reg_u32(name: &str) -> Option<u32> {
        let key_w: Vec<u16> = KEY.encode_utf16().chain(std::iter::once(0)).collect();
        let name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key_w.as_ptr()),
                PCWSTR(name_w.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some((&mut value as *mut u32).cast()),
                Some(&mut size),
            )
        };
        (rc == windows::Win32::Foundation::ERROR_SUCCESS).then_some(value)
    }

    let name = reg_string("ProductName");
    let display = reg_string("DisplayVersion");
    let build = reg_string("CurrentBuildNumber");
    let ubr = reg_u32("UBR");
    if name.is_none() && build.is_none() {
        return crate::i18n::t("sysinfo.no_version").to_string();
    }
    let mut head = name.unwrap_or_else(|| "Windows".to_string());
    if let Some(v) = display {
        head.push(' ');
        head.push_str(&v);
    }
    match (build, ubr) {
        (Some(b), Some(u)) => crate::i18n::tfmt("sysinfo.build_ubr", &[&head, &b, &u]),
        (Some(b), None) => crate::i18n::tfmt("sysinfo.build", &[&head, &b]),
        _ => head,
    }
}

/// 取 `%USERPROFILE%`：报告里脱敏用的那一份。取不到返回空串（[`redact_path`] 会原样放行）。
pub fn user_profile() -> String {
    std::env::var_os("USERPROFILE")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 路径脱敏的便捷包装：先 `display()` 再 [`redact_path`]。
pub fn redact(p: &Path, profile: &str) -> String {
    redact_path(&p.display().to_string(), profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_replaces_the_profile_prefix_and_keeps_the_tail() {
        let got = redact_path(
            r"程序 C:\Users\baoqi\AppData\Local\egui-launcher\launcher.log",
            r"C:\Users\baoqi",
        );
        assert_eq!(got, r"程序 %USERPROFILE%\AppData\Local\egui-launcher\launcher.log");
    }

    #[test]
    fn redact_is_case_insensitive_like_windows_paths() {
        let got = redact_path(r"c:\users\BAOQI\x", r"C:\Users\baoqi");
        assert_eq!(got, r"%USERPROFILE%\x");
    }

    #[test]
    fn redact_requires_a_boundary_so_bobby_is_not_boba() {
        // 前缀撞车：`Bo` 是 `Bobby` 的前缀，但不是一个独立的路径段 ⇒ 不许动。
        let got = redact_path(r"C:\Users\Bobby\x", r"C:\Users\Bo");
        assert_eq!(got, r"C:\Users\Bobby\x");
        // 独立的路径段结尾也算边界。
        let got = redact_path(r"C:\Users\Bo", r"C:\Users\Bo");
        assert_eq!(got, "%USERPROFILE%");
    }

    #[test]
    fn redact_handles_repeats_and_empty_profile() {
        let got = redact_path(r"C:\Users\a 和 C:\Users\a", r"C:\Users\a");
        assert_eq!(got, "%USERPROFILE% 和 %USERPROFILE%");
        // 取不到用户名 ⇒ 原样放行（不能因为脱敏失败就把整段路径吞掉）。
        assert_eq!(redact_path(r"C:\Users\a\x", ""), r"C:\Users\a\x");
    }

    #[test]
    fn redact_does_not_mangle_multibyte_text() {
        let got = redact_path("中文前缀 C:\\Users\\小明\\桌面", r"C:\Users\小明");
        assert_eq!(got, "中文前缀 %USERPROFILE%\\桌面");
    }

    #[test]
    fn report_layout_is_pinned() {
        let f = SystemFacts {
            version: "0.1.0".to_string(),
            build_kind: "debug".to_string(),
            log_level: "详细（跟随构建）".to_string(),
            os: "Windows 11 专业版 24H2（构建 26100.1742）".to_string(),
            arch: "x86_64".to_string(),
            backend: "OpenGL（wgpu GL 后端）".to_string(),
            dpi: "150%（1.50 px/pt）".to_string(),
            appearance: "Modern｜字号 22｜行高 40｜甲·紧凑｜窗口 500×438".to_string(),
            paths: vec![
                ("程序", r"C:\app\egui-launcher.exe".to_string()),
                ("数据", r"C:\app".to_string()),
            ],
            counts: vec![("清单", 142), ("隐藏", 2)],
            sources: "开 应用、网页；关 —".to_string(),
            hotkeys: vec![
                ("呼出", "Ctrl+Alt+Space".to_string()),
                ("补字符", "—".to_string()),
            ],
        };
        let text = build_report(&f);
        let expected = "egui Launcher 0.1.0 · debug 构建\n\
日志级别：详细（跟随构建）\n\
系统：Windows 11 专业版 24H2（构建 26100.1742）\n\
架构：x86_64\n\
渲染：OpenGL（wgpu GL 后端）\n\
缩放：150%（1.50 px/pt）\n\
外观：Modern｜字号 22｜行高 40｜甲·紧凑｜窗口 500×438\n\
路径：\n  \
程序 C:\\app\\egui-launcher.exe\n  \
数据 C:\\app\n\
计数：清单 142｜隐藏 2\n\
数据源：开 应用、网页；关 —\n\
热键：呼出 Ctrl+Alt+Space｜补字符 —\n";
        assert_eq!(text, expected);
        assert!(!text.contains("用户"), "报告里不该出现用户名这类痕迹");
    }

    #[test]
    fn empty_facts_still_produce_a_readable_skeleton() {
        // 全空是"字段没填全"的形态，不是要 panic 的形态：行还在，值是空的。
        let text = build_report(&SystemFacts::default());
        assert!(text.starts_with("egui Launcher  ·  构建\n"));
        assert!(text.contains("计数：\n"));
        assert!(text.ends_with("热键：\n"));
    }

    #[test]
    fn os_version_is_either_read_or_explicitly_unknown() {
        // 不断言具体版本号（换机器 / 换系统就会变）：只钉"要么读到一个构建号，要么明确说不知道"。
        let v = os_version();
        assert!(
            v.contains("构建") || v.ends_with("（版本未知）"),
            "系统版本行必须是可读的两种形态之一，实际：{v}"
        );
        assert!(!v.contains('\0'), "注册表读出来的串不能带 NUL");
    }
}

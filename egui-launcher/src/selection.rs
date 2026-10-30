//! 打磨档 G3（选中内容动作面板）：把「在任何程序里选中的东西」变成一个操作面板对象。
//!
//! 行为整条照 AHK 版 `Src/Extensions/SelectionActions.ahk` 的 `Capture()` + `ItemFor()`：
//! 记住剪贴板 → 清空 → 发 `^c`（控制台类窗口改发 `^{Insert}`）→ 等剪贴板 → 还原 →
//! 按 CF_HDROP / 网址 / 路径 / 文本分类 → 交给操作面板。
//!
//! **Win32 那一半不在这里**（记剪贴板、发键、等剪贴板都在 `main.rs`）；这里只放三件能纯函数
//! 单测的事：① 哪些窗口类名要用 `Ctrl+Insert`；② `CF_HDROP` 的字节怎么读、读出来的东西算哪一类；
//! ③ 面板标题那行单行预览。
//!
//! 一处**有意偏差**（甲案带来的）：AHK 多选文件只给一条「全部添加为命令」，本程序把多选直接送进
//! 已有的批量面板（FR-ACT-05，批次 E4 的七条），动作只会更多、不会更少。

/// AHK `ClipWait(0.8, 1)`：发出复制键之后等剪贴板出现内容的上限。
pub const CLIP_WAIT_MS: u64 = 800;

/// 预览最多保留多少个字符（AHK `SubStr($preview, 1, 80) "..."`，按字符数不按字节数）。
pub const PREVIEW_CHARS: usize = 80;

/// `DROPFILES` 头的字节数：`DWORD pFiles` + `POINT pt`(8) + `BOOL fNC`(4) + `BOOL fWide`(4)。
/// 后面跟着的那串路径从 `pFiles` 指的偏移开始，正常就等于这个数。
pub const DROPFILES_HEADER_LEN: usize = 20;

/// AHK `SelectionActions.ConsoleClasses`：这些窗口里 `Ctrl+C` 是"中断正在运行的程序"，
/// 复制得改用 `Ctrl+Insert`。逐字照搬那五个类名（大小写不敏感比较，AHK 的正则带 `i`）。
pub const CONSOLE_CLASSES: [&str; 5] = [
    "ConsoleWindowClass",
    "CASCADIA_HOSTING_WINDOW_CLASS",
    "mintty",
    "PuTTY",
    "VirtualConsoleClass",
];

/// 抓回来该按哪颗键复制。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyKey {
    /// 普通窗口：`Ctrl+C`。
    C,
    /// 控制台类窗口：`Ctrl+Insert`（不打断里面的程序）。
    Insert,
}

/// 按前台窗口类名决定复制键（`""` = 取不到类名 ⇒ 按普通窗口处理，与 AHK 的 `try` 兜底同效）。
pub fn copy_key_for(class_name: &str) -> CopyKey {
    if is_console_class(class_name) {
        CopyKey::Insert
    } else {
        CopyKey::C
    }
}

/// 是不是控制台/终端类窗口（AHK 那条正则的等义全文匹配）。
pub fn is_console_class(name: &str) -> bool {
    CONSOLE_CLASSES
        .iter()
        .any(|c| c.eq_ignore_ascii_case(name))
}

/// 一次抓取分类出来的对象。**只有这四类**，与 AHK `ItemFor` 的分支一一对应：
/// `{Files:[len==1]}` → 文件/目录、`{Files:[len>1]}` → 批量、网址串 → 网址、其余非空文本 → 文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// 文件或目录。单个还是多个由 `len()` 决定，是文件还是目录由调用方按盘上事实判。
    Files(Vec<String>),
    /// `^https?://\S+$` 的串（保留 trim 之后的样子）。
    Url(String),
    /// 其余非空文本：**原文**（去掉首尾空白只在判空和预览里用，粘贴出去的是原文）。
    Text(String),
}

/// 剪贴板里拿到的 `CF_HDROP` 那一批路径。空批（AHK `if files.Length` 那一格）⇒ `None`。
pub fn classify_files(paths: Vec<String>) -> Option<Target> {
    if paths.is_empty() {
        None
    } else {
        Some(Target::Files(paths))
    }
}

/// 剪贴板里拿到的文本（不是 `CF_HDROP` 时走这条）。分类顺序逐格照 AHK `ItemFor`：
/// 网址 → 单行且像路径且**盘上确实有** → 文本。
///
/// `exists` 由调用方注入（AHK 是 `FileExist`，对目录同样返回真）；抽成参数是为了能纯函数单测。
pub fn classify_text(text: &str, exists: impl Fn(&str) -> bool) -> Option<Target> {
    let trimmed = trim_selection_ws(text);
    if trimmed.is_empty() {
        return None;
    }
    if is_url(trimmed) {
        return Some(Target::Url(trimmed.to_string()));
    }
    // AHK 的第一条前置是 `!InStr(trimmed, "`n")`：多行即使像路径也不当路径。
    if !trimmed.contains('\n') && is_pathlike(trimmed) && exists(trimmed) {
        return Some(Target::Files(vec![trimmed.to_string()]));
    }
    Some(Target::Text(text.to_string()))
}

/// `^https?://\S+$`（AHK 带 `i`，且 `\S` 不许中间夹空白/换行）。
fn is_url(s: &str) -> bool {
    // 按字节比前缀：两段都是纯 ASCII，命中时切片的边界一定合法。
    let b = s.as_bytes();
    let rest = if b.len() >= 8 && b[..8].eq_ignore_ascii_case(b"https://") {
        &s[8..]
    } else if b.len() >= 7 && b[..7].eq_ignore_ascii_case(b"http://") {
        &s[7..]
    } else {
        return false;
    };
    !rest.is_empty() && !rest.chars().any(is_selection_ws)
}

/// `^(?:[A-Za-z]:\\|\\\\)`：盘符路径或 UNC。
fn is_pathlike(s: &str) -> bool {
    let mut chars = s.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(a), Some(':'), Some('\\')) => a.is_ascii_alphabetic(),
        (Some('\\'), Some('\\'), _) => true,
        _ => false,
    }
}

/// AHK `Trim(text, " `t`r`n")` 的等义实现：只去这五个字符，不去 NBSP 之类。
fn is_selection_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n' | '\u{b}' | '\u{c}')
}

fn trim_selection_ws(s: &str) -> &str {
    s.trim_matches(is_selection_ws)
}

/// 面板标题那行预览：空白折叠成单空格、去首尾、超过 [`PREVIEW_CHARS`] 个字符截断并加 `...`。
/// 用来当面板 `title`，所以**只剩这一行**——选中内容里的换行不会漏进标题。
pub fn preview(text: &str) -> String {
    let joined = text
        .split(is_selection_ws)
        .filter(|piece| !piece.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if joined.chars().count() > PREVIEW_CHARS {
        let mut out: String = joined.chars().take(PREVIEW_CHARS).collect();
        out.push_str("...");
        out
    } else {
        joined
    }
}

/// 读 `CF_HDROP` 载荷（`DROPFILES` 头 + 双 NUL 结尾的路径表）。读不出来 ⇒ `None`。
///
/// 宽字符与 ANSI 两种都认（`fWide`），中文路径只有宽字符那一支不会丢；表项按出现顺序返回。
/// `build_dropfiles`（`main.rs`，批次 E4）写的就是这个形状，这里只管读。
pub fn parse_hdrop(bytes: &[u8]) -> Option<Vec<String>> {
    if bytes.len() < DROPFILES_HEADER_LEN {
        return None;
    }
    let offset = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as usize;
    let wide = u32::from_le_bytes(bytes[16..20].try_into().ok()?) != 0;
    let body = bytes.get(offset..)?;
    let mut out = Vec::new();
    if wide {
        let mut cur: Vec<u16> = Vec::new();
        for pair in body.chunks_exact(2) {
            let unit = u16::from_le_bytes([pair[0], pair[1]]);
            if unit == 0 {
                // 空项 + 结尾 0 ⇒ 双 NUL 收尾；正常数据到这里就停。
                if cur.is_empty() {
                    break;
                }
                out.push(String::from_utf16_lossy(&cur));
                cur.clear();
            } else {
                cur.push(unit);
            }
        }
        // 没写结尾双 NUL 的坏数据：剩下的照样收下（AHK 的 `StrSplit` 也不会吞最后一条）。
        if !cur.is_empty() {
            out.push(String::from_utf16_lossy(&cur));
        }
    } else {
        let mut cur: Vec<u8> = Vec::new();
        for &b in body {
            if b == 0 {
                if cur.is_empty() {
                    break;
                }
                out.push(String::from_utf8_lossy(&cur).into_owned());
                cur.clear();
            } else {
                cur.push(b);
            }
        }
        if !cur.is_empty() {
            out.push(String::from_utf8_lossy(&cur).into_owned());
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按 `DROPFILES` 的字节布局手搓一份载荷（宽字符）。
    /// 与 `main.rs` 的 `build_dropfiles` 是同一个形状——那边用 Win32 结构体填头，
    /// 这里直接写字面量，好让"读"这一侧的单测不依赖 Win32。
    fn hdrop_wide(paths: &[&str]) -> Vec<u8> {
        let mut data = vec![0u8; DROPFILES_HEADER_LEN];
        data[0..4].copy_from_slice(&(DROPFILES_HEADER_LEN as u32).to_le_bytes());
        data[16..20].copy_from_slice(&1u32.to_le_bytes()); // fWide = TRUE
        for p in paths {
            for unit in p.encode_utf16() {
                data.extend_from_slice(&unit.to_le_bytes());
            }
            data.extend_from_slice(&0u16.to_le_bytes());
        }
        data.extend_from_slice(&0u16.to_le_bytes());
        data
    }

    #[test]
    fn hdrop_wide_round_trip_keeps_order_and_chinese() {
        let fixture = hdrop_wide(&["C:\\a.txt", "D:\\中文 目录\\b"]);
        assert_eq!(
            parse_hdrop(&fixture),
            Some(vec![
                "C:\\a.txt".to_string(),
                "D:\\中文 目录\\b".to_string()
            ])
        );
    }

    #[test]
    fn hdrop_ansi_is_read_as_bytes() {
        let mut data = vec![0u8; DROPFILES_HEADER_LEN];
        data[0..4].copy_from_slice(&(DROPFILES_HEADER_LEN as u32).to_le_bytes());
        // fWide 保持 0 = ANSI
        data.extend_from_slice(b"C:\\x.txt\0\0");
        assert_eq!(parse_hdrop(&data), Some(vec!["C:\\x.txt".to_string()]));
    }

    #[test]
    fn hdrop_missing_double_nul_still_keeps_last_entry() {
        let mut data = vec![0u8; DROPFILES_HEADER_LEN];
        data[0..4].copy_from_slice(&(DROPFILES_HEADER_LEN as u32).to_le_bytes());
        data[16..20].copy_from_slice(&1u32.to_le_bytes());
        for unit in "C:\\y.txt".encode_utf16() {
            data.extend_from_slice(&unit.to_le_bytes());
        }
        data.extend_from_slice(&0u16.to_le_bytes());
        assert_eq!(parse_hdrop(&data), Some(vec!["C:\\y.txt".to_string()]));
    }

    #[test]
    fn hdrop_garbage_is_none() {
        assert_eq!(parse_hdrop(&[]), None);
        assert_eq!(parse_hdrop(&[0u8; 4]), None);
        // 头够长但偏移指到界外
        let mut bad = vec![0u8; DROPFILES_HEADER_LEN];
        bad[0..4].copy_from_slice(&9999u32.to_le_bytes());
        assert_eq!(parse_hdrop(&bad), None);
        // 只有头、没有表
        assert_eq!(parse_hdrop(&vec![0u8; DROPFILES_HEADER_LEN]), None);
    }

    #[test]
    fn console_classes_match_case_insensitively() {
        for c in CONSOLE_CLASSES {
            assert!(is_console_class(c));
            assert_eq!(copy_key_for(c), CopyKey::Insert);
            assert!(is_console_class(&c.to_ascii_uppercase()));
        }
        assert!(!is_console_class("Notepad"));
        assert!(!is_console_class(""));
        assert!(!is_console_class("ConsoleWindowClassX"));
        assert_eq!(copy_key_for("Notepad"), CopyKey::C);
        assert_eq!(copy_key_for(""), CopyKey::C);
    }

    #[test]
    fn empty_or_blank_text_is_nothing() {
        assert_eq!(classify_text("", |_| true), None);
        assert_eq!(classify_text("  \r\n\t ", |_| true), None);
    }

    #[test]
    fn url_is_taken_and_trimmed() {
        assert_eq!(
            classify_text("  https://example.com/a?b=1 \r\n", |_| false),
            Some(Target::Url("https://example.com/a?b=1".to_string()))
        );
        // AHK 的正则带 `i`
        assert_eq!(
            classify_text("HTTP://EXAMPLE.COM", |_| false),
            Some(Target::Url("HTTP://EXAMPLE.COM".to_string()))
        );
        // 中间夹空白 / 换行就不算网址
        assert_eq!(
            classify_text("https://a.com b", |_| false),
            Some(Target::Text("https://a.com b".to_string()))
        );
        assert_eq!(
            classify_text("https://a.com\n第二行", |_| false),
            Some(Target::Text("https://a.com\n第二行".to_string()))
        );
        // 只有协议头也不算
        assert_eq!(
            classify_text("https://", |_| false),
            Some(Target::Text("https://".to_string()))
        );
        assert_eq!(
            classify_text("ftp://a.com", |_| false),
            Some(Target::Text("ftp://a.com".to_string()))
        );
    }

    #[test]
    fn existing_single_line_path_becomes_a_file_target() {
        let t = classify_text("D:\\tmp\\a.txt", |p| p == "D:\\tmp\\a.txt");
        assert_eq!(
            t,
            Some(Target::Files(vec!["D:\\tmp\\a.txt".to_string()]))
        );
        // UNC
        let t = classify_text("\\\\server\\share\\a.txt", |_| true);
        assert_eq!(
            t,
            Some(Target::Files(vec!["\\\\server\\share\\a.txt".to_string()]))
        );
    }

    #[test]
    fn pathlike_but_missing_or_multiline_stays_text() {
        // 盘上没有 ⇒ 文本（AHK `FileExist` 那条）
        assert_eq!(
            classify_text("D:\\nope\\a.txt", |_| false),
            Some(Target::Text("D:\\nope\\a.txt".to_string()))
        );
        // 多行即使第一行是存在的路径也不当路径
        assert_eq!(
            classify_text("D:\\a.txt\r\nD:\\b.txt", |_| true),
            Some(Target::Text("D:\\a.txt\r\nD:\\b.txt".to_string()))
        );
        // 相对路径本来就不像路径
        assert_eq!(
            classify_text("tmp\\a.txt", |_| true),
            Some(Target::Text("tmp\\a.txt".to_string()))
        );
        // 盘符只有一位字母
        assert_eq!(
            classify_text("1:\\a.txt", |_| true),
            Some(Target::Text("1:\\a.txt".to_string()))
        );
    }

    #[test]
    fn text_target_keeps_original_whitespace() {
        let raw = "  第一行\r\n第二行  ";
        assert_eq!(
            classify_text(raw, |_| false),
            Some(Target::Text(raw.to_string()))
        );
    }

    #[test]
    fn files_classification_rejects_empty_batch() {
        assert_eq!(classify_files(Vec::new()), None);
        assert_eq!(
            classify_files(vec!["a".to_string()]),
            Some(Target::Files(vec!["a".to_string()]))
        );
    }

    #[test]
    fn preview_collapses_whitespace_and_truncates() {
        assert_eq!(preview("  a \r\n\t b   c  "), "a b c");
        assert_eq!(preview("单行"), "单行");
        assert_eq!(preview(""), "");
        let long = "字".repeat(120);
        let p = preview(&long);
        assert_eq!(p.chars().count(), PREVIEW_CHARS + 3);
        assert!(p.ends_with("..."));
        // 恰好 80 个字符不加省略号（AHK 是 `> 80`）
        let exact = "字".repeat(PREVIEW_CHARS);
        assert_eq!(preview(&exact), exact);
    }

    #[test]
    fn preview_never_leaks_newlines() {
        let p = preview("a\nb");
        assert!(!p.contains('\n'));
        assert_eq!(p, "a b");
    }
}

//! 批次 D4（FR-CFG-06）：添加/编辑对话框的「选文件 / 选目录 / 拖拽导入」共用的取值规则。
//!
//! 口径照 Delphi 两个文件，**两处规则不一样，别混**：
//! - 选文件/选目录（`frmShortCut.pas` 的 `btnFileClick` / `btnDirClick`）＝ 只用选中的路径，
//!   名字＝去扩展名的文件名 / 目录末段；**不解引用 `.lnk`**（`.lnk` 交给 ShellExecute 直接启动）。
//! - 拖进来（`frmShortCut.pas` 的 `WM_DropFiles` → `untShortCutMan.pas` 的
//!   `ExtractShortCutItemFromFileName`）＝ 整套导入规则：`.lnk` 解引用、`%windir%\Installer\{…}`
//!   跳过、`http(s)://` 取域名、Chrome `--app=` 那种从参数里取名字。
//!
//! 这里只放**纯函数**：`.lnk` 解引用要 COM，调用方（`main.rs`）做完再把目标传进来，
//! 于是本模块的每条规则都能在单测里逐支路钉住。

use super::application::{is_url, ItemKind};

/// 一次「填进表单」的完整取值。名称/别名的冲突判定要跟表单现值比，归 UI 层。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFill {
    pub path: String,
    pub arguments: String,
    pub working_dir: String,
    pub keyword: String,
    pub name: String,
    pub kind: ItemKind,
}

/// 去外层引号（前后都带才去）并修掉首尾空白。Delphi 的 `RemoveQuotationMark` 只去前引号，
/// 而 `ExtractShortCutItemFromFileName` 是"解析后另剥结尾引号"——两处合在这里。
pub fn unquote(s: &str) -> &str {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

/// Delphi `ExtractFileName`：按最后一个 `\` **或 `:`** 切分。
///
/// 冒号不是笔误，它正是 Chrome 网页应用能导入出正确名字的原因：快捷方式解析出来是
/// `"C:\…\chrome.exe" --app=https://example.com`，最后一个 `:` 落在 `https:` 上，
/// 切出来正好是 `//example.com`，再走 [`derive_names`] 的 `//` 分支。
pub fn extract_file_name(s: &str) -> &str {
    match s.rfind(['\\', ':']) {
        Some(i) => &s[i + 1..],
        None => s,
    }
}

/// Delphi `ExtractFileExt` 的剥法：最后一个 `.` 起（含点）全算扩展名。
///
/// Delphi 对 `.gitignore` 这种"点开头的整名"会剥成空串；这里剥空了就退回原名——
/// 空名字在表单里没法用，是本仓的有意偏差。
pub fn strip_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

/// 命名规则（`ExtractShortCutItemFromFileName` 那一半），返回 `(别名, 显示名)`——
/// Delphi 里两者都取同一个 `ShortCut`。`is_dir` 由调用方判定，单测因此不碰文件系统。
pub fn derive_names(command_line: &str, is_dir: bool) -> (String, String) {
    let raw = unquote(command_line);
    // 尾分隔符先剥：目录选择框对盘根以外的目录不返尾 `\`，但拖放与手输会给，
    // 留着它 `extract_file_name` 只会切出一个空名字。
    let raw = raw.trim_end_matches(['\\', '/']);
    if raw.is_empty() {
        return (String::new(), String::new());
    }
    // ① 整行就是一个 http(s) 网址 ⇒ 别名取主机名（FR-CFG-06「http(s) 用域名作快捷名」）。
    //    Delphi 只认 `http://`（`Pos('http://', LowerCase(FileName)) = 1`）；`https://` 在
    //    Delphi 里落到 ②③ 两档，由 `ExtractFileName` 的冒号切分接住，结果同样是主机名。
    let lower = raw.to_ascii_lowercase();
    for scheme in ["http://", "https://"] {
        if lower.starts_with(scheme) {
            let rest = &raw[scheme.len()..];
            let host = rest.split('/').next().unwrap_or("").trim();
            return (host.to_string(), host.to_string());
        }
    }
    // ② 文件名。`ExtractFileName` 之后还会剩命令行结尾那个引号（`"C:\a\b.exe"` ⇒ `b.exe"`）。
    let file = extract_file_name(raw).trim().trim_end_matches('"').trim();
    if file.is_empty() {
        return (String::new(), String::new());
    }
    // ③ Chrome `--app=` 留下的 `//host/path`：去掉 `//` 与尾部 `/`。
    if let Some(rest) = file.strip_prefix("//") {
        let name = rest.trim_end_matches('/');
        return (name.to_string(), name.to_string());
    }
    // ④ 目录不剥扩展名：「1.2图片」这种目录名会被剥成「1」（Delphi 注释点名了这一格）。
    if is_dir {
        return (file.to_string(), file.to_string());
    }
    // ⑤ 普通文件剥扩展名。
    let stem = strip_extension(file);
    let stem = if stem.is_empty() { file } else { stem };
    (stem.to_string(), stem.to_string())
}

/// 选中/拖入的东西该落哪一档。只在按钮与拖放这两条入口用；编辑既有条目时不改用户选的类型。
pub fn kind_for(path: &str, is_dir: bool) -> ItemKind {
    if is_dir {
        return ItemKind::Folder;
    }
    if is_url(path) {
        return ItemKind::Url;
    }
    let low = path.to_ascii_lowercase();
    if low.ends_with(".exe") || low.ends_with(".lnk") || low.ends_with(".com") {
        ItemKind::App
    } else {
        ItemKind::File
    }
}

/// 「选文件…」「选目录…」的取值（Delphi `btnFileClick` / `btnDirClick`）。
pub fn fill_from_pick(path: &str, is_dir: bool) -> ImportFill {
    let path = path.trim().to_string();
    let (keyword, name) = derive_names(&path, is_dir);
    ImportFill {
        kind: kind_for(&path, is_dir),
        path,
        arguments: String::new(),
        working_dir: String::new(),
        keyword,
        name,
    }
}

/// 「拖进来」的取值：调用方先把 `.lnk` 解引用好，这里把目标与参数合成一整行再切名字。
///
/// 参数必须参与命名：Delphi 的 `ResolveLink` 返回的就是 `target + ' ' + arguments` 一整条，
/// Chrome 的 `--app=` 特例正是靠这一步才成立。
pub fn fill_from_import(
    target: &str,
    arguments: &str,
    working_dir: &str,
    is_dir: bool,
) -> ImportFill {
    let target = target.trim().to_string();
    let arguments = arguments.trim().to_string();
    let cmdline = if arguments.is_empty() {
        target.clone()
    } else {
        format!("{target} {arguments}")
    };
    let (keyword, name) = derive_names(&cmdline, is_dir);
    ImportFill {
        kind: kind_for(&target, is_dir),
        path: target,
        arguments,
        working_dir: working_dir.trim().to_string(),
        keyword,
        name,
    }
}

/// Delphi 的那条跳过规则：`.lnk` 解析到 `%windir%\Installer\{…}` 里（Office 的图标缓存之类）
/// 时**不用**解析结果，仍按 `.lnk` 本身启动，否则装完 Office 拖出来的快捷方式会指向一个
/// 几乎启动不了的缓存 exe。
///
/// 比较不分大小写：Delphi 的 `Pos` 区分大小写，而 `windir` 的实际写法（`C:\WINDOWS` 还是
/// `C:\Windows`）不由我们决定，照抄会漏掉一半机器。
pub fn is_installer_target(target: &str, windir: &str) -> bool {
    let windir = windir.trim().trim_end_matches(['\\', '/']);
    if windir.is_empty() {
        return false;
    }
    target
        .to_ascii_lowercase()
        .contains(&format!("{}\\installer\\{{", windir.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_file_name_splits_on_backslash_and_colon() {
        assert_eq!(extract_file_name(r"C:\Windows\notepad.exe"), "notepad.exe");
        // Delphi 的 `ExtractFileName` 把冒号也当分隔符——这一条是 Chrome 那套的根：
        // 最后一个分隔符落在参数里那个 `https:` 上，切出来就是 `//shit.com`。
        assert_eq!(
            extract_file_name(r#""C:\a\b.exe" --app=https://shit.com"#),
            "//shit.com"
        );
        assert_eq!(extract_file_name("notepad.exe"), "notepad.exe");
        assert_eq!(extract_file_name(r"D:1.txt"), "1.txt");
    }

    #[test]
    fn strip_extension_keeps_dot_files_whole() {
        assert_eq!(strip_extension("notepad.exe"), "notepad");
        assert_eq!(strip_extension("1.2图片"), "1");
        assert_eq!(strip_extension(".gitignore"), ".gitignore", "剥空就退回原名");
        assert_eq!(strip_extension("noext"), "noext");
    }

    #[test]
    fn unquote_strips_only_a_matching_pair() {
        assert_eq!(unquote(r#"  "C:\a b\c.exe"  "#), r"C:\a b\c.exe");
        assert_eq!(unquote(r#"C:\a\c.exe"#), r#"C:\a\c.exe"#);
        assert_eq!(unquote(r#""C:\a\c.exe"#), r#""C:\a\c.exe"#);
    }

    #[test]
    fn derive_names_follows_the_delphi_branches() {
        // 普通文件：剥扩展名。
        assert_eq!(
            derive_names(r"C:\Program Files\Notepad++\notepad++.exe", false),
            ("notepad++".into(), "notepad++".into())
        );
        // 目录：不剥扩展名（Delphi 注释点名「1.2图片」）。
        assert_eq!(
            derive_names(r"C:\temp\1.2图片", true),
            ("1.2图片".into(), "1.2图片".into())
        );
        // 整行就是一个网址：取主机名。
        assert_eq!(
            derive_names("http://www.douban.com/group/topic/4876691", false),
            ("www.douban.com".into(), "www.douban.com".into())
        );
        assert_eq!(
            derive_names("https://shit.com/", false),
            ("shit.com".into(), "shit.com".into())
        );
        // Chrome 网页应用：参数里的 URL 被冒号切开 ⇒ `//host/path` ⇒ 去 `//` 与尾 `/`。
        assert_eq!(
            derive_names(
                r#""C:\Program Files\Google\Chrome\Application\chrome.exe" --app=https://shit.com"#,
                false
            ),
            ("shit.com".into(), "shit.com".into())
        );
        assert_eq!(
            derive_names(
                r#"C:\Program Files\Google\Chrome\Application\chrome.exe --app=http://www.douban.com/group/topic/4876691/"#,
                false
            ),
            (
                "www.douban.com/group/topic/4876691".into(),
                "www.douban.com/group/topic/4876691".into()
            )
        );
        // 带引号的整条命令行：`ExtractFileName` 之后只剩结尾引号。
        assert_eq!(
            derive_names(r#""C:\a b\c.exe""#, false),
            ("c".into(), "c".into())
        );
        assert_eq!(derive_names("   ", false), (String::new(), String::new()));
    }

    #[test]
    fn kind_for_separates_the_four_forms() {
        assert_eq!(kind_for(r"C:\temp", true), ItemKind::Folder);
        assert_eq!(kind_for(r"C:\Windows\notepad.exe", false), ItemKind::App);
        assert_eq!(kind_for(r"C:\a\报告.pdf", false), ItemKind::File);
        assert_eq!(kind_for("https://example.com", false), ItemKind::Url);
        // 文件名的尾串不许误判成 exe。
        assert_eq!(kind_for(r"C:\a\exe.txt", false), ItemKind::File);
    }

    #[test]
    fn installer_target_check_is_case_insensitive_and_anchored() {
        assert!(is_installer_target(
            r"C:\WINDOWS\Installer\{90120000-0011-0000-0000-0000000FF1CE}\wordicon.exe",
            r"C:\WINDOWS"
        ));
        assert!(is_installer_target(
            r"c:\windows\installer\{AB}\x.exe",
            r"C:\Windows"
        ));
        assert!(!is_installer_target(
            r"C:\Windows\System32\notepad.exe",
            r"C:\Windows"
        ));
        // `windir` 为空（环境变量读不到）时不做判断，宁可照常解引用。
        assert!(!is_installer_target(
            r"C:\WINDOWS\Installer\{AB}\x.exe",
            ""
        ));
    }

    #[test]
    fn fill_from_pick_leaves_the_lnk_alone() {
        let f = fill_from_pick(r"C:\Users\me\Desktop\微信.lnk", false);
        assert_eq!(f.path, r"C:\Users\me\Desktop\微信.lnk");
        assert_eq!(f.keyword, "微信");
        assert_eq!(f.kind, ItemKind::App);
        assert!(f.arguments.is_empty() && f.working_dir.is_empty());

        let d = fill_from_pick(r"C:\temp\1.2图片\", true);
        assert_eq!(d.keyword, "1.2图片");
        assert_eq!(d.kind, ItemKind::Folder);
    }

    #[test]
    fn fill_from_import_puts_arguments_into_the_naming_line() {
        let f = fill_from_import(
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            "--app=http://www.douban.com/group/topic/4876691/",
            r"C:\Program Files\Google\Chrome\Application",
            false,
        );
        assert_eq!(f.keyword, "www.douban.com/group/topic/4876691");
        assert_eq!(f.name, f.keyword);
        assert_eq!(f.path, r"C:\Program Files\Google\Chrome\Application\chrome.exe");
        assert_eq!(f.arguments, "--app=http://www.douban.com/group/topic/4876691/");
        assert_eq!(f.kind, ItemKind::App);

        // 没有参数时就是普通文件：命名只看目标本身。
        let g = fill_from_import(r"C:\Windows\notepad.exe", "", "", false);
        assert_eq!(g.keyword, "notepad");
        assert_eq!(g.arguments, "");
    }
}

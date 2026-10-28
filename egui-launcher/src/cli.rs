//! FR-WIN-08 命令行协议：`-Startup` / `-SendTo <路径…>` / `-Preferences <页码>` / `-Reloaded`。
//! 纯解析，不碰 Win32；`-Preferences` 的页码含义由调用方映射（1 起）。
//!
//! 裸路径（不带开关）= SendTo 兼容形态：SendTo 快捷方式刻意不写参数，
//! 由资源管理器把选中路径追加为尾参（见 source/application.rs 的 write_sendto_link）。

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cli {
    pub startup: bool,
    pub reloaded: bool,
    pub send_to: Vec<String>,
    pub preferences: Option<u32>,
}

/// 未知开关只记日志并忽略，不阻断启动——启动器不能因为多一个参数就起不来。
pub fn parse(args: &[String]) -> Cli {
    let mut cli = Cli::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].trim();
        if arg.eq_ignore_ascii_case("-startup") {
            cli.startup = true;
            i += 1;
        } else if arg.eq_ignore_ascii_case("-reloaded") {
            cli.reloaded = true;
            i += 1;
        } else if arg.eq_ignore_ascii_case("-preferences") {
            let raw = args.get(i + 1).map(|s| s.trim()).filter(|s| !is_flag(s));
            cli.preferences = Some(raw.and_then(|s| s.parse::<u32>().ok()).unwrap_or(1));
            i += if raw.is_some() { 2 } else { 1 };
        } else if arg.eq_ignore_ascii_case("-sendto") {
            i += 1;
            while i < args.len() && !is_flag(&args[i]) {
                if !args[i].is_empty() {
                    cli.send_to.push(args[i].clone());
                }
                i += 1;
            }
        } else if is_flag(arg) {
            eprintln!("cli: 未知参数 {arg:?}，已忽略");
            i += 1;
        } else {
            if !arg.is_empty() {
                cli.send_to.push(args[i].clone());
            }
            i += 1;
        }
    }
    cli
}

fn is_flag(s: &str) -> bool {
    // SendTo 递来的都是绝对路径（盘符或 UNC），不会以 `-` 开头。
    s.trim().starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Cli {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn empty_is_plain_activate() {
        assert_eq!(parse_strs(&[]), Cli::default());
    }

    #[test]
    fn startup_and_reloaded_are_flags() {
        assert!(parse_strs(&["-Startup"]).startup);
        assert!(parse_strs(&["-reloaded"]).reloaded);
        let both = parse_strs(&["-Startup", "-Reloaded"]);
        assert!(both.startup && both.reloaded);
    }

    #[test]
    fn raw_paths_are_sendto_compatible() {
        let c = parse_strs(&[r"C:\a\b.lnk", r"D:\x y.txt"]);
        assert_eq!(c.send_to, vec![r"C:\a\b.lnk", r"D:\x y.txt"]);
        assert!(!c.startup && !c.reloaded && c.preferences.is_none());
    }

    #[test]
    fn explicit_sendto_collects_paths_until_next_flag() {
        let c = parse_strs(&["-SendTo", "a.txt", "b.txt", "-Startup"]);
        assert_eq!(c.send_to, vec!["a.txt", "b.txt"]);
        assert!(c.startup);
    }

    #[test]
    fn preferences_reads_next_number() {
        assert_eq!(parse_strs(&["-Preferences", "2"]).preferences, Some(2));
        // 【批次 D5】这里原来拿 8 当"越界"样例，第 8 页「批量管理」落地后 8 是合法页 ⇒ 改 9。
        // 【批次 D8】第 9 页「使用统计」落地 ⇒ 样例再挪到 10。
        assert_eq!(
            parse_strs(&["-Preferences", "10"]).preferences,
            Some(10),
            "越界页码由调用方落回"
        );
    }

    #[test]
    fn preferences_without_number_defaults_to_first_page() {
        assert_eq!(parse_strs(&["-Preferences"]).preferences, Some(1));
        assert_eq!(
            parse_strs(&["-Preferences", "-Startup"]).preferences,
            Some(1)
        );
        assert!(parse_strs(&["-Preferences", "-Startup"]).startup);
    }

    #[test]
    fn unknown_flag_is_ignored_but_does_not_eat_paths() {
        let c = parse_strs(&["-Update", r"C:\a.txt"]);
        assert_eq!(
            c.send_to,
            vec![r"C:\a.txt"],
            "裸路径在未知开关之后仍应被识别为 SendTo"
        );
    }
}

//! FR-ACT-08 运行标志：命令行首/尾的 `@+` / `@-` / `@`。
//!
//! 三档语义照 Delphi：`@+` 最大化、`@-` 最小化、`@` 隐藏。Delphi 把算出来的 `ShowCmd`
//! 交给 `ShellExecute` 的最后一个参数（`untShortCutMan.pas:170-185` / `:341`），本程序交给
//! `ShellExecuteExW` 的 `nShow` —— 同一格，所以"程序自己不认 `nCmdShow` 时标志无效"这条
//! 代价也一并继承（记事本这类拿 `nCmdShow` 当初始显示状态的程序认）。
//!
//! **位置这件事原文与实码不一致，两头都认**：Delphi 只认**前导**（三处判断全是
//! `Pos(flag, Command) = 1`，同段注释原文「去掉前面的 @/@+/@-」），而需求书 FR-ACT-08
//! 原文写「命令行尾」并给了 `notepad.exe@` 这个验收例子 ⇒ 先看前导（Delphi 口径），
//! 没有再看到尾部（原文口径）。一条命令只剥一个标志，**前导优先**；两端都写着时尾部那个
//! 留在命令里 —— 宁可让"找不到文件"把写错的形态暴露出来，也不替用户猜他到底想要哪种。
//!
//! 纯函数、零 I/O：`nShow` 的映射留在 `main.rs`（那边才认识 Win32 常量）。

/// 启动时给目标窗口的状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShowFlag {
    /// 什么都不改（`ShellExecute` 的缺省）。
    #[default]
    Normal,
    /// `@+`
    Maximized,
    /// `@-`
    Minimized,
    /// `@` —— 后台隐藏。
    Hidden,
}

/// 三档的**识别顺序**：长的在前、`@` 放最后。反过来的话 `@+` 会被当成 `@` 剥掉，
/// 剩一个 `+` 拼进命令里变成另一条命令。
const FLAGS: [(&str, ShowFlag); 3] = [
    ("@+", ShowFlag::Maximized),
    ("@-", ShowFlag::Minimized),
    ("@", ShowFlag::Hidden),
];

/// 剥掉命令行首/尾的运行标志；第二格＝这条命令拉起来的窗口该是什么状态。
///
/// 剥完可能是空串（整条命令只写了一个 `@`）—— 那不是合法的启动目标，
/// 由调用方拒绝，这里不替它编一个替代品。
pub fn split(command: &str) -> (&str, ShowFlag) {
    let cmd = command.trim();
    for (text, flag) in FLAGS {
        if let Some(rest) = cmd.strip_prefix(text) {
            return (rest.trim(), flag);
        }
    }
    for (text, flag) in FLAGS {
        if let Some(rest) = cmd.strip_suffix(text) {
            return (rest.trim(), flag);
        }
    }
    (cmd, ShowFlag::Normal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_without_a_flag_is_left_alone() {
        assert_eq!(
            split("C:\\Windows\\notepad.exe"),
            ("C:\\Windows\\notepad.exe", ShowFlag::Normal)
        );
        assert_eq!(split(""), ("", ShowFlag::Normal));
    }

    /// 需求书原文举的例子：`notepad.exe@` 以后台隐藏方式启动。
    #[test]
    fn trailing_flags_follow_the_requirement_document() {
        assert_eq!(split("notepad.exe@"), ("notepad.exe", ShowFlag::Hidden));
        assert_eq!(split("notepad.exe@+"), ("notepad.exe", ShowFlag::Maximized));
        assert_eq!(split("notepad.exe@-"), ("notepad.exe", ShowFlag::Minimized));
    }

    /// Delphi 的实码口径：标志在前（`untShortCutMan.pas:171-185`）。
    #[test]
    fn leading_flags_follow_delphi() {
        assert_eq!(split("@notepad.exe"), ("notepad.exe", ShowFlag::Hidden));
        assert_eq!(split("@+notepad.exe"), ("notepad.exe", ShowFlag::Maximized));
        assert_eq!(split("@-notepad.exe"), ("notepad.exe", ShowFlag::Minimized));
    }

    /// 长的先认：先按 `@` 剥会把 `+` 留成命令的一部分。
    #[test]
    fn the_longer_flag_wins() {
        assert_eq!(split("@+calc.exe"), ("calc.exe", ShowFlag::Maximized));
        assert_eq!(split("calc.exe@+"), ("calc.exe", ShowFlag::Maximized));
    }

    /// 一条命令只剥一个；两端都写着时前导说了算，尾部那个原样留着（见模块头）。
    #[test]
    fn only_one_flag_is_stripped_and_leading_wins() {
        assert_eq!(split("@+notepad.exe@"), ("notepad.exe@", ShowFlag::Maximized));
    }

    #[test]
    fn a_lone_flag_leaves_nothing_to_launch() {
        assert_eq!(split("@"), ("", ShowFlag::Hidden));
        assert_eq!(split("  @+  "), ("", ShowFlag::Maximized));
    }

    #[test]
    fn an_at_sign_in_the_middle_is_not_a_flag() {
        assert_eq!(
            split("C:\\a@b\\app.exe"),
            ("C:\\a@b\\app.exe", ShowFlag::Normal)
        );
        assert_eq!(split("user@example.com"), ("user@example.com", ShowFlag::Normal));
    }

    #[test]
    fn surrounding_whitespace_goes_with_the_flag() {
        assert_eq!(split("  notepad.exe@+  "), ("notepad.exe", ShowFlag::Maximized));
        assert_eq!(split("\t@+notepad.exe "), ("notepad.exe", ShowFlag::Maximized));
    }
}

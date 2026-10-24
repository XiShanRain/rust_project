//! FR-SRC-16 终端命令：`>` 前缀 + 在新终端窗口里执行。
//!
//! 判据全在纯函数 [`route`] 里，只看查询串本身；真正起进程只有 [`TermShell::spawn`] 一处。
//! 前缀固定成 `>`（需求书写着"可配"，本批没做：空串或字母前缀会把普通查询整片吞掉，
//! 而"配错了怎么退出"没有好的答案）。

use std::process::Command;

/// `>` 后面什么都没有时点/按回的那句话。
pub const USAGE_TOAST: &str = "继续输入命令，例：>ipconfig /all";

/// 用哪个 shell 跑。名字是**稳定名**：写进 `settings.json` 的 `terminal_shell`，
/// 与 `sources_off` 同一口径——不认识的名字就回落默认，不让整份配置判坏（E1 的形状）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TermShell {
    #[default]
    Cmd,
    PowerShell,
    Pwsh,
    Wt,
}

impl TermShell {
    pub const ALL: [TermShell; 4] = [Self::Cmd, Self::PowerShell, Self::Pwsh, Self::Wt];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cmd => "cmd",
            Self::PowerShell => "powershell",
            Self::Pwsh => "pwsh",
            Self::Wt => "wt",
        }
    }

    /// 大小写与首尾空白都不参与判断；不认识的返回 `None`，由调用方回落 [`Default`]。
    pub fn from_name(name: &str) -> Option<Self> {
        let want = name.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|s| s.as_str() == want)
    }

    /// 设置页那一行按钮上的字（短，四个并排）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Cmd => "cmd",
            Self::PowerShell => "powershell",
            Self::Pwsh => "pwsh",
            Self::Wt => "Windows Terminal",
        }
    }

    fn program(self) -> &'static str {
        match self {
            Self::Cmd => "cmd.exe",
            Self::PowerShell => "powershell.exe",
            Self::Pwsh => "pwsh.exe",
            Self::Wt => "wt.exe",
        }
    }

    /// 一律"执行完停在窗口里"（`/k` / `-NoExit`）：否则 `>ipconfig` 一闪就关，
    /// 用户连输出都没看清。`wt` 那一档把同样的东西交给 Windows Terminal 的新标签去跑。
    fn args(self, command: &str) -> Vec<String> {
        match self {
            Self::Cmd => vec!["/k".to_string(), command.to_string()],
            Self::PowerShell | Self::Pwsh => {
                vec![
                    "-NoExit".to_string(),
                    "-Command".to_string(),
                    command.to_string(),
                ]
            }
            Self::Wt => vec!["cmd.exe".to_string(), "/k".to_string(), command.to_string()],
        }
    }

    /// 起一个新窗口跑 `command`。
    ///
    /// `CREATE_NEW_CONSOLE` 只给三个控制台程序：本程序是窗口子系统，从终端里启动本程序时
    /// 子进程会**继承**那个终端，`/k` 会把命令塞进用户自己的控制台里，不是新窗口。
    /// `wt.exe` 是 GUI 程序，给它加这个标志反而会凭空多开一个控制台窗口。
    pub fn spawn(self, command: &str) -> std::io::Result<()> {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

        let mut cmd = Command::new(self.program());
        cmd.args(self.args(command));
        if self != Self::Wt {
            cmd.creation_flags(CREATE_NEW_CONSOLE);
        }
        cmd.spawn().map(|_| ())
    }
}

/// [`route`] 的三种结论。`None`（不是我们的）由调用方按 `Option` 处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermRoute<'a> {
    /// 只有前缀（`>`、`>   `）⇒ 出用法行，不执行任何东西。
    Usage,
    /// 前缀后面有命令。已去掉首尾空白，`>` 与命令之间可有空格（`> ipconfig`）。
    Command(&'a str),
}

/// 前缀只认**第 0 个字符**：前导空格归文件模式（FR-SRC-14 第六式），`　>` 之类不抢。
pub fn route(raw: &str) -> Option<TermRoute<'_>> {
    let body = raw.strip_prefix('>')?;
    let body = body.trim();
    Some(if body.is_empty() {
        TermRoute::Usage
    } else {
        TermRoute::Command(body)
    })
}

/// 独占行：前缀后面那条命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermHit {
    pub shell: TermShell,
    /// 原文（已去空白）。**不进日志**：它是用户敲的查询内容。
    pub command: String,
    pub title: String,
    pub subtitle: String,
}

impl TermHit {
    pub fn new(shell: TermShell, command: &str) -> Self {
        Self {
            shell,
            command: command.to_string(),
            title: command.to_string(),
            subtitle: format!("在新窗口里执行（{}，执行完停在窗口里）", shell.as_str()),
        }
    }
}

/// 裸 `>` 的用法行：不可执行，点/按只给一句 [`USAGE_TOAST`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermHint {
    pub title: String,
    pub subtitle: String,
}

impl TermHint {
    pub fn usage(shell: TermShell) -> Self {
        Self {
            title: "用法：>命令".to_string(),
            subtitle: format!(
                "例：>ipconfig /all　当前 shell：{}（设置页「功能」可改）",
                shell.as_str()
            ),
        }
    }
}

pub struct TerminalSource;

impl super::DataSource for TerminalSource {
    fn id(&self) -> super::SourceId {
        super::SourceId::Terminal
    }

    fn label(&self) -> &'static str {
        "终端命令"
    }

    fn note(&self) -> &'static str {
        "`>命令` 在新终端窗口里执行（例：`>ipconfig /all`），执行完停在窗口里；只敲 `>` 给用法行。用哪个 shell 在这页上面的「终端 shell」里选。关掉之后 `>` 回到普通搜索。"
    }

    fn take_over(&mut self, cx: &mut super::SearchCtx<'_>) -> Option<Vec<super::SourceRow>> {
        match route(cx.query)? {
            TermRoute::Usage => Some(vec![super::SourceRow::TermHint(TermHint::usage(
                cx.term_shell,
            ))]),
            TermRoute::Command(body) => Some(vec![super::SourceRow::Terminal(TermHit::new(
                cx.term_shell,
                body,
            ))]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_routes_only_at_position_zero() {
        assert_eq!(
            route(">ipconfig /all"),
            Some(TermRoute::Command("ipconfig /all"))
        );
        assert_eq!(
            route("> ipconfig  "),
            Some(TermRoute::Command("ipconfig")),
            "前缀后可带空格"
        );
        assert_eq!(
            route(">ping 127.0.0.1"),
            Some(TermRoute::Command("ping 127.0.0.1"))
        );
        assert_eq!(route(">"), Some(TermRoute::Usage));
        assert_eq!(route(">   "), Some(TermRoute::Usage));
        assert_eq!(route(" >dir"), None, "前导空格归文件模式，不许抢");
        assert_eq!(route("dir"), None);
        assert_eq!(route(""), None);
    }

    #[test]
    fn shell_names_are_stable_and_unknown_is_rejected() {
        assert_eq!(TermShell::default(), TermShell::Cmd);
        for s in TermShell::ALL {
            assert_eq!(
                TermShell::from_name(s.as_str()),
                Some(s),
                "稳定名必须能原地往返"
            );
            assert!(!s.args("dir").is_empty());
        }
        assert_eq!(TermShell::from_name(" CMD "), Some(TermShell::Cmd));
        assert_eq!(
            TermShell::from_name("bash"),
            None,
            "不认识的名字交给调用方回落默认"
        );
        assert_eq!(TermShell::from_name(""), None);
    }

    #[test]
    fn every_shell_keeps_the_window_open() {
        for s in TermShell::ALL {
            let args = s.args("ipconfig /all");
            let joined = args.join(" ");
            assert!(
                joined.contains("/k") || joined.contains("-NoExit"),
                "{} 不能执行完就关窗，否则用户看不到输出：{joined}",
                s.as_str()
            );
            assert!(
                joined.ends_with("ipconfig /all"),
                "{} 丢掉了命令原文：{joined}",
                s.as_str()
            );
        }
    }
}

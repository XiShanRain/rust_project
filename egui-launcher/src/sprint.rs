//! FR-WIN-06 按键抢跑不丢字符：呼出热键按下到窗口拿到焦点之间的那几个字符，
//! 先由临时低级键盘钩子截住，拿到焦点后整段补进查询框。
//!
//! 为什么需要这一层：`RegisterHotKey` 只把"最后那颗键"从系统里摘走，窗口抢焦点是
//! 另一件事——`ShowWindow` / `SetForegroundWindow` 落地要几十到一百多毫秒（批次 F9 实测
//! 呼出中位 105ms），手快的用户在这段时间里打的字会原样落进**呼出前那个窗口**
//! （记事本、浏览器地址栏），既进了别人的框、又没进我们的查询框。
//!
//! 这一层只有"纯逻辑"：修饰键判据、`ToUnicodeEx` 输出的取舍、有上限的缓冲。
//! Win32 那一半（装钩子、拼键盘状态、算截止时间、吞键）都在 `main.rs`——低级键盘钩子的
//! 回调只能跑在装钩子的那条线程，而那条线程是热键线程。

/// 抢跑缓冲最多收多少个字符。到顶就不再吞键（后面的键照常去它们该去的地方）——
/// 一直吞下去等于把用户的键盘从系统里拿走。
pub const MAX_CHARS: usize = 64;

/// 截字有效期（毫秒）。呼出因为任何原因没能拿到焦点时，到点必须收手：
/// 这段时间里既补不了字、又还在替用户吞键，拖得越久越像"键盘坏了"。
pub const CAPTURE_TIMEOUT_MS: u32 = 1200;

/// 修饰键：keydown 一律原样放行，也不进缓冲。吞掉 Shift / Ctrl 会让目标窗口的修饰
/// 状态与实际键盘脱节（之后的键按错的状态解释、`Shift` 卡住这类症状）。
///
/// 左右成对的 VK 与笼统 VK 都要算上：低级键盘钩子给的是 `VK_LSHIFT` 这一套，
/// 而重映射工具与 `SendInput` 可能用 `VK_SHIFT`。
pub fn is_modifier(vk: u32) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 // Shift / Ctrl / Alt（笼统）
            | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5 // 左右分开的 Shift / Ctrl / Alt
            | 0x5B | 0x5C // 左 / 右 Win
            | 0x14 | 0x90 | 0x91 // CapsLock / NumLock / ScrollLock
    )
}

/// 把 `ToUnicodeEx` 吐出来的一小段 UTF-16 折成"要补进查询框的那一个字符"。
///
/// `None` 的情形都**不吞**（调用方原样放行）：
/// · 空 / 负数：死键（-1）与输入法组合中（0）没有可补的字符；
/// · 控制字符：Enter（0x0D）、Tab（0x09）、Backspace（0x08）、`Ctrl+A`（0x01）
///   这些不是"字"，擅自塞进查询框会把用户敲的内容改味；
/// · 多于一个字符：重音死键补全时会吐两格，只取一半就是错字；
/// · 不是合法 UTF-16（孤立代理项）。
pub fn captured_char(units: &[u16]) -> Option<char> {
    let s = String::from_utf16(units).ok()?;
    let mut it = s.chars();
    let ch = it.next()?;
    if it.next().is_some() || ch.is_control() {
        return None;
    }
    Some(ch)
}

/// 抢跑缓冲。钩子线程往里放、UI 线程整块取走；两边都只持锁几个指令，回调里没有 I/O。
pub struct SprintBuf {
    text: String,
    /// 与 `text` 并存的字符数：`String::chars().count()` 是 O(n)，钩子回调里不做那件事。
    count: usize,
}

impl SprintBuf {
    pub const fn new() -> Self {
        Self {
            text: String::new(),
            count: 0,
        }
    }

    /// 收一个字符。返回 `false` = 已满，调用方**不得**吞这颗键。
    pub fn push(&mut self, ch: char) -> bool {
        if self.count >= MAX_CHARS {
            return false;
        }
        self.text.push(ch);
        self.count += 1;
        true
    }

    /// 退掉最后收下的那个字符（抢跑期间按 Backspace 的语义：改的是还没送出去的这一截，
    /// 不该让 Backspace 落到呼出前那个窗口里去删别人的内容）。返回 `false` = 本来就是空的。
    pub fn pop(&mut self) -> bool {
        if self.text.pop().is_some() {
            self.count -= 1;
            true
        } else {
            false
        }
    }

    /// 整块取走（UI 线程把它原样补到查询框末尾）。取走不等于"关闭"——钩子可能还在往里放，
    /// 下一帧再取就是了。
    pub fn take(&mut self) -> String {
        self.count = 0;
        std::mem::take(&mut self.text)
    }

    /// 开新一轮截字前清空：上一轮没送出去的字符属于上一轮，不该迟到地出现在下一次呼出里。
    pub fn clear(&mut self) {
        self.text.clear();
        self.count = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn len(&self) -> usize {
        self.count
    }
}

impl Default for SprintBuf {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_are_passed_through_and_letters_are_not() {
        // 低级钩子给左右分开的码
        for vk in [0xA0u32, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C, 0x14, 0x90, 0x91] {
            assert!(is_modifier(vk), "0x{vk:02X} 应当是修饰键");
        }
        // 笼统的那一套也算（重映射工具与 SendInput 用它们）
        for vk in [0x10u32, 0x11, 0x12] {
            assert!(is_modifier(vk), "0x{vk:02X} 应当是修饰键");
        }
        // 字母、空格、退格、回车都不是修饰键
        for vk in [0x41u32, 0x5A, 0x20, 0x08, 0x0D, 0x09, 0x1B] {
            assert!(!is_modifier(vk), "0x{vk:02X} 不该算修饰键");
        }
    }

    #[test]
    fn captured_char_keeps_printable_and_drops_controls() {
        assert_eq!(captured_char(&[0x41]), Some('A'));
        assert_eq!(captured_char(&[0x20]), Some(' ')); // 空格是字
        assert_eq!(captured_char(&[0x4E2D]), Some('中')); // 布局直出的非 ASCII
        // 控制字符：Enter / Tab / Backspace / Ctrl+A
        for d in [0x0Du16, 0x09, 0x08, 0x01, 0x0A, 0x1B, 0x7F] {
            assert_eq!(captured_char(&[d]), None, "0x{d:02X} 不该补进查询框");
        }
    }

    #[test]
    fn captured_char_handles_surrogates_and_multi_char_output() {
        // 代理对是一个字符
        assert_eq!(captured_char(&[0xD83D, 0xDE42]), Some('🙂'));
        // 孤立代理项不是合法 UTF-16
        assert_eq!(captured_char(&[0xD83D]), None);
        // 死键补全会一次吐两格：只取一半就是错字，整段放弃
        assert_eq!(captured_char(&[0x41, 0x42]), None);
        assert_eq!(captured_char(&[]), None);
    }

    #[test]
    fn buffer_caps_at_limit_and_reports_full() {
        let mut b = SprintBuf::new();
        for i in 0..MAX_CHARS {
            assert!(b.push('x'), "第 {} 个字符应当收下", i + 1);
        }
        assert_eq!(b.len(), MAX_CHARS);
        // 到顶：返回 false，调用方据此放行而不是继续吞
        assert!(!b.push('y'));
        assert_eq!(b.len(), MAX_CHARS);
        assert_eq!(b.take().chars().count(), MAX_CHARS);
        assert!(b.is_empty());
        // 取空之后又能收
        assert!(b.push('z'));
    }

    #[test]
    fn backspace_pops_only_what_is_buffered() {
        let mut b = SprintBuf::new();
        assert!(!b.pop()); // 空的时候按退格：没有可退的，也不许 panic
        b.push('a');
        b.push('b');
        assert!(b.pop());
        assert_eq!(b.take(), "a");
        assert!(b.is_empty());
    }

    #[test]
    fn clear_discards_a_previous_round() {
        let mut b = SprintBuf::new();
        b.push('a');
        b.clear();
        assert!(b.is_empty());
        assert_eq!(b.take(), "");
    }

    /// 多字节字符按"字符"计数，不按字节：上限 64 个中文不能被算成 192 个。
    #[test]
    fn cap_counts_chars_not_bytes() {
        let mut b = SprintBuf::new();
        for _ in 0..MAX_CHARS {
            assert!(b.push('中'));
        }
        assert!(!b.push('文'));
        assert_eq!(b.take().chars().count(), MAX_CHARS);
    }
}

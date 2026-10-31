//! FR-SRC-10 计算器：把查询串求成一个结果行。
//!
//! 纯函数模块：不碰窗口、不碰剪贴板、不读时钟（“今天”由调用方作为参数传入，
//! 这样日期加减能被单测钉死在固定日历上）。语法错误一律返回 `None` ⇒ 界面表现为
//! “只不出结果行”，与本条原文一致。
//!
//! 三条本模块自己定的口径（需求原文没钉，已登记进发布说明）：
//! - **纯数字不出行**：查询整串就是一个十进制数时不当计算器结果。否则第 0 行永远是
//!   计算器，已实测通过的 `1-9 0` 数字键执行会集体变成“复制数字”。
//! - **裸数字沿用左侧单位**：`1 day + 7` ＝ 8 天，不是 8 天 7 秒。
//! - **换算单位写在副标题**：标题与 `copy` 都只是数字（`Enter` 复制纯数字）。

use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::source::{DataSource, SearchCtx, SourceId, SourceRow};

/// 一行计算器结果。`title` 与 `subtitle` 只进界面；`copy` 是 `Enter` 写进剪贴板的内容，
/// 恒为不带千分位、不带单位的纯数字（日期除外——日期复制 ISO 串）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalcHit {
    pub title: String,
    pub subtitle: String,
    pub copy: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dim {
    Len,
    Area,
    Vol,
    Mass,
    Temp,
    Speed,
    Time,
    Data,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    /// 展示用符号（大小写不敏感地匹配输入，展示按这张表）
    pub sym: &'static str,
    pub dim: Dim,
    /// 乘到基准单位：m / m² / m³ / kg / m·s⁻¹ / s / byte。温度不用这个值。
    pub to_base: f64,
}

impl Unit {
    fn base_sym(&self) -> &'static str {
        match self.dim {
            Dim::Len => "m",
            Dim::Area => "m²",
            Dim::Vol => "m³",
            Dim::Mass => "kg",
            Dim::Speed => "m/s",
            Dim::Time => "s",
            Dim::Data => "B",
            Dim::Temp => "",
        }
    }
}

/// 单位表。别名各自独立成行（`l`/`liter`/`litre`），查找按整词小写匹配。
const UNITS: &[Unit] = &[
    // 长度 → m
    u("mm", Dim::Len, 0.001),
    u("cm", Dim::Len, 0.01),
    u("m", Dim::Len, 1.0),
    u("km", Dim::Len, 1000.0),
    u("in", Dim::Len, 0.0254),
    u("ft", Dim::Len, 0.3048),
    u("yd", Dim::Len, 0.9144),
    u("mi", Dim::Len, 1609.344),
    u("nmi", Dim::Len, 1852.0),
    // 面积 → m²
    u("mm2", Dim::Area, 1e-6),
    u("cm2", Dim::Area, 1e-4),
    u("m2", Dim::Area, 1.0),
    u("ha", Dim::Area, 1e4),
    u("acre", Dim::Area, 4046.8564224),
    u("km2", Dim::Area, 1e6),
    u("in2", Dim::Area, 0.00064516),
    u("ft2", Dim::Area, 0.09290304),
    u("yd2", Dim::Area, 0.83612736),
    u("mi2", Dim::Area, 2589988.110336),
    // 体积 → m³
    u("mm3", Dim::Vol, 1e-9),
    u("cm3", Dim::Vol, 1e-6),
    u("ml", Dim::Vol, 1e-6),
    u("cl", Dim::Vol, 1e-5),
    u("l", Dim::Vol, 1e-3),
    u("liter", Dim::Vol, 1e-3),
    u("litre", Dim::Vol, 1e-3),
    u("m3", Dim::Vol, 1.0),
    u("in3", Dim::Vol, 1.6387064e-5),
    u("ft3", Dim::Vol, 0.028316846592),
    u("floz", Dim::Vol, 2.95735295625e-5),
    u("cup", Dim::Vol, 2.365882365e-4),
    u("pt", Dim::Vol, 4.73176473e-4),
    u("qt", Dim::Vol, 9.46352946e-4),
    u("gal", Dim::Vol, 3.785411784e-3),
    // 重量 → kg
    u("mg", Dim::Mass, 1e-6),
    u("g", Dim::Mass, 1e-3),
    u("kg", Dim::Mass, 1.0),
    u("t", Dim::Mass, 1000.0),
    u("oz", Dim::Mass, 0.028349523125),
    u("lb", Dim::Mass, 0.45359237),
    u("st", Dim::Mass, 6.35029318),
    // 温度（换算走 temp_to_c / c_to_temp）
    u("c", Dim::Temp, 1.0),
    u("f", Dim::Temp, 1.0),
    u("k", Dim::Temp, 1.0),
    // 速度 → m/s
    u("mps", Dim::Speed, 1.0),
    u("fps", Dim::Speed, 0.3048),
    u("kmh", Dim::Speed, 1.0 / 3.6),
    u("kph", Dim::Speed, 1.0 / 3.6),
    u("mph", Dim::Speed, 0.44704),
    u("kn", Dim::Speed, 1852.0 / 3600.0),
    u("knot", Dim::Speed, 1852.0 / 3600.0),
    // 时间 → s
    u("ms", Dim::Time, 0.001),
    u("s", Dim::Time, 1.0),
    u("sec", Dim::Time, 1.0),
    u("second", Dim::Time, 1.0),
    u("seconds", Dim::Time, 1.0),
    u("min", Dim::Time, 60.0),
    u("minute", Dim::Time, 60.0),
    u("minutes", Dim::Time, 60.0),
    u("h", Dim::Time, 3600.0),
    u("hr", Dim::Time, 3600.0),
    u("hour", Dim::Time, 3600.0),
    u("hours", Dim::Time, 3600.0),
    u("d", Dim::Time, 86400.0),
    u("day", Dim::Time, 86400.0),
    u("days", Dim::Time, 86400.0),
    u("wk", Dim::Time, 604800.0),
    u("week", Dim::Time, 604800.0),
    u("weeks", Dim::Time, 604800.0),
    // 日历加减用整月/整年，不按固定秒数
    u("month", Dim::Time, 2592000.0),
    u("months", Dim::Time, 2592000.0),
    u("year", Dim::Time, 31557600.0),
    u("years", Dim::Time, 31557600.0),
    // 数据 → byte
    u("b", Dim::Data, 1.0),
    u("kb", Dim::Data, 1024.0),
    u("mb", Dim::Data, 1024f64 * 1024.0),
    u("gb", Dim::Data, 1024f64 * 1024.0 * 1024.0),
    u("tb", Dim::Data, 1024f64 * 1024.0 * 1024.0 * 1024.0),
];

const fn u(sym: &'static str, dim: Dim, to_base: f64) -> Unit {
    Unit { sym, dim, to_base }
}

/// `m/s` 这类"由除法自然得出"的速度单位，展示符号固定用它。
const SPEED_SYM: &str = "m/s";

fn unit_of(word: &str) -> Option<Unit> {
    let w = word.to_ascii_lowercase();
    UNITS.iter().find(|x| x.sym == w).copied()
}

fn is_calendar_unit(sym: &str) -> bool {
    matches!(sym, "month" | "months" | "year" | "years")
}

/// 展示层的单位符号。表里的 `sym` 是查表键、必须保持小写（`unit_of` 先 `to_ascii_lowercase` 再比），
/// 所以大小写只在写副标题这一刻映射一次。动因是真机实测：`100C to F` 的副标题渲染成了 `c → f`，
/// 那读起来像「库仑 → 法拉」。面积/体积的 `km2`/`cm3` 这类仍按查表键原样展示（不成歧义，见发布说明的已知限制）。
fn disp_sym(sym: &str) -> &str {
    match sym {
        "c" => "°C",
        "f" => "°F",
        "k" => "K",
        "b" => "B",
        "kb" => "KB",
        "mb" => "MB",
        "gb" => "GB",
        "tb" => "TB",
        "mps" => "m/s",
        "fps" => "ft/s",
        "kmh" | "kph" => "km/h",
        _ => sym,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tok {
    Num(f64),
    /// 进制字面量：值 + 原进制（只有整串就是一个进制字面量时才展示互转副标题）
    Radix(f64, u32),
    Unit(Unit),
    Date(i64),
    Op(char),
}

#[derive(Debug, Clone, Copy)]
enum Val {
    Num(f64),
    /// `base` 已折算到该维度的基准单位；`unit` 只决定怎么展示
    Qty {
        base: f64,
        unit: Unit,
    },
    Date(i64),
}

/// 查询串 → 结果行。`today` 是本地日历的日序（用 [`today_day`] 取）。
pub fn evaluate(raw: &str, today: i64) -> Option<CalcHit> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    // 前导 `=` 是"这是一道算式"的显式信号，剥掉后照常求值。
    let s = s.strip_prefix('=').unwrap_or(s).trim();
    if s.is_empty() {
        return None;
    }

    // 换算形态：`<左> in|to <单位>`。取最后一个独立关键字，让 `10 in in cm` 也能切开。
    // 切不开或换算不成立就回落到普通求值（`1 km in 2` 那种照样不出行，但走的是同一条判据）。
    if let Some((left, right)) = split_conversion(s) {
        if let Some(hit) = convert_expr(left, right, today) {
            return Some(hit);
        }
    }

    let toks = lex(s, today)?;
    // 纯数字 / 单个单位名不出行（见文件头口径：`1` 必须是查询词，`km` 也是）。
    if toks.len() == 1 && matches!(toks[0], Tok::Num(_) | Tok::Unit(_)) {
        return None;
    }
    let mut hit = format_value(parse(&toks)?)?;
    if let Some((v, _)) = radix_only(&toks) {
        let n = v as i64;
        hit.subtitle = crate::i18n::tfmt(
            "calc.radix_subtitle",
            &[&format!("{n:X}"), &format!("{n:o}"), &format!("{n:b}")],
        );
    } else if hit.subtitle.is_empty() {
        hit.subtitle = crate::i18n::tfmt("calc.subtitle", &[&row_tag()]);
    }
    Some(hit)
}

/// 整串就是一个进制字面量时返回（值, 进制）——只有这种形态才展示互转后的其它进制。
fn radix_only(toks: &[Tok]) -> Option<(f64, u32)> {
    match toks {
        [Tok::Radix(v, base)] => Some((*v, *base)),
        _ => None,
    }
}

/// `<左> in|to <右单位>` 的换算求值。左侧必须是带量纲的量，两侧维度必须一致。
fn convert_expr(left: &str, target_word: &str, today: i64) -> Option<CalcHit> {
    let Val::Qty { base, unit } = parse(&lex(left, today)?)? else {
        return None;
    };
    let target = unit_of(target_word.trim())?;
    let disp = finite(convert_qty(base, unit, target)?)?;
    Some(CalcHit {
        title: format!("= {}", fmt_num(disp, true)),
        subtitle: format!("{} → {}", disp_sym(unit.sym), disp_sym(target.sym)),
        copy: fmt_num(disp, false),
    })
}

fn convert_qty(base: f64, from: Unit, to: Unit) -> Option<f64> {
    if from.dim != to.dim {
        return None;
    }
    match from.dim {
        Dim::Temp => {
            let c = temp_to_c(base, from.sym);
            Some(c_to_temp(c, to.sym))
        }
        _ => Some(base / to.to_base),
    }
}

fn temp_to_c(v: f64, sym: &str) -> f64 {
    match sym {
        "f" => (v - 32.0) * 5.0 / 9.0,
        "k" => v - 273.15,
        _ => v,
    }
}

fn c_to_temp(c: f64, sym: &str) -> f64 {
    match sym {
        "f" => c * 9.0 / 5.0 + 32.0,
        "k" => c + 273.15,
        _ => c,
    }
}

/// 独立换算关键字：`in` / `to`。取最后一个整词命中，返回 (左串, 右串)。
///
/// 走的是字节游标，所以**切片口的两个下标必须都落在 ASCII 字母段的两侧**：`&s[a..b]` 即使
/// `a == b` 也要求 `a` 是字符边界（2026-10-22 真机崩溃：中文查询让 `word_start` 停在
/// `报` 的中间字节上，`&s[2..2]` 直接 panic，搜索框里敲任何非 ASCII 都闪退）。
fn split_conversion(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let mut found: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_alphabetic() {
            // 非字母字节逐个跳过：多字节字符的中间字节也走到这里，但这一支不切片。
            i += 1;
            continue;
        }
        let word_start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        // 字母段的两侧必然是字符边界：段首是刚跳过的非字母字节之后，段尾要么是 ASCII
        // 非字母、要么是一个多字节字符的首字节。
        let word = &s[word_start..i];
        let wl = word.to_ascii_lowercase();
        let standalone = (word_start == 0 || !is_word_byte(bytes[word_start - 1]))
            && (i == bytes.len() || !is_word_byte(bytes[i]));
        if (wl == "in" || wl == "to") && standalone {
            found = Some((word_start, i));
        }
    }
    let (a, b) = found?;
    let left = s[..a].trim();
    let right = s[b..].trim();
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some((left, right))
}

fn is_word_byte(c: u8) -> bool {
    (c as char).is_ascii_alphanumeric()
}

fn lex(s: &str, today: i64) -> Option<Vec<Tok>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() {
            if let Some((day, next)) = try_date(b, i) {
                out.push(Tok::Date(day));
                i = next;
                continue;
            }
            if i + 2 <= b.len()
                && b[i] == b'0'
                && matches!(b[i + 1], b'x' | b'X' | b'o' | b'O' | b'b' | b'B')
            {
                let radix = match b[i + 1] {
                    b'x' | b'X' => 16,
                    b'o' | b'O' => 8,
                    _ => 2,
                };
                let mut j = i + 2;
                let mut acc: i64 = 0;
                let mut digits = 0;
                while j < b.len() {
                    if let Some(d) = (b[j] as char).to_digit(radix) {
                        acc = acc.checked_mul(radix as i64)? + d as i64;
                        digits += 1;
                        j += 1;
                    } else {
                        break;
                    }
                }
                if digits == 0 {
                    return None;
                }
                out.push(Tok::Radix(acc as f64, radix));
                i = j;
                continue;
            }
            let (v, next) = number(b, i)?;
            out.push(Tok::Num(v));
            i = next;
            continue;
        }
        if (c as char).is_ascii_alphabetic() {
            let start = i;
            while i < b.len() && (b[i] as char).is_ascii_alphabetic() {
                i += 1;
            }
            let word = &s[start..i];
            let wl = word.to_ascii_lowercase();
            match wl.as_str() {
                "today" | "now" => out.push(Tok::Date(today)),
                "tomorrow" => out.push(Tok::Date(today + 1)),
                "yesterday" => out.push(Tok::Date(today - 1)),
                _ => out.push(Tok::Unit(unit_of(&wl)?)),
            }
            continue;
        }
        if matches!(c, '+' | '-' | '*' | '/' | '^' | '(' | ')') {
            out.push(Tok::Op(c));
            i += 1;
            continue;
        }
        return None;
    }
    if out.is_empty() { None } else { Some(out) }
}

/// 十进制数：可选小数与 `e` 科学计数。返回 (值, 下一个下标)。
fn number(b: &[u8], mut i: usize) -> Option<(f64, usize)> {
    let start = i;
    while i < b.len() && (b[i] as char).is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && (b[i] as char).is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let save = i;
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        if i < b.len() && (b[i] as char).is_ascii_digit() {
            while i < b.len() && (b[i] as char).is_ascii_digit() {
                i += 1;
            }
        } else {
            i = save;
        }
    }
    if i == start {
        return None;
    }
    s_slice(b, start, i)?.parse::<f64>().ok().map(|v| (v, i))
}

fn s_slice(b: &[u8], from: usize, to: usize) -> Option<&str> {
    std::str::from_utf8(&b[from..to]).ok()
}

/// `YYYY-MM-DD` / `YYYY/M/D`（也接受 `.`, `，` 不管）。返回 (日序, 下一个下标)。
fn try_date(b: &[u8], i: usize) -> Option<(i64, usize)> {
    let sep = *b.get(i + 4)?;
    if sep != b'-' && sep != b'/' && sep != b'.' {
        return None;
    }
    if !b
        .get(i..i + 4)?
        .iter()
        .all(|c| (*c as char).is_ascii_digit())
    {
        return None;
    }
    let y: i64 = s_slice(b, i, i + 4)?.parse().ok()?;
    let mut j = i + 5;
    let m_start = j;
    while j < b.len() && (b[j] as char).is_ascii_digit() {
        j += 1;
    }
    if j == m_start || j - m_start > 2 {
        return None;
    }
    let m: u32 = s_slice(b, m_start, j)?.parse().ok()?;
    let sep2 = *b.get(j)?;
    if sep2 != b'-' && sep2 != b'/' && sep2 != b'.' {
        return None;
    }
    j += 1;
    let d_start = j;
    while j < b.len() && (b[j] as char).is_ascii_digit() {
        j += 1;
    }
    if j == d_start || j - d_start > 2 {
        return None;
    }
    let d: u32 = s_slice(b, d_start, j)?.parse().ok()?;
    let day = days_from_civil(y, m, d)?;
    Some((day, j))
}

// ---------------------------------------------------------------- 解析器

fn parse(toks: &[Tok]) -> Option<Val> {
    let mut p = Parser { toks, pos: 0 };
    let v = p.expr()?;
    if p.pos != toks.len() {
        return None;
    }
    Some(v)
}

struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<Tok> {
        self.toks.get(self.pos).copied()
    }

    fn eat_op(&mut self, c: char) -> bool {
        if self.peek() == Some(Tok::Op(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Option<Val> {
        let mut acc = self.term()?;
        loop {
            if self.eat_op('+') {
                let rhs = self.term()?;
                acc = add(acc, rhs)?;
            } else if self.eat_op('-') {
                let rhs = self.term()?;
                acc = sub(acc, rhs)?;
            } else {
                return Some(acc);
            }
        }
    }

    fn term(&mut self) -> Option<Val> {
        let mut acc = self.unary()?;
        loop {
            if self.eat_op('*') {
                acc = mul(acc, self.unary()?)?;
            } else if self.eat_op('/') {
                acc = div(acc, self.unary()?)?;
            } else {
                return Some(acc);
            }
        }
    }

    fn unary(&mut self) -> Option<Val> {
        if self.eat_op('-') {
            return neg(self.unary()?);
        }
        if self.eat_op('+') {
            return self.unary();
        }
        self.power()
    }

    fn power(&mut self) -> Option<Val> {
        let base = self.primary()?;
        if self.eat_op('^') {
            let exp = self.unary()?; // 右结合
            return pow(base, exp);
        }
        Some(base)
    }

    fn primary(&mut self) -> Option<Val> {
        let v = match self.peek() {
            Some(Tok::Num(x)) => {
                self.pos += 1;
                Val::Num(x)
            }
            Some(Tok::Radix(x, _)) => {
                self.pos += 1;
                Val::Num(x)
            }
            Some(Tok::Date(d)) => {
                self.pos += 1;
                Val::Date(d)
            }
            Some(Tok::Unit(u)) => {
                self.pos += 1;
                // 单独一个单位＝「1 个该单位」，`km/h`、`g/ml` 这类写法因此不必专门加词法
                match u.dim {
                    Dim::Temp => return None,
                    _ => Val::Qty {
                        base: u.to_base,
                        unit: u,
                    },
                }
            }
            Some(Tok::Op('(')) => {
                self.pos += 1;
                let inner = self.expr()?;
                if !self.eat_op(')') {
                    return None;
                }
                inner
            }
            _ => return None,
        };
        // 后缀单位：`10 km`、`(2+3) days`
        match (v, self.peek()) {
            (Val::Num(x), Some(Tok::Unit(u))) => {
                self.pos += 1;
                Some(Val::Qty {
                    base: x * u.to_base,
                    unit: u,
                })
            }
            (other, _) => Some(other),
        }
    }
}

fn neg(v: Val) -> Option<Val> {
    match v {
        Val::Num(x) => Some(Val::Num(-x)),
        Val::Qty { base, unit } if unit.dim != Dim::Temp => Some(Val::Qty { base: -base, unit }),
        _ => None,
    }
}

fn add(a: Val, b: Val) -> Option<Val> {
    match (a, b) {
        (Val::Num(x), Val::Num(y)) => Some(Val::Num(x + y)),
        (Val::Qty { base, unit }, Val::Qty { base: b2, unit: u2 }) if unit.dim == u2.dim => {
            Some(Val::Qty {
                base: base + b2,
                unit,
            })
        }
        (Val::Qty { base, unit }, Val::Num(n)) | (Val::Num(n), Val::Qty { base, unit }) => {
            // 裸数字沿用带单位那一侧的单位：`1 day + 7` ＝ 8 天
            Some(Val::Qty {
                base: base + n * unit.to_base,
                unit,
            })
        }
        (Val::Date(d), Val::Qty { base, unit }) | (Val::Qty { base, unit }, Val::Date(d)) => {
            date_add(d, base, unit)
        }
        (Val::Date(d), Val::Num(n)) => date_add(d, n * 86400.0, day_unit()),
        _ => None,
    }
}

/// 减法：两个日期相减得「相差几天」，这是唯一不能靠「取负再加」表达的形态
/// （日期本身没有负语义，`neg` 会直接判不适用）。
fn sub(a: Val, b: Val) -> Option<Val> {
    if let (Val::Date(x), Val::Date(y)) = (a, b) {
        return Some(Val::Qty {
            base: ((x - y) as f64) * 86400.0,
            unit: day_unit(),
        });
    }
    add(a, neg(b)?)
}

fn day_unit() -> Unit {
    *UNITS.iter().find(|x| x.sym == "day").unwrap()
}

/// 日期加减：整月/整年按日历，其余按秒。
fn date_add(d: i64, value: f64, unit: Unit) -> Option<Val> {
    if unit.dim != Dim::Time {
        return None;
    }
    if is_calendar_unit(unit.sym) {
        let n = value / unit.to_base;
        let n = n.round() as i64;
        let months = if unit.sym.starts_with("year") {
            n * 12
        } else {
            n
        };
        return Some(Val::Date(add_months(d, months)));
    }
    Some(Val::Date((d as f64 + value / 86400.0).round() as i64))
}

fn mul(a: Val, b: Val) -> Option<Val> {
    match (a, b) {
        (Val::Num(x), Val::Num(y)) => Some(Val::Num(x * y)),
        (Val::Qty { base, unit }, Val::Num(n)) | (Val::Num(n), Val::Qty { base, unit }) => {
            Some(Val::Qty {
                base: base * n,
                unit,
            })
        }
        _ => None,
    }
}

fn div(a: Val, b: Val) -> Option<Val> {
    let r = match (a, b) {
        (Val::Num(x), Val::Num(y)) => return finite(x / y).map(Val::Num),
        (Val::Qty { base, unit }, Val::Num(n)) => Some(Val::Qty {
            base: base / n,
            unit,
        }),
        (Val::Qty { base: x, unit: ux }, Val::Qty { base: y, unit: uy }) if ux.dim == uy.dim => {
            Some(Val::Num(x / y))
        }
        // 长度 ÷ 时间 ⇒ 速度（`km/h`、`m/s` 因此不需要专门词法）
        (Val::Qty { base: x, unit: ux }, Val::Qty { base: y, unit: uy })
            if ux.dim == Dim::Len && uy.dim == Dim::Time =>
        {
            Some(Val::Qty {
                base: x / y,
                unit: u(SPEED_SYM, Dim::Speed, 1.0),
            })
        }
        _ => None,
    };
    r
}

fn finite(v: f64) -> Option<f64> {
    if v.is_finite() { Some(v) } else { None }
}

fn pow(a: Val, b: Val) -> Option<Val> {
    match (a, b) {
        (Val::Num(x), Val::Num(y)) => finite(x.powf(y)).map(Val::Num),
        (Val::Qty { base, unit }, Val::Num(n)) if unit.dim == Dim::Data && n == 1.0 => {
            Some(Val::Qty { base, unit })
        }
        _ => None, // 带量纲的乘方会产生这里没有的维度 ⇒ 不出行
    }
}

// ---------------------------------------------------------------- 结果成形

fn format_value(v: Val) -> Option<CalcHit> {
    match v {
        Val::Num(x) => {
            let x = finite(x)?;
            Some(CalcHit {
                title: format!("= {}", fmt_num(x, true)),
                subtitle: String::new(),
                copy: fmt_num(x, false),
            })
        }
        Val::Qty { base, unit } => {
            let disp = match unit.dim {
                Dim::Temp => c_to_temp(base, unit.sym),
                _ => base / unit.to_base,
            };
            let disp = finite(disp)?;
            // 单位一律写在副标题，标题与 copy 只有数字（FR-SRC-10「Enter 复制纯数字」）。
            let subtitle = match unit.dim {
                Dim::Temp => format!("{} {}", fmt_num(disp, false), disp_sym(unit.sym)),
                _ => crate::i18n::tfmt(
                    "calc.convert_subtitle",
                    &[
                        &fmt_num(disp, true),
                        &disp_sym(unit.sym),
                        &fmt_num(base, true),
                        &unit.base_sym(),
                    ],
                ),
            };
            Some(CalcHit {
                title: format!("= {}", fmt_num(disp, true)),
                subtitle,
                copy: fmt_num(disp, false),
            })
        }
        Val::Date(d) => {
            let (y, m, day) = civil_from_days(d);
            Some(CalcHit {
                title: format!("= {y:04}-{m:02}-{day:02}"),
                subtitle: weekday_name(d).to_string(),
                copy: format!("{y:04}-{m:02}-{day:02}"),
            })
        }
    }
}

/// 数字展示。`sep` ＝ 是否加千分位（复制出去时一律不加）。
pub fn fmt_num(v: f64, sep: bool) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let neg = v < 0.0 || v.is_sign_negative();
    let a = v.abs();
    let mut body = if a < 1e15 && (a - a.round()).abs() < 1e-9 {
        format!("{}", a.round() as i64)
    } else if a != 0.0 && (a < 1e-4 || a >= 1e16) {
        format!("{a:e}")
    } else {
        format!("{a:.6}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    };
    if sep && !body.contains(['e', 'E', '.']) {
        body = group_thousands(&body);
    }
    if neg && body.parse::<f64>().unwrap_or(0.0) != 0.0 {
        body.insert(0, '-');
    }
    body
}

fn group_thousands(digits: &str) -> String {
    let mut out = String::new();
    let bytes = digits.as_bytes();
    let lead = bytes.len() % 3;
    for (i, c) in bytes.iter().enumerate() {
        if i > 0 && i >= lead && (i - lead) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

fn weekday_name(d: i64) -> &'static str {
    const KEYS: [&str; 7] = [
        "calc.weekday.mon",
        "calc.weekday.tue",
        "calc.weekday.wed",
        "calc.weekday.thu",
        "calc.weekday.fri",
        "calc.weekday.sat",
        "calc.weekday.sun",
    ];
    // 日序 0 是 1970-01-01（周四）⇒ 偏移 3 才让 0 落在周一这一格
    crate::i18n::t(KEYS[(d + 3).rem_euclid(7) as usize])
}

// ---------------------------------------------------------------- 公历换算
// 1970-01-01 ＝ 日序 0（proleptic Gregorian）。日期加减只用整数值，不碰时区。

pub fn days_from_civil(y: i64, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    let yy = if m <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

pub fn civil_from_days(z0: i64) -> (i64, u32, u32) {
    let z = z0 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let yy = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { yy + 1 } else { yy };
    (y, m as u32, d as u32)
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn add_months(day: i64, months: i64) -> i64 {
    let (y, m, d) = civil_from_days(day);
    let total = y * 12 + (m as i64) - 1 + months;
    let ny = total.div_euclid(12);
    let nm = total.rem_euclid(12) as u32 + 1;
    let nd = d.min(days_in_month(ny, nm));
    days_from_civil(ny, nm, nd).unwrap_or(day)
}

/// 本地日历的今日日序。用 `GetLocalTime`（与日志轮转同一条路子），不走 UTC。
pub fn today_day() -> i64 {
    let t = unsafe { GetLocalTime() };
    days_from_civil(t.wYear as i64, t.wMonth as u32, t.wDay as u32).unwrap_or(0)
}

/// 结果行标题右侧的固定标识（界面上一眼看出这行不是快捷项）。
pub fn row_tag() -> &'static str {
    crate::i18n::t("calc.row_tag")
}

/// 批次 A10：把本模块接进数据源注册表。只贡献头段一行（不出结果集）。
#[derive(Debug, Default)]
pub struct CalcSource;

impl DataSource for CalcSource {
    fn id(&self) -> SourceId {
        SourceId::Calc
    }

    fn label(&self) -> &'static str {
        crate::i18n::t("source.calc.label")
    }

    fn note(&self) -> &'static str {
        crate::i18n::t("source.calc.note")
    }

    fn head(&mut self, cx: &mut SearchCtx<'_>) -> Vec<SourceRow> {
        // "今天"在 `SearchCtx` 里只有毫秒时间戳，日期加减要的是本地日序 ⇒ 这里现取一次。
        // 它与 `cx.now_ms` 取自同一帧，不会跨日抖动。
        evaluate(cx.query, today_day())
            .map(SourceRow::Calc)
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定"今天"＝2026-10-20（周二），日期加减才有确定期望值。
    const TODAY: i64 = 20_746;

    fn hit(raw: &str) -> Option<CalcHit> {
        // 日历自检：先确认钉住的 TODAY 就是 2026-10-20
        assert_eq!(civil_from_days(TODAY), (2026, 10, 20));
        evaluate(raw, TODAY)
    }

    fn val(raw: &str) -> Option<String> {
        hit(raw).map(|h| h.copy)
    }

    #[test]
    fn calendar_round_trip_and_weekday_anchor() {
        assert_eq!(days_from_civil(2026, 10, 20), Some(TODAY));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(weekday_name(0), "周四", "1970-01-01 确实是周四");
        assert_eq!(weekday_name(TODAY), "周二");
    }

    #[test]
    fn accepts_the_four_samples_from_the_spec() {
        assert_eq!(val("(200+100)*2").as_deref(), Some("600"));
        assert_eq!(val("1 day + 7").as_deref(), Some("8"));
        assert_eq!(val("0xff").as_deref(), Some("255"));
        assert_eq!(val("10 km in mi").as_deref(), Some("6.213712"));
    }

    #[test]
    fn leading_equals_sign_is_stripped() {
        assert_eq!(val("=2^10").as_deref(), Some("1024"));
        assert_eq!(hit("=2^10").map(|h| h.title), Some("= 1,024".to_string()));
    }

    #[test]
    fn thousands_in_title_but_never_in_copy() {
        let h = hit("2^40").unwrap();
        assert_eq!(h.title, "= 1,099,511,627,776");
        assert_eq!(h.copy, "1099511627776");
    }

    #[test]
    fn unary_minus_and_precedence() {
        assert_eq!(val("-5+2").as_deref(), Some("-3"));
        assert_eq!(val("2^3^2").as_deref(), Some("512"), "^ 右结合");
        assert_eq!(val("(2+3)*4-6/3").as_deref(), Some("18"));
        assert_eq!(val("-2^2").as_deref(), Some("-4"));
    }

    #[test]
    fn radix_literals_show_other_bases() {
        let h = hit("0x1f").unwrap();
        assert_eq!(h.copy, "31");
        assert!(h.subtitle.contains("11111"), "{:?}", h.subtitle);
        assert_eq!(val("0b1010").as_deref(), Some("10"));
        assert_eq!(val("0o17").as_deref(), Some("15"));
    }

    #[test]
    fn unit_conversion_both_directions_and_rounding() {
        assert_eq!(val("1 mi in km").as_deref(), Some("1.609344"));
        assert_eq!(val("10 km to mi").as_deref(), Some("6.213712"));
        assert_eq!(val("1 lb in oz").as_deref(), Some("16"));
        // 换算行的单位只写在副标题里
        let h = hit("10 km in mi").unwrap();
        assert_eq!(h.title, "= 6.213712");
        assert!(h.subtitle.contains("mi"), "{:?}", h.subtitle);
    }

    #[test]
    fn temperature_uses_offsets_not_ratios() {
        assert_eq!(val("100 c in f").as_deref(), Some("212"));
        assert_eq!(val("32 f in c").as_deref(), Some("0"));
        assert_eq!(val("0 c in k").as_deref(), Some("273.15"));
    }

    #[test]
    fn subtitle_uses_conventional_unit_symbols() {
        // 动因是真机实测：`100C to F` 曾渲染成 `c → f`，读起来像「库仑 → 法拉」。
        assert_eq!(hit("100C to F").unwrap().subtitle, "°C → °F");
        assert_eq!(hit("100 mb to gb").unwrap().subtitle, "MB → GB");
        assert_eq!(hit("10 kmh to mph").unwrap().subtitle, "km/h → mph");
        // 单量行的副标题走同一条映射；等号右边的基准单位符号本来就是规范写法。
        assert_eq!(hit("5 mb").unwrap().subtitle, "5 MB ＝ 5,242,880 B");
    }

    #[test]
    fn speed_reaches_mph_through_plain_division() {
        assert_eq!(val("10 km/h in mph").as_deref(), Some("6.213712"));
        assert_eq!(val("1000 m/s in kmh").as_deref(), Some("3600"));
    }

    #[test]
    fn data_units_are_binary() {
        assert_eq!(val("1 mb in kb").as_deref(), Some("1024"));
    }

    #[test]
    fn bare_number_does_not_produce_a_row_but_expression_does() {
        assert!(hit("1").is_none());
        assert!(hit("42").is_none());
        assert!(hit("3.5").is_none());
        assert!(hit("1+1").is_some());
        assert!(hit("1 day").is_some(), "带单位就不是裸数字");
        assert!(hit("0xff").is_some(), "进制字面量不是裸数字");
    }

    #[test]
    fn syntax_errors_stay_silent() {
        for raw in [
            "1+", "(1+2", "2 ** 3", "jsb", "1 2", "1,024", "%", "km", "1 in 2", "1 ?? 2",
        ] {
            assert!(hit(raw).is_none(), "{raw} 不该出行");
        }
    }

    #[test]
    fn dimension_mismatch_produces_no_row() {
        assert!(hit("1 km + 1 kg").is_none());
        assert!(hit("1 km * 2 km").is_none());
        assert!(hit("10 in mi").is_none(), "换算目标必须是个单位");
        assert!(hit("1 km in 2").is_none());
    }

    #[test]
    fn date_arithmetic_is_calendar_aware() {
        assert_eq!(val("2026-10-20 + 7 days").as_deref(), Some("2026-10-27"));
        assert_eq!(val("today + 1 day + 7").as_deref(), Some("2026-10-28"));
        assert_eq!(
            val("2026-01-31 + 1 month").as_deref(),
            Some("2026-02-28"),
            "月末夹住"
        );
        assert_eq!(val("2024-02-29 + 1 year").as_deref(), Some("2025-02-28"));
        assert_eq!(
            val("2026-12-25 - today").as_deref(),
            Some("66"),
            "日期差＝天数"
        );
        assert_eq!(val("2026-10-20 - 1 week").as_deref(), Some("2026-10-13"));
    }

    #[test]
    fn date_row_title_and_subtitle() {
        let h = hit("today + 7 days").unwrap();
        assert_eq!(h.title, "= 2026-10-27");
        assert_eq!(h.subtitle, "周二");
    }

    #[test]
    fn quantity_without_target_shows_base_equivalent() {
        let h = hit("10 in").unwrap();
        assert_eq!(h.copy, "10");
        assert!(h.subtitle.contains("0.254"), "{:?}", h.subtitle);
    }

    #[test]
    fn number_formatter_covers_edges() {
        assert_eq!(fmt_num(600.0, true), "600");
        assert_eq!(fmt_num(1000.0, true), "1,000");
        assert_eq!(fmt_num(1000.0, false), "1000");
        assert_eq!(fmt_num(2.5, true), "2.5");
        assert_eq!(fmt_num(-0.0, true), "0", "负零不该冒出减号");
        assert_eq!(fmt_num(1e-7, true), "1e-7");
    }

    #[test]
    fn conversion_keyword_split_survives_in_as_inch() {
        // `in` 既是换算关键字又是英寸单位
        assert_eq!(val("10 in in cm").as_deref(), Some("25.4"));
        assert_eq!(val("2 in in mm").as_deref(), Some("50.8"));
    }

    #[test]
    fn non_ascii_queries_cannot_panic_the_byte_walker() {
        // 2026-10-22 真机闪退（用户报的三条 Backspace 崩溃是同一个根因）：搜索框里敲中文，
        // `split_conversion` 把 `word_start` 停在多字节字符的中间字节上再切片 ⇒ panic。
        // 计算器对中文查询的正确答案只有"不出行"，绝不许是"整个程序退出"。
        for q in [
            "报",
            "报告",
            "'报",
            "'报告",
            "pic 报",
            "folder 项",
            "记事本",
            "计算器",
            "你好",
            "10 公里 in cm",
            "2 in 你好",
            "今天 + 7 天",
            "米",
            "°C → °F",
        ] {
            assert!(
                evaluate(q, today_day()).is_none(),
                "{q:?} 不该出计算器行（中文一律走「不出行」，不是闪退）"
            );
        }
    }
}

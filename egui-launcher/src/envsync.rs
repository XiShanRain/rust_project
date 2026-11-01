//! 批次 S1（FR-SYS-07）：环境变量热更新——「改了 PATH 不必重启」。
//!
//! 分工与本仓其它模块一样：纯函数只管字符串，`Win32` 那层只管读两把键与写
//! [`std::env::set_var`]。单测喂假的注册表行和假的环境查表，真机才走真的键。
//!
//! **口径（本批定死，别后面当成缺陷来"修"）**：
//! - **只补不改**：`PATH` 走「把注册表里有、进程里没有的段落补进去」；其余格子**只有进程里
//!   根本没有这一格（或它是空串）才新增**，已经有值的一律不动；注册表里**已经没有**的格子
//!   也一律不动（不清零任何格）。
//!   【2026-11-01 本条从「值不同才覆盖」改窄，取证在 `target/sysmsg-envdiff.ps1`：本机两把键
//!   对着真的进程环境比，**照旧口径要动四格**——`USERNAME` 会从 `baoqi` 改成注册表里的 `SYSTEM`、
//!   `MISE_STATE_DIR` 会被改回一个已经不用的旧目录、`PATHEXT` 会丢掉用户后加的 `.CPL`、
//!   `PSModulePath` 会丢掉会话头部那一段。四条没有一条是"用户在系统属性里改了环境变量"，
//!   全是**会话/启动器在注册表之后又改过**。注册表只是登录时的输入，不是环境的当前真相；
//!   分不清"注册表被改了"和"会话改过它"，就选择不猜：已有格一律不动。】
//!   代价两条，一起登记为口径偏差：① 改一个**已存在**的变量值，本进程要等下次启动才跟上；
//!   ② 删一个用户环境变量要等下次登录。两条都不影响验收那句「修改系统 PATH 后不重启即可搜到新加的 exe」，
//!   因为 PATH 走的是段落级的只补不删。
//! - 刷新失败＝什么都不写。读不动键就回空表，空表算出来的计划也是空计划，
//!   绝不让一次失败的广播把进程环境改坏。

/// 系统级格子。路径里带空格是本来如此，不是笔误。
const MACHINE_SUBKEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
/// 用户级格子（相对 `HKEY_CURRENT_USER`）。
const USER_SUBKEY: &str = "Environment";

/// 注册表里读出来的一行原样事实。
///
/// `expandable` 为真表示类型是 `REG_EXPAND_SZ`：`raw` 里还嵌着 `%OTHER%`，
/// 要过一遍展开才是 Windows 登录时真正交给进程的那个值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegValue {
    pub name: String,
    pub expandable: bool,
    pub raw: String,
}

/// 两把键合成后的格子：`(名字原文, 是否需要展开, 值原文)`。
pub type Merged = (String, bool, String);

/// 合成「按登录规则本该得到的那一份」。
///
/// 同名以用户为准，**唯一的例外是 `Path`**：Windows 登录时是系统 Path 在前、
/// 用户 Path 在后**串起来**，不是谁覆盖谁（两处都填了 Path 的人不少）。
/// 名字保持注册表原文的大小写——查环境变量本来不分大小写，但写回去时照原样更好读。
pub fn merge_values(machine: &[RegValue], user: &[RegValue]) -> Vec<Merged> {
    let mut out: Vec<Merged> = machine
        .iter()
        .filter(|v| !v.name.is_empty())
        .map(|v| (v.name.clone(), v.expandable, v.raw.clone()))
        .collect();

    for v in user {
        if v.name.is_empty() {
            continue;
        }
        match out.iter().position(|(n, _, _)| n.eq_ignore_ascii_case(&v.name)) {
            Some(i) => {
                let (name, exp_old, raw_old) = out[i].clone();
                if name.eq_ignore_ascii_case("path") {
                    // 拼接而不是覆盖；两格里只要有一格是可展开类型，合成就得展开。
                    out[i] = (name, exp_old || v.expandable, join_path(&raw_old, &v.raw));
                } else {
                    out[i] = (name, v.expandable, v.raw.clone());
                }
            }
            None => out.push((v.name.clone(), v.expandable, v.raw.clone())),
        }
    }
    out
}

/// 串两段 `PATH` 文本。任一段为空就用另一段，两段都有才加分号。
fn join_path(machine: &str, user: &str) -> String {
    let m = machine.trim_matches(|c: char| c == ';' || c.is_whitespace());
    let u = user.trim_matches(|c: char| c == ';' || c.is_whitespace());
    match (m.is_empty(), u.is_empty()) {
        (true, _) => u.to_string(),
        (_, true) => m.to_string(),
        (false, false) => format!("{m};{u}"),
    }
}

/// 归一化一段目录名，**只为比较**：去首尾空白、去尾随反斜杠、转小写、`/` 当 `\`。
/// 写回去的永远是原文，这个结果不外泄。
fn norm_entry(s: &str) -> String {
    s.trim()
        .trim_end_matches('\\')
        .to_ascii_lowercase()
        .replace('/', "\\")
}

/// 只增不减地把 `desired` 里缺的段落进 `current`。
///
/// 返回 `None` = 一条都不缺，一个字都不该写。这是绝大多数广播的结局：系统里到处在广播
/// `Environment`（改壁纸、改鼠标速度都发这一封），真正有环境变量变化的很少。
/// 追加顺序照 `desired` 原文，便于人肉比对注册表。
pub fn path_augment(current: &str, desired: &str) -> Option<String> {
    let have: Vec<String> = current.split(';').map(norm_entry).filter(|s| !s.is_empty()).collect();
    let mut missing: Vec<String> = Vec::new();
    for seg in desired.split(';') {
        let n = norm_entry(seg);
        if n.is_empty() {
            continue;
        }
        if !have.contains(&n) && !missing.iter().any(|m| norm_entry(m) == n) {
            missing.push(seg.trim().to_string());
        }
    }
    if missing.is_empty() {
        return None;
    }
    let mut base = current.trim_matches(';').to_string();
    if !base.is_empty() {
        base.push(';');
    }
    base.push_str(&missing.join(";"));
    Some(base)
}

/// 一份合并结果对着当前环境算「要写哪几格」——**只补不改**。
///
/// - `Path`（不分大小写）走 [`path_augment`]，只补不删；
/// - 其余格子：`expandable` 的先过 `expand`，**只有进程里根本没有这一格（或它是空串）才新增**，
///   已经有值的一律不动，哪怕和注册表不一样。`lookup` 认不出的格子当作"当前没有"⇒ 就是新增。
///
/// 为什么不是「值不同才覆盖」：注册表是登录时的**输入**，不是环境的**当前真相**——会话变量、
/// venv、IDE、启动器自己都在注册表之后改过格子。取证见文件头与 `target/sysmsg-envdiff.ps1`。
pub fn plan_changes(
    merged: &[Merged],
    expand: impl Fn(&str) -> String,
    mut lookup: impl FnMut(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (name, expandable, raw) in merged {
        if name.is_empty() {
            continue;
        }
        let want = if *expandable { expand(raw) } else { raw.clone() };
        if want.is_empty() {
            continue;
        }
        let now = lookup(name);
        if name.eq_ignore_ascii_case("path") {
            let augmented = match now {
                Some(cur) => path_augment(&cur, &want),
                None => Some(want.clone()),
            };
            if let Some(v) = augmented {
                out.push((name.clone(), v));
            }
        } else if now.as_deref().map_or(true, |v| v.is_empty()) {
            out.push((name.clone(), want));
        }
    }
    out
}

/// 广播附带的区域名是不是「Environment」。
///
/// 这个名字**不分大小写**。`lParam` 也可以是 NULL（有的程序只喊一声"设置变了"不说哪块），
/// 那条路在调用方就被挡掉、根本不该走到这里。
/// 只认这一件事：`Display`、`ThemeChange`、`WindowMetrics` 这些同一封信的其它来客一律不理，
/// 免得每次换壁纸都去读一遍注册表。
pub fn is_environment_str(area: &str) -> bool {
    area.eq_ignore_ascii_case("environment")
}

/// 本次刷新的结论，只用来写日志与计数——**不含任何值**。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// 写进了几格（含 PATH 那一格）。0 表示广播收到了但没有任何可应用的差异。
    pub applied: usize,
    /// `PATH` 那一格是否被补了段。单独记它是因为验收只看这一条。
    pub path_grew: bool,
}

impl Outcome {
    /// 日志句子。只有计数，没有变量名、没有值、没有路径——环境变量整柜子都是路径。
    pub fn log_line(self) -> String {
        format!(
            "env: 收到 Environment 广播，应用 {} 格{}",
            self.applied,
            if self.path_grew { "（含 PATH 补段）" } else { "" }
        )
    }
}

/// 合并两把键 → 与当前环境比 → 写进本进程。纯逻辑与写盘动作都在这里收口，
/// 好让真机取证与单测走的是**同一条**计划算法。
fn apply(
    machine: &[RegValue],
    user: &[RegValue],
    expand: impl Fn(&str) -> String,
    mut write: impl FnMut(&str, &str),
) -> Outcome {
    let merged = merge_values(machine, user);
    let changes = plan_changes(&merged, expand, |name| {
        std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
    });
    let mut out = Outcome::default();
    for (name, value) in &changes {
        write(name, value);
        out.applied += 1;
        if name.eq_ignore_ascii_case("path") {
            out.path_grew = true;
        }
    }
    out
}

/// 走一遍全流程并落进本进程的环境副本。**只在 UI 线程调**：
/// [`std::env::set_var`] 改的是进程级状态，与后台扫描线程的 `var_os` 读是一对竞争；
/// 把应用点收成一处（一帧之内），就不会出现"索引扫到一半 PATH 变了"那种中间态。
///
/// 调用方拿到 `applied > 0` 才该去重扫索引与路径四态——没有差异的广播占绝大多数，
/// 白重扫一次索引就是把用户的盘多读一遍。
pub fn refresh_now() -> Outcome {
    let (machine, user) = read_env_keys();
    apply(&machine, &user, expand, |name, value| {
        // SAFETY: 见函数注释——全仓只有这一处写环境变量，且固定在 UI 线程。
        // Rust 把它标成 unsafe 正是因为别的线程此刻读同一格会看到新旧之一（不会读到半截）。
        unsafe { std::env::set_var(name, value) };
    })
}

/// 离线入口（真机取证与单测共用）：喂进两把键的内容，只跑计划与 `set_var`。
/// **不读注册表、不广播、不碰别的进程。**
#[cfg(test)]
fn refresh_with(machine: &[RegValue], user: &[RegValue]) -> Outcome {
    apply(machine, user, |s| s.to_string(), |name, value| {
        unsafe { std::env::set_var(name, value) };
    })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `ExpandEnvironmentStringsW` 的薄包装：把 `%SystemRoot%\x` 落成 `C:\Windows\x`。
/// 展开失败（调用失败、串超长）就**原样返回**——宁可这一格因为值没变而不写，
/// 也不写一个被截断的值。
#[cfg(windows)]
pub fn expand(s: &str) -> String {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    use windows::core::PCWSTR;

    unsafe {
        let src = wide(s);
        let need = ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), None);
        // 两次调用之间格子被改大是可能的；上限防的是畸形输入，不是正常长度。
        if need == 0 || need as usize > 64 * 1024 {
            return s.to_string();
        }
        let mut dst = vec![0u16; need as usize + 2];
        let got = ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(dst.as_mut_slice()));
        if got == 0 || got as usize > dst.len() {
            return s.to_string();
        }
        String::from_utf16_lossy(&dst[..(got as usize).saturating_sub(1)])
    }
}

#[cfg(not(windows))]
pub fn expand(s: &str) -> String {
    s.to_string()
}

/// 枚举一把键下的所有值。读不动（键不存在、权限不够）就回**空表**——
/// 空表喂给 [`plan_changes`] 的结局是"什么都不写"，正是上面那条失败姿态。
#[cfg(windows)]
fn enumerate_values(
    root: windows::Win32::System::Registry::HKEY,
    subkey: &str,
) -> Vec<RegValue> {
    use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        KEY_READ, REG_EXPAND_SZ, REG_MULTI_SZ, REG_SZ, RegCloseKey, RegEnumValueW, RegOpenKeyExW,
        RegQueryInfoKeyW,
    };
    use windows::core::{PCWSTR, PWSTR};

    unsafe {
        let sub = wide(subkey);
        let mut hkey = Default::default();
        if RegOpenKeyExW(root, PCWSTR(sub.as_ptr()), None, KEY_READ, &mut hkey) != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut values = 0u32;
        let mut name_chars = 0u32;
        let mut data_bytes = 0u32;
        let ok = RegQueryInfoKeyW(
            hkey,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&mut values),
            Some(&mut name_chars),
            Some(&mut data_bytes),
            None,
            None,
        ) == ERROR_SUCCESS;
        let mut out = Vec::new();
        if ok {
            // 上限值含结尾 NUL，各留一格余量，省掉一轮 MORE_DATA 重试。
            let name_cap = (name_chars as usize + 2).max(64);
            let data_cap = (data_bytes as usize + 4).max(8);
            for i in 0..values {
                let mut name_buf = vec![0u16; name_cap];
                let mut nlen = name_cap as u32;
                let mut data_buf = vec![0u8; data_cap];
                let mut dlen = data_cap as u32;
                let mut kind = 0u32;
                let rc = RegEnumValueW(
                    hkey,
                    i,
                    Some(PWSTR(name_buf.as_mut_ptr())),
                    &mut nlen,
                    None,
                    Some(&mut kind),
                    Some(data_buf.as_mut_ptr()),
                    Some(&mut dlen),
                );
                if rc != ERROR_SUCCESS {
                    // MORE_DATA 说明上限值不可信（枚举途中键被改过）：这一格跳过，下一格照常。
                    if rc == ERROR_MORE_DATA {
                        continue;
                    }
                    break;
                }
                let name = String::from_utf16_lossy(&name_buf[..(nlen as usize).min(name_cap)])
                    .trim_end_matches('\0')
                    .to_string();
                let units: Vec<u16> = data_buf[..(dlen as usize).min(data_cap)]
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect();
                let utf16 = String::from_utf16_lossy(&units);
                let (expandable, raw) = if kind == REG_SZ.0 {
                    (false, utf16.trim_end_matches('\0').to_string())
                } else if kind == REG_EXPAND_SZ.0 {
                    (true, utf16.trim_end_matches('\0').to_string())
                } else if kind == REG_MULTI_SZ.0 {
                    // 少见但存在（有人把 Path 存成多行）：按 NUL 分段串起来，
                    // 那正是登录时会得到的形态。
                    (
                        false,
                        utf16
                            .split('\0')
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(";"),
                    )
                } else {
                    // REG_DWORD 之类不是环境变量。
                    continue;
                };
                if !name.is_empty() && !raw.is_empty() {
                    out.push(RegValue { name, expandable, raw });
                }
            }
        }
        let _ = RegCloseKey(hkey);
        out
    }
}

/// 读两把键：`(系统格子, 用户格子)`。非 Windows 平台回两张空表（本仓只在 Windows 跑，
/// 留这条路是为了 `cargo check` 在别的 target 上也不炸）。
#[cfg(windows)]
pub fn read_env_keys() -> (Vec<RegValue>, Vec<RegValue>) {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    (
        enumerate_values(HKEY_LOCAL_MACHINE, MACHINE_SUBKEY),
        enumerate_values(HKEY_CURRENT_USER, USER_SUBKEY),
    )
}

#[cfg(not(windows))]
pub fn read_env_keys() -> (Vec<RegValue>, Vec<RegValue>) {
    (Vec::new(), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rv(name: &str, expandable: bool, raw: &str) -> RegValue {
        RegValue { name: name.to_string(), expandable, raw: raw.to_string() }
    }

    fn m(name: &str, expandable: bool, raw: &str) -> Merged {
        (name.to_string(), expandable, raw.to_string())
    }

    /// 恒等展开器（单测里 `%X%` 由 `lookup` 之外自己控）。
    fn no_expand(s: &str) -> String {
        s.to_string()
    }

    /// `Path` 是拼接，其它同名格子是用户覆盖系统。
    #[test]
    fn merge_concatenates_path_but_user_wins_everything_else() {
        let machine = [rv("Path", false, r"C:\Windows"), rv("FOO", false, "machine")];
        let user = [rv("path", false, r"C:\Tools"), rv("FOO", false, "user")];
        let got = merge_values(&machine, &user);
        assert_eq!(got.len(), 2, "用户格不该新增第三条：{got:?}");
        assert_eq!(got[0], m("Path", false, r"C:\Windows;C:\Tools"));
        assert_eq!(got[1], m("FOO", false, "user"));
    }

    /// 两格里只要有一格是 `REG_EXPAND_SZ`，合成结果就得标成需要展开。
    #[test]
    fn merge_keeps_expandable_flag_from_either_side() {
        let machine = [rv("Path", true, r"%SystemRoot%;C:\a")];
        let user = [rv("Path", false, r"C:\b")];
        let got = merge_values(&machine, &user);
        assert!(got[0].1, "可展开标记不能因为用户那格是 REG_SZ 就丢");
        assert_eq!(got[0].2, r"%SystemRoot%;C:\a;C:\b");
    }

    /// 只有一格有 Path 时不产生空段，也不凭空造分隔符。
    #[test]
    fn merge_path_single_side_has_no_stray_separator() {
        let machine = [rv("Path", false, "C:\\only")];
        let got = merge_values(&machine, &[]);
        assert_eq!(got[0].2, "C:\\only");
        // 用户只有一格空 Path ⇒ 系统那格原样留下。
        let user = [rv("Path", false, "")];
        let got = merge_values(&machine, &user);
        assert_eq!(got[0].2, "C:\\only");
    }

    /// `path_augment` 的核心判据：大小写与尾随反斜杠都算同一个目录；什么都不缺回 `None`。
    #[test]
    fn path_augment_only_appends_genuinely_missing_entries() {
        let cur = r"C:\Windows;C:\tools\Bin";
        assert_eq!(path_augment(cur, r"c:\windows;c:\tools\bin\"), None);
        assert_eq!(path_augment(cur, r"C:\Windows;C:\new"), Some(format!("{cur};C:\\new")));
    }

    /// 追加不重复：`desired` 里同一段出现两次只补一次；已有的一段绝不落到结果里。
    #[test]
    fn path_augment_dedupes_within_the_wanted_list() {
        assert_eq!(path_augment("A", "B;B;C;A").as_deref(), Some("A;B;C"));
        // 当前为空 ⇒ 整条 desired 就是结果，且不带前导分号。
        assert_eq!(path_augment("", "B;C").as_deref(), Some("B;C"));
    }

    /// 当前没有 PATH 这一格 ⇒ 整条按注册表写入。
    #[test]
    fn plan_adds_path_when_process_has_none() {
        let merged = [m("Path", false, r"C:\Windows")];
        let got = plan_changes(&merged, no_expand, |_| None);
        assert_eq!(got, vec![("Path".to_string(), r"C:\Windows".to_string())]);
    }

    /// 非 PATH 格子按**只补不改**：进程里没有这一格（或它是空串）才新增，已有值的一律不动。
    /// 名字保留注册表原文大小写（`NEWguy` 原样写回，不是全大写）。
    #[test]
    fn plan_adds_absent_cells_and_never_touches_existing_ones() {
        let merged = [m("SAME", false, "x"), m("BLANK", false, "filled"), m("NEWguy", false, "fresh")];
        let got = plan_changes(&merged, no_expand, |n| match n {
            "SAME" => Some("x".to_string()),
            "BLANK" => Some(String::new()),
            _ => None,
        });
        assert_eq!(
            got,
            vec![("BLANK".to_string(), "filled".to_string()), ("NEWguy".to_string(), "fresh".to_string())],
            "空串算「没有这一格」⇒ 补；已有非空值的 SAME 不许动"
        );
    }

    /// 会话在注册表之后改过的格子，一个都不许动。
    ///
    /// 三条形状全取自 `target/sysmsg-envdiff.ps1` 的本机实测：照旧口径「值不同才覆盖」，
    /// `USERNAME` 会被改回注册表里的 `SYSTEM`、`MISE_STATE_DIR` 会被改回一个已废弃的旧目录、
    /// `PATHEXT` 会丢掉用户后加的 `.CPL`。这三条都不是"用户在系统属性里改了环境变量"，
    /// 而是会话/启动器在注册表之后动过——本函数分不出这两种，所以一律不猜。
    /// 第三条是 `REG_EXPAND_SZ`（展开后的值仍然与当前不同）：判据只看"当前有没有"，不看值，
    /// 所以展开成什么样都救不了它。
    #[test]
    fn plan_never_overwrites_a_cell_the_process_already_has() {
        let merged = [
            m("USERNAME", false, "SYSTEM"),
            m("MISE_STATE_DIR", false, r"C:\old\state"),
            m("PATHEXT", true, ".COM;.EXE"),
        ];
        let expand = |s: &str| s.replace(".EXE", ".EXE;.CPL");
        let got = plan_changes(
            &merged,
            expand,
            |n| match n {
                "USERNAME" => Some("baoqi".to_string()),
                "MISE_STATE_DIR" => Some(r"D:\newer\state".to_string()),
                "PATHEXT" => Some(".COM;.EXE;.CPL".to_string()),
                _ => None,
            },
        );
        assert!(got.is_empty(), "已有格子哪怕与注册表不同也不许写，实得：{got:?}");
    }

    /// 「只补不改」仍然要补**展开后**的值：进程里没有这一格时，写进去的必须是 `%V%\bin`
    /// 落成 `D:\tool\bin` 的那一份，而不是注册表原文（那一条是旧口径里唯一还成立的行为，单独钉住）。
    #[test]
    fn plan_writes_the_expanded_value_into_an_absent_cell() {
        let merged = [m("NEWEXP", true, r"%V%\bin")];
        let got = plan_changes(&merged, |s| s.replace("%V%", r"D:\tool"), |_| None);
        assert_eq!(got, vec![("NEWEXP".to_string(), r"D:\tool\bin".to_string())]);
    }

    /// 区域名只认 Environment，其它广播一律不理。
    #[test]
    fn area_match_is_case_insensitive_and_narrow() {
        assert!(is_environment_str("Environment"));
        assert!(is_environment_str("environment"));
        for other in ["Display", "ThemeChange", "WindowMetrics", ""] {
            assert!(!is_environment_str(other), "{other} 不该触发重读注册表");
        }
    }

    /// 空表（读键失败的形状）算不出任何写入：失败绝不等于清空环境。
    #[test]
    fn empty_registry_reads_plan_no_changes() {
        let got = plan_changes(&merge_values(&[], &[]), no_expand, |_| None);
        assert!(got.is_empty());
    }

    /// 日志句子只有计数；`applied=0` 时也不提 PATH。
    #[test]
    fn log_line_carries_counts_only() {
        let s = Outcome { applied: 2, path_grew: true }.log_line();
        assert!(s.contains("2"), "{s}");
        assert!(s.contains("PATH"), "{s}");
        assert!(!Outcome::default().log_line().contains("PATH"));
    }

    /// 端到端（离线）：喂真实形状的两把键，走 `set_var`，再读回来。
    /// 只用 `S1TEST_` 前缀的格子，测完删掉——不留残余，也不碰 PATH 这种共享格子。
    #[test]
    fn refresh_with_applies_user_wins_and_reports_counts() {
        // `S1TEST_B` 是"进程里先已有、且与注册表不同"的形状（会话/venv 在注册表之后改过的那种）。
        unsafe { std::env::set_var("S1TEST_B", "from_session") };
        let machine = [rv("S1TEST_A", false, "one"), rv("S1TEST_ONLYM", false, "keepme")];
        let user = [rv("S1TEST_A", false, "two"), rv("S1TEST_B", false, "from_registry")];
        let out = refresh_with(&machine, &user);
        assert_eq!(out.applied, 2, "A、ONLYM 都是新增：{out:?}");
        assert!(!out.path_grew);
        assert_eq!(std::env::var("S1TEST_A").unwrap(), "two");
        assert_eq!(std::env::var("S1TEST_ONLYM").unwrap(), "keepme");
        assert_eq!(
            std::env::var("S1TEST_B").unwrap(),
            "from_session",
            "已存在的一格不许被注册表覆盖（本批口径收窄的那条）"
        );
        // 第二轮同样的两把键 ⇒ 已经没有差异，一条都不该再写（幂等，防每次广播都重扫索引）。
        let again = refresh_with(&machine, &user);
        assert_eq!(again.applied, 0, "{again:?} 第二轮必须零写入");
        unsafe {
            std::env::remove_var("S1TEST_A");
            std::env::remove_var("S1TEST_ONLYM");
            std::env::remove_var("S1TEST_B");
        }
    }
}

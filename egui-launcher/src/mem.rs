//! 批次 REL（E-09 / NFR-03）：读本进程自己的内存与 GDI 对象数。
//!
//! 目的只有一个——让需求书 NFR-03 那句「常驻内存 ≤120MB」有一个可比的那一列。
//! 任务管理器里那列叫「内存(专用工作集)」，对应
//! `PROCESS_MEMORY_COUNTERS_EX2::PrivateWorkingSetSize`：可共享的页不计进来，
//! 因此它比 `WorkingSetSize` 小，也正是人眼平时盯着的那个数。EX2 要 Win8+，
//! 万一拿不到就退回 EX 的 `WorkingSetSize`（含共享页，偏大），**并把换过口径这件事
//! 一起交出去**——两个不同名的数混着报就是造假。
//!
//! 达标与否一律按**字节**判（取证脚本读性能计数器的原始字节），展示串（[`mib_text`]）只是给人看的，
//! 免得四舍五入把 119.6 说成 120、把 120.4 也说成 120 这种两头都能解释的场面。

/// 内存读数出自哪一列。换列 = 换口径，必须能被说出来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caliber {
    /// `PrivateWorkingSetSize`：专用工作集，与任务管理器「内存」同列。
    PrivateWorkingSet,
    /// `WorkingSetSize`：含共享页，偏大。只在 EX2 拿不到时用。
    WorkingSet,
}

/// 纯函数：两路 PSAPI 读数的取用规则。参数是 `(现在, 峰值)`，单位字节。
///
/// EX2 在 ⇒ 用 EX2 的专用工作集；只有 EX 在 ⇒ 用工作集并挂上换列的旗标；两路都坏 ⇒ `None`
/// （调用方报「—」，不报 0——0 MB 会被读成"这程序真省"，而事实是"没读到"）。
pub fn pick(ex2: Option<(u64, u64)>, ex: Option<(u64, u64)>) -> Option<(u64, u64, Caliber)> {
    match (ex2, ex) {
        (Some((now, peak)), _) => Some((now, peak, Caliber::PrivateWorkingSet)),
        (None, Some((now, peak))) => Some((now, peak, Caliber::WorkingSet)),
        (None, None) => None,
    }
}

/// 字节 → 显示用的一位小数 MiB 串（1 MiB = 1048576 字节；任务管理器那列写着"MB"、
/// 除的也是 1048576，所以这里跟它同口径）。
pub fn mib_text(bytes: u64) -> String {
    let mib = bytes as f64 / 1_048_576.0;
    format!("{mib:.1}")
}

/// 现在值＝**专用工作集**、峰值＝**工作集（含共享页）的峰值**，两者字节，外加口径。取不到 ⇒ `None`。
///
/// 这两格**不同列**，是 PSAPI 的既有事实而不是本模块的偷懒：`PROCESS_MEMORY_COUNTERS_EX2` 里
/// 没有"专用工作集峰值"这一格。所以口径串（`sysinfo.caliber.private`）必须把两列都念出来，
/// 而不是只说"专用工作集"——后者会让人以为峰值 170 MB 也是专用口径，进而以为它超了 120 MB。
#[cfg(windows)]
pub fn memory() -> Option<(u64, u64, Caliber)> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        PROCESS_MEMORY_COUNTERS_EX2,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    let process: HANDLE = unsafe { GetCurrentProcess() };

    let ex2 = || -> Option<(u64, u64)> {
        let mut c = PROCESS_MEMORY_COUNTERS_EX2::default();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
        let cb = c.cb;
        let ok = unsafe {
            GetProcessMemoryInfo(
                process,
                &mut c as *mut PROCESS_MEMORY_COUNTERS_EX2 as *mut PROCESS_MEMORY_COUNTERS,
                cb,
            )
        }
        .is_ok();
        ok.then(|| (c.PrivateWorkingSetSize as u64, c.PeakWorkingSetSize as u64))
    };
    // 【2026-11-01 本批自查修】这里原来写的是 `c.WorkingSetSize`——那是**现值**不是峰值，
    // 报出去就是"现在 21.8 / 峰值 21.8"，一个永远不会动的峰值。改成 `PeakWorkingSetSize`。
    // PSAPI 没有"专用工作集峰值"这个东西，所以峰值天生是含共享页那一列：换列这件事
    // 写进口径串里（`sysinfo.caliber.private`），不装作两格同源。
    // EX 的读数只有工作集一列，现值与峰值同源，不存在混列。
    let ex = || -> Option<(u64, u64)> {
        let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let cb = c.cb;
        let ok = unsafe {
            GetProcessMemoryInfo(
                process,
                &mut c as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
                cb,
            )
        }
        .is_ok();
        ok.then(|| (c.WorkingSetSize as u64, c.PeakWorkingSetSize as u64))
    };

    pick(ex2(), ex())
}

#[cfg(not(windows))]
pub fn memory() -> Option<(u64, u64, Caliber)> {
    None
}

/// GDI 对象数。`GetGuiResources` 失败也返回 0，与"真的 0 个"不可区分——
/// 判据用的是**同一次运行内的变化趋势**（NFR-03 那句"不持续增长"），不是绝对值，
/// 所以这里不做失败猜测。
#[cfg(windows)]
pub fn gdi_objects() -> u32 {
    use windows::Win32::System::Threading::{GetGuiResources, GetCurrentProcess, GR_GDIOBJECTS};
    unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) }
}

#[cfg(not(windows))]
pub fn gdi_objects() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1_048_576;

    #[test]
    fn pick_prefers_the_private_column() {
        let got = pick(Some((80 * MIB, 90 * MIB)), Some((120 * MIB, 130 * MIB)));
        assert_eq!(got, Some((80 * MIB, 90 * MIB, Caliber::PrivateWorkingSet)));
    }

    #[test]
    fn pick_falls_back_and_flags_the_different_column() {
        let got = pick(None, Some((120 * MIB, 130 * MIB)));
        assert_eq!(got, Some((120 * MIB, 130 * MIB, Caliber::WorkingSet)));
    }

    #[test]
    fn pick_both_failed_is_none_not_zero() {
        assert_eq!(pick(None, None), None);
    }

    #[test]
    fn text_rounds_to_one_decimal() {
        // 显示串会四舍五入：119.95 MiB 打成 "120.0"。所以"是否达标"从来不在这里判——
        // 取证读的是性能计数器的字节，这里只负责人类能读的那一位小数。
        let bytes = (119.95 * MIB as f64) as u64;
        assert_eq!(mib_text(bytes), "119.9");
        assert_eq!(mib_text((119.96 * MIB as f64) as u64), "120.0");
        assert_eq!(mib_text(MIB - 1), "1.0");
        assert_eq!(mib_text(0), "0.0");
    }

    #[test]
    fn reading_this_process_has_a_nonzero_working_set() {
        // 只钉两件事：读得到、且"现值（专用列）≤ 峰值（含共享列）"——跨列也成立的必然关系
        // （专用 ≤ 含共享 ≤ 含共享的峰值）。具体 MB 数不进断言——它是这台机器上别的进程
        // 一起挤出来的量。
        let (now, peak, caliber) = memory().expect("PSAPI 在 Win10+ 必读得到自己的进程");
        assert!(now > 0, "现在值 0 ⇒ 读数没接上");
        assert!(peak >= now, "峰值 {peak} 小于现在 {now} ⇒ 两列不同源");
        assert_eq!(caliber, Caliber::PrivateWorkingSet);
    }

    /// 【本批取证时真踩到的坑，这条就是那场事故的网】写工装时我把 `PageFaultCount` 当成
    /// `SIZE_T` 声明（它是 `DWORD`），整个结构从第 3 格起偏移 4 字节，于是读出
    /// 「专用工作集 21.8 MB／工作集 0.6 MB」——私有列不可能大于含共享列，一眼就该炸。
    /// 产品侧用的是 windows crate 自带的类型，布局由元数据保证，所以没中招；
    /// 但这条不变式只有读**真句柄**才验得出来，纯函数测试抓不到，故单独一条。
    #[test]
    #[cfg(windows)]
    fn the_two_columns_keep_their_containment_order() {
        use windows::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;

        let mut c = PROCESS_MEMORY_COUNTERS_EX2::default();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut c as *mut PROCESS_MEMORY_COUNTERS_EX2 as *mut PROCESS_MEMORY_COUNTERS,
                c.cb,
            )
        }
        .is_ok();
        assert!(ok, "读自己的 EX2 都能失败 ⇒ 句柄或 cb 传错了");
        let (peak_ws, ws, private_ws) = (
            c.PeakWorkingSetSize as u64,
            c.WorkingSetSize as u64,
            c.PrivateWorkingSetSize as u64,
        );
        assert!(private_ws <= ws, "私有列 {private_ws} 大于含共享列 {ws} ⇒ 字段偏移错位");
        assert!(peak_ws >= ws, "工作集峰值 {peak_ws} 小于现值 {ws} ⇒ 取错了列");
    }

    /// 【取证时另一条踩偏的地方，记在这儿】x64 上这个结构是 96 字节，不是 88：
    /// `cb` + `PageFaultCount` 两个 DWORD（合起来正好 8，没有填充），中间 10 个 `SIZE_T`，
    /// 末尾还跟着新 SDK 加的 `SharedCommitUsage`。读的三格（峰值 / 工作集 / 专用工作集）
    /// 都排在那条尾巴前面，所以尾巴多一个字段不会挪动偏移——但**手抄结构体的人**只要把
    /// `PageFaultCount` 当成 `SIZE_T`，后面每一格就整体错开 4 字节，而函数照样返回成功。
    /// 上一条断言负责钉"到底是几"，那条包含关系断言负责在真错开时先炸。
    /// 32 位不钉数字，本产品只出 x64。
    #[test]
    #[cfg(all(windows, target_pointer_width = "64"))]
    fn ex2_is_96_bytes_on_x64() {
        use windows::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS_EX2;
        assert_eq!(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>(), 96);
    }
}

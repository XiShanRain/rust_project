// 本批「给程序加自己的图标」的资源脚本。
//
// 只做一件事：把 assets/app.rc 编成 app.res，再作为链接输入递给 link.exe，
// 让 exe 的资源段里真的出现 RT_ICON / RT_GROUP_ICON（文件管理器那张白纸就是这么来的）
// 与 RT_VERSION（右键「属性 → 详细信息」里那几格，批次 VER 加进同一个 .rc 的）。
//
// 为什么调 rc.exe 而不是加 crate：本仓「不新增第三方依赖」是硬规，rc.exe 是 Windows SDK
// 自带工具（本机实测存在于 Windows Kits\10\bin\<版本>\<架构>\rc.exe），不进依赖面。
//
// 为什么整段可以静默跳过：图标是观感项，不是正确性项。没有 SDK 的机器（别人的电脑、
// CI）必须照样能 build，所以这里一律 `cargo:warning` + 返回，绝不 panic、绝不让构建失败。
// 代价是「本机有图标 ≠ 任何机器都有图标」，这条差异已写进发布说明本节。

use std::path::PathBuf;
use std::process::Command;

/// 版本号目录名按数字比，不按字典序：`10.0.9` 与 `10.0.10` 用字符串比会挑错那个。
fn version_key(name: &str) -> Option<[u64; 4]> {
    let mut parts = [0u64; 4];
    for (i, seg) in name.split('.').take(4).enumerate() {
        parts[i] = seg.parse().ok()?;
    }
    Some(parts)
}

/// 找 rc.exe。优先环境变量 `LAUNCHER_RC`（CI 可钉死），否则扫 Windows Kits 的版本目录，
/// 取版本号最大的那个里面对应架构的 rc.exe。
fn find_rc() -> Option<PathBuf> {
    if let Some(over) = std::env::var_os("LAUNCHER_RC") {
        let p = PathBuf::from(over);
        if p.is_file() {
            return Some(p);
        }
    }
    let arch = if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    let mut roots: Vec<PathBuf> = Vec::new();
    for key in ["PROGRAMFILES(X86)", "ProgramFiles(x86)", "PROGRAMW6432", "PROGRAMFILES"] {
        if let Some(v) = std::env::var_os(key) {
            let base = PathBuf::from(v).join("Windows Kits").join("10").join("bin");
            if base.is_dir() {
                roots.push(base);
            }
        }
    }
    let mut best: Option<([u64; 4], PathBuf)> = None;
    for root in roots {
        // 新布局：bin\<版本>\<架构>\rc.exe
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let Some(key) = version_key(&entry.file_name().to_string_lossy()) else {
                    continue;
                };
                let cand = dir.join(arch).join("rc.exe");
                if cand.is_file() && best.as_ref().map_or(true, |(k, _)| key > *k) {
                    best = Some((key, cand));
                }
            }
        }
        // 旧布局：bin\<架构>\rc.exe（没有版本层）
        let flat = root.join(arch).join("rc.exe");
        if flat.is_file() && best.is_none() {
            best = Some(([0, 0, 0, 0], flat));
        }
    }
    best.map(|(_, p)| p)
}

/// 跳过时喊的两句。批次 ICON 起这段话写的是「只影响 exe 的文件图标」——批次 VER 把
/// VERSIONINFO 也挂进同一个 `assets/app.rc` 之后那句话不再成立，所以前缀一并改成「资源脚本跳过」；
/// 批次 SKIN 再加了一条 `1 WAVE`，同一个文件、同一次 rc.exe，所以少了这一段现在是三样一起没有：
/// 文件图标、右键属性里那几格版本信息、呼出提示音。其余不受影响。
fn skip(msg: &str) {
    println!("cargo:warning=资源脚本跳过：{msg}");
    println!("cargo:warning=跳过影响 exe 的文件图标、版本信息（右键属性里那几格）与内嵌提示音，功能与测试不受影响。");
}

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    // 批次 SKIN：`1 WAVE "popup.wav"` 引的是 .rc 同目录下的那颗 wav。rc.exe 只认文件在不在，
    // 换掉 wav 的内容不会碰到 app.rc / app.ico，没有下面这一行 cargo 就不会重跑 rc.exe，
    // 产物里的提示音还是旧的那串字节（批次 TRAY 就是被 `rerun-if-changed` 没触发坑过一次）。
    println!("cargo:rerun-if-changed=assets/popup.wav");
    // 窗口/任务栏图标走的是 `include_bytes!`，cargo 不会自动跟踪那条路径，
    // 贴片重画后必须靠这一行才会重编。
    println!("cargo:rerun-if-changed=assets/app-icon-64.rgba");
    if !target.contains("windows") || !target.contains("msvc") {
        skip(&format!("目标 {target} 不是 Windows MSVC"));
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let rc_file: PathBuf = manifest.join("assets").join("app.rc");
    let ico_file: PathBuf = manifest.join("assets").join("app.ico");
    let wav_file: PathBuf = manifest.join("assets").join("popup.wav");
    if !rc_file.is_file() {
        skip("缺 assets/app.rc");
        return;
    }
    if !ico_file.is_file() {
        skip("缺 assets/app.ico");
        return;
    }
    // 这颗不在的话 rc.exe 自己会报 `error RC2135 : file not found` 并留一个非零退出码，结局与
    // 这里主动跳过完全一样（整段资源都不产），只是喊出来的是「退出码 1」而不是原因。提前查一下，
    // 把那两句 warning 换成能读懂的那一句。
    if !wav_file.is_file() {
        skip("缺 assets/popup.wav");
        return;
    }
    let Some(rc_exe) = find_rc() else {
        skip("本机没找到 Windows SDK 的 rc.exe（可设 LAUNCHER_RC 指路）");
        return;
    };
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    let res_file = out_dir.join("app.res");
    let _ = std::fs::remove_file(&res_file);
    let status = Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_file)
        .arg(&rc_file)
        .status();
    match status {
        Ok(s) if s.success() && res_file.is_file() => {
            // 一个裸 .res 路径就是 link.exe 的输入文件，不需要额外的开关。
            println!("cargo:rustc-link-arg={}", res_file.display());
            // 成功只留一行普通输出（`cargo:warning` 会被算进本仓「零警告」那道闸的分子，
            // 而跳过才是需要喊出来的那一半）。
            let bytes = std::fs::metadata(&res_file).map(|m| m.len()).unwrap_or(0);
            println!("icon resource: {res_file:?} ({bytes} bytes, rc.exe = {rc_exe:?})");
        }
        Ok(s) => skip(&format!("rc.exe 退出码 {s}")),
        Err(e) => skip(&format!("rc.exe 起不来：{e}")),
    }
}

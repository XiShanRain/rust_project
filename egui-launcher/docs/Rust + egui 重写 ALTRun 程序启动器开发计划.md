# Rust + egui 重写 ALTRun 程序启动器开发计划

> ## 修订说明（2026-10-14）
>
> 本次修订基于对两个参考项目的逐文件盘点（`D:\temp\AltRun参考项目`）：
>
> | 参考项目 | 版本 | 体量 | 实际定位 |
> | --- | --- | --- | --- |
> | `ALTRun-master-delphi版` | V1.46.7 | 项目自身约 10,600 行 Pascal（不含 3rdUnit） | 快捷项启动器：`ShortCutList.txt` + 通配符/正则过滤 + 参数标记 `{%p}`/`{%c}` + `ALTRun.ini` 6 页设置 + 分层透明皮肤窗口 |
> | `ALTRun-main-AHK版` | 2026.10.12.19 | 约 21,000 行 AHK v2 | Alfred/Raycast 级启动器：14 个数据源 Provider + 打分表/拼音/学习排序 + `→` 操作面板 + 21 套主题 + 19 页偏好设置 + `ALTRun.json`(SchemaVersion 4) |
>
> **原计划存在的问题**：1.3 节的"ALTRun 功能对照表"、第二章优先级矩阵、第五章末尾的"与 ALTRun 结构对照"只对应 **Delphi 版**，完全没有覆盖 AHK 版的数据源体系、匹配打分与操作面板；因此按原计划做完，产物只是 Delphi 版的等价复刻，达不到 1.1 节自述的"替代并超越"。
>
> **本次改动清单**：
>
> 1. 1.1 节补充目标基线：**以 AHK 版核心搜索能力 + Delphi 版独有机制为范围基线**。
> 2. 1.3 节改为 Delphi / AHK / egui-launcher **三方对照表**，并按 `src/main.rs` 实测修正"当前状态"。
> 3. 第二章改为**功能域优先级矩阵**，逐项标注来源（D/A/N）、优先级、所属阶段、对应《需求说明书》的 FR 编号区间；明细编号由需求书持有，避免两份文档各写一份清单而失同步。
> 4. 第三章由 5 阶段改为 **6 个里程碑（M1–M6）**，并显式映射到《需求说明书》的**阶段一（功能全量）**与**阶段二（视觉打磨）**。
> 5. 4.1 节**勘误**：eframe 0.36.2 的 `App` trait 方法是 `fn ui(&mut self, ui: &mut Ui, frame: &mut Frame)`，**不是** `update(&mut self, ui)`；`CentralPanel::default().show(ui, ...)` 传入 `ui`。现有 `src/main.rs` 已按正确签名编译通过，可作实证。
> 6. 新增 4.6/4.7 两节：匹配打分模型、后台线程与并发模型（这两块是 AHK 版的主要复杂度所在，原计划未提）。
> 7. 第五章模块结构补 `provider/`、`matching/`、`index/`、`actions/`；第六章依赖清单按 crates.io 与 `Cargo.lock` 实测更新，确定 **windows 0.62.2**（eframe/winit 已引入同版本，避免重复主版本）。
> 8. 明确**范围裁剪**：AHK 版的专业/行业扩展（PTTools、SPF2M、QuickSwitch 对话框跳转与文件夹面板、一键加日期、CapsLock 切输入法、结构计算、货币换算）**不纳入**；文件搜索后端定为**内置索引**，不实现 Everything IPC。
> 9. 工作量口径修正：原计划 5 阶段合计 19 人天对应的是 Delphi 版范围；本修订后按 AHK 核心 + Delphi 独有的范围重估为 **39 人天净工作量 / 建议 50 人天排期**。
>
> 配套文档：`docs/ALTRun 启动器需求说明书.md`（本计划中所有 FR/NFR 编号以该文档为准）。

## 第一章：项目概述

### 1.1 项目目标

本项目旨在使用 Rust 语言与 egui 即时模式 GUI 框架，从零构建一款轻量、极速、跨平台潜力良好的程序启动器，以替代并超越经典的 ALTRun 工具。核心目标包括： - 极致的响应速度：利用 Rust 的内存安全与零成本抽象，实现毫秒级搜索与启动。 - 现代化的交互体验：无边框、透明背景、全局热键唤起、失焦自动隐藏。 - 完善的程序管理：自动扫描系统路径、开始菜单，支持自定义收藏与参数配置。 - 低资源占用：常驻后台时内存占用控制在 20MB 以内。



> **1.1 修订（2026-10-14）**
>
> **目标基线**：本项目**不是** Delphi 版 ALTRun 的复刻，而是以 **AHK 版 2026.10.12.19 的核心搜索能力**为主体、叠加 **Delphi 版独有机制**（参数标记与参数输入、通配符搜索、快捷键管理器的失效校验/路径批量转换）的 Windows 启动器。凡两个参考项目冲突之处，以 AHK 版的交互模型为准（它更新、且是 Delphi 版的超集演化方向）；凡 AHK 版缺失而 Delphi 版有之的，单独补齐。范围明细见《需求说明书》第 1.4 节"范围与非目标"。
>
> **"内存 20MB 以内"目标不成立，需修正**：该指标源自 Delphi 版 VCL 原生程序。eframe 单窗口进程的常驻内存由 winit 事件循环 + 渲染上下文（glow/wgpu）+ egui 纹理缓存主导，与托管的语言无关，实测普遍在数十至上百 MB 量级。**修订为：常驻后台 ≤120MB（以实际后端为准 —— 2026-10-15 更正：一直是 wgpu，当前候选限制为 `DX12|GL`，不是原文写的 glow），且必须实测后再定稿**。截至本次修订，本机尚未跑出新框架下的实测值，该上限属**待校准目标**，不作为验收硬门。见《需求说明书》NFR-03。
>
> **"毫秒级搜索"目标补充口径**：AHK 版为此专门做过两轮优化（JSON 读取线性化使 140KB 配置从 1.1s 降到 50ms；命令首字符匹配从 300–400ms 降到 10–20ms）。Rust 版必须继承的是**结论而非巧合**：输入到列表刷新 ≤16ms（单帧）、热启动 ≤1s、后台索引不得进入主线程。见 NFR-01/02。



### 1.2 技术栈说明

•    **核心语言**：Rust (Edition 2024)，本机 rustc 1.96

•    **GUI** **框架**：egui 0.36.2 + eframe 0.36.2（~~渲染后端固定 **glow**，不用 wgpu~~ **2026-10-15 更正：该选型从未生效，eframe 0.36.2 默认特性不含 `glow`，实现一直是 wgpu；现把候选后端限制为 `DX12|GL`。**~~选型已于同日定稿：沿用 wgpu + DX12\|GL，不开 `glow` 特性~~ 二次更正：「不开 `glow`、沿用 wgpu + `DX12\|GL`」成立，但同批宣布的「透明实测闭环」被用户肉眼推翻 —— **DX12 的 HWND 交换链按 DXGI 规则不可能带逐像素 alpha，只有 GL 可以**；因此阶段一撤掉 `with_transparent(true)`，阶段二要做透明必须把后端改成只留 `Backends::GL`**，依据与四组对照实测见需求书 E-18**）

•    **系统交互**：windows crate **0.62.2**（Win32 / COM 绑定；`Cargo.lock` 中已由 eframe→winit 引入同主版本，直接复用可避免一份 crate 树里出现两个 windows 大版本）

•    **数据与配置**：serde + serde_json（**不用 TOML**，见修订说明）+ 自管 SchemaVersion 迁移

•    **并发**：std::thread + std::sync::mpsc + arc-swap（**不引入 tokio**，见 4.7 节）

•    **目标平台**：Windows 11 (x64)；Windows 10 1903+ 尽力可用但不作验收目标

•    **开发环境**：VS Code + rust-analyzer



> **1.2 修订（2026-10-14）**
>
> - ~~**渲染后端固定 glow**：需求要求窗口透明、圆角、颜色键、点击穿透（HUD）与逐帧自绘结果行。eframe 的透明与圆角在 glow 下路径最短、wgpu 在 Win11 无边框透明窗口上的黑闪问题更难绕。锁定后端也让 NFR 的内存/首帧指标可复现。~~
>   **（2026-10-15 更正，理由不成立的部分已作废）**：① 这条从未落地——`eframe = "0.36.2"` 用的是默认特性，而 eframe 0.36.2 的 `default` 含 `wgpu`、**不含 `glow`**（`eframe-0.36.2/Cargo.toml:60-69`），`Renderer::default()` 即使两者都开也返回 `Wgpu`（`epi.rs:606-618`），所以程序一直在 wgpu 上跑。② "wgpu 在 Win11 无边框透明窗口黑闪"这一条~~**在本机 DX12 后端下未复现**（截图核对：现有窗口配置渲染正确、无黑块）~~，但当前 `CentralPanel` 是不透明底色，逐像素透明/圆角尚未实现，**因此不能据此完全推翻原理由**，只能记为待实测。**（2026-10-15 同日二次更正：这条现在应验了 —— 用户肉眼实测，`with_transparent(true)` + 半透明绘制在 DX12 下就是暗红块 + 四角残留黑。先前"未复现"是因为只测了不透明底色的 `CentralPanel`，等于没测透明。原计划"黑闪/黑块"的方向是对的，我当时判"待实测"才是对的，不该在同一天升级为"闭环"。）**③ ~~真正必须定的是：照计划真的开 `glow` 特性，还是确认沿用 wgpu/DX12 并改计划 —— 见需求书 E-18~~ **【2026-10-15 已定：沿用 wgpu + `DX12\|GL`，不开 `glow`。】**~~决定性依据是透明路径 —— `egui-wgpu/src/winit.rs:272-288` 会查询并优先选 `PreMultiplied` alpha mode~~ **（2026-10-15 同日作废该依据：这个 alpha 能力查询在 Windows 上恒失败 —— wgpu-hal 对 HWND 表面硬编码只上报 `[Opaque]`（`wgpu-hal-30.0.1/src/dx12/adapter.rs:1364`），egui-wgpu 必然回退 `Auto`；真正的分水岭是呈现路径本身，DXGI 交换链不带 alpha、WGL 像素格式带，跟 eframe 的协商代码无关。保留成立的部分：glow 侧无此协商、且 eframe 的"GL 不支持透明"检查在 Windows 根窗口被 `!cfg!(target_os="windows")` 绕过；开 glow 还要 `default-features=false` 加回 7 个特性并新增 4 个 crate。）**~~实测：`ADAPTER=Dx12`，帧缓冲预乘 alpha 与圆角裁剪均正确~~ **该实测只证明了离屏帧缓冲；屏幕合成结果由用户肉眼判定为"不透"，GL 后端才透 —— 四组对照见需求书 E-18。**）④ 无论选哪个，**候选后端都必须排除 Vulkan**（本机 Intel Vulkan 驱动确定性崩，见第七章风险表与需求书附录）。
> - **配置格式由 "TOML/JSON" 收敛为 JSON**：两个参考项目实际都用非严格 JSON 类格式——AHK 版是 `ALTRun.json`（含 `SchemaVersion`、`.bad`/`.bak`/`v<N>.backup.json` 三套容错与迁移文件），Delphi 版是 `ALTRun.ini` + `ShortCutList.txt`（管道分隔行 + `F` 前缀频率）。**TOML 无先例**，且用户手写数据里含大量 Windows 路径，TOML 的反斜杠转义会持续踩坑。选型定为：主配置 JSON，用户自定义命令/片段同样 JSON，路径统一用正斜杠或双反斜杠并由 `path_utils` 归一。
> - **不引入 tokio**：原计划第七章风险表建议"图标提取放在 tokio/async_std 后台线程"。本项目的后台工作是**阻塞型 Win32 调用与目录遍历**，没有 I/O 多路复用需求；引入异步运行时只会与 winit 的所有权模型打架。改用专职扫描线程 + `mpsc` 回投 + `arc-swap` 发布索引快照（详见 4.7，并已在第七章对该行作勘误）。
> - **Windows 10 兼容降级**：`DWMWCA_WINDOW_ROUNDED_COLOR`/圆角属性、`SetPreferredAppMode`（uxtheme 序号 135/136，AHK 版要求 build ≥ 18362）在旧版本不存在，UIX 视觉项必须逐项做版本门槛判断。



### 1.3 ALTRun 功能对照表

> **1.3 修订（2026-10-14）**：原表只列 Delphi 版一列功能，现改为 **Delphi 版 / AHK 版 / egui-launcher 现状** 三方对照。"当前状态"按 `src/main.rs`（99 行，已编译产出 `target/debug/egui-launcher.exe`）逐行核对，不是估计。原表 12 行 → 现 24 行，并新增"来源"列（D=Delphi，A=AHK，N=两者）。

| 功能域 | Delphi 版 V1.46.7 | AHK 版 2026.10.12.19 | egui-launcher 当前状态 | 来源 | 优先级 |
| --- | --- | --- | --- | --- | --- |
| 主启动器界面 | `frmALTRun`：无边框 + 分层透明 + 置顶 + 皮肤图 + 列表 + 命令行预览条 | `-Caption +AlwaysOnTop +ToolWindow`，输入框 + 自绘 ListView + 状态栏，窗口高随结果行数伸缩 | 已有窗口与列表渲染（假数据 6 条）；**已设** `with_decorations(false)`/`with_transparent(true)`/`with_always_on_top()`；无状态栏、窗口高固定 500×400 | N | P0 |
| 程序数据模型 | `TShortCutItem{ShortCutType,ParamType,ShortCut,Name,CommandLine,Rank,Freq}` | `ResultItem{Title,Subtitle,Icon,Kind,Arg,Arguments,Uid,OnRun,RunTitle,Actions,Valid,AutoComplete,LargeText,Score,Exclusive,Pinned,Source,Provider}` | 仅 `AppEntry{name: String, path: String}`，**未 derive Serialize/Deserialize**，无参数/工作目录/Kind/Uid | N | P0 |
| 搜索与过滤 | 通配符 `*`→`.*`、`?`→`.` 后 TRegExpr 匹配；`Rank = 1024 + Freq*4 - Rank*128 - (len差异)*16` | `FuzzyMatcher` 七档分值表 + 词首字母 + 拼音首字母 + `lengthPenalty`，再叠 Provider 分值偏移与 `Knowledge.Boost` | 只有 `name.to_lowercase().contains(q)`，**无打分、无排序、无拼音、无高亮区间** | N | P0 |
| 键盘导航 | ↑↓/Tab/PgUp/PgDn/滚轮；Enter+Space+双击+**中键**+数字键 `1-9 0 ; '` 执行；Ctrl+C/D/L、Ins、F2、Del | ↑↓/`Ctrl+P/N`、`Ctrl+↑/↓` 翻历史、PgUp/PgDn、`Ctrl+1..9`、Tab 补全、`→` 进操作面板、`←`/Backspace 返回、Insert 标记、Space 进文件模式 | **完全未实现**：结果项只能用鼠标点击；Esc 走 `ViewportCommand::Close` 直接**退出进程**而非隐藏 | N | P0 |
| 全局热键唤起 | `RegisterHotKey`（HotKeyManager）主 `Alt+R` + 次 `Pause` + `ALT+L` 重跑上次 | `General.Hotkey "!Space"` + `SecondaryHotkey "!r"` + 双击 Ctrl/Shift + 自定义 `Hotkeys[]` | 未开始（`Cargo.toml` 里 `global-hotkey` 已被注释） | N | P0 |
| 失焦自动隐藏 | `tmrFocus` 500ms + `HideDelay` 分钟级空闲隐藏 | `WM_ACTIVATE` 低 16 位=0 → 50ms 后 `_HideIfInactive`，`_keepOpen` 期间豁免 | 未开始 | N | P0 |
| 真实程序启动 | 工作线程 `ShellExecute(GetDesktopWindow,'open',…)` → 失败退 `WinExec` → 提示 `resCanNotExecute`；`@+/@-/@` 控制窗口状态；工作目录自动取目标所在目录 | `ActionCatalog.RunDefault` 按 `Kind` 分派 file/folder/url/text；`*RunAs` 提升；Store 应用走 `shell:AppsFolder\<AppID>` | `std::process::Command::new(path).spawn().expect(...)` ——**对 .lnk/URL/Store 应用无效，且失败会 panic**（`expect`） | N | P0 |
| 程序来源扫描 | 不扫描系统，靠用户手工维护 `ShortCutList.txt`（`.\` 相对 exe 目录、`%VAR%` 环境变量展开） | ~~扫 `Features.Applications.Folders` × `FileTypes`（深度 3、`Exclude` 正则）+ `.lnk` 解引用 + `Get-StartApps` 的 Store 应用 + `Data\AppIndex.json` 缓存 + 60min 刷新~~ **【2026-10-16 需求变更，与 Delphi 版口径趋同：改为策展按需 —— 内置种子 + 用户清单（`app_index.json` 语义=用户清单，事件写入、无自动重扫/刷新）；`.lnk` 解引用约束保留（作用域=解析被导入的单条，见 FR-SRC-02/附录 E-03）；SendTo「发送到」+ 应用内添加 + 无匹配预填 + 选中快捷键 + 可选批量导入（FR-SRC-20），单实例+IPC 转发（FR-SRC-21），条目管理（FR-SRC-22），无匹配兜底运行（FR-SRC-23）；Store 应用 `Get-StartApps` 与 60min 刷新仅在「批量导入」档相关】** | 未开始 | N | P1 |
| 图标 | `TImageList ilHotRun` | `IconCache`（LRU 上限 1500）+ 图标语法 `res:`/`ext:`/`thumb:`/`folder:`/`shell:` + 启动 2s 后预读 300 个 | **完全无图标** | N | P1 |
| 多数据源体系 | 无（单一快捷项列表） | **14 个 Provider**：应用/自定义命令/片段/剪贴板历史/计算器/网页搜索/书签/窗口切换/文件搜索/脚本/终端/系统命令/速查/置顶最近 | 未开始 | A | P0（框架）+P1 |
| 操作面板 | 仅列表右键菜单 Add/Edit/Delete/Open Dir | `→`/右键进入 actions 模式，`ActionCatalog.ListFor` 提供 16+ 动作（打开/管理员/打开方式/所在位置/复制路径/复制名/终端/属性/回收站/大字/编辑/删除/置顶/复制/粘贴/全部打开） | 未开始 | A | P1 |
| 参数与命令扩展 | **参数标记** `{%p}` `{%c}` `{%wd}` `{%wt}` `{%wc}` + `ParamType`（无编码/URL 查询/UTF8 查询）+ `frmParam` 输入框 + `ParamHistory.txt` LRU 50 | `{query}` 与 `{query\|默认值}` 占位符、片段占位符 `{date}`/`{clipboard:n}`/`{uuid}`/`{cursor}` | 未开始 | N | P1 |
| 配置持久化 | `ALTRun.ini`（`[Config]` 26 键、`[Window]`、`[GUI]`、`[DEBUG]`），加载时主动删除遗留键 | `ALTRun.json`（`SchemaVersion: 4` + `SchemaMigration` + `.bad`/`.bak`/`v<N>.backup.json` + `DataLocation` 迁移） | 未开始 | N | P0 |
| 快捷键管理 | `frmShortCutMan`：ListView 4 列、增删改排序、**绝对↔相对路径批量转换**、**失效项校验并批量删除**（`frmInvalid` 勾选清单） | 偏好设置 Commands 页 + `F3` 编辑 + `Ctrl+Del` 删除 + 资源管理器"发送到 ALTRun"添加 + **Check Paths**（`OK`/`Not found`/`Offline`/`-` 四态） | 未开始 | N | P1 |
| 收藏 / 置顶 | `FavoriteList.txt`（`keyword\|name`，成功执行即自动记）+ `RememberFavouratMatch` 置顶 | `Features.Recent.Pinned` + `RecentCount` + 空输入框默认结果 + `Pin to Empty Search Box` 动作 | 未开始 | N | P0 |
| 系统托盘 | `TCoolTrayIcon`：单击与双击均切换、气泡提示、菜单 Show/ShortCut/Config/About/Close | `Menu Tray`：Show / Preferences / — / Rebuild Index / Check for Updates / — / Reload / Quit；左键单击呼出；`TaskbarCreated` 消息重建图标 | 未开始 | N | P1 |
| 开机自启 | 生效实现是往 `CSIDL_COMMON_STARTUP` 写 `.lnk`（注册表 Run 分支代码存在但调用已注释），首次启动询问 | `A_Startup\ALTRun.lnk -Startup` + SendTo + 开始菜单快捷方式，**只删自己创建的** | ~~未开始~~ ~~**三项齐（批次 L + M，2026-10-19）**~~ **两项落地 + 第三项裁定不做（批次 L，2026-10-19）**：开机自启＝设置页「通用」开关 → 写 `%APPDATA%\...\Programs\Startup\egui-launcher.lnk` 带 `-Startup`（**默认关**，2026-10-19 用户裁定，不做"首次启动询问"）；SendTo 已于 2026-10-17 落地；`ChangeWindowMessageFilterEx` 对本消息窗放行 `WM_COPYDATA` 同批落地；~~**未做＝开始菜单快捷方式**（Windows 搜索找得到本程序那条）~~【2026-10-19 批次 M 曾落地：设置页第三个开关，写 `…\Programs\egui-launcher.lnk`、参数为空；两枚入口抽成同一套 `ShellLinkSlot` 槽位模型，**参数串既是写入内容也是归属凭据**】【同日按「绿色软件」取向**撤除**：用户正文「如果要做绿色软件，我感觉没有必要加，那就删除往系统开始菜单加lnk的功能」⇒ 该项不做，两个源文件回到 `bf3c092` 的状态（`cargo test` 45/45），被撤除的改动留档在 `target/batch-m-start-menu.patch`（`target/` 不进仓库）⇒ FR-WIN-11 现行口径＝SendTo + 开机自启两项落地，开始菜单那项裁定不做】。归属判据比参考项目更严：target 与参数两个都要对得上才认领【2026-10-26 批次 E3-乙 裁定（用户正文「按你的建议: 甲 → 乙 -> 丙」，乙＝"注册表与启动文件夹哪个更好，帮我拍板"）：**维持启动夹 `.lnk`，不改注册表 `Run`** —— 功能等价（都不比对方更早或更可靠）、与「绿色软件」取向一致、开关态可肉眼核对（读磁盘上的 `.lnk` 而不是看 regedit）、归属凭据更强（`.lnk` 同时带 target 与参数，`Run` 只有一串命令行）；唯一翻盘条件＝将来做 Store/MSIX 打包（要用 `StartupTask` 扩展）。参考项目"注册表分支代码存在但调用已注释、最终写 `CSIDL_COMMON_STARTUP`"即同一结论的旁证。】 | N | P2 |
| 单实例 | 命名互斥体 `ALTRUN_MUTEX` + INI 里存 `HandleID` + `SendMessage(WM_ALTRUN_ADD_SHORTCUT/SHOW_WINDOW)`；`ALTRun.exe Restart/Clean/<路径>` CLI 协议 | `#SingleInstance Off` + `WM_COPYDATA`(`0x414C5452`="ALTR") 交接给在跑实例，无应答才强杀；`-Startup/-Reloaded/-Preferences N/-SendTo/-Update` | M1 已落地（互斥体 + `WM_COPYDATA` 2s 超时 + 无应答强杀接管；CLI 见 FR-WIN-08） | N | P1 |
| 剪贴板集成 | 只做单向读：`{%c}` 取剪贴板 + `CTRL+C` 复制命令行 | **剪贴板历史**（文本/文件/图片，`clip` 或 `Ctrl+Alt+C`，上限 200 条、`IgnoreApps` 排除密码管理器、临时借用不写历史） | 未开始 | N | P1 |
| 多语言 | `.lang` 文件（EN/简体中文/繁体中文/PT-BR，各 224 键，8 个 section）+ 首次运行 `frmLang` 选择 | `Resources\Lang\{zh-CN,zh-TW,ja}.json` + `_name/_font/_fallback` 元键 + `auto` 跟随系统 + 单测断言每键都翻译 | 硬编码中文 `"搜索程序..."`/`"未找到匹配的程序"` | N | P2 |
| 日志 | `ExeName.username.log`，双开关（`TraceEnable` + `DEBUG_MODE`，发布版实际关闭） | `%Temp%\ALTRun.log`，`General.SaveLog` 开关，`Perf:` 前缀记耗时，**明确不记用户输入内容** | ~~未开始~~【2026-10-19 批次 O+P 后为**部分**：文件与控制台两路都有（无控制台时 stderr 落 `launcher.log`，终端启动照旧实时刷）、按日期轮转 + 总量上限、「打开日志」入口（托盘菜单项；~~+ `Ctrl+Shift+L`~~ 那条键当日按用户裁定撤除）、隐私脱敏 2 处（旧版真机漏词已有实物样本 `launcher.log` 里的 `「ba」`，修复后的复测还没跑）；仍无 `tracing`、级别开关、`Perf:` 统一前缀】 | N | P2 |
| 背景图 / 皮肤 | `BG.jpg`（exe 目录，缺省从内嵌 JPEG 自动导出），关闭可省约 1M 虚拟内存 | 属于主题系统的 `Background` 纯色 + `Opacity`，无位图皮肤 | `with_transparent(true)` 已开但无内容 | D | P3 |
| 正则搜索 | 通配符先转正则再 TRegExpr 匹配，用户可写正则 | 无独立正则模式（用打分匹配 + `Exclude` 正则在配置里） | ~~未开始~~【2026-10-28 更正：C2 已实现（§五十五），通配符路径下"其余字符按正则解释"正是本行说的"用户可写正则"——两者是同一套 `regex` 引擎，只是入口是查询串而非配置。`Exclude` 配置正则仍未做，不在本批】 | D | P2 |
| 反馈层（HUD / 大字 / 状态栏提示） | `frmAutoHide` AlphaBlend 淡出提示 + `Popup.wav` 音效 + `HintList[0..24]` 轮换提示 | HUD 气泡（`WS_EX_NOACTIVATE\|TRANSPARENT`，点击穿透，`App.Notify` 1500ms）+ `Ctrl+L` 大字显示 + 状态栏键帽提示 + `?` 速查表 | ~~未开始~~ **HUD 气泡已交付（2026-10-24，FR-UIX-09：`src/hud.rs`，deferred 子视口 + 点击穿透 + 不抢焦点，真机通过）；大字显示 / 皮肤音效 / 画廊仍未开始** | N | P2/P3 |

**范围排除（本次修订明确不做，需求书 1.4 节有完整清单）**：PTTools / SPF2M 结构工程计算、QuickSwitch 对话框跳转与文件夹面板、一键加日期 AutoDate、CapsLock 切输入法（含管理员计划任务小助手）、结构计算与钢筋参数、货币换算（外网 API）、Everything IPC、以管理员实例替换普通实例、Scoop/winget 分发（保留 cargo-dist）。



## 第二章：功能优先级矩阵

> **第二章 修订（2026-10-14）**
>
> 原表 16 行（F01–F16）是 Delphi 版范围的功能清单，与 1.3 节修订后的基线不符；且若在本章再维护一份逐条功能清单，会和《需求说明书》的编号系统各写一遍、必然失同步。
>
> 因此本章**改为功能域矩阵**：每行一个需求域，给出该域的来源、优先级、所属阶段、依赖与《需求说明书》中的 FR 编号区间。**逐条功能需求的定义、触发方式、处理规则与验收标准，统一由《需求说明书》第 5 章持有**（共 80 条 FR + 10 条 NFR；2026-10-15 因新增 FR-SYS-08「CJK 可渲染」由 79 条增至 80 条）。
>
> 本章不再放"预估工作量"列——工作量按里程碑估算（第三章 M1–M6，合计 39 人天），若同时给按域和按里程碑两套估算，两者难以自洽。

| 需求域 | 内容摘要 | 来源 | 最高优先级 | 依赖 | 所属阶段 | 需求书编号区间 |
| --- | --- | --- | --- | --- | --- | --- |
| 窗口与唤起 | 无边框置顶工具窗口、全局主/次热键、双击 Ctrl/Shift、失焦自动隐藏、焦点与置顶、按键抢跑、单实例 `WM_COPYDATA` 接管、CLI 参数协议、位置与多显示器落点、输入法处理、开机自启/SendTo/开始菜单 | N | P0 | 无 | **阶段一** | FR-WIN-01 … FR-WIN-11（11 条） |
| 输入解析与匹配 | `关键字+空格=独占模式`、前缀标记（`>` `'` `?` 空格）、七档模糊打分表、拼音首字母、高亮区间、学习排序 Boost、稳定排序+去重+上限、无结果兜底、通配符转正则、历史回溯 | N | P0 | 程序数据模型 | **阶段一** | FR-QRY-01 … FR-QRY-10（10 条） |
| 数据源体系 | Provider 框架与逐项容错，及 18 类源：应用、自定义命令、命令路径四态校验、片段、片段自动展开、剪贴板历史、计算器（表达式/单位/进制/日期）、网页搜索、浏览器书签、窗口切换、文件搜索（内置索引）、脚本、终端、系统命令、速查表、置顶与最近、图标 | A 为主 | P0（框架）/ P1 | 窗口与唤起、匹配 | **阶段一** | FR-SRC-01 … FR-SRC-19（19 条） |
| 执行与操作 | Enter/双击/中键/数字键、按 `Kind` 的启动语义、`→` 操作面板与 ActionCatalog、以管理员/打开方式/属性/所在位置/回收站/复制路径、Insert 多标记与批量、Tab 自动补全与进入文件夹、参数标记 `{%p}/{%c}/{%wd}/{%wt}/{%wc}` + 编码类型 + 参数历史、`@+/@-/@` 运行标志、键盘导航总表 | N | P0 | 匹配 | **阶段一** | FR-ACT-01 … FR-ACT-10（10 条） |
| 配置与持久化 | JSON 配置模型 + `SchemaVersion` 迁移、`.bad`/`.bak` 损坏恢复、数据目录策略（便携/`%APPDATA%` 退化/`DataLocation`）、偏好设置各页、快捷键录制控件、快捷项批量管理（绝对↔相对、失效校验批删）、收藏/置顶持久化、恢复默认与保留项 | N | P0 | 数据源 | **阶段一** | FR-CFG-01 … FR-CFG-08（8 条） |
| 系统集成与可观测 | 托盘与菜单、图标提取与 LRU 缓存、日志（不记输入内容）、多语言 zh-CN/en、后台线程与索引快照、使用统计、环境变更刷新与工作集修剪、**CJK 可渲染（2026-10-15 新增，阶段一 P0）** | N | P0（CJK 可渲染）/ P1 | 执行与操作 | **阶段一**（统计/修剪为 P2） | FR-SYS-01 … FR-SYS-08（8 条） |
| 界面与视觉 | 主题系统与 `Base` 继承、24 个主题键、结果行版式、匹配高亮渲染、选中态圆角、窗口圆角/边框色/透明度、淡入淡出、状态栏与 ☰ 菜单、HUD 气泡、大字显示、皮肤背景图与提示音、主题缩略图画廊、空态视觉、高 DPI 与主题字体+宽度预热（基础 CJK 可渲染已拆到阶段一 FR-SYS-08，2026-10-15） | N | P1 | 阶段一全部功能 | **阶段二** | FR-UIX-01 … FR-UIX-14（14 条） |
| 非功能 | 搜索延迟、启动时间、内存、稳定性与崩溃边界、隐私（无遥测/不记输入）、兼容性门槛、可维护性与测试、可访问性与键盘可达、打包产物、异常恢复 | N | — | 全部 | 两阶段各自收口 | NFR-01 … NFR-10（10 条） |

**优先级口径（与需求书一致）**：P0 = 阶段一必须交付，缺失则产品不可用；P1 = 阶段一目标项，允许在 M4 末尾降级；P2 = 阶段一尾期或阶段二初补做；P3 = 阶段二内可选，不进首发验收。

**两阶段边界（本修订确立，需求书第 8 章展开）**：

- **阶段一 = 功能全量**。所有 66 条 FR-WIN/FR-QRY/FR-SRC/FR-ACT/FR-CFG/FR-SYS 需求，用 **egui 默认控件**实现（默认 `Visuals`、默认字体、`Frame`+`ScrollArea` 拼出的可用界面）。判据是"功能可被操作并完成端到端验证"，不是"好看"。
  - **唯一例外（2026-10-15）**：默认字体不许原样用。egui 自带的 Ubuntu-Light 不含 CJK 字形，中文直接渲染成 `.notdef` 方块，属功能不可用而非"不好看"，因此**中文字体回退划入阶段一 P0（FR-SYS-08）**；阶段二（FR-UIX-14）只保留"主题字体 + 高 DPI + 宽度预热"。Latin 字形与字距仍用默认字体，回退字体追加在字体族末尾。
- **阶段二 = 视觉打磨**。14 条 FR-UIX，以及把阶段一的默认控件替换为自绘结果行、主题化、圆角透明、状态栏、HUD、大字模式。**阶段二不得新增功能**，只允许为视觉目标做性能优化（自绘文本测量、图标缓存、虚拟滚动）。
- **一条硬约束**：阶段一必须把"每行 = 图标 + 标题 + 副标题 + 右侧快捷键提示"的数据结构先建好（`ResultItem` 字段齐全），即使当时不渲染副标题。否则阶段二的自绘行会反过来改动匹配与数据源逻辑，两阶段就白分了。



## 第三章：里程碑计划（M1–M6）

> **第三章 修订（2026-10-14）**
>
> 原 5 阶段（合计 19 人天）对应的是 Delphi 版范围，无法容纳 AHK 核心基线；且阶段之间与《需求说明书》的"阶段一功能 / 阶段二视觉"没有对应关系。现重排为 **6 个里程碑**：
>
> | 里程碑 | 所属阶段 | 净工作量 |
> | --- | --- | --- |
> | M1 骨架、窗口与可执行链路 | 阶段一 | 5 |
> | M2 数据源全量 | 阶段一 | 10 |
> | M3 操作面板与参数体系 | 阶段一 | 5 |
> | M4 配置、持久化与系统集成 | 阶段一 | 8 |
> | M5 视觉系统 | 阶段二 | 6 |
> | M6 反馈层、统计与发布 | 阶段二 | 5 |
> | **合计** | | **39 人天净工作量** |
>
> **排期口径**：39 人天是"单人、熟悉 Win32、不含返工"的净估算。本计划的依赖清单里有 4 处（`.lnk` 解析、Everything-free 文件索引、`RegisterHotKey` 与 winit 消息循环的接合、egui 透明窗口的圆角/点击穿透）**必须实测才能定方案**，历史上这类点平均带来 25–30% 返工。**建议对外排期按 50 人天**，并在 M1 结束时用实测重估 M2–M4。
>
> **2026-10-14 实测进度**：上述 4 处中 **`.lnk` 解析、`RegisterHotKey` 与 winit 消息循环的接合**已闭环（前者选 `IShellLinkW`，见 4.7；后者选专职 message-only 线程，且实测推翻了"隐藏用 `Visible(false)`"——会把唤起延迟从 1.6ms 抬到 118.1ms（**2026-10-15 更正：右端 118.1ms 成立、左端 1.6ms 作废**：park 那批的收表判据在按下前就已满足，1.6ms 实为唤醒延迟 1.7 / 3.5ms，~~含抢焦点的 park 端到端尚无数据~~ **【同日 20:41 已补出：物理 × park 有效 7 轮端到端 min/中位/max = 12.2 / 14.7 / 58.4 ms、抢焦点 7/7 ⇒ 真实差距约 8 倍而非 74 倍；14.7ms 中位超出需求书 FR-WIN-02 的 ≤10ms 验收阈值，待用户裁决；~~样本 7 < 20，还需再跑一轮~~】** **【同日 21:21 已复跑：单轮有效 15、抢焦点 15/15、端到端 min/中位/max = 12.5 / 15.7 / 47.6 ms；两轮合并 = 22 个有效轮次 22/22、中位 15.2 ms，22 个读数里没有一个 ≤10ms ⇒ 阈值冲突坐实，不能靠多采几轮蒙过去；"跨轮合并算不算满 20 轮"属验收口径，由用户裁定（**同日已裁定：合并成立，FR-WIN-05 的验收措辞改为"累计 ≥20 个有效轮次、可跨多次运行合并" ⇒ E-02 / E-16 闭环，不再补第三轮；阈值那一半同日裁定 (a)：FR-WIN-02 放宽为"中位 ≤20ms 且不得出现 ~100ms 平台" ⇒ 实测中位 15.2ms 达标、冲突解除，尾部 4 个读数（24.8 / 25.6 / 47.6 / 58.4 ms）记录在案但不作闸门 ⇒ 本行两条冲突全部落地**）】**，见需求书附录 E-02/E-16，见第三章 M4 修订项 2 与需求书附录 E-01/E-02）。余 2 处待实测：**Everything-free 文件索引**、**egui 透明窗口的圆角/点击穿透**〔**2026-10-24：「点击穿透 + 不抢焦点」这一半已由 E-08 实测闭环**——deferred 子视口 + `with_mouse_passthrough(true).with_active(false)` 成立，主窗隐藏后 HUD 自绘回调仍在跑；后端前置（只留 `Backends::GL` + 恢复 `with_transparent(true)`）已随批次 F1 落地，见需求书 E-08 与 FR-UIX-09 注记。**「圆角半径 / 边框色 / 阴影 / DWM 属性」仍是 E-07 的开放面，本条不因此改写为整格闭环。**〕。另新增一条与排期相关的环境问题：~~本机核显 OpenGL 驱动在 `run_native` 期间随机崩进程（4–8 次/10，与窗口选项无关），详见第七章风险表末段~~ **（2026-10-15 已闭环：不是 OpenGL 而是 Intel Vulkan 驱动，不是随机而是确定性（默认 8/8 崩），根因是 eframe 默认走 wgpu 且 wgpu 优先枚举 Vulkan；限制候选后端为 `DX12|GL` 后本机 5/5 正常，排期阻塞解除，E-15/E-16/E-17 恢复可执行。详见第七章风险表与需求书附录 E 的根因更正节）**。
>
> **两处需求级变更**（不是措辞调整）：
>
> 1. **默认热键由 `Win+Space` 改为 `Alt+Space`（主）+ `Alt+R`（次）**。原计划第三章阶段3 定的 `Win+Space` 与 Windows 系统级的"切换输入法 / 打开任务视图"绑定冲突，注册后基本无法稳定生效。AHK 版实测默认就是 `!Space` + 次热键 `!r`，Delphi 版默认 `Alt + R`——两个参考项目都避开了 Win 组合键，本修订与之一致。
> 2. **`WM_HOTKEY` 的接合方式（2026-10-14 已实测定稿，首选方案成立）**：专职 message-only 线程 —— 后台线程 `RegisterClassExW` + `CreateWindowExW(HWND_MESSAGE)` + `RegisterHotKey(hwnd, ...)`，线程内 `GetMessageW` 循环收 `WM_HOTKEY`，经 `mpsc` 投给主线程，主线程在 **`App::logic`（不是 `App::ui`）**里消费；唤醒靠任意线程的 `ctx.request_repaint()`，它会走 `EventLoopProxy::send_event(UserEvent::RequestRepaint)` 唤醒 `ControlFlow::Wait`，因此**不需要**挤进 winit 事件循环。原计划设想的 `raw_input_hook` / `SetWindowsHookExW` 两条路仍不成立（前者收不到 Win32 消息，后者是全局键盘钩子）。备选方案（winit 子类化）**未用**，首选已够。
>    实测附带结论：① 隐藏**不能**用 `ViewportCommand::Visible(false)`，eframe 对不可见窗口的重绘钳到 100ms，唤起端到端实测中位 **118.1ms**；~~改"窗口常驻可见 + 移出屏幕"后中位 **1.6ms**（这两项延迟读数在 2026-10-15 的复测中依旧成立）~~ **【2026-10-15 二次更正：park 侧站得住的只有"唤醒中位 1.7 / 3.5ms"；那个 1.6ms 端到端与 park 的 20/20 命中同源于一个恒真判据 —— 探针收表条件是"前台已是自己或超时"（`src/bin/winprobe.rs:736-750`），而 park 下按下之前前台就是自己，第一 tick 即满足 ⇒ 它量的是唤醒时间，不含抢焦点、也不含移出屏幕后的重新出帧】**（数据 `experiments/e01-e02-hotkey-report.txt` / `-park-report.txt`；10-14 的 park 报告在今日重跑覆盖前已备份为 `.2026-10-14.bak.txt`）。**结论方向不变（依然禁用 `Visible(false)`，因为那 100ms 节流是真的），但"74 倍"这个幅度不再成立，park 的真实端到端~~挂在 `hotkey 20 physical park` 补测上~~ **【同日 20:41 已补出】**`experiments/e02-physical-park-hotkey-report.txt` 有效 7 轮 min/中位/max = 12.2 / **14.7** / 58.4 ms、抢焦点 **7/7** ⇒ 与方案 A 的 117.5ms 相比实测约 **8 倍**（不是 74 倍）；两条新后果：① 14.7ms **超出需求书 FR-WIN-02 验收写的"中位 ≤10ms"**，"消除 ~100ms 平台"达成、"≤10ms"未达成，阈值放宽还是列 M1 优化项**待用户裁决**；② ~~有效样本仅 7（用户按到第 9 次显示键后停手，60 秒看门狗收尾），撑不起"20 轮命中率"验收 ⇒ 还要再跑一轮凑满 20 个有效轮次，协议不变（每轮先点击别的应用）~~ **【同日 21:21 已按同一协议复跑（每轮显示前先点别的应用）：显示键 16 次、剔除 1 次创建即持焦、有效 15 轮 15/15、端到端 12.5 / 15.7 / 47.6 ms；两轮合并 = 22 个有效轮次 22/22、min/中位/max = 12.2 / 15.2 / 58.4 ms（逐轮重算已复核）⇒ 命中率这一半只剩口径问题（"跨轮合并算不算满 20"由用户裁定 ⇒ 【同日已裁定：合并成立】，FR-WIN-05 的验收措辞同步改为"累计 ≥20 个有效轮次、允许跨多次运行合并"，故命中率判为闭环，不再要求真人补第三轮）；延迟那一半不是采样噪声 —— 22 个读数没有一个 ≤10ms，16 个落在 12.2–17.4 ms，FR-WIN-02 的 ≤10ms 阈值冲突坐实，~~放宽还是列 M1 优化项待用户裁决~~；同日裁定 (a)：放宽为"中位 ≤20ms 且不得出现 ~100ms 平台" ⇒ 冲突解除，实测中位 15.2ms 达标，尾部 4 个读数（24.8 / 25.6 / 47.6 / 58.4 ms）记录在案但不作闸门**】。**② 热键被占用时 `RegisterHotKey` 返回 HRESULT `0x80070581`（Win32 `1409 ERROR_HOTKEY_ALREADY_REGISTERED`），据此区分"被占用"与其它失败。③ ~~`ViewportCommand::Focus` 20/20 拿到前台，不需要 `AllowSetForegroundWindow`/最小化-还原。~~ **【2026-10-15 更正】**这条只在**方案 A `Visible(false)`** 下成立（注入 20/20、物理有效 19/19）；当年"移出屏幕"那 20 轮的 20/20 经复测判定**判据恒真、有效命中 0** —— park 模式下每次按显示键时前台窗口**本来就是探针自己**，所以"`Focus` 在方案 B 下能抢回焦点"从未被证明，不需要 `AllowSetForegroundWindow`/最小化-还原 这条也随之降级为**待证**（详见需求书附录 E 的 E-01/E-02 更正块）。**【同日 20:41 已证伪"未证明"这半句】**物理 park 补测里 `ViewportCommand::Focus` 单独发、不带任何辅助手段，**有效 7 轮全部抢到前台（7/7）** ⇒ 方案 B 下"`Focus` 单独就够"从待证升为~~有 7 轮支持、待满 20 轮复算~~**【同日 21:21 复跑后：单轮 15/15、两轮合并 22/22】**。④ **新增一条设计约束（同日实测导出）**：把窗口移到屏幕外**不改变焦点归属**，19/19 轮隐藏后前台仍是启动器自己 ⇒ park 隐藏**必须显式交还焦点**，否则用户按下隐藏键后键盘输入会打进一个看不见的窗口；**同日 20:41 的物理复测又当场复现一次**（开局两轮用户还没点击别的应用，`按下时前台` 就等于 park 中窗口的 hwnd `0x3c0d5e`）⇒ 注入 19/19 + 物理 2/2 两类独立证据，这条不再是推断；已写进需求书 FR-WIN-02 第 ③ 项，交还手段待 E-15 复测。~~**【2026-10-15 E-15 cloak 实测已答：cloak 同样不交还焦点（20/20 先验成立）⇒ 硬前置对 park 与 cloak 两种隐藏手段都成立，交还动作必须自己做，cloak 不自带交还效果】**~~
>    **遗留（新增待实测项）**：实测按键为 `SendInput` 自注入，物理按键下的命中率需 M4 人工复测（**2026-10-15 已部分补测：物理有效 19/19（原报 21/21，2 次按下时前台已是自己、剔除），但跑在方案 A 上 ⇒ 方案 B 那一格待补；且同日 park 注入基线的 20/20 已作废 ⇒ ~~方案 B 现在是"注入也无证据"~~【同日 20:41 更新：方案 B 的物理那一格已由 7 个有效轮次填上（抢焦点 7/7、端到端中位 14.7ms），"无证据"现在只剩"注入 × 方案 B"这一半；~~有效样本 7 < 20 ⇒ 命中率与延迟都还要再跑一轮才够验收口径~~**】同日 21:21 已复跑：单轮有效 15、抢焦点 15/15、端到端 12.5 / 15.7 / 47.6 ms，两轮合并 22 个有效轮次 22/22、中位 15.2 ms ⇒ 方案 B 的命中率与延迟都已有两轮一致样本，剩下的只差"跨轮合并算不算满 20"这条口径，由用户裁定）；本机 `Alt+Space`/`Alt+R` 被运行中的 ALTRun 占用，故用实测空闲的 `Ctrl+Alt+Space`/`Ctrl+Alt+R` 跑，机制等价但 `Alt+Space` 撞系统菜单与否未验；"移出屏幕"与 `DWMWA_CLOAK`、失焦隐藏的相互影响列为 M1 验证项（**2026-10-15 已答一半：焦点归属那半实测完成，`DWMWA_CLOAK` 那半未测**）。
>    **2026-10-15 进度**：物理按键复测**部分完成** —— 真人按 `Ctrl+Alt+Space` 21 次，~~21/21 抢到前台焦点~~ ⇒ 更正为**有效 19/19**（剔除 2 次按下时前台已是窗口自身），但**测的是方案 A `Visible(false)`**，~~方案 B 那一格仍空。补测命令是一条 `winprobe hotkey 20 physical park`，**但必须配合协议：每轮显示之前用鼠标点击另一个应用**，否则采到的还是先验成立的轮次（探针现在会打印"探针窗口 hwnd"并在汇总里把这类轮次计为"剔除"，剔除数过半即整份作废重跑）。~~**【同日 20:41 该协议已由用户执行完毕：`winprobe hotkey 20 physical park`，数据 `experiments/e02-physical-park-hotkey-report.txt`。显示键按下 9 次，开局 2 次"按下时前台"就等于 park 窗口自身 `0x3c0d5e`（先验成立、剔除，同时当场复现"park 不交还焦点"），有效 7 轮抢焦点 7/7、端到端 min/中位/max = 12.2 / 14.7 / 58.4 ms；60 秒看门狗在第 9 轮后收尾，所以样本只有 7 ⇒ 方案 B 从"零证据"变为"7 轮全命中、待满 20 轮"。三条必须一起读的限定：① 报告汇总行打印的端到端 min 是 0.9ms，那是修复前的旧二进制把先验轮次也算进了延迟样本，本行 12.2ms 是按 7 个有效轮次重算的真值，引用以重算为准；② 报告的唤醒 0.3 / 2.0 / 15.5 ms 把显示键与隐藏键的送达混在同一个 push 点统计（`src/bin/winprobe.rs:634-639`），单看显示键那 9 轮是 0.3 / 0.3 / 11.2 ms；③ 14.7ms 中位超出需求书 FR-WIN-02 写的"中位 ≤10ms"——"消除 ~100ms 平台"达成、"≤10ms"未达成，阈值放宽还是列 M1 优化项待用户裁决，我不擅自改验收标准。】** **【同日 21:21 已复跑，"凑满 20 个有效轮次"这一条照协议执行】** `winprobe hotkey 20 physical park`（探针 hwnd `0x4f0d5e`；第一轮报告被同名覆盖，已落盘留档为 `experiments/e02-physical-park-hotkey-report.round1-7of20.txt`）：显示键按 16 次，1 次"按下时前台"等于探针自身 hwnd（创建即持焦，与"park 不交还焦点"不是同一条路径），剔除后**有效 15 轮、抢焦点 15/15、0 失败**，端到端 min/中位/max = 12.5 / **15.7** / 47.6 ms。三点必须一起读：其一 汇总行打印的端到端 min 12.5ms 与逐轮重算一致 ⇒ 上面①那条"先验轮次污染 min"的口径瑕疵**已修复并经第二轮验证**；其二 两轮合并 = 22 个有效轮次 22/22、min/中位/max = 12.2 / 15.2 / 58.4 ms，**22 个读数里没有一个 ≤10ms** ⇒ FR-WIN-02 的 ≤10ms 冲突是稳定实测而非采样噪声，不能靠多采几轮蒙过去（同日裁定 (a)：FR-WIN-02 阈值放宽为"中位 ≤20ms 且不得出现 ~100ms 平台" ⇒ 冲突解除，实测中位 15.2ms 达标；尾部 4 个读数 24.8 / 25.6 / 47.6 / 58.4 ms 记录在案但不作闸门，延迟分解不列 M1 硬项）；其三 单轮 15 仍 < 20，"跨轮合并算不算满 20 轮"属验收口径，**由用户裁定**，我不自行宣布 E-16 清零【同日裁定结果：合并成立 ⇒ E-16 / E-02 记为已闭环，FR-WIN-05 的验收措辞同时改为"累计 ≥20 个有效轮次、允许跨多次运行合并"，无需第三轮】。另记一条操作层面的坑：本轮之前有一次 0 轮废跑 —— 用户还在读协议时 60 秒首键看门狗就到期收尾，根因在 `idle_limit`（物理模式下首键与后续键共用同一个 60 秒），已改为**首键前宽限 300 秒、开按后仍 60 秒**并重编译，废报告留档 `experiments/e02-physical-park-hotkey-report.watchdog-empty-run.txt`。 另有一条现状由用户肉眼确认：**失焦自动隐藏（FR-WIN-04）目前完全没有实现** —— 探针与 `src/main.rs` 都没有任何焦点丢失处理，所以"点别的窗口它不消失"是当前代码的正确行为，属 M1 待实现，不是热键缺陷。**同日由 park 复测新查出的一条**：park 隐藏不交还焦点（19/19 轮隐藏后前台仍是自己）⇒ FR-WIN-04 实现时必须与"移出屏幕"隐藏打通，这是 M1 的实现项而不是可选项。

### M1：骨架、窗口与可执行链路（阶段一，5 人天）

**里程碑目标**：一条可回归的功能主干——真实数据源之一进得来、搜得出、选得动、启得起来，窗口行为符合启动器语义。

**任务清单**：

1. **模块骨架落地**：把单文件 `main.rs` 拆为 `app / entry / matching / provider / launcher / hotkey / config` 七模块（详见第五章），先只接一个数据源。
2. **`ResultItem` 数据模型**：对齐 AHK 版字段集——`title / subtitle / icon / kind(file\|folder\|url\|text\|command) / arg / arguments / uid / on_run(闭包或枚举) / run_title / actions / valid / auto_complete / large_text / score / exclusive / pinned / source / provider`；`#[derive(Serialize, Deserialize)]`，路径一律 `PathBuf` 或带 `A_` 变量语法的字符串。
3. **输入解析**（FR-QRY-01/02）：`keyword + 空格 ⇒ 独占模式`；前缀标记 `>`、`'`、`?`、空格文件模式。这是数据源框架的前置，必须在 M1 定形态。**【2026-10-17 状态：FR-QRY-01 的三元组已在 `src/matching.rs` 的 `parse()` 落地；独占与 02 的前缀标记未定形态——依赖的 FR-SRC-14/16/18 数据源还不存在，现在做只有空路径，改到 provider 那批一起做。见需求书 §5.2 落地注记。】**
4. **七档模糊打分**（FR-QRY-03）：100 全等 / 90 前缀 / 80 词首 / 75 词首字母 / 60 包含（位置衰减 + `lengthPenalty`）/ 45 多词 / 30 子序列；`Key()` 预计算 `{lower, words, initials, length}`。**M1 只做英文与子序列，拼音在 M2 接**。**【2026-10-17 已落地：七档 + `MatchKeys` 预计算 + 内容指纹重建，逐档单测覆盖；拼音首字母已由 FR-SRC-22 的 `keyword` 承载（`default_keyword` 生成），不另建 `pinyin` 键。高亮区间未做（用户裁定推到界面精修）。1000 条实测最差 1.033ms（debug、不含绘制）。】**
5. **键盘导航**（FR-ACT-10）：↑↓ / PgUp / PgDn / Enter / 双击 / **中键** / 数字键 `1-9 0` / Tab / Esc。**Esc 必须是"清空后隐藏"，不是退出进程**——现 `src/main.rs` 走 `ViewportCommand::Close`，属于行为错误。**【2026-10-17 落地注记（批次 E）】Esc 的行为错误早已改对（三段：弹窗→清空输入→关到托盘，已复测）；本批补齐 `PgUp/PgDn` 整页翻页 + 列表滚动跟随选中项（结果列表改 `ScrollArea`）+ `Tab` 补全选中项名 + `1-9 0`/`Ctrl+数字`/`Alt+数字`/中键执行。注入按键实测：500×400 窗口一页 9 行，PgDn 走 1→10→19、PgUp 回 10，第 19 行截图确认滚到视口中位；`jsq` 下 Tab 补成「计算器」；`Ctrl+1`/`Alt+1`/空输入裸 `1` 均执行第 1 行并 `launch: calc.exe`；非空输入下裸 `2` 落进搜索框（截图 `计算器2`）未被接管。【2026-10-18 用户复测改判：本批初版中键执行的是选中项（~~中键点窗口任意位置即执行选中项~~），改为**指针所指的那一行**，仅当点在非结果行（搜索框/提示条/空白）才兜底执行选中项；实测光标在第 3 行、选中项为第 1 行时执行「记事本」`launch: notepad.exe`，光标在搜索框时执行选中项。行命中带已横向补到列表右边缘】。【2026-10-18 批次 F 再改判：上面那条改判~~（中键执行指针所指行）~~整体作废——用户裁定"启动器以键盘为主，中键改成拖动窗口的功能"，中键现在不执行任何动作；移动窗口改为**左键拖空白**，隐形手柄铺在运行时量出来的三段真空白上（搜索页顶栏「搜索框右缘到＋新建左缘」、列表右侧空白、设置页顶栏「设置」标题右侧）。实现坑两条实测记录在案：① `ViewportCommand::StartDrag` 会把鼠标交给系统拖拽循环并吃掉抬起事件，egui 的左键从此逻辑卡在按下态——用户复测的三个症状（设置页点不动、右键菜单弹不出、第二次拖不动）都是这一个根因；且它只跟左键，中键用它实测零位移（矩形 208,208 → 208,208）。现改为按下时取 Win32 光标与窗口原点做锚点、之后按帧 `SetWindowPos` 跟随，坐标一律物理像素（egui 的 `outer_rect` 要等 winit 回读 `WM_MOVE`，按帧用它算位移实测滞后：注入 +120/+80 只走出 +98/+33，注入 -60/-40 反而走出 +9/+15）。② egui `ScrollArea` 的内容 Ui 由 `ui.new_child(...)` 创建、只覆盖 `max_rect` 不覆盖 `layout`（scroll_area.rs:805-809），会继承外层 `Ui::horizontal` 的 LeftToRight——设置页整块内容因此被挤成一行并溢出右边界，用户看到的"设置页没有空格开关"是这个排版缺陷、不是功能没做。本批同时落地：空格执行（默认仅输入框为空时接管；设置页「通用」的「空格执行选中项」开关打开后任何时候执行，值写 `settings.json`）、删除确认框（`Delete` 与 `Ctrl+Shift+D` 都先弹「确认删除〈名称〉」，起因是用户实测时误删了「控制面板」）、结果列表前 10 行行首标 `1…9、0` 与裸数字键同口径。~~待真人复测：顶栏拖窗手柄两处、设置页排版修复后的空格开关可见性。~~【2026-10-18 复测结论：三项全过——设置页「通用」默认就显示两个开关（不用滚动）、搜索页与列表右侧空白可拖、空格执行开关读写正常、中键确认无任何动作。复测又揪出两个真缺陷并已修完：① 设置页内容区只有约 28px 高，用户症状"要滚鼠标才看到第二个配置项"，根因除了上面那条 layout 继承，更直接的是 `Ui::horizontal` 把行高初值写死成 `spacing().interact_size.y`（ui.rs:2376-2379），在横向行里量 `available_height()` 只有一行高——改为在 TopDown 父 Ui 上量高度、并用 `allocate_ui_with_layout` 给内容区显式分配"剩余宽 × 整列高"；② 设置页顶栏拖不动，根因是手柄右边界取了横向行 `response.rect.max.x`，而它只覆盖**已用宽度**（返回按钮加标题约 110px），算出负宽矩形被守卫静默丢弃——改用 `available_rect_before_wrap()` 的整行右边界。另登记一条现状：设置页右侧内容区被 `ScrollArea` 认领拖拽（0.36 不导出 `drag_to_scroll`，关不掉），背景层手柄抢不到命中，设置页只有顶栏能拖；用户裁定"这样也满足需求"。】本批再追加一项：**拖边缘改窗口大小**（用户 2026-10-18 提"窗口应该允许拖边缘改变大小"；~~待真人复测~~【同日复测 D/E/F/G 四项全过：手柄命中并出双向箭头、缩到最小不再变小且不翻转、拖左缘右缘钉住、`Ctrl+=`/`Ctrl+-` 逐档生效、缩放后拖动与点击/右键/`Enter` 不受影响】）。`with_decorations(false)` 连系统可命中的调整边框一起关了（`HTLEFT`/`HTRIGHT`/`HTTOP`/`HTBOTTOM` 及各角），Windows 不会告诉我们"正在拖边"，所以自绘四边减四角、四角各 6×6 共 8 块 6px 隐形手柄（铺在 `Context::viewport_rect()` 上，本帧最后铺设，同 Background 层后铺者赢），悬停用 `on_hover_cursor` 给双向箭头；动作复用按下取 Win32 锚点、之后按帧 `SetWindowPos` 那条已跑通的路径，拖左/上边钉住对侧，下限 360×240（逻辑像素写、按 `pixels_per_point` 换算后夹）。键盘路径按立规补齐 `Ctrl+=` / `Ctrl+-`（一档 60×40，`ViewportCommand::InnerSize`）；`Ctrl+0` 因与"执行第 10 行"冲突**不做重置键**，登记为偏差。三条偏差已登记在需求书 FR-ACT-01/06/10 落地注记：数字键按结果行而非视口可见行、`;`/`'` 别名不做、~~`Tab` 暂为"补全名称"（FR-ACT-06 的文件夹进目录语义等 FR-SRC-14 落地后按 `kind` 分派）~~【2026-10-18 批次 G 更正：Tab 已改成两种口径可配，进目录语义仍等 FR-SRC-14】】**。 **【2026-10-18 批次 G 落地注记（Tab 兼顾 + 提交即学别名）】**：起因是用户要求先读 Delphi 源码，看 Tab 能否"既选各项又兼其他功能"。读到的事实：Delphi 的 Tab 是纯导航（`actDown`/`actUp`，靠 `KeyPreview` 与 `TAction.SecondaryShortCuts`，到头回绕），↑↓ 在 `FormKeyDown` 里、行内那段 Up/Down 已被注释掉；Delphi 根本没有补全功能；它的"提交即学别名"是 `edtShortCutChange` 事后把查询词写进 `m_FavoriteList` 的别名列。用户随后追加要求：做成设置项，①补全优先 ②专职导航，默认①，代价可接受就照做——评估结论是可接受，故按②的口径实现为可配。落地：`tab_mode`（`complete`/`navigate`）进 `settings.json`，设置页「通用 → Tab 键行为」两个 `selectable_value` 选项；职责分流用**可观测状态**判定而不是时序（不采用"双击间隔"，理由是按需重绘下帧边界不稳定、且无法向用户解释）——原词是选中项名称或别名前缀则补全，否则循环。实测记录见需求书 FR-ACT-06/FR-ACT-10 的批次 G 注记。**三条实现坑记档**：① `consume_key(Modifiers::NONE, Tab)` 会连 Shift+Tab 一起吃掉（它用 `Modifiers::matches_logically` 比对），必须先收 `SHIFT+Tab` 再收 `Tab`；② egui 的 `EventFilter::matches` 对 `Key::Tab` 忽略修饰键，`Focus::begin_pass` 又跑在用户代码之前，所以 Tab/Shift+Tab 必然先把焦点跳走一帧，两个分支都得 `focus_query = true` 抢回；③ 别名学习会让 `keyword` 从单键变成多键，而 `MatchKeys` 原先只存 `alias: Option<Key>`，把 `"jsq qsj"` 当一个键算首字母会得出假分数——改成 `aliases: Vec<Key>`（`split_whitespace` 逐个建键），回归测试 `aliases_split_on_space_so_they_do_not_blend_initials` 钉住。`cargo test` 35 passed，本批新增 `can_complete_needs_name_or_alias_prefix`、`learn_alias_gates_then_appends`、`legacy_settings_without_tab_mode_load`、`commit_word_takes_first_word_and_strips_row_digit` 四条。**【2026-10-18 批次 H 落地注记（Tab 补全改甲案 + `Ctrl+J`/`Ctrl+K`）】**：批次 G 的「原词是选中项名称或别名前缀就补全」被用户复测推翻——`Tas` 出 7 条（Task Manager、Task Scheduler 都在内）时 Tab 补成 Task Manager，列表随即塌成 1 行，第二好的候选再也够不着。根因是判据只看选中项、没看兄弟候选，而列表本身是查询的派生值，补成整名必然塌列表。三案（甲＝无歧义才补／乙＝补到最长公共前缀，bash 口径／丙＝补全后在记住的候选集里循环，zsh menu-complete 口径即用户最初提法）里用户点甲：代价最小、无新状态、不违反本项已立的「分流只看可观测状态」判据（丙要引入按键历史）。落地＝`matching::unique_completion(查询, 清单, 结果集)` 数可补长候选，恰好 1 条才补、≥2 条交给导航；`Shift+Tab`、`navigate` 口径不动。同批补一对恒为导航的 `Ctrl+J`/`Ctrl+K`（用户原点名 `Ctrl+N`/`Ctrl+P`，与「新建」「置顶」冲突，上交后改选空闲键），并顺带纠正需求书附录 A 把 `Ctrl+P/N` 误记为已落地的账。实测（注入 + 回读 stderr + 截图）：`Tas` 连按 Tab 记「循环选中第 2 行（共 7 行）」「第 3 行」，`Ctrl+K` 记「第 2 行」；`task sch`（结果集只剩一条）记「查询补全为「Task Scheduler」（唯一可补全的候选）」。`cargo test` 36 passed，新增 `completion_waits_until_the_candidate_set_is_unambiguous`。两条遗留：补全改 `query` 后 TextEdit 光标停在旧长度处（补完接着打字会插进词中，截图 `Task Sch` 与 `eduler` 之间（光标停在第 8 个字符后））；另查出一条与 Tab 无关的启动路径缺陷（直接可见时不走 `on_show` ⇒ 输入法没切英文、~~焦点没给搜索框~~【2026-10-18 复核更正：焦点那半句作废，`new()` 里 `focus_query: intent.visible && intent.dialog.is_none() && !settings_open`（`src/main.rs:842`）在启动即可见时已为真；缺陷只剩输入法没切英文，症状是字母被中文输入法截走而 `Tab`/方向键仍直达应用】），已单独上报待裁定，实测对照见需求书 FR-ACT-06 批次 H 注记。**【2026-10-18 批次 I 收口：两条都按用户裁定修完并注入实测】**——光标那半：补全那一支同时写 TextEdit 状态（`TextEditState::load` → `cursor.set_char_range(Some(CCursorRange::one(CCursor::new(字符数))))` → `store`），搜索框 widget `Id` 提成 `search_field_id()` 让快捷键分支与 widget 共用，实测 `task sch` 补全后注入 `x` 得 `Task Schedulerx`（修复前 `Task Schxeduler`）；启动那半：`new()` 末尾在「首屏就是搜索页」时补调一次 `on_show()`，判据取 `focus_query` 以免抢走弹窗/设置页的焦点，冷启动日志直接出现 `IME→English: before open=false conv=0x1 → after open=false conv=0x0`，证明 `ImmGetDefaultIMEWnd` 在 `App::new` 阶段就可用、不必推迟到首帧。`cargo test` 36/36 不回归。【2026-10-18 批次 J：`space_to_run` 由 bool 扩成四档 off/word/unique/always，默认 `word`＝查询整串等于高亮行的名称、或等于它别名表里的某个词才执行（前缀阶段空格仍是文本，多词查询照打），`off`/`always` 原样保留旧 `false`/`true` 两个口径、旧 `settings.json` 照读；新增 `Shift+空格` 作"任何时候都插一个真空格"的逃生门，四档下都赢过执行判据。判据集中在纯函数 `matching::space_executes`，入参只有档位、查询、清单、结果集、选中行，不看按键历史；逐档真值表由单测 `space_executes_dispatches_on_observable_state_only` 覆盖，`cargo test` 39/39。**顺带查清一条对标事实**：Delphi 版根本没有空格执行——执行只绑 `FormKeyPress` 的 `#13`，`edtShortCutKeyDown` 全段 case 无 32，全仓搜 `VK_SPACE` 只命中一行被注释掉的 `PostKeyEx32(VK_SPACE, ...)`，数字键也要求带 Ctrl 或 Alt 才执行；故四档是本项目的自定口径，不是照抄参考项目（AHK 版未复核，参考目录在工作区外被权限层拦下）。】【2026-10-22 用户改键：`Ctrl+N`/`Ctrl+P` 从「新建」「置顶」改作**下一项/上一项**（与 `Ctrl+J/K` 同一支、回绕、恒导航），新建移到 `Ctrl+Shift+N`、置顶移到 `Ctrl+Shift+P`（用户从选项面板选定）；至此需求书里 Delphi 那条「`Ctrl+P/N` 移动」才真落地。同批查出并修掉一条顺序缺陷：`consume_key` 用 `matches_logically` 比对、**忽略多余的 Shift**，旧代码把 `Ctrl+D` 排在 `Ctrl+Shift+D` 之前 ⇒ 删除键自批次 F 起从未触发，现改为三条带 Shift 的整体先消费。添加快捷项/编辑快捷项弹窗同时放大并支持拖右下角缩放（默认 640×420、最小 460×300、上限＝主窗口 − 48px），尺寸只在本次运行内记住。详见发布说明第二十四节；`cargo test` 134/134、零警告，**`cargo build` 被用户运行中的实例锁住未成、界面未实测**。】【2026-10-22 同日晚些时候真人复测四条（`Ctrl+N`/`Ctrl+P` 回绕、`Ctrl+Shift+N` 文件行预填、`Ctrl+Shift+P` 置顶 Toast、`Ctrl+Shift+D` 删除确认）**全部通过**（正文「1. 通过 2. 通过 3. 通过 4. 通过」）；改动已随 `41021e5` 提交并推送。】
6. **真实启动**（FR-ACT-02）：`ShellExecuteExW` 替换 `std::process::Command`，按 `kind` 分派 file/folder/url/command；工作目录取目标所在目录；**移除 `.expect()`**，失败改为写日志 + UI 提示 + 不 panic。
7. **失焦自动隐藏与可见性控制**（FR-WIN-04/05）：焦点判定用 `ui.input(|i| i.focused)` + 延迟重绘，配置窗口打开时豁免。**隐藏与唤起的命令组合已实测定稿（2026-10-14）**：唤起 = `OuterPosition(回屏内)` + `WindowLevel(AlwaysOnTop)` + `Focus`，隐藏 = `OuterPosition(移出屏幕)`；**不用 `Visible(false)`**（会把唤起延迟抬高约 100ms 的重绘节流；118.1ms 那一侧实测成立，~~从 1.6ms 抬到~~ park 基线 1.6ms 已于 2026-10-15 判为恒真读数、作废，同日 20:41 由物理 × park 有效 7 轮补出真值 min/中位/max = 12.2 / 14.7 / 58.4 ms（见 4.7 与需求书附录 E-01 更正块））。~~`Focus` 的抢占成功率实测 20/20，无需 `AllowSetForegroundWindow`。~~ **【2026-10-15 更正 + 新增实现项】**那 20/20 里"移出屏幕"那半经复测判定为恒真数据（按下时前台本来就是自己，有效 0），所以 ~~`Focus` 在方案 B 下的抢占成功率**尚无数据**、`AllowSetForegroundWindow` 是否可省也**尚未证实**~~ **【同日 20:41 更新：物理 × park 的有效 7 轮里 `ViewportCommand::Focus` 单发、不借助任何辅助手段，7/7 抢到前台 ⇒ 方案 B 下"`Focus` 单独就够"已从待证升为~~有 7 轮支持；但样本未满 20 轮~~【同日 21:21 复跑：单轮 15/15，两轮合并 22/22；样本量口径同日由用户裁定为"累计 ≥20 个有效轮次、可跨多次运行合并" ⇒ 达标，E-16 记为已闭环】，`AllowSetForegroundWindow`/最小化-还原 是否可省仍未做对照，不能宣布已证实】**；并且实测发现 park 隐藏**不交还焦点**（注入 19/19 轮隐藏后前台仍是启动器自身；同日 20:41 的物理复测开局 2 轮再次当场复现）⇒ **本条的实现清单里必须补一步"隐藏时显式交还焦点"**，与 FR-WIN-04 的失焦隐藏合并设计（谁触发、交还给哪个 hwnd），手段选型待 E-15。~~**【2026-10-15 E-15 cloak 实测已答：cloak 同样不交还焦点（20/20 先验成立）⇒ 硬前置对 park 与 cloak 两种隐藏手段都成立，交还动作必须自己做，cloak 不自带交还效果】**~~**同日 20:41 又补出一条与本条直接相关的数字：方案 B 的唤起端到端有效 7 轮 min/中位/max = 12.2 / 14.7 / 58.4 ms ⇒ "~100ms 重绘节流平台"确实被消除（对比方案 A 的 117.5ms，约 8 倍），但 14.7ms 中位仍超出需求书 FR-WIN-02 写的"中位 ≤10ms"验收阈值。~~7 轮里有 4 轮落在 12–15ms，说明达标有空间~~【同日 21:21 复跑后按两轮合并的 22 个有效读数重算：中位 15.2 ms、min 12.2 ms、16 个落在 12.2–17.4 ms、0 个 ≤10 ms ⇒ "达标有空间"要改成"要么放宽阈值、要么把延迟分解后做优化"】，只是这 15 ms 量级（第一轮中位 14.7ms、两轮合并中位 15.2ms）目前还没分解成 `logic` 自轮询（8ms 一轮）、`SetForegroundWindow` 往返、移出屏幕后重新出帧三部分的贡献；~~阈值放宽还是列为 M1 优化项由用户裁决，我不擅自改验收标准~~。** **【同日裁定 (a)：FR-WIN-02 阈值放宽为"中位 ≤20ms 且不得出现 ~100ms 平台" ⇒ 冲突解除，22 个有效读数中位 15.2ms 达标；尾部 4 个（24.8 / 25.6 / 47.6 / 58.4 ms）如实记录但不作闸门，延迟分解不列 M1 硬项】**
8. **CJK 字体回退**（FR-SYS-08，**2026-10-15 已实现并实测**）：`CreationContext` 里取 `FontDefinitions::default()`，把运行时读自 `C:\Windows\Fonts` 的中文字体以 **fallback 追加到 `Proportional` / `Monospace` 各族末尾**（主字体不变 ⇒ Latin 字距不变）；候选 `msyh.ttc`(index 0) → `simhei.ttf` → `STXIHEI.TTF`，全部读不到则保持默认字体、不 panic。见 `src/main.rs` 的 `install_cjk_fallback`。**拆自原 FR-UIX-14**，理由见第二章阶段边界与需求书 8.4 第 6 条。

**验收标准**：从真实开始菜单加载 ≥100 条应用；输入 `jsb` 之外的英文查询可实时过滤且顺序稳定；键盘可完整导航并回车启动；点窗口外部 300ms 内自动隐藏；Esc 只隐藏不退出；启动不存在的路径不崩溃且有提示；占位符与结果标题的中文截图放大核对无 `.notdef` 方块（FR-SYS-08，SYS-08-1）。**图标此里程碑可缺省，但 `ResultItem.icon` 字段必须已存在。** **预估工作量**：5 人天（FR-SYS-08 已实现，不额外增加人天）



### M2：数据源全量（阶段一，10 人天）

**里程碑目标**：19 条 FR-SRC 全部到位，`→` 之前的一切"搜得出"能力成型。

**任务清单**：

1. **数据源框架**（FR-SRC-01）：`trait DataSource { id, init, search(&Query) -> Vec<ResultItem>, empty_results(), edit/delete hooks }` + 注册表（注册顺序 = 同分时的稳定次序）。**容错语义必须照抄 AHK 版：单个数据源 panic/报错只记日志并跳过，绝不中断整次搜索。**
2. **应用源**（FR-SRC-02/03/04）：~~扫 `Folders × FileTypes`（`*.lnk *.exe *.url *.appref-ms`）、深度与 `Exclude` 正则、`.lnk` 解引用（**选型已闭环：`windows` 0.62.2 的 `IShellLinkW` + `IPersistFile`，不引入 `lnk` crate**，详见 4.7 与需求书附录 E-03）、Store 应用（`Get-StartApps` → `shell:AppsFolder\<AppID>`）、`PATH` 目录枚举；索引缓存 `AppIndex.json` + 60min 刷新 + 手动重建。~~ **【2026-10-16 需求变更：改策展按需 —— 内置种子 + 用户清单（`app_index.json` 语义=用户清单，事件写入、无自动重扫）；`.lnk` 解引用选型与约束不变（`IShellLinkW`+`IPersistFile`，附录 E-03），作用域改「解析被导入的单条」；引入渠道 SendTo/添加对话框/无匹配预填/选中快捷键/可选批量导入见 FR-SRC-20，单实例+IPC 见 FR-SRC-21，条目管理见 FR-SRC-22，无匹配兜底运行见 FR-SRC-23；Store 应用 `Get-StartApps`、`PATH` 枚举与 60min 刷新只在「批量导入」档下相关。M1 已落地的扫描/缓存/图标代码复用为「批量导入 + 解析单条」，非丢弃】**M2 开工前置条件已满足，`.lnk` 4 条实测约束仍需在导入路径实现：`%VAR%` 展开、空图标/负数资源号、空 target 丢弃、按扩展名先过滤。**M1 已实现**：`src/source/application.rs`（IShellLinkW 解析 + ExpandEnvironmentStringsW + 空 target 丢弃 + 扩展名过滤 + `app_index.json` 缓存 + `ScanPolicy`）、`src/source/icon.rs`（FR-SRC-03 图标 186/186 实测）；**待实现**：`resolve_one(path)`（SendTo/添加用）、策展清单数据结构与种子、SendTo 引入、单实例+IPC、AddDialog、条目管理、无匹配兜底。

**【2026-10-19 批次 N 落地：批量导入的自排除** —— `spawn_batch_import` 的解析循环里，`.lnk` 的 target 经 `canonicalize` 后与 `current_exe()` 的规范化路径做大小写不敏感比较，相等即不收进清单。判据**不写死 exe 名** ⇒ 将来改名或搬家后基准自动跟着变（用户 2026-10-19 正文要求的"动态适配"）；任一侧规范化失败**不跳过**（宁多收一条，不误判成自己，与"拒删归属不明的文件"同一条取向）。起因：开机自启写的 `Programs\Startup\egui-launcher.lnk` 正落在默认扫描根 `Programs` 的深度 2 处，必被 `MAX_DEPTH = 3` 的递归扫到。只管新的批量导入，SendTo 预填与已入库的旧自条目刻意没动。判据用例 `self_exclusion_follows_the_running_exe_not_a_hardcoded_name`，取舍与注册表替代方案见发布说明第十二节。】
**【2026-10-19 批次 O 落地：改成窗口子系统，自启不再带出黑窗** —— 登录自启的真人复测通过（用户正文「1. 自启动测试通过」），但同一轮报出"启动后打开一个名称为 egui-launcher 的命令提示符窗口，这是什么原因？测试用的吗"。根因与自启本身无关、是打包层的：源码没有 `#![windows_subsystem]` 声明 ⇒ 链接器出**控制台子系统**的 exe，凡"没继承到控制台"的启动（登录自启、资源管理器双击）系统都替它新建一个窗刷那 87 条 `eprintln!`，而且关掉那个窗会把进程一起终止（实测：用户关窗后查不到进程）。用户在三档选项里选甲 ⇒ `src/main.rs:5` 声明窗口子系统（黑窗从根上消失），`src/main.rs:2612` 的 `redirect_stderr_if_orphan()` 在 `main()` 第一句把"没人听"的 stderr 接到 `launcher.log`（`src/source/application.rs:330`，与清单同目录、超 512 KiB 滚一代 `.old`），`Cargo.toml` 为 `GetStdHandle`/`SetStdHandle` 打开 `Win32_System_Console`（自 `7b3a7c5` 以来第一次动 feature 列表）。判据是**只在没人听时接管** ⇒ 终端与脚本跑法照旧实时刷日志，取证通道不断；丙案（改 `.lnk` 的窗口状态）被否是因为本机默认终端是 Windows Terminal，隐藏状态能否传导未证，而"根本不创建控制台"是确定的。机制先用独立探针在真机跑通两条分支再改码（顺带抓到 `STD_ERROR_HANDLE` 是 `-12` 不是 `2`）；验证：`cargo test` 46/46、`cargo check --all-targets` 零警告、PE 头 `Subsystem=2`（同一读法在 `where.exe` 上读出 3 作对照）、真机两种启动各跑一次（自启⇒日志落文件；外部重定向⇒文件一字节没涨）。**本条改了"日志去哪儿看"的口径**：自启与双击看 `launcher.log`，终端启动看窗口。】
3. **图标**（FR-SRC-03 / FR-SYS-02）：`SHGetFileInfoW`/`ExtractIconExW` → `HICON` → `ColorImage` → `TextureHandle`，**后台线程提取 + 路径→纹理 LRU 缓存（上限 1500）+ 启动后分批预读**，绝不在主线程做。
4. **自定义命令源**（FR-SRC-05/06）：`File\|Folder\|Command\|Url` 四类型 + `Keyword` + `{query}` 独占 + 路径四态校验 `OK / Not found / Offline / -`（`Offline` 必须是盘/网络位置不可达时不误报为"不存在"）。**【2026-10-21 批次 U 已落地，实现、偏差、真机清单与行号快照见发布说明第十九节】**两个新模块都是纯函数/后台侧，主线程只接产物：`src/source/custom.rs`（348 行：`{query}` 填充、关键字独占、重复关键字检查）与 `src/source/pathcheck.rs`（288 行：四态判据 + `spawn_scan` 后台整表检查）。四类型**没有新造**——沿用 `ItemKind` 与既有 AddDialog；模板填充**复用** `websearch::fill_template_with`，Url 编码、命令参数原样；环境变量先展开模板再填查询词。`main.rs` 那侧只多两类头段行（`HeadRow::Custom` / `CustomHint`）、一条每 3 秒的路径刷新、一条按 uid 把重复条目从结果里摘掉的过滤（`drop_hinted`，2026-10-21 真机发现同一命令既出提示行又占结果行后补的）、一条脱敏执行分支；同日第二轮真机反馈又补两条（`custom.rs` 因此 339 → 348 行）：提示行显示该条目的原始命令行（`CustomHint.command`），以及头段行的条目身份 `HeadRow::custom_uid` ⇒ 提示行/独占行的右键「编辑…」「删除」与 `Ctrl+E`、`Delete`、`Ctrl+Shift+D` 按 uid 回查原条目照常可用（置顶与打开目录仍不认头段行）。四条与本章/需求书原口径的差异，逐条写在明面上：① ~~**§4.2.1 的出厂内置命令集（12 条）没有写进种子**（本批刻意不动既有清单）⇒ FR-SRC-05 剩这一项~~【2026-10-21 批次 V 已落地：出厂集扩到 19 条（`builtin_seeds()`），老清单用 `SEED_ROUND` 一次性补齐、设置页可手动「补回缺失的出厂命令」；独占补了 `needs_query` 第二道门槛。见发布说明第二十节】；② **拼音只有首字母**（复用 `default_keyword`，无全拼字段），`file/folder` 的目标名与其拼音首字母键已追加；③ **路径检查是周期轮询而非文件系统通知** ⇒ 拔盘后最迟约 3 秒才显示 `Offline`；④ 本计划 4.6 节的分值表**仍然没有常量集中落地**（计算器、搜索行、自定义行三批都靠行模型直接占头段，不走打分），这笔账继续欠着。
5. **片段源**（FR-SRC-07/08）：占位符 `{date}`/`{time}`/`{datetime}`/`{clipboard}`/`{clipboard:n}`/`{uuid}`/`{cursor}`/`{date:fmt}`/`{date±N}`；粘贴回呼出前的窗口（保存 `ClipboardAll` → 写剪贴板 → 发送 `^v` → 延迟还原）；`;keyword` 全局自动展开是独立子系统（低层键盘钩子），**列 P2，允许在 M4 末尾降级**。
6. **剪贴板历史源**（FR-SRC-09）：`arboard 3.6` 监听；文本/文件/图片；上限 200、单条 10 万字；`IgnoreApps` 排除密码管理器；**本程序临时借用剪贴板时不得写入历史**。【2026-10-22 **用户裁定不做**（正文原话「剪贴板历史功能不做，只专注快速启动」）⇒ 本节整条作废、不排期；`arboard` 因此从未进入 `Cargo.toml`，本程序仍只有写剪贴板、不读剪贴板】
7. **计算器源**（FR-SRC-10）：表达式（`evalexpr 13` 或 `meval`，需支持 `^`、括号、负数、千分位输出）+ 单位换算 + 进制 + 日期加减；`Enter` 复制**纯数字**，单位写副标题。结构计算与货币换算**已排除**。【2026-10-20 批次 S 已落地，实现与真机 14 项实测见发布说明第十七节。三处与本章原口径的差异，逐条写在明面上：① 选型换了——没有引入 `evalexpr`/`meval`，`src/source/calc.rs` 是自写的递归下降（1020 行、零新 crate）。理由不是"想自己写"：这两个 crate 都只给数值表达式，而本条的**单位换算 / 进制 / 日期加减**三者都要自己写，剩下那半条括号与优先级不值得为它新增一个依赖；顺带把"千分位只进展示、不进复制"这条口径做成了 `fmt_num(v, sep)` 的一个布尔参数。② 本计划 4.6 节那张分值表（计算器 `200` 恒在顶）**没有以常量集中的形式落地**——计算器行是靠行模型 `enum Row { Calc, Item }` 直接占第 0 行，不走打分。那张表要求"可被测试锁定"，目前只有行模型的三条等价性单测，**分值集中表这件事仍然欠着**，别当已完成。③ 本条没做数据源开关（FR-CFG-04 那页还不存在），计算器恒开 ⇒ 登记为偏差。另给 FR-SRC-09 留一句前置：本批写剪贴板走的是 `OpenClipboard`+`SetClipboardData(CF_UNICODETEXT)`（`Cargo.toml` 为此新增 `Win32_System_DataExchange` 与 `Win32_System_Memory` 两条 feature，**不是原计划的一条**），做剪贴板历史时若用 `arboard` 监听，会照常收到这次复制——计算器这次**是用户主动要复制**，不该被"临时借用"那条排除规则吃掉。】**待实现**：单位表目前只到八维（多出计划里没列的**时间**与**数据**），且没有 `pi`/`e` 这类常量 ⇒ 直接后果是"计算器行 + 条目行"的混合列表在真机上构造不出来，那条分支只有单测覆盖。
8. **网页搜索源**（FR-SRC-11）：引擎表 + `{query}` URL 编码 + `{query|默认值}` + 空词只出补全行 + 无结果兜底序列。【2026-10-21 批次 T 已落地，实现、口径与待跑的真机清单见发布说明第十八节。三处与本条原口径的差异：① **引擎表目前只能手改 `settings.json`**（设置页没有引擎编辑 UI，`icon` 字段存而不显示）——"内置 9 个 + 兜底序列"已齐，但需求书那句"引擎图标可指本地文件或内置、下载站点图标仅在该次点击访问该站"只落了字段、没落展示；② **独占只认「首词 + 空格」**（`g ` 独占、`g` 仍是普通搜索），与 FR-QRY-01「关键字后无空格 ⇒ 普通搜索」同口径——两条原文在这里的接缝是"空词补全行只在空格之后出现"；③ 本计划 4.6 节的分值表**仍然没有常量集中落地**（计算器那格欠着的账没还上，本批搜索行同样靠行模型直接排在应用行之前，不走打分）。】
9. **书签源**（FR-SRC-12）：Chrome/Edge/Brave/Vivaldi 的全 profile `Bookmarks` JSON；Firefox 明确不支持（与参考项目一致）。
10. **窗口切换源**（FR-SRC-13）：`EnumWindows` 枚举可见顶层窗口，跳过自身与工具窗；激活前最小化先还原。【2026-10-21 批次 W 已落地，实现、口径与待跑的真机清单见发布说明第二十一节。三处与本条原口径的差异：① **枚举过滤比原文多三层**（DWM `DWMWA_CLOAKED`、零尺寸、空标题）——原文只说"可见顶层窗口"，UWP 挂起窗口在 `IsWindowVisible` 眼里仍是可见的，只能靠 DWM 那一位排掉；② **默认结果里的窗口按 −14 偏移与应用行交错**，不是"追加在应用结果之后"——计划 4.6 的分值表仍是常量未集中（这笔旧账与计算器/搜索行同一处）；③ **`→` 落在独立的关闭确认框上**，不是操作面板（本程序至今没有 FR-ACT-04 的操作面板），确定后只发 `WM_CLOSE`。**本批零新依赖**：所需 API 全在既有 `windows` feature 面内。】【2026-10-21 同日按用户裁定**整项撤除**：用户正文「w 切换窗口目前感觉是鸡肋，还不如直接按Alt+Tab好用，是不是考虑删除该功能？」⇒ 从选项面板拍板整套撤除。代码反向应用到批次 V（与 `30498b0` 逐字节一致），`src/source/window.rs` 移出仓库、与原补丁一起存 `target/removed-batch-w/`；`cargo test` 回到 **116/116**、`cargo check --all-targets` 零警告。**本项状态＝撤除，不排期**；上文那段实现记录保留为决策快照】
11. **文件搜索源**（FR-SRC-14）：**内置索引**（`ScopeFolders` + `Depth` + `ScopeExclude` + `MaxEntries` + 周期刷新）；路径浏览 `BrowsePath`、`'` 前缀、`open|find|file` 关键字、`folder`、类型词 `doc|pic|video|audio|zip|exe|cad`。**不实现 Everything IPC。**
12. **脚本 / 终端 / 系统 / 速查 / 置顶最近源**（FR-SRC-15…19）：脚本目录注释元数据 `@altrun.title/keyword/argument/mode`；终端 `>` 前缀 + Shell 选择；系统命令按**分组**交付（电源、音量与媒体、Windows 工具、设置页 `ms-settings:`、自身命令）；`?` 速查表；空输入框 = 置顶 + 最近。
13. **拼音与学习排序**（FR-QRY-04/06）：`pinyin 0.11` 取首字母（注意 AHK 版用的是 GBK 区间表，存在多音字取舍，需与 crate 结果做对照）；`Boost = 8·ln(1+count)` + 近 7 天 +5 + 该输入或其前缀选过它 ≤+60，**应用侧只把 Boost 加在排名上，避免高频应用挤扁新查询**。【2026-10-18 批次 K 落地注记：这一条已按「只加在排名上」实现为设置页两档，默认甲。`matching::learns_before` 在档位闸门之后比较：甲（`count`）＝次数 → 最近一次；乙（`decay`）＝`log2(1+use_count) × 0.5^(天数/7)` → 最近一次，半衰期常量 `HEAT_HALF_LIFE_DAYS`。与参考式子的三条对照：① `8·ln(1+count)` 与 `log2(1+count)` 在"只用来比大小、不参与加和"的前提下是同序的（差一个常数因子），所以我们取更直观的 `log2`，次数 1→0、3→2、8→3.17；② 参考式子的时间项是"近 7 天 +5"的**台阶加分**，加在次数旁边 ⇒ 一个用了 50 次的项（8·ln51≈31）永远带不动，正是用户这次报的"赖在前面"反过来那半边；我们改成**乘以**半衰期，让老项的整体热度随时间塌下去，这是有意偏离、不是抄错；③ 参考式子第三项"该输入或其前缀选过它 ≤+60"要按查询串存一张历史表，本批未实现——我们已有的"提交即学别名"是它的更强替代：学成别名后下次直接跳到更高档位，而不是同档位内加分。整体不采用「`match_score × 1000 + boost`」的加和写法（早前讨论过的那版公式），因为档位间隔只有 5～70，加和会让次数反过来撬动匹配质量，与 FR-QRY-07 裁定 a 冲突。两档都只在同档位内生效、置顶仍是硬第一键，见需求书 FR-SRC-19 批次 K 注记。】

**验收标准**：14 类数据源全部可被搜到并按分值稳定排序；任一数据源注入异常时其余照常返回；`Enter` 对 file/folder/url/text/command 五类 `kind` 语义正确；拼音首字母、`{query}`、片段占位符、剪贴板粘回、计算器复制、文件类型词逐项通过需求书第 10 章用例。 **预估工作量**：10 人天



### M3：操作面板与参数体系（阶段一，5 人天）

**里程碑目标**：从"搜索结果"升级到"对结果做事"，并补齐 Delphi 版独有的参数机制。

**任务清单**：

1. **操作面板**（FR-ACT-03/04）：`→`（仅"新按"，需过滤键盘自重复）/右键进入 actions 模式；`ActionCatalog` 提供 16+ 动作——打开、以管理员运行（`*runas`）、打开方式（`rundll32 shell32.dll,OpenAs_RunDLL`）、所在位置（`explorer /select,`）、复制路径/名称、在终端打开、属性（`SHObjectProperties`）、移入回收站（`IShellFolder`/`InvokeVerb("delete")`）、大字显示、编辑/删除、置顶/取消置顶、复制、粘贴、全部打开；`Esc`/`←`/`Backspace` 返回结果且不丢已输入文字。
2. **多标记**（FR-ACT-05）：`Insert` 标记 file/folder 行；批量动作 OpenAll / CopyFiles / CutFiles / CopyPaths / CopyTo / MoveTo / 全部加入命令 / 回收站。文件剪贴板写入需 `DropEffect` 语义才能被资源管理器识别为剪切。**【2026-10-26 批次 E4 已实现**：八条里做了七条（`MoveTo` **不做**——「复制到」已覆盖"搬去另一个目录"，QuickSwitch 那套"最近两个目录"语义在本程序里没有对应物）；`DropEffect` 那半句的落点是 `Preferred DropEffect` = 1 复制 / 2 剪切，真机回读两种都验过。细节见需求书 FR-ACT-05 行内注记与发布说明第四十八节**】**
3. **Tab 自动补全 / 进入文件夹**（FR-ACT-06）：文件夹结果且匹配盘符或 UNC 时补成 `路径\`。
4. **参数标记与参数输入**（FR-ACT-07，Delphi 版独有）：`{%p}` `{%c}` `{%wd}` `{%wt}` `{%wc}`；`ParamType` = 无编码 / URL 查询 / UTF8 查询；呼出时抓取剪贴板与前台窗口句柄/标题/类名；弹参数输入框（可编辑下拉 + 历史 LRU 50）；带参项在行上必须可见标记（Delphi 用 `*%d|` 前缀）。
5. **运行标志**（FR-ACT-08，Delphi 版独有）：命令行尾 `@+` 最大化 / `@-` 最小化 / `@` 隐藏。
6. **快捷执行与学习写回**（FR-ACT-01）：`Ctrl+1..9` 仅作用于可见行；执行前依次 `Knowledge.Record(输入, uid)` → 最近使用记录 → 使用统计计数，再隐藏窗口，再执行；异常兜住不崩。

**验收标准**：需求书 FR-ACT 全条通过；`{%c}` 在跨窗口唤起链路上取到的是**呼出前**那个窗口的剪贴板/标题；操作面板返回后输入框文字与选中行复原。 **预估工作量**：5 人天



### M4：配置、持久化与系统集成（阶段一收口，8 人天）

**里程碑目标**：阶段一出口——功能全量、数据可存、系统集成齐备，界面允许难看。

**任务清单**：

1. **配置模型与持久化**（FR-CFG-01/02/03/08）：JSON + `SchemaVersion` + 逐版迁移 + 缺失项补默认（Map 递归合并、数组整体替换、类型不符取默认）；解析失败改名 `.bad` 并用默认重建、`Restore Defaults` 前写 `.bak`；数据目录 `exe\Data` 可写则便携、不可写退化 `%APPDATA%`、`DataLocation` 支持 `%OneDrive%` 类变量且**不递归解析**；"恢复默认"保留用户数据项。
2. **偏好设置**（FR-CFG-04/05）：页面按功能域组织（通用 / 窗口 / 外观 / 数据源开关 / 应用 / 文件搜索 / 命令 / 片段 / 剪贴板 / 网页搜索 / 计算器 / 脚本 / 热键 / 高级）。**阶段一用默认控件 + 左侧列表切换即可**，两列表单与图标导航属阶段二。改动先落深拷贝草稿、确定/应用才生效并重载。快捷键录制框：显示 `Alt+Space` 风格、内部存规范串，单字母/数字/空格必须带修饰键、`F1–F24`/`Pause` 可单独、全局重复检测、`~` 保留原义复选框。
3. **热键与唤起**（FR-WIN-02/03/06/07/08）：主/次热键注册与重注册（先注销后注册，失败降级托盘唤起并提示）、双击 Ctrl/Shift 呼出、按键抢跑（拿到焦点前输入的字符不丢）、单实例（命名互斥体 + `WM_COPYDATA` 交接，无应答才强杀）、CLI 协议（`-Startup / -SendTo <路径> / -Preferences <页> / -Reloaded`）。**【2026-10-17 状态：FR-WIN-07/08 已落地。互斥体 `Local\XiShan.EguiLauncher.SingleInstance`（探测次实例立刻关句柄）；转发 2s 超时 + 消息窗未建时 20×50ms 重试；无应答 ⇒ 按消息窗类名取 PID → `TerminateProcess` + 等 2s → 重抢互斥体接管（挂起旧实例实测接管成功）；找不到消息窗不猜 PID（降级并记日志，`-Startup` 直接退出）。CLI 解析在 `src/cli.rs`（7 单测）：`-SendTo` 单路径预填弹窗、多路径批量 uid 去重（实测新增 2 / 重复跳过 2）、`-Preferences` 1/2/3 页码映射（参考协议的 8=命令页无对应页 ⇒ 落回通用，偏差已登记）、`-Startup` 静默（eframe 首帧强制显示，用首帧末 `Visible(false)` 盖回，实测保持隐藏）、`-Reloaded` 等互斥体 2s。呼出统一 `ShowWindow(SW_RESTORE)` 兜底：winit 可见标志与实际窗口脱节时单发 `Visible(true)` 不生效。】**【2026-10-19 批次 Q 落地：本任务清单里 FR-WIN-02 的"可配"那一格补上，FR-WIN-03/06 仍未碰。用户在三个候选批次里点甲（热键可配置），三次裁定"冲突本地表 + 试注册都判 / 立即生效、失败回滚旧热键 / 撞键允许尝试、失败照实报错"。① 配置层：`settings.json` 新增 `toggle_hotkey`，存 Windows 原始数值对 `HotkeySpec { mods: u32, vk: u32 }`（`src/source/application.rs:254`）⇒ **不给配置层引入第三种字符串表示**，"存的东西 = 注册的东西"；展示串由 `display()` 现算、与启动日志共用同一份实现，键名口径英文（`Space`/`Enter`），因为 `hotkey: Ctrl+Alt+Space registered` 这行从批次 A 起就是复测判据原文。出厂值 = 写死的那组数并被单测钉住（改它等于偷换用户认识的键）；`#[serde(default)]` 兜住旧文件。② 换键落点：注册/注销只在热键线程（绑 hwnd），而该线程阻塞在 `GetMessageW` ⇒ **UI 写 Mutex 永远醒不来，必须 `PostMessageW(hwnd, WM_APP+1)`**；结果经回执槽 + `ctx.request_repaint()` 回 UI（窗口隐藏时 eframe 不跑 `ui()`）。**只有 `Applied` 才写 `settings.json`**，`Rejected`/`StartupFallback` 不动 prefs。③ 顺带修掉一条老缺陷：`spawn_hotkey_thread` 原先注册失败就 `return`，丢掉整个线程连同 message-only 窗 ⇒ **SendTo 与单实例交接跟着废**（热键写死时这条 `return` 看着无害）；现在退回出厂值，出厂值也失败则保留消息窗、只是没有全局键，托盘照常用。④ FR-CFG-05 侧：设置页录制卡片（录制新热键 / Esc・Tab 取消 / Backspace・Delete 恢复默认）+ 纯函数 `check()` 挡"裸 A 当全局键"；与原文的有意偏差（无修饰键勾选框、不收 `Pause` 与鼠标键、无 `~` 前缀、无第二热键可禁用、"录制期间挂起自身热键"只挂到 UI 层快捷键块）逐条登记在需求书 FR-CFG-05 那一格里。**本批没动 `Cargo.toml`**（`RegisterHotKey`/`PostMessageW`/`GetLastError` 全在现有 feature 面上），也不引入依赖。验证：`cargo test` ~~**58/58**~~【2026-10-19 复测轮】**59/59**（批次 Q 新增单测 ~~9 条~~ **10 条**全走纯函数——`RegisterHotKey` 那半边只有真机能答，不塞进单测冒充已证）、`cargo check --all-targets` 零警告；~~**真机未跑**（实例锁着 exe，等用户自己关闭后重建），七条复测判据见发布说明第十五节。~~【2026-10-19 复测轮：**真机跑过一轮**——"换键立刻生效"通过（用户正文「热键修改有效」），同一轮抓到第一条缺陷并当场修掉：**录 `Ctrl+I` 得到 `Ctrl+Win+I`**，根因是把 egui 的 `command` 位当 Win 键，而它在 Windows 上是 `ctrl` 的副本（`egui-winit-0.36.2/src/lib.rs:496-500` 与 `egui-0.36.2/src/data/input/modifiers.rs:33` 两条同证）⇒ 录制从此只认 `ctrl/alt/shift`，**Win 组合键不可录**（偏差已登记进需求书 FR-CFG-05）。**计划层要记的一条方法论**：这一格当时的验证判据写的是"单测全绿"，而唯一覆盖修饰键映射的那条测试是按我错误的字段语义写的（拿 `command: win` 造输入、断言含 `HKF_WIN`）⇒ **绿检 ≠ 对测**；今后凡"第三方字段语义"类断言，先去读上游映射代码，测试输入照上游形态构造、不照我的理解构造，并补一条"任何输入形态都产不出这一位"的穷举测试兜住语义再漂。八条复测判据见发布说明第十五节。】【2026-10-20 复测第二轮（批次 P + Q 已提交并推送 `9ed8892`；这一轮真机实测由我自己注入按键完成，不再占用户的手）：八条里 **①（隐藏半边）②（生效 + 落盘半边）④⑥⑧ 通过**，**③ 改由启动分支实测通过**——手改 `settings.json` 为 `{"mods":8,"vk":76}`（`Win+L`）后启动，日志 `hotkey: Win+L 注册失败（1409=已被占用），本次退回 Ctrl+Alt+Space；settings.json 不改动`，回读文件确认一个字节都没被覆盖，且消息窗照样建 ⇒ 回滚 / 不落盘 / 不牵连 SendTo 三条一起成立；**⑤ 只过 `Esc` 与 `Backspace`**（`Tab`、`Delete` 各是同一分支的另一半，没跑），**⑦（中文输入法下录制）仍未跑，只能真人**。**又抓到并修掉第二条真机缺陷**：本地判据（`check()` 失败 / 撞本程序键表 / 与当前一致 / 线程未就绪）拒绝后录制态已经退出，可 Toast 写的是「换一个」⇒ 用户直接按下一个键被当普通按键丢掉；修法＝`request_hotkey()` 返回 bool，只有真正投递出去才退出录制，判据用真机 A/B（修复前同一序列没有投递行、修复后有）。**计划层要记的第二条方法论（比"绿检 ≠ 对测"更狠）**：我一度从"`Ctrl+F12` 按了没反应"倒推出"和弦的 Ctrl 按下与主键按下落在同一帧 ⇒ 主键被 `retain` 清走"这个**根本不存在的缺陷**，并把 `is_modifier_key` + `first_recordable_press` + 一条单测全写完（60/60、零警告）**之后**才去跑判别实验——同一份构建上 `Ctrl+F13` 一次落定 ⇒ 假说被自己的实验否证，真因是 Windows 把被别的全局热键占住的组合整条吞掉、窗口一个键事件都收不到；改动**全部回滚**，只在函数注释里留三行"实测过再来动这里"。⇒ 判据升级：**动手前先写下"哪个实验能区分我的假说和竞争解释"，并且先查现有反证**（本轮反证其实早就在手上：用户正文说过「热键修改有效」，而 `Ctrl+I` 那次也正常落定过）。台账、限制与全部原文见发布说明第十五节。】】【2026-10-20 批次 R 落地（用户在批次 Q 复测通过的同一条正文里追加需求，原话「再加一层需求: 我需要按单键触发,比如重音号 `` "`" ``,即不需要辅助键也能作为快捷键」+「Delphi版的AltRun是可以这样使用单键触发的」+「对于重音符号 `` ` ``,目前win+ `` ` `` 这个组合无法设置;当设置了 `` ` `` 作为快捷键之后,可以通过win+ `` ` `` 或者输入~来替换为 `` ` `` 是否可以」；五轮裁定：补字符键要做且可配 / 裸键只放开 `` ` `` 这一个 / 取键期间真注销且退出一定注册回来 / 一次做完不拆批 / 控件形态照 Delphi「一个框输入字符 + 一个框选 Win・Ctrl・Alt・Shift」并附配置→热键 页截图）：**这一步把 FR-WIN-02 的"可配"从一格推成两格，也把 FR-CFG-05 的"修饰键勾选"第一次做实。**① 配置层新增 `AppSettings::typeback_hotkey: Option<HotkeySpec>`（`None` = 不启用，`#[serde(default)]` 兜住旧文件）；注册侧同一个 message-only 窗、另一个 id `HOTKEY_TYPEBACK = 2`，`WM_HOTKEY` 按 `wparam` 分派 ⇒ 原文那句"全局主/次热键"从此真有两格，**但第二格的语义是我们自己的**：Delphi 的辅热键（`frmALTRun.pas:1207` 的 `ApplyHotKey2`）仍是第二个呼出键，我们的补字符键不呼出窗口、只把字符投给此刻的焦点窗——**借它的控件形态，不借它的语义**。② 判据：`check()` 从"非 F 键必须带 Ctrl/Alt/Win"变成「不带修饰 ⇒ 只允许 `` ` ``；单带 Shift ⇒ 一律挡，因为 `Shift+` 打出来就是 `~`，挂成全局键等于连波浪号一起吞掉」，另加 `check_typeback()` 两条只有这个槽才有的规矩（必须带修饰、不能与呼出键同组），两条全走纯函数。可行性不是猜的：Delphi `HotKeyManager.pas:156-181` 就是拿 `Modifiers := 0` 去注册，它的辅热键默认值还是裸 `Pause`（`untALTRunOption.pas:747`）。③ 补哪个字符由 `MapVirtualKeyW(呼出键 vk, MAPVK_VK_TO_CHAR)` 现查 ⇒ 跟随键盘布局、不写死；返回 `0` 与带 `0x8000` 死键位都判为反查不到。注入走 `SendInput` + `KEYEVENTF_UNICODE`（`wVk = 0`、把字符放扫描码），**选它而不是按 vk 注入，是因为热键匹配认 vk——按 vk 注入会被我们自己那条裸键注册再吞一次**；这条至今是**未证的设计假设**，记事本 / Edge / VS Code / Windows Terminal / 中文输入法五处一条没测。④ 裁定③的落点：`WM_APP_SUSPEND_HOTKEY` / `WM_APP_RESUME_HOTKEY` / `WM_APP_CLEAR_TYPEBACK` 三条消息 + `SetTimer` 30 秒兜底（每次挂起重新上弦）⇒ **这条推翻了批次 Q 登记在 FR-CFG-05 的那句"录制期间挂起自身热键只挂到 UI 层"**（原文划掉保留），理由很简单：不注销就录不进裸键，系统把命中它的键在最上层整条吞掉。**兜底为什么是定时器而不是"把取消链写得更严"**：注册线程阻塞在 `GetMessageW`，`Mutex` 唤不醒它，任何一条 UI 收尾路径漏走就只能靠时间把键装回去——同一条理由决定了 `on_hide` 必须"先退取键态、再 `return`"。⑤ 控件形态的取舍：主热键保留批次 Q 已过真机的录制入口不动，"框 + 四个复选框"只做在补字符卡片上 ⇒ 同一个设置页出现两种按键编辑形状，要不要统一**待拍板**；连带代价是 Win 位只有这张卡片能设（egui 不传 Win 修饰位那条限制本身没变）。**必须写进验收的代价**：呼出键设成裸 `` ` `` 之后全系统打不出 `` ` ``（本机实测 `Shift+` `` ` `` 仍能打出 `~`，即裸注册只吞不带修饰那一下），出口就是 ② 那一格。**一条本地新坑**：同一线程里同 `vk` + 修饰位只能注册一次 ⇒ 主热键录成"当前补字符热键那一组"必须先在本地预检，否则 `RegisterHotKey` 返回 1409、提示写成"已被系统或其它程序占用"，而占用者其实是我们自己。**本批没动 `Cargo.toml`、零新依赖**：`MapVirtualKeyW` / `SendInput` / `SetTimer` / `KillTimer` 全在现有 feature 面上。验证：`cargo test` **61/61**（批次 R 新增 2 条纯函数单测，`hotkey_tests` 6 → 8）、`cargo check --all-targets` 零警告；**真机一条未跑**（实例 PID 28528 锁着 exe，且九条清单里 ``Win+` `` 与中文输入法开启态这两条只能用户手按——我这边的注入路径被硬否决、另一条绕不过 IME），判据与台账见发布说明第十六节。】
4. **系统集成**（FR-WIN-11 / FR-SYS-01 / FR-WIN-09/10）：托盘与菜单（图标内嵌、`TaskbarCreated` 重建、左键单击呼出）；开机自启 / SendTo / 开始菜单快捷方式（**只删自己创建的**，写前先比对内容避免无谓重写）；窗口落点 `Mouse|Primary|Active` 与记住位置（存工作区千分比而非绝对坐标）；输入法处理（可选切英文键盘或 IME 英文模式、隐藏后还原）。
5. **快捷项批量管理**（FR-CFG-06/07，Delphi 版独有）：列表式增删改排序、**绝对↔相对路径批量转换**（exe 目录内转 `.\`）、**失效项校验并批量删除**（校验前做环境变量与 `.\` 展开，网络路径跳过不误判）；收藏/置顶持久化。
6. **多语言与日志**（FR-SYS-04/03）：`locales/{zh-CN,en}.json` + `_name/_font/_fallback` 元键 + `auto` 跟随系统 + 单测断言键集完整与占位符数量一致；`tracing` 文件+控制台，级别可配，**明确不记录用户输入内容**。 **【2026-10-19 批次 P 落地：日志收口两格，多语言那一半没动** —— 用户在三个候选批次里点 A（日志收口），再裁定「级别过滤本批不做、托盘复制系统信息本批不做」⇒ 本批只收 FR-SYS-03 空着的两格：① 「打开日志」入口＝~~托盘菜单项 + `Ctrl+Shift+L`（同一个 `open_log_file()`；`Ctrl+L` 特意留给 FR-ACT 的「大字显示」；键位放在 `settings_open` 判断之外，与 `Ctrl+=` 缩放同口径——看设置页时最想翻日志）~~托盘菜单项一项（2026-10-19 复测当日用户正文「『打开日志』不必要弄一个快捷键，将来有其他需求替换掉就可以」⇒ **快捷键撤除**，`Ctrl+L` 仍不占；这条成为「每个功能必须配快捷键」立规的第二例外，第一例外是批次 K 的"设置页开关类不另配键"）；② 按日期轮转 + 总量上限＝跨天或超 512 KiB 把 `launcher.log` **改名**归档为 `launcher-<内容日期>.log`，只留最近 3 份（`LOG_ROTATE_BYTES` / `LOG_KEEP_ARCHIVES` 在 `src/source/application.rs`）。**轮转的日期不能问 mtime**：`FileTimeToSystemTime` 在 windows 0.62.2 现有特性面下三条路径探针全部 E0425，手搓公历换算不是这一格该背的复杂度 ⇒ 改成让日志**自己声明属于哪天**——接管 stderr 时首行写日期锚 `# log 日期: YYYY-MM-DD HH:MM`，归档读锚取名，锚解析失败归 `launcher-无日期.log`，绝不猜。拿本地历日要 `GetLocalTime` ⇒ `Cargo.toml` 新增 `Win32_System_SystemInformation`，**这是自 `7b3a7c5` 以来第二次动 feature 列表**（第一次是批次 O 的 `Win32_System_Console`）。**删文件那道刹车**：归档认领只认**完整名字形状**（`launcher-<ISO 日期>.log` + 可选纯数字后缀，外加 `launcher-无日期.log`）；第一版写成 `starts_with("launcher-") && ends_with(".log")`，写单测时当场发现它会删掉用户同目录的 `launcher-notes.log` ⇒ 收紧，与批次 L「归属不明一律不认领」、批次 N「宁多收一条也不误判」同一条取向。**顺带按「发现了就顺便修改一下」推翻一条历史误判**：批次 O 注记写过"FR-SYS-03 的隐私验收当前成立"，全仓扫 `eprintln!`（批次 P 当时 `src/` 下 92 处，批次 Q 复测已是 105 处 ⇒ 这类计数别当常量引用，以实时 grep 为准）后发现两处会把用户侧内容写进日志——「提交即学别名」打印查询框首词、`FR-SRC-21: 收到转发路径` 打印转发来的路径 ⇒ 本批已脱敏（前者只记条目名 + keyword，后者与 `AddBatch` 分支统一只记条数）。留下的口径：日志记「动作 + 清单内条目的 name/uid/keyword」，不记「查询框原文」；仍带路径的两条是失败诊断（`.lnk` 解析失败、`Ctrl+D` 无可定位文件）。验证：`cargo test` **49/49**（新增日期锚轮转、归档裁剪不碰外来名、时间戳格式三条，全在临时目录跑）、`cargo check --all-targets` 零警告、真机三条启动分支 + 「实例握着日志句柄时改名依然成功」（std 默认带 `FILE_SHARE_DELETE`）⇒ 跨天归档不会被活着的旧实例挡住。`tracing`、级别过滤、性能日志统一前缀（NFR-01）、多语言仍然没做，判据与复测项见发布说明第十四节。】
7. **线程与并发模型**（FR-SYS-05）：见 4.7 节。所有扫描/索引/图标/剪贴板/COM 均在后台线程，主线程只消费 `mpsc`。

**验收标准**：重启后配置、收藏、置顶、学习排序、剪贴板历史全部保持；手改配置成非法 JSON 后能自动改名 `.bad` 并以默认值启动；两个实例并跑时参数会转发给先起的实例；`Alt+Space`/`Alt+R` 均可唤起，改热键即时生效、被占用时给出提示不崩溃；托盘菜单四项齐全；阶段一 66 条 FR 全部可勾验收（含 FR-SYS-08「CJK 可渲染」：截图放大核对中文无方块）。 **预估工作量**：8 人天



### M5：视觉系统（阶段二，6 人天）

**里程碑目标**：把"能用"变成"像一款启动器"。**本里程碑不改任何功能语义。**

**任务清单**：

1. **主题系统与 `Base` 继承**（FR-UIX-01/02）：内置主题集（含 Light/Dark/Compact/Modern 若干，数量在实现时定），全部建立在 Light 之上，主题文件全丢也能运行；24 个主题键（`FontName/InputFontSize/TitleFontSize/SubtitleFontSize/ShortcutFontSize/Padding/RowHeight/IconSize/SelectedRadius/Opacity/Background/Border/InputText/Separator/Title/Subtitle/Shortcut/SelectedBackground/SelectedTitle/SelectedSubtitle/SelectedShortcut/Highlight/SelectedHighlight` + 继承用的 `Base`）；颜色 `RRGGBB` 无 `#`、字号 pt、尺寸按 96 DPI 定义并随缩放换算；`IsDark()` 亮度判据；跟随系统明暗并监听 `WM_SETTINGCHANGE("ImmersiveColorSet")`。
2. **自绘结果行**（FR-UIX-03/04/05）：图标 + 标题（匹配段高亮）+ 副标题（路径用中间省略、其余尾部省略）+ 右侧快捷键提示；置顶角标；标记行左缘竖线；选中态按 `SelectedRadius` 画圆角否则整行平涂。**高亮必须按分段一次性测量**（参考项目正是为解决"有高亮/无高亮差 1px 导致字距跳动"才自绘）。
3. **窗口外观**（FR-UIX-06/07/13/14）：无边框 + 圆角 + 边框色 + 透明度；`DWMWA_CLOAK` 防首帧白闪；窗口高随结果行数伸缩（行数不变不动尺寸，滚动不出现滚动条）；淡入淡出（走影子窗口，真窗口立即隐藏）；高 DPI、主题字体与字体宽度预热（**基础 CJK 可渲染已在阶段一 FR-SYS-08 交付，此处只做主题化与预热**）。**⚠ 开工前置条件（2026-10-15 实测钉死）：先把 `src/main.rs` 的候选后端从 `Backends::DX12 | Backends::GL` 改成只留 `Backends::GL`，再恢复 `with_transparent(true)` —— DX12 下逐像素透明在 DXGI 层面就不可能（实测：半透明红呈现成暗红、四角残留黑），补 `WS_EX_LAYERED` 也救不回；GL 下实测可干净透出桌面且不需要补任何窗口样式。改完必须重新验证显隐黑闪与 `DWMWA_CLOAK` 共存（E-15/E-07）。**~~【2026-10-15 E-15 cloak 生效性已在 `DX12|GL` 下实测（属性层：`DWMWA_CLOAKED=1` 而 `IsWindowVisible=true`，露出中位 9.3ms 不付 100ms 节流），但 GL 后端 + 截图回读那一路仍未做；阶段二切到只留 `Backends::GL` 后必须复测】~~
4. **空态与结果版式**（FR-UIX-13）：占位符、状态栏提示回填、无结果视觉。

**验收标准**：切主题即时生效且缩略图与实窗一致；192 DPI 下无模糊/无错位；淡入淡出不抢焦点；结果行三行信息在 700px 宽度内不互相挤压。**注意**：本里程碑的量化标准（行高/字号/留白）在需求书 FR-UIX-03 中以参考项目实测值给出，实现时逐键对照。 **预估工作量**：6 人天



### M6：反馈层、统计与发布（阶段二收口，5 人天）

**任务清单**：

1. **状态栏与 ☰ 菜单**（FR-UIX-08）：左侧返回/模式/轮换提示，右侧 `Enter` 行为与 `→` 键帽提示；☰ = 偏好设置 / 速查表 / 重建索引 / 检查更新 / 重载 / 退出。**【2026-10-24 批次 F6 状态：本任务只做了 ☰ 那一半，且位置与项集与原文不同——☰ 装在顶栏右缘（与输入框同排、宽 40、`Ctrl+M` 开合、Esc 单加"只收菜单"一档），六项 = 新建快捷项 / 批量导入全部 / 启动器设置 / 速查表 / 打开日志 / 退出；重建索引、检查更新、重载三项本程序还没有对应功能。状态栏整条（高约 38、左三段、右两键帽）一条没做，「关闭状态栏后提示回落占位符」这一验收仍无从测起。冲突与理由登记在需求书 FR-UIX-08 注记，原文未改】**
2. **HUD 与大字显示**（FR-UIX-09/10）：不抢焦点 + 点击穿透的提示气泡（优先贴主窗下方，放不下贴上方，主窗未开时按落点屏的 2/3 高度）；`Ctrl+L` 大字。egui 侧实现方式（独立 `ViewportBuilder` 子视口 vs 覆盖层）**需在 M5 末做技术验证**，穿透 + 不激活的组合在 eframe 里未实测。**【2026-10-24 技术验证已做（E-08）：选独立 deferred 子视口，`with_mouse_passthrough(true) + with_active(false)` 成立，不用降级方案；FR-UIX-09 已实现并真机通过（`src/hud.rs`），~~大字显示 FR-UIX-10 仍未做~~**大字显示 FR-UIX-10 也已实现（同日，`src/large_type.rs`）**。实测导出两条计划外事实：winit 的 `with_taskbar(false)` 不设 `WS_EX_TOOLWINDOW`（首帧后须自补 `NOACTIVATE|TOOLWINDOW`、清 `APPWINDOW`），且主窗 `Visible(false)` 后动画只能由 HUD 自己的回调驱动。偏差：`show_on`（FR-WIN-09）未实现 ⇒ 主窗未开时退化为「主窗屏 → 3s 内上次屏 → 鼠标屏」；后台事件 5000ms 停留尚无接线点。详见需求书 FR-UIX-09 注记】【**2026-10-24 同日后半：FR-UIX-10 也交付了**，新文件 `src/large_type.rs`（468 行、8 条纯函数单测）+ `Ctrl+L` 接线。导出第三条计划外事实：**大字窗要抢焦点才收得到 `Esc`，可 FR-WIN-04 会在主窗失焦 50ms 后把主窗藏到托盘** ⇒ 必须让 FR-WIN-04 在大字窗开着期间豁免，否则关大字时 `SetForegroundWindow` 只能激活隐藏窗、焦点落到外部程序（真机抓到的缺陷，已修）；同时 AHK 版「弹大字前 `Hide` 搜索窗」不能照抄——Hide 之后一关大字，焦点落到本进程隐藏的事件窗，按键全丢。面板动作没做（本程序尚无行动作面板），~~`>300` 字符 18 号档与焦点归还的干净真机取证待补~~【同日 20:15 守候轮已结清：382 字符 ⇒ 字号 18、窗口 1622×185；`Esc`×2 + 点击×1 三次焦点归还都回到主窗；顺带第一次跨屏验证「按鼠标所在屏居中」——本机两块 1920×1080，大字窗开在第二块屏上。该轮探针自己的 `inside` 判据只取主屏工作区、截图只抓主屏，那两张 png 不算证据，证据是 Win32 rect + 日志行】，见需求书 FR-UIX-10 注记与发布说明第四十三节】**
3. **皮肤与音效**（FR-UIX-11，Delphi 版独有）：`BG.jpg` 背景图 + `Popup.wav` 提示音，均可关。
4. **主题缩略图画廊**（FR-UIX-12）与 `Copy as Custom Theme`。
5. **使用统计与内存**（FR-SYS-06/07）：按功能桶计数（**不记输入内容**）、30 日柱状；空闲工作集修剪；`WM_SETTINGCHANGE("Environment")` 刷新环境变量。
6. **打包与性能收口**：`cargo-dist` 产出 ZIP 便携版 + MSI（可选）；图标/默认配置内嵌；干净 Win11 无外部依赖跑通；**实测并回填 NFR-01/02/03 的延迟、启动与内存数字**，把"待校准"标记消除。

**验收标准**：阶段二 14 条 FR-UIX 全通过；打包产物在干净 Win11 上解压即用；NFR 表内所有"待校准"项给出实测值或改为可达目标并注明依据。 **预估工作量**：5 人天



## 第四章：技术要点与注意事项

### 4.1 egui 0.36 API 注意事项

> **4.1 勘误（2026-10-14）**：本节原文把方法名写错了，且把"签名从 ctx 改为 ui"这件事描述成了 `update` 改名。以下每一条都按 `Cargo.lock` 锁定的 eframe/egui **0.36.2**、以已编译通过的 `src/main.rs` 为实证重写。原文保留在括号内以便对照。

•    **App trait 的方法名是 `ui`，不是 `update`**（原文误作 "update 已改为 update(&mut self, ui: &mut Ui)"）：实际为 `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)`。三个参数都在，`frame` 通常不用可写 `_frame`。实证：`src/main.rs` 的 `impl eframe::App for LauncherApp`。

•    **CentralPanel 接收 `&Ui`**（原文 "show 签名改为 show(&mut self, ui: &mut Ui)" 不准确）：`egui::CentralPanel::default().show(ui, |ui| { ... })`，传入的是当前 `ui` 借用；闭包内的 `ui` 是**新的子 Ui**，与外层同名，注意别把外层 `&mut` 借用带进闭包。需要 `Context` 时用 `ui.ctx()` 取，不要在 `ui` 可变借用期间另持一份 `ctx`。

•    **ViewportBuilder 的无参化与关闭方式**（原文正确，补实证）：`with_always_on_top()` 无参；`with_decorations(false)` + ~~`with_transparent(true)`~~ 是本项目窗口的基础配置，`src/main.rs` 已如此设置。**（2026-10-15 更正：`with_transparent(true)` 已从 `src/main.rs` 撤掉 —— 它当前优先拿到的 DX12 后端在 DXGI 规则下不可能带逐像素 alpha，实测呈现为暗红块 + 四角残留黑，属于"要了个拿不到、阶段一也不打算用"的半吊子状态。恢复条件：阶段二做透明时把后端改成只留 `Backends::GL`，已实测该组合可干净透出桌面。）**关闭走 `ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close)`；**启动器的 Esc 语义是隐藏而非关闭**，因此 `Close` 只用于真正退出。**修订（2026-10-14 实测）：隐藏不得使用 `ViewportCommand::Visible(false)`** —— eframe 把不可见窗口的重绘钳制到 100ms（`run.rs:27` 的 `INVISIBLE_WINDOW_REPAINT_INTERVAL`、`run.rs:333`），实测热键到"已可见且已聚焦"中位 **118.1ms**；改为"窗口常驻可见 + `ViewportCommand::OuterPosition` 移出屏幕"后同一指标~~中位 **1.6ms**~~**【2026-10-15 更正：park 侧的 1.6ms 不作数】**（20 轮对照，数据见 `experiments/e01-e02-hotkey-report.txt` 与 `-park-report.txt`；那两个数与 park 的"20/20 命中"出自同一个恒真判据 —— 收表条件是"前台已是自己"，而 park 下按下之前前台就是自己，所以它量到的是唤醒延迟 1.7 / 3.5ms，不含抢焦点、也不含移出屏幕后的重新出帧 ⇒ ~~**park 的真实端到端待 `hotkey 20 physical park` 复测**~~ **【同日 20:41 已复测：物理 × park 有效 7 轮端到端 min/中位/max = 12.2 / 14.7 / 58.4 ms、抢焦点 7/7 ⇒ 真实差距约 8 倍（117.5 对 14.7）而不是 74 倍；14.7ms 中位仍超出 FR-WIN-02 的"中位 ≤10ms"，阈值裁决待用户，~~样本还需凑满 20 轮~~】** **【同日 21:21 复跑：单轮有效 15、抢焦点 15/15、端到端 12.5 / 15.7 / 47.6 ms；两轮合并 22 个有效轮次 22/22、中位 15.2 ms、22 个读数无一 ≤10ms ⇒ 样本量只剩"跨轮合并算不算满 20"这条口径待用户裁定【同日已裁定：合并成立 ⇒ E-16 / E-02 闭环；阈值那一半同日裁定 (a) ⇒ FR-WIN-02 放宽为"中位 ≤20ms 且不得出现 ~100ms 平台"，实测中位 15.2ms 达标、冲突解除；尾部 4 个读数记录在案但不作闸门】】**；"禁用 `Visible(false)`"这条决策不受影响，100ms 节流是实测且方向明确的）。采用移出屏幕方案的两项代价必须在 M1 一并解决：① 空闲时**禁止无停止条件的重绘**（窗口一直可见，重绘策略错了会直接把 GPU/CPU 占用顶上去，直接顶掉 NFR-03）；② 屏幕外坐标要在多显示器、改分辨率、DPI 缩放后仍然有效，不被系统拉回；③ 与 FR-UIX-06 的 `DWMWA_CLOAK` 防首帧白闪如何共存，列为待实测（需求书附录 E-15）；~~**【2026-10-15 E-15 cloak 实测已答：cloak 在 DWM 合成层藏得住（`DWMWA_CLOAKED=1`）、Win32 可见语义保留（`IsWindowVisible=true`）、不被 eframe 100ms 不可见重绘节流钳制（露出中位 9.3ms）；但 cloak 不改变焦点归属（20/20 先验成立），与 park 同一缺陷 ⇒ 硬前置对两种隐藏手段都成立】****④ 新增（2026-10-15 实测导出，硬性）：park 隐藏必须显式把键盘焦点交还给别人** —— 注入 19/19 轮实测"移出屏幕之后前台仍是启动器自己"，同日 20:41 的物理 × park 复测开局 2 轮（用户还没来得及点别的应用）再次复现同一现象 ⇒ 两类独立证据，不再是推断，`OuterPosition` 只改位置不改焦点归属；不做这一步，用户按隐藏键后是在对着一个看不见的窗口打字。交还手段（`SetForegroundWindow` 到按键前的前台窗口 / 与 FR-WIN-04 失焦隐藏合并 / cloak 是否自带效果）未实测，同挂 E-15。~~**【2026-10-15 E-15 cloak 实测已答：cloak 不自带交还效果（20/20 先验成立），硬前置对 park 与 cloak 两种隐藏手段都成立，交还动作必须自己做】****新增（2026-10-15 实测定稿为硬性约束）：`NativeOptions` 必须显式限制 wgpu 候选后端** —— `renderer: eframe::Renderer::Wgpu` + `wgpu_options.wgpu_setup → CreateNew(..).instance_descriptor.backends = eframe::wgpu::Backends::DX12 | eframe::wgpu::Backends::GL`。原因：eframe 0.36.2 默认特性含 `wgpu` 不含 `glow`，`Backends::default() = all()` 且 `Backend::Vulkan = 1` 索引最小 ⇒ **默认必先枚举本机已损坏的 Intel Vulkan 驱动并确定性崩进程**（崩在 `AppCreator` 之前，应用层无法捕获）。已落地在 `src/main.rs`，实测 5/5 正常；零新增依赖（`eframe` 已 re-export `egui_wgpu`/`wgpu`，`lib.rs:162`）。是否改回 glow 属未定稿选型，见需求书 E-18。**（2026-10-15 定：不开 `glow`、沿用 wgpu；但"要做逐像素透明就必须只留 `Backends::GL`"是硬约束，`DX12 | GL` 里 wgpu 优先挑 DX12，"允许 GL"不等于"用 GL"，见 E-18 四组对照）**

•    **Input 查询**（原文正确）：`ui.input(|i| i.key_pressed(egui::Key::Escape))` 在 UI 构建过程中使用；`ui.ctx().input(...)` 也可，但要注意同一帧内重复消费。

•    **新增：即时模式下的"选中行滚动跟随"**（原文提到 `ScrollArea::scroll_to_rect`，该 API 在 0.36 不成立）：正确做法是给 `ScrollArea` 设 `scroll_bar_visibility` 并用 `ui.scroll_to_me(Some(egui::Align::Center))`（在被选中那一项的响应里调用），或在 `show_rows` 虚拟列表下用 `egui::scroll::ScrollAreaExtras`/手动 `set_scroll_offset`。**具体到本项目的自绘结果行，M5 阶段应改为不实例化超出行数的控件、用 offset 滚动（参考项目正是这么做的：只实例化 `VisibleRows` 行，永不出现滚动条）。**

•    **新增：重绘策略**。事件驱动的 egui 不会每帧自绘；`Ctrl+↑/↓` 历史回溯、打字机式淡入淡出、后台线程回投的图标/索引、失焦延迟隐藏都必须显式 `ctx.request_repaint()` 或 `ctx.request_repaint_after(Duration::from_millis(n))` 驱动，且**动画类重绘必须有明确停止条件**，否则常驻后台时 GPU 占用不归零（直接顶掉 NFR-03 的内存/功耗预期）。

•    **新增：`eframe::run_native` 的返回类型** 是 `eframe::Result`，`main` 可直接 `-> eframe::Result` 返回（现有代码即如此），但**启动器不能把初始化失败简单冒泡退出**——配置损坏、索引损坏都必须在 `Box::new(|cc| ...)` 构造 App 的闭包里降级处理。



### 4.2 Rust 借用检查常见陷阱

•    **问题场景**：在 update 中同时借用 self.entries 和 self.query 进行过滤，导致不可变借用与可变借用冲突。

•    **解决方案**：将过滤逻辑提取为独立函数：

   fn filter_entries<'a>(entries: &'a [AppEntry], query: &str) -> Vec<&'a AppEntry> {
   if query.is_empty() {
     entries.iter().collect()
   } else {
     entries.iter().filter(|e| e.name.contains(query)).collect()
   }
 }

•    **原则**：UI 构建闭包内只做展示，数据处理在闭包外完成，闭包仅接收处理后的数据引用。



### 4.3 Windows API 调用方案

> **4.3 修订（2026-10-14）**：按 `windows 0.62.2`（`Cargo.lock` 实测已锁此版本）核对模块路径与 feature 名，并补齐参考项目实际用到、原计划漏掉的 API。

•    **全局热键**：`windows::Win32::UI::WindowsAndMessaging::{RegisterHotKey, UnregisterHotKey}`，不使用第三方热键 crate（结论与原文一致）。**但注册句柄与消息接收的实现方式见第三章 M4 修订项 2**：优先用专职 message-only 线程接收 `WM_HOTKEY`，而非在 eframe 内拦截。feature：`Win32_UI_WindowsAndMessaging`、`Win32_Foundation`。

•    **进程启动**：`windows::Win32::UI::Shell::ShellExecuteExW`（原文正确），`SHELLEXECUTEINFOW{ lpVerb: "open" | *runas, lpDirectory: 工作目录, lpParameters, nShow: SW_SHOW|SW_SHOWMAXIMIZED|SW_SHOWMINIMIZED|SW_HIDE }`。feature：`Win32_UI_Shell`。**必须同时保留"启动后拿不到进程句柄"的预期**——`ShellExecute` 不返回子进程，脚本/终端类动作的退出码与输出要另走 `CreateProcessW`。

•    **快捷方式解析**（**2026-10-14 已实测闭环，选型定稿：`IShellLinkW`，不引入 `lnk` crate**）：`Win32::System::Com::{CoInitializeEx, CoCreateInstance, STGM_READ, IPersistFile}` + `Win32::UI::Shell::IShellLinkW`，`CLSID_ShellLink = {00021401-0000-0000-C000-000000000046}`，`InprocServer32 = windows.storage.dll`、`ThreadingModel = Both`。实测取证：后台线程 `CoInitializeEx(STA)` 返回 S_OK，35 个 `.lnk` 的 `GetPath/GetArguments/GetWorkingDirectory/GetDescription/GetIconLocation/GetShowCmd` 全部正确取到、中文不乱码。原"二选一"的 `lnk = "0.6.4"` 落选：`windows` 0.62.2 **已在依赖图内**（accesskit_windows / gpu-allocator / wgpu-hal），走 COM 零新增 crate，且纯 Rust 解析需自行处理代码页。**实测新导出的 4 条实现约束（必须写进 M2 任务）：**
    1. `WorkingDirectory` 与 `IconLocation` 常为未展开的 `%VAR%` 形态（实测 `%windir%\explorer.exe`）⇒ 使用前过 `ExpandEnvironmentStringsW`；
    2. `GetIconLocation` 可为空串 + 索引 0 ⇒ 空图标回落 target 自身图标，且索引是 `i32`，**可为负数**（资源号约定），不得按无符号处理；
    3. Store/MSIX 快捷方式的 target 解析出来是**空串**（`IShellLinkW` 与 PowerShell `WScript.Shell` 双路一致）⇒ 空 target 的 `.lnk` 丢弃，Store 应用一律由 `Get-StartApps` 源覆盖（本机实测 196 条）【2026-10-22 批次 A9 更正：覆盖方式改为 COM 枚举 `shell:AppsFolder`，不用 `Get-StartApps`；且本机该项共 198 条、其中 UWP/MSIX 34 条，"196"是全量数不是 Store 数，详见 8.1 的 A9 行】；
    4. 扫描**必须先按扩展名过滤**：目录直接交给 `IPersistFile::Load` 返回 `0x80070005`；单文件解析失败只跳过、不中断扫描。

    参考项目的实质细节保留：`.lnk` 除目标路径外还带 **Arguments 与 WorkingDirectory**，只取路径会丢参数。feature：`Win32_UI_Shell`、`Win32_System_Com`、`Win32_Foundation`。

•    **图标提取**：`SHGetFileInfoW( SHGFI_ICON | SHGFI_LARGEICON|SMALLICON )` 取 `HICON` → `GetIconInfo` → `DrawIconEx` 到 32 位 DIB → 拆 `BGRA` 给 `egui::ColorImage`（原文的 `GetIconInfo 转换为位图数据` 不完整：单靠 `GetIconInfo` 拿不到像素，必须有绘制到 DIB 这一步，且要处理 mask 与 alpha 预乘）。**`DestroyIcon` 必须显式调用**，否则 GDI 句柄泄漏。feature：`Win32_UI_Shell`、`Win32_Graphics_Gdi`。

•    **补：Store 应用**：`shell:AppsFolder\<AUMID>` 启动，AUMID 来自 `Get-StartApps`（PowerShell）——`ShellExecute` 对 `shell:` 动词有效，`std::process::Command` 无效。【2026-10-22 批次 A9 落地时改了后半句的用法：**没有**把 `shell:AppsFolder\<AUMID>` 交给 `ShellExecuteExW`（对裸 AUMID 是否解析没有文档保证），改走 `IApplicationActivationManager::ActivateApplication`；AUMID 也不来自 PowerShell，而是 COM 枚举时 `GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING)` 直接给出。实测：假 AUMID 走该接口返回 `0x80070057`（参数错误）⇒ 管理器对象与调用链是通的】

•    **补：文件操作动作**：`Shell.Application` 的 `NameSpace(dir).CopyHere/MoveHere`（走系统进度与重名询问）、`folderItem.InvokeVerb("delete")`（进回收站而非直接删）、`SHObjectProperties`（属性面板）、`explorer.exe /select,`（所在位置）、`rundll32.exe shell32.dll,OpenAs_RunDLL`（打开方式）。这些是操作面板（FR-ACT-04）的实际实现，原计划完全没有覆盖。

•    **补：窗口与焦点**：`EnumWindows`/`IsWindowVisible`/`GetWindowTextW`/`GetWindowThreadProcessId`（窗口切换源）、`SetForegroundWindow`/`AllowSetForegroundWindow`（唤起时抢焦点。**2026-10-14 实测：这两者都不需要手写** —— 直接发 `ViewportCommand::Focus` 即可，winit 内部已用 `SendInput VK_LMENU` + `SetForegroundWindow` 绕开限制，20/20 命中；`AllowSetForegroundWindow`/最小化-还原 降级为**仅在物理按键复测不达标时**才启用）、`GetForegroundWindow` + `GetWindowTextW`/`GetClassNameW`（Delphi 版 `{%wd}/{%wt}/{%wc}` 的来源）。

•    **补：单实例与跨进程**：`CreateMutexW`（命名互斥体）、`FindWindowW`/`SendMessageTimeoutW(WM_COPYDATA, ..., SMTO_ABORTIFHUNG)`（参考项目用 `0x414C5452` = "ALTR" 作为协议标识，本项目应自定义一个，勿复用）、`RegisterWindowMessage("TaskbarCreated")` + `ChangeWindowMessageFilterEx`（explorer 重启后重建托盘；管理员权限下必须放行 `WM_COPYDATA`，否则被 UIPI 挡掉）。

•    **补：主题跟随**：`HKCU\...\Themes\Personalize\AppsUseLightTheme` 读取 + `WM_SETTINGCHANGE(0x1A)` 且 `lParam == "ImmersiveColorSet"` 监听；弹出菜单跟随深浅用 uxtheme 序号 **135(SetPreferredAppMode)/136(FlushMenuThemes)**，且**必须判 Windows 10 build ≥ 18362**。

•    **feature 白名单（原文结论保留并具体化）**：只启用 `Win32_Foundation`、`Win32_UI_WindowsAndMessaging`、`Win32_UI_Shell`、`Win32_UI_Controls`、`Win32_System_Com`、`Win32_System_Threading`、`Win32_System_LibraryLoader`、`Win32_Graphics_Gdi`、`Win32_Graphics_Dwm`、`Win32_UI_HiDpi`、`Win32_System_DataExchange`（按需）、`Win32_UI_Input_KeyboardAndMouse`。**禁止全量 features**，这是原计划风险表里 windows-rs 体积膨胀问题的正解。



### 4.4 global-hotkey 崩溃问题替代方案

> **4.4 勘误（2026-10-14）**：本节的"问题根因"判断正确、"不使用 global-hotkey"的结论保留。但**下方"替代方案"里设想的 `raw_input_hook` 拦截 `WM_HOTKEY` 不成立**——`raw_input_hook` 处在 egui 的输入处理阶段，拿不到 Win32 窗口消息；示例代码（含被 Markdown 复制破坏的 `*//*` 注释痕迹）一并作废。**成立的做法见第三章 M4 修订项 2 与第七章新增风险行**：专职 message-only 线程 `RegisterHotKey` + `GetMessage` 循环 + `mpsc` 回投主线程。本节的"备选 rdev"降级为**最后手段**——rdev 是低级键盘钩子，与"不使用系统级钩子"的原立场矛盾，且会被杀软拦截。

•    **问题根因**：global-hotkey crate 内部创建独立消息循环，与 eframe 的 winit 消息循环冲突，导致 WM_HOTKEY 消息丢失或窗口异常。

•    **替代方案**：直接使用 windows-rs 调用 Win32 API，在 eframe 的 RawInputHook 中拦截消息：

   *//* *在 eframe::App* *实现中*
 fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &egui::RawInput) -> bool {
   *//* *通过自定义事件通道传递热键消息*
   *//* *返回 true* *表示消费该输入*
   false
 }

•    **备选方案**：若 eframe 不支持消息钩子，可使用 rdev crate 监听全局按键，但需注意其依赖系统级钩子可能被杀毒软件拦截。



### 4.5 打包分发方案

•    **推荐工具**：cargo-dist，自动配置 GitHub Actions 生成 Windows 安装包。

•    **安装格式**：优先提供 ZIP 便携版，可选提供 MSI 安装版。

•    **Winget** **提交**：使用 wingetcreate 生成清单并提交到 microsoft/winget-pkgs。

•    **签名**：若发布到 Winget，建议对 EXE 进行代码签名，避免 SmartScreen 警告。

> **4.5 修订（2026-10-14）**：结论保留。补充两点——① AHK 版的自动更新是完整子系统（GitHub Releases API + SHA256 校验 + 分步替换 + 失败回滚 + `-Update` 命令行），**本项目 M6 不实现自动更新**，只保证 `cargo-dist` 产物可手动升级；② AHK 版的 Scoop bucket 属于其私有分发渠道，本项目**不做 Scoop**，winget 清单列为可选后置项。



### 4.6 匹配与打分模型（2026-10-14 新增）

原计划通篇只有一个 `name.contains(query)`（1.3 节已核对），没有定义排序规则。这是本项目与"能用"差距最大的一块，也是 AHK 版最核心的资产，故单列一节。

**分值表（照 AHK 版 `FuzzyMatcher` 的七个分数带，逐档实现）**：

| 分值 | 规则 | 备注 |
| --- | --- | --- |
| 100 | 完全相等 | |
| 90 | 标题以查询开头 | |
| 80 | 某个词以查询开头 | |
| 75 | 词首字母匹配 | `vsc` → Visual Studio Code；分词前先按 `([a-z])([A-Z]) → "$1 $2"` 拆驼峰、再按非字母数字切词，故 `AutoHotkey`→`ah`、`my_file-name`→`mfn`；**要求查询长度 ≥ 2** |
| 60 | 包含 | `60 - min(pos, 20) * 0.25 - lengthPenalty`，越靠前越高 |
| 45 | 多词全命中 | `min(total/count, 90) * 0.5`；**任一词不命中即为 0** |
| 30 | 子序列 | 长度 ≥ 3 且首字母相同 |

**`lengthPenalty = min(max(key.len - query.len, 0), 40) * 0.1`（最多 −4）**；同一分值带内**文本越短越高**。

**三条必须继承的设计决策（不是随意参数）**：

1. **稳定排序 + 注册顺序兜底**：同分时保持数据源注册顺序。用 `sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap())` 配 Rust 稳定的 `sort_by`，不要引入随机或时间戳。
2. **Provider 分值偏移决定跨源次序**：参考项目的偏移带是 计算器 `200` > `{query}` 独占 / 终端 / 带词的网页搜索 `150` > 关键字完全命中的命令 `max(score,100)` > 裸关键字行 `50` > 应用 `+10` > 命令 `+15` > 目标文件名匹配 `-5` > 窗口在默认结果 `-14` > 书签正文命中 `35` > 引擎名补全提示 `20`。**这张表必须在 M2 里以常量集中的形式落地并可被测试锁定**，否则每个数据源各自调分会让排序不可预测。
3. **学习排序只加在排名上**：`Boost = 8·ln(1+count)` + 近 7 天 +5 + 该输入或其前缀曾选中它 ≤+60。参考项目**刻意不给应用源直接加 Boost**，理由是高频应用会被 Boost 挤扁、新查询失去位置。【2026-10-18 批次 K：本条原则已兑现为 `learn_rank` 两档（默认甲＝次数优先），时间项落地为半衰期而非"近 7 天 +5"台阶，对照理由见第 13 项注记。】

**匹配键预计算**：每个条目预存 `{lower, words, initials, pinyin_initials, length}`，扫描期一次算完。参考项目在这点上的性能结论是"命令首字符匹配 300–400ms → 10–20ms"，靠的就是预计算 + 线性化读取，**不是**靠语言优势。

**拼音**：`pinyin 0.11` 取首字母。参考项目用的是 GBK 码区间表（23 个区间映射 A..Z，跳过 K/N/E 等），与词典法的取舍不完全一致，**多音字结果会有差异，M2 需用一组中文程序名做对照测试后再定验收口径**（此处标注为待确认，不假装等价）。

**高亮区间**：匹配需同时产出 `MatchRanges = [(start, len)]` 供阶段二分段渲染。查询含非 ASCII 且"1 个 CJK 视为 1 字母"时回落拼音首字母匹配。注意参考项目的取舍：**靠关键字、路径或片段正文命中的结果，标题不高亮**——因为高亮正文会让"匹配了什么"变得误导。



### 4.7 线程与并发模型（2026-10-14 新增）

原计划只在风险表里提了一句"图标提取放 tokio/async_std 后台线程"。按 M2 的数据源规模，需要一份明确模型。

**主线程（winit/eframe）**：只做 UI 构建、输入事件消费、`ShellExecuteExW` 与需要 STA/前台权限的窗口操作。**任何目录遍历、COM 解析、图片解码都不得出现在这一帧里**（NFR-01 的 ≤16ms 由此保证）。

**后台线程（专职，std::thread）**：

| 线程 | 职责 | 产出 |
| --- | --- | --- |
| `index-scan` | 应用索引、文件索引的构建与周期刷新 | 索引快照 |
| `icon` | `SHGetFileInfoW` → `ColorImage` 字节 | `(路径, RGBA, 宽高)` |
| `clipboard-watch` | 剪贴板序列号轮询/监听、图片落盘 | 历史条目 |
| `hotkey-msg` | message-only 窗口的 `GetMessage` 循环 | `WM_HOTKEY` 事件 |
| `window-enum` | 可见顶层窗口枚举（可缓存 1000ms） | 窗口列表 |

**发布与消费协议**：

1. 后台线程算完通过 `mpsc::Sender<T>` 回投，主线程在 `App::ui` 开头**非阻塞 drain**（`try_recv` 循环），处理后 `ctx.request_repaint()`。
2. 大型只读快照（应用索引、文件索引）用 **`arc-swap::ArcSwap<Index>`** 发布：后台构建新 `Arc`、整体替换，主线程 `load_full()` 读。**不要用 `Mutex< Vec >` 包住索引**——那会让搜索帧被一次扫描卡住，正是 NFR-01 的违背方式。
3. **搜索代数（generation counter）**：输入去抖后每次搜索赋一个自增 `gen`，回投结果若 `gen < 当前 gen` 则**丢弃**。参考项目正是靠这个避免"旧结果覆盖新结果"的抖动。本项目在 M1 就要实现，因为文件搜索/书签这类慢源必然跨帧。
4. **去抖**：连续输入只在停止后搜一次。参考项目用一次性定时器（AHK `SetTimer(fn, -1)`），Rust 侧用"上次输入时刻 + 帧间隔判断"即可，**不要为此起线程定时器**。
5. **图标缓存线程安全**：`TextureHandle` 只能在主线程创建（它绑着渲染上下文），因此后台只做"解码成 RGBA 字节"，主线程再做纹理上传与 LRU 淘汰。这一条划分错了会直接编译不过或崩溃。
6. **COM**（**2026-10-14 实测确认**）：`.lnk` 解析线程需 `CoInitializeEx(COINIT_APARTMENTTHREADED)` 并在**同一线程**内完成 `CoCreateInstance → IPersistFile::Load → Get*` 再 `CoUninitialize`。实测取证：后台（非主）线程以 STA 初始化返回 S_OK、解析全部成功；`windows.storage.dll` 的 `ThreadingModel = Both`，所以 apartment 选择不会成为瓶颈，**不需要为 `.lnk` 单独开 MTA 线程**。`Get-StartApps` 是外部 PowerShell 调用，**必须有超时且失败即跳过**（本项目继承参考项目对 `.lnk` 的口径："单文件解析失败静默跳过、不中断扫描"，实测目录入参返回 `0x80070005` 即按此处理）。
7. **退出顺序**：`on_close`/关闭事件里先停后台线程（用 `AtomicBool` 取消标志 + join 超时），再落盘配置，避免"索引线程在配置已写完后又触发一次保存"。



## 第五章：项目文件结构规划

> **第五章 修订（2026-10-14）**：原结构是"单扫描器 + 单启动器"的形态，只能容纳一个数据源；按 M2 的 19 条 FR-SRC，必须改成 **provider 插件式目录 + matching + index + actions**。同时 `config.rs` 的 TOML 改 JSON、`locales/*.toml` 改 `*.json`（依据 1.2 节修订）。原结构见本章末尾的"原结构留档"。

```
egui-launcher/
├── src/
│   ├── main.rs                 # 入口：单实例检测 → 配置加载(含降级) → 线程启动 → run_native
│   ├── app.rs                  # App::ui 实现：状态机(visible/panel/marked) + 事件消费 + 重绘策略
│   │
│   ├── entry.rs                # ResultItem / Kind / IconRef / Actions —— 阶段二自绘所需的完整字段
│   ├── query.rs                # SearchQuery 解析：keyword + rest、独占模式、前缀标记
│   ├── matching/
│   │   ├── mod.rs              # 打分入口：score(query, key) -> f64
│   │   ├── fuzzy.rs            # 七档分值表 + lengthPenalty + MatchRanges 高亮区间
│   │   ├── key.rs              # 预计算 {lower, words, initials, pinyin, length}
│   │   ├── pinyin.rs           # 拼音首字母（对照测试见 4.6）
│   │   └── wildcard.rs         # Delphi 版 `* ?` → 正则【2026-10-28 更正：C2 落地时没单开文件，`WildPattern` + `wild_pattern()` 编译缓存就在 `src/matching.rs` 里，`src/` 下也没有 `matching/` 子目录——块里 `key.rs` / `pinyin.rs` 同样并入的单文件实现。这张树是规划稿，别当现状读】
│   ├── ranking.rs              # Provider 分值偏移常量表 + Knowledge.Boost + 稳定排序/去重/截断
│   │
│   ├── provider/
│   │   ├── mod.rs              # trait DataSource + Registry(注册顺序) + 逐项容错
│   │   ├── applications.rs     # 开始菜单/自定义目录/Store/PATH
│   │   ├── commands.rs         # 自定义命令 + {query} + 路径四态校验
│   │   ├── snippets.rs         # 片段 + 占位符展开 + 粘贴回
│   │   ├── clipboard.rs        # 剪贴板历史
│   │   ├── calculator.rs       # 表达式/单位/进制/日期
│   │   ├── websearch.rs        # 引擎表 + {query|默认} + 兜底
│   │   ├── bookmarks.rs        # Chrome 家族 Bookmarks JSON
│   │   ├── windows.rs          # 窗口切换（EnumWindows）
│   │   ├── filesearch.rs       # 文件搜索：路径浏览/类型词/空格模式
│   │   ├── scripts.rs          # 脚本目录 + @altrun.* 注释元数据
│   │   ├── terminal.rs         # > 前缀
│   │   ├── system.rs           # 系统命令（按分组，非按总数）
│   │   ├── help.rs             # ? 速查表
│   │   └── recent.rs           # 置顶与最近（空输入框默认结果）
│   ├── expand.rs               # ;keyword 全局片段自动展开（独立子系统，P2）
│   │
│   ├── index/
│   │   ├── mod.rs              # ArcSwap 快照发布 + 代数 generation
│   │   ├── app_index.rs        # AppIndex.json 读写与刷新
│   │   ├── file_index.rs       # ScopeFolders × Depth × Exclude，MaxEntries
│   │   └── knowledge.rs        # 学习计数 + 搜索历史
│   │
│   ├── actions/
│   │   ├── mod.rs              # ActionCatalog：按 kind 与可编辑性列动作
│   │   ├── panel.rs            # → 面板状态与返回语义
│   │   └── batch.rs            # Insert 多标记的批量动作
│   ├── launcher.rs             # ShellExecuteExW / CreateProcessW、参数标记替换、@+ @- @
│   ├── params.rs               # {%p} {%c} {%wd} {%wt} {%wc} + ParamType 编码 + 历史（rank 50，非 LRU）
│   │
│   ├── config.rs               # JSON 配置模型 + SchemaVersion 迁移 + .bad/.bak + DataLocation
│   ├── hotkey.rs               # RegisterHotKey + message-only 线程 + 录制框校验
│   ├── windowing.rs            # 可见性/焦点/置顶/落点/记住位置/输入法/DWM 圆角
│   ├── tray.rs                 # tray-icon + muda + TaskbarCreated
│   ├── single_instance.rs      # 命名互斥体 + WM_COPYDATA + CLI 协议
│   ├── shell_folders.rs        # A_* 路径变量、%VAR% 展开、自启/SendTo/开始菜单快捷方式
│   ├── icon.rs                 # HICON → RGBA（后台）→ TextureHandle（主线程）+ LRU
│   ├── clipboard_store.rs      # arboard 封装 + IgnoreApps + 临时借用不写历史
│   ├── i18n.rs                 # locales/*.json + _name/_font/_fallback + 键集完整性测试
│   ├── logger.rs               # tracing 初始化（不记输入内容）
│   ├── usage.rs                # 使用统计桶
│   └── ui/
│       ├── mod.rs
│       ├── search_bar.rs       # 输入框 + 占位符 + 按键抢跑缓冲
│       ├── result_row.rs       # 阶段一：默认控件版式；阶段二：自绘行（同一份数据）
│       ├── status_bar.rs       # 状态栏与 ☰ 菜单（阶段二）
│       ├── hud.rs              # 通知气泡（阶段二）
│       ├── large_type.rs       # 大字显示（阶段二）
│       ├── preferences/        # 偏好设置各页（阶段一默认控件，阶段二改两列表单）
│       │   ├── mod.rs
│       │   ├── general.rs  ├── window.rs  ├── appearance.rs  ├── features.rs
│       │   ├── commands.rs ├── snippets.rs ├── clipboard.rs   ├── websearch.rs
│       │   ├── calculator.rs ├── scripts.rs ├── hotkeys.rs    └── advanced.rs
│       └── manager.rs          # 快捷项批量管理（绝对↔相对、失效校验批删）
│
├── assets/                     # icon.ico / tray.ico / 内嵌默认背景与音效
├── locales/                    # zh-CN.json en.json
├── themes/                     # 阶段二：内置主题 JSON + 用户主题目录
├── tests/                      # 匹配打分/排序偏移/配置迁移/键集完整性/占位符
├── Cargo.toml
├── README.md
└── dist-workspace.toml         # cargo-dist
```

**与两个参考项目的结构对照**：

| 参考项目单元 | 本项目归属 |
| --- | --- |
| Delphi `Unit/untALTRunOption.pas`（INI + 语言 + 常量） | `config.rs` + `i18n.rs` |
| Delphi `Unit/untShortCutMan.pas`（数据模型 + 过滤 + 排序 + 启动线程） | `entry.rs` + `matching/` + `ranking.rs` + `launcher.rs` |
| Delphi `Unit/untUtilities.pas`（单实例/自启/SendTo/.lnk/剪贴板/分层窗口） | `single_instance.rs` + `shell_folders.rs` + `windowing.rs` + `clipboard_store.rs` |
| Delphi `Unit/untClipboard.pas` / `untLogger.pas` | `clipboard_store.rs` / `logger.rs` |
| Delphi `Form/frmALTRun` | `app.rs` + `ui/search_bar.rs` + `ui/result_row.rs` |
| Delphi `Form/frmConfig` / `frmShortCut` / `frmParam` / `frmShortCutMan` / `frmInvalid` | `ui/preferences/` / `ui/preferences/commands.rs` / `params.rs` + 编辑对话框 / `ui/manager.rs` / `ui/manager.rs`(失效清单)〔2026-10-28 批次 B6 实测：`frmParam` 的**数据与规则**落在 `src/source/params.rs`（纯函数），它的**框**没有独立文件，就在 `src/main.rs` 的主界面帧里（`param_prompt` 那一节）——本表是规划期的目录映射，如实记这一处落点差异〕 |
| AHK `Src/Core`（App/AppSettings/ProviderRegistry/SearchQuery/FuzzyMatcher/ThemeManager/I18n/ActionCatalog） | `app.rs` + `config.rs` + `provider/mod.rs` + `query.rs` + `matching/` + `themes/` + `i18n.rs` + `actions/` |
| AHK `Src/Providers/*`（14 个） | `provider/*`（本项目拆为 15 个文件，Help/Recent 独立） |
| AHK `Src/UI/SearchWindow.ahk`(1763) / `PreferencesWindow.ahk`(1673) / `Hud.ahk` | `app.rs` + `ui/result_row.rs` + `ui/status_bar.rs` / `ui/preferences/` / `ui/hud.rs` |
| AHK `Src/Extensions/`（QuickSwitch / AutoDate / SnippetExpander / PTTools） | **仅 `expand.rs` 在范围内**，其余全部排除（见 1.3 节范围排除） |
| AHK `Lib/Units.ahk`（ResultItem 等数据契约） | `entry.rs` |

**原结构留档（2026-10-14 修订前）**：`main.rs / app.rs / config.rs(TOML) / entry.rs / scanner/{mod,path_scanner,start_menu}.rs / launcher.rs / hotkey.rs / tray.rs / icon.rs / clipboard.rs / i18n.rs / logger.rs / ui/{mod,search_bar,app_list,settings,context_menu}.rs` + `assets/{icon.ico,tray.ico,default_bg.jpg}` + `locales/{en,zh-CN}.toml`。其 `scanner/` 只能容纳"PATH + 开始菜单"两件事，无法扩展到 15 个数据源，故替换。



## 第六章：依赖 Crate 推荐清单

> **第六章 修订（2026-10-14）**：原表 13 行只覆盖 Delphi 版所需能力，缺匹配、拼音、`.lnk`、表达式求值、书签、窗口枚举等 M2 的硬需求，且全部未给版本。现按 **crates.io 实测版本**（2026-10-14 `cargo search`）重写，并区分"必选/二选一/需实测后定"。**"待实测"三行不要在选型阶段就当已确认。**

**必选**

| Crate | 版本 | 用途 | 备注 |
| --- | --- | --- | --- |
| `eframe` / `egui` | **=0.36.2** | GUI 框架与事件循环 | **用 `=` 精确锁版**（原计划风险表的建议正确，照做）；~~渲染后端 glow~~ **2026-10-15 更正：默认特性不含 `glow`，实际后端是 wgpu，已在 `main.rs` 把候选后端限制为 `Backends::DX12 \| Backends::GL`（零新增依赖：eframe 已 re-export `egui_wgpu` / `wgpu`，见 `eframe-0.36.2/src/lib.rs:162`）。是否改回 glow 待决策，见需求书 E-18** → **已定稿：沿用 wgpu + `DX12\|GL`，不开 `glow`（2026-10-15）**；~~同批记为"透明实测闭环"~~ **同日二次更正：DX12 的 HWND 交换链按 DXGI 规则不可能带逐像素 alpha，`src/main.rs` 已撤掉 `with_transparent(true)`；阶段二要做透明必须只留 `Backends::GL`（已实测可干净透出桌面）。依据与四组对照见需求书 E-18** |
| `skrifa` | **0.44** | 字体解析预判（FR-SYS-08） | 与 epaint 0.36.2 同一个解析器，用于在把字体交给 egui 之前预判 `FontsImpl::new` 会不会 `panic!`（判据等价性实测 8/8）。已在 `Cargo.lock` 中（epaint、glifo 依赖），**零新增下载、零额外编译**；特性声明照抄 epaint（`default-features=false` + `std`+`autohint_shaping`）以免 feature 统一带进额外编译面 |
| `windows` | **0.62.2** | Win32/COM：热键、Shell、ShellLink、GDI/DWM、注册表消息、窗口枚举 | `Cargo.lock` 已因 eframe→winit 锁到同版本，直接复用；**feature 白名单见 4.3** |
| `serde` + `serde_json` | 1.0.229 / 1.0.154 | 配置、索引、历史、主题的序列化 | 替代原表的 `serde + toml`，理由见 1.2 修订 |
| `regex` | 1.13.1 | `Exclude` / `ScopeExclude` 过滤、Delphi 版通配符转正则 | 编译结果必须缓存（`LruCache` 或 `OnceLock`），不可每帧 compile【2026-10-28 满足：用的是 `MatchIndex.wild_cache: Option<(String, Option<WildPattern>)>` 单条目缓存——键是查询原文，换词才重编；不需要 LRU 是因为搜索只看当前这一串。`Exclude` 那条将来接同一个引擎，各自另存编译结果】 |
| `dirs` | 7.0.0 | APPDATA / 开始菜单 / 桌面 / ProgramData 等标准路径 | 与参考项目的 `A_*` 变量表对齐，见 `shell_folders.rs` |
| `arboard` | 3.6.1 | 剪贴板读写（文本 + 图片） | FR-SRC-09 剪贴板历史、`{%c}`、片段粘贴回 |
| `tracing` + `tracing-subscriber` | 0.1.44 / 0.3.23 | 日志 | 输出文件 + 控制台；**不得记录用户输入内容**（继承参考项目的隐私约束） |
| `image` | 0.25.10 | 背景图/站点图标/`thumb:` 解码 | `HICON` 转换走 GDI，不用 image |
| `walkdir` | 2.5.0 | 文件索引与应用目录递归 | 必须配 `max_depth` + `IntoIter::skip` + 自定义排除，否则 `node_modules` 会打死索引线程 |
| `arc-swap` | 1.9.2 | 索引快照无锁发布 | 见 4.7，替代 `Mutex<Vec>` 方案 |
| `pinyin` | 0.11.0 | 拼音首字母匹配 | **与参考项目的 GBK 区间表结果不完全一致，多音字需对照测试**（见 4.6） |
| `evalexpr` | 13.1.0 | 计算器表达式求值 | 备选 `meval 0.2.0`；两者都不支持单位/进制/日期，那三块**自研** |
| `tray-icon` + `muda` | 0.26.1 / 0.21.2 | 系统托盘与菜单 | 替代 CoolTrayIcon；需验证与 eframe 主循环共存（见第七章新增风险行） |
| `winreg` | 0.56.0 | 注册表：`AppsUseLightTheme`、`CurrentVersion\Run`（若不用启动夹） | 原表结论保留 |
| `cargo-dist` | 由 `dist-workspace.toml` 管理 | 打包发布 | 原表结论保留 |

**二选一（M2 开工前实测后定，当前未定）**

| 需求 | 方案 A | 方案 B | 决策依据 |
| --- | --- | --- | --- |
| `.lnk` 解析 | ~~`lnk` 0.6.4（纯 Rust，无 COM）~~ **落选** | `windows` 0.62.2 的 `IShellLinkW`（COM）**✅ 2026-10-14 实测定稿** | 决策依据已验证：`IShellLinkW` 一次拿全 **Arguments + WorkingDirectory + Description + IconLocation + ShowCmd**；后台线程 `CoInitializeEx(STA)` 实测 S_OK，`windows.storage.dll` 为 `ThreadingModel=Both`，apartment 不构成成本；`windows` 0.62.2 已在依赖图内 ⇒ 零新增 crate。详见 4.7 的实测记录与需求书附录 E-03 |
| 开机自启 | 启动夹写 `.lnk`（**Delphi 版实际生效的路径**） | `HKCU\...\CurrentVersion\Run` | 需要参数 `-Startup` 时用启动夹更贴近两个参考项目；`winreg` 分支留作可配 |

**明确不引入**

| 候选 | 不引入的原因 |
| --- | --- |
| `global-hotkey` | 原计划已判定其与 winit 消息循环冲突导致崩溃，修订后仍成立 |
| `tokio` / `async-std` | 见 1.2 与 4.7，工作负载是阻塞型 Win32 调用，异步运行时与 winit 所有权模型相冲 |
| `notify` | 最新稳定版是 **9.0.0-rc.5**（实测 `cargo search`），**不引 rc 版本**；文件索引用周期刷新（默认 30min）+ 手动重建，与参考项目一致 |
| `fluent` / `rust-i18n` | 原表建议 `fluent / rust-i18n`。改为自管 `locales/*.json`，因为需求要求 `_name/_font/_fallback` 元键与"每键必须被翻译"的单测断言（参考项目就是这么做的），第三方 i18n 框架的目录结构反而挡路 |
| `smallvec` | 最新为 `2.0.0-beta.2`，Beta 不进依赖树；且本场景无此优化需求 |
| `open` / `shellexecute` 一类的封装 crate | 需要 `*runas`、工作目录、`nShow` 标志与 `SEE_MASK`，直接调 `ShellExecuteExW` 更可控 |

**原表遗留项的处置**：`CoolTrayIcon → tray-icon + muda`、`HotKeyManager → windows`、`regexpr → regex`、`pngimage → image` 四条结论不变；`CnPack → 无需替代` 不变（它是 Delphi IDE 增强，仅被 `untLogger` 用于调试钩子）。



## 第七章：风险与应对

本章列出 egui-launcher 项目开发过程中可能遇到的技术风险及应对方案。



 

| **风险项**                         | **可能性** | **影响程度** | **应对方案**                                                 |
| ---------------------------------- | ---------- | ------------ | ------------------------------------------------------------ |
| egui 0.36 API 再次大版本变更       | 中         | 高           | 锁定 Cargo.toml 中的版本号（使用 =0.36.2 而非 ^0.36.2），关注 egui 发布说明；编写 UI 抽象层隔离框架依赖，减少变更影响范围。 |
| global-hotkey crate 崩溃复现       | 低         | 高           | 已确定替代方案：使用 windows-rs 直接调用 RegisterHotKey Win32 API。若 windows-rs 也有问题，备选 rdev crate（但需注意系统级钩子权限问题）。 |
| windows-rs crate 体积膨胀          | 中         | 中           | windows-rs 默认包含大量 API 绑定。通过 features 白名单精确控制包含的模块（如仅启用 Win32_UI_WindowsAndMessaging、Win32_Foundation、Win32_UI_Shell 等），避免全量引入。 |
| egui 无边框窗口在 Win11 上显示异常 | 中         | 中           | Win11 22H2+ 对无边框窗口有特殊处理。若出现闪烁或无法置顶问题，可尝试使用  WS_EX_LAYERED + WS_EX_TRANSPARENT 窗口样式，或改用 PopupWindow 模式。 |
| **开发机图形上下文创建即崩溃（2026-10-14 新增，2026-10-15 根因闭环）** | ~~高（本机 4–8/10 次）~~ **高（默认配置 8/8，确定性）** | ~~高（阻断 E-01/E-02 实测）~~ **中（已规避，不再阻塞开发；但驱动/系统层未根治，正式版复测仍需要）** | **2026-10-14 取证**：进程在 `eframe::run_native` 内崩，`AppCreator` 从未执行，`ACCESS_VIOLATION at igvk64.dll+0x64ea82 (write)`；崩溃率与窗口选项无关（默认/visible/visible+decorated/transparent+on_top = 4、8、5、6 次/10）；未改动的 `src/main.rs` 同样崩；`CrashDumps\egui-launcher.exe.7648.dmp` 时间戳 2026-10-13 早于本项目实测 ⇒ 环境问题。<br>**2026-10-15 根因更正（推翻了上一段的两个判断）**：① `igvk64.dll` **不是 OpenGL ICD**，其版本资源写明 `FileDescription = "Vulkan(R) Driver for Intel(R) Graphics Accelerator"` 31.0.101.2140；② **不是随机崩而是确定性崩**（默认 5/5、3/3）；③ 机制是 eframe 0.36.2 默认特性含 `wgpu` 不含 `glow`，`Renderer::default()` 返回 `Wgpu`，而 `Backends::default() = all()` 且 `Backend::Vulkan = 1` 索引最小、`request_adapter` 按索引顺序枚举（`wgpu-core instance.rs:492-496`）⇒ **默认必先试 Vulkan**；④ 虚拟显卡被排除：停掉向日葵后仍 5/5 崩；⑤ 交叉验证：进程内 `backends = DX12` 与 `GL` 各 3/3 正常并打印出真实适配器（`backend=Dx12 name="Intel(R) HD Graphics 630"`），改回 `VULKAN` 立即复现同一 AV。<br>**处置（已落地）**：`src/main.rs` 显式 `renderer: Renderer::Wgpu` + `instance_descriptor.backends = Backends::DX12 \| Backends::GL`，实测 5/5 正常、截图核对窗口渲染正确。① 该限制是**硬性约束**，写进需求书 2.3 分层约束 4 与退出标准；② 后端选型（glow vs wgpu/DX12）**未定稿**，见需求书 E-18；③ `VK_LOADER_DRIVERS_DISABLE=igvk64.dll` 实测**无效**（4/4 仍崩，原因未查明），不得作为规避手段；④ 驱动/系统层根治（更新或回退 Intel 驱动、换正式版 Win11）**未做**，属环境待办，绝对延迟指标仍需在正式版环境复测。 |
| 开始菜单 .lnk 解析失败或超时       | **中（原"高"，2026-10-14 实测后下调）** | 中           | .lnk 文件可能指向已卸载的程序或网络路径。解决方案：设置超时时间（如 500ms/文件），失败时静默跳过；扫描放在后台线程，不阻塞 UI。**实测修订**：本地 35 个 `.lnk` 在后台线程 STA 下逐个解析即时返回、无超时，因此风险来源不是解析本身，而是**网络/UNC 目标**，500ms/文件的兜底保留但降级为中概率。同时实测暴露 3 类必须处理的失效形态：① 目录入参 → `0x80070005`（按扩展名先过滤即可规避）；② Store/MSIX 项 target 为空串（直接丢弃，由 `Get-StartApps` 覆盖，实测 196 条）；③ `WorkingDirectory`/`IconLocation` 为未展开 `%VAR%`（必须 `ExpandEnvironmentStringsW`）。均不中断整体扫描。详见 4.7 与需求书附录 E-03。 |
| 图标提取性能问题                   | 中         | 中           | **勘误（2026-10-14）**：原方案写"放在 tokio/async_std 后台线程"，与第六章"不引入 tokio"冲突，作废。改为：专职 `icon` 线程做 `SHGetFileInfoW → RGBA`，经 `mpsc` 回投，主线程只做 `TextureHandle` 上传（`TextureHandle` 绑渲染上下文，不可跨线程创建）；路径→纹理 LRU 缓存上限 1500；启动后分批预读、每轮限时；`DestroyIcon` 必须显式调用防 GDI 泄漏。详见 4.7。 |
| 配置文件损坏导致启动崩溃           | 低         | 中           | **勘误（2026-10-14）**：Rust 没有 try-catch，原表述作废。改为：解析失败时把文件改名 `.bad` 并以默认值启动（照 AHK 版 `ALTRun.json.bad` 的做法），`Restore Defaults` 前写 `.bak`；`SchemaVersion` 逐版迁移 + 缺失项补默认 + 类型不符取默认；首次启动自动创建数据目录。注意 `.bad` 与 `.bak` 是两个不同用途的文件，不可合并成一个。 |
| 全局热键与其他软件冲突             | 中         | 低           | 提供热键配置 UI 让用户自定义快捷键；注册热键失败时记录日志并提示用户，不崩溃；支持多个热键组合以备切换。 |
| 杀毒软件误报/拦截                  | 中         | 中           | 使用 cargo-dist 打包时启用代码签名（需购买证书）；提供开源仓库链接方便用户审查代码；首次运行时提示用户添加信任。 |
| egui 渲染性能不足（大量程序列表）  | 低         | 低           | egui 本身性能优秀，即使数千条列表也能流畅渲染。若遇性能问题，可启用虚拟列表（仅渲染可见区域）优化。**补充（2026-10-14）**：真正的性能风险不在渲染而在**每帧重新打分**，对策见 4.6 的匹配键预计算与 4.7 的 `ArcSwap` 快照。 |
| WM_HOTKEY 与 winit 消息循环接合方式未定 | **已闭环（原：高）** | 高 | **2026-10-14 实测已定稿，风险解除**。首选方案成立：专职 message-only 线程 `RegisterClassExW`+`CreateWindowExW(HWND_MESSAGE)`+`RegisterHotKey`+`GetMessageW` 循环 + `mpsc` 回投，主线程在 `App::logic` 消费；唤醒靠任意线程 `ctx.request_repaint()` → `EventLoopProxy` 唤醒 `ControlFlow::Wait`，无须挤进 winit 事件循环。实测 20 轮热键全部送达。**新发现的不是"接不上"而是"接上了但慢"**：`Visible(false)` 隐藏会让唤起端到端~~从 1.6ms 涨到~~涨到 118.1ms（eframe 对不可见窗口重绘钳到 100ms；**2026-10-15 更正：1.6ms 那个 park 基线作废，它实为唤醒延迟 1.7 / 3.5ms，~~park 含抢焦点的端到端待测~~ 同日 20:41 已补测：物理 × park 有效 7 轮 min/中位/max = 12.2 / 14.7 / 58.4 ms，约 8 倍而非 74 倍；同日 21:21 复跑单轮有效 15、15/15，两轮合并 22 个有效轮次 22/22、中位 15.2 ms、22 个读数无一 ≤10ms；同日裁定 (a) 已把该阈值放宽为"中位 ≤20ms 且不得出现 ~100ms 平台" ⇒ 中位达标、冲突解除**），隐藏方式必须改成"移出屏幕"，详见第三章 M4 修订项 2 与需求书附录 E-01。 |
| 默认热键 Win+Space 与系统冲突 | 高 | 中 | **新增（2026-10-14）**。`Win+Space` 被 Windows 用于切换输入法/任务视图，注册后不稳定。已在第三章改为 `Alt+Space` 主 + `Alt+R` 次（与两个参考项目一致）。用户自定义时录制框需拒绝与已注册热键重复，并提示系统保留组合。 |
| 15 个数据源的排序互相踩踏 | 高 | 中 | **新增（2026-10-14）**。多源同分时用户感知为"顺序乱跳"。对策：4.6 的 Provider 分值偏移常量表 + 稳定排序 + 注册顺序兜底，并用单测锁定偏移表，禁止各源自行调分。 |
| 文件索引构建打爆 UI 或磁盘 | 高 | 中 | **新增（2026-10-14）**。已排除 Everything，索引完全自建，`ScopeFolders` 一旦含用户目录且 `Depth` 配大，会遍历 `node_modules`/`.git`。对策：`max_depth` + `MaxEntries`（默认 30000）+ 默认排除正则 + 周期分批；索引线程可取消；构建期间旧快照继续可用（`ArcSwap`），不得清空列表。 |
| egui 透明窗口下圆角/点击穿透能力不足 | 中 | 中 | **新增（2026-10-14）**。阶段二的 HUD 要求"不抢焦点 + 点击穿透"，eframe 子视口是否支持该组合**未实测**。若不成立，退化方案：HUD 用主视口内的覆盖层绘制（会失去独立窗口位置），或放弃穿透只做到不抢焦点。列为 M6 技术验证项。**〔2026-10-24 已关闭「点击穿透 + 不抢焦点」这一半：E-08 实测 deferred 子视口成立，退化方案不需要启用；生产实现 `src/hud.rs` 真机通过（弹出期间前台始终是外部应用、HUD 下方鼠标事件穿透、主窗隐藏后 HUD 继续画）。剩下的是 E-07 的「圆角半径 / 边框色 / 阴影 / DWM 属性」取证面，与本行描述的 HUD 组合无关。〕** |
| tray-icon/muda 与 eframe 主循环共存 | 中 | 中 | **新增（2026-10-14）**。托盘事件回调与 winit 事件循环的分发顺序、以及隐藏主窗口后托盘是否仍能唤醒，需在 M4 早期单独验证（该组合未实测）。备选：退回纯 `NotifyIcon` Win32 自实现。 |
| 内存目标 20MB 不可达 | 高 | 低 | **新增（2026-10-14）**。该指标源自 Delphi VCL，eframe 单窗口进程由渲染上下文主导，无法达到。已在 1.1 修订为 ≤120MB 待实测目标，M6 用实测值收口，避免拿一个不可能的数字做验收。 |
| 拼音实现与参考项目结果不一致 | 中 | 低 | **新增（2026-10-14）**。参考项目用 GBK 码区间表（23 区间，跳过部分字母），`pinyin` crate 用词典法，多音字与生僻字结果会有差异。对策：M2 建一组中文程序名对照用例，据结果决定是沿用 crate 还是自写区间表；不宣称等价。 |
| 抢前台焦点失败导致唤起"看不见" | **⚠ 2026-10-15 风险等级回调为中：~~生产选定的隐藏策略下无数据~~ 同日 20:41 已有 7 轮有效数据（7/7 命中），但样本未满 20 轮、且端到端中位 14.7ms 超出 FR-WIN-02 的 ≤10ms 阈值待裁决 ⇒ ~~维持中~~ **低**（2026-10-15 同日裁定 (a) 放宽阈值后，按本行自述规则降级；命中率两轮合并 22/22，延迟中位 15.2ms 在"中位 ≤20ms 且无 ~100ms 平台"下达标）**（原：已实测、概率降为低）**【同日 21:21 复跑：单轮有效 15、15/15；两轮合并 22 个有效轮次 22/22、中位 15.2 ms、22 个读数无一 ≤10ms ⇒ 命中率的证据已经够看，等级仍维持中的原因只剩两条口径待用户裁定：跨轮合并算不算满 20、≤10ms 阈值放宽还是列 M1 优化项】** **【同日裁定：前者裁定"合并成立" ⇒ 命中率那一半已闭环、不再需要真人补测；本行维持"中"只剩后一半（≤10ms 阈值）悬着，一旦裁定放宽到 ≤20ms 就降为低，裁定保留 ≤10ms 则改列 M1 优化项】** **【同日裁定 (a) 已出：放宽为"中位 ≤20ms 且不得出现 ~100ms 平台" ⇒ 22 个有效读数中位 15.2ms 达标，本行可能性按上面的规则由中降为低；尾部 4 个读数（24.8 / 25.6 / 47.6 / 58.4 ms）写入缓解栏但不构成闸门，两条待裁事项全部落地】** | 高 | ~~**2026-10-14 实测闭环**：`ViewportCommand::Focus` 连按 20 轮 **20/20** 拿到前台焦点（`Visible(false)` 与"移出屏幕"两种隐藏策略下都成立）~~ **【2026-10-15 更正】"两种隐藏策略下都成立"作废**：注入基线里"移出屏幕"那 20 轮的 20/20 是**恒真数据**（探针 19/20 轮在按键前就已持有前台，第 20 轮是创建即持焦），有效命中 0；被证明的只剩 `Visible(false)` 下的注入 20/20 与物理有效 19/19。所以**在生产选定的方案 B 下，`Focus` 能否抢回焦点属零证据**，`AllowSetForegroundWindow`/`AttachThreadInput`/最小化-还原 是否可省同样未证。已确认的机制仍在：winit 0.30.13 的 `focus_window()` 内部实现了防抢焦点 hack（`SendInput VK_LMENU` down/up 后 `SetForegroundWindow`，`window.rs:1537-1576`）。**保留一条偏差**：实测按键由本进程 `SendInput` 注入，物理按键下的命中率需 M4 人工复测 20 次（需求书附录 E E-16）。 **→ 2026-10-15 部分补测**：探针新增 `physical` 模式（不注入、等真人按），物理按 `Ctrl+Alt+Space` ~~21 次 ⇒ 21/21 拿到前台焦点~~ ⇒ **有效 19/19**（剔除 2 次按下时前台已是自己），端到端中位 117.5ms 与注入基线 118.1ms 差 0.6ms ⇒ 注入/物理两条路径实测等价。**但这 19 轮跑在 `Visible(false)`（方案 A）上，而本计划已把方案 A 判为不达标、生产选定方案 B"移出屏幕"** ⇒ ~~"物理按键 × 方案 B"仍空，需补 `winprobe hotkey 20 physical park`，E-16 保持未清零；~~ **【同日 20:41 该补测已由用户按协议执行完毕（每轮显示前先用鼠标点别的应用）：显示键按下 9 次、开局 2 次按下时前台已是 park 窗口自身 `0x3c0d5e`，判为先验成立并剔除，同时当场复现"park 不交还焦点"；有效 7 轮抢焦点 7/7、端到端 min/中位/max = 12.2 / 14.7 / 58.4 ms（`experiments/e02-physical-park-hotkey-report.txt`）⇒ 方案 B 下 "`ViewportCommand::Focus` 单发就够抢回前台" 由零证据升为 7 轮全支持，`AllowSetForegroundWindow`/最小化-还原 是否可省仍未做对照。E-16 未清零的原因换成两条新的：~~① 有效样本 7 < 20，命中率与延迟都撑不起验收口径，需再跑一轮；② 端到端中位 14.7ms 超出需求书 FR-WIN-02 的"中位 ≤10ms"，阈值放宽还是列 M1 优化项，待用户裁决。~~同日 21:21 复跑后：① 单轮有效 15、15/15，两轮合并 22 个有效轮次 22/22 ⇒ 只剩"跨轮合并算不算满 20"这条口径待裁定【同日已裁定：合并成立，E-16 / E-02 记为闭环】；② 两轮合并中位 15.2 ms、22 个读数没有一个 ≤10ms ⇒ 阈值冲突稳定成立，放宽还是列 M1 优化项仍待用户裁决。探针口径另记一笔：本轮报告汇总行打印的端到端 min 是 0.9ms，那是修复前的二进制把先验轮次也计入延迟样本所致，有效轮次真值 min 为 12.2ms；`src/bin/winprobe.rs` 已改为先验轮次不入延迟样本，汇总标签写成"端到端(仅有效轮次)"。】**同日 21:21 的第二轮里，汇总行打印的端到端 min 12.5ms 已与逐轮重算一致 ⇒ 该口径修复经第二次独立采样验证，引用汇总数字不必再手工重算。详见需求书附录 E"E-16 物理按键热键复测"。**同日新增一条同源风险**：park 隐藏不交还焦点（注入 19/19 轮隐藏后前台仍是启动器自身；同日 20:41 物理复测开局 2 轮再次复现）⇒ 若不显式交还，用户会对着一个看不见的窗口打字；该缺陷已列为 FR-WIN-02 硬前置 + E-15 待测。~~**【2026-10-15 E-15 cloak 实测已答：cloak 同样不交还焦点（20/20 先验成立）⇒ 硬前置对 park 与 cloak 两种隐藏手段都成立，交还动作必须自己做，cloak 不自带交还效果】**~~**同批附带修掉一个探针缺陷**：`hotkey_mode` 此前用 `NativeOptions::default()`（含 Vulkan），违反"候选后端必须排除 Vulkan"这条分层约束，2026-10-15 空跑即 `ACCESS_VIOLATION at igvk64.dll`；这也解释了 10-14 那份基线"重试到第 3 次才跑完 20 轮"——它是在随时会崩的后端配置下采的。现已固定 `DX12\|GL`，与生产一致。 |

## 第八章：待实现功能总表（2026-10-22 对照 AHK 版全量盘点）

**这一章是什么**：把 AHK 版 ALTRun（`D:\temp\AltRun参考项目\ALTRun-main-AHK版`，14 个 Provider）与本项目当前代码做一次全量差集，得到的**可筛选 backlog**。编号（A1…G5）是这份表内部的寻址方式，用户按编号点批次。

**取证方式（2026-10-22 本轮）**：AHK 侧逐项读源码给出处；本项目侧对 `src/` 做覆盖式 grep 确认"没有"而不是"文档没写"。两侧结论都记在本章，**其中"本项目现状"那一栏是代码真值，与需求书状态格不一致时以本章为准**。

**底数**：AHK 有 14 个源；本项目实际接进 `plan()` 的只有 **4 类** = 应用程序（种子 + 批量导入 + SendTo）/ 自定义命令 / 计算器 / 网页搜索。【2026-10-22 批次 X + A9 后更正：现量为 **5 类** —— 增加**文件搜索**（`file_plan()`，批次 X），应用清单里又并入了 **Store/MSIX**（批次 A9）⇒ "4 类"是写本章那一刻的真值，按更正保留旧记录；`plan()` 现量在 `src/main.rs:3322`，原来引的 `:3017` 已漂】grep 确认为 0 命中的关键字：`>` 终端、`'` 路径、`?` 速查、`open|find|file|folder`、类型词、`Bookmarks`、`EnumWindows`、`snippet`、剪贴板历史、脚本源、`schema_version`、`.bad`、`hidden`、`AppsFolder`、`runas`、`OpenAs`、`SHObjectProperties`。【2026-10-22 复核：这批关键字里 `'` 路径、`open|find|file|folder`、类型词、`.bad`、`AppsFolder` 现已不是 0 命中（分别随批次 X 与 A9 落地），清单保持原样作历史，查现状请直接 grep】

**规模档口径**：**S** ≤1 天量级 · **M** 2–4 天 · **L** 一周以上；均按单人实现速度、不含真机复测。

> **2026-10-22 排期口径第三次变更（用户正文「非主要功能都延后，将来做成插件的形式」）**：本章不再默认"逐项往下做"。全部条目按 **主干 / 插件 / 打磨** 三档处置，三档清单与新主干顺序见 **8.10**。8.1–8.8 各表**保留原样**（它们是"缺什么"的事实台账，与"先做什么"无关）；某条属于哪一档，以 8.10 为准。

### 8.1 A 组 数据源

| 编号 | 缺口 | AHK 版怎么做 | 本项目现状 | 规模 | 需求书 |
| --- | --- | --- | --- | --- | --- |
| A1 | 文件搜索 + 目录浏览 | 内置索引（scope / depth / exclude / max / refresh）；触发五式：`D:\proj\rep\` 列目录、`'report`、`open\|find\|file report`、`folder bk`、类型词 `doc/pic/video/audio/zip/exe/cad`；`ext=` 显式扩展名 | ~~完全没有（已明写不接 Everything IPC）~~【2026-10-22 **批次 X 已实现**：`src/source/filesearch.rs`（672 行）+ `main.rs` 接线，六式触发（实现把"空格开头"单列，故为六式）、索引范围三个用户目录 / 深度 4 / 上限 3 万 / 30 分钟一轮 / 排除 7 类目录、打分阶梯 100-90-80-60-30 多词取最低、结果上限 30；`Tab` 进目录同批结掉 **B4**。**唯一没做的是本行末那档 `ext=` 显式扩展名**——类型词表已覆盖七类，`ext=pdf` 这种直接指定没有实现，登记为偏差。Everything IPC 那半句仍是产品决策（需求书 1.4），不是遗漏。细节、口径与~~待真机 13 条~~见发布说明第二十二节】；~~真机未验~~【2026-10-22 用户跑完那 13 条：④⑤⑥⑦⑨⑬ 通过，①②③ **崩溃**（同一处 panic：`calc.rs` 的 `split_conversion` 在字节游标上切进多字节字符中间，退格那一下掉出文件模式才暴露，批次 S 起就埋着），⑧ 不通过，⑩ 半条（`未找到「/」` 是按设计，📁／📄 两枚字形仍未答），⑪⑫ 的"通过"作废 ⇒ 三处修复与逐条对号见第二十三节，本行状态改判为**已实现 + 真机跑过一轮、第二轮八条待复跑**】 | **L** →【已完成实现，真机一轮已过、一轮待跑】 | FR-SRC-14 P1 |
| A2 | 系统命令分组 | 约 70 条分 4 组：电源会话（锁屏/休眠/关机 + 确认）、音量与媒体键、Windows 工具、**42 个 `ms-settings:` 页**、文本批量工具 12 项、本程序自身（Reload/About/Log/Quit） | 种子里只有 3 条 `ms-settings`（设置 / 系统更新 / 控制面板）；无电源、无音量媒体、无文本工具。【2026-10-23 **批次 A2 已实现，代码闭环、真机零实测**：`src/source/system.rs::system_seeds()` **84 条**追加进出厂种子（出厂合计 103 条）——自身 10、电源与会话 8（含睡眠 / 休眠）、Windows 工具 23、`ms-settings:` 36、音量与媒体 7。按用户裁定**并进出厂条目**而不是新数据源；真·内置动作走 `ItemKind::Internal` + `internal:<id>`（20 个），`DataSource` 只加了一个默认空的 `rebuild()` 给「重建文件索引」用。`confirm_actions` 默认开、`hidden[]` 存派生 uid 且没有编辑界面。**文本批量工具 12 项按用户裁定整组暂缓**（剪贴板读写，待办见发布说明第三十一节）。本批抓到并修掉三处自有缺陷（Esc 双处理、编辑内置条目静默改类型、出厂关键字 `szys` 撞车）。`cargo test` **155/155**、`cargo check --all-targets` 零警告；`cargo build` 与界面/真机（锁屏、关机确认、隐藏后搜不到三条验收）**未跑**】【2026-10-29 真机补测（发布说明 §六十三）：三条里两条过、① 只到「可达」——搜 `lock` 能命中「锁定屏幕」（命令可达，执行会锁掉当前会话，本轮不执行）；搜 `shutdown` 回车 ⇒「internal: 「关机」执行前确认」、Esc 后「按 Esc 取消，未执行」；「隐藏后搜不到」用代理 A/B（验收原条目 Store 版「设置」不在沙箱清单里，改拿确实存在的「关机」写进 `hidden[]`）⇒ 启动日志「hidden[] 隐藏 1 条」、搜不到 ⇒ `drop_hidden` 端到端成立，验的是机制不是原条目】 | M →【**实现完成 → 2026-10-29 真机补测：两条过、① 只到可达**】 | FR-SRC-17 P1 |
| A3 | 终端命令 | `>` 前缀（不需空格），`>ipconfig /all`；shell 可选 `cmd\|powershell\|pwsh\|wt`；裸 `>` 出用法行 | 无。【2026-10-23 **批次 A3 已实现，真机零实测**：新数据源 `src/source/terminal.rs`，`default_sources()` 里排第一位；判据 `terminal::route()` 只看第 0 个字符（`>` / `>  ` ＝用法行，`>命令` ＝独占一行，前导空格不抢）；三个控制台程序一律 `CREATE_NEW_CONSOLE` + `/k`、`-NoExit`（从终端启动时子进程会继承终端，不显式开新控制台等于把命令塞进用户自己的窗口），`wt` 档不加该标志、交 `wt.exe cmd.exe /k …`；shell 存 `settings.json` 的 `terminal_shell`（稳定名，默认 cmd，陌生名回落），设置页「功能」页四选一、即时生效；命令原文不落日志。**偏差三条**：前缀"可配"没做、shell 只做全局选择、`-Command` 引号未验。`cargo test` **162/162**、零警告；`cargo build` 被用户实例 PID 36060 锁住 ⇒ 新二进制未产出】 | ~~S–M~~ **已实现（2026-10-23）** | FR-SRC-16 P2 |
| A4 | 剪贴板历史 | `clip` / `clip 词` / `clip image` / `Ctrl+Alt+C`；文本·文件·图片三类，上限 200，图片与超长落盘，密码管理器名单不记录，置顶不淘汰，`Enter` 粘回呼出前窗口 | 无 | **L** →【**撤除，不排期**】 | FR-SRC-09 P1 →【2026-10-22 用户裁定不做，见 8.9 第 2 行】 |
| A5 | 片段 + 自动展开 | `snip xxx`，正文全文搜索；`Enter` 粘回原窗口（Clipboard / Type 两模式）；`;keyword` 在任意程序里展开，占位符 `{date}{time}{uuid}{cursor}` | 无 | M + **L**（自动展开要低级键盘钩子，"排除密码框"无可靠判据） | FR-SRC-07/08 P1/P2 |
| A6 | 浏览器书签 | `bm xxx`，读 Chrome/Edge/Brave/Vivaldi 全部 profile 的 `Bookmarks` JSON，30 s 轮询，按 URL 去重；Firefox 不支持 | 无 | M | FR-SRC-12 P2 |
| A7 | 脚本目录 | 扫 `Scripts\` 下 `.ahk/.ps1/.bat/.cmd/.py`，读头 30 行注释元数据 `@altrun.title/keyword/argument/mode`，三种运行模式，改动 2 s 内生效 | 无 | M | FR-SRC-15 P2 |
| A8 | 速查表 | `?` 列 33 条语法与快捷键、`? file` 过滤，按功能启用状态过滤 | ~~无~~【2026-10-23 **批次 A8 已实现 + 真机自查**：新数据源 `src/source/cheatsheet.rs`，注册在 `Terminal` 之前（显式语法不被关键字抢）；判据 `route()` 只看第 0 个字符；条目 **35 条**（语法 9 / 文件 6 / 按键 18 / 窗口 2，比计划的 33 多出 A3 的终端条目与「速查表（本页）」自身），其中 10 条带「打开设置页」；过滤不分大小写、匹配 key/标题/摘要/标签/分组别名；按 `SearchCtx.source_on` 只列开着的源。`Enter` / 数字键 / 点击 ⇒ 程序内帮助弹窗（便携版没有 `docs/`），Esc 走全局分段链。真机抓到两处自有缺陷并修掉：帮助弹窗 520 定宽在默认窗口里被裁边、正文里的 `**粗体**` 原样带星号（已加单测守卫）。`cargo test` **167/167**、零警告、`cargo build` 通过；详情见发布说明第三十四节】 | ~~S~~ **已实现（2026-10-23）** | FR-SRC-18 P3 |
| A9 | Store / MSIX 应用 | ~~`Get-StartApps` 后台取 CSV → `shell:AppsFolder\<AUMID>`，本机实测 196 条~~【2026-10-22 批次 A9 **已落地，与本文写法三处不同**：① 没走 PowerShell / `Get-StartApps`，改用 COM 枚举 `shell:AppsFolder`（`SHCreateItemFromParsingName` → `IShellItem::BindToHandler(BHID_EnumItems)` → `IEnumShellItems`，解析名取 `SIGDN_DESKTOPABSOLUTEPARSING`），零新 crate、零子进程、无 CSV 解析；② 启动不用 `ShellExecute`，改用 `IApplicationActivationManager::ActivateApplication`（AUMID 的官方入口）；③ **数量口径更正**：本机 `shell:AppsFolder` 共 **198** 条，其中 UWP/MSIX **34** 条、Win32 项 **164** 条——"196 条"是早先 Get-StartApps 的全量数，把 Win32 也算进去了，不是 Store 应用数。实现见 `src/source/application.rs:1202`（`uwp_aumid`）/ `:1232`（`activate_uwp`）/ `:1246`（`scan_store_apps`），启动分派 `src/main.rs:973`】 | 无：`.lnk` target 为空串的直接丢弃 ⇒ Store 应用一条进不来（本批已补上） | ~~S–M~~ **已完成（2026-10-22）** | FR-SRC-02 半句 |
| A10 | `DataSource` trait 框架 + 每源开关 | 14 个 Provider 注册表：注册顺序 = 同分次序、`enabled=false` 不初始化也不参与搜索、单源 panic 只记日志跳过 | ~~4 类源硬编在 `plan()` 的匹配链里，trait 未抽~~【2026-10-22 **批次 A10 已实现**：5 个源（自定义命令 / 网页搜索 / 文件搜索 / 计算器 / 应用清单）改为注册表 `default_sources()`（`src/source/mod.rs:164`），**注册顺序即报文次序**；`dispatch()`（`src/source/mod.rs:190`）五段固定为「独占（首个成立者拿走整张列表）→ 头段按序拼 → 普通结果 → 自定义提示行按 uid 去重 → 兜底（头段与结果皆空才问）」；单源 panic 用 `catch_unwind(AssertUnwindSafe(..))` 隔离（只在 `panic="unwind"` 下有效，已写进注释）；开关存 `AppSettings.sources_off: Vec<String>`（存 `SourceId::as_str()` 稳定名，旧配置里不认识的名字不会触发 E1 隔离），设置页新增第 4 页「数据源」（`-Preferences 4`，行与说明文字都从注册表读，加源不用改 UI）；关掉的源**不 init、不 poll、不参与 dispatch**；文件源的 8 个状态字段（两条通道、索引快照、时间戳、在途旗标、目录命中、查询缓存）搬进 `FileSource`（`src/source/filesearch.rs:157`），`main.rs` 用 `use source::{SearchPlan as Plan, SourceRow as HeadRow};` 保住了 5000 行界面代码。**真机自查（自动注入，不是用户手测）**：500ms / 1.2s / 3s 三档节奏共 11 次点击，`settings.json` 每次都切对；把 `files` 写进 `sources_off` 启动，日志无「文件索引：本轮建成」行（修复前仍有 ⇒ 见下句），设置页显示未勾选。`cargo test` **142/142**、`cargo check --all-targets` 与 `cargo build` 零警告。**本批抓到并修掉的缺陷**：`ui()` 的 poll 循环对关掉的源照样调用，而 `FileSource::poll` 顺手会派下一轮索引 ⇒ 开关只停了搜索、后台仍在读盘；修法＝poll 前加 `prefs.source_on(s.id())`（`src/main.rs:3247`）。**与需求书 FR-SRC-01 的写法有三处不同**（trait 方法名不同、`edit_item/delete_item` 不进 trait、`search` 拆成 `take_over/head/fallback/rows`），逐条见需求书该条注记。剩余缺口：D1 其余部分（设置页扩到 15 页）仍未做】【2026-10-23 用户点测九条：六条通过，A2/A5/A6/A7 四条根因已定位并修（撇号、数字键让位、Default 引擎表、exe 目标名），见发布说明第三十节；四条待复测】 | M →【已完成实现；用户点测后修过一轮，2026-10-23 用户复查「全部测试通过」⇒ 本批结清】 | FR-SRC-01 P0 |

### 8.2 B 组 执行与操作

| 编号 | 缺口 | AHK 版怎么做 | 本项目现状 | 规模 | 需求书 |
| --- | --- | --- | --- | --- | --- |
| B1 | 操作面板 | `→`（过滤键盘自重复 + 光标在末尾）或右键「操作」进入动作模式，输入即模糊过滤，`Esc`/`←` 返回并**恢复原输入与选中行** | ~~右键只有三项：编辑 / 置顶 / 删除（`src/main.rs:4109`）~~【2026-10-25 **批次 P1 已实现并沙箱真机验完**：进入两条（`→` 四条件合取 `main.rs:6248`；右键菜单加「操作…」`main.rs:8397`，菜单帧只记意图、帧末再开——`open_panel` 要 `&mut self`）、退出四条（`Esc` / `←` / **空输入时** `Backspace` / **窗口隐藏兜底** `on_hide`）；现场（查询原文 + 选中行 + 光标）整套存进 `PanelState`，恢复走批次 I 那条 `set_query` 通道，所以"退出接着打字"不会插进词中间。**「过滤键盘自重复」做了库内取证 + 真机实测**：不能读 `key_pressed`（egui 把系统自重复也算"本帧按下"），只读原始事件的 `repeat: false`（`take_fresh_key`，`main.rs:1312`），命中后 `retain` 掉目标键那一格、不清空整帧；`repeat` 之所以可靠——该旗标由 egui 自己算（`input_state/mod.rs:405`），egui-winit 0.36.2 丢弃 winit 的那一份（`lib.rs:992`/`1042`）。**「光标在末尾」也是真判据**（`caret_at_end`，比 `chars().count()` 不比字节）。面板标题「关于〈项标题〉的操作」；查询框改作过滤词；`Enter` 执行动作而以**原查询**记学习（实测「字符映射表」`keyword` `zfysb charmap` → `…chr`、`use_count` 0→1，且全仓没有任何条目学到动作名「open/打开」）。隐私口径：查询原文、对象标题都不落日志，只记字符数。计算器 / 终端 / 提示 / 分类四类行**有意拒绝进面板**（当场记一句、现场不动）。**未做**：原文触发列的「可配选中热键」——`→` 与右键两条已够用，"可配"属键位设定那一档（与 B3 同批再议）。速查表 +4 条（共 44 条），状态栏左段按面板态改写（`status_left_text` 新增 `panel: bool`）。三轮真机 51 条断言 ⇒ 见发布说明第四十四节】 | M–L →【已实现，本批结清】 | FR-ACT-03/04 P1 |
| B2 | 动作集扩充 | 以管理员运行（`*runas`）、打开方式（`OpenAs_RunDLL`）、属性（`SHObjectProperties`）、在终端打开、复制路径 / 复制名称、大字显示、粘贴 | ~~只有「打开所在目录 `Ctrl+D`」；`runas` / `OpenAs` / `SHObjectProperties` 全 0 命中~~【2026-10-25 **批次 P1 已实现**：新文件 `src/actions.rs`（753 行、14 单测）出 `ActionId` 15 条 + `Subject` 七态条件表 `actions_for`（**顺序即面板顺序**，「打开」恒第一、「大字显示」恒最后，由单测钉住）；`shell32.dll,OpenAs_RunDLL` 路径不加引号（AHK 同口径）、`SHObjectProperties` 失败回落 `properties` 动词、`ShellExecuteExW` 的 `runas` 动词带 `working_dir`、`SHFileOperationW + FOF_ALLOWUNDO \| FOF_WANTNUKEWARNING` 走系统回收站、`explorer.exe /select,` 定位。**"按构造不给"取代特判表**：`ms-settings:` / `shell:` / `internal:` / 裸命令名因 `is_fs_path` 为假而同时失去属性·所在位置·终端·回收站，Store 应用因 `Subject::StoreApp` 而失去提权·打开方式·属性。**实测**：目录行 9 条 / 文件行 10 条 / 应用条目 8 条；目录的「所在位置」打开父目录并选中（资源管理器窗口标题 `fs - 文件资源管理器`）；回收站条目 254→255（可还原）；删除只到确认框、`Esc` 后清单仍 293 条。**一条被实测推翻的预测**：原判 `SHObjectProperties` 阻塞 UI 线程并准备挪 STA 线程，实测"框还开着、日志已落"⇒ 不改代码。**五条未做**：粘贴 / 粘贴为纯文本 / 全部打开 / 复制・剪切文件 / 复制到（含"当前目录"语义）⇒ ~~共同前提是 B5 的 `Insert` 多标记（P2），本批只把骨架与条件表铺好、这五条是往表里加行。~~【**2026-10-27 批次 E5 收口时更正这一句的"共同前提"**：粘贴缺的不是多标记（多标记是 B5／批次 E4 已交付的），是本程序从来没记过"呼出之前哪个窗口在前台"——当时把这五条捆在一个前提上记，就没有逐条回头看各自缺哪一块。落地情况：① **粘贴已做**（批次 E5，发布说明 §五十二）；② 全部打开 / 复制・剪切 / 复制到三条的**批量形态**随 B5 落地（见本组 B5 行）；③ ~~仍欠的是单行面板里的对应行~~【2026-10-27 批次 E6 补上：单行面板的「复制文件 / 剪切文件」两行，门槛 `on_disk && t.exists` 与位置都照 AHK `_FileActions`（目录行同样给），用的就是批量版那同一对 `ActionId` ⇒ 两种形态在剪贴板上写的东西一模一样；见发布说明 §五十三】、仍欠两格＝「粘贴为纯文本」（**裁定不做**：本程序的粘贴只有 `KEYEVENTF_UNICODE` 逐字符这一条通道，再开一格"纯文本"等于凭空造一个用户看不懂的区别）与"当前目录"语义（那一格要面板知道此刻在浏览哪个目录，批次 R 之后仍然没有那份上下文）。】**「第三方文件管理器不得塞 `/select`」不适用**：`select_in_explorer` 把 path 写死 `explorer.exe`，从不跟随默认管理器。**「网络盘属性不报错」只到按构造**（本机无共享可测）；「在终端打开」「以管理员运行」按用户禁投口径**未自动化、待人手验**。】 | M →【已实现，剩 B5 依赖的五条与两条人手项】 | FR-ACT-04 P1 |
| B3 | 修饰键次级动作 | `Ctrl+Enter` 显示位置 / 粘贴、`Alt+Enter` 复制路径、`Ctrl+C` 复制 | ~~无~~【2026-10-26 **批次 E3-丙 已实现（部分）**：用户同日裁定"两种形态都做、默认快捷键版、键可自定义" ⇒ `Ctrl+Enter` 落地为一颗**窗口内**热键（不 `RegisterHotKey`、不进热键线程、改完立刻写盘生效）。快捷键版：文件 / 文件夹 ⇒ 所在位置（`explorer /select,`）、网址 / 协议串 ⇒ 复制网址、其余行提示按 `→` 看全部；面板版＝同一颗键开 `→` 面板（同一个 `open_panel`）。配置两格 `secondary_action` / `secondary_hotkey`（schema 仍 1），设置页「热键」页两个单选 + 录制新键 + 恢复默认。**本行原文四件事只做了一件半**：「显示位置」做了，「复制链接」（URL 格）做了，~~**「文本 ⇒ 粘贴」没做**（挂 FR-ACT-05 多选）~~【**2026-10-27 批次 E5 已做**，发布说明 §五十二：`PREVIOUS_HWND` 记账 + 五道闸的 `KEYEVENTF_UNICODE` 逐字符注入（就是 AHK `PasteText` 自带的 `PasteMode="Type"` 那一条，不是自创）；快捷键版与面板版共用同一条 `do_paste`，成功收窗、失败一个字都不投且 Toast 报是哪道闸挡的。挂的那句"FR-ACT-05 多选"当时记错了前提，真正的前置是这份"呼出前的窗口"记账】、**「`Alt+Enter` 复制路径」与「`Ctrl+C` 复制」没做**（`Alt+Enter` 现在可以当**自定义键**去跑同一套次级动作，但它不是"复制路径"）**【2026-10-26 批次 B8 补记：这一句里「`Ctrl+C` 复制」已结**——B8 把它做成"复制**这一行等于什么**"（文件 / 目录⇒路径、计算器⇒数字、URL 与协议串⇒整条、应用 / 命令⇒名称），对象是选中行、与"次级动作"那套同源不同格；判据两条：焦点在查询框且选区非空 ⇒ 让位给 egui 复制选区，且这颗键在 egui-winit 被折成 `Event::Copy`、不能按 `Event::Key` 取。~~**「`Alt+Enter` 复制路径」与「文本 ⇒ 粘贴」两句仍未做**（`grep -n "Alt+Enter" src/main.rs` 零命中）~~【**2026-10-27 批次 E6 两句都结**：「文本 ⇒ 粘贴」随 §五十二 落地；`Alt+Enter` 随 §五十三 落地，但做的是**复制这一行**（走与 `Ctrl+C` 同一个复制臂、同一份载荷），不是本行原文措辞里的"复制路径"——本程序 `Ctrl+C` 在文件行上复制的正是路径，两格因此合成一格，**登记为与原文的措辞偏差**。那句"零命中"现在命中五处（`main.rs:2685` `LOCAL_TAKEN` 那格、`:2703` `ALT_ENTER`、`:7862` 接管、`:7925` 执行臂、`:7938` 日志键名）。**上面那句「`Alt+Enter` 现在可以当自定义键去跑同一套次级动作」从此反转**：它进了 `LOCAL_TAKEN` ⇒ 设置页拿它录"次级动作键"会被当场拒绝并保留旧值（真机有这条拒绝原文，`target/e6/launcher.log` 行 151）。**本行原文四件事至此全清**（显示位置／粘贴／`Alt+Enter`／`Ctrl+C`），只剩三条已登记偏差：不猜"复制路径"这一措辞、面板开着时 `Alt+Enter` 就是 `Enter`、三键以上组合送不进窗口（"逐位相等"那条判据只有单测）。】，见发布说明 §四十九】。真机九条、单测 +4，见发布说明 §四十七】【2026-10-25 记一句依赖状态变化：本行括号里的「依赖 B2」**已经解除**——B2 的动作表与 Win32 包装（提权 / 打开方式 / 属性 / 回收站 / 定位 / 复制）已全部落地，`Ctrl+Enter` 与 `Alt+Enter` 现在只是加判据、不再缺底层；本行仍未做，且注意它与刚上线的面板有重叠（面板里已有「所在位置」「复制路径」两条现成动作），做之前要先定"快捷键版"与"面板版"谁是权威】 | S（~~依赖 B2~~【B2 已于 2026-10-25 落地】） | FR-ACT-01 P0 |
| B4 | Tab 进目录 | 文件夹结果 Tab 补成 `路径\` 后浏览该目录内 | ~~Tab 现为补全 / 循环（`tab_mode` 两档），进目录语义待 A1~~【2026-10-22 批次 X **已结**：文件模式的目录行按 `Tab` ⇒ 查询换成「该目录 + `\`」并立即进浏览（`main.rs:3930` 的 `into_dir` 三条件判据）。这一档排在补全判据之前且**不看 `tab_mode`**；`Shift+Tab` 与条目行上的 Tab 语义一字未动。写的是 `HeadRow::File` 而不是"文件夹结果按 `kind` 分派"——文件行不是清单条目、没有 `kind`】 | S（依赖 A1）→【随 A1 完成，真机未验】 | FR-ACT-06 P1 |
| B5 | 多选与批量 | `Insert` 标记，批量：全部打开 / 复制 / 剪切 / 复制全部路径 / 全部加为命令 / 全部进回收站 | ~~无~~【2026-10-26 **批次 E4 已实现**：`Insert` 只吃文件搜索行与 file/folder 条目、标完选中自动下移、名称前加「✓ 」、下一次呼出清空；有标记时 `→` 不看选中行直接进批量面板（`Subject::Batch` 在 `actions_for` 里**早退于条件表之前**，`actions.rs:338`），七条动作顺序即 AHK `ListFor()` 顺序。剪贴板＝`CF_HDROP` + `Preferred DropEffect`（1 复制 / 2 剪切，剪切条目的 `DropEffect` 是原文那一格的落点）；「复制到」走系统文件夹框 + `SHFileOperationW(FO_COPY)`，AHK 的 `MoveTo` **不做**；「全部回收站」有独立确认框（`Esc` 取消且面板保留）并按 成功/失败/取消 三路计数；「全部加入命令」按 `same_target_path` 去重、条目形态与「添加为命令」逐格一致。**未做**：多选区间（`Shift+方向`）与框选；批量编排（"等前一个结束")。单测 266/266（本批 +6）、沙箱真机九条，见发布说明第四十八节】 | M →【已实现】 | FR-ACT-05 P2 |
| B6 | 参数标记 + 参数历史 | `{%p}` / `{%wt}` / `{%wc}` 等四种编码，呼出瞬间抓剪贴板与前台窗口，可编辑下拉 + ~~LRU 50~~ **rank 50（复用不挪位）** | ~~无（只有 `{query}`）~~【2026-10-28 **批次 B6 已实现**（发布说明 §六十）：五个标记落在新文件 `src/source/params.rs`（纯函数，530 行 14 条单测、零磁盘 I/O）——`{%p}` 用户参数 · `{%c}` 呼出瞬间剪贴板 · `{%wd}` / `{%wt}` / `{%wc}` 呼出前那个前台窗口的句柄 / 标题 / 类名；`ParamType` 四档（`ParamType` 的 `none` / `no_encoding` / `url_query` / `utf8_query`，落 `LaunchItem.param_type` 并补进 E2 的 `take_field!` 枚举，漏了会静默丢格）；`needs_prompt` 决定弹框（写了 `{%p}`，或一个标记都没有但编码非空），参数框在 `src/main.rs:11444`、确认那一帧先记历史再启动（`:4339`）；呼出快照 `:4263`（前台为空清三格、前台是自己留上一次、剪贴板每次重读）；历史单独 `param_history.json`（`application.rs:1817` 起，`load_or_quarantine` + `write_atomic` + 读盘 `sanitize`），上限 50，**顺序＝文件先后、复用只加 rank 不挪位**（原文"LRU"按 Delphi `frmParam.pas:82-110` 的实际口径作废）；编辑弹窗新增「编码」下拉 + 标记图例，保存时"有标记 + 编码=无参数"追问一次，名称含 ASCII 逗号拒收，列表行 `*` 前缀只在编码≠无参数时加；高级页补「清空参数历史」（不写备份）。**四处有意偏差**：逐个标记各换各的（Delphi 是 `else if` 只换第一个命中）、编码按 UTF-8 字节（Delphi 编 ANSI/GBK，故 `utf8_query` 与 `url_query` 同果，第四档只为认旧配置）、保留集多数字与 `~`（Delphi 漏数字是 bug）、历史顺序非 LRU。**最后一格「重复 shortcut 提示替换」裁定不新造**：Delphi 的「替换/保留」只属于导入覆盖弹窗字段（`frmShortCut.pas:93-135`，批次 D4 已实现），`ContainShortCutItem` 唯一调用点被注释（`untShortCutMan.pas:447` / `:566`），§4.1.2 明文只有设置页提示 ⇒ 按"设置页提示 + 执行取第一条"收口；真数据 295 条里重名 6 组、重别名 6 组，保存时替换会把存量挨个问一遍。`cargo test` **355/355**、零警告、`cargo build --target-dir target/b6` 21:56:15、exe 比全部源码新；速查表 61 ⇒ 63。**真机八条待点测**（用户实例在跑，按"绝不关用户实例"未起沙箱）~~【2026-10-28 同日补跑：八条全过，见发布说明 §六十一；沙箱 `target/b6`，途中修两条缺陷（`keys_live` 漏 `param_prompt` 的鬼影参数框、`expand_for_launch` 无标记短路导致不并参）】~~】 | ~~M~~ **已实现（2026-10-28，真机八条已跑）** | FR-ACT-07 P1 |
| B7 | 运行标志 | 命令行尾 `@+` 最大化、`@-` 最小化、`@` 隐藏 | ~~无~~【2026-10-29 **批次 B7 已实现**（发布说明 §六十二）：新文件 `src/source/runflags.rs`（纯函数、零 I/O、8 条单测）——`ShowFlag` 四档 + `split()`；`main.rs` 侧落到 `ShellExecuteExW` 的 `nShow`（`shell_execute_verb` 多收一格，Delphi 是把它交给 `ShellExecute` 的最后一个参数，同一位置），**单行启动四条路全认**（回车 / 操作面板「打开」/ 参数框确认 / 弹窗「测试执行」）。**位置这条原文与 Delphi 实码相反**：实码只认前导（`untShortCutMan.pas:171-185` 三处 `Pos(flag, Command) = 1`），原文写尾部并给了 `notepad.exe@` ⇒ 两头都认、一条只剥一个、前导优先。**三条有意偏差**：网址不认（浏览器自己开窗，剥尾巴会啃坏网址）、剥完为空不启动（Toast 直说，不换系统错误框）、批量动作不认（"对这个路径做事"≠"按条目声明的窗口状态启动"）。继承 Delphi 的代价：目标不认 `nCmdShow` 时静默无效。`cargo test` **363/363**、`cargo check --all-targets` 零警告；~~真机未跑（用户实例在跑），四条待点测见发布说明 §六十二~~【2026-10-29 真机补测（§六十三）：四条＋一格全过——`charmap.exe@+` ⇒ `zoomed=True`；`@-` ⇒ `iconic=True`；`@` ⇒ 进程在而主窗 `visible=False`；前导 `@+charmap.exe` 同果；整条只写 `@` ⇒ Toast「运行标志后面没有命令」、主窗不隐藏、无进程、不记统计。靶子用 `charmap.exe` 而非 `notepad.exe`：用户的记事本在前台且有未保存标签】】 | ~~S~~ **已实现（2026-10-29，真机四条＋一格全过）** | FR-ACT-08 P2 |
| B8 | 缺的键位 | `Ctrl+↑↓` 历史、`Ctrl+C` 复制、`Ctrl+L` 大字、`F1–F4`、`Ctrl+Backspace` 删词、滚轮移动选中 | ~~全部无（`F1–F4` 一个没占）~~【2026-10-26 **批次 B8 收口：六格结掉五格**——`Ctrl+L` 大字早在 2026-10-24 FR-UIX-10 就落地了（本行旧文本"全部无"当时漏记它，属记账错误，就地更正不删）；`Ctrl+Backspace` 查实**零代码**（删前一个词是 egui `TextEdit` 自带的 `builder.rs:1395-1408`，本程序没有接管这颗键）；本批新做三格＝`Ctrl+C` 复制这一行（走 `Event::Copy` 不走 `Event::Key`；有选区时让位；复制内容按 `Kind` 取；**偏差＝复制完不关窗**）、`F1`–`F4`（`F1` 关于 / `F2`＝`Ctrl+,` 别名 / `F3`＝`Ctrl+E` 别名 / `F4` **打开配置目录**，附录 A 原文是"打开配置文件"，偏差就地登记）、滚轮移动选中（一格一行、两端夹住不回绕、列表位置交给"滚动跟随选中"、`ScrollArea::scroll_source` 与取键共用同一个 `keys_live` 防两倍位移）。~~**只剩 `Ctrl+↑↓` 历史没做**：前置是 C3 搜索历史，本程序至今没有查询词历史这个数据结构~~【2026-10-26 同日批次 C3 结清：这一格连同它等的那整份数据结构一起做掉了 ⇒ 本行六格**全清**，见下一行 C3 与发布说明 §五十】。裸 `F1–F4` 已登记进 `LOCAL_TAKEN`（`(0, 0x70)`…`(0, 0x73)`）。真机跑这条时抓到并修掉一条**既有**缺陷：面板空过滤词时"空输入的 `Backspace`＝退出面板"那条判据不看修饰键 ⇒ 一次 `Ctrl+Backspace` 既删词又退面板。单测 +3 ⇒ **269/269**、`cargo check --all-targets` 零警告、沙箱 `target/b8/` 真机矩阵（`F1`–`F4` 各一条、`Ctrl+C` 五类行、有选区 / 无选区 A/B 一对、滚轮 13 格逐格计数与预测相等、`Ctrl+Backspace` 三条），见发布说明 §四十九】 | S–M →【已实现，除 `Ctrl+↑↓`】 | FR-ACT-10 P0 |

### 8.3 C 组 查询语法

| 编号 | 缺口 | AHK 版 | 本项目现状 | 规模 | 需求书 |
| --- | --- | --- | --- | --- | --- |
| C1 | 前缀标记路由 | `>` `'` `?` 与「空格进文件模式」 | ~~只有「首词 + 空格」独占一条通道~~【2026-10-22 批次 X 结掉 `'` / 空格 / 类型词 / `open\|find\|file` / `folder` 五档（纯函数 `filesearch::route()`，只看查询串本身），并**多结一档本行没列、需求书 FR-SRC-14 有写的路径形态**（`D:\proj\`、UNC、`%ENV%\`）；剩 `>`（FR-SRC-16 终端）与 `?`（FR-SRC-18 速查表）两档未做 ⇒ 本行**四分之三已结，不是整条**】【2026-10-23 批次 A3 再结一档：`>` 已落地 ⇒ **现只剩 `?` 速查表一档**】【2026-10-23 批次 A8 结掉最后一档：`?` 已落地（`cheatsheet::route()`，同样只看第 0 个字符；前导空格仍归文件模式）⇒ **本行五档全结**】【2026-10-24 批次 G1 再添一档 `@`（条目分类筛选，FR-QRY-11）：判据在纯函数 `application::parse_scope_query()`，同口径只看第 0 个字符，`x@` 与前导空格的 ` @` 都不抢 ⇒ **本行六档**】 | S（依赖 A3/A1/A8）→【**已完成（2026-10-23）**】 | FR-QRY-02 |
| C2 | 通配符 / 正则 | `*` `?` 转正则 | ~~无；本计划 §1.3 对照表那行直写「未」~~【2026-10-28 **批次 C2 已实现**（发布说明 §五十五）：`WildPattern::compile()` 在 `src/matching.rs`——`*`→`.*`、`?`→`.`、用户原文里的 `.*` 先拿 `\u{1}` 护住、其余字符原样进 `regex`（因 `parse()` 早已整串小写，`\D` 会被折成 `\d`，这是 Delphi `LowerCase` 同款副作用，已有单测钉住）；编译结果按查询原文缓存在 `MatchIndex.wild_cache`（单条，够用——搜索永远只看当前这一串）；编译失败 ⇒ `None` ⇒ 整轮走七档普通匹配，不报错；命中固定落 `T_CONTAINS` 档，**不拔档**（`*.exe` 的贪婪 `.*` 会把"后缀过滤"误判成精确命中）；通配模式下多词档与子序列档关闭；名称 / 别名 / 目标名主干三列都参与。**有意偏差两条**：① 不抄 Delphi 的 `Length(KeyWord) > Length(Item.ShortCut)` 前置剪枝（展开后长度变长会误杀 `*calc` 对 4 字 `calc`）；② 长度惩罚用**模式原串**字数而非展开后字数。`learn_alias` 拒收含 `*`/`?` 的词（发布说明同节）。测试 299/299（matching +9、application +1），真机八条在沙箱 `target/c2/sandbox/` 跑完】 | S–M →【已实现】 | FR-QRY-09 |
| C3 | 搜索历史 | `Ctrl+↑↓` 回溯自己敲过的词 | ~~无~~【2026-10-26 批次 C3 已实现：一份 `search_history.json`（顶层字符串数组、最新在前、去重比小写、**按原样存**、首尾空白剥掉）＋ settings 第 20 格 `history_size`（`take_field!` 链现量 20 条；默认 30、取用时夹到 200、`0`＝不再记，纯读不回写）＋窗口内 `Ctrl+↑↓` 取键（面板开着 / 输入框是文件模式 / 有弹窗盖着 ⇒ 让位，与 AHK `RecallHistory` 的 `Mode` 与 `FileMode` 早退同口径）＋通用页滑块＋高级页「清空搜索历史」（**不写备份**，理由明写在界面里：另两个按钮清的是统计，这张表清的是用户亲手敲过的原文）＋速查表 1 条。**记录点＝六个提交点、不等执行结果**（AHK `Knowledge.Record` 先记账再 `_SafeRun` 同口径；挂在 `mark_used` 上会漏掉启动失败那串，而那正是最该找回的）。`Ctrl+↑↓` **不进** `LOCAL_TAKEN`：`HotkeySpec::check` 与 `vk_from_egui_key` 都不认方向键 ⇒ 那颗键既录不成全局呼出键也录不成次级动作键，拦它等于白占一行，这一条是既有单测「表里每一格必须真录得出来」直接挡下来的，不是选择。翻页位置归零两处（用户改字 / 窗口隐藏）。277/277、零警告、`cargo build --target-dir target/c3` 通过；~~**真机九条一条没跑**：单实例互斥体是按会话的命名对象，用户实例在跑 ⇒ 沙箱 exe 只会转发 `Activate` 后自行退出，按"绝不关闭用户正在运行的实例"不碰它，清单见发布说明 §五十】【2026-10-27 三十轮真机补跑结清（发布说明 §五十一）：用户实例已退出 ⇒ 沙箱 `target/c3/debug` 起得来，九条逐条跑完，判据是**日志行号 + 截屏回读**两条（日志不记原文，也不记修饰键，所以⑤那条是"送 `Ctrl+Shift+↑` ⇒ 出回填行 + 框里内容变"这一对，不是子串搜索）。**跑出来的东西不止"通过"**：① 高级页那个滑块在自动化下三条路全静默失败（像素点、像素拖、UIA `Invoke`），可用的是 `Focus` + `Left`/`Right` 逐格挪 ⇒ 正好命中"松手才写"里的 `changed && !primary_down()` 那一臂；② ⑦ 那半句"`>` 前缀不算文件模式"真机没跑（整份日志 `grep -n "终端"` 零命中），「窗口隐藏 ⇒ 归零」那一路一次没触发（`grep -c "窗口隐藏"` = 0），`history_size = 0` 那档没试 ⇒ 三格仍是单测覆盖；③ **抓到一条真缺陷并当场修掉**：`load_json` 把 serde 报错原样落日志 ⇒ 坏文件的**内容**进 `launcher.log`（FR-CFG-01 ⓐ 那条"只记形态不记值"的不变量当时只做完了逐格容错那一路），修法 `serde_shape()` 只抹引号里的字面量、保留形态与行列号 ⇒ 修后复跑同一个探针：`invalid type: string "", expected a sequence at line 1 column 19`，探针串全库 0 命中、`.bad` 里原文完整。`cargo test` **278/278**、零警告、exe 07:02:28 比全部源码新】 | S →【已实现 + 真机九条已跑（三格仍只有单测，见上）】 | FR-QRY-10 |

### 8.4 D 组 设置与条目管理

| 编号 | 缺口 | AHK 版（19 页） | 本项目现状 | 规模 | 需求书 |
| --- | --- | --- | --- | --- | --- |
| D1 | 设置页扩充 | 通用 / 窗口 / 外观 / 数据源开关 / 应用 / 文件搜索 / 文件索引 / 命令 / 片段 / 剪贴板 / 网页搜索 / 计算器 / 脚本 / 热键 / 使用统计 / 高级 | ~~**3 页**（通用、功能、热键），`AppSettings` 只有 7 个字段；「数据源开关」整页不存在 ⇒ 计算器与网页搜索**恒开、无处关**（已登记为已知偏差）~~【2026-10-22 批次 A10 **只交了「数据源开关」这一页**（第 4 页 `-Preferences 4`，行由注册表自动生成，不硬编），`AppSettings` 增到 8 个字段（新增 `sources_off`）⇒ 计算器与网页搜索**不再恒开**。本行列的其余十五页（窗口 / 外观 / 应用 / 文件搜索 / 文件索引 / 命令 / 片段 / 剪贴板 / 网页搜索 / 计算器 / 脚本 / 使用统计 / 高级…）**都没做**，D1 整体仍开放 —— 特别是「网页搜索」那页缺着，引擎表目前仍只能手改 `settings.json`（见 D2 行）。】 | M →【一页已交，其余开放】 | FR-CFG-04 P1 |
| D2 | 引擎表 UI | 引擎增删改 + 图标显示 + 下载站点图标（先 apple-touch-icon 再 favicon，魔数校验、1 MB 上限） | 只能手改 `settings.json`；`WebEngine.icon` 存而不显示；兜底序列里 `files` 那一格是空的 | M →【2026-10-29 批次 D2 已交**增删改**（设置页第 10 页 + `Ctrl+W` + `-Preferences 10`，含「恢复出厂九条」）与**兜底序列可配**（`AppSettings.web_fallback`，出厂仍 `google,files,bing`）；~~只能手改 `settings.json`~~ 这句作废。**仍未做**：图标显示、「下载站点图标」，所以 D2 不结；本行也没有真机证据，见发布说明 §六十四】 | FR-SRC-11 偏差 |
| D3 | 重置学习 / 恢复默认 | `Reset Learned Ranking`、`Restore Defaults`（恢复前写 `.bak`，保留自定义命令 / 热键 / 引擎 / 置顶） | ~~两个入口都没有~~【2026-10-24 **批次 F7 已实现 + 沙箱真机自查**：设置页第 6 页「高级」两个按钮 + 独立确认框（Enter/Esc）；`reset_learning` 保置顶、`restore_defaults` 重置 10 项策略且保留热键/引擎/hidden/自定义条目、`backup_file` 按 `-1/-2` 递增不覆盖；细节见发布说明第三十九节】 | ~~S~~ **已实现（2026-10-24）** | FR-CFG-07/08 P0/P1 |
| D4 | 对话框辅助按钮 | 选文件、选目录、测试执行、拖拽导入、Chrome `--app=` 特例 | ~~四个按钮全无，只能手敲路径~~【2026-10-28 批次 D4 已实现，发布说明 §五十六：弹窗命令行下方一行「选文件… / 选目录… / 测试执行」+ 右侧「也可把 .lnk / 程序 / 文件夹拖进来」提示，热键 `Ctrl+O` / `Ctrl+Shift+O` / `Ctrl+Enter`（仅弹窗内）；命名规则抽成 `src/source/import.rs` 的纯函数并按 Delphi `ExtractShortCutItemFromFileName` 逐支路落地（`.lnk` 解引用、Installer 缓存跳过、`http(s)` 取主机名、剥扩展名、`--app=` 特例），8 条单测钉住；系统命令（`internal:<id>`）那一档整组禁用。三处有意偏差见需求书 FR-CFG-06 行；**真机只验到弹窗渲染三按钮，六项交互待点测**（本轮 Computer Use 被权限层拒绝）；同日插队项把这三颗键写进按钮标签，见 §五十七】【2026-10-29 真机补测（§六十三）：六项里五项过、② 只到一半——三颗键各自触发（`Ctrl+O` / `Ctrl+Shift+O` 弹出本进程的系统选择框、`Ctrl+Enter` 有「弹窗测试执行：已启动」）；测试执行不收窗不记统计；替换询问「替换 / 保留」两支都按预期落；Esc 只收询问；拖放 `.lnk` 解析成目标并填表判「应用」。**② 选择框回填未完成**：本机系统框被 Directory Opus 接管，点击 / 回车 / `BM_CLICK` / `WM_COMMAND IDOK` / `AttachThreadInput`+`SetFocus` / SDK 的 Esc 全部无响应，只有 `PostMessageW(WM_CLOSE)` 有效且程序按取消处理 ⇒ 取消分支有证据、选完交回路径的分支仍无；⑥ 走的是 `import_from_path`，证明不了 `fill_from_pick`】 | M | FR-CFG-06 P1 |
| D5 | 快捷项批量管理 | 绝对 ↔ 相对路径转换（报条数）、失效项校验生成勾选清单批量删 | ~~无~~【2026-10-28 **批次 D5 已实现**（发布说明 §五十八）：设置页新增**第 8 页「批量管理」**，`Ctrl+B` 直达、`Ctrl+R` 重跑四态（两颗键都进 `LOCAL_TAKEN`，否则能被录成全局呼出键、窗口里失聪）。转换逻辑抽成 `src/source/manage.rs` 的纯函数（`is_convertible` / `plan` / `relativize` / `absolutize`，315 行 10 条单测）：`ConvertPlan` 把"真改了几条"与"选中但跳过几条"分开数，目录边界（`Launcher2` 不能被 `Launcher` 吃掉）、正斜杠、大小写、中文目录、往返一字不差、转两次不再变都有证伪实验钉住。页面＝`ScrollArea` 勾选列表（复选框 + 64px 四态状态列 + 名称 + 64 字符截断路径，完整路径挂 hover）+ 工具行（全选 / 清空 / 只选失效项 / 重新校验 / 已勾选 N 条）+ 动作行（转相对 / 转绝对 / 删除选中），两种批量动作都走"这一帧只问、确认那一帧才落盘"的确认框并写清条数。`只选失效项` **只勾 `Not found`**，`Offline` 不算（介质不在 ≠ 条目写错）。**有意偏差**：便携写法用 `%LAUNCHER_DIR%` 而非原文的 `.\`（程序从不 `SetCurrentDirectory`，`.\` 依赖启动时的当前目录；反向转换两种写法都认）；网络路径 `\\` 不跳过而是照常判 `Ok`/`Offline`。**未做**：增/改/排序（仍走主界面与编辑弹窗）、列宽与窗口几何持久化。`cargo test` **321/321**、零警告、`cargo build --target-dir target/u1` 13:32:05、exe 比全部源码新；速查表 58 ⇒ 60。**真机七条待点测**（用户实例在跑、单实例互斥体是会话级的，按"绝不关用户实例"未起沙箱）；真数据往返取证只覆盖"其余 295 条被判不适用"，落在 exe 目录下的真实行 **0 条**，属弱证据，已如实记在发布说明里】【2026-10-29 真机补测（§六十三）：七条全过——`Ctrl+B` 直达、勾选 / 全选 / 清空计数一致、`只选失效项` 勾 13 条（正好 `Not found` 行数，`Offline` 不勾）、清空后再点走 Toast、`Ctrl+R` 在飞只弹 Toast 回来才写「重新校验路径（302 条）」、转相对「1 条 / 跳过 0」与确认框一致且清单仍 302 条、删除只删清单（302 → 301 → 288）、Esc 只收确认框；另更正一处读法：无障碍树四列是「复选框 / 状态 / 名称 / 路径」】 | ~~M~~ **已实现（2026-10-28，真机七条全过）** | FR-ACT-09 P1 |
| D6 | `hidden[]` | 隐藏内置命令 / 隐藏应用（`Ctrl+Del` 语义），可在设置里恢复 | ~~无：只有「删除」，删掉的出厂命令靠设置页那个按钮整体补回~~【2026-10-24 **批次 F7 已实现一半**：出厂命令可隐藏（`Ctrl+Del`，仅空查询）并在「高级 → 隐藏的条目」逐条恢复，孤儿 uid 灰显；**「隐藏应用」（扫描项）仍未做**——扫描项与自定义条目继续走删除，按 `Ctrl+Del` 只给警告 Toast】【2026-10-24 **批次 D6 补完后半**：隐藏对已入库的任何条目开放（出厂 / 扫描导入 / 自定义），新增 `hidden_stash` 完整快照 ⇒ 扫描项能显示原名并恢复、置顶与学习计数一起回来；重新扫描导入前按 `hidden[]` 过滤（隐藏项不复活），显式添加（弹窗 / SendTo）撤销同名隐藏。细节与沙箱七条见发布说明第四十一节；**仍未做**：批量导入对话框里尚未勾选的扫描项无法「隐藏」，老隐藏记录（无快照）恢复仍按种子重建】 | ~~S–M~~ **已实现（2026-10-24，F7 + D6 两批合起来）** | FR-SRC-02/17 |
| D7 | 扫描目录 / 时机配置 | `Folders` / `FileTypes` / `Depth` / `Exclude` / `RefreshMinutes` 全部可改 | ~~硬编码 4 个默认根、深度 3；2026-10-16 用户明确「扫描时机将来要添加配置」~~【2026-10-27 **批次 D7 已实现**：六个旋钮（目录清单 / 深度 / 排除表 / 上限 / 刷新分钟 / 最小字数）进 `AppSettings` 六个新格（`file_folders` / `file_depth` / `file_exclude` / `file_max_entries` / `file_refresh_minutes` / `file_min_query_len`，逐格容错、纯读不回写），`ScanScope` 结构体承载并下探到 `build_index`/`walk`/`route_at`；设置页新增第 5 页「文件搜索」（外观/高级顺延 6/7）——目录与排除表可增删改、「恢复默认排除」整表写回出厂七条、四组数值全部做成预设档位单选（深度 2/4/6/8、上限 1万/3万/10万、刷新 5/15/30/60/180 分钟、最小字数 1/2/3，档位外值照常生效只是界面不选中）；改动生效走新 trait 钩子 `apply_settings`（只对开着的源调，`FileSource::set_scope` 去重后：空闲⇒立即强制重扫，上一轮在飞⇒`scope_pending` 等旧轮回来再补一轮）；缺省值＝批次 X 常量零漂移，有单测 `scope_defaults_are_the_batch_x_constants` 钉住。`FileTypes` 一格原文就登记过「可扩展」词表，仍写死，不在本批。细节见发布说明 §五十四】 | ~~M~~ **已实现（2026-10-27）** | FR-SRC-02 |
| D8 | 使用统计页 | 合计 + 近 30 天柱状 + 按功能次数 | ~~`use_count` / `last_used_ms` 只写不读给用户看（只喂排序）~~【2026-10-28 **批次 D8 已实现**（发布说明 §五十九）：新账本 `usage.json`（`日期 → 桶名 → 次数`，`schema_version: 1`），纯函数落在新文件 `src/source/usage.rs`（505 行、零磁盘 I/O、10 条单测）；七桶＝清单项 / 系统命令 / 计算器 / 网页搜索 / 自定义命令 / 文件与目录 / 终端命令，`as_str()` 只增不改。`trim` **按真实日期裁**（保留 400 天含今天 ⇒ `today−399`，不是"留最新 400 条"；`today` 认不出则一条不删；解析不出的日期键一并删）；日期换算不引 chrono，用 `days_from_civil` / `civil_from_days`。设置页新增**第 9 页「使用统计」**（`src/main.rs:4972` `usage_ui`）+ `Ctrl+T` 直达（已登记 `LOCAL_TAKEN`），四块＝合计 / 近 30 天柱状（连续补 0、逐列 hover）/ 按功能次数（次数 + 条形 + 占比，分母取全部历史合计，未知桶并入「其他」行）/ 按条目前 10（读条目 `use_count`，与功能账本各算各的）；「清除统计」走既有破坏性确认（`Destructive::ClearUsage`），只清 `usage.json` 不备份、不动条目 `use_count`。接线六处＝`mark_used`（清单项 / 系统命令）、自定义命令（**uid 不在 `self.items` 里，必须单独接**，否则永远 0 次）、网页搜索、文件/目录、终端、计算器复制，口径统一为"真的跑成才记"。坏文件照 E1 口径 `.bad` 留证 + 本次按空统计跑并拒写（`usage_blocked`）。**有意偏差**：柱状只画近 30 天而占比按全部历史（同一页两把尺，各行写明）；「按条目前 10」是追加项。`cargo test` **333/333**、零警告、`cargo build --target-dir target/u1` 20:37:03、exe 比全部源码新；速查表 60 ⇒ 61。**真机七条待点测**（用户实例在跑，按"绝不关用户实例"未起沙箱）】【2026-10-29 真机补测（§六十三）：七条过、终端桶一条未跑——`Ctrl+T` / `Ctrl+,` / 互相跳页都灵；页面计数与 `usage.json` 独立对上；六类落桶各有「+1」行而**终端命令桶按 Computer Use 禁令未跑（零证据）**；取消不记有反证（关机确认 Esc 后无统计行）；柱状 30 列、零次日子 hover 报「N 次」、满高柱按窗口内最大值归一；删条目后「按条目前 10」补位而「按功能次数」不变；清除后三块归零、`usage.json` 空表、条目 `use_count` 未动；坏 `usage.json` ⇒ `.bad` 留证（62 字节）+ 按空统计启动 + 此后两次启动始终不写回。**顺带记一条已知双记**：一次自定义命令启动同时 +1 清单项与 +1 自定义命令，合计多算一次，去重与否待用户裁定】 | ~~S–M~~ **已实现（2026-10-28，真机七条过、终端桶未跑）**〔已由用户提交推送 `3fb40e8`（20:54:45）〕 | FR-CFG-04 / FR-SYS-06 P3 |

### 8.5 E 组 数据完整性（本轮新查出，唯一的丢数据路径）

| 编号 | 缺口 | 现状实证（2026-10-22 grep） | 风险 | 规模 | 需求书 |
| --- | --- | --- | --- | --- | --- |
| E1 | 配置损坏恢复 + 原子写 | `save_user_items`（`src/source/application.rs:186`）与 `save_settings`（`:664`）都是 `std::fs::write` 直接覆盖；`load_user_items`（`:174-183`）解析失败 ⇒ 打印「忽略」后**返回空清单**；全仓 `schema_version` / `.bad` / 临时文件改名 **0 命中** | 需求书 FR-CFG-02 明写「改名 `.bad`、默认值启动、界面提示」。**现路径＝清单坏 → 内存当成空清单 → 任何一次新增/置顶/编辑都全量覆盖写盘 → 用户清单被抹掉且无任何提示**【2026-10-22 批次 X **已修**，上面三处行号是查案时的快照、现量已漂。改法三条：① `load_json`（现 `:197`）把"文件不存在"（首次运行 ⇒ 默认值）与"存在但解析失败"（⇒ 留证）分成两条路，`load_user_items` / `load_settings`（现 `:175` / `:711`）返回 `(值, 隔离落点)`；② `quarantine`（`:214`）改名 `xxx.bad`（该位已占依次试 `.bad1`…`.bad9`，改名动不了就退回用内存字节另存副本），**坏原件始终留在盘上**；③ `write_atomic`（`:241`）先写 `{名}.{pid}.tmp` 再 `fs::rename` 顶替，任一步失败原文件一字不动 ⇒ 本行那句"全量覆盖"的通路被切断。上面那句"无任何提示"也已补：日志逐份一条 + 合成一条 Toast（`main.rs` 的 `startup_notice`，等窗口下一次变可见才弹）。**关键在次序**：先留证再启动 ⇒ 即便 `seed_round` 落后触发补齐写回新清单，写的也是新文件、碰不到证据。**仍未做的两桩**：`schema_version` 与 `config.v<N>.backup.json` 全仓**依旧 0 命中**（属 FR-CFG-01 的版本迁移，本批没动）；"类型不符只把那个键降级"未实现（现在整份按损坏隔离），已登记进需求书 FR-CFG-01 注记待裁定。【2026-10-25 批次 E2 结案：这两桩都做了，"待裁定"按**逐键降级**落地；`config.v<N>.backup.json` 那个名字按本程序的两份配置生成为 `settings.v0.backup.json`（`src/` 全仓 grep 证实 `config.*` 从无对应文件，照抄会多造一个谁都不读的文件）。E1 本批立的三条判据一条没退：语法坏照样先留证、`Lost` 照样拒绝写盘、`*.tmp` 照样不残留——新增的 `Value` 中间层由 `invalid_json_still_quarantines_through_the_value_layer` 那条单测钉住"不许把语法错吞掉"。】~~**真机 ⑪⑫⑬ 三条一条没跑**（写坏两份配置 + 看目录里不残留 `*.tmp`），见发布说明第二十二节~~】【2026-10-22 用户跑完：⑬ **通过**；⑪⑫ 回报的是"通过"，但那一轮**作废**——不是恢复没做，是**判坏判错了对象**：调用处那个 `unwrap_or_else` 写法让类型推断把「(内容, 下场)」整个元组当成解析目标 ⇒ 每一份合法配置必然被判坏、每次启动都改名一份，十个 `.bad` 名额被 9 份清单 + 1 份真撕裂件填满，第 10 次名额占满还照常覆盖写。四处修法（`load_or_quarantine` 现 `:232`、`Corrupted` 枚举现 `:199`、毫秒时间戳兜底格、`Lost` 时启动路径拒绝写盘）与两条往返单测见发布说明第二十三节；上面那句"现量已漂"还要再补一次：`load_json` / `quarantine` / `write_atomic` 现量 `:207` / `:249` / `:291`】 | **S** →【已修，~~真机未验~~ 真机跑过一轮：⑬ 过、⑪⑫ 那轮的"过"是假通过并已重做，待用户复跑第二十三节那八条】 | FR-CFG-01/02 P0 |
| E2 | `schema_version` + 迁移 + 备份 | 字段全无，旧文件只靠 `serde(default)` 兜【2026-10-25 **批次 E2 已实现**：读盘改为「字节 → `Value` 树 → 逐格取」，`AppSettings` 17 格、清单每条目 11 格各只兜自己那一格；新增 `SETTINGS_SCHEMA_VERSION = 1` 与一次升一版的 `migrate_settings`，迁移前先把**原始字节**写成 `settings.v0.backup.json`（名额 5 份、绝不覆盖同名旧现场，**留不到证就不迁移也不回写**）；`user_items.json` 顶层非数组、或数组非空却一条都不成形 ⇒ 与语法坏同一条留证路径。行号、两条证伪实验、`config.v<N>` 与 `schema_version` 只上偏好不上清单这两处有意偏差，全部登记在需求书 FR-CFG-01 的 2026-10-25 注记与发布说明第四十五节。验证：`cargo test` **254/254**（本批 +15 条，全走路径参数化的 `_at` 版 ⇒ 一条不碰用户数据目录）、`cargo check --all-targets` 零警告；**真实配置的副本已用真代码跑过**（17 格迁移前后逐项相等、清单 294 条一条不丢、备份与原文件逐字节相同、第二次启动不再产第二份）。~~**程序真机启动未跑**：用户实例 PID 1112 在跑，第二份会命中单实例互斥体并把他的窗口顶出来、且 exe 被锁 ⇒ 按"绝不动用户实例"挂待复测~~】【2026-10-25 22:06 用户关掉实例，`cargo build` 7.10s/7.12s 两轮通过 ⇒ **发布说明第四十五节那六条真机项逐条跑完**（①v0→v1 首次迁移是用户自己那次启动在真实目录里自然发生的：备份与原文件逐字节相同、现文件只多一行版本戳；②再启动不迁第二次；③单格类型不符只兜那一格且日志里搜不到值；④单条 `use_count` 坏只影响那一条，存活数由 `icon loader` 分母给出硬读数 191⇒190；⑤`[1,2]` 走留证、`[]` 不动盘，两条路分开；⑥手删 `confirm_actions` 自动补回、零 `config:` 行、不弹错误）。**抓到并当场修掉两处**：ⓐ 只有清单坏时 Toast 误说"从出厂默认启动"；ⓑ 只有 `settings.json` 坏时 FR-CFG-02 那句界面提示从不出现（根因＝设置被读两遍，第一遍已把坏文件改名隔离 ⇒ 第二遍看到"没有故障"），改为启动路径只读一次并把值 + 故障传进 `LauncherApp::new`；顺带把类型不符降级日志从一次启动两遍收敛成一遍。修复在 `src/main.rs`，实测用截屏回读。】 | ~~将来改字段形态时无版本可判~~【有版本可判了：v0→v1 这条空步子就是为了让后面每一步都有号可对；批次 J 那种塞在自定义 `Deserialize` 里的兼容写法从此退役，改语义要换算的走版本号】 | ~~S–M~~【实现 + 单测 + 副本取证 + **真机六条全部结清**（2026-10-25 22:30），另带出 `src/main.rs` 两处修复；本行不再有待办，将来清单改语义时按同一套 `_at` 骨架补 items 迁移链＝另一条缺口，不在本行】 | FR-CFG-01 P0 |
| E3 | `Corrupted::Lost` 门禁补真 | ~~`Lost`（十格 `.bad` 与毫秒兜底全失败 ⇒ 拒绝写盘）只有代码路径：真机零覆盖、**单测也没有**；六处启动期修补各自吃一个散落的布尔~~【2026-10-26 **批次 E3-甲 已补真**：门禁抽成纯函数 `lost_evidence_labels`（`src/main.rs:1364`，输入两份配置的 `Option<Corrupted>`、输出要停手且点得出名字的文件），`new()` 里一次算出 `writes_blocked`（`:2616`），六处 `if` 全改吃它（`:2679/:2693/:2706/:2721/:2744/:2757`）；新增两条单测——`quarantine_reports_lost_when_the_directory_refuses_writes`（`application.rs:4883`，用 `icacls /deny "*S-1-1-0:(OI)(CI)(WD,AD,DE)"` 造出"读得进、写不出"，**控制断言"新建文件必须失败"** 挡假绿，再断言 `Lost`、原文件逐字节不变、零残留，ACL 由 `Drop` 还原）与 `lost_evidence_blocks_writes_for_either_file`（`main.rs:9729`，四组合，钉住"只停手坏的那一份、`Kept` 不算事故"）。同一轮修掉两处 Toast 缺陷（长文案不换行被裁 ⇒ `set_max_width`；启动期提示 3 秒读不完 ⇒ `TOAST_TTL_STARTUP = 10s`）。**真机**（沙箱 `target/e3b/`，先让 exe 目录 deny 掉使便携判定落空、`LOCALAPPDATA` 指向沙箱、最后才 deny 数据目录、stderr 落在两个被拒目录之外）：两份配置同时 `Lost` ⇒ stderr 两条门禁各记一句，**两个文件逐字节不变、`.bad` 零个**、窗口照常起而 `launcher.log` 建不出来；屏幕文案由截屏回读确认。验证 `cargo test` **256/256**（+2）、`cargo check --all-targets` 零警告。台账见发布说明第四十六节。】 | 产品代码形状未动，本批只加"造得出来 + 测得住 + 看得见"三件套；**仍不可真机覆盖的成因**＝"目录可写但十个名额全占死 + 毫秒兜底也失败"，只有老单测（占名用地）覆盖 | **S** →【已实现，真机与单测双覆盖】 | FR-CFG-01/02 |

### 8.6 F 组 外观与窗口（阶段二，FR-UIX 14 条整域未开工）

| 编号 | 缺口 | AHK 版 | 规模 | 备注 |
| --- | --- | --- | --- | --- |
| F1 | 主题系统 | 跟随深浅色 + 19 内置主题 + 用户 `Themes\*.json`（`Base` 继承）+ 字体 / 字号 / 行高 / 圆角 / 内边距全可配 | **L** | 需求书 FR-UIX 域 |
| F2 | 真透明 | 分层窗 + `Fade` 淡入淡出 | M | **硬前置：后端只留 `Backends::GL` + 恢复 `with_transparent(true)`**（E-18 裁定，见第七章与需求书 E-18） |
| F3 | 位置与呼出策略 | `ShowOn = Mouse\|Primary\|Active`、`RememberPosition`、状态栏、HUD 提示条 | M | 现在只有固定尺寸 + 手动拖动/缩放【2026-10-29 批次 F3 剩余已交付，§六十五：`ShowOn` 三档 + 落点改存工作区千分比 + 老 `last_pos` 启动时一次性折算 + 设置页「外观」页「呼出落点」下拉 + HUD 优先级链补上 `show_on` 这一跳。**状态栏**早在 2026-10-24 批次 F8 交付、**HUD 提示条**同日 FR-UIX-09 交付 ⇒ **本行四件事至此全清**；欠格：HUD 按 `show_on` 选屏那一跳与设置页下拉的人手点击无真机证据，真人拖拽未测】 |
| F4 | 图标精修 | LRU 1500（80% 修剪）、AND 遮罩、图标规格语法 `res:dll,-id` / `ext:.pdf` / `thumb:`、置顶角标 | S–M | ~~`src/source/icon.rs:10` 自陈 LRU 未做；角标现用名称前 `📌`~~【2026-10-29 批次 F4 已交付，§六十六：点名四件事全清——`IconSpec` 六态 ＋ `parse_spec`（畸形前缀回落裸路径）/ `cache_key` 照 AHK `_CacheKey` 分组（`exe`/`lnk`/`ico`/`url`/`appref-ms`/`msc`/`cpl`/`scr` 与无扩展名按整条路径，其余按扩展名）/ `evict_keys` 超 `ICON_CACHE_MAX = 1500` 剪到 `ICON_CACHE_KEEP = 1200` / AND 遮罩只在"整幅 alpha 全 0"时才接、有 alpha 的图标像素与 F4 之前逐字节相同 / 置顶改画橙色三角，`📌` 全仓已无。另补两桩半成品：出厂 103 条里 9 条文件夹与网址恒空槽（新增 `icon_spec_of`，空的兜 `folder:` / `url:`，应用 / 命令 / 文件 / 内置动作不臆造）、22 个内置动作与 36 个设置页在 `SEED_ROUND` 3→4 回填图标源。真机抓到四条**既有缺陷**已修三条（PowerShell 种子混用 `/` ⇒ `SHGetFileInfoW` 返 0，改种子 ＋ 取图标层加 `normalize_path` 救老清单 / 资源管理器与注册表编辑器实住 `%WINDIR%` 根却被拼到 `System32\`，`system32()` 改存在性优先 / 9 条空 `source_path`），第四条「画图在 Win11 已是商店应用、盘上无 `mspaint.exe`」不臆造资源 id、登记为已知限制。`cargo test` **386/386**（+12）、零警告、`cargo build --target-dir target/f4` 约 3m19s；真机六条全过（出厂首屏图标、两条以前恒空的行、齿轮纹理共用、角标含空槽行、重启后角标仍在、`thumb:` 出真实壁纸）。欠格：LRU 修剪与「36 页共用一张纹理」只有单测（沙箱 42 个键离 1500 很远）、「图标源」输入框无人手点过、老清单里已存的两条错路径不自动改写】 |
| F5 | 主界面显示条数（2026-10-24 用户正文追加，**已完成**） | AHK 版没有这一档；本项目按用户口径做成：默认一屏 **10 行**（原 6 行），设置页「通用 → 主界面显示条数」两档 ~~`10 条（默认）` / `全部`~~【2026-10-24 第二轮用户正文「这个10条功能可以参考Delphi的AltRun版本」⇒ 选项面板裁定**默认改成 Delphi 口径**：两档改为 **`全部（默认）` / `10 条`**，缺省 `list_rows` 由 `ten` 改 `all`】、写 `settings.json` 的 `list_rows`；两档都不改窗口大小，「10 条」只列前 10 行（头段占名额）、「全部」不截断靠滚轮。**换主题时窗口高度按新行高重算**（Compact 543 / Modern 663 / Light 643），手动拖过的尺寸只在下一次换主题时被覆盖。Delphi 对照：`frmALTRun.pas:1101` 的 10 只是 `lstShortCut.Height`，`:1104-1107` 窗口高恒 250/230，`untShortCutMan.pas:1039-1081` 结果从不截断、只给前 10 行贴号 ⇒ 对应「全部」 | S | 同批修掉「主界面滚轮滚不动」：根因是列表右侧拖窗手柄铺在 `Order::Background` 的独立 `Area` 上，`ScrollArea` 的 `rect_contains_pointer` 认层不对就吞掉滚轮 ⇒ 手柄改到中央面板同层注册。细节与真机取证见发布说明第三十六节；**未提交**。**【2026-10-24 批次 F6 之后本行的三组数已过期**：输入框抬到下限 44px 使"顶栏 + 上下内边距"这一段变长，且各主题增加量不同（内边距同时进顶与底）⇒ 现为 **Compact 550 / Modern 682 / Light 658**（另见 F6 行的四档数）】 |
| F6 | 界面松紧与顶栏形态（2026-10-24 用户正文追加，**已完成**） | 用户正文两件事：「整体界面高度太高了，目前项与项之间的间距太大了，参考 Delphi 的 AltRun 版本，在它的基础上做一个优化版，输入框大一点，像 ahk 版的 AltRun 的输入框一样，大气一点」+「输入框的新建按钮可以去掉，换成快捷键就可以，或者换成一个三条杠的菜单，点击显示新建按钮和其他设置按钮」。选项面板三条裁定：三档做成可选项**默认甲** / **☰ 菜单 + 快捷键都留** / **标题 + 灰副标题同行**。落地：① `RowDensity` 四档（甲 34/24/行距2 默认、乙 40/28/2、丙 24/16/0、跟随主题）落 `settings.json` 的 `row_density`，在设置页通用页尾部；Light 下窗口高 **438 / 498 / 318 / 658**，四档都保持"一屏正好十行"、不改宽度；② **代价**：主题键 `row_height` / `icon_size` 从此只在「跟随主题」档生效（F1 那两键不失效，但所有权变了）；③ 输入框高下限 `INPUT_MIN_H = 44` + 文字垂直居中，Light `input_font_size` 20 → **22**——AHK 是 `Max(editH, Win.Scale(40))`（`SearchWindow.ahk:96`）+ 20pt（`ThemeManager.ahk:209`），**这两处是有意各高一档**，不是照抄；④ 顶栏「＋ 新建」撤除换 ☰（宽 40、与输入框同高），六项 = 新建 / 批量导入 / 设置 / 速查表 / 打开日志 / 退出，新配 `Ctrl+M`（`LOCAL_TAKEN` 加 `(HKF_CONTROL, 0x4D)`），Esc 分段最前面加"只收菜单"一档，菜单状态不跨隐藏；⑤ `SEARCH_HINT` 改短（22pt 下长串被右缘裁掉），完整键表入口 = ☰ 与 `?`；⑥ 速查表 35 → **37** 条。参照系：Delphi `ItemHeight=16` 且无图标（下限参照）、AHK 54 但行距 0 + 宽 700（上限参照），本项目带图标宽 500 ⇒ 甲取中间 34 | M | 真机抓到并当场修掉**两条只有自动化才会撞到的缺陷**：① 定宽布局把顶栏那条缝吃成 0 宽 ⇒「拖顶栏空白移动窗口」整条静默失效（日志 `搜索页顶栏空白` 零命中），补 `TOP_DRAG_GAP`/`TOP_DRAG_INSET` 并加单测钉住缝宽；② `Popup` 的 `CloseOnClick` 判据是 `pointer.any_click()`（`popup.rs:608`），**辅助功能 Invoke 不置这一位** ⇒ 菜单旗标不落地，下一次 Esc 跑去收菜单、把刚开的对话框留在原地，修法＝动作落地即 `menu_open = false`。`cargo test` **184/184**、零警告、`cargo build` 通过；台账见发布说明第三十七节；**未提交** |
| F7 | 设置页兜底：重置学习 / 恢复默认 / 隐藏条目（D3 + D6，**已完成**） | 上一轮选项面板选定「设置页兜底（推荐）」⇒ 把一直挂着「主干但未排批次」的 D3（FR-CFG-07 P0 / FR-CFG-08 P1）与 D6 的出厂条目那一半并成一批；用户正文「没异议，开工」。落地：① 设置页第 6 页「高级」（`SettingsTab::Advanced`，`-Preferences 6` 可直达）；② 两个破坏性动作各用独立确认框（Enter=执行 / Esc=取消，不碰已真机验过的删除确认链）；③ `reset_learning` 清统计、保置顶；`restore_defaults` 重置 10 项策略字段，保留热键两格 / 网页引擎 / `hidden[]` / 自定义命令 / 置顶 / 计数 / 分类；④ `backup_file` 写 `<name>.bak-YYYYMMDD-HHMMSS`，同名追加 `-1/-2`；⑤ `Ctrl+Del` 隐藏出厂条目（仅空查询；自定义条目只给警告），「高级」页逐条恢复 + 全部恢复，孤儿 uid 灰显不可恢复；⑥ 速查表 39 → **40** 条 | M | 沙箱真机七条全过：隐藏 + 跨重启、单条恢复、非出厂警告、重置学习两次（含备份递增与置顶保留）、恢复默认（十项回厂、热键/引擎保留、窗口 498→438）、`?隐藏` 直达高级页。**一条自查口径事故**：首轮沙箱二进制比源码旧 20 秒 ⇒ `?隐藏` 查不到，`grep -ac` 比对后重建复原——换二进制前先比 mtime。`cargo test` **198/198**（+5 数据层单测）、零警告、`cargo build` 6.86s；台账见发布说明第三十九节；**未提交** |
| F8 | 主界面收尾：状态栏 + 窗口宽度 / 位置 + 淡入 + 目录 Esc（FR-UIX-02 / 07 / 08，**已完成**） | 上一轮选项面板选定「主界面收尾（推荐）」，用户正文「开始实现 F8」。落地：① FR-UIX-08 状态栏：高 38 + 上留 6，只在有结果时出现；左格写目录浏览三档 Esc / 文件模式 / Tab 口径，右格「`Enter` 键帽 + 动作词」（与行内徽标同口径）；② `appearance.width` 360–900、缺省 500（滑块步 10、拖动即时、松手才落盘）+ `remember_position` / `last_pos`（`MonitorFromPoint` 校验、隐藏时记真实位置）；③ `fade` 淡入 120ms cubic ease-out（frame fill / stroke + `set_opacity` 同乘 alpha）；④ 目录浏览 `Esc` 逐级返回（清筛选 → 上一级 → 清空；超出原文，单列备查）；⑤ 外观页「窗口」四控件；⑥ 速查表 40 → **41** 条 | M | 沙箱真机七条全过：动作词四种、关栏 482→438 且占位符变长、宽度 800 即时 + 重启保持、拖窗 (126,96) 重启回原处、淡入连拍抓到半透明中间帧、目录 Esc 三档、零结果不画栏。**明示偏差**：淡出不做（影子窗口与「隐藏不阻塞下一次热键唤起」冲突）、右侧 `→` 键帽不做（无行动作面板）、状态栏高不随行密度缩放。`cargo test` **205/205**（+4）、零警告、`cargo build` 通过；台账见发布说明第四十节；**未提交** |
| D6 | 隐藏应用：扫描项也能隐藏 + 重新扫描不回填（D 组 D6 的后半，**已完成**） | 用户正文「已经提交并推送,继续下一个任务」+ 选项面板「D6 隐藏应用（推荐）」。落地：① `Ctrl+Del` 解除「仅出厂条目」限制（删掉 `is_factory_uid`），空查询下任何已入库条目都能隐藏；② `AppSettings.hidden_stash: Vec<LaunchItem>`（`#[serde(default)]`）存完整快照 ⇒ 扫描项有原名、恢复带置顶与学习计数；③ `hidden_rows` / `restore_hidden` / `restore_all_hidden` 全部接受快照，快照优先、出厂种子兜底；④ 批量导入合并前 `drop_hidden` 过滤（日志「批量导入：N 条在 hidden[] 里，已跳过」）；⑤ 添加弹窗与 SendTo 调 `unhide_uids` 撤销同名隐藏；⑥ 速查表 `Ctrl+Del` 文案改口径（条数不变）。**没有新增快捷键** | S–M | 沙箱真机十条全过：103 出厂 → 导入 142 → 隐藏 HP Smart 后 141（`hidden`/`hidden_stash` 各 1 条）→ 再导入仍 141 未复活 → 高级页显示原名 + 恢复 → 恢复后 142、两表清空、Toast → 重启仍命中；另三条＝自定义条目 `Ctrl+Del` 隐藏成功（原先只给警告）、预置「uid 在 hidden 清单已移除」后重录同一条自动撤销隐藏、自定义条目经高级页恢复。`cargo test` **209/209**（+4）、零警告、`cargo build` 8.31s；台账见发布说明第四十一节；**未提交** |

### 8.7 G 组 系统集成

| 编号 | 缺口 | AHK 版 | 规模 | 备注 |
| --- | --- | --- | --- | --- |
| G1 | 双击 Ctrl/Shift 呼出 + 副热键（**已完成**，2026-10-29） | `DoubleTap = ""\|ctrl\|shift`，400 ms 窗口；副热键 `Alt+R` | M | 本项目只有一条呼出键 + 一条补字符键。**落地**：① 新模块 `src/dbltap.rs`（纯函数，`DblTap::{Off,Ctrl,Shift}` + `tap_decision` + `TapTracker`，12 单测逐格复现 AHK `App.ahk:459-490` 判定表；另加需求书要求的 25ms 最小间隔）；② Win32 层 `WH_KEYBOARD_LL` 只旁听、装热键线程、仅非"关闭"档才装，判到双击走 `HOTKEY_TX` 同一漏斗，取键期不认；③ 第三槽 `extra_hotkey`（出厂 `Alt+R` = `HotkeySpec::EXTRA_DEFAULT`，`Option` 存、可清除），`HOTKEY_EXTRA = 3`，`WM_HOTKEY` 与主键同分支；④ 取键期连副键一起摘/装回；三格互不同组（`check_extra` + 两条反向判据），手改配置撞组时启动路径跳过注册；⑤ 设置页「热键」页新增两张卡（副呼出 = 录制/恢复默认/清除；双击 = 三档单选）+ 键表两行；⑥ 「恢复出厂」不碰这两格；⑦ 速查表 65 → **67** 条；⑧ 顺带修掉 D2 遗留缺陷（`web_fallback` 漏在 `take_field!` 表外 ⇒ 兜底搜索顺序每次读盘被静默改回出厂）。`cargo test` **398/398**、零警告；真机矩阵见发布说明第六十七节（**尚未跑**） |
| G2 | CapsLock 改键 | 按 CapsLock 切中英文 / 按住切换，提权走计划任务 | M | 平台敏感，提权那半不建议照搬 |
| G3 | 选中内容动作面板 | 全局热键伪造 `Ctrl+C` 取选区 → 按 text/url/file 给动作集 | **L** | 会写用户剪贴板 |
| G4 | 开机自启改注册表 | 现与 AHK 同为写 `Startup\*.lnk` | S | **已在待裁定清单里**（发布说明第十二节） |
| G5 | 更新检查 / 多语言 / 日志级别 / 复制系统信息 | scoop + winget 自替换；4 语言 JSON；级别过滤；一键复制 | 各 S–M | FR-SYS-03 剩三项 |

### 8.8 H 组 已裁定不做（登记以免反复讨论）

窗口切换 `w `（2026-10-21 实现后同日撤除）· Everything IPC · 开始菜单快捷方式（绿色软件取向）· 中键执行 · `Alt+L` 重跑最近项 · 自写多音字表 · Firefox 书签 · QuickSwitch / 一键日期改名 / PT Tools 三个扩展域。

### 8.9 排期裁定（2026-10-22 用户正文）

用户原话：「好，将本次功能列表记录一下，然后顺序做 A1/A4/A5/F1/L」。

**解读口径**（写下免得日后两解）：选中的是 **A1 → A4 → A5 → F1** 这四档，`/L` 指的就是我上一条建议里那句「A1/A4/A5/F1 这批 L 级项目」，不另含别的 L 项（G3 不在名单内）。**四档按序做，每档独立成批、不混批**（这条是我提的、用户接受的做法）。

| 顺序 | 批次 | 内容 | 依赖与前置 |
| --- | --- | --- | --- |
| 1 | ~~下一批~~【2026-10-22 **批次 X 已实现**，同日完成】 | **A1 文件搜索 + 目录浏览**（FR-SRC-14） | 做完顺手能结 B4（Tab 进目录）与 C1 的 `'` 那一档；**必须一并结掉 E1**，因为索引缓存同样是"写坏就丢数据"的那条路径 ⇒ **三项如预期一起结掉**：B4 已结（目录行 `Tab` 进目录）、C1 的 `'` / 空格 / 类型词 / `open\|find\|file` / `folder` 五档已结（`>` 与 `?` 仍等 FR-SRC-16/18）、E1 已修（留证 + 原子写）。`cargo test` ~~131/131~~【2026-10-22 真机回记：现量 **134/134** —— 那 131 条里没有一条问过"一份合法配置走真函数读回来会不会被判坏"】、零警告、构建通过；~~**真机 13 条一条没跑**~~【同日用户跑完并逐条回报：④⑤⑥⑦⑨⑬ 通过，①②③ **崩溃**（同一处 panic 在 `calc.rs` 的字节游标切片，批次 S 起埋着、被 `file_plan()` 的早返回挡住），⑧ 不通过，⑩ 半条（📁／📄 字形仍未答），⑪⑫ 的"通过"作废 ⇒ 三处修复与根因见发布说明第二十三节，第二轮八条待复跑】，`ext=` 那一档未做 ⇒ 详见 8.2 A1 行与发布说明第二十二、二十三节 |
| 2 | ~~再下一批~~【2026-10-22 **用户裁定不做**：正文原话「剪贴板历史功能不做，只专注快速启动」（追问图片与落盘位置时两次答「不做」）⇒ FR-SRC-09 整条撤除、不再排期，未写一行代码、未新增依赖】 | ~~**A4 剪贴板历史**（FR-SRC-09）~~ | 原前置（隐私口径）随功能一起作废；若将来重启此项，仍须先定落盘与日志规则 |
| 3 | 再下一批 | **A5 片段 + 自动展开**（FR-SRC-07/08） | 建议拆两步：先做 `snip` 源与粘贴回原窗口（M），`;keyword` 全局展开（L，要低级键盘钩子）单列一批。【2026-10-22 二次改判：**A5 移出主干、归入插件档延后**——用户正文「非主要功能都延后，将来做成插件的形式」；本行不再计入主干排期，见 8.10】 |
| 4 | 最后 | **F1 主题系统**（FR-UIX 域） | 属阶段二；**开工「窗口外观」前必须先改后端为只留 `Backends::GL` + 恢复 `with_transparent(true)`**（E-18 硬前置）；F2/F3/F4 不在本次名单。【2026-10-22：F1 仍留在主干，但前面插入了 A10 → A2 → A3 → A8 四项，见 8.10 的顺序表】 |

**没选中的不等于做完**：A2/A3/B2/E1/D1 等仍按本章原样挂着，E1 只因为"被 A1 顺带要求"才提到第一批，别读成"E1 已闭环"。【2026-10-22 批次 X 后更新：这句里的 **E1 现在确实已经实现闭环**（留证 + 原子写 + 提示三条都在代码里），但~~**真机三条验收一条没跑**~~【2026-10-22：跑过了——⑬ 通过，⑪⑫ 那两轮的"通过"**作废**，因为被验的那两份文件本来没坏，是我的解析形状（`unwrap_or_else` 把「(内容, 下场)」元组当成了 `T`）在判坏；已按"调用处逐格构造 + `Corrupted` 枚举 + 时间戳兜底格 + `Lost` 时拒绝写盘"四处重做，并补两条往返单测 ⇒ 见发布说明第二十三节】"闭环"到"验收过"之间那一步现在补上了一半（代码闭环 + 真机重做完毕，第二轮八条待用户复跑）；A2/A3/B2/D1 仍按本章原样挂着，与本批无关。**下一批按本表顺序是 A4 剪贴板历史**，开工前先定隐私口径（历史正文含密码级内容，落盘与日志规则必须先写清）。【2026-10-22 改判：**A4 已按用户裁定撤除**（「剪贴板历史功能不做，只专注快速启动」）⇒ 下一批从本表第 3 行起算；A5 片段是否仍算"快速启动"待用户点名，未点名之前不动手。】【2026-10-22 三度改判：A5 已点名——**归入插件档延后**；本表第 3、4 行的"下一批"次序随之作废，现行顺序见 8.10】】

### 8.10 主干 / 插件 / 打磨三档裁定（2026-10-22 用户正文）

用户原话：「**非主要功能都延后，将来做成插件的形式**」。追问「哪三项仍算主要功能」时，用户把 **A2 系统命令分组 / A3 终端命令 `>` / A8 速查表 `?`** 三项全勾为**留在主干**；追问 A10 排期时选 **「提前做」**（理由：`DataSource` trait 就是插件的插槽）。

**三档口径**

| 档 | 含义 | 处置 |
| --- | --- | --- |
| **主干** | 缺了就不叫启动器的能力；插件体系本身（A10）也在这一档 | 按下方顺序做，每项独立成批 |
| **插件** | 本质是"多一个数据源"，可以拆出主程序之外单独开关 | **延后**；将来以插件形式提供，主程序不为此改匹配链 |
| **打磨** | 主程序自身的交互/外观细化，拆成插件没有意义 | 延后，不做插件；主线功能收口后再按需挑 |

**主干顺序（2026-10-22 起）**

| 顺序 | 批次 | 内容 | 为什么排在这么前 |
| --- | --- | --- | --- |
| 已完成 | A1 / A9 / B 组已做 / C1 大部分 / D / E1 / G 热键托盘自启 | 见 8.1–8.7 各行的完成注记 | — |
| **1** | **A10** `DataSource` trait 框架 + 每源开关 | ~~4 类源改为注册制~~【实际是 **5 类**（含批次 X 的文件搜索）；2026-10-22 **已完成实现**，细节见 8.1 的 A10 行】；`enabled=false` 不初始化不参与搜索；单源 panic 只记日志跳过；设置页新增「数据源」开关区 | **插件插槽**：不先抽这个，将来每个插件都要改 `plan()`；顺带结掉 D1 里「计算器/网页搜索恒开、无处关」那条已知偏差。**下一步是顺序 2 的 A2**，2026-10-23 用户正文「继续下一个任务」⇒ 已开工 |
| **2** | **A2** 系统命令分组（FR-SRC-17） | 电源/会话、音量与媒体、Windows 工具、42 个 `ms-settings:` 页、~~文本批量工具~~【用户裁定本批暂缓】、本程序自身 | 用户勾为主干；是"打开设置/关机"这类高频目标的唯一来源。**2026-10-23 已实现**：84 条并进种子（出厂 103 条），`cargo test` 155/155、零警告，真机三条验收（锁屏 / 关机弹确认 / 隐藏后搜不到）**待跑** ⇒ 下一条顺序 3 的 A3 可随时开工 |
| **3** | **A3** 终端命令 `>`（FR-SRC-16） | `>` 前缀、shell 四选一、裸 `>` 出用法行 | 用户勾为主干；做完顺带结掉 C1 剩下的 `>` 那一档。**2026-10-23 已实现**：新源 `src/source/terminal.rs` 注册在第一位，设置页「功能」页四选一 shell（`terminal_shell`，默认 cmd、陌生名回落），`cargo test` **162/162**、零警告；**真机零实测**（新窗口是否真开、wt 档、powershell 引号三条未跑）、`cargo build` 被用户实例 PID 36060 锁住 ⇒ 新二进制未产出、未提交。偏差三条（前缀可配不做 / shell 只做全局选择 / `-Command` 引号未验）见发布说明第三十三节 ⇒ 下一条顺序 4 的 A8 可开工。【2026-10-23 补记：用户正文「终端测试没问题」⇒ 真机一条通过（`wt` / `powershell` 两档按自测确认记，未逐档留证据），已由用户提交为 `ad68ca1 终端`】 |
| **4** | **A8** 速查表 `?`（FR-SRC-18） | `?` 列语法与快捷键、`? 词` 过滤 | 用户勾为主干；体量 S，放在 A3 后可以把新语法一起写进表里；结掉 C1 的 `?` 档。**2026-10-23 已实现**：新源 `src/source/cheatsheet.rs` 注册在 `Terminal` 之前，表 **35 条**（语法 9 / 文件 6 / 按键 18 / 窗口 2）、10 条可跳设置页，按 `source_on` 只列开着的源，Enter / 数字键 / 点击打开程序内帮助弹窗；真机自查抓到并修掉两处（弹窗定宽裁边、`**粗体**` 原样出场），`cargo test` **167/167**、零警告、`cargo build` 通过，~~未提交~~【2026-10-24 用户正文「提交push后继续下一任务」⇒ 已提交并推送为 **`39f7f56「速查表」`**（6 文件 +864/−19）】 ⇒ 主干到此处收口，下一条是顺序 5 的 F1 |
| **5** | **F1** 主题系统（FR-UIX） | 19 内置主题 + `Themes\*.json` + `Base` 继承 | 用户此前正文「等主要的功能完成后开始美化 ui」；**硬前置：后端只留 `Backends::GL` + 恢复 `with_transparent(true)`**（E-18）。【A8 已收口 ⇒ 主干前置项全部走完，F1 可开工，但开工第一件事是先做 E-18 那道后端改动】**【2026-10-24 已实现**：E-18 前置先落地（`backends = Backends::GL` + `with_transparent(true)`，真机模块表里只有 `opengl32.dll`）；新文件 `src/theme.rs`（807 行）＝19 套内置 + `System` 选择项 + 用户 `Themes\*.json` 同名覆盖 + `base` 继承（深度 8、自继承改内置）+ 字体 / 字号 / 行高 / 图标 / 内边距 / 选中圆角 / `opacity` 全可配；设置页新增第 5 页「外观」，切换即时生效并写 `settings.json`；`cargo test` **179/179**、`cargo check --all-targets` 零警告、`cargo build` 通过；真机九项自查全过（含不透明度逐像素透明、同名覆盖、坏文件跳过、主题目录全丢仍浅色启动），两处真机缺陷当场修掉（按钮裁边 / 坏文件日志重复）；**未提交**，见发布说明第三十五节】 |

> **2026-10-24 主干收口后的插队项**：用户当日正文追加两件事——「程序主界面无法滚动滚轮」「默认显示 10 条记录，现在显示 6 条，并添加设置，可以配置默认显示 10 条还是显示所有」——已就地做完（不占主干批次，登记为 8.6 的 **F5**）。随后用户正文「这个10条功能可以参考Delphi的AltRun版本」，F5 按 Delphi 口径收口：缺省改「全部」、`10 条` 留作可选截断档、换主题按新行高重算窗口高（真机 543 / 663 / 643 三次已验证）。~~F1 的提交仍等用户正文授权；本插队项与 F1 一起待提交~~【2026-10-24 已提交并推送 `9c5be16 界面美化` + `4bd124e 代码格式化`】。
>
> **同日第二批插队（登记为 8.6 的 F6，已完成）**：用户正文再追加两件事——「整体界面高度太高了，项与项之间的间距太大了，参考 Delphi 版本做一个优化版，输入框大一点、大气一点」+「新建按钮去掉，换成三条杠菜单」。选项面板三条裁定（默认甲 / ☰ + 快捷键都留 / 副标题同行）⇒ 行密度四档、☰ 主菜单 + `Ctrl+M`、输入框 44px 下限 + 22pt。**上面那句「真机 543 / 663 / 643」是 F5 当时的实测值，保留不删；F6 把输入框抬高后同一批数变成 550 / 682 / 658，且默认档已不再是"跟随主题"的 658 而是甲档 438**。~~F1 / F5 / F6 三批一起等用户正文授权提交~~【2026-10-24 已提交并推送 `9c5be16 界面美化` + `4bd124e 代码格式化`】。

> **同日第三批插队（登记为批次 G1，已完成）**：用户正文「参考目前 claude cli 命令行的作法，能否使用它的 models 命令或者概念，将目前 egui 的命令分类…这样分类是不是能更好的提高筛选效率」，讨论后拍板「不添加档位，给命令项添加分类，使用特殊符号筛选某一组命令」+「甲先行，将来我需要再加一个界面显示目前 user_items.json 命令的想法…可以按照分类来查看管理的项目，所以可以预留设计」。选项面板四条裁定（出厂白名单 / 查询前缀语法 + 快捷键循环切档 / 缺省「任何档都恒显」/ 管理界面只预留）⇒ 落地：`LaunchItem.tags` 字段（`#[serde(default)]` 旧清单零迁移，**未标记 = 恒显**）+ 出厂 103 条挂标签（`FACTORY_TAG_BASIC` 24 条「基本」、其余 79 条「维护」，`SEED_ROUND` 2→3 门控 `apply_factory_tags()` 回填、只填空标签）+ `@分类` 前缀（`@维护 @开发 磁盘` 并集、裸 `@` 出总览行，与 `?`/`>` 同口径只看第 0 个字符）+ `default_scope`（all/basic、缺省 all、Ctrl+G 轮换）+ 弹窗「分类」框（v1 唯一标签维护入口）+ `all_tags()` 预留钩子（将来分类管理界面的数据源）+ 速查表 37→39。真机七条自查全过，当场修掉一条缺陷：`uid()` 不含名称 ⇒ 弹窗新增与既有条目「命令行+参数」相同时被 dedup 静默丢弃，补 `survived` 判据 + 红 Toast。192/192、零警告、`cargo build` 通过；**收口后用户又报一障：Shift+2/Shift+3 打不出 @/#（egui `consume_key` 的 `matches_logically` 忽略多余 Shift），已在裸数字分支先消费 SHIFT 层并补单测 ⇒ 193/193**；台账见发布说明第三十八节；~~**未提交**~~【2026-10-24 已提交并推送 `9c5be16 界面美化`】。**编号撞名说明**：8.7 G 组的「G1 双击 Ctrl/Shift 呼出 + 副热键」是本批之前的老编号，与本批无血缘——旧行保留不动，本批在台账里按「批次 G1（条目分类）」称呼。

> **同日第四批（登记为批次 F7，已完成）**：上一轮选项面板选定「设置页兜底（推荐）」⇒ 把本表一直挂着的 D3（FR-CFG-07 / FR-CFG-08）与 D6 的出厂条目那一半并成一批，用户正文「没异议，开工」。落地：设置页第 6 页「高级」（重置学习 / 恢复默认 / 隐藏的条目）、两个独立破坏性确认（Enter/Esc）、备份按 `-1/-2` 递增、`Ctrl+Del` 隐藏出厂条目且仅空查询生效、孤儿 uid 灰显不可恢复、速查表 40 条。沙箱真机七条全过（隐藏跨重启、恢复、警告分支、重置学习两次、恢复默认十项回厂且热键/引擎保留、`?隐藏` 直达高级页）；`cargo test` **198/198**、零警告、`cargo build` 6.86s；台账见发布说明第三十九节；~~**未提交**（F1 / F5 / F6 / G1 / F7 与 Shift 修复一起等用户正文授权）~~【2026-10-24 已提交并推送 `7e1513b 状态栏`（F7 + F8 + 文档）】。
> **同日第五批（登记为批次 F8，已完成）**：上一轮选项面板选定「主界面收尾（推荐）」，用户正文「开始实现 F8」⇒ FR-UIX-08 状态栏（高 38 + 缝 6，只在有结果时出现，左格写目录浏览三档 Esc，右格 Enter 键帽 + 动作词）、FR-UIX-02 的宽度 360–900（缺省 500）与记住位置（`MonitorFromPoint` 校验）、FR-UIX-07 的淡入 120ms cubic ease-out、目录浏览 `Esc` 逐级返回（超出原文，单列备查）、外观页「窗口」四控件。**淡出不做**：原文要影子窗口，「先淡再隐藏」会违反同一条的「隐藏不阻塞下一次热键唤起」。沙箱真机七条全过（含拖窗 (126,96) 重启回原处、淡入连拍中间帧）；`cargo test` **205/205**、零警告、`cargo build` 通过；台账见发布说明第四十节；~~F1 / F5 / F6 / G1 / F7 / F8 一起等用户正文授权提交~~【2026-10-24 用户正文「已经提交并推送,继续下一个任务」后已落库：`7e1513b 状态栏`（F7 + F8 + 文档，7 文件 +711/−27）】。
>
> **同日第六批（登记为批次 D6，已完成）**：用户正文「已经提交并推送,继续下一个任务」+ 选项面板「D6 隐藏应用（推荐）」⇒ 补完 F7 偏差①：`Ctrl+Del` 解除「仅出厂条目」限制（删掉 `is_factory_uid`），扫描导入项与自定义条目同样可隐藏；新增 `AppSettings.hidden_stash` 完整快照（`#[serde(default)]`）⇒ 设置页能显示扫描项原名、恢复时置顶与学习计数一起回来（F7 偏差④收口）；批量导入合并前按 `hidden[]` 过滤（重新扫描不复活），添加弹窗 / SendTo 调 `unhide_uids` 撤销同名隐藏；`drop_hidden` 文档注释从「两处调用」改为四处。**没有新增快捷键**（速查表仍 41 条，只改 `Ctrl+Del` 文案）。沙箱真机十条全过（出厂 103 → 导入 142 → 隐藏 HP Smart 141 → 再导入仍 141 → 高级页原名 + 恢复 → 142、两表清空 → 重启仍命中；另三条：自定义条目 `Ctrl+Del` 隐藏、重录同名条目撤销隐藏、自定义条目恢复）；`cargo test` **209/209**（+4）、零警告、`cargo build` 8.31s；台账见发布说明第四十一节；**未提交**（F7 + D6 两批合起来才算 D6 整条完成）。

**插件档（延后，将来做成插件）**

| 编号 | 条目 | 为什么归插件 |
| --- | --- | --- |
| A5 | 片段 + 自动展开（FR-SRC-07/08） | 独立的文本片段库；自动展开那半还要全局键盘钩子，与主程序主干无关 |
| A6 | 浏览器书签（FR-SRC-12） | 只读第三方应用的本地文件，典型的"外部数据接入" |
| A7 | 脚本目录（FR-SRC-15） | 扫用户自己的 `Scripts\`，按目录约定生成条目，与主程序无耦合 |

> **插件形态未定，不在本次裁定内**：编译期内置 trait 实现 + 配置开关、外部 DLL 动态加载、配置驱动的命令型扩展，三条路各有代价（前者"插件"实则随主程序编译；后者才真正可分发给别人）。**开工插件体系时单独裁定**，现在只确定 A10 是插槽、插件功能一律延后。A10 落地后，接一个插件的最小改动面应当是"加一个实现 + 一行注册 + 一个开关"，**不需要动 `plan()` 的匹配链**——这句话就是将来验收插件体系是否成功的判据。

**打磨档（延后，非插件）**

~~B1 操作面板~~【2026-10-25 批次 P1 已做，发布说明 §四十四】 · ~~B3 修饰键次级动作~~【2026-10-26 批次 E3-丙 已做，§四十七；当时欠的第三格「文本 ⇒ 粘贴」随 2026-10-27 批次 E5 补上，§五十二；最后一格「`Alt+Enter` 复制」随同日批次 E6 补上，§五十三 ⇒ **本行原文四件事至此全清**】 · ~~B5 多选与批量~~【2026-10-26 批次 E4 已做，§四十八】 · ~~B6 参数标记~~【2026-10-28 批次 B6 已做，§六十；五标记 + 四档编码 + 参数框 + `param_history.json`，~~真机八条待点测~~【同日补跑八条全过，§六十一】】 · ~~B7 运行标志~~【2026-10-29 批次 B7 已做，§六十二；`@+`/`@-`/`@` 首尾都认、落到 `nShow`，~~真机四条待点测~~【2026-10-29 真机四条＋一格全过，§六十三】】 · ~~B8 缺的键位~~【2026-10-26 批次 B8 已做（六格结五格，剩 `Ctrl+↑↓` 历史等 C3；同日下一批 C3 补上 ⇒ 六格全清，见 §五十），§四十九】 · ~~C2 通配符/正则~~【2026-10-28 批次 C2 已做，§五十五】 · ~~C3 搜索历史~~【2026-10-26 批次 C3 已做，§五十；~~真机九条待复跑~~【2026-10-27 三十轮已逐条跑完，§五十一】】 · ~~D2 引擎表 UI~~【2026-10-29 批次 D2 已交增删改（设置页第 10 页 + `Ctrl+W` + `-Preferences 10`）与兜底序列可配，§六十四；**真机一次都没跑**，图标显示 / 下载站点图标未做 ⇒ D2 整体未结】 · ~~D4 对话框辅助按钮~~【2026-10-28 批次 D4 已做，§五十六；~~真机六项待点测~~【2026-10-29 五项过、② 选择框回填只到取消分支，§六十三】；同日插队项把七个按钮的键名写进标签，§五十七】 · D5 快捷项批量管理【2026-10-28 批次 D5 已做，§五十八；设置页第 8 页 + `Ctrl+B`/`Ctrl+R`，~~真机七条待点测~~【2026-10-29 真机七条全过，§六十三】】 · ~~D8 使用统计页~~【2026-10-28 批次 D8 已做，§五十九；设置页第 9 页 + `Ctrl+T` + 新账本 `usage.json`，~~真机七条待点测~~【2026-10-29 真机七条过、终端桶未跑，§六十三】】 · F2 真透明 · ~~F3 位置与呼出策略~~【2026-10-29 批次 F3 剩余已做，§六十五；`show_on` 三档 + 千分比落点，真机七条全过】 · ~~F4 图标精修~~【2026-10-29 批次 F4 已做，§六十六；规格语法五路 + AND 遮罩 + 缓存换键与 LRU 1500 + 置顶角标改三角，另修三条既有路径缺陷，真机六条全过】 · G1 双击 Ctrl 呼出 · G2 CapsLock 改键 · G3 选中内容动作面板 · G5 更新检查/多语言/日志级别。

> 其中 **B3 / B8 体量都在 S–M**，且与「每个功能必须配快捷键」的成规直接相关；主线每批收尾时可顺手带一项，不单独排期。**【2026-10-26 收口：这两项都已被主线顺手带完】**——B3 随批次 E3-丙（发布说明 §四十七）、B8 随批次 B8 本身（§四十九，紧接 E4 之后一批），两句"不单独排期"至此兑现；B8 留下的那一格 `Ctrl+↑↓` 查询历史不在快捷键成规内（它等的是一整个 C3 数据结构，不是一颗键）**【2026-10-26 同日续：那一格也随批次 C3 结清**——这颗键与它等的整份数据结构一起做掉了，发布说明 §五十；这句"不是一颗键"的判断当时是对的，正因为对，才没有提前在 B8 里造半套历史】。
> **D1 与 D3 不在此档**：D1 的「数据源开关页」是 A10 的交付物；D3（重置学习 / 恢复默认）在需求书里是 P0/P1、体量 S，仍算主干，但不单独占一批，等 A10 或 F1 收尾时顺手做——**这是本表唯一一处"主干但未排批次"的条目**，写在这里免得日后当成漏项。【2026-10-24 批次 F7 收口：D3 与 D6 的出厂条目那一半已按用户选项面板裁定做成「设置页第 6 页」，见 8.6 的 F7 行与发布说明第三十九节；本段描述保留为当时的排期记录。D6 的「隐藏应用」仍未做】【2026-10-24 批次 D6 已补完「隐藏应用」：隐藏对已入库条目一律开放 + `hidden_stash` 快照 + 重新扫描过滤，见 8.6 的 D6 行与发布说明第四十一节】

 
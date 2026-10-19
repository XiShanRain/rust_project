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
| 开机自启 | 生效实现是往 `CSIDL_COMMON_STARTUP` 写 `.lnk`（注册表 Run 分支代码存在但调用已注释），首次启动询问 | `A_Startup\ALTRun.lnk -Startup` + SendTo + 开始菜单快捷方式，**只删自己创建的** | ~~未开始~~**部分落地（批次 L，2026-10-19）**：开机自启＝设置页「通用」开关 → 写 `%APPDATA%\...\Programs\Startup\egui-launcher.lnk` 带 `-Startup`（**默认关**，2026-10-19 用户裁定，不做"首次启动询问"）；SendTo 已于 2026-10-17 落地；`ChangeWindowMessageFilterEx` 对本消息窗放行 `WM_COPYDATA` 同批落地；**未做＝开始菜单快捷方式**（Windows 搜索找得到本程序那条）。归属判据比参考项目更严：target 与参数两个都要对得上才认领 | N | P2 |
| 单实例 | 命名互斥体 `ALTRUN_MUTEX` + INI 里存 `HandleID` + `SendMessage(WM_ALTRUN_ADD_SHORTCUT/SHOW_WINDOW)`；`ALTRun.exe Restart/Clean/<路径>` CLI 协议 | `#SingleInstance Off` + `WM_COPYDATA`(`0x414C5452`="ALTR") 交接给在跑实例，无应答才强杀；`-Startup/-Reloaded/-Preferences N/-SendTo/-Update` | M1 已落地（互斥体 + `WM_COPYDATA` 2s 超时 + 无应答强杀接管；CLI 见 FR-WIN-08） | N | P1 |
| 剪贴板集成 | 只做单向读：`{%c}` 取剪贴板 + `CTRL+C` 复制命令行 | **剪贴板历史**（文本/文件/图片，`clip` 或 `Ctrl+Alt+C`，上限 200 条、`IgnoreApps` 排除密码管理器、临时借用不写历史） | 未开始 | N | P1 |
| 多语言 | `.lang` 文件（EN/简体中文/繁体中文/PT-BR，各 224 键，8 个 section）+ 首次运行 `frmLang` 选择 | `Resources\Lang\{zh-CN,zh-TW,ja}.json` + `_name/_font/_fallback` 元键 + `auto` 跟随系统 + 单测断言每键都翻译 | 硬编码中文 `"搜索程序..."`/`"未找到匹配的程序"` | N | P2 |
| 日志 | `ExeName.username.log`，双开关（`TraceEnable` + `DEBUG_MODE`，发布版实际关闭） | `%Temp%\ALTRun.log`，`General.SaveLog` 开关，`Perf:` 前缀记耗时，**明确不记用户输入内容** | 未开始 | N | P2 |
| 背景图 / 皮肤 | `BG.jpg`（exe 目录，缺省从内嵌 JPEG 自动导出），关闭可省约 1M 虚拟内存 | 属于主题系统的 `Background` 纯色 + `Opacity`，无位图皮肤 | `with_transparent(true)` 已开但无内容 | D | P3 |
| 正则搜索 | 通配符先转正则再 TRegExpr 匹配，用户可写正则 | 无独立正则模式（用打分匹配 + `Exclude` 正则在配置里） | 未开始 | D | P2 |
| 反馈层（HUD / 大字 / 状态栏提示） | `frmAutoHide` AlphaBlend 淡出提示 + `Popup.wav` 音效 + `HintList[0..24]` 轮换提示 | HUD 气泡（`WS_EX_NOACTIVATE\|TRANSPARENT`，点击穿透，`App.Notify` 1500ms）+ `Ctrl+L` 大字显示 + 状态栏键帽提示 + `?` 速查表 | 未开始 | N | P2/P3 |

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
> **2026-10-14 实测进度**：上述 4 处中 **`.lnk` 解析、`RegisterHotKey` 与 winit 消息循环的接合**已闭环（前者选 `IShellLinkW`，见 4.7；后者选专职 message-only 线程，且实测推翻了"隐藏用 `Visible(false)`"——会把唤起延迟从 1.6ms 抬到 118.1ms（**2026-10-15 更正：右端 118.1ms 成立、左端 1.6ms 作废**：park 那批的收表判据在按下前就已满足，1.6ms 实为唤醒延迟 1.7 / 3.5ms，~~含抢焦点的 park 端到端尚无数据~~ **【同日 20:41 已补出：物理 × park 有效 7 轮端到端 min/中位/max = 12.2 / 14.7 / 58.4 ms、抢焦点 7/7 ⇒ 真实差距约 8 倍而非 74 倍；14.7ms 中位超出需求书 FR-WIN-02 的 ≤10ms 验收阈值，待用户裁决；~~样本 7 < 20，还需再跑一轮~~】** **【同日 21:21 已复跑：单轮有效 15、抢焦点 15/15、端到端 min/中位/max = 12.5 / 15.7 / 47.6 ms；两轮合并 = 22 个有效轮次 22/22、中位 15.2 ms，22 个读数里没有一个 ≤10ms ⇒ 阈值冲突坐实，不能靠多采几轮蒙过去；"跨轮合并算不算满 20 轮"属验收口径，由用户裁定（**同日已裁定：合并成立，FR-WIN-05 的验收措辞改为"累计 ≥20 个有效轮次、可跨多次运行合并" ⇒ E-02 / E-16 闭环，不再补第三轮；阈值那一半同日裁定 (a)：FR-WIN-02 放宽为"中位 ≤20ms 且不得出现 ~100ms 平台" ⇒ 实测中位 15.2ms 达标、冲突解除，尾部 4 个读数（24.8 / 25.6 / 47.6 / 58.4 ms）记录在案但不作闸门 ⇒ 本行两条冲突全部落地**）】**，见需求书附录 E-02/E-16，见第三章 M4 修订项 2 与需求书附录 E-01/E-02）。余 2 处待实测：**Everything-free 文件索引**、**egui 透明窗口的圆角/点击穿透**。另新增一条与排期相关的环境问题：~~本机核显 OpenGL 驱动在 `run_native` 期间随机崩进程（4–8 次/10，与窗口选项无关），详见第七章风险表末段~~ **（2026-10-15 已闭环：不是 OpenGL 而是 Intel Vulkan 驱动，不是随机而是确定性（默认 8/8 崩），根因是 eframe 默认走 wgpu 且 wgpu 优先枚举 Vulkan；限制候选后端为 `DX12|GL` 后本机 5/5 正常，排期阻塞解除，E-15/E-16/E-17 恢复可执行。详见第七章风险表与需求书附录 E 的根因更正节）**。
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
5. **键盘导航**（FR-ACT-10）：↑↓ / PgUp / PgDn / Enter / 双击 / **中键** / 数字键 `1-9 0` / Tab / Esc。**Esc 必须是"清空后隐藏"，不是退出进程**——现 `src/main.rs` 走 `ViewportCommand::Close`，属于行为错误。**【2026-10-17 落地注记（批次 E）】Esc 的行为错误早已改对（三段：弹窗→清空输入→关到托盘，已复测）；本批补齐 `PgUp/PgDn` 整页翻页 + 列表滚动跟随选中项（结果列表改 `ScrollArea`）+ `Tab` 补全选中项名 + `1-9 0`/`Ctrl+数字`/`Alt+数字`/中键执行。注入按键实测：500×400 窗口一页 9 行，PgDn 走 1→10→19、PgUp 回 10，第 19 行截图确认滚到视口中位；`jsq` 下 Tab 补成「计算器」；`Ctrl+1`/`Alt+1`/空输入裸 `1` 均执行第 1 行并 `launch: calc.exe`；非空输入下裸 `2` 落进搜索框（截图 `计算器2`）未被接管。【2026-10-18 用户复测改判：本批初版中键执行的是选中项（~~中键点窗口任意位置即执行选中项~~），改为**指针所指的那一行**，仅当点在非结果行（搜索框/提示条/空白）才兜底执行选中项；实测光标在第 3 行、选中项为第 1 行时执行「记事本」`launch: notepad.exe`，光标在搜索框时执行选中项。行命中带已横向补到列表右边缘】。【2026-10-18 批次 F 再改判：上面那条改判~~（中键执行指针所指行）~~整体作废——用户裁定"启动器以键盘为主，中键改成拖动窗口的功能"，中键现在不执行任何动作；移动窗口改为**左键拖空白**，隐形手柄铺在运行时量出来的三段真空白上（搜索页顶栏「搜索框右缘到＋新建左缘」、列表右侧空白、设置页顶栏「设置」标题右侧）。实现坑两条实测记录在案：① `ViewportCommand::StartDrag` 会把鼠标交给系统拖拽循环并吃掉抬起事件，egui 的左键从此逻辑卡在按下态——用户复测的三个症状（设置页点不动、右键菜单弹不出、第二次拖不动）都是这一个根因；且它只跟左键，中键用它实测零位移（矩形 208,208 → 208,208）。现改为按下时取 Win32 光标与窗口原点做锚点、之后按帧 `SetWindowPos` 跟随，坐标一律物理像素（egui 的 `outer_rect` 要等 winit 回读 `WM_MOVE`，按帧用它算位移实测滞后：注入 +120/+80 只走出 +98/+33，注入 -60/-40 反而走出 +9/+15）。② egui `ScrollArea` 的内容 Ui 由 `ui.new_child(...)` 创建、只覆盖 `max_rect` 不覆盖 `layout`（scroll_area.rs:805-809），会继承外层 `Ui::horizontal` 的 LeftToRight——设置页整块内容因此被挤成一行并溢出右边界，用户看到的"设置页没有空格开关"是这个排版缺陷、不是功能没做。本批同时落地：空格执行（默认仅输入框为空时接管；设置页「通用」的「空格执行选中项」开关打开后任何时候执行，值写 `settings.json`）、删除确认框（`Delete` 与 `Ctrl+Shift+D` 都先弹「确认删除〈名称〉」，起因是用户实测时误删了「控制面板」）、结果列表前 10 行行首标 `1…9、0` 与裸数字键同口径。~~待真人复测：顶栏拖窗手柄两处、设置页排版修复后的空格开关可见性。~~【2026-10-18 复测结论：三项全过——设置页「通用」默认就显示两个开关（不用滚动）、搜索页与列表右侧空白可拖、空格执行开关读写正常、中键确认无任何动作。复测又揪出两个真缺陷并已修完：① 设置页内容区只有约 28px 高，用户症状"要滚鼠标才看到第二个配置项"，根因除了上面那条 layout 继承，更直接的是 `Ui::horizontal` 把行高初值写死成 `spacing().interact_size.y`（ui.rs:2376-2379），在横向行里量 `available_height()` 只有一行高——改为在 TopDown 父 Ui 上量高度、并用 `allocate_ui_with_layout` 给内容区显式分配"剩余宽 × 整列高"；② 设置页顶栏拖不动，根因是手柄右边界取了横向行 `response.rect.max.x`，而它只覆盖**已用宽度**（返回按钮加标题约 110px），算出负宽矩形被守卫静默丢弃——改用 `available_rect_before_wrap()` 的整行右边界。另登记一条现状：设置页右侧内容区被 `ScrollArea` 认领拖拽（0.36 不导出 `drag_to_scroll`，关不掉），背景层手柄抢不到命中，设置页只有顶栏能拖；用户裁定"这样也满足需求"。】本批再追加一项：**拖边缘改窗口大小**（用户 2026-10-18 提"窗口应该允许拖边缘改变大小"；~~待真人复测~~【同日复测 D/E/F/G 四项全过：手柄命中并出双向箭头、缩到最小不再变小且不翻转、拖左缘右缘钉住、`Ctrl+=`/`Ctrl+-` 逐档生效、缩放后拖动与点击/右键/`Enter` 不受影响】）。`with_decorations(false)` 连系统可命中的调整边框一起关了（`HTLEFT`/`HTRIGHT`/`HTTOP`/`HTBOTTOM` 及各角），Windows 不会告诉我们"正在拖边"，所以自绘四边减四角、四角各 6×6 共 8 块 6px 隐形手柄（铺在 `Context::viewport_rect()` 上，本帧最后铺设，同 Background 层后铺者赢），悬停用 `on_hover_cursor` 给双向箭头；动作复用按下取 Win32 锚点、之后按帧 `SetWindowPos` 那条已跑通的路径，拖左/上边钉住对侧，下限 360×240（逻辑像素写、按 `pixels_per_point` 换算后夹）。键盘路径按立规补齐 `Ctrl+=` / `Ctrl+-`（一档 60×40，`ViewportCommand::InnerSize`）；`Ctrl+0` 因与"执行第 10 行"冲突**不做重置键**，登记为偏差。三条偏差已登记在需求书 FR-ACT-01/06/10 落地注记：数字键按结果行而非视口可见行、`;`/`'` 别名不做、~~`Tab` 暂为"补全名称"（FR-ACT-06 的文件夹进目录语义等 FR-SRC-14 落地后按 `kind` 分派）~~【2026-10-18 批次 G 更正：Tab 已改成两种口径可配，进目录语义仍等 FR-SRC-14】】**。 **【2026-10-18 批次 G 落地注记（Tab 兼顾 + 提交即学别名）】**：起因是用户要求先读 Delphi 源码，看 Tab 能否"既选各项又兼其他功能"。读到的事实：Delphi 的 Tab 是纯导航（`actDown`/`actUp`，靠 `KeyPreview` 与 `TAction.SecondaryShortCuts`，到头回绕），↑↓ 在 `FormKeyDown` 里、行内那段 Up/Down 已被注释掉；Delphi 根本没有补全功能；它的"提交即学别名"是 `edtShortCutChange` 事后把查询词写进 `m_FavoriteList` 的别名列。用户随后追加要求：做成设置项，①补全优先 ②专职导航，默认①，代价可接受就照做——评估结论是可接受，故按②的口径实现为可配。落地：`tab_mode`（`complete`/`navigate`）进 `settings.json`，设置页「通用 → Tab 键行为」两个 `selectable_value` 选项；职责分流用**可观测状态**判定而不是时序（不采用"双击间隔"，理由是按需重绘下帧边界不稳定、且无法向用户解释）——原词是选中项名称或别名前缀则补全，否则循环。实测记录见需求书 FR-ACT-06/FR-ACT-10 的批次 G 注记。**三条实现坑记档**：① `consume_key(Modifiers::NONE, Tab)` 会连 Shift+Tab 一起吃掉（它用 `Modifiers::matches_logically` 比对），必须先收 `SHIFT+Tab` 再收 `Tab`；② egui 的 `EventFilter::matches` 对 `Key::Tab` 忽略修饰键，`Focus::begin_pass` 又跑在用户代码之前，所以 Tab/Shift+Tab 必然先把焦点跳走一帧，两个分支都得 `focus_query = true` 抢回；③ 别名学习会让 `keyword` 从单键变成多键，而 `MatchKeys` 原先只存 `alias: Option<Key>`，把 `"jsq qsj"` 当一个键算首字母会得出假分数——改成 `aliases: Vec<Key>`（`split_whitespace` 逐个建键），回归测试 `aliases_split_on_space_so_they_do_not_blend_initials` 钉住。`cargo test` 35 passed，本批新增 `can_complete_needs_name_or_alias_prefix`、`learn_alias_gates_then_appends`、`legacy_settings_without_tab_mode_load`、`commit_word_takes_first_word_and_strips_row_digit` 四条。**【2026-10-18 批次 H 落地注记（Tab 补全改甲案 + `Ctrl+J`/`Ctrl+K`）】**：批次 G 的「原词是选中项名称或别名前缀就补全」被用户复测推翻——`Tas` 出 7 条（Task Manager、Task Scheduler 都在内）时 Tab 补成 Task Manager，列表随即塌成 1 行，第二好的候选再也够不着。根因是判据只看选中项、没看兄弟候选，而列表本身是查询的派生值，补成整名必然塌列表。三案（甲＝无歧义才补／乙＝补到最长公共前缀，bash 口径／丙＝补全后在记住的候选集里循环，zsh menu-complete 口径即用户最初提法）里用户点甲：代价最小、无新状态、不违反本项已立的「分流只看可观测状态」判据（丙要引入按键历史）。落地＝`matching::unique_completion(查询, 清单, 结果集)` 数可补长候选，恰好 1 条才补、≥2 条交给导航；`Shift+Tab`、`navigate` 口径不动。同批补一对恒为导航的 `Ctrl+J`/`Ctrl+K`（用户原点名 `Ctrl+N`/`Ctrl+P`，与「新建」「置顶」冲突，上交后改选空闲键），并顺带纠正需求书附录 A 把 `Ctrl+P/N` 误记为已落地的账。实测（注入 + 回读 stderr + 截图）：`Tas` 连按 Tab 记「循环选中第 2 行（共 7 行）」「第 3 行」，`Ctrl+K` 记「第 2 行」；`task sch`（结果集只剩一条）记「查询补全为「Task Scheduler」（唯一可补全的候选）」。`cargo test` 36 passed，新增 `completion_waits_until_the_candidate_set_is_unambiguous`。两条遗留：补全改 `query` 后 TextEdit 光标停在旧长度处（补完接着打字会插进词中，截图 `Task Sch` 与 `eduler` 之间（光标停在第 8 个字符后））；另查出一条与 Tab 无关的启动路径缺陷（直接可见时不走 `on_show` ⇒ 输入法没切英文、~~焦点没给搜索框~~【2026-10-18 复核更正：焦点那半句作废，`new()` 里 `focus_query: intent.visible && intent.dialog.is_none() && !settings_open`（`src/main.rs:842`）在启动即可见时已为真；缺陷只剩输入法没切英文，症状是字母被中文输入法截走而 `Tab`/方向键仍直达应用】），已单独上报待裁定，实测对照见需求书 FR-ACT-06 批次 H 注记。**【2026-10-18 批次 I 收口：两条都按用户裁定修完并注入实测】**——光标那半：补全那一支同时写 TextEdit 状态（`TextEditState::load` → `cursor.set_char_range(Some(CCursorRange::one(CCursor::new(字符数))))` → `store`），搜索框 widget `Id` 提成 `search_field_id()` 让快捷键分支与 widget 共用，实测 `task sch` 补全后注入 `x` 得 `Task Schedulerx`（修复前 `Task Schxeduler`）；启动那半：`new()` 末尾在「首屏就是搜索页」时补调一次 `on_show()`，判据取 `focus_query` 以免抢走弹窗/设置页的焦点，冷启动日志直接出现 `IME→English: before open=false conv=0x1 → after open=false conv=0x0`，证明 `ImmGetDefaultIMEWnd` 在 `App::new` 阶段就可用、不必推迟到首帧。`cargo test` 36/36 不回归。【2026-10-18 批次 J：`space_to_run` 由 bool 扩成四档 off/word/unique/always，默认 `word`＝查询整串等于高亮行的名称、或等于它别名表里的某个词才执行（前缀阶段空格仍是文本，多词查询照打），`off`/`always` 原样保留旧 `false`/`true` 两个口径、旧 `settings.json` 照读；新增 `Shift+空格` 作"任何时候都插一个真空格"的逃生门，四档下都赢过执行判据。判据集中在纯函数 `matching::space_executes`，入参只有档位、查询、清单、结果集、选中行，不看按键历史；逐档真值表由单测 `space_executes_dispatches_on_observable_state_only` 覆盖，`cargo test` 39/39。**顺带查清一条对标事实**：Delphi 版根本没有空格执行——执行只绑 `FormKeyPress` 的 `#13`，`edtShortCutKeyDown` 全段 case 无 32，全仓搜 `VK_SPACE` 只命中一行被注释掉的 `PostKeyEx32(VK_SPACE, ...)`，数字键也要求带 Ctrl 或 Alt 才执行；故四档是本项目的自定口径，不是照抄参考项目（AHK 版未复核，参考目录在工作区外被权限层拦下）。】
6. **真实启动**（FR-ACT-02）：`ShellExecuteExW` 替换 `std::process::Command`，按 `kind` 分派 file/folder/url/command；工作目录取目标所在目录；**移除 `.expect()`**，失败改为写日志 + UI 提示 + 不 panic。
7. **失焦自动隐藏与可见性控制**（FR-WIN-04/05）：焦点判定用 `ui.input(|i| i.focused)` + 延迟重绘，配置窗口打开时豁免。**隐藏与唤起的命令组合已实测定稿（2026-10-14）**：唤起 = `OuterPosition(回屏内)` + `WindowLevel(AlwaysOnTop)` + `Focus`，隐藏 = `OuterPosition(移出屏幕)`；**不用 `Visible(false)`**（会把唤起延迟抬高约 100ms 的重绘节流；118.1ms 那一侧实测成立，~~从 1.6ms 抬到~~ park 基线 1.6ms 已于 2026-10-15 判为恒真读数、作废，同日 20:41 由物理 × park 有效 7 轮补出真值 min/中位/max = 12.2 / 14.7 / 58.4 ms（见 4.7 与需求书附录 E-01 更正块））。~~`Focus` 的抢占成功率实测 20/20，无需 `AllowSetForegroundWindow`。~~ **【2026-10-15 更正 + 新增实现项】**那 20/20 里"移出屏幕"那半经复测判定为恒真数据（按下时前台本来就是自己，有效 0），所以 ~~`Focus` 在方案 B 下的抢占成功率**尚无数据**、`AllowSetForegroundWindow` 是否可省也**尚未证实**~~ **【同日 20:41 更新：物理 × park 的有效 7 轮里 `ViewportCommand::Focus` 单发、不借助任何辅助手段，7/7 抢到前台 ⇒ 方案 B 下"`Focus` 单独就够"已从待证升为~~有 7 轮支持；但样本未满 20 轮~~【同日 21:21 复跑：单轮 15/15，两轮合并 22/22；样本量口径同日由用户裁定为"累计 ≥20 个有效轮次、可跨多次运行合并" ⇒ 达标，E-16 记为已闭环】，`AllowSetForegroundWindow`/最小化-还原 是否可省仍未做对照，不能宣布已证实】**；并且实测发现 park 隐藏**不交还焦点**（注入 19/19 轮隐藏后前台仍是启动器自身；同日 20:41 的物理复测开局 2 轮再次当场复现）⇒ **本条的实现清单里必须补一步"隐藏时显式交还焦点"**，与 FR-WIN-04 的失焦隐藏合并设计（谁触发、交还给哪个 hwnd），手段选型待 E-15。~~**【2026-10-15 E-15 cloak 实测已答：cloak 同样不交还焦点（20/20 先验成立）⇒ 硬前置对 park 与 cloak 两种隐藏手段都成立，交还动作必须自己做，cloak 不自带交还效果】**~~**同日 20:41 又补出一条与本条直接相关的数字：方案 B 的唤起端到端有效 7 轮 min/中位/max = 12.2 / 14.7 / 58.4 ms ⇒ "~100ms 重绘节流平台"确实被消除（对比方案 A 的 117.5ms，约 8 倍），但 14.7ms 中位仍超出需求书 FR-WIN-02 写的"中位 ≤10ms"验收阈值。~~7 轮里有 4 轮落在 12–15ms，说明达标有空间~~【同日 21:21 复跑后按两轮合并的 22 个有效读数重算：中位 15.2 ms、min 12.2 ms、16 个落在 12.2–17.4 ms、0 个 ≤10 ms ⇒ "达标有空间"要改成"要么放宽阈值、要么把延迟分解后做优化"】，只是这 15 ms 量级（第一轮中位 14.7ms、两轮合并中位 15.2ms）目前还没分解成 `logic` 自轮询（8ms 一轮）、`SetForegroundWindow` 往返、移出屏幕后重新出帧三部分的贡献；~~阈值放宽还是列为 M1 优化项由用户裁决，我不擅自改验收标准~~。** **【同日裁定 (a)：FR-WIN-02 阈值放宽为"中位 ≤20ms 且不得出现 ~100ms 平台" ⇒ 冲突解除，22 个有效读数中位 15.2ms 达标；尾部 4 个（24.8 / 25.6 / 47.6 / 58.4 ms）如实记录但不作闸门，延迟分解不列 M1 硬项】**
8. **CJK 字体回退**（FR-SYS-08，**2026-10-15 已实现并实测**）：`CreationContext` 里取 `FontDefinitions::default()`，把运行时读自 `C:\Windows\Fonts` 的中文字体以 **fallback 追加到 `Proportional` / `Monospace` 各族末尾**（主字体不变 ⇒ Latin 字距不变）；候选 `msyh.ttc`(index 0) → `simhei.ttf` → `STXIHEI.TTF`，全部读不到则保持默认字体、不 panic。见 `src/main.rs` 的 `install_cjk_fallback`。**拆自原 FR-UIX-14**，理由见第二章阶段边界与需求书 8.4 第 6 条。

**验收标准**：从真实开始菜单加载 ≥100 条应用；输入 `jsb` 之外的英文查询可实时过滤且顺序稳定；键盘可完整导航并回车启动；点窗口外部 300ms 内自动隐藏；Esc 只隐藏不退出；启动不存在的路径不崩溃且有提示；占位符与结果标题的中文截图放大核对无 `.notdef` 方块（FR-SYS-08，SYS-08-1）。**图标此里程碑可缺省，但 `ResultItem.icon` 字段必须已存在。** **预估工作量**：5 人天（FR-SYS-08 已实现，不额外增加人天）



### M2：数据源全量（阶段一，10 人天）

**里程碑目标**：19 条 FR-SRC 全部到位，`→` 之前的一切"搜得出"能力成型。

**任务清单**：

1. **数据源框架**（FR-SRC-01）：`trait DataSource { id, init, search(&Query) -> Vec<ResultItem>, empty_results(), edit/delete hooks }` + 注册表（注册顺序 = 同分时的稳定次序）。**容错语义必须照抄 AHK 版：单个数据源 panic/报错只记日志并跳过，绝不中断整次搜索。**
2. **应用源**（FR-SRC-02/03/04）：~~扫 `Folders × FileTypes`（`*.lnk *.exe *.url *.appref-ms`）、深度与 `Exclude` 正则、`.lnk` 解引用（**选型已闭环：`windows` 0.62.2 的 `IShellLinkW` + `IPersistFile`，不引入 `lnk` crate**，详见 4.7 与需求书附录 E-03）、Store 应用（`Get-StartApps` → `shell:AppsFolder\<AppID>`）、`PATH` 目录枚举；索引缓存 `AppIndex.json` + 60min 刷新 + 手动重建。~~ **【2026-10-16 需求变更：改策展按需 —— 内置种子 + 用户清单（`app_index.json` 语义=用户清单，事件写入、无自动重扫）；`.lnk` 解引用选型与约束不变（`IShellLinkW`+`IPersistFile`，附录 E-03），作用域改「解析被导入的单条」；引入渠道 SendTo/添加对话框/无匹配预填/选中快捷键/可选批量导入见 FR-SRC-20，单实例+IPC 见 FR-SRC-21，条目管理见 FR-SRC-22，无匹配兜底运行见 FR-SRC-23；Store 应用 `Get-StartApps`、`PATH` 枚举与 60min 刷新只在「批量导入」档下相关。M1 已落地的扫描/缓存/图标代码复用为「批量导入 + 解析单条」，非丢弃】**M2 开工前置条件已满足，`.lnk` 4 条实测约束仍需在导入路径实现：`%VAR%` 展开、空图标/负数资源号、空 target 丢弃、按扩展名先过滤。**M1 已实现**：`src/source/application.rs`（IShellLinkW 解析 + ExpandEnvironmentStringsW + 空 target 丢弃 + 扩展名过滤 + `app_index.json` 缓存 + `ScanPolicy`）、`src/source/icon.rs`（FR-SRC-03 图标 186/186 实测）；**待实现**：`resolve_one(path)`（SendTo/添加用）、策展清单数据结构与种子、SendTo 引入、单实例+IPC、AddDialog、条目管理、无匹配兜底。
3. **图标**（FR-SRC-03 / FR-SYS-02）：`SHGetFileInfoW`/`ExtractIconExW` → `HICON` → `ColorImage` → `TextureHandle`，**后台线程提取 + 路径→纹理 LRU 缓存（上限 1500）+ 启动后分批预读**，绝不在主线程做。
4. **自定义命令源**（FR-SRC-05/06）：`File\|Folder\|Command\|Url` 四类型 + `Keyword` + `{query}` 独占 + 路径四态校验 `OK / Not found / Offline / -`（`Offline` 必须是盘/网络位置不可达时不误报为"不存在"）。
5. **片段源**（FR-SRC-07/08）：占位符 `{date}`/`{time}`/`{datetime}`/`{clipboard}`/`{clipboard:n}`/`{uuid}`/`{cursor}`/`{date:fmt}`/`{date±N}`；粘贴回呼出前的窗口（保存 `ClipboardAll` → 写剪贴板 → 发送 `^v` → 延迟还原）；`;keyword` 全局自动展开是独立子系统（低层键盘钩子），**列 P2，允许在 M4 末尾降级**。
6. **剪贴板历史源**（FR-SRC-09）：`arboard 3.6` 监听；文本/文件/图片；上限 200、单条 10 万字；`IgnoreApps` 排除密码管理器；**本程序临时借用剪贴板时不得写入历史**。
7. **计算器源**（FR-SRC-10）：表达式（`evalexpr 13` 或 `meval`，需支持 `^`、括号、负数、千分位输出）+ 单位换算 + 进制 + 日期加减；`Enter` 复制**纯数字**，单位写副标题。结构计算与货币换算**已排除**。
8. **网页搜索源**（FR-SRC-11）：引擎表 + `{query}` URL 编码 + `{query|默认值}` + 空词只出补全行 + 无结果兜底序列。
9. **书签源**（FR-SRC-12）：Chrome/Edge/Brave/Vivaldi 的全 profile `Bookmarks` JSON；Firefox 明确不支持（与参考项目一致）。
10. **窗口切换源**（FR-SRC-13）：`EnumWindows` 枚举可见顶层窗口，跳过自身与工具窗；激活前最小化先还原。
11. **文件搜索源**（FR-SRC-14）：**内置索引**（`ScopeFolders` + `Depth` + `ScopeExclude` + `MaxEntries` + 周期刷新）；路径浏览 `BrowsePath`、`'` 前缀、`open|find|file` 关键字、`folder`、类型词 `doc|pic|video|audio|zip|exe|cad`。**不实现 Everything IPC。**
12. **脚本 / 终端 / 系统 / 速查 / 置顶最近源**（FR-SRC-15…19）：脚本目录注释元数据 `@altrun.title/keyword/argument/mode`；终端 `>` 前缀 + Shell 选择；系统命令按**分组**交付（电源、音量与媒体、Windows 工具、设置页 `ms-settings:`、自身命令）；`?` 速查表；空输入框 = 置顶 + 最近。
13. **拼音与学习排序**（FR-QRY-04/06）：`pinyin 0.11` 取首字母（注意 AHK 版用的是 GBK 区间表，存在多音字取舍，需与 crate 结果做对照）；`Boost = 8·ln(1+count)` + 近 7 天 +5 + 该输入或其前缀选过它 ≤+60，**应用侧只把 Boost 加在排名上，避免高频应用挤扁新查询**。【2026-10-18 批次 K 落地注记：这一条已按「只加在排名上」实现为设置页两档，默认甲。`matching::learns_before` 在档位闸门之后比较：甲（`count`）＝次数 → 最近一次；乙（`decay`）＝`log2(1+use_count) × 0.5^(天数/7)` → 最近一次，半衰期常量 `HEAT_HALF_LIFE_DAYS`。与参考式子的三条对照：① `8·ln(1+count)` 与 `log2(1+count)` 在"只用来比大小、不参与加和"的前提下是同序的（差一个常数因子），所以我们取更直观的 `log2`，次数 1→0、3→2、8→3.17；② 参考式子的时间项是"近 7 天 +5"的**台阶加分**，加在次数旁边 ⇒ 一个用了 50 次的项（8·ln51≈31）永远带不动，正是用户这次报的"赖在前面"反过来那半边；我们改成**乘以**半衰期，让老项的整体热度随时间塌下去，这是有意偏离、不是抄错；③ 参考式子第三项"该输入或其前缀选过它 ≤+60"要按查询串存一张历史表，本批未实现——我们已有的"提交即学别名"是它的更强替代：学成别名后下次直接跳到更高档位，而不是同档位内加分。整体不采用「`match_score × 1000 + boost`」的加和写法（早前讨论过的那版公式），因为档位间隔只有 5～70，加和会让次数反过来撬动匹配质量，与 FR-QRY-07 裁定 a 冲突。两档都只在同档位内生效、置顶仍是硬第一键，见需求书 FR-SRC-19 批次 K 注记。】

**验收标准**：14 类数据源全部可被搜到并按分值稳定排序；任一数据源注入异常时其余照常返回；`Enter` 对 file/folder/url/text/command 五类 `kind` 语义正确；拼音首字母、`{query}`、片段占位符、剪贴板粘回、计算器复制、文件类型词逐项通过需求书第 10 章用例。 **预估工作量**：10 人天



### M3：操作面板与参数体系（阶段一，5 人天）

**里程碑目标**：从"搜索结果"升级到"对结果做事"，并补齐 Delphi 版独有的参数机制。

**任务清单**：

1. **操作面板**（FR-ACT-03/04）：`→`（仅"新按"，需过滤键盘自重复）/右键进入 actions 模式；`ActionCatalog` 提供 16+ 动作——打开、以管理员运行（`*runas`）、打开方式（`rundll32 shell32.dll,OpenAs_RunDLL`）、所在位置（`explorer /select,`）、复制路径/名称、在终端打开、属性（`SHObjectProperties`）、移入回收站（`IShellFolder`/`InvokeVerb("delete")`）、大字显示、编辑/删除、置顶/取消置顶、复制、粘贴、全部打开；`Esc`/`←`/`Backspace` 返回结果且不丢已输入文字。
2. **多标记**（FR-ACT-05）：`Insert` 标记 file/folder 行；批量动作 OpenAll / CopyFiles / CutFiles / CopyPaths / CopyTo / MoveTo / 全部加入命令 / 回收站。文件剪贴板写入需 `DropEffect` 语义才能被资源管理器识别为剪切。
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
3. **热键与唤起**（FR-WIN-02/03/06/07/08）：主/次热键注册与重注册（先注销后注册，失败降级托盘唤起并提示）、双击 Ctrl/Shift 呼出、按键抢跑（拿到焦点前输入的字符不丢）、单实例（命名互斥体 + `WM_COPYDATA` 交接，无应答才强杀）、CLI 协议（`-Startup / -SendTo <路径> / -Preferences <页> / -Reloaded`）。**【2026-10-17 状态：FR-WIN-07/08 已落地。互斥体 `Local\XiShan.EguiLauncher.SingleInstance`（探测次实例立刻关句柄）；转发 2s 超时 + 消息窗未建时 20×50ms 重试；无应答 ⇒ 按消息窗类名取 PID → `TerminateProcess` + 等 2s → 重抢互斥体接管（挂起旧实例实测接管成功）；找不到消息窗不猜 PID（降级并记日志，`-Startup` 直接退出）。CLI 解析在 `src/cli.rs`（7 单测）：`-SendTo` 单路径预填弹窗、多路径批量 uid 去重（实测新增 2 / 重复跳过 2）、`-Preferences` 1/2/3 页码映射（参考协议的 8=命令页无对应页 ⇒ 落回通用，偏差已登记）、`-Startup` 静默（eframe 首帧强制显示，用首帧末 `Visible(false)` 盖回，实测保持隐藏）、`-Reloaded` 等互斥体 2s。呼出统一 `ShowWindow(SW_RESTORE)` 兜底：winit 可见标志与实际窗口脱节时单发 `Visible(true)` 不生效。】**
4. **系统集成**（FR-WIN-11 / FR-SYS-01 / FR-WIN-09/10）：托盘与菜单（图标内嵌、`TaskbarCreated` 重建、左键单击呼出）；开机自启 / SendTo / 开始菜单快捷方式（**只删自己创建的**，写前先比对内容避免无谓重写）；窗口落点 `Mouse|Primary|Active` 与记住位置（存工作区千分比而非绝对坐标）；输入法处理（可选切英文键盘或 IME 英文模式、隐藏后还原）。
5. **快捷项批量管理**（FR-CFG-06/07，Delphi 版独有）：列表式增删改排序、**绝对↔相对路径批量转换**（exe 目录内转 `.\`）、**失效项校验并批量删除**（校验前做环境变量与 `.\` 展开，网络路径跳过不误判）；收藏/置顶持久化。
6. **多语言与日志**（FR-SYS-04/03）：`locales/{zh-CN,en}.json` + `_name/_font/_fallback` 元键 + `auto` 跟随系统 + 单测断言键集完整与占位符数量一致；`tracing` 文件+控制台，级别可配，**明确不记录用户输入内容**。
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

1. **状态栏与 ☰ 菜单**（FR-UIX-08）：左侧返回/模式/轮换提示，右侧 `Enter` 行为与 `→` 键帽提示；☰ = 偏好设置 / 速查表 / 重建索引 / 检查更新 / 重载 / 退出。
2. **HUD 与大字显示**（FR-UIX-09/10）：不抢焦点 + 点击穿透的提示气泡（优先贴主窗下方，放不下贴上方，主窗未开时按落点屏的 2/3 高度）；`Ctrl+L` 大字。egui 侧实现方式（独立 `ViewportBuilder` 子视口 vs 覆盖层）**需在 M5 末做技术验证**，穿透 + 不激活的组合在 eframe 里未实测。
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
    3. Store/MSIX 快捷方式的 target 解析出来是**空串**（`IShellLinkW` 与 PowerShell `WScript.Shell` 双路一致）⇒ 空 target 的 `.lnk` 丢弃，Store 应用一律由 `Get-StartApps` 源覆盖（本机实测 196 条）；
    4. 扫描**必须先按扩展名过滤**：目录直接交给 `IPersistFile::Load` 返回 `0x80070005`；单文件解析失败只跳过、不中断扫描。

    参考项目的实质细节保留：`.lnk` 除目标路径外还带 **Arguments 与 WorkingDirectory**，只取路径会丢参数。feature：`Win32_UI_Shell`、`Win32_System_Com`、`Win32_Foundation`。

•    **图标提取**：`SHGetFileInfoW( SHGFI_ICON | SHGFI_LARGEICON|SMALLICON )` 取 `HICON` → `GetIconInfo` → `DrawIconEx` 到 32 位 DIB → 拆 `BGRA` 给 `egui::ColorImage`（原文的 `GetIconInfo 转换为位图数据` 不完整：单靠 `GetIconInfo` 拿不到像素，必须有绘制到 DIB 这一步，且要处理 mask 与 alpha 预乘）。**`DestroyIcon` 必须显式调用**，否则 GDI 句柄泄漏。feature：`Win32_UI_Shell`、`Win32_Graphics_Gdi`。

•    **补：Store 应用**：`shell:AppsFolder\<AUMID>` 启动，AUMID 来自 `Get-StartApps`（PowerShell）——`ShellExecute` 对 `shell:` 动词有效，`std::process::Command` 无效。

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
│   │   └── wildcard.rs         # Delphi 版 `* ?` → 正则
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
│   ├── params.rs               # {%p} {%c} {%wd} {%wt} {%wc} + ParamType 编码 + 历史 LRU
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
| Delphi `Form/frmConfig` / `frmShortCut` / `frmParam` / `frmShortCutMan` / `frmInvalid` | `ui/preferences/` / `ui/preferences/commands.rs` / `params.rs` + 编辑对话框 / `ui/manager.rs` / `ui/manager.rs`(失效清单) |
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
| `regex` | 1.13.1 | `Exclude` / `ScopeExclude` 过滤、Delphi 版通配符转正则 | 编译结果必须缓存（`LruCache` 或 `OnceLock`），不可每帧 compile |
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
| egui 透明窗口下圆角/点击穿透能力不足 | 中 | 中 | **新增（2026-10-14）**。阶段二的 HUD 要求"不抢焦点 + 点击穿透"，eframe 子视口是否支持该组合**未实测**。若不成立，退化方案：HUD 用主视口内的覆盖层绘制（会失去独立窗口位置），或放弃穿透只做到不抢焦点。列为 M6 技术验证项。 |
| tray-icon/muda 与 eframe 主循环共存 | 中 | 中 | **新增（2026-10-14）**。托盘事件回调与 winit 事件循环的分发顺序、以及隐藏主窗口后托盘是否仍能唤醒，需在 M4 早期单独验证（该组合未实测）。备选：退回纯 `NotifyIcon` Win32 自实现。 |
| 内存目标 20MB 不可达 | 高 | 低 | **新增（2026-10-14）**。该指标源自 Delphi VCL，eframe 单窗口进程由渲染上下文主导，无法达到。已在 1.1 修订为 ≤120MB 待实测目标，M6 用实测值收口，避免拿一个不可能的数字做验收。 |
| 拼音实现与参考项目结果不一致 | 中 | 低 | **新增（2026-10-14）**。参考项目用 GBK 码区间表（23 区间，跳过部分字母），`pinyin` crate 用词典法，多音字与生僻字结果会有差异。对策：M2 建一组中文程序名对照用例，据结果决定是沿用 crate 还是自写区间表；不宣称等价。 |
| 抢前台焦点失败导致唤起"看不见" | **⚠ 2026-10-15 风险等级回调为中：~~生产选定的隐藏策略下无数据~~ 同日 20:41 已有 7 轮有效数据（7/7 命中），但样本未满 20 轮、且端到端中位 14.7ms 超出 FR-WIN-02 的 ≤10ms 阈值待裁决 ⇒ ~~维持中~~ **低**（2026-10-15 同日裁定 (a) 放宽阈值后，按本行自述规则降级；命中率两轮合并 22/22，延迟中位 15.2ms 在"中位 ≤20ms 且无 ~100ms 平台"下达标）**（原：已实测、概率降为低）**【同日 21:21 复跑：单轮有效 15、15/15；两轮合并 22 个有效轮次 22/22、中位 15.2 ms、22 个读数无一 ≤10ms ⇒ 命中率的证据已经够看，等级仍维持中的原因只剩两条口径待用户裁定：跨轮合并算不算满 20、≤10ms 阈值放宽还是列 M1 优化项】** **【同日裁定：前者裁定"合并成立" ⇒ 命中率那一半已闭环、不再需要真人补测；本行维持"中"只剩后一半（≤10ms 阈值）悬着，一旦裁定放宽到 ≤20ms 就降为低，裁定保留 ≤10ms 则改列 M1 优化项】** **【同日裁定 (a) 已出：放宽为"中位 ≤20ms 且不得出现 ~100ms 平台" ⇒ 22 个有效读数中位 15.2ms 达标，本行可能性按上面的规则由中降为低；尾部 4 个读数（24.8 / 25.6 / 47.6 / 58.4 ms）写入缓解栏但不构成闸门，两条待裁事项全部落地】** | 高 | ~~**2026-10-14 实测闭环**：`ViewportCommand::Focus` 连按 20 轮 **20/20** 拿到前台焦点（`Visible(false)` 与"移出屏幕"两种隐藏策略下都成立）~~ **【2026-10-15 更正】"两种隐藏策略下都成立"作废**：注入基线里"移出屏幕"那 20 轮的 20/20 是**恒真数据**（探针 19/20 轮在按键前就已持有前台，第 20 轮是创建即持焦），有效命中 0；被证明的只剩 `Visible(false)` 下的注入 20/20 与物理有效 19/19。所以**在生产选定的方案 B 下，`Focus` 能否抢回焦点属零证据**，`AllowSetForegroundWindow`/`AttachThreadInput`/最小化-还原 是否可省同样未证。已确认的机制仍在：winit 0.30.13 的 `focus_window()` 内部实现了防抢焦点 hack（`SendInput VK_LMENU` down/up 后 `SetForegroundWindow`，`window.rs:1537-1576`）。**保留一条偏差**：实测按键由本进程 `SendInput` 注入，物理按键下的命中率需 M4 人工复测 20 次（需求书附录 E E-16）。 **→ 2026-10-15 部分补测**：探针新增 `physical` 模式（不注入、等真人按），物理按 `Ctrl+Alt+Space` ~~21 次 ⇒ 21/21 拿到前台焦点~~ ⇒ **有效 19/19**（剔除 2 次按下时前台已是自己），端到端中位 117.5ms 与注入基线 118.1ms 差 0.6ms ⇒ 注入/物理两条路径实测等价。**但这 19 轮跑在 `Visible(false)`（方案 A）上，而本计划已把方案 A 判为不达标、生产选定方案 B"移出屏幕"** ⇒ ~~"物理按键 × 方案 B"仍空，需补 `winprobe hotkey 20 physical park`，E-16 保持未清零；~~ **【同日 20:41 该补测已由用户按协议执行完毕（每轮显示前先用鼠标点别的应用）：显示键按下 9 次、开局 2 次按下时前台已是 park 窗口自身 `0x3c0d5e`，判为先验成立并剔除，同时当场复现"park 不交还焦点"；有效 7 轮抢焦点 7/7、端到端 min/中位/max = 12.2 / 14.7 / 58.4 ms（`experiments/e02-physical-park-hotkey-report.txt`）⇒ 方案 B 下 "`ViewportCommand::Focus` 单发就够抢回前台" 由零证据升为 7 轮全支持，`AllowSetForegroundWindow`/最小化-还原 是否可省仍未做对照。E-16 未清零的原因换成两条新的：~~① 有效样本 7 < 20，命中率与延迟都撑不起验收口径，需再跑一轮；② 端到端中位 14.7ms 超出需求书 FR-WIN-02 的"中位 ≤10ms"，阈值放宽还是列 M1 优化项，待用户裁决。~~同日 21:21 复跑后：① 单轮有效 15、15/15，两轮合并 22 个有效轮次 22/22 ⇒ 只剩"跨轮合并算不算满 20"这条口径待裁定【同日已裁定：合并成立，E-16 / E-02 记为闭环】；② 两轮合并中位 15.2 ms、22 个读数没有一个 ≤10ms ⇒ 阈值冲突稳定成立，放宽还是列 M1 优化项仍待用户裁决。探针口径另记一笔：本轮报告汇总行打印的端到端 min 是 0.9ms，那是修复前的二进制把先验轮次也计入延迟样本所致，有效轮次真值 min 为 12.2ms；`src/bin/winprobe.rs` 已改为先验轮次不入延迟样本，汇总标签写成"端到端(仅有效轮次)"。】**同日 21:21 的第二轮里，汇总行打印的端到端 min 12.5ms 已与逐轮重算一致 ⇒ 该口径修复经第二次独立采样验证，引用汇总数字不必再手工重算。详见需求书附录 E"E-16 物理按键热键复测"。**同日新增一条同源风险**：park 隐藏不交还焦点（注入 19/19 轮隐藏后前台仍是启动器自身；同日 20:41 物理复测开局 2 轮再次复现）⇒ 若不显式交还，用户会对着一个看不见的窗口打字；该缺陷已列为 FR-WIN-02 硬前置 + E-15 待测。~~**【2026-10-15 E-15 cloak 实测已答：cloak 同样不交还焦点（20/20 先验成立）⇒ 硬前置对 park 与 cloak 两种隐藏手段都成立，交还动作必须自己做，cloak 不自带交还效果】**~~**同批附带修掉一个探针缺陷**：`hotkey_mode` 此前用 `NativeOptions::default()`（含 Vulkan），违反"候选后端必须排除 Vulkan"这条分层约束，2026-10-15 空跑即 `ACCESS_VIOLATION at igvk64.dll`；这也解释了 10-14 那份基线"重试到第 3 次才跑完 20 轮"——它是在随时会崩的后端配置下采的。现已固定 `DX12\|GL`，与生产一致。 |

 
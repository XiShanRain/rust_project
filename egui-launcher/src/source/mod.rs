//! FR-SRC 数据源模块。每个 provider 一个子模块，统一产出「可搜索的条目快照」。
//!
//! 现已落地八个：FR-SRC-02（应用程序源）、FR-SRC-05（自定义命令源）、
//! FR-SRC-06（路径四态校验）、FR-SRC-10（计算器）、FR-SRC-11（网页搜索）、
//! FR-SRC-14（文件搜索与目录浏览）、FR-SRC-16（终端命令 `>`）、
//! FR-SRC-18（速查表 `?`，表里每条语法也按所属源的开关过滤）。
//!
//! **批次 A10（2026-10-22）起**这几个源不再硬编在 `main.rs::plan()` 的匹配链里，而是实现
//! [`DataSource`]、由 [`default_sources`] 按固定顺序注册；`plan()` 只按注册表走一圈。
//!
//! 接一个新源的最小改动面（这就是 A10 的验收判据）：
//! 1. 在本目录写一个子模块，实现 [`DataSource`]（`id` / `label` + 需要的那几个方法）；
//! 2. 在 [`default_sources`] 里加一行 `Box::new(...)`；
//! 3. 完事——设置页的「数据源」开关会自己多出一行，`settings.json` 不用改结构，
//!    `main.rs` 的匹配链一个字都不用动。
//!
//! 顺序即报文次序：越靠前越先问「你要不要独占」，头段行也按这个次序拼接。

pub mod application;
pub mod calc;
pub mod cheatsheet;
pub mod custom;
pub mod filesearch;
pub mod icon;
pub mod import;
pub mod manage;
pub mod params;
pub mod pathcheck;
pub mod runflags;
pub mod system;
pub mod terminal;
pub mod usage;
pub mod websearch;

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::matching::MatchIndex;

use application::{LaunchItem, LearnRank};

/// 数据源的稳定标识。**只增不改**：`as_str` 的结果会写进 `settings.json` 的
/// `sources_off`，改一个名字等于把用户关掉的源又打开。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceId {
    /// 整张清单（应用 + 自定义命令）的模糊匹配结果。
    Apps,
    /// 清单里带参数的命令（`kw 参数`），命中即独占整张列表。
    Custom,
    /// 网页引擎（`g 词`），内置表 + 用户表。
    Web,
    /// 文件搜索与目录浏览（`'词`、空格开头、路径…）。
    Files,
    /// 计算器结果行。
    Calc,
    /// 终端命令（`>命令`）。
    Terminal,
    /// 速查表（`?`、`? 词`）。
    Cheatsheet,
}

impl SourceId {
    /// 注册表的实际顺序（只在单测里用：`default_sources` 的顺序不许漂）。
    /// 与 [`default_sources`] 一致——显式语法（`?` / `>`）最前，其后用户数据压过出厂数据，应用清单殿后。
    #[cfg(test)]
    pub const ALL: [Self; 7] = [
        Self::Cheatsheet,
        Self::Terminal,
        Self::Custom,
        Self::Web,
        Self::Files,
        Self::Calc,
        Self::Apps,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apps => "apps",
            Self::Custom => "custom",
            Self::Web => "web",
            Self::Files => "files",
            Self::Calc => "calc",
            Self::Terminal => "terminal",
            Self::Cheatsheet => "cheatsheet",
        }
    }
}

/// 列表头段（应用结果之前的那几行）的一行。原先叫 `HeadRow`、定义在 `main.rs`；
/// 批次 A10 搬到数据源层，`main.rs` 用 `use source::SourceRow as HeadRow` 保留旧名。
///
/// 头段次序照需求书 4.1.2 的分值偏移：计算器 200、`{query}` 网页搜索 150、
/// 应用最高 `score+10`=110 ⇒ 前两者都压在应用行之前。实际次序由 [`default_sources`]
/// 的注册顺序决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRow {
    Calc(calc::CalcHit),
    Search(websearch::SearchHit),
    /// 空词独占（`g `）时的补全提示。不可执行：`Enter` / 数字键只给 Toast。
    Hint(websearch::EngineHint),
    /// 自定义命令的独占行（`kw 参数`，`{query}` 已填好）。
    Custom(custom::CustomHit),
    /// 自定义命令「还缺一个参数」的提示行：不可执行，Enter 补全成 `"kw "`。
    CustomHint(custom::CustomHint),
    /// 文件搜索 / 目录浏览的一行。「执行」＝用系统关联程序打开它。
    File(filesearch::FileHit),
    /// 终端命令的独占行（`>命令`）。「执行」＝在新终端窗口里跑。
    Terminal(terminal::TermHit),
    /// 裸 `>` 的用法行：不可执行，`Enter` / 数字键只给一句 Toast。
    TermHint(terminal::TermHint),
    /// 速查表的一行。「执行」＝打开这一条的帮助页（不关窗口）。
    Cheat(cheatsheet::CheatHit),
    /// `? 词` 一条也没命中时的提示行：不可执行，只给一句 Toast。
    CheatHint(cheatsheet::CheatHint),
    /// 裸 `@` 的分类提示行（批次 G1）：不可执行，点/按只给一句用法 Toast。
    ScopeTags(application::ScopeTagHint),
    /// 操作面板的一行（批次 P1，FR-ACT-03）。**不是数据源的产物**：面板开着时
    /// `main.rs::plan()` 整段不跑注册表，头段就是这一种行、结果段为空。
    /// 借道头段而不是另起一套列表，是因为 ↑↓ / PgUp/PgDn / Ctrl+N/P/J/K / Tab / 数字键 /
    /// 点击 / 滚动跟随 / 状态栏这些机制全都按「合并行号」算 —— 另写一遍必漂。
    Action(crate::actions::Action),
}

impl SourceRow {
    /// 这一行是否代表清单里真实的一条自定义命令（提示行与独占行都算）⇒ 编辑/删除对它有意义。
    pub fn custom_uid(&self) -> Option<&str> {
        match self {
            SourceRow::Custom(hit) => Some(&hit.uid),
            SourceRow::CustomHint(hint) => Some(&hint.uid),
            _ => None,
        }
    }

    /// 这一行是不是一个文件/目录。文件行不在清单里 ⇒ 编辑/删除/置顶不适用，
    /// 但 Ctrl+D（资源管理器定位）与 Ctrl+Shift+N（预填新建）对它有意义。
    pub fn file_hit(&self) -> Option<&filesearch::FileHit> {
        match self {
            SourceRow::File(hit) => Some(hit),
            _ => None,
        }
    }
}

/// 一帧搜索的只读上下文。所有源看同一份输入 ⇒ 两处判据不会各读各的。
pub struct SearchCtx<'a> {
    pub query: &'a str,
    pub items: &'a [LaunchItem],
    pub engines: &'a [websearch::WebEngine],
    pub learn_rank: LearnRank,
    /// 清单的匹配键缓存。带 `&mut` 是因为它按内容指纹懒重建（FR-QRY-06）。
    pub index: &'a mut MatchIndex,
    /// 取一次、全帧共用：同一帧里几个源看到的时间戳必须一致，否则学习位序会抖。
    pub now_ms: i64,
    /// 终端源用哪个 shell（设置页「功能」页那四选一）。放在上下文里而不是源自己身上：
    /// 源是启动时注册的，配置却随时会变，源每次都读同一份上下文就不会落后一帧。
    pub term_shell: terminal::TermShell,
    /// 此刻哪些数据源是开着的。速查表据此整条过滤掉「已关掉的源」的语法
    /// （FR-SRC-18「按功能启用状态过滤」）；与 `dispatch` 的 `on` 是同一个判断。
    pub source_on: &'a dyn Fn(SourceId) -> bool,
}

/// 本帧列表的完整计划。原先叫 `Plan`、定义在 `main.rs`；批次 A10 搬到数据源层。
#[derive(Debug, Default, Clone)]
pub struct SearchPlan {
    pub head: Vec<SourceRow>,
    pub results: Vec<usize>,
}

/// 一个数据源。默认实现全是"不参与" ⇒ 新源只写自己需要的那几个方法。
pub trait DataSource {
    fn id(&self) -> SourceId;
    /// 设置页开关旁边的名字。
    fn label(&self) -> &'static str;
    /// 设置页开关下面那行小字。空串＝不显示。
    fn note(&self) -> &'static str {
        ""
    }
    /// 启动时的一次性初始化（建索引、预热缓存…）。**关掉的源不会被调用**。
    fn init(&mut self) {}
    /// 每帧一次的后台回报收取。返回 `true` ＝ 本帧有新内容（调用方据此重绘）。
    /// **关掉的源同样不会被调用**：`FileSource::poll` 顺手会派下一轮索引，放它进来
    /// 等于开关只停了搜索、后台仍在读盘。
    fn poll(&mut self) -> bool {
        false
    }
    /// 独占判定：返回 `Some` ＝ 本帧整张列表归本源，后面的源一个都不问。
    fn take_over(&mut self, _cx: &mut SearchCtx<'_>) -> Option<Vec<SourceRow>> {
        None
    }
    /// 独占没有成立时的头段贡献，按注册顺序拼接。
    fn head(&mut self, _cx: &mut SearchCtx<'_>) -> Vec<SourceRow> {
        Vec::new()
    }
    /// 整张列表一条都没有时的补位行。
    fn fallback(&mut self, _cx: &mut SearchCtx<'_>) -> Vec<SourceRow> {
        Vec::new()
    }
    /// 普通结果（`items` 的下标）。正常情况下只有应用清单这一个源会给非空。
    fn rows(&mut self, _cx: &mut SearchCtx<'_>) -> Vec<usize> {
        Vec::new()
    }
    /// 「重建索引」类内置动作（FR-SRC-17 自身命令组）的挂点，默认什么都不做。
    /// 只有真持有索引的源需要覆盖（目前是文件源）；调用方负责先看开关——
    /// 关掉的源不该因为一条命令就在后台读盘。
    fn rebuild(&mut self) {}
    /// 配置变更的回调（批次 D7）：源自己认领 `AppSettings` 里与它相关的格。默认什么都不做。
    /// 调用方在每帧的 poll 循环里、**只对开着的源**喂一遍——与 `poll` 同一道开关闸，
    /// 关掉的源不该因为改了设置就在后台读盘。文件源在这里比对扫描范围，
    /// 变了就走「保存即重扫」（`set_scope` 用相等比较去重，同值零成本）。
    fn apply_settings(&mut self, _prefs: &application::AppSettings) {}
}

/// 注册表。**顺序就是报文次序**：先按此序问独占，再按此序拼头段，最后按此序取结果。
///
/// 新源加在哪儿，决定它压不压得住别人：
/// - 想在用户自己的命令/引擎之后 ⇒ 加在 `Files` 附近；
/// - 想抢在它们之前 ⇒ 往前放（目前 `Custom` 在最前，是因为"用户数据压过出厂数据"）。
/// 全缺省设置的注册表。只有单测在用（`main.rs` 走 [`default_sources_with`]）；
/// 顺序不许漂，两个入口共享同一张 vec。
#[cfg(test)]
pub fn default_sources() -> Vec<Box<dyn DataSource>> {
    default_sources_with(&application::AppSettings::default())
}

/// 同上，但文件源按配置的扫描范围造（批次 D7：冷启动一轮就按 `settings.json` 里的
/// 目录/深度扫，不经历"先按缺省扫一轮再推倒重来"）。
pub fn default_sources_with(prefs: &application::AppSettings) -> Vec<Box<dyn DataSource>> {
    vec![
        // 两个显式语法排最前：`?` / `>` 是用户明说"这条给速查表 / 终端"，不该被同名关键字抢走。
        Box::new(cheatsheet::CheatSource),
        Box::new(terminal::TerminalSource),
        Box::new(custom::CustomSource),
        Box::new(websearch::WebSource),
        Box::new(filesearch::FileSource::with_scope(prefs.file_scan_scope())),
        Box::new(calc::CalcSource),
        Box::new(application::AppsSource),
    ]
}

/// 单源容错（NFR-04）：一个源 panic 只记日志，其余源照常出结果。
///
/// `catch_unwind` 只对 unwind 生效；若将来有人把 `panic = "abort"` 写进发布配置，
/// 这条保护就自动失效——那时要么改回 unwind，要么接受"一个源崩全崩"。
fn guarded<T>(id: SourceId, f: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(_) => {
            // 默认 panic hook 已经把位置与消息打到 stderr（落 launcher.log）；这里只补一句归因。
            eprintln!("数据源 {}：本帧 panic，已跳过（其余源照常）", id.as_str());
            None
        }
    }
}

/// 本帧的完整搜索：独占 → 头段 → 结果 → 补位。`on` 决定某个源此刻是否启用。
pub fn dispatch(
    sources: &mut [Box<dyn DataSource>],
    cx: &mut SearchCtx<'_>,
    on: impl Fn(SourceId) -> bool,
) -> SearchPlan {
    // 1) 独占段：注册序里第一个成立的拿走整张列表。
    for s in sources.iter_mut() {
        let id = s.id();
        if !on(id) {
            continue;
        }
        if let Some(rows) = guarded(id, || s.take_over(&mut *cx)).flatten() {
            return SearchPlan {
                head: rows,
                results: Vec::new(),
            };
        }
    }

    // 2) 头段：按注册序拼接（自定义提示行 → 计算器 → …）。
    let mut head: Vec<SourceRow> = Vec::new();
    for s in sources.iter_mut() {
        let id = s.id();
        if !on(id) {
            continue;
        }
        if let Some(mut rows) = guarded(id, || s.head(&mut *cx)) {
            head.append(&mut rows);
        }
    }

    // 3) 普通结果。
    let mut results: Vec<usize> = Vec::new();
    for s in sources.iter_mut() {
        let id = s.id();
        if !on(id) {
            continue;
        }
        if let Some(mut rows) = guarded(id, || s.rows(&mut *cx)) {
            results.append(&mut rows);
        }
    }

    // 4) 提示行与下方同一条目只留一处：`kw` 打全、还差参数时头段已经有提示行，
    //    清单里那条命令就不该在下面再出现一次（用户会以为有两个东西）。
    for row in &head {
        if let SourceRow::CustomHint(hint) = row {
            results.retain(|&i| cx.items[i].uid() != hint.uid);
        }
    }

    // 5) 补位：一条都没有时才问（FR-QRY-08 的网页兜底），第一个给出内容的胜出。
    if head.is_empty() && results.is_empty() {
        for s in sources.iter_mut() {
            let id = s.id();
            if !on(id) {
                continue;
            }
            if let Some(rows) = guarded(id, || s.fallback(&mut *cx)) {
                if !rows.is_empty() {
                    head = rows;
                    break;
                }
            }
        }
    }

    SearchPlan { head, results }
}

#[cfg(test)]
mod registry_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// 记下自己什么时候被问到，便于断言"注册顺序"与"关掉的源不问"。
    struct Probe {
        id: SourceId,
        seen: Rc<RefCell<Vec<SourceId>>>,
        take: Option<usize>,
        boom: bool,
    }

    impl DataSource for Probe {
        fn id(&self) -> SourceId {
            self.id
        }
        fn label(&self) -> &'static str {
            "探针"
        }
        fn take_over(&mut self, _cx: &mut SearchCtx<'_>) -> Option<Vec<SourceRow>> {
            self.seen.borrow_mut().push(self.id);
            if self.boom {
                panic!("探针甲故意 panic");
            }
            self.take.map(|_| Vec::new())
        }
    }

    /// 测试里所有源都当开着（速查表要的开关判断）。
    fn all_on(_: SourceId) -> bool {
        true
    }

    fn ctx_with<'a>(
        query: &'a str,
        items: &'a [LaunchItem],
        index: &'a mut MatchIndex,
    ) -> SearchCtx<'a> {
        SearchCtx {
            query,
            items,
            engines: &[],
            learn_rank: LearnRank::Count,
            index,
            now_ms: 0,
            term_shell: terminal::TermShell::default(),
            source_on: &all_on,
        }
    }

    /// 头段给一条自定义命令提示行（uid 由调用方钉住）。
    struct HintProbe {
        uid: String,
    }

    impl DataSource for HintProbe {
        fn id(&self) -> SourceId {
            SourceId::Custom
        }
        fn label(&self) -> &'static str {
            "提示探针"
        }
        fn head(&mut self, _cx: &mut SearchCtx<'_>) -> Vec<SourceRow> {
            vec![SourceRow::CustomHint(custom::CustomHint {
                uid: self.uid.clone(),
                title: String::new(),
                subtitle: String::new(),
                completion: String::new(),
                command: String::new(),
            })]
        }
    }

    /// 普通结果源：照给定下标表返回。
    struct RowsProbe {
        rows: Vec<usize>,
    }

    impl DataSource for RowsProbe {
        fn id(&self) -> SourceId {
            SourceId::Apps
        }
        fn label(&self) -> &'static str {
            "结果探针"
        }
        fn rows(&mut self, _cx: &mut SearchCtx<'_>) -> Vec<usize> {
            self.rows.clone()
        }
    }

    /// 原 `src/main.rs::drop_hinted` 的单测（批次 A10 随判据一起搬进来）：提示行已经代表
    /// 那条命令，下方结果里就不许再出现同一 uid——否则用户会以为清单里有两个 `ping`。
    #[test]
    fn a_custom_hint_absorbs_the_same_item_from_results() {
        let hinted = LaunchItem::default();
        let other = LaunchItem {
            path: "mspaint.exe".into(),
            ..Default::default()
        };
        let items = vec![hinted, other];
        let uid = items[0].uid();
        let mut sources: Vec<Box<dyn DataSource>> = vec![
            Box::new(HintProbe { uid }),
            Box::new(RowsProbe { rows: vec![0, 1] }),
        ];
        let mut index = MatchIndex::default();
        let mut cx = ctx_with("ping", &items, &mut index);
        let plan = dispatch(&mut sources, &mut cx, |_| true);
        assert_eq!(plan.head.len(), 1, "提示行留在头段");
        assert_eq!(
            plan.results,
            vec![1],
            "同一 uid 从下方结果里摘掉，别的条目不误伤"
        );
    }

    /// FR-SRC-16：`>` 是显式语法 ⇒ 独占整张列表；裸 `>` 只给用法行；源关掉后同一条查询
    /// 回到普通搜索（设置页那行开关的判据）。
    #[test]
    fn a_terminal_prefix_takes_over_and_bare_prefix_gives_usage() {
        let items = vec![LaunchItem {
            name: "计算器".into(),
            path: "calc.exe".into(),
            ..Default::default()
        }];
        let mut index = MatchIndex::default();
        {
            let mut sources = default_sources();
            let mut cx = ctx_with(">ipconfig /all", &items, &mut index);
            let plan = dispatch(&mut sources, &mut cx, |_| true);
            assert!(
                plan.results.is_empty(),
                "`>` 独占：下面的清单一条都不许露出"
            );
            match plan.head.as_slice() {
                [SourceRow::Terminal(h)] => {
                    assert_eq!(h.command, "ipconfig /all");
                    assert_eq!(h.shell, terminal::TermShell::Cmd, "默认 shell 是 cmd");
                }
                other => panic!("`>命令` 应独占成一行终端行，实际 {other:?}"),
            }
        }
        {
            let mut sources = default_sources();
            let mut cx = ctx_with(">", &items, &mut index);
            let plan = dispatch(&mut sources, &mut cx, |_| true);
            assert!(plan.results.is_empty());
            assert!(
                matches!(plan.head.as_slice(), [SourceRow::TermHint(_)]),
                "裸 `>` 只给用法行"
            );
        }
        {
            let mut sources = default_sources();
            let mut cx = ctx_with(">", &items, &mut index);
            let plan = dispatch(&mut sources, &mut cx, |id| id != SourceId::Terminal);
            assert!(plan.head.is_empty(), "终端源关着时 `>` 不产生任何行");
        }
    }

    #[test]
    fn default_registry_has_each_source_once_in_paper_order() {
        let sources = default_sources();
        let ids: Vec<SourceId> = sources.iter().map(|s| s.id()).collect();
        assert_eq!(
            ids,
            SourceId::ALL.to_vec(),
            "注册顺序必须与 SourceId::ALL 一致"
        );
        let mut labels: Vec<&str> = sources.iter().map(|s| s.label()).collect();
        let n = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), n, "设置页靠 label 区分开关，不许重名");
    }

    #[test]
    fn take_over_order_follows_registration_and_stops_at_first_hit() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut sources: Vec<Box<dyn DataSource>> = vec![
            Box::new(Probe {
                id: SourceId::Apps,
                seen: seen.clone(),
                take: None,
                boom: false,
            }),
            Box::new(Probe {
                id: SourceId::Custom,
                seen: seen.clone(),
                take: Some(0),
                boom: false,
            }),
            Box::new(Probe {
                id: SourceId::Web,
                seen: seen.clone(),
                take: Some(0),
                boom: false,
            }),
        ];
        let items: Vec<LaunchItem> = Vec::new();
        let mut index = MatchIndex::default();
        let mut cx = ctx_with("x", &items, &mut index);
        let plan = dispatch(&mut sources, &mut cx, |_| true);
        assert_eq!(&*seen.borrow(), &[SourceId::Apps, SourceId::Custom]);
        assert!(plan.head.is_empty() && plan.results.is_empty());
    }

    #[test]
    fn a_disabled_source_is_not_asked_at_all() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut sources: Vec<Box<dyn DataSource>> = vec![
            Box::new(Probe {
                id: SourceId::Apps,
                seen: seen.clone(),
                take: None,
                boom: true,
            }),
            Box::new(Probe {
                id: SourceId::Custom,
                seen: seen.clone(),
                take: None,
                boom: false,
            }),
        ];
        let items: Vec<LaunchItem> = Vec::new();
        let mut index = MatchIndex::default();
        let mut cx = ctx_with("x", &items, &mut index);
        // 关掉那个会 panic 的源 ⇒ 它连被问到的机会都没有，自然也不会崩。
        let plan = dispatch(&mut sources, &mut cx, |id| id != SourceId::Apps);
        assert_eq!(&*seen.borrow(), &[SourceId::Custom]);
        assert!(plan.head.is_empty() && plan.results.is_empty());
    }

    #[test]
    fn one_source_panicking_does_not_take_down_the_rest() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut sources: Vec<Box<dyn DataSource>> = vec![
            Box::new(Probe {
                id: SourceId::Custom,
                seen: seen.clone(),
                take: None,
                boom: true,
            }),
            Box::new(Probe {
                id: SourceId::Web,
                seen: seen.clone(),
                take: None,
                boom: false,
            }),
        ];
        let items: Vec<LaunchItem> = Vec::new();
        let mut index = MatchIndex::default();
        let mut cx = ctx_with("x", &items, &mut index);
        let _ = dispatch(&mut sources, &mut cx, |_| true);
        // 探针只在 `take_over` 里记账 ⇒ 这一段看到的是独占段的问询次序：
        // 崩掉的那个问过一次就被跳过，后面的源照常被问到（`dispatch` 没有 unwinding 出去）。
        assert_eq!(
            &*seen.borrow(),
            &[SourceId::Custom, SourceId::Web],
            "崩掉一个源不该带走下一个"
        );
    }
}

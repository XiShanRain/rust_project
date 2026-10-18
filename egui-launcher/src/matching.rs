//! FR-QRY 输入解析与匹配（需求书 §5.2；阶段一 P0 条款 01/03/04/06/07）。
//!
//! 纯计算模块，不含 egui / Win32 类型，可单测。三块职责：
//! - [`MatchKeys`]：FR-QRY-06 的预计算键 `{lower, words, initials, length}`，
//!   只在清单内容变化时整体重算，搜索路径内不再逐条 `to_lowercase` / 拆词；
//! - [`parse`]：FR-QRY-01 的 `(keyword, rest, has_rest)` 三元组；
//! - [`MatchIndex::search`]：FR-QRY-03 七档打分 + FR-SRC-19 学习位序 + 稳定排序 + 截断 50。
//!
//! 排序口径（2026-10-17 与 FR-SRC-19/FR-QRY-07 对齐）：**档位是闸门**，
//! 学习位序只在同档内生效，档内的位置/长度惩罚只作末名次键；
//! 时间戳不参与打分、不参与门槛，符合 FR-QRY-07 裁定 a。
//! 学习位序本身有两种口径（批次 K，`LearnRank`）：默认甲＝次数优先、次数相同看最近一次；
//! 乙＝热度 `log2(1+次数) × 0.5^(天数/半衰期)`。两者都不越过硬闸门去动档位。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::source::application::{LaunchItem, LearnRank, SpaceRun};

/// FR-QRY-07 结果上限。
const MAX_RESULTS: usize = 50;

// 七档的档位号（FR-QRY-03 分值表的规则序号，降序为优先级）。
// 档位与分数分开返回：60 档带内是连续小数，若只比分数会让 45 档（≤45）与 30 档混序。
const T_EXACT: u8 = 6; // 100 完全相等
const T_PREFIX: u8 = 5; // 90 以查询开头
const T_WORD_PREFIX: u8 = 4; // 80 某词以查询开头
const T_INITIALS: u8 = 3; // 75 词首字母（FR-QRY-04）
const T_CONTAINS: u8 = 2; // 60 包含（位置衰减 + lengthPenalty）
const T_WORDS: u8 = 1; // 45 多词全命中
const T_SUBSEQ: u8 = 0; // 30 子序列

/// 一条可匹配文本的预计算键（FR-QRY-06）。空文本不成键，交给 `Option` 而不是在打分里特判。
struct Key {
    lower: String,
    words: Vec<String>,
    initials: String,
    chars: usize,
}

impl Key {
    fn new(src: &str) -> Option<Key> {
        if src.trim().is_empty() {
            return None;
        }
        let words = split_words(src);
        let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
        let lower = src.to_lowercase();
        Some(Key { chars: lower.chars().count(), lower, words, initials })
    }
}

/// FR-QRY-04 拆词：先在 ASCII 小写→大写边界拆驼峰（`AutoHotkey`→`Auto Hotkey`），
/// 再按非字母数字切，逐词小写。顺序不能反：整体 lower 之后再拆驼峰就永远看不见大写了。
fn split_words(src: &str) -> Vec<String> {
    let mut spaced = String::with_capacity(src.len() + 8);
    let mut prev = '\0';
    for c in src.chars() {
        if prev.is_ascii_lowercase() && c.is_ascii_uppercase() {
            spaced.push(' ');
        }
        spaced.push(c);
        prev = c;
    }
    spaced
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// 一个条目的匹配键：名称 + 别名（FR-SRC-22 的 `keyword`）。与 `items` 同序存放。
/// 别名按空白拆成多键——`keyword` 里可能同时有默认拼音首字母和「提交即学」攒下来的触发词，
/// 合成一串会让 `initials` 变成跨别名的假缩写（"jsq qsj"→"jq"），污染 75 档。
pub struct MatchKeys {
    name: Option<Key>,
    aliases: Vec<Key>,
}

impl MatchKeys {
    fn build(item: &LaunchItem) -> MatchKeys {
        MatchKeys {
            name: Key::new(&item.name),
            aliases: item.keyword.split_whitespace().filter_map(Key::new).collect(),
        }
    }
    fn name_chars(&self) -> usize {
        self.name.as_ref().map_or(0, |k| k.chars)
    }
}

/// FR-QRY-01 解析结果。`keyword` 已小写供匹配；`rest` 保留原样（将来作 `{query}` 参数用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub keyword: String,
    pub rest: String,
    /// 首词之后出现过空格 ⇒ 独占语义（rest 可为空串）。
    pub has_rest: bool,
    /// 整串（小写、空白折叠）与逐词，供热路径直接用，不在条目循环里重建。
    pub full: String,
    pub words: Vec<String>,
}

pub fn parse(raw: &str) -> Parsed {
    let head = raw.trim_start();
    let (keyword, rest, has_rest) = match head.find(' ') {
        Some(i) => (head[..i].to_lowercase(), head[i + 1..].to_string(), true),
        None => (head.to_lowercase(), String::new(), false),
    };
    let words: Vec<String> = head.split_whitespace().map(str::to_lowercase).collect();
    let full = words.join(" ");
    Parsed { keyword, rest, has_rest, full, words }
}

/// 清单键表缓存（FR-QRY-06）。用内容指纹判断是否需要重算，而不是指望每个改动清单的地方
/// 都记得通知我——漏一处就是永久性错序；指纹只覆盖成键用到的字段（名称、别名）。
#[derive(Default)]
pub struct MatchIndex {
    fingerprint: u64,
    keys: Vec<MatchKeys>,
}

impl MatchIndex {
    fn fingerprint(items: &[LaunchItem]) -> u64 {
        let mut h = DefaultHasher::new();
        for it in items {
            it.name.hash(&mut h);
            it.keyword.hash(&mut h);
        }
        h.finish()
    }

    fn ensure(&mut self, items: &[LaunchItem]) {
        let fp = Self::fingerprint(items);
        if fp != self.fingerprint || self.keys.len() != items.len() {
            self.keys = items.iter().map(MatchKeys::build).collect();
            self.fingerprint = fp;
        }
    }

    /// 返回按最终次序排列的 `items` 下标（不返回引用：调用方随后还要改 `self.items`）。
    /// `rank` 与 `now_ms` 只喂学习位序那一层，档位闸门与打分都不看它们。
    pub fn search(
        &mut self,
        raw: &str,
        items: &[LaunchItem],
        rank: LearnRank,
        now_ms: i64,
    ) -> Vec<usize> {
        let p = parse(raw);
        self.ensure(items);
        let keys = &self.keys;

        // 空输入：不分桶，只让置顶/常用浮顶；三键全平的项保持清单原序（sort_by 稳定）。
        // 不截断——这是「浏览整张清单」，不是搜索结果；截 50 会把用户已有的项静默藏掉。
        if p.words.is_empty() {
            let mut v: Vec<usize> = (0..items.len()).collect();
            v.sort_by(|a, b| learns_before(&items[*a], &items[*b], rank, now_ms));
            return v;
        }

        let mut hits: Vec<(u8, f32, usize)> = Vec::new();
        for (idx, k) in keys.iter().enumerate() {
            if let Some((tier, score)) = score_item(&p, k) {
                hits.push((tier, score, idx));
            }
        }
        hits.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| learns_before(&items[a.2], &items[b.2], rank, now_ms))
                .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| keys[a.2].name_chars().cmp(&keys[b.2].name_chars()))
        });
        hits.truncate(MAX_RESULTS);
        hits.into_iter().map(|(_, _, idx)| idx).collect()
    }
}

/// FR-SRC-19 学习位序：置顶永远第一，其后按 `rank` 选的口径（全部降序）。
/// 甲（默认）＝累计次数 → 最近一次；乙＝热度 [`learn_heat`]，同分再退回到最近一次。
pub fn learns_before(
    a: &LaunchItem,
    b: &LaunchItem,
    rank: LearnRank,
    now_ms: i64,
) -> std::cmp::Ordering {
    let learn = match rank {
        LearnRank::Count => a
            .use_count
            .cmp(&b.use_count)
            .then_with(|| a.last_used_ms.cmp(&b.last_used_ms)),
        LearnRank::Decay => learn_heat(a, now_ms)
            .partial_cmp(&learn_heat(b, now_ms))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.last_used_ms.cmp(&b.last_used_ms)),
    };
    a.pinned.cmp(&b.pinned).then_with(|| learn).reverse()
}

/// 乙档的热度：**累计次数被 `log2` 压过量级，再按距上次使用的天数做真半衰期衰减**。
/// `log2(1+N)` 让 8 次（3.0）只是 1 次（1.0）的三倍权重，避免次数把匹配质量顶掉；
/// 用 `0.5^(days/H)` 而非 `e^(-days/H)`——后者名义 H 天的实际半衰期是 1.44H 天（旧公式踩过）。
pub fn learn_heat(item: &LaunchItem, now_ms: i64) -> f32 {
    let days = (now_ms - item.last_used_ms).max(0) as f32 / 86_400_000.0;
    (item.use_count as f32 + 1.0).log2() * 0.5f32.powf(days / HEAT_HALF_LIFE_DAYS)
}

/// 热度衰减的半衰期（天）：一条项恰好 N 天没用，热度掉一半。
pub const HEAT_HALF_LIFE_DAYS: f32 = 7.0;

/// 一个条目对整次查询的打分：名称与别名取优（别名是刻意设置的触发码，等同名称参与匹配）。
fn score_item(p: &Parsed, keys: &MatchKeys) -> Option<(u8, f32)> {
    let mut best: Option<(u8, f32)> = None;
    for k in [&keys.name].into_iter().flatten().chain(keys.aliases.iter()) {
        let hit = score_key(p, k);
        if hit.1 > 0.0 && best.is_none_or(|b| hit > b) {
            best = Some(hit);
        }
    }
    best
}

/// Tab 补全判据（FR-ACT-06，2026-10-18 与用户裁定的「可观测条件分流」）：
/// 查询首词是**名称或任一别名**的前缀，且名称比该词更长（补全会让查询变长）⇒ 值得补全。
/// 只看名称前缀不够：敲拼音首字母 `jsq` 要补成「计算器」，二者没有字符前缀关系，
/// 靠别名那半才成立。
pub fn can_complete(raw: &str, item: &LaunchItem) -> bool {
    let p = parse(raw);
    if p.keyword.is_empty() {
        return false;
    }
    let q = p.keyword.as_str();
    let name = item.name.to_lowercase();
    if name == q {
        return false;
    }
    name.starts_with(q) || item.keyword.split_whitespace().any(|a| a.to_lowercase().starts_with(q))
}

/// 甲案（FR-ACT-06，2026-10-18 用户裁定）：**补全只负责消歧**。
/// 结果集里「名称/别名还能被这个查询补长」的候选恰好一条 ⇒ 补全它（返回 `results` 中的名次）；
/// 两条及以上 ⇒ `None`，歧义还没消掉，绝不替用户猜第一条，交给 Tab 的循环导航。
/// 为什么必须有这一层：列表是查询的派生值，查询一旦被写成某一条的完整名称，
/// 派生结果就必然塌成那一条 ——「`Tas` 四条候选、Tab 补成 Task Manager 后再也到不了
/// Task Scheduler」这个缺陷的根因就是 `can_complete` 单条判据没看兄弟候选。
pub fn unique_completion(raw: &str, items: &[LaunchItem], results: &[usize]) -> Option<usize> {
    let mut found: Option<usize> = None;
    for (rank, &idx) in results.iter().enumerate() {
        if can_complete(raw, &items[idx]) {
            if found.is_some() {
                return None;
            }
            found = Some(rank);
        }
    }
    found
}

/// 批次 J（丙案判据）：查询去掉首尾空白后，是否与这一行的**名称整串相等**，
/// 或等于它**别名表里的某一个词**（`keyword` 按空格分词，与 FR-SRC-19 学习别名同一词表）。
/// 只认整串相等，不认前缀：`tas` 是 `Task Manager` 的前缀但不是全名 ⇒ false，
/// 那一下空格就该继续当文本（或交给 Tab 补全去消歧）。空查询返回 false ——
/// 空框那一档由 [`space_executes`] 自己判，不混进这里。
pub fn query_is_full_word(raw: &str, item: &LaunchItem) -> bool {
    let q = raw.trim().to_lowercase();
    if q.is_empty() {
        return false;
    }
    q == item.name.trim().to_lowercase()
        || item
            .keyword
            .split_whitespace()
            .any(|a| a.to_lowercase() == q)
}

/// 空格这一按到底执行不执行（FR-ACT-01 批次 J，四档判据全在这里，纯函数可单测）。
/// 五个入参全是当帧可观测状态（档位 / 查询 / 清单 / 结果集 / 选中行），**不看按键历史**
/// —— 与 Tab 甲案同一条已入档判据。`selected` 越界时按最后一行钳位，与执行分支同一口径；
/// off/word 在结果集为空时返回 false（没条目可执行），always 保持老口径恒为 true，
/// 由调用方那侧"取不到条目就什么都不做"兜住。
pub fn space_executes(
    mode: SpaceRun,
    raw: &str,
    items: &[LaunchItem],
    results: &[usize],
    selected: usize,
) -> bool {
    match mode {
        // 旧口径原样保留（批次 F）：off＝trim 后为空才执行，always＝任何时候都执行。
        SpaceRun::Off => raw.trim().is_empty() && !results.is_empty(),
        SpaceRun::Always => true,
        // 新口径只认"框里一个字符都没有"为空格框。用 `Shift+空格` 故意插进去的空白
        // 不算空框 —— 否则逃生门会变成陷阱：插完空格，下一次按空格就直接把程序启动了。
        SpaceRun::Unique => raw.is_empty() || results.len() == 1,
        SpaceRun::Word => {
            if raw.is_empty() {
                return !results.is_empty();
            }
            let last = results.len().saturating_sub(1);
            match results.get(selected.min(last)) {
                Some(&idx) => query_is_full_word(raw, &items[idx]),
                None => false,
            }
        }
    }
}

/// 单串规则与多词规则取优：`"visual studio"` 既应命中整段包含（60 档），
/// 也应在整串不成立时靠逐词全命中（45 档）兜住。
fn score_key(p: &Parsed, k: &Key) -> (u8, f32) {
    let whole = score_word(&p.full, k);
    if p.words.len() <= 1 {
        return whole;
    }
    let per = words_score(p, k);
    if per > 0.0 && per > whole.1 {
        (T_WORDS, per)
    } else {
        whole
    }
}

/// FR-QRY-03 分值表：单词对一组键。返回 `(档位, 分数)`，0 分=不匹配（闸门由调用方判）。
fn score_word(q: &str, k: &Key) -> (u8, f32) {
    if q.is_empty() {
        return (0, 0.0);
    }
    let ql = q.chars().count();
    if k.lower == q {
        return (T_EXACT, 100.0);
    }
    if k.lower.starts_with(q) {
        return (T_PREFIX, 90.0);
    }
    if k.words.iter().any(|w| w.starts_with(q)) {
        return (T_WORD_PREFIX, 80.0);
    }
    // 首字母串：查询长度 ≥2 且为 initials 的前缀（`vsc`→Visual Studio Code）。
    if ql >= 2 && k.initials.starts_with(q) {
        return (T_INITIALS, 75.0);
    }
    if let Some(byte_pos) = k.lower.find(q) {
        let pos = k.lower[..byte_pos].chars().count();
        let s = 60.0 - (pos.min(20) as f32) * 0.25 - length_penalty(k.chars, ql);
        return (T_CONTAINS, s);
    }
    // 子序列：长度 ≥3、首字符相同、按序不连续命中（`clcl`→calculator）。
    if ql >= 3
        && k.lower.chars().next().zip(q.chars().next()).is_some_and(|(a, b)| a == b)
        && is_subsequence(q, &k.lower)
    {
        return (T_SUBSEQ, 30.0);
    }
    (0, 0.0)
}

/// 45 档：`min(total/count, 90) × 0.5`；任一词完全不中 ⇒ 0（整条不匹配）。
fn words_score(p: &Parsed, k: &Key) -> f32 {
    let mut total = 0.0f32;
    for w in &p.words {
        let (_, s) = score_word(w, k);
        if s == 0.0 {
            return 0.0;
        }
        total += s;
    }
    (total / p.words.len() as f32).min(90.0) * 0.5
}

/// `lengthPenalty = min(max(key.len − query.len, 0), 40) × 0.1`（最多 −4）。
fn length_penalty(key_chars: usize, q_chars: usize) -> f32 {
    key_chars.saturating_sub(q_chars).min(40) as f32 * 0.1
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, keyword: &str, path: &str, pinned: bool, use_count: u32, last_used_ms: i64) -> LaunchItem {
        LaunchItem {
            name: name.to_string(),
            path: path.to_string(),
            keyword: keyword.to_string(),
            pinned,
            use_count,
            last_used_ms,
            ..Default::default()
        }
    }

    /// 测试里的"现在"。学习位序专项用例拿它当基准算天数，其余用例只要保证
    /// `last_used_ms` 全在它之下、不会溢出成负数即可。
    const NOW: i64 = 1_000_000_000;
    const DAY: i64 = 86_400_000;

    fn order_at(items: &[LaunchItem], q: &str, rank: LearnRank, now_ms: i64) -> Vec<String> {
        MatchIndex::default()
            .search(q, items, rank, now_ms)
            .into_iter()
            .map(|i| items[i].name.clone())
            .collect()
    }

    /// 默认走甲档：绝大多数用例只验证档位与打分，学习层保持清单原口径最省事。
    fn order(items: &[LaunchItem], q: &str) -> Vec<String> {
        order_at(items, q, LearnRank::Count, NOW)
    }

    fn hit(items: &[LaunchItem], q: &str) -> (u8, f32) {
        let p = parse(q);
        let k = MatchKeys::build(&items[0]);
        score_item(&p, &k).unwrap_or((0, 0.0))
    }

    // ---- FR-QRY-03 逐档覆盖（TC-QRY-03）----

    #[test]
    fn seven_tiers_score_as_specified() {
        let one = |name: &str| vec![item(name, "", "p", false, 0, 0)];
        assert_eq!(hit(&one("计算器"), "计算器"), (T_EXACT, 100.0));
        assert_eq!(hit(&one("计算器专业版"), "计算器"), (T_PREFIX, 90.0));
        assert_eq!(hit(&one("Visual Studio Code"), "studio"), (T_WORD_PREFIX, 80.0));
        assert_eq!(hit(&one("AutoHotkey"), "ah"), (T_INITIALS, 75.0)); // FR-QRY-04 驼峰拆分
        assert_eq!(hit(&one("my_file-name"), "mfn"), (T_INITIALS, 75.0)); // 非字母数字切词
        assert_eq!(hit(&one("Visual Studio Code"), "vsc"), (T_INITIALS, 75.0), "FR-QRY-04 验收例");
        // 包含档（既非前缀也非词首）："notepad" 里的 "ote" 在第 1 个字符 ⇒
        // 60 − 1×0.25 − (7−3)×0.1 = 59.35
        assert_eq!(hit(&one("Notepad"), "ote").0, T_CONTAINS);
        assert!((hit(&one("Notepad"), "ote").1 - 59.35).abs() < 1e-5);
        // 首字母串：长度 <2 不启用，避免单字母把整张清单捞进来
        assert_eq!(hit(&one("AutoHotkey"), "a").0, T_PREFIX);
        // 子序列：长度 ≥3、首字符相同、不连续 ⇒ 30
        assert_eq!(hit(&one("calculator"), "cltr"), (T_SUBSEQ, 30.0));
        // 多词全命中：visual→90（整串开头）、code→80（某词开头），mean=85 ⇒ 85×0.5=42.5
        let (tier, score) = hit(&one("Visual Studio Code"), "visual code");
        assert_eq!(tier, T_WORDS);
        assert!((score - 42.5).abs() < 1e-5, "实得 {score}");
    }

    #[test]
    fn position_and_length_penalties_are_capped() {
        // pos 封顶 20 ⇒ 最多 −5；长度惩罚封顶 40 ⇒ 最多 −4。
        let long = "a".repeat(100);
        let items = vec![item(&long, "", "p", false, 0, 0)];
        let (_, s) = hit(&items, "b");
        assert_eq!(s, 0.0, "不含就不匹配");
        let items = vec![item(&format!("{}x", long), "", "p", false, 0, 0)];
        let (tier, s) = hit(&items, "x");
        assert_eq!(tier, T_CONTAINS);
        assert!((s - (60.0 - 5.0 - 4.0)).abs() < 1e-5, "双封顶应 51.0，实得 {s}");
    }

    #[test]
    fn one_word_mismatch_is_excluded() {
        let items = vec![item("计算器", "", "p", false, 0, 0)];
        assert_eq!(order(&items, "paint"), Vec::<String>::new());
        // 多词里任一词不中 ⇒ 整条不匹配（45 档规则）
        assert_eq!(order(&items, "计算 不存在的词"), Vec::<String>::new());
    }

    // ---- FR-QRY-01 解析 ----

    #[test]
    fn parse_splits_keyword_and_rest() {
        let p = parse("clip ");
        assert_eq!((p.keyword.as_str(), p.rest.as_str(), p.has_rest), ("clip", "", true));
        let p = parse("clip");
        assert_eq!((p.keyword.as_str(), p.rest.as_str(), p.has_rest), ("clip", "", false));
        let p = parse("calc /k ping");
        assert!(p.has_rest);
        assert_eq!(p.keyword, "calc");
        assert_eq!(p.rest, "/k ping", "rest 保留原样供将来做参数替换");
        assert_eq!(p.words, ["calc", "/k", "ping"]);
        assert_eq!(parse("  calc").keyword, "calc", "前导空格不吃掉首词");
    }

    // ---- 档位为闸门 + FR-SRC-19 学习位序 ----

    #[test]
    fn bucket_outranks_pinned() {
        let items = vec![
            item("我的计算器", "", "a", true, 9, 999), // 包含 → 2 档
            item("计算器", "", "b", false, 0, 0), // 全等 → 6 档
        ];
        assert_eq!(order(&items, "计算器"), ["计算器", "我的计算器"]);
    }

    #[test]
    fn keyword_exact_gets_top_bucket() {
        let items = vec![
            item("我的jsq工具", "", "a", true, 5, 5),
            item("计算器", "jsq", "b", false, 0, 0), // 别名全等
        ];
        assert_eq!(order(&items, "jsq"), ["计算器", "我的jsq工具"]);
    }

    #[test]
    fn learn_keys_are_pinned_then_count_then_recent() {
        let items = vec![
            item("ax", "", "a", false, 0, 100),
            item("bx", "", "b", false, 99, 200),
            item("cx", "", "c", true, 0, 0),
        ];
        assert_eq!(order(&items, "x"), ["cx", "bx", "ax"]);
    }

    /// 批次 K 甲档（默认）的核心诉求：次数压过「刚刚用过」。
    /// 复刻用户 2026-10-18 报的场景 —— 记事本执行 8 次，命令提示符只执行过 1 次
    /// 但那次更晚，旧口径（最近一次优先）把它顶到了前面。
    #[test]
    fn count_rank_keeps_the_frequently_used_item_ahead() {
        let items = vec![
            item("axx", "", "a", false, 8, 1_000),
            item("bxx", "", "b", false, 1, NOW),
        ];
        assert_eq!(order(&items, "xx"), ["axx", "bxx"], "同档位内先比次数，后比最近一次");
        assert_eq!(order(&items, ""), ["axx", "bxx"], "空输入的浏览视图同判据");
    }

    /// 批次 K 乙档：热度 = log2(1+次数) × 0.5^(天数/半衰期)。
    /// 8 次但搁了整整一个半衰期（7 天）⇒ 1.585，输给三天前只用过 3 次、今天刚用的 2.00。
    #[test]
    fn decay_rank_lets_a_stale_hot_item_lose_to_a_fresh_one() {
        let items = vec![
            item("axx", "", "a", false, 8, NOW - 7 * DAY),
            item("bxx", "", "b", false, 3, NOW),
        ];
        let ha = learn_heat(&items[0], NOW);
        let hb = learn_heat(&items[1], NOW);
        assert!((ha - 1.58496).abs() < 1e-4, "log2(9)×0.5 = {ha}");
        assert!((hb - 2.0).abs() < 1e-6, "log2(4)×1 = {hb}");
        assert_eq!(order_at(&items, "xx", LearnRank::Decay, NOW), ["bxx", "axx"]);
        assert_eq!(
            order_at(&items, "xx", LearnRank::Count, NOW),
            ["axx", "bxx"],
            "同一份数据两档结论相反 ⇒ 配置确实在决定次序，不是巧合"
        );
    }

    #[test]
    fn learn_heat_uses_a_true_half_life() {
        let heat = |count: u32, days: i64| {
            learn_heat(&item("x", "", "p", false, count, NOW - days * DAY), NOW)
        };
        assert_eq!(heat(0, 0), 0.0, "0 次 ⇒ log2(1)=0，学习层闭嘴，交给档位与打分");
        assert!((heat(1, 0) - 1.0).abs() < 1e-6, "1 次 = log2(2)");
        assert!((heat(3, 0) - 2.0).abs() < 1e-6, "3 次 = log2(4)：次数被压成对数量级");
        assert!((heat(1, 7) - 0.5).abs() < 1e-6, "恰好一个半衰期 ⇒ 掉一半");
        assert!((heat(1, 14) - 0.25).abs() < 1e-6, "两个半衰期 ⇒ 四分之一");
        assert!(heat(1, 70).abs() < 1e-3, "10 个半衰期后基本归零");
        assert_eq!(
            learn_heat(&item("x", "", "p", false, 1, NOW + 10 * DAY), NOW),
            1.0,
            "时钟回拨（last_used 在未来）天数按 0 算，不能让热度爆表"
        );
    }

    #[test]
    fn pinned_beats_heat_in_both_ranks() {
        let items = vec![
            item("axx", "", "a", false, 8, NOW),
            item("bxx", "", "b", true, 0, NOW - 7 * DAY),
        ];
        for rank in [LearnRank::Count, LearnRank::Decay] {
            assert_eq!(
                order_at(&items, "xx", rank, NOW),
                ["bxx", "axx"],
                "{rank:?} 档下置顶仍是硬第一键，衰减只作用于其后那一层"
            );
        }
    }

    #[test]
    fn same_score_prefers_shorter_name() {
        let items = vec![item("xxxx", "", "a", false, 0, 0), item("xx", "", "b", false, 0, 0)];
        assert_eq!(order(&items, "x"), ["xx", "xxxx"]);
    }

    #[test]
    fn in_band_score_breaks_ties_when_learn_keys_are_flat() {
        // 同为包含档、学习键全平：靠位置/长度惩罚定序（越靠前、越短越前）
        let items = vec![item("xxhello", "", "a", false, 0, 0), item("helloxx", "", "b", false, 0, 0)];
        assert_eq!(order(&items, "hello"), ["helloxx", "xxhello"]);
    }

    #[test]
    fn empty_query_floats_pinned_and_keeps_list_order() {
        let items = vec![
            item("甲", "", "a", false, 0, 0),
            item("乙", "", "b", true, 0, 0),
            item("丙", "", "c", false, 0, 0),
        ];
        assert_eq!(order(&items, ""), ["乙", "甲", "丙"]);
    }

    #[test]
    fn empty_query_sorts_by_recency() {
        // 次数全 0 ⇒ 甲档第一键（次数）平手，退到最近一次；乙档热度也全为 0，
        // 同样退到最近一次。两档在这份数据上结论一致。
        let items = vec![
            item("甲", "", "a", false, 0, 5),
            item("乙", "", "b", false, 0, 9),
            item("丙", "", "c", false, 0, 7),
        ];
        assert_eq!(order(&items, ""), ["乙", "丙", "甲"]);
        assert_eq!(order_at(&items, "", LearnRank::Decay, NOW), ["乙", "丙", "甲"]);
    }

    // ---- FR-QRY-07 截断与稳定性 ----

    #[test]
    fn results_are_truncated_to_50() {
        let items: Vec<LaunchItem> =
            (0..200).map(|i| item(&format!("项{i}"), "", &i.to_string(), false, 0, 0)).collect();
        let r = MatchIndex::default().search("项", &items, LearnRank::Count, NOW);
        assert_eq!(r.len(), 50);
        let again = MatchIndex::default().search("项", &items, LearnRank::Count, NOW);
        assert_eq!(r, again, "同一输入两次结果序列必须完全一致");
    }

    #[test]
    fn index_rebuilds_when_list_changes() {
        let mut items = vec![item("记事本", "jsb", "a", false, 0, 0)];
        let mut idx = MatchIndex::default();
        assert_eq!(idx.search("jsb", &items, LearnRank::Count, NOW).len(), 1);
        items.push(item("计算器", "jsq", "b", false, 0, 0));
        assert_eq!(idx.search("jsq", &items, LearnRank::Count, NOW).len(), 1, "清单变化后新项的别名必须立即可搜");
        // 只改别名（条目数不变）也必须触发重算 —— 这是用内容指纹而不是长度比较的理由
        items[0].keyword = "jbb".to_string();
        assert_eq!(idx.search("jbb", &items, LearnRank::Count, NOW).len(), 1);
        assert!(idx.search("jsb", &items, LearnRank::Count, NOW).is_empty(), "旧别名不应继续命中");
    }

    #[test]
    fn empty_browse_view_is_not_truncated() {
        // 截断 50 只作用于搜索结果；空输入的「浏览整张清单」必须看得到全部项。
        let items: Vec<LaunchItem> =
            (0..120).map(|i| item(&format!("项{i}"), "", &i.to_string(), false, 0, 0)).collect();
        assert_eq!(MatchIndex::default().search("", &items, LearnRank::Count, NOW).len(), 120);
    }

    // ---- FR-ACT-06 别名多键与 Tab 补全判据 ----

    #[test]
    fn aliases_split_on_space_so_they_do_not_blend_initials() {
        // 「提交即学」会让 keyword 里出现多个空格分隔的触发词。合成一键时
        // initials 取整串词首字母 ⇒ "jsq qsj" 变成 "jq"，"jq" 这种查询会白吃 75 档噪声。
        let items = vec![item("计算器", "jsq qsj", "a", false, 0, 0)];
        assert_eq!(hit(&items, "jsq"), (T_EXACT, 100.0), "别名整词该按整词档位命中");
        assert_eq!(hit(&items, "qsj"), (T_EXACT, 100.0), "第二个别名同理");
        assert_eq!(hit(&items, "jq"), (0, 0.0), "跨别名的假缩写不得命中");
    }

    #[test]
    fn can_complete_needs_name_or_alias_prefix() {
        let cn = item("计算器", "jsq", "a", false, 0, 0);
        assert!(can_complete("计", &cn), "名称前缀可补全");
        assert!(can_complete("jsq", &cn), "别名整词也要能补全成名称——敲首字母才是常态");
        assert!(!can_complete("计算器", &cn), "已经等于名称时无可补全 ⇒ 该走导航");
        assert!(!can_complete("", &cn), "空输入没什么可补全");
        assert!(!can_complete("xj", &cn), "既不冠名称也不冠别名 ⇒ 走导航");
        let en = item("Notepad", "", "b", false, 0, 0);
        assert!(can_complete("no", &en), "纯英文名同样成立");
        assert!(can_complete("NO", &en), "大小写不敏感");
        let multi = item("Visual Studio Code", "vsc", "c", false, 0, 0);
        assert!(can_complete("visual", &multi), "多词名称的前缀");
        assert!(can_complete("vsc", &multi), "首字母别名补全成整名");
    }

    /// 甲案核心回归（2026-10-18 真人复测发现的缺陷）：
    /// `Tas` 命中 Task Manager / Task Scheduler / Camtasia Recorder / Tutorials 四条，
    /// 旧口径只问「选中项的名字可补吗」⇒ 补成 Task Manager，查询变成整名之后
    /// 派生结果集塌成 1 行，Task Scheduler 再也到不了（要退回只能重打）。
    #[test]
    fn completion_waits_until_the_candidate_set_is_unambiguous() {
        let items = vec![
            item("Task Manager", "", "a", false, 0, 0),
            item("Task Scheduler", "", "b", false, 0, 0),
            item("Camtasia Recorder", "", "c", false, 0, 0),
            item("Tutorials", "", "d", false, 0, 0),
        ];
        let mut idx = MatchIndex::default();
        let results = idx.search("tas", &items, LearnRank::Count, NOW);
        assert_eq!(results.len(), 4, "四条都该在结果里");
        assert_eq!(
            unique_completion("tas", &items, &results),
            None,
            "两条可补 ⇒ 歧义未消，不猜第一条，交给 Tab 导航"
        );

        // 打到只剩一条时照常补（这一步结果集只有一条，可补全即唯一）
        let results = idx.search("task sch", &items, LearnRank::Count, NOW);
        assert_eq!(results.len(), 1);
        assert_eq!(unique_completion("task sch", &items, &results), Some(0));

        // 甲案不能顺手关掉「唯一别名 ⇒ 补成整名」，那才是中文启动器最值钱的场景
        let cn = vec![
            item("计算器", "jsq", "a", false, 0, 0),
            item("控制面板", "kzmb", "b", false, 0, 0),
        ];
        let results = MatchIndex::default().search("jsq", &cn, LearnRank::Count, NOW);
        assert_eq!(unique_completion("jsq", &cn, &results), Some(0), "jsq→计算器 仍该补");

        // 兄弟别名撞前缀时两边都不补：`js` 既冠 jsq(计算器) 又冠名称「记事本」
        let cn2 = vec![
            item("计算器", "jsq", "a", false, 0, 0),
            item("记事本", "jsb", "b", false, 0, 0),
        ];
        let results = MatchIndex::default().search("js", &cn2, LearnRank::Count, NOW);
        assert_eq!(unique_completion("js", &cn2, &results), None);
    }

    /// 批次 J（丙案）判据的整串相等口径：只认名称整串或别名表里的**某一个词**，
    /// 前缀、后缀、多词拼接都不算 —— 否则「打到一半就误启动」回来了。
    #[test]
    fn full_word_needs_exact_name_or_one_alias() {
        let note = item("记事本", "jsb", "a", false, 0, 0);
        let task = item("Task Manager", "tas task ta", "b", false, 0, 0);
        assert!(query_is_full_word("jsb", &note));
        assert!(query_is_full_word("记事本", &note));
        assert!(query_is_full_word("  JSB  ", &note), "首尾空白与大小写不参与判断");
        assert!(!query_is_full_word("js", &note), "前缀不是全名");
        assert!(!query_is_full_word("jsbx", &note));
        assert!(!query_is_full_word("", &note), "空框由 space_executes 自己判");
        assert!(query_is_full_word("task", &task));
        assert!(
            !query_is_full_word("Task Scheduler", &task),
            "别人的名称不是它的别名"
        );
        assert!(
            !query_is_full_word("tas task", &task),
            "整串必须是词表里的一个词，多词拼接不赦（这种查询本来就还需要接着打）"
        );
    }

    /// 四档的分派表。每条断言都可复述成一句人话，方便设置页文案与需求书对齐口径。
    #[test]
    fn space_executes_dispatches_on_observable_state_only() {
        let items = vec![
            item("记事本", "jsb", "a", false, 0, 0),
            item("计算器", "jsq", "b", false, 0, 0),
        ];
        let mut idx = MatchIndex::default();
        let r_all = idx.search("", &items, LearnRank::Count, NOW); // 2 条
        let r_two = idx.search("js", &items, LearnRank::Count, NOW); // 2 条（js 冠 jsb 与 jsq）
        let r_one = idx.search("jsb", &items, LearnRank::Count, NOW); // 1 条
        assert_eq!((r_all.len(), r_two.len(), r_one.len()), (2, 2, 1));
        let none: Vec<usize> = Vec::new();
        let run = |m: SpaceRun, q: &str, r: &[usize]| space_executes(m, q, &items, r, 0);

        // off ＝ 批次 F 的老口径，一个字都不改
        assert!(run(SpaceRun::Off, "", &r_all));
        assert!(!run(SpaceRun::Off, "jsb", &r_one));
        assert!(run(SpaceRun::Off, " ", &r_all), "off 档按 trim 判空框（旧行为保留）");
        assert!(!run(SpaceRun::Off, "", &none), "没有结果就没有可执行的东西");

        // word ＝ 丙案默认档：空框，或查询整串＝名称/别名
        assert!(run(SpaceRun::Word, "", &r_all));
        assert!(run(SpaceRun::Word, "jsb", &r_one), "别名整词 ⇒ 空格即执行");
        assert!(run(SpaceRun::Word, "记事本", &r_one));
        assert!(!run(SpaceRun::Word, "js", &r_two), "前缀阶段空格仍是文本，能接着打");
        assert!(
            !run(SpaceRun::Word, " ", &r_all),
            "Shift+空格 插出来的真空格不算空框，否则下一按空格就误启动"
        );
        assert!(!run(SpaceRun::Word, "jsb", &none));

        // unique ＝ 乙案：结果集塌到一条即可确认
        assert!(run(SpaceRun::Unique, "jsb", &r_one));
        assert!(!run(SpaceRun::Unique, "js", &r_two));
        assert!(run(SpaceRun::Unique, "", &r_all));

        // always ＝ 原 true：恒执行（打不出空格，代价由用户自己认）
        assert!(run(SpaceRun::Always, "js", &r_two));
        assert!(run(SpaceRun::Always, "", &none));
    }

    /// FR-QRY-06 验收（NFR-01 首字符 ≤16ms）：1000 条清单的单次过滤计时。
    /// 口径要说清：**只覆盖匹配与排序，不含 egui 布局/绘制**，且先把键表建好再计时
    /// （首次建表是清单变更时的一次性成本，不在每次敲键的路径上）。
    #[test]
    fn thousand_items_match_within_frame_budget() {
        let items: Vec<LaunchItem> = (0..1000)
            .map(|i| item(&format!("工具{i} Terminal"), &format!("g{i}"), &i.to_string(), false, 0, 0))
            .collect();
        let mut idx = MatchIndex::default();
        idx.search("", &items, LearnRank::Count, NOW); // 预热：建表
        let mut worst = 0f64;
        for q in ["工", "工具5", "term", "g500", "具5 Terminal"] {
            let t = std::time::Instant::now();
            let n = idx.search(q, &items, LearnRank::Count, NOW).len();
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            eprintln!("FR-QRY-06：1000 条 query={q:?} 命中 {n} 用时 {ms:.3} ms");
            worst = worst.max(ms);
        }
        assert!(worst <= 16.0, "单帧预算 16ms，实测最差 {worst:.3}ms");
    }
}

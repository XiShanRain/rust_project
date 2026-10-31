//! 批次 F13（FR-SYS-04）：界面多语言。
//!
//! 口径（2026-10-31 用户裁定）：
//! - **只翻界面文案**。条目名称 / 别名属于用户数据，一律保持原样；日志也不翻
//!   （日志是取证材料，换语言只会让前后两轮对不上）。
//! - `language = auto` 跟随系统 UI 语言（`GetUserDefaultLocaleName`）；只有用户显式点过
//!   某一档才写进 `settings.json`（`None` 不落盘，与 `log_level` 同一条理由）。
//! - 语言包以 `include_str!` 编进程序，exe 同级的 `locales\<tag>.json` 同名覆盖——
//!   与 `Themes\` 同一语义（用户可改、坏了回落内置、不影响启动）。
//! - 键集必须两边一致；`{1}{2}` 这类占位符个数也必须一致，由单测钉住。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

/// 当前支持的界面语言。序列化名就是 BCP-47 标签，写进 `settings.json` 时一眼可读。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Language {
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en")]
    En,
}

impl Language {
    pub fn tag(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::En => "en",
        }
    }

    /// 语言名用**它自己的语言**写（简体中文 / English）：语言选择器里这两种写法最不容易认错。
    pub fn label(self) -> &'static str {
        match self {
            Self::ZhCn => "简体中文",
            Self::En => "English",
        }
    }

    /// 系统标签 → 语言：按主语言子标签匹配（`zh-Hans-CN`、`en-US` 都认）。
    pub fn from_tag(tag: &str) -> Option<Self> {
        let lower = tag.to_ascii_lowercase();
        let primary = lower.split(['-', '_']).next().unwrap_or("");
        match primary {
            "zh" => Some(Self::ZhCn),
            "en" => Some(Self::En),
            _ => None,
        }
    }
}

/// 设置值 + 系统语言 → 实际生效的语言。`None`（跟随系统）是唯一的分支点，
/// 所以它是个能单独跑单测的纯函数。
pub fn resolve(pref: Option<Language>, system: Language) -> Language {
    pref.unwrap_or(system)
}

/// 系统 UI 语言；取不到时回落简体中文（与程序出厂口径一致）。
pub fn system_language() -> Language {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    let len = unsafe { GetUserDefaultLocaleName(&mut buf) };
    if len <= 0 {
        return Language::ZhCn;
    }
    let name = String::from_utf16_lossy(&buf[..(len as usize - 1)]);
    Language::from_tag(&name).unwrap_or(Language::ZhCn)
}

const ZH_CN_BUILTIN: &str = include_str!("../locales/zh-CN.json");
const EN_BUILTIN: &str = include_str!("../locales/en.json");

/// 一份语言包：元键单列，其余全进 `strings`。
struct Locale {
    strings: HashMap<&'static str, &'static str>,
    name: &'static str,
    font: &'static str,
    fallback: &'static str,
}

#[derive(serde::Deserialize)]
struct RawLocale {
    #[serde(rename = "_name")]
    name: String,
    #[serde(rename = "_font")]
    font: String,
    #[serde(default, rename = "_fallback")]
    fallback: String,
    #[serde(flatten)]
    strings: HashMap<String, String>,
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn parse(raw: &str) -> Result<Locale, String> {
    let parsed: RawLocale = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let mut strings = HashMap::with_capacity(parsed.strings.len());
    for (k, v) in parsed.strings {
        strings.insert(leak(k), leak(v));
    }
    Ok(Locale {
        strings,
        name: leak(parsed.name),
        font: leak(parsed.font),
        fallback: leak(parsed.fallback),
    })
}

/// exe 同级 `locales\<tag>.json` 覆盖内置；读不到 / 解析失败都回落内置并记一条。
fn load_one(tag: &str, builtin: &'static str) -> Locale {
    let builtin_locale = parse(builtin).unwrap_or_else(|e| panic!("内置语言包 {tag} 解析失败: {e}"));
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
    else {
        return builtin_locale;
    };
    let path = dir.join("locales").join(format!("{tag}.json"));
    let Ok(text) = std::fs::read_to_string(&path) else {
        return builtin_locale;
    };
    match parse(&text) {
        Ok(locale) => {
            log_info!("i18n: 语言包使用外部文件 locales/{tag}.json");
            locale
        }
        Err(e) => {
            log_info!("i18n: locales/{tag}.json 解析失败（{e}）⇒ 用内置");
            builtin_locale
        }
    }
}

fn locales() -> &'static [Locale; 2] {
    static LOCALES: OnceLock<[Locale; 2]> = OnceLock::new();
    LOCALES.get_or_init(|| {
        [
            load_one(Language::ZhCn.tag(), ZH_CN_BUILTIN),
            load_one(Language::En.tag(), EN_BUILTIN),
        ]
    })
}

fn index(lang: Language) -> usize {
    match lang {
        Language::ZhCn => 0,
        Language::En => 1,
    }
}

/// 当前生效语言。默认简体中文；启动路径在装字体 / 建托盘之前先 [`set_language`]。
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn language() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::En,
        _ => Language::ZhCn,
    }
}

pub fn set_language(lang: Language) {
    CURRENT.store(index(lang) as u8, Ordering::Relaxed);
}

/// 缺键只记一次：界面每帧都会取同一批键，不设闸门会把详细档日志刷满。
fn missing_once(key: &'static str) {
    static SEEN: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = seen.lock().unwrap_or_else(|e| e.into_inner());
    if guard.insert(key) {
        log_debug!("i18n: 缺键「{key}」⇒ 回落 zh-CN");
    }
}

fn lookup(locales: &[Locale; 2], lang: Language, key: &'static str) -> Option<&'static str> {
    locales[index(lang)].strings.get(key).copied()
}

fn t_in(locales: &[Locale; 2], lang: Language, key: &'static str) -> &'static str {
    if let Some(v) = lookup(locales, lang, key) {
        return v;
    }
    missing_once(key);
    lookup(locales, Language::ZhCn, key).unwrap_or(key)
}

/// 当前语言的一条文案。键缺失时回落 zh-CN，再缺失就把键名原样返回（开发期一眼看得出）。
pub fn t(key: &'static str) -> &'static str {
    t_in(locales(), language(), key)
}

/// 指定语言的一条文案（纯函数入口，单测用它，不用改全局状态）。
/// 生产路径一律走 [`t`]（读全局当前语言），所以非测试构建里这个入口没人调用。
#[cfg(test)]
pub fn tr(lang: Language, key: &'static str) -> &'static str {
    t_in(locales(), lang, key)
}

/// 带占位符的文案：`tf("app.no_match", &[query])` 把 `{1}` 换成 `query`。
pub fn tf(key: &'static str, args: &[&str]) -> String {
    let mut text = t(key).to_string();
    for (i, arg) in args.iter().enumerate() {
        text = text.replace(&format!("{{{}}}", i + 1), arg);
    }
    text
}

/// 主题 `font_name = "auto"` 时用哪套字：中文 → 微软雅黑，英文 → Segoe UI。
pub fn auto_font() -> &'static str {
    font_for(language())
}

fn font_for(lang: Language) -> &'static str {
    locales()[index(lang)].font
}

/// 元键（`_name` / `_fallback`）；测试与诊断用。
#[allow(dead_code)]
pub fn meta(key: &str) -> &'static str {
    let locale = &locales()[index(language())];
    match key {
        "_name" => locale.name,
        "_fallback" => locale.fallback,
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(lang: Language) -> Vec<&'static str> {
        let mut v: Vec<_> = locales()[index(lang)].strings.keys().copied().collect();
        v.sort_unstable();
        v
    }

    fn placeholders(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find('{') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('}') else { break };
            let inner = &after[..end];
            if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()) {
                out.push(inner.to_string());
            }
            rest = &after[end + 1..];
        }
        out.sort_unstable();
        out
    }

    #[test]
    fn both_locales_have_the_same_key_set() {
        let zh = keys(Language::ZhCn);
        let en = keys(Language::En);
        assert!(!zh.is_empty(), "zh-CN 语言包不许为空");
        assert_eq!(zh, en, "两语言键集必须完全一致");
    }

    #[test]
    fn placeholders_match_key_by_key() {
        for key in keys(Language::ZhCn) {
            let zh = placeholders(tr(Language::ZhCn, key));
            let en = placeholders(tr(Language::En, key));
            assert_eq!(zh, en, "键 {key} 的占位符不一致");
        }
    }

    #[test]
    fn resolve_follows_system_unless_explicitly_set() {
        assert_eq!(resolve(None, Language::En), Language::En);
        assert_eq!(resolve(None, Language::ZhCn), Language::ZhCn);
        assert_eq!(resolve(Some(Language::ZhCn), Language::En), Language::ZhCn);
        assert_eq!(resolve(Some(Language::En), Language::ZhCn), Language::En);
    }

    #[test]
    fn tags_and_system_names_resolve() {
        assert_eq!(Language::from_tag("zh-Hans-CN"), Some(Language::ZhCn));
        assert_eq!(Language::from_tag("zh_CN"), Some(Language::ZhCn));
        assert_eq!(Language::from_tag("en-US"), Some(Language::En));
        assert_eq!(Language::from_tag("fr-FR"), None);
        assert_eq!(
            Language::from_tag(Language::En.tag()),
            Some(Language::En)
        );
    }

    #[test]
    fn missing_key_falls_back_to_chinese_then_key() {
        let mut zh = HashMap::new();
        zh.insert("only.zh", "中文");
        let mut en = HashMap::new();
        en.insert("only.en", "english");
        let fake = [
            Locale {
                strings: zh,
                name: "zh",
                font: "微软雅黑",
                fallback: "",
            },
            Locale {
                strings: en,
                name: "en",
                font: "Segoe UI",
                fallback: "zh-CN",
            },
        ];
        assert_eq!(t_in(&fake, Language::En, "only.zh"), "中文");
        assert_eq!(t_in(&fake, Language::En, "only.en"), "english");
        assert_eq!(t_in(&fake, Language::En, "nope"), "nope");
    }

    #[test]
    fn tf_replaces_numbered_placeholders() {
        let text = tf("app.no_match", &["probe"]);
        assert!(text.contains("probe"), "{text}");
        assert!(!text.contains("{1}"), "{text}");
    }

    #[test]
    fn auto_font_is_resolvable_by_the_theme_layer() {
        assert_eq!(font_for(Language::ZhCn), "微软雅黑");
        assert_eq!(font_for(Language::En), "Segoe UI");
        // 名字必须真的能落到 Windows 字体文件上，否则 auto 会静默退回 egui 自带字体。
        assert_eq!(
            crate::theme::font_candidates(font_for(Language::ZhCn))
                .first()
                .map(String::as_str),
            Some("msyh.ttc")
        );
        assert_eq!(
            crate::theme::font_candidates(font_for(Language::En))
                .first()
                .map(String::as_str),
            Some("segoeui.ttf")
        );
    }
}

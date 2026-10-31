//! 批次 G5（FR-SYS-01）：手动检查更新。
//!
//! 口径（2026-10-31 开发计划 4.5 修订）：**只查、不换**。拉一次 GitHub Releases 的
//! `latest` 接口，把结果（已最新 / 有新版本 / 暂无发布 / 失败原因）交给界面显示；
//! 不下载安装包、不算 SHA256、不替换 exe、不回滚——自动更新是 M6 明确不做的子系统。
//!
//! 网络层复用 [`crate::source::siteicon::fetch_winhttp`]（WinHTTP、跟系统代理、
//! 5/5/8/8 s 超时）。本模块里除了 [`check`] 那一层薄封装，全是纯函数：
//! 版本号比较、JSON 解析、HTTP 响应分类都能直接单测。

use crate::source::siteicon::FetchReply;

/// GitHub 的 `latest` 发布接口。仓库地址与 `Cargo.toml` 的 `repository` 同源。
pub const RELEASES_API: &str =
    "https://api.github.com/repos/XiShanRain/rust_project/releases/latest";

/// 最多收多少响应体。发布说明可能很长，但 256 KB 足够容纳正常条目；
/// 超出的部分不读，交给 [`classify`] 报「响应太大」而不是拿半截 JSON 去解析。
pub const FETCH_LIMIT: usize = 256 * 1024;

/// 一条发布里真正要用的两个字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// 原始标签（`v0.2.0` 这样带不带 `v` 都照原样留着，用于显示）。
    pub tag: String,
    /// 发布页地址（只接受 http/https，防止把别的协议交给 `ShellExecute`）。
    pub url: String,
}

/// 失败原因。**只存分类、不存原始错误串**：界面按分类取本语言的文案，
/// WinHTTP 的中文错误串留在详细档日志里（那里本来就不翻）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailKind {
    /// 连不上 / 被代理挡住 / TLS 失败……一句话：没拿到响应。
    Network,
    /// 拿到了响应，但不是 200 也不是 404。
    Http(u16),
    /// 响应体不是预期的 JSON，或缺字段 / 字段不合法。
    Parse,
    /// 标签不是能比较的版本号（例如 `nightly`）。
    Version,
    /// 响应超过 [`FETCH_LIMIT`]——半截 JSON 解析不了，单独报明白。
    Truncated,
}

impl FailKind {
    /// 给界面用的一句话（随当前语言）。
    pub fn message(self) -> String {
        use crate::i18n::{t, tfmt};
        match self {
            Self::Network => t("update.fail.network").to_string(),
            Self::Http(code) => tfmt("update.fail.http", &[&code]),
            Self::Parse => t("update.fail.parse").to_string(),
            Self::Version => t("update.fail.version").to_string(),
            Self::Truncated => t("update.fail.truncated").to_string(),
        }
    }

    /// 详细档日志用的稳定写法（不翻）。
    pub fn log_tag(self) -> String {
        match self {
            Self::Network => "network".to_string(),
            Self::Http(code) => format!("http {code}"),
            Self::Parse => "parse".to_string(),
            Self::Version => "version".to_string(),
            Self::Truncated => "truncated".to_string(),
        }
    }
}

/// 一次检查的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// 当前版本不低于最新发布（`latest` 是发布标签，原样显示）。
    UpToDate { latest: String },
    /// 最新发布比当前版本新。
    Newer { latest: String, url: String },
    /// 仓库还没有发布（GitHub 对「无 latest release」回 404）。
    NoRelease,
    /// 没查成。
    Failed(FailKind),
}

/// `v0.10.1` → `[0, 10, 1]`。只认「点分十进制」，前缀 `v`/`V` 忽略。
///
/// 预发布后缀（`1.0.0-beta.2`）在第一个非纯数字的小节处截断 ⇒ 与 `1.0.0` 视为同版。
/// 宁可把预发布当成「已最新」，也不要在正式版没出时催用户去下 beta——
/// 这个比较只决定一句提示，不值得为它实现 SemVer 全套。
pub fn parse_version(text: &str) -> Option<Vec<u64>> {
    let s = text.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    if s.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for part in s.split('.') {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            break;
        }
        out.push(digits.parse().ok()?);
        if digits.len() != part.len() {
            break;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 最新版是否比当前版新。任一边解析不了 ⇒ `None`（调用方报「版本号读不懂」）。
/// 短的一边按 0 补齐：`0.2` 与 `0.2.0` 相等。
pub fn is_newer(latest: &str, current: &str) -> Option<bool> {
    let a = parse_version(latest)?;
    let b = parse_version(current)?;
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return Some(x > y);
        }
    }
    Some(false)
}

#[derive(serde::Deserialize)]
struct RawRelease {
    tag_name: String,
    html_url: String,
}

/// 解析 GitHub 的发布 JSON。只取 `tag_name` 与 `html_url`，其余字段一律不管；
/// 两个字段缺一个、空一个、或者 `html_url` 不是网页地址，都算解析失败。
pub fn parse_release(json: &str) -> Result<Release, String> {
    let raw: RawRelease = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let tag = raw.tag_name.trim().to_string();
    if tag.is_empty() {
        return Err("tag_name 为空".to_string());
    }
    let url = raw.html_url.trim().to_string();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("html_url 不是网页地址".to_string());
    }
    Ok(Release { tag, url })
}

/// HTTP 响应 → 检查结论。把「状态码 / 截断 / JSON / 版本」四道闸门放在一处，
/// 这样单测能逐格覆盖，真机上只剩"网络是不是通"这一个变量。
pub fn classify(reply: FetchReply, current: &str) -> Status {
    if reply.status == 404 {
        return Status::NoRelease;
    }
    if reply.status != 200 {
        return Status::Failed(FailKind::Http(reply.status));
    }
    if reply.truncated {
        return Status::Failed(FailKind::Truncated);
    }
    let body = String::from_utf8_lossy(&reply.body);
    let release = match parse_release(&body) {
        Ok(release) => release,
        Err(e) => {
            log_debug!("检查更新：解析发布 JSON 失败（{e}）");
            return Status::Failed(FailKind::Parse);
        }
    };
    match is_newer(&release.tag, current) {
        Some(true) => Status::Newer {
            latest: release.tag,
            url: release.url,
        },
        Some(false) => Status::UpToDate {
            latest: release.tag,
        },
        None => Status::Failed(FailKind::Version),
    }
}

/// 联网查一次。**阻塞**，调用方放到后台线程里（最长 8 s 一档的超时）。
pub fn check(current: &str) -> Status {
    match crate::source::siteicon::fetch_winhttp(RELEASES_API, FETCH_LIMIT) {
        Ok(reply) => classify(reply, current),
        Err(e) => {
            // 错误串来自 WinHTTP 层（连不上 / 发送失败…），只进日志，不上界面。
            log_debug!("检查更新：网络失败（{e}）");
            Status::Failed(FailKind::Network)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u16, body: &str) -> FetchReply {
        FetchReply {
            status,
            body: body.as_bytes().to_vec(),
            truncated: false,
        }
    }

    const RELEASE_JSON: &str = r#"{
        "tag_name": "v0.2.0",
        "html_url": "https://github.com/XiShanRain/rust_project/releases/tag/v0.2.0",
        "name": "v0.2.0",
        "body": "notes"
    }"#;

    #[test]
    fn version_parses_tag_prefix_and_dots() {
        assert_eq!(parse_version("v0.10.1"), Some(vec![0, 10, 1]));
        assert_eq!(parse_version("V2"), Some(vec![2]));
        assert_eq!(parse_version(" 1.2 "), Some(vec![1, 2]));
    }

    /// 0.10 > 0.9 是数字比较，不是字符串比较（按字面 "0.10" < "0.9"）。
    #[test]
    fn numeric_compare_beats_lexicographic() {
        assert_eq!(is_newer("0.10.0", "0.9.0"), Some(true));
        assert_eq!(is_newer("0.9.0", "0.10.0"), Some(false));
    }

    #[test]
    fn equal_versions_pad_with_zero() {
        assert_eq!(is_newer("v0.2", "0.2.0"), Some(false));
        assert_eq!(is_newer("0.2.0", "v0.2"), Some(false));
        assert_eq!(is_newer("0.2.1", "0.2"), Some(true));
    }

    #[test]
    fn prerelease_suffix_is_conservatively_equal() {
        assert_eq!(parse_version("1.0.0-beta.2"), Some(vec![1, 0, 0]));
        assert_eq!(is_newer("1.0.0-beta.2", "1.0.0"), Some(false));
    }

    #[test]
    fn invalid_versions_are_none() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("v"), None);
        assert_eq!(parse_version("abc"), None);
        assert_eq!(parse_version("v1.x"), Some(vec![1]));
        assert_eq!(is_newer("nightly", "0.1.0"), None);
        assert_eq!(is_newer("0.1.0", "release"), None);
    }

    #[test]
    fn release_requires_tag_and_http_url() {
        let r = parse_release(RELEASE_JSON).unwrap();
        assert_eq!(r.tag, "v0.2.0");
        assert!(r.url.starts_with("https://"));
        assert!(parse_release(r#"{"html_url":"https://example.com"}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"v1","html_url":"file:///x"}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"  ","html_url":"https://example.com"}"#).is_err());
        assert!(parse_release("not json").is_err());
    }

    #[test]
    fn classify_newer_and_same() {
        assert_eq!(
            classify(reply(200, RELEASE_JSON), "0.1.0"),
            Status::Newer {
                latest: "v0.2.0".to_string(),
                url: "https://github.com/XiShanRain/rust_project/releases/tag/v0.2.0".to_string(),
            }
        );
        assert_eq!(
            classify(reply(200, RELEASE_JSON), "0.2.0"),
            Status::UpToDate {
                latest: "v0.2.0".to_string(),
            }
        );
        // 本机比发布还新（开发中途）：同样算「已最新」，不提示降级。
        assert_eq!(
            classify(reply(200, RELEASE_JSON), "0.3.0"),
            Status::UpToDate {
                latest: "v0.2.0".to_string(),
            }
        );
    }

    #[test]
    fn classify_404_is_no_release() {
        assert_eq!(classify(reply(404, r#"{"message":"Not Found"}"#), "0.1.0"), Status::NoRelease);
    }

    #[test]
    fn classify_http_and_truncated_fail() {
        assert_eq!(
            classify(reply(403, "rate limited"), "0.1.0"),
            Status::Failed(FailKind::Http(403))
        );
        let mut cut = reply(200, RELEASE_JSON);
        cut.truncated = true;
        assert_eq!(classify(cut, "0.1.0"), Status::Failed(FailKind::Truncated));
    }

    #[test]
    fn classify_bad_json_and_bad_tag_fail() {
        assert_eq!(
            classify(reply(200, "not json"), "0.1.0"),
            Status::Failed(FailKind::Parse)
        );
        assert_eq!(
            classify(
                reply(
                    200,
                    r#"{"tag_name":"nightly","html_url":"https://example.com/r"}"#
                ),
                "0.1.0"
            ),
            Status::Failed(FailKind::Version)
        );
    }

    /// 失败分类必须能落到本语言的文案上（键漏了会回落成键名，界面上很扎眼）。
    #[test]
    fn fail_kinds_have_readable_messages() {
        for kind in [
            FailKind::Network,
            FailKind::Http(500),
            FailKind::Parse,
            FailKind::Version,
            FailKind::Truncated,
        ] {
            let msg = kind.message();
            assert!(!msg.is_empty(), "空文案：{kind:?}");
            assert_ne!(msg, "update.fail.network", "缺键：{kind:?}");
            assert!(!msg.contains("{1}"), "占位符没替换：{msg}");
        }
    }
}

//! token 脱敏：任何落日志/事件的文字先过它。

/// 写 events.log 前脱敏：链接即凭据，日志里不能出现 token
/// （cloudflared 的请求日志可能带 ?token= 查询串）
pub(crate) fn redact_token(s: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"(?i)token=[^&\s"']+"#).unwrap());
    re.replace_all(s, "token=<redacted>").into_owned()
}

#[cfg(test)]
mod tests {
    use super::redact_token;

    #[test]
    fn redact_token_strips_credential() {
        assert_eq!(
            redact_token("dest=https://a-b-c.trycloudflare.com/?token=abc123xyz&type=http"),
            "dest=https://a-b-c.trycloudflare.com/?token=<redacted>&type=http"
        );
        assert_eq!(
            redact_token("link https://x.trycloudflare.com/?token=deadbeef"),
            "link https://x.trycloudflare.com/?token=<redacted>"
        );
        assert_eq!(redact_token("no token here"), "no token here");
    }
}

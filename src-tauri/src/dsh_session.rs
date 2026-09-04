//! dsh 会话凭证（0.1.2 起 BrowserAuth，无关闭开关，回环也在门内）。
//! 两种凭证的获取与解析集中于此（prep 文档 §二契约、§10.3.2 修法）：
//!   - launch token：每 dsh 进程一枚，stdout 就绪行 `dsh web: http://127.0.0.1:<port>/?token=<t>`
//!     打印（process.rs 捕获；就绪行晚于 HTTP 绑定，必须持续 pump 不能假设 ready 即有）
//!   - 会话 cookie：`GET /?token=<t>` → 303 + Set-Cookie `dsh-auth-<hash>=v1.…`
//!     （HttpOnly/SameSite=Strict，绑 `127.0.0.1:<port>` authority——换端口即失效要重换）
//! 凭证只在内存、不落盘；日志一律脱敏（token 经 remote::redact_token，cookie 不记值）。

use std::sync::Arc;

/// 一路 dsh 进程的会话凭证：端口 + launch token（cookie 按需经 exchange_cookie 换取）
#[derive(Debug, Clone)]
pub struct DshCreds {
    pub port: u16,
    pub token: Arc<str>,
}

/// stdout 就绪行解析：命中 `dsh web: http://127.0.0.1:<port>/?token=<token>` 返回
/// (port, token)。0.0.0.0 绑定时主 URL 无 token、token 在 "(LAN: …?token=…)" 后缀
/// （prep §2.5），故 token 在全行范围找、port 只认第一个 127.0.0.1 地址。
/// 非就绪行（如 "dsh web: opening the default browser"）返回 None。
pub fn parse_ready_line(line: &str) -> Option<(u16, String)> {
    use std::sync::OnceLock;
    static RE_PORT: OnceLock<regex::Regex> = OnceLock::new();
    static RE_TOKEN: OnceLock<regex::Regex> = OnceLock::new();
    let rest = line.trim().strip_prefix(crate::upstream::READY_URL_PREFIX)?;
    let port_re = RE_PORT.get_or_init(|| regex::Regex::new(r"http://127\.0\.0\.1:(\d+)").unwrap());
    let token_re =
        RE_TOKEN.get_or_init(|| regex::Regex::new(r"\?token=([A-Za-z0-9_-]+)").unwrap());
    let port: u16 = port_re.captures(rest)?[1].parse().ok()?;
    let token = token_re.captures(rest)?[1].to_string();
    Some((port, token))
}

/// token 换 cookie：`GET http://127.0.0.1:{port}/?token=…` 应得 303 + dsh-auth
/// Set-Cookie；返回 "name=value" 对（Cookie 头直接可用）。303 不跟随（跟随会把
/// cookie 藏在重定向历史里，且我们要校验 303 这个契约状态码本身）。
/// no_proxy：系统代理（Clash 等）会劫持 127.0.0.1，回环请求必须直连。
pub async fn exchange_cookie(port: u16, token: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("http://127.0.0.1:{port}/?token={token}");
    let resp = client
        .get(&url)
        .send()
        .await
        // without_url：reqwest 错误 Display 默认带 "for url (…/?token=…)"，
        // token 会随错误串落日志（0.5.1 events.log 实踩明文）
        .map_err(|e| format!("token 交换请求失败: {}", e.without_url()))?;
    let status = resp.status().as_u16();
    if status != 303 {
        return Err(format!("token 交换应得 303，实际 {status}（token 已轮换或契约漂移）"));
    }
    for v in resp.headers().get_all(reqwest::header::SET_COOKIE) {
        let Ok(s) = v.to_str() else { continue };
        if s.starts_with(crate::upstream::DSH_AUTH_COOKIE_PREFIX) {
            if let Some(nv) = parse_set_cookie(s) {
                return Ok(nv);
            }
        }
    }
    Err("token 交换响应缺 dsh-auth Set-Cookie".into())
}

/// Set-Cookie 头取 "name=value"（第一个 ';' 前的属性剥离）；无 '=' 视为非法返回 None
pub fn parse_set_cookie(set_cookie: &str) -> Option<String> {
    let pair = set_cookie.split(';').next()?.trim();
    let (name, _) = pair.split_once('=')?;
    if name.is_empty() {
        return None;
    }
    Some(pair.to_string())
}

/// 请求用 Cookie 头值（即 "name=value" 对本身；命名接缝，调用点不说字面量）
pub fn cookie_header(name_value: &str) -> String {
    name_value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ready_line() {
        assert_eq!(
            parse_ready_line("dsh web: http://127.0.0.1:49152/?token=abc_-X1"),
            Some((49152, "abc_-X1".into()))
        );
        // 0.0.0.0 绑定形态：主 URL 无 token，token 在 LAN 后缀（prep §2.5）
        assert_eq!(
            parse_ready_line("dsh web: http://127.0.0.1:1/ (LAN: http://10.0.0.1:1/?token=x)"),
            Some((1, "x".into()))
        );
        assert_eq!(parse_ready_line("dsh web: opening the default browser"), None);
        assert_eq!(parse_ready_line(""), None);
        // 非 dsh web 前缀的日志行不匹配
        assert_eq!(parse_ready_line("listening http://127.0.0.1:3080"), None);
    }

    #[test]
    fn parses_set_cookie() {
        assert_eq!(
            parse_set_cookie(
                "dsh-auth-X=v1.a.b; Max-Age=2592000; Path=/; HttpOnly; SameSite=Strict"
            ),
            Some("dsh-auth-X=v1.a.b".into())
        );
        assert_eq!(parse_set_cookie("invalid"), None);
        assert_eq!(parse_set_cookie("=v"), None);
    }

    /// 链接即凭据：交换失败的错误串不得含 token 与带 token 的 URL——reqwest 的
    /// Display 默认带 "for url (http://…/?token=…)"，0.5.1 实踩明文落 events.log
    #[tokio::test]
    async fn exchange_error_never_leaks_token_or_url() {
        // 拿一个刚关闭的端口保证连接失败（走 reqwest 网络错误分支）
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let err = exchange_cookie(port, "secret-token-xyz").await.unwrap_err();
        assert!(!err.contains("secret-token-xyz"), "错误串不得含 token：{err}");
        assert!(!err.contains("?token="), "错误串不得含带 token 的 URL：{err}");
        assert!(!err.contains("for url"), "reqwest URL 尾巴应剥掉：{err}");
    }
}

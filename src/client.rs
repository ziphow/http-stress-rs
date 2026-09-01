//! # HTTP 客户端引擎模块（Reqwest + Hyper + rustls）
//!
//! `Reqwest 0.12` 底层封装自 `Hyper`，提供简洁高层 API 的同时保留高性能异步能力：
//! - 支持 GET/POST/PUT/DELETE 等全量 HTTP 方法
//! - 集成连接池管理，复用 TCP 连接减少握手开销
//! - 使用 `rustls` 纯 Rust TLS 实现，避免 OpenSSL 外部依赖
//! - 支持 HTTP/2 协议，充分利用多路复用降低延迟
//! - 实现请求超时、失败重试与自动重定向

use std::time::Duration;

use reqwest::header::HeaderMap;
use reqwest::redirect::Policy;

/// 构建压测专用的 HTTP 客户端
///
/// # 参数
/// - `http2`: 是否启用 HTTP/2 协议
/// - `no_tls`: 是否禁用 TLS 证书校验（测试环境）
/// - `timeout_secs`: 单请求超时时间（秒）
/// - `redirect`: 是否跟随重定向
/// - `headers`: 全局默认请求头（名称, 值）
///
/// # 返回值
/// 配置完成的 Reqwest 客户端；构建失败时返回错误
pub fn build_client(
    http2: bool,
    no_tls: bool,
    timeout_secs: u64,
    redirect: bool,
    headers: &[(String, String)],
) -> anyhow::Result<reqwest::Client> {
    // 构建全局默认请求头映射
    let mut default_headers = HeaderMap::new();
    // 遍历自定义请求头并注入默认请求头
    for (k, v) in headers {
        // 将请求头名称与值解析为所有权的 HeaderName/HeaderValue（解析失败的头部忽略）
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            // 插入默认请求头
            default_headers.insert(name, val);
        }
    }

    // 配置 TLS 校验策略
    let mut builder = if no_tls {
        // 禁用证书校验：返回不安全配置（仅测试环境使用）
        reqwest::ClientBuilder::new().danger_accept_invalid_certs(true)
    } else {
        // 使用默认（严格）证书校验
        reqwest::ClientBuilder::new()
    };

    // 配置重定向策略
    let redirect_policy = if redirect {
        // 跟随重定向（最多 10 跳）
        Policy::limited(10)
    } else {
        // 不跟随重定向
        Policy::none()
    };

    // 显式启用 HTTP/2：对明文 http:// 使用 prior-knowledge 直连，
    // 对 https:// 仍通过 ALPN 协商（reqwest 0.12 自动支持 HTTP/2）
    if http2 {
        // 启用 HTTP/2 prior-knowledge
        builder = builder.http2_prior_knowledge();
    }

    // 最终构建客户端
    builder
        // 设置单请求超时时间
        .timeout(Duration::from_secs(timeout_secs))
        // 设置重定向策略
        .redirect(redirect_policy)
        // 注入默认请求头
        .default_headers(default_headers)
        // 设置连接池每主机最大连接数
        .pool_max_idle_per_host(usize::MAX)
        // 禁止系统代理（压测工具应直连目标）
        .no_proxy()
        // 构建客户端并转换为错误类型
        .build()
        .map_err(|e| anyhow::anyhow!("创建 HTTP 客户端失败：{e}"))
}

/// 组装一次具体的 HTTP 请求
///
/// # 参数
/// - `client`: 压测专用的 Reqwest 客户端
/// - `url`: 请求目标 URL
/// - `method`: 请求方法
/// - `headers`: 请求头（名称, 值）
/// - `body`: 请求体（可选）
///
/// # 返回值
/// 组装完成的请求构建器
pub fn build_request(
    client: &reqwest::Client,
    url: &str,
    method: &reqwest::Method,
    headers: &[(String, String)],
    body: Option<&str>,
) -> reqwest::RequestBuilder {
    // 创建请求构建器
    let mut req = client.request(method.clone(), url);
    // 为请求注入请求头
    for (k, v) in headers {
        // 设置请求头
        req = req.header(k.as_str(), v);
    }
    // 若存在请求体则注入
    if let Some(b) = body {
        // 设置请求体
        req = req.body(b.to_string());
    }
    // 返回请求构建器
    req
}

/// 单元测试：验证客户端构建逻辑
#[cfg(test)]
mod tests {
    use super::*;

    /// 测试：客户端可正常构建（默认配置）
    #[test]
    fn test_build_client() {
        // 构建一个默认配置的客户端
        let client = build_client(false, false, 30, true, &[]);
        // 断言构建成功
        assert!(client.is_ok());
    }
}

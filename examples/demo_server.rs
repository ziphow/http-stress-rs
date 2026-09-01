//! # 演示目标服务器示例
//!
//! 使用 Hyper 构建一个本地演示 HTTP 服务，用于对本压测工具进行实际压测验证。
//! 提供多个测试端点：
//! - `GET /`              返回固定文本（200）
//! - `GET /json`          返回 JSON 响应（200）
//! - `GET /delay/{ms}`    延迟指定毫秒后返回（200）
//! - `GET /error`         返回 500 错误
//! - `POST /echo`         回显请求体（200）
//!
//! 运行方式：`cargo run --example demo_server`（默认监听 127.0.0.1:8080）

use std::convert::Infallible;
use std::net::SocketAddr;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;

/// 演示服务器监听地址（可通过命令行参数覆盖）
const DEFAULT_ADDR: &str = "127.0.0.1:8080";

/// 处理单个 HTTP 请求
///
/// # 参数
/// - `req`: 收到的 HTTP 请求
///
/// # 返回值
/// 构造的 HTTP 响应
async fn handle(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    // 获取请求方法与路径
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    // 根据路径分发处理
    match path.as_str() {
        // 根路径：返回问候文本
        "/" => Ok(ok_response("hello from demo server")),
        // JSON 端点：返回 JSON 数据
        "/json" => Ok(ok_response(
            r#"{"message":"ok","timestamp":1234567890,"data":[1,2,3]}"#,
        )),
        // 延迟端点：按路径参数延迟响应
        _ if path.starts_with("/delay/") => {
            // 解析延迟毫秒数
            let ms: u64 = path
                // 去掉前缀获取参数
                .trim_start_matches("/delay/")
                // 解析为整数
                .parse()
                // 解析失败时默认 100ms
                .unwrap_or(100);
            // 异步延迟指定毫秒数
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            // 返回延迟响应
            Ok(ok_response(&format!("delayed {ms}ms")))
        }
        // 错误端点：返回 500
        "/error" => {
            // 构造 500 响应
            let mut resp = Response::new(Full::new(Bytes::from("internal error")));
            // 设置状态码为 500
            *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            // 返回错误响应
            Ok(resp)
        }
        // 回显端点：读取请求体并回显
        _ if path.starts_with("/echo") && method == hyper::Method::POST => {
            // 聚合请求体
            let collected = req.collect().await.unwrap_or_default();
            // 提取请求体字节
            let body = collected.to_bytes();
            // 返回回显响应
            Ok(ok_response(&format!("echo:{}", String::from_utf8_lossy(&body))))
        }
        // 其他路径：返回 404
        _ => {
            // 构造 404 响应
            let mut resp = Response::new(Full::new(Bytes::from("not found")));
            // 设置状态码为 404
            *resp.status_mut() = StatusCode::NOT_FOUND;
            // 返回错误响应
            Ok(resp)
        }
    }
}

/// 构造一个 200 状态码的纯文本响应
///
/// # 参数
/// - `text`: 响应体文本
///
/// # 返回值
/// 构造完成的响应
fn ok_response(text: &str) -> Response<Full<Bytes>> {
    // 构造 200 响应
    let resp = Response::new(Full::new(Bytes::from(text.to_string())));
    // 返回响应
    resp
}

/// 演示服务器主函数
#[tokio::main]
async fn main() {
    // 解析监听地址（支持命令行参数覆盖）
    let addr: SocketAddr = std::env::args()
        // 取第 2 个参数
        .nth(1)
        // 解析为地址
        .map(|s| s.parse().unwrap())
        // 默认地址
        .unwrap_or_else(|| DEFAULT_ADDR.parse().unwrap());
    // 绑定 TCP 监听器
    let listener = TcpListener::bind(addr).await.expect("绑定端口失败");
    // 打印启动信息
    println!("演示服务器已启动：http://{addr}");
    // 循环接受连接
    loop {
        // 接受一个 TCP 连接
        let (stream, _) = listener.accept().await.expect("接受连接失败");
        // 包装为 TokioIo
        let io = TokioIo::new(stream);
        // 为每个连接派生协程处理
        tokio::spawn(async move {
            // 构建自动协议（HTTP/1.1 + HTTP/2）服务连接
            let builder = Builder::new(TokioExecutor::new());
            // 提供服务函数
            let service = service_fn(handle);
            // 处理连接请求（忽略错误）
            let _ = builder.serve_connection(io, service).await;
        });
    }
}

//! # 集成测试：端到端压测流程验证
//!
//! 启动一个本地 Hyper 目标服务，通过 rhb 库执行一次完整压测，
//! 验证请求总数、成功率、延迟统计等核心指标的正确性。

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;

use rhb::config::Config;
use rhb::stats::Stats;
use rhb::worker::RunContext;

/// 启动一个本地测试 HTTP 服务并返回监听地址
///
/// # 返回值
/// 服务监听地址（127.0.0.1 上的随机端口）
async fn spawn_test_server() -> SocketAddr {
    // 绑定到随机端口
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    // 获取实际监听地址
    let addr = listener.local_addr().unwrap();
    // 派生服务协程
    tokio::spawn(async move {
        // 循环接受连接
        loop {
            // 接受 TCP 连接
            let (stream, _) = listener.accept().await.unwrap();
            // 包装 TokioIo
            let io = TokioIo::new(stream);
            // 派生连接处理协程
            tokio::spawn(async move {
                // 构建自动协议服务
                let builder = Builder::new(TokioExecutor::new());
                // 提供服务函数
                let service = service_fn(test_handler);
                // 处理连接
                let _ = builder.serve_connection(io, service).await;
            });
        }
    });
    // 返回监听地址
    addr
}

/// 测试服务的请求处理函数
///
/// # 参数
/// - `_req`: HTTP 请求
///
/// # 返回值
/// 固定返回 200 与文本
async fn test_handler(_req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    // 构造 200 响应
    Ok(Response::new(Full::new(Bytes::from("test ok"))))
}

/// 构造一个压测配置
///
/// # 参数
/// - `url`: 目标服务地址
/// - `requests`: 总请求数
/// - `concurrency`: 并发数
///
/// # 返回值
/// 合并后的压测配置
fn make_config(url: String, requests: u64, concurrency: usize) -> Config {
    // 返回配置结构
    Config {
        // 目标 URL
        url,
        // 并发数
        concurrency,
        // 总请求数
        requests: Some(requests),
        // 压测时长
        time: None,
        // 请求方法
        method: "GET".to_string(),
        // 请求头
        headers: vec![],
        // 请求体
        body: None,
        // 限流
        rate: None,
        // HTTP/2
        http2: false,
        // TLS 校验
        no_tls: false,
        // 数据集
        dataset: None,
        // 数据集策略
        dataset_mode: "random".to_string(),
        // 预热数
        warmup: 0,
        // 超时
        timeout: 10,
        // 重试
        retries: 0,
        // 重定向
        redirect: true,
        // 柱状图
        histogram: false,
        // JSON 输出
        json: false,
        // JSON 文件
        json_file: None,
        // CSV 文件
        csv: None,
        // p99 阈值
        p99_threshold_ms: None,
        // 错误率阈值
        error_threshold_pct: None,
        // 进度条
        no_progress: true,
    }
}

/// 测试：完整压测流程，验证请求总数与成功率
#[tokio::test]
async fn test_end_to_end_load() {
    // 启动测试服务
    let addr = spawn_test_server().await;
    // 构造压测配置（100 请求，10 并发）
    let cfg = make_config(format!("http://{addr}/"), 100, 10);
    // 创建统计聚合器
    let stats = Arc::new(Stats::new(false).unwrap());
    // 构建运行上下文
    let ctx = Arc::new(RunContext::build(cfg.clone(), stats.clone()).unwrap());
    // 执行压测
    ctx.run(None).await.unwrap();
    // 汇总统计
    let summary = stats.summarize(&cfg.url, cfg.concurrency, None, None);
    // 断言总请求数正确
    assert_eq!(summary.total, 100);
    // 断言全部成功
    assert_eq!(summary.success, 100);
    // 断言失败数为 0
    assert_eq!(summary.failure, 0);
    // 断言错误率为 0
    assert_eq!(summary.error_rate_pct, 0.0);
}

/// 测试：并发数超过 1 时统计仍正确
#[tokio::test]
async fn test_concurrent_load() {
    // 启动测试服务
    let addr = spawn_test_server().await;
    // 构造压测配置（300 请求，30 并发）
    let cfg = make_config(format!("http://{addr}/json"), 300, 30);
    // 创建统计聚合器
    let stats = Arc::new(Stats::new(false).unwrap());
    // 构建运行上下文
    let ctx = Arc::new(RunContext::build(cfg.clone(), stats.clone()).unwrap());
    // 执行压测
    ctx.run(None).await.unwrap();
    // 汇总统计
    let summary = stats.summarize(&cfg.url, cfg.concurrency, None, None);
    // 断言总请求数正确
    assert_eq!(summary.total, 300);
    // 断言全部成功
    assert_eq!(summary.success, 300);
}

/// 测试：限流模式下请求数仍准确
#[tokio::test]
async fn test_rate_limited_load() {
    // 启动测试服务
    let addr = spawn_test_server().await;
    // 构造压测配置（100 请求，5 并发，QPS=1000）
    let mut cfg = make_config(format!("http://{addr}/"), 100, 5);
    // 设置限流
    cfg.rate = Some(1000);
    // 创建统计聚合器
    let stats = Arc::new(Stats::new(false).unwrap());
    // 构建运行上下文
    let ctx = Arc::new(RunContext::build(cfg.clone(), stats.clone()).unwrap());
    // 执行压测
    ctx.run(None).await.unwrap();
    // 汇总统计
    let summary = stats.summarize(&cfg.url, cfg.concurrency, None, None);
    // 断言总请求数正确
    assert_eq!(summary.total, 100);
}

/// 测试：目标服务返回错误时失败计数正确
#[tokio::test]
async fn test_error_handling() {
    // 启动测试服务（此处不使用监听地址，仅保证服务协程启动）
    let _addr = spawn_test_server().await;
    // 构造压测配置（访问不存在的端口造成连接失败）
    let cfg = make_config(format!("http://127.0.0.1:1/"), 50, 5);
    // 创建统计聚合器
    let stats = Arc::new(Stats::new(false).unwrap());
    // 构建运行上下文
    let ctx = Arc::new(RunContext::build(cfg.clone(), stats.clone()).unwrap());
    // 执行压测
    ctx.run(None).await.unwrap();
    // 汇总统计
    let summary = stats.summarize(&cfg.url, cfg.concurrency, None, None);
    // 断言全部失败（连接拒绝）
    assert_eq!(summary.total, 50);
    assert_eq!(summary.failure, 50);
    // 断言错误率 100%
    assert_eq!(summary.error_rate_pct, 100.0);
}

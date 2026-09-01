//! # Rust HTTP 压测工具（rhb）库入口
//!
//! 本库实现一个高性能 HTTP 请求发生器与压力测试工具，支持：
//! - 同步/异步请求生成
//! - 自定义请求方法、URL、头信息、请求体
//! - HTTP/1.1、HTTP/2、HTTPS（rustls）
//! - 多并发配置（并发数、总请求数、压测时长）
//! - 精准 QPS 限流（令牌桶算法）
//! - 数据集驱动的随机化压测
//! - 延迟分布统计（p50/p95/p99）与 QPS、错误率指标
//! - 终端报告 + CSV/JSON 结果导出

// 声明各功能子模块
pub mod cli;      // 命令行参数解析模块
pub mod client;   // HTTP 客户端引擎模块
pub mod config;   // 配置文件加载与合并模块
pub mod dataset;  // 数据集驱动随机化压测模块
pub mod limiter;  // 令牌桶 QPS 限流模块
pub mod report;   // 报告生成与数据导出模块
pub mod stats;    // 性能统计聚合模块
pub mod worker;   // 负载调度与 Worker Pool 模块

// 重新导出常用类型，方便外部调用
pub use config::Config;                 // 压测配置结构
pub use stats::{RequestDetail, Summary}; // 请求明细与汇总统计结构

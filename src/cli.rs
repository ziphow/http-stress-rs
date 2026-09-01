//! # 命令行参数解析模块（Clap 4）
//!
//! 使用 `clap` 的派生宏构建类型安全的命令行解析引擎。
//! 支持必选 URL 参数、并发控制、HTTP 配置、限流、协议、数据集与输出配置。
//!
//! 优先级设计说明：凡参与「CLI > 配置文件 > 默认值」合并的标量参数
//! 均声明为 `Option`，合并逻辑在 `config::Config::from_cli` 中完成。

use clap::Parser;

/// 命令行入口参数结构体（对应 rhb 的全部命令行选项）
///
/// 每个字段通过属性宏声明参数名、缩写与帮助信息；
/// 这些字段将在 main 中与 YAML 配置文件合并（CLI 优先）。
#[derive(Parser, Debug, Clone)]
#[command(
    name = "rhb",
    version,
    about = "Rust HTTP 压测工具：高性能 HTTP 请求发生器与性能评估工具",
    long_about = "rhb 是一个对标 oha / wrk / hey 的 Rust HTTP 压测工具。\n\
                  它通过 Tokio 异步运行时 + Reqwest 客户端生成高并发 HTTP 请求，\n\
                  基于 HdrHistogram 输出 p50/p95/p99 延迟分布、QPS、错误率等指标，\n\
                  支持数据集驱动的随机化压测与 CSV/JSON 结果导出。"
)]
pub struct Cli {
    /// 目标 URL（必选参数，例如 http://localhost:8080/api）
    #[arg(value_name = "URL", help = "目标服务 URL（必选）")]
    pub url: String,

    /// 并发连接数（同时进行的请求数量，默认 50）
    #[arg(short = 'c', long, help = "并发连接数（默认 50）")]
    pub concurrency: Option<usize>,

    /// 总请求数（与 --time 二选一或同时使用）
    #[arg(short = 'n', long, help = "总请求数（与 --time 二选一或同时使用）")]
    pub requests: Option<u64>,

    /// 压测时长（秒），到时后停止发送新请求
    #[arg(short = 't', long, help = "压测时长（秒）")]
    pub time: Option<u64>,

    /// 请求方法（GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS，默认 GET）
    #[arg(short = 'X', long, help = "请求方法（默认 GET）")]
    pub method: Option<String>,

    /// 自定义请求头，可重复指定，格式 "名称: 值"
    #[arg(
        short = 'H',
        long,
        help = "自定义请求头，可多次指定，如 -H 'Content-Type: application/json'"
    )]
    pub header: Vec<String>,

    /// 请求体（内联字符串）
    #[arg(short = 'd', long, help = "请求体（内联）")]
    pub body: Option<String>,

    /// 从文件读取请求体
    #[arg(short = 'f', long, help = "从文件读取请求体")]
    pub file: Option<String>,

    /// QPS 限流（每秒最大请求数，0 表示不限流）
    #[arg(long, help = "QPS 限流（令牌桶），0 表示不限流")]
    pub rate: Option<u64>,

    /// 启用 HTTP/2 协议
    #[arg(long, help = "启用 HTTP/2 协议")]
    pub http2: bool,

    /// 禁用 TLS 证书校验（仅用于测试环境）
    #[arg(long, help = "禁用 TLS 证书校验（测试环境使用）")]
    pub no_tls: bool,

    /// JSON 数据集文件路径（实现请求随机化）
    #[arg(long, help = "JSON 数据集文件路径（请求随机化）")]
    pub dataset: Option<String>,

    /// 数据集选择策略：random（随机）或 sequential（顺序，默认 random）
    #[arg(long, help = "数据集选择策略：random/sequential（默认 random）")]
    pub dataset_mode: Option<String>,

    /// 预热请求数（不计入统计，用于消除冷启动影响）
    #[arg(long, help = "预热请求数（不计入统计）")]
    pub warmup: Option<u64>,

    /// 单请求超时时间（秒，默认 30）
    #[arg(long, help = "单请求超时时间（秒，默认 30）")]
    pub timeout: Option<u64>,

    /// 失败重试次数（默认 0）
    #[arg(long, help = "失败重试次数（默认 0）")]
    pub retries: Option<u32>,

    /// 禁止跟随重定向（默认跟随）
    #[arg(long, help = "禁止跟随重定向（默认跟随）")]
    pub no_redirect: bool,

    /// 是否在终端输出延迟柱状图
    #[arg(long, help = "输出 ASCII 延迟分布柱状图")]
    pub histogram: bool,

    /// JSON 格式输出到终端
    #[arg(long, help = "以 JSON 格式输出报告到终端")]
    pub json: bool,

    /// JSON 结果导出文件路径
    #[arg(long, help = "JSON 结果导出文件路径")]
    pub json_file: Option<String>,

    /// CSV 明细导出文件路径（每条请求一条记录）
    #[arg(long, help = "CSV 明细导出文件路径")]
    pub csv: Option<String>,

    /// YAML 配置文件路径
    #[arg(short = 'C', long, help = "YAML 配置文件路径")]
    pub config: Option<String>,

    /// p99 延迟达标阈值（毫秒），超过则退出码为 2
    #[arg(long, help = "p99 阈值（毫秒），超过则退出码为 2（CI 集成）")]
    pub p99_threshold_ms: Option<f64>,

    /// 错误率阈值（百分比 0-100），超过则退出码为 2
    #[arg(long, help = "错误率阈值（百分比），超过则退出码为 2（CI 集成）")]
    pub error_threshold_pct: Option<f64>,

    /// 是否隐藏进度条（管道输出时自动隐藏）
    #[arg(long, help = "隐藏进度条")]
    pub no_progress: bool,
}

/// 将自定义请求头字符串（"名称: 值"）解析为 (键, 值) 元组
///
/// # 参数
/// - `header`: 形如 "Content-Type: application/json" 的字符串
///
/// # 返回值
/// 解析成功返回 (名称, 值)；格式非法返回错误信息
pub fn parse_header(header: &str) -> anyhow::Result<(String, String)> {
    // 按第一个冒号分割名称与值
    let idx = header
        .find(':')
        .ok_or_else(|| anyhow::anyhow!("请求头格式错误（应为 \"名称: 值\"）：{header}"))?;
    // 取冒号前的名称并去除首尾空白
    let name = header[..idx].trim().to_string();
    // 取冒号后的值并去除首尾空白
    let value = header[idx + 1..].trim().to_string();
    // 名称不能为空
    if name.is_empty() {
        return Err(anyhow::anyhow!("请求头名称不能为空"));
    }
    // 返回 (名称, 值) 元组
    Ok((name, value))
}

/// 将请求方法字符串转换为 Reqwest 的请求构建器
///
/// # 参数
/// - `method`: 方法名（GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS）
///
/// # 返回值
/// Reqwest 的 Method 类型
pub fn parse_method(method: &str) -> anyhow::Result<reqwest::Method> {
    // 尝试把字符串转换为标准 HTTP 方法，失败则返回错误
    method
        .parse::<reqwest::Method>()
        .map_err(|e| anyhow::anyhow!("不支持的请求方法：{method}（{e}）"))
}

/// 单元测试：验证参数解析逻辑
#[cfg(test)]
mod tests {
    use super::*;

    /// 测试：合法的请求头能够正确解析为 (名称, 值)
    #[test]
    fn test_parse_header_ok() {
        // 构造 "Content-Type: application/json" 字符串并解析
        let (k, v) = parse_header("Content-Type: application/json").unwrap();
        // 断言名称与值正确
        assert_eq!(k, "Content-Type");
        assert_eq!(v, "application/json");
    }

    /// 测试：缺少冒号的请求头应返回错误
    #[test]
    fn test_parse_header_invalid() {
        // 无冒号的字符串解析应失败
        assert!(parse_header("no-colon-here").is_err());
    }

    /// 测试：空名称的请求头应返回错误
    #[test]
    fn test_parse_header_empty_name() {
        // 名称部分为空的字符串解析应失败
        assert!(parse_header(": value").is_err());
    }

    /// 测试：合法的请求方法能够正确转换
    #[test]
    fn test_parse_method_ok() {
        // GET 与 POST 应转换成功
        assert!(parse_method("GET").is_ok());
        assert!(parse_method("POST").is_ok());
    }

    /// 测试：非法的请求方法应返回错误
    #[test]
    fn test_parse_method_invalid() {
        // 含非法字符（空格）的方法名转换应失败
        assert!(parse_method("FOO BAR").is_err());
        // 含非法控制字符的方法名转换应失败
        assert!(parse_method("GET\r\n").is_err());
    }
}

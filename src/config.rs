//! # 配置模块
//!
//! 负责 YAML 配置文件的加载与「CLI > 配置文件 > 默认值」的优先级合并。
//! 压测参数可通过命令行或 YAML 文件两种方式指定，命令行参数优先。

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

/// 命令行参数（来自 cli 模块），用于与配置文件合并
use crate::cli::Cli;

/// 合并后的最终压测配置结构
///
/// 该结构保存压测运行所需的全部有效参数，
/// 由 `Config::from_cli` 从 CLI 与 YAML 配置文件合并而来。
#[derive(Debug, Clone)]
pub struct Config {
    /// 目标服务 URL
    pub url: String,
    /// 并发连接数
    pub concurrency: usize,
    /// 总请求数（None 表示不限制）
    pub requests: Option<u64>,
    /// 压测时长（秒，None 表示不限制）
    pub time: Option<u64>,
    /// 请求方法（GET/POST/...）
    pub method: String,
    /// 自定义请求头列表（名称, 值）
    pub headers: Vec<(String, String)>,
    /// 请求体内容
    pub body: Option<String>,
    /// QPS 限流（0/None 表示不限流）
    pub rate: Option<u64>,
    /// 是否启用 HTTP/2
    pub http2: bool,
    /// 是否禁用 TLS 校验
    pub no_tls: bool,
    /// JSON 数据集文件路径
    pub dataset: Option<String>,
    /// 数据集选择策略（random/sequential）
    pub dataset_mode: String,
    /// 预热请求数
    pub warmup: u64,
    /// 单请求超时（秒）
    pub timeout: u64,
    /// 失败重试次数
    pub retries: u32,
    /// 是否跟随重定向
    pub redirect: bool,
    /// 是否输出延迟柱状图
    pub histogram: bool,
    /// 是否 JSON 输出到终端
    pub json: bool,
    /// JSON 导出文件路径
    pub json_file: Option<String>,
    /// CSV 明细导出文件路径
    pub csv: Option<String>,
    /// p99 达标阈值（毫秒）
    pub p99_threshold_ms: Option<f64>,
    /// 错误率阈值（百分比）
    pub error_threshold_pct: Option<f64>,
    /// 是否隐藏进度条
    pub no_progress: bool,
}

/// YAML 配置文件结构（所有字段可选，缺失时使用默认值或 CLI 值）
#[derive(Debug, Default, Deserialize)]
pub struct FileConfig {
    /// 目标服务 URL
    pub url: Option<String>,
    /// 并发连接数
    pub concurrency: Option<usize>,
    /// 总请求数
    pub requests: Option<u64>,
    /// 压测时长（秒）
    pub time: Option<u64>,
    /// 请求方法
    pub method: Option<String>,
    /// 自定义请求头（键值映射）
    pub headers: Option<HashMap<String, String>>,
    /// 请求体内容
    pub body: Option<String>,
    /// QPS 限流
    pub rate: Option<u64>,
    /// 是否启用 HTTP/2
    pub http2: Option<bool>,
    /// 是否禁用 TLS 校验
    pub no_tls: Option<bool>,
    /// JSON 数据集文件路径
    pub dataset: Option<String>,
    /// 数据集选择策略
    pub dataset_mode: Option<String>,
    /// 预热请求数
    pub warmup: Option<u64>,
    /// 单请求超时（秒）
    pub timeout: Option<u64>,
    /// 失败重试次数
    pub retries: Option<u32>,
    /// 是否跟随重定向
    pub redirect: Option<bool>,
    /// 是否输出延迟柱状图
    pub histogram: Option<bool>,
    /// 是否 JSON 输出到终端
    pub json: Option<bool>,
    /// JSON 导出文件路径
    pub json_file: Option<String>,
    /// CSV 明细导出文件路径
    pub csv: Option<String>,
    /// p99 达标阈值（毫秒）
    pub p99_threshold_ms: Option<f64>,
    /// 错误率阈值（百分比）
    pub error_threshold_pct: Option<f64>,
    /// 是否隐藏进度条
    pub no_progress: Option<bool>,
}

impl Config {
    /// 从 CLI 与 YAML 配置文件合并生成最终配置
    ///
    /// # 参数
    /// - `cli`: 命令行解析结果（优先级最高）
    ///
    /// # 返回值
    /// 合并后的压测配置；URL 缺失或配置文件错误时返回错误
    pub fn from_cli(cli: &Cli) -> anyhow::Result<Self> {
        // 1. 若指定了配置文件，则加载 YAML 配置
        let file = match &cli.config {
            // 指定了配置文件：读取并反序列化
            Some(path) => Some(load_file_config(path)?),
            // 未指定配置文件：使用空配置
            None => None,
        };
        // 辅助闭包：从文件配置中取值（文件为空时返回 None）
        let f = |k: fn(&FileConfig) -> Option<String>| -> Option<String> {
            // 取文件配置对应字段
            file.as_ref().and_then(|c| k(c))
        };

        // 2. 目标 URL：CLI > 文件（必选，两者都缺失则报错）
        let url = if !cli.url.is_empty() {
            // 命令行显式提供了 URL
            cli.url.clone()
        } else {
            // 回退到配置文件中的 URL
            f(|c| c.url.clone())
                .ok_or_else(|| anyhow::anyhow!("必须指定目标 URL（命令行或配置文件）"))?
        };

        // 3. 合并自定义请求头：先合并配置文件，再追加命令行（命令行优先）
        let mut headers: Vec<(String, String)> = Vec::new();
        // 从配置文件读取请求头映射
        if let Some(c) = &file {
            if let Some(map) = &c.headers {
                // 遍历配置文件中的每个请求头并加入列表
                for (k, v) in map {
                    headers.push((k.clone(), v.clone()));
                }
            }
        }
        // 再解析命令行请求头（可重复 -H 参数）
        for h in &cli.header {
            // 解析 "名称: 值" 格式
            let (k, v) = crate::cli::parse_header(h)?;
            headers.push((k, v));
        }

        // 4. 组装最终配置（CLI > 文件 > 默认值）
        Ok(Config {
            url,
            // 并发数：CLI > 文件 > 50
            concurrency: cli
                .concurrency
                .or_else(|| file.as_ref().and_then(|c| c.concurrency))
                .unwrap_or(50),
            // 总请求数：CLI > 文件 > None
            requests: cli.requests.or_else(|| file.as_ref().and_then(|c| c.requests)),
            // 压测时长：CLI > 文件 > None
            time: cli.time.or_else(|| file.as_ref().and_then(|c| c.time)),
            // 请求方法：CLI > 文件 > "GET"
            method: cli
                .method
                .clone()
                .or_else(|| file.as_ref().and_then(|c| c.method.clone()))
                .unwrap_or_else(|| "GET".to_string()),
            // 合并后的请求头列表
            headers,
            // 请求体：CLI > 文件（若指定了 -f 文件则从文件读取，优先级最高）
            body: read_body_file(cli.file.as_deref())
                .ok()
                .flatten()
                .or_else(|| cli.body.clone())
                .or_else(|| file.as_ref().and_then(|c| c.body.clone())),
            // QPS 限流：CLI > 文件 > None
            rate: cli.rate.or_else(|| file.as_ref().and_then(|c| c.rate)),
            // HTTP/2 开关：命令行或文件任一开启即为 true
            http2: cli.http2 || file.as_ref().and_then(|c| c.http2).unwrap_or(false),
            // TLS 校验开关：命令行或文件任一开启即为 true
            no_tls: cli.no_tls || file.as_ref().and_then(|c| c.no_tls).unwrap_or(false),
            // 数据集文件：CLI > 文件 > None
            dataset: cli
                .dataset
                .clone()
                .or_else(|| file.as_ref().and_then(|c| c.dataset.clone())),
            // 数据集策略：CLI > 文件 > "random"
            dataset_mode: cli
                .dataset_mode
                .clone()
                .or_else(|| file.as_ref().and_then(|c| c.dataset_mode.clone()))
                .unwrap_or_else(|| "random".to_string()),
            // 预热请求数：CLI > 文件 > 0
            warmup: cli
                .warmup
                .or_else(|| file.as_ref().and_then(|c| c.warmup))
                .unwrap_or(0),
            // 单请求超时：CLI > 文件 > 30
            timeout: cli
                .timeout
                .or_else(|| file.as_ref().and_then(|c| c.timeout))
                .unwrap_or(30),
            // 重试次数：CLI > 文件 > 0
            retries: cli
                .retries
                .or_else(|| file.as_ref().and_then(|c| c.retries))
                .unwrap_or(0),
            // 跟随重定向：未禁用重定向且（文件默认 true）
            redirect: !cli.no_redirect && file.as_ref().and_then(|c| c.redirect).unwrap_or(true),
            // 柱状图开关：命令行或文件任一开启即为 true
            histogram: cli
                .histogram
                || file.as_ref().and_then(|c| c.histogram).unwrap_or(false),
            // JSON 终端输出：命令行或文件任一开启即为 true
            json: cli.json || file.as_ref().and_then(|c| c.json).unwrap_or(false),
            // JSON 导出文件：CLI > 文件 > None
            json_file: cli
                .json_file
                .clone()
                .or_else(|| file.as_ref().and_then(|c| c.json_file.clone())),
            // CSV 导出文件：CLI > 文件 > None
            csv: cli
                .csv
                .clone()
                .or_else(|| file.as_ref().and_then(|c| c.csv.clone())),
            // p99 阈值：CLI > 文件 > None
            p99_threshold_ms: cli
                .p99_threshold_ms
                .or_else(|| file.as_ref().and_then(|c| c.p99_threshold_ms)),
            // 错误率阈值：CLI > 文件 > None
            error_threshold_pct: cli
                .error_threshold_pct
                .or_else(|| file.as_ref().and_then(|c| c.error_threshold_pct)),
            // 隐藏进度条：命令行或文件任一开启即为 true
            no_progress: cli
                .no_progress
                || file.as_ref().and_then(|c| c.no_progress).unwrap_or(false),
        })
    }
}

/// 从 YAML 文件加载并反序列化配置文件
///
/// # 参数
/// - `path`: 配置文件路径
///
/// # 返回值
/// 反序列化得到的文件配置；失败时返回错误
fn load_file_config(path: &str) -> anyhow::Result<FileConfig> {
    // 读取文件内容为字符串
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("读取配置文件失败 {path}: {e}"))?;
    // 将 YAML 字符串反序列化为 FileConfig 结构
    serde_yaml::from_str(&content).map_err(|e| anyhow::anyhow!("解析配置文件失败 {path}: {e}"))
}

/// 从文件读取请求体内容
///
/// # 参数
/// - `path`: 请求体文件路径（可能为 None）
///
/// # 返回值
/// 读取成功返回 Some(内容)；文件不存在或读取失败返回错误
fn read_body_file(path: Option<&str>) -> anyhow::Result<Option<String>> {
    // 路径为 None 时直接返回 None
    let Some(p) = path else { return Ok(None) };
    // 校验文件是否存在
    if !Path::new(p).exists() {
        // 文件不存在则返回错误
        return Err(anyhow::anyhow!("请求体文件不存在：{p}"));
    }
    // 读取文件内容并去除尾部空白
    let content = std::fs::read_to_string(p)
        .map_err(|e| anyhow::anyhow!("读取请求体文件失败 {p}: {e}"))?;
    // 返回文件内容
    Ok(Some(content.trim_end().to_string()))
}

/// 单元测试：验证配置合并逻辑
#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助函数：构造一个全空 CLI 对象（方便测试）
    fn empty_cli() -> Cli {
        // 返回所有字段为默认/None 的 CLI
        Cli {
            url: String::new(),
            concurrency: None,
            requests: None,
            time: None,
            method: None,
            header: vec![],
            body: None,
            file: None,
            rate: None,
            http2: false,
            no_tls: false,
            dataset: None,
            dataset_mode: None,
            warmup: None,
            timeout: None,
            retries: None,
            no_redirect: false,
            histogram: false,
            json: false,
            json_file: None,
            csv: None,
            config: None,
            p99_threshold_ms: None,
            error_threshold_pct: None,
            no_progress: false,
        }
    }

    /// 测试：URL 缺失时应返回错误
    #[test]
    fn test_url_required() {
        // 空 CLI 缺少 URL，配置合并应失败
        let cli = empty_cli();
        assert!(Config::from_cli(&cli).is_err());
    }

    /// 测试：提供 URL 后应返回默认配置
    #[test]
    fn test_default_values() {
        // 构造一个仅含 URL 的 CLI
        let mut cli = empty_cli();
        cli.url = "http://localhost:8080".to_string();
        // 生成配置
        let cfg = Config::from_cli(&cli).unwrap();
        // 断言默认并发数、方法、超时等取值正确
        assert_eq!(cfg.concurrency, 50);
        assert_eq!(cfg.method, "GET");
        assert_eq!(cfg.timeout, 30);
        assert_eq!(cfg.dataset_mode, "random");
        assert!(cfg.redirect);
    }

    /// 测试：CLI 显式参数应覆盖默认值
    #[test]
    fn test_cli_override() {
        // 构造一个显式指定并发数与方法的 CLI
        let mut cli = empty_cli();
        cli.url = "http://localhost:8080".to_string();
        cli.concurrency = Some(128);
        cli.method = Some("POST".to_string());
        // 生成配置
        let cfg = Config::from_cli(&cli).unwrap();
        // 断言 CLI 值生效
        assert_eq!(cfg.concurrency, 128);
        assert_eq!(cfg.method, "POST");
    }
}

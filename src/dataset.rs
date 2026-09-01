//! # 数据集驱动随机化压测模块
//!
//! 为避免缓存预热造成的性能假象，支持数据集驱动的随机化压测：
//! - 从 JSON 数据集文件读取多条请求模板
//! - 每条模板包含 method、path、headers、body 字段
//! - Worker 按随机（random）或顺序（sequential）策略选取模板
//! - 支持字段值模板化：`{{random.string(8)}}`、`{{random.int(1,100000)}}`、
//!   `{{random.uuid}}`、`{{random.email}}` 动态生成请求参数

use std::collections::HashMap;
use std::path::Path;

use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Deserialize;

/// 单条请求模板（对应数据集 JSON 中的一条记录）
#[derive(Debug, Clone, Deserialize)]
pub struct RequestTemplate {
    /// 请求方法
    #[serde(default = "default_method")]
    pub method: String,
    /// 请求路径（相对路径或绝对 URL）
    pub path: String,
    /// 请求头映射
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// 请求体内容（可含随机模板占位符）
    #[serde(default)]
    pub body: Option<String>,
}

/// 数据集结构（对应数据集 JSON 文件顶层）
#[derive(Debug, Clone, Deserialize)]
pub struct DataSet {
    /// 请求模板列表
    pub templates: Vec<RequestTemplate>,
}

/// 默认请求方法（GET）
fn default_method() -> String {
    // 返回默认方法名
    "GET".to_string()
}

/// 数据集选择策略枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectMode {
    /// 随机选择
    Random,
    /// 顺序选择
    Sequential,
}

/// 从字符串解析选择策略
///
/// # 参数
/// - `s`: 策略名（"random" / "sequential"）
///
/// # 返回值
/// 解析成功返回对应枚举；未知策略返回错误
pub fn parse_mode(s: &str) -> anyhow::Result<SelectMode> {
    // 按名称匹配策略
    match s.to_ascii_lowercase().as_str() {
        // 随机策略
        "random" => Ok(SelectMode::Random),
        // 顺序策略
        "sequential" | "seq" => Ok(SelectMode::Sequential),
        // 未知策略
        _ => Err(anyhow::anyhow!("未知的数据集选择策略：{s}（可选 random/sequential）")),
    }
}

impl DataSet {
    /// 从 JSON 文件加载数据集
    ///
    /// # 参数
    /// - `path`: 数据集 JSON 文件路径
    ///
    /// # 返回值
    /// 加载成功返回数据集；文件缺失或格式错误返回错误
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        // 校验文件是否存在
        if !Path::new(path).exists() {
            // 文件不存在则返回错误
            return Err(anyhow::anyhow!("数据集文件不存在：{path}"));
        }
        // 读取文件内容
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("读取数据集文件失败 {path}: {e}"))?;
        // 将 JSON 字符串反序列化为数据集
        serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("解析数据集文件失败 {path}: {e}"))
    }

    /// 按选择策略挑选一条请求模板
    ///
    /// # 参数
    /// - `mode`: 选择策略（随机或顺序）
    /// - `seq`: 当前请求序号（顺序策略使用）
    ///
    /// # 返回值
    /// 选中的请求模板
    pub fn pick(&self, mode: SelectMode, seq: u64) -> &RequestTemplate {
        // 数据集为空时无法选择
        if self.templates.is_empty() {
            // 返回空模板占位（调用方会忽略）
            return &self.templates[0.min(self.templates.len())];
        }
        // 按策略计算选中索引
        let idx = match mode {
            // 随机策略：生成随机索引
            SelectMode::Random => rand::thread_rng().gen_range(0..self.templates.len()),
            // 顺序策略：按序号取模
            SelectMode::Sequential => (seq as usize) % self.templates.len(),
        };
        // 返回选中的模板
        &self.templates[idx]
    }
}

impl RequestTemplate {
    /// 渲染请求模板：替换其中的随机占位符
    ///
    /// # 参数
    /// - `base_url`: 目标服务基础 URL（用于拼接完整地址）
    ///
    /// # 返回值
    /// 渲染后的 (完整URL, 方法, 头信息, 请求体)
    pub fn render(&self, base_url: &str) -> (String, String, HashMap<String, String>, Option<String>) {
        // 渲染路径中的随机占位符
        let path = render_template(&self.path);
        // 拼接完整 URL（处理相对/绝对路径）
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            // 模板自带完整 URL 时直接使用
            path
        } else {
            // 否则拼接基础 URL 与路径（去除尾部斜杠）
            format!("{}/{}", base_url.trim_end_matches('/'), path.trim_start_matches('/'))
        };
        // 渲染请求体中的随机占位符
        let body = self.body.as_deref().map(render_template);
        // 返回渲染结果
        (url, self.method.clone(), self.headers.clone(), body)
    }
}

/// 渲染字符串中的随机模板占位符
///
/// 支持的占位符：
/// - `{{random.string(N)}}`：生成长度为 N 的随机字母数字字符串
/// - `{{random.int(A,B)}}`：生成 [A, B] 范围内的随机整数
/// - `{{random.uuid}}`：生成随机 UUID
/// - `{{random.email}}`：生成随机邮箱地址
///
/// # 参数
/// - `input`: 原始模板字符串
///
/// # 返回值
/// 渲染后的字符串
pub fn render_template(input: &str) -> String {
    // 随机数生成器
    let mut rng = rand::thread_rng();
    // 最终输出字符串
    let mut out = String::with_capacity(input.len());
    // 当前待解析的剩余片段
    let mut rest = input;
    // 循环查找下一个占位符
    while let Some(start) = rest.find("{{") {
        // 将占位符之前的内容追加到输出
        out.push_str(&rest[..start]);
        // 占位符表达式起始位置（跳过 "{{"）
        let after = start + 2;
        // 查找占位符结束标记 "}}"
        match rest[after..].find("}}") {
            // 找到结束标记：渲染占位符
            Some(end) => {
                // 提取占位符内部表达式
                let expr = &rest[after..after + end];
                // 渲染单个占位符并追加
                out.push_str(&render_expr(expr, &mut rng));
                // 跳过已处理的占位符
                rest = &rest[after + end + 2..];
            }
            // 未找到结束标记：原样保留剩余内容并结束
            None => {
                // 将含未闭合占位符的剩余内容原样追加
                out.push_str(&rest[start..]);
                // 清空剩余片段
                rest = "";
            }
        }
    }
    // 追加占位符之间的剩余内容
    out.push_str(rest);
    // 返回渲染结果
    out
}

/// 渲染单个占位符表达式
///
/// # 参数
/// - `expr`: 占位符内部表达式（如 "random.string(8)"）
/// - `rng`: 随机数生成器
///
/// # 返回值
/// 渲染后的字符串
fn render_expr(expr: &str, rng: &mut impl rand::Rng) -> String {
    // 匹配随机字符串生成表达式
    if let Some(rest) = expr.strip_prefix("random.string(") {
        // 提取长度参数并生成随机字符串
        return random_string(parse_len(rest), rng);
    }
    // 匹配随机整数生成表达式
    if let Some(rest) = expr.strip_prefix("random.int(") {
        // 解析范围参数并生成随机整数
        return random_int(rest, rng);
    }
    // 匹配随机 UUID
    if expr == "random.uuid" {
        // 生成随机 UUID 字符串
        return random_uuid(rng);
    }
    // 匹配随机邮箱
    if expr == "random.email" {
        // 生成随机邮箱地址
        return random_email(rng);
    }
    // 未知表达式原样返回
    format!("{{{{{expr}}}}}")
}

/// 从 "N)" 形式的参数串中解析长度
///
/// # 参数
/// - `rest`: 形如 "8)" 的字符串
///
/// # 返回值
/// 解析出的长度（默认 8）
fn parse_len(rest: &str) -> usize {
    // 取右括号前的内容并解析为整数
    rest.trim_end_matches(')').trim().parse().unwrap_or(8)
}

/// 生成指定长度的随机字母数字字符串
///
/// # 参数
/// - `len`: 字符串长度
/// - `rng`: 随机数生成器
///
/// # 返回值
/// 随机字符串
fn random_string(len: usize, rng: &mut impl rand::Rng) -> String {
    // 从字母数字字符集采样生成字符串
    (0..len).map(|_| rng.sample(Alphanumeric) as char).collect()
}

/// 生成随机整数（解析 "A,B)" 范围）
///
/// # 参数
/// - `rest`: 形如 "1,100000)" 的字符串
/// - `rng`: 随机数生成器
///
/// # 返回值
/// 范围内随机整数
fn random_int(rest: &str, rng: &mut impl rand::Rng) -> String {
    // 去除右括号并按逗号分割上下限
    let range = rest.trim_end_matches(')').trim();
    // 按逗号拆分
    let mut parts = range.split(',');
    // 解析下限
    let low: i64 = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    // 解析上限
    let high: i64 = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(low);
    // 生成并返回随机整数
    if high <= low {
        // 上下限相同时直接返回
        low.to_string()
    } else {
        // 在范围内取随机整数
        rng.gen_range(low..=high).to_string()
    }
}

/// 生成随机 UUID 字符串（版本 4 风格）
///
/// # 参数
/// - `rng`: 随机数生成器
///
/// # 返回值
/// 形如 xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx 的 UUID
fn random_uuid(rng: &mut impl rand::Rng) -> String {
    // 生成 16 字节随机数据
    let mut bytes = [0u8; 16];
    // 填充随机字节
    rng.fill(&mut bytes[..]);
    // 设置版本 4 与变体位
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    // 按 8-4-4-4-12 分组格式化为十六进制
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11],
        bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// 生成随机邮箱地址
///
/// # 参数
/// - `rng`: 随机数生成器
///
/// # 返回值
/// 形如 用户名@域名.后缀 的邮箱
fn random_email(rng: &mut impl rand::Rng) -> String {
    // 随机用户名（8 位）
    let user = random_string(8, rng);
    // 随机域名（6 位）
    let domain = random_string(6, rng);
    // 随机顶级域
    let tld = ["com", "net", "org", "io", "cn"][rng.gen_range(0..5)];
    // 拼接邮箱地址
    format!("{user}@{domain}.{tld}")
}

/// 单元测试：验证数据集模板渲染逻辑
#[cfg(test)]
mod tests {
    use super::*;

    /// 测试：随机字符串占位符被替换且长度正确
    #[test]
    fn test_render_random_string() {
        // 渲染含随机字符串占位符的模板
        let out = render_template("user_{{random.string(8)}}");
        // 断言前缀保留
        assert!(out.starts_with("user_"));
        // 断言替换后总长度正确
        assert_eq!(out.len(), 5 + 8);
    }

    /// 测试：随机整数占位符被替换为数字
    #[test]
    fn test_render_random_int() {
        // 多次渲染验证输出均为数字
        for _ in 0..10 {
            // 渲染随机整数占位符
            let out = render_template("id={{random.int(1,100000)}}");
            // 断言前缀保留
            assert!(out.starts_with("id="));
            // 断言数值部分能解析为整数
            let n: i64 = out[3..].parse().unwrap();
            // 断言在范围内
            assert!((1..=100000).contains(&n));
        }
    }

    /// 测试：无占位符的字符串原样返回
    #[test]
    fn test_render_plain() {
        // 渲染无占位符字符串
        let out = render_template("/api/users/123");
        // 断言原样返回
        assert_eq!(out, "/api/users/123");
    }

    /// 测试：选择策略解析
    #[test]
    fn test_parse_mode() {
        // 合法策略应解析成功
        assert_eq!(parse_mode("random").unwrap(), SelectMode::Random);
        assert_eq!(parse_mode("sequential").unwrap(), SelectMode::Sequential);
        // 非法策略应失败
        assert!(parse_mode("bad").is_err());
    }
}

//! # 报告生成与数据导出模块
//!
//! 负责压测结束后的结果呈现：
//! - 终端报告：汇总总请求数、成功/失败数、错误率、QPS、延迟百分位数、延迟柱状图
//! - JSON 输出：结构化数据供 CI/CD 解析
//! - CSV 导出：每条请求的详细记录供深度分析存档
//! - 退出码设计：可配置性能阈值，不达标时由调用方返回退出码 2

use std::io::Write;

use hdrhistogram::Histogram;

use crate::stats::{RequestDetail, Summary};

/// 生成终端报告文本
///
/// # 参数
/// - `summary`: 压测汇总统计
/// - `hist`: 延迟直方图快照（可选，用于绘制柱状图）
///
/// # 返回值
/// 格式化后的报告字符串
pub fn render_terminal(summary: &Summary, hist: Option<&Histogram<u64>>) -> String {
    // 使用字符串缓冲拼接报告
    let mut out = String::new();
    // 分隔线常量
    let line = "=".repeat(58);
    // 标题
    out.push_str(&format!("{line}\n"));
    out.push_str(&format!("  Rust HTTP 压测工具报告（rhb v{}）\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!("{line}\n"));
    // 基本信息
    out.push_str(&format!("目标 URL      : {}\n", summary.url));
    out.push_str(&format!("并发数        : {}\n", summary.concurrency));
    out.push_str(&format!("压测耗时      : {:.2} s\n", summary.elapsed_secs));
    out.push_str(&format!("{line}\n"));
    // 请求统计
    out.push_str(&format!("总请求数      : {}\n", summary.total));
    out.push_str(&format!("成功请求数    : {}\n", summary.success));
    out.push_str(&format!("失败请求数    : {}\n", summary.failure));
    out.push_str(&format!("错误率        : {:.2}%\n", summary.error_rate_pct));
    out.push_str(&format!("总传输字节    : {} bytes\n", summary.bytes));
    out.push_str(&format!("实际 QPS      : {:.2}\n", summary.qps));
    out.push_str(&format!("{line}\n"));
    // 延迟统计
    out.push_str(&format!("延迟统计（毫秒）：\n"));
    out.push_str(&format!("  最小延迟   : {:.3}\n", summary.min_ms));
    out.push_str(&format!("  平均延迟   : {:.3}\n", summary.mean_ms));
    out.push_str(&format!("  最大延迟   : {:.3}\n", summary.max_ms));
    out.push_str(&format!("  标准差     : {:.3}\n", summary.stddev_ms));
    out.push_str(&format!("  p50        : {:.3}\n", summary.p50_ms));
    out.push_str(&format!("  p90        : {:.3}\n", summary.p90_ms));
    out.push_str(&format!("  p95        : {:.3}\n", summary.p95_ms));
    out.push_str(&format!("  p99        : {:.3}\n", summary.p99_ms));
    out.push_str(&format!("  p99.9      : {:.3}\n", summary.p999_ms));
    out.push_str(&format!("{line}\n"));
    // 状态码分布
    out.push_str(&format!("状态码分布    :\n"));
    // 状态码分布为空时的提示
    if summary.status_codes.is_empty() {
        // 无状态码记录
        out.push_str(&format!("  （无）\n"));
    } else {
        // 遍历状态码分布表
        for (code, count) in &summary.status_codes {
            // 输出单条状态码计数
            out.push_str(&format!("  {code}: {count}\n"));
        }
    }
    // 阈值告警信息
    if summary.threshold_failed {
        // 输出告警标题
        out.push_str(&format!("{line}\n"));
        out.push_str(&format!("[警告] 性能阈值未达标：\n"));
        // 遍历告警消息
        for msg in &summary.threshold_messages {
            // 输出单条告警
            out.push_str(&format!("  - {msg}\n"));
        }
    }
    // 可选：附加 ASCII 延迟柱状图
    if let Some(h) = hist {
        // 追加柱状图标题
        out.push_str(&format!("\n延迟分布柱状图（横轴百分比，对数分桶）：\n"));
        // 追加柱状图内容
        out.push_str(&render_hist_from_histogram(h, 40));
    }
    // 分隔线收尾
    out.push_str(&format!("{line}\n"));
    // 返回报告字符串
    out
}

/// 从 HdrHistogram 直方图渲染 ASCII 延迟分布图（用于终端展示）
///
/// 采用固定对数分桶（毫秒），按各桶计数比例绘制横条：
/// ```text
///   0-1ms   ████████████████ 40.0%
///   1-2ms   ██████ 15.0%
///   ...
/// ```
///
/// # 参数
/// - `hist`: 纳秒级延迟直方图
/// - `bar_width`: 直方图条最大宽度（字符数）
///
/// # 返回值
/// 柱状图字符串
pub fn render_hist_from_histogram(hist: &Histogram<u64>, bar_width: usize) -> String {
    // 直方图总记录数
    let total = hist.len().max(1);
    // 定义分桶阈值（毫秒，对数递增）
    let thresholds_ms = [
        1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0, 512.0, 1024.0, 2048.0, 4096.0,
    ];
    // 各桶计数数组
    let mut counts = vec![0u64; thresholds_ms.len()];
    // 遍历直方图中所有已记录值并按值分桶
    for v in hist.iter_recorded() {
        // 将记录值（纳秒）转换为毫秒（取桶上界值作为代表）
        let val_ms = v.value_iterated_to() as f64 / 1_000_000.0;
        // 计算所属桶索引
        let idx = thresholds_ms
            // 找到第一个大于当前值的阈值
            .iter()
            // 返回阈值索引
            .position(|&t| val_ms < t)
            // 未找到（超过最大阈值）则归入最后一桶
            .unwrap_or(thresholds_ms.len() - 1);
        // 累加当前桶计数
        counts[idx] += v.count_at_value();
    }
    // 输出缓冲
    let mut out = String::new();
    // 逐桶渲染柱状图
    for (i, &thr) in thresholds_ms.iter().enumerate() {
        // 计算当前桶计数百分比
        let pct = counts[i] as f64 / total as f64 * 100.0;
        // 计算条长（按百分比映射到宽度）
        let bar_len = (pct / 100.0 * bar_width as f64) as usize;
        // 渲染条字符
        let bar = "█".repeat(bar_len.min(bar_width));
        // 计算桶范围标签
        let lower = if i == 0 { 0.0 } else { thresholds_ms[i - 1] };
        // 格式化桶范围标签
        let label = if i == thresholds_ms.len() - 1 {
            // 最后一桶显示 ≥
            format!("≥{lower:.0}ms")
        } else {
            // 普通桶显示范围
            format!("{lower:.0}-{:.0}ms", thr)
        };
        // 输出一行柱状图
        out.push_str(&format!("{label:>9} {bar} {:.1}%\n", pct));
    }
    // 返回柱状图
    out
}

/// 将汇总统计导出为 JSON 字符串
///
/// # 参数
/// - `summary`: 压测汇总统计
///
/// # 返回值
/// 格式化后的 JSON 字符串（含状态码分布）
pub fn export_json(summary: &Summary) -> String {
    // 构建状态码分布 JSON Map
    let status_codes: serde_json::Map<String, serde_json::Value> = summary
        // 遍历状态码分布
        .status_codes
        .iter()
        // 映射为 字符串->数值
        .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
        // 收集为 Map
        .collect();
    // 组装完整 JSON 对象
    let value = serde_json::json!({
        // 元信息
        "tool": "rhb",
        "version": env!("CARGO_PKG_VERSION"),
        // 基本信息
        "url": summary.url,
        "concurrency": summary.concurrency,
        // 请求统计
        "total": summary.total,
        "success": summary.success,
        "failure": summary.failure,
        "error_rate_pct": summary.error_rate_pct,
        "bytes": summary.bytes,
        "elapsed_secs": summary.elapsed_secs,
        "qps": summary.qps,
        // 延迟统计
        "latency_ms": {
            "min": summary.min_ms,
            "mean": summary.mean_ms,
            "max": summary.max_ms,
            "stddev": summary.stddev_ms,
            "p50": summary.p50_ms,
            "p90": summary.p90_ms,
            "p95": summary.p95_ms,
            "p99": summary.p99_ms,
            "p99_9": summary.p999_ms,
        },
        // 状态码分布
        "status_codes": status_codes,
        // 阈值判定
        "threshold_failed": summary.threshold_failed,
        "threshold_messages": summary.threshold_messages,
    });
    // 序列化为缩进 JSON 字符串
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
}

/// 将请求明细导出为 CSV 文件
///
/// # 参数
/// - `details`: 请求明细列表
/// - `path`: CSV 输出文件路径
///
/// # 返回值
/// 写入成功返回写入条数；失败返回错误
pub fn export_csv(details: &[RequestDetail], path: &str) -> anyhow::Result<usize> {
    // 创建 CSV 写入器
    let mut wtr = csv::Writer::from_path(path)
        .map_err(|e| anyhow::anyhow!("创建 CSV 文件失败 {path}: {e}"))?;
    // 写入表头
    wtr.write_record([
        "timestamp_ms",
        "worker",
        "method",
        "url",
        "status",
        "success",
        "latency_ms",
        "size_bytes",
    ])
    .map_err(|e| anyhow::anyhow!("写入 CSV 表头失败：{e}"))?;
    // 遍历请求明细逐条写入
    for d in details {
        // 写入一行记录
        wtr.write_record([
            d.timestamp_ms.to_string(),
            d.worker.to_string(),
            d.method.clone(),
            d.url.clone(),
            d.status.to_string(),
            d.success.to_string(),
            format!("{:.3}", d.latency_ms),
            d.size_bytes.to_string(),
        ])
        .map_err(|e| anyhow::anyhow!("写入 CSV 记录失败：{e}"))?;
    }
    // 刷新写入器
    wtr.flush().map_err(|e| anyhow::anyhow!("刷新 CSV 文件失败：{e}"))?;
    // 返回写入条数
    Ok(details.len())
}

/// 将 JSON 字符串写入文件
///
/// # 参数
/// - `json`: JSON 字符串
/// - `path`: 输出文件路径
///
/// # 返回值
/// 写入成功返回 Ok；失败返回错误
pub fn write_json_file(json: &str, path: &str) -> anyhow::Result<()> {
    // 创建并写入文件
    let mut f = std::fs::File::create(path)
        .map_err(|e| anyhow::anyhow!("创建 JSON 文件失败 {path}: {e}"))?;
    // 写入 JSON 内容
    f.write_all(json.as_bytes())
        .map_err(|e| anyhow::anyhow!("写入 JSON 文件失败 {path}: {e}"))?;
    // 返回成功
    Ok(())
}

/// 单元测试：验证报告与导出逻辑
#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::Stats;

    /// 辅助函数：构造一个含数据的 Summary
    fn fake_summary() -> Summary {
        // 创建统计聚合器
        let stats = Stats::new(false).unwrap();
        // 记录 1000 条成功请求（5ms）
        for _ in 0..1000 {
            // 记录成功请求
            stats.record_success(5_000_000, 200, 512, 0, "GET", "http://t/");
        }
        // 汇总统计
        stats.summarize("http://t/", 10, None, None)
    }

    /// 测试：终端报告包含关键统计字段
    #[test]
    fn test_render_terminal() {
        // 生成终端报告
        let report = render_terminal(&fake_summary(), None);
        // 断言报告包含关键字段
        assert!(report.contains("总请求数"));
        assert!(report.contains("p99"));
        assert!(report.contains("QPS"));
    }

    /// 测试：JSON 导出包含关键字段
    #[test]
    fn test_export_json() {
        // 生成 JSON 字符串
        let json = export_json(&fake_summary());
        // 解析 JSON
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // 断言关键字段存在
        assert_eq!(v["total"], 1000);
        assert_eq!(v["success"], 1000);
        assert!(v["latency_ms"]["p50"].as_f64().unwrap() > 0.0);
    }

    /// 测试：CSV 导出写入记录
    #[test]
    fn test_export_csv() {
        // 构造一条请求明细
        let detail = RequestDetail {
            timestamp_ms: 0,
            worker: 1,
            method: "GET".to_string(),
            url: "http://t/".to_string(),
            status: 200,
            success: true,
            latency_ms: 5.0,
            size_bytes: 512,
        };
        // 导出到临时文件
        let path = std::env::temp_dir().join("rhb_test.csv");
        // 导出 CSV
        let n = export_csv(&[detail], path.to_str().unwrap()).unwrap();
        // 断言写入条数
        assert_eq!(n, 1);
    }
}

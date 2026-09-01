//! # 性能统计聚合模块
//!
//! 采用 HdrHistogram 进行高精度延迟分布统计（记录纳秒级延迟值），
//! 使用原子计数器维护成功/失败数、字节数，并支持百分位数聚合。
//! 统计结果在压测结束后汇总为 `Summary`，供终端报告与数据导出使用。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use hdrhistogram::Histogram;

/// 直方图统计范围常量
///
/// - 最低可辨别值：1 纳秒
/// - 最高可跟踪值：1 小时（纳秒）
/// - 有效位数：3（保证常见延迟区间精度）
const HIST_LOWEST: u64 = 1;
const HIST_HIGHEST: u64 = 3_600_000_000_000;
const HIST_SIGFIGS: u8 = 3;

/// 单条请求的详细记录（用于 CSV 明细导出）
#[derive(Debug, Clone)]
pub struct RequestDetail {
    /// 请求完成的时间戳（毫秒）
    pub timestamp_ms: u64,
    /// 所属 Worker 编号
    pub worker: u32,
    /// 请求方法
    pub method: String,
    /// 请求 URL
    pub url: String,
    /// 响应状态码（失败时为 0）
    pub status: u16,
    /// 请求是否成功（2xx/3xx 视为成功）
    pub success: bool,
    /// 请求延迟（毫秒）
    pub latency_ms: f64,
    /// 响应体大小（字节）
    pub size_bytes: u64,
}

/// 压测汇总统计结果（压测结束后一次性生成）
#[derive(Debug, Clone, serde::Serialize)]
pub struct Summary {
    /// 目标 URL
    pub url: String,
    /// 配置的并发数
    pub concurrency: usize,
    /// 总请求数
    pub total: u64,
    /// 成功请求数
    pub success: u64,
    /// 失败请求数
    pub failure: u64,
    /// 错误率（百分比）
    pub error_rate_pct: f64,
    /// 总传输字节数
    pub bytes: u64,
    /// 压测总耗时（秒）
    pub elapsed_secs: f64,
    /// 实际 QPS（成功+失败）/ 耗时
    pub qps: f64,
    /// 延迟最小值（毫秒）
    pub min_ms: f64,
    /// 延迟最大值（毫秒）
    pub max_ms: f64,
    /// 延迟平均值（毫秒）
    pub mean_ms: f64,
    /// 延迟标准差（毫秒）
    pub stddev_ms: f64,
    /// 延迟 p50（毫秒）
    pub p50_ms: f64,
    /// 延迟 p90（毫秒）
    pub p90_ms: f64,
    /// 延迟 p95（毫秒）
    pub p95_ms: f64,
    /// 延迟 p99（毫秒）
    pub p99_ms: f64,
    /// 延迟 p999（毫秒）
    pub p999_ms: f64,
    /// 状态码分布（状态码 -> 次数）
    #[serde(skip_serializing)]
    pub status_codes: BTreeMap<u16, u64>,
    /// 是否触发性能阈值告警（用于 CI 退出码）
    #[serde(skip_serializing)]
    pub threshold_failed: bool,
    /// 触发告警的阈值描述
    #[serde(skip_serializing)]
    pub threshold_messages: Vec<String>,
}

/// 全局统计聚合器（通过 Arc 在多协程间共享）
pub struct Stats {
    /// 延迟直方图（互斥锁保护，记录纳秒级延迟）
    pub histogram: Mutex<Histogram<u64>>,
    /// 已发送请求总数（原子计数器）
    pub total: AtomicU64,
    /// 成功请求数（原子计数器）
    pub success: AtomicU64,
    /// 失败请求数（原子计数器）
    pub failure: AtomicU64,
    /// 总传输字节数（原子计数器）
    pub bytes: AtomicU64,
    /// 状态码分布（互斥锁保护）
    pub status_codes: Mutex<BTreeMap<u16, u64>>,
    /// 请求明细（仅启用 CSV 导出时记录）
    pub details: Mutex<Vec<RequestDetail>>,
    /// 是否记录请求明细
    pub record_details: bool,
    /// 压测开始时间
    pub start: Instant,
}

impl Stats {
    /// 创建一个新的统计聚合器
    ///
    /// # 参数
    /// - `record_details`: 是否需要记录每条请求的明细（CSV 导出时开启）
    ///
    /// # 返回值
    /// 初始化完成的统计聚合器；直方图创建失败时返回错误
    pub fn new(record_details: bool) -> anyhow::Result<Self> {
        // 创建高精度延迟直方图
        let histogram = Histogram::new_with_bounds(HIST_LOWEST, HIST_HIGHEST, HIST_SIGFIGS)
            .map_err(|e| anyhow::anyhow!("创建延迟直方图失败：{e}"))?;
        // 返回统计聚合器
        Ok(Stats {
            // 延迟直方图
            histogram: Mutex::new(histogram),
            // 原子计数器初始化为 0
            total: AtomicU64::new(0),
            success: AtomicU64::new(0),
            failure: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            // 状态码分布为空
            status_codes: Mutex::new(BTreeMap::new()),
            // 请求明细为空
            details: Mutex::new(Vec::new()),
            // 是否记录明细
            record_details,
            // 记录开始时间
            start: Instant::now(),
        })
    }

    /// 记录一次成功的请求
    ///
    /// # 参数
    /// - `latency_ns`: 请求延迟（纳秒）
    /// - `status`: 响应状态码
    /// - `size`: 响应体大小（字节）
    /// - `worker`: 所属 Worker 编号
    /// - `method`: 请求方法
    /// - `url`: 请求 URL
    pub fn record_success(&self, latency_ns: u64, status: u16, size: u64, worker: u32, method: &str, url: &str) {
        // 记录延迟到直方图（避免 0 值）
        self.histogram.lock().unwrap().record(latency_ns.max(1)).ok();
        // 原子累加成功数
        self.success.fetch_add(1, Ordering::Relaxed);
        // 原子累加总请求数
        self.total.fetch_add(1, Ordering::Relaxed);
        // 原子累加字节数
        self.bytes.fetch_add(size, Ordering::Relaxed);
        // 更新状态码分布
        self.note_status(status);
        // 按需记录请求明细
        self.note_detail(worker, method, url, status, true, latency_ns, size);
    }

    /// 记录一次失败的请求
    ///
    /// # 参数
    /// - `latency_ns`: 请求延迟（纳秒，超时或错误时的已耗时）
    /// - `status`: 响应状态码（无响应时为 0）
    /// - `worker`: 所属 Worker 编号
    /// - `method`: 请求方法
    /// - `url`: 请求 URL
    pub fn record_failure(&self, latency_ns: u64, status: u16, worker: u32, method: &str, url: &str) {
        // 记录延迟到直方图（失败也计入延迟分布）
        self.histogram.lock().unwrap().record(latency_ns.max(1)).ok();
        // 原子累加失败数
        self.failure.fetch_add(1, Ordering::Relaxed);
        // 原子累加总请求数
        self.total.fetch_add(1, Ordering::Relaxed);
        // 记录失败状态码（若状态码非 0）
        if status != 0 {
            // 更新状态码分布
            self.note_status(status);
        }
        // 按需记录请求明细
        self.note_detail(worker, method, url, status, false, latency_ns, 0);
    }

    /// 更新状态码分布计数
    ///
    /// # 参数
    /// - `status`: 响应状态码
    fn note_status(&self, status: u16) {
        // 锁定状态码映射
        let mut map = self.status_codes.lock().unwrap();
        // 状态码计数加一（不存在则初始为 1）
        *map.entry(status).or_insert(0) += 1;
    }

    /// 按需记录请求明细（用于 CSV 导出）
    ///
    /// # 参数
    /// - `worker`: Worker 编号
    /// - `method`: 请求方法
    /// - `url`: 请求 URL
    /// - `status`: 状态码
    /// - `success`: 是否成功
    /// - `latency_ns`: 延迟（纳秒）
    /// - `size`: 响应体大小
    fn note_detail(&self, worker: u32, method: &str, url: &str, status: u16, success: bool, latency_ns: u64, size: u64) {
        // 未开启明细记录时直接返回
        if !self.record_details {
            return;
        }
        // 锁定明细列表
        let mut list = self.details.lock().unwrap();
        // 追加一条请求明细
        list.push(RequestDetail {
            // 时间戳（毫秒）
            timestamp_ms: chrono::Local::now().timestamp_millis() as u64,
            // Worker 编号
            worker,
            // 请求方法
            method: method.to_string(),
            // 请求 URL
            url: url.to_string(),
            // 状态码
            status,
            // 是否成功
            success,
            // 延迟（毫秒）
            latency_ms: latency_ns as f64 / 1_000_000.0,
            // 响应体大小
            size_bytes: size,
        });
    }

    /// 压测结束后汇总所有统计结果为 Summary
    ///
    /// # 参数
    /// - `url`: 目标 URL
    /// - `concurrency`: 并发数
    /// - `p99_threshold_ms`: p99 达标阈值（毫秒，可选）
    /// - `error_threshold_pct`: 错误率阈值（百分比，可选）
    ///
    /// # 返回值
    /// 汇总后的统计结果
    pub fn summarize(
        &self,
        url: &str,
        concurrency: usize,
        p99_threshold_ms: Option<f64>,
        error_threshold_pct: Option<f64>,
    ) -> Summary {
        // 读取总耗时
        let elapsed_secs = self.start.elapsed().as_secs_f64();
        // 读取各原子计数
        let total = self.total.load(Ordering::Relaxed);
        let success = self.success.load(Ordering::Relaxed);
        let failure = self.failure.load(Ordering::Relaxed);
        let bytes = self.bytes.load(Ordering::Relaxed);
        // 计算错误率（百分比）
        let error_rate_pct = if total > 0 {
            // 失败数除以总数
            failure as f64 / total as f64 * 100.0
        } else {
            // 无请求时错误率为 0
            0.0
        };
        // 计算实际 QPS
        let qps = if elapsed_secs > 0.0 {
            // 总数除以耗时
            total as f64 / elapsed_secs
        } else {
            // 耗时为零时 QPS 为 0
            0.0
        };
        // 锁定直方图并读取统计值
        let hist = self.histogram.lock().unwrap();
        // 辅助函数：读取指定百分位（无数据时返回 0）
        let pct = |p: f64| -> f64 {
            // 无记录时返回 0
            if hist.len() == 0 {
                return 0.0;
            }
            // 读取百分位值（纳秒）并转换为毫秒
            hist.value_at_quantile(p / 100.0) as f64 / 1_000_000.0
        };
        // 汇总阈值告警判断
        let mut threshold_messages = Vec::new();
        // 检查 p99 是否超过阈值
        let p99_ms = pct(99.0);
        if let Some(thr) = p99_threshold_ms {
            // p99 超过阈值时记录告警
            if p99_ms > thr {
                threshold_messages.push(format!("p99 延迟 {p99_ms:.2}ms 超过阈值 {thr}ms"));
            }
        }
        // 检查错误率是否超过阈值
        if let Some(thr) = error_threshold_pct {
            // 错误率超过阈值时记录告警
            if error_rate_pct > thr {
                threshold_messages.push(format!("错误率 {error_rate_pct:.2}% 超过阈值 {thr}%"));
            }
        }
        // 阈值是否触发（存在告警即为失败）
        let threshold_failed = !threshold_messages.is_empty();
        // 读取状态码分布快照
        let status_codes = self.status_codes.lock().unwrap().clone();
        // 返回汇总结果
        Summary {
            // 目标 URL
            url: url.to_string(),
            // 并发数
            concurrency,
            // 总数
            total,
            // 成功数
            success,
            // 失败数
            failure,
            // 错误率
            error_rate_pct,
            // 字节数
            bytes,
            // 耗时
            elapsed_secs,
            // QPS
            qps,
            // 最小延迟
            min_ms: if hist.len() > 0 { hist.min() as f64 / 1_000_000.0 } else { 0.0 },
            // 最大延迟
            max_ms: if hist.len() > 0 { hist.max() as f64 / 1_000_000.0 } else { 0.0 },
            // 平均延迟
            mean_ms: if hist.len() > 0 { hist.mean() / 1_000_000.0 } else { 0.0 },
            // 标准差
            stddev_ms: if hist.len() > 0 { hist.stdev() / 1_000_000.0 } else { 0.0 },
            // 各百分位延迟
            p50_ms: pct(50.0),
            p90_ms: pct(90.0),
            p95_ms: pct(95.0),
            p99_ms,
            p999_ms: pct(99.9),
            // 状态码分布
            status_codes,
            // 阈值告警标志
            threshold_failed,
            // 阈值告警信息
            threshold_messages,
        }
    }

    /// 取回当前延迟直方图的只读快照（用于绘制柱状图）
    ///
    /// # 返回值
    /// 直方图快照
    pub fn histogram_snapshot(&self) -> Histogram<u64> {
        // 克隆直方图（锁定后克隆）
        self.histogram.lock().unwrap().clone()
    }
}

/// 单元测试：验证统计逻辑
#[cfg(test)]
mod tests {
    use super::*;

    /// 测试：成功与失败请求的计数与百分位统计正确
    #[test]
    fn test_stats_record_and_summary() {
        // 创建统计聚合器（不记录明细）
        let stats = Stats::new(false).unwrap();
        // 记录 100 条成功请求（延迟 10ms=10_000_000ns）
        for i in 0..100 {
            // 记录一条成功请求
            stats.record_success(10_000_000 + i, 200, 1024, 0, "GET", "http://t/");
        }
        // 记录 10 条失败请求（延迟 50ms）
        for _ in 0..10 {
            // 记录一条失败请求
            stats.record_failure(50_000_000, 0, 0, "GET", "http://t/");
        }
        // 汇总统计结果（无阈值）
        let s = stats.summarize("http://t/", 10, None, None);
        // 断言总数、成功数、失败数正确
        assert_eq!(s.total, 110);
        assert_eq!(s.success, 100);
        assert_eq!(s.failure, 10);
        // 断言 p50 约为 10ms（允许小误差）
        assert!((s.p50_ms - 10.0).abs() < 1.0, "p50={}", s.p50_ms);
        // 断言错误率约为 9.09%
        assert!((s.error_rate_pct - 100.0 * 10.0 / 110.0).abs() < 0.01);
    }

    /// 测试：阈值告警在 p99 超限时触发
    #[test]
    fn test_threshold_trigger() {
        // 创建统计聚合器
        let stats = Stats::new(false).unwrap();
        // 记录 100 条延迟 100ms 的请求
        for _ in 0..100 {
            // 记录一条成功请求
            stats.record_success(100_000_000, 200, 100, 0, "GET", "http://t/");
        }
        // 以 50ms 作为 p99 阈值汇总（100ms 必然超限）
        let s = stats.summarize("http://t/", 1, Some(50.0), None);
        // 断言阈值告警触发
        assert!(s.threshold_failed);
    }
}

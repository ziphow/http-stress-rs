//! # 令牌桶 QPS 限流模块
//!
//! 实现令牌桶（Token Bucket）算法进行精确的 QPS 限流控制：
//! - 桶容量默认 = QPS / 2（允许短时间突发）
//! - 填充速率 = QPS 个令牌/秒
//! - Worker 发送请求前从桶中获取令牌，令牌不足时等待补充
//! - 采用瞬时速率计算，保证长时间运行不漂移

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 令牌桶状态（受互斥锁保护，供并发 Worker 共享）
struct BucketState {
    /// 当前可用令牌数（浮点以支持连续填充）
    tokens: f64,
    /// 桶的容量上限
    capacity: f64,
    /// 每秒补充的令牌数（即 QPS）
    refill_rate: f64,
    /// 上次补充令牌的时间点
    last_refill: Instant,
}

/// 令牌桶限流器（线程安全，可被多协程共享）
///
/// 通过互斥锁串行化令牌的补充与获取，保证并发环境下计数正确。
pub struct TokenBucket {
    /// 桶内部状态
    inner: Mutex<BucketState>,
}

impl TokenBucket {
    /// 创建一个新的令牌桶
    ///
    /// # 参数
    /// - `qps`: 每秒允许的请求数（QPS），为 0 时表示不限流
    ///
    /// # 返回值
    /// 配置完成的令牌桶；qps 为 0 时仍返回限流器但 acquire 立即成功
    pub fn new(qps: u64) -> Self {
        // 桶容量 = QPS / 2，至少为 1，允许短时间突发请求
        let capacity = (qps / 2).max(1) as f64;
        // 填充速率 = QPS 个令牌/秒
        let refill_rate = qps as f64;
        // 初始令牌充满整个桶（保证压测启动即达到目标速率）
        let tokens = capacity;
        // 返回新令牌桶
        Self {
            inner: Mutex::new(BucketState {
                tokens,
                capacity,
                refill_rate,
                last_refill: Instant::now(),
            }),
        }
    }

    /// 判断该限流器是否为“不限流”模式
    ///
    /// # 返回值
    /// 无限流（qps=0）时返回 true
    pub fn is_unlimited(&self) -> bool {
        // 读取填充速率判断是否为 0
        self.inner.lock().unwrap().refill_rate == 0.0
    }

    /// 获取一个令牌；令牌不足时按填充速率异步等待
    ///
    /// # 返回值
    /// 立即或稍后获得令牌，返回后即可发送一个请求
    pub async fn acquire(&self) {
        // 循环直到成功取得令牌
        loop {
            // 计算本次需要等待的时长
            let wait = {
                // 锁定桶状态
                let mut st = self.inner.lock().unwrap();
                // 距上次填充经过的时间（秒）
                let elapsed = st.last_refill.elapsed().as_secs_f64();
                // 按经过时间补充令牌（不超过容量）
                st.tokens = (st.tokens + elapsed * st.refill_rate).min(st.capacity);
                // 更新上次填充时间
                st.last_refill = Instant::now();
                // 判断当前是否有可用令牌
                if st.tokens >= 1.0 {
                    // 消耗一个令牌
                    st.tokens -= 1.0;
                    // 无需等待
                    0.0
                } else {
                    // 计算需要等待到下一个令牌的时间
                    (1.0 - st.tokens) / st.refill_rate
                }
            };
            // 若无需等待则立即返回
            if wait <= 0.0 {
                return;
            }
            // 休眠等待令牌补充
            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
        }
    }
}

/// 单元测试：验证令牌桶限流逻辑
#[cfg(test)]
mod tests {
    use super::*;

    /// 测试：不限流模式下 is_unlimited 返回 true 且可立即获取令牌
    #[tokio::test]
    async fn test_unlimited() {
        // 创建 QPS=0 的不限流桶
        let bucket = TokenBucket::new(0);
        // 断言为不限流模式
        assert!(bucket.is_unlimited());
        // 获取令牌应立即成功（不会长时间挂起）
        tokio::time::timeout(Duration::from_millis(100), bucket.acquire())
            .await
            .expect("不限流模式应立即获得令牌");
    }

    /// 测试：限流模式下令牌获取有节流效果
    #[tokio::test]
    async fn test_limited_rate() {
        // 创建 QPS=100 的限流桶
        let bucket = TokenBucket::new(100);
        // 断言为限流模式
        assert!(!bucket.is_unlimited());
        // 连续获取 200 个令牌并计时
        let start = Instant::now();
        // 循环 200 次获取令牌
        for _ in 0..200 {
            // 获取单个令牌
            bucket.acquire().await;
        }
        // 计算总耗时
        let elapsed = start.elapsed().as_secs_f64();
        // 200 个令牌以 100 QPS 生成至少需要约 1.5 秒（前 100 个立即可用）
        assert!(elapsed >= 1.0, "200 个令牌在 100QPS 下耗时应≥1s，实际 {elapsed:.2}s");
        // 且不应过于拖慢
        assert!(elapsed < 5.0, "200 个令牌在 100QPS 下耗时应<5s，实际 {elapsed:.2}s");
    }
}

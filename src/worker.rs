//! # 负载调度与 Worker Pool 模块（Tokio 异步）
//!
//! 采用 Worker Pool（线程池）模式管理并发请求：
//! - 使用 Tokio 多线程异步运行时，`tokio::spawn` 创建并发 Worker 协程
//! - 通过原子计数器（Atomic）维护已分发请求数与已测量请求数
//! - 支持按「总请求数」或「压测时长」两种停止条件
//! - 支持预热模式（Warm-up）：丢弃前若干请求结果以消除冷启动影响
//! - 请求前通过令牌桶进行 QPS 限流
//! - 支持数据集驱动的随机化请求

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use indicatif::ProgressBar;

use crate::cli;
use crate::config::Config;
use crate::dataset::{DataSet, SelectMode};
use crate::limiter::TokenBucket;
use crate::stats::Stats;

/// 压测运行上下文（在并发 Worker 间共享）
pub struct RunContext {
    /// 合并后的压测配置
    pub cfg: Config,
    /// 压测专用的 HTTP 客户端
    pub client: reqwest::Client,
    /// 全局统计聚合器
    pub stats: Arc<Stats>,
    /// 令牌桶限流器（可选）
    pub limiter: Option<Arc<TokenBucket>>,
    /// 数据集（可选）
    pub dataset: Option<Arc<DataSet>>,
    /// 数据集选择策略
    pub dataset_mode: SelectMode,
    /// 已分发请求总数（原子计数器，含预热）
    pub dispatched: AtomicU64,
    /// 已测量请求数（原子计数器，不含预热）
    pub measured: AtomicU64,
}

impl RunContext {
    /// 根据合并配置构建压测运行上下文
    ///
    /// # 参数
    /// - `cfg`: 合并后的压测配置
    /// - `stats`: 全局统计聚合器
    ///
    /// # 返回值
    /// 构建完成的运行上下文；数据集或客户端创建失败时返回错误
    pub fn build(cfg: Config, stats: Arc<Stats>) -> anyhow::Result<Self> {
        // 构建 HTTP 客户端
        let client = crate::client::build_client(
            cfg.http2,
            cfg.no_tls,
            cfg.timeout,
            cfg.redirect,
            &cfg.headers,
        )?;
        // 按配置创建限流器（rate>0 时启用）
        let limiter = match cfg.rate {
            // 配置了限流：创建令牌桶
            Some(r) if r > 0 => Some(Arc::new(TokenBucket::new(r))),
            // 未配置限流：不创建
            _ => None,
        };
        // 按配置加载数据集
        let dataset = match &cfg.dataset {
            // 指定了数据集文件：加载数据集
            Some(path) => Some(Arc::new(DataSet::from_file(path)?)),
            // 未指定数据集：不加载
            None => None,
        };
        // 解析数据集选择策略
        let dataset_mode = SelectMode::parse(&cfg.dataset_mode)?;
        // 返回运行上下文
        Ok(RunContext {
            // 保存配置
            cfg,
            // 保存客户端
            client,
            // 保存统计器
            stats,
            // 保存限流器
            limiter,
            // 保存数据集
            dataset,
            // 保存数据集策略
            dataset_mode,
            // 分发计数器初始为 0
            dispatched: AtomicU64::new(0),
            // 测量计数器初始为 0
            measured: AtomicU64::new(0),
        })
    }

    /// 开始压测：并发运行 Worker Pool，直到停止条件达成
    ///
    /// # 参数
    /// - `progress`: 可选进度条（实时展示进度与 RPS）
    ///
    /// # 返回值
    /// 压测正常结束返回 Ok；未指定停止条件时返回错误
    pub async fn run(self: &Arc<RunContext>, progress: Option<Arc<ProgressBar>>) -> anyhow::Result<()> {
        // 校验停止条件：必须指定请求数或时长
        if self.cfg.requests.is_none() && self.cfg.time.is_none() {
            // 返回参数错误
            return Err(anyhow::anyhow!(
                "必须指定停止条件：-n/--requests（总请求数）或 -t/--time（压测时长）"
            ));
        }
        // 计算时间截止点（未指定时长时为 None）
        let deadline = self.cfg.time.map(|secs| Instant::now() + Duration::from_secs(secs));
        // 创建 Worker 任务列表
        let mut tasks = Vec::new();
        // 按并发数创建 Worker 协程
        for worker_id in 0..self.cfg.concurrency {
            // 克隆运行上下文引用
            let ctx = self.clone();
            // 克隆进度条引用
            let progress = progress.clone();
            // 派生 Worker 协程并加入任务列表
            tasks.push(tokio::spawn(Self::worker_loop(ctx, worker_id, deadline, progress)));
        }
        // 等待所有 Worker 完成
        for task in tasks {
            // 等待单个 Worker 完成
            task.await.map_err(|e| anyhow::anyhow!("Worker 协程异常终止：{e}"))?;
        }
        // 压测正常结束
        Ok(())
    }

    /// 单个 Worker 的循环执行逻辑
    ///
    /// # 参数
    /// - `self_`: 运行上下文引用
    /// - `worker_id`: Worker 编号（用于 CSV 明细）
    /// - `deadline`: 时间截止点（可选）
    /// - `progress`: 进度条引用（可选）
    async fn worker_loop(
        self_: Arc<RunContext>,
        worker_id: usize,
        deadline: Option<Instant>,
        progress: Option<Arc<ProgressBar>>,
    ) {
        // 循环发送请求直到停止条件达成
        loop {
            // 停止条件 1：压测时长已到
            if let Some(d) = deadline {
                // 已超过截止时间则退出循环
                if Instant::now() >= d {
                    break;
                }
            }
            // 停止条件 2：已测量请求数达到目标
            if let Some(target) = self_.cfg.requests {
                // 已测量数达到目标则退出循环
                if self_.measured.load(Ordering::Relaxed) >= target {
                    break;
                }
            }
            // 请求前进行 QPS 限流（令牌桶）
            if let Some(limiter) = &self_.limiter {
                // 从令牌桶获取令牌（必要时等待）
                limiter.acquire().await;
            }
            // 获取本次请求的全局序号（原子自增，含预热）
            let seq = self_.dispatched.fetch_add(1, Ordering::Relaxed);
            // 判断是否为预热请求（序号小于预热数）
            let is_warmup = seq < self_.cfg.warmup;
            // 非预热请求：测量计数加一
            if !is_warmup {
                // 原子累加测量数
                self_.measured.fetch_add(1, Ordering::Relaxed);
            }
            // 发送并处理本次请求
            self_.send_one(worker_id, seq, is_warmup).await;
            // 更新进度条
            if let Some(bar) = &progress {
                // 进度条前进一格
                bar.inc(1);
            }
        }
    }

    /// 发送单个请求并记录统计结果
    ///
    /// # 参数
    /// - `worker_id`: Worker 编号
    /// - `seq`: 全局请求序号
    /// - `is_warmup`: 是否为预热请求（不入统计）
    async fn send_one(&self, worker_id: usize, seq: u64, is_warmup: bool) {
        // 确定本次请求的 URL、方法、请求头与请求体
        let (url, method, headers, body) = match &self.dataset {
            // 使用数据集：挑选并渲染一条模板
            Some(ds) => {
                // 按策略挑选模板
                let tpl = ds.pick(self.dataset_mode, seq);
                // 渲染模板中的随机占位符
                let (u, m, h, b) = tpl.render(&self.cfg.url);
                // 将模板头映射转换为向量
                let headers_vec: Vec<(String, String)> = h.into_iter().collect();
                // 返回渲染结果
                (u, m, headers_vec, b)
            }
            // 使用命令行配置：固定 URL/方法/请求头/请求体
            None => (
                self.cfg.url.clone(),
                self.cfg.method.clone(),
                self.cfg.headers.clone(),
                self.cfg.body.clone(),
            ),
        };
        // 解析请求方法
        let method = match cli::parse_method(&method) {
            // 方法合法
            Ok(m) => m,
            // 方法非法：跳过本次请求
            Err(_) => return,
        };
        // 执行请求（含重试），返回 (延迟纳秒, 状态码, 字节数, 是否成功)
        let (latency_ns, status, size, success) = self
            .execute_with_retry(&url, &method, &headers, body.as_deref())
            .await;
        // 预热请求不进入统计
        if is_warmup {
            return;
        }
        // 记录统计结果
        if success {
            // 记录一次成功请求
            self.stats
                .record_success(latency_ns, status, size, worker_id as u32, method.as_str(), &url);
        } else {
            // 记录一次失败请求
            self.stats
                .record_failure(latency_ns, status, worker_id as u32, method.as_str(), &url);
        }
    }

    /// 执行请求并支持失败重试
    ///
    /// # 参数
    /// - `url`: 请求 URL
    /// - `method`: 请求方法
    /// - `headers`: 请求头（名称, 值）
    /// - `body`: 请求体（可选）
    ///
    /// # 返回值
    /// (延迟纳秒, 状态码, 响应体字节数, 是否成功)
    ///
    /// 说明：每次重试均重新构建请求（RequestBuilder 不可 Clone），
    /// 确保带请求体的请求在重试时仍携带完整的请求内容。
    async fn execute_with_retry(
        &self,
        url: &str,
        method: &reqwest::Method,
        headers: &[(String, String)],
        body: Option<&str>,
    ) -> (u64, u16, u64, bool) {
        // 重试次数（含首次，最多 retries+1 次）
        let attempts = self.cfg.retries + 1;
        // 循环执行请求
        for attempt in 0..attempts {
            // 记录请求开始时间（含完整的网络往返与响应体读取）
            let start = Instant::now();
            // 每次重新构建请求构建器（保证重试时请求内容完整）
            let req = crate::client::build_request(&self.client, url, method, headers, body);
            // 发送请求并等待响应
            let result = req.send().await;
            // 判断请求结果
            match result {
                // 收到响应
                Ok(resp) => {
                    // 读取响应状态码
                    let status = resp.status().as_u16();
                    // 读取完整响应体（确保连接可复用并统计字节数）
                    let body_res = resp.bytes().await;
                    // 根据读取结果计算字节数
                    let size = match body_res {
                        // 读取成功：字节长度
                        Ok(bytes) => bytes.len() as u64,
                        // 读取失败：0 字节
                        Err(_) => 0,
                    };
                    // 计算完整请求延迟（含响应体读取）
                    let latency = start.elapsed().as_nanos() as u64;
                    // 成功标准：状态码 < 400
                    let success = status < 400;
                    // 返回本次结果
                    return (latency, status, size, success);
                }
                // 网络错误
                Err(_) => {
                    // 计算失败请求延迟
                    let latency = start.elapsed().as_nanos() as u64;
                    // 判断是否最后一次尝试
                    let is_last = attempt + 1 >= attempts;
                    // 失败且可重试时继续循环
                    if !is_last {
                        // 短暂等待后重试
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                    // 返回失败结果（状态码 0、0 字节）
                    return (latency, 0, 0, false);
                }
            }
        }
        // 理论不可达：返回失败
        (0, 0, 0, false)
    }
}

/// 从数据集文件加载并解析选择策略
impl SelectMode {
    /// 解析选择策略字符串
    ///
    /// # 参数
    /// - `s`: 策略名（"random"/"sequential"）
    ///
    /// # 返回值
    /// 解析成功返回枚举；失败返回错误
    fn parse(s: &str) -> anyhow::Result<SelectMode> {
        // 调用数据集模块的解析函数
        crate::dataset::parse_mode(s)
    }
}

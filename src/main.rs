//! # Rust HTTP 压测工具（rhb）程序入口
//!
//! 主流程编排：
//! 1. 解析命令行参数（Clap）
//! 2. 合并 YAML 配置文件（CLI 优先）
//! 3. 构建 HTTP 客户端、限流器、数据集
//! 4. 并发运行 Worker Pool 进行压测（含实时进度条）
//! 5. 汇总统计并生成终端报告 / JSON / CSV
//! 6. 依据性能阈值决定进程退出码（CI 集成）

use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};

use rhb::cli::Cli;
use rhb::config::Config;
use rhb::stats::Stats;
use rhb::worker::RunContext;

/// 程序主入口
///
/// # 返回值
/// 进程退出码：0 成功；1 运行错误；2 性能阈值未达标（CI 判定失败）
#[tokio::main]
async fn main() {
    // 解析命令行参数
    let cli = Cli::parse();
    // 执行压测主流程并处理退出码
    let code = run(cli).await;
    // 以退出码结束进程
    std::process::exit(code);
}

/// 压测主流程
///
/// # 参数
/// - `cli`: 解析后的命令行参数
///
/// # 返回值
/// 进程退出码（0/1/2）
async fn run(cli: Cli) -> i32 {
    // 1. 合并配置（CLI > YAML 文件 > 默认值）
    let cfg = match Config::from_cli(&cli) {
        // 合并成功
        Ok(cfg) => cfg,
        // 配置错误：打印错误并返回退出码 1
        Err(e) => {
            eprintln!("配置错误：{e}");
            return 1;
        }
    };

    // 2. 创建统计聚合器（开启 CSV 明细时记录每条请求）
    let stats = match Stats::new(cfg.csv.is_some()) {
        // 创建成功
        Ok(s) => Arc::new(s),
        // 创建失败
        Err(e) => {
            eprintln!("初始化统计引擎失败：{e}");
            return 1;
        }
    };

    // 3. 构建压测运行上下文
    let ctx = match RunContext::build(cfg.clone(), stats.clone()) {
        // 构建成功
        Ok(c) => Arc::new(c),
        // 构建失败
        Err(e) => {
            eprintln!("初始化压测上下文失败：{e}");
            return 1;
        }
    };

    // 4. 判断是否需要展示进度条（终端 + 未禁用）
    let use_progress = !cfg.no_progress && std::io::stdout().is_terminal();
    // 创建进度条（可选）
    let progress = if use_progress {
        // 计算进度条总长度（预热 + 正式请求数）
        let total = cfg
            .requests
            .map(|r| r + cfg.warmup)
            .unwrap_or(0);
        // 创建进度条
        let bar = ProgressBar::new(total);
        // 设置进度条样式
        bar.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-"),
        );
        // 包装为 Arc 共享引用
        Some(Arc::new(bar))
    } else {
        // 不显示进度条
        None
    };

    // 5. 启动实时状态更新任务（展示 RPS 与平均延迟）
    let updater = if let Some(bar) = &progress {
        // 克隆统计器与进度条引用
        let stats2 = stats.clone();
        let bar2 = bar.clone();
        // 生成后台更新协程
        Some(tokio::spawn(async move {
            // 创建 500ms 周期定时器
            let mut ticker = tokio::time::interval(Duration::from_millis(500));
            // 循环更新进度条消息
            loop {
                // 等待下一个周期
                ticker.tick().await;
                // 计算当前已发送请求数与 RPS
                let done = stats2.total.load(std::sync::atomic::Ordering::Relaxed);
                // 计算经过时间
                let elapsed = stats2.start.elapsed().as_secs_f64();
                // 计算实时 RPS
                let rps = if elapsed > 0.0 { done as f64 / elapsed } else { 0.0 };
                // 计算平均延迟（毫秒）
                let mean_ms = stats2.histogram_snapshot().mean() / 1_000_000.0;
                // 更新进度条消息
                bar2.set_message(format!("RPS {rps:.0} | avg {mean_ms:.2}ms"));
            }
        }))
    } else {
        // 无进度条时不启动更新任务
        None
    };

    // 6. 并发运行压测（Worker Pool）
    if let Err(e) = ctx.run(progress.clone()).await {
        // 压测运行错误：打印错误
        eprintln!("压测运行失败：{e}");
        // 停止后台更新任务
        if let Some(u) = updater {
            // 终止更新协程
            u.abort();
        }
        // 结束进度条
        if let Some(bar) = progress {
            // 清除进度条
            bar.finish_and_clear();
        }
        // 返回退出码 1
        return 1;
    }

    // 7. 停止后台更新任务并完成进度条
    if let Some(u) = updater {
        // 终止更新协程
        u.abort();
    }
    if let Some(bar) = progress {
        // 完成进度条
        bar.finish_and_clear();
    }

    // 8. 汇总统计结果
    let summary = stats.summarize(
        &cfg.url,
        cfg.concurrency,
        cfg.p99_threshold_ms,
        cfg.error_threshold_pct,
    );

    // 9. 输出报告（JSON 或终端文本）
    if cfg.json {
        // JSON 输出到终端
        println!("{}", rhb::report::export_json(&summary));
    } else {
        // 获取直方图快照（可选）
        let hist_snap = if cfg.histogram {
            // 需要柱状图时获取快照
            Some(stats.histogram_snapshot())
        } else {
            // 不需要柱状图
            None
        };
        // 渲染并输出终端报告
        println!("{}", rhb::report::render_terminal(&summary, hist_snap.as_ref()));
    }

    // 10. 导出 JSON 文件
    if let Some(path) = &cfg.json_file {
        // 生成 JSON 字符串
        let json = rhb::report::export_json(&summary);
        // 写入 JSON 文件
        if let Err(e) = rhb::report::write_json_file(&json, path) {
            // 写入失败：打印错误
            eprintln!("导出 JSON 文件失败：{e}");
        } else {
            // 写入成功：打印提示
            eprintln!("JSON 结果已导出：{path}");
        }
    }

    // 11. 导出 CSV 明细
    if let Some(path) = &cfg.csv {
        // 锁定并读取请求明细
        let details = stats.details.lock().unwrap().clone();
        // 导出 CSV 文件
        match rhb::report::export_csv(&details, path) {
            // 导出成功
            Ok(n) => eprintln!("CSV 明细已导出：{path}（{n} 条记录）"),
            // 导出失败
            Err(e) => eprintln!("导出 CSV 失败：{e}"),
        }
    }

    // 12. 依据阈值决定退出码（未达标返回 2）
    if summary.threshold_failed {
        // 阈值未达标：返回退出码 2
        return 2;
    }
    // 压测成功：返回退出码 0
    0
}

# rhb —— Rust HTTP 压测工具

`rhb`（Rust HTTP Benchmarker）是一个对标 [oha](https://github.com/hatoo/oha) / [wrk](https://github.com/wg/wrk) / [hey](https://github.com/rakyll/hey) 的高性能 HTTP 压测工具，基于 Rust 异步生态从零构建，用于对目标 HTTP 服务进行压力测试与性能评估。

## 功能特性

- **多种请求生成方式**：同步/异步请求生成，自定义请求方法、URL、请求头、请求体
- **多协议支持**：HTTP/1.1、HTTP/2、HTTPS（纯 Rust rustls TLS，无需 OpenSSL）
- **灵活的并发配置**：并发数、总请求数、压测时长
- **精准 QPS 限流**：基于令牌桶算法，可精确控制请求速率
- **数据集随机化压测**：JSON 数据集驱动，支持 `{{random.*}}` 模板动态生成参数
- **高精度统计**：基于 HdrHistogram 输出 p50/p90/p95/p99/p99.9 延迟分布、QPS、错误率
- **多格式导出**：终端报告、JSON 结构化输出、CSV 请求明细
- **Web 图形界面**：内置本地 Web UI，可视化配置参数、实时展示结果图表
- **CI 友好**：可配置性能阈值（p99、错误率），不达标时退出码为 2

## Web 图形界面

`rhb` 附带一个本地 Web 图形界面（`webui/`），无需记忆命令行参数，可视化配置并一键压测。

### 启动方式

```bash
# Windows：双击 webui/启动UI.bat（自动打开浏览器）
# 或命令行启动：
python webui/server.py

# Linux / macOS：
bash webui/start_ui.sh
```

服务默认运行在 `http://127.0.0.1:8000/`，启动后自动打开浏览器。

> 前置要求：已通过 `cargo build --release` 生成 `target/release/rhb(.exe)`，且本机安装 Python 3.8+。

### 界面功能

- **参数配置**：目标 URL、请求方法、并发数、请求数/时长、QPS 限流、请求头、请求体、HTTP/2、TLS、预热、超时、数据集、性能阈值等
- **一键压测**：点击「开始压测」后台驱动 `rhb` 执行，可随时「停止」
- **结果展示**：QPS、请求量、错误率等指标卡片；p50/p90/p95/p99 延迟卡片；延迟柱状图与状态码分布图（ECharts）
- **结果导出**：一键下载 JSON 汇总与 CSV 请求明细（`webui/results/`）

## 快速开始

### 环境要求

- Rust 1.70+（推荐最新稳定版）
- Windows / Linux / macOS

### 构建

```bash
cargo build --release
```

构建产物位于 `target/release/rhb`（Windows 下为 `rhb.exe`）。

### 准备压测目标（先启动一个服务器）

`rhb` 需要一个 HTTP 服务作为压测目标。务必**先启动目标、再对它压测**，否则会得到 100% 错误率。

**方式 A：使用项目内置的演示服务器（推荐）**

```bash
cargo run --example demo_server
```

默认监听 `127.0.0.1:8080`，提供多种端点以便验证不同场景：

| 端点 | 说明 | 适合验证 |
|:---|:---|:---|
| `GET /` | 固定文本响应 | 基础压测 |
| `GET /json` | JSON 响应 | 正常吞吐 |
| `GET /delay/{ms}` | 延迟指定毫秒后返回 | 延迟统计 |
| `GET /error` | 返回 500 错误 | 错误率统计 |
| `POST /echo` | 回显请求体 | POST / 请求体 |

**方式 B：无 Rust 环境的免编译目标（Python 多线程服务器）**

不想编译演示服务器时，可用 Python 内置服务器临时充当目标：

```bash
python -c "from http.server import ThreadingHTTPServer,SimpleHTTPRequestHandler;ThreadingHTTPServer(('127.0.0.1',8080),SimpleHTTPRequestHandler).serve_forever()"
```

> ⚠️ 请用 `ThreadingHTTPServer`（多线程）。`python -m http.server` 是**单线程**的，高并发下会被打崩导致大量失败。

目标就绪后，**另开一个终端**参看下方基本用法对它发起压测。

### 基本用法

```bash
# 对本地服务发起 10000 个请求，50 并发
rhb http://127.0.0.1:8080 -n 10000 -c 50

# 压测 30 秒，输出延迟柱状图
rhb https://example.com/api -t 30 --histogram

# POST 请求，带请求头与请求体
rhb http://127.0.0.1:8080/api -X POST -H "Content-Type: application/json" -d '{"name":"test"}'

# QPS 限流：每秒最多 500 请求
rhb http://127.0.0.1:8080 -n 5000 --rate 500

# 启用 HTTP/2
rhb https://example.com -n 1000 --http2

# 数据集驱动的随机化压测
rhb http://127.0.0.1:8080 --dataset dataset/dataset.example.json -n 2000

# 导出 JSON 结果与 CSV 明细
rhb http://127.0.0.1:8080 -n 1000 --json-file result.json --csv result.csv

# 使用 YAML 配置文件
rhb -C config/config.example.yaml

# CI 阈值判定：p99 超过 500ms 或错误率超过 5% 时退出码为 2
rhb http://127.0.0.1:8080 -n 1000 --p99-threshold-ms 500 --error-threshold-pct 5
```

## 命令行参数

| 参数 | 类别 | 说明 | 默认值 |
|:---|:---|:---|:---|
| `URL` | 目标 | 目标服务 URL（**必选**位置参数）。可含路径、查询串，如 `http://192.168.1.10:8080/api/v1/users?page=2`。 | - |
| `-c, --concurrency` | 目标 | 并发连接数，即同时进行的请求数。压测前会先创建该数量个 Worker 协程并行发请求。 | `50` |
| `-n, --requests` | 目标 | 总请求数（含预热）。当同时指定 `-t` 时，先到者即停止。**与 `-t` 至少指定一个**，否则报错退出（退出码 1）。 | 无限制* |
| `-t, --time` | 目标 | 压测时长（秒），到点后停止发送新请求。**与 `-n` 至少指定一个**，否则报错退出（退出码 1）。 | 无限制* |
| `-X, --method` | 请求内容 | 请求方法。支持 `GET` / `POST` / `PUT` / `DELETE` / `PATCH` / `HEAD` / `OPTIONS`。 | `GET` |
| `-H, --header` | 请求内容 | 自定义请求头，**可多次指定**，格式 `"名称: 值"`（按第一个冒号分割，两侧空白自动去除）。如 `-H "Content-Type: application/json" -H "X-Token: abc"`。 | - |
| `-d, --body` | 请求内容 | 请求体（内联字符串）。常与 `-X POST` 搭配。 | - |
| `-f, --file` | 请求内容 | 从文件读取请求体（内容作为请求体发送）。与 `-d` 互斥，同时指定时以 `-d` 为准。 | - |
| `--rate` | 限流 | QPS 限流，基于**令牌桶算法**精确控制每秒最大请求数；`0` 表示不限流。受并发数约束。 | 不限流 |
| `--warmup` | 限流 | 预热请求数，**不计入任何统计**，用于消除连接建立 / TLS 握手等冷启动开销对指标的影响。 | `0` |
| `--http2` | 网络 | 启用 HTTP/2 协议（H2 需要 HTTPS，或经 HTTP/1.1 升级协商）。 | 关闭 |
| `--no-tls` | 网络 | 关闭 TLS 证书校验。**仅用于自签名证书 / 测试环境，生产勿用**。 | 关闭（校验） |
| `--timeout` | 网络 | 单请求超时（秒），超时该请求计为失败。 | `30` |
| `--retries` | 网络 | 单个请求的失败重试次数。每次重试仍计入请求总数。 | `0` |
| `--no-redirect` | 网络 | 禁止跟随 3xx 重定向（默认自动跟随）。 | 跟随 |
| `--dataset` | 数据集 | JSON 数据集文件路径，从中读取请求模板（方法 / 路径 / 请求头 / 请求体），支持 `{{random.*}}` 占位符动态渲染参数。 | - |
| `--dataset-mode` | 数据集 | 数据集选择策略：`random`（每次随机取一条模板）或 `sequential`（按顺序依次取）。 | `random` |
| `--histogram` | 输出 | 在终端输出 ASCII 延迟分布柱状图。 | 关闭 |
| `--json` | 输出 | 以 JSON 结构化格式输出报告到终端（便于脚本解析）。 | 关闭 |
| `--json-file` | 输出 | 将 JSON 汇总结果导出到指定文件（供 Web UI / CI 使用）。 | - |
| `--csv` | 输出 | 将 CSV 请求明细（每条请求一条记录，含耗时、状态码等）导出到指定文件。 | - |
| `--no-progress` | 输出 | 隐藏终端进度条（向管道重定向输出时脚本会自动隐藏）。 | 显示 |
| `-C, --config` | 输出 | YAML 配置文件路径。所有未在命令行指定的参数从中读取，命令行优先级最高。 | - |
| `--p99-threshold-ms` | CI | p99 延迟达标阈值（毫秒）。压测完成后若实测 p99 **超过**此值，进程以退出码 `2` 结束。 | - |
| `--error-threshold-pct` | CI | 错误率阈值（百分比 0~100）。压测完成后若实测错误率**超过**此值，进程以退出码 `2` 结束。 | - |

> *注：`-n` 与 `-t` **至少指定一个**，否则报错退出（退出码 1）。凡声明为"可合并"的参数均可省略（此时取 YAML 配置或内置默认值）；未列出的布尔开关默认关闭。

## YAML 配置文件

配置优先级：**命令行参数 > 配置文件 > 内置默认值**。

```yaml
# config/config.example.yaml
url: http://127.0.0.1:8080
concurrency: 100
requests: 10000
method: GET
headers:
  User-Agent: rhb-benchmark/0.1
warmup: 100
histogram: true
json_file: result.json
```

## 数据集格式

```json
{
  "templates": [
    { "method": "GET", "path": "/", "headers": {}, "body": null },
    { "method": "POST", "path": "/echo",
      "headers": { "Content-Type": "application/json" },
      "body": "{\"username\":\"{{random.string(8)}}\",\"age\":{{random.int(18,80)}}}" }
  ]
}
```

支持的模板占位符：

| 占位符 | 说明 | 示例输出 |
|:---|:---|:---|
| `{{random.string(N)}}` | 随机字母数字字符串 | `aB3xK9pQ` |
| `{{random.int(A,B)}}` | 范围内随机整数 | `52341` |
| `{{random.uuid}}` | 随机 UUID | `6f1c...-...` |
| `{{random.email}}` | 随机邮箱 | `uXm2kFq@zjPw8n.com` |

## 输出示例

```
==========================================================
  Rust HTTP 压测工具报告（rhb v1.0.0）
==========================================================
目标 URL      : http://127.0.0.1:8080
并发数        : 50
压测耗时      : 3.21 s
==========================================================
总请求数      : 10000
成功请求数    : 10000
失败请求数    : 0
错误率        : 0.00%
总传输字节    : 250000 bytes
实际 QPS      : 3115.26
==========================================================
延迟统计（毫秒）：
  最小延迟   : 0.512
  平均延迟   : 14.387
  最大延迟   : 238.910
  标准差     : 21.345
  p50        : 12.003
  p90        : 28.447
  p95        : 39.210
  p99        : 78.315
  p99.9      : 152.400
==========================================================
状态码分布    :
  200: 10000
==========================================================
```

## 测试

```bash
# 单元测试
cargo test --lib

# 集成测试（自动启动本地目标服务）
cargo test --test load_test

# 运行演示目标服务器
cargo run --example demo_server

# 另一终端压测演示服务器
cargo run --release -- http://127.0.0.1:8080 -n 1000 -c 20
```

## 退出码约定

| 退出码 | 含义 |
|:---:|:---|
| 0 | 压测成功，性能达标 |
| 1 | 运行错误（配置错误、网络错误等） |
| 2 | 压测完成但性能阈值未达标（CI 判定失败） |

## 项目结构

```
rhb/
├── Cargo.toml            # 依赖与构建配置
├── LICENSE               # MIT 开源许可
├── src/
│   ├── main.rs           # 程序入口与主流程编排
│   ├── lib.rs            # 库入口与模块声明
│   ├── cli.rs            # 命令行参数解析（Clap 4）
│   ├── config.rs         # YAML 配置加载与合并
│   ├── client.rs         # HTTP 客户端引擎（Reqwest + rustls）
│   ├── dataset.rs        # 数据集随机化压测与模板渲染
│   ├── limiter.rs        # 令牌桶 QPS 限流
│   ├── stats.rs          # HdrHistogram 统计与原子计数器
│   ├── worker.rs         # Worker Pool 负载调度
│   └── report.rs         # 报告生成与 JSON/CSV 导出
├── webui/                # Web 图形界面
│   ├── index.html        # 前端页面（参数配置 + 结果图表）
│   ├── server.py         # 本地后端（驱动 rhb 执行 + SSE 推送）
│   ├── 启动UI.bat        # Windows 一键启动
│   └── start_ui.sh       # Linux / macOS 一键启动
├── examples/
│   └── demo_server.rs    # 演示目标服务器
├── config/
│   └── config.example.yaml
├── dataset/
│   └── dataset.example.json
└── tests/
    └── load_test.rs      # 端到端集成测试
```

## 许可

本项目基于 MIT 许可开源，详见 [LICENSE](LICENSE) 与《版权和许可文档》。

## 免责声明

本工具仅用于对你有权测试的目标服务进行性能测试，请勿用于未经授权的服务，以免造成服务异常或违反服务条款。

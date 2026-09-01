#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""rhb Web UI 本地后端服务（仅依赖 Python 标准库，无需安装任何第三方包）。

职责：
- 提供静态页面（webui/index.html）
- POST /api/run    接收压测配置并启动 rhb.exe 子进程
- POST /api/stop   终止正在运行的压测
- GET  /api/events Server-Sent Events 实时推送输出行与最终结果
- GET  /download/<name> 下载导出的 JSON / CSV 结果文件

用法：
    python server.py [--port 8000] [--exe 路径/to/rhb.exe]
"""
import argparse
import json
import os
import queue
import subprocess
import sys
import threading
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import unquote, urlparse

BASE_DIR = os.path.dirname(os.path.abspath(__file__))
RESULTS_DIR = os.path.join(BASE_DIR, "results")
DEFAULT_EXE = os.path.join(BASE_DIR, "..", "target", "release", "rhb.exe")


def find_rhb_exe():
    candidates = [
        os.environ.get("RHB_EXE"),
        DEFAULT_EXE,
        os.path.join(BASE_DIR, "rhb.exe"),
        os.path.join(BASE_DIR, "..", "target", "release", "rhb"),
    ]
    for c in candidates:
        if c and os.path.isfile(c):
            return os.path.abspath(c)
    return None


EXE = find_rhb_exe()


class BenchState:
    """全局压测运行状态。"""
    lock = threading.Lock()
    proc = None            # 当前 rhb 子进程
    queues = []            # SSE 订阅者队列
    active = False
    result_json = None     # 本次导出的 JSON 文件绝对路径
    result_csv = None      # 本次导出的 CSV 文件绝对路径
    lines = []             # 已产生的输出行（缓存，供迟到的订阅者回放）
    last_done = None       # 最近一次完成/失败事件（缓存，供回放）


def broadcast(event, data):
    """向所有 SSE 订阅者推送事件，并缓存供迟到的订阅者回放。"""
    with BenchState.lock:
        if event == "line":
            BenchState.lines.append(data)
            if len(BenchState.lines) > 5000:
                del BenchState.lines[: len(BenchState.lines) - 5000]
        elif event in ("done", "fatal"):
            BenchState.last_done = (event, data)
        for q in list(BenchState.queues):
            try:
                q.put_nowait((event, data))
            except queue.Full:
                pass


def build_args(cfg):
    """把前端配置转换为 rhb 命令行参数（跳过空值与关闭项）。"""
    args = [cfg.get("url", "")]
    if args[0].strip() == "":
        raise ValueError("目标 URL 不能为空")

    def add_num(flag, val):
        nonlocal args
        if val is not None and str(val).strip() != "":
            try:
                if float(val) > 0:
                    args += [flag, str(val)]
            except (TypeError, ValueError):
                pass

    if cfg.get("concurrency"):
        add_num("-c", cfg["concurrency"])
    add_num("-n", cfg.get("requests"))
    add_num("-t", cfg.get("time"))
    if cfg.get("method") and cfg["method"].strip().upper() != "GET":
        args += ["-X", cfg["method"].strip().upper()]

    for h in cfg.get("headers", []) or []:
        name = (h.get("name") or "").strip()
        value = (h.get("value") or "").strip()
        if name:
            args += ["-H", f"{name}: {value}"]

    if cfg.get("body") and str(cfg["body"]).strip() != "":
        args += ["-d", cfg["body"]]

    add_num("--rate", cfg.get("rate"))
    if cfg.get("warmup"):
        add_num("--warmup", cfg["warmup"])
    if cfg.get("timeout"):
        add_num("--timeout", cfg["timeout"])
    if cfg.get("retries"):
        add_num("--retries", cfg["retries"])
    if cfg.get("dataset") and str(cfg["dataset"]).strip():
        args += ["--dataset", cfg["dataset"]]
        mode = (cfg.get("dataset_mode") or "random").strip()
        if mode in ("random", "sequential"):
            args += ["--dataset-mode", mode]

    if cfg.get("http2"):
        args += ["--http2"]
    if cfg.get("no_tls"):
        args += ["--no-tls"]
    if cfg.get("no_redirect"):
        args += ["--no-redirect"]
    if cfg.get("histogram"):
        args += ["--histogram"]

    if cfg.get("p99_threshold_ms"):
        add_num("--p99-threshold-ms", cfg["p99_threshold_ms"])
    if cfg.get("error_threshold_pct"):
        add_num("--error-threshold-pct", cfg["error_threshold_pct"])

    BenchState.result_json = os.path.join(RESULTS_DIR, "result.json")
    BenchState.result_csv = os.path.join(RESULTS_DIR, "result.csv")
    args += ["--json-file", BenchState.result_json]
    args += ["--csv", BenchState.result_csv]
    return args


def run_benchmark(cfg, exe):
    """启动 rhb 子进程并开启读取线程。"""
    os.makedirs(RESULTS_DIR, exist_ok=True)
    with BenchState.lock:
        BenchState.lines = []
        BenchState.last_done = None
    try:
        args = build_args(cfg)
    except ValueError as e:
        broadcast("fatal", {"message": str(e)})
        return
    workdir = os.path.dirname(os.path.dirname(exe))
    try:
        proc = subprocess.Popen(
            [exe] + args,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
            cwd=workdir,
        )
    except Exception as e:
        broadcast("fatal", {"message": f"启动 rhb 失败：{e}"})
        return

    with BenchState.lock:
        BenchState.proc = proc
        BenchState.active = True

    def reader():
        try:
            for line in proc.stdout:
                line = line.rstrip()
                if line:
                    broadcast("line", line)
        finally:
            rc = proc.wait()
            result = None
            if BenchState.result_json and os.path.exists(BenchState.result_json):
                try:
                    with open(BenchState.result_json, "r", encoding="utf-8") as f:
                        result = json.load(f)
                except Exception:
                    result = None
            with BenchState.lock:
                BenchState.active = False
                BenchState.proc = None
            broadcast("done", {"exit_code": rc, "result": result})

    threading.Thread(target=reader, daemon=True).start()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stdout.write("[rhb-web] %s\n" % (fmt % args))

    def _send_json(self, obj, code=200):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_file(self, path, content_type, as_attachment=False):
        if not os.path.isfile(path):
            self._send_json({"message": "文件不存在"}, 404)
            return
        with open(path, "rb") as f:
            body = f.read()
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        if as_attachment:
            self.send_header("Content-Disposition", "attachment")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        parsed = urlparse(self.path)
        path = parsed.path

        if path == "/" or path == "/index.html":
            idx = os.path.join(BASE_DIR, "index.html")
            self._send_file(idx, "text/html; charset=utf-8")
        elif path.startswith("/download/"):
            name = unquote(path[len("/download/"):])
            if name == "result.json":
                self._send_file(BenchState.result_json or os.path.join(RESULTS_DIR, "result.json"),
                                "application/json; charset=utf-8", as_attachment=True)
            elif name == "result.csv":
                self._send_file(BenchState.result_csv or os.path.join(RESULTS_DIR, "result.csv"),
                                "text/csv; charset=utf-8", as_attachment=True)
            else:
                self._send_json({"message": "未知文件"}, 404)
        elif path == "/api/events":
            self._handle_sse()
        elif path == "/api/health":
            self._send_json({
                "ok": True,
                "rhb_exe": EXE,
                "running": BenchState.active,
            })
        else:
            self._send_json({"message": "not found"}, 404)

    def _handle_sse(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.end_headers()
        q = queue.Queue(maxsize=200)
        with BenchState.lock:
            BenchState.queues.append(q)
            cached_lines = list(BenchState.lines)
            cached_done = BenchState.last_done

        def write_event(event, data):
            payload = ("event: %s\ndata: %s\n\n"
                       % (event, json.dumps(data, ensure_ascii=False))).encode("utf-8")
            self.wfile.write(payload)
            self.wfile.flush()

        try:
            # 回放缓存：迟到的订阅者也能收到完整输出与最终结果
            for ln in cached_lines:
                write_event("line", ln)
            if cached_done:
                write_event(*cached_done)
                return
            while True:
                try:
                    event, data = q.get(timeout=30)
                except queue.Empty:
                    event, data = "ping", {"t": 1}
                write_event(event, data)
                if event == "done" or event == "fatal":
                    break
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            with BenchState.lock:
                try:
                    BenchState.queues.remove(q)
                except ValueError:
                    pass

    def do_POST(self):
        parsed = urlparse(self.path)
        if parsed.path not in ("/api/run", "/api/stop"):
            self._send_json({"message": "not found"}, 404)
            return
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length).decode("utf-8", errors="replace") if length else "{}"

        if parsed.path == "/api/run":
            if BenchState.active:
                self._send_json({"ok": False, "message": "已有压测在运行，请先停止"}, 409)
                return
            try:
                cfg = json.loads(body)
            except json.JSONDecodeError:
                self._send_json({"ok": False, "message": "配置格式错误"}, 400)
                return
            if not EXE:
                self._send_json({"ok": False, "message":
                                 "未找到 rhb 可执行文件，请先 cargo build --release 或指定 --exe"}, 500)
                return
            run_benchmark(cfg, EXE)
            self._send_json({"ok": True})
        else:
            with BenchState.lock:
                proc = BenchState.proc
            if proc and proc.poll() is None:
                proc.terminate()
                self._send_json({"ok": True, "message": "已发送停止信号"})
            else:
                self._send_json({"ok": True, "message": "当前无运行中的压测"})


def main():
    ap = argparse.ArgumentParser(description="rhb Web UI 本地服务")
    ap.add_argument("--port", type=int, default=8000)
    ap.add_argument("--exe", default=None, help="rhb 可执行文件路径")
    ap.add_argument("--no-browser", action="store_true", help="不自动打开浏览器")
    args = ap.parse_args()

    global EXE
    if args.exe:
        EXE = os.path.abspath(args.exe)
        if not os.path.isfile(EXE):
            print("错误：指定的 rhb 可执行文件不存在：%s" % EXE)
            sys.exit(1)
    if not EXE:
        print("警告：未找到 rhb 可执行文件，请先运行 cargo build --release 或使用 --exe 指定。")

    httpd = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    url = "http://127.0.0.1:%d/" % args.port
    print("rhb Web UI 已启动：%s" % url)
    if EXE:
        print("rhb 可执行文件：%s" % EXE)
    if not args.no_browser:
        threading.Timer(0.6, lambda: webbrowser.open(url)).start()
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\n已停止服务。")


if __name__ == "__main__":
    main()

#!/usr/bin/env bash
# rhb 压测控制台一键启动（Linux / macOS）
set -e
cd "$(dirname "$0")"

if [ -z "$RHB_EXE" ] && [ -f ../target/release/rhb ]; then
    export RHB_EXE="../target/release/rhb"
fi

echo "============================================"
echo "  rhb 压测控制台"
echo "============================================"
exec python3 server.py "$@"

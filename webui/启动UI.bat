@echo off
rem rhb 压测控制台一键启动（Windows）
chcp 65001 >nul
setlocal
cd /d "%~dp0"

set "PY="
where python >nul 2>nul && set "PY=python"
if not defined PY (
    where py >nul 2>nul && set "PY=py"
)
if not defined PY (
    echo [错误] 未找到 Python，请先安装 Python 3.8+ 并加入 PATH。
    pause
    exit /b 1
)

if exist "..\target\release\rhb.exe" set "RHB_EXE=..\target\release\rhb.exe"

echo ============================================
echo   rhb 压测控制台
echo   服务启动后将自动打开浏览器窗口
echo   关闭本窗口或在窗口中按 Ctrl+C 可停止服务
echo ============================================
%PY% server.py %*
pause

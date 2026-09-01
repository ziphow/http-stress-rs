@echo off
rem rhb Web UI launcher (Windows)
chcp 65001 >nul 2>&1
setlocal
cd /d "%~dp0"

set "PY="
where python >nul 2>nul && set "PY=python"
if not defined PY (
    where py >nul 2>nul && set "PY=py"
)
if not defined PY (
    echo [ERROR] Python not found. Please install Python 3.8+ and add it to PATH.
    pause
    exit /b 1
)

if exist "..\target\release\rhb.exe" set "RHB_EXE=..\target\release\rhb.exe"

echo ============================================
echo   rhb Benchmark Control Panel
echo   A browser window will open automatically.
echo   Press Ctrl+C in this window to stop.
echo ============================================
%PY% server.py %*
pause

@echo off
setlocal enabledelayedexpansion

cd /d "%~dp0"

echo [1/3] Building Rust TUI app...
pushd src-tui
cargo build --release
if errorlevel 1 (
    popd
    echo [ERROR] Build failed. Please check the output above.
    exit /b 1
)
popd

set "DEST=%USERPROFILE%\Desktop\LZCR-Build"
set "EXE=src-tui\target\release\lzcr.exe"

echo [2/3] Copying binary to Desktop...
if not exist "%DEST%" mkdir "%DEST%"

if exist "%EXE%" (
    copy /y "%EXE%" "%DEST%\" >nul
) else (
    echo [ERROR] Executable not found: %EXE%
    exit /b 1
)

echo [3/3] Done. Files copied to:
echo %DEST%
dir /b "%DEST%"

exit /b 0

@echo off
REM Build and run the Void & Thunder native client.
REM
REM   run.bat            build + run (debug, lightly optimised)
REM   run.bat fast       build + run with dynamic linking (fastest iterative builds)
REM   run.bat hud        build + run with the native HTML HUD (Ultralight texture)
REM   run.bat release    build + run an optimised release binary
REM   run.bat test       run the simulation tests instead of the game

setlocal
cd /d "%~dp0"

if /i "%1"=="test" (
    cargo test -p vt_sim
    goto :end
)

if /i "%1"=="release" (
    cargo run -p vt_client --release
    goto :end
)

if /i "%1"=="fast" (
    cargo run -p vt_client --features fast-compile
    goto :end
)
if /i "%1"=="dev" (
    cargo run -p vt_client --features dev-panel
    goto :end
)

REM The hud branch uses a label (not an if-block): its PowerShell staging line
REM contains parentheses, which would prematurely close a cmd `if (...)` block.
if /i "%1"=="hud" goto :hud

cargo run -p vt_client
goto :end

:hud
REM Dynamic linking for fast iteration + the Ultralight HTML HUD.
echo [run.bat] building...
cargo build -p vt_client --features "fast-compile native-html-hud"
if errorlevel 1 goto :end
REM Stage the Ultralight SDK: DLLs next to the exe, resources/ in the cwd.
echo [run.bat] staging Ultralight SDK...
powershell -NoProfile -ExecutionPolicy Bypass -Command "$sdk = Get-ChildItem 'target\debug\build' -Directory -Filter 'ul-next-sys-*' -ErrorAction SilentlyContinue | ForEach-Object { Join-Path $_.FullName 'out\ul-sdk' } | Where-Object { Test-Path $_ } | Select-Object -First 1; if ($sdk) { Copy-Item (Join-Path $sdk 'bin\*.dll') 'target\debug\' -Force; if (-not (Test-Path 'resources')) { Copy-Item (Join-Path $sdk 'resources') 'resources' -Recurse -Force } } else { Write-Host 'WARN: Ultralight ul-sdk not found under target\debug\build' }"
echo [run.bat] launching...
cargo run -p vt_client --features "fast-compile native-html-hud"
goto :end

:end
if errorlevel 1 (
    echo.
    echo Build or run failed with error %errorlevel%.
    pause
)
endlocal

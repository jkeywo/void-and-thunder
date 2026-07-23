@echo off
REM Build and run the Void & Thunder native client.
REM
REM   run.bat            build + run (debug, lightly optimised)
REM   run.bat fast       build + run with dynamic linking (fastest iterative builds)
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

cargo run -p vt_client

:end
if errorlevel 1 (
    echo.
    echo Build or run failed with error %errorlevel%.
    pause
)
endlocal

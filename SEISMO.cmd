@echo off
rem Double-click to start SEISMO. Builds the desktop app first if the code has changed
rem (the first build takes a few minutes; later starts are quick), then opens it.
rem Any arguments are passed to the app, e.g.  SEISMO.cmd --look modern
setlocal
cd /d "%~dp0"
set "EXE=target\release\seismo.exe"

where cargo >nul 2>nul
if errorlevel 1 goto nocargo

echo Building SEISMO (the first build takes a few minutes)...
cargo build -p app --release
if errorlevel 1 (
    echo.
    echo The build failed; the error is above.
    pause
    exit /b 1
)
goto launch

:nocargo
if exist "%EXE%" goto launch
echo SEISMO has not been built yet, and building it needs Rust.
echo Install Rust from https://rustup.rs and double-click this file again.
pause
exit /b 1

:launch
start "" "%EXE%" %*
exit /b 0

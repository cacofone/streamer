@echo off
rem Test one crate, and smoke-test its binary.
rem Usage: test.bat <crate> <binary or -> [--installed] [--live] [--prefix DIR]
setlocal EnableExtensions

for %%I in ("%~dp0..") do set "ROOT=%%~fI"

if /i "%~1"=="-h" goto usage_ok
if /i "%~1"=="--help" goto usage_ok
if "%~2"=="" goto usage_error
set "CRATE=%~1"
set "BINARY=%~2"
shift /1
shift /1

set "INSTALLED=0"
set "LIVE=0"
set "PREFIX_DIR="

:parse
if "%~1"=="" goto parsed
if /i "%~1"=="--installed" goto opt_installed
if /i "%~1"=="--live" goto opt_live
if /i "%~1"=="--prefix" goto opt_prefix
if /i "%~1"=="-h" goto usage_ok
if /i "%~1"=="--help" goto usage_ok
echo error: unknown option: %~1 1>&2
exit /b 2

:opt_installed
set "INSTALLED=1"
shift /1
goto parse

:opt_live
set "LIVE=1"
shift /1
goto parse

:opt_prefix
if "%~2"=="" goto prefix_missing
set "PREFIX_DIR=%~2"
shift /1
shift /1
goto parse

:prefix_missing
echo error: --prefix needs a value 1>&2
exit /b 2

:parsed
if "%LIVE%"=="0" goto live_checked
if /i "%BINARY%"=="streamer" goto live_checked
echo error: --live is only available for the streamer binary 1>&2
exit /b 2

:live_checked
set "MANIFEST=%ROOT%\Cargo.toml"
set "TARGET_DIR=%ROOT%\target"
if defined CARGO_TARGET_DIR set "TARGET_DIR=%CARGO_TARGET_DIR%"
set "PASSES=0"
set "FAILS=0"

echo.
echo == cargo test
cargo test --manifest-path "%MANIFEST%" -p %CRATE% --locked --no-fail-fast
if errorlevel 1 (set /a FAILS+=1) else (set /a PASSES+=1)

echo.
echo == rustfmt
cargo fmt --manifest-path "%MANIFEST%" -p %CRATE% -- --check
if errorlevel 1 (set /a FAILS+=1) else (set /a PASSES+=1)

echo.
echo == clippy
cargo clippy --manifest-path "%MANIFEST%" -p %CRATE% --all-targets --locked -- -D warnings
if errorlevel 1 (set /a FAILS+=1) else (set /a PASSES+=1)

if "%BINARY%"=="-" goto summary

set "PKGID="
for /f "delims=" %%P in ('cargo pkgid --manifest-path "%MANIFEST%" -p %CRATE%') do set "PKGID=%%P"
set "AFTER_HASH=%PKGID:*#=%"
set "EXPECTED=%AFTER_HASH:*@=%"

set "BINPATH="
if "%INSTALLED%"=="1" goto use_installed
echo.
echo == build
cargo build --manifest-path "%MANIFEST%" -p %CRATE% --locked
if errorlevel 1 (set /a FAILS+=1) else (set /a PASSES+=1)
set "BINPATH=%TARGET_DIR%\debug\%BINARY%.exe"
goto have_binary

:use_installed
if defined PREFIX_DIR goto installed_prefix
for %%B in ("%BINARY%.exe") do set "BINPATH=%%~$PATH:B"
goto check_installed

:installed_prefix
set "BINPATH=%PREFIX_DIR%\bin\%BINARY%.exe"

:check_installed
if not defined BINPATH goto not_installed
if exist "%BINPATH%" goto have_binary

:not_installed
echo error: installed binary %BINARY% not found, install it first or pass --prefix 1>&2
exit /b 1

:have_binary
echo.
echo testing binary: %BINPATH% (expected version %EXPECTED%)

echo.
echo == smoke: --version
set "VERSION_OUT=%TEMP%\cacofone-version-%RANDOM%.txt"
"%BINPATH%" --version > "%VERSION_OUT%"
if errorlevel 1 goto version_failed
type "%VERSION_OUT%"
findstr /C:"%EXPECTED%" "%VERSION_OUT%" >nul
if errorlevel 1 goto version_failed
set /a PASSES+=1
echo PASS: smoke --version
goto after_version

:version_failed
set /a FAILS+=1
echo FAIL: smoke --version

:after_version
del /Q "%VERSION_OUT%" 2>nul

echo.
echo == smoke: --help
"%BINPATH%" --help >nul
if errorlevel 1 goto help_failed
set /a PASSES+=1
echo PASS: smoke --help
goto discover_smoke

:help_failed
set /a FAILS+=1
echo FAIL: smoke --help

:discover_smoke
if /i not "%BINARY%"=="streamer" goto summary

echo.
echo == smoke: discover --help
"%BINPATH%" discover --help >nul
if errorlevel 1 goto dhelp_failed
set /a PASSES+=1
echo PASS: smoke discover --help
goto discover_json

:dhelp_failed
set /a FAILS+=1
echo FAIL: smoke discover --help

:discover_json
echo.
echo == smoke: discover --timeout 2 --json
rem Must work with no devices and no multicast: it just finds nothing.
rem stdout must hold nothing but JSON lines (each line starts with { and ends with }).
set "JSON_OUT=%TEMP%\cacofone-discover-%RANDOM%.txt"
"%BINPATH%" discover --timeout 2 --json > "%JSON_OUT%" 2>nul
if errorlevel 1 goto djson_failed
findstr /v /r "^{.*}$" "%JSON_OUT%" >nul
if not errorlevel 1 goto djson_failed
del /Q "%JSON_OUT%" 2>nul
set /a PASSES+=1
echo PASS: smoke discover --json
goto live

:djson_failed
del /Q "%JSON_OUT%" 2>nul
set /a FAILS+=1
echo FAIL: smoke discover --json

:live
if not "%LIVE%"=="1" goto summary
echo.
echo == live: discover on the real network
"%BINPATH%" discover --timeout 5
echo.
set "ANSWER="
set /p "ANSWER=Are the devices you expected listed above? [y/N] "
if /i "%ANSWER:~0,1%"=="y" goto live_ok
set /a FAILS+=1
echo FAIL: live discover
goto summary

:live_ok
set /a PASSES+=1
echo PASS: live discover

:summary
echo.
echo summary: %PASSES% passed, %FAILS% failed
if "%FAILS%"=="0" exit /b 0
exit /b 1

:usage_ok
call :usage
exit /b 0

:usage_error
call :usage 1>&2
exit /b 2

:usage
echo Usage: test.bat ^<crate^> ^<binary or -^> [options]
echo.
echo Runs the crate's tests, formatting and lint checks. For a crate with a
echo binary it also checks that the binary runs. Use - for a library crate.
echo.
echo Options:
echo   --installed    smoke-test the installed binary instead of a fresh build
echo   --prefix DIR   where to look for the installed binary (default: search PATH)
echo   --live         also run a real search on your network and ask you to confirm
echo                  the result (only for the streamer binary, never run in CI)
echo   -h, --help     show this help
exit /b 0

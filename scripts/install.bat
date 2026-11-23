@echo off
rem Build one crate's binary and install it, without cargo install.
rem Usage: install.bat <crate> <binary> [--prefix DIR] [--debug] [--dry-run] [--force]
setlocal EnableExtensions

for %%I in ("%~dp0..") do set "ROOT=%%~fI"

if /i "%~1"=="-h" goto usage_ok
if /i "%~1"=="--help" goto usage_ok
if "%~2"=="" goto usage_error
set "CRATE=%~1"
set "BINARY=%~2"
shift /1
shift /1

set "PREFIX_DIR=%PREFIX%"
if not defined PREFIX_DIR set "PREFIX_DIR=%LOCALAPPDATA%\Programs\cacofone-streamer"
set "PROFILE=release"
set "DRYRUN=0"
set "FORCE=0"

:parse
if "%~1"=="" goto parsed
if /i "%~1"=="--prefix" goto opt_prefix
if /i "%~1"=="--debug" goto opt_debug
if /i "%~1"=="--dry-run" goto opt_dry
if /i "%~1"=="--force" goto opt_force
if /i "%~1"=="-h" goto usage_ok
if /i "%~1"=="--help" goto usage_ok
echo error: unknown option: %~1 1>&2
exit /b 2

:opt_prefix
if "%~2"=="" goto prefix_missing
set "PREFIX_DIR=%~2"
shift /1
shift /1
goto parse

:prefix_missing
echo error: --prefix needs a value 1>&2
exit /b 2

:opt_debug
set "PROFILE=debug"
shift /1
goto parse

:opt_dry
set "DRYRUN=1"
shift /1
goto parse

:opt_force
set "FORCE=1"
shift /1
goto parse

:parsed
set "TARGET_DIR=%ROOT%\target"
if defined CARGO_TARGET_DIR set "TARGET_DIR=%CARGO_TARGET_DIR%"
set "SRC=%TARGET_DIR%\%PROFILE%\%BINARY%.exe"
set "DEST=%PREFIX_DIR%\bin\%BINARY%.exe"
set "SHARE=%PREFIX_DIR%\share\cacofone-streamer"
set "MARKER=%SHARE%\%BINARY%.installed"

if not exist "%DEST%" goto not_existing
if exist "%MARKER%" goto not_existing
if "%FORCE%"=="1" goto not_existing
echo error: %DEST% exists and was not installed by these scripts, use --force to overwrite 1>&2
exit /b 1

:not_existing
if not "%DRYRUN%"=="1" goto do_install
echo dry run, nothing will be changed
echo would build:   cargo build --locked -p %CRATE% (%PROFILE%)
echo would install: %SRC% to %DEST%
echo would write:   %MARKER%
exit /b 0

:do_install
echo building %CRATE% (%PROFILE%)
if "%PROFILE%"=="release" goto build_release
cargo build --manifest-path "%ROOT%\Cargo.toml" --locked -p %CRATE%
goto built

:build_release
cargo build --manifest-path "%ROOT%\Cargo.toml" --locked --release -p %CRATE%

:built
if errorlevel 1 exit /b 1
if not exist "%SRC%" goto no_output
if not exist "%PREFIX_DIR%\bin" mkdir "%PREFIX_DIR%\bin"
if not exist "%SHARE%" mkdir "%SHARE%"
copy /Y "%SRC%" "%DEST%" >nul
if errorlevel 1 exit /b 1

set "COMMIT=unknown"
for /f %%H in ('git -C "%ROOT%" rev-parse --short HEAD 2^>nul') do set "COMMIT=%%H"
> "%MARKER%" echo crate=%CRATE%
>> "%MARKER%" echo binary=%BINARY%
>> "%MARKER%" echo commit=%COMMIT%
echo installed %DEST%

"%DEST%" --version
if errorlevel 1 goto not_running

echo ;%PATH%; | find /i ";%PREFIX_DIR%\bin;" >nul
if errorlevel 1 echo note: %PREFIX_DIR%\bin is not on your PATH, add it to your user PATH in the Environment Variables settings
exit /b 0

:no_output
echo error: the build did not produce %SRC% 1>&2
exit /b 1

:not_running
echo error: the installed binary does not run 1>&2
exit /b 1

:usage_ok
call :usage
exit /b 0

:usage_error
call :usage 1>&2
exit /b 2

:usage
echo Usage: install.bat ^<crate^> ^<binary^> [options]
echo.
echo Builds the crate with cargo and copies the binary to PREFIX\bin.
echo.
echo Options:
echo   --prefix DIR   install under DIR (default: the PREFIX variable, or
echo                  %%LOCALAPPDATA%%\Programs\cacofone-streamer)
echo   --debug        install the debug build instead of the release build
echo   --dry-run      print what would happen and change nothing
echo   --force        overwrite a binary that was not installed by these scripts
echo   -h, --help     show this help
exit /b 0

@echo off
rem Remove a binary that was installed with install.bat.
rem Usage: uninstall.bat <binary> [--prefix DIR] [--dry-run]
setlocal EnableExtensions

if /i "%~1"=="-h" goto usage_ok
if /i "%~1"=="--help" goto usage_ok
if "%~1"=="" goto usage_error
set "BINARY=%~1"
shift /1

set "PREFIX_DIR=%PREFIX%"
if not defined PREFIX_DIR set "PREFIX_DIR=%LOCALAPPDATA%\Programs\cacofone-streamer"
set "DRYRUN=0"

:parse
if "%~1"=="" goto parsed
if /i "%~1"=="--prefix" goto opt_prefix
if /i "%~1"=="--dry-run" goto opt_dry
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

:opt_dry
set "DRYRUN=1"
shift /1
goto parse

:parsed
set "DEST=%PREFIX_DIR%\bin\%BINARY%.exe"
set "SHARE=%PREFIX_DIR%\share\cacofone-streamer"
set "MARKER=%SHARE%\%BINARY%.installed"

if exist "%MARKER%" goto found
echo nothing to uninstall: %BINARY% was not installed by these scripts under %PREFIX_DIR%
exit /b 0

:found
if not "%DRYRUN%"=="1" goto remove
echo dry run, nothing will be changed
echo would remove: %DEST%
echo would remove: %MARKER%
exit /b 0

:remove
del /Q "%DEST%" 2>nul
del /Q "%MARKER%"
rmdir "%SHARE%" 2>nul
echo removed %DEST%
exit /b 0

:usage_ok
call :usage
exit /b 0

:usage_error
call :usage 1>&2
exit /b 2

:usage
echo Usage: uninstall.bat ^<binary^> [options]
echo.
echo Removes PREFIX\bin\^<binary^>.exe, but only if it was installed by install.bat.
echo.
echo Options:
echo   --prefix DIR   the prefix used for the install (default: the PREFIX variable, or
echo                  %%LOCALAPPDATA%%\Programs\cacofone-streamer)
echo   --dry-run      print what would happen and change nothing
echo   -h, --help     show this help
exit /b 0

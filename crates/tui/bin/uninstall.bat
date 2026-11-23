@echo off
rem Wrapper: see scripts\uninstall.bat for the options.
call "%~dp0..\..\..\scripts\uninstall.bat" streamer-tui %*
exit /b %errorlevel%

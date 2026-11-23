@echo off
rem Wrapper: see scripts\install.bat for the options.
call "%~dp0..\..\..\scripts\install.bat" cacofone-streamer-tui streamer-tui %*
exit /b %errorlevel%

@echo off
rem Wrapper: see scripts\test.bat for the options.
call "%~dp0..\..\..\scripts\test.bat" cacofone-streamer-tui streamer-tui %*
exit /b %errorlevel%

@echo off
rem Wrapper: see scripts\install.bat for the options.
call "%~dp0..\..\..\scripts\install.bat" cacofone-streamer-cli streamer %*
exit /b %errorlevel%

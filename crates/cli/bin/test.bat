@echo off
rem Wrapper: see scripts\test.bat for the options.
call "%~dp0..\..\..\scripts\test.bat" cacofone-streamer-cli streamer %*
exit /b %errorlevel%

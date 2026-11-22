@echo off
rem Builds the Reclaim installer and portable zip into dist\. See installer\README.md.
rem Arguments are passed on, e.g. "scripts\build-installer.cmd -Run".
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-installer.ps1" %*
exit /b %ERRORLEVEL%

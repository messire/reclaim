@echo off
rem Builds the Reclaim installer and portable zip into dist\ (in the repository folder).
rem Run it from anywhere, or double-click it in Explorer. See installer\README.md.
rem Arguments are passed on, e.g. "scripts\build-installer.cmd -Run".
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-installer.ps1" %*
set RC=%ERRORLEVEL%
rem Started by a double-click: keep the window open so the result can be read.
echo %CMDCMDLINE% | find /i "/c" >nul && pause
exit /b %RC%

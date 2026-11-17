@echo off
rem Start muis: keeps the frontend bundle current, then serves it in a browser.
rem Double-click this file, or run `muis` from a terminal in this folder.
setlocal
node "%~dp0node-host\launch.mjs" %*
if errorlevel 1 pause

@echo off
rem ============================================================
rem  Craft AI MCP Bridge - launcher
rem  ------------------------------------------------------------
rem  Starts the Craft AI MCP bridge (craft-ai-mcp.exe) if it is
rem  not already running on port 12345. Safe to run repeatedly:
rem  it is a no-op when the bridge is already up, so it can be
rem  double-clicked by hand OR fired by the logon scheduled task
rem  without creating duplicate instances (a second instance would
rem  fail to bind port 12345 and exit on its own).
rem
rem  The binary path is resolved relative to this script (%~dp0),
rem  so the launcher works from any checkout location.
rem
rem  The bridge listens on http://127.0.0.1:12345/mcp, which is the
rem  URL Bionic's ng-mcp.json "Craft AI" entry points at. Restarting
rem  Bionic after this is running makes the Craft AI MCP appear and
rem  connect.
rem ============================================================
setlocal

set "BIN=%~dp0target\debug\craft-ai-mcp.exe"

if not exist "%BIN%" (
    echo ERROR: bridge binary not found at %BIN%
    exit /b 1
)

rem If something is already listening on port 12345, the bridge is up.
netstat -ano | findstr ":12345" >nul 2>nul
if not errorlevel 1 (
    echo Craft AI MCP bridge already running on port 12345.
    exit /b 0
)

start "" /min "%BIN%"
echo Started Craft AI MCP bridge: %BIN%
exit /b 0

@echo off
setlocal
cd /d "%~dp0"

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"
set "PATH=%ROOT%\bin;%ROOT%\tools\libimobiledevice;%PATH%"

set "IOSCPY=%ROOT%\bin\ioscpy.exe"
if not exist "%IOSCPY%" set "IOSCPY=%ROOT%\ioscpy\host\target\release\ioscpy.exe"

if not exist "%IOSCPY%" (
  echo ioscpy.exe is missing - Defender may have removed it. Building...
  call "%ROOT%\build-host.bat"
  set "IOSCPY=%ROOT%\bin\ioscpy.exe"
)

if not exist "%IOSCPY%" set "IOSCPY=%ROOT%\ioscpy\host\target\release\ioscpy.exe"

if not exist "%IOSCPY%" (
  echo ioscpy.exe is still missing.
  echo Allow the folder in Windows Security exclusions, then run build-host.bat.
  pause
  exit /b 1
)

REM Keep the console for diagnostics. Everything else is the GUI window.
if /i "%~1"=="--list" goto :console
if /i "%~1"=="--debug" goto :console
if /i "%~1"=="--handshake-only" goto :console
if /i "%~1"=="--snapshot" goto :console
if /i "%~1"=="--bench" goto :console
if /i "%~1"=="--action" goto :console
if /i "%~1"=="--soak" goto :console

start "" "%IOSCPY%" %*
exit /b 0

:console
"%IOSCPY%" %*
endlocal

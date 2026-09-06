@echo off
setlocal
cd /d "%~dp0"

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"
set "PATH=%ROOT%\bin;%ROOT%\tools\libimobiledevice;%PATH%"

set "EXE=%ROOT%\ioscpy\host\target\release\ioscpy-auto.exe"
if not exist "%EXE%" set "EXE=%ROOT%\bin\ioscpy-auto.exe"

if not exist "%EXE%" (
  echo ioscpy-auto.exe is missing - building...
  call "%ROOT%\build-auto.bat"
  set "EXE=%ROOT%\bin\ioscpy-auto.exe"
)

if not exist "%EXE%" (
  echo ioscpy-auto.exe is still missing.
  pause
  exit /b 1
)

start "" "%EXE%" %*
endlocal

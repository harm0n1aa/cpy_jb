@echo off
setlocal EnableDelayedExpansion
set "PATH=S:\programms\cpy_jb\tools\libimobiledevice;%PATH%"
set "IOSCPY=S:\programms\cpy_jb\ioscpy\host\target\release\ioscpy.exe"

if /i "%~1"=="--list" (
  "%IOSCPY%" %*
  exit /b %ERRORLEVEL%
)

echo %*| findstr /i /c:"--device" >nul
if not errorlevel 1 (
  "%IOSCPY%" %*
  exit /b %ERRORLEVEL%
)

set "UDIDS="
set COUNT=0
for /f "usebackq tokens=1" %%U in (`idevice_id -l 2^>nul`) do (
  set /a COUNT+=1
  set "UDIDS=!UDIDS! %%U"
)

if !COUNT! EQU 0 (
  echo No iPhone found over USB. Plug in a jailbroken iPhone, unlock it, and tap Trust if asked.
  pause
  exit /b 1
)

if !COUNT! EQU 1 (
  echo Connecting to!UDIDS!
  "%IOSCPY%" --device!UDIDS! %*
  exit /b %ERRORLEVEL%
)

echo Found !COUNT! iPhones. Opening a window for each...
for %%U in (!UDIDS!) do (
  echo   %%U
  start "ioscpy %%U" "%IOSCPY%" --device %%U %*
)
endlocal

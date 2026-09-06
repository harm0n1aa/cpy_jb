@echo off
setlocal
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set "PATH=%~dp0tools\nasm\nasm-3.02;%USERPROFILE%\.cargo\bin;%PATH%"
cd /d "%~dp0"
echo Building ioscpy.exe ...
cargo build --release --manifest-path ioscpy\host\Cargo.toml
if errorlevel 1 exit /b 1

if not exist "%~dp0bin" mkdir "%~dp0bin"
copy /Y "ioscpy\host\target\release\ioscpy.exe" "%~dp0bin\ioscpy.exe"
if errorlevel 1 (
  echo Failed to copy ioscpy.exe - close the app and run build-host.bat again.
  exit /b 1
)
copy /Y "%~dp0tools\libimobiledevice\iproxy.exe" "%~dp0bin\" >nul
copy /Y "%~dp0tools\libimobiledevice\idevice_id.exe" "%~dp0bin\" >nul
copy /Y "%~dp0tools\libimobiledevice\ideviceinfo.exe" "%~dp0bin\" >nul
copy /Y "%~dp0tools\libimobiledevice\*.dll" "%~dp0bin\" >nul
echo Copied to %~dp0bin\ioscpy.exe
endlocal

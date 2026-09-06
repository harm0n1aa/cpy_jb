@echo off
setlocal
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set "PATH=%~dp0tools\nasm\nasm-3.02;%USERPROFILE%\.cargo\bin;%PATH%"
cd /d "%~dp0"
echo Building ioscpy-auto.exe (leaves ioscpy.exe alone) ...
cargo build --release --manifest-path ioscpy\host\Cargo.toml --bin ioscpy-auto
if errorlevel 1 exit /b 1

if not exist "%~dp0bin" mkdir "%~dp0bin"
copy /Y "ioscpy\host\target\release\ioscpy-auto.exe" "%~dp0bin\ioscpy-auto.exe"
if errorlevel 1 (
  echo Failed to copy ioscpy-auto.exe - close it and run build-auto.bat again.
  exit /b 1
)
echo Copied to %~dp0bin\ioscpy-auto.exe
endlocal

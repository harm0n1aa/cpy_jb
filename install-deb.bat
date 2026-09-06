@echo off
cd /d "%~dp0"
echo Keep this window open.
echo In Sileo DELETE https://192.168.1.8:8080
echo Then add EXACTLY:  http://192.168.1.8:8081
echo Turn off HTTPS if Sileo asks. Use 192.168 not 26.x
echo.
py -3 "%~dp0serve-deb.py"
pause

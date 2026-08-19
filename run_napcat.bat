@echo off
set NODE=C:\nvm4w\nodejs\node.exe
start "NapCatBackend" "%NODE%" "H:\NapCat\start_napcat2.mjs" > "C:\Users\28643\AppData\Local\Temp\opencode\napcat_run.log" 2>&1
echo Started, log at C:\Users\28643\AppData\Local\Temp\opencode\napcat_run.log
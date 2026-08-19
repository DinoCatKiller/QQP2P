@echo off
chcp 65001 >nul
echo ========================================
echo    QQP2P - QQ P2P连接机器人
echo ========================================
echo.
echo [1] 启动P2P机器人（自动监听QQ消息）
echo [2] 查询本机IP
echo [3] 查看连接状态
echo [4] 检查机器人在线状态
echo [5] 查看好友列表
echo [6] 查看群列表
echo [0] 退出
echo.
set /p choice=请输入选项: 

if "%choice%"=="1" goto start
if "%choice%"=="2" goto ip
if "%choice%"=="3" goto status
if "%choice%"=="4" goto online
if "%choice%"=="5" goto friends
if "%choice%"=="6" goto groups
if "%choice%"=="0" goto end
goto menu

:start
echo.
set /p user_id=请输入你的QQ用户ID: 
set /p port=请输入TCP端口 (默认8080): 
if "%port%"=="" set port=8080
echo.
echo [*] 启动P2P机器人...
echo [*] 请在QQ中@机器人发送以下命令:
echo     /ip          - 获取本机IP
echo     /connect IP  - 连接到对方
echo     /status      - 查看状态
echo     /help        - 帮助
echo.
cargo run -- start --user-id %user_id% --port %port%
pause
goto menu

:ip
echo.
set /p user_id=请输入用户ID (默认12345): 
if "%user_id%"=="" set user_id=12345
cargo run -- ip --user-id %user_id%
pause
goto menu

:status
echo.
set /p user_id=请输入用户ID (默认12345): 
if "%user_id%"=="" set user_id=12345
cargo run -- status --user-id %user_id%
pause
goto menu

:online
echo.
cargo run -- online
pause
goto menu

:friends
echo.
cargo run -- friends
pause
goto menu

:groups
echo.
cargo run -- groups
pause
goto menu

:menu
cls
goto menu

:end
exit

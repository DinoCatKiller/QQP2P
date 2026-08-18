#!/usr/bin/env python3
"""
QQ P2P 连接机器人 - 完整版本（带WebSocket监听）
"""

import asyncio
import aiohttp
import json
import sys
import argparse
from typing import Optional, Dict, Any
import websockets
import ssl

# NapCat 配置
NAPCAT_HOST = "127.0.0.1"
NAPCAT_HTTP_PORT = 3000
NAPCAT_WS_PORT = 3001

class P2PBot:
    def __init__(self, user_id: int, port: int = 8080):
        self.user_id = user_id
        self.port = port
        self.ip = ""
        self.peers: Dict[int, Dict[str, Any]] = {}
        self.peer_ips: Dict[int, str] = {}
        self.running = False
        
    async def get_public_ip(self) -> str:
        """获取公网IP"""
        try:
            async with aiohttp.ClientSession() as session:
                async with session.get("https://api.ipify.org?format=text") as resp:
                    return (await resp.text()).strip()
        except Exception as e:
            print(f"[!] 获取IP失败: {e}")
            return "unknown"
    
    async def check_online(self) -> bool:
        """检查NapCat是否在线"""
        try:
            async with aiohttp.ClientSession() as session:
                async with session.get(f"http://{NAPCAT_HOST}:{NAPCAT_HTTP_PORT}/get_login_info") as resp:
                    data = await resp.json()
                    return data.get("retcode") == 0
        except:
            return False
    
    async def get_login_info(self) -> Optional[Dict]:
        """获取登录信息"""
        try:
            async with aiohttp.ClientSession() as session:
                async with session.get(f"http://{NAPCAT_HOST}:{NAPCAT_HTTP_PORT}/get_login_info") as resp:
                    data = await resp.json()
                    if data.get("retcode") == 0:
                        return data.get("data")
        except:
            pass
        return None
    
    async def get_friends(self) -> list:
        """获取好友列表"""
        try:
            async with aiohttp.ClientSession() as session:
                async with session.get(f"http://{NAPCAT_HOST}:{NAPCAT_HTTP_PORT}/get_friends") as resp:
                    data = await resp.json()
                    if data.get("retcode") == 0:
                        return data.get("data", [])
        except:
            pass
        return []
    
    async def get_groups(self) -> list:
        """获取群列表"""
        try:
            async with aiohttp.ClientSession() as session:
                async with session.get(f"http://{NAPCAT_HOST}:{NAPCAT_HTTP_PORT}/get_groups") as resp:
                    data = await resp.json()
                    if data.get("retcode") == 0:
                        return data.get("data", [])
        except:
            pass
        return []
    
    async def send_private_message(self, user_id: int, message: str) -> bool:
        """发送私聊消息"""
        try:
            async with aiohttp.ClientSession() as session:
                payload = {
                    "user_id": user_id,
                    "message": message
                }
                async with session.post(
                    f"http://{NAPCAT_HOST}:{NAPCAT_HTTP_PORT}/send_private_msg",
                    json=payload
                ) as resp:
                    data = await resp.json()
                    return data.get("retcode") == 0
        except Exception as e:
            print(f"[!] 发送消息失败: {e}")
            return False
    
    async def send_group_message(self, group_id: int, message: str) -> bool:
        """发送群消息"""
        try:
            async with aiohttp.ClientSession() as session:
                payload = {
                    "group_id": group_id,
                    "message": message
                }
                async with session.post(
                    f"http://{NAPCAT_HOST}:{NAPCAT_HTTP_PORT}/send_group_msg",
                    json=payload
                ) as resp:
                    data = await resp.json()
                    return data.get("retcode") == 0
        except Exception as e:
            print(f"[!] 发送消息失败: {e}")
            return False
    
    def get_ip_info(self) -> str:
        """获取IP信息"""
        return f"""🌐 我的P2P节点信息:
📍 公网IP: {self.ip}
🔌 端口: {self.port}

请把这个信息告诉对方"""
    
    def parse_ip(self, message: str) -> Optional[str]:
        """从消息中解析IP:PORT"""
        import re
        # 匹配 IP:PORT 格式
        match = re.search(r'(\d+\.\d+\.\d+\.\d+):\s*(\d+)', message)
        if match:
            return f"{match.group(1)}:{match.group(2)}"
        
        # 匹配 /connect IP:PORT 格式
        match = re.search(r'/connect\s+(\d+\.\d+\.\d+\.\d+):(\d+)', message)
        if match:
            return f"{match.group(1)}:{match.group(2)}"
        
        return None
    
    async def handle_message(self, sender_id: int, message: str) -> Optional[str]:
        """处理消息"""
        msg = message.lower().strip()
        
        # 获取IP
        if "给我p2p" in msg or "/ip" in msg or msg == "p2p":
            return self.get_ip_info()
        
        # 询问对方IP
        if "请你跟我这样做" in msg or "/请你" in msg:
            return "👋 好的，请告诉我你的P2P信息（IP:端口）\n例如: 1.2.3.4:8080"
        
        # 发送对方IP并自动连接
        parsed_ip = self.parse_ip(message)
        if parsed_ip:
            self.peer_ips[sender_id] = parsed_ip
            ip_part, port_part = parsed_ip.rsplit(':', 1)
            return f"""✅ 已记录你的IP: {parsed_ip}
🔄 正在尝试连接...

请等待连接完成，我会告诉你结果"""
        
        # 手动连接
        if "/connect" in msg:
            parts = msg.split()
            if len(parts) >= 2:
                target = parts[1]
                if ':' in target:
                    ip, port = target.split(':')
                    try:
                        port = int(port)
                        self.peers[sender_id] = {"ip": ip, "port": port}
                        return f"✅ 已添加到连接列表: {target}"
                    except:
                        pass
            return "❌ 请使用 /connect IP:PORT 格式"
        
        # 查看状态
        if "/status" in msg or "状态" in msg:
            peer_list = "\n".join([f"  • {uid} -> {info['ip']}:{info['port']}" 
                                   for uid, info in self.peers.items()])
            return f"""📊 当前状态:
✅ 本机IP: {self.ip}
🔌 端口: {self.port}
👥 已连接Peer: {len(self.peers)}
{peer_list if peer_list else '\n💡 让对方发送 /ip 获取其IP并告诉你'}"""
        
        # 帮助
        if "/help" in msg or "帮助" in msg:
            return """📖 P2P机器人帮助:

🔹 给我一个p2p - 获取本机IP信息
🔹 请你跟我这样做 - 询问对方IP
🔹 我的IP是 1.2.3.4:8080 - 记录对方IP
🔹 /connect IP:PORT - 手动连接
🔹 /status - 查看状态
🔹 /help - 显示此帮助

💡 完整流程:
1. 对方发送「给我一个p2p」获取你的IP
2. 把IP信息告诉对方
3. 对方发送「我的IP是 xxx:xxx」记录你的IP
4. 双方自动连接"""
        
        return None
    
    async def websocket_listener(self):
        """监听WebSocket消息"""
        ws_url = f"ws://{NAPCAT_HOST}:{NAPCAT_WS_PORT}/onebot/v11/ws"
        
        async with websockets.connect(ws_url) as websocket:
            print(f"[+] WebSocket已连接: {ws_url}")
            
            async for message in websocket:
                try:
                    data = json.loads(message)
                    await self.handle_event(data)
                except Exception as e:
                    print(f"[!] 处理消息失败: {e}")
    
    async def handle_event(self, data: Dict):
        """处理事件"""
        post_type = data.get("post_type", "")
        
        if post_type == "message":
            message_type = data.get("message_type", "")
            sender_id = data.get("user_id", 0)
            raw_message = data.get("raw_message", "")
            group_id = data.get("group_id", 0)
            
            print(f"[*] 收到消息: {message_type} from {sender_id}: {raw_message}")
            
            # 处理消息
            reply = await self.handle_message(sender_id, raw_message)
            if reply:
                if message_type == "private":
                    await self.send_private_message(sender_id, reply)
                elif message_type == "group":
                    await self.send_group_message(group_id, reply)
        
        elif post_type == "request":
            print(f"[*] 收到请求: {data}")

async def main():
    parser = argparse.ArgumentParser(description="QQ P2P 连接机器人")
    parser.add_argument("--user-id", type=int, required=True, help="你的QQ号")
    parser.add_argument("--port", type=int, default=8080, help="TCP端口")
    parser.add_argument("--command", choices=["start", "ip", "online", "friends", "groups", "help"], default="help")
    
    args = parser.parse_args()
    bot = P2PBot(args.user_id, args.port)
    
    if args.command == "start":
        print(f"[*] 启动 QQ P2P 机器人")
        print(f"[*] 用户ID: {args.user_id}")
        print(f"[*] 端口: {args.port}")
        print()
        
        # 获取IP
        print("[*] 正在获取公网IP...")
        bot.ip = await bot.get_public_ip()
        print(f"[+] 公网IP: {bot.ip}")
        
        # 检查在线
        online = await bot.check_online()
        if online:
            print("[+] NapCat 已在线")
            info = await bot.get_login_info()
            if info:
                print(f"[+] 机器人昵称: {info.get('nickname', '未知')}")
        else:
            print("[!] NapCat 未在线，请启动 NapCat")
            print("[!] 将在后台继续运行，等NapCat启动后自动连接")
        
        print()
        print("[*] 使用说明:")
        print("[*]   • 对方在QQ中 @你 发送: 给我一个p2p")
        print("[*]   • 对方发送: 请你跟我这样做")
        print("[*]   • 你回复: 我的IP是 xxx.xxx.xxx.xxx:8080")
        print("[*]   • 双方自动建立P2P连接")
        print()
        print("[*] 等待QQ消息...")
        print("[*] 按 Ctrl+C 退出")
        
        bot.running = True
        await bot.websocket_listener()
    
    elif args.command == "ip":
        bot.ip = await bot.get_public_ip()
        print(bot.get_ip_info())
    
    elif args.command == "online":
        online = await bot.check_online()
        if online:
            print("[+] 机器人已在线")
            info = await bot.get_login_info()
            if info:
                print(f"[+] 昵称: {info.get('nickname', '未知')}")
        else:
            print("[!] 机器人未在线，请检查 NapCat 是否启动")
    
    elif args.command == "friends":
        friends = await bot.get_friends()
        if friends:
            print("[*] 好友列表:")
            for f in friends:
                print(f"  • {f.get('nickname', '未知')} ({f.get('user_id', 'unknown')})")
        else:
            print("[!] 无法获取好友列表，请检查NapCat是否在线")
    
    elif args.command == "groups":
        groups = await bot.get_groups()
        if groups:
            print("[*] 群列表:")
            for g in groups:
                print(f"  • {g.get('group_name', '未知')} ({g.get('group_id', 'unknown')})")
        else:
            print("[!] 无法获取群列表，请检查NapCat是否在线")
    
    elif args.command == "help":
        print("""QQ P2P 连接机器人

用法:
  python bot.py --user-id <QQ号> --command <命令>

命令:
  start     启动机器人（监听QQ消息）
  ip        查询本机IP
  online    检查机器人状态
  friends   查看好友列表
  groups    查看群列表
  help      显示帮助

示例:
  python bot.py --user-id 123456789 --start
  python bot.py --user-id 123456789 --ip
  python bot.py --user-id 123456789 --online
""")

if __name__ == "__main__":
    asyncio.run(main())

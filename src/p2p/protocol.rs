//! /tun/1.0.0 自定义协议
//!
//! 协议 ID: `/tun/1.0.0`
//!
//! 将 TUN 数据包与控制消息复用在同一条 libp2p connection 上。
//! 控制帧走 reliable stream，数据帧优先走 datagram（尽力而为）。

#![allow(dead_code)]

use libp2p::PeerId;
use serde::{Deserialize, Serialize};

// ============================================================
// 消息类型
// ============================================================

/// 协议消息类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(non_camel_case_types)]
pub enum MessageType {
    HELLO = 0x01,
    JOIN_ACK = 0x02,
    LEAVE = 0x03,
    IP_CONFLICT = 0x04,
    DATA = 0x05,
}

impl MessageType {
    pub fn as_byte(self) -> u8 { self as u8 }
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(MessageType::HELLO),
            0x02 => Some(MessageType::JOIN_ACK),
            0x03 => Some(MessageType::LEAVE),
            0x04 => Some(MessageType::IP_CONFLICT),
            0x05 => Some(MessageType::DATA),
            _ => None,
        }
    }
}

// ============================================================
// 消息载荷
// ============================================================

/// HELLO 消息：发起连接时发送，宣告自己的存在
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloMsg {
    /// libp2p PeerId 的 base58/hex 编码
    pub peer_id: String,
    /// 编排层分配的虚拟 IP
    pub virtual_ip: String,
    /// 特性位掩码（支持 DCUtR、QUIC 等）
    pub features: u32,
}

/// JOIN_ACK 消息：对方 HELLO 的响应，确认成员列表
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinAckMsg {
    pub members: Vec<MemberInfo>,
}

/// 成员信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberInfo {
    pub peer_id: String,
    pub virtual_ip: String,
}

/// LEAVE 消息：退网通知
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaveMsg {
    pub peer_id: String,
}

/// IP_CONFLICT 消息：虚拟 IP 冲突通知
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpConflictMsg {
    pub virtual_ip: String,
}

/// DATA 消息：TUN 原始数据包
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataMsg {
    pub src_ip: String,
    pub dst_ip: String,
    pub raw: Vec<u8>,
}

// ============================================================
// 协议帧编解码
// ============================================================

/// 编码单帧: [1B type][4B len][payload]
/// 总计: 1 + 4 + payload.len() 字节
pub fn encode_frame(type_: MessageType, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(1 + 4 + payload.len());
    frame.push(type_.as_byte());
    // 大端序写入 4 字节长度
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// 解码单帧
/// 返回: (MessageType, payload_bytes)
pub fn decode_frame(data: &[u8]) -> Option<(MessageType, Vec<u8>)> {
    // 最小帧: 1B type + 4B len = 5 字节（即使 payload 为空）
    if data.len() < 5 {
        return None;
    }

    let type_byte = data[0];
    let msg_type = MessageType::from_byte(type_byte)?;

    // 提取 payload 长度 (大端序 4 字节)
    let payload_len = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize;

    // 检查数据是否足够包含完整的 payload
    if data.len() < 5 + payload_len {
        return None;
    }

    let payload = data[5..(5 + payload_len)].to_vec();
    Some((msg_type, payload))
}

// ============================================================
// 协议处理器
// ============================================================

/// /tun/1.0.0 协议处理器
///
/// 该处理器负责：
/// 1. 接收对等节点发来的帧
/// 2. 解码并分发到对应的消息处理逻辑
/// 3. 对于控制帧 (HELLO, JOIN_ACK 等) 确保可靠送达
/// 4. 数据帧 (DATA) 可根据情况走 stream 或 datagram
pub struct TunProtocolHandler {
    /// 本机 PeerId，用于在 HELLO 中自我宣告
    local_peer_id: PeerId,
    /// 本机分配的虚拟 IP，用于在 HELLO 中宣告
    local_virtual_ip: String,
}

impl TunProtocolHandler {
    /// 创建新的协议处理器
    pub fn new(local_peer_id: PeerId, local_virtual_ip: String) -> Self {
        TunProtocolHandler {
            local_peer_id,
            local_virtual_ip,
        }
    }

    /// 获取本机 PeerId
    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    /// 编码 HELLO 消息帧
    fn encode_hello(&self) -> Vec<u8> {
        let hello = HelloMsg {
            peer_id: self.local_peer_id.to_base58(),
            virtual_ip: self.local_virtual_ip.clone(),
            features: 0b11, // 示例：位 0 = DCUtR 支持, 位 1 = QUIC 支持
        };
        let payload = serde_json::to_vec(&hello).unwrap_or_default();
        encode_frame(MessageType::HELLO, &payload)
    }

    /// 编码 JOIN_ACK 消息帧 (成员列表)
    pub fn encode_join_ack(members: &[MemberInfo]) -> Vec<u8> {
        let msg = JoinAckMsg {
            members: members.to_vec(),
        };
        let payload = serde_json::to_vec(&msg).unwrap_or_default();
        encode_frame(MessageType::JOIN_ACK, &payload)
    }

    /// 解码并处理收到的帧
    pub fn handle_incoming(&self, data: &[u8]) -> Option<(MessageType, Vec<u8>)> {
        decode_frame(data)
    }

    /// 构造发送 HELLO 消息的帧数据
    pub fn build_hello_frame(&self) -> Vec<u8> {
        self.encode_hello()
    }

    /// 解析收到的 HELLO 消息载荷
    pub fn parse_hello_payload(payload: &[u8]) -> Option<(String, String, u32)> {
        let msg: HelloMsg = serde_json::from_slice(payload).ok()?;
        Some((msg.peer_id, msg.virtual_ip, msg.features))
    }

    /// 解析收到的 JOIN_ACK 消息载荷
    pub fn parse_join_ack_payload(payload: &[u8]) -> Option<Vec<MemberInfo>> {
        let msg: JoinAckMsg = serde_json::from_slice(payload).ok()?;
        Some(msg.members)
    }
}

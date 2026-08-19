//! NapCat API 客户端：HTTP 接口封装 + 配置 + 数据结构
//!
//! 只负责与 NapCat 交互（发消息、查好友/群/登录信息）。
//! P2P 节点逻辑见 `crate::p2p`，WebSocket 事件监听见 `crate::ws`。

use reqwest::Client;
use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};

/// NapCat 连接配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NapCatConfig {
    pub http_host: String,
    pub http_port: u16,
    pub ws_host: String,
    pub ws_port: u16,
    pub token: Option<String>,
}

impl Default for NapCatConfig {
    fn default() -> Self {
        Self {
            http_host: "127.0.0.1".to_string(),
            http_port: 3000,
            ws_host: "127.0.0.1".to_string(),
            ws_port: 3001,
            token: None,
        }
    }
}

/// NapCat HTTP 响应统一包装
#[derive(Debug, Deserialize)]
pub struct NapCatResponse<T> {
    /// serde 反序列化字段（供 API 响应解析）
    #[allow(dead_code)]
    pub status: String,
    pub retcode: i32,
    pub data: Option<T>,
    pub message: Option<String>,
}

/// 登录账号信息
#[derive(Debug, Deserialize)]
pub struct UserInfo {
    /// serde 反序列化字段
    #[allow(dead_code)]
    pub user_id: u64,
    pub nickname: String,
}

/// 好友信息
#[derive(Debug, Deserialize)]
pub struct FriendInfo {
    pub user_id: u64,
    pub nickname: String,
}

/// 群信息
#[derive(Debug, Deserialize)]
pub struct GroupInfo {
    pub group_id: u64,
    pub group_name: String,
}

/// NapCat HTTP API 客户端
#[derive(Debug, Clone)]
pub struct NapCatClient {
    client: Client,
    pub config: NapCatConfig,
}

impl NapCatClient {
    pub fn new(config: NapCatConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    pub async fn new_default() -> Result<Self> {
        Ok(Self::new(NapCatConfig::default()))
    }

    fn base_url(&self) -> String {
        format!("http://{}:{}", self.config.http_host, self.config.http_port)
    }

    pub async fn get_friends(&self) -> Result<Vec<FriendInfo>> {
        let url = format!("{}/get_friend_list", self.base_url());
        let resp: NapCatResponse<Vec<FriendInfo>> = self.client.get(&url).send().await?
            .json().await?;

        if resp.retcode != 0 {
            anyhow::bail!("获取好友列表失败: {:?}", resp.message);
        }

        Ok(resp.data.unwrap_or_default())
    }

    pub async fn get_groups(&self) -> Result<Vec<GroupInfo>> {
        let url = format!("{}/get_group_list", self.base_url());
        let resp: NapCatResponse<Vec<GroupInfo>> = self.client.get(&url).send().await?
            .json().await?;

        if resp.retcode != 0 {
            anyhow::bail!("获取群列表失败: {:?}", resp.message);
        }

        Ok(resp.data.unwrap_or_default())
    }

    pub async fn send_private_message(&self, user_id: u64, message: &str) -> Result<i64> {
        let url = format!("{}/send_private_msg", self.base_url());
        let params = serde_json::json!({
            "user_id": user_id,
            "message": message
        });

        let resp: NapCatResponse<serde_json::Value> = self.client.post(&url)
            .json(&params)
            .send()
            .await?
            .json()
            .await?;

        if resp.retcode != 0 {
            anyhow::bail!("发送私聊消息失败: {:?}", resp.message);
        }

        Ok(0)
    }

    pub async fn send_group_message(&self, group_id: u64, message: &str) -> Result<i64> {
        let url = format!("{}/send_group_msg", self.base_url());
        let params = serde_json::json!({
            "group_id": group_id,
            "message": message
        });

        let resp: NapCatResponse<serde_json::Value> = self.client.post(&url)
            .json(&params)
            .send()
            .await?
            .json()
            .await?;

        if resp.retcode != 0 {
            anyhow::bail!("发送群消息失败: {:?}", resp.message);
        }

        Ok(0)
    }

    pub async fn check_online(&self) -> Result<bool> {
        let url = format!("{}/get_login_info", self.base_url());
        Ok(self.client.get(&url).send().await.is_ok())
    }

    pub async fn get_login_info(&self) -> Result<UserInfo> {
        let url = format!("{}/get_login_info", self.base_url());
        let resp: NapCatResponse<UserInfo> = self.client.get(&url).send().await?.json().await?;

        if resp.retcode != 0 {
            anyhow::bail!("获取登录信息失败: {:?}", resp.message);
        }

        resp.data.context("登录信息为空")
    }
}

//! 数据模型定义

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 监控类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MonitorType {
    Http,
    Tcp,
    Ping,
}

impl MonitorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MonitorType::Http => "http",
            MonitorType::Tcp => "tcp",
            MonitorType::Ping => "ping",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "http" | "https" => Some(MonitorType::Http),
            "tcp" => Some(MonitorType::Tcp),
            "ping" | "icmp" => Some(MonitorType::Ping),
            _ => None,
        }
    }
}

/// 监控项（Monitor）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {
    pub id: String,           // UUID（应用侧生成，避免依赖自增行为）
    pub name: String,
    pub monitor_type: MonitorType,
    pub target: String,
    pub interval: u64,        // 检测间隔（秒）
    pub timeout: u64,         // 超时（秒）
    pub retries: u32,         // 重试次数
    pub active: bool,         // 是否启用
    pub created_at: DateTime<Utc>,
}

/// 创建/更新监控项的请求体
#[derive(Debug, Clone, Deserialize)]
pub struct MonitorInput {
    pub name: String,
    pub monitor_type: String,
    pub target: String,
    #[serde(default = "default_interval")]
    pub interval: i64,
    #[serde(default = "default_timeout")]
    pub timeout: i64,
    #[serde(default = "default_retries")]
    pub retries: i64,
    #[serde(default = "default_active")]
    pub active: bool,
}

fn default_interval() -> i64 { 60 }
fn default_timeout() -> i64 { 10 }
fn default_retries() -> i64 { 1 }
fn default_active() -> bool { true }

/// 一次心跳（检测结果）记录
#[derive(Debug, Clone, Serialize)]
pub struct Heartbeat {
    pub id: String,
    pub monitor_id: String,
    pub status: String,       // "up" | "down" | "pending"
    pub response_time_ms: Option<i64>,
    pub message: String,
    pub checked_at: DateTime<Utc>,
}

/// 事件（状态变化）
#[derive(Debug, Clone, Serialize)]
pub struct Incident {
    pub id: String,
    pub monitor_id: String,
    pub from_status: String,
    pub to_status: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

/// 图表数据点（仅存内存，随服务器重启丢失）
#[derive(Debug, Clone, Serialize)]
pub struct ChartPoint {
    pub status: String,
    pub response_time_ms: Option<i64>,
    pub checked_at: DateTime<Utc>,
}

/// 监控项运行状态（含最近心跳统计），用于状态页
#[derive(Debug, Clone, Serialize)]
pub struct MonitorStatus {
    #[serde(flatten)]
    pub monitor: Monitor,
    pub status: String,
    pub avg_response_ms: Option<f64>,
    pub uptime_24h: Option<f64>, // 24 小时可用率（百分比）
    pub last_check: Option<DateTime<Utc>>,
    pub last_message: Option<String>,
    pub heartbeat_count: i64,
}

//! 内存图表数据：每个监控项保留最近 3 天的检测点，服务器重启即丢失。

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use chrono::{Duration, Utc};

use crate::models::ChartPoint;

/// 数据保留时长：3 天
const RETENTION_HOURS: i64 = 72;
/// 单个监控项最大点数（防止超短间隔打爆内存）
const MAX_POINTS: usize = 20_000;

#[derive(Default)]
pub struct ChartStore {
    inner: Mutex<HashMap<String, VecDeque<ChartPoint>>>,
}

impl ChartStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一个检测点，并做 3 天 / 点数上限裁剪
    pub fn push(&self, monitor_id: &str, status: &str, response_time_ms: Option<i64>) {
        let mut map = self.inner.lock().unwrap();
        let q = map.entry(monitor_id.to_string()).or_default();
        q.push_back(ChartPoint {
            status: status.to_string(),
            response_time_ms,
            checked_at: Utc::now(),
        });
        let cutoff = Utc::now() - Duration::hours(RETENTION_HOURS);
        while q.front().is_some_and(|p| p.checked_at < cutoff) {
            q.pop_front();
        }
        while q.len() > MAX_POINTS {
            q.pop_front();
        }
    }

    /// 取最近 `limit` 个点（按时间升序返回）
    pub fn recent(&self, monitor_id: &str, limit: usize) -> Vec<ChartPoint> {
        let map = self.inner.lock().unwrap();
        match map.get(monitor_id) {
            Some(q) => {
                let skip = q.len().saturating_sub(limit);
                q.iter().skip(skip).cloned().collect()
            }
            None => Vec::new(),
        }
    }

    /// 删除监控项时同步清理图表数据
    pub fn remove(&self, monitor_id: &str) {
        self.inner.lock().unwrap().remove(monitor_id);
    }
}

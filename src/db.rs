//! 数据层：turso（纯 Rust 实现的嵌入式 SQLite 兼容数据库），本地文件存储
//!
//! - 数据文件路径：RUST_KUMA_DB > DATA_DIR > 默认 rust-kuma.db
//! - 主键全部为应用侧生成的 TEXT UUID，排序依赖时间列

use std::sync::Arc;

use chrono::{Duration, Utc};
use tokio::sync::Mutex;
use turso::Value;

use crate::error::{AppError, AppResult};
use crate::models::{Heartbeat, Incident, Monitor, MonitorInput, MonitorStatus, MonitorType};

pub struct Db {
    conn: Arc<Mutex<turso::Connection>>,
    /// 数据库描述，用于启动日志
    pub desc: String,
}

// ---------- Value 辅助转换 ----------

fn v_text(v: Value) -> String {
    match v {
        Value::Text(s) => s,
        _ => String::new(),
    }
}

fn v_i64(v: Value) -> i64 {
    match v {
        Value::Integer(i) => i,
        Value::Real(f) => f as i64,
        _ => 0,
    }
}

fn v_opt_i64(v: Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(i),
        Value::Real(f) => Some(f as i64),
        _ => None,
    }
}

fn v_opt_f64(v: Value) -> Option<f64> {
    match v {
        Value::Real(f) => Some(f),
        Value::Integer(i) => Some(i as f64),
        _ => None,
    }
}

fn v_bool(v: Value) -> bool {
    v_i64(v) != 0
}

fn v_opt_time(v: Value) -> chrono::DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(&v_text(v))
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

impl Db {
    /// 打开（或创建）本地嵌入式数据库并建表
    pub async fn open() -> AppResult<Self> {
        let path = std::env::var("RUST_KUMA_DB")
            .or_else(|_| std::env::var("DATA_DIR")
                .map(|d| format!("{}/rust-kuma.db", d.trim_end_matches('/'))))
            .unwrap_or_else(|_| "rust-kuma.db".to_string());

        let db = turso::Builder::new_local(&path).build().await?;
        let desc = format!("turso 本地嵌入式库 ({path})");

        let conn = db.connect()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS monitors (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                type        TEXT NOT NULL,
                target      TEXT NOT NULL,
                interval    INTEGER NOT NULL DEFAULT 60,
                timeout     INTEGER NOT NULL DEFAULT 10,
                retries     INTEGER NOT NULL DEFAULT 1,
                active      INTEGER NOT NULL DEFAULT 1,
                created_at  TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS heartbeats (
                id               TEXT PRIMARY KEY,
                monitor_id       TEXT NOT NULL,
                status           TEXT NOT NULL,
                response_time_ms INTEGER,
                message          TEXT NOT NULL DEFAULT '',
                checked_at       TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_heartbeats_monitor_time
                ON heartbeats(monitor_id, checked_at);

            CREATE TABLE IF NOT EXISTS incidents (
                id          TEXT PRIMARY KEY,
                monitor_id  TEXT NOT NULL,
                from_status TEXT NOT NULL,
                to_status   TEXT NOT NULL,
                message     TEXT NOT NULL DEFAULT '',
                created_at  TEXT NOT NULL
            );
            "#,
        )
        .await?;

        Ok(Db { conn: Arc::new(Mutex::new(conn)), desc })
    }

    fn now() -> String {
        Utc::now().to_rfc3339()
    }

    /// 规范化目标地址：
    /// - HTTP 类型未写协议时自动补 `https://`
    /// - TCP / Ping 类型去掉误填的 `http(s)://` 前缀
    fn normalize_target(ty: MonitorType, target: &str) -> AppResult<String> {
        let t = target.trim();
        if t.is_empty() {
            return Err(AppError::BadRequest("target 不能为空".into()));
        }
        Ok(match ty {
            MonitorType::Http => {
                let lower = t.to_ascii_lowercase();
                if lower.starts_with("http://") || lower.starts_with("https://") {
                    t.to_string()
                } else {
                    format!("https://{t}")
                }
            }
            MonitorType::Tcp | MonitorType::Ping => {
                let lower = t.to_ascii_lowercase();
                if let Some(rest) = lower.strip_prefix("https://") {
                    t[t.len() - rest.len()..].trim_end_matches('/').to_string()
                } else if let Some(rest) = lower.strip_prefix("http://") {
                    t[t.len() - rest.len()..].trim_end_matches('/').to_string()
                } else {
                    t.to_string()
                }
            }
        })
    }

    // ---------- Monitor CRUD ----------

    pub async fn create_monitor(&self, input: MonitorInput) -> AppResult<Monitor> {
        let ty = MonitorType::from_str(&input.monitor_type)
            .ok_or_else(|| AppError::BadRequest(format!("未知监控类型: {}", input.monitor_type)))?;
        if input.interval == 0 {
            return Err(AppError::BadRequest("interval 必须大于 0".into()));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let target = Self::normalize_target(ty, &input.target)?;
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO monitors (id, name, type, target, interval, timeout, retries, active, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            (
                Value::Text(id.clone()),
                Value::Text(input.name),
                Value::Text(ty.as_str().to_string()),
                Value::Text(target),
                Value::Integer(input.interval),
                Value::Integer(input.timeout),
                Value::Integer(input.retries),
                Value::Integer(input.active as i64),
                Value::Text(Self::now()),
            ),
        )
        .await?;
        drop(conn);
        self.get_monitor(&id).await
    }

    pub async fn get_monitor(&self, id: &str) -> AppResult<Monitor> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, name, type, target, interval, timeout, retries, active, created_at
                 FROM monitors WHERE id = ?1",
                (Value::Text(id.to_string()),),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(row_to_monitor(&row)?),
            None => Err(AppError::NotFound),
        }
    }

    async fn query_monitors(&self, sql: &str, params: Vec<Value>) -> AppResult<Vec<Monitor>> {
        let conn = self.conn.lock().await;
        let mut rows = conn.query(sql, params).await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(row_to_monitor(&row)?);
        }
        Ok(out)
    }

    pub async fn list_monitors(&self) -> AppResult<Vec<Monitor>> {
        self.query_monitors(
            "SELECT id, name, type, target, interval, timeout, retries, active, created_at
             FROM monitors ORDER BY created_at, id",
            vec![],
        )
        .await
    }

    pub async fn list_active_monitors(&self) -> AppResult<Vec<Monitor>> {
        self.query_monitors(
            "SELECT id, name, type, target, interval, timeout, retries, active, created_at
             FROM monitors WHERE active = 1 ORDER BY created_at, id",
            vec![],
        )
        .await
    }

    pub async fn update_monitor(&self, id: &str, input: MonitorInput) -> AppResult<Monitor> {
        let ty = MonitorType::from_str(&input.monitor_type)
            .ok_or_else(|| AppError::BadRequest(format!("未知监控类型: {}", input.monitor_type)))?;
        let conn = self.conn.lock().await;
        let target = Self::normalize_target(ty, &input.target)?;
        let n = conn
            .execute(
                "UPDATE monitors SET name=?1, type=?2, target=?3, interval=?4, timeout=?5,
                        retries=?6, active=?7 WHERE id=?8",
                (
                    Value::Text(input.name),
                    Value::Text(ty.as_str().to_string()),
                    Value::Text(target),
                    Value::Integer(input.interval),
                    Value::Integer(input.timeout),
                    Value::Integer(input.retries),
                    Value::Integer(input.active as i64),
                    Value::Text(id.to_string()),
                ),
            )
            .await?;
        if n == 0 {
            return Err(AppError::NotFound);
        }
        drop(conn);
        self.get_monitor(id).await
    }

    pub async fn delete_monitor(&self, id: &str) -> AppResult<()> {
        let conn = self.conn.lock().await;
        // 手动级联删除（不依赖外键 ON DELETE CASCADE）
        conn.execute(
            "DELETE FROM heartbeats WHERE monitor_id = ?1",
            (Value::Text(id.to_string()),),
        )
        .await?;
        conn.execute(
            "DELETE FROM incidents WHERE monitor_id = ?1",
            (Value::Text(id.to_string()),),
        )
        .await?;
        let n = conn
            .execute("DELETE FROM monitors WHERE id = ?1", (Value::Text(id.to_string()),))
            .await?;
        if n == 0 {
            return Err(AppError::NotFound);
        }
        Ok(())
    }

    // ---------- Heartbeat ----------

    pub async fn insert_heartbeat(
        &self,
        monitor_id: &str,
        status: &str,
        response_time_ms: Option<i64>,
        message: &str,
    ) -> AppResult<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO heartbeats (id, monitor_id, status, response_time_ms, message, checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                Value::Text(uuid::Uuid::new_v4().to_string()),
                Value::Text(monitor_id.to_string()),
                Value::Text(status.to_string()),
                match response_time_ms {
                    Some(v) => Value::Integer(v),
                    None => Value::Null,
                },
                Value::Text(message.to_string()),
                Value::Text(Self::now()),
            ),
        )
        .await?;
        Ok(())
    }

    pub async fn last_heartbeat(&self, monitor_id: &str) -> AppResult<Option<Heartbeat>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, monitor_id, status, response_time_ms, message, checked_at
                 FROM heartbeats WHERE monitor_id = ?1 ORDER BY checked_at DESC LIMIT 1",
                (Value::Text(monitor_id.to_string()),),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(row_to_heartbeat(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn recent_heartbeats(&self, monitor_id: &str, limit: usize) -> AppResult<Vec<Heartbeat>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, monitor_id, status, response_time_ms, message, checked_at
                 FROM heartbeats WHERE monitor_id = ?1 ORDER BY checked_at DESC LIMIT ?2",
                (Value::Text(monitor_id.to_string()), Value::Integer(limit as i64)),
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(row_to_heartbeat(&row)?);
        }
        Ok(out)
    }

    /// 删除早于 `hours` 小时的心跳与事件记录（配合 3 天数据保留策略）
    pub async fn prune_old(&self, hours: i64) -> AppResult<()> {
        let cutoff = Value::Text((Utc::now() - Duration::hours(hours)).to_rfc3339());
        let conn = self.conn.lock().await;
        conn.execute("DELETE FROM heartbeats WHERE checked_at < ?1", (cutoff.clone(),)).await?;
        conn.execute("DELETE FROM incidents WHERE created_at < ?1", (cutoff,)).await?;
        Ok(())
    }

    // ---------- Incident ----------

    pub async fn list_incidents(&self, monitor_id: &str, limit: usize) -> AppResult<Vec<Incident>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, monitor_id, from_status, to_status, message, created_at
                 FROM incidents WHERE monitor_id = ?1 ORDER BY created_at DESC LIMIT ?2",
                (Value::Text(monitor_id.to_string()), Value::Integer(limit as i64)),
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(row_to_incident(&row)?);
        }
        Ok(out)
    }

    #[allow(dead_code)] // 预留：查询最近一次状态变化
    pub async fn last_incident(&self, monitor_id: &str) -> AppResult<Option<Incident>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, monitor_id, from_status, to_status, message, created_at
                 FROM incidents WHERE monitor_id = ?1 ORDER BY created_at DESC LIMIT 1",
                (Value::Text(monitor_id.to_string()),),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(row_to_incident(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn insert_incident(
        &self,
        monitor_id: &str,
        from_status: &str,
        to_status: &str,
        message: &str,
    ) -> AppResult<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO incidents (id, monitor_id, from_status, to_status, message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                Value::Text(uuid::Uuid::new_v4().to_string()),
                Value::Text(monitor_id.to_string()),
                Value::Text(from_status.to_string()),
                Value::Text(to_status.to_string()),
                Value::Text(message.to_string()),
                Value::Text(Self::now()),
            ),
        )
        .await?;
        Ok(())
    }

    // ---------- Status / stats ----------

    /// 计算某个监控项的综合状态（含 24h 可用率、平均延迟、心跳数）
    pub async fn monitor_status(&self, monitor_id: &str) -> AppResult<Option<MonitorStatus>> {
        let monitor = match self.get_monitor(monitor_id).await {
            Ok(m) => m,
            Err(AppError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };

        let since = (Utc::now() - Duration::hours(24)).to_rfc3339();
        let conn = self.conn.lock().await;

        let mut rows = conn
            .query(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN status='up' THEN 1 ELSE 0 END), 0),
                        AVG(response_time_ms)
                 FROM heartbeats
                 WHERE monitor_id = ?1 AND checked_at >= ?2",
                (Value::Text(monitor_id.to_string()), Value::Text(since)),
            )
            .await?;
        let (count, up_count, avg_rt) = match rows.next().await? {
            Some(row) => (
                v_i64(row.get_value(0)?),
                v_i64(row.get_value(1)?),
                v_opt_f64(row.get_value(2)?),
            ),
            None => (0, 0, None),
        };
        drop(conn);

        let last = self.last_heartbeat(monitor_id).await?;

        let uptime = if count > 0 {
            Some(up_count as f64 / count as f64 * 100.0)
        } else {
            None
        };

        Ok(Some(MonitorStatus {
            status: last.as_ref().map(|h| h.status.clone()).unwrap_or_else(|| "pending".into()),
            avg_response_ms: avg_rt,
            uptime_24h: uptime,
            last_check: last.as_ref().map(|h| h.checked_at),
            last_message: last.map(|h| h.message),
            heartbeat_count: count,
            monitor,
        }))
    }
}

fn row_to_monitor(row: &turso::Row) -> AppResult<Monitor> {
    Ok(Monitor {
        id: v_text(row.get_value(0)?),
        name: v_text(row.get_value(1)?),
        monitor_type: MonitorType::from_str(&v_text(row.get_value(2)?)).unwrap_or(MonitorType::Http),
        target: v_text(row.get_value(3)?),
        interval: v_i64(row.get_value(4)?) as u64,
        timeout: v_i64(row.get_value(5)?) as u64,
        retries: v_i64(row.get_value(6)?) as u32,
        active: v_bool(row.get_value(7)?),
        created_at: v_opt_time(row.get_value(8)?),
    })
}

fn row_to_heartbeat(row: &turso::Row) -> AppResult<Heartbeat> {
    Ok(Heartbeat {
        id: v_text(row.get_value(0)?),
        monitor_id: v_text(row.get_value(1)?),
        status: v_text(row.get_value(2)?),
        response_time_ms: v_opt_i64(row.get_value(3)?),
        message: v_text(row.get_value(4)?),
        checked_at: v_opt_time(row.get_value(5)?),
    })
}

fn row_to_incident(row: &turso::Row) -> AppResult<Incident> {
    Ok(Incident {
        id: v_text(row.get_value(0)?),
        monitor_id: v_text(row.get_value(1)?),
        from_status: v_text(row.get_value(2)?),
        to_status: v_text(row.get_value(3)?),
        message: v_text(row.get_value(4)?),
        created_at: v_opt_time(row.get_value(5)?),
    })
}

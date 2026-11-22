//! REST API

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::auth::Auth;
use crate::chart::ChartStore;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::models::{ChartPoint, MonitorInput, MonitorStatus};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub charts: Arc<ChartStore>,
    pub auth: Arc<Auth>,
}

pub fn router(db: Arc<Db>, charts: Arc<ChartStore>, auth: Arc<Auth>) -> Router {
    let state = AppState { db, charts, auth };
    Router::new()
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .route("/api/monitors", get(list_monitors).post(create_monitor))
        .route(
            "/api/monitors/{id}",
            get(get_monitor).put(update_monitor).delete(delete_monitor),
        )
        .route("/api/monitors/{id}/status", get(get_status))
        .route("/api/monitors/{id}/heartbeats", get(get_heartbeats))
        .route("/api/monitors/{id}/incidents", get(get_incidents))
        .route("/api/monitors/{id}/chart", get(get_chart))
        .route("/api/stats", get(get_stats))
        .with_state(state)
}

// ---------- 鉴权 ----------

/// 从 Authorization: Bearer <token> 提取并校验
fn require_auth(s: &AppState, headers: &HeaderMap) -> AppResult<String> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| t.strip_prefix("Bearer "));
    match token {
        Some(t) if s.auth.verify(t) => Ok(t.to_string()),
        _ => Err(AppError::Unauthorized),
    }
}

#[derive(Deserialize)]
struct LoginInput {
    password: String,
}

async fn login(State(s): State<AppState>, Json(input): Json<LoginInput>) -> AppResult<Json<serde_json::Value>> {
    match s.auth.login(&input.password) {
        Some(token) => Ok(Json(serde_json::json!({ "token": token }))),
        None => Err(AppError::Unauthorized),
    }
}

async fn logout(State(s): State<AppState>, headers: HeaderMap) -> AppResult<StatusCode> {
    if let Ok(token) = require_auth(&s, &headers) {
        s.auth.logout(&token);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// 前端启动时校验 token 是否仍有效
async fn me(State(s): State<AppState>, headers: HeaderMap) -> AppResult<Json<serde_json::Value>> {
    let admin = require_auth(&s, &headers).is_ok();
    Ok(Json(serde_json::json!({ "admin": admin })))
}

// ---------- 监控项 ----------

async fn list_monitors(State(s): State<AppState>) -> AppResult<Json<Vec<MonitorStatus>>> {
    let monitors = s.db.list_monitors().await?;
    let mut out = Vec::new();
    for m in monitors {
        if let Some(st) = s.db.monitor_status(&m.id).await? {
            out.push(st);
        }
    }
    Ok(Json(out))
}

async fn create_monitor(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<MonitorInput>,
) -> AppResult<(StatusCode, Json<crate::models::Monitor>)> {
    require_auth(&s, &headers)?;
    let m = s.db.create_monitor(input).await?;
    Ok((StatusCode::CREATED, Json(m)))
}

async fn get_monitor(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<MonitorStatus>> {
    let st = s.db.monitor_status(&id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(st))
}

async fn update_monitor(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<MonitorInput>,
) -> AppResult<Json<crate::models::Monitor>> {
    require_auth(&s, &headers)?;
    let m = s.db.update_monitor(&id, input).await?;
    Ok(Json(m))
}

async fn delete_monitor(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    require_auth(&s, &headers)?;
    s.db.delete_monitor(&id).await?;
    s.charts.remove(&id);
    Ok(StatusCode::NO_CONTENT)
}

async fn get_status(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<MonitorStatus>> {
    let st = s.db.monitor_status(&id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(st))
}

async fn get_heartbeats(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<crate::models::Heartbeat>>> {
    let hb = s.db.recent_heartbeats(&id, 100).await?;
    Ok(Json(hb))
}

async fn get_incidents(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<crate::models::Incident>>> {
    let inc = s.db.list_incidents(&id, 100).await?;
    Ok(Json(inc))
}

/// 图表数据：内存缓存，最多 3 天（最多 5000 点）
async fn get_chart(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<ChartPoint>>> {
    // 不存在的监控项返回 404
    s.db.get_monitor(&id).await?;
    Ok(Json(s.charts.recent(&id, 5000)))
}

/// 全局统计
async fn get_stats(State(s): State<AppState>) -> AppResult<Json<serde_json::Value>> {
    let monitors = s.db.list_monitors().await?;
    let mut up = 0;
    let mut down = 0;
    let mut pending = 0;
    for m in &monitors {
        if let Some(st) = s.db.monitor_status(&m.id).await? {
            match st.status.as_str() {
                "up" => up += 1,
                "down" => down += 1,
                _ => pending += 1,
            }
        }
    }
    Ok(Json(serde_json::json!({
        "total": monitors.len(),
        "up": up,
        "down": down,
        "pending": pending,
    })))
}

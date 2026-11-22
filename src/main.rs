use std::sync::Arc;

use crate::auth::Auth;
use crate::chart::ChartStore;
use crate::db::Db;
use crate::scheduler::Scheduler;

mod api;
mod auth;
mod checker;
mod chart;
mod db;
mod error;
mod models;
mod scheduler;

#[tokio::main]
async fn main() {
    // 数据库：turso 本地嵌入式库（纯 Rust，无 C 依赖，无需任何云服务）
    let db = Arc::new(Db::open().await.expect("打开本地数据库失败"));

    // 监听地址：优先使用容器平台注入的 PORT，否则用 RUST_KUMA_ADDR。
    let listen_addr = std::env::var("PORT")
        .map(|p| format!("0.0.0.0:{}", p.trim()))
        .or_else(|_| std::env::var("RUST_KUMA_ADDR"))
        .unwrap_or_else(|_| "0.0.0.0:3001".to_string());

    // 管理员密码：优先环境变量，否则自动生成随机密码并打印到日志
    let (auth, generated_pw) = match std::env::var("RUST_KUMA_ADMIN_PASSWORD") {
        Ok(p) if !p.trim().is_empty() => (Arc::new(Auth::new(p)), None),
        _ => {
            let p = uuid::Uuid::new_v4().to_string();
            (Arc::new(Auth::new(p.clone())), Some(p))
        }
    };

    // 内存图表数据（重启丢失，每个监控保留 3 天）
    let charts = Arc::new(ChartStore::new());

    // 启动后台调度器
    let scheduler = Arc::new(Scheduler::new(db.clone(), charts.clone()));
    let sched_handle = tokio::spawn(async move {
        scheduler.run().await;
    });

    // 构建路由：API + 静态页面
    let app = api::router(db.clone(), charts.clone(), auth)
        .fallback_service(tower_http::services::ServeDir::new("static"));

    println!("rust-kuma 已启动");
    println!("  数据库: {}", db.desc);
    println!("  监听:   http://{listen_addr}");
    if let Some(pw) = generated_pw {
        println!("  管理员密码（自动生成，可用 RUST_KUMA_ADMIN_PASSWORD 固定）: {pw}");
    }

    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .expect("绑定端口失败");

    let server = axum::serve(listener, app);
    tokio::select! {
        _ = server => {},
        _ = sched_handle => {},
    }
}

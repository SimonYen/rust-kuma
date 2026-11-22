//! 监控检测：HTTP / TCP / Ping

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use crate::models::{Monitor, MonitorType};

/// 一次检测结果
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub status: String,           // "up" | "down"
    pub response_time_ms: Option<i64>,
    pub message: String,
}

/// 执行一次检测（含重试）
pub async fn check(monitor: &Monitor) -> CheckResult {
    let mut last: Option<CheckResult> = None;
    // retries 表示失败后的额外重试次数，总尝试次数 = retries + 1
    for _ in 0..=monitor.retries {
        let r = check_once(monitor).await;
        if r.status == "up" {
            return r;
        }
        last = Some(r);
    }
    last.unwrap_or(CheckResult {
        status: "down".into(),
        response_time_ms: None,
        message: "无检测结果".into(),
    })
}

async fn check_once(monitor: &Monitor) -> CheckResult {
    match monitor.monitor_type {
        MonitorType::Http => check_http(monitor).await,
        MonitorType::Tcp => check_tcp(monitor).await,
        MonitorType::Ping => check_ping(monitor).await,
    }
}

/// 浏览器风格的 User-Agent：部分站点/网关会直接拒绝默认的 reqwest UA
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 rust-kuma/0.1";

async fn check_http(monitor: &Monitor) -> CheckResult {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(monitor.timeout))
        .danger_accept_invalid_certs(true)
        // 保留跳转过程中的 cookie：不少网关会 303 到登录页并依赖 set-cookie 放行
        .cookie_store(true)
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return down(None, format!("构建 HTTP 客户端失败: {e}"));
        }
    };

    let start = tokio::time::Instant::now();
    match client.get(&monitor.target).send().await {
        Ok(resp) => {
            let rt = start.elapsed().as_millis() as i64;
            let code = resp.status().as_u16();
            if resp.status().is_success() || resp.status().is_redirection() {
                up(rt, format!("HTTP {code}"))
            } else {
                down(Some(rt), format!("HTTP {code}"))
            }
        }
        Err(e) => down(None, format!("请求失败: {e}")),
    }
}

async fn check_tcp(monitor: &Monitor) -> CheckResult {
    let start = tokio::time::Instant::now();
    let addr = parse_host_port(&monitor.target);
    match addr {
        Some(a) => match tokio::time::timeout(
            Duration::from_secs(monitor.timeout),
            tokio::net::TcpStream::connect(a),
        )
        .await
        {
            Ok(Ok(_)) => {
                let rt = start.elapsed().as_millis() as i64;
                up(rt, "TCP 连接成功".into())
            }
            Ok(Err(e)) => down(None, format!("TCP 连接失败: {e}")),
            Err(_) => down(None, "连接超时".into()),
        },
        None => down(None, format!("无效的目标地址: {}", monitor.target)),
    }
}

async fn check_ping(monitor: &Monitor) -> CheckResult {
    // 轻量级实现：不引入 ICMP raw socket（需要 root），
    // 通过 TCP 连接目标的 80/443 端口来近似判断主机可达性。
    // 若目标是 host:port 则直连该端口。
    let target = if monitor.target.contains(':') {
        monitor.target.clone()
    } else {
        format!("{}:80", monitor.target)
    };

    let start = tokio::time::Instant::now();
    match parse_host_port(&target) {
        Some(a) => match tokio::time::timeout(
            Duration::from_secs(monitor.timeout),
            tokio::net::TcpStream::connect(a),
        )
        .await
        {
            Ok(Ok(_)) => {
                let rt = start.elapsed().as_millis() as i64;
                up(rt, "主机可达".into())
            }
            Ok(Err(e)) => down(None, format!("主机不可达: {e}")),
            Err(_) => down(None, "超时".into()),
        },
        None => down(None, format!("无效的主机: {}", monitor.target)),
    }
}

fn parse_host_port(target: &str) -> Option<SocketAddr> {
    // 支持 host:port
    if let Ok(sa) = target.to_socket_addrs() {
        if let Some(a) = sa.into_iter().next() {
            return Some(a);
        }
    }
    None
}

fn up(rt: i64, msg: String) -> CheckResult {
    CheckResult {
        status: "up".into(),
        response_time_ms: Some(rt),
        message: msg,
    }
}

fn down(rt: Option<i64>, msg: String) -> CheckResult {
    CheckResult {
        status: "down".into(),
        response_time_ms: rt,
        message: msg,
    }
}

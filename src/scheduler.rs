//! 后台调度器：按各自间隔并发检测所有启用的监控项

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::chart::ChartStore;
use crate::checker;
use crate::db::Db;
use crate::models::Monitor;

/// 数据保留：3 天（图表内存 + 数据库心跳/事件统一清理）
const RETENTION_HOURS: i64 = 72;
/// 数据库清理周期
const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);

pub struct Scheduler {
    db: Arc<Db>,
    charts: Arc<ChartStore>,
    last_prune: Mutex<Instant>,
}

impl Scheduler {
    pub fn new(db: Arc<Db>, charts: Arc<ChartStore>) -> Self {
        Scheduler { db, charts, last_prune: Mutex::new(Instant::now()) }
    }

    /// 启动调度循环（永不返回）。每个监控项一个独立 task。
    pub async fn run(self: Arc<Self>) {
        let mut running: HashSet<String> = HashSet::new();
        // 定期重新加载监控项，以捕获新增/修改/删除
        loop {
            match self.db.list_active_monitors().await {
                Ok(monitors) => {
                    let active_ids: HashSet<String> = monitors.iter().map(|m| m.id.clone()).collect();
                    for m in monitors {
                        // 只为新出现的监控项启动 task，避免重复检测
                        if running.insert(m.id.clone()) {
                            let sched = self.clone();
                            tokio::spawn(async move {
                                sched.run_one(m).await;
                            });
                        }
                    }
                    // 清理已停用/删除的监控项
                    running.retain(|id| active_ids.contains(id));
                }
                Err(e) => {
                    eprintln!("[scheduler] 加载监控项失败: {e}");
                }
            }

            // 每小时清理一次数据库中超过 3 天的心跳/事件
            // （先在锁内判断并更新时间戳，再释放锁后执行清理，避免 guard 跨 await）
            let should_prune = {
                let mut last = self.last_prune.lock().unwrap();
                if last.elapsed() >= PRUNE_INTERVAL {
                    *last = Instant::now();
                    true
                } else {
                    false
                }
            };
            if should_prune {
                match self.db.prune_old(RETENTION_HOURS).await {
                    Ok(_) => println!("[scheduler] 已清理超过 {RETENTION_HOURS} 小时的历史数据"),
                    Err(e) => eprintln!("[scheduler] 清理历史数据失败: {e}"),
                }
            }

            // 每 10 秒重新扫描一次配置变化
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }

    /// 单个监控项的循环检测
    async fn run_one(&self, monitor: Monitor) {
        loop {
            // 若已被删除或停用，则退出该循环（由外层重新加载）
            let still_active = self
                .db
                .get_monitor(&monitor.id)
                .await
                .map(|m| m.active)
                .unwrap_or(false);
            if !still_active {
                return;
            }

            let result = checker::check(&monitor).await;
            if let Err(e) = self.record(&monitor, &result).await {
                eprintln!("[scheduler] 记录心跳失败 (monitor {}): {e}", monitor.id);
            }

            tokio::time::sleep(Duration::from_secs(monitor.interval)).await;
        }
    }

    /// 写入心跳（数据库 + 内存图表），并在状态发生变化时记录事件
    async fn record(
        &self,
        monitor: &Monitor,
        result: &checker::CheckResult,
    ) -> Result<(), crate::error::AppError> {
        let last = self.db.last_heartbeat(&monitor.id).await?;
        let previous_status = last.map(|h| h.status).unwrap_or_else(|| "pending".into());

        self.db
            .insert_heartbeat(&monitor.id, &result.status, result.response_time_ms, &result.message)
            .await?;
        self.charts.push(&monitor.id, &result.status, result.response_time_ms);

        if previous_status != result.status {
            self.db
                .insert_incident(&monitor.id, &previous_status, &result.status, &result.message)
                .await?;
            println!(
                "[incident] monitor {} ({}) {} -> {}: {}",
                monitor.id, monitor.name, previous_status, result.status, result.message
            );
        }
        Ok(())
    }
}

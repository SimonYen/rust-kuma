# rust-kuma

一个用 Rust 编写的轻量级、自托管服务可用性监控工具。存储层使用 [turso](https://crates.io/crates/turso)（Turso 团队用纯 Rust 从零实现的嵌入式 SQLite 兼容数据库，本地文件存储，零 C 依赖、不依赖任何云服务）。灵感来自 [Uptime Kuma](https://github.com/louislam/uptime-kuma)，但更轻量、零运行时依赖（单二进制）。

## 特性

- **三种监控类型**：HTTP(S)、TCP、Ping（ICMP，以 TCP 可达性近似实现）
- **独立检测间隔**：每个监控项可配置自己的间隔、超时、失败重试次数
- **纯 Rust 嵌入式存储**：监控项、心跳历史、状态变更事件（incident）存入本地 turso 数据库文件，服务器重启数据不丢
- **图表功能**：每个监控项内建响应时间折线图 + 故障区间标记，数据存内存，保留最近 3 天（重启清空）
- **明暗主题**：一键切换，跟随系统偏好，localStorage 持久化
- **管理员鉴权**：仅管理员（凭密码登录）可添加/编辑/删除监控项，游客可查看状态页与图表
- **状态页 + 管理界面**：内置 Web UI，实时展示服务状态、平均响应、24h 可用率
- **REST API**：完整的 CRUD + 状态查询接口
- **轻量**：单二进制发布，内存占用极小

## 快速开始

```bash
# 构建
cargo build

# 运行
cargo run
```

首次运行若未设置 `RUST_KUMA_ADMIN_PASSWORD`，会自动生成一个随机管理员密码并打印在启动日志中。

### 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `RUST_KUMA_DB` | 数据文件路径 | `rust-kuma.db` |
| `DATA_DIR` | 备选：数据目录（`$DATA_DIR/rust-kuma.db`） | 未设置 |
| `RUST_KUMA_ADMIN_PASSWORD` | 管理员密码（不设置则启动时随机生成并打印） | 随机 |
| `RUST_KUMA_ADDR` | 监听地址 | `0.0.0.0:3001` |
| `PORT` | 容器平台注入的监听端口（优先级最高） | 未设置 |

### 发布构建（更小、更快）

```bash
cargo build --release
./target/release/rust-kuma
```

## 使用示例

管理员登录后（或在请求头带 `Authorization: Bearer <token>`）添加监控：

```bash
# 登录获取 token
curl -X POST http://localhost:3001/api/login \
  -H 'Content-Type: application/json' \
  -d '{"password":"管理员密码"}'

# 添加 HTTP 监控
curl -X POST http://localhost:3001/api/monitors \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer <token>' \
  -d '{"name":"我的博客","monitor_type":"http","target":"https://example.com","interval":30,"timeout":10,"retries":1}'

# 添加 TCP 监控
curl -X POST http://localhost:3001/api/monitors \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer <token>' \
  -d '{"name":"数据库","monitor_type":"tcp","target":"db.example.com:5432","interval":30}'
```

## REST API

| 方法 | 路径 | 鉴权 | 说明 |
|------|------|------|------|
| `POST` | `/api/login` | - | 登录，返回内存 token（重启失效） |
| `POST` | `/api/logout` | token | 登出并销毁 token |
| `GET` | `/api/me` | token | 校验 token 是否有效（`{admin: bool}`） |
| `GET` | `/api/monitors` | - | 列出所有监控项（含实时状态） |
| `POST` | `/api/monitors` | **管理员** | 创建监控项 |
| `GET` | `/api/monitors/{id}` | - | 获取单个监控项 |
| `PUT` | `/api/monitors/{id}` | **管理员** | 更新监控项 |
| `DELETE` | `/api/monitors/{id}` | **管理员** | 删除监控项 |
| `GET` | `/api/monitors/{id}/chart` | - | 图表数据（内存，最近 3 天，最多 5000 点） |
| `GET` | `/api/monitors/{id}/heartbeats` | - | 最近 100 条心跳 |
| `GET` | `/api/monitors/{id}/incidents` | - | 最近 100 条状态变化事件 |
| `GET` | `/api/stats` | - | 全局统计（总数/正常/故障/待检测） |

## 项目结构

```
src/
├── main.rs       # 入口：打开数据库 + 启动 HTTP 服务 + 后台调度器
├── models.rs     # 数据模型（Monitor/Heartbeat/Incident/ChartPoint 等）
├── db.rs         # 数据层（turso 嵌入式库，纯 Rust 本地文件）
├── chart.rs      # 内存图表存储（每监控 3 天滑动窗口）
├── auth.rs       # 管理员鉴权（密码 → 内存 token）
├── checker.rs    # 检测器（HTTP/TCP/Ping）
├── scheduler.rs  # 后台调度器（并发检测 + 事件 + 每小时清理 3 天前数据）
├── api.rs        # REST API（axum）+ 鉴权中间逻辑
└── error.rs      # 错误类型
static/
└── index.html    # Web UI（状态页 + 图表 + 明暗主题 + 管理员登录）
```

## 数据表

- `monitors` — 监控项配置（UUID 主键）
- `heartbeats` — 每次检测的心跳记录（状态、响应时间、消息），后台每小时清理 3 天前数据
- `incidents` — 状态变化事件（up/down 切换历史），同样保留 3 天

## 说明

- **存储引擎**：turso 是 SQLite 兼容的纯 Rust 嵌入式数据库（前身项目 Limbo），数据就是一个普通文件 `rust-kuma.db`，可直接备份/复制。如需同步到 Turso Cloud，可给 turso crate 开启 `sync` feature。
- **版本锁定说明**：turso 固定为 `0.9.0-pre.6`。原因是 Rust 1.100（2026-11-12）稳定了 Allocator API，turso 0.8.x 的 `Vec::new_in` 调用会与之类型冲突（[turso#9463](https://github.com/tursodatabase/turso/issues/9463)）；0.8.4 仍是稳定版但尚未包含该修复，故使用 0.9 预览版。后续 turso 发布含修复的稳定版后可放开版本约束。
- **Ping 检测**：为避免引入需要 root 权限的 ICMP raw socket（会破坏"轻量、无特权"的目标），Ping 采用 TCP 可达性近似判断主机是否在线。如需真正的 ICMP ping，可自行扩展 `checker.rs`。
- **目标地址规范化**：HTTP 类型不写协议时自动补 `https://`（`example.com` → `https://example.com`）；TCP/Ping 类型若误填 `http(s)://` 会自动去掉，并去掉尾部 `/`。服务端统一处理，前端失焦时也会自动补全。
- **HTTP 检测行为**：使用浏览器风格 User-Agent、自动跟随最多 10 次跳转，并在跳转过程中**保留 cookie**（兼容 CloudStudio 网关这类「303 到登录页并依赖 set-cookie 放行」的站点）。2xx/3xx 视为正常，4xx/5xx 视为故障并显示状态码。
- **HTTPS**：`monitor_type` 填 `http` 即可，是否走 TLS 由 `target` 的 URL 决定（`https://...` 自动 TLS）。
- **图表数据**：仅存内存（3 天滑动窗口），服务器重启后从零积累；数据库中的心跳/事件也按 3 天保留，由后台调度器每小时清理。
- **Token 安全**：登录 token 存内存，服务重启后所有会话失效，需重新登录。
- 后续可扩展：Webhook / 邮件等告警通知、更多图表维度、多管理员账号。

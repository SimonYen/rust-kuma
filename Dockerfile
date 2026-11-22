# rust-kuma 的多阶段构建 Dockerfile

# 阶段 1：构建 Rust 二进制
# 固定工具链版本，与本地验证过的 1.100 一致（turso 0.9 依赖 1.100 的 Allocator API 行为）
FROM rust:1.100.0-slim AS builder

WORKDIR /app

# 存储层（turso）是纯 Rust，无需编译 SQLite 的 C 源码；
# 但 reqwest 默认使用 native-tls（系统 OpenSSL），编译 openssl-sys 需要
# pkg-config 和 OpenSSL 开发头文件，否则会报 "Could not find openssl via pkg-config"。
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# 先拷贝依赖清单，利用 Docker 层缓存加速重复构建
COPY Cargo.toml Cargo.lock ./
# 创建空的源码占位，让 cargo 能预取并编译依赖
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/deps/rust_kuma*

# 拷贝真实源码并构建
COPY src ./src
COPY static ./static
RUN cargo build --release --locked

# 阶段 2：最小运行时镜像
FROM debian:bookworm-slim AS runtime

# ca-certificates：HTTPS 检测校验证书；libssl3：二进制动态链接的 OpenSSL 运行时
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# 数据目录：数据库文件默认落在 /data（挂载持久卷即可保留数据）
RUN mkdir -p /data
ENV RUST_KUMA_DB=/data/rust-kuma.db

# 拷贝编译好的二进制和静态资源
COPY --from=builder /app/target/release/rust-kuma /app/rust-kuma
COPY --from=builder /app/static /app/static

# 暴露端口（容器平台会注入 PORT 环境变量）
EXPOSE 3000

# 启动：读取 PORT 环境变量（默认 3000）
CMD ["/app/rust-kuma"]

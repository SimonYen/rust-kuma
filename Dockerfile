FROM rust:1-slim AS builder

WORKDIR /app

# 无需安装 C 编译器：turso 是纯 Rust 实现的嵌入式数据库，数据存本地文件

# 先拷贝依赖清单，利用 Docker 层缓存加速重复构建
COPY Cargo.toml Cargo.lock ./
# 创建空的源码占位，让 cargo 能预取依赖
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && rm -rf src target/release/deps/rust_kuma*

# 拷贝真实源码并构建
COPY src ./src
COPY static ./static
RUN cargo build --release

# 阶段 2：最小运行时镜像
FROM debian:bookworm-slim AS runtime

# 安装 CA 证书（HTTPS 检测需要）
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# 拷贝编译好的二进制和静态资源
COPY --from=builder /app/target/release/rust-kuma /app/rust-kuma
COPY --from=builder /app/static /app/static

# 暴露端口（Antideploy 会注入 PORT 环境变量）
EXPOSE 3000

# 启动：读取 PORT 环境变量（默认 3000）
CMD ["/app/rust-kuma"]

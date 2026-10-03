# syntax=docker/dockerfile:1
FROM node:20-slim AS frontend-builder
WORKDIR /app
COPY package*.json ./
RUN --mount=type=cache,target=/root/.npm npm ci --legacy-peer-deps --no-audit --no-fund
COPY tsconfig.json tsconfig.node.json vite.config.ts tailwind.config.js postcss.config.cjs index.html ./
COPY public ./public
COPY src ./src
RUN npm run build

FROM rust:1.96-slim-bookworm@sha256:e18a79fc84dfcfc3ab5ba72290398a644c135c97eaa881447fddc354ee4701a3 AS backend-builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential pkg-config cmake clang libclang-dev perl golang-go git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
ARG BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${BUILD_JOBS}
WORKDIR /app/src-tauri
COPY src-tauri ./
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,target=/app/src-tauri/target \
    cargo build --locked --release --bin antigravity-tools && \
    cp target/release/antigravity-tools /tmp/antigravity-tools

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=backend-builder /tmp/antigravity-tools /app/antigravity-tools
COPY --from=frontend-builder /app/dist /app/dist
# 运行镜像中验证动态链接依赖，缺失库时阻止发布。
RUN ldd /app/antigravity-tools > /tmp/linked-libraries && \
    cat /tmp/linked-libraries && ! grep -q 'not found' /tmp/linked-libraries && \
    rm /tmp/linked-libraries
ARG SOURCE_REVISION=unknown
ARG RELEASE_VERSION=unknown
LABEL org.opencontainers.image.revision="${SOURCE_REVISION}"
LABEL org.opencontainers.image.version="${RELEASE_VERSION}"
LABEL org.opencontainers.image.source="https://github.com/Echo7659/Antigravity-Manager-Lee"
ENV ABV_DIST_PATH=/app/dist RUST_LOG=info PORT=8045 ABV_BIND_LOCAL_ONLY=false
EXPOSE 8045
HEALTHCHECK --interval=30s --timeout=8s --start-period=30s --retries=3 \
    CMD ["/app/antigravity-tools", "--health-check"]
ENTRYPOINT ["/app/antigravity-tools"]

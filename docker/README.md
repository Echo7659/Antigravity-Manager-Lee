# Linux / Web Docker 部署

项目只提供 Linux/amd64 服务镜像和 Web 管理面板。唯一构建入口是 `docker/Dockerfile`，包含 Node 20 Web 构建、Rust 1.96 服务编译及最小运行镜像。

## 发布镜像

镜像名称：`ghcr.io/echo7659/antigravity-manager-lee`。发布 compose 按 digest 固定镜像，不自动跟随 latest。

1. 从目标 commit 的成功 GitHub Actions 记录或发布附件取得 `sha256:...`。
2. 复制 `docker/.env.example` 为 `docker/.env`，填写独立的 `API_KEY`、`WEB_PASSWORD` 与 `IMAGE_DIGEST`。
3. 已有部署填写 `ABV_HOST_DATA_DIR` 为原数据目录绝对路径；新部署默认使用 `docker/data`。
4. 在仓库根目录执行：

```sh
docker compose --env-file docker/.env -f docker/docker-compose.release.yml config --quiet
docker compose --env-file docker/.env -f docker/docker-compose.release.yml pull
docker compose --env-file docker/.env -f docker/docker-compose.release.yml up -d
```

Web 地址为 `http://localhost:8045`。管理接口使用 Web 密码；生成接口使用 API Key 或 User Token。公开部署应在 HTTPS 反向代理后运行，并按需限制管理入口。

发布附件中的 compose 已直接固定镜像 digest；使用附件部署时，在附件目录填写 `.env` 后运行 `docker compose up -d`。

## 本地构建

填写相同的 `docker/.env` 后执行：

```sh
docker compose --env-file docker/.env -f docker/docker-compose.yml up -d --build
```

本地构建默认挂载宿主的 `~/.antigravity_tools`；release compose 默认挂载 `docker/data`。在两者之间切换时必须显式指定同一 `ABV_HOST_DATA_DIR`，避免误用空账号目录。两种 compose 的容器内目录都是 `/root/.antigravity_tools`。

## 配置

| 设置 | 作用 |
| --- | --- |
| `API_KEY` / `ABV_API_KEY` | 生成接口主密钥；后者为服务进程的优先覆盖项 |
| `WEB_PASSWORD` / `ABV_WEB_PASSWORD` | 管理密码；后者优先，未设置管理密码时服务回退至 API Key |
| `ABV_AUTH_MODE` / `AUTH_MODE` | 生成接口鉴权；compose 默认 all_except_health，管理 API 始终鉴权 |
| `IMAGE_DIGEST` | release compose 必填的完整镜像摘要 |
| `ABV_HOST_DATA_DIR` | 宿主持久化目录 |
| `ABV_DATA_DIR` | 服务进程的数据目录；compose 固定为持久化挂载目标 |
| `LOG_LEVEL` | compose 转为服务使用的 RUST_LOG，默认 info |
| `ABV_MAX_BODY_SIZE` | 请求体上限，默认 104857600 字节 |
| `ABV_HEALTH_WATCHDOG_ENABLED` | 健康后台任务开关 |
| `ABV_PROXY_HEALTH_BATCH_SIZE` / `ABV_PROXY_HEALTH_CONCURRENCY` | 代理健康检查批大小 / 并发数 |
| `ABV_PROXY_HEALTH_START_DELAY_SECS` | 健康检查启动延迟，默认 60 秒 |

Compose 通过 8045 端口映射访问服务，容器监听需要 `ABV_BIND_LOCAL_ONLY=false`。直接启动二进制时可将该项设为 true，仅监听回环地址。配置文件端口仍由 `gui_config.json` 中的 proxy.port 决定；这些 compose 要求保持 8045。

启动环境覆盖会写入对应配置字段。Web 保存配置会热更新当前服务，但下次启动仍以已设置的环境覆盖为准。旧配置的未知字段可读取，不应为清理字段而重写已有数据。

## 验证与排错

```sh
docker compose --env-file docker/.env -f docker/docker-compose.release.yml ps
docker compose --env-file docker/.env -f docker/docker-compose.release.yml logs --tail=100
curl --fail http://127.0.0.1:8045/health
```

CI 对无账号临时容器运行 `scripts/ci_smoke.py --base-url http://127.0.0.1:18045 --version 4.9.1`。检查涵盖 health/version、管理接口未鉴权拒绝与已鉴权成功、动态模型目录、静态页面与资源、已删除 API 返回 404。默认测试密码为 ci-smoke-only，可用 `--admin-password` 覆盖。该检查要求账号列表为空，不应对生产数据运行。

镜像构建在最终运行层执行 ldd 检查，缺失动态库时停止构建。容器 health check 运行同一二进制的 `--health-check`。HTTP 健康不代表真实上游请求成功，账号资格、模型请求与费用统计需单独验收。

## 升级与回滚

升级前记录当前容器的镜像 ID/digest，并保留旧镜像。制作一致的数据备份时先停止服务，再备份完整挂载目录；SQLite 文件、账号 JSON、凭据、代理绑定及配置应成套保留。

只将 `IMAGE_DIGEST` 替换为目标 commit 对应的成功 CI digest，再执行 pull 与 up。保留原端口、数据挂载和密钥。升级后核对账号数量、代理绑定、模型目录与 Token 统计。

回滚时恢复旧 `IMAGE_DIGEST` 并执行 up，继续使用原数据挂载和环境。发布包里的固定 image 则直接恢复旧 digest 引用。不要执行删除数据卷、清空数据目录或重新初始化账号的操作。

Compose 配置了日志轮转：本地构建每文件 100 MB，release 每文件 50 MB，均保留 3 个文件。数据库中的日志保留策略独立于 Docker 日志轮转。

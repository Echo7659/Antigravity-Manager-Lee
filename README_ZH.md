# Antigravity Manager Lee (v4.9.1)

面向 Linux 服务器的 Rust/Axum 服务与 React Web 管理面板。支持 OpenAI Responses、OpenAI Chat Completions、Anthropic Claude 和 Google Gemini 四种协议，统一转换到 Gemini 流水线处理。

[简体中文](README_ZH.md) · [English](README_EN.md) · [Actions](https://github.com/Echo7659/Antigravity-Manager-Lee/actions) · [Releases](https://github.com/Echo7659/Antigravity-Manager-Lee/releases)

## 核心功能

- 账号 JSON 导入导出、浏览器 OAuth、配额刷新和健康状态管理。
- 配额保护、按重置时间排序、周配额预留；单次生成最多尝试六个不同账号。
- `/api/proxy/models` 提供动态模型目录，包含 24 小时新鲜度与 last-known-good 回退。
- 账号代理绑定、proxy pool 与运行中配置热更新。
- 调用记录、调试日志、Token 和成本统计、User Token，以及独立的管理鉴权。
- 同一 HTTP 服务提供 Web、管理 API 和生成接口。停止 API Proxy 后，管理功能与模型目录仍然可用。

## Docker 部署

发布镜像为 **Linux/amd64**，名称固定为 `ghcr.io/echo7659/antigravity-manager-lee`。从目标 commit 的成功 CI 记录中取得精确镜像 digest。

复制 `docker/.env.example` 为 `docker/.env`，填写 `API_KEY`、`WEB_PASSWORD` 和 `IMAGE_DIGEST=sha256:...`。已有部署必须先将 `ABV_HOST_DATA_DIR` 指向原有数据目录的绝对路径。

```sh
docker compose --env-file docker/.env -f docker/docker-compose.release.yml pull
docker compose --env-file docker/.env -f docker/docker-compose.release.yml up -d
```

通过 `http://localhost:8045` 打开 Web 管理面板。客户端按协议配置服务地址与 API Key，模型 ID 以动态目录为准。配置、本地构建、升级和回滚详见 [Docker 部署指南](docker/README.md)。

## 持久化与安全

容器数据挂载保持 `/root/.antigravity_tools`，保存账号 JSON、凭据、`gui_config.json`、代理绑定、SQLite 日志与统计。升级时保留原宿主路径和密钥，不以空目录替代已有数据目录。

管理 API 使用 Web 管理密码，生成接口使用 API Key 或 User Token。环境变量在启动时覆盖相应配置；Web 保存配置同时更新磁盘与常驻状态。对外访问时通过 HTTPS 反向代理并限制管理入口访问范围。

升级前记录旧镜像 digest 并保留一致的数据备份；回滚时恢复旧 `IMAGE_DIGEST`，使用原挂载和环境重新创建容器。相关行为见 [账号重试](docs/ACCOUNT_FAILOVER.md)、[配额保护](docs/QUOTA_AND_STABILITY.md) 与 [上游同步说明](docs/UPSTREAM_4_8_1.md)。

## 开发与验证

后端为保持仓库路径兼容继续位于 `src-tauri/`，使用 Rust 1.96、edition 2024；Web 使用 Node 20。

```sh
npm ci --legacy-peer-deps
npm run test:dashboard
npm run build
bash scripts/check_server_only.sh
cargo +1.96.0 fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo +1.96.0 check --locked --manifest-path src-tauri/Cargo.toml --all-targets
cargo +1.96.0 clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets --all-features
```

CI 还执行状态、配置及协议兼容性定向测试，并在 Linux 镜像完成 health、鉴权、模型目录、Web 资源和删除 API 的 404 检查后发布 digest。应用版本与历史贡献者署名保持不变。

## 📝 更新日志

> 最新版本 **v4.9.1**（2026-10-02）：修复 `gemini-3.1-flash-lite` 误重定向至已故 2.5 系列导致 503 报错的严重问题并恢复健康直传，将后台摘要压缩任务重定向至存活轻量模型，从公开目录清理 2.5 全系列并平滑重定向至 `gemini-3.6-flash-medium`（Fixes #3577，感谢 @Xyloz3n）；收紧下游 SSE 流式思考心跳至 3 秒防止长推理提前断开连接（PR #3578，感谢 @EricZhou05）。

👉 **[查看完整更新日志 CHANGELOG.md →](CHANGELOG.md)**

<details>
<summary><b>👥 核心贡献者 (Contributors) - 点击展开</b></summary>

<a href="https://github.com/lbjlaq"><img src="https://github.com/lbjlaq.png" width="50px" style="border-radius: 50%;" alt="lbjlaq"/></a>
<a href="https://github.com/jeikl"><img src="https://github.com/jeikl.png" width="50px" style="border-radius: 50%;" alt="jeikl"/></a>
<a href="https://github.com/XinXin622"><img src="https://github.com/XinXin622.png" width="50px" style="border-radius: 50%;" alt="XinXin622"/></a>
<a href="https://github.com/llsenyue"><img src="https://github.com/llsenyue.png" width="50px" style="border-radius: 50%;" alt="llsenyue"/></a>
<a href="https://github.com/salacoste"><img src="https://github.com/salacoste.png" width="50px" style="border-radius: 50%;" alt="salacoste"/></a>
<a href="https://github.com/84hero"><img src="https://github.com/84hero.png" width="50px" style="border-radius: 50%;" alt="84hero"/></a>
<a href="https://github.com/karasungur"><img src="https://github.com/karasungur.png" width="50px" style="border-radius: 50%;" alt="karasungur"/></a>
<a href="https://github.com/marovole"><img src="https://github.com/marovole.png" width="50px" style="border-radius: 50%;" alt="marovole"/></a>
<a href="https://github.com/wanglei8888"><img src="https://github.com/wanglei8888.png" width="50px" style="border-radius: 50%;" alt="wanglei8888"/></a>
<a href="https://github.com/yinjianhong22-design"><img src="https://github.com/yinjianhong22-design.png" width="50px" style="border-radius: 50%;" alt="yinjianhong22-design"/></a>
<a href="https://github.com/Mag1cFall"><img src="https://github.com/Mag1cFall.png" width="50px" style="border-radius: 50%;" alt="Mag1cFall"/></a>
<a href="https://github.com/AmbitionsXXXV"><img src="https://github.com/AmbitionsXXXV.png" width="50px" style="border-radius: 50%;" alt="AmbitionsXXXV"/></a>
<a href="https://github.com/fishheadwithchili"><img src="https://github.com/fishheadwithchili.png" width="50px" style="border-radius: 50%;" alt="fishheadwithchili"/></a>
<a href="https://github.com/ThanhNguyxn"><img src="https://github.com/ThanhNguyxn.png" width="50px" style="border-radius: 50%;" alt="ThanhNguyxn"/></a>
<a href="https://github.com/Stranmor"><img src="https://github.com/Stranmor.png" width="50px" style="border-radius: 50%;" alt="Stranmor"/></a>
<a href="https://github.com/Jint8888"><img src="https://github.com/Jint8888.png" width="50px" style="border-radius: 50%;" alt="Jint8888"/></a>
<a href="https://github.com/0-don"><img src="https://github.com/0-don.png" width="50px" style="border-radius: 50%;" alt="0-don"/></a>
<a href="https://github.com/dlukt"><img src="https://github.com/dlukt.png" width="50px" style="border-radius: 50%;" alt="dlukt"/></a>
<a href="https://github.com/Silviovespoli"><img src="https://github.com/Silviovespoli.png" width="50px" style="border-radius: 50%;" alt="Silviovespoli"/></a>
<a href="https://github.com/i-smile"><img src="https://github.com/i-smile.png" width="50px" style="border-radius: 50%;" alt="i-smile"/></a>
<a href="https://github.com/jalen0x"><img src="https://github.com/jalen0x.png" width="50px" style="border-radius: 50%;" alt="jalen0x"/></a>
<a href="https://linux.do/u/wendavid"><img src="https://linux.do/user_avatar/linux.do/wendavid/48/122218_2.png" width="50px" style="border-radius: 50%;" alt="wendavid"/></a>
<a href="https://github.com/byte-sunlight"><img src="https://github.com/byte-sunlight.png" width="50px" style="border-radius: 50%;" alt="byte-sunlight"/></a>
<a href="https://github.com/jlcodes99"><img src="https://github.com/jlcodes99.png" width="50px" style="border-radius: 50%;" alt="jlcodes99"/></a>
<a href="https://github.com/Vucius"><img src="https://github.com/Vucius.png" width="50px" style="border-radius: 50%;" alt="Vucius"/></a>
<a href="https://github.com/Koshikai"><img src="https://github.com/Koshikai.png" width="50px" style="border-radius: 50%;" alt="Koshikai"/></a>
<a href="https://github.com/hakanyalitekin"><img src="https://github.com/hakanyalitekin.png" width="50px" style="border-radius: 50%;" alt="hakanyalitekin"/></a>
<a href="https://github.com/Gok-tug"><img src="https://github.com/Gok-tug.png" width="50px" style="border-radius: 50%;" alt="Gok-tug"/></a>
<a href="https://github.com/johngbl"><img src="https://github.com/johngbl.png" width="50px" style="border-radius: 50%;" alt="johngbl"/></a>

感谢所有为本项目付出汗水与智慧的开发者。

</details>

<details>
<summary><b>🤝 鸣谢项目 (Special Thanks) - 点击展开</b></summary>

本项目在开发过程中参考或借鉴了以下优秀开源项目的思路或代码，排名不分先后：

*   [learn-claude-code](https://github.com/shareAI-lab/learn-claude-code)
*   [Practical-Guide-to-Context-Engineering](https://github.com/WakeUp-Jin/Practical-Guide-to-Context-Engineering)
*   [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)
*   [OmniRoute](https://github.com/diegosouzapw/OmniRoute)
*   [antigravity-claude-proxy](https://github.com/badrisnarayanan/antigravity-claude-proxy)
*   [aistudio-gemini-proxy](https://github.com/zhongruichen/aistudio-gemini-proxy)
*   [gcli2api](https://github.com/su-kaka/gcli2api)
*   [agent-vibes](https://github.com/funny-vibes/agent-vibes)

</details>

*   **版权许可**: 基于 **CC BY-NC-SA 4.0** 许可，**严禁任何形式的商业行为**。
*   **安全声明**: 账号文件与 SQLite 数据保存在服务器挂载目录；请求及 OAuth 凭据按功能需要发送至配置的上游服务。

---

<div align="center">
  <p>如果您觉得这个工具有所帮助，欢迎在 GitHub 上点一个 ⭐️</p>
  <p>Copyright © 2024-2026 Antigravity Team.</p>
</div>

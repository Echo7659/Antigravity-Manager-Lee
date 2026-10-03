# Antigravity Manager Linux/Web 核心化设计

**日期：** 2026-10-04  
**状态：** 已批准

## 目标

将当前同时承载 Tauri 桌面应用和 Linux headless 服务的项目，精简为只面向 Linux 服务器部署的纯 Rust 服务与 Web 管理面板。删除桌面、本机工具集成、Cloudflared 和“中转站”推广功能，同时保留账号调度、API 转换、统计和安全等核心能力，并继续完成 Opus 5.5 与动态模型目录优化。

## 决策

采用“原目录硬拆除”方案：保留 `src-tauri/` 路径以避免无业务价值的大规模路径迁移，但彻底移除 Tauri 运行时、宏、插件和桌面专属代码。后端改为单一的 `Axum + Tokio` Linux 服务。

不采用以下方案：

- 不重命名 `src-tauri/` 为 `server/`，避免扩大 Docker、CI 和历史脚本的纯路径改动。
- 不以 feature flag 保留桌面版本，避免继续承担桌面依赖、条件编译和双运行时维护成本。

## 目标架构

### 单一服务进程

后端二进制启动后依次初始化数据目录、日志、配置、账号快照、TokenManager、SQLite、后台调度器和 Axum。Axum 同时提供：

- Web 管理面板静态资源；
- 管理 API `/api/*`；
- OpenAI、Claude、Gemini 等兼容生成接口；
- 健康检查。

项目不再包含桌面窗口、托盘或 Tauri command。前端只通过 HTTP 调用后端。

### 单一运行时状态

配置、账号和 TokenManager 只存在一套常驻服务器状态。配置保存必须同时完成原子写盘与内存热更新。模型映射、`only_raw_quota_models`、代理池等配置不得在磁盘状态、桌面状态与服务器状态之间分叉。

`/api/proxy/models` 是唯一动态模型目录接口，直接读取常驻 TokenManager。逻辑上的 API Proxy 停止时，管理服务与模型目录仍可用；不得为查询目录临时加载账号、启动代理或触发账号/config 写盘。

### 前端数据流

前端删除 Tauri 环境探测与 command 分支，统一使用 `/api/*`。保存模型映射或 raw-only 配置后显式刷新模型目录，不轮询。浏览器上传和下载替代系统文件对话框。

## 保留的核心能力

- 账号导入、OAuth 登录、刷新、删除、排序与状态管理；
- 配额刷新、配额保护、健康度与重置时间排序、周配额预留；
- 每次生成请求最多尝试 6 个不同账号，失败账号不在同一请求中重用；
- 账号代理绑定、代理池和热更新；
- OpenAI、Claude、Gemini 协议转换与模型路由；
- 24 小时新鲜动态模型目录及 last-known-good 回退；
- 调用记录、调试日志、Token 与成本统计；
- User Token、管理鉴权和安全配置；
- 服务器配置与 Web 管理面板；
- SQLite 与现有账号 JSON、配置和代理绑定格式。

## 删除范围

### 桌面与本机功能

- Tauri command、窗口、托盘、Mini View、轻量模式、桌面自启动和桌面更新器；
- Tauri 配置、capabilities、安装器、桌面图标构建和桌面发布流程；
- `@tauri-apps/*` 前端依赖和所有环境分支；
- GTK、WebKit、AppIndicator 及 Tauri 插件 Rust 依赖；
- macOS、Windows 与 Linux GUI 兼容代码；
- 文件选择、打开本机目录、桌面数据目录迁移界面；
- Codex、OpenCode、Hermes、OpenClaw 等本机配置同步及其 Web 卡片、弹窗和 API；
- Cloudflared 安装、启动、停止、状态和设置。

账号导入导出如仍属于核心流程，改用标准浏览器上传和下载，不保留系统对话框兼容层。

### “中转站”推广功能

删除以下内容：

- `/apikey-fun` 页面与路由；
- 导航栏中的“中转站”菜单；
- 设置页中的对应菜单显隐项；
- 页面专属翻译、localStorage 数据、类型、测试和素材；
- README 中对应的 APIKEY.FUN 推广内容。

“中转站”删除不涉及核心 API Proxy，也不删除 `proxy_pool` 代理池和账号代理绑定。

## 配置与数据兼容

- 不删除、迁移或重建生产账号、凭据、SQLite 日志与统计库、代理绑定或配置文件。
- 旧配置中的桌面、本机集成和 Cloudflared 字段可以被新版本忽略；不为清理字段主动重写生产配置。
- 配置反序列化保持向后兼容。删除的功能不得阻止旧配置启动。
- OAuth 继续通过 Web 管理面板和服务器回调完成。
- 删除的 API 不保留假成功兼容层；旧路径返回标准 404。

## 错误处理与安全

- 管理 API 继续受 admin 鉴权保护；健康检查保持最小公开范围。
- 管理操作返回统一 JSON 错误；前端展示真实错误。
- API Proxy 停止只影响生成端点，账号、配置、模型目录、日志、统计和安全管理保持可用。
- 目录查询和只读管理操作不得修改账号或配置文件。
- Rust 项目使用 edition 2024，不新增外部数据库、任务服务或不必要的运行时依赖。

## 实施顺序

1. 审计当前未提交的 Task 3 修复，只保留纯 Web/Axum 仍需要的部分。
2. 建立纯服务器启动入口和常驻状态，确保核心 HTTP API 覆盖现有 Web 调用。
3. 将前端统一到 HTTP，补齐浏览器导入导出并删除桌面分支。
4. 删除 Tauri、桌面、本机集成、Cloudflared 与“中转站”代码和依赖。
5. 精简 Docker、CI 和文档，只构建 Linux 服务与 Web 镜像。
6. 独立审查精简结果。
7. 在新架构上继续 Opus 5.5、付费 Pro/Ultra 资格、adaptive thinking 和成本统计。
8. 推送代码，等待 CI，使用精确 GHCR digest 部署生产并保留旧 digest 回滚。

## 验证标准

### 静态边界

- `cargo tree` 不含 Tauri、GTK、WebKit、AppIndicator 或桌面插件。
- 前端依赖与源码不含 `@tauri-apps/*`。
- 不存在 `/apikey-fun` 路由、页面、菜单或专属翻译。
- 不存在本机工具同步、Cloudflared、桌面更新、托盘和自启动 API。

### 自动化验证

- Rust 1.96：format、check、Clippy、核心测试和兼容测试通过；
- 前端：dashboard 测试和 production build 通过；
- HTTP 契约：账号、代理启停、动态模型、日志、Token 统计、代理池和鉴权通过；
- 删除路径返回 404；
- Docker 构建与容器 health check 通过；
- edition 2024 编译通过。

### 生产验收

- 只部署 GitHub CI 通过的精确 commit 和 GHCR digest；
- 部署前记录旧 digest 和回滚方式；
- 使用原数据挂载启动，不修改账号、配置或 SQLite 数据；
- 核对账号数量、代理配置、模型目录、健康状态、日志和 Token 统计；
- 对 Opus 5.5 做资格、请求协议和计费统计验收；
- 自动化检查完成后由用户进行最终人工测试。

## 非目标

- 不重命名 `src-tauri/` 目录；
- 不重做 Web 视觉设计；
- 不删除代理池或账号代理绑定；
- 不清理生产数据；
- 不增加第二套配置、模型目录或后台任务系统；
- 不保留可构建的桌面兼容模式。

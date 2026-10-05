# Opus 5.5 调度与运行时稳定性设计

## 背景

生产容器出现过 HTTP 无响应但进程仍在的假死。日志表明假死紧跟在 `invalid_grant` 连续失败并停用账号之后。源码中 `invalid_grant_failures` 的 DashMap 写入 guard 跨越了 `disable_account(...).await`，后续又在同一 DashMap 上执行 `remove`，会产生自锁。

同时，Opus 5.5 请求会返回 `No accounts available with quota for model: claude`。线上账号数据显示，唯一明确拥有 `claude-opus-5-5-low/medium/high` 配额的 Ultra 账号仍有剩余配额，但它因其他 Gemini 模型进入配额保护而被整个账号排除。当前调度将“账号保护了任意模型”错误解释为“账号不能用于任意请求”。

## 目标

1. 消除 `invalid_grant` 停用路径上的 DashMap 自锁，使单个账号凭据失效不再拖垮整个网关。
2. Opus 5.5 只从当前物理变体有正配额的 Ultra 或有付费证据的 Pro 账号调度。
3. 配额保护按当前请求模型生效；不相关的 Gemini 保护不得阻断 Claude/Opus 请求。
4. 将 headless 健康监控从 Tokio 运行时分离；当 Tokio 工作线程整体饿饿或阻塞时，监控仍能退出进程，让 Docker 的 `unless-stopped` 执行重启。
5. 先在 `beta` 分支完成测试、构建和生产更新，不改写 stable 更新通道或 `latest` 标记。

## 非目标

- 不改造整个账号调度器。
- 不改变 Opus 5.5 以外模型的 tier 资格规则。
- 不自动解除任何已有配额保护或账号停用状态。
- 不在本次修复中处理 npm 依赖审计报告中的无关漏洞。

## 方案选择

| 方案 | 做法 | 优点 | 代价 | 结论 |
| --- | --- | --- | --- | --- |
| 模型感知的通用调度 | 按请求的物理模型判定资格、正配额和保护 | 修复根因，覆盖后续模型，不牺牲其他模型保护 | 需要系统更新调度过滤调用点 | 采用 |
| Opus 5.5 特判 | 只在 Opus 5.5 分支忽略其他保护 | 改动小 | 规则重复，新模型会再次出错 | 不采用 |
| 关闭配额保护 | 全局关闭 quota protection | 可快速恢复调度 | 会绕过用户设定的配额安全线 | 不采用 |

## 设计

### 1. 目标模型感知的配额保护

将“账号是否被保护”改为“账号对当前目标模型是否被保护”。判定流程如下：

1. 配额保护未启用时始终返回 false。
2. 将目标模型按现有 model mapping 归一化为保护键，例如 Claude 家族归一化为 `claude`。
3. 只在 `protected_models` 包含该保护键时排除账号。
4. 对没有目标模型上下文的管理性选择，保留当前全账号保守语义，避免放大未知路径的行为变化。

调度中的绑定会话、首选账号、轮询候选、动态能力过滤和配额选择都必须传入当前目标模型，保证所有入口语义一致。

### 2. Opus 5.5 账号资格

Opus 5.5 的账号可用性同时满足：

- tier 是 Ultra，或 tier 是 Pro 且 `is_paid_subscription = true`；
- 模型 alias 经 canonicalization 和路由解析后，得到具体的 `low` / `medium` / `high` 物理模型；
- `exact_model_quotas[physical_model] > 0`；
- 该物理模型对应的保护键没有进入配额保护；
- 账号没有处于现有的 cooldown、validation block、disabled 或请求内排除状态。

不再仅检查 `exact_model_quotas` 是否存在键，值为 0 的配额必须视为不可用。其他 Claude 模型继续使用当前通用能力规则。

### 3. `invalid_grant` 锁生命周期

对 DashMap 失败计数的更新只在同步作用域内完成，并立即复制出 `current_fails`。离开作用域后 guard 必须已释放，才能执行：

- `disable_account(...).await`；
- 删除失败计数；
- 放弃 session 绑定等后续逻辑。

这个修复不改变“连续两次 `invalid_grant` 才停用”的业务语义。

### 4. 独立 headless watchdog

当 `ABV_HEALTH_WATCHDOG_ENABLED` 启用时，watchdog 在独立 OS 线程中运行，不依赖主 Tokio runtime 的调度。watchdog 用有界的阻塞 HTTP 健康检查访问本地端口，保持现有节奏：启动后宽限 30 秒，每 15 秒检查一次，连续 3 次失败时以非零状态退出。

正常收到 SIGINT/SIGTERM 时，主运行时通知 watchdog 结束并回收线程。watchdog 的等待使用可中断机制，不得让正常关停额外等待 15 秒。`--health-check` 命令保持原有异步检查实现。

### 5. 错误与可观测性

候选账号被全部过滤时，日志必须区分“没有此模型能力或正配额”与“账号对此模型进入保护/刷新”。对外仍使用现有结构化网关错误，避免破坏四种协议适配器。日志只记录模型和计数，不输出 token、refresh token 或完整账号文件。

## 测试设计

### 调度与 Opus 5.5

- Ultra 账号可调度 `low` / `medium` / `high` 中存在且大于 0 的物理变体。
- 有付费证据的 Pro 账号可调度；Free 和无付费证据的 Pro 不可调度。
- 请求 `low` 时，只有 `high` 配额的账号不可调度。
- 物理变体配额为 0 时不可调度。
- 账号只保护 Gemini 模型时，Opus 5.5 仍可调度。
- 账号保护 `claude` 时，Opus 5.5 被排除。
- 现有的会话绑定、首选账号和轮询入口都使用同一目标模型保护判定。

### 死锁与 watchdog

- 模拟第二次 `invalid_grant` 的处理，在有界超时内完成停用和计数删除。
- 通过注入可控的 probe 和短间隔验证 watchdog 连续失败计数、正常恢复清零和关停可中断。
- 测试不直接调用 `std::process::exit`；将“达到退出条件”与“真正退出进程”分开验证。

## 验证与发布

1. 运行触及模块的定向 Rust 测试。
2. 运行 `cd src-tauri && cargo fmt -- --check`。
3. 运行 `cd src-tauri && cargo clippy --all-targets --all-features`。
4. 构建独立 Beta Docker 镜像，使用不覆盖 stable/`latest` 的标签。
5. 在更新服务器前记录当前容器镜像 ID、配置和健康状态，保留原镜像用于回滚。
6. 更新后分别验证容器运行状态、`/health`、`/v1/models`、Opus 5.5 真实请求及持续日志。HTTP 200 或容器 `healthy` 不单独作为 Opus 5.5 业务成功证据。
7. 若新版本无法通过健康或真实请求验收，立即回滚到记录的原镜像和容器配置。

## 发布边界

- 本次变更仅进入 `beta`，不合并 `main`。
- 不删除账号数据、配置、卷或原镜像。
- 服务器凭据仅使用用户指定的本地私钥路径，不读取、回显或写入仓库。
- 生产更新只替换 `antigravity-manager` 容器的应用镜像，保留现有挂载、端口和环境变量。

# Gemini 兼容性与流式稳定性补丁

本补丁基于 4.7.1。程序版本号继续使用 4.7.1，定制镜像标签为 `antigravity-manager:4.7.1-compat-20260914`，镜像 revision 标签记录源代码提交。

## 请求兼容

- 顶层 `top_p` 转为 `generationConfig.topP`。原生 `topP` 已存在时保留原值，并移除多余顶层别名。
- 普通无角色输入补为 `user`，模型工具调用补为 `model`，工具结果补为 `user`。
- 明确声明的角色、工具参数、思考签名保持不变；角色冲突或非法参数交由正常校验处理。

## 用量与流式响应

- Gemini 思考 token 计入 OpenAI 完成 token；借助原始总量区分已合并思考的旧格式，避免重复计数。
- 合并分段累计用量，后续缺失字段和占位零值不覆盖已有数据。
- 处理分片 UTF-8 和没有终止换行的末尾 SSE 帧，保留最后一段回复及用量。
- OpenAI Chat 流只有思考、没有正文或工具调用时，返回结构化 `empty_response`，而不是空白成功。已经输出的流不盲目重放。
- Gemini 非流式聚合遇到空回复或流错误，使用现有有限重试次数重新尝试；原生流式错误和超时采用 Gemini 错误结构。
- 请求日志保留流内错误，不再仅根据已经发出的 HTTP 200 响应头判定成功。
- 上游明确拒绝的结果保持原语义，不作为空回复自动重试。上游未提供用量时不伪造计数；历史统计不自动重算。

## 构建与部署

`docker/Dockerfile.compat` 固定官方 4.7.1 前端/运行镜像及 Rust 构建镜像摘要，仅重新编译后端。编译默认使用 4 个任务，`BUILD_JOBS` 可覆盖。

可在项目根目录使用原 Compose 与兼容覆盖文件构建：

```sh
docker compose -f docker/docker-compose.yml -f docker/docker-compose.compat.yml build
docker compose -f docker/docker-compose.yml -f docker/docker-compose.compat.yml up -d
```

部署前须沿用自己的端口、网络、环境变量和数据挂载配置。升级上游版本时，需要同步调整官方运行镜像摘要；不可让新版后端长期复用不匹配的前端。

请求调度属于持久化配置，不硬编码到默认源代码。需要主动分散流量时，可在管理界面选择 `PerformanceFirst`。它跳过会话粘性和 60 秒复用，仍保留 P2C 选择、配额保护及重试上限，不能保证消除上游真实限流。

## 回归测试

在具备项目原生构建依赖的环境中运行：

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib request_compat
cargo test --manifest-path src-tauri/Cargo.toml --lib compat_
```

测试覆盖原生参数优先、多轮角色、工具往返、异常输入、思考计数、分段用量、无换行尾帧、空回复、工具/媒体输出以及明确拒绝响应。部署后还应验证真实 Gemini/OpenAI 请求、账号完整性和用量返回。

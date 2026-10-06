# ThinkingStore 内存边界修复计划

用户已授权 beta 修复、测试、推送及直接更新生产，并明确不需要数据库备份。本计划延续稳定性优化，不撤销已上线的 beta.2。

## Global Constraints
工作目录：/Users/lee/.codex/worktrees/antigravity-opus55-stability-beta/反重力
分支：codex/thinking-cache-bounds，基线 origin/beta 0b1973397bf050a0fa6300cebcac661cd14b5a13。
Rust edition 2024，中文工程注释。仅落实 ThinkingStore 既有缓存限制，不调整用户 retention/max_turns 配置，不新增全局缓存架构，不修改协议 adapter。SQLite 保留完整历史，RAM 拒收或淘汰不能引发持久记录删除或漏存。主控负责发布和生产；子任务不得推送、操作生产或修改其他任务文件。测试使用 cargo +1.96.0 --locked，不为既有 warnings 扩大范围。

## Task 1: 有界历史读取与统一缓存限额
### 范围
修改 src-tauri/src/modules/proxy_db.rs、src-tauri/src/proxy/thinking_store.rs 及其中聚焦测试。先阅读 /tmp/antigravity-incident-20261006/memory-boundary-review.md 作为源码审查证据；以下要求是实现边界，不要求照搬报告中的所有可选建议。

### 要求
1. 所有 RAM 写入入口（append、merge、ingest upgrade、L2 hydrate、restore 点查回填）统一遵守 max_turns_per_session()、MAX_BYTES_PER_SESSION=64MiB 和 MAX_SESSIONS=2000。保持现有 record_bytes 的 thought/signature/visible 内容字节语义，修复重复计量及 purge 后不一致。保留最新完整记录，不截断 thought/signature。明确内容预算不等同于进程 RSS 限额。
2. 原本需要持久化的新 record 与 latest upgrade 的保存独立于 RAM 接纳，单条超限仍完整持久化，RAM 可以拒收。保留原有 ingest 保存边界：非 latest 的历史 stronger upgrade 仍只更新 RAM；本补丁不将它追加为新历史版本、不回溯覆盖旧行，也不改变历史匹配顺序。RAM 裁剪不得驱动 SQLite 删除；审查上下文 prune 与部分缓存组合，不能把未加载历史当作应该删除的数据。保持显式业务 prune 的正确语义。
3. L2 批量加载只读取至多配置轮数的最新有限候选窗口，使用现有 session/id 索引，累计内容字节有界，最后按原时间顺序返回。不先全量加载再裁剪。遇超限行跳过整条并保留 DB；候选窗口不能因跳过而无界延长。
4. AGZ1 解压必须限制输出读取至剩余预算+1 后判超限；RAW1、legacy text 和 lossy UTF-8 也须检查实际输出限额。输入 blob/text/signature/visible/tool 等字段在 row.get 大量复制前检查长度/使用受限读取；不得先完整分配再拒绝。有效超限 gzip 不能作为损坏数据走原始文本 fallback。
5. signature/tool/fingerprint 点查都应用单行预算，restore 请求局部 records Vec 的补充也受限。采用初始缓存快照加独立补充预算：补充最多现有 turns/64MiB，允许从冷窗口外恢复必要历史；缓存回填仍执行单 session 限额。记录其瞬时内容上界可接近两个单 session 预算加当前请求/分配开销，不能宣称整个请求仅占64MiB。
6. 所有创建 session 的路径通过短临界区统一 admission/淘汰，严格不超过 MAX_SESSIONS（包含并发）。临界区不得包含 DB I/O、解压、持久化；进入前释放 DashMap guard，避免锁重入。L2 查询可先有界读取再接纳。不新建全局 byte-cap/LRU 管理器。
7. ingest upgrade 在加载/裁剪/并发后不能依赖可能漂移的索引更新错误记录，核对记录身份。保持空 session hydrate 后新 capture 可见的回归语义；若超限空结果会重复读取，至少确保重复工作有界，不添加永久负缓存导致漏恢复。超限通过简洁日志/计数可诊断，不输出内容或凭据。
8. 保留 beta.2 的 DB blocking/维护边界、retention/清理节奏和 watchdog 行为，禁止为内存问题重新全量扫描或删除数据库。

### 必需测试与提交
先用最小 fixture 证明旧代码的冷加载限额缺口 RED，再实现 GREEN。使用可注入小预算测试避免无必要的大内存压力。
- 最新 N 条、顺序正确、累计字节限制、DB 历史不变，窗口外历史仍可有界点查恢复。
- AGZ1 高压缩比、RAW1、legacy、非 thought 大字段及 lossy UTF-8：拒收整条、无超限 fallback；解码上限可验证。
- append/merge/upgrade/hydrate/point-fetch/purge 后预算和计量一致；超大新记录仍保存 SQLite。
- 请求点查补充累计有界；RAM trim/eviction/prune 不误删未加载持久记录。
- 仅 restore 的连续及并发 session admission 不超过上限、无死锁；空 hydrate 后 capture、历史工具签名回归通过。
运行 proxy_db、thinking_store 聚焦测试与 fmt；全 Rust suite 由主控在最终候选上串行执行一次，clippy/CI 由主控汇总。自审后提交单一独立可回滚修复，报告 RED/GREEN 命令和结果、变更/风险。

## Task 2: 发布与验收
主控执行。完成任务审查及最终分支审查后，npm run bump beta 升到 beta.3，同步双语 changelog 和归属。运行准确候选提交 fmt、clippy --all-targets --all-features、Rust suite 串行及受影响版本检查。创建 beta PR、附加 artifact，CI 成功后合并并打匹配 beta tag。main/latest 不变。
等待 beta.2 已启动的62分钟观察结束，保留小时维护后可用性证据。beta.3 使用验证过的镜像 digest 更新，保留旧镜像/环境文件回滚，不备份数据库。验证 health、Web、账号/模型目录与配置不漂移、一笔最小真实请求、维护和内存同口径采样。新内存限额不能替代长期负载或全局 RSS 保证；最终报告明确已验证范围。

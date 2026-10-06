# ThinkingStore 稳定性实施计划
> 执行方式：subagent-driven-development。用户已批准 beta 推送和生产更新。

## Global Constraints
工作目录：/Users/lee/.codex/worktrees/antigravity-opus55-stability-beta/反重力
分支：codex/thinking-cleanup-stability，基线 origin/beta 282c65b3ecad6fa78dc93a9fbaeef04b5557329e。
Rust edition 2024，中文工程注释。仅修复本次存储维护/自动重试引发的稳定性问题，不改协议行为、用户数据保留配置或 watchdog 阈值。主控负责推送、PR、发布及生产操作。子任务不得操作生产或推送。不得修改其他任务文件。提交前聚焦测试，已有 95 个基线 warnings 不作为扩大改动范围的理由。使用 cargo +1.96.0 --locked。报告保存实际测试命令与输出摘要。

## Task 1: ThinkingStore 阻塞边界和有界维护
### 范围
修改 src-tauri/src/modules/proxy_db.rs、src-tauri/src/proxy/monitor.rs、src-tauri/Cargo.toml，必要时 runtime.rs 或 thinking_store.rs 的测试。避免无关格式化。设计参考 docs/superpowers/specs/2026-10-06-thinking-storage-stability-design.md；独立审查报告 /tmp/antigravity-incident-20261006/storage-design-review.md 文末为最终推荐。

### 实现要求
1. 保留共享 THINKING_DB 连接，统一 with_thinking_db 闭包入口包住路径查找、锁等待、首次连接和 SQL。MultiThread runtime 使用 block_in_place，普通线程/CurrentThread 直接执行；guard/statement 不逃逸，不嵌套取得相同锁。覆盖所有生产用 thinking_db 调用。不能只包清理，前台同步锁等待才是 runtime 耗尽传播路径。
2. 前台入口在锁等待前用 RAII AtomicUsize 统计 pending 请求。维护每批 try_lock，失败立即退让；取得锁后再检查 pending。循环在 guard 外，每批释放锁，不持锁 sleep，不自旋。
3. 新增 idx_thinking_sessions_accessed(last_accessed, session_key)，DDL 失败传播。首次初始化不受短清理 deadline 限制，不构建扫描 thinking_records 全表的新索引。
4. 每批至多 128 条 thinking_records、至多 128 个空过期 sessions。过期会话选择及其记录选择使用索引；单大 session 分批删，只有记录清空后才删 session。严格沿用 cutoff 的小于语义。
5. 孤儿清理先按 id keyset 读取最多 512 条元数据，再过滤 session 不存在且 COALESCE(last_accessed,created_at) < cutoff；只删除窗口中最多 128 条符合记录。游标持久化 thinking_meta，成功事务才推进；窗口内剩余待删记录不可被游标跳过；无符合行也应推进，末尾回绕。每轮过期 session 清理不能饿死 orphan 扫描，两者均有限进展。
6. rusqlite 添加 hooks feature。单批 SQLite VM 时间预算 100ms（每 1000 VM ops 检查，允许因 pending 请求提前中断），整轮时间预算 2 秒、最多 16 个批次。handler 在所有路径清除，Interrupted 先恢复 handler 再完成 rollback，不能污染正常请求。busy/Interrupted 明确表示延期，其他错误返回。不宣称硬实时保证。测试可注入较小限制与确定性中断条件，避免 sleep 驱动脆弱测试。
7. tool_signatures 使用现有 created_at 索引按相同记录上限清理，不能无界删除或每次扫描全表；错误不再静默成功。保留 cleanup_old_thinking_records 原调用者兼容或同步更新全部调用者。增加简洁维护统计（deleted/scanned/batches/deferred/elapsed 等有用字段），正常零删除也可验证维护已执行。
8. monitor 启动清理和后续每小时清理合并为一个串行 async loop，等待 spawn_blocking 完成；interval 使用 Skip，不改变每小时频率。各维护错误均记录。保留 log retention 和内部错误日志原职责，不引入 VACUUM。维护执行需可从日志确认。
9. 不引入第二个 thinking 连接，保留 save + session touch 原本的锁内连续性。检查 ThinkingStore/DashMap 调用点不持 guard 跨 blocking 边界；如发现实际同类问题则精确修复并报告。不要扩大为全面存储重构。

### 必需测试
- 真实 facade 在同步线程、CurrentThread、MultiThread 与 spawn_blocking 均兼容，错误传播。
- 2 worker MultiThread runtime，独立 OS 线程持有真实共享锁，同时至少 2 个请求进入真实 facade；独立 HTTP /health 在解锁前可成功。测试回归时也必须有独立 OS 线程安全解锁而不会挂死。
- cleanup 锁忙立即退让；已排队请求阻止后续维护批次抢锁；save/touch 期间 maintenance 无法插入。
- TTL 矩阵：活跃 session 旧记录保留，过期 session 新旧记录删除，孤儿新旧/NULL 回落/non-NULL 优先/恰等 cutoff，空 expired session。
- 大 session 超过 batch cap，第一批不超过 cap，session 仍存在；后续完成再删 session。
- 大量健康记录前缀后有孤儿，跨轮持久化 cursor 能到达；窗口内超过 cap 的 orphan 不漏删。
- progress interrupt 事务回滚且 cursor 不推进，后续 facade 查询成功；SQL 真错误不能吞掉。
- EXPLAIN 过期 session 查询、按 session 选 records、orphan 窗口不作整表扫描。
运行受影响 proxy_db/monitor/runtime/thinking_store 测试，保存测试结果。先实现能揭露既有回归的核心测试并证明旧路径失败，再修复。不执行全仓测试。提交一份可独立回滚的修复。

## Task 2: 禁用账号不参与自动配额刷新
### 范围与约束
工作目录同 Global Constraints。修改 src-tauri/src/modules/account.rs 及必要聚焦测试；Rust edition 2024，中文工程注释。主控负责推送，不操作生产，不改凭据、账号数据或手动刷新能力。

### 要求
在 refresh_quotas_logic 的 protected_only 自动刷新选择中跳过 disabled=true 的账号。保留 protected_quota_refresh_due、forbidden 等现有过滤语义。全量手动刷新仍允许 disabled 账号，不能用 proxy_disabled 代替 disabled 导致误排除配额保护账号。抽取最小纯选择 helper 或在已有 predicate 中表达，避免重复业务判断。
测试覆盖 disabled 的到期 protected 账号自动排除，正常到期允许，未到期/无protected排除，手动全量行为保持，forbidden行为保持。先验证新增回归测试旧代码失败，然后实现。运行相关账户测试、fmt check。创建独立可回滚提交，报告真实测试证据。

## Task 3: 发布验证与生产更新
由主控执行。整合独立任务审查后运行准确候选提交上的 fmt、clippy --all-targets --all-features、相关测试。npm run bump beta 同步到 beta.2，维护中英文 changelog，npm run build，版本检查。按 PR 模板创建到 beta 的 PR、附加artifact，CI 完成后合入 beta，发布匹配 beta 的 tag；不要改 main/latest。审查准确发布差异，保留回滚。
生产先验证现有部署配置与镜像。用户已明确不需要数据库备份；保留旧镜像与部署配置后直接切换 IMAGE_DIGEST，更新容器。验证镜像 revision、内外 health、前端、管理模型目录和一笔小额真实请求；观察至少一次启动维护和一次小时维护后仍响应。记录重启计数与维护耗时、已验证/未验证边界。全程不得回显凭据。

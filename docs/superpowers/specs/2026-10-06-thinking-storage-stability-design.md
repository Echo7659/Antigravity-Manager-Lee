# ThinkingStore 定时清理稳定性设计

## 目标与证据
线上 beta.1 的 watchdog 已累计退出 57 次。较长运行段在启动后一小时左右停止处理请求；现有每小时清理对约 13.92 GiB thinking_store 执行整表条件删除，且持有所有 ThinkingStore 请求共用的 Mutex。同步请求等待可耗尽 Tokio worker。该结论来自时间、SQL 计划和源码，未捕获故障时线程栈。

修复必须保留 ThinkingStore 和配置的 15 天 TTL，避免通过关闭功能或延长 watchdog 掩盖问题。用户已授权 beta 验证、推送和生产更新。

## 方案比较
| 方案 | 收益 | 代价 | 选择 |
| --- | --- | --- | --- |
| 只释放 Tokio worker | 独立 HTTP 任务可运行 | 无界清理仍占用连接 | 不足 |
| 独立维护连接 | 请求可并行读取 | 引入 save 与 session touch 竞态，SQLite 写入仍串行 | 本次不用 |
| 共享连接、短批次、请求优先、阻塞边界 | 保留现有互斥语义，限制清理工作并保护调度 | 大块数据与磁盘 I/O 无硬实时保证 | 采用 |
| 全面异步存储服务 | 显式排队与取消 | API 改动面大 | 本次不用 |

## 数据库边界
请求同步接口通过一个闭包入口执行路径查找、锁等待、打开连接与 SQL。MultiThread Tokio runtime 在入口调用 block_in_place；普通线程与 CurrentThread 保留同步语义。连接与 statement 不得逃出闭包。维护每批 try_lock，有请求排队或锁繁忙就退出本轮。RAII 计数覆盖前台锁等待。

TTL 会话选择新增 (last_accessed, session_key) 索引，创建失败必须可见。每批记录、会话和扫描数量有限。过期 session 仅在没有记录后删除。孤儿先以主键 LIMIT 取得有限窗口，再判断 session 不存在与 COALESCE(last_accessed, created_at) < cutoff；扫描游标事务性持久化，按已检查最大 ID 推进并在尾部回绕。清理不读取 thought blob，不新增 VACUUM。

批次使用 SQLite progress handler 控制执行预算；成功、错误、回滚、提前退出均恢复连接状态。它不能中断系统调用或提供硬时延保证。维护有整轮工作量与时间预算；旧 tool_signatures 同样按现有时间索引分批处理。后台 startup 与 hourly 串行运行，错过的 tick 跳过。日志输出工作量、耗时、退让与错误。

自动 protected quota 刷新跳过已 disabled 账号；手动全量刷新保留既有行为。

## 验证与发布
覆盖 TTL、孤儿分页、清理中断回滚、请求优先、不同 runtime 上下文与真实 HTTP 健康响应。Rust fmt、clippy 和相关测试先本地通过，再 beta PR 与 CI、版本化镜像。按用户明确要求，不备份数据库；保留旧 digest 和部署配置用于镜像回滚。上线核对镜像 revision、内外 health、管理页面、模型目录与小额真实请求，并跨过一小时定时清理窗口。

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

TTL 会话选择新增 (last_accessed, session_key) 索引，创建失败必须可见。session/tool 删除单元初始上限 128；orphan 主键元数据窗口初始上限 16，再判断 session 不存在与 COALESCE(last_accessed, created_at) < cutoff。过期 session 仅在没有记录后删除。游标与删除同事务提交，仅推进实际完整处理位置，未处理尾部留给下一批，末尾回绕。清理不读取 thought blob，不新增 VACUUM、第二个 thinking 连接或覆盖记录全表的大索引。

仅预算到期导致的中断会将该类别单元减半，最低为 1；pending、锁忙和 SQLite busy 不缩小单元。自适应状态由串行 monitor loop 持有，位于失败事务之外，跨轮保留。每轮使用当前 cutoff；保留天数配置变化时重新开放类别，保留缩小单元。已完成类别在后续 60 秒重试中停止执行；全部类别完成后的下一遍或日志维护的小时点会重新开放已完成类别，避免新到期 session/tool 等待长时间 orphan 巡扫。进行中的 orphan cursor 不重置。orphan cursor 在 SQLite 持久化，类别完成标记、单元和轮转起点为进程内状态。

单批名义预算 100ms，由每 1000 VM ops 的 SQLite progress handler 和 Rust 循环/COMMIT 前 checkpoint 共用 deadline；中断后先卸载 handler 再回滚。最小单元（一个 orphan 元数据行、一个过期 session 至多一条记录、或一条 tool signature）采用软墙钟预算：允许已开始单元超过 100ms 后提交，pending 与 SQL 错误仍中断并回滚，COMMIT 前仍检查 pending。更大单元不得沿用此放宽。整轮名义预算 2 秒、最多 256 次尝试；最小单元完成后若整轮已耗尽，不再启动下一单元。I/O、busy 等待、回滚与提交均可能超过墙钟预算，不提供硬实时保证。

同一串行 async loop 等待 spawn_blocking 完成。Thinking 维护未完成、延期或出错时，从本轮完成时刻起 60 秒后重试；整遍完成后 3600 秒再开始新遍历；日志维护的小时点也会触发 thinking 检查并重新开放已完成类别。proxy log retention 与内部错误日志维护仍在启动和各自上次完成后 3600 秒执行，不随 thinking 重试提频，错过周期不补跑。日志报告尝试批次、已提交批次和扫描数、删除数、延期原因、cursor、本轮耗时、当前单元以及 orphan/整体完成状态；`deferred=false` 不表示没有剩余工作。

日志维护到期时仅加载一次只读配置快照，proxy log retention 与内部错误日志预算共用该快照。读取入口复用既有解析和内存迁移规则，但不创建数据目录或配置、不执行迁移写回；缺失、读取或解析失败时，本轮使用 proxy 默认保留策略和内部错误日志默认预算。普通配置加载与显式保存行为保持不变。

## 冷窗口证据与容量边界
生产只读采样显示同一 512 行 orphan 元数据查询在起始/中间/尾部窗口约耗时 705ms、2205ms、3.8ms。固定 512 行配合 100ms 整批回滚存在反复停在同一 cursor 的实际风险；16 是自适应起点，并非已测得的耗时保证。缩小至 1 后的软预算用于保证无前台竞争时最小有界单元可以提交，不能扩展为大窗口超时继续。

维护吞吐还取决于冷 I/O、前台竞争、新增/到期速率和数据形态。仅有总 session 数与 15 天配置不足以推导生产到期速率；不得承诺积压清空时间或保留期容量已达标，应结合提交扫描/删除日志和实际新增速率验收。

自动 protected quota 刷新跳过已 disabled 账号；手动全量刷新保留既有行为。

## 验证与发布
覆盖 TTL、孤儿分页、清理中断回滚、请求优先、不同 runtime 上下文与真实 HTTP 健康响应。Rust fmt、clippy 和相关测试先本地通过，再 beta PR 与 CI、版本化镜像。按用户明确要求，不备份数据库；保留旧 digest 和部署配置用于镜像回滚。上线核对镜像 revision、内外 health、管理页面、模型目录与小额真实请求，并跨过一小时定时清理窗口。

use crate::proxy::config::LogRetentionConfig;
use crate::proxy::monitor::ProxyRequestLog;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, Instant};

static LOG_WRITE_LOCK: Mutex<()> = Mutex::new(());
static TOOL_SIGNATURE_DB: OnceLock<Mutex<Option<(PathBuf, Connection)>>> = OnceLock::new();

const THOUGHT_RAW_MAGIC: &[u8] = b"RAW1";
const THOUGHT_GZIP_MAGIC: &[u8] = b"AGZ1";
const MIN_GZIP_THOUGHT: usize = 384;
pub const SENTINEL_SIGNATURE: &str = "skip_thought_signature_validator";
pub const MIN_REAL_SIGNATURE: usize = 32;

pub fn normalize_and_heal_signature(sig: &str) -> Option<String> {
    if sig.is_empty() || sig == SENTINEL_SIGNATURE {
        return None;
    }
    // 自愈防裂化：若签名被误传或脏存储为原始 Protobuf 二进制 (首字节 0x12)，自动纠正编码为标准 Base64
    let normalized = if sig.as_bytes().first() == Some(&0x12) {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(sig.as_bytes())
    } else {
        sig.to_string()
    };
    if normalized.len() >= MIN_REAL_SIGNATURE {
        Some(normalized)
    } else {
        None
    }
}

fn persist_signature(signature: Option<&str>) -> Option<String> {
    signature.and_then(normalize_and_heal_signature)
}

/// Tool turns match by tool_id at fill time — visible/tool_names are in the request JSON.
/// Only text-only turns keep visible so prefix matching still works after restart.
fn persist_visible<'a>(tool_ids: &[String], visible: &'a str) -> &'a str {
    if tool_ids.is_empty() {
        visible
    } else {
        ""
    }
}

fn pack_thought(s: &str) -> Vec<u8> {
    if s.len() >= MIN_GZIP_THOUGHT {
        let mut enc = GzEncoder::new(Vec::with_capacity(s.len() / 2), Compression::fast());
        if enc.write_all(s.as_bytes()).is_ok() {
            if let Ok(buf) = enc.finish() {
                if buf.len() + THOUGHT_GZIP_MAGIC.len() < s.len() {
                    let mut out = Vec::with_capacity(THOUGHT_GZIP_MAGIC.len() + buf.len());
                    out.extend_from_slice(THOUGHT_GZIP_MAGIC);
                    out.extend_from_slice(&buf);
                    return out;
                }
            }
        }
    }
    let mut out = Vec::with_capacity(THOUGHT_RAW_MAGIC.len() + s.len());
    out.extend_from_slice(THOUGHT_RAW_MAGIC);
    out.extend_from_slice(s.as_bytes());
    out
}

fn read_thought_bytes(reader: impl Read, budget: usize) -> (Vec<u8>, std::io::Result<usize>) {
    let mut decoded = Vec::new();
    let result = reader
        .take(budget.saturating_add(1) as u64)
        .read_to_end(&mut decoded);
    (decoded, result)
}

/// 解码至多预算加一个字节；超限记录整体拒收，不进入损坏数据回退。
fn unpack_thought_bounded(bytes: &[u8], budget: usize) -> Option<String> {
    fn lossy_bounded(bytes: &[u8], budget: usize) -> Option<String> {
        let output_len = bytes.utf8_chunks().try_fold(0usize, |len, chunk| {
            let next = len
                .checked_add(chunk.valid().len())?
                .checked_add(if chunk.invalid().is_empty() { 0 } else { 3 })?;
            (next <= budget).then_some(next)
        })?;
        let mut result = String::with_capacity(output_len);
        for chunk in bytes.utf8_chunks() {
            result.push_str(chunk.valid());
            if !chunk.invalid().is_empty() {
                result.push('�');
            }
        }
        Some(result)
    }
    if let Some(rest) = bytes.strip_prefix(THOUGHT_GZIP_MAGIC) {
        let (decoded, result) = read_thought_bytes(GzDecoder::new(rest), budget);
        if decoded.len() > budget {
            return None;
        }
        if result.is_ok() {
            return lossy_bounded(&decoded, budget);
        }
    }
    lossy_bounded(
        bytes.strip_prefix(THOUGHT_RAW_MAGIC).unwrap_or(bytes),
        budget,
    )
}

#[cfg(test)]
fn unpack_thought(bytes: &[u8]) -> String {
    unpack_thought_bounded(bytes, crate::proxy::thinking_store::MAX_BYTES_PER_SESSION)
        .unwrap_or_default()
}

pub fn get_proxy_db_path() -> Result<PathBuf, String> {
    let data_dir = crate::modules::account::get_data_dir()?;
    Ok(data_dir.join("proxy_logs.db"))
}

pub fn get_thinking_db_path() -> Result<PathBuf, String> {
    let data_dir = crate::modules::account::get_data_dir()?;
    Ok(data_dir.join("thinking_store.db"))
}

fn apply_fast_pragmas(conn: &Connection) -> Result<(), String> {
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| e.to_string())?;
    let _ = conn.pragma_update(None, "cache_size", -64000);
    let _ = conn.pragma_update(None, "temp_store", "MEMORY");
    let _ = conn.pragma_update(None, "mmap_size", 268435456);
    Ok(())
}

fn connect_db() -> Result<Connection, String> {
    let db_path = get_proxy_db_path()?;
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    apply_fast_pragmas(&conn)?;
    Ok(conn)
}

pub fn is_synthetic_tool_id(id: &str) -> bool {
    id.starts_with("call_") && id.chars().filter(|&c| c == '_').count() >= 3
}

fn init_thinking_schema(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS thinking_records (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_key TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            thought TEXT NOT NULL,
            signature TEXT,
            tool_ids TEXT NOT NULL,
            tool_names TEXT NOT NULL,
            visible TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_accessed INTEGER
        )",
        [],
    )
    .map_err(|e| e.to_string())?;

    // 动态升级：增加 primary_tool_id 列用于旧版兼容点查
    let _ = conn.execute(
        "ALTER TABLE thinking_records ADD COLUMN primary_tool_id TEXT",
        [],
    );

    // 动态升级：增加 causal_tool_id 列用于确定性因果伪哈希 ID 极速穿透点查
    let _ = conn.execute(
        "ALTER TABLE thinking_records ADD COLUMN causal_tool_id TEXT",
        [],
    );

    // 1. 覆盖 load_thinking_records 的正向序列扫描 (ORDER BY id ASC)，同时完美承接逆序扫描 (ORDER BY id DESC)
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thinking_rec_seq ON thinking_records (session_key, id ASC)",
        [],
    );
    // 2. 覆盖基于 causal_tool_id 的快速穿透点查 (极简 Partial Index，极致纳秒响应)
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thinking_rec_causal ON thinking_records (session_key, causal_tool_id) WHERE causal_tool_id IS NOT NULL",
        [],
    );
    // 3. 覆盖基于 primary_tool_id 的快速穿透点查 (兼容旧版数据)
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thinking_rec_tool ON thinking_records (session_key, primary_tool_id) WHERE primary_tool_id IS NOT NULL",
        [],
    );
    // 4. 覆盖基于 fingerprint 的指纹点查
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thinking_rec_fp ON thinking_records (session_key, fingerprint)",
        [],
    );
    // 5. 覆盖历史清理时间索引
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thinking_rec_accessed ON thinking_records (last_accessed ASC)",
        [],
    );
    // 6. 覆盖基于 signature 的精准穿透点查 (极简 Partial Index，WHERE signature IS NOT NULL)
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thinking_rec_sig ON thinking_records (session_key, signature) WHERE signature IS NOT NULL",
        [],
    );

    // 7. 索引大瘦身：安全清理物理冗余的重复索引，削减写放大开销
    let _ = conn.execute("DROP INDEX IF EXISTS idx_thinking_rec_latest", []);
    let _ = conn.execute("DROP INDEX IF EXISTS idx_thinking_rec_session", []);
    conn.execute(
        "CREATE TABLE IF NOT EXISTS thinking_sessions (
            session_key TEXT PRIMARY KEY,
            last_accessed INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thinking_sessions_accessed ON thinking_sessions (last_accessed, session_key)",
        [],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS thinking_meta (
            k TEXT PRIMARY KEY,
            v TEXT NOT NULL
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn open_thinking_db_at(db_path: &PathBuf) -> Result<Connection, String> {
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    apply_fast_pragmas(&conn)?;
    init_thinking_schema(&conn)?;
    Ok(conn)
}

fn open_thinking_db() -> Result<Connection, String> {
    let db_path = get_thinking_db_path()?;
    open_thinking_db_at(&db_path)
}

static THINKING_DB: OnceLock<Mutex<Option<(PathBuf, Connection)>>> = OnceLock::new();
static THINKING_DB_GENERATION: AtomicUsize = AtomicUsize::new(0);

/// 快照同时识别连接重建、当前连接写入和其他连接提交。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThinkingDbSnapshot {
    generation: usize,
    total_changes: u64,
    data_version: i64,
}

fn thinking_db_snapshot_at(conn: &Connection) -> Result<ThinkingDbSnapshot, String> {
    Ok(ThinkingDbSnapshot {
        generation: THINKING_DB_GENERATION.load(Ordering::Relaxed),
        total_changes: conn.total_changes(),
        data_version: conn
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .map_err(|e| e.to_string())?,
    })
}

pub(crate) fn thinking_db_snapshot() -> Result<ThinkingDbSnapshot, String> {
    with_thinking_db(thinking_db_snapshot_at)
}

pub struct ThinkingDbGuard(MutexGuard<'static, Option<(PathBuf, Connection)>>);

#[cfg(test)]
pub(crate) fn hold_thinking_db_for_test() -> ThinkingDbGuard {
    ThinkingDbGuard(THINKING_DB.get_or_init(|| Mutex::new(None)).lock().unwrap())
}

impl std::ops::Deref for ThinkingDbGuard {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        &self.0.as_ref().expect("thinking db connection").1
    }
}

impl std::ops::DerefMut for ThinkingDbGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0.as_mut().expect("thinking db connection").1
    }
}

/// Process-lifetime connection to thinking_store.db.
/// Fill/hydrate must not open proxy_logs.db (it can be multi-GB on HDD).
/// Automatically tracks data directory changes and reuses connection with fast pragmas.
fn thinking_db() -> Result<ThinkingDbGuard, String> {
    let db_path = get_thinking_db_path()?;
    let slot = THINKING_DB.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock().map_err(|e| format!("thinking db lock: {e}"))?;
    if guard.as_ref().map(|(p, _)| p) != Some(&db_path) {
        let conn = open_thinking_db_at(&db_path)?;
        *guard = Some((db_path, conn));
        THINKING_DB_GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    Ok(ThinkingDbGuard(guard))
}

static THINKING_PENDING: AtomicUsize = AtomicUsize::new(0);

/// 前台请求计数覆盖路径查找、连接初始化、锁等待和 SQL 执行。
struct ThinkingRequest;

impl ThinkingRequest {
    fn enter() -> Self {
        THINKING_PENDING.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ThinkingRequest {
    fn drop(&mut self) {
        THINKING_PENDING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 同步数据库边界在 MultiThread runtime 上释放 worker；CurrentThread 保留同步语义。
fn with_thinking_db<T>(
    operation: impl FnOnce(&Connection) -> Result<T, String>,
) -> Result<T, String> {
    let _pending = ThinkingRequest::enter();
    let run = || {
        #[cfg(test)]
        thinking_maintenance_tests::run_hook(&thinking_maintenance_tests::BEFORE_LOCK);
        let conn = thinking_db()?;
        operation(&conn)
    };
    if tokio::runtime::Handle::try_current()
        .is_ok_and(|handle| handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
    {
        tokio::task::block_in_place(run)
    } else {
        run()
    }
}

fn mark_thinking_imported(conn: &Connection) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO thinking_meta (k, v) VALUES ('imported_from_proxy_logs', '1')",
        [],
    );
}

/// Copy old thinking rows out of proxy_logs.db into thinking_store.db.
/// Never deletes the log DB. Old uncompressed rows stay readable via unpack_thought.
fn migrate_thinking_from_logs() -> Result<(), String> {
    with_thinking_db(|conn| {
        let imported: Option<String> = conn
            .query_row(
                "SELECT v FROM thinking_meta WHERE k = 'imported_from_proxy_logs'",
                [],
                |r| r.get(0),
            )
            .ok();
        if imported.as_deref() == Some("1") {
            return Ok(());
        }

        let logs_path = get_proxy_db_path()?;
        if !logs_path.exists() {
            mark_thinking_imported(conn);
            return Ok(());
        }

        let escaped = logs_path
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''");
        if conn
            .execute(&format!("ATTACH DATABASE '{}' AS logs", escaped), [])
            .is_err()
        {
            return Ok(());
        }

        let has_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM logs.sqlite_master WHERE type='table' AND name='thinking_records'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

        if has_table == 0 {
            let _ = conn.execute("DETACH DATABASE logs", []);
            mark_thinking_imported(conn);
            return Ok(());
        }

        // Copy only rows not already present. Do not gzip/rewrite on import — that
        // would stall HDD by touching every old thought blob at startup.
        let copy_with_accessed = "INSERT INTO thinking_records (session_key, fingerprint, thought, signature, tool_ids, tool_names, visible, created_at, last_accessed)
             SELECT src.session_key, src.fingerprint, src.thought, src.signature, src.tool_ids, src.tool_names, src.visible, src.created_at,
                    COALESCE(src.last_accessed, src.created_at)
             FROM logs.thinking_records src
             WHERE NOT EXISTS (
                SELECT 1 FROM thinking_records t
                WHERE t.session_key = src.session_key
                  AND t.fingerprint = src.fingerprint
                  AND t.created_at = src.created_at
             )";
        let copy_basic = "INSERT INTO thinking_records (session_key, fingerprint, thought, signature, tool_ids, tool_names, visible, created_at, last_accessed)
             SELECT src.session_key, src.fingerprint, src.thought, src.signature, src.tool_ids, src.tool_names, src.visible, src.created_at, src.created_at
             FROM logs.thinking_records src
             WHERE NOT EXISTS (
                SELECT 1 FROM thinking_records t
                WHERE t.session_key = src.session_key
                  AND t.fingerprint = src.fingerprint
                  AND t.created_at = src.created_at
             )";
        let copied = match conn.execute(copy_with_accessed, []) {
            Ok(n) => n,
            Err(_) => match conn.execute(copy_basic, []) {
                Ok(n) => n,
                Err(e) => {
                    let _ = conn.execute("DETACH DATABASE logs", []);
                    tracing::warn!("[ThinkingStore] Import from proxy_logs.db failed (will retry next start): {e}");
                    return Ok(());
                }
            },
        };
        let _ = conn.execute(
            "INSERT OR IGNORE INTO thinking_sessions (session_key, last_accessed)
         SELECT session_key, MAX(created_at) FROM thinking_records GROUP BY session_key",
            [],
        );
        let _ = conn.execute("DETACH DATABASE logs", []);
        mark_thinking_imported(conn);
        if copied > 0 {
            tracing::info!(
            "[ThinkingStore] Imported {} thinking row(s) from proxy_logs.db (old file kept as backup)",
            copied
        );
        }
        Ok(())
    })
}

pub fn init_db() -> Result<(), String> {
    let conn = Connection::open(get_proxy_db_path()?).map_err(|e| e.to_string())?;
    // 新数据库启用增量回收；既有日志库的完整重写属于显式维护，不能阻塞代理启动。
    let page_count: i64 = conn
        .pragma_query_value(None, "page_count", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if page_count == 0 {
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")
            .map_err(|e| e.to_string())?;
    }
    apply_fast_pragmas(&conn)?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS request_logs (
            id TEXT PRIMARY KEY,
            timestamp INTEGER,
            method TEXT,
            url TEXT,
            status INTEGER,
            duration INTEGER,
            model TEXT,
            error TEXT
        )",
        [],
    )
    .map_err(|e| e.to_string())?;

    // Try to add new columns (ignore errors if they exist)
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN request_body TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE request_logs ADD COLUMN upstream_request_body TEXT",
        [],
    );
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN response_body TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE request_logs ADD COLUMN input_tokens INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE request_logs ADD COLUMN output_tokens INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE request_logs ADD COLUMN cached_tokens INTEGER",
        [],
    );
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN account_email TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN mapped_model TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN protocol TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN client_ip TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN username TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE request_logs ADD COLUMN request_headers TEXT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE request_logs ADD COLUMN upstream_request_headers TEXT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE request_logs ADD COLUMN response_headers TEXT",
        [],
    );
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN session_id TEXT", []);

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_timestamp ON request_logs (timestamp DESC)",
        [],
    )
    .map_err(|e| e.to_string())?;

    // Add status index for faster stats queries
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_status ON request_logs (status)",
        [],
    )
    .map_err(|e| e.to_string())?;

    // 大型历史日志可在维护窗口创建附加索引，避免升级启动等待全表扫描。
    if std::env::var("ABV_DEFER_LOG_INDEX_MIGRATIONS").as_deref() != Ok("true") {
        // 高效复合索引：状态与时间戳倒序（针对错误筛选与分页排序，极大提升大数据量下的响应速度）
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_status_timestamp ON request_logs (status, timestamp DESC)",
            [],
        );

        // 复合索引：模型与时间戳倒序（针对模型级日志过滤与排序）
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_model_timestamp ON request_logs (model, timestamp DESC)",
            [],
        );

        // 复合索引：账号邮箱与时间戳倒序（针对多用户/多账号过滤）
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_account_timestamp ON request_logs (account_email, timestamp DESC)",
            [],
        );

        // 复合索引：客户端IP与时间戳倒序（针对安全审计与IP过滤）
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_client_ip_timestamp ON request_logs (client_ip, timestamp DESC)",
            [],
        );

        // 复合索引：用户名与时间戳倒序
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_username_timestamp ON request_logs (username, timestamp DESC)",
            [],
        );

        // 复合索引：会话与时间戳倒序（针对会话粒度运维分析）
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_session_timestamp ON request_logs (session_id, timestamp DESC)",
            [],
        );

        // 单列索引：协议类型
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_protocol ON request_logs (protocol)",
            [],
        );

        // 单列索引：请求方法
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_method ON request_logs (method)",
            [],
        );
    }

    // 持久化工具签名表 (支持代理重启后根据 tool_id 秒级恢复真实加密签名)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tool_signatures (
            tool_id TEXT PRIMARY KEY,
            signature TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_tool_sig_created ON tool_signatures (created_at DESC)",
        [],
    );

    drop(conn);
    migrate_thinking_from_logs()?;

    Ok(())
}

fn map_request_log_row(row: &rusqlite::Row) -> rusqlite::Result<ProxyRequestLog> {
    Ok(ProxyRequestLog {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        method: row.get(2)?,
        url: row.get(3)?,
        status: row.get(4)?,
        duration: row.get(5)?,
        model: row.get(6)?,
        error: row.get(7)?,
        request_body: row.get(8).unwrap_or(None),
        upstream_request_body: row.get(9).unwrap_or(None),
        response_body: row.get(10).unwrap_or(None),
        input_tokens: row.get(11).unwrap_or(None),
        output_tokens: row.get(12).unwrap_or(None),
        cached_tokens: row.get(13).unwrap_or(None),
        account_email: row.get(14).unwrap_or(None),
        mapped_model: row.get(15).unwrap_or(None),
        protocol: row.get(16).unwrap_or(None),
        client_ip: row.get(17).unwrap_or(None),
        username: row.get(18).unwrap_or(None),
        request_headers: row.get(19).unwrap_or(None),
        upstream_request_headers: row.get(20).unwrap_or(None),
        response_headers: row.get(21).unwrap_or(None),
        session_id: row.get(22).unwrap_or(None),
    })
}

pub fn save_tool_signature(tool_id: &str, signature: &str) -> Result<(), String> {
    if tool_id.is_empty() || signature.is_empty() {
        return Ok(());
    }
    let norm_id = crate::proxy::common::utils::normalize_tool_id(tool_id);
    let healed_sig = match normalize_and_heal_signature(signature) {
        Some(s) => s,
        None => return Ok(()),
    };
    let conn = connect_db()?;
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT OR REPLACE INTO tool_signatures (tool_id, signature, created_at) VALUES (?1, ?2, ?3)",
        params![norm_id.as_ref(), healed_sig, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_tool_signature(tool_id: &str) -> Result<Option<String>, String> {
    if tool_id.is_empty() {
        return Ok(None);
    }
    let norm_id = crate::proxy::common::utils::normalize_tool_id(tool_id);
    let db_path = get_proxy_db_path()?;
    let found = {
        let mut db = TOOL_SIGNATURE_DB
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|e| format!("tool signature db lock: {e}"))?;
        if db.as_ref().map(|(path, _)| path) != Some(&db_path) {
            let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
            conn.busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|e| e.to_string())?;
            *db = Some((db_path, conn));
        }
        let conn = &db
            .as_ref()
            .ok_or("tool signature db was not initialized")?
            .1;
        let mut stmt = conn
            .prepare_cached("SELECT signature FROM tool_signatures WHERE tool_id = ?1 LIMIT 1")
            .map_err(|e| e.to_string())?;
        let res: Option<String> = {
            let mut rows = stmt
                .query(params![norm_id.as_ref()])
                .map_err(|e| e.to_string())?;
            if let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let sig: String = row.get(0).map_err(|e| e.to_string())?;
                Some(sig)
            } else {
                None
            }
        };
        if res.is_some() {
            res
        } else if norm_id.as_ref() != tool_id {
            let mut rows = stmt.query(params![tool_id]).map_err(|e| e.to_string())?;
            if let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let sig: String = row.get(0).map_err(|e| e.to_string())?;
                Some(sig)
            } else {
                None
            }
        } else {
            None
        }
    };
    if let Some(sig) = found {
        if let Some(healed) = normalize_and_heal_signature(&sig) {
            if healed != sig {
                let _ = save_tool_signature(norm_id.as_ref(), &healed);
            }
            return Ok(Some(healed));
        }
    }
    Ok(None)
}

#[derive(Debug, Clone)]
pub struct PersistedThinkingRecord {
    pub id: i64,
    pub fingerprint: String,
    pub thought: String,
    pub signature: Option<String>,
    pub tool_ids: Vec<String>,
    pub tool_names: Vec<String>,
    pub visible: String,
}

pub fn save_thinking_record(
    session_key: &str,
    fingerprint: &str,
    thought: &str,
    signature: Option<&str>,
    tool_ids: &[String],
    _tool_names: &[String],
    visible: &str,
) -> Result<(), String> {
    save_thinking_record_inner(
        session_key,
        fingerprint,
        thought,
        signature,
        tool_ids,
        visible,
        ThinkingSaveGuard::None,
    )
    .map(|_| ())
}

/// expected_latest 限定已有最新行的升级；None 沿用新 capture 的保存语义。
pub(crate) fn save_thinking_record_with_id(
    session_key: &str,
    rec: &crate::proxy::thinking_store::ThinkingRecord,
    expected_latest: Option<i64>,
) -> Result<Option<i64>, String> {
    save_thinking_record_inner(
        session_key,
        &rec.fingerprint,
        &rec.thought,
        rec.signature.as_deref(),
        &rec.tool_ids,
        &rec.visible,
        expected_latest.map_or(ThinkingSaveGuard::None, ThinkingSaveGuard::LatestId),
    )
}

enum ThinkingSaveGuard {
    None,
    LatestId(i64),
    PreserveStrongerLatest,
}

/// 仅供 RAM 判为弱重复的 capture 使用，真实 latest 的抑制条件在保存事务内确认。
pub(crate) fn save_thinking_capture_unless_weaker(
    session_key: &str,
    rec: &crate::proxy::thinking_store::ThinkingRecord,
) -> Result<Option<i64>, String> {
    save_thinking_record_inner(
        session_key,
        &rec.fingerprint,
        &rec.thought,
        rec.signature.as_deref(),
        &rec.tool_ids,
        &rec.visible,
        ThinkingSaveGuard::PreserveStrongerLatest,
    )
}

/// 只读取到比较阈值；超预算保守保留 latest，不证明尚未读取的 gzip 尾部有效。
fn thought_exceeds_capture_length(raw: &[u8], threshold: usize) -> bool {
    let lossy_exceeds = |bytes: &[u8]| {
        if bytes.len() > threshold {
            return true;
        }
        let mut length = 0usize;
        for chunk in bytes.utf8_chunks() {
            length = length.saturating_add(chunk.valid().len());
            if !chunk.invalid().is_empty() {
                length = length.saturating_add('�'.len_utf8());
            }
            if length > threshold {
                return true;
            }
        }
        false
    };
    if let Some(compressed) = raw.strip_prefix(THOUGHT_GZIP_MAGIC) {
        let (decoded, result) = read_thought_bytes(GzDecoder::new(compressed), threshold);
        if decoded.len() > threshold {
            return true;
        }
        if result.is_ok() {
            return lossy_exceeds(&decoded);
        }
        // 预算内已经发生的解码错误沿用原始字节 fallback，不当作更长的有效 thought。
    }
    lossy_exceeds(raw.strip_prefix(THOUGHT_RAW_MAGIC).unwrap_or(raw))
}

fn latest_capture_is_stronger(
    conn: &Connection,
    session_key: &str,
    fingerprint: &str,
    thought: &str,
    signature: Option<&str>,
) -> Result<bool, String> {
    conn.query_row(
        "SELECT fingerprint, thought, signature FROM thinking_records WHERE session_key = ?1 ORDER BY id DESC LIMIT 1",
        [session_key],
        |row| {
            use rusqlite::types::ValueRef;
            let mut fields = [&[][..]; 3];
            for (i, field) in fields.iter_mut().enumerate() {
                *field = match row.get_ref(i)? {
                    ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes,
                    ValueRef::Null => &[],
                    _ => return Ok(false),
                };
            }
            let [old_fp, raw, old_signature] = fields;
            if old_fp != fingerprint.as_bytes() {
                return Ok(false);
            }
            // 与持久行读取时的签名自愈长度一致，不复制可能超大的签名字段。
            let old_signature_len = match std::str::from_utf8(old_signature) {
                Ok(sig) if !sig.is_empty() && sig != SENTINEL_SIGNATURE => {
                    let length = if old_signature.first() == Some(&0x12) {
                        old_signature.len().div_ceil(3).saturating_mul(4)
                    } else {
                        old_signature.len()
                    };
                    if length >= MIN_REAL_SIGNATURE { length } else { 0 }
                }
                _ => 0,
            };
            Ok(signature.map_or(0, str::len) <= old_signature_len
                && thought_exceeds_capture_length(raw, thought.len()))
        },
    ).optional().map(|result| result.unwrap_or(false)).map_err(|e| e.to_string())
}

fn save_thinking_record_inner(
    session_key: &str,
    fingerprint: &str,
    thought: &str,
    signature: Option<&str>,
    tool_ids: &[String],
    visible: &str,
    guard: ThinkingSaveGuard,
) -> Result<Option<i64>, String> {
    if session_key.is_empty() {
        return Ok(None);
    }
    with_thinking_db(|conn| {
        // 条件升级和弱 capture 判定均与保存共用事务，禁止用旧 RAM 快照证明当前 latest。
        let transaction = if matches!(guard, ThinkingSaveGuard::None) {
            None
        } else {
            Some(
                rusqlite::Transaction::new_unchecked(
                    conn,
                    rusqlite::TransactionBehavior::Immediate,
                )
                .map_err(|e| e.to_string())?,
            )
        };
        if let ThinkingSaveGuard::LatestId(expected) = guard {
            let latest: Option<i64> = conn.query_row(
                "SELECT id FROM thinking_records WHERE session_key = ?1 ORDER BY id DESC LIMIT 1",
                [session_key], |row| row.get(0),
            ).optional().map_err(|e| e.to_string())?;
            if latest != Some(expected) {
                return Ok(None);
            }
        }
        if matches!(guard, ThinkingSaveGuard::PreserveStrongerLatest)
            && latest_capture_is_stronger(conn, session_key, fingerprint, thought, signature)?
        {
            return Ok(None);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let normalized_tool_ids: Vec<String> = tool_ids
            .iter()
            .map(|id| crate::proxy::common::utils::normalize_tool_id(id).into_owned())
            .collect();
        let tool_ids_json =
            serde_json::to_string(&normalized_tool_ids).unwrap_or_else(|_| "[]".to_string());
        let causal_tool_id = normalized_tool_ids
            .iter()
            .find(|id| is_synthetic_tool_id(id))
            .map(|s| s.as_str());
        let primary_tool_id = normalized_tool_ids.first().map(|s| s.as_str());
        // tool_names / full visible for tool turns are reconstructable from the next
        // request JSON at fill time. Do not write them.
        let visible_persist = persist_visible(&normalized_tool_ids, visible);
        let packed_thought = pack_thought(thought);
        let signature = persist_signature(signature);

        // 智能防叠加与幂等查重：只允许合并/更新当前会话中的【最新一条】活跃轮次（流式碎片拼接或更长思考补齐）
        // 绝不能回溯更新历史早期轮次！
        let latest_row: Option<(
        i64,
        usize,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
    )> = conn
        .query_row(
            "SELECT id, length(thought), signature, fingerprint, primary_tool_id, causal_tool_id
             FROM thinking_records
             WHERE session_key = ?1
             ORDER BY id DESC LIMIT 1",
            params![session_key],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .ok();

        let existing_id: Option<(i64, usize, Option<String>)> = match latest_row {
            Some((id, len, sig, ref last_fp, ref last_tool_id, ref last_causal_id)) => {
                let is_match = if let Some(c_id) = causal_tool_id {
                    last_causal_id.as_deref() == Some(c_id)
                        || last_tool_id.as_deref() == Some(c_id)
                        || last_causal_id
                            .as_deref()
                            .map(|s| crate::proxy::common::utils::normalize_tool_id(s))
                            .as_deref()
                            == Some(c_id)
                        || last_tool_id
                            .as_deref()
                            .map(|s| crate::proxy::common::utils::normalize_tool_id(s))
                            .as_deref()
                            == Some(c_id)
                } else if let Some(p_id) = primary_tool_id {
                    last_tool_id.as_deref() == Some(p_id)
                        || last_tool_id
                            .as_deref()
                            .map(|s| crate::proxy::common::utils::normalize_tool_id(s))
                            .as_deref()
                            == Some(p_id)
                } else {
                    last_fp == fingerprint && last_tool_id.is_none() && last_causal_id.is_none()
                };
                if is_match {
                    Some((id, len, sig))
                } else {
                    None
                }
            }
            None => None,
        };

        let saved_id = if let Some((id, old_thought_len, old_sig)) = existing_id {
            // 已存在记录：检查是否需要更新（防止将已有实质思考覆盖为占位符，但允许补全更长思考或有效签名）
            let incoming_has_meaningful_thought =
                !crate::proxy::thinking_store::is_placeholder_thought(thought)
                    && !thought.trim().is_empty();
            let old_is_dummy = old_thought_len <= 10; // "RAW1..." 或占位符非常短

            let should_update_thought = incoming_has_meaningful_thought || old_is_dummy;
            let healed_old_sig = old_sig.as_deref().and_then(normalize_and_heal_signature);
            let effective_sig = signature.as_deref().or(healed_old_sig.as_deref());

            if should_update_thought {
                let mut stmt = conn
                .prepare_cached(
                    "UPDATE thinking_records
                     SET thought = ?1, signature = ?2, tool_ids = ?3, visible = ?4, created_at = ?5, primary_tool_id = ?6, causal_tool_id = ?7
                     WHERE id = ?8",
                )
                .map_err(|e| e.to_string())?;
                stmt.execute(params![
                    packed_thought.as_slice(),
                    effective_sig,
                    &tool_ids_json,
                    visible_persist,
                    now,
                    primary_tool_id,
                    causal_tool_id,
                    id,
                ])
                .map_err(|e| e.to_string())?;
            } else if (signature.is_some() && signature.as_deref() != old_sig.as_deref())
                || (healed_old_sig.as_deref() != old_sig.as_deref())
            {
                // 仅更新签名，保留已有的高质量实质思考（同时修复旧签名的脏数据）
                let mut stmt = conn
                    .prepare_cached(
                        "UPDATE thinking_records
                     SET signature = ?1, created_at = ?2
                     WHERE id = ?3",
                    )
                    .map_err(|e| e.to_string())?;
                stmt.execute(params![effective_sig, now, id])
                    .map_err(|e| e.to_string())?;
            }
            id
        } else {
            // 全新轮次：插入新记录（同时写入 primary_tool_id 与 causal_tool_id 列）
            let mut stmt = conn
            .prepare_cached(
                "INSERT INTO thinking_records (session_key, fingerprint, thought, signature, tool_ids, tool_names, visible, created_at, primary_tool_id, causal_tool_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, '[]', ?6, ?7, ?8, ?9)",
            )
            .map_err(|e| e.to_string())?;
            stmt.execute(params![
                session_key,
                fingerprint,
                packed_thought.as_slice(),
                signature.as_deref(),
                &tool_ids_json,
                visible_persist,
                now,
                primary_tool_id,
                causal_tool_id,
            ])
            .map_err(|e| e.to_string())?;
            conn.last_insert_rowid()
        };

        #[cfg(test)]
        thinking_maintenance_tests::run_hook(&thinking_maintenance_tests::BEFORE_TOUCH);
        let mut session_stmt = conn
            .prepare_cached(
                "INSERT INTO thinking_sessions (session_key, last_accessed) VALUES (?1, ?2)
             ON CONFLICT(session_key) DO UPDATE SET last_accessed = excluded.last_accessed",
            )
            .map_err(|e| e.to_string())?;
        session_stmt
            .execute(params![session_key, now])
            .map_err(|e| e.to_string())?;

        if let Some(transaction) = transaction {
            transaction.commit().map_err(|e| e.to_string())?;
        }
        Ok(Some(saved_id))
    })
}

/// 行字段在复制前借用检查；内容预算不包含容器、索引和分配器开销。
fn read_thinking_row(
    row: &rusqlite::Row<'_>,
    budget: usize,
) -> rusqlite::Result<Option<PersistedThinkingRecord>> {
    use rusqlite::types::ValueRef;
    let mut fields = [&[][..]; 6];
    for (i, field) in fields.iter_mut().enumerate() {
        *field = match row.get_ref(i + 1)? {
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes,
            ValueRef::Null => &[],
            _ => return Ok(None),
        };
    }
    let [fp, raw, signature, ids, names, visible] = fields;
    // 压缩输入及辅助元数据也设上限，禁止先复制大字段再拒收。
    if raw.len() > budget.saturating_add(4)
        || fp
            .len()
            .saturating_add(ids.len())
            .saturating_add(names.len())
            > budget
        || signature.len().saturating_add(visible.len()) > budget
    {
        tracing::debug!("[ThinkingStore] Skipped oversized persisted fields");
        return Ok(None);
    }
    let string = |bytes: &[u8]| std::str::from_utf8(bytes).map(str::to_owned);
    let (Ok(fp), Ok(ids), Ok(names), Ok(visible), Ok(raw_signature)) = (
        string(fp),
        string(ids),
        string(names),
        string(visible),
        string(signature),
    ) else {
        return Ok(None);
    };
    // 原始 protobuf 签名转 Base64 时会膨胀，转换前检查最终长度。
    let signature_len = if raw_signature.as_bytes().first() == Some(&0x12) {
        raw_signature.len().div_ceil(3).saturating_mul(4)
    } else {
        raw_signature.len()
    };
    if signature_len.saturating_add(visible.len()) > budget {
        tracing::debug!("[ThinkingStore] Skipped oversized healed signature");
        return Ok(None);
    }
    let signature = persist_signature(Some(&raw_signature));
    let remaining = budget - visible.len() - signature.as_ref().map_or(0, String::len);
    let Some(thought) = unpack_thought_bounded(raw, remaining) else {
        tracing::debug!("[ThinkingStore] Skipped oversized persisted thought");
        return Ok(None);
    };
    Ok(Some(PersistedThinkingRecord {
        id: row.get(0)?,
        fingerprint: fp,
        thought,
        signature,
        tool_ids: serde_json::from_str(&ids).unwrap_or_default(),
        tool_names: serde_json::from_str(&names).unwrap_or_default(),
        visible,
    }))
}

pub fn load_thinking_records(session_key: &str) -> Result<Vec<PersistedThinkingRecord>, String> {
    load_thinking_records_bounded(
        session_key,
        crate::proxy::config::get_thinking_max_memory_turns(),
        crate::proxy::thinking_store::MAX_BYTES_PER_SESSION,
    )
}

pub(crate) fn load_thinking_records_bounded(
    session_key: &str,
    max_turns: usize,
    max_bytes: usize,
) -> Result<Vec<PersistedThinkingRecord>, String> {
    load_thinking_history_bounded(session_key, max_turns, max_bytes).map(|history| history.records)
}

#[derive(Default)]
pub(crate) struct ThinkingHistory {
    pub records: Vec<PersistedThinkingRecord>,
    /// 只有全部持久历史均已接纳时为 true，读取失败或预算拒收不能视为完整。
    pub complete: bool,
}

pub(crate) fn load_thinking_history_bounded(
    session_key: &str,
    max_turns: usize,
    max_bytes: usize,
) -> Result<ThinkingHistory, String> {
    if session_key.is_empty() || max_turns == 0 || max_bytes == 0 {
        return Ok(ThinkingHistory::default());
    }
    with_thinking_db(|conn| {
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, fingerprint, thought, signature, tool_ids, tool_names, visible
             FROM thinking_records WHERE session_key = ?1 ORDER BY id DESC LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params![
                session_key,
                i64::try_from(max_turns).unwrap_or(i64::MAX)
            ])
            .map_err(|e| e.to_string())?;
        let mut result = ThinkingHistory {
            records: Vec::new(),
            complete: true,
        };
        let mut remaining = max_bytes;
        let mut oldest_candidate = None;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            oldest_candidate = Some(row.get::<_, i64>(0).map_err(|e| e.to_string())?);
            if let Some(rec) = read_thinking_row(row, remaining).map_err(|e| e.to_string())? {
                remaining -= rec.thought.len()
                    + rec.signature.as_ref().map_or(0, String::len)
                    + rec.visible.len();
                result.records.push(rec);
            } else {
                result.complete = false;
            }
        }
        drop(rows);
        if result.complete {
            if let Some(oldest) = oldest_candidate {
                // 同索引只检查窗口前是否有历史，不扫描或读取历史内容。
                let has_older: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM thinking_records WHERE session_key = ?1 AND id < ?2 LIMIT 1)",
                    params![session_key, oldest], |row| row.get(0)
                ).map_err(|e| e.to_string())?;
                result.complete = !has_older;
            }
        }
        result.records.reverse();
        Ok(result)
    })
}

/// 身份点查只返回标量 id；已消费 id 保留同一请求内重复轮次的次数语义。
pub(crate) fn find_thinking_record_id(
    session_key: &str,
    rec: &crate::proxy::thinking_store::ThinkingRecord,
    used_ids: &std::collections::HashSet<i64>,
) -> Result<Option<i64>, String> {
    let used_json = serde_json::to_string(used_ids).map_err(|e| e.to_string())?;
    let has_tools = !rec.tool_ids.is_empty() || !rec.tool_names.is_empty();
    with_thinking_db(|conn| {
        let find = |predicate: &str, value: &str| -> Result<Option<i64>, String> {
            let sql = format!("SELECT id FROM thinking_records WHERE session_key = ?1 AND {predicate} AND id NOT IN (SELECT value FROM json_each(?3)) ORDER BY id ASC LIMIT 1");
            conn.query_row(&sql, params![session_key, value, used_json], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())
        };
        let oldest =
            |current: Option<i64>, found: Option<i64>| current.into_iter().chain(found).min();
        if let Some(sig) = rec
            .signature
            .as_deref()
            .filter(|sig| crate::proxy::thinking_store::is_real_signature(sig))
        {
            let mut candidates = vec![sig.to_string()];
            if sig.as_bytes().first() == Some(&0x12) {
                if let Some(healed) = normalize_and_heal_signature(sig) {
                    candidates.push(healed);
                }
            }
            if crate::proxy::thinking_store::is_claude_signature(sig) {
                candidates.extend([
                    crate::proxy::thinking_store::ensure_google_claude_thought_signature(sig),
                    crate::proxy::thinking_store::ensure_raw_claude_thought_signature(sig),
                ]);
            }
            candidates.sort_unstable();
            candidates.dedup();
            // 同一级的编码候选共享历史 ASC 优先级，候选枚举顺序不决定身份。
            let mut earliest = None;
            for candidate in candidates {
                earliest = oldest(earliest, find("signature = ?2", &candidate)?);
            }
            if earliest.is_some() {
                return Ok(earliest);
            }
        }
        let mut candidates = Vec::new();
        for tool_id in &rec.tool_ids {
            candidates.push(crate::proxy::common::utils::normalize_tool_id(tool_id).into_owned());
            candidates.push(tool_id.clone());
        }
        candidates.sort_unstable();
        candidates.dedup();
        let mut earliest = None;
        for candidate in candidates {
            for predicate in ["causal_tool_id = ?2", "primary_tool_id = ?2"] {
                earliest = oldest(earliest, find(predicate, &candidate)?);
            }
            // secondary ID 按 JSON 字符串精确比较，避免 LIKE 通配符和转义造成错配。
            earliest = oldest(
                earliest,
                find(
                    "EXISTS (SELECT 1 FROM json_each(thinking_records.tool_ids) WHERE value = ?2)",
                    &candidate,
                )?,
            );
        }
        if earliest.is_some() {
            return Ok(earliest);
        }
        // 工具类型约束只属于 fingerprint fallback，与 RAM 匹配规则一致。
        find(
            if has_tools {
                "fingerprint = ?2 AND (tool_ids != '[]' OR tool_names != '[]')"
            } else {
                "fingerprint = ?2 AND tool_ids = '[]' AND tool_names = '[]'"
            },
            &rec.fingerprint,
        )
    })
}

pub(crate) fn load_thinking_by_id_bounded(
    session_key: &str,
    id: i64,
    budget: usize,
) -> Result<Option<PersistedThinkingRecord>, String> {
    with_thinking_db(|conn| {
        query_thinking_record(conn,
        "SELECT id, fingerprint, thought, signature, tool_ids, tool_names, visible FROM thinking_records WHERE session_key = ?1 AND id = ?2 LIMIT 1",
        session_key, &id.to_string(), budget, None)
    })
}

/// 点查沿用既有索引与自愈语义，只对完整且可接纳的记录写回修复。
fn query_thinking_record(
    conn: &Connection,
    sql: &str,
    session_key: &str,
    value: &str,
    budget: usize,
    heal_causal_id: Option<&str>,
) -> Result<Option<PersistedThinkingRecord>, String> {
    let mut stmt = conn.prepare_cached(sql).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(params![session_key, value])
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let id: i64 = row.get(0).map_err(|e| e.to_string())?;
    let Some(rec) = read_thinking_row(row, budget).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let raw_signature = row.get_ref(3).map_err(|e| e.to_string())?;
    if let Some(signature) = rec.signature.as_deref() {
        if raw_signature.as_str().ok() != Some(signature) {
            let _ = conn.execute(
                "UPDATE thinking_records SET signature = ?1 WHERE id = ?2",
                params![signature, id],
            );
        }
    }
    if let Some(causal_id) = heal_causal_id {
        let _ = conn.execute("UPDATE thinking_records SET causal_tool_id = ?1 WHERE id = ?2 AND causal_tool_id IS NULL", params![causal_id, id]);
    }
    Ok(Some(rec))
}

pub fn load_thinking_by_tool_id(
    session_key: &str,
    tool_id: &str,
) -> Result<Option<PersistedThinkingRecord>, String> {
    load_thinking_by_tool_id_bounded(
        session_key,
        tool_id,
        crate::proxy::thinking_store::MAX_BYTES_PER_SESSION,
    )
}

pub(crate) fn load_thinking_by_tool_id_bounded(
    session_key: &str,
    tool_id: &str,
    budget: usize,
) -> Result<Option<PersistedThinkingRecord>, String> {
    if session_key.is_empty() || tool_id.is_empty() || budget == 0 {
        return Ok(None);
    }
    let norm_id = crate::proxy::common::utils::normalize_tool_id(tool_id);
    let mut candidates = vec![norm_id.as_ref()];
    if norm_id.as_ref() != tool_id {
        candidates.push(tool_id);
    }
    with_thinking_db(|conn| {
        for candidate in candidates {
            for column in ["causal_tool_id", "primary_tool_id", "tool_ids"] {
                let (operator, value) = if column == "tool_ids" {
                    ("LIKE", format!("%\"{}\"%", candidate))
                } else {
                    ("=", candidate.to_owned())
                };
                let sql = format!("SELECT id, fingerprint, thought, signature, tool_ids, tool_names, visible FROM thinking_records WHERE session_key = ?1 AND {column} {operator} ?2 ORDER BY id DESC LIMIT 1");
                if let Some(rec) = query_thinking_record(
                    conn,
                    &sql,
                    session_key,
                    &value,
                    budget,
                    is_synthetic_tool_id(candidate).then_some(candidate),
                )? {
                    return Ok(Some(rec));
                }
            }
        }
        Ok(None)
    })
}

pub(crate) fn load_thinking_by_signature_bounded(
    session_key: &str,
    signature: &str,
    budget: usize,
) -> Result<Option<PersistedThinkingRecord>, String> {
    if session_key.is_empty() || signature.is_empty() || budget == 0 {
        return Ok(None);
    }
    with_thinking_db(|conn| {
        query_thinking_record(conn,
        "SELECT id, fingerprint, thought, signature, tool_ids, tool_names, visible FROM thinking_records WHERE session_key = ?1 AND signature = ?2 ORDER BY id DESC LIMIT 1",
        session_key, signature, budget, None)
    })
}

pub fn load_thinking_by_fingerprint(
    session_key: &str,
    fingerprint: &str,
) -> Result<Option<PersistedThinkingRecord>, String> {
    load_thinking_by_fingerprint_bounded(
        session_key,
        fingerprint,
        crate::proxy::thinking_store::MAX_BYTES_PER_SESSION,
    )
}

pub(crate) fn load_thinking_by_fingerprint_bounded(
    session_key: &str,
    fingerprint: &str,
    budget: usize,
) -> Result<Option<PersistedThinkingRecord>, String> {
    if session_key.is_empty() || fingerprint.is_empty() || budget == 0 {
        return Ok(None);
    }
    with_thinking_db(|conn| {
        query_thinking_record(conn,
        "SELECT id, fingerprint, thought, signature, tool_ids, tool_names, visible FROM thinking_records WHERE session_key = ?1 AND fingerprint = ?2 ORDER BY id DESC LIMIT 1",
        session_key, fingerprint, budget, None)
    })
}

pub fn touch_thinking_session(session_key: &str) -> Result<usize, String> {
    if session_key.is_empty() {
        return Ok(0);
    }
    with_thinking_db(|conn| {
        let now = chrono::Utc::now().timestamp_millis();
        // Touch a 1-row session table. Never UPDATE thinking_records here — that
        // rewrites every thought/visible TEXT blob for the session.
        conn.execute(
            "INSERT INTO thinking_sessions (session_key, last_accessed) VALUES (?1, ?2)
         ON CONFLICT(session_key) DO UPDATE SET last_accessed = excluded.last_accessed",
            params![session_key, now],
        )
        .map_err(|e| e.to_string())
    })
}

/// 只允许未发生持久变更的完整快照驱动 prune；变化或连接重建时返回 None。
pub(crate) fn prune_thinking_records_if_unchanged(
    session_key: &str,
    keep_fps: &[String],
    snapshot: ThinkingDbSnapshot,
) -> Result<Option<usize>, String> {
    if session_key.is_empty() || keep_fps.is_empty() {
        return Ok(None);
    }
    with_thinking_db(|conn| {
        let transaction =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
        if thinking_db_snapshot_at(&transaction)? != snapshot {
            return Ok(None);
        }
        let fps_json = serde_json::to_string(keep_fps).map_err(|e| e.to_string())?;
        let deleted = transaction.execute(
            "DELETE FROM thinking_records WHERE session_key = ?1 AND fingerprint NOT IN (SELECT value FROM json_each(?2))",
            params![session_key, fps_json],
        ).map_err(|e| e.to_string())?;
        transaction.commit().map_err(|e| e.to_string())?;
        Ok(Some(deleted))
    })
}

pub fn delete_thinking_records_for_session(session_key: &str) -> Result<usize, String> {
    with_thinking_db(|conn| {
        let _ = conn.execute(
            "DELETE FROM thinking_sessions WHERE session_key = ?1",
            params![session_key],
        );
        conn.execute(
            "DELETE FROM thinking_records WHERE session_key = ?1",
            params![session_key],
        )
        .map_err(|e| e.to_string())
    })
}

/// 精准净化思考记录表中的非法异构签名（保留思考文本与其它健康签名）
pub fn purge_foreign_signatures_for_session_with_model(
    session_key: &str,
    target_model: &str,
) -> Result<usize, String> {
    let is_gemini = target_model.to_lowercase().contains("gemini");
    let is_claude = target_model.to_lowercase().contains("claude");
    if (!is_gemini && !is_claude) || session_key.is_empty() {
        return Ok(0);
    }

    with_thinking_db(|conn| {
        let mut stmt = conn
        .prepare_cached("SELECT id, signature FROM thinking_records WHERE session_key = ?1 AND signature IS NOT NULL")
        .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![session_key], |row| {
                let id: i64 = row.get(0)?;
                let sig: String = row.get(1)?;
                Ok((id, sig))
            })
            .map_err(|e| e.to_string())?;

        let mut ids_to_null = Vec::new();
        for row in rows.flatten() {
            let (id, sig) = row;
            let is_foreign = if is_gemini {
                !crate::proxy::thinking_store::is_likely_gemini_signature(&sig)
            } else if is_claude {
                !crate::proxy::thinking_store::is_claude_signature(&sig)
            } else {
                false
            };
            if is_foreign {
                ids_to_null.push(id);
            }
        }

        let mut total_updated = 0;
        if !ids_to_null.is_empty() {
            let mut update_stmt = conn
                .prepare_cached("UPDATE thinking_records SET signature = NULL WHERE id = ?1")
                .map_err(|e| e.to_string())?;
            for id in ids_to_null {
                if let Ok(n) = update_stmt.execute(params![id]) {
                    total_updated += n;
                }
            }
        }

        Ok(total_updated)
    })
}

/// 兼容旧接口：默认按 Gemini 清洗
pub fn purge_foreign_signatures_for_session(session_key: &str) -> Result<usize, String> {
    purge_foreign_signatures_for_session_with_model(session_key, "gemini")
}

/// 全量清空思考块数据库 (仅清空 thinking_records / thinking_sessions / tool_signatures，绝不触碰 request_logs 日志)
pub fn clear_all_thinking_data() -> Result<usize, String> {
    let mut total_deleted = 0;
    // 1. 清空 thinking_store.db 中的记录与会话
    with_thinking_db(|conn| {
        let deleted = conn
            .execute("DELETE FROM thinking_records", [])
            .map_err(|e| e.to_string())?;
        total_deleted += deleted;
        let _ = conn.execute("DELETE FROM thinking_sessions", []);
        let _ = conn.execute("VACUUM", []);

        // 2. 清空 proxy_logs.db 中残留的历史工具签名表与陈旧思考表 (绝不触碰 request_logs)
        if let Ok(log_conn) = connect_db() {
            let _ = log_conn.execute("DELETE FROM tool_signatures", []);
            let _ = log_conn.execute("DELETE FROM thinking_records", []);
            let _ = log_conn.execute("DELETE FROM thinking_sessions", []);
        }

        Ok(total_deleted)
    })
}

pub fn get_thinking_records_count() -> Result<usize, String> {
    with_thinking_db(|conn| {
        conn.query_row("SELECT COUNT(*) FROM thinking_records", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())
    })
}

/// 一轮维护的已提交工作量；unfinished 与 deferred 分别表示剩余工作和本轮退让。
#[derive(Debug, Default)]
pub struct ThinkingCleanupStats {
    pub deleted_records: usize,
    pub deleted_sessions: usize,
    pub deleted_tools: usize,
    /// 成功提交批次读取的 session/orphan 元数据行数。
    pub scanned: usize,
    /// 尝试次数，包括最终回滚或退让的批次。
    pub batches: usize,
    pub committed_batches: usize,
    pub deferred: bool,
    pub defer_reason: Option<CleanupDeferReason>,
    pub unfinished: bool,
    pub orphan_cursor: Option<i64>,
    pub orphan_sweep_complete: bool,
    /// 当前单元上限，依次对应 session 记录、orphan 元数据、tool 签名。
    pub units: [usize; 3],
    pub elapsed: Duration,
    category_complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupDeferReason {
    Pending,
    LockBusy,
    SqliteBusy,
    Budget,
    Interrupted,
    RoundBudget,
}

impl ThinkingCleanupStats {
    fn deferred(reason: CleanupDeferReason) -> Self {
        Self {
            deferred: true,
            defer_reason: Some(reason),
            ..Default::default()
        }
    }
}

/// 串行维护循环持有的跨轮状态；完成一遍后重置完成标记，保留自适应单元。
pub struct ThinkingMaintenance {
    units: [usize; 3],
    complete: [bool; 3],
    next: usize,
    retention_days: Option<i64>,
}

impl Default for ThinkingMaintenance {
    fn default() -> Self {
        Self::with_limits(CleanupLimits::default())
    }
}

impl ThinkingMaintenance {
    /// 小时维护重新检查各类别；进行中的 orphan cursor 和自适应单元保持不变。
    pub fn reopen_completed_categories(&mut self) {
        self.complete = [false; 3];
    }

    fn with_limits(limits: CleanupLimits) -> Self {
        Self {
            units: [limits.rows, limits.window, limits.rows],
            complete: [false; 3],
            next: 0,
            retention_days: None,
        }
    }
}

#[derive(Debug)]
enum CleanupError {
    Deferred(CleanupDeferReason),
    Sqlite(rusqlite::Error),
}

impl From<rusqlite::Error> for CleanupError {
    fn from(error: rusqlite::Error) -> Self {
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                Self::Deferred(CleanupDeferReason::SqliteBusy)
            }
            Some(rusqlite::ErrorCode::OperationInterrupted) => {
                Self::Deferred(CleanupDeferReason::Interrupted)
            }
            _ => Self::Sqlite(error),
        }
    }
}

impl std::fmt::Display for CleanupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deferred(reason) => write!(formatter, "maintenance deferred: {reason:?}"),
            Self::Sqlite(error) => error.fmt(formatter),
        }
    }
}

#[cfg(test)]
impl CleanupError {
    fn sqlite_error_code(&self) -> Option<rusqlite::ErrorCode> {
        match self {
            Self::Deferred(CleanupDeferReason::SqliteBusy) => {
                Some(rusqlite::ErrorCode::DatabaseBusy)
            }
            Self::Deferred(_) => Some(rusqlite::ErrorCode::OperationInterrupted),
            Self::Sqlite(error) => error.sqlite_error_code(),
        }
    }
}

#[derive(Clone, Copy)]
struct CleanupLimits {
    rows: usize,
    window: usize,
    batches: usize,
    batch_budget: Duration,
    round_budget: Duration,
    vm_steps: i32,
    soft_budget: bool,
    #[cfg(test)]
    interrupt_after_checks: Option<usize>,
    #[cfg(test)]
    checkpoint_clock: Option<fn(usize, Instant) -> Instant>,
    #[cfg(test)]
    round_clock: Option<fn(usize) -> Duration>,
}

impl Default for CleanupLimits {
    fn default() -> Self {
        Self {
            rows: 128,
            window: 16,
            batches: 256,
            batch_budget: Duration::from_millis(100),
            round_budget: Duration::from_secs(2),
            vm_steps: 1000,
            soft_budget: false,
            #[cfg(test)]
            interrupt_after_checks: None,
            #[cfg(test)]
            checkpoint_clock: None,
            #[cfg(test)]
            round_clock: None,
        }
    }
}

#[derive(Clone, Copy)]
enum ThinkingBatch {
    Sessions,
    Orphans,
}

const EXPIRED_SESSIONS_SQL: &str = "SELECT session_key FROM thinking_sessions INDEXED BY idx_thinking_sessions_accessed WHERE last_accessed < ?1 ORDER BY last_accessed, session_key LIMIT ?2";
const SESSION_RECORDS_SQL: &str = "SELECT id FROM thinking_records INDEXED BY idx_thinking_rec_seq WHERE session_key = ?1 ORDER BY id LIMIT ?2";
const ORPHAN_WINDOW_SQL: &str = "SELECT id, session_key, COALESCE(last_accessed, created_at) FROM thinking_records WHERE id > ?1 ORDER BY id LIMIT ?2";

/// 批次预算覆盖 Rust 循环和 SQLite VM，不能中断磁盘系统调用或 busy 等待。
struct CleanupProgress<'a> {
    conn: &'a Connection,
    deadline: Instant,
    soft_budget: bool,
    interruption: Arc<AtomicUsize>,
    #[cfg(test)]
    checkpoints: std::cell::Cell<usize>,
    #[cfg(test)]
    limits: CleanupLimits,
}

impl<'a> CleanupProgress<'a> {
    fn install(conn: &'a Connection, limits: CleanupLimits, deadline: Instant) -> Self {
        let interruption = Arc::new(AtomicUsize::new(0));
        let signal = interruption.clone();
        #[cfg(test)]
        let mut checks = 0;
        conn.progress_handler(
            limits.vm_steps,
            Some(move || {
                if THINKING_PENDING.load(Ordering::SeqCst) > 0 {
                    signal.store(1, Ordering::Relaxed);
                    return true;
                }
                #[cfg(test)]
                {
                    checks += 1;
                    if !limits.soft_budget
                        && limits
                            .interrupt_after_checks
                            .is_some_and(|limit| checks >= limit)
                    {
                        signal.store(2, Ordering::Relaxed);
                        return true;
                    }
                }
                let expired = !limits.soft_budget && Instant::now() >= deadline;
                if expired {
                    signal.store(2, Ordering::Relaxed);
                }
                expired
            }),
        );
        Self {
            conn,
            deadline,
            soft_budget: limits.soft_budget,
            interruption,
            #[cfg(test)]
            checkpoints: std::cell::Cell::new(0),
            #[cfg(test)]
            limits,
        }
    }

    /// 短 statement 之间显式检查同一预算，不依赖 SQLite 的 VM 指令计数跨语句累计。
    fn checkpoint(&self) -> rusqlite::Result<()> {
        let now = Instant::now();
        #[cfg(test)]
        let now = {
            let checks = self.checkpoints.get() + 1;
            self.checkpoints.set(checks);
            self.limits
                .checkpoint_clock
                .map_or(now, |clock| clock(checks, self.deadline))
        };
        let reason = if THINKING_PENDING.load(Ordering::SeqCst) > 0 {
            1
        } else if !self.soft_budget && now >= self.deadline {
            2
        } else {
            0
        };
        if reason != 0 {
            self.interruption.store(reason, Ordering::Relaxed);
            Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERRUPT),
                Some("maintenance budget exhausted or request pending".into()),
            ))
        } else {
            Ok(())
        }
    }
}

impl Drop for CleanupProgress<'_> {
    fn drop(&mut self) {
        self.conn.progress_handler(0, None::<fn() -> bool>);
    }
}

fn cleanup_transaction<T>(
    conn: &Connection,
    limits: CleanupLimits,
    operation: impl FnOnce(&Connection, &CleanupProgress<'_>) -> rusqlite::Result<T>,
) -> Result<T, CleanupError> {
    let deadline = Instant::now() + limits.batch_budget;
    let tx = conn.unchecked_transaction()?;
    // 声明顺序保证 panic 时先清除回调，再由 transaction 回滚。
    let progress = CleanupProgress::install(&tx, limits, deadline);
    let result = progress
        .checkpoint()
        .and_then(|()| operation(&tx, &progress))
        .and_then(|value| {
            progress.checkpoint()?;
            tx.execute_batch("COMMIT")?;
            Ok(value)
        });
    let interruption = progress.interruption.load(Ordering::Relaxed);
    drop(progress);
    // Interrupted 可能已自动回滚；finish 会检查 autocommit，且不再受过期回调影响。
    tx.finish()?;
    result.map_err(|error| match interruption {
        1 => CleanupError::Deferred(CleanupDeferReason::Pending),
        2 => CleanupError::Deferred(CleanupDeferReason::Budget),
        _ => CleanupError::from(error),
    })
}

#[cfg(test)]
fn cleanup_is_deferred(error: &CleanupError) -> bool {
    matches!(error, CleanupError::Deferred(_))
}

fn cleanup_thinking_batch(
    conn: &Connection,
    cutoff: i64,
    kind: ThinkingBatch,
    mut limits: CleanupLimits,
) -> Result<ThinkingCleanupStats, CleanupError> {
    limits.soft_budget = match kind {
        ThinkingBatch::Sessions => limits.rows == 1,
        ThinkingBatch::Orphans => limits.window == 1,
    };
    cleanup_transaction(conn, limits, |conn, budget| {
        let mut stats = ThinkingCleanupStats::default();
        match kind {
            ThinkingBatch::Sessions => {
                let sessions = conn
                    .prepare_cached(EXPIRED_SESSIONS_SQL)?
                    .query_map(params![cutoff, limits.rows], |row| row.get::<_, String>(0))?
                    .map(|row| {
                        budget.checkpoint()?;
                        row
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                stats.scanned = sessions.len();
                stats.category_complete = sessions.is_empty();
                for session in sessions {
                    budget.checkpoint()?;
                    let ids = conn
                        .prepare_cached(SESSION_RECORDS_SQL)?
                        .query_map(
                            params![session, limits.rows - stats.deleted_records],
                            |row| row.get::<_, i64>(0),
                        )?
                        .map(|row| {
                            budget.checkpoint()?;
                            row
                        })
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    for id in ids {
                        budget.checkpoint()?;
                        stats.deleted_records +=
                            conn.execute("DELETE FROM thinking_records WHERE id = ?1", [id])?;
                    }
                    budget.checkpoint()?;
                    stats.deleted_sessions += conn.execute(
                        "DELETE FROM thinking_sessions WHERE session_key = ?1 AND last_accessed < ?2 AND NOT EXISTS (SELECT 1 FROM thinking_records WHERE session_key = ?1)",
                        params![session, cutoff],
                    )?;
                    if stats.deleted_records == limits.rows {
                        break;
                    }
                }
            }
            ThinkingBatch::Orphans => {
                let cursor = conn.query_row(
                    "SELECT CAST(v AS INTEGER) FROM thinking_meta WHERE k = 'cleanup_orphan_cursor'", [], |row| row.get::<_, i64>(0),
                ).optional()?.unwrap_or(0);
                // 先限定主键窗口，再查询所属 session；健康前缀也只能读取 window 条元数据。
                let rows = conn
                    .prepare_cached(ORPHAN_WINDOW_SQL)?
                    .query_map(params![cursor, limits.window], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })?
                    .map(|row| {
                        budget.checkpoint()?;
                        row
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                stats.scanned = rows.len();
                let mut next = 0;
                let mut processed = 0;
                for (id, session, accessed) in &rows {
                    budget.checkpoint()?;
                    next = *id;
                    processed += 1;
                    if *accessed < cutoff {
                        stats.deleted_records += conn.execute(
                            "DELETE FROM thinking_records WHERE id = ?1 AND COALESCE(last_accessed, created_at) < ?2 AND NOT EXISTS (SELECT 1 FROM thinking_sessions WHERE session_key = ?3)",
                            params![id, cutoff, session],
                        )?;
                    }
                    if stats.deleted_records == limits.rows {
                        break;
                    }
                }
                // 未处理的窗口尾部留给下一批，只有完整走到尾部才回绕。
                if processed == rows.len() && rows.len() < limits.window {
                    next = 0;
                    stats.category_complete = true;
                }
                budget.checkpoint()?;
                conn.execute(
                    "INSERT INTO thinking_meta (k, v) VALUES ('cleanup_orphan_cursor', ?1) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
                    [next.to_string()],
                )?;
                stats.orphan_cursor = Some(next);
            }
        }
        Ok(stats)
    })
}

fn try_cleanup_thinking_batch(
    cutoff: i64,
    kind: ThinkingBatch,
    limits: CleanupLimits,
) -> Result<ThinkingCleanupStats, String> {
    let slot = THINKING_DB.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::WouldBlock) => {
            return Ok(ThinkingCleanupStats::deferred(CleanupDeferReason::LockBusy))
        }
        Err(error) => return Err(format!("thinking db lock: {error}")),
    };
    if THINKING_PENDING.load(Ordering::SeqCst) > 0 {
        return Ok(ThinkingCleanupStats::deferred(CleanupDeferReason::Pending));
    }
    let db_path = get_thinking_db_path()?;
    if guard.as_ref().map(|(path, _)| path) != Some(&db_path) {
        // 首次 schema 初始化不受维护批次的短 deadline 限制。
        *guard = Some((db_path.clone(), open_thinking_db_at(&db_path)?));
        THINKING_DB_GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    let conn = &guard.as_ref().expect("thinking db connection").1;
    match cleanup_thinking_batch(conn, cutoff, kind, limits) {
        Ok(stats) => Ok(stats),
        Err(CleanupError::Deferred(reason)) => Ok(ThinkingCleanupStats::deferred(reason)),
        Err(error) => Err(format!("thinking cleanup: {error}")),
    }
}

fn cleanup_tools_batch(
    conn: &Connection,
    cutoff: i64,
    mut limits: CleanupLimits,
) -> Result<usize, CleanupError> {
    limits.soft_budget = limits.rows == 1;
    cleanup_transaction(conn, limits, |conn, _budget| {
        conn.execute(
        "DELETE FROM tool_signatures WHERE tool_id IN (SELECT tool_id FROM tool_signatures INDEXED BY idx_tool_sig_created WHERE created_at < ?1 ORDER BY created_at LIMIT ?2)",
        params![cutoff, limits.rows],
    )
    })
}

/// 每轮使用当前截止时间；已完成类别等待下一遍或小时维护重新开放。
pub fn cleanup_thinking_storage(
    days: i64,
    state: &mut ThinkingMaintenance,
) -> Result<ThinkingCleanupStats, String> {
    if state.retention_days != Some(days) {
        state.complete = [false; 3];
        state.retention_days = Some(days);
    }
    let cutoff = chrono::Utc::now().timestamp_millis() - (days * 24 * 3600 * 1000);
    cleanup_thinking_storage_with_limits(cutoff, CleanupLimits::default(), state)
}

fn cleanup_thinking_storage_with_limits(
    cutoff: i64,
    limits: CleanupLimits,
    state: &mut ThinkingMaintenance,
) -> Result<ThinkingCleanupStats, String> {
    if state.complete.iter().all(|complete| *complete) {
        state.complete = [false; 3];
    }
    let started = Instant::now();
    let mut total = ThinkingCleanupStats::default();
    let mut tools = None;
    for _ in 0..limits.batches {
        let Some(kind) = (0..3)
            .map(|offset| (state.next + offset) % 3)
            .find(|kind| !state.complete[*kind])
        else {
            break;
        };
        if THINKING_PENDING.load(Ordering::SeqCst) > 0 {
            total.deferred = true;
            total.defer_reason = Some(CleanupDeferReason::Pending);
            break;
        }
        let elapsed = started.elapsed();
        #[cfg(test)]
        let elapsed = limits
            .round_clock
            .map_or(elapsed, |clock| clock(total.batches));
        if elapsed >= limits.round_budget {
            total.deferred = true;
            total.defer_reason = Some(CleanupDeferReason::RoundBudget);
            break;
        }
        state.next = (kind + 1) % 3;
        total.batches += 1;
        let mut unit_limits = limits;
        if kind == 1 {
            unit_limits.window = state.units[kind];
        } else {
            unit_limits.rows = state.units[kind];
        }
        let result = match kind {
            0 => try_cleanup_thinking_batch(cutoff, ThinkingBatch::Sessions, unit_limits),
            1 => try_cleanup_thinking_batch(cutoff, ThinkingBatch::Orphans, unit_limits),
            _ => {
                let conn = match tools.as_ref() {
                    Some(conn) => conn,
                    None => tools.insert(connect_db()?),
                };
                match cleanup_tools_batch(conn, cutoff, unit_limits) {
                    Ok(deleted_tools) => Ok(ThinkingCleanupStats {
                        deleted_tools,
                        category_complete: deleted_tools < unit_limits.rows,
                        ..Default::default()
                    }),
                    Err(CleanupError::Deferred(reason)) => {
                        Ok(ThinkingCleanupStats::deferred(reason))
                    }
                    Err(error) => Err(format!("tool signature cleanup: {error}")),
                }
            }
        };
        let stats = result.map_err(|error| {
            format!(
                "{error}; committed_batches={}, scanned={}, deleted_records={}, deleted_tools={}, orphan_cursor={:?}",
                total.committed_batches, total.scanned, total.deleted_records, total.deleted_tools, total.orphan_cursor
            )
        })?;
        total.deleted_records += stats.deleted_records;
        total.deleted_sessions += stats.deleted_sessions;
        total.deleted_tools += stats.deleted_tools;
        total.scanned += stats.scanned;
        if stats.deferred {
            if stats.defer_reason == Some(CleanupDeferReason::Budget) {
                state.units[kind] = (state.units[kind] / 2).max(1);
            }
            total.deferred = true;
            total.defer_reason = stats.defer_reason;
            break;
        }
        total.committed_batches += 1;
        state.complete[kind] = stats.category_complete;
        if stats.orphan_cursor.is_some() {
            total.orphan_cursor = stats.orphan_cursor;
        }
    }
    total.unfinished = state.complete.iter().any(|complete| !complete);
    total.orphan_sweep_complete = state.complete[1];
    total.units = state.units;
    total.elapsed = started.elapsed();
    Ok(total)
}

pub fn apply_retention(policy: &LogRetentionConfig) -> Result<(usize, usize), String> {
    let _guard = LOG_WRITE_LOCK.lock().map_err(|e| e.to_string())?;
    let conn = connect_db()?;
    apply_retention_with_connection(&conn, policy)
}

fn apply_retention_with_connection(
    conn: &Connection,
    policy: &LogRetentionConfig,
) -> Result<(usize, usize), String> {
    // 请求体不再按时间强制清空，完全由容量上限与行数滑动窗口整体托管，保留完整报文
    let bodies_cleared = 0;

    // 注意：已移除基于 max_age_days 的按天整行删除逻辑，改为条数上限与空间上限滑动窗口淘汰
    let mut rows_deleted = 0;
    if policy.max_rows > 0 {
        rows_deleted += conn.execute(
            "DELETE FROM request_logs WHERE id NOT IN (SELECT id FROM request_logs ORDER BY timestamp DESC LIMIT ?1)",
            [policy.max_rows],
        ).map_err(|e| e.to_string())?;
    }

    // 按空间上限执行 30% 滑动窗口尾部淘汰
    let budget = policy.budget_bytes();
    if budget > 0 && disk_bytes(conn).unwrap_or(0) > budget {
        let (evicted, _) = evict_sliding_window(conn, budget)?;
        rows_deleted += evicted;
    }

    reclaim_space(conn)?;
    Ok((bodies_cleared, rows_deleted))
}

fn reclaim_space(conn: &Connection) -> Result<(), String> {
    let checkpoint = || -> Result<(), String> {
        let busy: i64 = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if busy != 0 {
            tracing::warn!("proxy log checkpoint busy");
        }
        Ok(())
    };
    checkpoint()?;

    let auto_vacuum: i64 = conn
        .pragma_query_value(None, "auto_vacuum", |r| r.get(0))
        .unwrap_or(0);

    if auto_vacuum == 2 {
        // Draining all free pages incrementally in batches
        for _ in 0..50 {
            let free: u64 = conn
                .pragma_query_value(None, "freelist_count", |r| r.get(0))
                .unwrap_or(0);
            if free == 0 {
                break;
            }
            let step = free.min(1000);
            let mut vacuum = conn
                .prepare(&format!("PRAGMA incremental_vacuum({})", step))
                .map_err(|e| e.to_string())?;
            let mut pages = vacuum.query([]).map_err(|e| e.to_string())?;
            while pages.next().map_err(|e| e.to_string())?.is_some() {}
            drop(pages);
        }
    }
    // 旧日志库的整库 VACUUM 由显式维护执行，自动清理只回收支持增量回收的数据库。

    checkpoint()
}

fn disk_bytes(conn: &Connection) -> Result<u64, String> {
    let path = conn.path().ok_or("proxy log database has no file path")?;
    [PathBuf::from(path), PathBuf::from(format!("{path}-wal"))]
        .iter()
        .try_fold(0u64, |total, path| match std::fs::metadata(path) {
            Ok(metadata) => Ok(total.saturating_add(metadata.len())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(total),
            Err(e) => Err(e.to_string()),
        })
}

pub fn get_proxy_db_disk_bytes() -> Result<u64, String> {
    let conn = connect_db()?;
    disk_bytes(&conn)
}

/// 滑动窗口尾部淘汰机制：
/// 当日志数据库达到或即将超过预算上限时，自动清理最尾部（最早）的日志，
/// 一次性挤出最大存储空间的 30%（即让体积回落到 <= 70% 预算内），
/// 并记录日志，随后返回清理的记录数与释放字节数。
pub fn evict_sliding_window(conn: &Connection, budget: u64) -> Result<(usize, u64), String> {
    if budget == 0 {
        return Ok((0, 0));
    }
    let before_bytes = disk_bytes(conn)?;
    // 一次挤出最大空间的 30% (即目标保留 <= 70% 的最大上限)
    let evict_quota = (budget as f64 * 0.30) as u64;
    let target_bytes = budget.saturating_sub(evict_quota);

    if before_bytes <= target_bytes {
        return Ok((0, 0));
    }

    let mut total_deleted: usize = 0;
    // 循环按批次从最尾部（最早记录，timestamp ASC）清理
    for _ in 0..100 {
        let deleted = conn
            .execute(
                "DELETE FROM request_logs WHERE id IN (
                SELECT id FROM request_logs ORDER BY timestamp ASC LIMIT 250
            )",
                [],
            )
            .map_err(|e| e.to_string())?;

        if deleted == 0 {
            break;
        }
        total_deleted += deleted;
        reclaim_space(conn)?;

        let current_bytes = disk_bytes(conn)?;
        if current_bytes <= target_bytes {
            break;
        }
    }

    let after_bytes = disk_bytes(conn)?;
    let freed_bytes = before_bytes.saturating_sub(after_bytes);

    if total_deleted > 0 {
        tracing::info!(
            "[ProxyLog Sliding Window] Disk budget reached ({:.2} GB limit). Evicted {} tail records, freed {:.2} MB (target 30% quota: {:.2} MB). Current size: {:.2} MB.",
            budget as f64 / 1_073_741_824.0,
            total_deleted,
            freed_bytes as f64 / 1_048_576.0,
            evict_quota as f64 / 1_048_576.0,
            after_bytes as f64 / 1_048_576.0
        );
    }

    Ok((total_deleted, freed_bytes))
}

fn projected_bytes(conn: &Connection, log_bytes: u64) -> Result<u64, String> {
    let free: u64 = conn
        .pragma_query_value(None, "freelist_count", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let page_size: u64 = conn
        .pragma_query_value(None, "page_size", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    // Free pages avoid database growth, but still need WAL frames during the transaction.
    Ok(disk_bytes(conn)?
        .saturating_add(log_bytes.saturating_mul(2))
        .saturating_add(log_bytes.saturating_sub(free.saturating_mul(page_size)))
        .saturating_add(64 * 1024))
}

fn make_room(conn: &Connection, budget: u64, log_bytes: u64) -> Result<(), String> {
    if budget == 0 {
        return Ok(());
    }
    if projected_bytes(conn, log_bytes)? <= budget {
        return Ok(());
    }
    reclaim_space(conn)?;
    if projected_bytes(conn, log_bytes)? <= budget {
        return Ok(());
    }

    let auto_vacuum: i64 = conn
        .pragma_query_value(None, "auto_vacuum", |r| r.get(0))
        .unwrap_or(0);
    // Legacy files cannot shrink: even reusing all free pages still needs WAL headroom.
    if auto_vacuum == 0
        && disk_bytes(conn)?
            .saturating_add(log_bytes.saturating_mul(2))
            .saturating_add(64 * 1024)
            > budget
    {
        return Err("legacy proxy log database cannot shrink within budget".to_string());
    }

    // 优先触发 30% 滑动窗口机制清理最尾部历史日志
    let (evicted, _) = evict_sliding_window(conn, budget)?;
    if evicted > 0 {
        reclaim_space(conn)?;
    }

    if projected_bytes(conn, log_bytes)? <= budget {
        return Ok(());
    }

    let target = budget.saturating_mul(7) / 10;
    // Bounded work per write, oldest bodies first, then oldest summaries. No full-body reads.
    for _ in 0..8 {
        let before = projected_bytes(conn, log_bytes)?;
        let cleared = conn.execute(
            "UPDATE request_logs SET request_body = NULL, upstream_request_body = NULL, response_body = NULL,
             request_headers = NULL, upstream_request_headers = NULL, response_headers = NULL WHERE id IN
             (SELECT id FROM request_logs WHERE request_body IS NOT NULL OR upstream_request_body IS NOT NULL OR response_body IS NOT NULL ORDER BY timestamp ASC LIMIT 64)", []
        ).map_err(|e| e.to_string())?;
        if cleared == 0 {
            conn.execute("DELETE FROM request_logs WHERE id IN (SELECT id FROM request_logs ORDER BY timestamp ASC LIMIT 64)", [])
                .map_err(|e| e.to_string())?;
        }
        reclaim_space(conn)?;
        let after = projected_bytes(conn, log_bytes)?;
        if after <= target {
            return Ok(());
        }
        if after >= before {
            break;
        }
    }
    if projected_bytes(conn, log_bytes)? <= budget {
        Ok(())
    } else {
        Err("proxy log disk budget exhausted".to_string())
    }
}

pub fn save_log(log: ProxyRequestLog) -> Result<(), String> {
    let _guard = LOG_WRITE_LOCK.lock().map_err(|e| e.to_string())?;
    // Read the file for every admitted write, including after a runtime budget change.
    let policy = crate::modules::config::load_app_config()?
        .proxy
        .log_retention;
    let conn = connect_db()?;
    save_log_with_connection(&conn, log, &policy)
}

fn save_log_with_connection(
    conn: &Connection,
    mut log: ProxyRequestLog,
    policy: &LogRetentionConfig,
) -> Result<(), String> {
    conn.busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|e| e.to_string())?;
    log.error = log
        .error
        .as_ref()
        .map(|error| error.chars().take(1024).collect());
    let budget = policy.budget_bytes();
    let summary_bytes = [&log.id, &log.method, &log.url]
        .iter()
        .map(|s| s.len() as u64)
        .sum::<u64>()
        + [
            &log.model,
            &log.mapped_model,
            &log.account_email,
            &log.client_ip,
            &log.error,
            &log.protocol,
            &log.username,
        ]
        .iter()
        .filter_map(|s| s.as_ref())
        .map(|s| s.len() as u64)
        .sum::<u64>()
        + 1024;
    let body_bytes = [
        &log.request_body,
        &log.upstream_request_body,
        &log.response_body,
        &log.request_headers,
        &log.upstream_request_headers,
        &log.response_headers,
    ]
    .iter()
    .filter_map(|s| s.as_ref())
    .map(|s| s.len() as u64)
    .sum::<u64>();
    let mut log_bytes = summary_bytes.saturating_add(body_bytes);
    if budget > 0 && log_bytes.saturating_mul(3).saturating_add(64 * 1024) > budget / 5 * 4 {
        log.request_body = None;
        log.upstream_request_body = None;
        log.response_body = None;
        log.request_headers = None;
        log.upstream_request_headers = None;
        log.response_headers = None;
        log_bytes = summary_bytes;
    }
    if budget > 0 && log_bytes.saturating_mul(3).saturating_add(64 * 1024) > budget {
        return Err("proxy log summary exceeds disk budget".to_string());
    }
    make_room(conn, budget, log_bytes)?;

    conn.execute(
        "INSERT INTO request_logs (id, timestamp, method, url, status, duration, model, error, request_body, upstream_request_body, response_body, input_tokens, output_tokens, cached_tokens, account_email, mapped_model, protocol, client_ip, username, request_headers, upstream_request_headers, response_headers, session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
        params![
            log.id,
            log.timestamp,
            log.method,
            log.url,
            log.status,
            log.duration,
            log.model,
            log.error,
            log.request_body,
            log.upstream_request_body,
            log.response_body,
            log.input_tokens,
            log.output_tokens,
            log.cached_tokens,
            log.account_email,
            log.mapped_model,
            log.protocol,
            log.client_ip,
            log.username,
            log.request_headers,
            log.upstream_request_headers,
            log.response_headers,
            log.session_id,
        ],
    ).map_err(|e| e.to_string())?;

    Ok(())
}

/// Get logs summary (without large request_body and response_body fields) with pagination
pub fn get_logs_summary(limit: usize, offset: usize) -> Result<Vec<ProxyRequestLog>, String> {
    let conn = connect_db()?;

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, method, url, status, duration, model, substr(error, 1, 1024),
                NULL as request_body, NULL as upstream_request_body, NULL as response_body,
                input_tokens, output_tokens, cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                NULL as request_headers, NULL as upstream_request_headers, NULL as response_headers,
                session_id
         FROM request_logs
         ORDER BY timestamp DESC
         LIMIT ?1 OFFSET ?2",
        )
        .map_err(|e| e.to_string())?;

    let logs_iter = stmt
        .query_map([limit, offset], map_request_log_row)
        .map_err(|e| e.to_string())?;

    let mut logs = Vec::new();
    for log in logs_iter {
        logs.push(log.map_err(|e| e.to_string())?);
    }
    Ok(logs)
}

/// Get logs (backward compatible, calls get_logs_summary)
pub fn get_logs(limit: usize) -> Result<Vec<ProxyRequestLog>, String> {
    get_logs_summary(limit, 0)
}

pub fn get_stats() -> Result<crate::proxy::monitor::ProxyStats, String> {
    let conn = connect_db()?;

    // Optimized: Use single query instead of three separate queries
    // Use COALESCE to handle NULL values when table is empty (SUM returns NULL for empty set)
    let (total_requests, success_count, error_count): (u64, u64, u64) = conn
        .query_row(
            "SELECT
            COUNT(*) as total,
            COALESCE(SUM(CASE WHEN status >= 200 AND status < 400 THEN 1 ELSE 0 END), 0) as success,
            COALESCE(SUM(CASE WHEN status < 200 OR status >= 400 THEN 1 ELSE 0 END), 0) as error
         FROM request_logs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| e.to_string())?;

    Ok(crate::proxy::monitor::ProxyStats {
        total_requests,
        success_count,
        error_count,
    })
}

/// Get single log detail (with request_body and response_body)
pub fn get_log_detail(log_id: &str) -> Result<ProxyRequestLog, String> {
    let conn = connect_db()?;

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, method, url, status, duration, model, error,
                request_body, upstream_request_body, response_body, input_tokens, output_tokens,
                cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                request_headers, upstream_request_headers, response_headers,
                session_id
         FROM request_logs
         WHERE id = ?1",
        )
        .map_err(|e| e.to_string())?;

    stmt.query_row([log_id], map_request_log_row)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod thinking_pack_tests {
    use super::*;

    #[test]
    fn pack_roundtrip_short_and_long() {
        let short = "hello thought";
        assert_eq!(unpack_thought(&pack_thought(short)), short);
        assert!(pack_thought(short).starts_with(THOUGHT_RAW_MAGIC));

        let long = "word ".repeat(2000);
        let packed = pack_thought(&long);
        assert!(
            packed.starts_with(THOUGHT_GZIP_MAGIC),
            "long thought should gzip"
        );
        assert!(packed.len() < long.len());
        assert_eq!(unpack_thought(&packed), long);
    }

    #[test]
    fn unpack_legacy_utf8() {
        assert_eq!(unpack_thought(b"plain old thought"), "plain old thought");
    }

    #[test]
    fn persist_visible_drops_tool_turns() {
        assert_eq!(
            persist_visible(&["call_1".to_string()], "I will run the tool"),
            ""
        );
        assert_eq!(persist_visible(&[], "hello"), "hello");
    }
}

#[cfg(test)]
mod tool_signature_tests {
    use super::*;
    use crate::proxy::monitor::prompt_log_tests::TestDataDir;

    #[test]
    fn tool_signature_misses_reuse_readonly_connection() {
        let _dir = TestDataDir::new();
        assert!(load_tool_signature("missing").is_err());
        assert!(!get_proxy_db_path().unwrap().exists());
        init_db().unwrap();
        let writer = connect_db().unwrap();
        assert_eq!(
            writer
                .pragma_query_value::<i64, _>(None, "auto_vacuum", |r| r.get(0))
                .unwrap(),
            2
        );
        let before: i64 = writer
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .unwrap();
        assert_eq!(load_tool_signature("missing").unwrap(), None);
        {
            let db = TOOL_SIGNATURE_DB.get().unwrap().lock().unwrap();
            let conn = &db.as_ref().unwrap().1;
            assert!(conn.is_readonly(rusqlite::DatabaseName::Main).unwrap());
            // A connection-local setting detects accidental reopening on a miss.
            conn.pragma_update(None, "cache_size", -1234).unwrap();
        }
        for _ in 0..32 {
            assert_eq!(load_tool_signature("missing").unwrap(), None);
        }
        {
            let db = TOOL_SIGNATURE_DB.get().unwrap().lock().unwrap();
            let conn = &db.as_ref().unwrap().1;
            assert_eq!(
                conn.pragma_query_value::<i64, _>(None, "cache_size", |r| r.get(0))
                    .unwrap(),
                -1234
            );
            assert!(conn.is_autocommit());
            assert_eq!(conn.total_changes(), 0);
        }
        let after: i64 = writer
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .unwrap();
        assert_eq!(after, before);
        TOOL_SIGNATURE_DB.get().unwrap().lock().unwrap().take();
    }

    #[test]
    fn tool_signature_reads_follow_writes_and_data_dir_changes() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        let signature = "s".repeat(60);
        assert_eq!(load_tool_signature("tool").unwrap(), None);
        save_tool_signature("tool", &signature).unwrap();
        assert_eq!(
            load_tool_signature("tool").unwrap(),
            Some(signature.clone())
        );
        let replacement = "r".repeat(60);
        save_tool_signature("tool", &replacement).unwrap();
        assert_eq!(
            load_tool_signature("tool").unwrap(),
            Some(replacement.clone())
        );
        {
            let _other_dir = TestDataDir::new();
            assert!(load_tool_signature("tool").is_err());
            init_db().unwrap();
            assert_eq!(load_tool_signature("tool").unwrap(), None);
            save_tool_signature("tool", &signature).unwrap();
            assert_eq!(load_tool_signature("tool").unwrap(), Some(signature));
            TOOL_SIGNATURE_DB.get().unwrap().lock().unwrap().take();
        }
        assert_eq!(load_tool_signature("tool").unwrap(), Some(replacement));
        TOOL_SIGNATURE_DB.get().unwrap().lock().unwrap().take();
    }
}

#[cfg(test)]
mod thinking_sqlite_tests {
    use super::*;
    use crate::proxy::monitor::prompt_log_tests::TestDataDir;

    #[test]
    fn cold_history_respects_configured_window() {
        let _dir = TestDataDir::new();
        let limit = crate::proxy::config::get_thinking_max_memory_turns();
        with_thinking_db(|conn| {
            for i in 0..limit + 2 {
                conn.execute("INSERT INTO thinking_records (session_key, fingerprint, thought, tool_ids, tool_names, visible, created_at) VALUES ('bounded', ?1, X'5241573178', '[]', '[]', '', 0)", [i.to_string()]).unwrap();
            }
            Ok(())
        }).unwrap();
        let records = load_thinking_records("bounded").unwrap();
        assert_eq!(records.len(), limit);
        assert_eq!(records.first().unwrap().fingerprint, "2");
        assert_eq!(records.last().unwrap().fingerprint, (limit + 1).to_string());
        assert_eq!(get_thinking_records_count().unwrap(), (limit + 2));
    }

    fn insert_bounded_fixture(conn: &Connection, fp: &str, thought: &[u8]) {
        conn.execute("INSERT INTO thinking_records (session_key, fingerprint, thought, signature, tool_ids, tool_names, visible, created_at, primary_tool_id) VALUES ('budget', ?1, ?2, ?3, ?4, '[]', '', 0, ?1)",
            params![fp, thought, "s".repeat(32), format!("[\"{fp}\"]")]).unwrap();
    }

    #[test]
    fn bounded_history_preserves_order_bytes_and_unloaded_rows() {
        let _dir = TestDataDir::new();
        with_thinking_db(|conn| {
            for fp in ["0", "1", "2", "3", "4"] {
                insert_bounded_fixture(conn, fp, b"RAW1abcdefgh");
            }
            Ok(())
        })
        .unwrap();
        assert!(
            load_thinking_history_bounded("empty", 5, 200)
                .unwrap()
                .complete
        );
        assert!(
            load_thinking_history_bounded("budget", 5, 200)
                .unwrap()
                .complete
        );
        assert!(
            !load_thinking_history_bounded("budget", 4, 200)
                .unwrap()
                .complete
        );
        assert!(
            !load_thinking_history_bounded("budget", 5, 199)
                .unwrap()
                .complete
        );
        let records = load_thinking_records_bounded("budget", 3, 80).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|r| r.fingerprint.as_str())
                .collect::<Vec<_>>(),
            ["3", "4"]
        );
        assert_eq!(get_thinking_records_count().unwrap(), 5);
        assert!(load_thinking_by_fingerprint_bounded("budget", "0", 40)
            .unwrap()
            .is_some());
        assert!(load_thinking_by_tool_id_bounded("budget", "0", 40)
            .unwrap()
            .is_some());
        assert!(
            load_thinking_by_signature_bounded("budget", &"s".repeat(32), 40)
                .unwrap()
                .is_some()
        );
        for result in [
            load_thinking_by_fingerprint_bounded("budget", "0", 39),
            load_thinking_by_tool_id_bounded("budget", "0", 39),
            load_thinking_by_signature_bounded("budget", &"s".repeat(32), 39),
        ] {
            assert!(result.unwrap().is_none());
        }
        with_thinking_db(|conn| {
            insert_bounded_fixture(conn, "5", &pack_thought(&"z".repeat(20_000)));
            insert_bounded_fixture(conn, "6", &pack_thought(&"z".repeat(20_000)));
            Ok(())
        })
        .unwrap();
        assert!(load_thinking_records_bounded("budget", 2, 256)
            .unwrap()
            .is_empty());
        assert_eq!(get_thinking_records_count().unwrap(), 7);
        assert_eq!(
            load_thinking_by_fingerprint("budget", "5")
                .unwrap()
                .unwrap()
                .thought
                .len(),
            20_000
        );
    }

    fn identity_fixture(
        fp: &str,
        ids: &[&str],
        signature: Option<&str>,
    ) -> crate::proxy::thinking_store::ThinkingRecord {
        crate::proxy::thinking_store::ThinkingRecord {
            persisted_id: Arc::default(),
            fingerprint: fp.into(),
            thought: "reasoning".into(),
            signature: signature.map(str::to_string),
            tool_ids: ids.iter().map(|s| s.to_string()).collect(),
            tool_names: vec![],
            visible: fp.into(),
        }
    }

    #[test]
    fn bounded_identity_tool_level_uses_oldest_across_all_candidates() {
        let _dir = TestDataDir::new();
        for (key, old_ids, incoming_ids) in [
            ("secondary", vec!["y", "x"], vec!["x"]),
            ("multiple", vec!["y"], vec!["x", "y"]),
        ] {
            let first =
                save_thinking_record_with_id(key, &identity_fixture("a", &old_ids, None), None)
                    .unwrap()
                    .unwrap();
            let last =
                save_thinking_record_with_id(key, &identity_fixture("b", &["x"], None), None)
                    .unwrap()
                    .unwrap();
            let incoming = identity_fixture("unmatched", &incoming_ids, None);
            assert_eq!(
                find_thinking_record_id(key, &incoming, &Default::default()).unwrap(),
                Some(first),
                "{key}"
            );
            assert_eq!(
                find_thinking_record_id(key, &incoming, &[first].into()).unwrap(),
                Some(last),
                "{key}"
            );
        }
    }

    #[test]
    fn bounded_identity_tool_ids_are_exact_json_values() {
        let _dir = TestDataDir::new();
        for (key, candidate, actual) in [
            ("percent", "call_%", "call_abc"),
            ("underscore", "call_x", "callax"),
            ("quoted", "call_\"quoted", "call_\"quoted"),
            ("backslash", "call_\\path", "call_\\path"),
        ] {
            let saved = save_thinking_record_with_id(
                key,
                &identity_fixture("a", &["primary", actual], None),
                None,
            )
            .unwrap()
            .unwrap();
            let incoming = identity_fixture("unmatched", &[candidate], None);
            assert_eq!(
                find_thinking_record_id(key, &incoming, &Default::default()).unwrap(),
                (candidate == actual).then_some(saved),
                "{key}"
            );
        }
    }

    #[test]
    fn bounded_identity_signature_level_uses_oldest_equivalent_encoding() {
        use base64::Engine;
        let _dir = TestDataDir::new();
        let raw = base64::engine::general_purpose::STANDARD
            .encode("claude-signature-with-enough-payload-for-tests");
        let wrapped = crate::proxy::thinking_store::ensure_google_claude_thought_signature(&raw);
        assert_ne!(raw, wrapped);
        let proto = format!("\u{12}{}", "signature-payload".repeat(3));
        let healed = normalize_and_heal_signature(&proto).unwrap();
        for (key, old_sig, new_sig) in [
            ("old-wrapped", &wrapped, &raw),
            ("old-raw", &raw, &wrapped),
            ("old-healed", &healed, &proto),
        ] {
            let first =
                save_thinking_record_with_id(key, &identity_fixture("a", &[], Some(old_sig)), None)
                    .unwrap()
                    .unwrap();
            let last =
                save_thinking_record_with_id(key, &identity_fixture("b", &[], Some(new_sig)), None)
                    .unwrap()
                    .unwrap();
            // legacy 原始 protobuf 行绕过保存时的自愈，验证等价候选之间的历史顺序。
            with_thinking_db(|conn| {
                conn.execute(
                    "UPDATE thinking_records SET signature = ?1 WHERE id = ?2",
                    params![new_sig, last],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
            let incoming = identity_fixture("unmatched", &[], Some(new_sig));
            assert_eq!(
                find_thinking_record_id(key, &incoming, &Default::default()).unwrap(),
                Some(first),
                "{key}"
            );
            assert_eq!(
                find_thinking_record_id(key, &incoming, &[first].into()).unwrap(),
                Some(last),
                "{key}"
            );
        }
    }

    #[test]
    fn bounded_capture_length_comparison_preserves_decode_fallback_and_read_limit() {
        for raw in [b"RAW1long thought".as_slice(), b"long thought".as_slice()] {
            assert!(thought_exceeds_capture_length(raw, 4));
            assert!(!thought_exceeds_capture_length(raw, "long thought".len()));
        }
        assert!(thought_exceeds_capture_length(b"RAW1\xff", 2));
        assert!(!thought_exceeds_capture_length(b"RAW1\xff", 3));
        let mut gzip = pack_thought(&"a".repeat(2_000));
        assert!(gzip.starts_with(THOUGHT_GZIP_MAGIC));
        assert!(thought_exceeds_capture_length(&gzip, 5));
        assert!(!thought_exceeds_capture_length(&gzip, 2_000));
        *gzip.last_mut().unwrap() ^= 1;
        // 输出先超预算时保守保留，不为验证尾部继续读取。
        assert!(thought_exceeds_capture_length(&gzip, 5));
        // 预算内已观察到损坏时沿用原始字节 fallback，不能直接判为更长。
        assert!(!thought_exceeds_capture_length(&gzip, 2_000));
        assert!(!thought_exceeds_capture_length(b"AGZ1invalid gzip", 64));
        assert!(thought_exceeds_capture_length(b"AGZ1invalid gzip", 3));
    }

    #[test]
    fn bounded_prune_snapshot_rejects_external_updates_and_connection_rebuilds() {
        let _dir = TestDataDir::new();
        with_thinking_db(|conn| {
            for fp in ["0", "1", "2"] {
                insert_bounded_fixture(conn, fp, b"RAW1old");
            }
            Ok(())
        })
        .unwrap();
        let token = thinking_db_snapshot().unwrap();
        let external = Connection::open(get_thinking_db_path().unwrap()).unwrap();
        external.execute("UPDATE thinking_records SET thought = X'524157316E6577' WHERE session_key = 'budget' AND fingerprint = '2'", []).unwrap();
        let after_update = thinking_db_snapshot().unwrap();
        assert_eq!(token.total_changes, after_update.total_changes);
        assert_ne!(token.data_version, after_update.data_version);
        assert!(
            prune_thinking_records_if_unchanged("budget", &["1".into(), "2".into()], token)
                .unwrap()
                .is_none()
        );
        drop(external);
        let before_rebuild = thinking_db_snapshot().unwrap();
        {
            let _other_dir = TestDataDir::new();
            thinking_db_snapshot().unwrap();
        }
        let rebuilt = thinking_db_snapshot().unwrap();
        assert_ne!(before_rebuild.generation, rebuilt.generation);
        assert!(prune_thinking_records_if_unchanged(
            "budget",
            &["1".into(), "2".into()],
            before_rebuild
        )
        .unwrap()
        .is_none());
        assert_eq!(get_thinking_records_count().unwrap(), 3);
        let valid = thinking_db_snapshot().unwrap();
        assert_eq!(
            prune_thinking_records_if_unchanged("budget", &["1".into(), "2".into()], valid)
                .unwrap(),
            Some(1)
        );
    }

    #[test]
    fn bounded_decoding_rejects_gzip_raw_legacy_and_lossy_expansion() {
        let mut source = std::io::Cursor::new(vec![0u8; 50_000]);
        let (decoded, result) = read_thought_bytes(&mut source, 64);
        assert!(result.is_ok());
        assert_eq!(source.position(), 65);
        assert_eq!(decoded.len(), 65);
        let compressed = pack_thought(&"a".repeat(50_000));
        assert!(compressed.starts_with(THOUGHT_GZIP_MAGIC));
        assert!(unpack_thought_bounded(&compressed, 64).is_none());
        assert_eq!(
            unpack_thought_bounded(&compressed, 50_000).unwrap().len(),
            50_000
        );
        assert!(unpack_thought_bounded(b"RAW1abcde", 4).is_none());
        assert!(unpack_thought_bounded(b"abcde", 4).is_none());
        for bytes in [&b"RAW1\xff\xff"[..], &b"\xff\xff"[..]] {
            assert!(unpack_thought_bounded(bytes, 5).is_none());
            assert_eq!(unpack_thought_bounded(bytes, 6).unwrap(), "��");
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&[0xff; 30]).unwrap();
        let mut invalid_utf8_gzip = THOUGHT_GZIP_MAGIC.to_vec();
        invalid_utf8_gzip.extend(encoder.finish().unwrap());
        assert!(unpack_thought_bounded(&invalid_utf8_gzip, 64).is_none());
        assert_eq!(
            unpack_thought_bounded(&invalid_utf8_gzip, 90)
                .unwrap()
                .len(),
            90
        );
    }

    #[test]
    fn bounded_rows_reject_large_auxiliary_fields_without_deleting_history() {
        let _dir = TestDataDir::new();
        with_thinking_db(|conn| {
            for (idx, column) in [
                "fingerprint",
                "signature",
                "tool_ids",
                "tool_names",
                "visible",
            ]
            .iter()
            .enumerate()
            {
                let fp = idx.to_string();
                insert_bounded_fixture(conn, &fp, b"RAW1x");
                conn.execute(
                    &format!(
                        "UPDATE thinking_records SET {column} = ?1 WHERE primary_tool_id = ?2"
                    ),
                    params!["x".repeat(1024), fp],
                )
                .unwrap();
            }
            insert_bounded_fixture(conn, "legacy", b"x");
            conn.execute(
                "UPDATE thinking_records SET thought = ?1 WHERE primary_tool_id = 'legacy'",
                ["x".repeat(1024)],
            )
            .unwrap();
            insert_bounded_fixture(conn, "raw", &pack_thought(&"x".repeat(100)));
            Ok(())
        })
        .unwrap();
        assert!(load_thinking_records_bounded("budget", 7, 64)
            .unwrap()
            .is_empty());
        for fp in ["0", "1", "2", "3", "4", "legacy", "raw"] {
            assert!(load_thinking_by_tool_id_bounded("budget", fp, 64)
                .unwrap()
                .is_none());
        }
        assert_eq!(get_thinking_records_count().unwrap(), 7);
    }

    #[test]
    fn test_thinking_record_deduplication_and_penetration_lookup() {
        let _dir = TestDataDir::new();

        let session_key = "test_tenant:sess-123456";
        let tool_id = "call_abc999";
        let real_sig = "s".repeat(60);

        // 1. 首次写入：实质思考 + tool_id
        save_thinking_record(
            session_key,
            "fp_turn1",
            "This is deep analytical thinking about rust code",
            Some(&real_sig),
            &[tool_id.to_string()],
            &[],
            "visible",
        )
        .unwrap();

        // 2. 二次写入相同 tool_id（例如客户端再次回传包含占位符的相同轮次）：绝不叠加新行，绝不将实质思考覆盖为占位符！
        save_thinking_record(
            session_key,
            "fp_turn1",
            "...",
            Some(&real_sig),
            &[tool_id.to_string()],
            &[],
            "visible",
        )
        .unwrap();

        // 3. 验证 SQLite 中仅存 1 行，且保留高质量思考
        let all = load_thinking_records(session_key).unwrap();
        assert_eq!(all.len(), 1, "Duplicate tool saves must be deduplicated!");
        assert_eq!(
            all[0].thought,
            "This is deep analytical thinking about rust code"
        );
        assert_eq!(all[0].signature, Some(real_sig.clone()));

        // 4. 精准穿透点查 tool_id
        let loaded = load_thinking_by_tool_id(session_key, tool_id).unwrap();
        assert!(loaded.is_some());
        let rec = loaded.unwrap();
        assert_eq!(
            rec.thought,
            "This is deep analytical thinking about rust code"
        );
        assert_eq!(rec.signature, Some(real_sig));

        // 5. 不存在的 tool_id 应当正确返回 None
        let missing = load_thinking_by_tool_id(session_key, "call_nonexistent").unwrap();
        assert!(missing.is_none());

        // 6. 纯文本指纹点查测试
        let text_fp = "fp_pure_text_1";
        save_thinking_record(
            session_key,
            text_fp,
            "Pure text reasoning",
            None,
            &[],
            &[],
            "pure text visible",
        )
        .unwrap();
        let loaded_text = load_thinking_by_fingerprint(session_key, text_fp).unwrap();
        assert!(loaded_text.is_some());
        assert_eq!(loaded_text.unwrap().thought, "Pure text reasoning");
    }

    #[test]
    fn test_signature_healing_and_write_back() {
        use base64::Engine;
        let _dir = TestDataDir::new();
        init_db().unwrap();

        let session_key = "test_tenant:sess-healing";
        let tool_id = "call_corrupted_1";

        // 构造一个典型的被错误解码为 UTF-8 原始 Protobuf 二进制的签名 (首字节 0x12)
        let raw_proto_bytes = [
            0x12, 0x26, 0x0a, 0x24, b'e', b'2', b'4', b'8', b'3', b'0', b'a', b'7', b'-', b'5',
            b'c', b'd', b'6', b'-', b'4', b'2', b'f', b'e', b'-', b'9', b'9', b'8', b'b', b'-',
            b'e', b'e', b'5', b'3', b'9', b'e', b'7', b'2', b'b', b'9', b'c', b'3',
        ];
        let raw_corrupted_sig = String::from_utf8(raw_proto_bytes.to_vec()).unwrap();
        let expected_base64 = base64::engine::general_purpose::STANDARD.encode(raw_proto_bytes);
        assert_eq!(
            expected_base64,
            "EiYKJGUyNDgzMGE3LTVjZDYtNDJmZS05OThiLWVlNTM5ZTcyYjljMw=="
        );

        // 1. normalize_and_heal_signature 单测
        assert_eq!(
            normalize_and_heal_signature(&raw_corrupted_sig),
            Some(expected_base64.clone())
        );
        assert_eq!(
            normalize_and_heal_signature(&expected_base64),
            Some(expected_base64.clone())
        );
        assert_eq!(normalize_and_heal_signature(SENTINEL_SIGNATURE), None);
        assert_eq!(normalize_and_heal_signature("short"), None);

        // 2. save_tool_signature 会自动自愈为 Base64 存储
        save_tool_signature(tool_id, &raw_corrupted_sig).unwrap();
        let loaded_tool_sig = load_tool_signature(tool_id).unwrap();
        assert_eq!(loaded_tool_sig, Some(expected_base64.clone()));

        // 3. 模拟底层 SQLite 已经脏存了原始二进制签名的历史数据
        let conn = connect_db().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO tool_signatures (tool_id, signature, created_at) VALUES (?1, ?2, ?3)",
            params!["call_legacy_dirty", &raw_corrupted_sig, 1000],
        ).unwrap();
        drop(conn);

        // load_tool_signature 读出时自动识别并修复，且反向写回 SQLite
        let loaded_dirty = load_tool_signature("call_legacy_dirty").unwrap();
        assert_eq!(loaded_dirty, Some(expected_base64.clone()));

        // 验证 SQLite 中确实已被写回替换为标准 Base64 格式
        let conn = connect_db().unwrap();
        let in_db: String = conn
            .query_row(
                "SELECT signature FROM tool_signatures WHERE tool_id = 'call_legacy_dirty'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(in_db, expected_base64);

        // 4. thinking_records 自愈与反向写回测试
        let think_conn = thinking_db().unwrap();
        think_conn.execute(
            "INSERT INTO thinking_records (session_key, fingerprint, thought, signature, tool_ids, tool_names, visible, created_at, primary_tool_id, causal_tool_id)
             VALUES (?1, 'fp_dirty', ?2, ?3, '[\"call_corrupted_1\"]', '[]', 'vis', 1000, 'call_corrupted_1', 'call_corrupted_1')",
            params![session_key, pack_thought("thinking content"), &raw_corrupted_sig],
        ).unwrap();
        drop(think_conn);

        // load_thinking_by_tool_id 点查时触发反向自愈写回
        let loaded_rec = load_thinking_by_tool_id(session_key, "call_corrupted_1")
            .unwrap()
            .unwrap();
        assert_eq!(loaded_rec.signature, Some(expected_base64.clone()));

        // 验证 thinking_records 表中 signature 字段已被更新为自愈后的 Base64
        let think_conn = thinking_db().unwrap();
        let sig_in_db: String = think_conn
            .query_row(
                "SELECT signature FROM thinking_records WHERE session_key = ?1 AND primary_tool_id = 'call_corrupted_1'",
                params![session_key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sig_in_db, expected_base64);
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    use crate::proxy::config::LogRetentionConfig;
    use rusqlite::Connection;

    #[test]
    fn compat_legacy_log_upgrade_preserves_rows_bodies_and_unlimited_writes() {
        use crate::proxy::monitor::prompt_log_tests::{sample_log, TestDataDir};
        let _dir = TestDataDir::new();
        let conn = Connection::open(get_proxy_db_path().unwrap()).unwrap();
        conn.execute_batch("CREATE TABLE request_logs (id TEXT PRIMARY KEY, timestamp INTEGER, method TEXT, url TEXT, status INTEGER, duration INTEGER, model TEXT, error TEXT, response_body TEXT)").unwrap();
        conn.execute("INSERT INTO request_logs (id, timestamp, method, url, status, duration, response_body) VALUES ('legacy', 1, 'POST', '/v1/chat/completions', 200, 1, 'historical body')", []).unwrap();
        drop(conn);
        init_db().unwrap();
        let conn = connect_db().unwrap();
        assert_eq!(
            conn.pragma_query_value::<i64, _>(None, "auto_vacuum", |r| r.get(0))
                .unwrap(),
            0
        );
        let policy = LogRetentionConfig {
            max_rows: 0,
            max_disk_mb: 0,
            max_storage_gb: 0.0,
            ..LogRetentionConfig::default()
        };
        save_log_with_connection(&conn, sample_log("new-unlimited", 1024), &policy).unwrap();
        assert_eq!(
            apply_retention_with_connection(&conn, &policy).unwrap(),
            (0, 0)
        );
        assert_eq!(
            get_log_detail("legacy").unwrap().response_body.as_deref(),
            Some("historical body")
        );
        assert_eq!(
            get_log_detail("new-unlimited").unwrap().response_body,
            Some("错".repeat(1024))
        );
        assert_eq!(
            conn.pragma_query_value::<i64, _>(None, "auto_vacuum", |r| r.get(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn prompt_log_disk_budget_cleanup_and_live_config_reload() {
        use crate::proxy::monitor::prompt_log_tests::{sample_log, TestDataDir};
        let _dir = TestDataDir::new();
        init_db().unwrap();
        let mut config = crate::modules::config::load_app_config().unwrap();
        assert_eq!(
            serde_json::from_str::<LogRetentionConfig>("{}")
                .unwrap()
                .max_disk_mb,
            1024
        );
        config.proxy.log_retention.max_disk_mb = 8;
        config.proxy.log_retention.max_storage_gb = 0.0;
        crate::modules::config::save_app_config(&config).unwrap();
        save_log(sample_log("old", 300_000)).unwrap();
        let conn = connect_db().unwrap();
        reclaim_space(&conn).unwrap();
        assert!(disk_bytes(&conn).unwrap() > 1024 * 1024);
        config.proxy.log_retention.max_disk_mb = 1;
        config.proxy.log_retention.max_storage_gb = 0.0;
        crate::modules::config::save_app_config(&config).unwrap();
        save_log(sample_log("new", 4096)).unwrap();
        assert!(get_log_detail("old").is_err());
        assert_eq!(
            get_log_detail("new").unwrap().response_body,
            Some("错".repeat(4096))
        );
        assert!(disk_bytes(&conn).unwrap() <= 1024 * 1024);
        save_log(sample_log("oversize", 400_000)).unwrap();
        assert!(get_log_detail("oversize").unwrap().response_body.is_none());
        let zero_budget_policy = LogRetentionConfig {
            max_disk_mb: 0,
            max_storage_gb: 0.0,
            ..config.proxy.log_retention
        };
        save_log_with_connection(&conn, sample_log("unlimited", 100), &zero_budget_policy).unwrap();
        assert_eq!(
            get_log_detail("unlimited").unwrap().response_body,
            Some("错".repeat(100))
        );
    }

    #[test]
    fn prompt_log_legacy_headroom_rejection_preserves_history_on_retries() {
        use crate::proxy::monitor::prompt_log_tests::TestDataDir;
        let _dir = TestDataDir::new();
        let conn = Connection::open(get_proxy_db_path().unwrap()).unwrap();
        conn.execute_batch("CREATE TABLE request_logs (id TEXT PRIMARY KEY, timestamp INTEGER, method TEXT, url TEXT, status INTEGER, duration INTEGER, model TEXT, error TEXT, response_body TEXT)").unwrap();
        assert_eq!(
            conn.pragma_query_value::<i64, _>(None, "auto_vacuum", |r| r.get(0))
                .unwrap(),
            0
        );
        conn.execute_batch(
            "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i + 1 FROM n WHERE i < 128)
             INSERT INTO request_logs (id, timestamp, response_body)
             SELECT CAST(i AS TEXT), i, zeroblob(8192) FROM n;",
        )
        .unwrap();
        reclaim_space(&conn).unwrap();
        let before = disk_bytes(&conn).unwrap();
        let budget = 2 * 1024 * 1024;
        let log_bytes = 500_000;
        assert!(before < budget);
        assert!(before + 2 * log_bytes + 64 * 1024 > budget);
        assert!(3 * log_bytes + 64 * 1024 <= budget / 5 * 4);

        for _ in 0..6 {
            assert!(make_room(&conn, budget, log_bytes).is_err());
            let counts: (i64, i64) = conn
                .query_row(
                    "SELECT COUNT(*), COUNT(response_body) FROM request_logs",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(counts, (128, 128));
            assert_eq!(disk_bytes(&conn).unwrap(), before);
        }
    }

    #[test]
    fn prompt_log_reclaims_free_pages_before_deleting_summaries() {
        use crate::proxy::monitor::prompt_log_tests::{sample_log, TestDataDir};
        let _dir = TestDataDir::new();
        init_db().unwrap();
        let conn = connect_db().unwrap();
        conn.execute_batch(
            "INSERT INTO request_logs (id, timestamp, response_body) VALUES
             ('old-1', 1, zeroblob(2097152)), ('old-2', 2, zeroblob(2097152)),
             ('old-3', 3, zeroblob(2097152));",
        )
        .unwrap();
        reclaim_space(&conn).unwrap();
        assert!(disk_bytes(&conn).unwrap() > 6 * 1024 * 1024);
        let policy = LogRetentionConfig {
            max_disk_mb: 1,
            max_storage_gb: 0.0,
            ..LogRetentionConfig::default()
        };

        save_log_with_connection(&conn, sample_log("new", 100), &policy).unwrap();

        let counts: (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COUNT(response_body) FROM request_logs WHERE id != 'new'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (0, 0));
        assert_eq!(
            get_log_detail("new").unwrap().response_body,
            Some("错".repeat(100))
        );
        assert!(disk_bytes(&conn).unwrap() <= 1024 * 1024);
    }

    #[test]
    fn clears_old_bodies_and_limits_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE request_logs (id TEXT PRIMARY KEY, timestamp INTEGER, request_body TEXT, upstream_request_body TEXT, response_body TEXT)").unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO request_logs VALUES ('retained-with-old-body', ?1, 'request', NULL, 'response')",
            [now - 25 * 3600 * 1000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO request_logs VALUES ('new-1', ?1, NULL, NULL, NULL)",
            [now - 30 * 3600 * 1000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO request_logs VALUES ('deleted-1', ?1, NULL, NULL, NULL)",
            [now - 35 * 3600 * 1000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO request_logs VALUES ('deleted-2', ?1, NULL, NULL, NULL)",
            [now - 40 * 3600 * 1000],
        )
        .unwrap();
        let policy = LogRetentionConfig {
            max_body_age_hours: 24,
            max_age_days: 30,
            max_rows: 2,
            ..LogRetentionConfig::default()
        };
        let (cleared, deleted) = apply_retention_with_connection(&conn, &policy).unwrap();
        assert_eq!(cleared, 0);
        assert_eq!(deleted, 2);
        let body: Option<String> = conn
            .query_row(
                "SELECT request_body FROM request_logs WHERE id = 'retained-with-old-body'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(body, Some("request".to_string()));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM request_logs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}

/// Cleanup old logs (keep last N days)
pub fn cleanup_old_logs(days: i64) -> Result<usize, String> {
    let conn = connect_db()?;

    // Note: Request log timestamp is stored in milliseconds (chrono::Utc::now().timestamp_millis())
    let cutoff_timestamp_ms = chrono::Utc::now().timestamp_millis() - (days * 24 * 3600 * 1000);

    let deleted = conn
        .execute(
            "DELETE FROM request_logs WHERE timestamp < ?1",
            [cutoff_timestamp_ms],
        )
        .map_err(|e| e.to_string())?;

    // Only execute VACUUM when substantial rows were deleted to avoid saturating disk I/O on startup
    if deleted >= 500 {
        if let Err(e) = conn.execute("VACUUM", []) {
            tracing::warn!("VACUUM failed after log cleanup: {}", e);
        }
    }

    Ok(deleted)
}

/// Limit maximum log count (keep newest N records)
#[allow(dead_code)]
pub fn limit_max_logs(max_count: usize) -> Result<usize, String> {
    let conn = connect_db()?;

    let deleted = conn
        .execute(
            "DELETE FROM request_logs WHERE id NOT IN (
            SELECT id FROM request_logs ORDER BY timestamp DESC LIMIT ?1
        )",
            [max_count],
        )
        .map_err(|e| e.to_string())?;

    // Only execute VACUUM when substantial rows were deleted
    if deleted >= 500 {
        if let Err(e) = conn.execute("VACUUM", []) {
            tracing::warn!("VACUUM failed after limit_max_logs: {}", e);
        }
    }

    Ok(deleted)
}

pub fn clear_logs() -> Result<(), String> {
    let _guard = LOG_WRITE_LOCK.lock().map_err(|e| e.to_string())?;
    let conn = connect_db()?;
    conn.execute("DELETE FROM request_logs", [])
        .map_err(|e| e.to_string())?;
    // Full vacuum to reclaim all disk space immediately
    let _ = conn.execute("VACUUM", []);
    let _ = conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");
    Ok(())
}

/// Get total count of logs in database
pub fn get_logs_count() -> Result<u64, String> {
    let conn = connect_db()?;

    let count: u64 = conn
        .query_row("SELECT COUNT(*) FROM request_logs", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;

    Ok(count)
}

/// Get count of logs matching search filter
/// filter: search text to match in url, method, model, or status
/// errors_only: if true, only count logs with status < 200 or >= 400
pub fn get_logs_count_filtered(filter: &str, errors_only: bool) -> Result<u64, String> {
    let conn = connect_db()?;

    let filter_pattern = format!("%{}%", filter);

    let sql = if errors_only {
        "SELECT COUNT(*) FROM request_logs WHERE (status < 200 OR status >= 400)"
    } else if filter.is_empty() {
        "SELECT COUNT(*) FROM request_logs"
    } else {
        "SELECT COUNT(*) FROM request_logs WHERE
            (url LIKE ?1 OR method LIKE ?1 OR model LIKE ?1 OR CAST(status AS TEXT) LIKE ?1 OR account_email LIKE ?1)"
    };

    let count: u64 = if filter.is_empty() && !errors_only {
        conn.query_row(sql, [], |row| row.get(0))
    } else if errors_only {
        conn.query_row(sql, [], |row| row.get(0))
    } else {
        conn.query_row(sql, [&filter_pattern], |row| row.get(0))
    }
    .map_err(|e| e.to_string())?;

    Ok(count)
}

/// Get logs with search filter and pagination
/// filter: search text to match in url, method, model, or status
/// errors_only: if true, only return logs with status < 200 or >= 400
pub fn get_logs_filtered(
    filter: &str,
    errors_only: bool,
    limit: usize,
    offset: usize,
) -> Result<Vec<ProxyRequestLog>, String> {
    let conn = connect_db()?;

    let filter_pattern = format!("%{}%", filter);

    let sql = if errors_only {
        "SELECT id, timestamp, method, url, status, duration, model, substr(error, 1, 1024),
                NULL as request_body, NULL as upstream_request_body, NULL as response_body,
                input_tokens, output_tokens, cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                NULL as request_headers, NULL as upstream_request_headers, NULL as response_headers,
                session_id
         FROM request_logs
         WHERE (status < 200 OR status >= 400)
         ORDER BY timestamp DESC
         LIMIT ?1 OFFSET ?2"
    } else if filter.is_empty() {
        "SELECT id, timestamp, method, url, status, duration, model, substr(error, 1, 1024),
                NULL as request_body, NULL as upstream_request_body, NULL as response_body,
                input_tokens, output_tokens, cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                NULL as request_headers, NULL as upstream_request_headers, NULL as response_headers,
                session_id
         FROM request_logs
         ORDER BY timestamp DESC
         LIMIT ?1 OFFSET ?2"
    } else {
        "SELECT id, timestamp, method, url, status, duration, model, substr(error, 1, 1024),
                NULL as request_body, NULL as upstream_request_body, NULL as response_body,
                input_tokens, output_tokens, cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                NULL as request_headers, NULL as upstream_request_headers, NULL as response_headers,
                session_id
         FROM request_logs
         WHERE (url LIKE ?3 OR method LIKE ?3 OR model LIKE ?3 OR CAST(status AS TEXT) LIKE ?3 OR account_email LIKE ?3 OR client_ip LIKE ?3 OR session_id LIKE ?3)
         ORDER BY timestamp DESC
         LIMIT ?1 OFFSET ?2"
    };

    let logs: Vec<ProxyRequestLog> = if filter.is_empty() && !errors_only {
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let logs_iter = stmt
            .query_map([limit, offset], map_request_log_row)
            .map_err(|e| e.to_string())?;
        logs_iter.filter_map(|r| r.ok()).collect()
    } else if errors_only {
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let logs_iter = stmt
            .query_map([limit, offset], map_request_log_row)
            .map_err(|e| e.to_string())?;
        logs_iter.filter_map(|r| r.ok()).collect()
    } else {
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let logs_iter = stmt
            .query_map(
                rusqlite::params![limit, offset, filter_pattern],
                map_request_log_row,
            )
            .map_err(|e| e.to_string())?;
        logs_iter.filter_map(|r| r.ok()).collect()
    };

    Ok(logs)
}

/// Get all logs with full details for export
pub fn get_all_logs_for_export() -> Result<Vec<ProxyRequestLog>, String> {
    let conn = connect_db()?;

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, method, url, status, duration, model, error,
                request_body, upstream_request_body, response_body, input_tokens, output_tokens,
                cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                request_headers, upstream_request_headers, response_headers,
                session_id
         FROM request_logs
         ORDER BY timestamp DESC",
        )
        .map_err(|e| e.to_string())?;

    let logs_iter = stmt
        .query_map([], map_request_log_row)
        .map_err(|e| e.to_string())?;

    let mut logs = Vec::new();
    for log in logs_iter {
        logs.push(log.map_err(|e| e.to_string())?);
    }
    Ok(logs)
}

// ... existing code ...

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IpTokenStats {
    pub client_ip: String,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub request_count: i64,
    pub username: Option<String>,
}

/// Get token usage grouped by IP
pub fn get_token_usage_by_ip(limit: usize, hours: i64) -> Result<Vec<IpTokenStats>, String> {
    let conn = connect_db()?;

    // Fix: Database stores timestamp in milliseconds, but we were calculating 'since' in seconds
    // Convert 'hours' to milliseconds
    let since = chrono::Utc::now().timestamp_millis() - (hours * 3600 * 1000);

    // [FIX] 不再从 request_logs 表获取 username，因为该字段可能为空
    // 先获取 IP 统计数据，然后再单独查询每个 IP 的用户名
    let mut stmt = conn
        .prepare(
            "SELECT
            client_ip,
            COALESCE(SUM(input_tokens), 0) + COALESCE(SUM(output_tokens), 0) as total,
            COALESCE(SUM(input_tokens), 0) as input,
            COALESCE(SUM(output_tokens), 0) as output,
            COUNT(*) as cnt
         FROM request_logs
         WHERE timestamp >= ?1 AND client_ip IS NOT NULL AND client_ip != ''
         GROUP BY client_ip
         ORDER BY total DESC
         LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![since, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut stats = Vec::new();
    for row in rows {
        let (client_ip, total_tokens, input_tokens, output_tokens, request_count) =
            row.map_err(|e| e.to_string())?;

        // 从 user_token_db 获取该 IP 关联的用户名
        // 这比从 request_logs 获取更可靠，因为 token_ip_bindings 表在每次 User Token 使用时都会更新
        let username =
            crate::modules::user_token_db::get_username_for_ip(&client_ip).unwrap_or(None);

        stats.push(IpTokenStats {
            client_ip,
            total_tokens,
            input_tokens,
            output_tokens,
            request_count,
            username,
        });
    }

    Ok(stats)
}

#[cfg(test)]
mod thinking_maintenance_tests {
    use super::*;
    use crate::proxy::monitor::prompt_log_tests::TestDataDir;
    use std::sync::{mpsc, Arc, Barrier};
    use std::time::Duration;

    #[test]
    fn thinking_facade_waiters_do_not_starve_http_health() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let address = runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(
                    listener,
                    axum::Router::new().route("/health", axum::routing::get(|| async { "ok" })),
                )
                .await
                .unwrap();
            });
            address
        });
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let owner = std::thread::spawn(move || {
            let _guard = hold_thinking_db_for_test();
            locked_tx.send(()).unwrap();
            // 独立线程负责兜底解锁，旧阻塞路径也不会挂死测试。
            let _ = release_rx.recv_timeout(Duration::from_secs(3));
        });
        locked_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let entered = Arc::new(Barrier::new(3));
        let requests: Vec<_> = (0..2)
            .map(|_| {
                let entered = entered.clone();
                runtime.spawn(async move {
                    entered.wait();
                    get_thinking_records_count()
                })
            })
            .collect();
        entered.wait();
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        client
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        let health = client.read_to_string(&mut response);
        release_tx.send(()).unwrap();
        owner.join().unwrap();
        runtime.block_on(async {
            for request in requests {
                request.await.unwrap().unwrap();
            }
        });
        assert!(
            health.is_ok(),
            "HTTP health stalled behind thinking DB waiters: {health:?}"
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    }
    pub(super) static BEFORE_LOCK: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);
    pub(super) static BEFORE_TOUCH: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

    pub(super) fn run_hook(slot: &Mutex<Option<Box<dyn FnOnce() + Send>>>) {
        let hook = slot.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
    }

    fn record(conn: &Connection, session: &str, created: i64, accessed: Option<i64>) -> i64 {
        conn.execute("INSERT INTO thinking_records (session_key, fingerprint, thought, tool_ids, tool_names, visible, created_at, last_accessed) VALUES (?1, 'fp', X'5241573178', '[]', '[]', '', ?2, ?3)", params![session, created, accessed]).unwrap();
        conn.last_insert_rowid()
    }

    fn session(conn: &Connection, key: &str, accessed: i64) {
        conn.execute(
            "INSERT INTO thinking_sessions VALUES (?1, ?2)",
            params![key, accessed],
        )
        .unwrap();
    }

    fn exists(conn: &Connection, id: i64) -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM thinking_records WHERE id = ?1)",
            [id],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn local_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_thinking_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn thinking_facade_supports_all_runtime_contexts_and_propagates_errors() {
        let _dir = TestDataDir::new();
        let check = || {
            assert_eq!(
                with_thinking_db(|conn| conn
                    .query_row("SELECT 7", [], |row| row.get::<_, i64>(0))
                    .map_err(|e| e.to_string()))
                .unwrap(),
                7
            );
            assert!(with_thinking_db(|conn| conn
                .execute("SELECT missing_column", [])
                .map_err(|e| e.to_string()))
            .unwrap_err()
            .contains("missing_column"));
        };
        check();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                check();
            });
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            check();
            tokio::task::spawn_blocking(check).await.unwrap();
        });
        assert_eq!(THINKING_PENDING.load(Ordering::SeqCst), 0);
        with_thinking_db(|conn| {
            conn.execute("DROP TABLE thinking_records", [])
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(get_thinking_records_count()
            .unwrap_err()
            .contains("thinking_records"));
    }

    #[test]
    fn thinking_cleanup_ttl_matrix_and_empty_sessions() {
        let conn = local_db();
        session(&conn, "active", 101);
        session(&conn, "expired", 99);
        session(&conn, "equal", 100);
        session(&conn, "empty", 99);
        let active = record(&conn, "active", 1, None);
        let expired_old = record(&conn, "expired", 1, None);
        let expired_new = record(&conn, "expired", 101, None);
        let equal_session = record(&conn, "equal", 1, None);
        let orphan_old = record(&conn, "old", 99, None);
        let orphan_new = record(&conn, "new", 101, None);
        let orphan_equal = record(&conn, "equal-orphan", 100, None);
        let accessed_new = record(&conn, "accessed-new", 1, Some(101));
        let accessed_old = record(&conn, "accessed-old", 101, Some(99));
        let accessed_equal = record(&conn, "accessed-equal", 1, Some(100));
        let sessions = cleanup_thinking_batch(
            &conn,
            100,
            ThinkingBatch::Sessions,
            CleanupLimits::default(),
        )
        .unwrap();
        let orphans =
            cleanup_thinking_batch(&conn, 100, ThinkingBatch::Orphans, CleanupLimits::default())
                .unwrap();
        assert_eq!(
            (
                sessions.deleted_records,
                sessions.deleted_sessions,
                orphans.deleted_records
            ),
            (2, 2, 2)
        );
        for id in [
            active,
            equal_session,
            orphan_new,
            orphan_equal,
            accessed_new,
            accessed_equal,
        ] {
            assert!(exists(&conn, id), "lost {id}");
        }
        for id in [expired_old, expired_new, orphan_old, accessed_old] {
            assert!(!exists(&conn, id), "retained {id}");
        }
    }

    #[test]
    fn thinking_cleanup_large_session_and_empty_session_caps() {
        let conn = local_db();
        session(&conn, "large", 1);
        for _ in 0..257 {
            record(&conn, "large", 1, None);
        }
        let limits = CleanupLimits::default();
        let first = cleanup_thinking_batch(&conn, 100, ThinkingBatch::Sessions, limits).unwrap();
        assert_eq!((first.deleted_records, first.deleted_sessions), (128, 0));
        let second = cleanup_thinking_batch(&conn, 100, ThinkingBatch::Sessions, limits).unwrap();
        assert_eq!((second.deleted_records, second.deleted_sessions), (128, 0));
        let third = cleanup_thinking_batch(&conn, 100, ThinkingBatch::Sessions, limits).unwrap();
        assert_eq!((third.deleted_records, third.deleted_sessions), (1, 1));
        for i in 0..257 {
            session(&conn, &format!("empty-{i}"), 1);
        }
        assert_eq!(
            cleanup_thinking_batch(&conn, 100, ThinkingBatch::Sessions, limits)
                .unwrap()
                .deleted_sessions,
            128
        );
    }

    #[test]
    fn thinking_cleanup_cursor_reaches_orphans_and_does_not_skip_window_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cursor.db");
        {
            let conn = open_thinking_db_at(&path).unwrap();
            session(&conn, "healthy", 101);
            for _ in 0..1100 {
                record(&conn, "healthy", 1, None);
            }
            for _ in 0..300 {
                record(&conn, "orphan", 1, None);
            }
        }
        let limits = CleanupLimits {
            window: 512,
            ..Default::default()
        };
        let mut deleted = 0;
        let mut cursors = Vec::new();
        // 每批重新打开数据库，证明游标不依赖进程内状态。
        for _ in 0..6 {
            let conn = open_thinking_db_at(&path).unwrap();
            let stats = cleanup_thinking_batch(&conn, 100, ThinkingBatch::Orphans, limits).unwrap();
            assert!(stats.scanned <= 512 && stats.deleted_records <= 128);
            deleted += stats.deleted_records;
            cursors.push(conn.query_row("SELECT CAST(v AS INTEGER) FROM thinking_meta WHERE k = 'cleanup_orphan_cursor'", [], |row| row.get::<_, i64>(0)).unwrap());
        }
        assert_eq!(deleted, 300);
        assert_eq!(&cursors[..4], &[512, 1024, 1228, 1356]);
        assert_eq!(cursors[4], 0);
    }

    #[test]
    fn thinking_cleanup_interrupt_rolls_back_cursor_and_clears_handler() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        with_thinking_db(|conn| {
            record(conn, "orphan", 1, None);
            conn.execute(
                "INSERT INTO thinking_meta VALUES ('cleanup_orphan_cursor', '0')",
                [],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
        // 直接使用共享连接执行维护；前台计数不能包含维护本身。
        let conn = thinking_db().unwrap();
        let limits = CleanupLimits {
            vm_steps: 1,
            interrupt_after_checks: Some(150),
            ..Default::default()
        };
        let error = cleanup_transaction(&conn, limits, |conn, _budget| {
            conn.execute("DELETE FROM thinking_records", [])?;
            conn.execute("UPDATE thinking_meta SET v = '99' WHERE k = 'cleanup_orphan_cursor'", [])?;
            conn.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) SELECT sum(x) FROM n", [], |row| row.get::<_, i64>(0))
        }).unwrap_err();
        assert!(cleanup_is_deferred(&error), "{error}");
        assert_eq!(
            conn.query_row(
                "SELECT v FROM thinking_meta WHERE k = 'cleanup_orphan_cursor'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "0"
        );
        drop(conn);
        assert_eq!(get_thinking_records_count().unwrap(), 1);
        let deferred = try_cleanup_thinking_batch(
            100,
            ThinkingBatch::Orphans,
            CleanupLimits {
                vm_steps: 1,
                interrupt_after_checks: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(deferred.deferred);
        assert_eq!(get_thinking_records_count().unwrap(), 1);
        with_thinking_db(|conn| {
            conn.execute("DROP TABLE thinking_meta", [])
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(
            try_cleanup_thinking_batch(100, ThinkingBatch::Orphans, CleanupLimits::default())
                .unwrap_err()
                .contains("thinking_meta")
        );
    }

    #[test]
    fn thinking_cleanup_busy_writer_defers_without_poisoning_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("busy.db");
        let conn = open_thinking_db_at(&path).unwrap();
        session(&conn, "expired", 1);
        record(&conn, "expired", 1, None);
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.busy_timeout(Duration::ZERO).unwrap();
        let error = cleanup_thinking_batch(
            &conn,
            100,
            ThinkingBatch::Sessions,
            CleanupLimits::default(),
        )
        .unwrap_err();
        assert!(cleanup_is_deferred(&error));
        writer.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            cleanup_thinking_batch(
                &conn,
                100,
                ThinkingBatch::Sessions,
                CleanupLimits::default()
            )
            .unwrap()
            .deleted_records,
            1
        );
    }

    #[test]
    fn thinking_cleanup_lock_busy_and_pending_requests_defer() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        let held = hold_thinking_db_for_test();
        let (tx, rx) = mpsc::channel();
        let task = std::thread::spawn(move || {
            tx.send(try_cleanup_thinking_batch(
                100,
                ThinkingBatch::Sessions,
                CleanupLimits::default(),
            ))
            .unwrap()
        });
        let result = rx.recv_timeout(Duration::from_millis(500));
        drop(held);
        task.join().unwrap();
        assert!(result.unwrap().unwrap().deferred);

        let (queued_tx, queued_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *BEFORE_LOCK.lock().unwrap() = Some(Box::new(move || {
            queued_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        }));
        let request = std::thread::spawn(get_thinking_records_count);
        queued_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(THINKING_PENDING.load(Ordering::SeqCst), 1);
        let deferred =
            try_cleanup_thinking_batch(100, ThinkingBatch::Sessions, CleanupLimits::default())
                .unwrap()
                .deferred;
        release_tx.send(()).unwrap();
        assert_eq!(request.join().unwrap().unwrap(), 0);
        assert!(deferred);
        assert!(
            !try_cleanup_thinking_batch(100, ThinkingBatch::Sessions, CleanupLimits::default())
                .unwrap()
                .deferred
        );
    }

    #[test]
    fn thinking_cleanup_cannot_interleave_save_and_session_touch() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        for existing in [false, true] {
            if existing {
                with_thinking_db(|conn| {
                    session(conn, "save-existing", 1);
                    Ok(())
                })
                .unwrap();
            }
            let key = if existing {
                "save-existing"
            } else {
                "save-new"
            };
            let (written_tx, written_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            *BEFORE_TOUCH.lock().unwrap() = Some(Box::new(move || {
                written_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            }));
            let request = std::thread::spawn(move || {
                save_thinking_record(key, "fp", "reasoning", None, &[], &[], "visible")
            });
            written_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            let deferred =
                try_cleanup_thinking_batch(100, ThinkingBatch::Sessions, CleanupLimits::default())
                    .unwrap()
                    .deferred;
            release_tx.send(()).unwrap();
            request.join().unwrap().unwrap();
            assert!(deferred);
            assert_eq!(load_thinking_records(key).unwrap().len(), 1);
            with_thinking_db(|conn| {
                assert!(
                    conn.query_row(
                        "SELECT last_accessed FROM thinking_sessions WHERE session_key = ?1",
                        [key],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap()
                        > 100
                );
                Ok(())
            })
            .unwrap();
        }
    }

    #[test]
    fn thinking_cleanup_queries_use_bounded_index_windows() {
        let conn = local_db();
        for (sql, expected) in [
            (EXPIRED_SESSIONS_SQL, "idx_thinking_sessions_accessed"),
            (SESSION_RECORDS_SQL, "idx_thinking_rec_seq"),
            (ORPHAN_WINDOW_SQL, "INTEGER PRIMARY KEY"),
        ] {
            let details = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(params![100, 128], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .join("; ");
            assert!(
                details.contains("SEARCH") && details.contains(expected),
                "{details}"
            );
            assert!(!details.contains("SCAN "), "{details}");
        }
        conn.execute("DROP INDEX idx_thinking_sessions_accessed", [])
            .unwrap();
        conn.execute(
            "CREATE TABLE idx_thinking_sessions_accessed (invalid INTEGER)",
            [],
        )
        .unwrap();
        assert!(init_thinking_schema(&conn).is_err());
    }

    #[test]
    fn thinking_cleanup_round_bounds_tools_and_gives_orphans_progress() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        with_thinking_db(|conn| {
            session(conn, "large", 1);
            for _ in 0..400 {
                record(conn, "large", 1, None);
            }
            record(conn, "orphan", 1, None);
            Ok(())
        })
        .unwrap();
        let conn = connect_db().unwrap();
        for i in 0..300 {
            conn.execute(
                "INSERT INTO tool_signatures VALUES (?1, 'sig', 1)",
                [format!("tool-{i}")],
            )
            .unwrap();
        }
        let limits = CleanupLimits {
            batches: 3,
            window: 512,
            ..Default::default()
        };
        let mut state = ThinkingMaintenance::with_limits(limits);
        let stats = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
        assert_eq!(
            (stats.batches, stats.deleted_records, stats.deleted_tools),
            (3, 129, 128)
        );
        assert!(!stats.deferred);
        assert_eq!(get_thinking_records_count().unwrap(), 272);
        let zero = cleanup_thinking_storage_with_limits(
            100,
            CleanupLimits {
                round_budget: Duration::ZERO,
                ..limits
            },
            &mut state,
        )
        .unwrap();
        assert_eq!(zero.batches, 0);
        assert!(zero.deferred);
        conn.execute("DROP TABLE tool_signatures", []).unwrap();
        assert!(
            cleanup_thinking_storage_with_limits(100, limits, &mut state)
                .unwrap_err()
                .contains("tool_signatures")
        );
    }
    #[test]
    fn thinking_review_short_statements_observe_pending_before_commit() {
        let conn = local_db();
        let mut pending = None;
        let result = cleanup_transaction(
            &conn,
            CleanupLimits {
                vm_steps: i32::MAX,
                ..Default::default()
            },
            |conn, _budget| {
                for i in 0..20 {
                    conn.execute(
                        "INSERT INTO thinking_meta VALUES (?1, 'value')",
                        [i.to_string()],
                    )?;
                    if i == 8 {
                        pending = Some(ThinkingRequest::enter());
                    }
                }
                Ok(())
            },
        );
        drop(pending);
        assert!(
            result.is_err(),
            "short statements bypassed the pending gate"
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM thinking_meta", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn thinking_review_repeated_session_interrupts_do_not_starve_orphans() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        let orphan = with_thinking_db(|conn| {
            session(conn, "large", 1);
            record(conn, "large", 1, None);
            let orphan = record(conn, "orphan", 1, None);
            conn.execute_batch("CREATE TRIGGER expensive_session_delete BEFORE DELETE ON thinking_records WHEN OLD.session_key = 'large' BEGIN SELECT (WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) SELECT sum(x) FROM n); END;").unwrap();
            Ok(orphan)
        }).unwrap();
        let limits = CleanupLimits {
            batches: 3,
            interrupt_after_checks: Some(1),
            ..Default::default()
        };
        let mut state = ThinkingMaintenance::with_limits(limits);
        let mut deleted = 0;
        for _ in 0..3 {
            let stats = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
            assert!(stats.deferred);
            deleted += stats.deleted_records;
        }
        assert_eq!(
            deleted, 1,
            "every round restarted the interrupted session category"
        );
        with_thinking_db(|conn| {
            assert!(!exists(conn, orphan));
            Ok(())
        })
        .unwrap();
        assert_eq!(get_thinking_records_count().unwrap(), 1);
    }
    static SHORT_SQL_CHECKPOINTS: AtomicUsize = AtomicUsize::new(0);

    fn expire_during_short_sql_loop(checks: usize, deadline: Instant) -> Instant {
        SHORT_SQL_CHECKPOINTS.store(checks, Ordering::SeqCst);
        if checks >= 45 {
            deadline
        } else {
            deadline - Duration::from_secs(1)
        }
    }

    fn enqueue_during_short_sql_loop(checks: usize, deadline: Instant) -> Instant {
        SHORT_SQL_CHECKPOINTS.store(checks, Ordering::SeqCst);
        if checks >= 45 {
            THINKING_PENDING.store(1, Ordering::SeqCst);
        }
        deadline - Duration::from_secs(1)
    }

    #[test]
    fn thinking_review_short_statement_loops_observe_budget_and_pending() {
        for kind in [ThinkingBatch::Sessions, ThinkingBatch::Orphans] {
            for clock in [
                expire_during_short_sql_loop as fn(usize, Instant) -> Instant,
                enqueue_during_short_sql_loop,
            ] {
                let conn = local_db();
                if matches!(kind, ThinkingBatch::Sessions) {
                    session(&conn, "expired", 1);
                }
                for _ in 0..30 {
                    record(&conn, "expired", 1, None);
                }
                conn.execute(
                    "INSERT INTO thinking_meta VALUES ('cleanup_orphan_cursor', '0')",
                    [],
                )
                .unwrap();
                let changes_before = conn.total_changes();
                SHORT_SQL_CHECKPOINTS.store(0, Ordering::SeqCst);
                let result = cleanup_thinking_batch(
                    &conn,
                    100,
                    kind,
                    CleanupLimits {
                        vm_steps: i32::MAX,
                        window: 128,
                        checkpoint_clock: Some(clock),
                        ..Default::default()
                    },
                );
                THINKING_PENDING.store(0, Ordering::SeqCst);
                let error = result.unwrap_err();
                assert_eq!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::OperationInterrupted)
                );
                assert_eq!(SHORT_SQL_CHECKPOINTS.load(Ordering::SeqCst), 45);
                assert!(
                    conn.total_changes() > changes_before,
                    "interrupt must follow actual row mutations"
                );
                assert_eq!(
                    conn.query_row("SELECT count(*) FROM thinking_records", [], |row| row
                        .get::<_, i64>(0))
                        .unwrap(),
                    30
                );
                assert_eq!(
                    conn.query_row(
                        "SELECT v FROM thinking_meta WHERE k = 'cleanup_orphan_cursor'",
                        [],
                        |row| row.get::<_, String>(0)
                    )
                    .unwrap(),
                    "0"
                );
                // 卸载中断回调后，同一连接上的正常请求不受影响。
                conn.execute("INSERT INTO thinking_meta VALUES ('normal', 'ok')", [])
                    .unwrap();
            }
        }
    }

    #[test]
    fn thinking_review_commit_observes_shared_deadline() {
        let conn = local_db();
        let result = cleanup_transaction(
            &conn,
            CleanupLimits {
                vm_steps: i32::MAX,
                checkpoint_clock: Some(|checks, deadline| {
                    if checks == 2 {
                        deadline
                    } else {
                        deadline - Duration::from_secs(1)
                    }
                }),
                ..Default::default()
            },
            |conn, _budget| {
                for i in 0..20 {
                    conn.execute(
                        "INSERT INTO thinking_meta VALUES (?1, 'value')",
                        [i.to_string()],
                    )?;
                }
                Ok(())
            },
        );
        assert_eq!(
            result.unwrap_err().sqlite_error_code(),
            Some(rusqlite::ErrorCode::OperationInterrupted)
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM thinking_meta", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    fn expire_large_cold_window(checks: usize, deadline: Instant) -> Instant {
        if checks >= 20 {
            deadline
        } else {
            deadline - Duration::from_secs(1)
        }
    }

    #[test]
    fn cold_cleanup_shrinks_across_rounds_and_commits_every_tail_row() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        with_thinking_db(|conn| {
            for _ in 0..25 {
                record(conn, "orphan", 1, None);
            }
            Ok(())
        })
        .unwrap();
        let limits = CleanupLimits {
            batches: 1,
            vm_steps: i32::MAX,
            checkpoint_clock: Some(expire_large_cold_window),
            ..Default::default()
        };
        let mut state = ThinkingMaintenance::with_limits(limits);
        state.complete = [true, false, true];
        state.next = 1;
        let failed = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
        assert_eq!(failed.defer_reason, Some(CleanupDeferReason::Budget));
        assert_eq!(
            (
                failed.committed_batches,
                failed.scanned,
                failed.orphan_cursor
            ),
            (0, 0, None)
        );
        assert_eq!(state.units, [128, 8, 128]);
        assert_eq!(get_thinking_records_count().unwrap(), 25);
        let mut deleted = 0;
        let mut cursors = Vec::new();
        for _ in 0..4 {
            let stats = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
            assert!(!stats.deferred);
            assert_eq!(stats.committed_batches, 1);
            deleted += stats.deleted_records;
            cursors.push(stats.orphan_cursor.unwrap());
        }
        assert_eq!(deleted, 25);
        assert_eq!(cursors, [8, 16, 24, 0]);
        assert!(state.complete.iter().all(|done| *done));
        assert_eq!(get_thinking_records_count().unwrap(), 0);
        // 新一遍遍历保留缩小单元，并且不会重复扫描已经完成的空类别。
        let next = cleanup_thinking_storage_with_limits(200, CleanupLimits::default(), &mut state)
            .unwrap();
        assert_eq!(next.units, [128, 8, 128]);
        assert_eq!(next.committed_batches, 3);
        assert!(!next.unfinished);
    }

    #[test]
    fn cold_cleanup_minimum_units_commit_after_budget_expiry() {
        let conn = local_db();
        session(&conn, "expired", 1);
        record(&conn, "expired", 1, None);
        record(&conn, "expired", 1, None);
        let orphan = record(&conn, "orphan", 1, None);
        let limits = CleanupLimits {
            rows: 1,
            window: 1,
            batch_budget: Duration::ZERO,
            vm_steps: 1,
            ..Default::default()
        };
        let first = cleanup_thinking_batch(&conn, 100, ThinkingBatch::Sessions, limits).unwrap();
        assert_eq!((first.deleted_records, first.deleted_sessions), (1, 0));
        let second = cleanup_thinking_batch(&conn, 100, ThinkingBatch::Sessions, limits).unwrap();
        assert_eq!((second.deleted_records, second.deleted_sessions), (1, 1));
        let stats = cleanup_thinking_batch(&conn, 100, ThinkingBatch::Orphans, limits).unwrap();
        assert_eq!(stats.deleted_records, 1);
        assert_eq!(stats.orphan_cursor, Some(orphan));
        assert!(!stats.category_complete);
        assert!(
            cleanup_thinking_batch(&conn, 100, ThinkingBatch::Orphans, limits)
                .unwrap()
                .category_complete
        );
        conn.execute_batch("CREATE TABLE tool_signatures (tool_id TEXT PRIMARY KEY, signature TEXT, created_at INTEGER); CREATE INDEX idx_tool_sig_created ON tool_signatures(created_at); INSERT INTO tool_signatures VALUES ('a', 'sig', 1), ('b', 'sig', 1);").unwrap();
        assert_eq!(cleanup_tools_batch(&conn, 100, limits).unwrap(), 1);
        assert_eq!(cleanup_tools_batch(&conn, 100, limits).unwrap(), 1);
        assert_eq!(cleanup_tools_batch(&conn, 100, limits).unwrap(), 0);
    }

    fn pending_at_minimum_commit(checks: usize, deadline: Instant) -> Instant {
        if checks >= 5 {
            THINKING_PENDING.store(1, Ordering::SeqCst);
        }
        deadline
    }

    #[test]
    fn cold_cleanup_pending_rolls_back_minimum_unit_but_preserves_prior_commits() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        with_thinking_db(|conn| {
            for _ in 0..3 {
                record(conn, "orphan", 1, None);
            }
            Ok(())
        })
        .unwrap();
        let limits = CleanupLimits {
            window: 1,
            batches: 1,
            batch_budget: Duration::ZERO,
            vm_steps: i32::MAX,
            ..Default::default()
        };
        let mut state = ThinkingMaintenance::with_limits(limits);
        state.complete = [true, false, true];
        state.next = 1;
        let committed = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
        assert_eq!(committed.orphan_cursor, Some(1));
        assert_eq!(committed.deleted_records, 1);
        let result = cleanup_thinking_storage_with_limits(
            100,
            CleanupLimits {
                checkpoint_clock: Some(pending_at_minimum_commit),
                ..limits
            },
            &mut state,
        );
        THINKING_PENDING.store(0, Ordering::SeqCst);
        let pending = result.unwrap();
        assert_eq!(pending.defer_reason, Some(CleanupDeferReason::Pending));
        assert_eq!((pending.committed_batches, pending.scanned), (0, 0));
        assert_eq!(state.units, [128, 1, 128]);
        with_thinking_db(|conn| {
            assert!(!exists(conn, 1));
            assert!(exists(conn, 2) && exists(conn, 3));
            assert_eq!(
                conn.query_row(
                    "SELECT v FROM thinking_meta WHERE k = 'cleanup_orphan_cursor'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
                "1"
            );
            Ok(())
        })
        .unwrap();
        let resumed = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
        assert_eq!(resumed.orphan_cursor, Some(2));
    }

    #[test]
    fn cold_cleanup_contention_does_not_shrink_and_completion_survives_rounds() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        let limits = CleanupLimits {
            batches: 1,
            ..Default::default()
        };
        let mut state = ThinkingMaintenance::with_limits(limits);
        let pending = ThinkingRequest::enter();
        let stats = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
        assert_eq!(stats.defer_reason, Some(CleanupDeferReason::Pending));
        drop(pending);
        let guard = hold_thinking_db_for_test();
        let stats = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
        assert_eq!(stats.defer_reason, Some(CleanupDeferReason::LockBusy));
        drop(guard);
        assert_eq!(state.units, [128, 16, 128]);
        // 结束空 orphan 类别后，后续调用只执行尚未完成的类别。
        let orphan = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
        assert!(orphan.orphan_sweep_complete && orphan.unfinished);
        let tools = cleanup_thinking_storage_with_limits(200, limits, &mut state).unwrap();
        assert!(tools.orphan_sweep_complete && tools.unfinished);
        let sessions = cleanup_thinking_storage_with_limits(300, limits, &mut state).unwrap();
        assert!(!sessions.unfinished);
        with_thinking_db(|conn| {
            record(conn, "next-pass", 250, None);
            Ok(())
        })
        .unwrap();
        let next = cleanup_thinking_storage_with_limits(300, CleanupLimits::default(), &mut state)
            .unwrap();
        assert_eq!(next.deleted_records, 1);
        assert!(!next.unfinished);
    }

    #[test]
    fn cold_cleanup_budget_shrink_stops_at_one_and_allows_progress() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        with_thinking_db(|conn| {
            record(conn, "orphan", 1, None);
            Ok(())
        })
        .unwrap();
        let limits = CleanupLimits {
            batches: 1,
            batch_budget: Duration::ZERO,
            ..Default::default()
        };
        let mut state = ThinkingMaintenance::with_limits(limits);
        state.complete = [true, false, true];
        state.next = 1;
        for expected in [8, 4, 2, 1] {
            let stats = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
            assert_eq!(stats.defer_reason, Some(CleanupDeferReason::Budget));
            assert_eq!(stats.units[1], expected);
            assert_eq!(get_thinking_records_count().unwrap(), 1);
        }
        let minimum = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
        assert_eq!(minimum.deleted_records, 1);
        assert!(!minimum.deferred);
        assert_eq!(minimum.units[1], 1);
    }

    #[test]
    fn cold_cleanup_attempt_limit_reports_unfinished_without_repeating_completed_categories() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        with_thinking_db(|conn| {
            session(conn, "active", 101);
            for _ in 0..4100 {
                record(conn, "active", 1, None);
            }
            Ok(())
        })
        .unwrap();
        let mut state = ThinkingMaintenance {
            complete: [true, false, true],
            next: 1,
            ..Default::default()
        };
        let stats = cleanup_thinking_storage_with_limits(100, CleanupLimits::default(), &mut state)
            .unwrap();
        assert_eq!(
            (stats.batches, stats.committed_batches, stats.scanned),
            (256, 256, 4096)
        );
        assert!(stats.unfinished && !stats.deferred);
        assert_eq!(stats.orphan_cursor, Some(4096));
        let last = cleanup_thinking_storage_with_limits(100, CleanupLimits::default(), &mut state)
            .unwrap();
        assert_eq!(
            (last.batches, last.scanned, last.orphan_cursor),
            (1, 4, Some(0))
        );
        assert!(!last.unfinished);
    }
    #[test]
    fn cold_cleanup_sqlite_busy_preserves_adaptive_units() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        with_thinking_db(|conn| {
            record(conn, "orphan", 1, None);
            conn.busy_timeout(Duration::ZERO).unwrap();
            Ok(())
        })
        .unwrap();
        let writer = Connection::open(get_thinking_db_path().unwrap()).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        let mut state = ThinkingMaintenance {
            complete: [true, false, true],
            next: 1,
            ..Default::default()
        };
        let result =
            cleanup_thinking_storage_with_limits(100, CleanupLimits::default(), &mut state);
        writer.execute_batch("ROLLBACK").unwrap();
        with_thinking_db(|conn| {
            conn.busy_timeout(Duration::from_secs(5))
                .map_err(|error| error.to_string())
        })
        .unwrap();
        let stats = result.unwrap();
        assert_eq!(stats.defer_reason, Some(CleanupDeferReason::SqliteBusy));
        assert_eq!(state.units, [128, 16, 128]);
        assert_eq!(get_thinking_records_count().unwrap(), 1);
    }

    #[test]
    fn cold_cleanup_retention_change_restarts_cutoff_without_resetting_units() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let day = 24 * 3600 * 1000;
        with_thinking_db(|conn| {
            record(conn, "orphan", now - 20 * day, None);
            Ok(())
        })
        .unwrap();
        let mut state = ThinkingMaintenance {
            units: [1, 1, 1],
            complete: [true, false, true],
            retention_days: Some(15),
            ..Default::default()
        };
        assert!(!cleanup_thinking_storage(30, &mut state).unwrap().unfinished);
        assert_eq!(get_thinking_records_count().unwrap(), 1);
        assert_eq!(state.units, [1, 1, 1]);
        assert_eq!(
            cleanup_thinking_storage(10, &mut state)
                .unwrap()
                .deleted_records,
            1
        );
    }
    #[test]
    fn cold_cleanup_hourly_reopens_expiry_categories_without_restarting_orphan_cursor() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        with_thinking_db(|conn| {
            session(conn, "active", 300);
            record(conn, "active", 1, None);
            session(conn, "newly-expired", 150);
            record(conn, "newly-expired", 250, None);
            record(conn, "orphan", 1, None);
            conn.execute(
                "INSERT INTO thinking_meta VALUES ('cleanup_orphan_cursor', '1')",
                [],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
        let tools = connect_db().unwrap();
        tools
            .execute(
                "INSERT INTO tool_signatures VALUES ('newly-expired', 'sig', 150)",
                [],
            )
            .unwrap();
        let limits = CleanupLimits {
            rows: 1,
            window: 1,
            batches: 1,
            ..Default::default()
        };
        let mut state = ThinkingMaintenance::with_limits(limits);
        // 上一轮截止时间 100 时 session/tool 已完成，orphan 从 id=1 之后继续。
        state.complete = [true, false, true];
        state.reopen_completed_categories();
        let sessions = cleanup_thinking_storage_with_limits(200, limits, &mut state).unwrap();
        assert_eq!(sessions.deleted_records, 1);
        with_thinking_db(|conn| {
            assert!(!exists(conn, 2));
            assert!(exists(conn, 1) && exists(conn, 3));
            assert_eq!(
                conn.query_row(
                    "SELECT v FROM thinking_meta WHERE k = 'cleanup_orphan_cursor'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
                "1"
            );
            Ok(())
        })
        .unwrap();
        let rest = cleanup_thinking_storage_with_limits(
            200,
            CleanupLimits {
                batches: 256,
                ..limits
            },
            &mut state,
        )
        .unwrap();
        assert_eq!(rest.deleted_tools, 1);
        assert_eq!(rest.deleted_records, 1);
        assert!(!rest.unfinished);
        assert_eq!(get_thinking_records_count().unwrap(), 1);
    }
    #[test]
    fn cold_cleanup_slow_minimum_commit_does_not_start_work_after_round_expiry() {
        let _dir = TestDataDir::new();
        init_db().unwrap();
        with_thinking_db(|conn| {
            for _ in 0..3 {
                record(conn, "orphan", 1, None);
            }
            Ok(())
        })
        .unwrap();
        let limits = CleanupLimits {
            window: 1,
            batch_budget: Duration::ZERO,
            round_clock: Some(|attempts| {
                if attempts == 0 {
                    Duration::ZERO
                } else {
                    Duration::from_secs(3)
                }
            }),
            ..Default::default()
        };
        let mut state = ThinkingMaintenance::with_limits(limits);
        state.complete = [true, false, true];
        state.next = 1;
        let stats = cleanup_thinking_storage_with_limits(100, limits, &mut state).unwrap();
        assert_eq!(
            (
                stats.batches,
                stats.committed_batches,
                stats.deleted_records
            ),
            (1, 1, 1)
        );
        assert_eq!(stats.defer_reason, Some(CleanupDeferReason::RoundBudget));
        assert_eq!(stats.orphan_cursor, Some(1));
        assert_eq!(get_thinking_records_count().unwrap(), 2);
    }
}

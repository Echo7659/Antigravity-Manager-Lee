# Opus 5.5 Routing and Runtime Stability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 `invalid_grant` 自锁和 Opus 5.5 账号误过滤，并让 headless watchdog 在 Tokio runtime 卡死时仍可触发容器重启。

**Architecture:** TokenManager 使用目标模型感知的保护判定，Opus 5.5 资格同时校验 tier 和具体物理模型的正配额。`invalid_grant` 计数更新封装为不携带 DashMap guard 的同步边界。headless runtime 使用独立 OS 线程执行有界 HTTP probe，并通过可中断的关停信号回收。

**Tech Stack:** Rust 2024, Tokio, DashMap, std::net/std::thread, Axum, Docker

## Global Constraints

- 所有 Rust 代码保持 `edition = "2024"`。
- 不新增第三方依赖。
- 不改变四种对外协议的请求和响应结构。
- 配额保护仅对当前目标模型生效；没有模型上下文的管理路径保持保守语义。
- Opus 5.5 必须为 Ultra 或有付费证据的 Pro，且当前物理变体的 `exact_model_quotas` 大于 0。
- 发布仅使用 `beta` 分支和独立 Beta 镜像标签，不更改 `main` 或 `latest`。
- 生产替换前保留原镜像和容器配置，失败时按原镜像回滚。

---

### Task 1: 目标模型感知的配额保护与 Opus 5.5 正配额

**Files:**
- Modify: `src-tauri/src/proxy/token_manager.rs:174-200`
- Modify: `src-tauri/src/proxy/token_manager.rs:1450-2380`
- Test: `src-tauri/src/proxy/token_manager.rs:4330-4420`
- Test: `src-tauri/src/proxy/token_manager.rs:5990-6070`

**Interfaces:**
- Consumes: `normalize_to_standard_id(model: &str) -> Option<String>` and `resolve_opus_5_5_route(model: &str, effort: Option<&str>) -> Option<String>`.
- Produces: `is_model_quota_protected(token: &ProxyToken, protection_enabled: bool, target_model: &str) -> bool`.
- Produces: `is_model_account_eligible(token: &ProxyToken, model: &str) -> bool` where Opus 5.5 requires a positive exact quota.

- [ ] **Step 1: Write failing model-protection and exact-quota tests**

Add focused assertions to the existing token-manager test module:

```rust
#[test]
fn quota_protection_only_blocks_the_requested_model_family() {
    let mut token = create_test_token("ultra", Some("ULTRA"), 1.0, None, Some(100));
    token.protected_models.insert("gemini-3-pro-high".into());
    assert!(!is_model_quota_protected(
        &token,
        true,
        "claude-opus-5-5-low"
    ));
    token.protected_models.insert("claude".into());
    assert!(is_model_quota_protected(
        &token,
        true,
        "claude-opus-5-5-low"
    ));
}

#[test]
fn opus_5_5_requires_positive_exact_variant_quota() {
    let mut token = create_test_token("ultra", Some("ULTRA"), 1.0, None, Some(100));
    token.exact_model_quotas.insert("claude-opus-5-5-low".into(), 0);
    token.exact_model_quotas.insert("claude-opus-5-5-high".into(), 100);
    assert!(!is_model_account_eligible(&token, "claude-opus-5-5-low"));
    assert!(is_model_account_eligible(&token, "claude-opus-5-5-high"));
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cd src-tauri
cargo test proxy::token_manager::tests::quota_protection_only_blocks_the_requested_model_family -- --exact
cargo test proxy::token_manager::tests::opus_5_5_requires_positive_exact_variant_quota -- --exact
```

Expected: the first test fails to compile because `is_model_quota_protected` does not exist; the second fails because a zero-valued exact quota is currently accepted.

- [ ] **Step 3: Implement the target-aware predicates**

Replace the global account predicate with:

```rust
fn is_model_quota_protected(
    token: &ProxyToken,
    protection_enabled: bool,
    target_model: &str,
) -> bool {
    if !protection_enabled {
        return false;
    }
    let protected_key = crate::proxy::common::model_mapping::normalize_to_standard_id(target_model)
        .unwrap_or_else(|| target_model.to_string());
    token.protected_models.contains(&protected_key)
}
```

Change the Opus 5.5 exact quota condition to:

```rust
token
    .exact_model_quotas
    .get(&physical_model)
    .is_some_and(|percentage| *percentage > 0)
```

Update every request-time selection path that has `target_model` to use `is_model_quota_protected`. Retain a separately named conservative helper only for management paths that have no model context.

- [ ] **Step 4: Run all token-manager tests**

Run:

```bash
cd src-tauri
cargo test proxy::token_manager::tests --lib
```

Expected: all token-manager tests pass, including tier, alias, physical variant, zero quota, preferred account, sticky session and P2C coverage.

- [ ] **Step 5: Commit the routing fix**

```bash
git add src-tauri/src/proxy/token_manager.rs
git commit -m "fix(proxy): make quota protection model aware"
```

### Task 2: `invalid_grant` DashMap guard 生命周期

**Files:**
- Modify: `src-tauri/src/proxy/token_manager.rs:230-275`
- Modify: `src-tauri/src/proxy/token_manager.rs:2495-2535`
- Test: `src-tauri/src/proxy/token_manager.rs` internal test module

**Interfaces:**
- Consumes: `invalid_grant_failures: Arc<DashMap<String, u32>>`.
- Produces: `fn record_invalid_grant_failure(&self, account_id: &str) -> u32` which returns a copied count and never exposes a DashMap guard.

- [ ] **Step 1: Write the failing failure-counter test**

```rust
#[test]
fn invalid_grant_counter_releases_map_guard_before_follow_up_work() {
    let manager = TokenManager::new(PathBuf::new());
    assert_eq!(manager.record_invalid_grant_failure("account-a"), 1);
    assert_eq!(manager.record_invalid_grant_failure("account-a"), 2);
    assert_eq!(manager.invalid_grant_failures.remove("account-a"), Some(("account-a".into(), 2)));
}
```

- [ ] **Step 2: Run the test and verify it fails to compile**

Run:

```bash
cd src-tauri
cargo test proxy::token_manager::tests::invalid_grant_counter_releases_map_guard_before_follow_up_work -- --exact
```

Expected: FAIL because `record_invalid_grant_failure` does not exist.

- [ ] **Step 3: Implement the bounded synchronous update**

```rust
fn record_invalid_grant_failure(&self, account_id: &str) -> u32 {
    let mut count = self
        .invalid_grant_failures
        .entry(account_id.to_string())
        .or_insert(0);
    *count += 1;
    *count
}
```

Call this helper before `disable_account(...).await`. No DashMap `RefMut` may remain in the async scope. Preserve the existing two-failure threshold and remove the counter only after the disable future completes.

- [ ] **Step 4: Run the focused test and invalid-grant-related token tests**

Run:

```bash
cd src-tauri
cargo test proxy::token_manager::tests::invalid_grant_counter_releases_map_guard_before_follow_up_work -- --exact
cargo test proxy::token_manager::tests --lib invalid_grant
```

Expected: all selected tests pass without timeout.

- [ ] **Step 5: Commit the deadlock fix**

```bash
git add src-tauri/src/proxy/token_manager.rs
git commit -m "fix(proxy): release invalid grant counter lock before await"
```

### Task 3: 独立 headless watchdog

**Files:**
- Modify: `src-tauri/src/runtime.rs:1-310`
- Test: `src-tauri/src/runtime.rs:390-620`

**Interfaces:**
- Consumes: local proxy port and the current API key captured before the watchdog thread starts.
- Produces: `WatchdogHandle` containing a stop sender and `std::thread::JoinHandle<()>`.
- Produces: `run_blocking_health_check(port: u16, api_key: Option<&str>, timeout: Duration) -> bool`.
- Produces: a pure failure transition helper that returns whether the process-exit threshold has been reached.

- [ ] **Step 1: Write failing blocking-probe and failure-transition tests**

Add tests that start `ServerFixture`, invoke the blocking probe from `tokio::task::spawn_blocking`, and verify 200/auth failure behavior. Add a pure transition test:

```rust
#[test]
fn watchdog_failure_counter_resets_and_trips_on_third_failure() {
    let mut failures = 0;
    assert!(!record_watchdog_probe(&mut failures, false));
    assert!(!record_watchdog_probe(&mut failures, false));
    assert!(record_watchdog_probe(&mut failures, false));
    assert!(!record_watchdog_probe(&mut failures, true));
    assert_eq!(failures, 0);
}
```

- [ ] **Step 2: Run the runtime tests and verify the new tests fail**

Run:

```bash
cd src-tauri
cargo test runtime::tests --lib
```

Expected: FAIL because the blocking probe, watchdog counter and thread handle do not exist.

- [ ] **Step 3: Implement the blocking probe with explicit timeouts**

Use `std::net::TcpStream::connect_timeout`, `set_read_timeout` and `set_write_timeout`; send the same `GET /health` request and Authorization header used by the async probe. Reject API keys containing CR/LF before writing. Treat only `HTTP/1.0 200` and `HTTP/1.1 200` as success.

- [ ] **Step 4: Move watchdog scheduling to an OS thread**

Create a stop channel and use `recv_timeout` for both the 30-second startup grace period and 15-second interval. On three consecutive failures, log and call an injected exit callback in tests or `std::process::exit(1)` in production. Update `shutdown_after_signal` to stop and join the watchdog without an unbounded wait.

- [ ] **Step 5: Run runtime tests and shutdown regression tests**

Run:

```bash
cd src-tauri
cargo test runtime::tests --lib
```

Expected: health probe, API-key, SIGINT/SIGTERM, listener release, failure counter and watchdog-stop tests all pass.

- [ ] **Step 6: Commit the watchdog fix**

```bash
git add src-tauri/src/runtime.rs
git commit -m "fix(headless): isolate health watchdog from tokio runtime"
```

### Task 4: 完整本地验证

**Files:**
- Modify only if formatting or clippy identifies a defect in a touched file.

**Interfaces:**
- Consumes: the three implementation commits.
- Produces: a Beta commit that passes focused tests, formatting and comprehensive clippy.

- [ ] **Step 1: Run focused tests together**

```bash
cd src-tauri
cargo test proxy::token_manager::tests --lib
cargo test runtime::tests --lib
```

Expected: PASS.

- [ ] **Step 2: Run formatting gate**

```bash
cd src-tauri
cargo fmt -- --check
```

Expected: PASS with no diff. If it fails, run `cargo fmt`, inspect the exact diff, and commit only formatting in the touched files.

- [ ] **Step 3: Run comprehensive clippy gate**

```bash
cd src-tauri
cargo clippy --all-targets --all-features
```

Expected: exit code 0. Existing warnings are recorded separately; new warnings in touched code are fixed before release.

- [ ] **Step 4: Verify branch scope and commit history**

```bash
git status --short
git diff --stat origin/main...HEAD
git log --oneline origin/main..HEAD
```

Expected: clean worktree; only the design, plan, `token_manager.rs`, and `runtime.rs` differ from `origin/main`.

### Task 5: Beta 镜像、生产更新和回归验证

**Files:**
- No repository file changes expected.
- Server target: `root@149.56.241.197:7652`, container `antigravity-manager`.

**Interfaces:**
- Consumes: exact local Beta commit and existing production container configuration.
- Produces: a uniquely tagged Beta Docker image, a restarted production container using the same mounts/ports/env, and verification evidence.

- [ ] **Step 1: Record rollback evidence before any server mutation**

Over SSH, record the container ID, image reference, immutable image ID, mounts, port bindings, restart policy, environment-variable names, health status and recent logs. Do not print secret environment values. Export `docker inspect` to a timestamped file under `/root/antigravity-backups/` and retain the current image.

- [ ] **Step 2: Push the Beta branch without altering stable refs**

```bash
git push -u origin beta
```

Expected: `origin/beta` points at the exact tested commit; `origin/main` is unchanged.

- [ ] **Step 3: Build a uniquely tagged image from the exact Beta commit**

Use the repository Dockerfile and tag the image with the short commit, for example `antigravity-manager:beta-<short-sha>`. Verify the image label or embedded version corresponds to the tested commit. Do not tag it `latest`.

- [ ] **Step 4: Recreate only the application container**

Recreate `antigravity-manager` with the recorded mounts, port `8045`, environment and `unless-stopped` restart policy. Do not delete account data, configuration, volumes or the previous image.

- [ ] **Step 5: Verify infrastructure health**

Check `docker ps`, health status, restart count, memory use, `/health`, `/v1/models`, and logs after startup. Confirm the container remains responsive across several watchdog intervals.

- [ ] **Step 6: Verify Opus 5.5 routing with a real request**

Send one minimal request through the existing OpenAI-compatible endpoint using an existing server-side credential without printing it. Accept only an upstream/model response as success; reject `all_accounts_limited`, `No accounts available with quota for model: claude`, HTTP 503, timeout or connection failure.

- [ ] **Step 7: Roll back on any failed acceptance criterion**

If health or the real Opus request fails, recreate the container from the recorded immutable previous image ID and the saved inspect configuration. Re-run `/health` and record that rollback completed.

- [ ] **Step 8: Report the deployed evidence**

Report Beta commit, image tag and image ID, previous rollback image ID, container health, `/health`, `/v1/models`, real Opus 5.5 result, restart count and any paths that remain unverified.

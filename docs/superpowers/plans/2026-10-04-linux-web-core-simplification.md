# Linux/Web Core Simplification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 Antigravity Manager 精简为只面向 Linux 服务器的纯 Axum 服务与 Web 管理面板，同时保留账号调度、代理池、动态模型、日志和 Token 统计等核心能力。

**Architecture:** 保留 `src-tauri/` 路径但移除全部 Tauri 运行时、插件和桌面发布资产。单一 Tokio 进程持有配置、TokenManager 与 AxumServer，前端只通过 `/api/*` 访问同一内存状态；逻辑代理停止仅屏蔽生成端点，管理 API 与 Web 始终可用。

**Tech Stack:** Rust 1.96 edition 2024, Tokio, Axum 0.7, Rusqlite, React 19, TypeScript 5.8, Vite 7, Docker BuildKit, GitHub Actions, GHCR.

## Global Constraints

- 保留 `src-tauri/` 目录名，但最终依赖树和源码不得包含 Tauri、GTK、WebKit、AppIndicator 或桌面插件。
- 只支持 Linux 服务器与 Web 管理面板；不保留可构建的桌面 feature。
- 保留账号/OAuth、配额保护、账号排名、周配额预留、六个不同账号重试、代理池绑定、协议转换、动态模型目录、日志、Token/成本统计、User Token 和安全配置。
- 删除 Codex/OpenCode/Hermes/OpenClaw/Droid 本机配置同步、Cloudflared、桌面更新、托盘、自启动、Mini View、系统文件对话框和本机目录操作。
- 删除 `/apikey-fun` 中转站页面、路由、菜单、设置项、翻译、localStorage 数据、专属素材和 README 推广内容。
- `/api/proxy/models` 是唯一管理模型目录接口；逻辑代理停止时仍可读，且查询不得写账号或配置文件。
- 配置保存必须原子写盘并同步更新唯一内存状态；禁止磁盘配置和运行配置分叉。
- 旧配置中的删除字段必须可被忽略；不得删除、迁移或重建生产账号、凭据、SQLite、代理绑定或数据目录。
- Rust 使用 edition 2024；不新增外部数据库、任务服务或不必要运行时依赖。
- 详细日志关闭时 Token/成本统计仍必须累计。
- 生产发布只使用 GitHub CI 通过的精确 commit 和 GHCR digest。

---

## File Structure

- `src-tauri/src/runtime.rs`：唯一 Linux 服务生命周期、环境覆盖、数据库初始化和优雅退出。
- `src-tauri/src/modules/quota_refresh.rs`：调度器与 HTTP 管理接口共用的配额刷新服务，不依赖 Tauri command。
- `src-tauri/src/proxy/server.rs`：Axum 路由、唯一 AppState、模型目录与配置热更新。
- `src-tauri/src/main.rs`、`src-tauri/src/lib.rs`：纯 Tokio 入口与模块导出。
- `src/utils/request.ts`：浏览器 HTTP 请求唯一入口。
- `src/utils/browserFiles.ts`：浏览器 JSON 上传和下载的最小工具。
- `src/App.tsx`、`src/pages/*`、`src/components/*`：只保留 Web 核心界面。
- `docker/Dockerfile`、`docker/docker-compose.release.yml`：唯一生产容器构建与部署定义。
- `.github/workflows/ci.yml`：Rust、前端、server-only 静态门禁与 Linux/amd64 镜像发布。

---

### Task 1: Replace the dual desktop/headless runtime with one server runtime

**Files:**
- Create: `src-tauri/src/runtime.rs`
- Create: `src-tauri/src/modules/quota_refresh.rs`
- Modify: `src-tauri/src/main.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/proxy/server.rs`
- Modify: `src-tauri/src/modules/account_service.rs`
- Modify: `src-tauri/src/modules/scheduler.rs`
- Modify: `src-tauri/src/modules/log_bridge.rs`
- Modify: `src-tauri/src/modules/oauth_server.rs`
- Modify: `src-tauri/src/modules/mod.rs`
- Modify: `src-tauri/src/proxy/monitor.rs`
- Modify: `src-tauri/src/proxy/common/model_mapping.rs`
- Test: `src-tauri/src/runtime.rs`
- Test: `src-tauri/src/proxy/server.rs`

**Interfaces:**
- Produces: `runtime::run() -> Result<(), String>`.
- Produces: `ServerRuntime { server: AxumServer, task: JoinHandle<()> }`.
- Produces: `quota_refresh::refresh_quotas(token_manager: &TokenManager, protected_only: bool) -> Result<RefreshStats, String>`.
- Produces: `scheduler::start_scheduler(token_manager: Arc<TokenManager>) -> JoinHandle<()>`.
- Consumes: Task 2 dynamic catalog commits `70f4e09` and `beb4a61`; preserve the useful uncommitted `model_ids_from_catalog` helper and remove stopped-desktop-command-only test code by editing it out.

- [ ] **Step 1: Write failing runtime and model-state tests**

Add focused tests that assert one persistent TokenManager serves `/api/proxy/models` before and after logical proxy stop, config updates change model IDs immediately, and reads do not change config/account bytes.

```rust
#[tokio::test]
async fn model_catalog_uses_one_state_while_proxy_is_stopped() {
    let fixture = ServerFixture::new().await;
    fixture.stop_proxy().await;
    let before = fixture.snapshot_data_files();
    assert_eq!(fixture.get_model_ids().await, vec!["gemini-3.8-flash"]);
    assert_eq!(before, fixture.snapshot_data_files());
}

#[tokio::test]
async fn saving_mapping_updates_the_same_model_catalog() {
    let fixture = ServerFixture::new().await;
    fixture.save_mapping("my-model", "gemini-3.8-flash").await;
    assert!(fixture.get_model_ids().await.contains(&"my-model".to_string()));
}
```

- [ ] **Step 2: Verify red**

```bash
cargo +1.96.0 test --locked --manifest-path src-tauri/Cargo.toml server_runtime_ -- --test-threads=1
cargo +1.96.0 test --locked --manifest-path src-tauri/Cargo.toml admin_model_catalog_ -- --test-threads=1
```

Expected: FAIL because the single runtime and shared state fixture do not exist and the current stopped command reloads from disk.

- [ ] **Step 3: Extract command-independent quota refresh**

Move the current all-account and protected-only refresh bodies into:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshStats {
    pub success: usize,
    pub failed: usize,
}

pub async fn refresh_quotas(
    token_manager: &crate::proxy::TokenManager,
    protected_only: bool,
) -> Result<RefreshStats, String>;
```

The function updates account JSON only as an explicit refresh operation, reloads the supplied TokenManager, and emits no desktop events.

- [ ] **Step 4: Build the single runtime**

Implement:

```rust
pub struct ServerRuntime {
    pub server: crate::proxy::AxumServer,
    task: tokio::task::JoinHandle<()>,
}

impl ServerRuntime {
    pub async fn start(config: crate::models::AppConfig) -> Result<Self, String>;
    pub async fn shutdown(self);
}

pub async fn run() -> Result<(), String>;
```

`run` handles `--health-check`, initializes logger and the three SQLite stores, loads/migrates config once, applies `ABV_API_KEY`/`API_KEY`, `ABV_WEB_PASSWORD`/`WEB_PASSWORD`, `ABV_AUTH_MODE`/`AUTH_MODE`, and `ABV_BIND_LOCAL_ONLY`, persists only explicit startup overrides, starts Axum and scheduler, then waits for `ctrl_c`.

- [ ] **Step 5: Remove desktop state from core services**

Make `AccountService::new()` parameterless and keep account switching in server memory. Remove `SystemManager`, `AppHandle`, emitter and notification parameters from `AxumServer::start`, AppState, scheduler, OAuth callback, monitor and log bridge. Replace `tauri::Url` with the existing `url::Url`. Keep the in-memory Web debug log buffer.

- [ ] **Step 6: Make main Tokio-only**

```rust
#[tokio::main]
async fn main() {
    if let Err(error) = antigravity_tools_lib::runtime::run().await {
        eprintln!("server startup failed: {error}");
        std::process::exit(1);
    }
}
```

- [ ] **Step 7: Verify green**

```bash
cargo +1.96.0 fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo +1.96.0 test --locked --manifest-path src-tauri/Cargo.toml server_runtime_ -- --test-threads=1
cargo +1.96.0 test --locked --manifest-path src-tauri/Cargo.toml admin_model_catalog_ -- --test-threads=1
cargo +1.96.0 check --locked --manifest-path src-tauri/Cargo.toml
```

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/runtime.rs src-tauri/src/main.rs src-tauri/src/lib.rs src-tauri/src/proxy/server.rs src-tauri/src/proxy/monitor.rs src-tauri/src/modules/quota_refresh.rs src-tauri/src/modules/account_service.rs src-tauri/src/modules/scheduler.rs src-tauri/src/modules/log_bridge.rs src-tauri/src/modules/oauth_server.rs src-tauri/src/modules/mod.rs src-tauri/src/proxy/common/model_mapping.rs
git commit -m "refactor(server): use one axum runtime"
```

---

### Task 2: Remove desktop and local-workstation backend code

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`
- Modify: `src-tauri/src/constants.rs`, `src-tauri/src/error.rs`, `src-tauri/src/models/config.rs`, `src-tauri/src/modules/migration.rs`, `src-tauri/src/modules/mod.rs`, `src-tauri/src/proxy/mod.rs`, `src-tauri/src/proxy/server.rs`
- Delete: `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/Entitlements.plist`, `src-tauri/installer.nsi`, `src-tauri/build.bat`
- Delete: `src-tauri/capabilities/`, `src-tauri/icons/`, `src-tauri/resources/WebView2Loader.dll`
- Delete: `src-tauri/src/commands/`, `src-tauri/src/linux_graphics.rs`
- Delete: `src-tauri/src/modules/cloudflared.rs`, `http_api.rs`, `integration.rs`, `lightweight.rs`, `process.rs`, `startup_quiet.rs`, `tray.rs`, `update_checker.rs`, `version.rs`
- Delete: `src-tauri/src/proxy/cli_sync.rs`, `droid_sync.rs`, `hermes_sync.rs`, `openclaw_sync.rs`, `opencode_sync.rs`
- Delete: `src-tauri/src/utils/command.rs`, `win_shortcut.rs`
- Test: `src-tauri/src/models/config.rs`, `src-tauri/src/proxy/server.rs`

**Interfaces:**
- Consumes: Task 1 server runtime and shared quota refresh.
- Produces: a Rust edition 2024 crate with no desktop dependency and no local-workstation admin routes.
- Preserves: `src-tauri/src/proxy/common/client_adapters/opencode.rs`; it is request compatibility, not local config sync.

- [ ] **Step 1: Add failing compatibility tests**

```rust
#[test]
fn legacy_desktop_fields_are_ignored() {
    let value = serde_json::json!({
        "language": "zh",
        "theme": "dark",
        "auto_refresh": true,
        "refresh_interval": 15,
        "auto_sync": false,
        "sync_interval": 5,
        "proxy": crate::proxy::ProxyConfig::default(),
        "auto_launch": true,
        "cloudflared": { "mode": "quick" }
    });
    let config: AppConfig = serde_json::from_value(value).unwrap();
    assert_eq!(config.language, "zh");
}
```

Add router assertions that `/api/proxy/cloudflared/status`, `/api/proxy/opencode/status`, `/api/system/autostart/status`, `/api/system/updates/check`, `/api/system/open-folder`, `/api/accounts/import/db`, `/api/accounts/import/db-custom` and `/api/accounts/sync/db` return 404.

- [ ] **Step 2: Verify red**

```bash
cargo +1.96.0 test --locked --manifest-path src-tauri/Cargo.toml legacy_desktop_fields_are_ignored -- --exact
cargo +1.96.0 test --locked --manifest-path src-tauri/Cargo.toml removed_admin_routes_return_not_found -- --exact
```

Expected: route test FAIL because deleted endpoints still exist.

- [ ] **Step 3: Delete desktop module graph and routes**

Remove the files listed above and every route/handler/import that exists only for them. Keep account device-profile generation and binding code that modifies account data; remove only paths that inspect or mutate a desktop Antigravity installation. Retain JSON/v1 account import, but prune `modules/migration.rs` functions that read desktop SQLite, Keyring or arbitrary server-local paths. Remove the Tauri-specific error variant from `error.rs`.

- [ ] **Step 4: Shrink AppConfig without rewriting old files**

Remove desktop executable paths, desktop auto-launch, quiet startup, Cloudflared and lightweight fields from `AppConfig`. Keep language, theme, refresh scheduling, quota protection, pinned models, circuit breaker, hidden menu items and thinking cleanup settings. Rely on Serde's default unknown-field behavior so existing JSON remains readable.

- [ ] **Step 5: Remove Rust dependencies and select edition 2024**

Set `edition = "2024"`. Remove `tauri`, `tauri-build`, every `tauri-plugin-*`, `gtk`, `plist` and dependencies proven unused after module deletion. Keep `machine-uid` because `utils/crypto.rs` uses it to derive the existing account-encryption key; removing or changing that derivation would make persisted credentials unreadable. Replace `constants.rs` desktop-version probing with the existing stable server user-agent floor, then regenerate `Cargo.lock` with Rust 1.96.

- [ ] **Step 6: Add static server-only gates**

```bash
rg -n 'tauri::|tauri-plugin|tauri_build|gtk|webkit|appindicator' src-tauri/Cargo.toml src-tauri/src
rg -n 'cloudflared|opencode_sync|hermes_sync|openclaw_sync|droid_sync|cli_sync' src-tauri/src
```

Expected: both commands have no output.

- [ ] **Step 7: Verify green**

```bash
cargo +1.96.0 fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo +1.96.0 check --locked --manifest-path src-tauri/Cargo.toml --all-targets
cargo +1.96.0 test --locked --manifest-path src-tauri/Cargo.toml legacy_desktop_fields_are_ignored -- --exact
cargo +1.96.0 test --locked --manifest-path src-tauri/Cargo.toml removed_admin_routes_return_not_found -- --exact
```

- [ ] **Step 8: Commit**

```bash
git add -A src-tauri
git commit -m "refactor(server): remove desktop backend"
```

---

### Task 3: Make the Web panel HTTP-only and preserve browser workflows

**Files:**
- Create: `src/utils/browserFiles.ts`
- Modify: `src/utils/request.ts`, `src/main.tsx`, `src/App.tsx`
- Modify: `src/components/common/AdminAuthGuard.tsx`, `ThemeManager.tsx`
- Modify: `src/components/layout/Layout.tsx`
- Modify: `src/components/navbar/Navbar.tsx`, `NavDropdowns.tsx`, `NavSettings.tsx`
- Modify: `src/components/accounts/AddAccountDialog.tsx`, `DeviceFingerprintDialog.tsx`
- Modify: `src/components/proxy/ProxyMonitor.tsx`
- Modify: `src/pages/Accounts.tsx`, `Dashboard.tsx`, `ApiProxy.tsx`, `Settings.tsx`
- Modify: `src/stores/useConfigStore.ts`, `src/stores/useDebugConsole.ts`
- Modify: `src/hooks/useProxyModels.tsx`, `src/types/config.ts`, `package.json`, `package-lock.json`
- Modify: `src/locales/ar.json`, `en.json`, `es.json`, `ja.json`, `ko.json`, `my.json`, `pt.json`, `ru.json`, `tr.json`, `vi.json`, `zh-TW.json`, `zh.json`
- Delete: `src/components/UpdateNotification.tsx`, `src/components/layout/MiniView.tsx`
- Delete: `src/components/proxy/CliSyncCard.tsx`, `HermesSyncModal.tsx`, `OpenClawSyncModal.tsx`, `OpenCodeSyncModal.tsx`
- Delete: `src/stores/useViewStore.ts`, `src/utils/windowManager.ts`, `src/utils/opencodeProfiles.ts`, `src/utils/env.ts`
- Test: `tests/dashboard/proxyModels.test.ts`
- Create test: `tests/dashboard/httpOnly.test.ts`

**Interfaces:**
- Consumes: `/api/*` routes from Tasks 1-2.
- Produces: `request<T>(command: string, args?: Record<string, unknown>) -> Promise<T>` using Fetch only.
- Produces: `downloadJson(filename: string, value: unknown): void` and `readJsonFile(file: File): Promise<unknown>`.
- Produces: `buildProxyModels(ids: string[]): ProxyModel[]` from backend IDs only.

- [ ] **Step 1: Write failing browser-only tests**

```ts
assert.equal(source.includes('@tauri-apps'), false);
assert.equal(source.includes('isTauri('), false);
await request('get_proxy_models');
assert.equal(lastFetchUrl, '/api/proxy/models');

const models = buildProxyModels(['claude-opus-5-5', 'future-model-9']);
assert.deepEqual(models.map(model => model.id), ['claude-opus-5-5', 'future-model-9']);
```

Test `save_config` dispatches `proxy-models-updated` and the hook refetches once. Test `readJsonFile` parses a browser `File` and rejects invalid JSON.

- [ ] **Step 2: Verify red**

```bash
npx tsx tests/dashboard/httpOnly.test.ts
npm run test:dashboard
```

Expected: FAIL because Tauri imports and environment branches still exist.

- [ ] **Step 3: Rewrite the request layer**

Remove dynamic Tauri imports. Keep the established command-to-route table only for retained HTTP endpoints. Unknown commands throw `Unsupported command: <name>`. Preserve admin password headers and parameter substitution.

- [ ] **Step 4: Preserve import/export in the browser**

```ts
export function downloadJson(filename: string, value: unknown): void {
  const url = URL.createObjectURL(new Blob([JSON.stringify(value, null, 2)], { type: 'application/json' }));
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  URL.revokeObjectURL(url);
}

export async function readJsonFile(file: File): Promise<unknown> {
  return JSON.parse(await file.text());
}
```

Use these helpers in account import/export and dashboard log export. Do not add a file library.

- [ ] **Step 5: Remove desktop-only UI**

Delete updater, mini view, window state, local sync cards/modals, Cloudflared controls, local executable/data-directory/autostart/update sections, desktop-only event listeners and their dedicated locale namespaces. Keep server proxy settings, proxy pool, quota protection, model routing, monitoring and security.

- [ ] **Step 6: Make model refresh explicit**

Listen for `proxy-models-updated` in `useProxyModels`, increment a local refresh epoch, and refetch `/api/proxy/models`. `request('save_config')` dispatches the event only after a successful response.

- [ ] **Step 7: Remove frontend dependencies**

Remove every `@tauri-apps/*` runtime package and `@tauri-apps/cli`, then regenerate `package-lock.json` with `npm install --package-lock-only --legacy-peer-deps`.

- [ ] **Step 8: Verify green**

```bash
npm run test:dashboard
npm run build
test -z "$(rg -l '@tauri-apps|isTauri\(' src package.json)"
```

- [ ] **Step 9: Commit**

```bash
git add src package.json package-lock.json tests/dashboard
git commit -m "refactor(web): remove desktop client paths"
```

---

### Task 4: Remove the relay promotion page and related assets

**Files:**
- Delete: `src/pages/ApiKeyFun.tsx`
- Modify: `src/App.tsx`, `src/components/navbar/Navbar.tsx`, `src/pages/Settings.tsx`
- Modify: `src/locales/ar.json`, `en.json`, `es.json`, `ja.json`, `ko.json`, `my.json`, `pt.json`, `ru.json`, `tr.json`, `vi.json`, `zh-TW.json`, `zh.json`
- Modify: `README.md`, `README_EN.md`, `README_ZH.md`
- Delete: APIKEY.FUN page-only image assets identified by `rg -l 'APIKEY|apikey.fun' src public docs`
- Delete: `scripts/test-opencode-profiles.mjs` if it has no remaining consumer after Task 3.
- Test: `tests/dashboard/httpOnly.test.ts`

**Interfaces:**
- Produces: no `/apikey-fun` route or navigation entry.
- Preserves: API Proxy and `proxy_pool`; generic operational uses of the Chinese word “中转” in protocol/log descriptions are not promotion code.

- [ ] **Step 1: Add failing route/source test**

Add assertions that route declarations, navigation arrays, menu settings and locale namespaces contain neither `/apikey-fun` nor `nav.apikey_fun` nor top-level `apiKeyFun`.

- [ ] **Step 2: Verify red**

```bash
npx tsx tests/dashboard/httpOnly.test.ts
```

Expected: FAIL on the existing route and navigation item.

- [ ] **Step 3: Delete the feature**

Delete the page and page-only helper/assets, remove route/nav/menu settings, remove the dedicated locale namespaces, and remove APIKEY.FUN promotional README rows and copy. Do not modify proxy routing or proxy-pool code.

- [ ] **Step 4: Verify green**

```bash
npx tsx tests/dashboard/httpOnly.test.ts
npm run build
test -z "$(rg -l '/apikey-fun|nav\.apikey_fun|apiKeyFun' src README.md README_EN.md README_ZH.md)"
```

- [ ] **Step 5: Commit**

```bash
git add -A src README.md README_EN.md README_ZH.md public docs scripts/test-opencode-profiles.mjs tests/dashboard/httpOnly.test.ts
git commit -m "refactor(web): remove relay promotion"
```

---

### Task 5: Simplify Docker, CI and server documentation

**Files:**
- Modify: `docker/Dockerfile`, `docker/docker-compose.release.yml`, `docker/docker-compose.yml`, `docker/.env.example`, `docker/README.md`
- Modify: `.github/workflows/ci.yml`, `.github/PULL_REQUEST_TEMPLATE.md`, `.github/CODEOWNERS`
- Modify: `scripts/ci_smoke.py`, `scripts/build_release_assets.py`, `scripts/release_metadata.py`
- Delete: `docker/Dockerfile.backend`, `Dockerfile.backend.localdist`, `Dockerfile.compat`
- Delete: `docker/docker-compose.backend.yml`, `docker-compose.compat.yml`, `docker-compose.fork.yml`, `docker-compose.localdist.yml`, `docker/build.ps1`
- Delete: `scripts/Fix_Damaged.command`, `scripts/fix_app.sh`, `scripts/package_dmg.sh`
- Modify: root `README.md`, `README_EN.md`, `README_ZH.md`

**Interfaces:**
- Produces: one `docker/Dockerfile`, one local compose and one release compose.
- Produces: Linux/amd64 GHCR image containing Web dist and pure server binary.

- [ ] **Step 1: Add CI server-only gates**

```bash
test -z "$(rg -l '@tauri-apps|isTauri\(' src package.json)"
test -z "$(rg -l 'tauri::|tauri-plugin|tauri_build|gtk|webkit|appindicator' src-tauri/Cargo.toml src-tauri/src)"
test -z "$(rg -l '/apikey-fun|nav\.apikey_fun|apiKeyFun' src)"
```

CI installs Rust 1.96 with rustfmt and clippy, runs `cargo check --all-targets`, focused compatibility tests, dashboard tests and frontend build before image publication.

- [ ] **Step 2: Rewrite the single Dockerfile**

Keep Node 20 frontend build and Rust 1.96-compatible backend build. Builder packages are limited to actual native build requirements such as `build-essential`, `pkg-config`, `cmake`, `clang`, `libclang-dev`, `perl`, `golang-go` and certificates. Runtime installs only `ca-certificates` and `curl` unless `ldd` proves another shared library is required. Entrypoint is:

```dockerfile
ENTRYPOINT ["/app/antigravity-tools"]
```

- [ ] **Step 3: Consolidate Compose files**

Keep the persistent mount target `/root/.antigravity_tools`, environment variables and port `8045`. Remove desktop/headless wording and obsolete variants. Release compose continues to use `ghcr.io/echo7659/antigravity-manager-lee` and must support digest pinning.

- [ ] **Step 4: Update smoke checks and docs**

Smoke-test `/health`, admin authentication, `/api/proxy/models`, static `index.html`, and 404 for one removed route. README describes Linux/Docker and Web only; no desktop installation instructions or desktop screenshots remain.

- [ ] **Step 5: Verify locally**

```bash
npm ci --legacy-peer-deps
npm run test:dashboard
npm run build
cargo +1.96.0 fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo +1.96.0 check --locked --manifest-path src-tauri/Cargo.toml --all-targets
cargo +1.96.0 clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets --all-features
docker build -f docker/Dockerfile -t antigravity-manager:server-only .
docker run -d --rm --name antigravity-server-only -p 127.0.0.1:18045:8045 -e API_KEY=smoke-only -e WEB_PASSWORD=smoke-only antigravity-manager:server-only
python3 scripts/ci_smoke.py --base-url http://127.0.0.1:18045 --version 4.9.1
docker stop antigravity-server-only
```

- [ ] **Step 6: Commit**

```bash
git add -A docker .github scripts README.md README_EN.md README_ZH.md
git commit -m "chore(server): simplify linux container release"
```

---

### Task 6: Run the complete simplification regression gate

**Files:**
- Modify only files required by failures found in this task.
- Append evidence: `.superpowers/sdd/2026-10-04-linux-web-core-simplification/task-6-report.md`.

**Interfaces:**
- Produces: a clean server-only branch ready for the Opus 5.5 rollout plan.

- [ ] **Step 1: Run static boundaries**

```bash
test -z "$(rg -l '@tauri-apps|isTauri\(' src package.json)"
test -z "$(rg -l 'tauri::|tauri-plugin|tauri_build|gtk|webkit|appindicator' src-tauri/Cargo.toml src-tauri/src)"
test -z "$(rg -l 'cloudflared|opencode_sync|hermes_sync|openclaw_sync|droid_sync|cli_sync' src-tauri/src src)"
test -z "$(rg -l '/apikey-fun|nav\.apikey_fun|apiKeyFun' src)"
```

- [ ] **Step 2: Run Rust gates**

```bash
cargo +1.96.0 fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo +1.96.0 check --locked --manifest-path src-tauri/Cargo.toml --all-targets
cargo +1.96.0 clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets --all-features
python3 scripts/prepare_compat_tests.py /tmp/antigravity-compat-tests
cargo +1.96.0 test --locked --manifest-path /tmp/antigravity-compat-tests/Cargo.toml --lib compat_
cargo +1.96.0 test --locked --manifest-path src-tauri/Cargo.toml model_catalog_ -- --test-threads=1
```

- [ ] **Step 3: Run Web and container gates**

```bash
npm ci --legacy-peer-deps
npm run test:dashboard
npm run build
docker build -f docker/Dockerfile -t antigravity-manager:server-only .
```

- [ ] **Step 4: Compare core persistence behavior**

Start the image against a temporary copy of representative config/account/SQLite fixtures. Assert account count, custom mapping, proxy bindings and token-stat rows are unchanged after startup and model-catalog reads.

- [ ] **Step 5: Commit only necessary fixes**

```bash
git status --short
git diff --check
git add -A
git commit -m "test(server): lock core-only regression gates"
```

If no source or test change is needed, do not create an empty commit; record the clean gate in the task report.

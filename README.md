# Antigravity Manager Lee (v4.9.1)

A Linux server and Web management panel built on Rust, Axum and React. It normalizes OpenAI Responses, OpenAI Chat Completions, Anthropic Claude and Google Gemini requests into a shared Gemini pipeline.

[简体中文](README_ZH.md) · [English](README_EN.md) · [Actions](https://github.com/Echo7659/Antigravity-Manager-Lee/actions) · [Releases](https://github.com/Echo7659/Antigravity-Manager-Lee/releases)

## Core capabilities

- Account JSON import/export, browser OAuth, quota refresh and account health management.
- Quota protection, reset-time ordering, weekly reserves and at most six different accounts per generation request.
- A dynamic model catalog at `/api/proxy/models`, with 24-hour freshness and last-known-good fallback.
- Account proxy bindings, proxy pools and configuration updates applied to the running server.
- Request/debug logs, Token and cost statistics, User Tokens, separate API and management authentication.
- One HTTP service for Web, management APIs and generation endpoints. Stopping the API Proxy leaves management and the model catalog available.

## Docker deployment

Published images target **Linux/amd64** at `ghcr.io/echo7659/antigravity-manager-lee`. Use a digest from a successful CI run for the exact source commit.

Stable tags `vX.Y.Z` must belong to `origin/main`; beta tags `vX.Y.Z-beta.N` must belong to `origin/beta`. CI validates the exact tag, manifest versions and bilingual changelog before any image push. Only stable main tags update latest; beta releases are prereleases and never become latest. Branch builds publish SHA images only.

Copy `docker/.env.example` to `docker/.env`, then set `API_KEY`, `WEB_PASSWORD` and `IMAGE_DIGEST=sha256:...`. For existing deployments, set `ABV_HOST_DATA_DIR` to the existing absolute data path before starting.

```sh
docker compose --env-file docker/.env -f docker/docker-compose.release.yml pull
docker compose --env-file docker/.env -f docker/docker-compose.release.yml up -d
```

Open `http://localhost:8045` for Web management. Configure clients with the required protocol endpoint and API key; use the live model catalog instead of a fixed list. See the [Docker guide](docker/README.md) for configuration, upgrades, rollback and local builds.

## Persistence and security

Keep the mount at `/root/.antigravity_tools`: account JSON, credentials, `gui_config.json`, proxy bindings and SQLite logs/statistics share this directory. Preserve the original host path and secrets during upgrades. Do not replace an existing data directory with an empty one.

Management APIs require the Web password; API clients use their API key or User Token. Environment overrides are applied at startup, and Web configuration saves update disk and running state. Use an HTTPS reverse proxy and restrict management access when exposing the service.

Before upgrading, record the old image digest and retain a consistent data backup. Roll back by restoring the old `IMAGE_DIGEST` and recreating the container with the same mount and environment. See [account failover](docs/ACCOUNT_FAILOVER.md), [quota behavior](docs/QUOTA_AND_STABILITY.md) and [upstream reconciliation](docs/UPSTREAM_4_8_1.md).

## Development and verification

The backend remains in `src-tauri/` for repository compatibility and uses Rust 1.96, edition 2024. Web builds use Node 20.

```sh
npm ci --legacy-peer-deps
npm run test:dashboard
npm run build
bash scripts/check_server_only.sh
cargo +1.96.0 fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo +1.96.0 check --locked --manifest-path src-tauri/Cargo.toml --all-targets
cargo +1.96.0 clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets --all-features
```

CI also runs focused state/configuration and protocol compatibility tests, builds the Linux image, verifies container health, authentication, model catalog, Web assets and removed-route 404, then publishes the image digest. Application version and historical contributor attribution remain unchanged.

## 📝 Changelog

- **v4.8.1-lee.3**：同步官方 4.8.1，保留现有稳定性与仪表盘修复。[合并说明](docs/UPSTREAM_4_8_1.md)。感谢 @jeikl、@Avlaak 及上游贡献者。

> Latest version **v4.9.1** (2026-10-02): Fixed `gemini-3.1-flash-lite` misrouting to the retired 2.5 family to restore healthy 200 OK passthrough, revived Layer-3 background compression, purged dead 2.5 models from advertised catalogs while routing legacy requests to `gemini-3.6-flash-medium` (Fixes #3577, thanks to @Xyloz3n); tightened downstream SSE thinking heartbeats to 3s to prevent client disconnects during deep reasoning (PR #3578, thanks to @EricZhou05).

👉 **[View Full Changelog → CHANGELOG_EN.md](CHANGELOG_EN.md)**

<details>
<summary><b>👥 Contributors - Click to expand</b></summary>

<a href="https://github.com/lbjlaq"><img src="https://github.com/lbjlaq.png" width="50px" style="border-radius: 50%;" alt="lbjlaq"/></a>
<a href="https://github.com/jeikl"><img src="https://github.com/jeikl.png" width="50px" style="border-radius: 50%;" alt="jeikl"/></a>
<a href="https://github.com/XinXin622"><img src="https://github.com/XinXin622.png" width="50px" style="border-radius: 50%;" alt="XinXin622"/></a>
<a href="https://github.com/llsenyue"><img src="https://github.com/llsenyue.png" width="50px" style="border-radius: 50%;" alt="llsenyue"/></a>
<a href="https://github.com/salacoste"><img src="https://github.com/salacoste.png" width="50px" style="border-radius: 50%;" alt="salacoste"/></a>
<a href="https://github.com/84hero"><img src="https://github.com/84hero.png" width="50px" style="border-radius: 50%;" alt="84hero"/></a>
<a href="https://github.com/karasungur"><img src="https://github.com/karasungur.png" width="50px" style="border-radius: 50%;" alt="karasungur"/></a>
<a href="https://github.com/marovole"><img src="https://github.com/marovole.png" width="50px" style="border-radius: 50%;" alt="marovole"/></a>
<a href="https://github.com/wanglei8888"><img src="https://github.com/wanglei8888.png" width="50px" style="border-radius: 50%;" alt="wanglei8888"/></a>
<a href="https://github.com/yinjianhong22-design"><img src="https://github.com/yinjianhong22-design.png" width="50px" style="border-radius: 50%;" alt="yinjianhong22-design"/></a>
<a href="https://github.com/Mag1cFall"><img src="https://github.com/Mag1cFall.png" width="50px" style="border-radius: 50%;" alt="Mag1cFall"/></a>
<a href="https://github.com/AmbitionsXXXV"><img src="https://github.com/AmbitionsXXXV.png" width="50px" style="border-radius: 50%;" alt="AmbitionsXXXV"/></a>
<a href="https://github.com/fishheadwithchili"><img src="https://github.com/fishheadwithchili.png" width="50px" style="border-radius: 50%;" alt="fishheadwithchili"/></a>
<a href="https://github.com/ThanhNguyxn"><img src="https://github.com/ThanhNguyxn.png" width="50px" style="border-radius: 50%;" alt="ThanhNguyxn"/></a>
<a href="https://github.com/Stranmor"><img src="https://github.com/Stranmor.png" width="50px" style="border-radius: 50%;" alt="Stranmor"/></a>
<a href="https://github.com/Jint8888"><img src="https://github.com/Jint8888.png" width="50px" style="border-radius: 50%;" alt="Jint8888"/></a>
<a href="https://github.com/0-don"><img src="https://github.com/0-don.png" width="50px" style="border-radius: 50%;" alt="0-don"/></a>
<a href="https://github.com/dlukt"><img src="https://github.com/dlukt.png" width="50px" style="border-radius: 50%;" alt="dlukt"/></a>
<a href="https://github.com/Silviovespoli"><img src="https://github.com/Silviovespoli.png" width="50px" style="border-radius: 50%;" alt="Silviovespoli"/></a>
<a href="https://github.com/i-smile"><img src="https://github.com/i-smile.png" width="50px" style="border-radius: 50%;" alt="i-smile"/></a>
<a href="https://github.com/jalen0x"><img src="https://github.com/jalen0x.png" width="50px" style="border-radius: 50%;" alt="jalen0x"/></a>
<a href="https://linux.do/u/wendavid"><img src="https://linux.do/user_avatar/linux.do/wendavid/48/122218_2.png" width="50px" style="border-radius: 50%;" alt="wendavid"/></a>
<a href="https://github.com/byte-sunlight"><img src="https://github.com/byte-sunlight.png" width="50px" style="border-radius: 50%;" alt="byte-sunlight"/></a>
<a href="https://github.com/jlcodes99"><img src="https://github.com/jlcodes99.png" width="50px" style="border-radius: 50%;" alt="jlcodes99"/></a>
<a href="https://github.com/Vucius"><img src="https://github.com/Vucius.png" width="50px" style="border-radius: 50%;" alt="Vucius"/></a>
<a href="https://github.com/Koshikai"><img src="https://github.com/Koshikai.png" width="50px" style="border-radius: 50%;" alt="Koshikai"/></a>
<a href="https://github.com/hakanyalitekin"><img src="https://github.com/hakanyalitekin.png" width="50px" style="border-radius: 50%;" alt="hakanyalitekin"/></a>
<a href="https://github.com/Gok-tug"><img src="https://github.com/Gok-tug.png" width="50px" style="border-radius: 50%;" alt="Gok-tug"/></a>
<a href="https://github.com/johngbl"><img src="https://github.com/johngbl.png" width="50px" style="border-radius: 50%;" alt="johngbl"/></a>

Special thanks to all developers who have contributed to this project.

</details>

<details>
<summary><b>🤝 Special Thanks - Click to expand</b></summary>

This project has referenced or learned from the ideas or code of the following excellent open-source projects during its development (in no particular order):

*   [learn-claude-code](https://github.com/shareAI-lab/learn-claude-code)
*   [Practical-Guide-to-Context-Engineering](https://github.com/WakeUp-Jin/Practical-Guide-to-Context-Engineering)
*   [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)
*   [OmniRoute](https://github.com/diegosouzapw/OmniRoute)
*   [antigravity-claude-proxy](https://github.com/badrisnarayanan/antigravity-claude-proxy)
*   [aistudio-gemini-proxy](https://github.com/zhongruichen/aistudio-gemini-proxy)
*   [gcli2api](https://github.com/su-kaka/gcli2api)
*   [agent-vibes](https://github.com/funny-vibes/agent-vibes)

</details>

*   **License**: **CC BY-NC-SA 4.0**. Strictly for non-commercial use.
*   **Security**: Account files and SQLite databases are stored in the mounted server data directory. Requests and OAuth credentials are sent to the configured upstream services as required.

---

<div align="center">
  <p>If you find this tool helpful, please give it a ⭐️ on GitHub!</p>
  <p>Copyright © 2024-2026 Antigravity Team.</p>
</div>

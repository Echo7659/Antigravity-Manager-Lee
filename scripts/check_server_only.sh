#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# 发布边界检查覆盖运行代码、依赖声明和容器入口。
reject() {
    if rg -n "$@"; then
        echo 'Server-only boundary violated' >&2
        exit 1
    else
        local status=$?
        [[ $status == 1 ]] || exit "$status"
    fi
}

reject '@tauri-apps|isTauri\(' src package.json
reject 'tauri::|tauri-plugin|tauri_build|gtk|webkit|appindicator' src-tauri/Cargo.toml src-tauri/src
reject 'cloudflared|opencode_sync|hermes_sync|openclaw_sync|droid_sync|cli_sync' src-tauri/src src
reject '/apikey-fun|nav\.apikey_fun|apiKeyFun' src
reject 'libgtk|webkit|appindicator|src-tauri/icons|--headless' docker/Dockerfile
reject 'Dockerfile\.(compat|backend)|tauri\.conf' .github/workflows scripts/release_metadata.py
reject 'tauri\.conf|tauri-action|tauri build|MiniView|Casks/|\.dmg|\.AppImage|\.msi' \
    .github/workflows scripts/bump-version.mjs scripts/build_release_assets.py scripts/release_metadata.py

for obsolete in docker/Dockerfile.backend docker/Dockerfile.backend.localdist \
    docker/Dockerfile.compat docker/docker-compose.backend.yml \
    docker/docker-compose.compat.yml docker/docker-compose.fork.yml \
    docker/docker-compose.localdist.yml docker/build.ps1 \
    scripts/Fix_Damaged.command scripts/fix_app.sh scripts/package_dmg.sh \
    Casks/antigravity-tools.rb install.sh install.ps1 workflows/release.yml workflows/ci.yml \
    deploy/arch/install.sh deploy/arch/PKGBUILD.template; do
    if [[ -e "$obsolete" ]]; then
        echo "Obsolete release path: $obsolete" >&2
        exit 1
    fi
done
echo 'Server-only boundaries passed.'

"""生成固定镜像摘要的版本部署包，不包含运行凭据。"""
import hashlib
import json
import os
import pathlib
import re
import tarfile

root = pathlib.Path(__file__).resolve().parents[1]
output = pathlib.Path(os.environ['RELEASE_DIR'])
output.mkdir(parents=True, exist_ok=True)
tag = os.environ['RELEASE_TAG']
assert re.fullmatch(r'v\d+\.\d+\.\d+(?:-lee\.[1-9][0-9]*)?', tag)
image = os.environ['IMAGE_NAME']
digest = os.environ['IMAGE_DIGEST']
assert re.fullmatch(r'sha256:[0-9a-f]{64}', digest)
reference = image + '@' + digest
compose = (root / 'docker/docker-compose.release.yml').read_text()
compose = compose.replace('ghcr.io/echo7659/antigravity-manager-lee:latest', reference)
(output / 'docker-compose.yml').write_text(compose)
(output / '.env.example').write_text('API_KEY=\nWEB_PASSWORD=\n')
manifest = {'release': tag, 'base_version': json.loads((root / 'package.json').read_text())['version'], 'source_revision': os.environ['GITHUB_SHA'], 'image': reference, 'platform': 'linux/amd64'}
(output / 'image-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
(output / 'README.md').write_text(f'''# Antigravity Manager Lee {tag}

镜像：`{reference}`

平台：Linux amd64。配置中的数据目录为 `./data`，容器更新后账号和配置继续保留。

首次部署时复制 `.env.example` 为 `.env` 并填写两个密钥，然后执行 `docker compose up -d`。
现有部署应保留原端口、环境变量和数据挂载，不能直接覆盖已有数据目录。

附件是部署配置包；容器镜像从 GHCR 获取。基础应用版本为 {manifest['base_version']}，Lee 发布标签单独标识定制修复。
''')
archive = output / f'antigravity-manager-lee-{tag}-deployment.tar.gz'
with tarfile.open(archive, 'w:gz') as bundle:
    for name in ['docker-compose.yml', '.env.example', 'image-manifest.json', 'README.md']:
        bundle.add(output / name, arcname=name)
(output / 'SHA256SUMS').write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + '  ' + archive.name + '\n')
(output / 'RELEASE_NOTES.md').write_text(f'''## Container release

- Release: `{tag}`
- Base application: `{manifest['base_version']}`
- Source: `{manifest['source_revision']}`
- Platform: `linux/amd64`
- Image: `{reference}`

Synchronizes official v4.7.12, including the protocol pipeline, signature recovery, model routing, quota dashboard and UTF-8 safety fixes. Thanks to upstream lbjlaq/Antigravity-Manager contributors, including @jeikl. See `docs/UPSTREAM_4_7_12.md` for reconciliation details.

Retains uncovered Gemini request compatibility, cumulative/reasoning token accounting and explicit empty-stream error handling. See `docs/GEMINI_COMPAT_DEPLOYMENT.md` for behavior and deployment details.

Account failover now tries the initial account plus up to five different accounts for upstream HTTP errors, including 400 and 429. Successful attempts return immediately; exhaustion preserves the last actual failure. Native Gemini 429 responses now update account/model cooldowns. See `docs/ACCOUNT_FAILOVER.md` for stream and eligibility boundaries.

Fixes account sorting panics that disconnected HTTP requests, uses upstream model/effort routing, and checks inclusive model quota thresholds before account selection. Used-account quotas refresh after responses, selection samples the full eligible subscription tier, and quota reloads preserve cooldowns. See `docs/QUOTA_AND_STABILITY.md` for limits and behavior.

The attached deployment bundle pins the tested image digest and contains no credentials. Existing deployments must retain their data mounts and secrets.
''')
print(archive)

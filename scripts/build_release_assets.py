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
image_line = '    image: ghcr.io/echo7659/antigravity-manager-lee@${IMAGE_DIGEST:?Set IMAGE_DIGEST to the verified sha256 digest}'
assert compose.count(image_line) == 1, 'Release compose image declaration changed'
compose = compose.replace(image_line, '    image: ' + reference)
(output / 'docker-compose.yml').write_text(compose)
(output / '.env.example').write_text((root / 'docker/.env.example').read_text().replace('IMAGE_DIGEST=', 'IMAGE_DIGEST=' + digest))
manifest = {'release': tag, 'base_version': json.loads((root / 'package.json').read_text())['version'], 'source_revision': os.environ['GITHUB_SHA'], 'image': reference, 'platform': 'linux/amd64'}
(output / 'image-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
(output / 'README.md').write_text(f'''# Antigravity Manager Lee {tag}

镜像：`{reference}`

平台：Linux amd64。配置中的数据目录为 `./data`，容器更新后账号和配置继续保留。

首次部署时复制 `.env.example` 为 `.env` 并填写两个密钥，然后执行 `docker compose up -d`。
现有部署应保留原端口、环境变量和数据挂载，不能直接覆盖已有数据目录。

附件是部署配置包；容器镜像从 GHCR 获取。基础应用版本为 {manifest['base_version']}，Lee 发布标签单独标识定制修复。

升级前记录旧镜像 digest，备份数据目录并保留原挂载与密钥；回滚时将 compose 的 image 恢复为旧 digest 后重新启动。
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

This release contains one Linux server and Web panel image. It preserves account scheduling, four-protocol normalization, dynamic model catalogs, proxy pools, quota protection, usage statistics and management authentication.

Upstream attribution and version history are preserved in `CHANGELOG.md` and `CHANGELOG_EN.md`. Thanks to lbjlaq/Antigravity-Manager contributors, @jeikl and @Avlaak (PR #3518); see `docs/UPSTREAM_4_8_1.md` for reconciliation details.

The attached deployment bundle pins the tested image digest and contains no credentials. Existing deployments must retain their data mounts and secrets.
''')
print(archive)

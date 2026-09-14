"""校验发布版本并生成 Actions 使用的镜像元数据。"""
import json
import os
import pathlib
import re
import tomllib

root = pathlib.Path(__file__).resolve().parents[1]
version = json.loads((root / 'package.json').read_text())['version']
rust_version = tomllib.loads((root / 'src-tauri/Cargo.toml').read_text())['package']['version']
tauri_version = json.loads((root / 'src-tauri/tauri.conf.json').read_text())['version']
assert version == rust_version == tauri_version, 'Frontend, Rust and Tauri versions must match'
ref = os.environ.get('GITHUB_REF_NAME', '')
if os.environ.get('GITHUB_REF_TYPE') == 'tag':
    assert re.fullmatch('v' + re.escape(version) + r'(?:-lee\.[1-9][0-9]*)?', ref), 'Release tag must match the base version, optionally followed by -lee.N'
repository = os.environ['GITHUB_REPOSITORY'].lower()
assert re.fullmatch(r'[a-z0-9_.-]+/[a-z0-9_.-]+', repository)
values = {'image': 'ghcr.io/' + repository, 'base_version': version, 'local_image': 'antigravity-lee-ci:' + os.environ['GITHUB_SHA']}
with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
    for name, value in values.items():
        output.write(f'{name}={value}\n')

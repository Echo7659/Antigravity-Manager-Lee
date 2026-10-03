"""在推送镜像前校验版本、发布通道、Git 来源及双语更新日志。"""
import json
import os
import pathlib
import re
import subprocess
import tomllib

NUMBER = r'(?:0|[1-9]\d*)'
VERSION_PATTERN = rf'{NUMBER}\.{NUMBER}\.{NUMBER}(?:-lee\.{NUMBER})?(?:-beta\.{NUMBER})?'


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], text=True, stderr=subprocess.PIPE).strip()


def validate_source(root, revision):
    assert re.fullmatch(r'[0-9a-f]{40}', revision), 'Invalid source revision'
    assert git(root, 'rev-parse', 'HEAD') == revision, 'Checkout must match GITHUB_SHA'
    version = json.loads((root / 'package.json').read_text())['version']
    lock = json.loads((root / 'package-lock.json').read_text())
    rust_version = tomllib.loads((root / 'src-tauri/Cargo.toml').read_text())['package']['version']
    rust_packages = tomllib.loads((root / 'src-tauri/Cargo.lock').read_text())['package']
    rust_lock_versions = [package['version'] for package in rust_packages if package['name'] == 'antigravity-tools']
    assert rust_lock_versions == [version], 'Rust lockfile version must match the manifest'
    assert version == rust_version == lock['version'] == lock['packages']['']['version'], 'Web, Rust and lockfile versions must match'
    assert re.fullmatch(VERSION_PATTERN, version), 'Only stable, lee.N and beta.N versions are supported'
    channel = 'beta' if '-beta.' in version else 'stable'
    branch = 'beta' if channel == 'beta' else 'main'
    ancestor = subprocess.run(
        ['git', '-C', str(root), 'merge-base', '--is-ancestor', revision, f'refs/remotes/origin/{branch}'],
        capture_output=True,
    )
    assert ancestor.returncode == 0, f'{channel} source must belong to origin/{branch}'
    tag = 'v' + version
    heading = re.compile(
        rf'^(?:#{{1,6}}[ \t]+{re.escape(tag)}(?:[ \t]+\([^\r\n)]*\))?|'
        rf'[ \t]*[*-][ \t]+\*\*{re.escape(tag)}(?:[ \t]+\([^\r\n)]*\))?\*\*:?)'
        rf'[ \t]*$', re.MULTILINE,
    )
    for name in ['CHANGELOG.md', 'CHANGELOG_EN.md']:
        assert heading.search((root / name).read_text()), f'{name} requires exact heading {tag}'
    return version, channel, branch


def validate_release(root, tag, revision):
    version, channel, branch = validate_source(root, revision)
    assert tag == 'v' + version, 'Release tag must exactly match the manifest version'
    assert git(root, 'rev-parse', f'refs/tags/{tag}^{{commit}}') == revision, 'Tag must resolve to the source revision'
    return version, channel, branch


def metadata(root, env):
    revision = env['GITHUB_SHA']
    ref, ref_type = env['GITHUB_REF_NAME'], env['GITHUB_REF_TYPE']
    if ref_type == 'tag':
        version, channel, branch = validate_release(root, ref, revision)
    else:
        assert ref_type == 'branch' and ref in ('main', 'beta'), 'Only main/beta branches may publish images'
        version, channel, branch = validate_source(root, revision)
        assert ref == branch, 'Branch does not match the version release channel'
    repository = env['GITHUB_REPOSITORY'].lower()
    assert re.fullmatch(r'[a-z0-9_.-]+/[a-z0-9_.-]+', repository)
    return {
        'image': 'ghcr.io/' + repository,
        'base_version': version.split('-')[0],
        'release_version': version,
        'local_image': 'antigravity-lee-ci:' + revision,
        'channel': channel,
        'prerelease': str(channel == 'beta').lower(),
        'make_latest': str(ref_type == 'tag' and channel == 'stable').lower(),
    }


if __name__ == '__main__':
    root = pathlib.Path(__file__).resolve().parents[1]
    values = metadata(root, os.environ)
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        for name, value in values.items():
            output.write(f'{name}={value}\n')

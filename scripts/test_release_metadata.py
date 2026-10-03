"""使用临时 Git 历史验证通道隔离、版本一致性和部署包命名。"""
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

from release_metadata import VERSION_PATTERN, metadata

SOURCE = pathlib.Path(__file__).resolve().parents[1]


class ReleaseChannelTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='antigravity-release-test-')
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)

    def git(self, *args):
        return subprocess.check_output(
            ['git', '-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgsign=false', '-C', str(self.root), *args],
            text=True, stderr=subprocess.PIPE,
        ).strip()

    def fixture(self, version, source_branch):
        self.git('init', '-q', '-b', 'fixture')
        self.git('config', 'user.name', 'Release Test')
        self.git('config', 'user.email', 'release-test@example.invalid')
        self.git('commit', '--allow-empty', '-qm', 'base')
        base = self.git('rev-parse', 'HEAD')
        (self.root / 'package.json').write_text(json.dumps({'version': version}))
        (self.root / 'package-lock.json').write_text(json.dumps({'version': version, 'packages': {'': {'version': version}}}))
        (self.root / 'src-tauri').mkdir()
        (self.root / 'src-tauri/Cargo.toml').write_text(f'[package]\nversion = "{version}"\n')
        (self.root / 'src-tauri/Cargo.lock').write_text(f'[[package]]\nname = "antigravity-tools"\nversion = "{version}"\n')
        (self.root / 'CHANGELOG.md').write_text(f'## v{version}\n')
        (self.root / 'CHANGELOG_EN.md').write_text(f'    *   **v{version} (2026-10-04)**:\n')
        (self.root / 'scripts').mkdir()
        for name in ['release_metadata.py', 'build_release_assets.py']:
            shutil.copyfile(SOURCE / 'scripts' / name, self.root / 'scripts' / name)
        (self.root / 'docker').mkdir()
        for name in ['docker-compose.release.yml', '.env.example']:
            shutil.copyfile(SOURCE / 'docker' / name, self.root / 'docker' / name)
        self.git('add', '.')
        self.git('commit', '-qm', 'release fixture')
        revision = self.git('rev-parse', 'HEAD')
        for branch in ['main', 'beta']:
            self.git('update-ref', f'refs/remotes/origin/{branch}', revision if branch == source_branch else base)
        tag = 'v' + version
        self.git('tag', tag)
        return dict(GITHUB_SHA=revision, GITHUB_REF_NAME=tag, GITHUB_REF_TYPE='tag', GITHUB_REPOSITORY='Echo7659/Antigravity-Manager-Lee')

    def test_stable_tag_on_main_is_latest(self):
        env = self.fixture('4.9.1', 'main')
        result = metadata(self.root, env)
        self.assertEqual((result['prerelease'], result['make_latest']), ('false', 'true'))

    def test_beta_tag_on_beta_is_never_latest(self):
        env = self.fixture('4.9.2-beta.0', 'beta')
        result = metadata(self.root, env)
        self.assertEqual((result['prerelease'], result['make_latest']), ('true', 'false'))

    def test_lee_tag_is_stable_and_preserves_upstream_base(self):
        result = metadata(self.root, self.fixture('4.9.1-lee.1', 'main'))
        self.assertEqual((result['base_version'], result['release_version']), ('4.9.1', '4.9.1-lee.1'))
        self.assertEqual((result['channel'], result['prerelease'], result['make_latest']), ('stable', 'false', 'true'))

    def test_lee_beta_tag_is_never_latest(self):
        result = metadata(self.root, self.fixture('4.9.1-lee.1-beta.2', 'beta'))
        self.assertEqual((result['base_version'], result['release_version']), ('4.9.1', '4.9.1-lee.1-beta.2'))
        self.assertEqual((result['channel'], result['prerelease'], result['make_latest']), ('beta', 'true', 'false'))

    def test_lee_stable_cannot_publish_from_beta(self):
        with self.assertRaisesRegex(AssertionError, 'origin/main'):
            metadata(self.root, self.fixture('4.9.1-lee.1', 'beta'))

    def test_lee_beta_cannot_publish_from_main(self):
        with self.assertRaisesRegex(AssertionError, 'origin/beta'):
            metadata(self.root, self.fixture('4.9.1-lee.1-beta.2', 'main'))

    def test_unsupported_release_suffix_is_rejected(self):
        with self.assertRaisesRegex(AssertionError, 'versions are supported'):
            metadata(self.root, self.fixture('4.9.1-cleaned', 'main'))

    def test_release_grammar_rejects_incomplete_and_noncanonical_suffixes(self):
        for version in ['4.9.1-rc.1', '4.9.1-lee', '4.9.1-beta', '4.9.1-lee.01',
                        '4.9.1-lee.1-beta.01', '4.9.1-beta.1-lee.1', '04.9.1', '4.9.1+local']:
            with self.subTest(version=version):
                self.assertIsNone(re.fullmatch(VERSION_PATTERN, version))

    def test_stable_source_only_on_beta_is_rejected(self):
        with self.assertRaisesRegex(AssertionError, 'origin/main'):
            metadata(self.root, self.fixture('4.9.1', 'beta'))

    def test_beta_source_only_on_main_is_rejected(self):
        with self.assertRaisesRegex(AssertionError, 'origin/beta'):
            metadata(self.root, self.fixture('4.9.2-beta.0', 'main'))

    def test_branch_build_never_updates_latest(self):
        env = self.fixture('4.9.1', 'main')
        env.update(GITHUB_REF_TYPE='branch', GITHUB_REF_NAME='main')
        self.assertEqual(metadata(self.root, env)['make_latest'], 'false')
        env['GITHUB_REF_NAME'] = 'beta'
        with self.assertRaisesRegex(AssertionError, 'Branch does not match'):
            metadata(self.root, env)

    def test_tag_and_changelog_must_match_exactly(self):
        env = self.fixture('4.9.2-beta.1', 'beta')
        env['GITHUB_REF_NAME'] = 'v4.9.2-beta.2'
        with self.assertRaisesRegex(AssertionError, 'exactly match'):
            metadata(self.root, env)
        env['GITHUB_REF_NAME'] = 'v4.9.2-beta.1'
        (self.root / 'CHANGELOG_EN.md').write_text('## v4.9.2-beta.10\n')
        with self.assertRaisesRegex(AssertionError, 'exact heading'):
            metadata(self.root, env)

    def test_manifest_mismatch_is_rejected(self):
        env = self.fixture('4.9.1', 'main')
        (self.root / 'src-tauri/Cargo.toml').write_text('[package]\nversion = "4.9.0"\n')
        with self.assertRaisesRegex(AssertionError, 'versions must match'):
            metadata(self.root, env)

    def test_tag_must_point_at_checked_out_revision(self):
        env = self.fixture('4.9.1', 'main')
        self.git('tag', '-f', env['GITHUB_REF_NAME'], 'HEAD^')
        with self.assertRaisesRegex(AssertionError, 'Tag must resolve'):
            metadata(self.root, env)

    def test_rust_lockfile_mismatch_is_rejected(self):
        env = self.fixture('4.9.1', 'main')
        (self.root / 'src-tauri/Cargo.lock').write_text('[[package]]\nname = "antigravity-tools"\nversion = "4.9.0"\n')
        with self.assertRaisesRegex(AssertionError, 'Rust lockfile version'):
            metadata(self.root, env)

    def assert_bundle(self, version, branch):
        env = self.fixture(version, branch)
        output = self.root / 'release'
        digest = 'sha256:' + 'a' * 64
        subprocess.run(
            [sys.executable, str(self.root / 'scripts/build_release_assets.py')],
            env=dict(os.environ, **env, RELEASE_TAG=env['GITHUB_REF_NAME'], RELEASE_DIR=str(output),
                     IMAGE_NAME='ghcr.io/echo7659/antigravity-manager-lee', IMAGE_DIGEST=digest),
            check=True, capture_output=True, text=True,
        )
        manifest = json.loads((output / 'image-manifest.json').read_text())
        self.assertEqual(manifest['release'], 'v' + version)
        self.assertEqual(manifest['base_version'], version.split('-')[0])
        self.assertEqual(manifest['release_version'], version)
        self.assertEqual(manifest['channel'], 'beta' if branch == 'beta' else 'stable')
        self.assertTrue(manifest['image'].endswith('@' + digest))
        archive = output / f'antigravity-manager-lee-v{version}-deployment.tar.gz'
        self.assertEqual((output / 'SHA256SUMS').read_text().split()[0], hashlib.sha256(archive.read_bytes()).hexdigest())

    def test_beta_bundle_preserves_tag_digest_and_checksum(self):
        self.assert_bundle('4.9.2-beta.0', 'beta')

    def test_stable_bundle_preserves_tag_digest_and_checksum(self):
        self.assert_bundle('4.9.1', 'main')

    def test_lee_bundle_preserves_tag_digest_and_checksum(self):
        self.assert_bundle('4.9.1-lee.1', 'main')

    def test_lee_beta_bundle_preserves_tag_digest_and_checksum(self):
        self.assert_bundle('4.9.1-lee.1-beta.2', 'beta')


if __name__ == '__main__':
    unittest.main()

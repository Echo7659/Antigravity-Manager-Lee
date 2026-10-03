import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';

function fixture(t, version = '4.9.1') {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'antigravity-bump-test-'));
    t.after(() => fs.rmSync(root, { recursive: true, force: true }));
    const files = {
        'package.json': JSON.stringify({ name: 'fixture', version }, null, 2),
        'package-lock.json': JSON.stringify({ version, packages: { '': { name: 'fixture', version } } }, null, 2),
        'src-tauri/Cargo.toml': `[package]\nname = "antigravity-tools"\nversion = "${version}"\n`,
        'src-tauri/Cargo.lock': `[[package]]\nname = "antigravity-tools"\nversion = "${version}"\n`,
        'src/pages/Settings.tsx': `const appVersion = '${version}';\n`,
        'CHANGELOG.md': `## v${version}\n`,
        'CHANGELOG_EN.md': `## v${version}\n`,
        'README.md': '# Lee (v4.9.1)\n',
        'README_EN.md': '# Lee (v4.9.1)\n',
        'README_ZH.md': '# Lee (v4.9.1)\n',
    };
    for (const [name, content] of Object.entries(files)) {
        fs.mkdirSync(path.dirname(path.join(root, name)), { recursive: true });
        fs.writeFileSync(path.join(root, name), content);
    }
    fs.mkdirSync(path.join(root, 'scripts'));
    fs.copyFileSync(new URL('./bump-version.mjs', import.meta.url), path.join(root, 'scripts/bump-version.mjs'));
    return { root, read: name => fs.readFileSync(path.join(root, name), 'utf8'),
        run: (...args) => spawnSync(process.execPath, ['scripts/bump-version.mjs', ...args], { cwd: root, encoding: 'utf8' }) };
}

test('Lee stable synchronizes all manifests and README without desktop config', t => {
    const f = fixture(t);
    assert.equal(f.run('4.9.1-lee.1').status, 0);
    assert.equal(f.run('--check', '4.9.1-lee.1').status, 0);
    assert.equal(f.run('--check', '4.9.1').status, 0);
    for (const name of ['README.md', 'README_EN.md', 'README_ZH.md', 'CHANGELOG.md', 'CHANGELOG_EN.md']) {
        assert.match(f.read(name), /v4\.9\.1-lee\.1/);
    }
});

test('Lee beta keeps stable README and beta increment preserves Lee revision', t => {
    const f = fixture(t, '4.9.1-lee.1');
    assert.equal(f.run('4.9.1-lee.1-beta.1').status, 0);
    assert.equal(f.read('README.md'), '# Lee (v4.9.1)\n');
    assert.equal(f.run('beta').status, 0);
    assert.equal(JSON.parse(f.read('package.json')).version, '4.9.1-lee.1-beta.2');
    assert.equal(f.run('patch').status, 0);
    assert.equal(JSON.parse(f.read('package.json')).version, '4.9.1-lee.1');
});

test('check and dry run never write; unsupported or backwards versions fail', t => {
    const f = fixture(t, '4.9.1-lee.2');
    const original = f.read('package.json');
    assert.equal(f.run('--check').status, 0);
    assert.equal(f.run('4.9.1-lee.3', '--dry-run').status, 0);
    for (const version of ['4.9.1-cleaned', '4.9.1-rc.1', '4.9.1-beta', '4.9.1-lee.01', '4.9.1-lee.1', '4.9.0', '4.9.1']) {
        assert.notEqual(f.run(version).status, 0, version);
    }
    assert.equal(f.read('package.json'), original);
});

test('missing or mismatched mandatory fields fail before any writes', t => {
    const f = fixture(t);
    const original = f.read('package.json');
    fs.writeFileSync(path.join(f.root, 'src/pages/Settings.tsx'), "const appVersion = '4.8.1';\n");
    assert.notEqual(f.run('--check').status, 0);
    assert.notEqual(f.run('4.9.1-lee.1').status, 0);
    assert.equal(f.read('package.json'), original);
    fs.unlinkSync(path.join(f.root, 'package-lock.json'));
    assert.notEqual(f.run('4.9.1-lee.1').status, 0);
    assert.equal(f.read('package.json'), original);
});

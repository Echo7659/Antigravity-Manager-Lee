#!/usr/bin/env node

/** 同步服务端发布版本；所有必需字段通过校验后才写入文件。 */
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const number = '(0|[1-9]\\d*)';
const pattern = new RegExp(`^v?${number}\\.${number}\\.${number}(?:-lee\\.${number})?(?:-beta\\.${number})?$`);
const args = process.argv.slice(2);
const check = args.includes('--check');
const dryRun = args.includes('--dry-run') || process.env.npm_config_dry_run === 'true';
const autoCommit = args.includes('--commit') || process.env.npm_config_commit === 'true';
const targets = args.filter(arg => !arg.startsWith('--'));
const read = name => fs.readFileSync(path.join(root, name), 'utf8');

function parse(version) {
    const match = typeof version === 'string' && version.match(pattern);
    if (!match) throw new Error(`不支持版本 ${version}；只允许 X.Y.Z[-lee.N][-beta.N]，数字不得有前导零。`);
    return { major: Number(match[1]), minor: Number(match[2]), patch: Number(match[3]),
        lee: match[4] === undefined ? null : Number(match[4]),
        beta: match[5] === undefined ? null : Number(match[5]) };
}

const baseVersion = v => `${v.major}.${v.minor}.${v.patch}`;
const format = v => baseVersion(v) + (v.lee === null ? '' : `-lee.${v.lee}`) + (v.beta === null ? '' : `-beta.${v.beta}`);

function field(content, expression, name) {
    const match = content.match(expression);
    if (!match) throw new Error(`${name} 缺少必需版本字段。`);
    return match[2];
}

function main() {
    if (targets.length > 1 || args.some(a => a.startsWith('--') && !['--check', '--dry-run', '--commit', '--help'].includes(a))) {
        throw new Error('参数错误；使用 --help 查看用法。');
    }
    if (check && (dryRun || autoCommit)) throw new Error('--check 不能与写入选项组合。');
    const current = JSON.parse(read('package.json')).version;
    const cur = parse(current);
    if ((!targets.length && !check) || args.includes('--help')) {
        console.log(`当前版本: ${current}
用法: npm run bump <patch|minor|major|beta|X.Y.Z[-lee.N][-beta.N]> [-- --dry-run|--commit]
      node scripts/bump-version.mjs --check [发布版本或上游基础版本]
main 正式版: X.Y.Z 或 X.Y.Z-lee.N；更新 README，正式标签可成为 latest。
beta 预览版: X.Y.Z-beta.N 或 X.Y.Z-lee.N-beta.N；不更新 README 或 latest。
patch 将 beta 转为同号正式版，否则递增基础补丁；beta 递增已有预览序号，否则开启下一补丁预览。
指定 Lee 版本示例: npm run bump 4.9.1-lee.1
--check 只读核对 Web、Rust、lockfile 和设置页版本。脚本不发布或推送。`);
        return;
    }
    const lock = JSON.parse(read('package-lock.json'));
    const manifestPattern = /(\[package\][\s\S]*?^version\s*=\s*)"([^"]+)"/m;
    const lockPattern = /(\[\[package\]\]\r?\nname = "antigravity-tools"\r?\nversion = )"([^"]+)"/;
    const settingsPattern = /(const appVersion = )'([^']+)'/;
    const cargo = read('src-tauri/Cargo.toml');
    const cargoLock = read('src-tauri/Cargo.lock');
    const settings = read('src/pages/Settings.tsx');
    const versions = [lock.version, lock.packages?.['']?.version,
        field(cargo, manifestPattern, 'Cargo.toml'), field(cargoLock, lockPattern, 'Cargo.lock'),
        field(settings, settingsPattern, 'Settings.tsx')];
    if (versions.some(version => version !== current)) throw new Error('Web、Rust、lockfile 或设置页版本不一致；未写入任何文件。');
    if (check) {
        const expected = targets[0]?.replace(/^v/, '');
        if (expected && expected !== current && expected !== baseVersion(cur)) throw new Error(`目标 ${expected} 与发布版本 ${current} 或基础版本 ${baseVersion(cur)} 不符。`);
        console.log(`版本一致: release=${current}, base=${baseVersion(cur)}, channel=${cur.beta === null ? 'stable' : 'beta'}`);
        return;
    }
    const target = targets[0];
    let next;
    if (target === 'patch') next = cur.beta !== null ? { ...cur, beta: null } : { ...cur, patch: cur.patch + 1, lee: null, beta: null };
    else if (target === 'minor') next = { ...cur, minor: cur.minor + 1, patch: 0, lee: null, beta: null };
    else if (target === 'major') next = { ...cur, major: cur.major + 1, minor: 0, patch: 0, lee: null, beta: null };
    else if (target === 'beta') next = cur.beta !== null ? { ...cur, beta: cur.beta + 1 } : { ...cur, patch: cur.patch + 1, lee: null, beta: 1 };
    else next = parse(target);
    const version = format(next);
    let comparison = 0;
    for (const key of ['major', 'minor', 'patch', 'lee']) {
        comparison = (next[key] ?? -1) - (cur[key] ?? -1);
        if (comparison) break;
    }
    if (comparison < 0 || (comparison === 0 && (version === current ||
        (cur.beta !== null && next.beta !== null && next.beta <= cur.beta)))) {
        throw new Error(`版本必须前进，不能从 ${current} 改为 ${version}。`);
    }
    const changes = new Map();
    const queue = (name, updated) => changes.set(name, { original: read(name), updated });
    const pkg = JSON.parse(read('package.json'));
    pkg.version = version;
    lock.version = version;
    lock.packages[''].version = version;
    queue('package.json', JSON.stringify(pkg, null, 2) + '\n');
    queue('package-lock.json', JSON.stringify(lock, null, 2) + '\n');
    queue('src-tauri/Cargo.toml', cargo.replace(manifestPattern, `$1"${version}"`));
    queue('src-tauri/Cargo.lock', cargoLock.replace(lockPattern, `$1"${version}"`));
    queue('src/pages/Settings.tsx', settings.replace(settingsPattern, `$1'${version}'`));
    if (next.beta === null) {
        for (const name of ['README.md', 'README_EN.md', 'README_ZH.md']) {
            const content = read(name);
            if (!/^# .+\(v[^)]+\)/m.test(content)) throw new Error(`${name} 缺少版本标题。`);
            queue(name, content.replace(/^(# .+)\(v[^)]+\)/m, `$1(v${version})`)
                .replace(/Version-[0-9][^"\s]*-blue/, `Version-${version}-blue`));
        }
    }
    for (const name of ['CHANGELOG.md', 'CHANGELOG_EN.md']) {
        const content = read(name);
        const escaped = version.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
        const heading = new RegExp(`^(?:#{1,6}\\s+v${escaped}(?:\\s+\\([^\\r\\n)]*\\))?|\\s*[*-]\\s+\\*\\*v${escaped}(?:\\s+\\([^\\r\\n)]*\\))?\\*\\*:?)\\s*$`, 'm');
        queue(name, heading.test(content) ? content : `## v${version}\n\n${name === 'CHANGELOG.md' ? '待补充变更、验证范围与贡献者归属。' : 'Document changes, validation scope and contributor attribution.'}\n\n${content}`);
    }
    console.log(`${dryRun ? '[DRY-RUN] ' : ''}${current} -> ${version}; channel=${next.beta === null ? 'stable (main)' : 'beta'}`);
    if (dryRun) return;
    const written = [];
    try {
        for (const [name, change] of changes) {
            written.push(name);
            fs.writeFileSync(path.join(root, name), change.updated);
        }
    } catch (error) {
        for (const name of written) fs.writeFileSync(path.join(root, name), changes.get(name).original);
        throw error;
    }
    if (autoCommit) {
        execFileSync('git', ['add', '--', ...changes.keys()], { cwd: root, stdio: 'inherit' });
        execFileSync('git', ['commit', '-m', `chore(release): bump version to ${version}`], { cwd: root, stdio: 'inherit' });
    }
    console.log('版本同步完成；发布前需补齐双语 changelog、正式版 README 摘要并执行门禁。');
}

try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; }

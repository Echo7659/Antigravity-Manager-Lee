import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { request } from '../../src/utils/request';
import { downloadJson, readJsonFile } from '../../src/utils/browserFiles';

async function sourceFiles(path: string): Promise<string[]> {
    const entries = await readdir(path, { withFileTypes: true });
    return (await Promise.all(entries.map(entry => entry.isDirectory()
        ? sourceFiles(`${path}/${entry.name}`)
        : Promise.resolve(/\.tsx?$/.test(entry.name) ? [`${path}/${entry.name}`] : [])))).flat();
}
for (const path of await sourceFiles('src')) {
    const source = await readFile(path, 'utf8');
    assert.doesNotMatch(source, /@tauri-apps|isTauri\(/, path);
}
await assert.rejects(request('unknown_command'), /Unsupported command: unknown_command/);
assert.deepEqual(await readJsonFile(new File(['{"accounts":[]}'], 'accounts.json')), { accounts: [] });
await assert.rejects(readJsonFile(new File(['invalid'], 'accounts.json')), SyntaxError);

const requestSource = await readFile('src/utils/request.ts', 'utf8');
const serverSource = await readFile('src-tauri/src/proxy/server.rs', 'utf8');
const routes = new Set([...serverSource.matchAll(/\.route\(\s*"([^"]+)"/g)].map(match => match[1]));
for (const [, route] of requestSource.matchAll(/url: '([^']+)'/g)) {
    assert.ok(routes.has(route.replace(/^\/api/, '').split('?')[0]), `Missing server route: ${route}`);
}
for (const removed of ['import_from_db', 'import_custom_db', 'sync_account_from_db', 'open_data_folder', 'restore_original_device', 'cloudflared_start', 'execute_cli_sync', 'get_canonical_families']) {
    await assert.rejects(request(removed), new RegExp(`Unsupported command: ${removed}`));
}

const originals = {
    fetch: globalThis.fetch,
    window: Object.getOwnPropertyDescriptor(globalThis, 'window'),
    sessionStorage: Object.getOwnPropertyDescriptor(globalThis, 'sessionStorage'),
    document: Object.getOwnPropertyDescriptor(globalThis, 'document'),
    createObjectURL: URL.createObjectURL,
    revokeObjectURL: URL.revokeObjectURL,
};
const events = new EventTarget();
Object.defineProperty(globalThis, 'window', { value: events, configurable: true });
Object.defineProperty(globalThis, 'sessionStorage', { value: { getItem: () => 'fixture-password' }, configurable: true });
let refreshes = 0;
events.addEventListener('proxy-models-updated', () => { refreshes++; });
try {
    globalThis.fetch = async (input, init) => {
        assert.equal(input, '/api/accounts/id%2Fone/quota?filter=one');
        assert.equal((init?.headers as Record<string, string>).Authorization, 'Bearer fixture-password');
        assert.equal((init?.headers as Record<string, string>)['x-api-key'], 'fixture-password');
        return Response.json({ percentage: 90 });
    };
    assert.deepEqual(await request('refresh_account_quota', { accountId: 'id/one', filter: 'one' }), { percentage: 90 });
    globalThis.fetch = async (input, init) => {
        assert.equal(input, '/api/user-tokens/token%2F1');
        assert.equal(init?.method, 'PATCH');
        assert.deepEqual(JSON.parse(String(init?.body)), { enabled: true });
        return new Response(null, { status: 204 });
    };
    assert.equal(await request('update_user_token', { id: 'token/1', request: { enabled: true } }), null);
    globalThis.fetch = async () => Response.json({ error: 'save rejected' }, { status: 500 });
    await assert.rejects(request('save_config', { config: {} }));
    assert.equal(refreshes, 0);
    globalThis.fetch = async () => new Response(null, { status: 204 });
    await request('save_config', { config: {} });
    assert.equal(refreshes, 1);

    let blob: Blob | undefined;
    let revoked = false;
    let clicked = false;
    let removed = false;
    const anchor = { href: '', download: '', click: () => { clicked = true; }, remove: () => { removed = true; } };
    URL.createObjectURL = value => { blob = value as Blob; return 'blob:fixture'; };
    URL.revokeObjectURL = url => { assert.equal(url, 'blob:fixture'); revoked = true; };
    Object.defineProperty(globalThis, 'document', {
        value: { createElement: () => anchor, body: { appendChild: () => undefined } }, configurable: true,
    });
    downloadJson('accounts.json', [{ email: 'fixture@example.test' }]);
    assert.equal(anchor.download, 'accounts.json');
    assert.equal(anchor.href, 'blob:fixture');
    assert.ok(clicked && removed);
    assert.equal(blob?.type, 'application/json');
    assert.deepEqual(JSON.parse(await blob!.text()), [{ email: 'fixture@example.test' }]);
    await new Promise(resolve => setTimeout(resolve, 0));
    assert.ok(revoked);
} finally {
    globalThis.fetch = originals.fetch;
    URL.createObjectURL = originals.createObjectURL;
    URL.revokeObjectURL = originals.revokeObjectURL;
    for (const key of ['window', 'sessionStorage', 'document'] as const) {
        const descriptor = originals[key];
        if (descriptor) Object.defineProperty(globalThis, key, descriptor);
        else Reflect.deleteProperty(globalThis, key);
    }
}
console.log('HTTP-only browser tests passed');

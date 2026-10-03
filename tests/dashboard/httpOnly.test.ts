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
for (const path of ['src/App.tsx', 'src/components/navbar/Navbar.tsx', 'src/pages/Settings.tsx']) {
    const source = await readFile(path, 'utf8');
    assert.doesNotMatch(source, /apikey-fun|nav\.apikey_fun|ApiKeyFun/, `Removed promotion entry: ${path}`);
}
for (const file of (await readdir('src/locales')).filter(file => file.endsWith('.json'))) {
    const locale = JSON.parse(await readFile(`src/locales/${file}`, 'utf8'));
    assert.equal(Object.hasOwn(locale, 'apiKeyFun'), false, `${file}: removed promotion namespace`);
    assert.equal(Object.hasOwn(locale.nav ?? {}, 'apikey_fun'), false, `${file}: removed promotion menu label`);
}
assert.equal((await sourceFiles('src')).some(path => path.endsWith('/ApiKeyFun.tsx')), false);
assert.match(await readFile('src/App.tsx', 'utf8'), /path: 'api-proxy'/);
assert.match(await readFile('src/pages/Settings.tsx', 'utf8'), /<ProxyPoolSettings\b/);
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

// Independent wire contracts from the Axum routes and request DTOs in server.rs/user_token.rs.
const httpContracts: Array<{
    command: string;
    method: 'GET' | 'DELETE' | 'POST' | 'PATCH';
    url: string;
    args?: Record<string, unknown>;
    body?: unknown;
}> = [
    { command: 'list_accounts', method: 'GET', url: '/api/accounts' },
    { command: 'get_proxy_models', method: 'GET', url: '/api/proxy/models' },
    { command: 'get_proxy_pool_config', method: 'GET', url: '/api/proxy/pool/config' },
    { command: 'get_token_stats_summary', method: 'GET', url: '/api/stats/token/summary?hours=24', args: { hours: 24 } },
    { command: 'refresh_account_quota', method: 'GET', url: '/api/accounts/id%2Fone/quota?filter=one', args: { accountId: 'id/one', filter: 'one' } },
    { command: 'get_proxy_logs_filtered', method: 'GET', url: '/api/logs?filter=Claude+%26+Gemini&errorsOnly=true&limit=20&offset=0', args: { filter: 'Claude & Gemini', errorsOnly: true, limit: 20, offset: 0 } },
    { command: 'delete_device_version', method: 'DELETE', url: '/api/accounts/id%2Fone/device-versions/v%232', args: { accountId: 'id/one', versionId: 'v#2' } },
    { command: 'remove_ip_from_blacklist', method: 'DELETE', url: '/api/security/blacklist?ipPattern=10.0.0.0%2F8', args: { ipPattern: '10.0.0.0/8' } },
    { command: 'refresh_all_quotas', method: 'POST', url: '/api/accounts/refresh?scope=all' },
    { command: 'add_account', method: 'POST', url: '/api/accounts', args: { refreshToken: 'fixture-token' }, body: { refreshToken: 'fixture-token' } },
    { command: 'export_accounts', method: 'POST', url: '/api/accounts/export', args: { request: { accountIds: ['account-1'] } }, body: { accountIds: ['account-1'] } },
    { command: 'bind_account_proxy', method: 'POST', url: '/api/proxy/pool/bind', args: { request: { accountId: 'account-1', proxyId: 'proxy-2' } }, body: { accountId: 'account-1', proxyId: 'proxy-2' } },
    { command: 'renew_user_token', method: 'POST', url: '/api/user-tokens/token%2F1/renew', args: { id: 'token/1', request: { expiresType: 'month' } }, body: { expiresType: 'month' } },
    { command: 'create_user_token', method: 'POST', url: '/api/user-tokens', args: { request: { username: 'fixture', expires_type: 'month', max_ips: 2 } }, body: { username: 'fixture', expires_type: 'month', max_ips: 2 } },
    { command: 'save_config', method: 'POST', url: '/api/config', args: { config: { language: 'en', proxy: { only_raw_quota_models: true } } }, body: { config: { language: 'en', proxy: { only_raw_quota_models: true } } } },
    { command: 'update_user_token', method: 'PATCH', url: '/api/user-tokens/token%2F1', args: { id: 'token/1', request: { enabled: false, max_ips: 3 } }, body: { enabled: false, max_ips: 3 } },
];

let refreshes = 0;
events.addEventListener('proxy-models-updated', () => { refreshes++; });
try {
    for (const contract of httpContracts) {
        globalThis.fetch = async (input, init) => {
            assert.equal(input, contract.url, `${contract.command}: URL`);
            assert.equal(init?.method, contract.method, `${contract.command}: HTTP method`);
            assert.deepEqual(init?.body === undefined ? undefined : JSON.parse(String(init.body)), contract.body, `${contract.command}: JSON fields`);
            assert.equal((init?.headers as Record<string, string>).Authorization, 'Bearer fixture-password');
            assert.equal((init?.headers as Record<string, string>)['x-api-key'], 'fixture-password');
            return new Response(null, { status: 204 });
        };
        assert.equal(await request(contract.command, contract.args), null);
    }
    refreshes = 0;
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

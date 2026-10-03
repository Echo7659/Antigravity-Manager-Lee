import assert from 'node:assert/strict';
import { buildProxyModels } from '../../src/utils/proxyModels';
import { request } from '../../src/utils/request';
import { readFile } from 'node:fs/promises';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';

const models = buildProxyModels(['claude-opus-5-5', 'gemini-3.8-flash', 'claude-opus-5-5']);
assert.deepEqual(models.map(model => model.id), ['gemini-3.8-flash', 'claude-opus-5-5']);
assert.equal(models.some(model => model.id === 'gpt-4o'), false);
assert.equal(models.some(model => model.id === 'claude-sonnet-4-6'), false);

const future = buildProxyModels(['future-model-9'])[0];
assert.equal(future.id, 'future-model-9');
assert.equal(future.name, 'future-model-9');
assert.equal(future.desc, 'future-model-9');
assert.equal(future.group, 'Other');
assert.ok(future.icon);

const originalFetch = globalThis.fetch;
globalThis.fetch = async (input, init) => {
    assert.equal(input, '/api/proxy/models');
    assert.equal(init?.method, 'GET');
    return new Response(JSON.stringify(['future-model-9']), {
        headers: { 'Content-Type': 'application/json' },
    });
};
try {
    assert.deepEqual(await request<string[]>('get_proxy_models'), ['future-model-9']);
} finally {
    globalThis.fetch = originalFetch;
}

const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'window');
const storageDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'sessionStorage');
const events = new EventTarget();
Object.defineProperty(globalThis, 'window', { value: events, configurable: true });
Object.defineProperty(globalThis, 'sessionStorage', {
    value: { getItem: () => null },
    configurable: true,
});
let refreshes = 0;
events.addEventListener('proxy-models-updated', () => { refreshes += 1; });
globalThis.fetch = async input => {
    assert.equal(input, '/api/config');
    return new Response('{}', { headers: { 'Content-Type': 'application/json' } });
};
try {
    await request('save_config', { config: { proxy: { custom_mapping: { 'my-model': 'gemini-3.8-flash' } } } });
    await request('save_config', { config: { proxy: { only_raw_quota_models: true } } });
    assert.equal(refreshes, 2);
} finally {
    globalThis.fetch = originalFetch;
    if (windowDescriptor) Object.defineProperty(globalThis, 'window', windowDescriptor);
    else Reflect.deleteProperty(globalThis, 'window');
    if (storageDescriptor) Object.defineProperty(globalThis, 'sessionStorage', storageDescriptor);
    else Reflect.deleteProperty(globalThis, 'sessionStorage');
}

console.log('proxy model catalog tests passed');

// Effect harness checks the hook's HTTP and event lifecycle without a browser renderer.
const state: unknown[] = [];
const effects: Array<{ deps: unknown[]; cleanup?: () => void }> = [];
let stateIndex = 0;
let effectIndex = 0;
let pendingEffects: Array<() => void> = [];
let needsRender = false;
let catalogReads = 0;
let catalogIds = ['initial-model'];
const hookEvents = new EventTarget();
const accountState = { accounts: [{ id: 'fixture' }], fetchAccounts: () => assert.fail('Unexpected account load') };
const reactHarness = {
    useState(initial: unknown) {
        const index = stateIndex++;
        if (!(index in state)) state[index] = initial;
        return [state[index], (next: unknown) => {
            state[index] = typeof next === 'function' ? next(state[index]) : next;
            needsRender = true;
        }];
    },
    useEffect(effect: () => void | (() => void), deps: unknown[]) {
        const index = effectIndex++;
        if (!effects[index] || deps.some((value, i) => !Object.is(value, effects[index].deps[i]))) {
            pendingEffects.push(() => {
                effects[index]?.cleanup?.();
                effects[index] = { deps, cleanup: effect() || undefined };
            });
        }
    },
    useMemo(factory: () => unknown) { return factory(); },
};
const hookSource = await readFile('src/hooks/useProxyModels.tsx', 'utf8');
const compiled = ts.transpileModule(hookSource, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText;
const hookModule: { useProxyModels?: () => { models: ReturnType<typeof buildProxyModels> } } = {};
runInNewContext(compiled, {
    exports: hookModule, window: hookEvents, console,
    require(id: string) {
        if (id === 'react') return reactHarness;
        if (id.endsWith('/request')) return { request: async (command: string) => {
            assert.equal(command, 'get_proxy_models');
            catalogReads++;
            return catalogIds;
        } };
        if (id.endsWith('/proxyModels')) return { buildProxyModels };
        if (id.endsWith('/useAccountStore')) return { useAccountStore: (selector: (store: typeof accountState) => unknown) => selector(accountState) };
        throw new Error(`Unexpected import: ${id}`);
    },
});
let hookModels: ReturnType<typeof buildProxyModels> = [];
async function renderHook() {
    do {
        needsRender = false;
        stateIndex = 0;
        effectIndex = 0;
        hookModels = hookModule.useProxyModels!().models;
        const run = pendingEffects;
        pendingEffects = [];
        run.forEach(effect => effect());
        await Promise.resolve();
    } while (needsRender);
}
await renderHook();
assert.equal(catalogReads, 1);
assert.deepEqual(hookModels.map(model => model.id), ['initial-model']);
catalogIds = ['future-model-9'];
hookEvents.dispatchEvent(new Event('proxy-models-updated'));
await renderHook();
assert.equal(catalogReads, 2);
assert.deepEqual(hookModels.map(model => model.id), ['future-model-9']);
await renderHook();
assert.equal(catalogReads, 2);
effects.forEach(effect => effect.cleanup?.());
needsRender = false;
hookEvents.dispatchEvent(new Event('proxy-models-updated'));
assert.equal(needsRender, false);
console.log('proxy model hook refetch and cleanup tests passed');

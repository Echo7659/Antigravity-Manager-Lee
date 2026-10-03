import assert from 'node:assert/strict';
import { buildProxyModels } from '../../src/utils/proxyModels';
import { request } from '../../src/utils/request';

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

console.log('proxy model catalog tests passed');

import assert from 'node:assert/strict';
import { useAccountStore as store } from '../../src/stores/useAccountStore';
import { matchesAccountStatus } from '../../src/utils/accountStatus';
import type { Account } from '../../src/types/account';

const active = { id: 'active', email: 'active@example.test', last_used: 1, quota: { models: [], last_updated: 1 } } as Account;
const forbidden = { ...active, id: 'forbidden', quota: { ...active.quota!, is_forbidden: true } };
const disabled = { ...active, id: 'disabled', disabled: true };
const proxyDisabled = { ...active, id: 'proxy-disabled', proxy_disabled: true };
const verification = { ...active, id: 'verification', validation_blocked: true };
const accounts = [active, forbidden, disabled, proxyDisabled, verification];
assert.deepEqual(accounts.filter(a => matchesAccountStatus(a, 'forbidden')).map(a => a.id), ['forbidden']);
assert.deepEqual(accounts.filter(a => matchesAccountStatus(a, 'disabled')).map(a => a.id), ['disabled', 'proxy-disabled']);
assert.equal(accounts.filter(a => matchesAccountStatus(a, 'unavailable')).length, 3);
assert.equal(accounts.filter(a => matchesAccountStatus(a, 'all')).length, 5);

const originalFetch = globalThis.fetch;
try {
    store.setState({ accounts, currentAccount: active, loading: false });
    let releaseList!: (response: Response) => void;
    let listReads = 0;
    globalThis.fetch = async (url, init) => {
        if (init?.method === 'DELETE') return new Response(null, { status: 204 });
        listReads++;
        return new Promise<Response>(resolve => { releaseList = resolve; });
    };
    const staleList = store.getState().fetchAccounts();
    await store.getState().deleteAccount(forbidden.id);
    assert.equal(listReads, 1, 'deletion must not fetch the full list');
    assert.equal(store.getState().currentAccount?.id, active.id);
    assert.equal(store.getState().accounts.some(a => a.id === forbidden.id), false);
    releaseList(Response.json({ accounts }));
    await staleList;
    assert.equal(store.getState().accounts.some(a => a.id === forbidden.id), false, 'late list response must not restore deleted account');
    globalThis.fetch = async () => new Response(JSON.stringify({ error: 'fixture deletion failure' }), { status: 500 });
    await assert.rejects(store.getState().deleteAccount(disabled.id));
    assert.equal(store.getState().accounts.some(a => a.id === disabled.id), true);
    const requests: string[] = [];
    globalThis.fetch = async url => {
        requests.push(String(url));
        if (String(url).endsWith('/bulk-delete')) return new Response(null, { status: 200 });
        assert.equal(String(url).endsWith('/current'), true);
        return Response.json(verification);
    };
    await store.getState().deleteAccounts([active.id, disabled.id]);
    assert.deepEqual(store.getState().accounts.map(a => a.id), [proxyDisabled.id, verification.id]);
    assert.equal(store.getState().currentAccount?.id, verification.id);
    assert.equal(requests.length, 2, 'only read the replacement current account');
    console.log('Status filters and confirmed deletion, failure preservation, stale reads and current-account fallback passed.');
} finally {
    globalThis.fetch = originalFetch;
}

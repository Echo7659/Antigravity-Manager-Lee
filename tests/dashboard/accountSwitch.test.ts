import assert from 'node:assert/strict';
import { useAccountStore } from '../../src/stores/useAccountStore';
import type { Account } from '../../src/types/account';
const originalFetch = globalThis.fetch;
const old = { id: 'old', email: 'old@example.test', last_used: 1, quota: { last_updated: 1, models: [] } } as Account;
const selected = { ...old, id: 'selected', email: 'selected@example.test' };
const store = useAccountStore;
try {
    store.setState({ currentAccount: old, accounts: [old, selected], loading: false, error: null });
    let releaseOld!: (response: Response) => void;
    let reads = 0;
    globalThis.fetch = async (url) => {
        if (String(url).endsWith('/switch')) return new Response(null, { status: 200 });
        if (++reads === 1) return new Promise<Response>(resolve => { releaseOld = resolve; });
        return Response.json(selected);
    };
    const staleFetch = store.getState().fetchCurrentAccount();
    await store.getState().switchAccount(selected.id);
    releaseOld(Response.json(old));
    await staleFetch;
    assert.equal(store.getState().currentAccount?.id, selected.id);
    const refreshed = { ...selected, quota: { last_updated: 2, models: [{ name: 'gemini-test', percentage: 92, reset_time: '' }] } };
    globalThis.fetch = async () => Response.json({ accounts: [old, refreshed] });
    await store.getState().fetchAccounts();
    assert.equal(store.getState().currentAccount?.quota?.models[0].percentage, 92);
    globalThis.fetch = async url => String(url).endsWith('/switch') ? new Response(null, { status: 200 }) : Response.json(old);
    await assert.rejects(store.getState().switchAccount(selected.id));
    assert.equal(store.getState().loading, false);
    globalThis.fetch = async () => new Response(JSON.stringify({ error: 'simulated switch failure' }), { status: 500 });
    await assert.rejects(store.getState().switchAccount(old.id));
    assert.equal(store.getState().loading, false);
    console.log('Switch confirmation, stale-response isolation, quota synchronization and error propagation passed.');
} finally {
    globalThis.fetch = originalFetch;
}

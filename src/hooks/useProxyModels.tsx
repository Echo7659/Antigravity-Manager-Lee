import { useMemo, useEffect, useState } from 'react';
import { request } from '../utils/request';
import { buildProxyModels } from '../utils/proxyModels';
import { useAccountStore } from '../stores/useAccountStore';

export type { ProxyModel } from '../utils/proxyModels';

export const useProxyModels = () => {
    const accounts = useAccountStore(state => state.accounts);
    const fetchAccounts = useAccountStore(state => state.fetchAccounts);
    const [modelIds, setModelIds] = useState<string[]>([]);
    const [refreshEpoch, setRefreshEpoch] = useState(0);

    useEffect(() => {
        const refresh = () => setRefreshEpoch(epoch => epoch + 1);
        window.addEventListener('proxy-models-updated', refresh);
        return () => window.removeEventListener('proxy-models-updated', refresh);
    }, []);

    useEffect(() => {
        if (accounts.length === 0) fetchAccounts();
    }, []); // eslint-disable-line react-hooks/exhaustive-deps

    useEffect(() => {
        let cancelled = false;
        request<string[]>('get_proxy_models')
            .then(ids => { if (!cancelled) setModelIds(ids); })
            .catch(err => console.error('Failed to fetch proxy models:', err));

        return () => { cancelled = true; };
    }, [accounts, refreshEpoch]);

    const models = useMemo(() => buildProxyModels(modelIds), [modelIds]);
    return { models };
};

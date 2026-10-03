import { useMemo, useEffect, useState } from 'react';
import { request } from '../utils/request';
import { buildProxyModels } from '../utils/proxyModels';
import { useAccountStore } from '../stores/useAccountStore';

export type { ProxyModel } from '../utils/proxyModels';

export interface CanonicalFamilyDto {
    canonical_id: string;
    display_name: string;
    match_ids: string[];
}

export const useProxyModels = () => {
    const accounts = useAccountStore(state => state.accounts);
    const fetchAccounts = useAccountStore(state => state.fetchAccounts);
    const [modelIds, setModelIds] = useState<string[]>([]);
    const [canonicalFamilies, setCanonicalFamilies] = useState<CanonicalFamilyDto[]>([]);

    useEffect(() => {
        if (accounts.length === 0) fetchAccounts();
    }, []); // eslint-disable-line react-hooks/exhaustive-deps

    useEffect(() => {
        let cancelled = false;
        request<string[]>('get_proxy_models')
            .then(ids => { if (!cancelled) setModelIds(ids); })
            .catch(err => console.error('Failed to fetch proxy models:', err));

        return () => { cancelled = true; };
    }, [accounts]);

    useEffect(() => {
        let cancelled = false;
        request<CanonicalFamilyDto[]>('get_canonical_families')
            .then(data => { if (!cancelled && data) setCanonicalFamilies(data); })
            .catch(err => console.error('Failed to fetch canonical families:', err));

        return () => { cancelled = true; };
    }, []);

    const models = useMemo(() => buildProxyModels(modelIds), [modelIds]);
    return { models, canonicalFamilies };
};

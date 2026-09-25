import type { Account } from '../types/account';

export type AccountStatusFilter = 'all' | 'forbidden' | 'disabled' | 'unavailable';

export function matchesAccountStatus(account: Account, status: AccountStatusFilter): boolean {
    const forbidden = Boolean(account.quota?.is_forbidden);
    const disabled = Boolean(account.disabled || account.proxy_disabled);
    switch (status) {
        case 'forbidden': return forbidden;
        case 'disabled': return disabled;
        case 'unavailable': return forbidden || disabled;
        default: return true;
    }
}

import type { Account } from '../types/account';
import type { QuotaProtectionConfig } from '../types/config';
import { findImageQuotaModel, findQuotaModel, getModelProtectionKey } from './modelCategory';
import { getModelQuotaDisplay } from './quotaDisplay';

export type DashboardQuotaCategory = 'gemini' | 'image' | 'claude';

export function isAccountAvailable(account: Account): boolean {
    return !account.disabled && !account.proxy_disabled && !account.validation_blocked && !account.quota?.is_forbidden;
}

export function getAccountCategoryQuota(account: Account, category: DashboardQuotaCategory) {
    const models = account.quota?.models;
    let model = findQuotaModel(models, 'gemini-pro') || findQuotaModel(models, 'gemini-flash');
    if (category === 'image') model = findImageQuotaModel(models);
    if (category === 'claude') model = findQuotaModel(models, 'claude');
    const family = category === 'claude' ? 'claude' : 'gemini';
    return getModelQuotaDisplay(model?.name || family, model, account.quota?.quota_groups);
}

/** 百分比按已知账号等权平均，不推断套餐容量或把缺失配额算成零。 */
export function computeQuotaMetrics(accounts: Account[], category: DashboardQuotaCategory) {
    const values = accounts.map(account => getAccountCategoryQuota(account, category));
    const average = (input: (number | null)[]) => {
        const known = input.filter((value): value is number => value !== null);
        return known.length ? Math.round(known.reduce((sum, value) => sum + value, 0) / known.length) : null;
    };
    return {
        avg5h: average(values.map(value => value.fiveHourPercentage)),
        avgWeekly: average(values.map(value => value.weeklyPercentage)),
        averageEffective: average(values.map(value => value.effectivePercentage)),
        zeroWeeklyCount: values.filter(value => value.weeklyPercentage === 0).length,
    };
}

export function recommendAccount(accounts: Account[], category: 'gemini' | 'claude', currentId?: string, protection?: QuotaProtectionConfig) {
    const now = Date.now() / 1000;
    return accounts.filter(isAccountAvailable).map(account => ({ account, quota: getAccountCategoryQuota(account, category).effectivePercentage }))
        .filter(({ account, quota }) => {
            if (quota === null || quota <= 0) return false;
            const belongs = (name: string) => category === 'claude' ? /^(claude|gpt)/i.test(name) : /^gemini/i.test(name) && !/image/i.test(name);
            if (protection?.enabled && (account.protected_models || []).some(belongs)) return false;
            if (Object.entries(account.live_limited_models || {}).some(([name, limit]) => belongs(name) && limit.until > now)) return false;
            const monitored = protection?.monitored_models.some(name => belongs(name) || (category === 'claude' && getModelProtectionKey(name) === 'claude'));
            return !(protection?.enabled && monitored && quota <= protection.threshold_percentage);
        })
        .sort((a, b) => (b.quota! - a.quota!) || Number(b.account.id === currentId) - Number(a.account.id === currentId) || a.account.id.localeCompare(b.account.id))[0];
}

import type { ModelQuota, QuotaBucket, QuotaGroup } from '../types/account';

/** 配额显示同时保留窗口原值和综合剩余比例；未知窗口不视为零。 */
export function getModelQuotaDisplay(modelId: string, model: ModelQuota | undefined, groups: QuotaGroup[] = [], window: 'effective' | '5h' = 'effective') {
    const name = modelId.toLowerCase();
    const thirdParty = /^(claude|gpt)/.test(name);
    const buckets = (groups || []).filter(group => {
        const groupName = (group?.display_name || '').toLowerCase();
        const isThirdParty = /claude|gpt|3p/.test(groupName)
            || (group?.buckets || []).some(bucket => /3p/.test(bucket?.bucket_id || ''));
        return thirdParty ? isThirdParty : name.startsWith('gemini') && !isThirdParty;
    }).flatMap(group => group?.buckets || [])
        .filter(bucket => bucket && Number.isFinite(bucket.remaining_fraction) && bucket.remaining_fraction >= 0 && bucket.remaining_fraction <= 1);
    const lowest = (values: QuotaBucket[]) => values.reduce<QuotaBucket | undefined>((chosen, bucket) =>
        !chosen || bucket.remaining_fraction < chosen.remaining_fraction ? bucket : chosen, undefined);
    const fiveHour = lowest(buckets.filter(bucket => /5h|hour/i.test(`${bucket.window} ${bucket.bucket_id}`)));
    const weekly = lowest(buckets.filter(bucket => /week|7d/i.test(`${bucket.window} ${bucket.bucket_id}`)));
    const modelPercentage = model && Number.isFinite(model.percentage) && model.percentage >= 0 && model.percentage <= 100
        ? model.percentage : null;
    const fiveHourPercentage = fiveHour ? Math.round(fiveHour.remaining_fraction * 100) : null;
    const weeklyPercentage = weekly ? Math.round(weekly.remaining_fraction * 100) : null;
    const known = [modelPercentage, fiveHour ? fiveHour.remaining_fraction * 100 : null, weekly ? weekly.remaining_fraction * 100 : null]
        .filter((value): value is number => value !== null);
    const effectivePercentage = known.length ? Math.floor(Math.min(...known) + 1e-9) : null;
    const limitingBucket = lowest(buckets);
    const resetTime = limitingBucket && (modelPercentage === null || limitingBucket.remaining_fraction * 100 <= modelPercentage + 1)
        ? limitingBucket.reset_time : model?.reset_time;
    const displayPercentage = window === '5h' ? (fiveHourPercentage ?? (weekly ? null : modelPercentage)) : effectivePercentage;
    return {
        percentage: displayPercentage ?? 0,
        displayPercentage,
        effectivePercentage,
        fiveHourPercentage,
        weeklyPercentage,
        resetTime: window === '5h' ? fiveHour?.reset_time || (weekly ? undefined : model?.reset_time) : resetTime,
        isWeeklyConstrained: !!weekly && weekly.remaining_fraction <= 0.001,
        weeklyResetTime: weekly?.reset_time,
    };
}

export function formatQuotaPercentage(value: number | null): string {
    return value === null ? '—' : `${value}%`;
}

import type { ModelQuota, QuotaBucket, QuotaGroup } from '../types/account';

export type DashboardQuotaView = 'weighted' | '5h' | 'weekly';

export interface ConstrainedQuotaResult {
    /** 当前视图的剩余百分比；null 表示该窗口未知。 */
    effectivePercentage: number | null;
    /** 原始 5H 滑动窗口配额百分比 (0-100, 或 null) */
    raw5h: number | null;
    /** 原始 7 天周配额百分比 (0-100, 或 null) */
    rawWeekly: number | null;
    /** 已知模型和时间窗口的最低剩余百分比；全部未知时为 null。 */
    weighted: number | null;
    /** 依据当前模式和约束状态计算出的建议重置时间 */
    resetTime?: string;
    /** 是否受到周配额短板压制 (例如 5H 本有 80% 但周配额仅剩 30%，上限被压至 30%) */
    isWeeklyConstrained: boolean;
    /** 周窗口显示为 0% 时为 true；实际调度资格由后端决定。 */
    isWeeklyExhausted: boolean;
    /** 是否处于 5H 瞬时冷却态 (周配额虽有，但当前 5H 窗口打满归零) */
    is5hCooling: boolean;
    /** 5H 窗口专属重置时间 */
    fiveHourResetTime?: string;
    /** 7天周配额专属重置时间 */
    weeklyResetTime?: string;
}

function getModelQuotaBuckets(modelId: string, groups: QuotaGroup[]): QuotaBucket[] {
    const name = modelId.toLowerCase();
    const thirdParty = /^(claude|gpt)/.test(name);
    return (groups || []).filter(group => {
        const groupName = (group?.display_name || '').toLowerCase();
        const isThirdParty = /claude|gpt|3p/.test(groupName)
            || (group?.buckets || []).some(bucket => /3p/.test(bucket?.bucket_id || ''));
        return thirdParty ? isThirdParty : name.startsWith('gemini') && !isThirdParty;
    }).flatMap(group => group?.buckets || [])
        .filter(bucket => bucket && Number.isFinite(bucket.remaining_fraction)
            && bucket.remaining_fraction >= 0 && bucket.remaining_fraction <= 1);
}

/** 配额保护使用原始周额度比例，避免 5h 窗口或整数取整改变 10% 边界。 */
export function getWeeklyQuotaFraction(modelId: string, groups: QuotaGroup[] = []): number | null {
    const weekly = getModelQuotaBuckets(modelId, groups)
        .filter(bucket => /week|7d/i.test(`${bucket.window} ${bucket.bucket_id}`))
        .reduce<QuotaBucket | undefined>((chosen, bucket) =>
            !chosen || bucket.remaining_fraction < chosen.remaining_fraction ? bucket : chosen, undefined);
    return weekly?.remaining_fraction ?? null;
}

/** 配额显示同时保留窗口原值和综合剩余比例；未知窗口不视为零。 */
export function getModelQuotaDisplay(modelId: string, model: ModelQuota | undefined, groups: QuotaGroup[] = [], window: 'effective' | '5h' = 'effective') {
    const buckets = getModelQuotaBuckets(modelId, groups);
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

/** 返回所选配额窗口及限制状态；窗口值保持独立，未知值不补零。 */
export function getModelConstrainedQuota(
    modelId: string,
    model: ModelQuota | undefined,
    groups: QuotaGroup[] = [],
    view: DashboardQuotaView = 'weighted'
): ConstrainedQuotaResult {
    const display = getModelQuotaDisplay(modelId, model, groups);
    const fiveHour = getModelQuotaDisplay(modelId, model, groups, '5h');
    const raw5h = fiveHour.displayPercentage;
    const rawWeekly = display.weeklyPercentage;
    const fiveHourResetTime = fiveHour.resetTime;
    const weeklyResetTime = display.weeklyResetTime;

    // 2. 状态判定
    const isWeeklyExhausted = rawWeekly !== null && rawWeekly <= 0;
    const isWeeklyConstrained = isWeeklyExhausted || (rawWeekly !== null && raw5h !== null && rawWeekly < raw5h);
    const is5hCooling = raw5h !== null && raw5h <= 0 && (rawWeekly === null || rawWeekly > 0);

    const weighted = display.effectivePercentage;
    const effectivePercentage = view === '5h' ? raw5h : view === 'weekly' ? rawWeekly : weighted;
    const resetTime = view === '5h' ? fiveHourResetTime : view === 'weekly' ? weeklyResetTime : display.resetTime;

    return {
        effectivePercentage,
        raw5h,
        rawWeekly,
        weighted,
        resetTime,
        isWeeklyConstrained,
        isWeeklyExhausted,
        is5hCooling,
        fiveHourResetTime,
        weeklyResetTime,
    };
}

import assert from 'node:assert/strict';
import { getModelQuotaDisplay } from '../../src/utils/quotaDisplay';
import { computeQuotaMetrics, recommendAccount } from '../../src/utils/dashboardQuota';
import type { Account, QuotaGroup } from '../../src/types/account';

const groups = (fiveHour: number, weekly: number): QuotaGroup[] => [{ display_name: 'Gemini Models', buckets: [
    { bucket_id: 'gemini-5h', window: '5h', remaining_fraction: fiveHour, reset_time: '2030-01-01T00:00:00Z' },
    { bucket_id: 'gemini-weekly', window: 'weekly', remaining_fraction: weekly, reset_time: '2030-01-07T00:00:00Z' },
] }];
const account = (id: string, fiveHour: number, weekly: number) => ({ id, email: `${id}@example.test`, last_used: 1,
    quota: { last_updated: 1, models: [{ name: 'gemini-3.7-flash-medium', percentage: Math.floor(100 * Math.min(fiveHour, weekly)), reset_time: '' }], quota_groups: groups(fiveHour, weekly) },
} as Account);
const target = account('reported', .9216947, .15096714);
const display = getModelQuotaDisplay(target.quota!.models[0].name, target.quota!.models[0], target.quota!.quota_groups);
assert.equal(display.fiveHourPercentage, 92);
assert.equal(display.weeklyPercentage, 15);
assert.equal(display.effectivePercentage, 15);
assert.equal(display.percentage, 15);
const shortDisplay = getModelQuotaDisplay(target.quota!.models[0].name, target.quota!.models[0], target.quota!.quota_groups, '5h');
assert.equal(shortDisplay.percentage, 92);
assert.equal(shortDisplay.effectivePercentage, 15);
assert.equal(shortDisplay.resetTime, '2030-01-01T00:00:00Z');
assert.equal(display.resetTime, '2030-01-07T00:00:00Z');
assert.equal(target.quota!.models[0].percentage, 15);
const metrics = computeQuotaMetrics([target, account('fresh', 1, .55), { id: 'unknown' } as Account], 'gemini');
assert.equal(metrics.avg5h, 96);
assert.equal(metrics.avgWeekly, 35);
assert.equal(metrics.averageEffective, 35);
assert.equal(computeQuotaMetrics([], 'gemini').averageEffective, null);
const invalid = groups(.9, .8); invalid[0].buckets![0].remaining_fraction = Number.NaN;
assert.equal(getModelQuotaDisplay('gemini-test', undefined, invalid).fiveHourPercentage, null);
assert.equal(getModelQuotaDisplay('gemini-test', undefined, invalid).effectivePercentage, 80);
assert.equal(getModelQuotaDisplay('claude-test', undefined, groups(.92, .15)).effectivePercentage, null);
const shortOnly = account('short', .6, 1); shortOnly.quota!.quota_groups![0].buckets!.pop();
assert.equal(getModelQuotaDisplay('gemini-test', undefined, shortOnly.quota!.quota_groups).weeklyPercentage, null);
const protection = { enabled: true, threshold_percentage: 10, monitored_models: ['gemini-3-flash'] };
const healthy = account('healthy', .95, .95);
for (const state of [{ validation_blocked: true }, { disabled: true }, { proxy_disabled: true }, { protected_models: ['gemini-3-flash'] }]) {
    assert.equal(recommendAccount([{ ...account('invalid', 1, 1), ...state }, healthy], 'gemini', undefined, protection)?.account.id, 'healthy');
}
assert.equal(recommendAccount([account('boundary', 1, .1)], 'gemini', undefined, protection), undefined);
assert.equal(recommendAccount([account('allowed', 1, .11)], 'gemini', undefined, protection)?.account.id, 'allowed');
assert.equal(recommendAccount([healthy, account('tie', .95, .95)], 'gemini', 'tie', protection)?.account.id, 'tie');
assert.equal(recommendAccount([{...healthy, quota:{...healthy.quota!, is_forbidden: true}}], 'gemini'), undefined);
console.log('Dashboard quota, unknown data, recommendation eligibility and stable selection passed.');

assert.equal(recommendAccount([{ ...account('image-only-protection', 1, 1), protected_models: ['gemini-3.1-flash-image'] }, healthy], 'gemini', undefined, protection)?.account.id, 'image-only-protection');

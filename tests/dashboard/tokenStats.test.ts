import assert from 'node:assert/strict';
import {
    filterAccountTokenStats,
    summarizeAccountTokenStats,
    type AccountTokenStats,
} from '../../src/utils/tokenStats';

const accounts: AccountTokenStats[] = [
    {
        account_email: 'Alpha.User@example.com',
        total_input_tokens: 100,
        total_output_tokens: 20,
        total_cached_tokens: 40,
        total_tokens: 120,
        request_count: 2,
        input_cost_usd: 0.1,
        output_cost_usd: 0.2,
        cached_cost_usd: 0.03,
        total_cost_usd: 0.33,
        unpriced_tokens: 5,
    },
    {
        account_email: 'beta@example.com',
        total_input_tokens: 200,
        total_output_tokens: 50,
        total_cached_tokens: 80,
        total_tokens: 250,
        request_count: 3,
        input_cost_usd: 0.4,
        output_cost_usd: 0.5,
        cached_cost_usd: 0.06,
        total_cost_usd: 0.96,
        unpriced_tokens: 0,
    },
];

const alpha = filterAccountTokenStats(accounts, '  ALPHA.user  ');
assert.equal(alpha.length, 1);
assert.equal(alpha[0].account_email, 'Alpha.User@example.com');
assert.equal(filterAccountTokenStats(accounts, '').length, 2);
assert.equal(filterAccountTokenStats(accounts, 'missing').length, 0);

assert.deepEqual(summarizeAccountTokenStats(accounts), {
    total_input_tokens: 300,
    total_output_tokens: 70,
    total_cached_tokens: 120,
    total_tokens: 370,
    total_requests: 5,
    total_cost_usd: 1.29,
    total_unpriced_tokens: 5,
    unique_accounts: 2,
});

assert.deepEqual(summarizeAccountTokenStats(alpha), {
    total_input_tokens: 100,
    total_output_tokens: 20,
    total_cached_tokens: 40,
    total_tokens: 120,
    total_requests: 2,
    total_cost_usd: 0.33,
    total_unpriced_tokens: 5,
    unique_accounts: 1,
});

console.log('token stats filtering tests passed');

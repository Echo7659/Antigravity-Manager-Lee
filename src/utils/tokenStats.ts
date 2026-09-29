export interface AccountTokenStats {
    account_email: string;
    total_input_tokens: number;
    total_output_tokens: number;
    total_cached_tokens: number;
    total_tokens: number;
    request_count: number;
    input_cost_usd: number;
    output_cost_usd: number;
    cached_cost_usd: number;
    total_cost_usd: number;
    unpriced_tokens: number;
}

export interface AccountTokenSummary {
    total_input_tokens: number;
    total_output_tokens: number;
    total_cached_tokens: number;
    total_tokens: number;
    total_requests: number;
    total_cost_usd: number;
    total_unpriced_tokens: number;
    unique_accounts: number;
}

export function filterAccountTokenStats(
    accounts: AccountTokenStats[],
    query: string,
): AccountTokenStats[] {
    const normalizedQuery = query.trim().toLowerCase();
    if (!normalizedQuery) return accounts;

    return accounts.filter((account) =>
        account.account_email.toLowerCase().includes(normalizedQuery),
    );
}

export function summarizeAccountTokenStats(
    accounts: AccountTokenStats[],
): AccountTokenSummary {
    return accounts.reduce<AccountTokenSummary>(
        (summary, account) => ({
            total_input_tokens: summary.total_input_tokens + account.total_input_tokens,
            total_output_tokens: summary.total_output_tokens + account.total_output_tokens,
            total_cached_tokens: summary.total_cached_tokens + account.total_cached_tokens,
            total_tokens: summary.total_tokens + account.total_tokens,
            total_requests: summary.total_requests + account.request_count,
            total_cost_usd: summary.total_cost_usd + account.total_cost_usd,
            total_unpriced_tokens: summary.total_unpriced_tokens + account.unpriced_tokens,
            unique_accounts: summary.unique_accounts + 1,
        }),
        {
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cached_tokens: 0,
            total_tokens: 0,
            total_requests: 0,
            total_cost_usd: 0,
            total_unpriced_tokens: 0,
            unique_accounts: 0,
        },
    );
}

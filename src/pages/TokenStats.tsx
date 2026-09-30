import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Calendar, CalendarDays, Clock, RefreshCw, Search } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import Pagination from '../components/common/Pagination';
import { request as invoke } from '../utils/request';
import {
    filterAccountTokenStats,
    summarizeAccountTokenStats,
    type AccountTokenStats,
} from '../utils/tokenStats';

type TimeRange = 'hourly' | 'daily' | 'weekly';

const TIME_RANGE_HOURS: Record<TimeRange, number> = {
    hourly: 24,
    daily: 168,
    weekly: 720,
};

const ACCOUNTS_PER_PAGE = 50;

function formatNumber(num: number): string {
    if (num >= 1_000_000) return `${(num / 1_000_000).toFixed(1)}M`;
    if (num >= 1_000) return `${(num / 1_000).toFixed(1)}K`;
    return num.toLocaleString();
}

function formatUsd(amount: number): string {
    const fractionDigits = amount >= 1 ? 4 : 6;
    return `$${amount.toFixed(fractionDigits)}`;
}

const TokenStats: React.FC = () => {
    const { t } = useTranslation();
    const [timeRange, setTimeRange] = useState<TimeRange>('daily');
    const [accountData, setAccountData] = useState<AccountTokenStats[]>([]);
    const [searchQuery, setSearchQuery] = useState('');
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [currentPage, setCurrentPage] = useState(1);
    const requestIdRef = useRef(0);
    const loadedRangeRef = useRef<TimeRange | null>(null);

    const fetchData = useCallback(async () => {
        const requestId = ++requestIdRef.current;
        if (loadedRangeRef.current !== timeRange) {
            setAccountData([]);
        }
        setLoading(true);
        setError(null);

        try {
            const accounts = await invoke<AccountTokenStats[]>('get_token_stats_by_account', {
                hours: TIME_RANGE_HOURS[timeRange],
            });
            if (requestId === requestIdRef.current) {
                setAccountData(accounts);
                setCurrentPage(1);
                loadedRangeRef.current = timeRange;
            }
        } catch (fetchError) {
            console.error('Failed to fetch token stats:', fetchError);
            if (requestId === requestIdRef.current) {
                setError(String(fetchError));
            }
        } finally {
            if (requestId === requestIdRef.current) {
                setLoading(false);
            }
        }
    }, [timeRange]);

    useEffect(() => {
        void fetchData();
    }, [fetchData]);

    const visibleAccounts = useMemo(
        () => filterAccountTokenStats(accountData, searchQuery),
        [accountData, searchQuery],
    );
    const summary = useMemo(
        () => summarizeAccountTokenStats(visibleAccounts),
        [visibleAccounts],
    );
    const totalPages = Math.max(1, Math.ceil(visibleAccounts.length / ACCOUNTS_PER_PAGE));
    const paginatedAccounts = useMemo(() => {
        const start = (currentPage - 1) * ACCOUNTS_PER_PAGE;
        return visibleAccounts.slice(start, start + ACCOUNTS_PER_PAGE);
    }, [currentPage, visibleAccounts]);

    useEffect(() => {
        setCurrentPage(1);
    }, [searchQuery, timeRange]);

    useEffect(() => {
        setCurrentPage((page) => Math.min(page, totalPages));
    }, [totalPages]);

    const rangeOptions: Array<{
        id: TimeRange;
        label: string;
        icon: React.ComponentType<{ className?: string }>;
    }> = [
        { id: 'hourly', label: t('token_stats.hourly', '小时'), icon: Clock },
        { id: 'daily', label: t('token_stats.daily', '日'), icon: Calendar },
        { id: 'weekly', label: t('token_stats.weekly', '周'), icon: CalendarDays },
    ];
    const selectedRangeLabel = rangeOptions.find((option) => option.id === timeRange)?.label ?? '';

    return (
        <div className="h-full w-full overflow-y-auto">
            <div className="p-5 space-y-4 max-w-7xl mx-auto">
                <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                    <h1 className="text-2xl font-bold text-gray-800 dark:text-white">
                        {t('token_stats.title', 'Token 消费统计')}
                    </h1>
                    <div className="flex items-center gap-2">
                        <div className="flex bg-gray-100 dark:bg-gray-800 rounded-lg p-1">
                            {rangeOptions.map(({ id, label, icon: Icon }) => (
                                <button
                                    key={id}
                                    type="button"
                                    onClick={() => setTimeRange(id)}
                                    aria-pressed={timeRange === id}
                                    className={`px-3 py-1.5 rounded-md text-sm font-medium transition-colors flex items-center gap-1.5 ${timeRange === id
                                        ? 'bg-white dark:bg-gray-700 text-blue-600 shadow-sm'
                                        : 'text-gray-600 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200'
                                        }`}
                                >
                                    <Icon className="w-4 h-4" />
                                    {label}
                                </button>
                            ))}
                        </div>
                        <button
                            type="button"
                            onClick={() => void fetchData()}
                            disabled={loading}
                            aria-label={t('common.refresh', '刷新')}
                            className="p-2 rounded-lg bg-blue-500 text-white hover:bg-blue-600 transition-colors disabled:opacity-50"
                        >
                            <RefreshCw className={`w-4 h-4 ${loading ? 'animate-spin' : ''}`} />
                        </button>
                    </div>
                </div>

                {error && (
                    <div className="rounded-xl border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700 dark:border-red-900/50 dark:bg-red-950/30 dark:text-red-300">
                        {t('token_stats.load_failed', '统计数据加载失败，请重试。')} {error}
                    </div>
                )}

                <div className="grid grid-cols-2 lg:grid-cols-3 xl:grid-cols-6 gap-3">
                    <div className="rounded-xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800">
                        <div className="text-sm text-gray-500 dark:text-gray-400">
                            {t('token_stats.total_tokens', '总 Token')}
                        </div>
                        <div className="mt-2 text-2xl font-bold text-gray-800 dark:text-white">
                            {formatNumber(summary.total_tokens)}
                        </div>
                        <div className="mt-1 text-xs text-gray-400 dark:text-gray-500">
                            {t('token_stats.requests_count', '{{requests}} 次请求', { requests: formatNumber(summary.total_requests) })}
                        </div>
                    </div>
                    <div className="rounded-xl border border-emerald-100 bg-white p-4 shadow-sm dark:border-emerald-900/30 dark:bg-gray-800">
                        <div className="text-sm text-emerald-600/80 dark:text-emerald-400/80">
                            {t('token_stats.estimated_cost', '预估金额')}
                        </div>
                        <div className="mt-2 text-2xl font-bold text-emerald-600 dark:text-emerald-400">
                            {formatUsd(summary.total_cost_usd)}
                        </div>
                        {summary.total_unpriced_tokens > 0 && (
                            <div className="mt-1 text-xs text-amber-600 dark:text-amber-400">
                                {t('token_stats.unpriced_tokens', '{{tokens}} Token 未计价', {
                                    tokens: formatNumber(summary.total_unpriced_tokens),
                                })}
                            </div>
                        )}
                    </div>
                    <div className="rounded-xl border border-blue-100 bg-white p-4 shadow-sm dark:border-blue-900/30 dark:bg-gray-800">
                        <div className="text-sm text-blue-600/80 dark:text-blue-400/80">
                            {t('token_stats.input_tokens', '输入 Token')}
                        </div>
                        <div className="mt-2 text-2xl font-bold text-blue-600 dark:text-blue-400">
                            {formatNumber(summary.total_input_tokens)}
                        </div>
                    </div>
                    <div className="rounded-xl border border-purple-100 bg-white p-4 shadow-sm dark:border-purple-900/30 dark:bg-gray-800">
                        <div className="text-sm text-purple-600/80 dark:text-purple-400/80">
                            {t('token_stats.output_tokens', '输出 Token')}
                        </div>
                        <div className="mt-2 text-2xl font-bold text-purple-600 dark:text-purple-400">
                            {formatNumber(summary.total_output_tokens)}
                        </div>
                    </div>
                    <div className="rounded-xl border border-sky-100 bg-white p-4 shadow-sm dark:border-sky-900/30 dark:bg-gray-800">
                        <div className="text-sm text-sky-600/80 dark:text-sky-400/80">
                            {t('token_stats.cached_token', '缓存命中')}
                        </div>
                        <div className="mt-2 text-2xl font-bold text-sky-600 dark:text-sky-400">
                            {formatNumber(summary.total_cached_tokens)}
                        </div>
                    </div>
                    <div className="rounded-xl border border-green-100 bg-white p-4 shadow-sm dark:border-green-900/30 dark:bg-gray-800">
                        <div className="text-sm text-green-600/80 dark:text-green-400/80">
                            {searchQuery.trim()
                                ? t('token_stats.matched_accounts', '匹配账号')
                                : t('token_stats.accounts_used', '活跃账号')}
                        </div>
                        <div className="mt-2 text-2xl font-bold text-green-600 dark:text-green-400">
                            {summary.unique_accounts}
                        </div>
                        {searchQuery.trim() && (
                            <div className="mt-1 text-xs text-gray-400 dark:text-gray-500">
                                {t('token_stats.total_accounts', '共 {{total}} 个有用量账号', {
                                    total: accountData.length,
                                })}
                            </div>
                        )}
                    </div>
                </div>

                <div className="rounded-xl border border-gray-200 bg-white p-5 shadow-sm dark:border-gray-700 dark:bg-gray-800">
                    <div className="flex flex-col gap-3 md:flex-row md:items-start md:justify-between">
                        <div>
                            <h2 className="text-lg font-semibold text-gray-800 dark:text-white">
                                {t('token_stats.account_details', '账号详细统计')}
                            </h2>
                            <p className="mt-1 text-xs text-gray-500 dark:text-gray-400">
                                {t('token_stats.cost_note', '按实际路由模型计价；缓存 Token 统一按 $0.03/M，未配置价格的 Token 会单独标记。')}
                            </p>
                        </div>
                        <div className="relative w-full md:w-80">
                            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-gray-400" />
                            <input
                                type="search"
                                value={searchQuery}
                                onChange={(event) => setSearchQuery(event.target.value)}
                                placeholder={t('token_stats.search_account_placeholder', '输入邮箱搜索账号')}
                                aria-label={t('token_stats.search_account', '搜索账号')}
                                className="w-full rounded-lg border border-gray-200 bg-gray-50 py-2 pl-9 pr-3 text-sm text-gray-800 outline-none transition focus:border-blue-400 focus:ring-2 focus:ring-blue-100 dark:border-gray-600 dark:bg-gray-900 dark:text-gray-100 dark:focus:border-blue-500 dark:focus:ring-blue-900/40"
                            />
                        </div>
                    </div>

                    <div className="mt-4 overflow-x-auto" aria-busy={loading}>
                        <table className="w-full min-w-[850px] text-sm">
                            <thead>
                                <tr className="border-b border-gray-200 dark:border-gray-700">
                                    <th className="px-4 py-3 text-left font-medium text-gray-500 dark:text-gray-400">
                                        {t('token_stats.account', '账号')}
                                    </th>
                                    <th className="px-4 py-3 text-right font-medium text-gray-500 dark:text-gray-400">
                                        {t('token_stats.requests', '请求数')}
                                    </th>
                                    <th className="px-4 py-3 text-right font-medium text-gray-500 dark:text-gray-400">
                                        {t('token_stats.input', '输入')}
                                    </th>
                                    <th className="px-4 py-3 text-right font-medium text-gray-500 dark:text-gray-400">
                                        {t('token_stats.output', '输出')}
                                    </th>
                                    <th className="px-4 py-3 text-right font-medium text-gray-500 dark:text-gray-400">
                                        {t('token_stats.cached_token', '缓存命中')}
                                    </th>
                                    <th className="px-4 py-3 text-right font-medium text-gray-500 dark:text-gray-400">
                                        {t('token_stats.total', '合计')}
                                    </th>
                                    <th className="px-4 py-3 text-right font-medium text-gray-500 dark:text-gray-400">
                                        {t('token_stats.estimated_cost_usd', '预估金额 (USD)')}
                                    </th>
                                </tr>
                            </thead>
                            <tbody>
                                {paginatedAccounts.map((account) => (
                                    <tr
                                        key={account.account_email}
                                        className="border-b border-gray-100 hover:bg-gray-50 dark:border-gray-700/50 dark:hover:bg-gray-700/30"
                                    >
                                        <td className="px-4 py-3 text-gray-800 dark:text-white">
                                            {account.account_email}
                                        </td>
                                        <td className="px-4 py-3 text-right text-gray-600 dark:text-gray-300">
                                            {account.request_count.toLocaleString()}
                                        </td>
                                        <td className="px-4 py-3 text-right text-blue-600 dark:text-blue-400">
                                            {formatNumber(account.total_input_tokens)}
                                        </td>
                                        <td className="px-4 py-3 text-right text-purple-600 dark:text-purple-400">
                                            {formatNumber(account.total_output_tokens)}
                                        </td>
                                        <td className="px-4 py-3 text-right text-sky-600 dark:text-sky-400">
                                            {formatNumber(account.total_cached_tokens)}
                                        </td>
                                        <td className="px-4 py-3 text-right font-semibold text-gray-800 dark:text-white">
                                            {formatNumber(account.total_tokens)}
                                        </td>
                                        <td
                                            className="whitespace-nowrap px-4 py-3 text-right"
                                            title={`${t('token_stats.input_cost', '输入金额')}: ${formatUsd(account.input_cost_usd)} · ${t('token_stats.cached_cost', '缓存金额')}: ${formatUsd(account.cached_cost_usd)} · ${t('token_stats.output_cost', '输出金额')}: ${formatUsd(account.output_cost_usd)}`}
                                        >
                                            <div className="font-semibold text-emerald-600 dark:text-emerald-400">
                                                {formatUsd(account.total_cost_usd)}
                                            </div>
                                            {account.unpriced_tokens > 0 && (
                                                <div className="text-xs text-amber-600 dark:text-amber-400">
                                                    {t('token_stats.unpriced_tokens', '{{tokens}} Token 未计价', {
                                                        tokens: formatNumber(account.unpriced_tokens),
                                                    })}
                                                </div>
                                            )}
                                        </td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>

                        {loading && accountData.length === 0 && (
                            <div className="py-12 text-center text-sm text-gray-400">
                                {t('token_stats.loading_range', '正在加载{{range}}数据...', {
                                    range: selectedRangeLabel,
                                })}
                            </div>
                        )}
                        {!loading && visibleAccounts.length === 0 && (
                            <div className="py-12 text-center text-sm text-gray-400">
                                {searchQuery.trim()
                                    ? t('token_stats.no_matching_accounts', '没有找到匹配的账号')
                                    : t('token_stats.no_data', '暂无数据')}
                            </div>
                        )}
                    </div>
                    {visibleAccounts.length > 0 && (
                        <Pagination
                            currentPage={currentPage}
                            totalPages={totalPages}
                            onPageChange={setCurrentPage}
                            totalItems={visibleAccounts.length}
                            itemsPerPage={ACCOUNTS_PER_PAGE}
                        />
                    )}
                </div>
            </div>
        </div>
    );
};

export default TokenStats;

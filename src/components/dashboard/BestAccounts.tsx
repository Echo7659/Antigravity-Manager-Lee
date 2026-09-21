import { Loader2, TrendingUp } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { Account } from '../../types/account';
import { useConfigStore } from '../../stores/useConfigStore';
import { recommendAccount } from '../../utils/dashboardQuota';

interface BestAccountsProps {
    accounts: Account[];
    currentAccountId?: string;
    switching?: boolean;
    onSwitch?: (accountId: string) => void;
}

function BestAccounts({ accounts, currentAccountId, switching = false, onSwitch }: BestAccountsProps) {
    const { t } = useTranslation();
    const protection = useConfigStore(state => state.config?.quota_protection);
    const recommendations = (['gemini', 'claude'] as const).map(category => ({
        category,
        best: recommendAccount(accounts, category, currentAccountId, protection),
    }));
    return (
        <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200 h-full flex flex-col">
            <h2 className="text-base font-semibold text-gray-900 dark:text-base-content mb-3 flex items-center gap-2">
                <TrendingUp className="w-4 h-4 text-blue-500" />{t('dashboard.best_accounts')}
            </h2>
            <div className="space-y-3 flex-1">
                {recommendations.map(({ category, best }) => {
                    const family = category === 'gemini' ? 'Gemini' : 'Claude';
                    const isCurrent = best?.account.id === currentAccountId;
                    let buttonLabel = t('dashboard.switch_best_for', { defaultValue: '切换 {{model}} 最佳', model: family });
                    if (isCurrent) buttonLabel = t('dashboard.already_best', '当前已是最佳账号');
                    if (switching) buttonLabel = t('dashboard.switching_account', '正在切换账号，请稍候…');
                    return (
                        <div key={category} className="p-3 bg-gray-50 dark:bg-base-200 rounded-lg border border-gray-100 dark:border-base-300">
                            <div className="text-xs text-gray-500 mb-1">{t(`dashboard.for_${category}`)}</div>
                            {best ? <>
                                <div className="flex justify-between gap-2 items-center">
                                    <span className="text-sm truncate text-gray-900 dark:text-base-content">{best.account.email}</span>
                                    <span className="text-sm font-bold text-emerald-600 shrink-0">{best.quota}%</span>
                                </div>
                                <p className="text-[11px] text-gray-500 mt-1">{t('dashboard.recommendation_basis', '按综合可用额度推荐，排除异常、停用和受保护账号。')}</p>
                                {onSwitch && <button type="button" disabled={switching || isCurrent}
                                    aria-busy={switching} data-quota-category={category} data-account-id={best.account.id}
                                    className="mt-2 w-full py-1.5 rounded-lg bg-blue-500 text-white text-xs hover:bg-blue-600 disabled:opacity-50 disabled:cursor-not-allowed flex items-center justify-center gap-1"
                                    onClick={() => onSwitch(best.account.id)}>
                                    {switching && <Loader2 className="w-3 h-3 animate-spin" />}
                                    {buttonLabel}
                                </button>}
                            </> : <p className="text-sm text-gray-400">{t('dashboard.no_eligible_recommendation', '暂无符合条件的账号')}</p>}
                        </div>
                    );
                })}
            </div>
        </div>
    );
}
export default BestAccounts;

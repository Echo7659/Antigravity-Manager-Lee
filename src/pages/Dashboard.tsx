import { save } from '@tauri-apps/plugin-dialog';
import {
    AlertTriangle,
    ArrowRight,
    Ban,
    Bot,
    Download,
    RefreshCw,
    ShieldAlert,
    ShieldCheck,
    Sparkles,
    Users,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate } from 'react-router-dom';
import AddAccountDialog from '../components/accounts/AddAccountDialog';
import { showToast } from '../components/common/ToastContainer';
import BestAccounts from '../components/dashboard/BestAccounts';
import { computeQuotaMetrics, isAccountAvailable } from '../utils/dashboardQuota';
import { formatQuotaPercentage } from '../utils/quotaDisplay';
import CurrentAccount from '../components/dashboard/CurrentAccount';
import { exportAccounts } from '../services/accountService';
import { useAccountStore } from '../stores/useAccountStore';
import { Account } from '../types/account';
import { isTauri } from '../utils/env';
import { request as invoke } from '../utils/request';

function Dashboard() {
    const { t } = useTranslation();
    const navigate = useNavigate();
    const {
        accounts,
        currentAccount,
        fetchAccounts,
        fetchCurrentAccount,
        switchAccount,
        addAccount,
        refreshQuota
    } = useAccountStore();

    useEffect(() => {
        fetchAccounts();
        fetchCurrentAccount();
    }, []);

    const [onlyAvailable, setOnlyAvailable] = useState(true);

    // 全维度账号健康与配额计算矩阵 (状态由风控定生死，底座池支持仅可用账号与全部正常账号无缝切换)
    const stats = useMemo(() => {
        const abnormalAccounts = accounts.filter(a => a.validation_blocked || a.quota?.is_forbidden);
        const disabledAccounts = accounts.filter(a => !a.validation_blocked && !a.quota?.is_forbidden && (a.disabled || a.proxy_disabled));
        const availableAccounts = accounts.filter(isAccountAvailable);
        const normalAccounts = accounts.filter(a => !a.validation_blocked && !a.quota?.is_forbidden);
        const basePool = onlyAvailable ? availableAccounts : normalAccounts;
        const gemini = computeQuotaMetrics(basePool, 'gemini');
        const geminiImage = computeQuotaMetrics(basePool, 'image');
        const claude = computeQuotaMetrics(basePool, 'claude');

        return {
            total: accounts.length,
            available: availableAccounts.length,
            disabled: disabledAccounts.length,
            abnormal: abnormalAccounts.length,
            normalCount: normalAccounts.length,
            basePoolCount: basePool.length,
            gemini,
            geminiImage,
            claude,
        };
    }, [accounts, onlyAvailable]);

    const quotaStatus = (value: number | null) => {
        if (value === null) return t('common.unknown');
        return value >= 50 ? t('dashboard.quota_sufficient_short', '充足') : t('dashboard.quota_tight_short', '偏紧');
    };

    const isSwitchingRef = useRef(false);
    const [isSwitching, setIsSwitching] = useState(false);
    const [switchFeedback, setSwitchFeedback] = useState<{ message: string; failed?: boolean } | null>(null);

    const handleSwitch = async (accountId: string) => {
        if (isSwitchingRef.current) return;

        isSwitchingRef.current = true;
        setIsSwitching(true);
        setSwitchFeedback({ message: t('dashboard.switching_account', '正在切换账号，请稍候…') });
        try {
            await switchAccount(accountId);
            const selected = useAccountStore.getState().currentAccount;
            const message = t('dashboard.switched_to', { defaultValue: '已切换到 {{account}}', account: selected?.email || accountId });
            setSwitchFeedback({ message });
            showToast(message, 'success');
        } catch (error) {
            console.error('切换账号失败:', error);
            const message = `${t('dashboard.toast.switch_error')}: ${error}`;
            setSwitchFeedback({ message, failed: true });
            showToast(message, 'error');
        } finally {
            isSwitchingRef.current = false;
            setIsSwitching(false);
        }
    };

    const handleAddAccount = async (email: string, refreshToken: string) => {
        await addAccount(email, refreshToken);
        await fetchAccounts(); // 刷新列表
    };

    const [isRefreshing, setIsRefreshing] = useState(false);

    const handleRefreshCurrent = async () => {
        if (!currentAccount) return;

        setIsRefreshing(true);
        try {
            await refreshQuota(currentAccount.id);
            // 刷新成功后重新获取最新数据
            await fetchCurrentAccount();
            showToast(t('dashboard.toast.refresh_success'), 'success');
        } catch (error) {
            console.error('[Dashboard] Refresh failed:', error);
            showToast(`${t('dashboard.toast.refresh_error')}: ${error}`, 'error');
        } finally {
            setIsRefreshing(false);
        }
    };

    const exportAccountsToJson = async (accountsToExport: Account[]) => {
        try {
            if (accountsToExport.length === 0) {
                showToast(t('dashboard.toast.export_no_accounts'), 'warning');
                return;
            }

            // Get export data from API (contains refresh_token)
            const accountIds = accountsToExport.map(acc => acc.id);
            const response = await exportAccounts(accountIds);

            if (!response.accounts || response.accounts.length === 0) {
                showToast(t('dashboard.toast.export_no_accounts'), 'warning');
                return;
            }

            const exportData = response.accounts;
            const content = JSON.stringify(exportData, null, 2);
            const fileName = `antigravity_accounts_${new Date().toISOString().split('T')[0]}.json`;

            if (isTauri()) {
                const path = await save({
                    filters: [{
                        name: 'JSON',
                        extensions: ['json']
                    }],
                    defaultPath: fileName
                });

                if (!path) return;

                await invoke('save_text_file', { path, content });
                showToast(t('dashboard.toast.export_success', { path }), 'success');
            } else {
                // Web 模式：使用浏览器下载
                const blob = new Blob([content], { type: 'application/json' });
                const url = URL.createObjectURL(blob);
                const a = document.createElement('a');
                a.href = url;
                a.download = fileName;
                document.body.appendChild(a);
                a.click();
                document.body.removeChild(a);
                URL.revokeObjectURL(url);
                showToast(t('dashboard.toast.export_success', { path: fileName }), 'success');
            }
        } catch (error: any) {
            console.error('Export failed:', error);
            showToast(`${t('dashboard.toast.export_error')}: ${error.toString()}`, 'error');
        }
    };

    const handleExport = () => {
        exportAccountsToJson(accounts);
    };

    return (
        <div className="h-full w-full overflow-y-auto">
            <div
                className="p-5 space-y-4 max-w-7xl mx-auto"
                onMouseMove={() => console.log('Mouse moving over Dashboard')}
                style={{ position: 'relative', zIndex: 1 }}
            >
                {/* 问候语和操作按钮 */}
                <div
                    className="flex justify-between items-center"
                >
                    <div>
                        <h1 className="text-2xl font-bold text-gray-900 dark:text-base-content">
                            {currentAccount
                                ? t('dashboard.hello').replace('用户', currentAccount.name || currentAccount.email.split('@')[0])
                                : t('dashboard.hello')
                            }
                        </h1>
                    </div>
                    <div className="flex gap-2">
                        <AddAccountDialog onAdd={handleAddAccount} />
                        <button
                            className={`px-3 py-1.5 bg-blue-500 text-white text-xs font-medium rounded-lg hover:bg-blue-600 transition-colors flex items-center gap-1.5 shadow-sm ${isRefreshing || !currentAccount ? 'opacity-70 cursor-not-allowed' : ''}`}
                            onClick={handleRefreshCurrent}
                            disabled={isRefreshing || !currentAccount}
                            title={isRefreshing ? t('dashboard.refreshing') : t('dashboard.refresh_quota')}
                        >
                            <RefreshCw className={`w-3.5 h-3.5 ${isRefreshing ? 'animate-spin' : ''}`} />
                            <span className="hidden sm:inline">{isRefreshing ? t('dashboard.refreshing') : t('dashboard.refresh_quota')}</span>
                        </button>
                    </div>
                </div>

                {/* 1. 账号生态健康状态阵列 (4 核心卡片，以风控与配置定生死) */}
                <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
                    {/* 总账号数 */}
                    <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
                        <div className="flex items-center justify-between mb-2">
                            <div className="p-1.5 bg-blue-50 dark:bg-blue-900/20 rounded-md">
                                <Users className="w-4 h-4 text-blue-500 dark:text-blue-400" />
                            </div>
                            <span className="text-[10px] font-mono font-bold text-gray-400 dark:text-gray-500">TOTAL</span>
                        </div>
                        <div className="text-2xl font-bold text-gray-900 dark:text-base-content mb-0.5">{stats.total}</div>
                        <div className="text-xs text-gray-500 dark:text-gray-400 font-medium">
                            {t('dashboard.total_accounts', '总账号数')}
                        </div>
                        <div className="text-[10px] text-gray-400 dark:text-gray-500 mt-1.5">
                            {t('dashboard.total_accounts_desc', '已录入配置的全部账号')}
                        </div>
                    </div>

                    {/* 可用账号数 (开启且风控正常，不以额度定生死) */}
                    <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-emerald-100 dark:border-emerald-950/40">
                        <div className="flex items-center justify-between mb-2">
                            <div className="p-1.5 bg-emerald-50 dark:bg-emerald-900/20 rounded-md">
                                <ShieldCheck className="w-4 h-4 text-emerald-600 dark:text-emerald-400" />
                            </div>
                            <span className="text-[10px] font-mono font-bold text-emerald-600 dark:text-emerald-400">ACTIVE</span>
                        </div>
                        <div className="text-2xl font-bold text-emerald-600 dark:text-emerald-400 mb-0.5">{stats.available}</div>
                        <div className="text-xs text-gray-700 dark:text-gray-300 font-bold">
                            {t('dashboard.available_accounts', '可用账号数')}
                        </div>
                        <div className="text-[10px] text-emerald-600/80 dark:text-emerald-400/80 mt-1.5">
                            {t('dashboard.available_accounts_desc', '✓ 开启且状态正常 · 反代服务中')}
                        </div>
                    </div>

                    {/* 禁用账号数 (手动停用) */}
                    <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
                        <div className="flex items-center justify-between mb-2">
                            <div className="p-1.5 bg-gray-100 dark:bg-base-300 rounded-md">
                                <Ban className="w-4 h-4 text-gray-500 dark:text-gray-400" />
                            </div>
                            <span className="text-[10px] font-mono font-bold text-gray-400 dark:text-gray-500">DISABLED</span>
                        </div>
                        <div className="text-2xl font-bold text-gray-600 dark:text-gray-300 mb-0.5">{stats.disabled}</div>
                        <div className="text-xs text-gray-500 dark:text-gray-400 font-medium">
                            {t('dashboard.disabled_accounts', '禁用账号数')}
                        </div>
                        <div className="text-[10px] text-gray-400 dark:text-gray-500 mt-1.5">
                            {t('dashboard.disabled_accounts_desc', '手动停用 / 反代已禁用')}
                        </div>
                    </div>

                    {/* 异常账号数 (风控阻断或403封禁) */}
                    <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
                        <div className="flex items-center justify-between mb-2">
                            <div className={`p-1.5 rounded-md ${stats.abnormal > 0 ? 'bg-rose-50 dark:bg-rose-900/20' : 'bg-gray-50 dark:bg-base-300'}`}>
                                <ShieldAlert className={`w-4 h-4 ${stats.abnormal > 0 ? 'text-rose-600 dark:text-rose-400' : 'text-gray-400'}`} />
                            </div>
                            <span className={`text-[10px] font-mono font-bold ${stats.abnormal > 0 ? 'text-rose-500' : 'text-gray-400'}`}>RISK</span>
                        </div>
                        <div className={`text-2xl font-bold mb-0.5 ${stats.abnormal > 0 ? 'text-rose-600 dark:text-rose-400' : 'text-gray-900 dark:text-base-content'}`}>{stats.abnormal}</div>
                        <div className="text-xs text-gray-500 dark:text-gray-400 font-medium">
                            {t('dashboard.abnormal_accounts', '异常账号数')}
                        </div>
                        <div className={`text-[10px] mt-1.5 ${stats.abnormal > 0 ? 'text-rose-600 font-bold' : 'text-gray-400 dark:text-gray-500'}`}>
                            {stats.abnormal > 0
                                ? t('dashboard.abnormal_accounts_has_risk', '⚠ 需处理风控验证 / 封禁')
                                : t('dashboard.abnormal_accounts_no_risk', '✓ 零风控异常账号')
                            }
                        </div>
                    </div>
                </div>

                {/* 2. 配额生产力矩阵与显眼的双选胶囊控制器 (Pill Capsule Control) */}
                <div className="flex flex-wrap items-center justify-between gap-2 pt-1 pb-1">
                    <div className="flex items-center gap-2">
                        <span className="text-xs font-bold text-gray-800 dark:text-gray-200">
                            {onlyAvailable
                                ? t('dashboard.quota_matrix_active', '可用账号配额生产力矩阵')
                                : t('dashboard.quota_matrix_all_normal', '全量正常账号配额矩阵 (包含已禁用)')
                            }
                        </span>
                        <span className="text-[10px] text-gray-400 dark:text-gray-500 font-mono">
                            ({t('dashboard.base_pool_count', { count: stats.basePoolCount })})
                        </span>
                    </div>

                    {/* 显眼的双选胶囊控制器 (Pill Capsule) */}
                    <div className="flex items-center gap-1 bg-gray-200/80 dark:bg-base-300 p-1 rounded-full shadow-inner border border-gray-200/80 dark:border-base-200 select-none">
                        <button
                            type="button"
                            onClick={() => setOnlyAvailable(true)}
                            className={`px-3.5 py-1.5 rounded-full text-xs font-bold transition-all flex items-center gap-1.5 cursor-pointer ${
                                onlyAvailable
                                    ? 'bg-emerald-600 text-white shadow-md scale-100 ring-2 ring-emerald-400/30'
                                    : 'text-gray-600 dark:text-gray-400 hover:text-gray-900 dark:hover:text-gray-100 hover:bg-black/5 dark:hover:bg-white/5'
                            }`}
                            title={t('dashboard.btn_title_only_available', '当前：仅看开启且正常的可用账号 (点击包含已禁用账号)')}
                        >
                            <ShieldCheck className="w-3.5 h-3.5" />
                            <span>
                                {t('dashboard.btn_only_available', { count: stats.available })}
                            </span>
                        </button>
                        <button
                            type="button"
                            onClick={() => setOnlyAvailable(false)}
                            className={`px-3.5 py-1.5 rounded-full text-xs font-bold transition-all flex items-center gap-1.5 cursor-pointer ${
                                !onlyAvailable
                                    ? 'bg-blue-600 text-white shadow-md scale-100 ring-2 ring-blue-400/30'
                                    : 'text-gray-600 dark:text-gray-400 hover:text-gray-900 dark:hover:text-gray-100 hover:bg-black/5 dark:hover:bg-white/5'
                            }`}
                            title={t('dashboard.btn_title_include_disabled', '当前：查看全部正常状态账号包含禁用 (点击仅看可用账号)')}
                        >
                            <Users className="w-3.5 h-3.5" />
                            <span>
                                {t('dashboard.btn_include_disabled', { count: stats.normalCount })}
                            </span>
                        </button>
                    </div>
                </div>

                {/* 三类配额：综合、5 小时与周配额的已知账号均值 */}
                <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
                    {/* Gemini 文本模型配额 */}
                    <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200 flex flex-col justify-between">
                        <div>
                            <div className="flex items-center justify-between mb-2">
                                <div className="flex items-center gap-2">
                                    <div className="p-1.5 bg-green-50 dark:bg-green-900/20 rounded-md">
                                        <Sparkles className="w-4 h-4 text-green-500 dark:text-green-400" />
                                    </div>
                                    <span className="text-xs font-bold text-gray-800 dark:text-gray-200">
                                        {t('dashboard.gemini_available_quota', 'Gemini 可用配额')}
                                    </span>
                                </div>
                                <span className={`px-2 py-0.5 rounded text-[10px] font-bold ${(stats.gemini.averageEffective ?? 0) >= 50 ? 'bg-green-50 text-green-700 dark:bg-green-900/30 dark:text-green-300' : 'bg-amber-50 text-amber-700 dark:bg-amber-900/30 dark:text-amber-300'}`}>
                                    {quotaStatus(stats.gemini.averageEffective)}
                                </span>
                            </div>

                            <div className="flex items-baseline gap-2 mb-2">
                                <span className="text-3xl font-extrabold text-gray-900 dark:text-base-content font-mono">
                                    {formatQuotaPercentage(stats.gemini.averageEffective)}
                                </span>
                                <span className="text-[11px] text-gray-400 dark:text-gray-500 font-medium">
                                    {t('dashboard.weighted_available', '综合加权可用')}
                                </span>
                            </div>
                        </div>

                        <div className="pt-2 border-t border-gray-100 dark:border-base-300/60 flex items-center justify-between text-[11px]">
                            <div className="flex items-center gap-1.5">
                                <span className="text-gray-400 dark:text-gray-500">{t('dashboard.rolling_5h', '5小时滚动:')}</span>
                                <span className="font-mono font-bold text-emerald-600 dark:text-emerald-400">{formatQuotaPercentage(stats.gemini.avg5h)}</span>
                            </div>
                            <div className="flex items-center gap-1.5">
                                <span className="text-gray-400 dark:text-gray-500">{t('dashboard.weekly_7d', '7天周配额:')}</span>
                                <span className={`font-mono font-bold ${(stats.gemini.avgWeekly ?? 100) <= 10 ? 'text-amber-600 dark:text-amber-400' : 'text-gray-700 dark:text-gray-300'}`}>{formatQuotaPercentage(stats.gemini.avgWeekly)}</span>
                            </div>
                        </div>
                        {stats.gemini.zeroWeeklyCount > 0 && (
                            <div className="text-[10px] text-amber-600 dark:text-amber-400 mt-1.5 font-medium flex items-center gap-1">
                                <AlertTriangle className="w-3 h-3 shrink-0" />
                                <span>{t('dashboard.zero_weekly_warning', { count: stats.gemini.zeroWeeklyCount })}</span>
                            </div>
                        )}
                    </div>

                    {/* Gemini 绘图模型配额 */}
                    <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200 flex flex-col justify-between">
                        <div>
                            <div className="flex items-center justify-between mb-2">
                                <div className="flex items-center gap-2">
                                    <div className="p-1.5 bg-purple-50 dark:bg-purple-900/20 rounded-md">
                                        <Sparkles className="w-4 h-4 text-purple-500 dark:text-purple-400" />
                                    </div>
                                    <span className="text-xs font-bold text-gray-800 dark:text-gray-200">
                                        {t('dashboard.gemini_image_quota', 'Gemini 绘图配额')}
                                    </span>
                                </div>
                                <span className={`px-2 py-0.5 rounded text-[10px] font-bold ${(stats.geminiImage.averageEffective ?? 0) >= 50 ? 'bg-purple-50 text-purple-700 dark:bg-purple-900/30 dark:text-purple-300' : 'bg-amber-50 text-amber-700 dark:bg-amber-900/30 dark:text-amber-300'}`}>
                                    {quotaStatus(stats.geminiImage.averageEffective)}
                                </span>
                            </div>

                            <div className="flex items-baseline gap-2 mb-2">
                                <span className="text-3xl font-extrabold text-gray-900 dark:text-base-content font-mono">
                                    {formatQuotaPercentage(stats.geminiImage.averageEffective)}
                                </span>
                                <span className="text-[11px] text-gray-400 dark:text-gray-500 font-medium">
                                    {t('dashboard.weighted_available', '综合加权可用')}
                                </span>
                            </div>
                        </div>

                        <div className="pt-2 border-t border-gray-100 dark:border-base-300/60 flex items-center justify-between text-[11px]">
                            <div className="flex items-center gap-1.5">
                                <span className="text-gray-400 dark:text-gray-500">{t('dashboard.rolling_5h', '5小时滚动:')}</span>
                                <span className="font-mono font-bold text-emerald-600 dark:text-emerald-400">{formatQuotaPercentage(stats.geminiImage.avg5h)}</span>
                            </div>
                            <div className="flex items-center gap-1.5">
                                <span className="text-gray-400 dark:text-gray-500">{t('dashboard.weekly_7d', '7天周配额:')}</span>
                                <span className={`font-mono font-bold ${(stats.geminiImage.avgWeekly ?? 100) <= 10 ? 'text-amber-600 dark:text-amber-400' : 'text-gray-700 dark:text-gray-300'}`}>{formatQuotaPercentage(stats.geminiImage.avgWeekly)}</span>
                            </div>
                        </div>
                        {stats.geminiImage.zeroWeeklyCount > 0 && (
                            <div className="text-[10px] text-amber-600 dark:text-amber-400 mt-1.5 font-medium flex items-center gap-1">
                                <AlertTriangle className="w-3 h-3 shrink-0" />
                                <span>{t('dashboard.zero_weekly_warning', { count: stats.geminiImage.zeroWeeklyCount })}</span>
                            </div>
                        )}
                    </div>

                    {/* Claude 模型配额 */}
                    <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200 flex flex-col justify-between">
                        <div>
                            <div className="flex items-center justify-between mb-2">
                                <div className="flex items-center gap-2">
                                    <div className="p-1.5 bg-cyan-50 dark:bg-cyan-900/20 rounded-md">
                                        <Bot className="w-4 h-4 text-cyan-500 dark:text-cyan-400" />
                                    </div>
                                    <span className="text-xs font-bold text-gray-800 dark:text-gray-200">
                                        {t('dashboard.claude_available_quota', 'Claude 可用配额')}
                                    </span>
                                </div>
                                <span className={`px-2 py-0.5 rounded text-[10px] font-bold ${(stats.claude.averageEffective ?? 0) >= 50 ? 'bg-cyan-50 text-cyan-700 dark:bg-cyan-900/30 dark:text-cyan-300' : 'bg-amber-50 text-amber-700 dark:bg-amber-900/30 dark:text-amber-300'}`}>
                                    {quotaStatus(stats.claude.averageEffective)}
                                </span>
                            </div>

                            <div className="flex items-baseline gap-2 mb-2">
                                <span className="text-3xl font-extrabold text-gray-900 dark:text-base-content font-mono">
                                    {formatQuotaPercentage(stats.claude.averageEffective)}
                                </span>
                                <span className="text-[11px] text-gray-400 dark:text-gray-500 font-medium">
                                    {t('dashboard.weighted_available', '综合加权可用')}
                                </span>
                            </div>
                        </div>

                        <div className="pt-2 border-t border-gray-100 dark:border-base-300/60 flex items-center justify-between text-[11px]">
                            <div className="flex items-center gap-1.5">
                                <span className="text-gray-400 dark:text-gray-500">{t('dashboard.rolling_5h', '5小时滚动:')}</span>
                                <span className="font-mono font-bold text-emerald-600 dark:text-emerald-400">{formatQuotaPercentage(stats.claude.avg5h)}</span>
                            </div>
                            <div className="flex items-center gap-1.5">
                                <span className="text-gray-400 dark:text-gray-500">{t('dashboard.weekly_7d', '7天周配额:')}</span>
                                <span className={`font-mono font-bold ${(stats.claude.avgWeekly ?? 100) <= 10 ? 'text-amber-600 dark:text-amber-400' : 'text-gray-700 dark:text-gray-300'}`}>{formatQuotaPercentage(stats.claude.avgWeekly)}</span>
                            </div>
                        </div>
                        {stats.claude.zeroWeeklyCount > 0 && (
                            <div className="text-[10px] text-amber-600 dark:text-amber-400 mt-1.5 font-medium flex items-center gap-1">
                                <AlertTriangle className="w-3 h-3 shrink-0" />
                                <span>{t('dashboard.zero_weekly_warning', { count: stats.claude.zeroWeeklyCount })}</span>
                            </div>
                        )}
                    </div>
                </div>

                <p className="text-xs text-gray-500">{t('dashboard.quota_explanation', '综合可用取模型、5 小时和周额度中较低的已知剩余比例；下方分别列出两个时间窗口，未知数据不计入平均值。')}</p>
                {switchFeedback && <div role={switchFeedback.failed ? 'alert' : 'status'} aria-live="polite" className={`rounded-lg p-3 text-sm ${switchFeedback.failed ? 'bg-red-50 text-red-700' : 'bg-blue-50 text-blue-700'}`}>{switchFeedback.message}</div>}
                {/* 双栏布局 */}
                <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                    <CurrentAccount
                        account={currentAccount}
                        onSwitch={() => navigate('/accounts')}
                    />
                    <BestAccounts
                        switching={isSwitching}
                        accounts={accounts}
                        currentAccountId={currentAccount?.id}
                        onSwitch={handleSwitch}
                    />
                </div>

                {/* 快速链接 */}
                <div className="grid grid-cols-2 gap-3">
                    <button
                        className="bg-indigo-50 dark:bg-indigo-900/20 rounded-lg p-3 shadow-sm border border-indigo-100 dark:border-indigo-900/30 hover:border-indigo-300 dark:hover:border-indigo-700 hover:shadow-md transition-all flex items-center justify-between group"
                        onClick={() => navigate('/accounts')}
                    >
                        <span className="text-indigo-700 dark:text-indigo-300 font-medium text-sm">{t('dashboard.view_all_accounts')}</span>
                        <ArrowRight className="w-4 h-4 text-indigo-400 dark:text-indigo-500 group-hover:text-indigo-600 dark:group-hover:text-indigo-300 group-hover:translate-x-1 transition-all" />
                    </button>
                    <button
                        className="bg-purple-50 dark:bg-purple-900/20 rounded-lg p-3 shadow-sm border border-purple-100 dark:border-purple-900/30 hover:border-purple-300 dark:hover:border-purple-700 hover:shadow-md transition-all flex items-center justify-between group"
                        onClick={handleExport}
                    >
                        <span className="text-purple-700 dark:text-purple-300 font-medium text-sm">{t('dashboard.export_data')}</span>
                        <Download className="w-4 h-4 text-purple-400 dark:text-purple-500 group-hover:text-purple-600 dark:group-hover:text-purple-300 transition-all" />
                    </button>
                </div>
            </div>
        </div>
    );
}

export default Dashboard;

import { useTranslation } from 'react-i18next';
import { formatQuotaPercentage } from '../../utils/quotaDisplay';

interface QuotaWindowDetailsProps {
    effectivePercentage?: number | null;
    showEffective?: boolean;
    fiveHourPercentage?: number | null;
    weeklyPercentage?: number | null;
}

export function QuotaWindowDetails({ fiveHourPercentage, weeklyPercentage, effectivePercentage, showEffective }: QuotaWindowDetailsProps) {
    const { t } = useTranslation();
    if (fiveHourPercentage == null && weeklyPercentage == null) return null;
    return <div className="flex flex-wrap gap-x-3 text-[10px] text-gray-500 mt-1" data-quota-windows>
        <span>{t('dashboard.window_5h_short', '5h')} {formatQuotaPercentage(fiveHourPercentage ?? null)}</span>
        <span>{t('accounts.quota_window_weekly_short', '周配额')} {formatQuotaPercentage(weeklyPercentage ?? null)}</span>
        {showEffective && effectivePercentage != null && <span>{t('dashboard.effective_quota_short', '综合')} {formatQuotaPercentage(effectivePercentage)}</span>}
    </div>;
}

use crate::proxy::TokenManager;

/// 本次显式刷新成功及失败的账号数量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RefreshStats {
    pub success: usize,
    pub failed: usize,
}

/// 刷新配额并同步常驻账号池；保护范围刷新仅重载发生变化的账号。
pub async fn refresh_quotas(
    token_manager: &TokenManager,
    protected_only: bool,
) -> Result<RefreshStats, String> {
    let stats = refresh_quotas_detailed(token_manager, protected_only).await?;
    Ok(RefreshStats {
        success: stats.success,
        failed: stats.failed,
    })
}

/// 保留 Web 刷新接口的总数和逐账号错误明细。
pub(crate) async fn refresh_quotas_detailed(
    token_manager: &TokenManager,
    protected_only: bool,
) -> Result<super::account::RefreshStats, String> {
    let stats = if protected_only {
        super::account::refresh_protected_quotas_logic().await?
    } else {
        super::account::refresh_all_quotas_logic().await?
    };
    let mut pending = crate::proxy::server::take_pending_reload_accounts();
    if !protected_only {
        match token_manager.reload_all_accounts().await {
            Ok(_) => pending.clear(),
            Err(error) => tracing::warn!("Account reload after quota refresh failed: {error}"),
        }
        pending.extend(crate::proxy::server::take_pending_reload_accounts());
    }
    pending.sort_unstable();
    pending.dedup();
    for (account_id, error) in token_manager.reload_accounts(&pending).await {
        crate::proxy::server::trigger_account_reload(&account_id);
        tracing::warn!("Quota refresh reload failed for {account_id}: {error}");
    }
    Ok(stats)
}

use crate::models::{Account, TokenData};
use crate::modules::account;
use crate::utils::protobuf;
use base64::{engine::general_purpose, Engine as _};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

/// Scan and import V1 data
pub async fn import_from_v1() -> Result<Vec<Account>, String> {
    use crate::modules::oauth;

    let home = dirs::home_dir().ok_or("Failed to get home directory")?;

    // V1 data directory (confirmed cross-platform consistency from utils.py)
    let v1_dir = home.join(".antigravity-agent");

    let mut imported_accounts = Vec::new();

    // Try multiple possible filenames
    let index_files = vec![
        "antigravity_accounts.json", // Directly use string literal
        "accounts.json",
    ];

    let mut found_index = false;

    for index_filename in index_files {
        let v1_accounts_path = v1_dir.join(index_filename);

        if !v1_accounts_path.exists() {
            continue;
        }

        found_index = true;
        crate::modules::logger::log_info(&format!("V1 data discovered: {:?}", v1_accounts_path));

        let content = match fs::read_to_string(&v1_accounts_path) {
            Ok(c) => c,
            Err(e) => {
                crate::modules::logger::log_warn(&format!("Failed to read index: {}", e));
                continue;
            }
        };

        let v1_index: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                crate::modules::logger::log_warn(&format!("Failed to parse index JSON: {}", e));
                continue;
            }
        };

        // Compatible with two formats: direct map, or contains "accounts" field
        let accounts_map = if let Some(map) = v1_index.as_object() {
            if let Some(accounts) = map.get("accounts").and_then(|v| v.as_object()) {
                accounts
            } else {
                map
            }
        } else {
            continue;
        };

        for (id, acc_info) in accounts_map {
            let email_placeholder = acc_info
                .get("email")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string();

            // Skip non-account keys (e.g. "current_account_id")
            if !acc_info.is_object() {
                continue;
            }

            let backup_file_str = acc_info.get("backup_file").and_then(|v| v.as_str());
            let data_file_str = acc_info.get("data_file").and_then(|v| v.as_str());

            // Prefer backup_file, then data_file
            let target_file = backup_file_str.or(data_file_str);

            if target_file.is_none() {
                crate::modules::logger::log_warn(&format!(
                    "Account {} ({}) missing data file path",
                    id, email_placeholder
                ));
                continue;
            }

            let mut backup_path = PathBuf::from(target_file.unwrap());

            // If relative path, try joining with v1_dir
            if !backup_path.exists() {
                backup_path = v1_dir.join(backup_path.file_name().unwrap_or_default());
            }

            // Try joining data/ or backups/ subdirectories again
            if !backup_path.exists() {
                let file_name = backup_path.file_name().unwrap_or_default();
                let try_backups = v1_dir.join("backups").join(file_name);
                if try_backups.exists() {
                    backup_path = try_backups;
                } else {
                    let try_accounts = v1_dir.join("accounts").join(file_name);
                    if try_accounts.exists() {
                        backup_path = try_accounts;
                    }
                }
            }

            if !backup_path.exists() {
                crate::modules::logger::log_warn(&format!(
                    "Account {} ({}) backup file not found: {:?}",
                    id, email_placeholder, backup_path
                ));
                continue;
            }

            // Read backup file
            if let Ok(backup_content) = fs::read_to_string(&backup_path) {
                if let Ok(backup_json) = serde_json::from_str::<Value>(&backup_content) {
                    // Compatible with two formats:
                    // 1. V1 backup: jetskiStateSync.agentManagerInitState -> Protobuf
                    // 2. V2/Script data: JSON containing "token" field

                    let mut refresh_token_opt = None;

                    // Try format 2
                    if let Some(token_data) = backup_json.get("token") {
                        if let Some(rt) = token_data.get("refresh_token").and_then(|v| v.as_str()) {
                            refresh_token_opt = Some(rt.to_string());
                        }
                    }

                    // Try format 1
                    if refresh_token_opt.is_none() {
                        if let Some(state_b64) = backup_json
                            .get("jetskiStateSync.agentManagerInitState")
                            .and_then(|v| v.as_str())
                        {
                            // Parse Protobuf
                            if let Ok(blob) = general_purpose::STANDARD.decode(state_b64) {
                                if let Ok(Some(oauth_data)) = protobuf::find_field(&blob, 6) {
                                    if let Ok(Some(refresh_bytes)) =
                                        protobuf::find_field(&oauth_data, 3)
                                    {
                                        if let Ok(rt) = String::from_utf8(refresh_bytes) {
                                            refresh_token_opt = Some(rt);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if let Some(refresh_token) = refresh_token_opt {
                        crate::modules::logger::log_info(&format!(
                            "Importing account: {}",
                            email_placeholder
                        ));
                        let (email, access_token, expires_in, oauth_client_key) =
                            match oauth::refresh_access_token(&refresh_token, None).await {
                                Ok(token_resp) => {
                                    let oauth_client_key = token_resp.oauth_client_key.clone();
                                    match oauth::get_user_info(&token_resp.access_token, None).await
                                    {
                                        Ok(user_info) => (
                                            user_info.email,
                                            token_resp.access_token,
                                            token_resp.expires_in,
                                            oauth_client_key,
                                        ),
                                        Err(_) => (
                                            email_placeholder.clone(),
                                            token_resp.access_token,
                                            token_resp.expires_in,
                                            oauth_client_key,
                                        ),
                                    }
                                }
                                Err(e) => {
                                    crate::modules::logger::log_warn(&format!(
                                        "Token refresh failed (likely expired): {}",
                                        e
                                    ));
                                    (
                                        email_placeholder.clone(),
                                        "imported_access_token".to_string(),
                                        0,
                                        None,
                                    )
                                }
                            };
                        let token_data = TokenData::new(
                            access_token,
                            refresh_token,
                            expires_in,
                            Some(email.clone()),
                            None, // project_id will be fetched on demand
                            None, // session_id
                            true, // V1 tokens are Antigravity Google OAuth tokens
                            None, // V1 doesn't have id_token saved
                        )
                        .with_oauth_client_key(oauth_client_key);
                        // Name already fetched in get_user_info at line 153, but outside match scope, use None to be safe
                        match account::upsert_account(email.clone(), None, token_data) {
                            Ok(acc) => {
                                crate::modules::logger::log_info(&format!(
                                    "Import successful: {}",
                                    email
                                ));
                                imported_accounts.push(acc);
                            }
                            Err(e) => crate::modules::logger::log_error(&format!(
                                "Import save failed {}: {}",
                                email, e
                            )),
                        }
                    } else {
                        crate::modules::logger::log_warn(&format!(
                            "Account {} data file missing Refresh Token",
                            email_placeholder
                        ));
                    }
                }
            }
        }
    }

    if !found_index {
        return Err("V1 account data file not found".to_string());
    }

    Ok(imported_accounts)
}

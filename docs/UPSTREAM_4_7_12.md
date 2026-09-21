# Official 4.7.12 synchronization

This release merges upstream `v4.7.12` into the Lee branch. Upstream implementations replace equivalent Lee work; the remaining differences preserve the deployment's established behavior.

| Area | Implementation | Reason |
| --- | --- | --- |
| Model variants and reasoning effort | Upstream routing and protocol adapters | Replaces the Lee suffix reconstruction helper; supports upstream tiered routing. |
| Usage conversion | Upstream `CanonicalUsage` | One converter serves the protocols. A small legacy Gemini correction retains separate/already-merged reasoning count compatibility. |
| Signature recovery, thinking storage, UTF-8 safety, dashboard | Upstream | Adopt the official pipeline and interface. |
| Error classification and cooldown | Upstream `UpstreamClassification` | Service errors such as 500/503 do not imply an exhausted account. |
| Account attempt budget | Lee | First account plus at most five different eligible accounts; upstream's two-pool-round policy is different. No replay after visible stream delivery. |
| Account ordering | Lee comparator with upstream subscription classification | Fixed reset-time buckets preserve transitivity and avoid sorting panics. |
| Quota protection | Lee inclusive boundary and exact-model checks | A model at the configured threshold is excluded even when a sibling model has more quota. Both weekly and short-window limits apply; used-account snapshots refresh after responses. |
| Stream boundaries | Lee supplemental handling | Fragmented UTF-8, a final frame without newline, partial cumulative usage, explicit upstream errors and thinking-only empty answers remain covered. Upstream `include_usage` behavior is preserved. |
| Existing log database startup | Supplemental migration safeguard | New databases enable incremental vacuum. Existing databases are not fully rewritten with `VACUUM` during startup; large historical logs require a separate maintenance window for that conversion. Official additive schema migrations still apply. `ABV_DEFER_LOG_INDEX_MIGRATIONS=true` defers new secondary indexes for an existing large log database until a maintenance window; the original timestamp/status indexes remain usable. |
| Native Gemini normalization | Lee supplemental normalization | Role inference and root `top_p` compatibility remain covered. |

Docker builds the frontend and backend from this same checkout. The pinned runtime supplies OS libraries only; its old frontend and executable are overwritten. CI keeps the Lee image/release workflow and adds the upstream formatting gate. No desktop release workflow is activated.

Targeted compatibility tests use actual converter and policy sources. Only unrelated cache/persistence effects are stubbed in the portable harness. Full Linux compilation and isolated runtime checks are separate release gates; the portable tests are not a full-application integration suite.

Existing deployments must preserve their Compose ports, credential environment, persistent data mount, account records, custom model mapping, quota settings, and log retention. The deployment package is a digest-pinned Compose bundle, not an offline image archive.

Thanks to the contributors of `lbjlaq/Antigravity-Manager`, including @jeikl, for the upstream pipeline and release improvements.

Legacy configurations with `max_rows=0` and no disk-budget fields retain unlimited log storage after migration. Both capacity fields set to zero now mean unlimited; new installations still default to the official 1 GiB budget. Automatic retention never runs a full VACUUM on legacy databases.

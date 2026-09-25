# Official 4.8.1 synchronization

This release merges official `v4.8.1` into the Lee branch. Upstream history and authorship are retained. Thanks to lbjlaq/Antigravity-Manager contributors, @jeikl and @Avlaak (Hermes sync, PR #3518).

## Adopted upstream behavior

- Deterministic payload structure, causal tool IDs, thinking/signature recovery and multimodal tool results use the official pipeline.
- Client thinking-budget controls, explicit session headers, CLI synchronization and raw model labels use official implementations.
- Official protocol error envelopes preserve the actual upstream message and response under `upstream_error`, alongside gateway diagnostics. HTTP failure status remains the last actual attempt status.
- Removed obsolete adapter recovery-prompt injection and tool-result compressor. Signature recovery continues through bounded account rotation rather than repeating the same account.
- The one-time thinking-cache cleanup suggestion is retained. Upgrading does not automatically delete historical thinking data.

## Retained Lee behavior

The upstream implementation is not equivalent to the following operational requirements, so these remain: initial attempt plus five different eligible accounts; inclusive model quota threshold and response-triggered refresh; conservative weekly/five-hour quota constraints; cumulative output usage and explicit empty-stream errors; configured aliases before model variants; transitive account ranking; proxy-binding hot reload; header and pre-response deadlines; dashboard window explanations and confirmed switches.

Existing data mounts, credentials, proxy bindings, model mappings, unlimited historical-log retention and deferred optional indexes must be preserved during deployment. See the corresponding ACCOUNT_FAILOVER, QUOTA_AND_STABILITY, UPSTREAM_TIMEOUTS and DASHBOARD_QUOTA documents for limits.

## Validation and build

Rust 1.96 is pinned for the new yaml-rt dependency. Frontend tests/build, portable protocol regressions, full application compilation and focused compatibility/database tests gate image publication. Runtime checks must use the published image and inspect real JSON/SSE output and usage before deployment.

## Large historical thinking stores

Official 4.8.1 log decoration attempted leading-wildcard searches over thought text and session keys while holding the shared thinking database mutex. On a 12 GB store, a cache miss caused a full scan and blocked Tokio workers, including health requests. This is separate from protocol-level signature recovery.

Lee.3 keeps log decoration in memory: preserve upstream signatures, otherwise consult the existing tool/session signature cache. A missing diagnostic signature remains absent; no unrelated historical signature is guessed. The official protocol pipeline and its indexed causal/fingerprint lookups remain intact, and historical caches are preserved. Removing byte slicing also avoids panics on multibyte thinking text. Regression coverage holds the SQLite mutex while formatting Chinese thinking text to ensure response logging does not wait for the database.

# Claude 5.5 upstream v4.9.6 scoped migration

**Goal:** Adopt upstream Claude Opus/Sonnet 5.5 catalog and catalog-driven routing, preserving strict account eligibility and existing stability fixes.

**Architecture:** Official model metadata supplies physical tiers, adaptive-thinking support and output limits. Shared routing selects a real tier; token selection requires positive exact-model quota. The shared inbound pipeline enforces adaptive thinking across all four protocols.

**Tech Stack:** Rust, serde JSON catalog, existing headless HTTP service, GitHub Actions and Docker Compose.

**Global Constraints:** Start from origin/beta f826515. Preserve Opus bare alias default high, Gemini mappings, paid Pro/Ultra eligibility, prefix normalization, all stability/cache fixes, API keys, passwords, data volumes and ports. No concurrency testing. User explicitly requested no new deployment backup; retain existing old image for rollback. Do not import the full upstream token manager or desktop code.

## Tasks

- [x] Import the six v4.9.6 Claude 5.5 catalog entries; attribute upstream af5b791/fd9c30c/4e8487d.
- [x] Replace Opus-only tier routing with catalog-driven Claude routing; default Sonnet to medium and preserve explicit tiers and Opus high.
- [x] Generalize strict account filtering without allowing normalized Claude quota or another tier/family to establish capability.
- [x] Use shared pipeline metadata for adaptive thinking and the 128000 output ceiling; test both model families and all protocol adapters.
- [ ] Run focused Rust tests, fmt/clippy, Web build and independent read-only review; prepare beta.4 and bilingual release notes.
- [ ] Open and merge a reviewed beta PR after its checks pass; await exact-commit image CI.
- [ ] Deploy the verified digest without new backups; verify revision, health, model directory and minimal eligible-model requests. Report unavailable capabilities separately.

## Source scope

Accept: six entries in `official_models.json`, metadata-driven tier discovery/default selection concepts, official output limits. Adapt: generic routing only for Claude so existing Gemini routes do not change; Opus default remains high. Retain: strict exact quota + tier eligibility and numeric-thinking-budget removal. Exclude: upstream capability-unknown account fallback, unrelated token-manager rewrites, desktop changes and whole-release merge. This remains 4.9.1-lee.1-beta.4, not a full v4.9.6 upgrade.

## Local verification

Passed: Claude 5.5 regressions (7), existing Opus regressions (25), catalog (18), token manager (56), model specs (10), isolated request compatibility (7), retry/stream compatibility (40), usage pipeline (4), Rust fmt/clippy and Web build. Some test groups overlap. Existing clippy/build warnings remain. The six catalog entries match v4.9.6; no unrelated catalog entries are replaced.

Deployment preflight found 582 account records and exactly one account with positive Opus/Sonnet 5.5 quotas, but that account is proxy-disabled. Do not enable it as part of this release. A successful health/model-directory check will not establish provider availability.

# Account list management

Web account deletion previously reloaded the entire token pool, then fetched the complete list again. Single and bulk deletion now invalidate only deleted accounts in the pool; desktop commands follow the same rule. The Web disk operation runs on a blocking worker. No remaining account needs OAuth refresh or a full pool rebuild for deletion.

The UI waits for successful deletion before removing rows. Failed requests retain rows; a late list response cannot restore deleted rows. Deleting the current account reads its confirmed replacement. The confirmation dialog shows progress and prevents duplicate submissions. Pagination clamps when the final row on a page is removed.

The independent status selector supports all, forbidden/403, disabled (account or proxy), and their union. It combines with email search and subscription tier, clears hidden selections and resets pagination. Verification-required and temporary 429 cooldowns are not mislabeled as forbidden accounts.

Validation includes store races/failure handling, browser fixtures and actual single/bulk deletion against disposable account copies. Production accounts are not deleted by these tests.

# Rust staging permanent deletion (#16)

The public `/api/staging` adapter keeps #8 restore requests unchanged and adds
`delete-staged` and `clear-staging`. Both use an empty `person`, an immutable
UUID request and a saved plan version. Selected deletion accepts 1–500 exact
`{vid,path,version}` selections. Full clearing accepts no selections: Rust scans
all staged ordinary files independently of search and 100-row list pages.
Apply requires the top-level exact `confirmation`: `永久删除` or `清空回收站`.
These action names deliberately adapt the uncommitted behavioral reference's
`delete`/`clear`; there is no old Node/Python execution fallback.

The staging page reuses existing Card, Button, Dialog and the already integrated
ReUI Input Group. Search retains selections; select-all reads matching pages
and refuses an incomplete selection above the selected-delete limit. The clear
preview always shows its full scope. Details retain actual video playback and
single-file restore. Submitted deletion plans persist a browser recovery hint;
refresh queries the same durable operation and never resubmits deletion.

## Durable execution

`staging_delete` runs inside the shared Engine, worker, Store, authority lock,
ownership markers and review write gate. Its `StopIrreversible` intent is not a
reversible move. The plan records each source's identity/permissions, exact
origin, affected JSON projections and native local/Peach deltas. Each item has
a distinct native database receipt (`<operation>:delete:<index>`). Original
videos, stable IDs, faces/thumbnails and unrelated assets/relationships remain.
Only the actual staged asset gets `disposal = COALESCE(disposal, 'trash')`.
History and health records match actual paths; source-origin IDs are audit
information and never authority to remove a canonical duplicate's references.

Before unlink the journal contains the complete intent and recovery material,
and the Peach commit must be confirmed. The namespace syscall and parent sync
use the shared durable mutation-intent/resolved protocol. Local receipts and
scoped JSON updates complete forward after the byte boundary. A failed item
stops the batch; prior successful items remain complete and later items remain
unexecuted. Deleted bytes are never described as rolled back or restored.
Unknown mutation responses and unresolved syscall journals retain the gate
across restart and repeated reconciliation, even if paths look normal. A deletion
intent with no media-mutation marker proves the syscall never started and can
resume once under recovered authority after identity/permission checks. A known
completed unlink can finish local records after repair. Recovery never starts
later untouched items from an interrupted batch.

JSON versioning covers only affected history, origins, overrides and health
records, preserving unrelated background progress and merging it at write time.
A source/related row/permission change rejects an old preview. Full clear also
rejects added files. Symlinks, FIFOs, traversal and incomplete full scans fail
closed; scan limits are errors, never truncated successful clear plans.
Legacy staging/person receipts and archive manifests are read-only provenance;
unknown origins stay unknown. Pending legacy staging work blocks new disposal.
Old IDs cannot be adopted. Journal schema 4 prevents an older reversible schema
3 executor from opening or replaying pending irreversible operations.

## Isolated verification

Native executable tests use marked `/tmp/face-admin-person-fileops-*` roots,
actual bytes/SQLite and no Python ledger executor. `tests/http-rust-staging-delete.py`
covers selected deletion, full 103-file clear, filtered paging, stale guards,
actual special files, unrelated health/task/history progress, partial stop,
legacy receipts/fences, canonical hardlinks/assets/thumbnail/index preservation,
real SQLite trigger failures after unlink, native faults, ten actual SIGKILL
windows, EIO/ESTALE/ETIMEDOUT after/delayed unlink, restart and disconnected HTTP
waiters. Set `FILEOPS_OLD_BINARY` to a schema-3 build for actual downgrade refusal.

`tests/http-rust-staging-delete-gateway.py` uses the dedicated seeded
`/tmp/face-admin-person-fileops-staging-16` fixture and ports 19002/19003. It checks
Next adapters, #8 restore, both confirmation texts, receipt replay and actual
HTTP 206 playback after canonical duplicate disposal. Browser evidence covers
1280px desktop, 390×844, reduced motion, search-preserved selection, clear scope,
keyboard submission, Escape/focus restoration, sample decoding at 960px and
refreshing a completed receipt. The existing video-health panel's unconfigured
fixture status is separate from this deletion workflow.

No production files were deleted or cleared. File-size totals describe logical
selected bytes, not guaranteed disk-space reclamation (hardlinks/NFS snapshots).
Production/NFS permissions and #6/#11 joint rollout remain central migration
checks; these local fixtures are not NFS deployment evidence.

## Operator handoff for ambiguous mutation results

An ambiguous result requires explicit operator recovery; automatic replay stays
blocked. This version deliberately exposes no timeout, PID-based unlock or
"force delete" endpoint. Do not remove `.review-write.lock` to bypass it.

1. Keep the operation ID and its `/api/staging?id=<id>` receipt. Preserve the
   local `file-operations-rust/operations.sqlite3` database and its WAL with a
   consistent SQLite backup, the gate/routing markers and each affected store.
2. Export the operation's immutable `operations.plan`, `operations.evidence`,
   `operations.error` and ordered `steps` rows. Evidence includes source/parent
   identity, scope, per-item snapshots and the unresolved mutation boundary.
3. For each `<id>:delete:<index>`, inspect native `_face_admin_person_ops` in
   Peach and `_face_admin_rust_commits` in faces/processing databases. Compare
   exact path/disposal/related-row images and the scoped JSON records. Identify
   which item is confirmed complete, uncertain or never started.
4. Have the storage operator establish that the original storage client/server
   has no pending delayed mutation, using storage-side evidence. Elapsed time,
   process death, an inode/path observation or a currently absent path alone is
   insufficient. Preserve existing bytes while obtaining this evidence.
5. Hand off the evidence bundle for a separately reviewed, explicit recovery
   decision. Any resolution must be durably attached to the original operation,
   validate current identities/store receipts and preserve successful items;
   it must not silently authorize another unlink. Already deleted content can
   only come from an independent backup, not from journal rollback.

Ordinary known-outcome interruptions use the existing reconcile request. It can
finish confirmed unlink records or continue a proven never-started syscall,
without clearing ambiguous mutation evidence.

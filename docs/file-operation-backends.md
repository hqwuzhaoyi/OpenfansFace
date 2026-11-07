# File operation execution interface

The first migrated capability is a single `rename-file`. `lib/file-operation-backend.ts` is the interchangeable module used by the existing person-manage route. The page continues sending the same preview/apply envelopes and receiving the same `PersonFilePlan` and final `PersonFileResult`.

## Selection and ownership

`FILE_OPERATIONS_BACKEND` defaults to `legacy`. Selecting `rust` affects **only** `rename-file` in this first slice; other person actions and all other business entry points retain their existing implementation. Rust dispatch also requires `FILE_OPERATIONS_ENABLE_RUST=1` and `FILE_OPERATIONS_RUST_URL`. Optional `FILE_OPERATIONS_RUST_TOKEN` becomes a bearer header. Production remains on the default until cutover verification.

Every person operation is claimed in `FACES_BASE/file-operation-routing/<UUID>.json` with immutable `id`, `backend` and `action`. Preview supplies an ID if the caller omitted it. Legacy core preview/apply also claims ownership, so batch orchestration cannot bypass the gate. Older execution journals without ownership records remain legacy-owned. Conflicting ownership, malformed records, or an unknown backend fail closed. A failed network call never dispatches the same request to the other provider.

Ownership is not a replacement for the existing preview version. Each provider must bind the confirmed request/version to its own durable execution record, reject changed scope for the same ID, and return an existing successful receipt on repetition. The public request and preview version are not rewritten by the dispatch module.

## Native transport contract for ticket #2

The Rust adapter calls `POST <FILE_OPERATIONS_RUST_URL>/v1/person-files` with the existing `{mode: "preview", request}` or `{mode: "apply", request, version}` envelope. Preview returns a `PersonFilePlan` with the same operation ID. Apply returns a **final HTTP 200** `PersonFileResult`; HTTP 202 admission is not interpreted as success by the existing page. The executor must persist admission and continue independently if this waiting HTTP connection disappears.

`GET <FILE_OPERATIONS_RUST_URL>/v1/person-files?id=<UUID>` returns `{id,status,result?,error?}`. The gateway exposes this at `GET /api/persons/manage?id=<UUID>` and adds `backend`. Result queries follow persisted ownership even after dispatch configuration changes. A legacy operation claimed but not started returns `status: "not_started"`; completed operations return the exact persisted result. Upstream errors retain their HTTP status; transport failure reports uncertainty without retrying the mutation. Rust success invalidates the existing Next read caches before returning.

The Rust provider must also validate or atomically create the same ownership record for direct requests. It must acquire **the same** `FACES_BASE/.review-write.lock` with exclusive creation before mutations, and retain it on unknown/recovery-needed outcomes. Its process PID is not proof of lock ownership across containers; no implementation may delete a lock merely because a PID looks absent. The gateway must not hold this lock while calling Rust, since the provider owns it for the actual transaction. Legacy writers continue acquiring it themselves.

Cutover must drain or reconcile in-flight operations before changing ownership configuration. A restart or dispatch-flag change cannot transfer an existing operation ID to a new provider. Final single-writer retirement may absorb these compatibility records only after their outstanding receipts remain queryable.

## Shared writer inventory at the accepted baseline

| Data / side effect | Existing writer and interface | Coordination / migration obligation |
| --- | --- | --- |
| Person files, directories, path and identity metadata | `person-file-operations`, through person manage and batch | Shared review lock; core also claims per-ID legacy ownership. Native parity must cover SQLite, JSON and Peach. |
| Archive files, duplicates, queue/history and archive receipts | Plan apply/resolve/progress interfaces | Shared review lock; remains legacy until its ticket migrates it. |
| Archive recovery | Legacy plan rollback interface | Now acquires the review lock. Existing overwrite-capable rename and shared Peach temporary payload are legacy hazards; replace/retire before Rust production cutover, not a parity target. |
| Confirmation queue, review decisions, manual person remembrance, optional watermark mapping | Plan queue and claims decision interfaces, people catalog called from them | Shared review lock at caller; native queue/confirmation ticket owns the complete write transaction. |
| Manual account links | Person accounts interface | Shared review lock; must use the future shared writer coordinator. External profile cache is separate read-model data. |
| Rejected recommendations and confirmed watermark rules | Recommendations and watermark mapping interfaces | Shared review lock; preserve manual versus inferred evidence. |
| Person-template overrides | Calibrator `identity_pick`, reached through the calibration proxy | **Bypass at baseline:** sidecar writes directly and launches identification. Coordinate persistence before cutover; model execution may remain Python. |
| Video index discovery and analysis state | Processing worker, processing interfaces | Independent SQLite transactions. Preserve unrelated progress; protect only records affected by a file operation. |
| Video-health reports | Video-health report interface | Shared review lock for reports; maintenance scanner has a separate health lock and does not authorize media moves. |
| Analysis caches, progress and external profile cache | Watermark/processing workers, creator-directory loader | Separate operational/cache records; do not accidentally turn this migration into an analysis-algorithm rewrite. |
| Imported media and NAS ownership/ACL | External importers and permission provisioning | Not governed by the web mutex. Native execution must validate paths/fingerprints and actual effective permissions. |

Read `person-file-metadata.ts` for the authoritative transforms: aliases, manual accounts, reviewed clues, templates, rejections, pending queue, archive history, path overrides, report/claims/clusters, deleted-person markers and affected processing candidates. A generic recursive string replacement is not equivalent.

The checked-in staging baseline only exposes statistics. Uncommitted staging restoration/permanent-delete work in another worktree is **not** part of this baseline. Inventory its accepted contracts again before final cutover; do not copy unrelated work or leave a new write endpoint uncoordinated.

## HTTP acceptance harness

Use a fresh directory named `/tmp/face-admin-person-fileops-<unique>`. `tests/http-file-operation-backend.py --root <fixture> --seed --sample <test-video>` delegates to the existing marked person-operation fixture seed. Run Next using the **resolved** fixture paths for `FACES_BASE`, `MEDIA2_ROOT` and `PEACH_LEDGER_PATH` on a dedicated loopback port.

Run `tests/http-file-operation-backend.py --root <fixture> --url <loopback-url> --backend legacy` for the existing provider. The same scenario with `--backend rust` verifies the native provider through the same public HTTP seam once implemented. The script refuses unmarked roots, non-loopback hosts and production ports before writing.

The scenario asserts preview scope, competing-writer exclusion, unchanged source bytes, unrelated-file preservation, related local records, real SQLite ledger paths/relations, repeated-apply receipt identity, conflicting request rejection, result lookup and stable-ID Range playback. It does not assert implementation calls or require version-hash equality across providers. Existing comprehensive person-operation tests remain applicable as later migration slices expand capability coverage.

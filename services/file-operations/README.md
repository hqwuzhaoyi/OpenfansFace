# Native file operations — registered video rename

This service implements the Rust 02/03 vertical slice: one already indexed video renamed inside its existing person directory. Axum serves the existing person-file interface; a bounded dedicated worker performs the operation using Rust, rusqlite and directory-relative filesystem calls. It does not launch Node, Python, Docker or a shell. The Next page persists the immutable plan before submitting, then queries the same operation after disconnect, navigation or refresh.

Build with Rust 1.89+ (validated with 1.95):

```sh
cargo build --locked --manifest-path services/file-operations/Cargo.toml
```

Run with explicit existing storage paths. `FACES_BASE` and the Peach database directory, including its WAL/SHM siblings, must be on local storage. Media may be a mounted filesystem. Use the same resolved paths in Next and Rust so source fingerprints agree.

```sh
FACES_BASE=/absolute/local/base \
MEDIA2_ROOT=/absolute/media \
PEACH_LEDGER_PATH=/absolute/local/peach/ledger.db \
FILE_OPERATIONS_BIND=127.0.0.1:8930 \
services/file-operations/target/debug/face-file-operations
```

Next selects this service using `FILE_OPERATIONS_BACKEND=rust`, `FILE_OPERATIONS_ENABLE_RUST=1` and `FILE_OPERATIONS_RUST_URL=http://127.0.0.1:8930`. `FILE_OPERATIONS_RUST_TOKEN` must match in both processes when configured. Non-loopback binding requires a nonempty token. Default routing remains legacy. No production cutover is performed by building or testing this crate.

## HTTP contract

- `POST /v1/person-files` with `{mode:"preview",request}` returns the existing `PersonFilePlan`. The typed request requires a UUID v4 `id`, `action:"rename-file"`, `person`, one `{vid,path,version}` in `files`, and `filename`. The Next gateway supplies the ID. Unsupported actions/fields fail explicitly.
- Apply uses `{mode:"apply",request:plan.request,version:plan.version}` and waits for the exact terminal `PersonFileResult` with HTTP 200. There is no ambiguous 202 success. Losing the waiting HTTP connection does not cancel the worker.
- `GET /v1/person-files?id=...` returns durable `status`, `phase`, monotonically increasing `revision`, measured operation counts and the original `result` or failure. Next exposes this through `/api/persons/manage` and preserves routing ownership.
- `POST /v1/person-files` with `{mode:"reconcile",request,version?}` explicitly reconciles an admitted operation. For an absent or not-yet-admitted operation it durably fences that immutable ID; a later apply/preview cannot run. Passive GET never fences or cancels. A running worker finishes its actual I/O before handling reconciliation; closing the HTTP waiter does not revoke its execution authority.
- `GET /health` is liveness only; it does not declare a recovery-blocked writer ready for new work.

Preview and admission bind immutable request hashes and exact plan versions. Reusing an ID with a changed request/version returns 409. A completed request returns its stored result even after restart. A busy shared gate returns 409 without consuming the preview; retrying the same untouched plan is valid. Validation errors are JSON with `code`, Chinese `error`, HTTP `status`, and affected `path` where available.

## Ownership, steps and recovery handoff

`FACES_BASE/file-operations-rust/operations.sqlite3` is a local WAL database with `synchronous=FULL` and schema `user_version=2` (upgrades v1 in place). A newer schema refuses an older writer. `operations` stores request hash, full immutable plan, admission evidence, state, revision, result/error and timestamps. `steps` stores intent and confirmation at each external side-effect boundary. SQLite integers in before/after evidence use `{"$integer":"..."}` and blobs use `{"$blob":[...]}` to avoid lossy JavaScript numeric conversion.

The process holds an exclusive advisory lock on `file-operations-rust/executor.lock`. Schema v2 binds that authority to the persistent lock inode/device and rejects a replaced lock; this file must never be unlinked as a way to restart. The gate contains its state-directory authority and a random token; exact marker bytes/identity are persisted in `gates` before preflight, then included in admission evidence. A crash before that proof is durable leaves the marker for manual investigation; marker contents alone never prove ownership. Restart may recover this gate only while holding the original lifetime authority. A v1/foreign/replaced gate without verifiable ownership stays blocked. Direct requests also claim the same immutable `file-operation-routing/<id>.json` used by Next. Apply exclusively creates the shared `.review-write.lock` before revalidation/admission. The gate is removed only after successful completion or a known pre-admission validation failure, and only if its device/inode still identify the owned marker. No PID expiry or TTL steals either lock. SIGINT/SIGTERM drain the worker; abrupt termination leaves durable evidence and the shared marker.

The file step pins no-follow directory and source descriptors, checks the current fingerprint and parent identity, publishes an exclusive hard link, verifies both names identify the accepted inode, then removes the source. Fingerprints retain the existing SHA-256 contract over path/dev/inode/size/mtime-ns string values, excluding ctime. Same-directory rename does not need a cross-device copy. Unsupported hard-link/fsync behavior fails explicitly; there is no overwrite-capable fallback. Directory descriptors protect against path redirection; arbitrary concurrent external media editors still require operational coordination.

Local metadata preserves the rename-specific semantics in `lib/person-file-metadata.ts`: stable IDs and OCR/audit text remain unchanged; path fields, matching filename fields, archive history and path overrides update; external account profiles remain untouched. `faces.db` introspects optional columns. Affected processing rows update their location/data and become `excluded`; unrelated progress is excluded from the preview fingerprint. Each local database commits its own `_face_admin_rust_commits(id,signature,data)` receipt in the same transaction as its changes. JSON before/after bytes are durable before replacement and current bytes are checked under the shared gate.

Peach reserves `_face_admin_person_ops` under `BEGIN IMMEDIATE`, validates the scoped row snapshot again at commit, updates only `asset.path`, and records `committed` in that same transaction. The adapter snapshots full rows, rejects path ambiguity across locations and target conflicts, and preserves all role/source composite-key links and aliases. There is no asset/entity insertion or merge in this slice.

These stores are separate commit domains. Startup and explicit reconciliation inspect the source/destination inode identity, every local transaction receipt and exact affected before/after rows, every JSON before/after byte sequence, and Peach's native transaction state before any recovery mutation. Clean interrupted work resumes only missing effects. A committed Peach receipt is verified against actual rows and never treated as a failed commit merely because the response was lost. Unknown, unreadable, replaced or contradictory evidence remains `recovery_needed`, keeps the shared gate and blocks new work.

On a confirmed failure before Peach commits, the engine persists `compensation_intent`, fences Peach's prepared transaction as `cancelled`, restores JSON and each local database only against its recorded after-image, and restores the file using no-replace publication plus identity checks. Local receipts retain before/after images with `state:"rolled_back"` in the same restoring transaction. Interruptions during compensation resume that direction; a later POST cannot reapply it. Completed operations and rolled-back operations retain their result/error; a durable final receipt plus leftover exact owned gate is cleaned at startup. Gate acquired before durable admission is fenced and released only under proven authority, without executing the preview.

Query includes `revision`, actual `phase`, `io_state` (`idle`, `running`, or `unknown`), and the terminal receipt/error. Waiting is a browser concern, separate from these execution states. The browser stores the confirmed plan before POST, automatically opens its result on refresh, ignores older poll revisions, retains completion/errors until explicitly dismissed, and offers “停止等待” without claiming to stop I/O. A successful recovered receipt also invalidates Next's library/stat caches. No retry creates a fresh operation ID automatically.

## Extension contract for subsequent operations

- Schema v2 retains v1 `operations`, `steps` and native receipt shapes, and adds `authority`, `gates` and `fences`. New optional plan/evidence fields must remain readable for these historical rows. Native SQLite before/after values preserve integer/blob tags; do not round-trip them through JavaScript numbers.
- Admission evidence currently contains `{metadata:{documents,videos,jobs},ledger,source_identity,gate:{identity,marker}}`, bound by the immutable plan's metadata/ledger hashes. The gate and lifetime authority are independent of operation kind. Never derive takeover permission from marker PID or TTL.
- The accepted vertical is still one registered rename: `Plan.file()`, `Request.files.len()==1`, `PinnedMove`'s same-parent validation and current engine phase counts are intentionally singular. Multi-file/copy extensions must add stable per-item keys and persisted source/temp/published identities before each effect; repeated phase labels alone cannot identify an item. Keep the existing single-file evidence readable when introducing vectors.
- `PinnedMove::recover/position/compensate` verify accepted identity under no-follow directory descriptors and distinguish source-only, dual-name and destination-only states. Copy variants require separate owned-temp/content evidence; they cannot infer ownership from matching names or sizes.
- `_face_admin_rust_commits(id,signature,data)` stays transaction-local. `data` contains exact `before`/`after` rows and optionally `state:"rolled_back"`. `metadata::inspect_domain` and `compensate_domain` validate actual scoped rows; a third state blocks recovery. JSON documents contain `{name,before,after}` byte strings, independently reconciled.
- Peach `_face_admin_person_ops` uses `prepared`, `committed`, `cancelled`; committed data contains `{backend:"rust",kind:"rename-file",before,after_assets}`. Missing/prepared/cancelled states are verified against the accepted snapshot. New operation adapters must implement their own full scoped row verification before reusing this recovery coordinator.
- Forward phases are reserve, destination publication, source removal, faces/jobs transactions, each JSON replacement, ledger commit and final receipt, each with durable intent/confirmation. `compensation_intent` fixes rollback direction; compensation steps can be repeated only through evidence checks. Store final result before gate removal, and preserve the marker on any unknown outcome.

## Acceptance

```sh
cargo fmt --check --manifest-path services/file-operations/Cargo.toml
cargo clippy --locked --manifest-path services/file-operations/Cargo.toml --all-targets -- -D warnings
cargo build --locked --manifest-path services/file-operations/Cargo.toml
python3 tests/http-rust-file-operations.py --port 18977
python3 tests/http-rust-recovery.py --port 18977
```

The recovery suite uses actual SIGKILL at 31 forward/JSON checkpoints and 15 compensation checkpoints, counts real SQLite updates to detect replay, and checks late submissions, different scope, lost responses, unknown ledger outcomes, foreign source/target/JSON/rows/gates, final-receipt gate cleanup and replaced executor authority.

The native HTTP suite launches the compiled service with `PATH=/no-executables` and isolated marked fixtures. It checks no-overwrite/symlink/root/extension guards, real Peach composite keys and foreign keys, related metadata and task conflicts, unrelated task progress, native commit failure, idempotency, process ownership, disconnect continuation and crash checkpoints before/after file publication, each SQLite commit, JSON replacement and final response. It preserves fixtures under `/tmp` for inspection.

For the unchanged public Next seam, seed a fresh marked fixture with the observed Peach core schema:

```sh
python3 tests/http-file-operation-backend.py \
  --root /tmp/face-admin-person-fileops-example --seed --native-ledger \
  --sample /path/to/test-video.mp4
```

Run Rust and Next on independent loopback ports using that fixture's resolved `base`, `media` and `ledger.db`, then run:

```sh
python3 tests/http-file-operation-backend.py \
  --root /tmp/face-admin-person-fileops-example \
  --url http://127.0.0.1:18972 --backend rust
```

Fault injection is enabled only with `FILE_OPERATIONS_FIXTURE_ROOT` resolving to a correctly marked `/tmp/face-admin-person-fileops-*` root containing **all** configured storage paths, with loopback binding. A `rust-fault.json` file supplies `{phase,mode:"exit"|"error"|"pause",milliseconds?}`; production has no enabled hook. The phase names are the journal checkpoint names in `engine.rs`; document-specific hooks use `json_replace_intent:<name>` and `json_replaced_unrecorded:<name>`. Pause mode writes `rust-fault-reached.json` for external SIGKILL tests, only inside a validated fixture. Exit code 86 is intentional fixture termination.

The sanitized core Peach schema fixture is structural evidence, not full deployment compatibility. It omits application views/FTS, maintenance triggers and other tables. Full isolated Peach deployment, NFS UID/GID/ACL behavior and production cutover remain subsequent acceptance work. Unregistered files, cross-directory moves, identity operations and other write routes are outside this ticket.

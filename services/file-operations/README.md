# Native file operations — registered video rename

This service implements the Rust 02 vertical slice: one already indexed video renamed inside its existing person directory. Axum serves the existing person-file interface; a bounded dedicated worker performs the operation using Rust, rusqlite and directory-relative filesystem calls. It does not launch Node, Python, Docker or a shell. The Next page and its immediate submitting state remain unchanged.

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
- `GET /health` is liveness only; it does not declare a recovery-blocked writer ready for new work.

Preview and admission bind immutable request hashes and exact plan versions. Reusing an ID with a changed request/version returns 409. A completed request returns its stored result even after restart. A busy shared gate returns 409 without consuming the preview; retrying the same untouched plan is valid. Validation errors are JSON with `code`, Chinese `error`, HTTP `status`, and affected `path` where available.

## Ownership, steps and recovery handoff

`FACES_BASE/file-operations-rust/operations.sqlite3` is a local WAL database with `synchronous=FULL` and schema `user_version=1`. A newer schema refuses an older writer. `operations` stores request hash, full immutable plan, admission evidence, state, revision, result/error and timestamps. `steps` stores intent and confirmation at each external side-effect boundary. SQLite integers in before/after evidence use `{"$integer":"..."}` and blobs use `{"$blob":[...]}` to avoid lossy JavaScript numeric conversion.

The process holds an exclusive advisory lock on `file-operations-rust/executor.lock`. Direct requests also claim the same immutable `file-operation-routing/<id>.json` used by Next. Apply exclusively creates the shared `.review-write.lock` before revalidation/admission. The gate is removed only after successful completion or a known pre-admission validation failure, and only if its device/inode still identify the owned marker. No PID expiry or TTL steals either lock. SIGINT/SIGTERM drain the worker; abrupt termination leaves durable evidence and the shared marker.

The file step pins no-follow directory and source descriptors, checks the current fingerprint and parent identity, publishes an exclusive hard link, verifies both names identify the accepted inode, then removes the source. Fingerprints retain the existing SHA-256 contract over path/dev/inode/size/mtime-ns string values, excluding ctime. Same-directory rename does not need a cross-device copy. Unsupported hard-link/fsync behavior fails explicitly; there is no overwrite-capable fallback. Directory descriptors protect against path redirection; arbitrary concurrent external media editors still require operational coordination.

Local metadata preserves the rename-specific semantics in `lib/person-file-metadata.ts`: stable IDs and OCR/audit text remain unchanged; path fields, matching filename fields, archive history and path overrides update; external account profiles remain untouched. `faces.db` introspects optional columns. Affected processing rows update their location/data and become `excluded`; unrelated progress is excluded from the preview fingerprint. Each local database commits its own `_face_admin_rust_commits(id,signature,data)` receipt in the same transaction as its changes. JSON before/after bytes are durable before replacement and current bytes are checked under the shared gate.

Peach reserves `_face_admin_person_ops` under `BEGIN IMMEDIATE`, validates the scoped row snapshot again at commit, updates only `asset.path`, and records `committed` in that same transaction. The adapter snapshots full rows, rejects path ambiguity across locations and target conflicts, and preserves all role/source composite-key links and aliases. There is no asset/entity insertion or merge in this slice.

These stores are separate commit domains. After durable admission, any uncertain step is `recovery_needed` with the marker retained; the service does not guess compensation. On restart, admitted/in-flight operations become `recovery_needed`, keep their phase and steps, and block new writes. Reads and successful historical receipts remain available. **Rust 03 owns reconciliation, guarded compensation and releasing a reconciled gate.** Inspect the journal, both file names, per-database receipts and JSON before/after values; do not manually delete a marker to force a retry. A crash after durable final receipt returns that receipt on query; a leftover gate still awaits reconciliation.

## Acceptance

```sh
cargo fmt --check --manifest-path services/file-operations/Cargo.toml
cargo clippy --locked --manifest-path services/file-operations/Cargo.toml --all-targets -- -D warnings
cargo build --locked --manifest-path services/file-operations/Cargo.toml
python3 tests/http-rust-file-operations.py
```

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

Fault injection is enabled only with `FILE_OPERATIONS_FIXTURE_ROOT` resolving to a correctly marked `/tmp/face-admin-person-fileops-*` root containing **all** configured storage paths, with loopback binding. A `rust-fault.json` file supplies `{phase,mode:"exit"|"error"|"pause",milliseconds?}`; production has no enabled hook. The phase names are the journal checkpoint names in `engine.rs`. Exit code 86 is intentional fixture termination.

The sanitized core Peach schema fixture is structural evidence, not full deployment compatibility. It omits application views/FTS, maintenance triggers and other tables. Full isolated Peach deployment, NFS UID/GID/ACL behavior and production cutover remain subsequent acceptance work. Unregistered files, cross-directory moves, identity operations, automatic recovery and other write routes are outside this ticket.

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

The sanitized core Peach schema fixture is structural evidence, not full deployment compatibility. It omits application views/FTS, maintenance triggers and other tables. Full isolated Peach deployment and production cutover remain subsequent acceptance work. NFS UID/GID/ACL acceptance is documented below. Unregistered files, cross-directory moves, identity operations, automatic recovery and other write routes are outside this ticket.

## NFS acceptance and deployment identity (Rust 04)

`permissions.rs` uses effective-identity `accessat` through no-follow directory descriptors. It checks readable source content and writable/searchable source and destination parents, or the nearest existing destination ancestor. Both a fresh/cached preview and final pre-admission validation run these checks. A denial returns HTTP 403 with `step` (`source_directory`, `destination_directory`, `source_read`) and the concrete blocking `path`; real storage errors remain 503 rather than being mislabeled permissions or empty directories. Existing serialized errors without `step` remain readable. An admitted failure retains the error step and recovery gate.

Build Linux AMD64 **on a development/build machine**, then transfer the image to the NFS client; do not compile on the resource-limited service host:

```sh
docker build --platform linux/amd64 -f services/file-operations/Dockerfile \
  --target acceptance -t face-file-operations:nfs-ticket04 .
docker save face-file-operations:nfs-ticket04 > /tmp/face-rust-nfs-image.tar
```

The `runtime` target is the deployable service; it defaults to `USER 1000:1000`. `RUST_IMAGE` and `RUNTIME_IMAGE` build arguments permit a trusted mirror or locally cached compatible Debian image. The `acceptance` target adds only the fixture driver and ACL inspector. Production database directories must remain local, with WAL/SHM siblings and permissions for the chosen service UID. This Dockerfile alone does not authorize production rollout or change export protection.

Provision one new child under `<media-export>/.face-admin-rust-fixtures/<unique-run>` **outside all watched media directories**. Write `.rust-nfs-fixture` containing exactly `rust nfs acceptance\n`. Only that owned child may be granted service UID 1000 access/default ACL; never change the export root, existing datasets, root-squash or existing file owners. Inspect `getfacl -pn` before and after. For example, on the NAS after creating the marked new child, `setfacl -m u:1000:rwx,d:u:1000:rwx,d:m:rwx "$owned_fixture"` grants that isolated directory's service identity. Other inherited ACL entries must remain intact.

After loading the image on the Linux NFS client, run the checked-in wrapper as the host's UID 1000 (requires `sudo -n docker` and local-directory ownership permission):

```sh
tests/run-rust-nfs.sh /tmp/face-admin-person-fileops-nfs-unique \
  /mounted/export/.face-admin-rust-fixtures/unique-run 1000:1000 ESTALE
```

Use a **fresh pair of local/NFS roots per run**. Repeat with `0:0` if matching a current root container whose export maps root to 1000, and with `EIO` / `ETIMEDOUT` to cover I/O and unavailable-mount errors. The wrapper mounts only those fixtures, uses no container network and exposes no port. SQLite stays under the local root; only `media` is NFS. It preserves artifacts for inspection and never recursively cleans a supplied path.

The native `storage-probe` executable tests mkdir/default inheritance, hard link, source unlink, no-replace rollback, synced exclusive copy, rejection of an occupied copy destination, and concrete destination/nearest-ancestor permission errors. The HTTP driver then verifies source permission denial in preview/apply, real registered-video rename with native metadata/Peach receipts, duplicate result lookup, bounded queue overload and responsive status while the writer is paused. No Python operation is used as evidence of a Rust media mutation; Python only seeds/checks fixtures.

The marked fixture checkpoint supports `io_error` with only `EIO`, `ESTALE` or `ETIMEDOUT`. These are deterministic injected kernel-style failures, **not a claim that the shared production mount or network was disconnected**. A blocked kernel call can still outlive an HTTP request: the single worker and 32-slot queue bound admitted execution, status reads run separately, and no timeout is interpreted as cancelled I/O. The filesystem probe's copy/rollback is primitive acceptance; production cross-filesystem moves and crash recovery belong to their own tickets.

### Recorded isolated rehearsal — 2026-11-08

The image `sha256:44055bb5a04f9bdb9981d1f94f92e37f8b1cde00a77bd5f6d2f23bd614ac2e8d` was built locally for Linux AMD64 with Rust 1.95, the public ECR Rust mirror and a cached Debian Bookworm Node image as the runtime-base override. Node was not invoked. It ran on the actual NFS client in three disposable containers with synthetic local ext4 databases and marked NFS media. No production service was restarted and no production media/database was written.

| Container UID:GID | NAS file UID:GID | Injected error | Result |
| --- | --- | --- | --- |
| 0:0 (matching existing web service) | 1000:1000 via root-squash | ESTALE | All primitive/HTTP checks passed; failure retained bytes and recovery gate |
| 1000:1000 (runtime default) | 1000:1000 | EIO | Same |
| 1000:1000 | 1000:1000 | ETIMEDOUT | Same |

Every run rejected 14 requests beyond the single active worker plus 32 queue slots; status/liveness remained responsive, and duplicate successful requests returned the same receipt. NAS-side `getfacl -pn` confirmed `user:1000:rwx` and `default:user:1000:rwx` on Rust-created directories without a post-creation chmod. The NFS client's `getfacl` showed mode entries only, so client output alone was **not** used to claim ACL inheritance. The observed mount was NFS 4.2 with `root_squash`, anonymous UID/GID 1000, `soft,timeo=50,retrans=2`; these export/mount settings were inspected, not changed.

All pre-existing native HTTP scenarios, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and the Linux release build passed. This proves the isolated same-directory rename and storage primitives under the actual identity/mount mapping. It does not prove full Peach application compatibility, cross-device move recovery, hard-mounted NFS timeout behavior, or production cutover readiness.

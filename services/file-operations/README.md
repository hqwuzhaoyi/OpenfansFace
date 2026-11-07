# Native file operations — person rename and selected moves

This service implements Rust 02/03/07: single-video rename and selected moves of 1–500 videos between existing people or their subfolders, including stable registration for unindexed files. Axum serves the existing person-file interface; a bounded dedicated worker performs the operation using Rust, rusqlite and directory-relative filesystem calls. It does not launch Node, Python, Docker or a shell. The Next page persists the immutable plan before submitting, then queries the same operation after disconnect, navigation or refresh.

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

- `POST /v1/person-files` with `{mode:"preview",request}` returns the existing `PersonFilePlan`. The typed request requires a UUID v4 `id`, `action`, `person`, and `{vid,path,version}` selections. `rename-file` requires one selection and `filename`; `move-files` accepts 1–500 selections, `target_person`, and optional relative `target_folder`. The Next gateway supplies the ID. Unsupported actions/fields fail explicitly.
- Apply uses `{mode:"apply",request:plan.request,version:plan.version}` and waits for the exact terminal `PersonFileResult` with HTTP 200. There is no ambiguous 202 success. Losing the waiting HTTP connection does not cancel the worker.
- `GET /v1/person-files?id=...` returns durable `status`, `phase`, monotonically increasing `revision`, measured operation counts and the original `result` or failure. Next exposes this through `/api/persons/manage` and preserves routing ownership.
- `POST /v1/person-files` with `{mode:"reconcile",request,version?}` explicitly reconciles an admitted operation. For an absent or not-yet-admitted operation it durably fences that immutable ID; a later apply/preview cannot run. Passive GET never fences or cancels. A running worker finishes its actual I/O before handling reconciliation; closing the HTTP waiter does not revoke its execution authority.
- `GET /health` is liveness only; it does not declare a recovery-blocked writer ready for new work.

Preview and admission bind immutable request hashes and exact plan versions. Reusing an ID with a changed request/version returns 409. A completed request returns its stored result even after restart. A busy shared gate returns 409 without consuming the preview; retrying the same untouched plan is valid. Validation errors are JSON with `code`, Chinese `error`, HTTP `status`, and affected `path` where available.

## Ownership, steps and recovery handoff

`FACES_BASE/file-operations-rust/operations.sqlite3` is a local WAL database with `synchronous=FULL` and schema `user_version=3` (upgrades v1/v2 in place). A newer schema refuses an older writer. `operations` stores request hash, full immutable plan, admission evidence, state, revision, result/error and timestamps. `steps` stores intent and confirmation at each external side-effect boundary. SQLite integers in before/after evidence use `{"$integer":"..."}` and blobs use `{"$blob":[...]}` to avoid lossy JavaScript numeric conversion.

The process holds an exclusive advisory lock on `file-operations-rust/executor.lock`. Schema v2 binds that authority to the persistent lock inode/device and rejects a replaced lock; this file must never be unlinked as a way to restart. The gate contains its state-directory authority and a random token; exact marker bytes/identity are persisted in `gates` before preflight, then included in admission evidence. A crash before that proof is durable leaves the marker for manual investigation; marker contents alone never prove ownership. Restart may recover this gate only while holding the original lifetime authority. A v1/foreign/replaced gate without verifiable ownership stays blocked. Direct requests also claim the same immutable `file-operation-routing/<id>.json` used by Next. Apply exclusively creates the shared `.review-write.lock` before revalidation/admission. The gate is removed only after successful completion or a known pre-admission validation failure, and only if its device/inode still identify the owned marker. No PID expiry or TTL steals either lock. SIGINT/SIGTERM drain the worker; abrupt termination leaves durable evidence and the shared marker.

The historical indexed-rename file step pins no-follow directory and source descriptors, checks the current fingerprint and parent identity, publishes an exclusive hard link, verifies both names identify the accepted inode, then removes the source. Fingerprints retain the existing SHA-256 contract over path/dev/inode/size/mtime-ns string values, excluding ctime. Same-directory rename does not need a cross-device copy. Unsupported hard-link/fsync behavior fails explicitly; there is no overwrite-capable fallback. Directory descriptors protect against path redirection; arbitrary concurrent external media editors still require operational coordination.

Local metadata preserves the rename-specific semantics in `lib/person-file-metadata.ts`: stable IDs and OCR/audit text remain unchanged; path fields, matching filename fields, archive history and path overrides update; external account profiles remain untouched. `faces.db` introspects optional columns. Affected processing rows update their location/data and become `excluded`; unrelated progress is excluded from the preview fingerprint. Each local database commits its own `_face_admin_rust_commits(id,signature,data)` receipt in the same transaction as its changes. JSON before/after bytes are durable before replacement and current bytes are checked under the shared gate.

For the historical indexed-rename lane, Peach reserves `_face_admin_person_ops` under `BEGIN IMMEDIATE`, validates the scoped row snapshot again at commit, updates only `asset.path`, and records `committed` in that same transaction. The adapter snapshots full rows, rejects path ambiguity across locations and target conflicts, and preserves all role/source composite-key links and aliases. That compatibility lane does not insert assets/entities or merge people.

These stores are separate commit domains. Startup and explicit reconciliation inspect the source/destination inode identity, every local transaction receipt and exact affected before/after rows, every JSON before/after byte sequence, and Peach's native transaction state before any recovery mutation. Clean interrupted work resumes only missing effects. A committed Peach receipt is verified against actual rows and never treated as a failed commit merely because the response was lost. Unknown, unreadable, replaced or contradictory evidence remains `recovery_needed`, keeps the shared gate and blocks new work.

On a confirmed failure before Peach commits, the engine persists `compensation_intent`, fences Peach's prepared transaction as `cancelled`, restores JSON and each local database only against its recorded after-image, and restores the file using no-replace publication plus identity checks. Local receipts retain before/after images with `state:"rolled_back"` in the same restoring transaction. Interruptions during compensation resume that direction; a later POST cannot reapply it. Completed operations and rolled-back operations retain their result/error; a durable final receipt plus leftover exact owned gate is cleaned at startup. Gate acquired before durable admission is fenced and released only under proven authority, without executing the preview.

Query includes `revision`, actual `phase`, `io_state` (`idle`, `running`, or `unknown`), and the terminal receipt/error. Waiting is a browser concern, separate from these execution states. The browser stores the confirmed plan before POST, automatically opens its result on refresh, ignores older poll revisions, retains completion/errors until explicitly dismissed, and offers “停止等待” without claiming to stop I/O. A successful recovered receipt also invalidates Next's library/stat caches. No retry creates a fresh operation ID automatically.

## Extension contract for subsequent operations

- Schema v2 retains v1 `operations`, `steps` and native receipt shapes, and adds `authority`, `gates` and `fences`. New optional plan/evidence fields must remain readable for these historical rows. Native SQLite before/after values preserve integer/blob tags; do not round-trip them through JavaScript numbers.
- Admission evidence currently contains `{metadata:{documents,videos,jobs},ledger,source_identity,gate:{identity,marker}}`, bound by the immutable plan's metadata/ledger hashes. The gate and lifetime authority are independent of operation kind. Never derive takeover permission from marker PID or TTL.
- Historical registered-rename plans retain their single-file shape and same-parent guard. Native multi-file plans carry indexed scopes and source/temp/publication evidence; repeated phase labels alone never identify an item.
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

The sanitized core Peach schema fixture is structural evidence, not full deployment compatibility. It omits application views/FTS, maintenance triggers and other tables. Full isolated Peach deployment and production cutover remain subsequent acceptance work. NFS UID/GID/ACL acceptance is documented below. Unregistered files, cross-directory moves, identity operations and other write routes are outside these slices.

## NFS acceptance and deployment identity (Rust 04)

`permissions.rs` uses effective-identity `accessat` through no-follow directory descriptors. It checks readable source content and writable/searchable source and destination parents, or the nearest existing destination ancestor. Both a fresh/cached preview and final pre-admission validation run these checks. A denial returns HTTP 403 with `step` (`source_directory`, `destination_directory`, `source_read`) and the concrete blocking `path`; real storage errors remain 503 rather than being mislabeled permissions or empty directories. Existing serialized errors without `step` remain readable. An admitted failure retains its structured error step. Proven compensation ends as `rolled_back` and releases the owned gate; uncertain or foreign outcomes remain `recovery_needed` with the gate retained.

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

After integrating Rust 03, the NFS harness expects provable pre-publication injected failures to compensate as `rolled_back`, preserving bytes and releasing the owned gate. The recorded Rust 04 runs above predate automatic reconciliation and retain their original outcomes; combined local recovery regressions validate the new assertion, while final deployment acceptance must use the integrated image.

## Shared multi-file preparation (#5 / #7)

The same worker, gate, operation store, and recovery loop now accept native domain-built
execution intents. Person rename and move-files are wired through
`native::{validate,plan,snapshot,revalidate,result}`; archive admission remains separately
owned by its feature adapter.

Existing rename Request/Plan JSON omits all new optional fields, preserving old request and
preview hashes. Evidence without a schema remains the old single-file format. New evidence
uses `schema:2`, immutable file indexes, and an admitted aggregate domain snapshot. The store
schema is 3 so older writers refuse a downgrade; legacy records remain readable.

`Plan.execution` declares approved internal scopes, failure policy, affected stores, native
intent data, and a result template. `native::DomainSnapshot` contains metadata plus an optional
`ledger::Delta { changes, creators }`. Typed deltas allow only known tables and exact complete
before/after rows, with path/creator/link predicate guards. Creator IDs are allocated by SQLite
inside the final Peach transaction, and its receipt stores the fully resolved concrete delta.
No client SQL or executable callback is accepted. A false store flag means that database is
not opened; local-only zero-file operations therefore need no faces or Peach access.

File steps carry `(operation_id,file_index)` and source/destination paths. The runner opens
one file and its two parents at a time, verifies pinned identities, and never retains 5,000
sets of descriptors. Missing destination directories use exclusive owned temporary directories
and no-replace publication, with inode evidence persisted before publication. Cleanup removes
only proven owned empty directories and preserves external children. Hard-link failures on
unsupported/cross-device filesystems use a streamed, SHA-256 verified owned copy and atomic
`RENAME_NOREPLACE` publication; source unlink follows durable publication evidence. Reverse
copy compensation also records its new inode. Unknown temp ownership or foreign changes keep
the operation in recovery; names and sizes alone never confer ownership.

Archive's continue-unchanged policy may skip a file only before its first mutation boundary.
It persists `successful_file_subset` before constructing partial native deltas, then persists
`commit_scope` before the aggregate transactions. Recovery reloads both rather than deriving
success from filesystem presence. A full-success operation reuses its admitted snapshot.
Multi-file rollback restores the operation's complete affected subset. `status` now includes
per-file phases/status, `processed`, and `committed`, distinguishing moved files awaiting local
and Peach commits from final success.

Bounds are explicit: 5,000 physical moves, 64 MiB domain snapshot, 8 MiB HTTP body, one worker
and the existing 32 queued commands. Fault phases retain all old names and also support
`<file-phase>:<index>` for targeted file interruption. The 12 shared Rust tests cover cross-parent
moves, copied publication/compensation recovery, foreign temp/parent/content rejection,
owned directory cleanup, two-file aggregate execution/restart/rollback, zero-file local-only
writes, and legacy serialization. Existing native HTTP and recovery suites still apply.
The shared tests run only in newly marked isolated `/tmp` fixtures; copy tests exercise the
same primitive directly. Production NFS multi-file/cross-device deployment remains unproven
until the feature and deployment acceptance runs complete.


## Selected person moves (#7)

`person_move` plans only explicitly selected ordinary videos under the current person.
Aliases resolve with cycle checks; the target must be an existing person. Relative subfolders
cannot contain empty, hidden, traversal or reserved components. Existing destinations,
case/NFC-equivalent selected names, unrelated destination indexes, symlinks and stale source
fingerprints reject the preview or admission. Each selected move supplies an internal person
scope to the common file primitive; it introduces no additional worker or transaction protocol.

`person_registration` streams the existing video index and retains only matching identifiers.
Existing IDs survive every move. A first unindexed rename/move allocates `file:SHA256([path,inode])`
and commits its path override with the other metadata; it creates no synthetic face-index row.
Repeated unindexed operations resolve override-only IDs. Failed operations restore the original
override bytes, including absence before the first registration.

`person_metadata` builds complete native deltas for selected video/task rows and preserves raw
OCR, historical IDs and external account associations. Report membership, selected queue/claim
owner fields, paths, history and thumbnail references follow the accepted plan. LocalFiles
predicate guards cover selected IDs plus source/destination paths, including absent rows, so
late matching inserts cannot evade exact-row validation. Unrelated worker progress stays valid.
Peach changes only selected asset paths and the old person's creator-role links, retaining
other roles and existing target link sources. Both source and target use ambiguity-checked
canonical-or-normalized lookup. Missing target creators receive their actual SQLite ID inside
the final transaction; unrelated entity allocation does not invalidate a preview.

The selected set is one compensation unit. A later file that fails before its first mutation
is durably marked unchanged; prior moved files restore, while external bytes on that untouched
file remain. A foreign change after publication keeps recovery blocked. Native receipt lookup
resolves interrupted or lost commits without repeating successful file/database mutations.

```sh
python3 tests/http-rust-person-move.py --port 18985
# Seed a fresh observed-schema fixture as above; serve its resolved paths in both services.
python3 tests/http-rust-person-move-gateway.py \
  --root /tmp/face-admin-person-fileops-example --url http://127.0.0.1:18984
```

The HTTP suite covers cross-person/nested selection, source/target/index/permission changes,
source and target identity case reuse/ambiguity, unindexed rename/move/re-rename, exact rollback,
late second-file changes, local index phantoms, 500-file success/501 rejection, indexed process
kills and foreign destination replacement. The gateway suite verifies public routing, real
sample bytes, native Peach constraints, stable Range requests and result lookup. These are
isolated local acceptance checks; production cutover and integrated NFS rehearsal remain separate.

### Native confirmation and ordinary archive (#5)

`POST /v1/archive` accepts `{command,body}` on the existing bounded writer queue.
Commands are `queue-confirm`, `queue-undo`, `claim-decide`, `archive-preview`,
`archive-apply`, and `archive-reconcile`. `GET /v1/archive?id=<UUID>` returns the
legacy `{job,result?}` presentation; omitting the ID selects the latest archive.
Next's existing queue/claims/plan endpoints are transport adapters when the
explicit Rust feature switch is enabled. There is no execution fallback after a
native request fails. The duplicate-resolution endpoint remains ticket #6.

Confirmation preserves review file fingerprints, group/deferred checks, common
account evidence, notes, source links and stable IDs. Confirmation and undo have
zero media files and no Peach transaction. Optional account-rule persistence and
processing-state restoration use deterministic, separately journaled zero-file
follow-ups on the **same Engine**. The mandatory human decision first records an
honest warning. Only a proven follow-up commit clears it; known failures remain
warnings, and replay never repeats a committed follow-up. Both optional commands
verify their exact intent against the completed parent receipt.

Ordinary archive versions include source fingerprints and affected index/task,
metadata and Peach records. Unrelated task/entity changes do not expire them.
Native preflight requires exactly one active source asset, an unoccupied target,
and unambiguous canonical/lowercase-normalized creator lookup. Missing creators
are allocated inside the Peach transaction, using actual schema defaults and
constraints. Existing IDs, all existing link roles/sources and audit text survive.

Archive uses the common runner's `ContinueUnchanged` policy. Proven untouched
file failures remain queued; the persisted successful index set determines one
aggregate local/Peach commit. A failed ledger transaction compensates that whole
successful subset. A confirmed ledger commit is recovered forward, including a
lost final reply. Results include the final file locations, actual failed files,
and `manifest: "rust:<operation-id>"` referring to the durable operation journal.
The deprecated filesystem-manifest rollback endpoint is not a native undo API.

Run on marked isolated fixtures only:

```sh
cargo test --manifest-path services/file-operations/Cargo.toml --locked
cargo clippy --manifest-path services/file-operations/Cargo.toml --locked --all-targets -- -D warnings
cargo build --manifest-path services/file-operations/Cargo.toml --locked
python3 tests/http-rust-archive.py --port 18981
python3 tests/http-rust-archive-gateway.py --port 18981
```

The gateway harness additionally uses reserved Next port 18980. The native suite
uses real bytes, the observed Peach schema and the processing schema. It covers
partial commits, actual SQLite and permission failures, late target races,
concurrent confirmation, scoped versions, 5000-ID requests, duplicate submits,
never-started fences, and process exits at multi-file/transaction/optional-child
boundaries. The service's fixture PATH excludes external executables. These
checks do not authorize or perform production media changes.

### Control-file read guards

Control JSON reads walk parent directories without following symlinks, open the final entry with `O_NOFOLLOW | O_NONBLOCK`, and require a regular file before reading. Reads use bounded `pread` from the held descriptor; device/inode, length and modification/change timestamps must remain stable, and the pathname must still name that descriptor. FIFOs, symlinks, replaced files and oversize input produce explicit errors with the affected path. Only an absent path is treated as optional. This prevents special-file stalls; it does not claim that regular-file storage I/O has a cancellable deadline.

Gate evidence reads the already held read/write descriptor rather than reopening `.review-write.lock`. Startup recovery and routing markers are bounded to 1 MiB. Metadata and legacy JSON documents default to 256 MiB per file; `FILE_OPERATIONS_CONTROL_MAX_BYTES` accepts a positive byte count when a known valid larger document requires it. Limits are checked against file size before allocation and against bytes read if the file grows. No content is truncated or silently skipped. The executor lock also opens nonblocking/no-follow, must be regular and must retain its directory-entry identity before authority checks. Rejected/replaced gate entries remain in place and uncertain operations retain recovery protection.

`python3 tests/http-rust-control-files.py` exercises actual FIFO/symlink/oversize metadata and markers, prompt startup/recovery behavior, and a 2 MiB legacy document through normal execution and restart. Rust control tests additionally replace an already-held gate with a FIFO, symlink and byte-identical new regular inode, verify that evidence/release reject all replacements without deleting them, and check exact byte-limit boundaries.

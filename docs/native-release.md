# Native release and compatible rollback

This is release preparation for #14, not a deployment receipt. Production cutover
is still blocked on full 640 GPU coexistence/resource measurement, a prepared
WSL peer, sandbox enforcement, real-data parity and the final joint/runtime
checks. A synthetic CUDA probe or successful package build does not clear these
gates. Do not pause neighboring GPU services to obtain headroom without the
user's authorization. Do not close #14 based on this code.

## Release unit and frozen inputs

The release comprises **four Docker services on the VM** (native operations,
worker, calibrator and web) and **one external WSL systemd GPU peer**. Peach stays
in place. `deployment/native-release.manifest.example.json` and the independent
compatible rollback example have intentionally failing placeholders. Prepare
private copies with exact container/image identities, source commit, GPU
release/model/instance identity, probes, volume mappings, UID/GID, resource
limits and evidence hashes. Keep env files and inspect receipts mode 0600; never
commit credentials or host inventory. Candidate Docker images must already be
present by immutable image ID. The GPU is not a VM Docker image.

Native state schema is **9**. Schema 8 introduced native processing receipts;
schema 9 makes persisted admission closure part of the compatibility contract.
Older binaries must refuse schema 9 rather than silently reopen writes. Never
set `user_version` backward. Native authority is the original
`executor.lock` device/inode paired with the `authority` row. Preserve that
volume and inode across container replacement; a copied lock is not a new
executor authority. An initial native bootstrap is explicit: set
`services.native.initially_absent=true`, expected identity to null and
`authority_identity=null`; the authority directory must not exist. The
controller records the created inode and produces `activated-manifest.json` for
subsequent verification/open. No existing authority can be adopted by bootstrap.

Use local metadata and Peach **database directories including WAL/SHM**, NFS
media, and the actual service UID/GID. Never mount an unrelated empty database
file as the ledger. Mount media read-only in the worker and read/write only in
the authoritative native executor. Keep needed read-side web Docker access until
its separate readers are migrated. No broad recursive media permission changes.

Before draining, run `maintenance/release_control.py permissions --manifest
PRIVATE.json` under the native UID/GID. It checks RWX on actual native mutation parents (BASE, watermarks,
external-profiles, processing's SQLite/WAL directory, routing and authority), RW
on databases, and READ on existing atomically replaced JSON documents. Worker
attempts/faces/evidence, groups and legacy jobs/receipt directories require only
RX; optionally list exact accepted artifact files in `permission_read_files`
for READ checks. Native does not need to chown root-owned read-only `jobs`.
Listed mutable media parents require RWX. No probe creates files. Root-owned BASE alone is not proof
that UID 1000 can replace a nested JSON file. Review denied paths and arrange
only the necessary ownership/ACL correction centrally. Preserve this report
alongside the marked NFS write-fixture acceptance; access checks alone cannot
prove every NFS operation.

`check` also verifies all candidate images are local and the backup destination
has capacity for BASE plus ledger and a configured reserve. The only omitted
namespace is `processing/tmp`, disposable worker scratch frames; this exclusion
is recorded. Accepted `processing/attempts`, publication payloads, native state,
business JSON and all receipts remain included. Unreadable durable directories
fail capacity inventory instead of being silently counted as empty. It does
not prune images, delete unrelated data, or silently omit a large part of the
backup. A full VM filesystem blocks release until an approved persistent backup
location/capacity plan is available.

## Safe parity

Prepare copies with the SQLite backup API, not `cp` of live WAL databases:

```sh
python3 maintenance/release_control.py parity-snapshot \
  --source-base /SOURCE/metadata --source-ledger /SOURCE/peach/ledger.db \
  --output /NEW/isolated-parity
```

The destination must be new and outside the source. The helper rejects symlinks,
backs up all SQLite files consistently per database, excludes live native
authority, the shared write marker and disposable `processing/tmp` scratch,
preserves accepted attempts and historical receipts, and emits
a snapshot marker. It does not claim a cross-database atomic snapshot: fence
writers for final acceptance, or record source changes during read-only capture
and repeat affected comparisons. Preserve the actual media path namespace.

Start the native binary with `FILE_OPERATIONS_PARITY_ROOT=/NEW/isolated-parity`,
`FACES_BASE=/NEW/isolated-parity/base`,
`PEACH_LEDGER_PATH=/NEW/isolated-parity/ledger.db`, the actual `MEDIA2_ROOT`, a
loopback bind and token. Do not set `FILE_OPERATIONS_FIXTURE_ROOT`. Plans use the
real identity's existing permission checks. A media RO mount can cause truthful
permission denials; do not call those semantic mismatches or bypass the normal
checks. Prefer an OS-level read-only media mount as additional protection.

Parity allows only file/batch previews and archive-preview plus GET readers.
Database snapshots use immutable **read-only** SQLite handles; this is never
used for live WAL databases. Plans and control state live under the new
`parity-root/state`; owner markers are not written to BASE. It cannot execute,
reconcile, restore, publish metadata/results, start the health scheduler, recover
startup operations, or be promoted to a writer. Any existing state directory or
copied BASE authority is rejected, including on restart. Use a fresh snapshot
and new state per run. Compare scope, paths, collision behavior, permissions,
stable IDs and ledger relationships rather than UUID/timestamp/hash equality.

The committed `tests/fixtures/peach-full-schema.sql` contains schema only from the
275-object capture (SQLite's internal `sqlite_sequence` is omitted), no library
rows or private paths. The repeatable full-schema HTTP test exercises rename,
merge, batch partial completion/retry, FTS, revision triggers, foreign keys and
integrity on marked synthetic data. Parent-run archive/confirmation coverage is
recorded separately in private `full-schema-validation.json`; recheck live Peach
image and schema digests at cutover.

## Drain and deploy sequence

All controller actions default to a non-executing plan except read-only `check`,
`permissions` and creation of a new parity snapshot. Deployment commands below
are for the central release owner after the missing gates are satisfied.
`--execute` is not evidence that permission has already been granted.

1. Freeze a complete manifest and all evidence hashes. Run `check` and the UID
   permission inventory. Include latest accepted UI, static asset and playback
   Range URLs, legacy and native receipts, worker/calibrator/GPU health. The
   controller requires positive evidence files for every gate before fencing or
   replacing services.
2. Prepare the external GPU separately using the frozen
   `services/gpu_inference/deploy/build.py`, reviewed `face-gpu.service`, secret,
   model and immutable bundle. Installation/start/symlink changes on the WSL
   host require the separately authorized central stage. Record the prepared
   peer receipt, release manifest canonical digest, code/lock hashes and
   `service_instance`; no such live receipt currently exists. Authenticated
   `/health/ready` must prove exact model/preprocessor/runtime, actual CUDA
   startup kernels, managed release and accepting state. Do not infer this from
   a local VM container. For a GPU replacement, first finish the local drain
   below, then use that bundle's `deploy/control.py` to drain the same instance,
   confirm no requests remain, switch the approved unit/bundle and record the
   new accepting instance before replacing VM peers.
3. Verify the system `face-video-health.timer` is inactive and disabled/masked
   (or absent), and its service is inactive. An inactive **static** service is
   valid; it does not need an artificial disabled state. The controller performs
   fresh `systemctl show` checks before fencing, replacement and reopening; it
   never disables/stops that host timer itself. The observed production timer
   remains active/enabled, so the central owner must explicitly fence it after
   authorization. Retiring the repository Python file alone does not change
   the previously deployed timer/script. Do not enable Rust health alongside it.

   Run `fence-peers --execute`: pause worker claims and wait for current worker
   and calibrator publication to finish. For an existing native service, close and
   drain it next, allowing its already-admitted health publication to finish
   before the external deployment fence can block it. Publish `.review-write.lock` with an exclusive same-filesystem hard link only
   **after** any existing holder finishes. The prepared regular file is fsynced,
   and its exact inode/token/hash ownership receipt and receipt directory are
   durable before that shared link becomes visible. A held or
   dangling foreign marker times out without stopping web, deleting the marker
   or inferring ownership from PID/age. The deployment marker has a unique token,
   exact inode/byte evidence and durable private intent/ownership receipts.
   Under this fence, verify person/batch/archive/staging journals are terminal,
   then stop web, recheck worker/calibrator idle and stop those named peers.
   No known long legacy file copy is killed merely to drain. A late publisher
   that remains busy causes bounded failure with the owned fence retained.

   Legacy active/recovery states block release. Unclassified old-format history
   requires an individually reviewed `legacy_terminal_acknowledgements` path→hash;
   this cannot override an explicitly active or recovery-required record. The
   parser checks `status`, staging `state`, nested `result.status` and job states;
   `state:finished` cannot conceal `result.status:recovery_needed`. Once
   all peers are stopped and journals/timer are checked again, release only the
   exact owned deployment fence, then close/drain native. Replacement refuses
   any remaining shared marker, including before first native bootstrap.
   Failure receipts preserve actual stopped/running services and owned fence
   evidence. After resolving the cause and stopping all old peers, the central
   owner can use `release-fence --execute` with the original release ID; it
   verifies inode, bytes, token-bound receipt and peer states and cannot remove
   a foreign replacement. Reads are bounded, nonblocking and no-follow, so a
   replacement FIFO, symlink, oversized marker or corrupt receipt fails promptly.
   Recovery also finishes a crash after shared unlink but before the release
   receipt. Before ownership was saved, recovery records `not_published` only
   when the shared path is absent and leaves the unowned prepared file untouched.
   Never delete the shared lock by hand or force-stop
   uncertain accepted work to make the checks pass.
4. `POST /v1/admission {"accepting":false}` persists closure before returning.
   Already queued work and an already admitted health scan finish; new HTTP and
   background writes are rejected. `/v1/readiness` reports pending commands,
   admitted publishers, unresolved operations, shared gate presence, actual
   authority identity, mode and schema. `drained=true` requires all these lanes
   clear and the authority intact. `ready` remains true during ordinary accepted work when authority is valid and no protected recovery blocks it; it is distinct from `drained`. `/health` alone is liveness. GET receipt reads
   remain available. Shutdown does not imply a stalled NFS syscall was cancelled.
5. Run `replace --execute`. The shared deploy flock, exact identities, Peach
   schema and capacity are checked again. Old web/worker/calibrator must already
   be stopped. It saves private original inspect/config and complete local
   snapshots, keeps original containers as anchors, and creates/starts native,
   worker, calibrator and web in order. Native must start closed; the persisted
   worker pause survives replacement. The unchanged authority inode is checked
   after each step. Each step gets its own receipt; a failure leaves completed
   steps visible and admission closed. There is no multi-service atomic commit.
6. Verify with the produced `activated-manifest.json`, then `open --execute`
   only after exact runtime probes, schema, authority and full drain still pass.
   Open native admission first, then centrally resume worker claims via
   `/api/processing` (`{"action":"resume"}`). No media operation, auto-archive
   or merge is a deployment smoke test. Verify continued analysis progress,
   unchanged manual records/queues/history and the current review snapshot.

## Rollback

Use `native-rollback.manifest.example.json` as a **new** manifest, with current
container identities and a proven compatible prior native/web/worker/calibrator
bundle. `rollback --execute` requires `purpose=compatible-native-rollback` and
all the same drain, schema, remote GPU, authority, backup and probe checks.
Freeze the GPU revert as its own external prepared receipt; use its previous
compatible immutable bundle and unit/config only after the VM attempts and GPU
instance drain. Preserve task/native databases and receipts.

Do not toggle back to legacy, reuse a schema-8 writer on schema 9, delete native
state, replace the executor inode, or restore old global backups over newer user
changes. A UI/service rollback can retain the compatible native query/execution
service. If no compatible prior native bundle exists, leave admission closed
and repair forward. Outstanding new-format operations must be reconciled with
that compatible executor before any authority change. The archived old containers
are evidence/anchors, not authorization to restart obsolete Python writers.

## Retired writers and retained history

Person/batch/staging, archive/queue/review/claim, shared metadata, video-health
report routes now dispatch only to native. Environment defaults or an old
`FILE_OPERATIONS_BACKEND=legacy` cannot select the retired writer. Web no longer
creates native routing ownership records before a native admission. Legacy
rollback and legacy watermark scan launch return 410. Legacy completed person,
batch and archive receipts remain readable; an unfinished legacy preview must
be previewed again. Homepage archive state uses the native latest receipt while
preserving a genuinely newer legacy receipt, rather than stale jobs/archive.json.

Native-owned JSON writes fail closed regardless of the old env toggle. Calibrator
uses isolated F_OUT and native publication; direct identify_faces BASE output
and the old health publisher are retired. `processing_store.finish` is removed
from production; old result seeding exists only in the marked test fixture helper.
Old Node implementation modules are retained as historical test baselines but
are unreachable from production route dispatch; read-only archive filename
helpers are split into `archive-paths.ts`. No service rollback revives them.

The deployed review-summary manifest's 11 files were checked against the
integration baseline: 10 exact hashes match; d33's review/page exactly matches
the deployed hash and its only integrated successor is #18 review continuity.
Thus no unrelated original-worktree WIP was imported. Preserve #16 staging and
#19 processing contracts in the final joint preview.

## Verification of this preparation

- Rust: 71 unit/integration cases and clippy with warnings denied; native HTTP
  readiness, persisted closure, malformed/dangling markers, parity immutability,
  health-scan drain and an actual older-schema binary rejection.
- Isolated native HTTP: complete 275-object schema rename/merge/batch/partial
  recovery; #17 scan/restart/error guards; real worker→native publication and
  #18 held review evidence during refreshed background suggestions.
- Web: 101 model/helper tests, TypeScript check, and a live Next gateway fixture
  exercising queue/undo/claim/archive, native latest versus legacy receipts,
  homepage archive, retired endpoints and stable video Range. The gateway was
  deliberately configured with the obsolete `legacy` flag and still used native.
- Python: 36 processing cases; retirement assertions for health/watermark
  publishers; 24 controller tests covering schema/container/drain/capacity
  refusal, SQLite snapshots and partial replacement receipts. The watermark
  frame-extraction case is skipped without Pillow in the local worker venv.

## Actual local Docker controller rehearsal

`tests/docker-release-rehearsal.py --execute-isolated --native-image IMAGE
--controller-image PYTHON_DOCKER_CLI_IMAGE` uses an actual Linux schema-9 native
binary, real SQLite/files and Docker stop/rename/create/start commands. Every
container/network has a unique rehearsal label; all paths are under a newly
marked `/tmp/face-admin-release-rehearsal-*` root. Cleanup removes only those
owned containers/networks. The native source used in this rehearsal is frozen
at `13fc3bf`; the script records immutable image and controller hashes.

Passed: an active legacy timer blocks before mutation; a held legacy shared lock
times out with web alive and foreign marker untouched; an unresolved legacy
journal retains the owned fence and exact-owner recovery releases it only after
peers stop; initial native bootstrap; a real isolated rename and receipt; compatible
replacement preserving authority device/inode and historical query; an actual
Docker create error after native replacement with durable partial receipts and
closed admission; explicit fixture repair; unresolved-operation rollback refusal;
compatible native rollback; and refusal of a deliberately seeded future schema
before stopping the current native container. The future-schema fixture is never
downgraded. Short, bounded retry tolerates endpoint startup only; wrong identity,
contract or authorization still fails.

Web/worker/calibrator/Peach HTTP processes and the external GPU response are
**protocol simulators**, not application/model acceptance. Their evidence carries
`simulated:true`; the controller rejects it outside a marked rehearsal manifest
and explicit `FILE_OPERATIONS_RELEASE_REHEARSAL_ROOT`. It verifies owned existing
container labels before any rehearsal mutation. These manifests cannot authorize
production or claim full GPU/NFS/application acceptance.

This is an actual **local Docker controller** rehearsal, not a deployment-host,
WSL/systemd or production rollout/rollback. Production capacity remains a gate:
the observed roughly 4.6 GiB BASE and 6 GiB available root filesystem do not
justify a complete backup plus images/reserve. The controller does not drop
immutable caches/thumbnail bytes silently; choose an approved persistent backup
destination or capacity plan, keeping live SQLite local. Full GPU coexistence,
real-data parity and final joint checks remain required.

Review follow-up evidence: `tests/evidence/review-fixes-20261108.json`. The
controller additionally injects ownership-receipt failure, interruption before
and after shared publication, directory-sync interruption and post-unlink receipt
failure. Marker and ownership-record FIFO/symlink/oversize/corruption cases are
bounded and preserve foreign objects. The actual Docker controller rehearsal
was rerun with the repaired prepared-inode publication protocol; its simulated
peer/GPU boundaries above still apply.

# VM processing / remote detection integration (#19)

Implemented code is not a production rollout or a GPU throughput acceptance. The
production detector remains 640; the measured 160 development probe is not a
substitute. Default worker capacity is **1**. Do not set a larger limit without
same-fixture CPU/GPU/coexistence measurements and the release owner's approval.

## Authority and recovery

The VM owns discovery, the independent SQLite analysis queue, claims, retry limits,
leases and progress. One coordinator holds `processing/.worker.lock`. Each claim
gets a UUIDv4 run ID, coordinator generation, and 660-second lease; the subprocess
has a 600-second limit. `PROCESSING_MAX_CONCURRENCY` defaults to 1, caps at 2; the
persisted setting can drain 2 → 1 without cancelling an existing attempt.

Accepted analysis uses native `/v1/metadata`, command `processing-result`.
`operation_id` equals the run UUID. The request binds the task's stage, batch,
source path/size/mtime, generation/expiry, scoped native material version, result
and next stage. Text snapshots include **all** watermark rules because a newly
seen OCR key may acquire a mapping during inference. Face snapshots additionally
bind `reference_input_version` from native `analysis-state`.

`processing_publication.py` durably writes the exact request before submission.
A transport-unknown result keeps its attempt active; startup and idle recovery
replay that identical operation before fencing interrupted tasks. Never retry an
uncertain mutation under a new operation ID. The native transaction receipt is
`_face_admin_processing_results`; it commits task data, run outcome/provenance,
first-valid batch completion, unknown-face grouping and event together. Native
also owns the selected-video OCR merge. Python no longer writes `faces.db.videos`
or `watermarks/results.json` during automatic processing.

Results and thumbnails are immutable under `processing/attempts/<run UUID>/`.
They are not copied into shared caches. Image routes require the native accepted
receipt, an explicitly referenced image and the accepted source fingerprint.
Unaccepted attempt artifacts are retained for receipt recovery; this version does
not add an automatic garbage collector. Do not remove pending publication files.

Reference building brackets authoritative faces/business inputs with native
analysis-state, records that input token in `references.npz`, and each attempt
copies the reference snapshot. Legacy `processing/faces/*.npz` references must
match the current path, size, mtime and model version. New receipt-backed caches
also bind their byte hash and source device/inode: a same-filesystem rename or
archive can preserve reference usefulness, while replacement or tampering cannot.
Confirmed people with only valid cached features remain reference people. An
accepted per-video cache can avoid repeated extraction. Matching remains on VM;
new candidate/reference consistency measurements are required before rollout.

## Explicit execution mode

- `PROCESSING_FACE_BACKEND=cpu` (default): existing CPU detector and recognition.
- `PROCESSING_FACE_BACKEND=gpu-detection`: remote CUDA detector, local CPU
  recognition and matching. OCR remains CPU. There is no automatic CPU fallback.
- `PROCESSING_GPU_URL`, `PROCESSING_GPU_TOKEN`: restricted service origin/token;
  the token is 32–256 ASCII non-whitespace characters. TLS/private tunnel preferred.
  Remote private-IP HTTP additionally requires `PROCESSING_GPU_PRIVATE_HTTP=1`.
- `PROCESSING_GPU_MODEL_SHA256` defaults to the observed buffalo_l detector hash;
  `PROCESSING_GPU_PREPROCESS_VERSION` defaults to
  `scrfd-640-bgr8-det0.5-nms0.4-v1`. Changing either requires equivalence validation.
- Native `FILE_OPERATIONS_RUST_URL` and `FILE_OPERATIONS_RUST_TOKEN` are required
  before discovery/reference building and accepted publication.

The GPU receives bounded JPEG samples, hashes and opaque IDs, never raw video
paths, reference identities, business databases or archive credentials. Responses
must echo the attempt/source/model/preprocess context and contain actual CUDA
provider proof. Network errors, OOM, refusal and backpressure are failures, never
converted to an empty/no-face result. At most three attempts with bounded backoff
are retained; explicit errors remain visible in the dashboard. Cancellation does
not assert that remote kernels stopped.

## Dashboard

`/operations` uses the existing operations component entry point and the new
batch dashboard. `/operations/tasks` retains the detailed queue. Native review
continuity remains owned by #18. The dashboard reads `batch_items` and
`processing_runs`; confirmed/archived items do not reduce historical completion.
Manual takeover is excluded, not a machine success. Missing ledgers are unknown
(503), and absent inference provenance is unknown, never assumed CPU or GPU.
Actual execution metadata is attached to run history; configured availability
comes from protocol-3 heartbeat and is distinct from completed-run provenance.

Settings are persisted in the independent processing database. Heartbeat caps
constrain accepted settings. Failed requests preserve the current UI snapshot.
Recent user activity, search/detail state, playback and submitting work protect
the current view; idle auto-refresh requires two minutes and an unchanged
activity generation. Explicit real-time following is opt-in.

## Joint release gates

Before central release: merge native schema-8 publication, #18 continuity and this
worker/web code; exercise real native concurrent file/rule/manual changes and
receipt recovery. CPU/mock and loopback GPU tests prove contracts, not performance.
Still required: positive-face identical-fixture consistency; production VM CPU
baseline; stable 640 GPU headroom; safe higher-concurrency/coexistence measurements;
worker-container transport; reproducible clean runtime and model licensing;
restricted OS/service principal; coordinated startup, drain and rollback rehearsal.

Start native authority and verify recovery/readiness before VM worker; make GPU
ready before enabling remote mode. Bring up matching web after fixture checks.
For rollback, pause new claims, resolve pending native receipts, drain or fence
attempts, then switch compatible worker/web. Keep receipts, cohorts and analysis
history. No service stop, network change, production scan, model unloading or
production deployment is authorized by this document.

Model files are hashed locally before extraction against the observed detector
and recognizer manifest; new NPZ files record those hashes. A mismatch fails
explicitly and needs a separately versioned/evaluated model change. Legacy NPZ
has only its pipeline/model-version label, so its path/fingerprint validation is
retained for migration compatibility; it does not acquire invented hash evidence.

The paired GPU service revision supports original portrait samples up to width
1280, height 4096 and 1280×4096 pixels, while retaining encoded/request byte caps
and fixed detector shape. VM transport and CPU recognition keep the original
image, without silent resizing or CPU fallback. The older b7ab29c service has
stricter square bounds and must not be the final paired deployment. Exceptional
oversized inputs still fail explicitly; real 640 portrait consistency remains a
release measurement gate.

The VM image copies the stdlib GPU client/protocol beside `/opt` worker modules;
it does not require CUDA packages. Startup checks native snapshot readiness before
claiming. A native read outage returns the attempt's model retry budget and waits
30 seconds; an uncertain mutation retains its original operation for recovery.
Legacy standalone watermark worker entrypoints/shared-result writes fail closed
when Rust ownership is configured. Central cutover must also drain and retire any
pre-upgrade process that loaded older code before enabling the new authority.

Joint review regression runs with `PROCESSING_REVIEW_JOINT=1` in
`tests/http-processing-native-worker.py` against the combined #18/schema8 native
binary. It opens a delivered review session, publishes a changed suggestion
through the actual worker/native flow, checks frozen rows/data/group and pending
updates, then confirms using the original delivered token. Refresh and receipt
replay keep batch completion at one.

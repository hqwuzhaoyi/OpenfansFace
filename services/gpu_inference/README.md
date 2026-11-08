# Bounded GPU detector adapter (#19, integration incomplete)

This adapter moves **detection only** into an isolated Python process. It receives one encoded image and returns anonymous boxes, scores and five-point landmarks. It does not load the recognition model, identify people, fetch resources, scan directories, write business databases, publish report files or archive media. VM recognition, reference matching and task/accounting behavior remain local; the current worker integration is documented in `docs/processing-gpu-worker.md`.

The existing WSL environment can execute CUDA detection. Current coexisting workloads did not leave enough stable headroom for the 640 configuration to pass the configured admission floor. A 160 development configuration completed a synthetic loopback HTTP probe; that is not an approved replacement for the current 640 detector configuration. **#19 remains incomplete and this package is not a production release.**

## Modules and execution

- `protocol.py`: strict dataclass request/response contract and validation, shared with the VM.
- `client.py`: authenticated typed transport, bounded same-request retries; no redirects/proxies or CPU fallback.
- `model.py`: explicit ORT CUDA session injected into the installed InsightFace SCRFD detector; no derived resolution sessions, no implicit model download.
- `broker.py`: one executing inference, zero or one waiting request, bounded in-memory replay cache, deadlines, cancellations, backpressure.
- `server.py`: stdlib HTTP adapter, loopback by default, bounded connection handlers/uploads and bearer authentication on every route.
- `benchmark.py`: anonymous synthetic fixtures and resource/coexisting-health sampling. It never reads media or creates an identity/reference gallery.

From the repository root, in a **separately approved isolated environment** with an existing model:

```sh
python -m services.gpu_inference.server \
  --model "$GPU_DETECTOR_MODEL" \
  --model-sha256 "$GPU_DETECTOR_SHA256" \
  --token-file "$GPU_INFERENCE_TOKEN_FILE"
```

Variables are operator-provided paths/values, not business paths or credentials to commit. Token file must be mode 0600 and hold 32–256 non-whitespace ASCII characters. No token is generated into source, shown on the CLI, or logged. Default listener is loopback port 8921. A remote deployment needs a private authenticated tunnel or TLS ingress and access restriction; changing `--host` does not establish those protections. The VM client refuses ordinary remote plaintext HTTP unless an operator explicitly permits a private numeric IP. Redirects and environment proxies are disabled to avoid forwarding credentials elsewhere.

`requirements-observed.txt` records the original environment. The deploy directory now contains a complete hash lock, clean-environment validator and isolated builder; see [deploy/README.md](deploy/README.md). Neither file authorizes installing into the existing environment. Existing CPU and GPU ORT distributions overlap; create and validate a clean isolated environment with only the GPU distribution before release. Verify model/package provenance and applicable licenses separately. No model weights are included.

The GPU service must run as a restricted principal/container with no business DB credentials, media mount, home-directory access or archive write permission. Mount only the model and runtime dependencies read-only, supply a secret read-only, permit a bounded private temporary directory for the startup profile, and expose only the selected private transport. **This code does not establish OS isolation by itself.** Running it as the existing unrestricted scanning user is not the final deployment boundary.

## Wire contract

`POST /v1/detect` takes JSON, schema version 1, stage `detection`. Exactly one PNG/JPEG frame per request; the VM extracts its existing sampling frames. Required fields:

| Field | Meaning |
|---|---|
| `request_id` | Unique per frame and immutable canonical request. Reusing it with changed context is a conflict. |
| `binding.task_id`, `video_id`, `batch_id` | Opaque VM identifiers, never local paths or names. |
| `binding.attempt_id` | Existing VM run ID for the active attempt. |
| `binding.lease_id`, `coordinator_generation`, `lease_expires_at_ms` | VM-owned lease/fencing context. The GPU echoes these; it cannot establish which VM lease is current. |
| `binding.fingerprint` | `size`, decimal-string `mtime_ns`, and `content_sha256` (nullable when not computed). Null is explicitly not proof of byte identity. Path/file identity changes must be covered by VM material state and local source validation. |
| `binding.material_version` | Exact scoped native material token covering source and relevant rules/manual disposition. |
| `binding.pipeline_version` | VM sampling/feature pipeline revision. |
| `model_sha256`, `preprocess_version` | Exact expected detector hash and preprocessing manifest; mismatches reject the request. |
| `deadline_ms` | Absolute deadline, at most 120 seconds ahead and no later than the request's lease expiry. |
| `frame` | `index`, `timestamp_ms`, encoded bytes SHA-256, actual raster `width`/`height`, `data_base64`. |

Identifiers have a bounded opaque character set; no arbitrary URLs or file-path fields exist. A video may have multiple frame requests under the same attempt, each with a unique request ID and an immutable expiry snapshot. Submit at most one at a time for this version. A lease renewal changes binding semantics and must be coordinated by the VM, not guessed by the client.

The response echoes the complete context except image bytes, its canonical SHA-256, service-instance UUID, timing, execution metadata and actual detections. Geometry is in the transmitted raster coordinates; EXIF orientation is ignored, and the VM must use that same raster for alignment/cropping. Out-of-raster model coordinates are bounded but preserved for alignment. No faces is an empty list following successful decoding and model execution; corrupt frames produce errors.

`execution.stage` is always `detection`. `provider_proof=startup_profile` identifies a readiness warmup that recorded CUDA kernel events from the exact session; counters are **startup profile counts, not per-request counts**. Actual providers include CPU when ORT assigns some graph operations there. The session disables runtime provider fallback and does not change providers after startup. No “all GPU” claim is made.

Preprocessing identifiers currently supported:

- `scrfd-640-bgr8-det0.5-nms0.4-v1`: current detector shape/threshold/NMS configuration, **resource/performance acceptance incomplete**.
- `scrfd-160-bgr8-det0.5-nms0.4-v1`: development-only small-shape probe; not equivalent to 640 for recall or candidate behavior.

The VM continues any final score threshold, face alignment, recognition, vector normalization and candidate matching. Adding GPU recognition or OCR requires a new explicit stage/manifest and independent measurements; this implementation does not pretend those stages ran remotely.

## Bounds, health and errors

Hard limits: 1.5 MB encoded raster bytes, 2.01 MB request body, width <=1280, height <=4096, <=1280×4096 pixels, <=256 detections, response <=300 KB. Dimensions are inspected before raster decode and verified against actual decoded dimensions. Multi-frame images are rejected. Eight connection handlers and an eight-connection listen backlog are the HTTP ceiling; headers plus body upload have a five-second wall-clock bound, with idle socket timeouts as well.

The inference lane has concurrency **one**, with zero or one waiting slot. This is a conservative implementation ceiling, not a measured production throughput claim. Duplicate in-flight requests return `request_in_progress`; complete identical requests replay a cached result. The replay cache is bounded to 64 records, has no image bytes after completion, and is volatile. A completed identity stays pinned until its original deadline has expired in both wall and monotonic time; the 60-second TTL cannot evict it sooner. Cache pressure returns `queue_full` rather than evicting a still-replayable identity. Expired terminal entries may be evicted to make room even before TTL. A restarted process may recompute a request. Business exactly-once counting is always the VM's responsibility.

Admission reserves one of the two bounded slots even when resources are busy. Before any execution, the FIFO head checks actual GPU counters and waits within the fixed policy below. Default resource policy requires at least 600 MiB **reported free** memory and utilization <=25%; unavailable counters never permit execution. Startup uses a 128 MiB ORT arena, constrained workspace and a single session. The arena cap is **not** a cap on CUDA context, library, process or total GPU memory. A low-memory startup can leave model readiness false; the service does not repeatedly retry loading or unload neighbors. Model runtime exceptions fail the session closed until operator replacement; queue jobs do not continue executing on the failed session.

Authenticated endpoints:

- `GET /health/live`: process liveness and service instance.
- `GET /health/ready`: model/provider proof, resource counters, running/queued/capacity, `model_ready`, `resource_ready`, `busy`, `accepting`, `resource_waiting`, `waiting_reason`, startup/runtime fault. `accepting=true` means a bounded queue slot is available; it does not mean GPU counters currently permit execution. `running` counts only an actual model call, while `queued` includes the resource-waiting head. HTTP 503 when not accepting, even if the model itself is ready.
- `POST /v1/drain`: `{}` stops new computations and reports whether existing queued/running work has drained. Readiness exposes `draining`/`drained` and managed release identity; draining does not cancel a kernel or resume automatically.
- `POST /v1/cancel`: `{request_id,attempt_id}` returns `cancel_requested` and `execution_finished`. It does not interrupt an already running ORT call or free its slot early. A cancelled cached result is not returned as successful.

Errors are structured `{error:{code,retryable}}`. Typical statuses: 400 malformed input, 401 auth, 408 expired request/upload, 409 changed version/idempotency/attempt or cancelled result, 413 size bound, 429 queue full, 503 unavailable/resource busy/model failure, 504 response deadline. Busy errors include `Retry-After: 2`; the client applies the bounded policy below. A transport timeout does not prove remote execution stopped. There is no automatic CPU fallback. A deliberate CPU policy needs separate measurement and visible provenance.

## Fixed queue/retry policy v1

The release manifest is **schema 2**, with `retry_policy.version=1`. These are
fixed source policy values, not runtime knobs. Managed startup verifies their
exact manifest; a policy change requires a new version and validation.

- Concurrency stays **1**, waiting capacity **1**, at most **2 accepted** requests.
  No CPU fallback, extra execution lane, resource-threshold change or model reload.
- Each admission gets **20 seconds** for queue/resource waiting, including time
  behind the preceding request. The head checks counters at most once per second
  after each sample. Telemetry subprocess timeout is at most 3 seconds, clipped to
  the remaining wait/request budget; cancellation/deadline is checked again after
  sampling. A separate bounded deadline monitor expires never-started queued
  admissions even while the lane is occupied by a long-running kernel. Expiry and
  cancellation remove the pending entry immediately under the admission lock;
  its sealed receipt remains in the bounded cache. They never release a running
  kernel. Model execution never begins after an exhausted wait budget.
- Exhausted pre-execution waiting returns `resource_busy`. Only an identical
  cached request with `execution_started=false`, no cancellation, and that exact
  terminal reason may rearm, at most **2** times. Rearm joins the FIFO tail and
  keeps its original ID, hash, body, binding, deadline and cumulative queue time.
  The third resource timeout returns nonretryable `resource_wait_exhausted`.
  Success, cancellation, model faults and any started execution remain sealed.
- The client sends at most **8** times per frame, only for `queue_full`,
  `resource_busy`, `request_in_progress`, or `transport_unavailable`, when marked
  retryable. Backoff starts at **0.5 s**, doubles to **4 s**, with **0.8–1.2** jitter
  (maximum **4.8 s**). HTTP 429/503 `Retry-After` seconds or date is a minimum delay
  when <=**10 s**; a larger hint ends this loop instead of retrying too early.
  An insufficient remaining deadline also ends retry. Cancellation is checked
  before sending, after receiving and during backoff; it never frees a GPU slot.
- VM frames keep **60 seconds total**, clipped to the unchanged **600-second**
  stage deadline and **660-second** attempt lease (reserving the lease's final
  minute for publication). HTTP attempts and resource rearms share that one frame
  deadline; none renews it. The business task still has at most **3 stage attempts**.
  No attempt refund or new SQL writer is introduced.
- Auth, model/version, material/source, attempt/lease, drain and model-fault errors
  are not blindly retried. Lost responses replay the exact ID/body, never a newly
  generated request ID. A short Content-Length body or incomplete chunked body
  is transport uncertainty, including truncated HTTP error responses. Complete
  malformed JSON/context stays nonretryable. The cache is process-local; restart can recompute, so only
  the existing native accepted receipt authorizes business completion.
- Drain refuses new admissions/rearms and lets already owned work finish within
  its existing budget. Close/cancel wake resource waits. A disconnected caller or
  cancelled/expired running kernel retains its slot until `infer()` actually exits.

`timing.queue_ms` includes resource waiting and rearm/backoff before execution;
`inference_ms` starts only at the model call. Health exposes resource waiting
separately from running. Production INFO logs record one wait start/end per
admission and terminal refusal: truncated request digest, reason, initial numeric
resource counters, rearm count and queue time. No poll logs, images, tokens,
identities or private paths are emitted. Existing VM progress shows the GPU request
wait and bounded retry reason; no UI redesign is needed.

CPU-only tests inject time at the resource-wait seam to exercise full **20-second**
policy windows; they do not lower production policy. Real GPU acceptance must use
the unmodified policy and the same eight-frame fixture in the retained locked
runtime. TTS remains OFF; these changes authorize no service or production change.

## VM/native integration

The current worker implements this boundary; see [processing-gpu-worker.md](../../docs/processing-gpu-worker.md). The requirements remain:


1. Claim the task using the existing coordinator/SQLite exclusivity and unique run ID; preserve real batch/run accounting. Read the native scoped `state` token and validate the source before extracting frames.
2. Form the immutable binding and image requests. Keep private paths, identities and reference data on the VM. Stage returned features/evidence in attempt-specific locations, not final cache paths.
3. Validate every response, geometry, version, frame hash and provider proof with `Result.parse`. `Result.require_current(current_binding)` is an additional preflight guard; it is **not** an atomic publication operation.
4. Under the agreed native material gate, revalidate scoped material/file state and CAS the active VM task attempt/fingerprint/lease. A separate HTTP `check-state` followed by an unguarded Python write has a race and is not sufficient.
5. Independent task/progress records remain worker-owned and analysis-only. Rust-owned report/shared JSON writes go through #13 `analysis-publish` or its typed domain operation, never through this service/client. No manual link, review decision or archive action may be inferred from a detection response.
6. Publish staged evidence and finish the cohort once only after accepted CAS; rejected, late, duplicated, superseded or cancelled responses never increment completion or overwrite human decisions. A worker restart must fence old attempts before scheduling new ones.
7. Preserve #18's active review snapshot and notify rather than reorder it. Distinguish evidence material changes from suggestion recomputation.

The integrated worker/native path supplies scoped publication, feature-cache provenance and task fencing. Joint release must still validate the paired binaries, CPU/GPU recognition consistency, transport/coexistence performance and operational recovery in the intended environment. This package intentionally has no callback that can publish business data merely because inference returned.

## Validation and rollout status

Run CPU-only contract/HTTP tests without GPU packages:

```sh
python3 -m unittest discover -s tests -p test_gpu_inference.py -v
```

They cover bounded concurrent admission/HTTP handlers, replay/conflicts, authentication, body limits, model/resource errors, current-attempt/material/source/generation checks, late/cancelled responses, malformed geometry, session poisoning, bounded same-ID network recovery, and new service generations. Tests use marked synthetic data and loopback ephemeral ports.

The optional `benchmark` module requires explicit `--neighbor-port` for every coexisting local `/health` service, an existing model/hash, and sufficient resource headroom. `--cpu-only` measures the detector on that host without loading CUDA; `--size 160` is explicitly developmental. It exits 0 for completed/CPU-only, 2 for failure, 3 for skipped preflight. It samples aggregate GPU counters, process peak RSS and CPU usage, neighbor health and timings. It is a short synthetic probe, not a sustained or real-person benchmark; no positive-face/candidate consistency is established.

Observed isolated evidence is summarized in `verification.md`. Private host addresses, credentials, runtime paths and raw traces remain outside the repository. Full #19 acceptance still requires production-equivalent CPU/640 GPU fixture comparison, reliable coexistence headroom, VM-container network/transfer metrics, positive anonymous fixture geometry/recognition agreement, integrated task fencing/publication, recovery, UI and unified-release checks. No higher concurrency was tested or selected.

For eventual central rollout: establish the restricted runtime/transport first, verify exact model/runtime/preprocessing manifests and CUDA profile readiness, then enable only the explicitly measured worker stage with compatible manifests. Drain and fence worker attempts before stopping or replacing the GPU service; cancellation alone does not mean a kernel exited. Preserve VM task/receipt state during rollback and explicitly pause the GPU stage when unavailable. Switch worker/web versions only through the central release owner. This change neither installs a service nor authorizes a production cutover.

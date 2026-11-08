# #19 isolated verification — 2026-11-08

**Current assessment: one controlled VM-first trial completed all eight VM requests and passed positive geometry checks, but #19 and production release remain incomplete.** A later loopback request was refused by the unchanged utilization criterion: **40% >25%**, with telemetry available and **4193 MiB free >600 MiB**. The full worker continuation did not start because the unchanged VM CPU/RAM admission guard declined it. No speedup or production-cutover claim is supported.

**TTS remains OFF at the user's request.** The earlier pause-and-restore instruction was superseded; no automatic restoration was created for this latest trial. At **2026-11-08 04:06 UTC**, the release owner confirmed TTS inactive with its port closed, the other three services HTTP 200, and the test unit/restore timer inactive.

The sections below are chronological records. Initial no-stop/no-640 statements and the later successful TTS restorations are historical, not the current service state. See [the latest sanitized VM-first evidence](../../tests/evidence/gpu-640-vmfirst-20261108.json) and [the earlier pause-window evidence](../../tests/evidence/gpu-640-20261108.json).

During that initial phase, no production service was installed, stopped or restarted. No existing model was unloaded, live dependency installed, firewall changed, business DB/media accessed, or real person identified. The initial probes used new temporary code/profile files and anonymous zero/gradient/seeded-noise rasters.

## Initial contract and HTTP checks (historical)

`python3 -m unittest discover -s tests -p test_gpu_inference.py -v`: **19 tests pass**. Includes actual loopback HTTP transport using a clearly synthetic model fixture. No GPU claim comes from those mock-model tests. Compile check and `git diff --check` also pass.

Covered: strict context and image hash validation; malformed JSON; request bounds; token authentication; handler and inference queue bounds; exact replay versus conflicting/in-flight requests; cancellation retaining running-slot ownership; deadline fencing and late-response discard; resource/readiness distinction; explicit model fault with queued jobs refused; old attempt/coordinator/material/source rejection; invalid geometry and CPU-only response rejection; process generation changes; network failure without retries; busy state preserved through the client.

## Initial GPU runtime and real inference (historical)

The installed runtime reported ONNX Runtime 1.23.2 and CUDA + CPU providers on an RTX 4070 Ti. Model SHA-256 for `det_10g.onnx`:

`5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91`

The detector used an explicit 128 MiB arena, single session, single execution lane, one CPU intra/inter-op thread, constrained cuDNN workspace, and disabled derived InsightFace resolution sessions. No recognition or OCR model was loaded.

| Probe | Before reported free GPU memory | After model / final in-process free | Result |
|---|---:|---:|---|
| 640 detector, initial | 629 MiB | 536 MiB | CUDA warmup succeeded; admission refused below 600 MiB floor |
| 640 detector, recheck after headroom increased | 841 MiB | 581 MiB | CUDA warmup succeeded; admission again refused |
| 640 CPU-only on WSL | 887 MiB | 887 MiB | Six synthetic direct detector calls completed |
| 160 development detector + HTTP | 887 MiB | 650 / 646 MiB | Three authenticated synthetic requests completed |

Both 640 warmups recorded **141 CUDA node kernel events and 12 CPU node events** in ORT's profile. This is proof of execution for the actual adapter, not just provider enumeration. It is not evidence that full 640 workload can be admitted safely under current coexistence load. The short monitor sampled a low of 532 MiB during the first probe; aggregate samples are not isolated process peaks.

The second initial 640 probe was attempted only after a fresh headroom sample increased. Its resource gate was not relaxed when it failed. That inspection phase ended without another 640 run or higher inference concurrency; later authorized requests are recorded below.

CPU-only 640 direct detector times for three anonymous rasters over two passes were **213.8, 200.9, 200.8, 191.2, 192.1, 190.2 ms**. All outputs were empty. Process peak RSS was about 261 MiB. This baseline runs on WSL, **not the production VM**, and excludes image transport/decoding, recognition and candidate matching.

The 160 development probe recorded the same 141 CUDA / 12 CPU readiness events. At equal 160 shape, direct CPU detector times were **13.9, 12.9, 13.1 ms**; GPU HTTP round trips were **89.0, 58.5, 56.1 ms**, with service decode/preprocess/inference work **64.5, 28.4, 30.4 ms**. Encoded frame sizes were 398, 11734 and 77156 bytes. Empty output geometries agreed. Deliberately mismatched dimensions were rejected by the real decoder path.

These paths include different overhead: CPU numbers are direct detector calls, while HTTP numbers include JSON, authentication, admission/resource checks, image decode and response validation. There is **no demonstrated speedup**. Three empty-result fixtures do not establish positive-face accuracy, recall, candidate consistency or sustained throughput. Peak RSS (~844 MiB) includes the isolated benchmark's GPU and CPU sessions/imports together, not a production worker reservation.

All monitored pre/post neighbor health endpoints returned HTTP 200. Existing service PIDs and restart counts were unchanged in follow-up inspection. Raw private endpoint/path evidence, exact resource samples, sanitized logs and the temporary runner are stored in the external implementation context, not this repository. No temporary server or profile directory was left running after the probes.

## Release gates recorded after initial probes (historical)

These were the outstanding checks at that stage. The authorized-window section below records later progress and the current remaining limits.

- Stable resource/coexistence plan; model-ready 640 service must actually admit and sustain requests. No workload may be unloaded without explicit authorization.
- Same marked fixtures and production-equivalent configuration on VM CPU and single GPU; separate transfer, decode, detector, recognition, matching and end-to-end timings, failure rate and reliable peak memory. Higher concurrency only if independent measurements justify it.
- Anonymous positive-face geometry/recognition/candidate tolerance checks; empty synthetic outputs are insufficient.
- Worker-container network path and authenticated remote transport, outage/restart/cancel/late-response behavior across real service/worker processes.
- #13's atomic scoped material/attempt publication contract and cache publication; #18 review continuity; preserved real dashboard run/batch accounting and actual source labels.
- Reproducible isolated environment (resolve overlapping CPU/GPU ORT installations), model license/provenance verification, restricted OS permissions, versioned configuration, drain/start/rollback plan and central GPU/worker/web deployment.

The service's single-lane limit and development shape are implementation constraints, not approval to switch production. #19 must stay open until these gates are satisfied.

## Deployment preparation and CPU portrait follow-up (historical)

The follow-up adds width <=1280 / height <=4096 / <=1280×4096 pixels while retaining the existing encoded-byte and JSON bounds. The original portrait raster is preserved for VM crops/recognition; the detector still uses its explicit 640 manifest. An image-generated fictional adult fixture is committed with provenance and hash, not sourced from any real person/account/media collection.

A **new temporary CPython 3.12.3 Linux x86_64 venv** installed 31 fully pinned/hash-checked runtime distributions from the new lock and passed exact-version, import-ownership and dependency-edge validation. Only `onnxruntime-gpu` was installed; the single documented InsightFace dependency substitution avoids CPU/GPU module overlap. The existing shared venv was not modified. The temporary bundle was removed after validation. This verifies Python packages/imports on the target platform; it does not establish managed CUDA startup under the proposed OS sandbox.

Using that clean environment, the same original 1024×1536 synthetic JPEG was passed through the wire-format round trip and real decoder into CPU SCRFD at 640. One face was detected (score 0.851891); bounding boxes and five landmarks matched direct CPU detection exactly (maximum absolute difference 0). The 1280×4096 blank-raster boundary passed, and a false declared width was rejected. Direct CPU detection was ~211 ms; decode + detect ~262 ms on that WSL host, not the production VM.

A separate CPU-only numerical handoff check in the existing environment used the same immutable JPEG and verified both detector and recognition model hashes. CPU FaceAnalysis detection+recognition was compared with `Face` reconstructed from JSON bbox/keypoints followed by `recognizer.get` on the unchanged original raster. The 512-dimensional normalized vectors had norms 1, cosine 1 and maximum absolute difference 0. No identity/reference database, name lookup, classification or archive write occurred; no raw vectors were recorded. This is a synthetic handoff check, not a candidate-accuracy or production-throughput claim.

The systemd 255 unit passed structural verification, substituting only the not-yet-installed executable path to the temporary interpreter. The unit was **not started**; actual kernel/BPF, filesystem/device isolation and managed-provider startup remain deployment gates. Managed release manifests now bind source/lock/model/policy, authenticated health exposes release identity, and an authenticated drain API refuses new computation while existing ownership drains. The follow-up suite passes **25 tests**, including version/policy tampering, locked-runtime validation rules, portrait bounds and drain state through HTTP.

Before pause authorization, a read-only GPU sample showed 603 MiB free. That barely exceeded the 600 MiB initial gate and could not reserve the previously observed 93–260 MiB model-load increment while preserving the running gate. No repeat GPU load or neighbor pause was performed at that point, and no floor reduction or 160 substitution was made. The VM baseline and authorized 640 progress that followed are recorded below; this earlier observation is not the final acceptance status.

## Earlier authorized pause-and-restore window (historical)

The user explicitly authorized a temporary IndexTTS pause and restoration. The release owner alone verified idle counters, armed an independent restore watchdog, stopped/restored the service and confirmed recovery. The first window failed before GPU model loading because the transient launcher could not resolve `nvidia-smi`. A same-policy read-only check confirmed the missing PATH entry; the launcher was aligned with the existing committed service template before the authorized retry. No model option, numerical tolerance or resource threshold changed. TTS was restored successfully between those attempts and again after that retry; the checks for that earlier window are recorded below. The later user instruction to keep TTS off is recorded at the top and in the latest section.

A separate marked VM CPU baseline had completed the real face-stage claim → subprocess → native accepted publication on a fictional two-second portrait video: eight frames/features, empty references, one cohort/run/receipt completion, 20.430 s claim-to-receipt and 733.352 MiB cgroup peak under CPU1/1536 MiB at lower scheduling priority. That is one run under coexisting load, not a throughput distribution. Its exact eight encoded JPEG frames were retained for the GPU comparison.

The authorized GPU retry used the frozen source/lock, a clean isolated runtime and a temporary user service capped at 240 seconds and 1536 MiB. GPU experiment duration was 13.029 s. It was **not the full hardened production unit**: this does not prove its filesystem/device/network sandbox or the intended final private transport. No permanent service was installed and no business media/database/reference-person collection was used.

The actual 640 startup profile recorded **141 CUDA / 12 CPU kernel events**. Positive requests used the same session with runtime fallback disabled. Actual provider options reported `use_tf32=1`, unchanged; this permits TF32 but does not establish that every kernel used it. Eight synchronous loopback requests each returned one positive face and passed the tolerances frozen before execution:

| Check against retained VM CPU result | Maximum absolute difference | Frozen limit |
|---|---:|---:|
| Bounding boxes | 0.000366211 px | 1 px |
| Five keypoints | 0.000122070 px | 1 px |
| Detection score | 0.000000119209 | 0.01 |

The VM runtime (ORT 1.31.1 / NumPy 2.4.6 / OpenCV 5.0.0.93) differs from the WSL GPU runtime (ORT 1.23.2 / NumPy 1.26.4 / OpenCV 4.11.0.86). This is a numerical comparison of those actual stacks on identical bytes, not a hardware-only experiment. Detector-only loopback HTTP timings cannot be divided into the earlier whole face-task timing to claim speedup.

The actual VM transport client used a new container with the existing image and host networking like the worker. **Only frame 0 completed, at 157.48 ms. Frame 1 received `503 resource_busy`; no retry occurred and the other six were not sent.** No VM recognition, queue processing or native publication occurred in this transport-only phase. A separate deliberate eight-request burst completed two requests and rejected six with `429 queue_full`, preserving one execution lane plus one waiting slot.

Aggregate free GPU memory was 4608 MiB before loading and 4303 MiB afterward; host cgroup peak was 1045.164 MiB, with no OOM events. Three remaining service health endpoints stayed HTTP 200. Approximately 0.5-second aggregate utilization samples ranged 1–21%; a separate **post-burst** health query read 82%, model-ready true and accepting false. **No per-request triggering resource snapshot was captured for the VM 503. The later 82% cannot be attributed to that rejection, a neighbor or the probe's own work from these data.** Global coexistence/admission observability and reliable completion remain release findings.

After TTS restoration, the same-VM-runtime CPU recognition comparison was prepared against the frozen cosine/max-difference/norm tolerances. It did not run: the first preflight had about 2302.6 MiB available, below the unchanged 2560 MiB guard; the single authorized fresh check had about 1583.3 MiB available and 5.28% CPU idle, failing the unchanged memory and 15% idle guards. No threshold was lowered, no further poll/retry was made and no comparison container/model was started. The earlier CPU-only handoff equality does not substitute for this still-pending GPU-geometry → VM-recognition vector comparison.

Both benchmark processes exited; owned temporary runtimes, fixtures and credentials were removed. The release owner's final check for that earlier window, **03:04:55 UTC**, confirmed all four services healthy, TTS ready, unchanged service configuration/other-three PIDs, and inactive benchmark units/restore timers. The repository evidence contains no host addresses, private paths, tokens or raw vectors.

At the end of that earlier window, remaining gates included complete eight-frame VM transport with per-admission evidence, the frozen same-runtime vector comparison when resource admission allows, full split-pipeline worker/native-publication timing and recovery, sustained coexistence with TTS restored, actual hardened-unit/private-transport verification and central release/rollback acceptance. Temporary pause authorization did not adopt/exclude legacy caches or authorize cutover. **Result: partial pass for CUDA640/positive geometry/bounded queue; partial failure for VM transport; remaining quality and release checks pending. #19 is not complete.**


## Latest VM-first diagnostic — TTS kept OFF

After explicitly requesting that TTS stay stopped, the user authorized “try now”. The previously cancelled-before-GO preparation was resumed only after fresh checks: TTS inactive/PID 0, other three services healthy, free GPU memory 4498 MiB and utilization 8%. The retained clean locked runtime was revalidated. No service startup configuration, model option, numerical tolerance, memory floor/utilization ceiling, inference concurrency, queue size or legacy-cache state changed.

The ordering changed deliberately: **VM eight-frame back-to-back transport ran first after normal authenticated readiness**, without preceding positive loopback requests. Only after VM success was an eight-frame loopback phase attempted. There were no artificial frame delays, retries or extra concurrent burst. Metadata-only wrappers called the original resource sampler/submit/wait functions once and preserved their values/errors while logging request/frame IDs, UTC, the actual sample and decision; tokens and image bytes were not logged. This ordering/observation change does not establish the cause of the earlier trial's failure.

All **8 VM requests completed**, each with one positive detection on the exact retained JPEG bytes. Client latency was **63.68–134.88 ms**, median **82.01 ms**, and the phase took **730.37 ms**. These are detector-only transport timings, not full worker or throughput measurements. Against the retained VM CPU baseline, maximum differences were bbox **0.000366211 px**, keypoints **0.000122070 px**, and score **0.000000119209**; all frozen tolerances passed. All 16 resource checks associated with these VM requests were recorded, with available telemetry, at least 4193 MiB free and utilization no higher than 11%.

The following loopback phase completed frames 0–2. At **2026-11-08T04:00:02.276638Z**, the actual sample for frame index 3 in `Broker.submit` returned available=true, free=4193 MiB, utilization=40%, busy=true. The same request was immediately refused with **503 `resource_busy`**, reason **`utilization_above_limit`**. Thus this particular refusal was the **40% >25% utilization criterion**, not a memory shortage or unavailable telemetry. Four remaining loopback frames were not sent. The preceding request had completed about 2.868 ms before the sample began, but aggregate device utilization still does not identify which process caused the load. Do not retrospectively assign this sample to the earlier trial's VM rejection.

The actual 640 profile again recorded **141 CUDA / 12 CPU** kernel events in the pinned session. The whole diagnostic lasted **7.922 s**; host cgroup peak was **944.473 MiB** under the 1536 MiB cap, with no OOM. The transient user unit had a 180-second maximum runtime and is not proof of the full production sandbox. Metadata observer errors were empty.

One fresh admission read for the full GPU-detection → VM-recognition → native-publication continuation declined: MemAvailable **1825.99 MiB <2560**, CPU idle **13.23% <15%**, and memory-full avg10 **0.09%** within its 1.5% ceiling. No heavy VM stage, vector comparison or native publication was started, and no further resource polling was done. The complete split-pipeline quality/end-to-end/recovery gates therefore remain open.

The test process exited; its unit was inactive/MainPID 0, VM fixtures and all temporary credentials/GO markers were removed, and one **inactive** clean locked runtime/code cache was retained as requested. It is not a deployed peer. The release owner's final **04:06 UTC** check confirmed TTS still inactive/port closed, the other three services healthy, and test/restore units inactive. TTS was not restarted and no auto-restore timer was created. No permanent startup setting, production cutover or legacy-cache adoption/exclusion was changed.

**Latest result: VM8 transport and geometry passed once; later utilization admission refusal is precisely diagnosed; full worker/vector/sustained-policy/sandbox/release acceptance remains pending.**

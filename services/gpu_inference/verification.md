# #19 isolated verification — 2026-11-08

Status: bounded detector service/client implemented; production integration and full performance acceptance **not complete**. No production service was installed, stopped or restarted. No existing model was unloaded, dependency installed, firewall changed, business DB/media accessed, or real person identified. Only new temporary code/profile files and anonymous zero/gradient/seeded-noise rasters were used.

## Contract and HTTP checks

`python3 -m unittest discover -s tests -p test_gpu_inference.py -v`: **19 tests pass**. Includes actual loopback HTTP transport using a clearly synthetic model fixture. No GPU claim comes from those mock-model tests. Compile check and `git diff --check` also pass.

Covered: strict context and image hash validation; malformed JSON; request bounds; token authentication; handler and inference queue bounds; exact replay versus conflicting/in-flight requests; cancellation retaining running-slot ownership; deadline fencing and late-response discard; resource/readiness distinction; explicit model fault with queued jobs refused; old attempt/coordinator/material/source rejection; invalid geometry and CPU-only response rejection; process generation changes; network failure without retries; busy state preserved through the client.

## Existing GPU runtime, real inference

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

The second 640 probe was attempted only after a fresh headroom sample increased. Its resource gate was not relaxed when it failed. No third 640 run and no higher concurrency were attempted.

CPU-only 640 direct detector times for three anonymous rasters over two passes were **213.8, 200.9, 200.8, 191.2, 192.1, 190.2 ms**. All outputs were empty. Process peak RSS was about 261 MiB. This baseline runs on WSL, **not the production VM**, and excludes image transport/decoding, recognition and candidate matching.

The 160 development probe recorded the same 141 CUDA / 12 CPU readiness events. At equal 160 shape, direct CPU detector times were **13.9, 12.9, 13.1 ms**; GPU HTTP round trips were **89.0, 58.5, 56.1 ms**, with service decode/preprocess/inference work **64.5, 28.4, 30.4 ms**. Encoded frame sizes were 398, 11734 and 77156 bytes. Empty output geometries agreed. Deliberately mismatched dimensions were rejected by the real decoder path.

These paths include different overhead: CPU numbers are direct detector calls, while HTTP numbers include JSON, authentication, admission/resource checks, image decode and response validation. There is **no demonstrated speedup**. Three empty-result fixtures do not establish positive-face accuracy, recall, candidate consistency or sustained throughput. Peak RSS (~844 MiB) includes the isolated benchmark's GPU and CPU sessions/imports together, not a production worker reservation.

All monitored pre/post neighbor health endpoints returned HTTP 200. Existing service PIDs and restart counts were unchanged in follow-up inspection. Raw private endpoint/path evidence, exact resource samples, sanitized logs and the temporary runner are stored in the external implementation context, not this repository. No temporary server or profile directory was left running after the probes.

## Remaining release gates

- Stable resource/coexistence plan; model-ready 640 service must actually admit and sustain requests. No workload may be unloaded without explicit authorization.
- Same marked fixtures and production-equivalent configuration on VM CPU and single GPU; separate transfer, decode, detector, recognition, matching and end-to-end timings, failure rate and reliable peak memory. Higher concurrency only if independent measurements justify it.
- Anonymous positive-face geometry/recognition/candidate tolerance checks; empty synthetic outputs are insufficient.
- Worker-container network path and authenticated remote transport, outage/restart/cancel/late-response behavior across real service/worker processes.
- #13's atomic scoped material/attempt publication contract and cache publication; #18 review continuity; preserved real dashboard run/batch accounting and actual source labels.
- Reproducible isolated environment (resolve overlapping CPU/GPU ORT installations), model license/provenance verification, restricted OS permissions, versioned configuration, drain/start/rollback plan and central GPU/worker/web deployment.

The service's single-lane limit and development shape are implementation constraints, not approval to switch production. #19 must stay open until these gates are satisfied.

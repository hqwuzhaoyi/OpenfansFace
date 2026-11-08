# Isolated WSL release preparation

This directory prepares and validates a release; it does not install or enable a live service. #19 is still blocked on production-equivalent 640 GPU coexistence/performance, a VM CPU baseline and coordinated worker/native/UI verification. Keep existing GPU workloads running unless the user explicitly authorizes a resource-policy change.

## Locked Python runtime

Target is Linux x86_64, CPython 3.12, Ubuntu 24.04 / WSL2. `requirements.in` fixes the observed direct dependencies; `requirements.lock` pins all 31 runtime distributions with SHA-256 hashes. The lock was generated with uv 0.8.22:

```sh
uv pip compile services/gpu_inference/deploy/requirements.in \
  --python-version 3.12 --python-platform x86_64-manylinux_2_39 \
  --only-binary :all: --generate-hashes --no-emit-package onnxruntime \
  --no-header --no-annotate --default-index https://pypi.org/simple \
  --output-file services/gpu_inference/deploy/requirements.lock
```

Updating that lock is an intentional dependency change requiring new validation; never regenerate it during deployment. Hashes prevent package bytes from changing silently. Interpreter, OS/driver libraries and model hashes remain separate release inputs; this is not a byte-identical whole-OS image. Record the exact Python patch, OS, kernel and driver in the release evidence.

InsightFace's upstream metadata declares the CPU distribution `onnxruntime`. GPU and CPU ORT distributions install the **same** Python module and must not coexist. This release intentionally omits the CPU distribution, installs the complete lock with `--no-deps --require-hashes`, and permits exactly one dependency substitution: InsightFace's ORT module is supplied by `onnxruntime-gpu`. `environment.py` checks every other active dependency edge, exact versions, unexpected packages, no CPU distribution, runtime import ownership, and available providers. Ordinary `pip check` will report the deliberately substituted InsightFace dependency; it is not used as the release validator. Do not repair that warning by installing CPU ORT. No third-party wheel/source metadata is patched or vendored.

Build with the target interpreter into a **new, isolated destination**:

```sh
python3.12 services/gpu_inference/deploy/build.py \
  --destination "$GPU_NEW_RELEASE_DIRECTORY" \
  --release-id "$GPU_RELEASE_ID" --revision "$GPU_FULL_GIT_REVISION" \
  --model-sha256 "$GPU_DETECTOR_SHA256"
```

The builder rejects an existing destination, copies only this package, creates a new venv, installs hash-checked binary packages without touching the shared venv, checks it, and writes `environment.json` and `release.json`. It does not copy models or credentials, change a current-release pointer, install a unit, start a service, change networking or initialize CUDA inference. On failure it deletes only the directory it just created. The tested clean build used CPython 3.12.3; future patch-level or system-library changes need release validation too.

Native dependencies include the target WSL driver interface, `nvidia-smi`, and OpenCV's system loader libraries. The clean build must actually import OpenCV and the model module on its target; resolving wheels on a different machine is insufficient. Existing CUDA wheel library preload remains process-local. Model licensing/provenance still requires deployment review; weights are not bundled here.

## Versioned identity and health

`release.json` binds the full source revision, relative source-content digest, dependency lock digest, detector model hash, schema, preprocessing version and fixed admission policy. Manifest schema 2 additionally binds `retry_policy.version=1`: 20-second queue/resource windows, at most two safe pre-execution rearms, 1-second resource polling, eight HTTP sends, 0.5→4-second backoff with 0.8–1.2 jitter and Retry-After capped at 10 seconds (larger hints end the retry loop). Requests still keep their original deadline and lease; see the package README for exact semantics. Managed startup verifies those values and the locked environment before attempting the model. A changed code file, lock or policy refuses managed startup. The managed release manifest supports detection **640 only**; the development 160 mode cannot masquerade as a managed production release.

`GET /health/live` and `/health/ready` include `release.managed`, release ID, source revision and manifest/code/lock hashes. Readiness separately reports actual session providers and startup CUDA profile evidence, model readiness, resource busy state, queue state and `accepting`. Queue acceptance is distinct from `resource_ready`; `resource_waiting` and `waiting_reason` identify an admitted request waiting for counters, while `running` counts only actual model execution. A validated package install or provider list alone is never reported as CUDA execution proof. Do not enable a worker merely because liveness is 200.

Run the control helper from the intended immutable bundle directory with its venv interpreter, so local manifest verification uses that release’s exact code. Authenticated checks use a private token file rather than putting its contents in shell arguments or logs:

```sh
python -m services.gpu_inference.deploy.control health \
  --token-file "$GPU_PRIVATE_TOKEN_FILE" --manifest "$GPU_RELEASE_MANIFEST" \
  --require-ready
```

The command checks remote release/model identity and exits nonzero if readiness is required but false. It defaults to a loopback transport; use an approved private tunnel or TLS origin for remote operation. The control utility never changes system services.

## OS boundary — unit template, not an installed service

`face-gpu.service` targets **WSL2's `/dev/dxg`**. It assumes a root-owned immutable release at `/opt/face-gpu/current`, a separately reviewed model under `/opt/face-gpu/model`, a private token at `/etc/face-gpu/api-token` and nonsecret model hash configuration at `/etc/face-gpu/model.env`. These are deployment-layout names, not observed private host paths. The central release owner must prepare them; this task did not create any of them.

The unit uses a dedicated static unprivileged system user, systemd credentials, read-only system/release files, hidden home/media/state locations, a private bounded temporary filesystem, no capabilities, no privilege escalation, restricted device access, bounded tasks/files/RAM and loopback-only networking. It grants `/dev/dxg` access for CUDA, not business mounts or credentials. GPU memory is not fully isolated by an OS cgroup: the ORT arena cap does not bound driver/context allocations. Keep the admission gate and coexistence monitoring.

A private tunnel terminates at the host's loopback listener. The template does not create a tunnel, TLS ingress, firewall or Windows portproxy rule. `IPAddressDeny/Allow` relies on systemd's kernel/BPF support; structural validation does not prove enforcement. Before deployment, run an isolated fixture under the actual unit and verify denied home/business paths and writes, device access, loopback-only reachability, memory bounds and provider startup. If a kernel cannot enforce a required boundary, retain the deployment block and use an equivalently tested container/namespace boundary; do not silently drop restrictions.

The release owner must provision a **new** dedicated `face-gpu` system group/user, with no home creation, home `/nonexistent`, and shell `/usr/sbin/nologin`. Inspect any existing account rather than silently reusing it. An authorized host preparation may use:

```sh
useradd --system --user-group --no-create-home \
  --home-dir /nonexistent --shell /usr/sbin/nologin face-gpu
```

Keep `DynamicUser=no`, `PrivateTmp=no`, `PrivateMounts=yes` and `RemoveIPC=yes`. Actual systemd 255 probes showed that `DynamicUser=yes` implicitly reinstates `PrivateTmp` and its private ext4 bind over the intended `/tmp` tmpfs even with `PrivateTmp=no`; that earlier layout was writable/executable with host-sized capacity. The static account removes that conflict. `/tmp` must actually be tmpfs, at most 64 MiB by `statvfs`, mounted `nosuid,nodev,noexec`, mode1777; `/var/tmp` is inaccessible. Do not infer these from a unit text or successful writable-file test.

The no-model system-manager rehearsal with these properties verified nonroot UID/GID, no effective capabilities, NoNewPrivileges, read-only system writes, denied home/business paths, `/dev/dxg` open, actual 64 MiB tmpfs and refused execution, plus real permitted VM/loopback and denied other-source network connections. The private rehearsal allowed the explicitly intended VM address; the committed template remains **loopback-only**. Any private LAN deployment must separately freeze its exact listener address and single-VM `/32` allowlist in the operator unit, retain the loopback ranges, and repeat positive and negative connection checks. This is no-model sandbox evidence, not proof of CUDA readiness, full worker completion or deployment acceptance. A `-BPF_FRAMEWORK` version label alone did not predict enforcement on the tested WSL kernel; actual allowed/denied traffic is required.

The template itself participates in `release.code_digest`. After this change, regenerate and verify the release manifest before copying/starting a candidate. Do not label the updated package with an older code digest. Copy the retained locked venv into a new root-owned `/opt/face-gpu` release and copy only the separately hash-verified detector model; private `/tmp` and protected `/home` intentionally make their old preparation paths unavailable. The old environment and model remain untouched. The external operator unit has its own frozen hash; do not overwrite the packaged template inside an already verified release to change the bind address.

The unit deliberately has `Restart=no`. A bad environment, poisoned CUDA session, resource-starved startup or model mismatch must surface to the operator/worker queue instead of producing an unbounded restart/model-load loop. Resource recovery policy and restart sequencing belong to the central release owner. No existing services may be stopped or unloaded as an implied part of preparing this unit.

## Drain, switch and rollback contract

1. Pause new VM GPU-stage claims under the existing task coordinator. Do not change human-review decisions or clear queues. Record the active worker/task generation and current GPU release/instance.
2. Issue the authenticated drain command against the expected release. It prevents **new computations**, lets already-owned queued/running work complete within deadlines, and waits for `drained=true` on the same service instance:

   ```sh
   python -m services.gpu_inference.deploy.control drain \
     --token-file "$GPU_PRIVATE_TOKEN_FILE" --manifest "$GPU_RELEASE_MANIFEST"
   ```

3. Reconcile accepted results through native material/attempt fencing and the real batch ledger. If drain times out, fence old VM attempts before stopping/replacing the service. A cancellation response, disconnected client or expired deadline does not prove an old kernel stopped. Do not overlap replacement processes until the old unit/process is confirmed inactive.
4. Only the central release owner stops the old service, changes the release pointer and starts the reviewed replacement. Token/model storage stays separate from immutable code. Validate exact release ID/hashes, model/provider proof and current resource readiness before resuming explicitly supported worker stages.
5. Rollback also drains/fences first. Restore the previous **compatible** release pointer and worker/web configuration, verify its identity/readiness, then resume claims. Never erase or restore an old processing DB snapshot merely to match a binary. GPU inference is stateless; VM/native receipts and manual decisions remain authoritative. A new GPU instance may recompute a request, so VM CAS still prevents double counting.

The VM image must include `services/gpu_inference/__init__.py`, `client.py` and `protocol.py` from the same compatible source release because `maintenance/processing_gpu.py` imports this package from the repository root. These client modules use only the standard library; the VM does not need the GPU dependency lock merely to send requests.

## Evidence and remaining gates

An isolated temporary Linux venv successfully installed all 31 locked distributions, passed the environment validator, and completed real CPU detector/portrait checks; the live face-scan venv was not changed. The temporary release was then deleted. The original dynamic-user template passed only structural verification and was not a sandbox enforcement proof. The later no-model system-unit rehearsal exposed and fixed the bounded-tmpfs conflict described above, then verified the static-user boundary. Managed CUDA startup and full application behavior under the exact final operator unit still require their own acceptance.

The marked generated fixture under `tests/fixtures/gpu-inference/` is a fictional adult, not a real-person reference. CPU 640 detects one face. Original JPEG bytes survive the request round trip; CPU decode/geometry agrees exactly. The maximum 1280×4096 blank portrait also decodes within bounds. This does not yet establish GPU 640 positive-face agreement, production VM performance, remote worker-container transfer or sustained neighbor coexistence. See the verification record and private integration handoff for exact results and outstanding work; #19 remains incomplete.

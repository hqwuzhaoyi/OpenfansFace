# Native analysis result publication (#19)

`POST /v1/metadata` with `command: "processing-result"` commits a completed VM analysis attempt through the existing Rust `Engine`, durable `Store`, authority lock and bounded writer queue. Python retains task discovery/claim/lease/retry and model execution. This endpoint does not move media, write Peach, create people, associate accounts or confirm ownership.

## Request

The body is closed to unknown top-level/task fields:

```json
{
  "operation_id": "UUID-v4 identical to task.run_id",
  "scope": {"vids": ["stable-video-id"], "people": [], "watermark_keys": [], "all_watermark_rules": true},
  "version": "native state token captured before computation",
  "task": {
    "vid": "stable-video-id", "path": "/configured/media/pikpak/source.mp4",
    "size": 123, "mtime_ns": "123456789", "stage": "text",
    "run_id": "same UUID-v4", "batch_id": "cohort-id",
    "lease_expires_at": 2000000000.0,
    "coordinator_generation": "current VM generation"
  },
  "data": {},
  "next_stage": "face_queued"
}
```

`task.stage` is `text` or `face`; `next_stage` is `face_queued` or `review`. Face results require `reference_input_version` captured through `analysis-state` around building the immutable reference cache. The data object contains only the current analysis contract's `watermark`, `folder`, `text_candidates`, `text_complete`, `face_skipped`, `face`, `face_complete`, `ready`, `kind`, `candidates`, `suggested`. It cannot write manual/deferred/note/queue/creator decisions. Face publication preserves the saved text evidence exactly; text reanalysis discards old face fields. Ready/complete flags must agree with the stage.

Rule saves/removals (including the optional mapping after human confirmation) requeue matching non-manual active tasks from text/face/face_queued/retry/error/review/queued to text, clear the affected run ID/lease, and preserve existing evidence/manual fields. This selected-row CAS closes the gap where a face attempt starts after a mapping change but still carries the previous text interpretation. Manual-takeover, excluded and unrelated tasks remain untouched. Existing queue/history decisions never change; as in the previous rule scheduler, a still-active task for an already handled video may be requeued, and the VM excludes it before model execution. The VM retains claim/lease/retry ownership; late old attempts fail their native lease guard.

OCR scope must set `all_watermark_rules: true`. Its material token includes every mapping, so a newly added key discovered only during OCR cannot slip past a snapshot of previously known keys. Native source resolution now fills IDs missing from the face index from `processing.tasks`, without overriding indexed paths or explicit native path overrides. These task-only IDs also work in native human confirmation/archive. Whole-report/reference tokens exclude task-only discovery paths, stage/progress and unrelated task retries.

## Atomic boundary and recovery

After acquiring the shared gate, the publisher opens `BEGIN IMMEDIATE` on the existing processing database. Under this same transaction it validates the exact active task path/size/mtime/stage/run/batch/lease/generation, current coordinator generation, active run row, matching non-excluded cohort member, manual state, material token, reference token and actual source file. It checks the material/file inputs again immediately before committing. Concurrent VM task writes serialize through SQLite; native file/rule/person operations serialize through the common gate.

The immutable admission evidence stores only the selected video ID's prior/accepted OCR value and the accepted result/group choice. Native publication merges this one key into `watermarks/results.json`, retaining unrelated keys. No attempt artifact URL is authorized merely because files were computed: the VM/web serves only paths explicitly present in an accepted native database receipt, with source identity and fingerprint checks.

One processing transaction writes:

- the selected task's analysis data/stage, error reset, retry time and update time;
- the exact run's terminal `done` status, finish time and actual execution provenance (`unknown` for an unlabelled reused cache);
- evidence flags and first completion time only where the batch member is still `completed=0`;
- one completion event;
- an optional new unknown-face group;
- `_face_admin_processing_results(id TEXT PRIMARY KEY, signature TEXT NOT NULL, result TEXT NOT NULL)`.

All SQL targets and columns are fixed in the native module; there is no client SQL/table/delta interface and no expansion of generic reversible media table capabilities. The journal action is `processing-result` with commit-only intent. It deliberately does not enter the reversible file saga. If the processing receipt exists, later legitimate task stages, leases, coordinator generations or human edits never cause its result to be rolled back. The original receipt is replayed without changing counts, rows or events.

If interruption occurred before the processing commit, recovery restores only the selected OCR key, and only if it is still the admitted before/after value. Other keys survive. A clean abort becomes terminal HTTP 409 `publication_aborted`; the VM may retry through its normal new-attempt policy. A foreign edit to the selected key, unreadable receipt, uncertain persistence or failed restoration retains HTTP 503 `recovery_required` and the shared gate. Exact replay attempts common reconciliation, but does not clear a foreign value. A committed processing result is never undone because its final main-journal response was lost.

Successful responses contain `{ok:true,accepted:true,id,vid,run_id,stage,next_stage,task,source_identity,data}`. Source identities are verified native stat values with decimal-string `dev`, `ino`, `size`, `mtime_ns`; face data also carries `source_identity`. The caller must keep the exact publication body durably and retry the same run ID. Changes to an existing ID conflict. Existing `GET /v1/person-files?id=…` and same-origin `/api/persons/manage?id=…` query the main journal.

## Model artifacts and unknown groups

New feature and thumbnail paths must be under `FACES_BASE/processing/attempts/<run UUID>/`. Prior-attempt reuse requires an accepted native receipt for the same source/model and exact artifact path. Legacy paths are accepted only if they already appear in the stored task's face evidence. New/cached feature files must match `face.feature_cache_sha256`; Rust streams at most 256 MiB and rechecks the bytes at the commit boundary. The VM verifies NPZ model/source metadata and normalized finite embeddings. Rust does not execute model code or parse NPZ arrays.

Reference tokens include actual bytes of accepted attempt caches and legacy `processing/faces/<sha256(vid)>.npz` caches for explicitly queued/archived stable IDs. Receipt source identity, expected hashes and model versions are included. This binds reference building to the accepted evidence without reviving Python writes to the shared face index. A same-filesystem source move can retain dev/inode evidence; a replaced source with the same size/mtime does not acquire that identity.

`face.unknown_group_input` is temporary `{id,version,embedding:[512 finite numbers]}`. Under the same SQLite transaction Rust normalizes the vector, considers only compatible-version groups active in unhandled non-manual `review`/`face` tasks, and matches cosine similarity at least 0.60. Otherwise it inserts a run-bound group ID. It replaces `face.unknown_group` with the actual selected ID and removes `unknown_group_input` from visible task data. Failed/stale attempts cannot insert groups. A model's proposed group ID is never trusted as an instruction to overwrite a stored group.

OCR cache reuse accepts an exact previously published watermark record; absent legacy artifact attempt IDs require exact existing evidence. New evidence filenames are constrained to local JPEG names and resolved inside the declared attempt directory. Manual associations and inference remain separate.

## Compatibility and verification

Native journal schema is 8. Opening a newer schema fails before writing; migration is additive and never resets earlier operations/receipts. The central merger must preserve the #12/#17/#18 tables and handlers while taking the maximum schema version. Worker processing schema/claim semantics remain owned by #19's VM implementation; the native publisher requires its frozen lease/generation/run/cohort fields, rather than silently creating a replacement queue.

`tests/fixtures/processing-publication-schema.sql` is a frozen isolated fixture schema, not a production migration. `tests/http-rust-processing-result.py` exercises real HTTP/SQLite/files with marked fixtures: duplicates and later manual progress; stale files/leases/generations/rules/manual decisions/run/cohort membership; atomic group matching/insertion; malformed vectors/cache paths; pre/post-commit process death; exact replay/reconciliation; selective OCR restoration and foreign-key retention; feature digest/reference invalidation; task-only source confirmation; schema upgrade, newer-version refusal and an actual older binary's refusal.

Run `cargo test`, `cargo clippy -- -D warnings`, and the existing shared-metadata HTTP regression as well. The VM owner's `tests/http-processing-native-worker.py` additionally exercises real process-one publication, recovery, accepted artifact/reference handling and native queue/archive with no face-index row. These are isolated contract checks with controlled model outputs, not GPU throughput or production deployment evidence. No production state was changed by this implementation.

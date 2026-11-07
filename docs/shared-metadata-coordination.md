# Shared metadata authority (#13)

With `FILE_OPERATIONS_BACKEND=rust` and `FILE_OPERATIONS_ENABLE_RUST=1`, the existing account, watermark and recommendation URLs forward business writes to the native executor. They retain their result/error shapes and add a stable `operation_id` on transport or backend errors. No failed native request falls back to a Node/Python business write. UI components and manual-versus-inferred classification are unchanged.

## Native protocol

Authenticated `POST /v1/metadata` accepts `{command, body}`. Mutation bodies may provide a UUID-v4 `operation_id`; otherwise the caller/server allocates one. Exact repeated ID/content returns the durable result. A changed payload using that ID returns 409. `GET /v1/person-files?id=…` queries the existing operation receipt; `metadata-write` uses the same authority, bounded writer lane, recovery fence and `.review-write.lock` as media operations. Request bodies are bounded at 8 MiB; persisted domain snapshots remain bounded at 64 MiB.

| Command | Body | Persisted effect |
| --- | --- | --- |
| `account-link` | `name, profile_id, handle` | Explicit manual link from the verified local public-directory cache; no video claim or mapping |
| `account-unlink` | `name, link_id` | Remove only the current person's matching manual link |
| `watermark-set` | `key, kind: person/source/remove, name?, candidate?, evidence_vid?` | Save/remove the exact rule; candidate must still exist in the selected, file-fingerprinted local evidence |
| `recommendation-reject` | `name, vid, path, size, mtime_ns` | Fingerprinted rejection of a still-available recommendation |
| `recommendation-undo` | `name, vid, rejection_id` | Remove only the matching rejection receipt |
| `template-set` | `name, groups: number[]` | Save explicit template group indexes |
| `person-remember` | `name, thumb?` | Remember the canonical person and explicitly clear its deleted marker |
| `state` | `scope: {people?, vids?, watermark_keys?}` | Read `{version,scope,material}` |
| `check-state` | `scope, version` | Validate the same scoped material; read-only |
| `analysis-state` | `{}` | Read a conservative whole-report input version |
| `analysis-publish` | `version, outputs` | Publish only `report.json` and present optional `claims-detail.json`/`promoted.json` |

All mutations also accept optional `scope` plus `expected_version`. Names resolve through native alias validation again during execution. Foreign edits to accepted document before-images fail CAS; compensation never overwrites an unrecognized current document. Errors include `invalid_request` (400), `missing_person` (404), `material_changed` / `conflict` / `writer_busy` (409), and `recovery_required` / `outcome_unknown` (503). An uncertain response must be resolved using the original ID.

`state` includes the selected stable IDs' actual source paths/fingerprints, selected manual task fields (`manual`, `deferred`, `note` where present), canonical name resolution, relevant explicit account/rule/template/deleted/rejection records, and selected queue/archive/review decisions. It excludes task stage/retry/progress, candidate ranks, report regrouping, person recent-use timestamps and another video's confirmation. A server-owned review session must keep its accepted scope and seen evidence: a client-supplied arbitrary scope is not authorization. #18 can call `shared_metadata::material_state` / `check_state` inside its own native admission gate. A separate HTTP check followed by a business write is not atomic.

## Calibrator computation and publication

Both calibrator entrypoints call native `template-set`, then run the existing model script in a single bounded background runner with a unique temporary `F_OUT`. Before computing they capture `analysis-state`; after successful computation they send the generated report and only the optional files actually produced to `analysis-publish`. Missing optional outputs retain the previous authoritative files. A later template/rule/person/file/index change rejects the old output. Repeated picks while computing request another pass; they cannot publish an obsolete version over a newer decision. The rebuild status reports errors and the publication operation ID.

`analysis-state` hashes all shared business documents, current report outputs, source paths/fingerprints and a consistent read of face/index tables. Face data is streamed through a digest rather than loading every embedding into JSON. This is intentionally conservative for a whole-report rebuild. It does not version independent task progress. The Python algorithm remains Python; no private deployed script is copied into this repository. The checked-in script additionally refuses direct BASE output under Rust mode.

The calibrator requires `FILE_OPERATIONS_RUST_URL` and the matching token. Deployment must give the compute sidecar read-only input/business mounts and writable temporary space, verify its actual deployed script honors `F_OUT`, and route scheduled/manual invocations through this coordinator. The private runtime script's supported `F_OUT` contract was inspected separately. Setting only an environment variable is not a filesystem permission boundary. Restarting a compute sidecar loses transient compute status; no partial report was published, and saved template/possible publish outcomes remain queryable in native receipts. Recovery does not silently rerun a model.

## Writer inventory and cutover ownership

| Existing writer | Coordination under native ownership |
| --- | --- |
| `lib/person-accounts.ts`, `/api/persons/accounts` | Native account-link/unlink; read/search/cache remains in Next |
| `/api/persons/recommendations` | Native reject/undo, same source fingerprint and matching undo ID |
| `/api/watermarks` PUT | Native mapping save/remove; after success, existing independent task scheduler may request re-analysis; scheduling failure returns a warning without losing the saved rule |
| `lib/people-catalog.ts:rememberPerson` | Queue/claim native operations already remember people in their transaction; typed person-remember covers other explicit callers. Old direct helper fails closed under Rust |
| `/api/plan/queue`, `/api/claims/decide` | #5 native queue/decision/people transaction; optional rule and task restoration have separate bound receipts |
| `/api/plan/resolve`, `/api/plan/apply`, `/api/plan/progress` | #4/#5 native archive state, file/ledger execution and receipts |
| Person file/manage/batch operations | Native domain transformations include manual/reviewed links, rules, templates, people, aliases, rejections, report references and path overrides; merge/delete/batch integration remains with #10/#11 |
| `sidecar/calibrator.py`, root `calibrator.py:identity_pick` | Native template persistence; isolated Python computation; native versioned output publication |
| `identify_faces.py` direct invocation | No direct authoritative output under Rust; deployment must retire/gate unwrapped private/scheduled invocations |
| `lib/jsonio.ts` old shared JSON helpers | Closed list rejects direct business writes under Rust, including reviewed-links and report/claims/promoted. Ownership switch back to legacy remains a coordinated release operation |
| Offline reviewed-profile import | No active runtime writer in baseline. Existing uncertainty-bearing records migrate with native identity operations; future imports must be integrated with typed native commands before enabling them, never run old deployment scripts against live Rust-owned data |
| `lib/creator-directory.ts` | Public directory fetch and cache only; not a manual association or ledger writer; deliberately unchanged |
| `maintenance/processing_store.py`, processing worker, `/api/processing` | Independent analysis queue/progress remains owned by the operations/#19 thread. Exact affected native task rows use transaction CAS; unrelated task progress is not blocked. No wholesale queue rewrite in #13 |
| `maintenance/processing_faces.py` | Model NPZ/reference caches are analysis cache, not business JSON or Peach writes |
| Legacy watermark worker job/results/progress | Separate analysis evidence, stable video/file fingerprints; does not write mappings/manual links/claims. Existing automatic-processing gate disables obsolete scan admission when its DB is present |
| `maintenance/video_health.py`, `/api/video-health/report` | #17 owns migration to native health scheduling/publication, with file fingerprint checks; mandatory final cutover gate |
| `/api/delete`, legacy `/api/plan/rollback` | Retired/gated by preceding migration tickets; no direct rename/Python ledger parity fallback |

#19's attempt/lease CAS and GPU result handling remain part of the analysis queue owner. It must submit any shared JSON through native publication and atomically validate its selected task delta and material token in a native domain transaction when publishing against shared rules. A preflight HTTP check alone cannot make a later independent business write safe. #17/#18/#19 and the current parallel operations implementation must be jointly verified before #14 cutover; this change does not authorize deployment.

## Verification

- `cargo test --manifest-path services/file-operations/Cargo.toml`: 56 tests passed.
- `python3 tests/http-rust-shared-metadata.py`: nine real HTTP scenario groups: aliases and save/rename races; scoped material/nonmaterial changes; mappings/rejections/templates; optional report preservation and foreign changes; crash receipts and shared locks; overlapping save/rename; actual Python F_OUT handoff; interrupted multi-document publication with foreign-edit recovery fence; claims/promoted array publication recovery.
- `pnpm build` and `tsc --noEmit`: passed using an isolated worktree output.
- `python3 tests/http-rust-shared-metadata-gateway.py`: compiled Next same-origin account save/unlink, recommendation reject/undo, mapping save/remove, native conflict and operation ID propagation passed.
- `python3 tests/http-rust-person-rename.py`: existing native rename/metadata/index/Peach, affected-candidate, fault/restart and guarded-foreign-edit scenarios passed.
- `node --experimental-strip-types --test tests/person-accounts.test.mjs`: three manual/inferred/corrupt-record tests passed.
- Python calibrator/runner/script compilation passed. No model accuracy claim, production mutation, deployment, or new UI behavior is included.

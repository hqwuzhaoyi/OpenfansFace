# Native archive conflict resolution (#6)

Implemented on the validated #5/#7 native-operation baseline. No production writes,
deployment, push or PR merge were performed for this ticket.

## Contract

`POST /api/plan/resolve` uses the native `/v1/archive` command `archive-resolve`
when the existing Rust switch is enabled. `rename`, `stage-duplicate` and `clear`
are version-checked, zero-file operations in the existing durable Engine. They
persist queue intent only. A failed native request never executes a legacy fallback.

The preview includes `suggested_name`, `original_size`, canonical target fingerprint
and the exact keep-both destination. Suggested names retain the established
` (保留两份-<8 hex>).ext` convention and 240-byte filename limit. Apply keeps the
existing selected-subset semantics and durable result/progress endpoint.

Duplicate execution reads both complete files through pinned, non-symlink paths,
compares every byte and SHA-256, and checks source/target fingerprints before and
after reading. ctime additionally guards edits during comparison, including edits
that restore mtime. Only proven unchanged failures may be skipped. Canonical
fingerprints are checked again before removing the source and before aggregate
ledger commit. Canonical non-regular targets, including FIFOs, cannot block a read.

The source is moved with the existing no-replace filesystem/recovery engine into
`pikpak/.face-cleanup-staging/archive-duplicates/<operation>/<source hash>/<name>`.
Its stable IDs and source aliases follow the staged file. Existing canonical IDs
and file bytes stay at the original target. Archive history retains canonical
`dst`/`win_dst` and adds the actual `staged_path`; receipt `locations` reports the
actual moved-file paths. Peach source assets follow staging; both source and
canonical assets retain/add the confirmed creator relationship. Canonical asset
and link guards participate in the same native receipt transaction. Multiple
sources sharing one canonical asset create its relationship only once.

`staging-origins.json` is part of the same aggregate local metadata snapshot and
compensation as history, queue, faces, tasks and path overrides. It is keyed by
absolute staged path and records:

```json
{
  "operation": "<operation UUID>",
  "action": "stage-duplicate",
  "original_path": "<original unorganized absolute path>",
  "person": "",
  "vids": ["<source stable ID and source aliases>"],
  "source": "native-receipt"
}
```

The shared `ScopeRoot::Staging` unit variant maps to
`config.media/pikpak/.face-cleanup-staging`, matching ticket #8. A ledger failure
compensates the whole successful subset and all local metadata; external late
targets are preserved. Admission/preflight failures return an explicit HTTP error;
post-admission failures retain the existing durable archive result semantics.

## Verification

The fixtures are marked `/tmp/face-admin-person-fileops-archive-*` directories
with real files, native processing schema, faces SQLite and the observed Peach
schema. The Rust service runs with `PATH=/no-executables`. Python only seeds,
drives HTTP and asserts results.

- `cargo test --manifest-path services/file-operations/Cargo.toml --locked`:
  54 tests passed.
- `cargo clippy --manifest-path services/file-operations/Cargo.toml --locked --all-targets -- -D warnings` passed.
- `node node_modules/typescript/bin/tsc --noEmit --incremental false` passed.
- `python3 tests/http-rust-archive-resolution.py --port 18983` passed: safe names,
  extension protection, intent-only resolution/clear, keep-both, exact duplicates,
  multi-buffer comparison with a differing last byte, equal-size unequal content,
  partial completion, source aliases, canonical references/creator relationships,
  multiple sources sharing a new canonical creator, source/target mutation while
  reading, restored-mtime mutation, canonical symlink/FIFO, missing canonical
  ledger, late keep-both/staging files and symlinks, full successful-subset ledger
  rollback and canonical mutation immediately before ledger commit.
- The same suite exits/restarts the real process at comparison start, comparison
  chunk, comparison completion, destination publication, source removal, origins
  JSON replacement, ledger commit and final receipt, plus `copy_created`,
  `copy_verified`, `copy_publish_intent`, `copy_published`. It verifies exact
  contents, metadata, one durable receipt and idempotent replay. A fixture-only
  `force_copy` flag exercises the actual copy/recovery implementation on one
  filesystem; this is not a claim of physical cross-device or NFS validation.
- `python3 tests/http-rust-archive.py --port 18983` passed the existing #5 suite:
  confirmation/undo/claim, optional rules, stale scopes, normal archive, native
  ledger faults, successful subsets, concurrency, fencing and restart recovery.
- `python3 tests/http-rust-archive-resolution-gateway.py --port 18983` passed
  through Next on 18982, including resolve/preview/apply/progress and HTTP 206 Range
  for the stable source ID, canonical file and staged path. Use
  `ARCHIVE_VIDEO_SAMPLE` for a real playable MP4; default `/tmp/persons-flower.mp4`.
  `ARCHIVE_UI_HOLD=1` leaves the isolated fixture available for browser checks.
- Python syntax, Ruff formatting/unused-import checks and `git diff --check` passed.

## Existing UI verification

The existing resolution dialog, wave list, confirmation and result/history views
were exercised through agent-browser against fixture
`/private/tmp/face-admin-person-fileops-archive-7m6i5to9`.

Custom keep-both naming saved without moving media. Cross-wave selection survived
resolution and a search hiding two selected waves. Submitting all three produced
two successes (one renamed, one staged) and one full-content mismatch; the failed
item remained queued and selected. The result kept “查看归档位置” and “查看失败项”.
History displayed canonical and duplicate-staging paths. Actual video playback
advanced on both source and canonical history files (5.055 s sample), alongside
separate Range assertions.

Desktop and 390×844 layouts, light/dark appearance, keyboard Escape/Enter and
`prefers-reduced-motion: reduce` were checked. The resolution dialog previously
lost focus to BODY on close; it now captures the opening control and restores it,
with the existing search input as fallback when a filter removes that control.
No new UI component system was introduced. Screenshot evidence is local at
`/tmp/openfansface-rust-implementation/context/ticket06-browser/` (desktop,
narrow-dialog, partial-result, history-location). No runtime fixture or screenshot
is treated as production validation.

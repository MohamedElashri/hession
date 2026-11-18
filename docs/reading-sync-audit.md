# Reading layout and refresh audit

This audit covers the reading and sync changes in v0.5.4 after v0.5.3 (`66a4047`). The
baseline executable is v0.5.3 plus the same synthetic private-capture probe; the
candidate includes the reading and sync implementation. Timings were recorded
before the release version bump.

## Behavior

- Focus mode hides surrounding panels and bars while retaining conversation
  search, pagination, and an exit button. It preserves transcript DOM, page,
  and scroll. Navigation/list dividers support dragging, keyboard steps, and
  reset; Layout saves independent panel visibility and reading width.
- Library refresh requests share one admitted operation and at most one pending
  follow-up. Manual refresh progress no longer launches separate duplicate
  list/detail/metadata refresh loops. Hidden windows defer UI polling and catch
  up when visible; native local observation continues.
- Session read tokens cover content parts, entries, tools, context, annotations,
  tags, session metadata, and referenced metadata. Tokens include page and limit.
  An unchanged selected conversation avoids transcript projection and IPC of its
  bodies. Missing or changed tracking disables reuse; deletion is explicit.
- Private sync reuses those change tokens, namespaced by portable content policy.
  Quiet capture avoids reading, redacting, serializing, or encrypting unchanged
  histories. Changed capture prepares from a read snapshot outside the write
  gate, stages only ciphertext in a buffer capped at 4 MiB, spilling larger
  revisions to an automatically removed private temporary file, and rechecks content/identity before queuing atomically. Equal-content
  cache refreshes preserve the actual published revision identity.
- Portable content queries reuse statements; unchanged redacted strings retain
  their allocations. Digest serialization streams without a whole JSON copy
  and preserves the v1 length-prefixed digest. Chunk encoding yields one base64
  chunk at a time. Download assembly reads one stored chunk at a time, and
  normalized download inserts reuse statements.
- Private progress polling reads only an in-memory status snapshot, with one
  request admitted at a time. It no longer rescans workspace counts every 800 ms.
- Local manual imports commit before SSH begins. Remote refresh uses at most two
  host workers, preserves previous mirrors on failure, and reports worker
  failures. Discovery and CLI pricing state are reused within each pass.

## Measurements

Raw reports and preserved executables live in ignored
`target/performance/reading-sync/`. Run the documented probes serially after
builds and tests finish. Histories, keys, relay credentials, source roots, and
browser profiles are synthetic/disposable; no personal library is used.

Measured on Linux x86-64, AMD Ryzen 7 7735H, using optimized release test
executables with build/test workers stopped. Other desktop activity and storage
latency introduce variance. These are synthetic warm-cache measurements, not
network or cold-start timings. All portable content digests match the baseline.

Private projection and quiet-capture medians pool 21 retained samples per case
(three fixtures, each with three warmups and seven measured iterations). Initial
and edited capture values are medians of three independent fixtures.

| Private capture (ms) | Events | v0.5.3 | Candidate |
| --- | ---: | ---: | ---: |
| Projection + digest | 1,000 | 30.22 | 10.55 |
| Unchanged capture | 1,000 | 33.66 | 2.10 |
| Initial capture | 1,000 | 77.64 | 74.84 |
| Edited capture | 1,000 | 75.50 | 56.58 |
| Projection + digest | 10,000 | 301.22 | 99.45 |
| Unchanged capture | 10,000 | 315.06 | 2.16 |
| Initial capture | 10,000 | 680.79 | 651.54 |
| Edited capture | 10,000 | 599.54 | 500.49 |

Across each complete private probe, including fixture construction and both
history sizes, median process CPU is 11.90 → 7.76 seconds and maximum resident
memory is 102.6 → 97.9 MiB. Initial/edited captures include storage work;
see the retained samples for their spread.

The paired 10,000-entry refresh probe returns a 500-entry page. Full projection
and JSON encoding take 50.62 ms with 3,028,738 response bytes; an
unchanged-token response takes 1.95 ms with 167 bytes.

| Source import (median ms) | Historical records/calls | v0.5.3 | Candidate |
| --- | ---: | ---: | ---: |
| Ordinary ten-event append | 1,000 | 3.57 | 3.39 |
| Forced full reimport | 1,000 | 140.37 | 158.78 |
| Ordinary ten-event append | 10,000 | 8.72 | 9.62 |
| Forced full reimport | 10,000 | 3699.97 | 2370.41 |
| Ordinary ten-event append | 100,000 | 73.74 | 161.00 |
| Forced full reimport | 100,000 | 21035.90 | 27466.12 |
| Tool-heavy append | 1,000 | 4.64 | 4.83 |
| Tool-heavy append | 10,000 | 26.13 | 26.31 |

Ordinary appends and full reimports retain five samples per size; tool-heavy
appends retain eight. Source timings include integrity hashing and SQLite commit
work. They do not show uniform improvement: change tracking adds derived writes,
and large-file append/commit timings vary substantially. No source-import
speedup is claimed. The complete ordinary-import probe consumes 127.46 →
134.35 process CPU seconds, including fixture creation and forced rebuilds. Its
wall time is 143.00 → 165.60 seconds. The intermediate candidate, with identical
importer/change-tracking code, measured 22.21 seconds for the 100k full-reimport
median versus the final run's 27.47 seconds; storage/scheduling variance is
material. Raw samples and CPU/RSS summaries are retained.

The native observer's 30-second quiet probe uses 0.04 →
0.04 process CPU seconds (0.13% →
0.13% of one core). Revision/event assertions remain unchanged.

Validation: `make verify` passes, including 284 Rust tests and 279 JavaScript
tests, formatting, ESLint, ShellCheck, Clippy with warnings denied, unused-code
checks, frontend build, and release metadata agreement. Ignored benchmarks run
separately; the final browser reading/pointer/mobile check also passes.

## Reading UI verification

The production App browser fixture uses a 1,000-entry synthetic conversation,
showing a 500-entry page (250 turns), on Chrome 158/Linux. At a 1,280 px viewport,
focus expands the detail panel from 660 px to 1,280 px. The same transcript and
first turn remain mounted and scrollTop stays 300. A real pointer drag changes
the list from 360 px to 400 px and persists on release. One hundred keyboard
divider adjustments cause zero transcript mutations and zero detail requests.
The 390 px
viewport shows the selected conversation without horizontal document overflow;
Browse sessions returns to the list. Light/dark and normal/focused phone
screenshots are saved with the report.

These checks establish layout and DOM behavior. They do not establish native
WebKit frame rates or cold-start performance.

## Correctness and limits

Regression checks cover content-part-only edits, unrelated-session writes,
tool/annotation/tag changes, page/limit changes, deletion and row-ID reuse,
rollback, maintenance epochs, missing triggers, corrupt nonce state, counter
rollover, legacy private-sync schema upgrades, and equal-digest publication
ancestry. Refresh tests cover bursts, failures/retry, latest worker selection,
unmount/StrictMode replay, hidden-window catch-up, lightweight single-flight
progress, and preserving the visible library after a transient refresh failure.
Existing tests exercise native source integrity, cancellation, replay, partial
Unicode chunks, scope revocation, relay rollback, and wrong keys. The isolated
SSH fixture requires the local history to be committed before SSH can run.

Private transfers still publish full changed revisions under protocol v1; these
changes do not provide delta transfer or remove network latency. Invalid derived
tracking falls back to full guarded capture. Shared reference-table changes
conservatively invalidate tokens across sessions. Source integrity still verifies
historical prefixes and same-stat rewrites, so content audits remain proportional
to source bytes. A two-host limit bounds resource pressure and can reduce total
throughput when many independently fast hosts are available. Focus mode does not
remove the existing 500-entry transcript page cap.

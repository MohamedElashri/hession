# Patch performance audit

Tested on 18 November 2026 with synthetic data, optimized Rust executables, and
production React components in isolated headless Chrome.

The patch removes repeated work in session browsing, filter loading, incremental
imports, search pagination failures, and repeated large tool arguments. It also
fixes failures found while exercising those paths.

## Measurements

Medians in milliseconds; lower is better. These catalog timings include exact
counts and page details, not just the ID lookup.

| Sessions | Catalog operation | Before | After | Speedup |
| ---: | --- | ---: | ---: | ---: |
| 10,000 | visible-recent | 20.42 | 2.02 | 10.1× |
| 10,000 | all-recent | 21.41 | 2.76 | 7.7× |
| 10,000 | visible-oldest | 23.49 | 2.16 | 10.9× |
| 10,000 | visible-deep-page | 22.31 | 2.24 | 10.0× |
| 10,000 | visible-most-messages | 20.12 | 2.67 | 7.5× |
| 10,000 | visible-transcript-search | 20.90 | 4.74 | 4.4× |
| 10,000 | missing-transcript-search | 20.18 | 3.56 | 5.7× |
| 10,000 | visible-project | 19.58 | 2.06 | 9.5× |
| 10,000 | startup-projects | 2.49 | 2.30 | 1.1× |
| 10,000 | startup-stats | 1.87 | 1.60 | 1.2× |
| 10,000 | startup-facets | 24.86 | 1.43 | 17.4× |
| 100,000 | visible-recent | 191.54 | 2.81 | 68.1× |
| 100,000 | all-recent | 207.84 | 9.00 | 23.1× |
| 100,000 | visible-oldest | 235.87 | 3.10 | 76.0× |
| 100,000 | visible-deep-page | 188.63 | 3.31 | 57.0× |
| 100,000 | visible-most-messages | 184.56 | 9.43 | 19.6× |
| 100,000 | visible-transcript-search | 195.08 | 20.81 | 9.4× |
| 100,000 | missing-transcript-search | 188.47 | 24.03 | 7.8× |
| 100,000 | visible-project | 186.57 | 3.15 | 59.3× |
| 100,000 | startup-projects | 14.29 | 12.22 | 1.2× |
| 100,000 | startup-stats | 6.45 | 5.05 | 1.3× |
| 100,000 | startup-facets | 232.96 | 1.33 | 175.1× |

The visible-only developer search in the 100,000-session fixture previously
failed its metadata preflight. It now succeeds (116.16 ms median). That change
is a correctness fix, so its before/after durations are not a speedup claim.
The corresponding 10,000-session search improves from 54.43 to 13.94 ms.

| Search rejection (10,000 sessions, 50,000 entries) | Before | After |
| --- | ---: | ---: |
| malformed-cursor | 26.95 | 0.74 |
| changed-query-cursor | 24.95 | 0.91 |
| stale-cursor | 25.00 | 0.86 |

All twelve comparable public-search result hashes match. Their medians range
from 28.45–326.77 ms before and 24.55–297.01 ms after; the largest
improvements concern rejected cursors and the default helper-excluding view.

| Ten-event append | Before median | After median | Before slowest | After slowest |
| --- | ---: | ---: | ---: | ---: |
| 1,000 completed tool calls | 92.01 | 4.16 | 98.42 | 5.49 |
| 10,000 completed tool calls | 927.26 | 22.22 | 996.82 | 125.81 |
| 1,000 ordinary records | 3.90 | 3.80 | 5.21 | 4.14 |
| 10,000 ordinary records | 12.97 | 8.83 | 216.05 | 624.83 |
| 100,000 ordinary records | 203.79 | 168.17 | 236.84 | 170.95 |

Tool-heavy appends retain eight samples; ordinary appends retain five. Storage
work varies: the 100,000-record full-reimport median changes from 25.59 to
20.80 seconds, with candidate samples spanning 19.80–29.42 seconds. No broad
full-import speedup is claimed from that result. A 10,000-record append also
spiked to 624.83 ms, versus a 216.05 ms baseline maximum. The median gains do
not establish consistently low tail latencies.

| Fifty collapsed 1 MiB tool cards | Before commit | After commit | Before retained MiB | After retained MiB |
| --- | ---: | ---: | ---: | ---: |
| malformed | 66.40 | 5.10 | 0.97 | 0.97 |
| path | 40.10 | 4.90 | 50.98 | 0.96 |
| code | 41.50 | 4.90 | 50.98 | 1.97 |
| malformed-unique | 66.10 | 7.80 | 0.96 | 0.98 |
| path-unique | 45.60 | 41.40 | 50.97 | 50.99 |
| code-unique | 44.70 | 43.50 | 50.98 | 51.00 |

Repeated valid path/code payloads retain 96–98% less component/cache heap.
Unique valid inputs retain about 51 MiB in both versions: decoding and displaying
distinct large inputs still has a cost. Indexed-card controls stay around
4–15 ms, and the live-parent refresh control remains 0.60 ms.

The quiet-watcher probe consumes 0.04 process CPU seconds over thirty seconds
in both versions (about 0.13% of one core), with unchanged revision and no
notifications. Candidate RSS is 15.1 MiB versus 14.3 MiB before. Eight stuck
syntax jobs fall back through one worker timeout at approximately 2.5 seconds
in both versions. Export cancellation and size-limit probes pass and leave no
partial destinations.

The isolated metadata-size guard probe processes 1,000 records per sample.
It compares full JSON serialization with the new exact cached-field accounting;
these are guard costs, not end-to-end import latencies. Eight samples follow
three warmups, alternating reference/candidate order.

| Stable header payload | Old guard (ms) | New guard (ms) |
| --- | ---: | ---: |
| 4 KiB | 2.87 | 0.27 |
| 32 KiB | 18.15 | 0.27 |
| 1,024 KiB | 533.17 | 0.25 |


## What changed

- Session browsing selects the page's IDs before loading previews, model and
  branch details, and annotations. Covering indexes serve the default view,
  agent filters, and project filters. Ordering-index scans avoid a bad plan
  caused by skewed sampled SQLite statistics. Exact counts avoid unused joins.
- Filter options use indexed distinct values and existence checks instead of
  joining every recorded context and tool call to a wide session row.
- Imports correlate results only for calls affected by the incoming records.
  Durable entry metrics advance with the append instead of recounting all old
  message text. Tool totals use narrow indexes instead of reading historical
  result bodies. The exact metadata-size guard caches serialized sizes of stable
  fields instead of serializing the whole header for every event. Late results
  still update tool status and analytics atomically.
- Search rejects expired or mismatched cursors before scanning history. Its
  size preflight uses the same subagent visibility filter as result selection.
- Tool cards reuse bounded previews for repeated large argument strings.
  The cache retains display strings, not arbitrary parsed JSON trees, and is
  limited to four entries and `4 * 1024 * 1024` retained UTF-16 code units. A short
  lookup hint always verifies equality against the complete input before reuse.
  Complete argument formatting stays lazy and local to the expanded view. Obviously
  incomplete large JSON envelopes can remain plain text without a failed parse.

## Failure paths

- Excluded subagents no longer consume the visible search's metadata budget or
  make otherwise searchable primary conversations fail on an oversized cell.
- Corrupt or missing local append summaries rebuild automatically while
  preserving session IDs, stars, and tags.
- Library statistics propagate storage errors instead of reporting false zeros.
- The live window includes JSON brackets and commas in its byte budget.
- Regression checks cover Unicode and NUL character counts, result-before-call
  ordering, changing tool exit status, restart, cancellation, rollback, stale
  cursors, denied workspaces, orphaned evidence, and old portable backups.

The schema migration adds the indexes transactionally, including upgrades from
the earlier classification-only schema. Existing histories and annotations are
preserved.

## Method

The host is Linux x86-64 on an AMD Ryzen 7 7735H with sixteen logical CPUs and
approximately 60 GiB of system memory. Browser probes use Chrome 158.0.8127.36.

The development baseline is the working tree after the session/subagent fix,
before this performance audit. Baseline and candidate test executables were
preserved. This comparison is not presented as a measurement of an untouched
published installer.

- Catalogs contain 10,000 or 100,000 sessions, with 90% newer background
  sessions. Each session has an entry, context, tool invocation, file reference,
  and approximately 1 KiB of raw metadata. Browsing returns twenty rows. Catalog
  and public search measurements discard three warmups and retain eight samples.
- Ordinary append probes start at 1,000, 10,000, or 100,000 records and append
  ten events per sample. Five samples also check full reimport and entry counts.
  Tool-heavy probes have 1,000 or 10,000 completed calls with 1 KiB results,
  then append ten ordinary events eight times.
- Browser cases mount fifty collapsed tool cards with approximately 1 MiB of
  arguments each. Repeated and unique payloads are separate cases. Seven samples
  follow three warmups. Each workload starts in a fresh page; input strings are
  flattened before the initial garbage collection. Heap deltas describe retained
  JavaScript component/cache memory, not total browser memory.
- The watcher probe measures thirty seconds of process CPU across two metadata
  reconciliation intervals, using 100 source files and 1,100 entries. It checks
  that the library revision and notification count remain unchanged.

Probes run serially after compilation and other tests finish, without globally
dropping filesystem caches. These are warm-cache Linux measurements; the Chrome
figures do not establish native WebKit or macOS rendering performance. CPU
samples accompany catalog and public-search wall times. Raw samples, browser
versions, host details, logs, and preserved executables are in
`target/performance/patch-audit/` locally.

Returned-row hashes check equivalence across comparable catalog and public
search cases. The intended exception is the large visible-only search that used
to fail its size preflight. Archive failure probes check cancellation, limits,
and absence of partial destinations. No personal histories are used as fixtures.

## Reproduce

See [development benchmarks](development.md#performance-measurements) for the
existing transcript, browser, search, export, and watcher probes. Additional
release-mode test probes are:

```sh
PKG_CONFIG=/usr/bin/pkg-config cargo test --release --locked -p app --lib --no-run
cargo test --release --locked -p hession-core --test search_performance --no-run
```

Preserve the emitted executables for each revision before timing. Run the
selected executable with the exact test name followed by
`--ignored --exact --nocapture --test-threads=1`:

| Test | Output/configuration environment |
| --- | --- |
| `commands::catalog_benchmark::library_catalog_performance` | `HESSION_CATALOG_BENCH_DIR`, `HESSION_CATALOG_BENCH_OUTPUT`; optional comma-separated `HESSION_CATALOG_BENCH_SIZES` |
| `importer::indexing_scaling_benchmark::tool_result_append_scaling_benchmark` | `HESSION_TOOL_APPEND_OUTPUT` |
| `importer::indexing_scaling_benchmark::fixed_ten_append_scaling_benchmark` | `HESSION_INDEX_SCALING_OUTPUT` |
| `importer::indexing_scaling_benchmark::quiet_watcher_cpu_benchmark` | `HESSION_INDEX_IDLE_OUTPUT` |
| `importer::incremental::tests::summary_budget_performance` | `HESSION_SUMMARY_BENCH_OUTPUT`; isolated old-serializer versus cached-size comparison |

Use new synthetic catalog directories when comparing different schema versions.
The logical records must match; physical indexes are part of the change being
measured. `benchmark-browser.mjs --source-dir ARCHIVED_SRC` compares an archived
production source directory against the working tree without importing personal
preview fixtures. `--unhappy` includes repeated and unique payload controls.

## Costs that remain

Additional browsing and facet indexes consume storage and add maintenance work
on writes. The 100,000-session synthetic catalog grows from 209.6 to 231.2 MiB
(about 10.3%); the import measurements include those indexes. This is a catalog
fixture with small transcripts, so that percentage does not predict every real
library's growth.

Verified prefix hashing still reads the committed source bytes before an append
can commit. Append latency therefore grows with source size, even when no old
tool results or message text are recounted. Full reimports still rebuild derived
rows and indexes and have substantial storage cost.

Large valid arguments that have not been seen before still need decoding to
produce an accurate preview; the unique-payload controls expose that cost. The
repeated-payload improvement should not be described as accelerating every
possible tool call.

## Validation

The complete `make verify` release gate passes. Formatting, ESLint, ShellCheck,
Clippy with warnings denied, unused-code checks,
and the production frontend build pass. JavaScript checks pass 115 unit/script
tests and 152 component tests. All 110 application tests pass, including device
sync, import recovery, backup compatibility, query ordering, and cancellation.
The CLI, core migrations/search/export/bounds suites, relay unit tests, and seven
crypto/sync tests also pass. Release metadata is consistent at version 0.5.2.

An initial workspace run encountered an environment blocker in the packaged relay
CLI integration test: OpenSnitch explicitly denied localhost connections for
`target/debug/deps/cli-76c7aa3b9905e728`. Socket traces show a bound listener and
a client connection that times out before HTTP. The standalone relay test
variant passed all three cases; OpenSnitch's log records an allow rule for that
variant and a deny rule for the workspace variant. After the user allowed
localhost for the exact workspace executable, all three relay CLI tests passed
without code or timeout changes. The subsequent full `make verify` run passed.

## Patch release justification

The release can cite measured improvements to large-library browsing, filter
loading, tool-heavy history appends, rejected search pagination, and repeated
large tool-card rendering. It also fixes visible-search failures caused by
hidden helpers, recovers corrupt indexing summaries, reports storage failures
accurately, and enforces the live-window byte limit exactly. Keep the workload
sizes and measurement conditions alongside any performance claims.

The accompanying library fix hides background histories by default, provides
**Show subagents**, and preserves a Codex fork's owning session metadata instead
of replacing it with an inherited parent header.

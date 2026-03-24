# Buffer + Promote Architecture for SearchDB

**Date:** 2026-03-24
**Status:** Draft

## Problem

SearchDB is a serverless, single-binary search tool backed by Delta Lake. The current implementation holds a tantivy `IndexWriter` lock during sync operations, blocking concurrent queries. In event-driven, high-throughput patterns — many small Delta versions with 10-50 rows each, many concurrent queries triggering auto-sync — this lock becomes the bottleneck.

The fundamental tension: documents are keyed by `_id`, but the inverted index is keyed by terms. Updating a document requires `delete_term` + `add_document`, which holds an exclusive writer lock on the entire index.

## Design Principles

1. **Serverless.** No daemon, no background process. Every operation is a CLI invocation.
2. **Index-on-the-fly.** Users don't think about indexing. Queries trigger sync automatically.
3. **Write-only, compact later.** Borrowed from the lakehouse movement — instead of coordinating concurrent writers, make writes append-only and defer compaction.
4. **Dumb, reliable architecture.** Prefer duplicate work over coordination. Dedup at read time rather than synchronize at write time.

## Architecture: Two-Tier (Buffer + Index)

### On-Disk Layout

```
{index_name}/
├── searchdb.json          ← schema + delta source
├── manifest.json          ← {"index_version": 103}
├── index/                 ← compacted tantivy index (the indexed tier)
├── buffer/                ← append-only NDJSON files (the unindexed tier)
│   ├── v103-v105-a1b2c3d4.ndjson
│   ├── v103-v105-e5f6g7h8.ndjson   ← duplicate range from concurrent sync (OK)
│   ├── v105-v108-i9j0k1l2.ndjson
│   └── ...
└── buffer/.promoting      ← lockfile, exists only during promotion
```

### The Two Tiers

**Index tier:** A compacted tantivy index. Searched via the inverted index — fast, selective. Contains all data up through the Delta version recorded in `manifest.json` as `index_version`.

**Buffer tier:** A directory of NDJSON files. Each file contains rows from a Delta version range. Scanned linearly at query time — brute force, but fast for small buffers. No inverted index, no write lock, no coordination.

The inverted index only earns its keep at ~10K+ documents. Below that, a linear scan of raw JSON is faster than opening a tantivy index. The buffer tier exploits this: tiny incremental syncs stay as flat files until there's enough data to justify building an index.

### Cost Model

| Operation | Index tier | Buffer tier |
|-----------|-----------|-------------|
| Write | IndexWriter lock, build segments, commit | Append NDJSON file (no lock) |
| Read | Term dictionary lookup → posting list → score | Linear scan + JSON parse |
| Per-file open cost | mmap + FST load (~ms) | open() + read() (~μs) |
| Useful when | >10K docs (high selectivity) | <10K docs (scan is fast enough) |

## Write Path: Sync to Buffer

When a query triggers auto-sync:

1. Read `manifest.json` → `index_version` (e.g., 103)
2. Check Delta table → current version (e.g., 108)
3. If current > index_version:
   a. Diff Delta file URIs: `v108.file_uris() - v103.file_uris()`
   b. Read new Parquet files → rows as JSON
   c. Write rows to `buffer/v103-v108-{uuid}.ndjson`

That's it. No tantivy writer, no lock, no segment creation.

### Concurrent Write Behavior

Multiple processes may sync the same Delta version range simultaneously. Each writes its own buffer file (UUID suffix ensures unique filenames). This produces duplicate rows.

**Duplicates are harmless.** Dedup by `_id` at read time ensures correctness. The cost is extra scan bytes — under realistic concurrency (10 processes syncing 50 rows each), that's ~500 duplicate rows, maybe 100KB. Negligible.

**No check-before-write optimization.** We considered checking if a buffer file for the target version range already exists. This introduces partial-coverage detection logic and a crash-safety hole (half-written file tricks other processes into skipping their sync). The "always write, always dedup" approach is simpler, more reliable, and the I/O cost is noise.

### File Naming Convention

```
buffer/v{from}-v{to}-{uuid}.ndjson
```

- `from`: Delta version the sync started from (inclusive, matches `index_version`)
- `to`: Delta version the sync ended at (inclusive)
- `uuid`: Random suffix for uniqueness under concurrent writes
- Format: One JSON object per line (NDJSON)

## Read Path: Search Index + Scan Buffer

When a query executes:

1. Search the tantivy index (covers data up to `index_version`)
2. Scan all `buffer/*.ndjson` files (covers data since `index_version`)
3. Merge results, dedup by `_id` — buffer wins (newer data)
4. Apply field projection, scoring, limit/offset
5. Return results

### Dedup Strategy

When the same `_id` appears in both the index and the buffer, the buffer version wins. When the same `_id` appears in multiple buffer files, any copy is fine — they contain the same data (from the same Delta source).

Implementation: collect results from the index, then scan buffer rows. Build a `HashSet<String>` of `_id`s seen in the buffer. When merging index results, skip any `_id` already in the set.

### Performance Characteristics

- **Index search:** O(query selectivity) — unchanged from current implementation
- **Buffer scan:** O(total buffer bytes) — linear but fast for small buffers
- **Dedup:** O(buffer rows) — single-pass HashSet construction
- **Threshold for concern:** When buffer exceeds ~5,000 rows or ~5MB, scan cost becomes noticeable. This is the promotion trigger.

## Promotion: Buffer → Index

Promotion moves buffered rows into the tantivy index and clears the buffer. This is the only operation that acquires the tantivy `IndexWriter` lock.

### Trigger Conditions

Promote when either:
- Total buffer rows > 5,000
- Buffer file count > 50

Checked before each query, after auto-sync.

### Promotion Flow

```
1. Check trigger: buffer exceeds threshold?
   → No: skip promotion, proceed to query
   → Yes: attempt promotion

2. Create lockfile: buffer/.promoting (O_CREAT | O_EXCL)
   → Exists: another process is promoting → skip, proceed to query
   → Created: we are the promoter

3. Snapshot buffer file list (ls buffer/*.ndjson)

4. Acquire tantivy IndexWriter on index/
   → Fails (locked): remove .promoting, skip promotion, proceed to query
   → Success: continue

5. Read all snapshot files, upsert rows into tantivy index
   (delete_term by _id + add_document — handles dedup against existing index data)

6. Commit IndexWriter

7. Update manifest.json: set index_version to max Delta version from buffer files

8. Delete snapshot files

9. Remove buffer/.promoting lockfile

10. Proceed to query (search index + any buffer files written during promotion)
```

### Concurrency During Promotion

**New syncs during promotion:** Safe. New buffer files are written after the snapshot (step 3), so they aren't deleted (step 8). They'll be scanned by the current query and promoted next time.

**Concurrent readers during promotion:** Safe. POSIX unlink semantics — deleted files remain readable until all open handles close. A query scanning a buffer file that gets deleted in step 8 continues reading without error.

**Concurrent promotion attempts:** Safe. The `.promoting` lockfile (step 2) ensures at most one promoter. Other processes skip promotion and proceed to query with the buffer.

**Crash during promotion:**
- Before commit (step 6): Index unchanged, buffer files intact, `.promoting` is stale.
- After commit, before cleanup (steps 7-8): Index has the data AND buffer files still exist. Duplicates are handled by dedup. Next promoter cleans up.
- Stale `.promoting` lockfile: If older than 60 seconds, assume crashed promoter. Remove and take over.

### Why the IndexWriter Lock Is Acceptable Here

The lock is only held during promotion, not during normal sync. Promotion of 5,000 rows takes ~100-200ms. Under realistic concurrency, the collision window is small. When a process can't acquire the lock, it simply skips promotion — the buffer continues working. The system degrades gracefully: more concurrent load means the buffer stays a little longer, not that queries fail.

## Manifest

The manifest is minimal — a single field:

```json
{
  "index_version": 103
}
```

`index_version` means: the tantivy index contains all Delta data up through this version.

Buffer files on disk are the source of truth for what's buffered. The version range is encoded in the filename. No need to duplicate this in the manifest — one source of truth per fact.

### Version Watermark Logic

On auto-sync:
- Current Delta version = 108
- `index_version` = 103
- Buffer files exist covering some subset of v103→v108
- Sync any uncovered gap between `index_version` and Delta HEAD
- In the simple "always write" model: just sync the full range v103→v108 and let dedup handle overlaps with existing buffer files

After promotion:
- `index_version` updated to the max Delta version covered by promoted buffer files

## Comparison to Current Architecture

| Aspect | Current | Buffer + Promote |
|--------|---------|-----------------|
| Write path | Acquire IndexWriter, upsert, commit | Append NDJSON file |
| Write contention | IndexWriter lock blocks concurrent syncs | None — each process writes own file |
| Concurrent queries during sync | Error (lock held) | Both proceed (buffer scan) |
| Read cost | Single tantivy search | Tantivy search + buffer scan |
| Data freshness | Immediate (after lock acquired) | Immediate (buffer scanned on read) |
| Crash safety | Tantivy commit is atomic | Append is atomic; promotion crash leaves duplicates |

## Lakehouse Analogy

| Concept | Delta Lake | LSM-tree | SearchDB |
|---------|-----------|----------|----------|
| Hot/write tier | Uncommitted writes | Memtable | Buffer (NDJSON) |
| Cold/read tier | Parquet files | SSTables | Tantivy index (inverted, immutable segments) |
| Compaction | OPTIMIZE | Level compaction | Promotion (buffer → index) |
| Write contention | Transaction log (optimistic) | WAL (sequential) | File-per-sync (parallel, dedup at read) |
| Coordination | Conflict detection on commit | Single writer | None (dedup handles races) |

This is the lakehouse pattern — write-only, compact later — applied to inverted indices. Writes never touch the index. The index is built lazily from accumulated buffer data when the buffer grows large enough to justify it.

## Open Questions

1. **Buffer row counting.** To check the promotion trigger, we need to know total buffer rows. Options: count lines across all files (I/O cost), track count in manifest (coordination), or use file count as a proxy (simple, good enough).

2. **Buffer cleanup on drop/reindex.** When `searchdb drop` or `searchdb reindex` runs, clear the buffer directory along with the index.

3. **Integration with existing commands.** The `searchdb index` (NDJSON bulk import) command could write to the buffer instead of directly to the IndexWriter, unifying the write path. Or it could bypass the buffer and write directly for large bulk loads where the IndexWriter lock is acceptable.

4. **Serve command (future).** The HTTP server would be a long-lived process that could hold the IndexWriter and promote continuously. The buffer architecture doesn't preclude this — the serve command could simply promote on a timer instead of waiting for a query to trigger it.

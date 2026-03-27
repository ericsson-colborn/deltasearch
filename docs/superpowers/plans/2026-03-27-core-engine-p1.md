# Core Engine P1: Parallel Pipeline + Legacy Cleanup

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace full-materialization indexing with a bounded parallel pipeline (#22, subsumes #21), then remove the deprecated `sync` command and fire-and-forget sync hack (#17 partial). The two-tier search (gap reading) is kept — it's a core product feature that ensures always-correct results.

**Architecture:** Two independent tracks. Track A introduces a 3-stage parallel pipeline (row producer → document builders → index writer) with bounded crossbeam channels, replacing `read_parquet_files_to_json()` materialization. Track B removes only the legacy sync artifacts (`sync` command, `maybe_spawn_sync`) — the compact worker properly replaces these. Gap reading, two-tier search, and `gap_info` in stats are preserved.

**Tech Stack:** crossbeam-channel (bounded MPMC), tantivy IndexWriter, Arrow RecordBatch, existing writer.rs functions.

**Issues:** #22 (parallel pipeline), #21 (streaming — superseded by #22), #17 (partial — remove sync command and maybe_spawn_sync only)

---

## Track A: Parallel Indexing Pipeline (#22, subsumes #21)

### Task A1: Add crossbeam-channel dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add crossbeam-channel to Cargo.toml**

Add under `[dependencies]`, gated behind the `delta` feature:

```toml
# Bounded channels for parallel indexing pipeline
crossbeam-channel = { version = "0.5", optional = true }
```

Update the `delta` feature to include it:

```toml
delta = ["dep:deltalake", "dep:arrow", "dep:parquet", "dep:tokio", "dep:glob", "dep:arrow-csv", "dep:arrow-json", "dep:crossbeam-channel"]
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: clean build

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add crossbeam-channel for parallel indexing pipeline (#22)"
```

---

### Task A2: Create pipeline module with channel types and tests

**Files:**
- Create: `src/pipeline.rs`
- Modify: `src/main.rs` (add `mod pipeline`)

This task defines the data types, the pipeline function signature, and the tests. The function body is a stub — implementation comes in Task A3.

- [ ] **Step 1: Create pipeline module with tests-first**

Create `src/pipeline.rs`:

```rust
//! Parallel indexing pipeline: bounded channels between stages.
//!
//! Architecture (adapted from tantivy-cli):
//! ```text
//! Stage 1: Row producer (caller's iterator)
//!   → bounded channel (capacity: CHANNEL_CAP)
//! Stage 2: Doc builders (N threads)
//!   → bounded channel (capacity: CHANNEL_CAP)
//! Stage 3: Writer (this thread, owns IndexWriter reference)
//! ```
//!
//! Back-pressure is automatic — when the writer can't keep up,
//! channels fill, producers block. Memory stays bounded.

use crossbeam_channel::{bounded, Receiver, Sender};
use std::thread;
use tantivy::IndexWriter;

use crate::error::{Result, SearchDbError};
use crate::schema::Schema;
use crate::writer;

const CHANNEL_CAP: usize = 100;

/// Configuration for the pipeline.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Number of document-builder threads. Default: available_parallelism / 4, min 1.
    pub num_builders: usize,
    /// Channel capacity between stages.
    pub channel_cap: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        let cpus = thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);
        Self {
            num_builders: (cpus / 4).max(1),
            channel_cap: CHANNEL_CAP,
        }
    }
}

/// Stats returned after pipeline completes.
#[derive(Debug, Default)]
pub struct PipelineStats {
    pub docs_indexed: u64,
    pub errors: u64,
}

/// A built document ready for indexing.
struct BuiltDoc {
    doc: tantivy::TantivyDocument,
    doc_id: String,
}

/// Run the parallel indexing pipeline over an iterator of JSON rows.
///
/// The caller retains ownership of the IndexWriter — the pipeline does NOT commit.
/// This allows the caller to control commit boundaries (e.g., per segment_size batch).
pub fn run_pipeline(
    rows: impl Iterator<Item = serde_json::Value> + Send + 'static,
    tv_schema: &tantivy::schema::Schema,
    app_schema: &Schema,
    index_writer: &mut IndexWriter,
    config: PipelineConfig,
) -> Result<PipelineStats> {
    // Stub — implemented in Task A3
    let _ = (rows, tv_schema, app_schema, index_writer, config);
    Ok(PipelineStats::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{FieldType, Schema};
    use std::collections::BTreeMap;

    fn test_schema() -> Schema {
        Schema {
            fields: BTreeMap::from([
                ("name".into(), FieldType::Keyword),
                ("score".into(), FieldType::Numeric),
            ]),
        }
    }

    #[test]
    fn test_pipeline_indexes_rows() {
        let schema = test_schema();
        let tv_schema = schema.build_tantivy_schema();
        let dir = tantivy::directory::RamDirectory::create();
        let index = tantivy::Index::create(
            dir,
            tv_schema.clone(),
            tantivy::IndexSettings::default(),
        )
        .unwrap();
        let mut writer = index.writer(15_000_000).unwrap();

        let rows: Vec<serde_json::Value> = (0..100)
            .map(|i| {
                serde_json::json!({
                    "_id": format!("doc-{i}"),
                    "name": format!("name-{i}"),
                    "score": i
                })
            })
            .collect();

        let stats = run_pipeline(
            rows.into_iter(),
            &tv_schema,
            &schema,
            &mut writer,
            PipelineConfig::default(),
        )
        .unwrap();

        writer.commit().unwrap();

        assert_eq!(stats.docs_indexed, 100);
        assert_eq!(stats.errors, 0);

        let reader = index.reader().unwrap();
        assert_eq!(reader.searcher().num_docs(), 100);
    }

    #[test]
    fn test_pipeline_handles_empty_input() {
        let schema = test_schema();
        let tv_schema = schema.build_tantivy_schema();
        let dir = tantivy::directory::RamDirectory::create();
        let index = tantivy::Index::create(
            dir,
            tv_schema.clone(),
            tantivy::IndexSettings::default(),
        )
        .unwrap();
        let mut writer = index.writer(15_000_000).unwrap();

        let rows: Vec<serde_json::Value> = vec![];

        let stats = run_pipeline(
            rows.into_iter(),
            &tv_schema,
            &schema,
            &mut writer,
            PipelineConfig::default(),
        )
        .unwrap();

        assert_eq!(stats.docs_indexed, 0);
    }

    #[test]
    fn test_pipeline_upserts_duplicate_ids() {
        let schema = test_schema();
        let tv_schema = schema.build_tantivy_schema();
        let dir = tantivy::directory::RamDirectory::create();
        let index = tantivy::Index::create(
            dir,
            tv_schema.clone(),
            tantivy::IndexSettings::default(),
        )
        .unwrap();
        let mut writer = index.writer(15_000_000).unwrap();

        let rows = vec![
            serde_json::json!({"_id": "dup", "name": "first", "score": 1}),
            serde_json::json!({"_id": "dup", "name": "second", "score": 2}),
            serde_json::json!({"_id": "unique", "name": "third", "score": 3}),
        ];

        let stats = run_pipeline(
            rows.into_iter(),
            &tv_schema,
            &schema,
            &mut writer,
            PipelineConfig::default(),
        )
        .unwrap();

        writer.commit().unwrap();

        assert_eq!(stats.docs_indexed, 3);
        let reader = index.reader().unwrap();
        assert_eq!(reader.searcher().num_docs(), 2);
    }

    #[test]
    fn test_pipeline_with_segment_commits() {
        let schema = test_schema();
        let tv_schema = schema.build_tantivy_schema();
        let dir = tantivy::directory::RamDirectory::create();
        let index = tantivy::Index::create(
            dir,
            tv_schema.clone(),
            tantivy::IndexSettings::default(),
        )
        .unwrap();
        let mut writer = index.writer(15_000_000).unwrap();

        // Simulate segment_size=50: index 120 rows in 3 batches (50, 50, 20)
        let segment_size = 50;
        let all_rows: Vec<serde_json::Value> = (0..120)
            .map(|i| {
                serde_json::json!({
                    "_id": format!("doc-{i}"),
                    "name": format!("n{i}"),
                    "score": i
                })
            })
            .collect();

        let mut total_indexed = 0u64;
        for chunk in all_rows.chunks(segment_size) {
            let stats = run_pipeline(
                chunk.to_vec().into_iter(),
                &tv_schema,
                &schema,
                &mut writer,
                PipelineConfig {
                    num_builders: 2,
                    channel_cap: 10,
                },
            )
            .unwrap();
            writer.commit().unwrap();
            total_indexed += stats.docs_indexed;
        }

        assert_eq!(total_indexed, 120);
        let reader = index.reader().unwrap();
        assert_eq!(reader.searcher().num_docs(), 120);
    }
}
```

- [ ] **Step 2: Register pipeline module in main.rs**

Add after the existing `#[cfg(feature = "delta")] mod ingest;` line in `src/main.rs`:

```rust
#[cfg(feature = "delta")]
mod pipeline;
```

- [ ] **Step 3: Run tests to verify they fail (stub returns empty stats)**

Run: `cargo test pipeline`
Expected: `test_pipeline_indexes_rows` fails (docs_indexed == 0, not 100)

- [ ] **Step 4: Commit**

```bash
git add src/pipeline.rs src/main.rs
git commit -m "feat: pipeline module skeleton with tests (#22)"
```

---

### Task A3: Implement the 3-stage parallel pipeline

**Files:**
- Modify: `src/pipeline.rs` (replace `run_pipeline` body)

- [ ] **Step 1: Implement run_pipeline**

Replace the `run_pipeline` function body in `src/pipeline.rs`:

```rust
pub fn run_pipeline(
    rows: impl Iterator<Item = serde_json::Value> + Send + 'static,
    tv_schema: &tantivy::schema::Schema,
    app_schema: &Schema,
    index_writer: &mut IndexWriter,
    config: PipelineConfig,
) -> Result<PipelineStats> {
    let tv_schema_clone = tv_schema.clone();
    let app_schema_clone = app_schema.clone();

    let id_field = tv_schema
        .get_field("_id")
        .map_err(|_| SearchDbError::Schema("missing _id field".into()))?;

    // Stage 1 → Stage 2 channel: raw JSON rows
    let (row_tx, row_rx): (Sender<serde_json::Value>, Receiver<serde_json::Value>) =
        bounded(config.channel_cap);

    // Stage 2 → Stage 3 channel: built tantivy documents
    let (doc_tx, doc_rx): (Sender<BuiltDoc>, Receiver<BuiltDoc>) = bounded(config.channel_cap);

    // Stage 1: Producer — feeds rows into the channel on a dedicated thread.
    let producer = thread::spawn(move || {
        for row in rows {
            if row_tx.send(row).is_err() {
                break; // Receivers dropped
            }
        }
        // row_tx dropped here → channel closes → builders see Disconnected
    });

    // Stage 2: Document builders — N threads pull from row_rx, build docs, send to doc_tx.
    let mut builders = Vec::with_capacity(config.num_builders);
    for _ in 0..config.num_builders {
        let rx = row_rx.clone();
        let tx = doc_tx.clone();
        let tv = tv_schema_clone.clone();
        let app = app_schema_clone.clone();

        builders.push(thread::spawn(move || {
            let mut errs = 0u64;
            for row in rx {
                let doc_id = writer::make_doc_id(&row);
                match writer::build_document(&tv, &app, &row, &doc_id) {
                    Ok(doc) => {
                        if tx.send(BuiltDoc { doc, doc_id }).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        errs += 1;
                        eprintln!("[dsrch] pipeline: doc build error: {e}");
                    }
                }
            }
            errs
        }));
    }
    // Drop our copies so channels close when all builders finish.
    drop(row_rx);
    drop(doc_tx);

    // Stage 3: Writer — this thread. Receives built docs and upserts them.
    let mut stats = PipelineStats::default();
    for built_doc in doc_rx {
        writer::upsert_document(index_writer, id_field, built_doc.doc, &built_doc.doc_id);
        stats.docs_indexed += 1;
    }

    // Join all threads and collect error counts
    producer
        .join()
        .map_err(|_| SearchDbError::Delta("pipeline producer thread panicked".into()))?;

    for handle in builders {
        let errs = handle
            .join()
            .map_err(|_| SearchDbError::Delta("pipeline builder thread panicked".into()))?;
        stats.errors += errs;
    }

    Ok(stats)
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test pipeline`
Expected: all 4 tests pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: clean

- [ ] **Step 4: Commit**

```bash
git add src/pipeline.rs
git commit -m "feat: implement 3-stage parallel indexing pipeline (#22)"
```

---

### Task A4: Wire pipeline into CompactWorker

**Files:**
- Modify: `src/compact.rs`

Replace the single-threaded for-loop in `poll_and_segment` with the pipeline. Keep the commit-per-segment_size behavior by chunking rows and running the pipeline per chunk.

- [ ] **Step 1: Add pipeline import to compact.rs**

Add at the top of `src/compact.rs`:

```rust
use crate::pipeline::{self, PipelineConfig};
```

- [ ] **Step 2: Replace the indexing loop in poll_and_segment**

In `src/compact.rs`, replace lines 185-219 (the `let rows = &changes.added_rows;` block through the end of the batching loop) with:

```rust
        // Process added rows via parallel pipeline
        let rows = changes.added_rows;
        if !rows.is_empty() {
            eprintln!(
                "[dsrch] compact: read {} rows from Delta v{}..v{}",
                rows.len(),
                index_version,
                current_version
            );

            let pipeline_config = PipelineConfig::default();

            // Split into batches of segment_size, commit after each
            let batches: Vec<Vec<serde_json::Value>> = rows
                .chunks(self.opts.segment_size)
                .map(|c| c.to_vec())
                .collect();
            let num_batches = batches.len();

            for (i, batch) in batches.into_iter().enumerate() {
                let batch_len = batch.len();
                let stats = pipeline::run_pipeline(
                    batch.into_iter(),
                    tantivy_schema,
                    &initial_config.schema,
                    index_writer,
                    pipeline_config.clone(),
                )?;

                index_writer.commit()?;

                if stats.errors > 0 {
                    eprintln!(
                        "[dsrch] compact: segment {}/{} — {} docs, {} errors",
                        i + 1, num_batches, batch_len, stats.errors
                    );
                } else {
                    eprintln!(
                        "[dsrch] compact: committed segment {}/{} ({} docs)",
                        i + 1, num_batches, batch_len
                    );
                }
            }
        }
```

Note: Change `let rows = &changes.added_rows;` (reference) to `let rows = changes.added_rows;` (move) so we can chunk into owned Vecs.

- [ ] **Step 3: Run all tests**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: clean

- [ ] **Step 5: Commit**

```bash
git add src/compact.rs
git commit -m "feat: wire parallel pipeline into CompactWorker (#22)"
```

---

### Task A5: Add streaming Parquet reader to delta.rs

**Files:**
- Modify: `src/delta.rs`

Add a channel-based variant of `changes_since` that streams rows through a crossbeam Sender instead of materializing them into a Vec. This solves #21 (bounded memory for large backfills).

- [ ] **Step 1: Add streaming method to DeltaSync**

Add a new method to the `impl DeltaSync` block in `src/delta.rs`:

```rust
    /// Like `changes_since`, but streams added rows through a channel instead of
    /// materializing them all in memory. Returns removed_ids immediately.
    pub async fn changes_since_streaming(
        &self,
        last_version: i64,
        row_tx: crossbeam_channel::Sender<serde_json::Value>,
    ) -> Result<Vec<String>> {
        if last_version < 0 {
            let table = self.open(None).await?;
            let file_uris: Vec<String> = table
                .get_file_uris()
                .map_err(|e| SearchDbError::Delta(format!("failed to get file URIs: {e}")))?
                .collect();
            stream_parquet_files_to_channel(&file_uris, row_tx)?;
            return Ok(vec![]);
        }

        let current = self.open(None).await?;
        let current_files: std::collections::HashSet<String> = current
            .get_file_uris()
            .map_err(|e| SearchDbError::Delta(format!("failed to get file URIs: {e}")))?
            .collect();

        let prev = match self.open(Some(last_version)).await {
            Ok(t) => t,
            Err(_) => {
                log::warn!(
                    "Cannot open Delta v{last_version} (vacuumed?), falling back to full reload"
                );
                let file_uris: Vec<String> = current_files.into_iter().collect();
                stream_parquet_files_to_channel(&file_uris, row_tx)?;
                return Ok(vec![]);
            }
        };
        let prev_files: std::collections::HashSet<String> = prev
            .get_file_uris()
            .map_err(|e| SearchDbError::Delta(format!("failed to get file URIs: {e}")))?
            .collect();

        let added: Vec<String> = current_files.difference(&prev_files).cloned().collect();
        let removed: Vec<&String> = prev_files.difference(&current_files).collect();

        // Extract IDs from removed files (no ? — returns Vec<String> directly)
        let removed_ids = if removed.is_empty() {
            vec![]
        } else {
            extract_ids_from_parquet_files(&removed)
        };

        // Stream added rows through channel
        if !added.is_empty() {
            stream_parquet_files_to_channel(&added, row_tx)?;
        }
        // row_tx dropped here, closing the channel

        Ok(removed_ids)
    }
```

- [ ] **Step 2: Add stream_parquet_files_to_channel function**

Add as a free function near `read_parquet_files_to_json` in `src/delta.rs`:

```rust
/// Read Parquet files and send each row as a JSON Value through the channel.
/// Uses `arrow::json::ArrayWriter` (same as `read_parquet_files_to_json`).
fn stream_parquet_files_to_channel(
    uris: &[impl AsRef<str>],
    tx: crossbeam_channel::Sender<serde_json::Value>,
) -> Result<()> {
    for uri in uris {
        let path = strip_file_uri(uri.as_ref());
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("[dsrch] warning: cannot open {path}: {e}");
                continue;
            }
        };

        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|e| SearchDbError::Delta(format!("invalid Parquet file '{path}': {e}")))?
            .build()
            .map_err(|e| {
                SearchDbError::Delta(format!("failed to build Parquet reader for '{path}': {e}"))
            })?;

        for batch_result in reader {
            let batch = batch_result.map_err(|e| {
                SearchDbError::Delta(format!("error reading batch from '{path}': {e}"))
            })?;

            let buf = Vec::new();
            let mut writer = ArrayWriter::new(buf);
            writer.write_batches(&[&batch]).map_err(|e| {
                SearchDbError::Delta(format!("error converting batch to JSON: {e}"))
            })?;
            writer
                .finish()
                .map_err(|e| SearchDbError::Delta(format!("error finishing JSON writer: {e}")))?;

            let rows: Vec<serde_json::Value> = serde_json::from_slice(&writer.into_inner())?;
            for row in rows {
                if tx.send(row).is_err() {
                    return Ok(()); // Receiver dropped, stop early
                }
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Run all tests**

Run: `cargo test`
Expected: all tests pass (new code isn't called yet from existing paths)

- [ ] **Step 4: Commit**

```bash
git add src/delta.rs
git commit -m "feat: streaming Parquet reader with channel output (#22, #21)"
```

---

### Task A6: Wire streaming delta into CompactWorker

**Files:**
- Modify: `src/compact.rs`

Replace the two-step "materialize all rows, then chunk" with "stream from Delta directly into pipeline via channel." This eliminates the Vec materialization entirely.

- [ ] **Step 1: Rewrite poll_and_segment to use streaming**

Replace the full `poll_and_segment` method body:

```rust
    async fn poll_and_segment(
        &self,
        delta: &DeltaSync,
        tantivy_schema: &tantivy::schema::Schema,
        initial_config: &IndexConfig,
        index_writer: &mut tantivy::IndexWriter,
        _id_field: tantivy::schema::Field,
    ) -> Result<bool> {
        let mut current_config = self.storage.load_config(&self.name)?;
        let index_version = current_config.index_version.unwrap_or(-1);

        let current_version = delta.current_version().await?;

        if current_version <= index_version {
            return Ok(false);
        }

        let gap = current_version - index_version;
        eprintln!(
            "[dsrch] compact: polling Delta... HEAD={current_version}, \
             index={index_version}, gap={gap} versions"
        );

        // Create channel for streaming rows from Delta
        let (row_tx, row_rx) = crossbeam_channel::bounded(100);

        // Get removed IDs and stream added rows into channel
        let removed_ids = delta
            .changes_since_streaming(index_version, row_tx)
            .await?;

        // Process deletions first (before upserts, so re-added rows win)
        if !removed_ids.is_empty() {
            let del_count =
                writer::delete_documents(index_writer, _id_field, &removed_ids);
            index_writer.commit()?;
            eprintln!("[dsrch] compact: deleted {del_count} document(s) from removed Delta files");
        }

        // Stream rows through pipeline — commit per segment_size batch
        let pipeline_config = PipelineConfig::default();
        let segment_size = self.opts.segment_size;
        let mut total_docs = 0u64;
        let mut batch_num = 0usize;

        let mut batch = Vec::with_capacity(segment_size);
        for row in row_rx {
            batch.push(row);
            if batch.len() >= segment_size {
                batch_num += 1;
                let batch_len = batch.len();
                let stats = pipeline::run_pipeline(
                    std::mem::take(&mut batch).into_iter(),
                    tantivy_schema,
                    &initial_config.schema,
                    index_writer,
                    pipeline_config.clone(),
                )?;
                index_writer.commit()?;
                total_docs += stats.docs_indexed;
                eprintln!(
                    "[dsrch] compact: committed segment {batch_num} ({batch_len} docs)"
                );
                batch = Vec::with_capacity(segment_size);
            }
        }

        // Final partial batch
        if !batch.is_empty() {
            batch_num += 1;
            let batch_len = batch.len();
            let stats = pipeline::run_pipeline(
                batch.into_iter(),
                tantivy_schema,
                &initial_config.schema,
                index_writer,
                pipeline_config.clone(),
            )?;
            index_writer.commit()?;
            total_docs += stats.docs_indexed;
            eprintln!(
                "[dsrch] compact: committed segment {batch_num} ({batch_len} docs)"
            );
        }

        if total_docs == 0 && removed_ids.is_empty() {
            eprintln!("[dsrch] compact: no changes to process");
            current_config.index_version = Some(current_version);
            self.save_config_with_compact(&current_config)?;
            return Ok(false);
        }

        // Update watermark AFTER all segments committed (crash safety)
        current_config.index_version = Some(current_version);
        self.save_compact_meta(&mut current_config, true, false)?;
        self.storage.save_config(&self.name, &current_config)?;

        eprintln!(
            "[dsrch] compact: indexed {total_docs} docs in {batch_num} segment(s), now at Delta v{current_version}"
        );
        Ok(true)
    }
```

- [ ] **Step 2: Run all tests**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: clean

- [ ] **Step 4: Commit**

```bash
git add src/compact.rs
git commit -m "feat: end-to-end streaming compaction, bounded memory (#22, #21)"
```

---

## Track B: Legacy Cleanup (#17 partial)

Two-tier search and gap reading are **kept** — they're core product features. We only remove:
1. `maybe_spawn_sync` — the fire-and-forget background sync hack (compact worker replaces it)
2. `sync` command — already deprecated, is just `compact --once`

### Task B1: Remove maybe_spawn_sync from main.rs

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Delete maybe_spawn_sync function**

Remove the `maybe_spawn_sync()` function (lines 393-408) from `src/main.rs`.

- [ ] **Step 2: Remove calls to maybe_spawn_sync**

In `run_cli()`, remove the two `maybe_spawn_sync(...)` calls:
- In the Search arm (line 277): delete `maybe_spawn_sync(&cli.data_dir, &name, gap_versions);`
- In the Get arm (line 287): delete `maybe_spawn_sync(&cli.data_dir, &name, gap_versions);`

Also simplify the Search and Get arms — they no longer need `gap_versions` from `read_gap()`, but they still need `gap_rows`. The `read_gap()` function returns `(Vec<Value>, i64)` — we can ignore the second element:

In the Search arm:
```rust
        Commands::Search { name, query, dsl, limit, offset, fields, score } => {
            let (gap_rows, _) = read_gap(&storage, &name).await;
            commands::search::run(
                &storage, &name, query.as_deref(), dsl.as_deref(),
                limit, offset, fields, score, fmt, &gap_rows,
            )
        }
```

In the Get arm:
```rust
        Commands::Get { name, doc_id, fields } => {
            let (gap_rows, _) = read_gap(&storage, &name).await;
            commands::get::run(&storage, &name, &doc_id, fields, fmt, &gap_rows)
        }
```

- [ ] **Step 3: Build and test**

Run: `cargo build && cargo test`
Expected: clean build, all tests pass

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "refactor: remove fire-and-forget background sync (#17)"
```

---

### Task B2: Remove sync command

**Files:**
- Delete: `src/commands/sync.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Delete sync.rs**

```bash
rm src/commands/sync.rs
```

- [ ] **Step 2: Remove sync from commands/mod.rs**

Remove `#[cfg(feature = "delta")] pub mod sync;` from `src/commands/mod.rs`.

- [ ] **Step 3: Remove Sync variant from Commands enum and dispatch**

In `src/main.rs`:
- Delete the `Sync` variant from the `Commands` enum
- Delete the `Commands::Sync { name } => commands::sync::run(&storage, &name).await,` match arm

- [ ] **Step 4: Build and test**

Run: `cargo build && cargo test`
Expected: clean build, all tests pass

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: clean

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: remove deprecated sync command (#17)"
```

---

## Execution Order

Tracks A and B are independent and can execute in parallel (separate worktrees).

**Track A** (parallel pipeline): A1 → A2 → A3 → A4 → A5 → A6
**Track B** (legacy cleanup): B1 → B2

If executing sequentially, do Track A first (bigger value delivery), then Track B.

## Final Verification

After both tracks merge:

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

All 220+ tests should pass. The `sync` command is gone. Fire-and-forget sync is gone. The compact worker streams through bounded channels. Two-tier search still works — clients always see fresh data.

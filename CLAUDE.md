# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What Is This

deltasearch (`dsrch`) is an embedded search engine that combines tantivy (Rust full-text search), Delta Lake (versioned data lake), and a DuckDB-style philosophy (single binary, zero config, no server). The index is incrementally hydrated from Delta Lake — Delta is the source of truth, but the index is a persistent asset that's expensive to rebuild at scale. No cluster, no daemon, no JVM.

## Build & Development Commands

```bash
cargo build                          # Debug build (default features include "delta")
cargo build --release                # Release build (LTO + strip enabled)
cargo test                           # Run all tests (~220 tests)
cargo test <test_name>               # Run a single test by name
cargo test -- --nocapture            # Run tests with stdout visible
cargo clippy -- -D warnings          # Lint (must pass, CI enforced)
cargo fmt                            # Format code
cargo fmt --check                    # Check formatting without modifying
cargo build --no-default-features    # Build without Delta Lake (sync-only binary)
```

CI runs: `cargo fmt --check` → `cargo clippy -- -D warnings` → `cargo build` → `cargo test`

## Architecture

### Feature-Gated Dual Binary

The `delta` feature (enabled by default) controls whether the binary includes Delta Lake integration. This gates the async runtime:

- **With `delta` (default):** `#[tokio::main] async fn main()` — Delta-facing code is async, search/index operations are sync
- **Without `delta`:** `fn main()` — pure sync binary, only local JSON indexing and search

Modules gated behind `#[cfg(feature = "delta")]`: `compact`, `delta`, `ingest`, `pipeline`, and CLI commands `Reindex`, `Compact`, `Ingest`.

### Module Responsibilities

| Module | Role |
|--------|------|
| `main.rs` | CLI definition (clap derive), command dispatch, gap-reading |
| `commands/` | One file per CLI subcommand. Each exports a `run()` function |
| `storage.rs` | On-disk layout management. `Storage` struct handles paths, config persistence (`IndexConfig` ↔ `searchdb.json`) |
| `schema.rs` | `Schema` + `FieldType` enum (keyword/text/numeric/date). Builds tantivy schema with 3 internal fields (`_id`, `_source`, `__present__`) |
| `writer.rs` | Document-to-tantivy conversion. Handles `_id` extraction/generation, typed field mapping, `__present__` token population, upsert (delete-before-add), array field expansion |
| `searcher.rs` | Query execution, result formatting, two-tier search (persistent index + ephemeral RamDirectory gap index for un-synced Delta rows) |
| `compact.rs` | `CompactWorker` — polls Delta for new rows, creates segments (L1), merges segments (L2) via `StableLogMergePolicy`. Graceful shutdown via `tokio::watch` |
| `delta.rs` | `DeltaSync` — reads Delta Lake tables, file-level diffing for incremental sync, handles Delta delete operations |
| `ingest.rs` | Raw file ingestion (NDJSON, JSON, CSV, Parquet) into Delta Lake tables via Arrow |
| `merge_policy.rs` | Vendored from quickwit — `StableLogMergePolicy` for level-based segment merging |
| `es_dsl/` | Elasticsearch query DSL → tantivy query compilation (bool, term, terms, match, match_phrase, range, exists, match_all, match_none) |
| `error.rs` | `SearchDbError` enum (thiserror), `Result<T>` type alias |

### Two-Tier Search (Legacy, Being Replaced)

Search and get commands read un-indexed Delta rows ("the gap") into a `RamDirectory` ephemeral index, merging results with the persistent index. If the gap exceeds 10 versions, a background sync is spawned fire-and-forget. This is being replaced by the compaction worker model.

### On-Disk Layout

```
{data_dir}/           (default: .dsrch/)
├── {index_name}/
│   ├── searchdb.json   ← IndexConfig: schema + delta_source + index_version + compact metadata
│   └── index/          ← Tantivy segment files
```

### Key Data Flow

```
JSON/NDJSON → writer::add_document() → tantivy IndexWriter → on-disk segments
Delta Lake  → delta::DeltaSync       → compact::CompactWorker → tantivy segments
ES DSL JSON → es_dsl::ElasticQueryDsl::compile() → tantivy Query → searcher
```

## Schema System

4 field types: `keyword` (raw tokenizer, exact match), `text` (en_stem tokenizer, full-text), `numeric` (f64), `date` (ISO 8601).

3 internal fields added automatically: `_id` (document identity), `_source` (verbatim JSON), `__present__` (tokens `__all__` + field names for null-tracking).

Schema can be explicit (JSON) or inferred from data. The `inferred` flag in `IndexConfig` controls whether schema evolution is enabled.

## Coding Standards

- `cargo fmt` before every commit
- `cargo clippy -- -D warnings` must pass
- No `unwrap()` in non-test code — use `?` or explicit error handling
- `anyhow::Result` for application errors, `thiserror` for the `SearchDbError` enum
- Prefer `&str` over `String` in function parameters
- Keep functions under 50 lines
- One module per file, one concern per module

## Query Syntax

Tantivy native (Lucene-like): `+field:"value"` (must), `field:term` (should), `-field:"value"` (must not), `field:[100 TO 200]` (range), `+__present__:__all__` (anchor for null-only queries).

ES DSL via `--dsl` flag: supports `bool`, `term`, `terms`, `match`, `match_phrase`, `range`, `exists`, `match_all`, `match_none`.

## Known Tech Debt

- `atty` crate is unmaintained — should be replaced with `std::io::IsTerminal` (#36)
- Config file is named `searchdb.json` (legacy) — kept for backward compatibility
- Two-tier gap search is a temporary bridge being replaced by compaction worker (#17)

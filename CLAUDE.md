# SearchDB — Rust CLI

## What This Is

SearchDB is a single-binary CLI search tool: "ES in your pocket." Users dump data to Delta Lake on blob storage and search it like Elasticsearch — no cluster, no daemon. The index is a disposable cache that rebuilds itself lazily on query.

This is a **Rust rewrite** of a working Python prototype at `../searchdb/`. The Python version has 63 passing tests and validates the architecture. This Rust version is the production target.

## Architecture

Fork of [tantivy-cli](https://github.com/quickwit-oss/tantivy-cli) (1,722 lines) with Delta Lake sync added via the `deltalake` crate (same Rust codebase that powers the Python `deltalake` package).

### Commands

```
searchdb new <name> --schema <json>       # Create index from schema declaration
searchdb index <name> [-f file.ndjson]    # Bulk index NDJSON (stdin or file)
searchdb search <name> -q <query>         # Search (auto-syncs from Delta first)
searchdb serve <name> [--port 3000]       # HTTP search server
searchdb connect-delta <name> --source <uri> --schema <json>  # Attach Delta source + initial load
searchdb sync <name>                      # Manual incremental sync
searchdb reindex <name> [--as-of-version N]  # Full rebuild from Delta
searchdb get <name> <doc_id>              # Single doc by _id
searchdb stats <name>                     # Index stats
searchdb drop <name>                      # Delete index
```

### The 5 Moats

1. **Delta Lake as source of truth** — `deltalake` crate for table access, version tracking, file-level diffing
2. **Lazy incremental sync** — auto-sync before search: diff `file_uris()` between last_version and HEAD, read only new Parquet files
3. **Upsert semantics** — `IndexWriter::delete_term()` + `add_document()` by `_id`. tantivy-cli never exposed delete; we use the library API directly.
4. **Single binary, zero deps** — `cargo build --release` produces one binary. No Python, no pip, no runtime.
5. **Schema-as-declaration** — 4 field types map to tantivy config:
   - `keyword` → text field, `raw` tokenizer, stored
   - `text` → text field, `en_stem` tokenizer, stored
   - `numeric` → f64 field, indexed + fast + stored
   - `date` → date field, indexed + fast + stored

### Internal Fields (not user-visible in schema)

- `_id` — text/raw/stored. Document identity for upsert.
- `_source` — text/raw/stored. Verbatim JSON of original doc for round-trip.
- `__present__` — text/raw/NOT stored. Tokens: `__all__` (every doc) + field names for non-null fields. Enables null/not-null queries.

### On-Disk Layout

```
{data_dir}/
├── {index_name}/
│   ├── searchdb.json    ← Schema + Delta source + version watermark
│   └── index/           ← Tantivy segment files
```

## Development

```bash
cargo build                     # Debug build
cargo build --release           # Release build
cargo test                      # Run all tests
cargo clippy                    # Lint
cargo fmt                       # Format
```

### Key Crates

- `tantivy 0.25` — search engine (IndexWriter, Searcher, Schema, Document)
- `deltalake` — Delta Lake table access (async, needs tokio)
- `clap 4` — CLI argument parsing (derive macros)
- `axum` — HTTP server (replaces tantivy-cli's dead `iron` dep)
- `serde/serde_json` — JSON serialization
- `anyhow/thiserror` — error handling
- `arrow/parquet` — reading Delta's Parquet files

### Async Bridging

delta-rs is async (tokio). tantivy is sync. Use `#[tokio::main]` on the binary entry point and `block_on()` / `.await` in Delta-facing code. Search/index operations are sync.

## Reference Implementation

The Python prototype at `../searchdb/` has:
- `searchdb/database.py` — Main API, Delta integration orchestration
- `searchdb/searcher.py` — Query building, filter→query string translation
- `searchdb/writer.py` — Document construction, upsert (delete+add), background commit
- `searchdb/delta.py` — DeltaSync, file-level diffing, Parquet reading
- `searchdb/schema.py` — Schema definition, field type mapping
- `tests/` — 63 tests covering search, filters, field types, Delta sync, _source round-trip

Port logic from these files, not the Python patterns. Idiomatic Rust, not Python-in-Rust.

## Query Syntax

Tantivy's native query parser (Lucene-like):
- `+field:"value"` — must match (keyword exact)
- `field:term` — should match (text, stemmed)
- `-field:"value"` — must not match
- `field:[100 TO 200]` — range inclusive
- `field:{100 TO *}` — range exclusive lower, open upper
- `+__present__:__all__` — anchor for null-only queries

## Coding Standards

- `cargo fmt` before every commit
- `cargo clippy -- -D warnings` must pass
- `cargo test` must pass
- Use `anyhow::Result` for application errors, `thiserror` for library error types
- Prefer `&str` over `String` in function parameters where possible
- No `unwrap()` in non-test code — use `?` or explicit error handling
- Keep functions under 50 lines
- One module per file, one concern per module

## Current State

Project just initialized. Implementation plan in `docs/plans/`.

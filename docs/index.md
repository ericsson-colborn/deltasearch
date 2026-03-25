---
layout: default
title: Home
---

# deltasearch

**Embedded search for the lakehouse. ES in your pocket.**

deltasearch is a single-binary search engine that combines tantivy (Rust full-text search) with Delta Lake (versioned data lake). Point it at JSON files or blob storage and you're searching in minutes -- no cluster, no daemon, no JVM.

## Getting Started

- [Quickstart](getting-started/quickstart.md) -- 5-minute end-to-end tutorial
- [Concepts](getting-started/concepts.md) -- Delta Lake, tantivy, segments, compaction

## Guides

- [Schema](guides/schema.md) -- Field types, inference, `--infer-from`
- [Searching](guides/searching.md) -- Query syntax and Elasticsearch DSL
- [Compaction](guides/compaction.md) -- L1/L2, configurable parameters, `--force-merge`

## Design & Architecture

- [Rust MVP Plan](plans/2026-03-23-searchdb-rust-mvp.md)
- [Quickwit Components Research](research/2026-03-24-quickwit-components.md)

### Superpowers

Plans:

- [Delta-as-Buffer](superpowers/plans/2026-03-24-delta-as-buffer.md)
- [ES DSL](superpowers/plans/2026-03-24-es-dsl.md)
- [Schema Inference](superpowers/plans/2026-03-24-schema-inference.md)
- [Tiered Compaction](superpowers/plans/2026-03-24-tiered-compaction.md)
- [Ingest](superpowers/plans/2026-03-25-ingest.md)

Specs:

- [Buffer-Promote Architecture Design](superpowers/specs/2026-03-24-buffer-promote-architecture-design.md)
- [ES DSL Design](superpowers/specs/2026-03-24-es-dsl-design.md)
- [Schema Inference Design](superpowers/specs/2026-03-24-schema-inference-design.md)
- [Tiered Compaction Design](superpowers/specs/2026-03-24-tiered-compaction-design.md)
- [Ingest Design](superpowers/specs/2026-03-25-ingest-design.md)

## Links

- [GitHub Repository](https://github.com/ericsson-colborn/searchdb)
- [README](https://github.com/ericsson-colborn/searchdb#readme)

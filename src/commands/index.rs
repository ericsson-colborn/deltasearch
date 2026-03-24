use std::fs::File;
use std::io::{self, BufRead, BufReader};

use tantivy::Index;

use crate::error::{Result, SearchDbError};
use crate::storage::Storage;
use crate::writer;

/// Bulk index NDJSON documents into an existing index.
///
/// Reads from stdin or a file (`-f`). Each line is a JSON object.
/// Documents with `_id` are upserted (delete + add); without `_id`, a UUID is generated.
pub fn run(storage: &Storage, name: &str, file: Option<&str>) -> Result<()> {
    if !storage.exists(name) {
        return Err(SearchDbError::IndexNotFound(name.to_string()));
    }

    let config = storage.load_config(name)?;
    let tantivy_schema = config.schema.build_tantivy_schema();

    let index = Index::open_in_dir(storage.tantivy_dir(name))?;
    let mut index_writer = index.writer(50_000_000)?;

    let id_field = tantivy_schema
        .get_field("_id")
        .map_err(|_| SearchDbError::Schema("missing _id field".into()))?;

    let reader: Box<dyn BufRead> = match file {
        Some(path) => {
            let f = File::open(path)
                .map_err(|e| SearchDbError::Schema(format!("cannot open file '{path}': {e}")))?;
            Box::new(BufReader::new(f))
        }
        None => Box::new(BufReader::new(io::stdin())),
    };

    let mut count = 0u64;
    let mut errors = 0u64;

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let doc_json: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[searchdb] Skipping invalid JSON: {e}");
                errors += 1;
                continue;
            }
        };

        let doc_id = writer::make_doc_id(&doc_json);
        let doc = writer::build_document(&tantivy_schema, &config.schema, &doc_json, &doc_id)?;
        writer::upsert_document(&index_writer, id_field, doc, &doc_id);
        count += 1;
    }

    index_writer.commit()?;

    eprintln!("[searchdb] Indexed {count} document(s) into '{name}'");
    if errors > 0 {
        eprintln!("[searchdb] Skipped {errors} invalid line(s)");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::new_index;
    use std::io::Write;

    fn setup_index(dir: &std::path::Path) -> Storage {
        let storage = Storage::new(dir.to_str().unwrap());
        let schema_json = r#"{"fields":{"name":"keyword","notes":"text","age":"numeric"}}"#;
        new_index::run(&storage, "test", schema_json, false).unwrap();
        storage
    }

    #[test]
    fn test_index_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let storage = setup_index(dir.path());

        // Write NDJSON to a temp file
        let ndjson_path = dir.path().join("data.ndjson");
        let mut f = std::fs::File::create(&ndjson_path).unwrap();
        writeln!(
            f,
            r#"{{"_id":"d1","name":"glucose","notes":"fasting sample","age":45}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"_id":"d2","name":"a1c","notes":"borderline diabetic","age":62}}"#
        )
        .unwrap();

        run(&storage, "test", Some(ndjson_path.to_str().unwrap())).unwrap();

        // Verify doc count via tantivy reader
        let index = tantivy::Index::open_in_dir(storage.tantivy_dir("test")).unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.num_docs(), 2);
    }

    #[test]
    fn test_index_upsert_deduplicates() {
        let dir = tempfile::tempdir().unwrap();
        let storage = setup_index(dir.path());

        // First batch
        let ndjson1 = dir.path().join("batch1.ndjson");
        std::fs::write(&ndjson1, r#"{"_id":"d1","name":"original"}"#).unwrap();
        run(&storage, "test", Some(ndjson1.to_str().unwrap())).unwrap();

        // Second batch — same _id, different value
        let ndjson2 = dir.path().join("batch2.ndjson");
        std::fs::write(&ndjson2, r#"{"_id":"d1","name":"updated"}"#).unwrap();
        run(&storage, "test", Some(ndjson2.to_str().unwrap())).unwrap();

        let index = tantivy::Index::open_in_dir(storage.tantivy_dir("test")).unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.num_docs(), 1);
    }

    #[test]
    fn test_index_skips_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let storage = setup_index(dir.path());

        let ndjson = dir.path().join("mixed.ndjson");
        let mut f = std::fs::File::create(&ndjson).unwrap();
        writeln!(f, r#"{{"_id":"d1","name":"good"}}"#).unwrap();
        writeln!(f, "not valid json").unwrap();
        writeln!(f, r#"{{"_id":"d2","name":"also good"}}"#).unwrap();

        run(&storage, "test", Some(ndjson.to_str().unwrap())).unwrap();

        let index = tantivy::Index::open_in_dir(storage.tantivy_dir("test")).unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.num_docs(), 2);
    }

    #[test]
    fn test_index_nonexistent_index_errors() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_str().unwrap());
        let result = run(&storage, "no_such_index", None);
        assert!(result.is_err());
    }
}

use tantivy::Index;

use crate::error::{Result, SearchDbError};
use crate::storage::Storage;
use crate::OutputFormat;

/// Print index statistics.
pub fn run(storage: &Storage, name: &str, fmt: OutputFormat) -> Result<()> {
    if !storage.exists(name) {
        return Err(SearchDbError::IndexNotFound(name.to_string()));
    }

    let config = storage.load_config(name)?;
    let index = Index::open_in_dir(storage.tantivy_dir(name))?;
    let reader = index
        .reader()
        .map_err(|e| SearchDbError::Schema(format!("failed to open reader: {e}")))?;
    let searcher = reader.searcher();

    let num_docs: u64 = searcher
        .segment_readers()
        .iter()
        .map(|r| r.num_docs() as u64)
        .sum();
    let num_segments = searcher.segment_readers().len();

    match fmt {
        OutputFormat::Json => {
            let stats = serde_json::json!({
                "index": name,
                "num_docs": num_docs,
                "num_segments": num_segments,
                "fields": &config.schema.fields,
                "delta_source": config.delta_source,
                "index_version": config.index_version,
            });
            println!("{}", serde_json::to_string(&stats)?);
        }
        OutputFormat::Text => {
            println!("Index:      {name}");
            println!("Documents:  {num_docs}");
            println!("Segments:   {num_segments}");
            println!("Fields:");
            for (field_name, field_type) in &config.schema.fields {
                println!("  {field_name}: {field_type:?}");
            }
            if let Some(ref src) = config.delta_source {
                println!("Delta:      {src}");
                if let Some(v) = config.index_version {
                    println!("Version:    {v}");
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{index as index_cmd, new_index};
    use std::io::Write;

    #[test]
    fn test_stats_empty_index() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_str().unwrap());
        new_index::run(&storage, "test", r#"{"fields":{"name":"keyword"}}"#, false).unwrap();
        run(&storage, "test", OutputFormat::Json).unwrap();
    }

    #[test]
    fn test_stats_with_docs() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_str().unwrap());
        new_index::run(&storage, "test", r#"{"fields":{"name":"keyword"}}"#, false).unwrap();

        let ndjson = dir.path().join("data.ndjson");
        let mut f = std::fs::File::create(&ndjson).unwrap();
        writeln!(f, r#"{{"_id":"d1","name":"alice"}}"#).unwrap();
        writeln!(f, r#"{{"_id":"d2","name":"bob"}}"#).unwrap();
        index_cmd::run(&storage, "test", Some(ndjson.to_str().unwrap())).unwrap();

        run(&storage, "test", OutputFormat::Json).unwrap();

        let index = Index::open_in_dir(storage.tantivy_dir("test")).unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.num_docs(), 2);
    }

    #[test]
    fn test_stats_nonexistent_index() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_str().unwrap());
        let result = run(&storage, "nope", OutputFormat::Json);
        assert!(result.is_err());
    }
}

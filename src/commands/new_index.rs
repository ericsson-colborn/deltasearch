use std::collections::BTreeMap;

use crate::error::{Result, SearchDbError};
use crate::schema::Schema;
use crate::storage::{IndexConfig, Storage};

/// Create a new empty index, optionally from a schema declaration.
///
/// If `schema_json` is None, creates an empty-schema index with `inferred: true`.
/// Schema will be populated on first `dewey index` via schema evolution.
///
/// If `dry_run` is true, prints the schema as JSON and exits without creating.
pub fn run(
    storage: &Storage,
    name: &str,
    schema_json: Option<&str>,
    overwrite: bool,
    dry_run: bool,
) -> Result<()> {
    let (schema, inferred) = match schema_json {
        Some(json) => (Schema::from_json(json)?, false),
        None => {
            if dry_run {
                return Err(SearchDbError::Schema(
                    "--dry-run requires --schema or --source".into(),
                ));
            }
            (
                Schema {
                    fields: BTreeMap::new(),
                },
                true,
            )
        }
    };

    if dry_run {
        let json = serde_json::to_string(&schema)?;
        println!("{json}");
        return Ok(());
    }

    create_index(storage, name, schema, inferred, overwrite)
}

/// Create the index on disk with the given schema.
fn create_index(
    storage: &Storage,
    name: &str,
    schema: Schema,
    inferred: bool,
    overwrite: bool,
) -> Result<()> {
    if storage.exists(name) {
        if overwrite {
            storage.drop(name)?;
        } else {
            return Err(SearchDbError::IndexExists(name.to_string()));
        }
    }

    let tantivy_schema = schema.build_tantivy_schema();
    storage.create_dirs(name)?;
    let tantivy_dir = storage.tantivy_dir(name);
    let _index = tantivy::Index::create_in_dir(&tantivy_dir, tantivy_schema)?;

    let config = IndexConfig {
        schema,
        inferred,
        delta_source: None,
        index_store: None,
        index_version: None,
        compact: None,
    };
    storage.save_config(name, &config)?;

    eprintln!(
        "[dewey] Created index '{name}' with {} field(s){}",
        config.schema.fields.len(),
        if inferred {
            " (schema will be inferred from data)"
        } else {
            ""
        }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_index_without_schema() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_str().unwrap());

        run(&storage, "test", None, false, false).unwrap();

        let config = storage.load_config("test").unwrap();
        assert!(config.schema.fields.is_empty());
        assert!(config.inferred);
    }

    #[test]
    fn test_new_index_with_schema() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_str().unwrap());

        let schema_json = r#"{"fields":{"name":"keyword"}}"#;
        run(&storage, "test", Some(schema_json), false, false).unwrap();

        let config = storage.load_config("test").unwrap();
        assert_eq!(config.schema.fields.len(), 1);
        assert!(!config.inferred);
    }

    #[test]
    fn test_new_index_dry_run_with_schema() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_str().unwrap());

        let schema_json = r#"{"fields":{"name":"keyword","age":"numeric"}}"#;

        // Dry run should succeed but NOT create the index
        run(&storage, "test", Some(schema_json), false, true).unwrap();

        assert!(!storage.exists("test"));
    }

    #[test]
    fn test_new_index_dry_run_without_schema_errors() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_str().unwrap());

        // Dry run without schema should error
        let result = run(&storage, "test", None, false, true);
        assert!(result.is_err());
    }
}

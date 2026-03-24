use tantivy::Index;

use crate::delta::DeltaSync;
use crate::error::{Result, SearchDbError};
use crate::storage::Storage;
use crate::writer;

/// Manual incremental sync from a Delta Lake source.
///
/// Diffs file URIs between the last indexed version and HEAD,
/// reads only new Parquet files, and upserts their rows.
pub async fn run(storage: &Storage, name: &str) -> Result<()> {
    if !storage.exists(name) {
        return Err(SearchDbError::IndexNotFound(name.to_string()));
    }

    let mut config = storage.load_config(name)?;

    let source = config.delta_source.as_deref().ok_or_else(|| {
        SearchDbError::Delta(format!(
            "index '{name}' has no Delta source — use connect-delta first"
        ))
    })?;

    let delta = DeltaSync::new(source);
    let current_version = delta.current_version().await?;
    let last_version = config.index_version.unwrap_or(-1);

    if current_version == last_version {
        eprintln!("[searchdb] Already up to date at Delta v{current_version}");
        return Ok(());
    }

    eprintln!("[searchdb] Syncing Delta v{last_version} → v{current_version}");

    let rows = delta.rows_added_since(last_version).await?;
    if rows.is_empty() {
        eprintln!("[searchdb] No new rows to index");
        config.index_version = Some(current_version);
        storage.save_config(name, &config)?;
        return Ok(());
    }

    let tantivy_schema = config.schema.build_tantivy_schema();
    let index = Index::open_in_dir(storage.tantivy_dir(name))?;
    let mut index_writer = index.writer(50_000_000)?;

    let id_field = tantivy_schema
        .get_field("_id")
        .map_err(|_| SearchDbError::Schema("missing _id field".into()))?;

    let mut count = 0u64;
    for row in &rows {
        let doc_id = writer::make_doc_id(row);
        let doc = writer::build_document(&tantivy_schema, &config.schema, row, &doc_id)?;
        writer::upsert_document(&index_writer, id_field, doc, &doc_id);
        count += 1;
    }
    index_writer.commit()?;

    config.index_version = Some(current_version);
    storage.save_config(name, &config)?;

    eprintln!("[searchdb] Synced {count} document(s), now at Delta v{current_version}");
    Ok(())
}

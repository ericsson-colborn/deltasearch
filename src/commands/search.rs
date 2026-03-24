use tantivy::Index;

use crate::error::{Result, SearchDbError};
use crate::searcher;
use crate::storage::Storage;
use crate::OutputFormat;

/// Execute a search query against an index and print results.
#[allow(clippy::too_many_arguments)]
pub fn run(
    storage: &Storage,
    name: &str,
    query: &str,
    limit: usize,
    offset: usize,
    fields: Option<Vec<String>>,
    include_score: bool,
    fmt: OutputFormat,
) -> Result<()> {
    if !storage.exists(name) {
        return Err(SearchDbError::IndexNotFound(name.to_string()));
    }

    let config = storage.load_config(name)?;
    let index = Index::open_in_dir(storage.tantivy_dir(name))?;

    let results = searcher::search(
        &index,
        &config.schema,
        query,
        limit,
        offset,
        fields.as_deref(),
        include_score,
    )?;

    match fmt {
        OutputFormat::Json => {
            for hit in &results {
                println!("{}", serde_json::to_string(&hit.doc)?);
            }
        }
        OutputFormat::Text => {
            if results.is_empty() {
                eprintln!("[searchdb] No results");
                return Ok(());
            }
            // Collect field names from first result for column headers
            let first = results[0].doc.as_object().unwrap();
            let cols: Vec<&String> = first.keys().collect();

            // Print header
            let header: Vec<String> = cols.iter().map(|c| c.to_string()).collect();
            println!("{}", header.join("\t"));
            println!(
                "{}",
                header
                    .iter()
                    .map(|h| "-".repeat(h.len().max(8)))
                    .collect::<Vec<_>>()
                    .join("\t")
            );

            // Print rows
            for hit in &results {
                let obj = hit.doc.as_object().unwrap();
                let row: Vec<String> = cols
                    .iter()
                    .map(|c| match obj.get(*c) {
                        Some(serde_json::Value::String(s)) => s.clone(),
                        Some(v) => v.to_string(),
                        None => String::new(),
                    })
                    .collect();
                println!("{}", row.join("\t"));
            }
        }
    }

    eprintln!("[searchdb] {} result(s)", results.len());
    Ok(())
}

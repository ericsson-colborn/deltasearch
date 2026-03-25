use arrow::record_batch::RecordBatch;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};

use crate::error::{Result, SearchDbError};

/// Supported input file formats for ingest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    /// One JSON object per line
    Ndjson,
    /// JSON array of objects
    Json,
    /// CSV with header row
    Csv,
    /// Apache Parquet (columnar)
    Parquet,
}

/// Detect input format from file extension.
///
/// Returns None for unrecognized extensions.
pub fn detect_format(path: &str) -> Option<InputFormat> {
    let lower = path.to_lowercase();
    if lower.ends_with(".ndjson") || lower.ends_with(".jsonl") {
        Some(InputFormat::Ndjson)
    } else if lower.ends_with(".json") {
        Some(InputFormat::Json)
    } else if lower.ends_with(".csv") {
        Some(InputFormat::Csv)
    } else if lower.ends_with(".parquet") {
        Some(InputFormat::Parquet)
    } else {
        None
    }
}

/// Parse a format string from CLI --format flag.
pub fn parse_format(s: &str) -> Result<InputFormat> {
    match s.to_lowercase().as_str() {
        "ndjson" | "jsonl" => Ok(InputFormat::Ndjson),
        "json" => Ok(InputFormat::Json),
        "csv" => Ok(InputFormat::Csv),
        "parquet" => Ok(InputFormat::Parquet),
        other => Err(SearchDbError::Schema(format!(
            "Unknown format '{other}'. Supported: ndjson, json, csv, parquet"
        ))),
    }
}

/// Read a file into Arrow RecordBatches.
///
/// Dispatches to format-specific readers based on `format`.
pub fn read_file(path: &str, format: InputFormat, batch_size: usize) -> Result<Vec<RecordBatch>> {
    match format {
        InputFormat::Ndjson => read_ndjson_file(path, batch_size),
        InputFormat::Json => read_json_file(path, batch_size),
        InputFormat::Csv => read_csv_file(path, batch_size),
        InputFormat::Parquet => read_parquet_file(path),
    }
}

/// Read NDJSON file into RecordBatches using arrow-json.
fn read_ndjson_file(path: &str, batch_size: usize) -> Result<Vec<RecordBatch>> {
    let file = File::open(path).map_err(SearchDbError::Io)?;
    read_ndjson_reader(BufReader::new(file), batch_size)
}

/// Read NDJSON from any BufRead source into RecordBatches.
fn read_ndjson_reader(reader: impl BufRead, batch_size: usize) -> Result<Vec<RecordBatch>> {
    // Collect non-empty lines
    let lines: Vec<String> = reader
        .lines()
        .filter_map(|l| l.ok())
        .filter(|l| !l.trim().is_empty())
        .collect();

    if lines.is_empty() {
        return Err(SearchDbError::Schema(
            "No data rows found in NDJSON input".into(),
        ));
    }

    // Infer schema from the NDJSON lines, then build reader
    let joined = lines.join("\n");
    let mut cursor = std::io::Cursor::new(joined.as_bytes().to_vec());

    let (schema, _) = arrow_json::reader::infer_json_schema_from_seekable(&mut cursor, None)
        .map_err(|e| SearchDbError::Schema(format!("Failed to infer NDJSON schema: {e}")))?;

    let schema_ref = std::sync::Arc::new(schema);
    let decoder = arrow_json::ReaderBuilder::new(schema_ref)
        .with_batch_size(batch_size)
        .build(cursor)
        .map_err(|e| SearchDbError::Schema(format!("Failed to build NDJSON reader: {e}")))?;

    let mut batches = Vec::new();
    for batch_result in decoder {
        let batch = batch_result
            .map_err(|e| SearchDbError::Schema(format!("Error reading NDJSON batch: {e}")))?;
        batches.push(batch);
    }

    Ok(batches)
}

fn read_json_file(_path: &str, _batch_size: usize) -> Result<Vec<RecordBatch>> {
    todo!("JSON array reader — Task 4")
}

fn read_csv_file(_path: &str, _batch_size: usize) -> Result<Vec<RecordBatch>> {
    todo!("CSV reader — Task 5")
}

fn read_parquet_file(_path: &str) -> Result<Vec<RecordBatch>> {
    todo!("Parquet reader — Task 6")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_ndjson() {
        assert_eq!(detect_format("data.ndjson"), Some(InputFormat::Ndjson));
        assert_eq!(detect_format("data.jsonl"), Some(InputFormat::Ndjson));
    }

    #[test]
    fn test_detect_json() {
        assert_eq!(detect_format("data.json"), Some(InputFormat::Json));
    }

    #[test]
    fn test_detect_csv() {
        assert_eq!(detect_format("data.csv"), Some(InputFormat::Csv));
    }

    #[test]
    fn test_detect_parquet() {
        assert_eq!(detect_format("data.parquet"), Some(InputFormat::Parquet));
    }

    #[test]
    fn test_detect_unknown() {
        assert_eq!(detect_format("data.xml"), None);
        assert_eq!(detect_format("data"), None);
    }

    #[test]
    fn test_detect_case_insensitive() {
        assert_eq!(detect_format("DATA.JSON"), Some(InputFormat::Json));
        assert_eq!(detect_format("file.CSV"), Some(InputFormat::Csv));
        assert_eq!(detect_format("file.Parquet"), Some(InputFormat::Parquet));
    }

    #[test]
    fn test_detect_with_path() {
        assert_eq!(
            detect_format("/tmp/data/file.ndjson"),
            Some(InputFormat::Ndjson)
        );
        assert_eq!(
            detect_format("./relative/path.csv"),
            Some(InputFormat::Csv)
        );
    }

    #[test]
    fn test_parse_format_string() {
        assert_eq!(parse_format("ndjson").unwrap(), InputFormat::Ndjson);
        assert_eq!(parse_format("jsonl").unwrap(), InputFormat::Ndjson);
        assert_eq!(parse_format("json").unwrap(), InputFormat::Json);
        assert_eq!(parse_format("csv").unwrap(), InputFormat::Csv);
        assert_eq!(parse_format("parquet").unwrap(), InputFormat::Parquet);
    }

    #[test]
    fn test_parse_format_invalid() {
        assert!(parse_format("xml").is_err());
    }

    #[test]
    fn test_read_ndjson_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.ndjson");
        std::fs::write(
            &path,
            r#"{"name":"glucose","value":95.0}
{"name":"a1c","value":6.1}
{"name":"creatinine","value":1.2}
"#,
        )
        .unwrap();

        let batches = read_file(path.to_str().unwrap(), InputFormat::Ndjson, 1024).unwrap();
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 3);

        // Verify schema has expected columns
        let schema = batches[0].schema();
        assert!(schema.field_with_name("name").is_ok());
        assert!(schema.field_with_name("value").is_ok());
    }

    #[test]
    fn test_read_ndjson_empty_lines_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.ndjson");
        std::fs::write(
            &path,
            r#"{"name":"glucose"}

{"name":"a1c"}
"#,
        )
        .unwrap();

        let batches = read_file(path.to_str().unwrap(), InputFormat::Ndjson, 1024).unwrap();
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 2);
    }
}

use serde::Deserialize;
use tantivy::query::{BooleanQuery, Occur, Query};
use tantivy::schema::{FieldEntry, FieldType as TvFieldType, IndexRecordOption};
use tantivy::tokenizer::TokenizerManager;
use tantivy::Term;

use super::one_field_map::OneFieldMap;
use crate::error::{Result, SearchDbError};
use crate::schema::Schema as AppSchema;

/// Elasticsearch `match` query -- tokenized full-text search.
///
/// Formats:
/// - `{"match": {"notes": "blood glucose"}}` (shorthand -- default OR)
/// - `{"match": {"notes": {"query": "blood glucose", "operator": "and"}}}` (full)
#[derive(Debug, Clone, Deserialize)]
pub struct MatchQuery(pub OneFieldMap<MatchQueryParams>);

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum MatchQueryParams {
    Full(MatchQueryFullParams),
    Shorthand(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct MatchQueryFullParams {
    pub query: String,
    #[serde(default)]
    pub operator: MatchOperator,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MatchOperator {
    #[default]
    Or,
    And,
}

impl MatchQuery {
    pub fn compile(
        &self,
        tv_schema: &tantivy::schema::Schema,
        _app_schema: &AppSchema,
    ) -> Result<Box<dyn Query>> {
        let field_name = &self.0.field;
        let field = tv_schema.get_field(field_name).map_err(|_| {
            SearchDbError::Schema(format!("field \"{field_name}\" not found in schema"))
        })?;

        let (query_text, operator) = match &self.0.value {
            MatchQueryParams::Full(full) => (full.query.as_str(), &full.operator),
            MatchQueryParams::Shorthand(text) => (text.as_str(), &MatchOperator::Or),
        };

        // Get the tokenizer name from the field's schema entry
        let field_entry: &FieldEntry = tv_schema.get_field_entry(field);
        let tokenizer_name = get_tokenizer_name(field_entry)?;

        // Tokenize the query text
        let tokens = tokenize_text(query_text, &tokenizer_name)?;

        if tokens.is_empty() {
            // zero_terms_query defaults to match_none in ES
            return Ok(Box::new(tantivy::query::EmptyQuery));
        }

        if tokens.len() == 1 {
            let term = Term::from_field_text(field, &tokens[0]);
            return Ok(Box::new(tantivy::query::TermQuery::new(
                term,
                IndexRecordOption::WithFreqs,
            )));
        }

        let occur = match operator {
            MatchOperator::Or => Occur::Should,
            MatchOperator::And => Occur::Must,
        };

        let clauses: Vec<(Occur, Box<dyn Query>)> = tokens
            .into_iter()
            .map(|token| {
                let term = Term::from_field_text(field, &token);
                (
                    occur,
                    Box::new(tantivy::query::TermQuery::new(
                        term,
                        IndexRecordOption::WithFreqs,
                    )) as Box<dyn Query>,
                )
            })
            .collect();

        Ok(Box::new(BooleanQuery::new(clauses)))
    }
}

/// Extract the tokenizer name from a tantivy field entry.
fn get_tokenizer_name(field_entry: &FieldEntry) -> Result<String> {
    match field_entry.field_type() {
        TvFieldType::Str(opts) => {
            let indexing = opts.get_indexing_options().ok_or_else(|| {
                SearchDbError::Schema(format!(
                    "field \"{}\" is not indexed, cannot run match query",
                    field_entry.name()
                ))
            })?;
            Ok(indexing.tokenizer().to_string())
        }
        other => Err(SearchDbError::Schema(format!(
            "match query requires a text field, but \"{}\" is {:?}",
            field_entry.name(),
            other
        ))),
    }
}

/// Tokenize text using tantivy's TokenizerManager.
pub(super) fn tokenize_text(text: &str, tokenizer_name: &str) -> Result<Vec<String>> {
    let manager = TokenizerManager::default();
    let mut tokenizer = manager.get(tokenizer_name).ok_or_else(|| {
        SearchDbError::Schema(format!("tokenizer \"{tokenizer_name}\" not found"))
    })?;

    let mut stream = tokenizer.token_stream(text);
    let mut tokens = Vec::new();
    while stream.advance() {
        tokens.push(stream.token().text.clone());
    }
    Ok(tokens)
}

/// Public helper for match_phrase to reuse tokenization.
pub(super) fn tokenize_for_field(
    tv_schema: &tantivy::schema::Schema,
    field_name: &str,
) -> Result<(tantivy::schema::Field, String)> {
    let field = tv_schema.get_field(field_name).map_err(|_| {
        SearchDbError::Schema(format!("field \"{field_name}\" not found in schema"))
    })?;
    let field_entry = tv_schema.get_field_entry(field);
    let tokenizer_name = get_tokenizer_name(field_entry)?;
    Ok((field, tokenizer_name))
}

/// Alias for match_phrase to use.
pub(super) use tokenize_text as tokenize;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::es_dsl::ElasticQueryDsl;
    use crate::schema::{FieldType, Schema};
    use std::collections::BTreeMap;

    fn text_schema() -> (tantivy::schema::Schema, Schema) {
        let schema = Schema {
            fields: BTreeMap::from([
                ("notes".into(), FieldType::Text),
                ("status".into(), FieldType::Keyword),
            ]),
        };
        let tv = schema.build_tantivy_schema();
        (tv, schema)
    }

    #[test]
    fn test_match_query_deserialize_shorthand() {
        let json = r#"{"match": {"notes": "blood glucose"}}"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        assert!(matches!(dsl, ElasticQueryDsl::Match(_)));
    }

    #[test]
    fn test_match_query_deserialize_full() {
        let json = r#"{"match": {"notes": {"query": "blood glucose", "operator": "and"}}}"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        assert!(matches!(dsl, ElasticQueryDsl::Match(_)));
    }

    #[test]
    fn test_match_query_compile_text_field() {
        let (tv, schema) = text_schema();
        let json = r#"{"match": {"notes": "blood glucose"}}"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        let query = dsl.compile(&tv, &schema);
        assert!(query.is_ok(), "compile failed: {:?}", query.err());
    }

    #[test]
    fn test_match_query_compile_keyword_field() {
        let (tv, schema) = text_schema();
        // Match on keyword field uses raw tokenizer -- whole string becomes single term
        let json = r#"{"match": {"status": "active"}}"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        let query = dsl.compile(&tv, &schema);
        assert!(query.is_ok(), "compile failed: {:?}", query.err());
    }

    #[test]
    fn test_match_query_empty_text() {
        let (tv, schema) = text_schema();
        let json = r#"{"match": {"notes": ""}}"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        let query = dsl.compile(&tv, &schema);
        assert!(query.is_ok());
    }

    #[test]
    fn test_match_query_unknown_field() {
        let (tv, schema) = text_schema();
        let json = r#"{"match": {"missing": "test"}}"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        let result = dsl.compile(&tv, &schema);
        assert!(result.is_err());
    }

    #[test]
    fn test_tokenize_en_stem() {
        let tokens = tokenize_text("running diabetes fasting", "en_stem").unwrap();
        // en_stem tokenizer lowercases and stems
        assert!(tokens.contains(&"run".to_string()));
        assert!(tokens.contains(&"diabet".to_string()));
        assert!(tokens.contains(&"fast".to_string()));
    }

    #[test]
    fn test_tokenize_raw() {
        let tokens = tokenize_text("Hello World", "raw").unwrap();
        assert_eq!(tokens, vec!["Hello World"]);
    }
}

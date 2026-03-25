use serde::Deserialize;
use serde_with::formats::PreferMany;
use serde_with::{serde_as, OneOrMany};
use tantivy::query::{BooleanQuery, ConstScoreQuery, Occur, Query};

use super::ElasticQueryDsl;
use crate::error::Result;
use crate::schema::Schema as AppSchema;

/// Elasticsearch `bool` query -- compound query with must/should/must_not/filter.
///
/// Each clause accepts either a single query or an array of queries.
/// Example:
/// ```json
/// {"bool": {
///   "must": [{"term": {"status": "active"}}],
///   "must_not": {"exists": {"field": "deleted"}},
///   "filter": {"range": {"age": {"gte": 18}}}
/// }}
/// ```
///
/// Edge cases:
/// - If only `must_not` clauses exist, an implicit `match_all` is prepended.
/// - Empty `must`/`should`/`must_not`/`filter` arrays are treated as absent.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
pub struct BoolQuery {
    #[serde_as(as = "Option<OneOrMany<_, PreferMany>>")]
    #[serde(default)]
    pub must: Option<Vec<ElasticQueryDsl>>,

    #[serde_as(as = "Option<OneOrMany<_, PreferMany>>")]
    #[serde(default)]
    pub should: Option<Vec<ElasticQueryDsl>>,

    #[serde_as(as = "Option<OneOrMany<_, PreferMany>>")]
    #[serde(default)]
    pub must_not: Option<Vec<ElasticQueryDsl>>,

    #[serde_as(as = "Option<OneOrMany<_, PreferMany>>")]
    #[serde(default)]
    pub filter: Option<Vec<ElasticQueryDsl>>,
}

impl BoolQuery {
    pub fn compile(
        &self,
        tv_schema: &tantivy::schema::Schema,
        app_schema: &AppSchema,
    ) -> Result<Box<dyn Query>> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        // Must clauses
        if let Some(must_queries) = &self.must {
            for q in must_queries {
                let compiled = q.compile(tv_schema, app_schema)?;
                clauses.push((Occur::Must, compiled));
            }
        }

        // Should clauses
        if let Some(should_queries) = &self.should {
            for q in should_queries {
                let compiled = q.compile(tv_schema, app_schema)?;
                clauses.push((Occur::Should, compiled));
            }
        }

        // Must not clauses
        if let Some(must_not_queries) = &self.must_not {
            for q in must_not_queries {
                let compiled = q.compile(tv_schema, app_schema)?;
                clauses.push((Occur::MustNot, compiled));
            }
        }

        // Filter clauses -- Must with ConstScoreQuery wrapper (no scoring)
        if let Some(filter_queries) = &self.filter {
            for q in filter_queries {
                let compiled = q.compile(tv_schema, app_schema)?;
                clauses.push((Occur::Must, Box::new(ConstScoreQuery::new(compiled, 0.0))));
            }
        }

        // Edge case: if only must_not clauses exist, prepend match_all
        let has_positive = clauses.iter().any(|(occur, _)| *occur != Occur::MustNot);
        if !has_positive && !clauses.is_empty() {
            clauses.insert(0, (Occur::Must, Box::new(tantivy::query::AllQuery)));
        }

        // Edge case: empty bool query = match_all
        if clauses.is_empty() {
            return Ok(Box::new(tantivy::query::AllQuery));
        }

        Ok(Box::new(BooleanQuery::new(clauses)))
    }
}

#[cfg(test)]
mod tests {
    use crate::es_dsl::ElasticQueryDsl;
    use crate::schema::{FieldType, Schema};
    use std::collections::BTreeMap;

    fn test_schema() -> (tantivy::schema::Schema, Schema) {
        let schema = Schema {
            fields: BTreeMap::from([
                ("status".into(), FieldType::Keyword),
                ("notes".into(), FieldType::Text),
                ("age".into(), FieldType::Numeric),
            ]),
        };
        let tv = schema.build_tantivy_schema();
        (tv, schema)
    }

    #[test]
    fn test_bool_query_deserialize_array_clauses() {
        let json = r#"{"bool": {"must": [{"term": {"status": "active"}}]}}"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        assert!(matches!(dsl, ElasticQueryDsl::Bool(_)));
    }

    #[test]
    fn test_bool_query_deserialize_single_clause() {
        // Single object instead of array -- serde_with OneOrMany handles this
        let json = r#"{"bool": {"must": {"term": {"status": "active"}}}}"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        assert!(matches!(dsl, ElasticQueryDsl::Bool(_)));
    }

    #[test]
    fn test_bool_query_compile_must() {
        let (tv, schema) = test_schema();
        let json = r#"{"bool": {"must": [{"term": {"status": "active"}}]}}"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        let query = dsl.compile(&tv, &schema);
        assert!(query.is_ok(), "compile failed: {:?}", query.err());
    }

    #[test]
    fn test_bool_query_compile_must_and_must_not() {
        let (tv, schema) = test_schema();
        let json = r#"{
            "bool": {
                "must": [{"term": {"status": "active"}}],
                "must_not": [{"term": {"status": "deleted"}}]
            }
        }"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        let query = dsl.compile(&tv, &schema);
        assert!(query.is_ok(), "compile failed: {:?}", query.err());
    }

    #[test]
    fn test_bool_query_only_must_not_gets_match_all() {
        let (tv, schema) = test_schema();
        let json = r#"{"bool": {"must_not": [{"term": {"status": "deleted"}}]}}"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        let query = dsl.compile(&tv, &schema);
        assert!(query.is_ok(), "compile failed: {:?}", query.err());
    }

    #[test]
    fn test_bool_query_empty_is_match_all() {
        let (tv, schema) = test_schema();
        let json = r#"{"bool": {}}"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        let query = dsl.compile(&tv, &schema);
        assert!(query.is_ok());
    }

    #[test]
    fn test_bool_query_nested() {
        let (tv, schema) = test_schema();
        let json = r#"{
            "bool": {
                "must": [
                    {"term": {"status": "active"}},
                    {"bool": {
                        "should": [
                            {"match": {"notes": "blood"}},
                            {"match": {"notes": "glucose"}}
                        ]
                    }}
                ]
            }
        }"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        let query = dsl.compile(&tv, &schema);
        assert!(query.is_ok(), "compile failed: {:?}", query.err());
    }

    #[test]
    fn test_bool_query_filter_clause() {
        let (tv, schema) = test_schema();
        let json = r#"{
            "bool": {
                "must": [{"match": {"notes": "glucose"}}],
                "filter": [{"range": {"age": {"gte": 18}}}]
            }
        }"#;
        let dsl: ElasticQueryDsl = serde_json::from_str(json).unwrap();
        let query = dsl.compile(&tv, &schema);
        assert!(query.is_ok(), "compile failed: {:?}", query.err());
    }
}

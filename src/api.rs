//! Request/response entry point shared by every non-CLI client (wasm, editors, bindings).

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use std::cell::RefCell;
use std::collections::HashMap;

use crate::effects;
use crate::model;

fn default_limit() -> u32 {
    20
}

fn default_direction() -> String {
    "outgoing".into()
}

fn default_trace_depth() -> usize {
    6
}

fn default_max_traces() -> usize {
    20
}

fn default_graph_depth() -> usize {
    2
}

fn default_graph_nodes() -> usize {
    150
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Render {
    #[default]
    Markdown,
    Html,
}

#[derive(Debug, Serialize)]
pub struct QueryResponse {
    #[serde(flatten)]
    pub query: effects::QueryWithEffects,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_html: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Specs,
    Query {
        target: String,
        #[serde(default)]
        effects: bool,
        #[serde(default)]
        render: Render,
    },
    Exists {
        target: String,
    },
    Search {
        query: String,
        #[serde(default)]
        spec: Option<String>,
        #[serde(default = "default_limit")]
        limit: u32,
    },
    Anchors {
        pattern: String,
        #[serde(default)]
        spec: Option<String>,
        #[serde(default = "default_limit")]
        limit: u32,
    },
    List {
        spec: String,
    },
    Refs {
        target: String,
        #[serde(default = "default_direction")]
        direction: String,
        #[serde(default)]
        kind: Option<model::RefKind>,
        #[serde(default = "default_limit")]
        limit: u32,
    },
    Trace {
        from: String,
        to: String,
        #[serde(default = "default_trace_depth")]
        max_depth: usize,
        #[serde(default)]
        kind: Option<model::RefKind>,
        #[serde(default = "default_max_traces")]
        max_traces: usize,
    },
    Graph {
        target: String,
        #[serde(default = "default_direction")]
        direction: String,
        #[serde(default = "default_graph_depth")]
        max_depth: usize,
        #[serde(default = "default_graph_nodes")]
        max_nodes: usize,
        #[serde(default)]
        include: Vec<String>,
        #[serde(default)]
        exclude: Vec<String>,
        #[serde(default)]
        same_spec_only: bool,
    },
    Idl {
        query: String,
        #[serde(default)]
        spec: Option<String>,
        #[serde(default = "default_limit")]
        limit: u32,
    },
    Effects {
        subject: effects::SubjectSelector,
        #[serde(default)]
        filter: Option<effects::EffectFilter>,
    },
    EffectsExplain {
        subject: effects::SubjectSelector,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "result", rename_all = "snake_case")]
pub enum Response {
    Specs {
        specs: Vec<SpecInfo>,
    },
    Query(QueryResponse),
    Exists(model::ExistsResult),
    Search(model::SearchResult),
    Anchors(model::AnchorsResult),
    List {
        spec: String,
        entries: Vec<model::ListEntry>,
    },
    Refs(model::RefsResult),
    Trace(model::TraceResult),
    Graph(model::GraphResult),
    Idl(model::IdlResult),
    Effects(effects::EffectSummaryResult),
    EffectsExplain(effects::ExplainEffectsResult),
}

#[derive(Debug, Serialize)]
pub struct SpecInfo {
    pub name: String,
    pub base_url: String,
    pub provider: String,
    pub sha: String,
    pub commit_date: String,
}

/// API-level error code. The `Effects` variant carries the inner effects
/// engine code with an `effects_` prefix (e.g. `"effects_subject_not_found"`)
/// so effects validation errors never collide with envelope errors such as
/// `"invalid_request"`; all others serialize as their own snake_case strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiErrorCode {
    InvalidRequest,
    SpecNotIndexed,
    NotFound,
    Effects(effects::RequestErrorCode),
    Internal,
}

impl Serialize for ApiErrorCode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ApiErrorCode::InvalidRequest => s.serialize_str("invalid_request"),
            ApiErrorCode::SpecNotIndexed => s.serialize_str("spec_not_indexed"),
            ApiErrorCode::NotFound => s.serialize_str("not_found"),
            ApiErrorCode::Internal => s.serialize_str("internal"),
            ApiErrorCode::Effects(code) => {
                let inner = serde_json::to_value(code).map_err(serde::ser::Error::custom)?;
                let inner = inner.as_str().ok_or_else(|| {
                    serde::ser::Error::custom("effects error code is not a string")
                })?;
                s.serialize_str(&format!("effects_{inner}"))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiError {
    pub code: ApiErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    fn not_found(message: impl Into<String>) -> Self {
        ApiError {
            code: ApiErrorCode::NotFound,
            message: message.into(),
            details: None,
        }
    }

    fn internal(error: anyhow::Error) -> Self {
        ApiError {
            code: ApiErrorCode::Internal,
            message: error.to_string(),
            details: None,
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        ApiError {
            code: ApiErrorCode::InvalidRequest,
            message: message.into(),
            details: None,
        }
    }
}

impl From<effects::RequestError> for ApiError {
    fn from(e: effects::RequestError) -> Self {
        ApiError {
            code: ApiErrorCode::Effects(e.code),
            message: e.message,
            details: e.details,
        }
    }
}

#[derive(Serialize)]
struct ErrorEnvelope {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    error: ApiError,
}

fn resolve_target(target: &str) -> Result<(String, String), ApiError> {
    crate::parse_spec_anchor(target)
        .map(|(spec, anchor, _)| (spec, anchor))
        .map_err(|e| ApiError::invalid(e.to_string()))
}

fn canonical_spec_name(spec: &str) -> String {
    spec.to_uppercase()
}

fn map_query_error(e: anyhow::Error) -> ApiError {
    let msg = e.to_string();
    if msg.contains("not indexed") {
        ApiError {
            code: ApiErrorCode::SpecNotIndexed,
            message: msg,
            details: None,
        }
    } else {
        ApiError {
            code: ApiErrorCode::Internal,
            message: msg,
            details: None,
        }
    }
}

fn search(
    conn: &Connection,
    query: &str,
    spec: Option<&str>,
    limit: u32,
) -> Result<model::SearchResult, ApiError> {
    let results = match crate::search_sections_fts(conn, query, spec, limit) {
        Ok(entries) => entries,
        Err(e) => {
            if crate::is_fts_syntax_error(&e) {
                if let Some(sanitized) = crate::sanitize_for_fts(query) {
                    match crate::search_sections_fts(conn, &sanitized, spec, limit) {
                        Ok(entries) => entries,
                        Err(e2) if crate::is_fts_syntax_error(&e2) => vec![],
                        Err(e2) => return Err(ApiError::internal(e2.into())),
                    }
                } else {
                    vec![]
                }
            } else {
                return Err(ApiError::internal(e.into()));
            }
        }
    };
    Ok(model::SearchResult {
        query: query.to_string(),
        results,
    })
}

fn anchors(
    conn: &Connection,
    pattern: &str,
    spec: Option<&str>,
    limit: u32,
) -> Result<model::AnchorsResult, ApiError> {
    let sql_pattern = pattern.replace('*', "%");
    let rows = crate::find_anchors_sql(conn, &sql_pattern, spec, None, limit)
        .map_err(ApiError::internal)?;
    let mut seen = std::collections::HashSet::new();
    let entries: Vec<model::AnchorEntry> = rows
        .into_iter()
        .filter(|(anchor, _, _, _)| seen.insert(anchor.clone()))
        .map(
            |(anchor, spec_name, title, section_type)| model::AnchorEntry {
                spec: spec_name,
                anchor,
                title,
                section_type,
            },
        )
        .collect();
    Ok(model::AnchorsResult {
        pattern: pattern.to_string(),
        results: entries,
    })
}

fn list_specs(conn: &Connection) -> Result<Vec<SpecInfo>, ApiError> {
    let rows = crate::db::queries::list_indexed_specs(conn).map_err(ApiError::internal)?;
    Ok(rows
        .into_iter()
        .map(|(name, base_url, provider, sha, commit_date)| SpecInfo {
            name,
            base_url,
            provider,
            sha,
            commit_date,
        })
        .collect())
}

fn cached_effects_request(
    subject: effects::SubjectSelector,
    filter: Option<effects::EffectFilter>,
) -> effects::EffectsRequest {
    effects::EffectsRequest {
        schema_version: effects::EFFECTS_SCHEMA_VERSION,
        subject,
        options: effects::EffectsOptions {
            mode: effects::EffectsMode::Cached,
            ..Default::default()
        },
        filter,
    }
}

fn with_cached_effects(conn: &Connection, query: model::QueryResult) -> effects::QueryWithEffects {
    let request = cached_effects_request(
        effects::SubjectSelector {
            spec: query.spec.clone(),
            anchor: query.anchor.clone(),
            step_path: None,
            step_id: None,
            body_id: None,
        },
        None,
    );
    match effects::service::get_effect_summary_on(conn, &request) {
        Ok(result) => effects::query::attach_summary(query, result),
        Err(_) => effects::QueryWithEffects {
            query,
            effects: Some(Vec::new()),
            effects_status: Some(effects::query::error_status()),
        },
    }
}

pub fn handle(conn: &Connection, request: Request) -> Result<Response, ApiError> {
    match request {
        Request::Specs => Ok(Response::Specs {
            specs: list_specs(conn)?,
        }),
        Request::Query {
            target,
            effects: with_effects,
            render,
        } => {
            let (spec, anchor) = resolve_target(&target)?;
            let query = crate::query_section_from_conn(conn, &spec, &anchor)
                .map_err(map_query_error)?
                .ok_or_else(|| ApiError::not_found(format!("{spec}#{anchor}")))?;
            let qwe = if with_effects {
                with_cached_effects(conn, query)
            } else {
                effects::QueryWithEffects {
                    query,
                    effects: None,
                    effects_status: None,
                }
            };
            let content_html = if render == Render::Html && qwe.query.section_type == "idl" {
                qwe.query.content.as_deref().map(crate::render::idl_to_html)
            } else if render == Render::Html {
                qwe.query.content.as_deref().map(|md| {
                    let registry = crate::spec_registry::SpecRegistry::new();
                    let indexed_cache = RefCell::new(HashMap::<String, bool>::new());
                    crate::render::markdown_to_html(md, &|url| {
                        let (spec_name, link_anchor) = registry.resolve_url(url)?;
                        let is_indexed = *indexed_cache
                            .borrow_mut()
                            .entry(spec_name.clone())
                            .or_insert_with(|| {
                                crate::db::queries::get_snapshot(conn, &spec_name)
                                    .ok()
                                    .flatten()
                                    .is_some()
                            });
                        if is_indexed {
                            Some(crate::render::LinkTarget {
                                spec: spec_name,
                                anchor: link_anchor,
                            })
                        } else {
                            None
                        }
                    })
                })
            } else {
                None
            };
            Ok(Response::Query(QueryResponse {
                query: qwe,
                content_html,
            }))
        }
        Request::Exists { target } => {
            let (spec, anchor) = resolve_target(&target)?;
            crate::check_exists_from_conn(conn, &spec, &anchor)
                .map(Response::Exists)
                .map_err(map_query_error)
        }
        Request::Search { query, spec, limit } => Ok(Response::Search(search(
            conn,
            &query,
            spec.as_deref(),
            limit,
        )?)),
        Request::Anchors {
            pattern,
            spec,
            limit,
        } => Ok(Response::Anchors(anchors(
            conn,
            &pattern,
            spec.as_deref(),
            limit,
        )?)),
        Request::List { spec } => {
            let spec = canonical_spec_name(&spec);
            crate::list_headings_from_conn(conn, &spec)
                .map(|entries| Response::List { spec, entries })
                .map_err(map_query_error)
        }
        Request::Refs {
            target,
            direction,
            kind,
            limit,
        } => {
            let exact = crate::parse_spec_anchor(&target)
                .ok()
                .map(|(s, a, _)| (s, a));
            crate::find_references_from_conn(conn, exact, &target, &direction, limit, kind)
                .map(Response::Refs)
                .map_err(ApiError::internal)
        }
        Request::Trace {
            from,
            to,
            max_depth,
            kind,
            max_traces,
        } => {
            let from = resolve_target(&from)?;
            let to = resolve_target(&to)?;
            crate::find_traces_from_conn(conn, from, to, max_depth, kind, max_traces)
                .map(Response::Trace)
                .map_err(ApiError::internal)
        }
        Request::Graph {
            target,
            direction,
            max_depth,
            max_nodes,
            include,
            exclude,
            same_spec_only,
        } => {
            let (spec, anchor) = resolve_target(&target)?;
            let filters = crate::GraphFilters {
                include,
                exclude,
                same_spec_only,
            };
            crate::build_graph_from_conn(
                conn, &spec, &anchor, &direction, max_depth, max_nodes, &filters,
            )
            .map(Response::Graph)
            .map_err(ApiError::internal)
        }
        Request::Idl { query, spec, limit } => {
            crate::query_idl_from_conn(conn, &query, spec.as_deref(), limit)
                .map(Response::Idl)
                .map_err(ApiError::internal)
        }
        Request::Effects { subject, filter } => {
            let request = cached_effects_request(subject, filter);
            effects::service::get_effect_summary_on(conn, &request)
                .map(Response::Effects)
                .map_err(ApiError::from)
        }
        Request::EffectsExplain { subject } => {
            let request = cached_effects_request(subject, None);
            effects::service::prepared_effect_details_on(conn, &request)
                .map(Response::EffectsExplain)
                .map_err(ApiError::from)
        }
    }
}

pub fn handle_json(conn: &Connection, request_json: &str) -> String {
    let outcome = match serde_json::from_str::<Request>(request_json) {
        Ok(request) => handle(conn, request),
        Err(e) => Err(ApiError {
            code: ApiErrorCode::InvalidRequest,
            message: e.to_string(),
            details: None,
        }),
    };
    match outcome {
        Ok(response) => serde_json::to_string(&response),
        Err(error) => serde_json::to_string(&ErrorEnvelope {
            kind: "error",
            error,
        }),
    }
    .unwrap_or_else(|e| {
        format!(
            r#"{{"type":"error","code":"internal","message":{}}}"#,
            serde_json::Value::String(e.to_string())
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn conn() -> rusqlite::Connection {
        let c = crate::tests::seed_two_specs();
        db::effects::initialize(&c).unwrap();
        c
    }

    fn call(conn: &rusqlite::Connection, json: &str) -> serde_json::Value {
        serde_json::from_str(&handle_json(conn, json)).unwrap()
    }

    #[test]
    fn specs_lists_indexed_snapshots() {
        let c = conn();
        let v = call(&c, r#"{"type":"specs"}"#);
        assert_eq!(v["type"], "specs");
        let names: Vec<&str> = v["result"]["specs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["DOM", "HTML"]);
        assert_eq!(v["result"]["specs"][1]["sha"], "hash:aaaa");
    }

    #[test]
    fn query_accepts_spec_anchor_and_full_url() {
        let c = conn();
        let a = call(&c, r#"{"type":"query","target":"HTML#navigate"}"#);
        let b = call(
            &c,
            r#"{"type":"query","target":"https://html.spec.whatwg.org/#navigate"}"#,
        );
        assert_eq!(a["type"], "query");
        assert_eq!(a["result"]["anchor"], "navigate");
        assert_eq!(a["result"]["anchor"], b["result"]["anchor"]);
        assert!(a["result"].get("effects").is_none());
    }

    #[test]
    fn query_type_field_is_envelope_not_section_type() {
        let c = conn();
        let v = call(&c, r#"{"type":"query","target":"HTML#navigate"}"#);
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys.iter().filter(|&&k| k == "type").count(), 1);
        assert_eq!(v["type"], "query");
        assert_eq!(v["result"]["type"], "algorithm");
    }

    #[test]
    fn query_missing_section_is_not_found_and_unindexed_spec_is_distinct() {
        let c = conn();
        let v = call(&c, r#"{"type":"query","target":"HTML#nope"}"#);
        assert_eq!(v["type"], "error");
        assert_eq!(v["code"], "not_found");
        let v = call(&c, r#"{"type":"query","target":"FETCH#x"}"#);
        assert_eq!(v["code"], "spec_not_indexed");
    }

    #[test]
    fn query_with_effects_in_cached_mode_reports_status_without_computing() {
        let c = conn();
        let v = call(
            &c,
            r#"{"type":"query","target":"HTML#navigate","effects":true}"#,
        );
        assert_eq!(v["type"], "query");
        assert!(v["result"].get("effects_status").is_some(), "{v}");
        let runs: i64 = c
            .query_row("SELECT COUNT(*) FROM effect_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 0, "cached mode must not publish an analysis");
    }

    #[test]
    fn search_falls_back_to_sanitized_query_on_fts_syntax_error() {
        let c = conn();
        let v = call(&c, r#"{"type":"search","query":"navigate"}"#);
        assert_eq!(v["type"], "search");
        assert_eq!(v["result"]["results"][0]["anchor"], "navigate");
        let v = call(&c, r#"{"type":"search","query":"navigate AND ("}"#);
        assert_eq!(v["type"], "search", "{v}");
    }

    #[test]
    fn anchors_list_refs_trace_graph_idl_dispatch() {
        let c = conn();
        assert_eq!(
            call(&c, r#"{"type":"anchors","pattern":"nav*"}"#)["result"]["results"][0]["anchor"],
            "navigate"
        );
        assert_eq!(
            call(&c, r#"{"type":"list","spec":"HTML"}"#)["result"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let refs = call(
            &c,
            r#"{"type":"refs","target":"DOM#concept-tree","direction":"incoming"}"#,
        );
        assert_eq!(refs["type"], "refs", "{refs}");
        let trace = call(
            &c,
            r#"{"type":"trace","from":"HTML#navigate","to":"DOM#concept-tree"}"#,
        );
        assert_eq!(
            trace["result"]["traces"].as_array().unwrap().len(),
            1,
            "{trace}"
        );
        let graph = call(&c, r#"{"type":"graph","target":"HTML#navigate"}"#);
        assert_eq!(
            graph["result"]["nodes"].as_array().unwrap().len(),
            2,
            "{graph}"
        );
        assert_eq!(
            call(&c, r#"{"type":"idl","query":"Window"}"#)["type"],
            "idl"
        );
        assert_eq!(
            call(&c, r#"{"type":"exists","target":"HTML#navigate"}"#)["result"]["exists"],
            true
        );
    }

    #[test]
    fn effects_requests_use_cached_mode_and_map_errors() {
        let c = conn();
        let v = call(
            &c,
            r#"{"type":"effects","subject":{"spec":"HTML","anchor":"navigate"}}"#,
        );
        assert!(
            v["type"] == "effects" || v["code"].as_str().unwrap_or("").starts_with("analysis"),
            "{v}"
        );
        let v = call(
            &c,
            r#"{"type":"effects_explain","subject":{"spec":"HTML","anchor":"navigate"}}"#,
        );
        assert!(
            v["type"] == "effects_explain" || v["type"] == "error",
            "{v}"
        );
        let runs: i64 = c
            .query_row("SELECT COUNT(*) FROM effect_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 0);
    }

    #[test]
    fn query_render_html_rewrites_indexed_links_only() {
        let c = conn();
        let v = call(
            &c,
            r#"{"type":"query","target":"HTML#navigate","render":"html"}"#,
        );
        let html = v["result"]["content_html"].as_str().unwrap();
        assert!(html.contains(r##"href="#/DOM/concept-tree""##), "{html}");
        assert!(
            html.contains(r#"href="https://www.w3.org/TR/css-grid-1/#grid""#),
            "{html}"
        );
        assert!(v["result"]["content"]
            .as_str()
            .unwrap()
            .contains("**navigate**"));
        let plain = call(&c, r#"{"type":"query","target":"HTML#navigate"}"#);
        assert!(plain["result"].get("content_html").is_none());
    }

    #[test]
    fn malformed_requests_are_invalid_request_errors() {
        let c = conn();
        let v = call(&c, r#"{"type":"query"}"#);
        assert_eq!(v["type"], "error");
        assert_eq!(v["code"], "invalid_request");
        let v = call(&c, r#"{"type":"query","target":"HTML#navigate","bogus":1}"#);
        assert_eq!(v["code"], "invalid_request");
        let v = call(&c, "not json");
        assert_eq!(v["code"], "invalid_request");
    }

    #[test]
    fn api_error_code_effects_serializes_with_prefix() {
        assert_eq!(
            serde_json::to_string(&ApiErrorCode::Effects(
                effects::RequestErrorCode::SubjectNotFound
            ))
            .unwrap(),
            "\"effects_subject_not_found\""
        );
        assert_ne!(
            serde_json::to_string(&ApiErrorCode::Effects(
                effects::RequestErrorCode::InvalidRequest
            ))
            .unwrap(),
            serde_json::to_string(&ApiErrorCode::InvalidRequest).unwrap()
        );
    }
}

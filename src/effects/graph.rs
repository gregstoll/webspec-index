//! Finite execution graph representation used by effect propagation.

use crate::effects::model::{Boundary, ContextRef, Execution, Relationship, SourceSite, Subject};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub trait SiteStore {
    fn site(&self, key: &str) -> Option<SourceSite>;
}

impl SiteStore for BTreeMap<String, SourceSite> {
    fn site(&self, key: &str) -> Option<SourceSite> {
        self.get(key).cloned()
    }
}

pub fn site_key(site: &SourceSite) -> String {
    let digest = crate::effects::model::digest_serializable(site).expect("serializable SourceSite");
    format!("site_{}", &digest[..20])
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionNode {
    pub id: String,
    pub subject: Subject,
    pub source_order: u64,
    pub is_body: bool,
    pub definition_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub relation: Relationship,
    pub execution: Execution,
    pub site_key: String,
    pub site_id: String,
    pub context: Vec<ContextRef>,
    #[serde(default)]
    pub context_truncated: bool,
    pub boundary: Option<Boundary>,
    pub source_order: u64,
}

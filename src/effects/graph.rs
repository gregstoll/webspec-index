//! Finite execution graph representation used by effect propagation.

use crate::effects::model::{Boundary, ContextItem, Execution, Relationship, SourceSite, Subject};
use serde::{Deserialize, Serialize};

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
    pub site: SourceSite,
    pub context: Vec<ContextItem>,
    #[serde(default)]
    pub context_truncated: bool,
    pub boundary: Option<Boundary>,
    pub source_order: u64,
}

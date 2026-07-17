//! Run-local affinity planning for shared Query Sources.
//!
//! The planner deliberately works on stable ids only. Connection payloads and
//! secrets are resolved at the execution boundary, never while building this
//! graph.

use super::{PipelineDoc, PlannerError};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffinityStageMode {
    /// The stage can execute while the affinity session remains available.
    SessionPreserving,
    /// The stage needs executor coordination around the session boundary.
    SessionSuspending,
    /// The stage cannot participate in an affinity group.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffinityGroup {
    pub id: String,
    pub query_source_ids: Vec<String>,
    pub data_source_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffinityPlan {
    pub groups: Vec<AffinityGroup>,
    pub node_to_group: BTreeMap<String, String>,
}

pub type AffinityError = PlannerError;

/// Build an affinity plan for all Query Sources in the document.
pub fn build_affinity_plan(pipeline: &PipelineDoc, known_data_source_ids: &HashSet<String>) -> Result<AffinityPlan, AffinityError> {
    build_affinity_plan_for_nodes(pipeline, known_data_source_ids, None)
}

/// Build an affinity plan for a selected execution subgraph. The selection is
/// applied before connected components are calculated, so unrelated canvas
/// nodes cannot accidentally merge two run-local groups.
pub fn build_affinity_plan_for_nodes(
    pipeline: &PipelineDoc,
    known_data_source_ids: &HashSet<String>,
    selected_node_ids: Option<&HashSet<String>>,
) -> Result<AffinityPlan, AffinityError> {
    let mut query_refs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for node in &pipeline.nodes {
        if selected_node_ids.is_some_and(|ids| !ids.contains(&node.id)) || node.data.component_id.as_deref() != Some("src.query") {
            continue;
        }
        let refs = node
            .data
            .properties
            .as_ref()
            .and_then(|props| props.get("dataSourceRefs"))
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::trim))
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        if refs.is_empty() {
            return Err(AffinityError::QuerySourceHasNoDataSources { node_id: node.id.clone() });
        }
        for data_source_id in &refs {
            if !known_data_source_ids.contains(data_source_id) {
                return Err(AffinityError::MissingDataSourceRef {
                    node_id: node.id.clone(),
                    data_source_id: data_source_id.clone(),
                });
            }
        }
        query_refs.insert(node.id.clone(), refs);
    }

    let mut source_to_queries: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (query_id, refs) in &query_refs {
        for data_source_id in refs {
            source_to_queries.entry(data_source_id.clone()).or_default().push(query_id.clone());
        }
    }
    for queries in source_to_queries.values_mut() {
        queries.sort();
    }

    let mut visited = HashSet::new();
    let mut groups = Vec::new();
    let mut node_to_group = BTreeMap::new();
    for query_id in query_refs.keys() {
        if !visited.insert(query_id.clone()) {
            continue;
        }
        let mut queries = BTreeSet::new();
        let mut data_sources = BTreeSet::new();
        let mut queue = VecDeque::from([query_id.clone()]);
        while let Some(current_query) = queue.pop_front() {
            queries.insert(current_query.clone());
            for data_source_id in query_refs.get(&current_query).into_iter().flat_map(|refs| refs.iter()) {
                data_sources.insert(data_source_id.clone());
                for adjacent_query in source_to_queries.get(data_source_id).into_iter().flat_map(|ids| ids.iter()) {
                    if visited.insert(adjacent_query.clone()) {
                        queue.push_back(adjacent_query.clone());
                    }
                }
            }
        }
        let id = format!("ag-{:03}", groups.len() + 1);
        for query in &queries {
            node_to_group.insert(query.clone(), id.clone());
        }
        groups.push(AffinityGroup {
            id,
            query_source_ids: queries.into_iter().collect(),
            data_source_ids: data_sources.into_iter().collect(),
        });
    }

    Ok(AffinityPlan { groups, node_to_group })
}

/// Compatibility matrix for a stage that occurs while an affinity worker owns
/// the run database:
///
/// | Stage | Mode | Reason |
/// |---|---|---|
/// | `src.query` | preserving | Uses the worker's attach-once session. |
/// | Pure SQL (`runtime == None`) | preserving | Executes serially in the same CLI. |
/// | `ctl.log` / `ctl.warn` | preserving | Emits an event while executing its SQL pass-through. |
/// | Other runtime `xf.*` / `ctl.*` | suspending | Needs an explicit materialize/suspend/resume boundary. |
/// | Other runtime stage | unsupported | Its executor may create a separate DuckDB connection. |
///
/// The executor must never silently fall back to per-stage execution for an
/// unsupported stage: doing so would invalidate the session ownership contract.
pub fn classify_affinity_stage(component_id: &str, has_runtime: bool) -> AffinityStageMode {
    if component_id == "src.query"
        || component_id == "code.python"
        || matches!(component_id, "ctl.log" | "ctl.warn")
        || !has_runtime
    {
        AffinityStageMode::SessionPreserving
    } else if component_id.starts_with("ctl.") || component_id.starts_with("xf.") {
        AffinityStageMode::SessionSuspending
    } else {
        AffinityStageMode::Unsupported
    }
}

/// Small helper used by diagnostics and tests to avoid exposing unordered
/// map iteration in event payloads.
pub fn group_by_node(plan: &AffinityPlan) -> HashMap<String, String> {
    plan.node_to_group.clone().into_iter().collect()
}

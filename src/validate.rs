//! Validation for `WorkerSpec`: structural and semantic checks beyond what
//! serde/schema enforce (e.g. cross-field invariants like kind/block
//! agreement, specialist cardinality, and iteration budget ranges).
use std::collections::HashSet;

use crate::model::{AgentKind, WorkerSpec};

/// A single validation failure. `field` names the offending path
/// (dot-separated, e.g. `"agent_graph.specialists"`) so callers can route
/// errors back to the originating form field or YAML key.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl ValidationError {
    fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

/// Validate a `WorkerSpec`, collecting every violation rather than stopping
/// at the first. Returns `Ok(())` when the spec is valid.
pub fn validate(spec: &WorkerSpec) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    validate_name(spec, &mut errors);
    validate_agent_graph(spec, &mut errors);
    validate_deep_worker(spec, &mut errors);
    validate_kind_block_agreement(spec, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_name(spec: &WorkerSpec, errors: &mut Vec<ValidationError>) {
    if spec.name.trim().is_empty() {
        errors.push(ValidationError::new(
            "name",
            "name must not be empty or whitespace-only",
        ));
    }
}

fn validate_agent_graph(spec: &WorkerSpec, errors: &mut Vec<ValidationError>) {
    if spec.kind != AgentKind::AgentGraph {
        return;
    }
    let Some(agent_graph) = &spec.agent_graph else {
        errors.push(ValidationError::new(
            "agent_graph.specialists",
            "agent_graph kind requires an agent_graph block with at least 2 specialists",
        ));
        return;
    };

    if agent_graph.specialists.len() < 2 {
        errors.push(ValidationError::new(
            "agent_graph.specialists",
            format!(
                "agent_graph requires at least 2 specialists, found {}",
                agent_graph.specialists.len()
            ),
        ));
    }

    let mut seen = HashSet::new();
    for specialist in &agent_graph.specialists {
        if !seen.insert(specialist.name.as_str()) {
            errors.push(ValidationError::new(
                "agent_graph.specialists",
                format!("duplicate specialist name: {}", specialist.name),
            ));
        }
    }
}

fn validate_deep_worker(spec: &WorkerSpec, errors: &mut Vec<ValidationError>) {
    if spec.kind != AgentKind::DeepWorker {
        return;
    }
    let Some(deep_worker) = &spec.deep_worker else {
        return;
    };

    if !(1..=100).contains(&deep_worker.iteration_budget) {
        errors.push(ValidationError::new(
            "deep_worker.iteration_budget",
            format!(
                "iteration_budget must be between 1 and 100, found {}",
                deep_worker.iteration_budget
            ),
        ));
    }
}

fn validate_kind_block_agreement(spec: &WorkerSpec, errors: &mut Vec<ValidationError>) {
    match spec.kind {
        AgentKind::SingleTurn => {
            if spec.agent_graph.is_some() {
                errors.push(ValidationError::new(
                    "kind",
                    "kind is single_turn but an agent_graph block is set",
                ));
            }
            if spec.deep_worker.is_some() {
                errors.push(ValidationError::new(
                    "kind",
                    "kind is single_turn but a deep_worker block is set",
                ));
            }
        }
        AgentKind::AgentGraph => {
            if spec.deep_worker.is_some() {
                errors.push(ValidationError::new(
                    "kind",
                    "kind is agent_graph but a deep_worker block is set",
                ));
            }
        }
        AgentKind::DeepWorker => {
            if spec.agent_graph.is_some() {
                errors.push(ValidationError::new(
                    "kind",
                    "kind is deep_worker but an agent_graph block is set",
                ));
            }
        }
    }
}

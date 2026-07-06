//! Project a [`WorkerSpec`] into the untyped "answer document" JSON that the
//! pack assembler consumes.
//!
//! Ported from the Designer's `dw_form_to_answer_doc::convert`
//! (`greentic-designer/src/orchestrate/dw_form_to_answer_doc.rs`), adapted to
//! read from [`WorkerSpec`] instead of `DwFormState`. The output shape is
//! kept field-for-field compatible with that function: top-level
//! `manifest_id` / `display_name` / `manifest` (with nested `metadata`,
//! `capability_plan`, `defaults`, `behavior_scaffold`) / `provider_overrides`
//! / `locale` / `tenant`, plus optional `extension_tools` / `guardrails` /
//! `executing_node`.

use crate::model::{AgentKind, ExtensionToolBinding, WorkerSpec};
use crate::slug::slugify;
use serde_json::{json, Map, Value};

/// Errors produced while projecting a [`WorkerSpec`] into an answer
/// document.
///
/// `MissingLlm` is retained for parity with the Designer's `MappingError`
/// even though `WorkerSpec.llm` is currently non-optional; full validation
/// (e.g. requiring a `cap://llm/chat`-capable binding) lands in a later
/// task.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("missing LLM binding — at least one enabled provider with cap://llm/chat is required")]
    MissingLlm,
}

/// Map the worker's [`AgentKind`] onto the `executing_node` marker the
/// runner reads to pick the right flow node. `SingleTurn` needs no special
/// node, so it maps to `None`.
pub fn executing_node(spec: &WorkerSpec) -> Option<Value> {
    match spec.kind {
        AgentKind::SingleTurn => None,
        AgentKind::AgentGraph => Some(json!({ "kind": "dw.agent_graph" })),
        AgentKind::DeepWorker => {
            let dw = spec.deep_worker.clone().unwrap_or_default();
            Some(json!({ "kind": "operala.call", "deep_worker": dw }))
        }
    }
}

/// Convert a [`WorkerSpec`] into the AnswerDocument JSON the pack builder
/// expects, mirroring the Designer's `convert()` output shape.
pub fn to_answer_document(spec: &WorkerSpec) -> Result<Value, ProjectError> {
    let manifest_id = resolve_manifest_id(spec);
    let display_name = if spec.name.is_empty() {
        "Agentic Worker".to_string()
    } else {
        spec.name.clone()
    };

    let mut caps: Vec<String> = spec.tools.clone();
    caps.push("cap://llm/chat".to_string());
    caps.sort();
    caps.dedup();

    let mut provider_ids = Map::new();
    provider_ids.insert(
        "cap://llm/chat".to_string(),
        Value::String(spec.llm.provider.clone()),
    );

    let defaults_values = build_defaults_values(spec, &display_name);

    let manifest = json!({
        "metadata": {
            "id": manifest_id,
            "name": display_name,
            "summary": spec.description,
            "category": spec.vertical,
            "tags": [],
            "maturity": "experimental"
        },
        "capability_plan": {
            "required_capabilities": caps,
            "optional_capabilities": [],
            "default_provider_ids": provider_ids
        },
        "defaults": { "values": defaults_values },
        "behavior_scaffold": {
            "default_mode_behavior": { "question_block_ids": [] }
        }
    });

    let mut provider_overrides = Map::new();
    provider_overrides.insert(
        "cap://llm/chat".to_string(),
        Value::String(spec.llm.provider.clone()),
    );

    let mut answer_doc = json!({
        "manifest_id": manifest_id,
        "display_name": display_name,
        "manifest": manifest,
        "provider_overrides": provider_overrides,
        "locale": spec.locale,
        "tenant": spec.tenant
    });

    if !spec.extension_tools.is_empty() {
        let tools: Vec<Value> = spec
            .extension_tools
            .iter()
            .map(extension_tool_to_doc)
            .collect();
        if let Value::Object(map) = &mut answer_doc {
            map.insert("extension_tools".to_string(), Value::Array(tools));
        }
    }

    if !spec.guardrails.is_empty() {
        let guardrails: Vec<Value> = spec.guardrails.iter().map(guardrail_to_doc).collect();
        if let Value::Object(map) = &mut answer_doc {
            map.insert("guardrails".to_string(), Value::Array(guardrails));
        }
    }

    if let Some(node) = executing_node(spec) {
        if let Value::Object(map) = &mut answer_doc {
            map.insert("executing_node".to_string(), node);
        }
    }

    Ok(answer_doc)
}

/// Serialize one [`ExtensionToolBinding`] into the snake_case `ExtensionTool`
/// shape greentic-dw expects. Mirrors the Designer's
/// `dw_form_to_answer_doc::extension_tool_to_doc`.
fn extension_tool_to_doc(b: &ExtensionToolBinding) -> Value {
    let mut m = Map::new();
    m.insert("extension_id".into(), Value::String(b.extension_id.clone()));
    m.insert(
        "extension_version".into(),
        Value::String(b.extension_version.clone()),
    );
    m.insert("tool_name".into(), Value::String(b.tool_name.clone()));
    m.insert("description".into(), Value::String(b.description.clone()));
    m.insert(
        "input_schema_json".into(),
        Value::String(b.input_schema_json.clone()),
    );
    if let Some(out) = &b.output_schema_json {
        m.insert("output_schema_json".into(), Value::String(out.clone()));
    }
    m.insert(
        "capabilities".into(),
        Value::Array(b.capabilities.iter().cloned().map(Value::String).collect()),
    );
    m.insert(
        "agentic_worker_metadata".into(),
        serde_json::to_value(&b.agentic_worker_metadata).unwrap_or(Value::Null),
    );
    if let Some(note) = b.usage_note.as_deref() {
        if !note.trim().is_empty() {
            m.insert("usage_note".into(), Value::String(note.to_string()));
        }
    }
    Value::Object(m)
}

/// Serialize one guardrail capability id into the runner's snake_case
/// `GuardrailRef` shape (`cap_id` + `config`). Mirrors the Designer's
/// `dw_form_to_answer_doc::guardrail_to_doc`. `WorkerSpec.guardrails` is a
/// flat `Vec<String>` of capability ids (no per-guardrail config yet), so
/// `config` is emitted as an empty object.
fn guardrail_to_doc(cap_id: &String) -> Value {
    json!({ "cap_id": cap_id, "config": {} })
}

fn build_defaults_values(spec: &WorkerSpec, display_name: &str) -> Map<String, Value> {
    let mut values = Map::new();
    values.insert(
        "display_name".into(),
        Value::String(display_name.to_string()),
    );
    if let Some(loc) = &spec.locale {
        values.insert("worker_default_locale".into(), Value::String(loc.clone()));
    }
    values.insert(
        "system_prompt".into(),
        Value::String(spec.instructions.clone()),
    );
    if let Some(open) = &spec.opening_message {
        values.insert("opening_message".into(), Value::String(open.clone()));
    }
    values
}

/// `slug(spec.name)` → `"dw-application"`. Mirrors the Designer's
/// `resolve_manifest_id` fallback chain, minus the `last_compose` step
/// (which is form-side metadata this crate has no equivalent of).
fn resolve_manifest_id(spec: &WorkerSpec) -> String {
    let slug = slugify(&spec.name);
    if !slug.is_empty() {
        return slug;
    }
    "dw-application".into()
}

//! The canonical authoring format for an agentic worker.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    SingleTurn,
    AgentGraph,
    DeepWorker,
}

/// Full-parity worker model: everything a Designer worker carries.
///
/// Note: this type does not derive `JsonSchema`. `extension_tools` embeds
/// [`ExtensionToolBinding`], which in turn embeds
/// `greentic_extension_sdk_contract::AgenticWorkerMetadata` — a vendored type
/// that does not implement `JsonSchema` (no `schemars` dependency in that
/// crate). See `ExtensionToolBinding` for details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerSpec {
    #[serde(default)]
    pub kind: AgentKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    pub llm: LlmRef,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemorySpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<KnowledgeSpec>,
    #[serde(default)]
    pub guardrails: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_graph: Option<AgentGraphSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deep_worker: Option<DeepWorkerSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_message: Option<String>,
    #[serde(default)]
    pub extension_tools: Vec<ExtensionToolBinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LlmRef {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemorySpec {
    #[serde(default)]
    pub short_term: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_term: Option<ProviderRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRef {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeSpec {
    pub provider: String,
    pub embedding: EmbeddingRef,
    #[serde(default = "default_top_k")]
    pub top_k: u32,
    /// Local file paths to bake into the corpus (CLI); ignored when `KnowledgeInput`s are supplied directly.
    #[serde(default)]
    pub documents: Vec<String>,
}
fn default_top_k() -> u32 {
    5
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddingRef {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentGraphSpec {
    pub coordinator: Coordinator,
    pub specialists: Vec<Specialist>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Coordinator {
    #[serde(default)]
    pub instructions: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Specialist {
    pub name: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeepWorkerSpec {
    #[serde(default = "default_iteration_budget")]
    pub iteration_budget: u32,
    #[serde(default)]
    pub reflection: bool,
    #[serde(default)]
    pub delegation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_model: Option<String>,
}
fn default_iteration_budget() -> u32 {
    8
}
impl Default for DeepWorkerSpec {
    fn default() -> Self {
        Self {
            iteration_budget: 8,
            reflection: false,
            delegation: false,
            planning_model: None,
        }
    }
}

/// Knowledge document text supplied by the caller (DB for the Designer, local files for the CLI).
#[derive(Debug, Clone, PartialEq)]
pub struct KnowledgeInput {
    pub id: String,
    pub text: String,
}

/// A Designer extension tool bound onto a worker. Moved verbatim (pure data,
/// no Designer-specific behavior) from `greentic-designer/src/orchestrate/dw_form.rs:65-81`.
///
/// Does not derive `JsonSchema`: `agentic_worker_metadata`'s type,
/// `greentic_extension_sdk_contract::AgenticWorkerMetadata`, does not
/// implement `JsonSchema` (that vendored crate has no `schemars` dependency
/// at all, so the derive cannot be satisfied without forking it). Kept
/// `Eq` since both `AgenticWorkerMetadata` and `SecretRequirement` implement it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionToolBinding {
    pub extension_id: String,
    pub extension_version: String,
    pub tool_name: String,
    pub description: String,
    pub input_schema_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema_json: Option<String>,
    pub capabilities: Vec<String>,
    pub agentic_worker_metadata: greentic_extension_sdk_contract::AgenticWorkerMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_requirements: Vec<greentic_types::secrets::SecretRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_note: Option<String>,
}

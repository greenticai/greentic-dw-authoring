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

/// Whether a worker's effective system prompt is the author's `instructions`
/// verbatim (`Custom`) or composed onto the platform prompt backbone
/// (`Managed`) via [`crate::assemble::compose_managed_prompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    #[default]
    Custom,
    Managed,
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
    pub prompt_mode: PromptMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemorySpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<KnowledgeSpec>,
    #[serde(default)]
    pub guardrails: Vec<GuardrailRefSpec>,
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

/// One guardrail attached to a worker. Deserializes from EITHER a bare
/// capability-id string (`"pii-redact"`, the pre-existing shape) OR a full
/// object carrying typed `config` (`{ cap_id: "pii-redact", config: {...} }`),
/// mirroring the Designer's `GuardrailFormRef`. Untagged so both forms parse
/// without a discriminant field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GuardrailRefSpec {
    /// Bare capability id, e.g. `"pii-redact"`. Carries no config (maps to
    /// JSON `null`, matching the crate's pre-existing behavior).
    CapId(String),
    /// Full reference with typed config.
    Full {
        cap_id: String,
        #[serde(default)]
        config: serde_json::Value,
    },
}

impl GuardrailRefSpec {
    pub fn cap_id(&self) -> &str {
        match self {
            GuardrailRefSpec::CapId(id) => id,
            GuardrailRefSpec::Full { cap_id, .. } => cap_id,
        }
    }

    pub fn config(&self) -> serde_json::Value {
        match self {
            GuardrailRefSpec::CapId(_) => serde_json::Value::Null,
            GuardrailRefSpec::Full { config, .. } => config.clone(),
        }
    }
}

impl From<String> for GuardrailRefSpec {
    fn from(cap_id: String) -> Self {
        GuardrailRefSpec::CapId(cap_id)
    }
}

impl From<&str> for GuardrailRefSpec {
    fn from(cap_id: &str) -> Self {
        GuardrailRefSpec::CapId(cap_id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemorySpec {
    /// Either a bare `bool` (backward-compatible: `true` synthesizes the
    /// built-in short-term provider id, `false`/absent disables short-term
    /// memory) or a full [`ProviderRef`] naming a specific provider.
    #[serde(default)]
    pub short_term: ShortTermSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_term: Option<ProviderRef>,
}

/// `MemorySpec.short_term` accepts either a bare `bool` (legacy shape,
/// e.g. `short_term: true`) or a full [`ProviderRef`] naming a specific
/// provider + params/credential. Untagged so both forms deserialize without
/// a discriminant field; `Enabled` is tried first so a JSON/YAML boolean
/// never mismatches into `Provider` (which requires an object).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ShortTermSpec {
    Enabled(bool),
    Provider(ProviderRef),
}

impl Default for ShortTermSpec {
    fn default() -> Self {
        ShortTermSpec::Enabled(false)
    }
}

impl From<bool> for ShortTermSpec {
    fn from(enabled: bool) -> Self {
        ShortTermSpec::Enabled(enabled)
    }
}

impl From<ProviderRef> for ShortTermSpec {
    fn from(provider: ProviderRef) -> Self {
        ShortTermSpec::Provider(provider)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRef {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
    /// Provider-specific parameters, forwarded verbatim into the runtime's
    /// `MemoryProviderRef.params`. Mirrors the Designer's
    /// `ProviderBinding.params`. Defaults to empty so pre-existing YAML/JSON
    /// (which never carried this field) still parses.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub params: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeSpec {
    pub provider: String,
    /// Credential reference for the knowledge retrieval provider, forwarded
    /// into the runtime knowledge provider's `credential_ref`. `None` by
    /// default so pre-existing YAML/JSON still parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_credential_ref: Option<String>,
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
    /// Precomputed chunk embeddings for this document (Slice 2 "embed at
    /// upload"). When `Some`, the assembler writes an
    /// `assets/knowledge/<id>.vec.json` asset alongside the plain `.txt` and
    /// records its path in the corpus annotation, so the runner ingests the
    /// pre-chunked text+vectors directly instead of re-chunking + re-embedding.
    /// `None` (the default) keeps the text-only behavior — the runner
    /// re-chunks and re-embeds as before.
    pub precomputed: Option<PrecomputedVectors>,
}

/// Precomputed chunk embeddings for a single knowledge document, serialized
/// verbatim into `assets/knowledge/<id>.vec.json`. The `chunks` carry the
/// VERBATIM Slice-2 chunk boundaries (do NOT re-chunk) so the runner emits
/// them directly. Shared JSON contract with the runner reader and the designer
/// writer (no shared Rust type — the field names below ARE the contract).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrecomputedVectors {
    /// Embedding model that produced these vectors, e.g.
    /// `"text-embedding-3-small"`.
    pub embedding_model: String,
    /// Embedding dimensionality (vector length), e.g. `1536`.
    pub dims: usize,
    pub chunks: Vec<PrecomputedChunk>,
}

/// One precomputed chunk: the verbatim Slice-2 chunk text and its embedding
/// vector. Field names are part of the `.vec.json` JSON contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrecomputedChunk {
    pub chunk_index: usize,
    pub chunk_text: String,
    pub vector: Vec<f32>,
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

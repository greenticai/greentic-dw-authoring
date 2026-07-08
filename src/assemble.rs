//! DB-free worker-pack assembler.
//!
//! This is the crate's centerpiece: it reproduces the Designer's
//! `materialize_worker_pack` orchestration —
//! `to_answer_document` → build base `.gtpack` → bake knowledge →
//! `make_runner_loadable` → `embed_dw_agents` — but with no `sqlx`/DB
//! dependency: everything is read from a [`WorkerSpec`] plus a caller-supplied
//! slice of [`crate::model::KnowledgeInput`].
//!
//! Reference files (Designer, read-only inspiration, not depended on):
//! - `greentic-designer/src/orchestrate/dw_form_to_agent_config.rs` — the
//!   `DwFormState -> AgentConfig` mapping mirrored by [`agent_configs`].
//! - `greentic-designer/src/orchestrate/dw_pack.rs` — `PackMeta`/`PackBuilder`
//!   construction mirrored by [`build_pack_meta`].
//! - `greentic-designer/src/orchestrate/runner_sidecar/materialize.rs` — the
//!   build → bake-knowledge → make-loadable → embed-agents order mirrored by
//!   [`build_worker_pack`].
//! - `greentic-designer/src/orchestrate/dw_application_pack.rs` — the
//!   knowledge-corpus sidecar shape mirrored by [`build_knowledge_corpus`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use greentic_aw_runtime::config::{GuardrailRef, KnowledgeSettings};
use greentic_aw_runtime::{
    AgentConfig, AgentLimits, LlmProviderRef, MemoryProviderRef, MemorySettings, ToolRef,
};
use greentic_pack::builder::{PackBuilder, PackMeta};
use greentic_pack::PackKind;
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::model::{AgentKind, ExtensionToolBinding, KnowledgeInput, WorkerSpec};
use crate::{inject, loadable, project};

/// Capability tag marking an extension tool as callable by the agentic
/// worker runtime. Mirrors the runner's `manifest_to_tool_refs` filter and
/// the Designer's `dw_form_to_agent_config::AGENTIC_WORKER_CAPABILITY`.
const AGENTIC_WORKER_CAPABILITY: &str = "agentic_worker";

/// Built-in short-term memory provider id used when `MemorySpec.short_term`
/// is `true` but carries no provider details of its own (unlike `long_term`,
/// which is a full [`crate::model::ProviderRef`]).
const DEFAULT_SHORT_TERM_PROVIDER: &str = "greentic.memory.short_term";

/// Fallback knowledge/embedding provider ids used when a knowledge corpus is
/// baked (`knowledge` input non-empty) but `WorkerSpec.knowledge` itself is
/// absent.
const DEFAULT_KNOWLEDGE_PROVIDER: &str = "provider.knowledge.default";
const DEFAULT_EMBEDDING_PROVIDER: &str = "provider.embedding.openai";
const DEFAULT_KNOWLEDGE_TOP_K: u32 = 5;

/// Per-file / total char caps for the knowledge corpus, ported verbatim from
/// `greentic-designer/src/orchestrate/kb_attacher.rs` so a CLI-built corpus
/// behaves identically to a Designer-built one.
const KB_PER_FILE_CHAR_CAP: usize = 15_000;
const KB_TOTAL_CHAR_CAP: usize = 100_000;
const KB_MAX_FILES: usize = 20;

/// Minimal schema-valid YGTC messaging skeleton the `AgentGraph` / `DeepWorker`
/// injectors are applied to. Mirrors
/// `dw_application_pack.rs::EMPTY_FLOW_SKELETON`.
///
/// `schema_version: 1` (legacy mode) is load-bearing and MUST be spelled out
/// explicitly: `greentic-flow`'s loader defaults a *missing* `schema_version`
/// to `Some(2)` (`loader.rs`: `if flow.schema_version.is_none() {
/// flow.schema_version = Some(2); }`), which makes `is_legacy` false. In v2
/// (non-legacy) mode `greentic-flow`'s `is_builtin` only whitelists
/// `dw.*`/`mcp` op-keys — any other native op-key (including `operala.call`)
/// is silently rewritten to `component.id = "component.exec"`,
/// `operation = <pack_id>`, breaking the runner's dispatch. Spelling out
/// `schema_version: 1` here keeps `dw.agent_graph`/`operala.call` op-keys
/// intact, verbatim, exactly as the Designer's `EMPTY_FLOW_SKELETON` does.
const MINIMAL_MESSAGING_YGTC: &str = "schema_version: 1\nid: main\ntype: messaging\nnodes: {}\n";

/// Errors produced while assembling a worker `.gtpack`.
#[derive(Debug, thiserror::Error)]
pub enum AssembleError {
    #[error("project worker spec: {0}")]
    Project(#[from] project::ProjectError),
    #[error("build flow ygtc: {0}")]
    Ygtc(String),
    #[error("validate flow: {0}")]
    FlowValidate(#[source] Box<greentic_flow::error::FlowError>),
    #[error("parse pack version: {0}")]
    Version(#[source] semver::Error),
    #[error("format created_at_utc: {0}")]
    Timestamp(#[source] time::error::Format),
    #[error("build pack: {0}")]
    PackBuild(anyhow::Error),
    #[error("io: {0}")]
    Io(#[source] std::io::Error),
    #[error("inject pack sidecar: {0}")]
    Sidecar(#[from] crate::cbor_flow_post::PostProcessError),
    #[error("make pack runner-loadable: {0}")]
    Loadable(#[from] loadable::LoadableError),
    #[error("embed dw-agents.json: {0}")]
    EmbedAgents(String),
}

/// A built worker `.gtpack`, ready to be loaded by `greentic-runner`.
#[derive(Debug, Clone)]
pub struct WorkerPack {
    pub pack_path: PathBuf,
    pub pack_id: String,
}

/// Build a runner-loadable worker `.gtpack` from a [`WorkerSpec`], baking in
/// `knowledge` (if any) and writing `dw-agents.json` from [`agent_configs`].
///
/// Orchestration (DB-free mirror of the Designer's `materialize_worker_pack`):
/// 1. Project the spec into an answer document (metadata + `manifest_id`).
/// 2. Build the base `.gtpack` via [`PackBuilder`] with a real, validated
///    `flows/main.ygtc`-equivalent flow (not a stub).
/// 3. Inject that same flow verbatim at the flat `flows/main.ygtc` path —
///    the path `make_runner_loadable`'s flow-compiler scans for.
/// 4. Bake `knowledge` into `knowledge_corpus.json` + `assets/knowledge/*.txt`
///    when non-empty.
/// 5. Make the pack runner-loadable (synthesize `manifest.cbor`, inline the
///    compiled flow).
/// 6. Embed `dw-agents.json` from [`agent_configs`].
pub fn build_worker_pack(
    spec: &WorkerSpec,
    knowledge: &[KnowledgeInput],
    out_dir: &Path,
) -> Result<WorkerPack, AssembleError> {
    let answer = project::to_answer_document(spec)?;
    let manifest_id = answer
        .get("manifest_id")
        .and_then(Value::as_str)
        .unwrap_or("dw-application")
        .to_string();
    let pack_id = format!("pack.dw.{manifest_id}");

    let ygtc = build_flow_ygtc(spec, &pack_id)?;
    let flow_bundle = greentic_flow::flow_bundle::load_and_validate_bundle(&ygtc, None)
        .map_err(|e| AssembleError::FlowValidate(Box::new(e)))?;
    let flow_id = flow_bundle.id.clone();

    let meta = build_pack_meta(&pack_id, spec, flow_id.clone())?;

    std::fs::create_dir_all(out_dir).map_err(AssembleError::Io)?;
    let pack_path = out_dir.join(format!("{pack_id}.gtpack"));

    // `PackBuilder` requires at least one flow to build at all, so we still
    // pass it one here — but `.with_flow` always nests that flow's content
    // at `flows/<id>/flow.ygtc` (+ `.json`), a path `make_runner_loadable`'s
    // `populate_manifest_flows` ALSO scans (its glob is `flows/*.ygtc`, not
    // depth-limited). Left in place alongside the flat `flows/main.ygtc`
    // injection below, that would double-compile the identical flow into
    // `manifest.flows` (two entries, both id `main`). Strip the nested pair
    // immediately after build so only the flat path survives.
    PackBuilder::new(meta)
        .with_flow(flow_bundle)
        .build(&pack_path)
        .map_err(AssembleError::PackBuild)?;

    let bytes = std::fs::read(&pack_path).map_err(AssembleError::Io)?;
    let bytes = crate::cbor_flow_post::remove_entries(
        &bytes,
        &[
            &format!("flows/{flow_id}/flow.ygtc"),
            &format!("flows/{flow_id}/flow.json"),
        ],
    )?;
    let bytes = crate::cbor_flow_post::inject_sidecar(&bytes, "flows/main.ygtc", ygtc.as_bytes())?;
    std::fs::write(&pack_path, &bytes).map_err(AssembleError::Io)?;

    if !knowledge.is_empty() {
        let (corpus_bytes, assets) = build_knowledge_corpus(spec, knowledge);
        let mut bytes = std::fs::read(&pack_path).map_err(AssembleError::Io)?;
        bytes =
            crate::cbor_flow_post::inject_sidecar(&bytes, "knowledge_corpus.json", &corpus_bytes)?;
        for asset in &assets {
            bytes = crate::cbor_flow_post::inject_sidecar(&bytes, &asset.path, &asset.bytes)?;
        }
        std::fs::write(&pack_path, &bytes).map_err(AssembleError::Io)?;
    }

    loadable::make_runner_loadable(&pack_path, &pack_id)?;

    let agents = agent_configs(spec);
    inject::embed_dw_agents(&pack_path, &agents).map_err(AssembleError::EmbedAgents)?;

    Ok(WorkerPack { pack_path, pack_id })
}

/// Build the YGTC text for the spec's main flow, per `AgentKind`. Mirrors
/// grounding A.5's split: `SingleTurn` gets `loadable::single_turn_main_ygtc`;
/// `AgentGraph`/`DeepWorker` start from [`MINIMAL_MESSAGING_YGTC`] and apply
/// the matching injector, with `target`/`operation == pack_id` (mirroring
/// `dw_application_pack.rs::write_gtpack`, which passes the pack's own
/// `pack_id` to both injectors).
fn build_flow_ygtc(spec: &WorkerSpec, pack_id: &str) -> Result<String, AssembleError> {
    match spec.kind {
        AgentKind::SingleTurn => {
            loadable::single_turn_main_ygtc(&spec.name).map_err(AssembleError::Ygtc)
        }
        AgentKind::AgentGraph => {
            inject::inject_dw_agent_graph_node(MINIMAL_MESSAGING_YGTC, pack_id)
                .map_err(AssembleError::Ygtc)
        }
        AgentKind::DeepWorker => {
            let deep_worker = project::executing_node(spec)
                .and_then(|node| node.get("deep_worker").cloned())
                .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
            inject::inject_operala_call_node(MINIMAL_MESSAGING_YGTC, pack_id, &deep_worker)
                .map_err(AssembleError::Ygtc)
        }
    }
}

/// Build the [`PackMeta`] shell. Mirrors
/// `greentic-designer/src/orchestrate/dw_pack.rs:226-249`'s
/// `build_pack_meta`, adapted to read from [`WorkerSpec`] and to carry the
/// real entry-flow id (the compiled `FlowBundle`'s own `id`) instead of a
/// synthesized `<agent_id>.entry` stub name.
fn build_pack_meta(
    pack_id: &str,
    spec: &WorkerSpec,
    flow_id: String,
) -> Result<PackMeta, AssembleError> {
    let display_name = if spec.name.is_empty() {
        "Agentic Worker".to_string()
    } else {
        spec.name.clone()
    };
    let version = "0.1.0".parse().map_err(AssembleError::Version)?;
    let created_at_utc = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(AssembleError::Timestamp)?;

    Ok(PackMeta {
        pack_version: greentic_pack::builder::PACK_VERSION,
        pack_id: pack_id.to_string(),
        version,
        name: display_name,
        kind: Some(PackKind::DwApplication),
        description: spec.description.clone(),
        authors: Vec::new(),
        license: None,
        homepage: None,
        support: None,
        vendor: None,
        imports: Vec::new(),
        entry_flows: vec![flow_id],
        created_at_utc,
        events: None,
        repo: None,
        messaging: None,
        interfaces: Vec::new(),
        annotations: serde_json::Map::new(),
        distribution: None,
        components: Vec::new(),
    })
}

/// One knowledge-corpus asset ready to be written into the pack ZIP.
struct CorpusAsset {
    path: String,
    bytes: Vec<u8>,
}

/// Build the `knowledge_corpus.json` sidecar + `assets/knowledge/<id>.txt`
/// asset bytes from `knowledge`. Ports the chunking/cap logic from
/// `greentic-designer/src/orchestrate/kb_attacher.rs::build_knowledge_corpus`
/// (same per-file/total char caps, same `KnowledgeCorpusAnnotation` JSON
/// shape), adapted to read `KnowledgeInput { id, text }` directly instead of
/// DB-backed `KbFileInput` rows — the asset path is `assets/knowledge/<id>.txt`
/// (the input's own `id`, not a filename-derived slug).
fn build_knowledge_corpus(
    spec: &WorkerSpec,
    knowledge: &[KnowledgeInput],
) -> (Vec<u8>, Vec<CorpusAsset>) {
    let capped = &knowledge[..knowledge.len().min(KB_MAX_FILES)];
    let mut assets = Vec::new();
    let mut files_json = Vec::new();
    let mut running_total = 0usize;
    let mut truncated = knowledge.len() > KB_MAX_FILES;

    for item in capped {
        let text: &str = if item.text.chars().count() > KB_PER_FILE_CHAR_CAP {
            let end_byte = item
                .text
                .char_indices()
                .nth(KB_PER_FILE_CHAR_CAP)
                .map(|(byte_idx, _)| byte_idx)
                .unwrap_or(item.text.len());
            &item.text[..end_byte]
        } else {
            &item.text
        };
        let chars = text.chars().count();

        if running_total + chars > KB_TOTAL_CHAR_CAP {
            truncated = true;
            break;
        }

        let asset_path = format!("assets/knowledge/{}.txt", item.id);
        assets.push(CorpusAsset {
            path: asset_path.clone(),
            bytes: text.as_bytes().to_vec(),
        });
        files_json.push(serde_json::json!({
            "asset_path": asset_path,
            "original_name": item.id,
            "chars": chars,
        }));
        running_total += chars;
    }

    let (knowledge_provider_id, embedding_provider_id, top_k) = match &spec.knowledge {
        Some(k) => (k.provider.clone(), k.embedding.provider.clone(), k.top_k),
        None => (
            DEFAULT_KNOWLEDGE_PROVIDER.to_string(),
            DEFAULT_EMBEDDING_PROVIDER.to_string(),
            DEFAULT_KNOWLEDGE_TOP_K,
        ),
    };

    let annotation = serde_json::json!({
        "version": 1u32,
        "strategy": "embedding_retrieval",
        "knowledge_provider_id": knowledge_provider_id,
        "embedding_provider_id": embedding_provider_id,
        "top_k": top_k,
        "total_chars": running_total,
        "truncated": truncated,
        "files": files_json,
    });
    let corpus_bytes = serde_json::to_vec_pretty(&annotation).unwrap_or_default();

    (corpus_bytes, assets)
}

/// Build one [`AgentConfig`] per agent in `spec`: `SingleTurn`/`DeepWorker`
/// yield one config keyed by `spec.name`; `AgentGraph` yields the coordinator
/// (keyed `spec.name`) plus one config per specialist (keyed by the
/// specialist's own `name`). Mirrors
/// `greentic-designer/src/orchestrate/dw_form_to_agent_config.rs`'s mapping
/// (llm/tools/memory/knowledge/guardrails/limits), reading from
/// [`WorkerSpec`] instead of `DwFormState`.
pub fn agent_configs(spec: &WorkerSpec) -> BTreeMap<String, AgentConfig> {
    let mut map = BTreeMap::new();

    match (&spec.kind, &spec.agent_graph) {
        (AgentKind::AgentGraph, Some(graph)) => {
            map.insert(
                spec.name.clone(),
                build_agent_config(
                    spec,
                    &spec.name,
                    &graph.coordinator.instructions,
                    &spec.tools,
                    true,
                ),
            );
            for specialist in &graph.specialists {
                map.insert(
                    specialist.name.clone(),
                    build_agent_config(
                        spec,
                        &specialist.name,
                        &specialist.instructions,
                        &specialist.tools,
                        false,
                    ),
                );
            }
        }
        _ => {
            // SingleTurn, DeepWorker, and an AgentGraph spec that (unusually)
            // carries no `agent_graph` detail all reduce to one agent keyed
            // by the worker's own name.
            map.insert(
                spec.name.clone(),
                build_agent_config(spec, &spec.name, &spec.instructions, &spec.tools, true),
            );
        }
    }

    map
}

/// Build one [`AgentConfig`], shared by every call site in [`agent_configs`].
/// `include_extension_tools` gates whether `spec.extension_tools` (a
/// worker-level list, not split per specialist) is folded in — only the
/// coordinator / single-turn / deep-worker agent gets them; specialists only
/// see their own `Specialist.tools`.
fn build_agent_config(
    spec: &WorkerSpec,
    agent_id: &str,
    system_prompt: &str,
    tools: &[String],
    include_extension_tools: bool,
) -> AgentConfig {
    let mut tool_refs = tool_refs_from_strings(tools);
    if include_extension_tools {
        for tool_ref in tool_refs_from_extension_tools(&spec.extension_tools) {
            if !tool_refs.contains(&tool_ref) {
                tool_refs.push(tool_ref);
            }
        }
    }

    AgentConfig {
        agent_id: agent_id.to_string(),
        system_prompt: system_prompt.to_string(),
        tools: tool_refs,
        guardrails: build_guardrail_refs(spec),
        llm: LlmProviderRef {
            provider: spec.llm.provider.clone(),
            model: spec.llm.model.clone(),
            credential_ref: spec.llm.credential_ref.clone(),
        },
        limits: AgentLimits::default(),
        memory: build_memory_settings(spec),
        knowledge: build_knowledge_settings(spec),
        // CLI-authored workers default to one-shot; the conversational
        // chat-segment mode is opt-in via the designer/composer path.
        conversational: false,
    }
}

/// Parse an author-supplied JSON-schema string. Blank or invalid JSON yields
/// `None` so the runtime falls back to its own catalog contract.
fn parse_input_schema(raw: &str) -> Option<serde_json::Value> {
    if raw.trim().is_empty() {
        return None;
    }
    serde_json::from_str(raw).ok()
}

/// Map a flat `Vec<String>` of tool ids into [`ToolRef`]s. `WorkerSpec.tools`
/// (unlike `extension_tools`) carries no structured `(extension_id,
/// tool_name)` split — each id is used verbatim for both fields, which keeps
/// simple CLI-authored tool lists (e.g. `tools: [web.search]`) from being
/// silently dropped from the runtime config. Richer bindings should use
/// `extension_tools` (see [`tool_refs_from_extension_tools`]).
fn tool_refs_from_strings(tools: &[String]) -> Vec<ToolRef> {
    tools
        .iter()
        .map(|tool_id| ToolRef {
            extension_id: tool_id.clone(),
            tool_name: tool_id.clone(),
            description: None,
            input_schema: None,
        })
        .collect()
}

/// Filter `bindings` to those exposing the `"agentic_worker"` capability and
/// map each to a [`ToolRef`], preserving order and dropping later duplicates
/// of an already-seen `(extension_id, tool_name)` pair. Populates
/// `description` and `input_schema` from the binding's author contract so a
/// `flow:` tool's metadata reaches the pack's `dw-agents.json`. Mirrors
/// `dw_form_to_agent_config::collect_agentic_tool_refs`.
fn tool_refs_from_extension_tools(bindings: &[ExtensionToolBinding]) -> Vec<ToolRef> {
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut tools = Vec::new();

    for binding in bindings {
        let is_agentic = binding
            .capabilities
            .iter()
            .any(|capability| capability == AGENTIC_WORKER_CAPABILITY);
        if !is_agentic {
            continue;
        }
        let key = (binding.extension_id.clone(), binding.tool_name.clone());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let description = if binding.description.trim().is_empty() {
            None
        } else {
            Some(binding.description.clone())
        };
        tools.push(ToolRef {
            extension_id: binding.extension_id.clone(),
            tool_name: binding.tool_name.clone(),
            description,
            input_schema: parse_input_schema(&binding.input_schema_json),
        });
    }

    tools
}

/// Project `spec.memory` into runtime [`MemorySettings`]. `short_term` is
/// either a bare `bool` (no provider details of its own — a built-in
/// provider id is synthesized when `true`) or a full [`crate::model::ProviderRef`]
/// naming a specific provider, unlike `long_term` which is always a full
/// `ProviderRef`. Mirrors `dw_form_to_agent_config::build_memory_settings`'s
/// "absent when neither tier is set" behavior.
fn build_memory_settings(spec: &WorkerSpec) -> Option<MemorySettings> {
    use crate::model::ShortTermSpec;

    let mem = spec.memory.as_ref()?;

    let short_term = match &mem.short_term {
        ShortTermSpec::Enabled(false) => None,
        ShortTermSpec::Enabled(true) => Some(MemoryProviderRef {
            provider: DEFAULT_SHORT_TERM_PROVIDER.to_string(),
            capability: "cap://memory/short-term".to_string(),
            params: serde_json::Map::new(),
            credential_ref: None,
        }),
        ShortTermSpec::Provider(provider) => Some(MemoryProviderRef {
            provider: provider.provider.clone(),
            capability: "cap://memory/short-term".to_string(),
            params: provider.params.clone(),
            credential_ref: provider.credential_ref.clone(),
        }),
    };
    let long_term = mem.long_term.as_ref().map(|provider| MemoryProviderRef {
        provider: provider.provider.clone(),
        capability: "cap://memory/long-term".to_string(),
        params: provider.params.clone(),
        credential_ref: provider.credential_ref.clone(),
    });

    if short_term.is_none() && long_term.is_none() {
        None
    } else {
        Some(MemorySettings {
            short_term,
            long_term,
        })
    }
}

/// Project `spec.knowledge` into runtime [`KnowledgeSettings`]. Mirrors
/// `dw_form_to_agent_config::build_knowledge_settings`; `embedding.model` is
/// forwarded via `params["model"]` since [`MemoryProviderRef`] has no
/// dedicated model field.
fn build_knowledge_settings(spec: &WorkerSpec) -> Option<KnowledgeSettings> {
    let knowledge = spec.knowledge.as_ref()?;

    let mut embedding_params = serde_json::Map::new();
    embedding_params.insert(
        "model".to_string(),
        Value::String(knowledge.embedding.model.clone()),
    );

    Some(KnowledgeSettings {
        knowledge: Some(MemoryProviderRef {
            provider: knowledge.provider.clone(),
            capability: "cap://dw.knowledge".to_string(),
            params: serde_json::Map::new(),
            credential_ref: knowledge.provider_credential_ref.clone(),
        }),
        embedding: Some(MemoryProviderRef {
            provider: knowledge.embedding.provider.clone(),
            capability: "cap://dw.embedding".to_string(),
            params: embedding_params,
            credential_ref: knowledge.embedding.credential_ref.clone(),
        }),
        top_k: knowledge.top_k as usize,
    })
}

/// Project `spec.guardrails` into runtime [`GuardrailRef`]s. Mirrors
/// `dw_form_to_agent_config::collect_guardrail_refs`; each entry carries its
/// own `config` (JSON `null` for the bare capability-id shape, forwarded
/// verbatim for the full `{cap_id, config}` shape).
fn build_guardrail_refs(spec: &WorkerSpec) -> Vec<GuardrailRef> {
    spec.guardrails
        .iter()
        .map(|g| GuardrailRef {
            cap_id: g.cap_id().to_string(),
            offer_id: None,
            config: g.config(),
        })
        .collect()
}

#[cfg(test)]
mod mapping_tests {
    use super::*;
    use crate::model::{EmbeddingRef, KnowledgeSpec, MemorySpec, ProviderRef, ShortTermSpec};

    /// Same builder shape as `tests/assemble.rs::spec` / `tests/project.rs::base`.
    fn base_spec() -> WorkerSpec {
        WorkerSpec {
            kind: AgentKind::SingleTurn,
            name: "w".into(),
            description: None,
            tenant: None,
            llm: crate::model::LlmRef {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                credential_ref: None,
            },
            instructions: "do things".into(),
            tools: vec![],
            memory: None,
            knowledge: None,
            guardrails: vec![],
            agent_graph: None,
            deep_worker: None,
            locale: None,
            icon: None,
            vertical: None,
            opening_message: None,
            extension_tools: vec![],
        }
    }

    #[test]
    fn build_memory_settings_maps_both_tiers() {
        let mut spec = base_spec();
        spec.memory = Some(MemorySpec {
            short_term: true.into(),
            long_term: Some(ProviderRef {
                provider: "chronicle".into(),
                credential_ref: Some("vault://acme/surreal".into()),
                params: serde_json::Map::new(),
            }),
        });

        let settings = build_memory_settings(&spec).expect("memory settings present");
        let short = settings.short_term.expect("short_term present");
        assert_eq!(short.provider, DEFAULT_SHORT_TERM_PROVIDER);
        assert_eq!(short.capability, "cap://memory/short-term");
        assert_eq!(short.credential_ref, None);

        let long = settings.long_term.expect("long_term present");
        assert_eq!(long.provider, "chronicle");
        assert_eq!(long.capability, "cap://memory/long-term");
        assert_eq!(long.credential_ref.as_deref(), Some("vault://acme/surreal"));
    }

    #[test]
    fn build_memory_settings_absent_when_neither_tier_set() {
        let spec = base_spec();
        assert!(build_memory_settings(&spec).is_none());
    }

    #[test]
    fn build_knowledge_settings_maps_provider_and_embedding() {
        let mut spec = base_spec();
        spec.knowledge = Some(KnowledgeSpec {
            provider: "acme.knowledge".into(),
            provider_credential_ref: Some("vault://acme/knowledge".into()),
            embedding: EmbeddingRef {
                provider: "acme.embedding".into(),
                model: "text-embedding-3-small".into(),
                credential_ref: Some("vault://acme/embed".into()),
            },
            top_k: 7,
            documents: vec![],
        });

        let settings = build_knowledge_settings(&spec).expect("knowledge settings present");
        let knowledge = settings.knowledge.expect("knowledge provider present");
        assert_eq!(knowledge.provider, "acme.knowledge");
        assert_eq!(knowledge.capability, "cap://dw.knowledge");
        assert_eq!(
            knowledge.credential_ref.as_deref(),
            Some("vault://acme/knowledge")
        );

        let embedding = settings.embedding.expect("embedding provider present");
        assert_eq!(embedding.provider, "acme.embedding");
        assert_eq!(embedding.capability, "cap://dw.embedding");
        assert_eq!(
            embedding.credential_ref.as_deref(),
            Some("vault://acme/embed")
        );
        assert_eq!(
            embedding.params.get("model").and_then(Value::as_str),
            Some("text-embedding-3-small")
        );

        assert_eq!(settings.top_k, 7);
    }

    #[test]
    fn build_knowledge_settings_absent_when_not_set() {
        let spec = base_spec();
        assert!(build_knowledge_settings(&spec).is_none());
    }

    #[test]
    fn build_guardrail_refs_maps_flat_cap_ids() {
        let mut spec = base_spec();
        spec.guardrails = vec![
            "greentic.cap.guardrail.pii".into(),
            "greentic.cap.guardrail.profanity".into(),
        ];

        let refs = build_guardrail_refs(&spec);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].cap_id, "greentic.cap.guardrail.pii");
        assert_eq!(refs[0].offer_id, None);
        assert_eq!(refs[0].config, Value::Null);
        assert_eq!(refs[1].cap_id, "greentic.cap.guardrail.profanity");
    }

    /// Backward-compat + enrichment: a guardrail carrying the full
    /// `{cap_id, config}` shape forwards its `config` verbatim into the
    /// runtime `GuardrailRef`, matching the Designer's
    /// `collect_guardrail_refs`; a plain string still maps to `config: null`.
    #[test]
    fn build_guardrail_refs_forwards_config_for_full_shape() {
        let mut spec = base_spec();
        spec.guardrails = vec![
            crate::model::GuardrailRefSpec::Full {
                cap_id: "greentic.cap.guardrail.pii".into(),
                config: serde_json::json!({ "blocklist": ["ssn"] }),
            },
            "greentic.cap.guardrail.profanity".into(),
        ];

        let refs = build_guardrail_refs(&spec);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].cap_id, "greentic.cap.guardrail.pii");
        assert_eq!(refs[0].config["blocklist"][0], "ssn");
        assert_eq!(refs[1].cap_id, "greentic.cap.guardrail.profanity");
        assert_eq!(refs[1].config, Value::Null);
    }

    /// Enrichment: `short_term` naming a specific provider (not the bare
    /// `true`/`false` legacy shape) forwards its own provider id + params +
    /// credential into the runtime short-term `MemoryProviderRef`.
    #[test]
    fn build_memory_settings_maps_named_short_term_provider() {
        let mut spec = base_spec();
        let mut params = serde_json::Map::new();
        params.insert("ttl_seconds".into(), serde_json::json!(300));
        spec.memory = Some(MemorySpec {
            short_term: ShortTermSpec::Provider(ProviderRef {
                provider: "redis".into(),
                credential_ref: Some("vault://acme/redis".into()),
                params,
            }),
            long_term: None,
        });

        let settings = build_memory_settings(&spec).expect("memory settings present");
        let short = settings.short_term.expect("short_term present");
        assert_eq!(short.provider, "redis");
        assert_eq!(short.capability, "cap://memory/short-term");
        assert_eq!(short.credential_ref.as_deref(), Some("vault://acme/redis"));
        assert_eq!(
            short.params.get("ttl_seconds").and_then(Value::as_i64),
            Some(300)
        );
    }

    /// Backward-compat: `short_term: false` (the legacy bare-bool shape)
    /// still disables short-term memory entirely.
    #[test]
    fn build_memory_settings_short_term_false_disables_it() {
        let mut spec = base_spec();
        spec.memory = Some(MemorySpec {
            short_term: false.into(),
            long_term: None,
        });

        assert!(build_memory_settings(&spec).is_none());
    }

    #[test]
    fn extension_tool_refs_carry_author_contract() {
        let bindings = vec![ExtensionToolBinding {
            extension_id: "flow:refund".to_string(),
            tool_name: "refund_lookup".to_string(),
            description: "Look up a refund by order id".to_string(),
            input_schema_json: r#"{"type":"object","properties":{"order_id":{"type":"string"}}}"#
                .to_string(),
            capabilities: vec![AGENTIC_WORKER_CAPABILITY.to_string()],
            ..Default::default()
        }];
        let refs = tool_refs_from_extension_tools(&bindings);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].extension_id, "flow:refund");
        assert_eq!(
            refs[0].description.as_deref(),
            Some("Look up a refund by order id")
        );
        let schema = refs[0]
            .input_schema
            .as_ref()
            .expect("input_schema populated");
        assert_eq!(schema["properties"]["order_id"]["type"], "string");
    }

    #[test]
    fn empty_description_and_invalid_schema_map_to_none() {
        let bindings = vec![ExtensionToolBinding {
            extension_id: "flow:x".to_string(),
            tool_name: "x".to_string(),
            description: "   ".to_string(),
            input_schema_json: "not json".to_string(),
            capabilities: vec![AGENTIC_WORKER_CAPABILITY.to_string()],
            ..Default::default()
        }];
        let refs = tool_refs_from_extension_tools(&bindings);
        assert_eq!(refs.len(), 1);
        assert!(refs[0].description.is_none(), "blank description -> None");
        assert!(refs[0].input_schema.is_none(), "invalid schema -> None");
    }
}

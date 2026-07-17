use greentic_dw_authoring::{assemble, AgentKind, KnowledgeInput, LlmRef, WorkerSpec};
use std::io::Read;

/// Same builder shape as `tests/project.rs::base`.
fn spec(kind: AgentKind) -> WorkerSpec {
    WorkerSpec {
        kind,
        name: "w".into(),
        description: None,
        tenant: None,
        llm: LlmRef {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            credential_ref: None,
        },
        instructions: "do things".into(),
        prompt_mode: Default::default(),
        tone: None,
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

fn read_zip_entry(pack: &std::path::Path, name: &str) -> Option<Vec<u8>> {
    let f = std::fs::File::open(pack).unwrap();
    let mut zip = zip::ZipArchive::new(f).unwrap();
    let mut file = zip.by_name(name).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).unwrap();
    Some(buf)
}

/// Decode `manifest.cbor`, assert exactly one flow entry (the Important-1
/// dedup regression guard), and return the single flow's single node's
/// `(component.id, operation)` for dispatch-identity assertions.
fn decode_single_node(pack: &std::path::Path) -> (String, Option<String>) {
    let cbor = read_zip_entry(pack, "manifest.cbor").expect("manifest.cbor present");
    let manifest = greentic_types::decode_pack_manifest(&cbor).expect("decodes");
    assert_eq!(
        manifest.flows.len(),
        1,
        "manifest.flows must carry exactly one entry (PackBuilder's nested \
         flows/<id>/flow.ygtc must not survive alongside the flat flows/main.ygtc)"
    );
    let flow = &manifest.flows[0].flow;
    assert_eq!(flow.nodes.len(), 1, "expected exactly one node in the flow");
    let node = flow.nodes.values().next().expect("one node");
    (
        node.component.id.as_str().to_string(),
        node.component.operation.clone(),
    )
}

#[test]
fn single_turn_pack_is_runner_loadable() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let out = assemble::build_worker_pack(&spec(AgentKind::SingleTurn), &[], dir).unwrap();

    assert!(read_zip_entry(&out.pack_path, "dw-agents.json").is_some());
    assert!(read_zip_entry(&out.pack_path, "flows/main.ygtc").is_some());

    // CRITICAL + IMPORTANT-1 regression: exactly one flow entry, dispatched
    // as `dw.agent` (not rewritten to `component.exec`).
    let (component_id, operation) = decode_single_node(&out.pack_path);
    assert_eq!(component_id, "dw.agent");
    assert_eq!(operation.as_deref(), Some("w"));
}

#[test]
fn single_turn_pack_with_knowledge_bakes_corpus() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let knowledge = [KnowledgeInput {
        id: "policy".into(),
        text: "our refund policy is 30 days".into(),
        precomputed: None,
    }];

    let out = assemble::build_worker_pack(&spec(AgentKind::SingleTurn), &knowledge, dir).unwrap();

    assert!(read_zip_entry(&out.pack_path, "knowledge_corpus.json").is_some());
    assert!(read_zip_entry(&out.pack_path, "assets/knowledge/policy.txt").is_some());

    // Backward compat: a plain (no-precomputed) input emits NO vec asset and
    // the corpus file entry carries no `vectors_asset_path`.
    assert!(read_zip_entry(&out.pack_path, "assets/knowledge/policy.vec.json").is_none());
    let corpus_bytes = read_zip_entry(&out.pack_path, "knowledge_corpus.json").unwrap();
    let corpus: serde_json::Value = serde_json::from_slice(&corpus_bytes).unwrap();
    let file = &corpus["files"][0];
    assert_eq!(file["original_name"], "policy");
    assert!(
        file.get("vectors_asset_path").is_none(),
        "plain input must not carry vectors_asset_path"
    );
}

/// Slice 4 (writer): a `KnowledgeInput` carrying `precomputed` chunk
/// embeddings bakes an `assets/knowledge/<id>.vec.json` asset (matching the
/// `{embedding_model, dims, chunks:[{chunk_index, chunk_text, vector}]}` JSON
/// contract the runner reader consumes) AND records `vectors_asset_path` on
/// that document's `knowledge_corpus.json` file entry.
#[test]
fn single_turn_pack_with_precomputed_vectors_bakes_vec_asset() {
    use greentic_dw_authoring::{PrecomputedChunk, PrecomputedVectors};

    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let knowledge = [KnowledgeInput {
        id: "policy".into(),
        text: "our refund policy is 30 days. contact support for exceptions".into(),
        precomputed: Some(PrecomputedVectors {
            embedding_model: "text-embedding-3-small".into(),
            dims: 3,
            chunks: vec![
                PrecomputedChunk {
                    chunk_index: 0,
                    chunk_text: "our refund policy is 30 days.".into(),
                    vector: vec![0.1, 0.2, 0.3],
                },
                PrecomputedChunk {
                    chunk_index: 1,
                    chunk_text: "contact support for exceptions".into(),
                    vector: vec![0.4, 0.5, 0.6],
                },
            ],
        }),
    }];

    let out = assemble::build_worker_pack(&spec(AgentKind::SingleTurn), &knowledge, dir).unwrap();

    // The plain `.txt` half of the hybrid corpus is still written.
    assert!(read_zip_entry(&out.pack_path, "assets/knowledge/policy.txt").is_some());

    // The vec asset is present and parses back to the exact contract shape.
    let vec_bytes = read_zip_entry(&out.pack_path, "assets/knowledge/policy.vec.json")
        .expect("policy.vec.json present");
    let parsed: PrecomputedVectors = serde_json::from_slice(&vec_bytes).unwrap();
    assert_eq!(parsed.embedding_model, "text-embedding-3-small");
    assert_eq!(parsed.dims, 3);
    assert_eq!(parsed.chunks.len(), 2);
    assert_eq!(parsed.chunks[0].chunk_index, 0);
    assert_eq!(parsed.chunks[0].chunk_text, "our refund policy is 30 days.");
    assert_eq!(parsed.chunks[0].vector, vec![0.1, 0.2, 0.3]);
    assert_eq!(parsed.chunks[1].chunk_index, 1);
    assert_eq!(
        parsed.chunks[1].chunk_text,
        "contact support for exceptions"
    );

    // The raw JSON uses exactly the contract field names.
    let raw: serde_json::Value = serde_json::from_slice(&vec_bytes).unwrap();
    assert!(raw["embedding_model"].is_string());
    assert!(raw["dims"].is_number());
    assert!(raw["chunks"][0]["chunk_index"].is_number());
    assert!(raw["chunks"][0]["chunk_text"].is_string());
    assert!(raw["chunks"][0]["vector"].is_array());

    // The corpus file entry records the vec asset path.
    let corpus_bytes = read_zip_entry(&out.pack_path, "knowledge_corpus.json").unwrap();
    let corpus: serde_json::Value = serde_json::from_slice(&corpus_bytes).unwrap();
    let file = &corpus["files"][0];
    assert_eq!(file["original_name"], "policy");
    assert_eq!(
        file["vectors_asset_path"], "assets/knowledge/policy.vec.json",
        "precomputed input must record its vec asset path"
    );
}

#[test]
fn agent_graph_pack_is_runner_loadable() {
    use greentic_dw_authoring::{AgentGraphSpec, Coordinator, Specialist};

    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let mut s = spec(AgentKind::AgentGraph);
    s.agent_graph = Some(AgentGraphSpec {
        coordinator: Coordinator {
            instructions: "route to a specialist".into(),
        },
        specialists: vec![Specialist {
            name: "billing".into(),
            instructions: "handle billing".into(),
            tools: vec![],
        }],
    });

    let out = assemble::build_worker_pack(&s, &[], dir).unwrap();

    assert!(read_zip_entry(&out.pack_path, "dw-agents.json").is_some());

    let agents_bytes = read_zip_entry(&out.pack_path, "dw-agents.json").unwrap();
    let agents: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&agents_bytes).unwrap();
    assert!(agents.contains_key("w"));
    assert!(agents.contains_key("billing"));

    // CRITICAL + IMPORTANT-1 regression: exactly one flow entry, dispatched
    // as `dw.agent_graph` (not rewritten to `component.exec`).
    let (component_id, operation) = decode_single_node(&out.pack_path);
    assert_eq!(component_id, "dw.agent_graph");
    assert_eq!(operation.as_deref(), Some(out.pack_id.as_str()));
}

#[test]
fn deep_worker_pack_is_runner_loadable() {
    use greentic_dw_authoring::DeepWorkerSpec;

    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let mut s = spec(AgentKind::DeepWorker);
    s.deep_worker = Some(DeepWorkerSpec {
        iteration_budget: 4,
        ..Default::default()
    });

    let out = assemble::build_worker_pack(&s, &[], dir).unwrap();

    assert!(read_zip_entry(&out.pack_path, "dw-agents.json").is_some());

    // CRITICAL regression (the review's headline bug): a DeepWorker pack
    // MUST dispatch through `operala.call`, not the runner-loadable
    // pipeline's builtin-op fallback `component.exec` (which would silently
    // drop the deep_worker payload). This is only true when
    // `MINIMAL_MESSAGING_YGTC` spells out `schema_version: 1` (legacy mode)
    // — see `src/assemble.rs`'s doc comment on that constant.
    let (component_id, operation) = decode_single_node(&out.pack_path);
    assert_eq!(component_id, "operala.call");
    assert_eq!(operation.as_deref(), Some(out.pack_id.as_str()));
}

#[test]
fn agent_configs_keys_coordinator_and_specialists() {
    use greentic_dw_authoring::{AgentGraphSpec, Coordinator, Specialist};

    let mut s = spec(AgentKind::AgentGraph);
    s.agent_graph = Some(AgentGraphSpec {
        coordinator: Coordinator {
            instructions: "route".into(),
        },
        specialists: vec![Specialist {
            name: "billing".into(),
            instructions: "handle billing".into(),
            tools: vec!["billing.lookup".into()],
        }],
    });

    let configs = assemble::agent_configs(&s);
    assert_eq!(configs.len(), 2);
    assert!(configs.contains_key("w"));
    let billing = configs.get("billing").expect("billing specialist present");
    assert_eq!(billing.system_prompt, "handle billing");
    assert_eq!(billing.tools.len(), 1);
}

/// IMPORTANT-2 e2e regression: a worker with BOTH `memory` (short + long
/// term) and `knowledge` set must carry both through `build_worker_pack` all
/// the way into the embedded `dw-agents.json`, deserializable as a real
/// runtime `AgentConfig` with the expected provider/top_k values. Extended
/// to cover the designer-parity enrichments: guardrail config,
/// memory-provider params, knowledge `credential_ref`, and a named
/// (non-bool) short-term provider all survive the same round trip.
#[test]
fn pack_with_memory_and_knowledge_embeds_expected_agent_config() {
    use greentic_dw_authoring::{
        EmbeddingRef, GuardrailRefSpec, KnowledgeSpec, MemorySpec, ProviderRef, ShortTermSpec,
    };

    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let mut s = spec(AgentKind::SingleTurn);
    let mut short_term_params = serde_json::Map::new();
    short_term_params.insert("ttl_seconds".into(), serde_json::json!(60));
    s.memory = Some(MemorySpec {
        short_term: ShortTermSpec::Provider(ProviderRef {
            provider: "redis".into(),
            credential_ref: Some("vault://acme/redis".into()),
            params: short_term_params,
        }),
        long_term: Some(ProviderRef {
            provider: "chronicle".into(),
            credential_ref: Some("vault://acme/surreal".into()),
            params: serde_json::Map::new(),
        }),
    });
    s.knowledge = Some(KnowledgeSpec {
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
    s.guardrails = vec![
        GuardrailRefSpec::Full {
            cap_id: "greentic.cap.guardrail.pii".into(),
            config: serde_json::json!({ "blocklist": ["ssn"] }),
        },
        "greentic.cap.guardrail.profanity".into(),
    ];

    let out = assemble::build_worker_pack(&s, &[], dir).unwrap();

    let agents_bytes = read_zip_entry(&out.pack_path, "dw-agents.json").expect("dw-agents.json");
    let agents: std::collections::BTreeMap<String, greentic_aw_runtime::AgentConfig> =
        serde_json::from_slice(&agents_bytes).expect("dw-agents.json deserializes as AgentConfig");
    let cfg = agents.get("w").expect("worker agent config present");

    let memory = cfg.memory.as_ref().expect("memory present");
    let short = memory.short_term.as_ref().expect("short_term present");
    assert_eq!(short.provider, "redis");
    assert_eq!(short.capability, "cap://memory/short-term");
    assert_eq!(short.credential_ref.as_deref(), Some("vault://acme/redis"));
    assert_eq!(
        short
            .params
            .get("ttl_seconds")
            .and_then(serde_json::Value::as_i64),
        Some(60)
    );
    let long = memory.long_term.as_ref().expect("long_term present");
    assert_eq!(long.provider, "chronicle");
    assert_eq!(long.capability, "cap://memory/long-term");
    assert_eq!(long.credential_ref.as_deref(), Some("vault://acme/surreal"));

    let knowledge = cfg.knowledge.as_ref().expect("knowledge present");
    let provider = knowledge
        .knowledge
        .as_ref()
        .expect("knowledge provider present");
    assert_eq!(provider.provider, "acme.knowledge");
    assert_eq!(provider.capability, "cap://dw.knowledge");
    assert_eq!(
        provider.credential_ref.as_deref(),
        Some("vault://acme/knowledge")
    );
    let embedding = knowledge
        .embedding
        .as_ref()
        .expect("embedding provider present");
    assert_eq!(embedding.provider, "acme.embedding");
    assert_eq!(embedding.capability, "cap://dw.embedding");
    assert_eq!(
        embedding
            .params
            .get("model")
            .and_then(serde_json::Value::as_str),
        Some("text-embedding-3-small")
    );
    assert_eq!(knowledge.top_k, 7);

    assert_eq!(cfg.guardrails.len(), 2);
    assert_eq!(cfg.guardrails[0].cap_id, "greentic.cap.guardrail.pii");
    assert_eq!(cfg.guardrails[0].config["blocklist"][0], "ssn");
    assert_eq!(cfg.guardrails[1].cap_id, "greentic.cap.guardrail.profanity");
    assert_eq!(cfg.guardrails[1].config, serde_json::Value::Null);
}

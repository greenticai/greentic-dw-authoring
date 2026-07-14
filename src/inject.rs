//! DW node-injector functions ported from `greentic-designer`'s
//! `orchestrate::pack_via_packc` module.
//!
//! These are pure text/byte transforms (YGTC in → YGTC out, or a
//! `.gtpack` zip rewrite) with no dependency on the designer's
//! HTTP/session/subprocess machinery — only `serde_json`,
//! `serde_yaml_bw`, and this crate's own [`crate::cbor_flow_post`].

use std::collections::BTreeMap;
use std::path::Path;

/// Embed (or replace) the `dw-agents.json` sidecar in a built `.gtpack`.
///
/// The sidecar carries the install-back `AgentConfig` map (agent_id →
/// config) so the designer can hydrate an installed worker's index entry
/// without decoding `manifest.cbor` (which it cannot, due to the
/// greentic-types 0.5.x version bridge — see `loading_manifest.rs`).
///
/// Mirrors `embed_secrets_policy`: same zip-rewrite via `inject_sidecar`,
/// other entries preserved verbatim. The agents map is the SAME one
/// secrets-policy derives from, so the two sidecars stay consistent.
///
/// When `agents` is empty, no sidecar is written — pre-sidecar packs and
/// form-less workers stay byte-identical to before.
pub fn embed_dw_agents(
    pack_path: &Path,
    agents: &BTreeMap<String, greentic_aw_runtime::AgentConfig>,
) -> Result<(), String> {
    if agents.is_empty() {
        return Ok(());
    }
    let agents_bytes =
        serde_json::to_vec(agents).map_err(|e| format!("serialize dw-agents.json: {e}"))?;
    let pack_bytes = std::fs::read(pack_path)
        .map_err(|e| format!("read {} for dw-agents embed: {e}", pack_path.display()))?;
    let rewritten =
        crate::cbor_flow_post::inject_sidecar(&pack_bytes, "dw-agents.json", &agents_bytes)
            .map_err(|e| format!("embed dw-agents.json: {e}"))?;
    std::fs::write(pack_path, &rewritten)
        .map_err(|e| format!("write {} after dw-agents embed: {e}", pack_path.display()))?;
    Ok(())
}

/// A single `dw.agent` node to inject into the YGTC, containing all
/// information needed to build the op-key mapping.
///
/// `node_id`     — the canvas node id (becomes the YGTC map key).
/// `agent_id`    — the DW id written into `operation:`.
/// `successor_id`— the outgoing edge target; `None` = terminal routing.
///
/// Not yet consumed outside this module's own tests: `assemble::build_worker_pack`
/// (Task 8) collapses an `AgentGraph` worker into a single `dw.agent_graph`
/// node via [`inject_dw_agent_graph_node`], not per-specialist `dw.agent`
/// nodes. This type is reserved for a future multi-node canvas-collapse
/// task (mirroring the Designer's `collapseAgentSubgraphs`).
#[allow(dead_code)]
#[derive(Debug)]
pub struct DwAgentInjection {
    pub node_id: String,
    pub agent_id: String,
    pub successor_id: Option<String>,
}

/// Post-process a wizard-generated `main.ygtc` text to insert
/// `dw.agent` nodes in op-key form.
///
/// For each entry in `injections`, the function inserts (or replaces)
/// the YGTC `nodes.<node_id>` key with:
/// ```yaml
/// <node_id>:
///   dw.agent: {}
///   operation: <agent_id>
///   routing:
///     - to: <successor_id>    # or `out: true` when successor is None
/// ```
///
/// The YGTC is parsed with `serde_yaml_bw` so round-trip formatting
/// stays consistent with sibling nodes. Returns the modified YAML
/// string, or an error if the YGTC cannot be parsed / re-serialised.
///
/// This is intentionally a pure function (text in → text out) so it
/// can be unit-tested without running the wizard subprocess.
///
/// Not yet consumed outside this module's own tests — see the note on
/// [`DwAgentInjection`].
#[allow(dead_code)]
pub fn inject_dw_agent_nodes(
    ygtc_text: &str,
    injections: &[DwAgentInjection],
) -> Result<String, String> {
    if injections.is_empty() {
        return Ok(ygtc_text.to_string());
    }

    // Parse the wizard-generated YGTC as a generic JSON-Value so we
    // can manipulate the `nodes` map without needing a typed struct
    // for every field the wizard might emit.
    let mut doc: serde_json::Value = serde_yaml_bw::from_str(ygtc_text)
        .map_err(|e| format!("inject_dw_agent_nodes: parse YGTC: {e}"))?;

    // Ensure `nodes` exists as an object; wizard always emits it but
    // be defensive for hand-crafted test fixtures.
    if doc.get("nodes").is_none() {
        doc["nodes"] = serde_json::Value::Object(serde_json::Map::new());
    }
    let nodes = doc["nodes"]
        .as_object_mut()
        .ok_or("inject_dw_agent_nodes: YGTC `nodes` is not a mapping")?;

    for injection in injections {
        let routing = match &injection.successor_id {
            Some(successor) => serde_json::json!([{"to": successor}]),
            None => serde_json::json!([{"out": true}]),
        };

        let node_entry = serde_json::json!({
            "dw.agent": {},
            "operation": injection.agent_id,
            "routing": routing,
        });

        // Insert or replace — if the wizard mangled the node as an AC
        // (the pre-fix behaviour), we overwrite it.
        nodes.insert(injection.node_id.clone(), node_entry);
    }

    serde_yaml_bw::to_string(&doc)
        .map_err(|e| format!("inject_dw_agent_nodes: serialise YGTC: {e}"))
}

/// Post-process a flow `main.ygtc` text to insert a single
/// `dw.agent_graph` node that executes the full agent graph shipped in
/// the pack's `agent-graph.json` sidecar.
///
/// The inserted node is keyed `agent_graph` and has the op-key form:
/// ```yaml
/// agent_graph:
///   dw.agent_graph: {}
///   operation: <pack_id>
///   routing:
///     - out: true
/// ```
///
/// The `operation` field is the node's runtime `graph_id`
/// (`NodeKind::DwAgentGraph { graph_id: operation }`).
/// The runner keys the sidecar-loaded `GraphConfig` by `pack_id`, so
/// `operation == pack_id` is what makes the embedded graph resolve and
/// execute with zero runner changes.
///
/// Like [`inject_dw_agent_nodes`] this is a pure function (text in →
/// text out) so it is unit-testable without running the wizard.
///
/// # Errors
/// Returns an error string when the YGTC cannot be parsed / re-serialised,
/// or when `pack_id` is empty (an empty `operation` would make the runner
/// resolve `graph_id == ""`, which never matches a sidecar key).
pub fn inject_dw_agent_graph_node(ygtc_text: &str, pack_id: &str) -> Result<String, String> {
    if pack_id.is_empty() {
        return Err(
            "inject_dw_agent_graph_node: pack_id is empty; operation would never \
             resolve against the sidecar graph key"
                .to_string(),
        );
    }

    let mut doc: serde_json::Value = serde_yaml_bw::from_str(ygtc_text)
        .map_err(|e| format!("inject_dw_agent_graph_node: parse YGTC: {e}"))?;

    if doc.get("nodes").is_none() {
        doc["nodes"] = serde_json::Value::Object(serde_json::Map::new());
    }
    let nodes = doc["nodes"]
        .as_object_mut()
        .ok_or("inject_dw_agent_graph_node: YGTC `nodes` is not a mapping")?;

    let node_entry = serde_json::json!({
        "dw.agent_graph": {},
        "operation": pack_id,
        // Map the inbound activity text into `user_text` (reserved `in_map` key,
        // compiles into the node input mapping without clobbering the component
        // op-key). The graph handler reads `user_text` from the node payload
        // (graph_node.rs mirrors agent_node.rs); without this it receives empty
        // text. Same fix as single_turn_main_ygtc (B4 live-verify).
        "in_map": { "user_text": "{{in.text}}" },
        "routing": [{ "out": true }],
    });
    nodes.insert("agent_graph".to_string(), node_entry);

    serde_yaml_bw::to_string(&doc)
        .map_err(|e| format!("inject_dw_agent_graph_node: serialise YGTC: {e}"))
}

/// Inject a single `operala.call` node into a YGTC flow skeleton for the
/// deep-worker lane.
///
/// The inserted node is keyed `deep_worker` and has the op-key form:
/// ```yaml
/// deep_worker:
///   operala.call:
///     await: true
///     operation: invoke
///     input:
///       deep_worker: <config>
///   operation: <target>
///   routing:
///     - out: true
/// ```
///
/// The payload lives INSIDE the `operala.call` op-key value (mirroring the
/// `mcp` node convention), not as a separate top-level sibling — a sibling
/// `input:` key would count as a SECOND non-reserved key and
/// `greentic-flow`'s loader rejects any node that doesn't have exactly one
/// (`NodeComponentShape`), a `.gtpack` that never actually compiles.
///
/// `target` is the pack_id that identifies the operala worker at runtime.
/// `deep_worker` is the serialised `DeepWorkerConfig` carried from the
/// form through the answer document.
///
/// Like [`inject_dw_agent_graph_node`] this is a pure function (text in →
/// text out) so it is unit-testable without running the wizard.
///
/// # Errors
/// Returns an error string when the YGTC cannot be parsed / re-serialised,
/// or when `target` is empty (an empty `operation` would never resolve).
pub fn inject_operala_call_node(
    ygtc_text: &str,
    target: &str,
    deep_worker: &serde_json::Value,
    llm: &serde_json::Value,
) -> Result<String, String> {
    if target.trim().is_empty() {
        return Err("operala.call target must not be empty".into());
    }

    let mut doc: serde_json::Value = serde_yaml_bw::from_str(ygtc_text)
        .map_err(|e| format!("inject_operala_call_node: parse YGTC: {e}"))?;

    if doc.get("nodes").is_none() {
        doc["nodes"] = serde_json::Value::Object(serde_json::Map::new());
    }
    let nodes = doc["nodes"]
        .as_object_mut()
        .ok_or("inject_operala_call_node: YGTC `nodes` is not a mapping")?;

    let node_entry = serde_json::json!({
        // The op-key value IS the node payload (native op-key). `remote_dispatch`
        // reads `input.user_text`, so map the inbound activity text into
        // `input.user_text` (rendered at runtime like any node input mapping)
        // alongside the deep_worker config — otherwise the worker receives empty
        // user text. Mirrors the single-turn/agent-graph user_text fix.
        "operala.call": {
            "await": true,
            "operation": "invoke",
            // `input.llm` carries the worker's own provider/model binding so the
            // runner builds the deep-worker LLM from the WORKER's config rather
            // than a hardcoded/global default (mirrors dw.agent's AgentConfig.llm).
            "input": { "deep_worker": deep_worker, "llm": llm, "user_text": "{{in.text}}" },
        },
        "operation": target,
        "routing": [{ "out": true }],
    });
    nodes.insert("deep_worker".to_string(), node_entry);

    serde_yaml_bw::to_string(&doc)
        .map_err(|e| format!("inject_operala_call_node: serialise YGTC: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Minimal YGTC text with a `start` node pointing to `agent`,
    /// matching the structure the wizard produces.
    fn minimal_ygtc_with_start_to_agent() -> &'static str {
        "id: demo\ntype: messaging\nnodes:\n  start:\n    qa.process:\n      welcome: hi\n    routing:\n      - to: agent\n"
    }

    // ── inject_dw_agent_nodes ────────────────────────────────────────

    /// inject_dw_agent_nodes: basic round-trip with a successor edge.
    #[test]
    fn inject_dw_agent_inserts_op_key_node_with_to_routing() {
        let ygtc_text = minimal_ygtc_with_start_to_agent();
        let injections = [DwAgentInjection {
            node_id: "agent".into(),
            agent_id: "greeter".into(),
            successor_id: Some("reply".into()),
        }];
        let result = inject_dw_agent_nodes(ygtc_text, &injections)
            .expect("inject_dw_agent_nodes must succeed");

        // Re-parse the result as YAML and inspect the node.
        let doc: serde_json::Value =
            serde_yaml_bw::from_str(&result).expect("result must be valid YAML");

        let agent_node = doc
            .pointer("/nodes/agent")
            .expect("nodes.agent must exist after injection");

        // The op-key `dw.agent` must be present and be an empty object.
        let dw_agent_value = agent_node
            .get("dw.agent")
            .expect("nodes.agent must have key 'dw.agent'");
        assert_eq!(
            dw_agent_value,
            &serde_json::json!({}),
            "dw.agent value must be an empty object"
        );

        // `operation` must equal the agent_id.
        assert_eq!(
            agent_node["operation"].as_str(),
            Some("greeter"),
            "operation must equal agent_id"
        );

        // `routing` must be [{to: "reply"}].
        let routing = agent_node["routing"]
            .as_array()
            .expect("routing must be an array");
        assert_eq!(
            routing.len(),
            1,
            "routing array must have exactly one entry"
        );
        assert_eq!(
            routing[0]["to"].as_str(),
            Some("reply"),
            "routing[0].to must equal the successor id"
        );
        assert!(
            routing[0].get("out").is_none(),
            "routing must use `to`, not `out`, when a successor exists"
        );
    }

    /// inject_dw_agent_nodes: terminal node (no successor → `out: true`).
    #[test]
    fn inject_dw_agent_terminal_node_uses_out_routing() {
        let ygtc_text = minimal_ygtc_with_start_to_agent();
        let injections = [DwAgentInjection {
            node_id: "agent".into(),
            agent_id: "greeter".into(),
            successor_id: None,
        }];
        let result = inject_dw_agent_nodes(ygtc_text, &injections)
            .expect("inject_dw_agent_nodes must succeed for terminal node");
        let doc: serde_json::Value = serde_yaml_bw::from_str(&result).unwrap();
        let routing = doc
            .pointer("/nodes/agent/routing")
            .and_then(serde_json::Value::as_array)
            .expect("routing must be array");
        assert_eq!(routing[0]["out"], serde_json::json!(true));
        assert!(routing[0].get("to").is_none());
    }

    /// inject_dw_agent_nodes: replaces a wizard-mangled AC node if it
    /// already occupies the same node id.
    #[test]
    fn inject_dw_agent_replaces_existing_mangled_ac_node() {
        // Simulate the pre-fix wizard output: "agent" was emitted as
        // an adaptive-card stub (the old `node_kind` fallthrough).
        let ygtc_with_mangled = concat!(
            "id: demo\ntype: messaging\nnodes:\n",
            "  agent:\n",
            "    adaptive-card:\n",
            "      default_card_asset: assets/cards/agent.json\n",
            "    routing:\n      - out: true\n",
        );
        let injections = [DwAgentInjection {
            node_id: "agent".into(),
            agent_id: "greeter".into(),
            successor_id: Some("reply".into()),
        }];
        let result = inject_dw_agent_nodes(ygtc_with_mangled, &injections).unwrap();
        let doc: serde_json::Value = serde_yaml_bw::from_str(&result).unwrap();
        let agent_node = doc.pointer("/nodes/agent").expect("nodes.agent must exist");
        assert!(
            agent_node.get("dw.agent").is_some(),
            "after injection 'dw.agent' key must be present"
        );
        assert!(
            agent_node.get("adaptive-card").is_none(),
            "mangled 'adaptive-card' key must be gone after injection"
        );
        assert_eq!(agent_node["operation"].as_str(), Some("greeter"));
    }

    /// inject_dw_agent_nodes: no-op when `injections` is empty.
    #[test]
    fn inject_dw_agent_noop_when_no_injections() {
        let ygtc_text = minimal_ygtc_with_start_to_agent();
        let result = inject_dw_agent_nodes(ygtc_text, &[])
            .expect("empty injection slice must succeed and return text unchanged");
        // The function short-circuits before parsing, so result == input.
        assert_eq!(result, ygtc_text);
    }

    // ── inject_dw_agent_graph_node ───────────────────────────────────

    /// inject_dw_agent_graph_node: yields the op-key `dw.agent_graph`
    /// node whose `operation` equals the pack_id and routing is
    /// terminal (`out: true`) — the exact shape the runner parses into
    /// `NodeKind::DwAgentGraph { graph_id: pack_id }`.
    #[test]
    fn inject_dw_agent_graph_node_binds_operation_to_pack_id() {
        let ygtc_text = minimal_ygtc_with_start_to_agent();
        let pack_id = "pack.dw.triage.deadbeef";
        let result = inject_dw_agent_graph_node(ygtc_text, pack_id)
            .expect("inject_dw_agent_graph_node must succeed");

        let doc: serde_json::Value =
            serde_yaml_bw::from_str(&result).expect("result must be valid YAML");
        let node = doc
            .pointer("/nodes/agent_graph")
            .expect("nodes.agent_graph must exist after injection");

        assert_eq!(
            node.get("dw.agent_graph"),
            Some(&serde_json::json!({})),
            "dw.agent_graph value must be an empty object"
        );
        assert_eq!(
            node["operation"].as_str(),
            Some(pack_id),
            "operation must equal the pack_id so the sidecar graph resolves"
        );
        let routing = node["routing"].as_array().expect("routing must be array");
        assert_eq!(routing.len(), 1);
        assert_eq!(routing[0]["out"], serde_json::json!(true));
        assert!(routing[0].get("to").is_none());
        // The graph handler reads `user_text` from the node payload; the node
        // must map the inbound activity text into it (reserved `in_map` key).
        assert_eq!(
            node["in_map"]["user_text"].as_str(),
            Some("{{in.text}}"),
            "agent_graph node must map inbound text into user_text"
        );
    }

    /// inject_dw_agent_graph_node: an empty pack_id is rejected — an
    /// empty `operation` would never match a sidecar graph key.
    #[test]
    fn inject_dw_agent_graph_node_rejects_empty_pack_id() {
        let ygtc_text = minimal_ygtc_with_start_to_agent();
        let err =
            inject_dw_agent_graph_node(ygtc_text, "").expect_err("empty pack_id must be rejected");
        assert!(err.contains("pack_id is empty"), "unexpected error: {err}");
    }

    // ── inject_operala_call_node ─────────────────────────────────────

    /// inject_operala_call_node: produces a `deep_worker` node with the
    /// `operala.call` op-key, the given `target` as `operation`, and the
    /// `deep_worker` config nested under `operala.call.input.deep_worker`
    /// (payload-inside-op-key, mirroring the `mcp` node convention).
    #[test]
    fn inject_operala_call_node_emits_node_with_config() {
        let cfg = json!({ "iterationBudget": 8, "reflection": true });
        let llm = json!({ "provider": "deepseek", "model": "deepseek-chat" });
        let out = inject_operala_call_node("nodes: {}\n", "worker-1", &cfg, &llm)
            .expect("inject_operala_call_node must succeed");
        let doc: serde_json::Value =
            serde_yaml_bw::from_str(&out).expect("result must be valid YAML");
        let node = doc
            .pointer("/nodes/deep_worker")
            .expect("nodes.deep_worker must exist after injection");
        assert!(
            node.get("operala.call").is_some(),
            "operala.call op-key must be present"
        );
        assert_eq!(
            node["operation"].as_str(),
            Some("worker-1"),
            "operation must equal the target"
        );
        assert_eq!(
            node["operala.call"]["input"]["deep_worker"], cfg,
            "deep_worker config must be nested under operala.call.input.deep_worker"
        );
        // remote_dispatch reads `input.user_text`; map inbound text into it
        // (rendered at runtime) alongside the deep_worker config.
        assert_eq!(
            node["operala.call"]["input"]["user_text"].as_str(),
            Some("{{in.text}}"),
            "operala node must map inbound text into input.user_text"
        );
    }

    /// The worker's LLM binding (provider + model) must be stamped into the
    /// operala.call node input so the runner can build the deep-worker's LLM
    /// from the WORKER's own config instead of a hardcoded/global default.
    #[test]
    fn inject_operala_call_node_stamps_llm_provider_and_model() {
        let cfg = json!({ "iterationBudget": 8 });
        let llm = json!({ "provider": "deepseek", "model": "deepseek-chat" });
        let out = inject_operala_call_node("nodes: {}\n", "worker-1", &cfg, &llm)
            .expect("inject_operala_call_node must succeed");
        let doc: serde_json::Value =
            serde_yaml_bw::from_str(&out).expect("result must be valid YAML");
        let node = doc
            .pointer("/nodes/deep_worker")
            .expect("nodes.deep_worker must exist after injection");
        assert_eq!(
            node["operala.call"]["input"]["llm"], llm,
            "worker LLM config must be nested under operala.call.input.llm"
        );
    }

    /// A node with a top-level `input:` sibling (the pre-fix shape) is a
    /// SECOND non-reserved key alongside the op-key and is rejected by
    /// `greentic-flow`'s loader (`NodeComponentShape`) — the exact bug this
    /// function's payload-inside-op-key fix avoids. The current output must
    /// actually compile via `greentic_flow::compile_ygtc_str`.
    #[test]
    fn inject_operala_call_node_output_compiles() {
        let cfg = json!({ "iterationBudget": 8, "reflection": true });
        let llm = json!({ "provider": "deepseek", "model": "deepseek-chat" });
        let out = inject_operala_call_node(
            "id: main\ntype: messaging\nnodes: {}\n",
            "worker-1",
            &cfg,
            &llm,
        )
        .expect("inject_operala_call_node must succeed");
        let flow =
            greentic_flow::compile_ygtc_str(&out).expect("compile injected deep_worker flow");
        assert_eq!(flow.nodes.len(), 1);
    }

    /// inject_operala_call_node: an empty target is rejected.
    #[test]
    fn inject_operala_call_node_rejects_empty_target() {
        let cfg = json!({});
        let llm = json!({ "provider": "deepseek", "model": "deepseek-chat" });
        let err = inject_operala_call_node("nodes: {}\n", "", &cfg, &llm)
            .expect_err("empty target must be rejected");
        assert!(
            err.contains("target must not be empty"),
            "unexpected error: {err}"
        );
    }

    // ── smoke test (task 6 brief) ─────────────────────────────────────

    /// Task 6 brief smoke test: a minimal messaging YGTC with an empty
    /// `nodes` map accepts an `operala.call` injection and the result
    /// contains the op-key.
    #[test]
    fn smoke_inject_operala_call_node_minimal_messaging_ygtc() {
        let cfg = json!({"iterationBudget": 8});
        let llm = json!({ "provider": "deepseek", "model": "deepseek-chat" });
        let out = inject_operala_call_node(
            "id: main\ntype: messaging\nstart: x\nnodes: {}",
            "tgt",
            &cfg,
            &llm,
        )
        .expect("inject_operala_call_node must succeed on minimal messaging YGTC");
        assert!(
            out.contains("operala.call"),
            "result must contain the operala.call op-key: {out}"
        );
    }

    // ── embed_dw_agents ───────────────────────────────────────────────

    fn sample_agent_config(id: &str) -> greentic_aw_runtime::AgentConfig {
        use greentic_aw_runtime::{AgentConfig, AgentLimits, LlmProviderRef};
        AgentConfig {
            guardrails: vec![],
            agent_id: id.to_string(),
            system_prompt: format!("You are {id}."),
            tools: vec![],
            llm: LlmProviderRef {
                provider: "openai".into(),
                model: "gpt-4o-mini".into(),
                credential_ref: None,
            },
            limits: AgentLimits::default(),
            memory: None,
            knowledge: None,
            conversational: false,
            opening_message: None,
        }
    }

    fn write_manifest_only_pack(path: &std::path::Path) {
        use std::io::Write;
        let f = std::fs::File::create(path).expect("create");
        let mut zw = zip::ZipWriter::new(f);
        let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        zw.start_file("manifest.cbor", opts).expect("start");
        zw.write_all(b"x").expect("write");
        zw.finish().expect("finish");
    }

    #[test]
    fn embed_dw_agents_round_trips_the_map() {
        use std::io::Read;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("p.gtpack");
        write_manifest_only_pack(&path);

        let mut agents = BTreeMap::new();
        agents.insert("greeter".to_string(), sample_agent_config("greeter"));

        embed_dw_agents(&path, &agents).expect("embed must succeed");

        let bytes = std::fs::read(&path).expect("read");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("open");
        let mut entry = zip.by_name("dw-agents.json").expect("entry present");
        let mut buf = String::new();
        entry.read_to_string(&mut buf).expect("read entry");
        let back: BTreeMap<String, greentic_aw_runtime::AgentConfig> =
            serde_json::from_str(&buf).expect("deserialize");
        assert_eq!(back.len(), 1);
        let cfg = back.get("greeter").expect("greeter present");
        assert_eq!(cfg.agent_id, "greeter");
        assert_eq!(cfg.system_prompt, "You are greeter.");

        drop(entry);
        assert!(
            zip.by_name("manifest.cbor").is_ok(),
            "existing entries preserved"
        );
    }

    #[test]
    fn embed_dw_agents_skips_empty_map() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("p.gtpack");
        write_manifest_only_pack(&path);

        let agents: BTreeMap<String, greentic_aw_runtime::AgentConfig> = BTreeMap::new();
        embed_dw_agents(&path, &agents).expect("empty map must be a no-op");

        let bytes = std::fs::read(&path).expect("read");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("open");
        assert!(
            zip.by_name("dw-agents.json").is_err(),
            "no sidecar must be written for an empty map"
        );
    }
}

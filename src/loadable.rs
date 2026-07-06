//! Turn the sidecar-built `.gtpack` into a runner-loadable pack.
//!
//! DECISION (from Task 1 spike): Recipe A — synthesize a `greentic_types`
//! `manifest.cbor` with EMPTY flows and let `populate_manifest_flows` compile
//! every `flows/*.ygtc` into an inline `PackFlowEntry` (the shape the runner
//! reads at `PackFlows::from_manifest` → `manifest.flows[].flow`).
//!
//! PRECONDITION: the pack must already contain `flows/main.ygtc`. write_gtpack
//! emits it for AgentGraph/DeepWorker, but NOT for SingleTurn (executing_node
//! is None) — for SingleTurn the caller (materialize, Task 4) injects it first
//! via `inject_sidecar("flows/main.ygtc", single_turn_main_ygtc(agent_id))`.
//! If no `flows/*.ygtc` is present, populate_manifest_flows leaves flows empty
//! and the pack won't run — that's a caller bug, surfaced by Task 5 tests.

use std::io::Read;
use std::path::Path;

use greentic_types::{decode_pack_manifest, encode_pack_manifest};
// PackManifest/PackId/PackKind referenced fully-qualified in build_runner_manifest.

use crate::cbor_flow_post::{inject_sidecar, populate_manifest_flows, PostProcessError};

#[derive(Debug, thiserror::Error)]
pub enum LoadableError {
    #[error("read pack {0}")]
    Read(#[source] std::io::Error),
    #[error("write pack {0}")]
    Write(#[source] std::io::Error),
    #[error("invalid pack id `{0}`: {1}")]
    PackId(String, String),
    #[error("encode manifest.cbor: {0}")]
    Encode(String),
    #[error(transparent)]
    Post(#[from] PostProcessError),
}

/// Turn a sidecar-built `.gtpack` on disk into a runner-loadable pack:
/// ensure it carries a decodable `greentic_types` `manifest.cbor`, then
/// inline-compile `flows/*.ygtc` into it. Idempotent — re-running on an
/// already-loadable pack is a no-op decode + no-op flow fill.
pub fn make_runner_loadable(pack_path: &Path, pack_id: &str) -> Result<(), LoadableError> {
    let bytes = std::fs::read(pack_path).map_err(LoadableError::Read)?;

    // Only synthesize manifest.cbor when absent; if a producer already wrote a
    // decodable greentic_types manifest, keep it (populate_manifest_flows will
    // fill empty flows or leave a populated one untouched).
    let bytes = if read_entry(&bytes, "manifest.cbor")
        .and_then(|b| decode_pack_manifest(&b).ok())
        .is_none()
    {
        let manifest = build_runner_manifest(pack_id)?;
        let cbor =
            encode_pack_manifest(&manifest).map_err(|e| LoadableError::Encode(format!("{e}")))?;
        inject_sidecar(&bytes, "manifest.cbor", &cbor)?
    } else {
        bytes
    };

    // Compile flows/main.ygtc → inline PackFlowEntry in manifest.cbor.
    let bytes = populate_manifest_flows(&bytes)?;
    std::fs::write(pack_path, &bytes).map_err(LoadableError::Write)?;
    Ok(())
}

/// Build a minimal greentic_types::PackManifest with EMPTY flows (verbatim from
/// the Task-1 spike; all 15 fields are mandatory in a Rust struct literal even
/// though several carry serde(default) for *deserialization*). Keep flows empty
/// so populate_manifest_flows fills it from flows/main.ygtc.
/// Note: PackId::new (not from_str) is the constructor. pack_id shape
/// `pack.dw.<manifest_id>.<uuid8>` (from DwPackResult::pack_id) is a valid PackId.
fn build_runner_manifest(pack_id: &str) -> Result<greentic_types::PackManifest, LoadableError> {
    Ok(greentic_types::PackManifest {
        schema_version: "1".to_string(),
        pack_id: greentic_types::PackId::new(pack_id)
            .map_err(|e| LoadableError::PackId(pack_id.into(), format!("{e}")))?,
        name: None,
        version: "0.1.0"
            .parse()
            .map_err(|e| LoadableError::Encode(format!("version: {e}")))?,
        kind: greentic_types::PackKind::Application,
        publisher: "greentic-designer".to_string(),
        components: vec![],
        flows: vec![],
        dependencies: vec![],
        capabilities: vec![],
        secret_requirements: vec![],
        signatures: Default::default(),
        bootstrap: None,
        extensions: None,
        agents: Default::default(),
    })
}

/// YGTC for a single-turn worker: one `dw.agent` node keyed `agent`, operation =
/// agent_id, plus an `in_map` that maps the inbound activity text into the
/// node's `user_text` input. Component id is the map KEY; routing is an array of
/// route objects.
///
/// The input mapping uses the RESERVED `in_map` key (greentic-flow special-cases
/// it into the node's input mapping) — NOT a bare `input:`, which is unreserved
/// and would clobber the `dw.agent` component key (Task-1 spike). The mapping is
/// load-bearing: the runner's `agent_node` reads `user_text` from the node input
/// (`agent_node.rs`), so without it the agent receives empty text and the LLM
/// greets instead of answering (found in B4 live-verify).
///
/// `agent_id` is embedded via structured serialization (`serde_json::json!` +
/// `serde_yaml_bw::to_string`), mirroring `pack_via_packc::inject_dw_agent_nodes`
/// — never via raw string interpolation. This keeps the parsed `operation`
/// scalar byte-identical to `agent_id` even when it contains a colon, quote, or
/// newline (a `display_name` is user-controlled), which matters because
/// `operation` MUST equal the `dw-agents.json` key (`cfg.agent_id`) and the
/// runner's lookup key.
pub(crate) fn single_turn_main_ygtc(agent_id: &str) -> Result<String, String> {
    let doc = serde_json::json!({
        "id": "main",
        "type": "messaging",
        "start": "agent",
        "nodes": {
            "agent": {
                "dw.agent": {},
                "operation": agent_id,
                "in_map": { "user_text": "{{in.text}}" },
                "routing": [{"out": true}],
            }
        }
    });
    serde_yaml_bw::to_string(&doc)
        .map_err(|e| format!("single_turn_main_ygtc: serialise YGTC: {e}"))
}

fn read_entry(pack: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(pack)).ok()?;
    let mut f = z.by_name(name).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// Build an in-memory zip from `(entry_name, entry_bytes)` pairs.
    fn minimal_zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
            let options = SimpleFileOptions::default();
            for (name, bytes) in entries {
                writer.start_file(*name, options).expect("start_file");
                writer.write_all(bytes).expect("write entry");
            }
            writer.finish().expect("finish zip");
        }
        out
    }

    #[test]
    fn single_turn_main_ygtc_compiles_to_one_dw_agent_node() {
        let ygtc = single_turn_main_ygtc("greeter").expect("serialise");
        assert!(ygtc.contains("dw.agent"));
        assert!(ygtc.contains("greeter"));
        // The real proof: greentic-flow compiles it into a Flow with one node.
        let flow = greentic_flow::compile_ygtc_str(&ygtc).expect("compile");
        assert_eq!(flow.id.as_str(), "main");
        assert_eq!(flow.nodes.len(), 1);
        // And the node's component is dw.agent (guards against the `input:`-clobber bug).
        let node = flow.nodes.values().next().unwrap();
        assert_eq!(node.component.id.as_str(), "dw.agent");
        assert_eq!(node.component.operation.as_deref(), Some("greeter"));
        // Regression guard (live-verify finding): the node MUST map the inbound
        // activity text into `user_text`, else `agent_node` reads an empty
        // `user_text` and the LLM greets instead of answering the user. The
        // `in_map` key is reserved in greentic-flow (so it does NOT clobber the
        // `dw.agent` component key, unlike a bare `input:`), and compiles into
        // the node's input mapping.
        assert_eq!(
            node.input.mapping.get("user_text").and_then(|v| v.as_str()),
            Some("{{in.text}}"),
            "single-turn node must map inbound text into user_text"
        );
    }

    /// Task-4 review hardening: a `display_name` with a colon (or other
    /// YAML-significant character) must round-trip byte-identical through
    /// the compiled node's `operation` field — not just "some node exists".
    /// This is the exact value the runner uses to look up the agent in
    /// `dw-agents.json`, so it must equal the raw input string exactly.
    #[test]
    fn single_turn_main_ygtc_roundtrips_tricky_agent_id() {
        let agent_id = "Support: Tier 1";
        let ygtc = single_turn_main_ygtc(agent_id).expect("serialise");
        let flow = greentic_flow::compile_ygtc_str(&ygtc).expect("compile");
        assert_eq!(flow.nodes.len(), 1);
        let node = flow.nodes.values().next().unwrap();
        assert_eq!(node.component.id.as_str(), "dw.agent");
        assert_eq!(node.component.operation.as_deref(), Some(agent_id));
    }

    #[test]
    fn make_runner_loadable_inlines_dw_agent_flow() {
        // Build a tiny in-memory .gtpack: flows/main.ygtc (dw.agent) + a placeholder file.
        // Corrected YGTC (Task-1 spike): `routing: end` is invalid (compile_routing
        // only accepts "out"/"reply" shorthand) and an `input:` key silently clobbers
        // the `dw.agent` component key. Use the canonical inject_dw_agent_nodes shape.
        let ygtc = "id: main\ntype: messaging\nstart: agent\nnodes:\n  agent:\n    dw.agent: {}\n    operation: greeter\n    routing:\n      - out: true\n";
        let pack = crate::cbor_flow_post::inject_sidecar(
            &minimal_zip_with(&[("placeholder.txt", b"x")]),
            "flows/main.ygtc",
            ygtc.as_bytes(),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.gtpack");
        std::fs::write(&path, &pack).unwrap();

        make_runner_loadable(&path, "greentic.dw.greeter").expect("loadable");

        let bytes = std::fs::read(&path).unwrap();
        let cbor = read_entry(&bytes, "manifest.cbor").expect("manifest.cbor present");
        let m = greentic_types::decode_pack_manifest(&cbor).expect("decode");
        assert_eq!(m.flows.len(), 1);
        assert_eq!(m.flows[0].flow.id.as_str(), "main");
        assert_eq!(m.flows[0].flow.nodes.len(), 1);
    }
}

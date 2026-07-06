//! Patch a freshly-rendered `.gtpack` so its `manifest.cbor` carries
//! the canonical Flow that the runtime actually reads.
//!
//! Why this exists: `bundle-standard` (the WASM extension that
//! converts a designer session into a `.gtpack`) emits
//! `flows/main.ygtc` text but leaves the CBOR `flows[]` array empty —
//! it can't pull in `greentic-flow` (heavy `tokio` / `wasmtime` deps,
//! incompatible with the wasm32-wasip2 target). The runtime
//! (`greentic-runner` / `gtc start`) only inspects the CBOR side, so
//! freshly-rendered card-only packs ran with "no flow available" and
//! autoStart never fired the welcome card. Fixing this in
//! bundle-standard would force a wasm-friendly YAML→Flow compiler;
//! doing it in-host here is a one-screen patch that runs after the
//! WASM extension returns and before we hand the bytes to the user.
//!
//! Flow:
//!   1. Open the rendered `.gtpack` (zip) in memory.
//!   2. Read every `flows/<name>.ygtc` entry.
//!   3. Compile each via `greentic_flow::compile_ygtc_str` into the
//!      canonical `greentic_types::Flow`.
//!   4. Decode the existing `manifest.cbor` → `PackManifest`.
//!   5. Replace `manifest.flows` with one `PackFlowEntry` per
//!      compiled flow.
//!   6. Re-encode and rewrite the zip with the new manifest.cbor
//!      entry, preserving every other file verbatim.

use std::io::{Cursor, Read, Write};

use greentic_types::{
    decode_pack_manifest, encode_pack_manifest, Flow, FlowKind, PackFlowEntry, PackManifest,
};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

// `DecodeManifest` / `EncodeManifest` / `CompileFlow` / `MissingManifest`
// are only constructed by `populate_manifest_flows`, which is ported
// ahead of the task that wires it into the pack build pipeline.
#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum PostProcessError {
    #[error("open input zip: {0}")]
    OpenZip(#[from] zip::result::ZipError),
    #[error("read entry {entry}: {source}")]
    ReadEntry {
        entry: String,
        #[source]
        source: std::io::Error,
    },
    #[error("write entry {entry}: {source}")]
    WriteEntry {
        entry: String,
        #[source]
        source: std::io::Error,
    },
    #[error("decode manifest.cbor: {0}")]
    DecodeManifest(String),
    #[error("encode manifest.cbor: {0}")]
    EncodeManifest(String),
    #[error("compile flows/{name}.ygtc: {error}")]
    CompileFlow { name: String, error: String },
    #[error("manifest.cbor missing from pack")]
    MissingManifest,
}

/// Read the rendered pack bytes, compile every `flows/*.ygtc` into
/// canonical [`Flow`]s, and rewrite `manifest.cbor` so the CBOR `flows`
/// array reflects the YGTC content.
///
/// The caller passes ownership of the rendered bytes; on success we
/// return a new `Vec<u8>` that fully replaces the original pack.
///
/// Ported ahead of the task that wires this into the pack build
/// pipeline; not yet consumed.
#[allow(dead_code)]
pub fn populate_manifest_flows(pack_bytes: &[u8]) -> Result<Vec<u8>, PostProcessError> {
    let mut archive = ZipArchive::new(Cursor::new(pack_bytes))?;

    // Phase 1 — pull every flow YGTC out of the archive while it's
    // still readable.
    let mut ygtc_entries: Vec<(String, String)> = Vec::new();
    let names: Vec<String> = archive.file_names().map(|s| s.to_owned()).collect();
    let mut manifest_bytes: Option<Vec<u8>> = None;
    for name in &names {
        if name.starts_with("flows/") && name.ends_with(".ygtc") {
            let mut entry = archive.by_name(name)?;
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .map_err(|source| PostProcessError::ReadEntry {
                    entry: name.clone(),
                    source,
                })?;
            // strip "flows/" prefix and ".ygtc" suffix to recover the
            // logical flow name (matches the FlowEntry shape used by
            // bundle-standard-core).
            let logical = name
                .trim_start_matches("flows/")
                .trim_end_matches(".ygtc")
                .to_owned();
            ygtc_entries.push((logical, buf));
        } else if name == "manifest.cbor" {
            let mut entry = archive.by_name(name)?;
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|source| PostProcessError::ReadEntry {
                    entry: name.clone(),
                    source,
                })?;
            manifest_bytes = Some(buf);
        }
    }

    let manifest_bytes = manifest_bytes.ok_or(PostProcessError::MissingManifest)?;
    if ygtc_entries.is_empty() {
        // No YGTC to embed (provider pack, library pack, …) — leave
        // the original bytes alone.
        return Ok(pack_bytes.to_vec());
    }

    let mut manifest: PackManifest = decode_pack_manifest(&manifest_bytes)
        .map_err(|e| PostProcessError::DecodeManifest(format!("{e}")))?;

    // Issue #102: when the producer (e.g. `greentic-pack wizard apply`)
    // already populated `manifest.flows[]` with the canonical compiled
    // Flow, recomputing it here would clobber producer-set fields like
    // `tags` and `entrypoints`. The bundle-standard WASM extension is
    // the only producer that ships an empty array; treat a populated
    // manifest as authoritative and short-circuit.
    if !manifest.flows.is_empty() {
        return Ok(pack_bytes.to_vec());
    }

    let mut flow_entries: Vec<PackFlowEntry> = Vec::with_capacity(ygtc_entries.len());
    for (name, yaml) in &ygtc_entries {
        let flow: Flow =
            greentic_flow::compile_ygtc_str(yaml).map_err(|e| PostProcessError::CompileFlow {
                name: name.clone(),
                error: format!("{e}"),
            })?;
        flow_entries.push(PackFlowEntry {
            id: flow.id.clone(),
            kind: flow_kind_to_canonical(&flow),
            flow,
            tags: Vec::new(),
            entrypoints: Vec::new(),
        });
    }
    manifest.flows = flow_entries;

    let new_manifest_bytes = encode_pack_manifest(&manifest)
        .map_err(|e| PostProcessError::EncodeManifest(format!("{e}")))?;

    // Phase 2 — rewrite the zip, preserving every entry verbatim
    // except `manifest.cbor`.
    let mut out = Vec::with_capacity(pack_bytes.len() + new_manifest_bytes.len());
    {
        let mut writer = ZipWriter::new(Cursor::new(&mut out));
        for name in &names {
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            if name == "manifest.cbor" {
                writer
                    .start_file(name, options)
                    .map_err(PostProcessError::OpenZip)?;
                writer.write_all(&new_manifest_bytes).map_err(|source| {
                    PostProcessError::WriteEntry {
                        entry: name.clone(),
                        source,
                    }
                })?;
            } else {
                let mut entry = archive.by_name(name)?;
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .map_err(|source| PostProcessError::ReadEntry {
                        entry: name.clone(),
                        source,
                    })?;
                writer
                    .start_file(name, options)
                    .map_err(PostProcessError::OpenZip)?;
                writer
                    .write_all(&buf)
                    .map_err(|source| PostProcessError::WriteEntry {
                        entry: name.clone(),
                        source,
                    })?;
            }
        }
        writer.finish().map_err(PostProcessError::OpenZip)?;
    }

    Ok(out)
}

/// Pull the canonical `FlowKind` out of the compiled flow, preserving
/// whatever `greentic-flow` produced rather than guessing from the
/// pack metadata.
fn flow_kind_to_canonical(flow: &Flow) -> FlowKind {
    flow.kind
}

/// Inject (or replace) a sidecar file into the pack zip. Used for
/// `loading_steps.cbor` (Stage 3 loading-UX hints) so designer can
/// surface runtime-side hints without modifying the
/// `PackManifest` shape.
///
/// `entry_name` is the in-zip path (e.g. `"loading_steps.cbor"`);
/// `entry_bytes` is the file body. Every other entry is copied
/// verbatim. If the pack already carries an entry with the same name,
/// the existing bytes are replaced — useful for idempotency on
/// re-builds.
pub fn inject_sidecar(
    pack_bytes: &[u8],
    entry_name: &str,
    entry_bytes: &[u8],
) -> Result<Vec<u8>, PostProcessError> {
    let mut archive = ZipArchive::new(Cursor::new(pack_bytes))?;
    let names: Vec<String> = archive.file_names().map(|s| s.to_owned()).collect();
    let mut out = Vec::with_capacity(pack_bytes.len() + entry_bytes.len());
    {
        let mut writer = ZipWriter::new(Cursor::new(&mut out));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        // Copy every existing entry except one matching `entry_name`
        // (which we overwrite below to keep the injection idempotent).
        for name in &names {
            if name == entry_name {
                continue;
            }
            let mut entry = archive.by_name(name)?;
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|source| PostProcessError::ReadEntry {
                    entry: name.clone(),
                    source,
                })?;
            writer
                .start_file(name, options)
                .map_err(PostProcessError::OpenZip)?;
            writer
                .write_all(&buf)
                .map_err(|source| PostProcessError::WriteEntry {
                    entry: name.clone(),
                    source,
                })?;
        }
        writer
            .start_file(entry_name, options)
            .map_err(PostProcessError::OpenZip)?;
        writer
            .write_all(entry_bytes)
            .map_err(|source| PostProcessError::WriteEntry {
                entry: entry_name.to_string(),
                source,
            })?;
        writer.finish().map_err(PostProcessError::OpenZip)?;
    }
    Ok(out)
}

/// Remove one or more entries from the pack zip, preserving every other
/// entry verbatim. Missing names are silently ignored (idempotent).
///
/// Used by the worker-pack assembler to strip `PackBuilder`'s nested
/// `flows/<id>/flow.ygtc` (+ `.json`) entries once their content has been
/// re-injected at the flat `flows/main.ygtc` path `make_runner_loadable`
/// expects — without this, `populate_manifest_flows`'s `flows/*.ygtc` scan
/// matches both the nested and flat paths and double-compiles the same flow
/// into `manifest.flows`.
pub fn remove_entries(
    pack_bytes: &[u8],
    entry_names: &[&str],
) -> Result<Vec<u8>, PostProcessError> {
    let mut archive = ZipArchive::new(Cursor::new(pack_bytes))?;
    let names: Vec<String> = archive.file_names().map(|s| s.to_owned()).collect();
    let mut out = Vec::with_capacity(pack_bytes.len());
    {
        let mut writer = ZipWriter::new(Cursor::new(&mut out));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for name in &names {
            if entry_names.contains(&name.as_str()) {
                continue;
            }
            let mut entry = archive.by_name(name)?;
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|source| PostProcessError::ReadEntry {
                    entry: name.clone(),
                    source,
                })?;
            writer
                .start_file(name, options)
                .map_err(PostProcessError::OpenZip)?;
            writer
                .write_all(&buf)
                .map_err(|source| PostProcessError::WriteEntry {
                    entry: name.clone(),
                    source,
                })?;
        }
        writer.finish().map_err(PostProcessError::OpenZip)?;
    }
    Ok(out)
}

#[cfg(test)]
mod sidecar_tests {
    use super::*;
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn build_two_entry_zip() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut writer = ZipWriter::new(Cursor::new(&mut out));
            let options = SimpleFileOptions::default();
            writer.start_file("manifest.cbor", options).unwrap();
            writer.write_all(b"manifest-bytes").unwrap();
            writer.start_file("flows/main.ygtc", options).unwrap();
            writer.write_all(b"flow: yaml").unwrap();
            writer.finish().unwrap();
        }
        out
    }

    fn read_entry(zip_bytes: &[u8], name: &str) -> Option<Vec<u8>> {
        let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).ok()?;
        let mut entry = archive.by_name(name).ok()?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).ok()?;
        Some(buf)
    }

    #[test]
    fn inject_sidecar_adds_new_entry_and_preserves_existing() {
        let pack = build_two_entry_zip();
        let new_bytes = b"loading-step-cbor-payload";
        let patched = inject_sidecar(&pack, "loading_steps.cbor", new_bytes).expect("inject");

        assert_eq!(
            read_entry(&patched, "manifest.cbor").as_deref(),
            Some(&b"manifest-bytes"[..])
        );
        assert_eq!(
            read_entry(&patched, "flows/main.ygtc").as_deref(),
            Some(&b"flow: yaml"[..])
        );
        assert_eq!(
            read_entry(&patched, "loading_steps.cbor").as_deref(),
            Some(&new_bytes[..])
        );
    }

    #[test]
    fn inject_sidecar_replaces_existing_entry() {
        let pack = build_two_entry_zip();
        let first = inject_sidecar(&pack, "loading_steps.cbor", b"v1").expect("inject");
        let second = inject_sidecar(&first, "loading_steps.cbor", b"v2").expect("re-inject");
        assert_eq!(
            read_entry(&second, "loading_steps.cbor").as_deref(),
            Some(&b"v2"[..])
        );
    }

    #[test]
    fn remove_entries_drops_named_entries_and_keeps_the_rest() {
        let pack = build_two_entry_zip();
        let patched =
            remove_entries(&pack, &["flows/main.ygtc"]).expect("remove_entries must succeed");

        assert_eq!(
            read_entry(&patched, "manifest.cbor").as_deref(),
            Some(&b"manifest-bytes"[..])
        );
        assert!(
            read_entry(&patched, "flows/main.ygtc").is_none(),
            "removed entry must be gone"
        );
    }

    #[test]
    fn remove_entries_is_idempotent_for_missing_names() {
        let pack = build_two_entry_zip();
        let patched =
            remove_entries(&pack, &["does/not/exist.ygtc"]).expect("missing name is a no-op");

        assert_eq!(
            read_entry(&patched, "manifest.cbor").as_deref(),
            Some(&b"manifest-bytes"[..])
        );
        assert_eq!(
            read_entry(&patched, "flows/main.ygtc").as_deref(),
            Some(&b"flow: yaml"[..])
        );
    }
}

//! Recompute a `.gtpack`'s SBOM so it describes the archive as it actually is.
//!
//! # Why this exists
//!
//! `greentic_pack::builder::PackBuilder` seals a pack at build time: it writes
//! `sbom.json` listing every content file with its blake3 hash, and signs a
//! digest over that inventory. Every producer in this crate then MUTATES the
//! sealed archive — [`crate::loadable::make_runner_loadable`] replaces
//! `manifest.cbor`, [`crate::assemble::build_worker_pack`] drops the JSON
//! manifest mirror, and [`crate::inject::embed_dw_agents`] and the knowledge
//! baker add sidecars. The seal is never recomputed, so the shipped pack's
//! SBOM lists files that are gone, omits files that are there, and records a
//! stale hash for `manifest.cbor`.
//!
//! Nothing on the RUNTIME path reads the SBOM — greentic-runner-host goes
//! straight to `manifest.cbor` — which is precisely why this went unnoticed:
//! the pack boots, serves turns, and answers `/healthz`, and only
//! `greentic-pack doctor` says otherwise, as six `PACK_SBOM_DANGLING_PATH` /
//! `PACK_MISSING_FILE` errors on an artefact a customer has already been
//! handed.
//!
//! # Why it lives inside the mutators rather than beside them
//!
//! The obvious shape is a `reseal()` every producer calls once it has finished
//! mutating. That is the shape that failed: this crate and greentic-designer
//! between them mutate a built pack from roughly a dozen call sites, each of
//! which would have to remember, and forgetting produces no error anywhere.
//! So [`reseal_archive`] is called from the tail of every function that
//! rewrites the zip ([`crate::cbor_flow_post::inject_sidecar`],
//! `remove_entries`, `populate_manifest_flows`) — the invariant then holds by
//! construction, and a new mutator gets it for free as long as it goes through
//! those.
//!
//! It is idempotent (the inventory is derived from the archive each time), so
//! running it after every single mutation costs a blake3 pass and nothing else.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use serde::{Deserialize, Serialize};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

use crate::cbor_flow_post::PostProcessError;

/// Entries that describe the archive rather than being part of its content,
/// and so are never listed in the SBOM. Mirrors the exclusion list in
/// `greentic_pack_lib::reader::verify_sbom`.
const SELF_DESCRIBING: &[&str] = &[SBOM_JSON, SBOM_CBOR, SIGNATURE_PATH, SIGNATURE_CHAIN_PATH];

const SBOM_JSON: &str = "sbom.json";
const SBOM_CBOR: &str = "sbom.cbor";
const SIGNATURE_PATH: &str = "signatures/pack.sig";
const SIGNATURE_CHAIN_PATH: &str = "signatures/chain.pem";

/// One SBOM row. Field-for-field `greentic_pack_lib::builder::SbomEntry`; the
/// reader deserializes exactly these four and nothing else.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SbomEntry {
    path: String,
    size: u64,
    hash_blake3: String,
    media_type: String,
}

/// The SBOM document.
///
/// `format` is round-tripped VERBATIM rather than written from a constant of
/// our own: the format string is greentic-pack-lib's to define, and a second
/// copy of it here is a copy that can go stale without anything failing until
/// a reader rejects the document.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SbomDocument {
    format: String,
    files: Vec<SbomEntry>,
}

/// Which encoding the archive's SBOM uses. `PackBuilder` writes JSON; packc
/// writes CBOR. Both are re-emitted in the encoding they arrived in.
#[derive(Clone, Copy)]
enum SbomEncoding {
    Json,
    Cbor,
}

impl SbomEncoding {
    fn entry_name(self) -> &'static str {
        match self {
            Self::Json => SBOM_JSON,
            Self::Cbor => SBOM_CBOR,
        }
    }
}

/// Rewrite `pack_bytes` with an SBOM recomputed from the archive's own
/// contents, and without the signature files.
///
/// # The signature is dropped, deliberately
///
/// The signed digest covers `manifest.cbor` plus the SBOM inventory, so any
/// mutation invalidates it. Leaving it in place makes a reader report
/// "signature verification failed" on a pack that is otherwise correct, which
/// reads as tampering; dropping it makes the same reader say "signature files
/// missing; skipping verification", which is what actually happened. A pack
/// that must ship signed has to be signed AFTER it stops being mutated —
/// which, for a worker pack, is after greentic-designer has embedded its own
/// sidecars, in a different process.
///
/// # Returns
///
/// The input bytes unchanged when the archive carries no SBOM at all — there
/// is then no seal to keep, and inventing one would claim a guarantee the
/// producer never made.
pub(crate) fn reseal_archive(pack_bytes: &[u8]) -> Result<Vec<u8>, PostProcessError> {
    let mut archive = ZipArchive::new(Cursor::new(pack_bytes))?;
    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();

    let encoding = if names.iter().any(|n| n == SBOM_JSON) {
        SbomEncoding::Json
    } else if names.iter().any(|n| n == SBOM_CBOR) {
        SbomEncoding::Cbor
    } else {
        return Ok(pack_bytes.to_vec());
    };

    // Read every entry once: the content files need hashing, and the old SBOM
    // is read for its `format` and for the media types it already assigned.
    let mut contents: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for name in &names {
        let mut entry = archive.by_name(name)?;
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|source| PostProcessError::ReadEntry {
                entry: name.clone(),
                source,
            })?;
        contents.insert(name.clone(), buf);
    }

    let old = decode_sbom(&contents[encoding.entry_name()], encoding)?;
    let known_media: BTreeMap<&str, &str> = old
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.media_type.as_str()))
        .collect();

    let files: Vec<SbomEntry> = contents
        .iter()
        .filter(|(path, _)| !SELF_DESCRIBING.contains(&path.as_str()))
        .map(|(path, bytes)| SbomEntry {
            // An entry the previous SBOM already described keeps its media
            // type: the producer knew better than an extension guess, and
            // preserving it means a rebuild is a no-op on that column.
            media_type: known_media
                .get(path.as_str())
                .map(|m| (*m).to_string())
                .unwrap_or_else(|| media_type_for(path).to_string()),
            path: path.clone(),
            size: bytes.len() as u64,
            hash_blake3: blake3::hash(bytes).to_hex().to_string(),
        })
        .collect();

    let sbom_bytes = encode_sbom(
        &SbomDocument {
            format: old.format,
            files,
        },
        encoding,
    )?;

    let mut out = Vec::with_capacity(pack_bytes.len());
    {
        let mut writer = ZipWriter::new(Cursor::new(&mut out));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (path, bytes) in &contents {
            if path == SIGNATURE_PATH || path == SIGNATURE_CHAIN_PATH {
                continue;
            }
            let body = if path == encoding.entry_name() {
                &sbom_bytes
            } else {
                bytes
            };
            writer
                .start_file(path, options)
                .map_err(PostProcessError::OpenZip)?;
            writer
                .write_all(body)
                .map_err(|source| PostProcessError::WriteEntry {
                    entry: path.clone(),
                    source,
                })?;
        }
        writer.finish().map_err(PostProcessError::OpenZip)?;
    }
    Ok(out)
}

fn decode_sbom(bytes: &[u8], encoding: SbomEncoding) -> Result<SbomDocument, PostProcessError> {
    match encoding {
        SbomEncoding::Json => {
            serde_json::from_slice(bytes).map_err(|e| PostProcessError::Sbom(format!("{e}")))
        }
        SbomEncoding::Cbor => {
            serde_cbor::from_slice(bytes).map_err(|e| PostProcessError::Sbom(format!("{e}")))
        }
    }
}

fn encode_sbom(doc: &SbomDocument, encoding: SbomEncoding) -> Result<Vec<u8>, PostProcessError> {
    match encoding {
        // Pretty, matching `PackBuilder`, so a hand-inspected pack reads the
        // same before and after a reseal.
        SbomEncoding::Json => {
            serde_json::to_vec_pretty(doc).map_err(|e| PostProcessError::Sbom(format!("{e}")))
        }
        SbomEncoding::Cbor => {
            serde_cbor::to_vec(doc).map_err(|e| PostProcessError::Sbom(format!("{e}")))
        }
    }
}

/// Map a path to its SBOM media type. Only ever consulted for an entry the
/// previous SBOM did not already describe, so it decides nothing about a file
/// a producer already classified.
///
/// `greentic_pack_lib::reader::media_type_for`'s arms plus one: a `.ygtc`
/// falls through to `application/octet-stream` there, while `PackBuilder`
/// writes `application/yaml` for the flow source it emits itself. The builder
/// is the one that knows, so this follows the builder. Nothing verifies this
/// column — `verify_sbom` checks path and hash only — so the disagreement is
/// cosmetic either way.
fn media_type_for(path: &str) -> &'static str {
    if path.ends_with(".cbor") {
        "application/cbor"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".wasm") {
        "application/wasm"
    } else if path.ends_with(".yaml") || path.ends_with(".yml") || path.ends_with(".ygtc") {
        "application/yaml"
    } else {
        "application/octet-stream"
    }
}

# Flow-Tool Author Contract — Slice B (greentic-dw-authoring) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the pack builder emits an agent's tool list, populate the new `ToolRef.description` + `ToolRef.input_schema` fields (added in Slice A) from the author's `ExtensionToolBinding`, so a `flow:` tool's author contract is baked into the pack's `dw-agents.json`.

**Architecture:** Bump the pinned `greentic-aw-runtime` rev to the merged Slice-A rev (the new `ToolRef` fields only exist there — the bump also breaks the two existing `ToolRef` literals, which we then fix), and populate the author `description` + parsed `input_schema` in `tool_refs_from_extension_tools`.

**Tech Stack:** Rust, `serde_json`.

**Repo:** greentic-dw-authoring. Worktree `dwauth-flowcontract`, branch `feat/flow-tool-author-contract` (from `origin/main`, HEAD 1b9bd51). PR to `main`.

## Global Constraints

- English only. Conventional Commits. **No Claude co-authorship trailer.** No `unwrap()`/`panic!()` in production paths (tests may).
- Pin `greentic-aw-runtime` to rev `953d2369dd0ee059e27a306be7d04de85435a0d5` (the merged Slice-A rev with the extended `ToolRef`).
- An EMPTY author description or empty/invalid `input_schema_json` MUST map to `None` (so the runtime falls back to the catalog), NOT `Some("")` / `Some(invalid)`.
- Only `tool_refs_from_extension_tools` gains real population. `tool_refs_from_strings` just gets `description: None, input_schema: None` (string tool ids carry no author metadata).
- `CARGO_BUILD_JOBS=2`.

## File map

- `Cargo.toml` — MODIFY: bump `greentic-aw-runtime` rev to `953d2369…`.
- `src/assemble.rs` — MODIFY: `tool_refs_from_strings` (`:433` literal) + `tool_refs_from_extension_tools` (`:461` literal, real population) + a small `parse_input_schema` helper.
- Test: `src/assemble.rs` test module.

---

### Task 1: Bump aw-runtime + populate the author contract

**Files:**
- Modify: `Cargo.toml` (aw-runtime rev)
- Modify: `src/assemble.rs` (`tool_refs_from_strings`, `tool_refs_from_extension_tools`, add `parse_input_schema`)
- Test: `src/assemble.rs` test module

**Interfaces:**
- Consumes: `greentic_aw_runtime::ToolRef { extension_id, tool_name, description: Option<String>, input_schema: Option<serde_json::Value> }` (Slice A); `ExtensionToolBinding { extension_id, tool_name, description: String, input_schema_json: String, capabilities, .. }` (`src/model.rs`).
- Produces: `tool_refs_from_extension_tools` emits `ToolRef`s with `description`/`input_schema` populated for agentic bindings.

**Context:** `ToolRef` gained two non-`Default` optional fields in Slice A. Bumping the pin makes both existing `ToolRef { extension_id, tool_name }` literals (`assemble.rs:433` in `tool_refs_from_strings`, `:461` in `tool_refs_from_extension_tools`) fail to compile until they name the new fields. `tool_refs_from_strings` has no author metadata → `None, None`. `tool_refs_from_extension_tools` iterates `bindings` (each an `ExtensionToolBinding` with `description: String` + `input_schema_json: String` in scope) → populate. An empty description or empty/invalid schema JSON → `None`.

- [ ] **Step 1: Write the failing test**

Add to the `assemble.rs` test module (mirror existing tests' `ExtensionToolBinding` construction — grep the module for how bindings are built; `AGENTIC_WORKER_CAPABILITY` is the agentic capability constant):

```rust
#[test]
fn extension_tool_refs_carry_author_contract() {
    let bindings = vec![ExtensionToolBinding {
        extension_id: "flow:refund".to_string(),
        tool_name: "refund_lookup".to_string(),
        description: "Look up a refund by order id".to_string(),
        input_schema_json: r#"{"type":"object","properties":{"order_id":{"type":"string"}}}"#.to_string(),
        capabilities: vec![AGENTIC_WORKER_CAPABILITY.to_string()],
        ..Default::default()
    }];
    let refs = tool_refs_from_extension_tools(&bindings);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].extension_id, "flow:refund");
    assert_eq!(refs[0].description.as_deref(), Some("Look up a refund by order id"));
    let schema = refs[0].input_schema.as_ref().expect("input_schema populated");
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
```

(If `ExtensionToolBinding` doesn't derive `Default`, construct the literal fully — mirror an existing test's binding and set the fields shown. Confirm `AGENTIC_WORKER_CAPABILITY` is imported in the test scope.)

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=2 cargo test extension_tool_refs_carry_author_contract 2>&1 | tail`
Expected: FAIL — before the pin bump, `ToolRef` has no `description`/`input_schema` (compile error on the test), OR after the bump, the fields are `None` (unpopulated).

- [ ] **Step 3: Bump the pin**

In `Cargo.toml`, change the `greentic-aw-runtime` rev to `953d2369dd0ee059e27a306be7d04de85435a0d5`. Run `CARGO_BUILD_JOBS=2 cargo update -p greentic-aw-runtime` to refresh `Cargo.lock`.

- [ ] **Step 4: Fix literals + populate**

In `src/assemble.rs`:

`tool_refs_from_strings` (`:433`):
```rust
        .map(|tool_id| ToolRef {
            extension_id: tool_id.clone(),
            tool_name: tool_id.clone(),
            description: None,
            input_schema: None,
        })
```

Add a helper near the tool-ref functions:
```rust
/// Parse an author-supplied JSON-schema string. Blank or invalid JSON yields
/// `None` so the runtime falls back to its own catalog contract.
fn parse_input_schema(raw: &str) -> Option<serde_json::Value> {
    if raw.trim().is_empty() {
        return None;
    }
    serde_json::from_str(raw).ok()
}
```

`tool_refs_from_extension_tools` (`:461`):
```rust
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
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `CARGO_BUILD_JOBS=2 cargo test 2>&1 | tail -15`
Expected: PASS — the two new tests + all existing (the pin bump + literal fixes compile). Then `CARGO_BUILD_JOBS=2 cargo build` clean.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/assemble.rs
git commit -m "feat(dw-authoring): bake flow-tool author contract (description + input_schema) into pack tool refs"
```

---

### Task 2: Gate + PR

- [ ] **Step 1: fmt + clippy + tests (foreground)**

Run (each FOREGROUND):
```
CARGO_BUILD_JOBS=2 cargo fmt --all -- --check
CARGO_BUILD_JOBS=2 cargo clippy --all-targets -- -D warnings
CARGO_BUILD_JOBS=2 cargo test 2>&1 | tail -15
```
If a subagent skipped `cargo fmt`, run `cargo fmt --all` and commit as a `style(...)` fixup. If `clippy --all-targets` hits an unrelated heavy optional backend that fails to build in this environment, fall back to the default-feature clippy and note it.

- [ ] **Step 2: PR to main**

```bash
git push -u origin feat/flow-tool-author-contract
gh pr create --base main --title "feat(dw-authoring): flow-tool author contract in pack tool refs (Slice B)" --body "Slice B of the flow-tool author-contract change. Bumps greentic-aw-runtime to the merged Slice-A rev (953d2369) and populates the new ToolRef.description + ToolRef.input_schema from each agentic ExtensionToolBinding in tool_refs_from_extension_tools (blank description / invalid schema -> None). This bakes a flow tool's author-defined LLM contract into the pack's dw-agents.json. Slice C (greentic-designer) does the same for the test-chat path. Spec: greentic-runner docs/superpowers/specs/2026-07-07-flow-tool-author-contract-design.md."
```

---

## Self-Review

**Spec coverage:** populate at the pack-builder `ToolRef` site (`tool_refs_from_extension_tools`) → Task 1; blank/invalid → None → Task 1 (`parse_input_schema` + description trim); pin bump to the Slice-A rev → Task 1; `tool_refs_from_strings` literal fixed (no metadata) → Task 1; gate + PR → Task 2. Answer-doc + designer test-chat = out of scope (already done / Slice C). ✓

**Placeholder scan:** No TBD. The `ExtensionToolBinding` construction in tests names the concrete fields + `AGENTIC_WORKER_CAPABILITY`; the `Default` caveat is called out. ✓

**Type consistency:** `description: Option<String>` + `input_schema: Option<serde_json::Value>` match Slice A's `ToolRef`; `parse_input_schema(&str) -> Option<Value>` consistent; both literal sites fixed. ✓

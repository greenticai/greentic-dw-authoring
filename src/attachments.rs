//! The flow-node mappings that carry message attachments to the agent.
//!
//! Contract: the attachments master plan, C4b. `attachments` is the envelope's
//! attachment list (artifact references once the host processed them);
//! `attachment_meta` is `extensions.artifacts` (sha256, kind, text_ref) and
//! `attachment_notes` is `extensions.attachment_notes` (why an attachment could
//! not be stored). All three are parallel by index, so every emitter maps all
//! three together: a node mapping only some of them silently hides the rest
//! (e.g. the host's "too large / unsupported" notes) from the agent.
//!
//! At run time an unresolved template renders as the EMPTY STRING, not an
//! error: greentic-runner-host's `templating::render_template_string` answers
//! an exact `{{path}}` expression whose path does not resolve with
//! `Value::String("")` (and a `warn`), while a resolved one is cloned as-is, so
//! an array stays an array. A message with no attachments has none of the
//! three; the runner treats `""`, `null` and non-arrays as "none". A pack built
//! before these mappings existed carries only `user_text` and reads the same
//! way.

use serde_json::{Map, Value};

/// `(input key, template)` pairs, the single definition every emitter uses.
pub(crate) const ATTACHMENT_MAPPINGS: [(&str, &str); 3] = [
    ("attachments", "{{in.attachments}}"),
    ("attachment_meta", "{{in.extensions.artifacts}}"),
    ("attachment_notes", "{{in.extensions.attachment_notes}}"),
];

/// [`ATTACHMENT_MAPPINGS`] as a JSON map of string values.
pub(crate) fn attachment_mappings() -> Map<String, Value> {
    ATTACHMENT_MAPPINGS
        .iter()
        .map(|(key, template)| ((*key).to_string(), Value::String((*template).to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_mappings_match_contract_c4b_exactly() {
        let m = attachment_mappings();
        assert_eq!(m.len(), 3);
        assert_eq!(m["attachments"], "{{in.attachments}}");
        assert_eq!(m["attachment_meta"], "{{in.extensions.artifacts}}");
        assert_eq!(m["attachment_notes"], "{{in.extensions.attachment_notes}}");
    }
}

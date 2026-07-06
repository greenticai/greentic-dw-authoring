//! Slugify a display string into a lowercase dash-separated identifier.

/// Convert an arbitrary string to a lowercase, dash-separated slug.
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = true; // suppress leading dashes
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::slugify;

    #[test]
    fn slugify_lowercases_and_dashes() {
        assert_eq!(slugify("Support Triage!"), "support-triage");
    }

    #[test]
    fn slugify_collapses_and_trims() {
        assert_eq!(slugify("  Hello --  World  "), "hello-world");
        assert_eq!(slugify("已经"), ""); // non-ascii dropped
    }
}

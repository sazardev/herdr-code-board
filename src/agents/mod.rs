//! Agent kinds and how each CLI accepts a model selection.

use std::collections::BTreeMap;

use crate::config::ModelFlag;

/// Agent kinds herdr 0.8.x can start. Source: `herdr agent` usage output.
pub const KINDS: &[&str] = &[
    "pi",
    "claude",
    "codex",
    "gemini",
    "cursor",
    "devin",
    "agy",
    "cline",
    "omp",
    "mastracode",
    "opencode",
    "copilot",
    "kimi",
    "kiro",
    "droid",
    "amp",
    "grok",
    "hermes",
    "kilo",
    "qodercli",
    "qwen",
    "maki",
];

pub fn is_known_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// Kinds whose model flag was checked against the installed CLI's own `--help`.
/// Everything else falls back to `--model`, which is a guess — see
/// [`model_args`] and the `model_flags` table in `config.toml`.
pub const VERIFIED_MODEL_FLAGS: &[&str] = &["claude", "opencode", "copilot"];

/// The fallback used for any kind without an entry.
pub const FALLBACK_MODEL_FLAG: &str = "--model";

pub fn default_model_flags() -> BTreeMap<String, ModelFlag> {
    let mut m = BTreeMap::new();
    let mut set = |kind: &str, flag: &str| {
        m.insert(kind.to_string(), ModelFlag::Flag(flag.to_string()));
    };
    // Verified locally.
    set("claude", "--model");
    set("opencode", "--model");
    set("copilot", "--model");
    // Widely documented, but not verified on this machine. Override in config.toml
    // if your CLI disagrees.
    set("codex", "--model");
    set("gemini", "--model");
    set("cursor", "--model");
    set("qwen", "--model");
    set("grok", "--model");
    set("droid", "--model");
    m
}

/// Build the argv fragment that selects `model` for `kind`.
///
/// Returns the args plus whether the mapping was configured or guessed, so the
/// caller can record the assumption in the run log instead of hiding it.
pub fn model_args(
    flags: &BTreeMap<String, ModelFlag>,
    kind: &str,
    model: &str,
) -> (Vec<String>, bool) {
    match flags.get(kind) {
        Some(flag) => (flag.render(model), true),
        None => (
            vec![FALLBACK_MODEL_FLAG.to_string(), model.to_string()],
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_flag_names_a_real_herdr_kind() {
        for kind in default_model_flags().keys() {
            assert!(is_known_kind(kind), "{kind} is not a herdr agent kind");
        }
    }

    #[test]
    fn verified_kinds_are_present_in_the_default_table() {
        let flags = default_model_flags();
        for kind in VERIFIED_MODEL_FLAGS {
            assert!(flags.contains_key(*kind), "{kind} missing from defaults");
        }
    }

    #[test]
    fn unmapped_kind_falls_back_and_reports_it() {
        let flags = default_model_flags();
        let (args, mapped) = model_args(&flags, "maki", "some-model");
        assert_eq!(args, vec!["--model", "some-model"]);
        assert!(!mapped, "an unmapped kind must report that it guessed");

        let (args, mapped) = model_args(&flags, "claude", "opus");
        assert_eq!(args, vec!["--model", "opus"]);
        assert!(mapped);
    }
}

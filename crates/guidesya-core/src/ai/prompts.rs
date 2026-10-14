//! Versioned prompt files. The version is recorded on generated text as
//! [`AiState::prompt_version`](crate::model::AiState); bump the file name
//! (e.g. `step.v2.md`) whenever a prompt changes meaningfully.

/// A system prompt and the version recorded with its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prompt {
    pub version: &'static str,
    pub text: &'static str,
}

/// Per-step pass.
pub const STEP: Prompt = Prompt {
    version: "step.v1",
    text: include_str!("prompts/step.v1.md"),
};

/// Guide-level pass.
pub const GUIDE: Prompt = Prompt {
    version: "guide.v1",
    text: include_str!("prompts/guide.v1.md"),
};

/// Step rewrite.
pub const REWRITE: Prompt = Prompt {
    version: "rewrite.v1",
    text: include_str!("prompts/rewrite.v1.md"),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_are_loaded_and_name_their_tools() {
        for (prompt, tool) in [
            (STEP, "record_step"),
            (GUIDE, "record_guide"),
            (REWRITE, "record_rewrite"),
        ] {
            assert!(prompt.text.len() > 200, "{} is too short", prompt.version);
            assert!(
                prompt.text.contains(tool),
                "{} must name {tool}",
                prompt.version
            );
        }
    }
}

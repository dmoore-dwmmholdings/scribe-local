//! Summary templates (design §9 extension).
//!
//! A *template* changes only the framing/instructions handed to the LLM for the
//! summarize stage — never the output shape. The model is always asked for the
//! same STRICT JSON object (`{title, summary, action_items, decisions, topics}`);
//! the template just steers *what to put in each field* for a given meeting type
//! (standup, interview, 1:1, lecture, sales call, …). This keeps every consumer
//! (mobile app, search, API) working against one fixed schema while letting a
//! recording be re-summarized with a different lens on demand.
//!
//! Lives in `scribe-core` (the shared contract) because both the worker
//! (`scribe-pipeline`, which renders the prompt) and the HTTP API
//! (`scribe-api`, which lists/validates templates) need it, and it carries no ML
//! dependencies.

/// A built-in summary template: a stable `id`, a human `label`, and the
/// `instructions` block injected into the user prompt.
#[derive(Debug, Clone, Copy)]
pub struct SummaryTemplate {
    /// Stable identifier (lowercase, used in API/payload). Never changes.
    pub id: &'static str,
    /// Short human-facing label for pickers.
    pub label: &'static str,
    /// The framing injected into the LLM user prompt for this template.
    pub instructions: &'static str,
}

/// The canonical built-in templates. `general` is first and is the fallback.
pub const TEMPLATES: &[SummaryTemplate] = &[
    SummaryTemplate {
        id: "general",
        label: "General meeting",
        instructions: "Summarize this meeting. Capture the overall discussion, \
            concrete action items, explicit decisions made, and the main topics covered.",
    },
    SummaryTemplate {
        id: "standup",
        label: "Standup",
        instructions: "This is a team standup. For the summary, capture per-person what was \
            done, what's next, and any blockers. action_items = unblock/follow-up tasks. \
            decisions = any scope or priority calls. topics = workstreams mentioned.",
    },
    SummaryTemplate {
        id: "interview",
        label: "Interview",
        instructions: "This is an interview. summary = the candidate's background and the \
            strongest signals (positive and negative). action_items = recommended next steps \
            (e.g. follow-up interview, references). decisions = the hiring lean if expressed. \
            topics = competencies/areas probed.",
    },
    SummaryTemplate {
        id: "one_on_one",
        label: "1:1",
        instructions: "This is a 1:1 meeting. summary = key discussion points and sentiment. \
            action_items = commitments made by either side. decisions = anything agreed. \
            topics = themes (growth, feedback, workload, etc.).",
    },
    SummaryTemplate {
        id: "lecture",
        label: "Lecture / talk",
        instructions: "This is a lecture or talk. summary = the thesis and main arguments. \
            action_items = suggested study/follow-up. decisions = N/A unless stated. \
            topics = key concepts introduced.",
    },
    SummaryTemplate {
        id: "sales",
        label: "Sales call",
        instructions: "This is a sales call. summary = the prospect's needs, pain points, and \
            buying signals. action_items = seller next steps. decisions = commitments/next-meeting. \
            topics = objections and products discussed.",
    },
];

/// The default template id used when none is supplied or one is unknown.
pub const DEFAULT_TEMPLATE_ID: &str = "general";

/// Resolve an id (case-insensitive, whitespace-trimmed) to its template,
/// falling back to `general` for an empty or unknown id. Never fails.
pub fn resolve(id: &str) -> &'static SummaryTemplate {
    let id = id.trim();
    TEMPLATES
        .iter()
        .find(|t| t.id.eq_ignore_ascii_case(id))
        .unwrap_or_else(|| {
            TEMPLATES
                .iter()
                .find(|t| t.id == DEFAULT_TEMPLATE_ID)
                .expect("general template is always present")
        })
}

/// True if `id` (case-insensitive, trimmed) names a known template.
pub fn is_known(id: &str) -> bool {
    let id = id.trim();
    TEMPLATES.iter().any(|t| t.id.eq_ignore_ascii_case(id))
}

/// The canonical id a known/unknown id resolves to (for persistence + responses).
pub fn canonical_id(id: &str) -> &'static str {
    resolve(id).id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_empty_fall_back_to_general() {
        assert_eq!(resolve("").id, "general");
        assert_eq!(resolve("does-not-exist").id, "general");
        assert_eq!(resolve("   ").id, "general");
    }

    #[test]
    fn resolve_is_case_insensitive_and_trims() {
        assert_eq!(resolve("STANDUP").id, "standup");
        assert_eq!(resolve("  Interview  ").id, "interview");
        assert_eq!(resolve("One_On_One").id, "one_on_one");
    }

    #[test]
    fn is_known_matches_registry() {
        assert!(is_known("sales"));
        assert!(is_known("LECTURE"));
        assert!(!is_known("brainstorm"));
        assert!(!is_known(""));
    }

    #[test]
    fn all_expected_ids_present() {
        let ids: Vec<&str> = TEMPLATES.iter().map(|t| t.id).collect();
        assert_eq!(
            ids,
            vec!["general", "standup", "interview", "one_on_one", "lecture", "sales"]
        );
    }
}

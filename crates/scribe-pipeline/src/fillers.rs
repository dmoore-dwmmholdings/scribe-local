//! Filler-word removal. Strips non-lexical hesitation sounds (uh, um, …) from
//! the ASR word stream before it becomes utterances, driven by
//! `[asr].remove_fillers` / `[asr].filler_words`.

use std::collections::HashSet;

use scribe_core::config::AsrConfig;
use scribe_core::types::Word;

/// A case-insensitive, punctuation-tolerant filler matcher built from config.
pub struct FillerFilter {
    enabled: bool,
    set: HashSet<String>,
}

impl FillerFilter {
    /// Build from the ASR config. When `remove_fillers` is false, [`clean`] is a
    /// no-op regardless of the word list.
    pub fn from_config(cfg: &AsrConfig) -> Self {
        Self {
            enabled: cfg.remove_fillers,
            set: cfg
                .filler_words
                .iter()
                .map(|w| normalize(w))
                .filter(|w| !w.is_empty())
                .collect(),
        }
    }

    /// Is `word` (after normalisation) a configured filler?
    pub fn is_filler(&self, word: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let n = normalize(word);
        !n.is_empty() && self.set.contains(&n)
    }

    /// Remove filler words from `words` in place. Returns the number removed.
    pub fn clean(&self, words: &mut Vec<Word>) -> usize {
        if !self.enabled || self.set.is_empty() {
            return 0;
        }
        let before = words.len();
        words.retain(|w| !self.is_filler(&w.text));
        before - words.len()
    }
}

/// Lowercase and strip leading/trailing non-alphanumerics so `"Um,"`, `"uh."`
/// and `"UH"` all match `uh`/`um`. Internal characters (e.g. the hyphen in
/// `uh-huh`) are preserved, so genuine words aren't collapsed onto a filler.
fn normalize(w: &str) -> String {
    w.trim()
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(enabled: bool) -> AsrConfig {
        AsrConfig {
            remove_fillers: enabled,
            ..AsrConfig::default()
        }
    }

    fn word(t: &str) -> Word {
        Word {
            text: t.to_string(),
            start_ms: 0,
            end_ms: 0,
            conf: 1.0,
            local_idx: None,
        }
    }

    #[test]
    fn removes_fillers_with_punctuation_and_case() {
        let f = FillerFilter::from_config(&cfg(true));
        let mut words = vec![
            word("So"),
            word("um,"),
            word("the"),
            word("Uh"),
            word("plan"),
            word("um."),
        ];
        let removed = f.clean(&mut words);
        assert_eq!(removed, 3);
        let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(text, vec!["So", "the", "plan"]);
    }

    #[test]
    fn keeps_meaningful_words_including_uh_huh() {
        let f = FillerFilter::from_config(&cfg(true));
        let mut words = vec![word("uh-huh"), word("I"), word("agree")];
        let removed = f.clean(&mut words);
        assert_eq!(removed, 0); // "uh-huh" is not a bare "uh"
    }

    #[test]
    fn disabled_is_noop() {
        let f = FillerFilter::from_config(&cfg(false));
        let mut words = vec![word("um"), word("yes")];
        assert_eq!(f.clean(&mut words), 0);
        assert_eq!(words.len(), 2);
    }
}

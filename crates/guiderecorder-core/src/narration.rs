//! Narration (#13): what the author says while recording, transcribed on
//! the device by the capture crate, split across the recorded steps.
//!
//! The capture crate hands over [`SpokenWord`]s on the event clock (host
//! monotonic ns, like [`crate::events`]), and the recording gives each
//! saved step the time of its action. [`split`] then:
//!
//! - joins the words into phrases: a pause longer than [`PHRASE_GAP_MS`],
//!   or a word ending a sentence, starts a new one;
//! - gives each phrase to one step, by when it starts. A step owns the
//!   speech from [`LEAD_MS`] before its action until [`LEAD_MS`] before the
//!   next step's. People tend to say what they are about to do just before
//!   they click, and to explain the result after, so a phrase begun just
//!   before a click goes with that click, and a phrase begun earlier (or
//!   after it) with the step before. Speech before the first step goes to
//!   the first, and speech after the last to the last.
//!
//! [`apply`] stores each step's part as [`Step::narration`] and makes it the
//! description of a step that has none yet (as a recording's steps don't).
//! The AI passes get the narration as context, so with AI on it improves
//! the description rather than replacing it blindly.

use ulid::Ulid;

use crate::events::Timestamp;
use crate::model::{AiStatus, Guide, Item, Step};

/// Speech starting this long before a step's action belongs to that step.
pub const LEAD_MS: u64 = 1500;
/// A pause between words longer than this starts a new phrase.
pub const PHRASE_GAP_MS: u64 = 700;

/// One recognised word (with any punctuation the recogniser attached),
/// on the event clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpokenWord {
    pub start: Timestamp,
    pub end: Timestamp,
    pub text: String,
}

/// Words joined into phrases (see the module docs), in time order. Each
/// phrase spans its first word's start to its last word's end; its text is
/// the words joined by single spaces.
pub fn phrases(words: &[SpokenWord]) -> Vec<SpokenWord> {
    let mut sorted: Vec<&SpokenWord> = words.iter().filter(|w| !w.text.trim().is_empty()).collect();
    sorted.sort_by_key(|w| w.start);
    let gap_ns = PHRASE_GAP_MS * 1_000_000;
    let mut out: Vec<SpokenWord> = Vec::new();
    for word in sorted {
        let text = word.text.split_whitespace().collect::<Vec<_>>().join(" ");
        match out.last_mut() {
            Some(phrase)
                if word.start.0.saturating_sub(phrase.end.0) <= gap_ns
                    && !ends_sentence(&phrase.text) =>
            {
                phrase.text.push(' ');
                phrase.text.push_str(&text);
                phrase.end = phrase.end.max(word.end);
            }
            _ => out.push(SpokenWord {
                start: word.start,
                end: word.end.max(word.start),
                text,
            }),
        }
    }
    out
}

fn ends_sentence(text: &str) -> bool {
    text.ends_with(['.', '?', '!', '…'])
}

/// The narration of each step: `step_times` are the steps' action times in
/// order, and the result has one entry per step (empty where nothing was
/// said). See the module docs for which phrase goes where.
pub fn split(words: &[SpokenWord], step_times: &[Timestamp]) -> Vec<String> {
    let mut parts = vec![String::new(); step_times.len()];
    if step_times.is_empty() {
        return parts;
    }
    let lead_ns = LEAD_MS * 1_000_000;
    for phrase in phrases(words) {
        let owner = step_times
            .iter()
            .rposition(|at| at.0 <= phrase.start.0.saturating_add(lead_ns))
            .unwrap_or(0);
        let part = &mut parts[owner];
        if !part.is_empty() {
            part.push(' ');
        }
        part.push_str(&phrase.text);
    }
    parts
}

/// Splits `words` across the guide's steps named in `step_times` (each
/// step's id and action time, in order; see [`split`]). Each step that got
/// some speech has it stored as its [`Step::narration`], and as its
/// description too when it has none and was not edited by hand. Returns
/// how many steps got some. Steps not named are left alone.
pub fn apply(guide: &mut Guide, step_times: &[(Ulid, Timestamp)], words: &[SpokenWord]) -> usize {
    let times: Vec<Timestamp> = step_times.iter().map(|(_, at)| *at).collect();
    let parts = split(words, &times);
    let mut narrated = 0;
    for ((id, _), text) in step_times.iter().zip(parts) {
        if text.is_empty() {
            continue;
        }
        let Some(step) = step_mut(guide, *id) else {
            continue;
        };
        if step.description.trim().is_empty() && step.ai_state.status != AiStatus::Edited {
            step.description = description(&text);
        }
        step.narration = text;
        narrated += 1;
    }
    narrated
}

fn step_mut(guide: &mut Guide, id: Ulid) -> Option<&mut Step> {
    guide.items.iter_mut().find_map(|item| match item {
        Item::Step(step) if step.id == id => Some(step),
        _ => None,
    })
}

/// Narration as a description: Markdown-escaped, starting with a capital
/// and ending a sentence.
pub fn description(narration: &str) -> String {
    let mut text = crate::steps::escape(narration);
    if let Some(first) = text.chars().next() {
        let upper: String = first.to_uppercase().collect();
        text.replace_range(..first.len_utf8(), &upper);
    }
    if !text.is_empty() && !ends_sentence(&text) && !text.ends_with([':', ';']) {
        text.push('.');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{StepImage, StepKind};

    const S: u64 = 1_000_000_000;

    /// Words at `secs` (start) each lasting 0.3 s.
    fn words(spoken: &[(f64, &str)]) -> Vec<SpokenWord> {
        spoken
            .iter()
            .map(|&(secs, text)| {
                let start = (secs * S as f64) as u64;
                SpokenWord {
                    start: Timestamp(start),
                    end: Timestamp(start + 300_000_000),
                    text: text.into(),
                }
            })
            .collect()
    }

    fn at(secs: u64) -> Timestamp {
        Timestamp(secs * S)
    }

    #[test]
    fn words_join_into_phrases_at_pauses_and_sentence_ends() {
        let spoken = words(&[
            (1.0, "open"),
            (1.4, "the"),
            (1.8, "menu."),
            (2.2, "Then"),
            (2.6, "pick"),
            (5.0, "export"),
        ]);
        let texts: Vec<String> = phrases(&spoken).into_iter().map(|p| p.text).collect();
        assert_eq!(texts, ["open the menu.", "Then pick", "export"]);
        let first = &phrases(&spoken)[0];
        assert_eq!(
            (first.start, first.end),
            (Timestamp(S), Timestamp(2_100_000_000))
        );
    }

    #[test]
    fn phrases_sort_words_and_skip_blank_ones() {
        let mut spoken = words(&[(2.0, "world"), (1.6, "hello"), (1.8, "  ")]);
        spoken[0].text = " world ".into();
        let texts: Vec<String> = phrases(&spoken).into_iter().map(|p| p.text).collect();
        assert_eq!(texts, ["hello world"]);
    }

    #[test]
    fn a_phrase_just_before_a_click_belongs_to_it() {
        // Steps at 10 s and 20 s.
        let steps = [at(10), at(20)];
        // Said 1 s before the second click: that click's.
        let parts = split(&words(&[(19.0, "now"), (19.3, "save")]), &steps);
        assert_eq!(parts, ["", "now save"]);
        // Said 3 s before it: the explanation of the first step.
        let parts = split(&words(&[(17.0, "that"), (17.3, "opened")]), &steps);
        assert_eq!(parts, ["that opened", ""]);
    }

    #[test]
    fn speech_outside_the_steps_goes_to_the_first_and_last() {
        let steps = [at(10), at(20)];
        let parts = split(&words(&[(1.0, "hello."), (30.0, "done.")]), &steps);
        assert_eq!(parts, ["hello.", "done."]);
        assert!(split(&words(&[(1.0, "hi")]), &[]).is_empty());
        assert_eq!(split(&[], &steps), ["", ""]);
    }

    #[test]
    fn a_phrase_spanning_a_click_stays_whole_with_the_step_it_began_in() {
        let steps = [at(10), at(12)];
        // Starts at 9.6 s (step 1, within the lead) and runs past 12 s.
        let spoken = words(&[
            (9.6, "click"),
            (10.2, "here"),
            (10.8, "and"),
            (11.4, "wait"),
        ]);
        assert_eq!(split(&spoken, &steps), ["click here and wait", ""]);
    }

    fn step(id: Ulid, description: &str, status: AiStatus) -> Step {
        let mut step = crate::steps::DraftStep {
            kind: StepKind::Click,
            at: Timestamp(0),
            point: None,
            end_point: None,
            element: None,
            app: None,
            keystrokes: None,
            url: None,
            title: "Click **Save**".into(),
        }
        .into_step(
            id,
            StepImage {
                full: format!("steps/{id}.webp"),
                crop: None,
                crop_rect: None,
                after: None,
                show: Default::default(),
            },
        );
        step.description = description.into();
        step.ai_state.status = status;
        step
    }

    #[test]
    fn apply_fills_empty_descriptions_and_keeps_written_ones() {
        let ids: Vec<Ulid> = (1..=4).map(|n| Ulid::from_parts(n, n.into())).collect();
        let mut guide = Guide::new("Narrated", "en");
        guide.items = vec![
            Item::Step(step(ids[0], "", AiStatus::Template)),
            Item::Step(step(ids[1], "Already written.", AiStatus::Ai)),
            Item::Step(step(ids[2], "", AiStatus::Edited)),
            Item::Step(step(ids[3], "", AiStatus::Template)),
        ];
        let times: Vec<(Ulid, Timestamp)> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, at(10 * (i as u64 + 1))))
            .collect();
        let spoken = words(&[
            (9.5, "open the *settings*"),
            (19.5, "this saves it"),
            (29.5, "type a name"),
        ]);
        assert_eq!(apply(&mut guide, &times, &spoken), 3);
        let steps: Vec<&Step> = guide.steps().collect();
        assert_eq!(steps[0].narration, "open the *settings*");
        assert_eq!(steps[0].description, r"Open the \*settings\*.");
        assert_eq!(steps[1].narration, "this saves it");
        assert_eq!(steps[1].description, "Already written.");
        // Edited by hand: the narration is kept, the description is not touched.
        assert_eq!(steps[2].narration, "type a name");
        assert_eq!(steps[2].description, "");
        assert_eq!(steps[3].narration, "");
        assert_eq!(steps[3].description, "");
    }

    #[test]
    fn apply_skips_steps_that_are_gone() {
        let mut guide = Guide::new("Narrated", "en");
        let gone = Ulid::from_parts(9, 9);
        assert_eq!(
            apply(&mut guide, &[(gone, at(1))], &words(&[(1.0, "hi")])),
            0
        );
    }

    #[test]
    fn descriptions_start_with_a_capital_and_end_a_sentence() {
        assert_eq!(description("ça marche"), "Ça marche.");
        assert_eq!(description("is it?"), "Is it?");
        assert_eq!(description("use  the [link]"), r"Use the \[link\].");
        assert_eq!(description("well:"), "Well:");
        assert_eq!(description("  "), "");
        assert_eq!(description("3 times"), "3 times.");
    }
}

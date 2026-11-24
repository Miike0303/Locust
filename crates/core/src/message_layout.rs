//! Display-line layout of game message boxes.
//!
//! RPG Maker and similar engines store a dialogue box as hand-wrapped lines.
//! Source-only machine translators (Google, DeepL, Argos) translate every line
//! as a separate sentence, so a sentence wrapped across lines comes back as
//! fragments. Injectors re-wrap message boxes to the game's width, so those
//! soft wraps can travel joined. A short first line followed by a quoted line
//! names the speaker and keeps its own line.

/// Characters that open a quoted or parenthesized line of dialogue.
const DIALOGUE_OPENERS: &[char] = &['"', '“', '”', '「', '『', '(', '（', '\'', '‘'];

/// Characters that end a sentence, a clause or a quotation; a speaker label
/// never ends with one.
const LINE_CLOSERS: &str = ".,!?…;、。！？\"”」』)）";

/// Longest first line still read as a speaker label, control codes included.
const SPEAKER_LABEL_MAX_CHARS: usize = 32;

/// Whether `line` reads as a speaker label: short, not itself dialogue, and
/// not ending like a sentence or a quotation.
pub fn is_speaker_label(line: &str) -> bool {
    let line = line.trim();
    let (Some(first), Some(last)) = (line.chars().next(), line.chars().next_back()) else {
        return false;
    };
    line.chars().count() <= SPEAKER_LABEL_MAX_CHARS
        && !DIALOGUE_OPENERS.contains(&first)
        && !LINE_CLOSERS.contains(last)
}

/// Whether the first line of `text` names the speaker of the dialogue below it.
pub fn has_speaker_line(text: &str) -> bool {
    let mut lines = text.split('\n');
    match (lines.next(), lines.next()) {
        (Some(first), Some(second)) => {
            is_speaker_label(first) && second.trim_start().starts_with(DIALOGUE_OPENERS)
        }
        _ => false,
    }
}

/// Join soft-wrapped lines so a translator sees whole sentences.
///
/// A break stays when either side is blank, after a speaker line, and when the
/// line ends with anything other than a letter, digit or comma: sentence
/// punctuation, closing quotes, ellipses and protected control tokens keep
/// the author's break. Each joined break becomes one space, so byte offsets
/// into the text stay valid.
pub fn join_soft_wraps(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let speaker = has_speaker_line(text);
    let mut joined = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        joined.push_str(line);
        let Some(next) = lines.get(i + 1) else {
            break;
        };
        let after_speaker = i == 0 && speaker;
        let soft = !after_speaker
            && !next.trim().is_empty()
            && line
                .trim_end()
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || matches!(c, ',' | '、' | '，'));
        joined.push(if soft { ' ' } else { '\n' });
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_a_sentence_wrapped_across_lines() {
        let source = "The old man who lived next to the river\nwas very kind to everyone.";
        let joined = join_soft_wraps(source);
        assert_eq!(
            joined,
            "The old man who lived next to the river was very kind to everyone."
        );
        assert_eq!(joined.len(), source.len());
    }

    #[test]
    fn keeps_the_speaker_line_and_joins_the_dialogue() {
        let source =
            "Karen\n\"Hello. If there are little trailing edge,\nYou are here to work today.\"";
        assert!(has_speaker_line(source));
        assert_eq!(
            join_soft_wraps(source),
            "Karen\n\"Hello. If there are little trailing edge, You are here to work today.\""
        );
    }

    #[test]
    fn keeps_breaks_after_sentences_quotes_ellipses_and_blank_lines() {
        for source in [
            "Hello.\nHow are you?",
            "\"Wait!\"\nShe ran.",
            "I said I couldn't ...\nbut I did.",
            "First paragraph\n\nsecond paragraph",
            "Color {PL_0}\nnext line",
            "single line",
        ] {
            assert_eq!(join_soft_wraps(source), source, "{source:?}");
        }
    }

    #[test]
    fn joins_japanese_soft_wraps_and_lines_ending_in_commas() {
        assert_eq!(join_soft_wraps("今日は、\n雨です。"), "今日は、 雨です。");
        assert_eq!(
            join_soft_wraps("Because there are savings,\neven if you don't care"),
            "Because there are savings, even if you don't care"
        );
    }

    #[test]
    fn speaker_labels_are_short_names_followed_by_dialogue() {
        assert!(is_speaker_label("Karen"));
        assert!(is_speaker_label("Old man"));
        assert!(is_speaker_label("\\C[6]Karen\\C[0]"));
        assert!(is_speaker_label("Karen:"));
        assert!(!is_speaker_label(""));
        assert!(!is_speaker_label("\"Hello"));
        assert!(!is_speaker_label("It was late."));
        assert!(!is_speaker_label(
            "This line is far too long to be anybody's name at all"
        ));
        assert!(has_speaker_line("Aaron\n(Not seen by Karen,"));
        assert!(has_speaker_line("Man\n「Hi」"));
        assert!(!has_speaker_line("Karen"));
        assert!(!has_speaker_line(
            "And that such adventurers\nIt was common."
        ));
    }
}

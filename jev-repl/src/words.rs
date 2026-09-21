//! Word-wise motion, and the keys that ask for it.
//!
//! Alt-← and Alt-→ are what a terminal user reaches for to cross a word, but terminals spell them
//! in more than one way: some send a modified arrow, some report the same keypress with the Meta
//! or Super bit instead of Alt, and some send the readline bindings `Alt-b` and `Alt-f`. All of
//! them mean the same thing here — along with Ctrl-←/→, which is the other common spelling.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Alt, however the terminal reported it.
///
/// `Esc [ 1;3A` is Alt-Up, and `Esc [ 1;9A` is the same keypress from a terminal that reports Alt
/// as Meta — which crossterm hands over as `SUPER`. Reading only `ALT` is what made Alt-arrow look
/// dead in half the terminals it was tried in.
pub fn is_alt(key: &KeyEvent) -> bool {
    key.modifiers
        .intersects(KeyModifiers::ALT | KeyModifiers::META | KeyModifiers::SUPER)
}

fn is_ctrl(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Alt-←, Ctrl-←, or Alt-b: move to the start of the word before the cursor.
pub fn is_word_left(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Left => is_alt(key) || is_ctrl(key),
        KeyCode::Char('b' | 'B') => is_alt(key) && !is_ctrl(key),
        _ => false,
    }
}

/// Alt-→, Ctrl-→, or Alt-f: move past the end of the word after the cursor.
pub fn is_word_right(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Right => is_alt(key) || is_ctrl(key),
        KeyCode::Char('f' | 'F') => is_alt(key) && !is_ctrl(key),
        _ => false,
    }
}

/// Alt-Backspace: delete the word before the cursor.
pub fn is_delete_word_left(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Backspace) && is_alt(key)
}

/// A character key that should be typed, rather than one carrying a modifier we did not bind.
pub fn is_typed(key: &KeyEvent) -> bool {
    !is_alt(key) && !is_ctrl(key)
}

/// The index the cursor lands on moving left by a word: over any spaces, then over the word.
pub fn word_left(chars: &[char], cursor: usize) -> usize {
    let mut i = cursor.min(chars.len());
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].is_whitespace() {
        i -= 1;
    }
    i
}

/// The index the cursor lands on moving right by a word: over any spaces, then over the word.
pub fn word_right(chars: &[char], cursor: usize) -> usize {
    let mut i = cursor.min(chars.len());
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    while i < chars.len() && !chars[i].is_whitespace() {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn crosses_one_word_at_a_time() {
        let c = chars("  the payout failed");
        assert_eq!(word_left(&c, c.len()), 13);
        assert_eq!(word_left(&c, 13), 6);
        assert_eq!(word_left(&c, 6), 2);
        assert_eq!(word_left(&c, 2), 0);
        assert_eq!(word_left(&c, 0), 0);

        assert_eq!(word_right(&c, 0), 5);
        assert_eq!(word_right(&c, 5), 12);
        assert_eq!(word_right(&c, 12), c.len());
        assert_eq!(word_right(&c, c.len()), c.len());
    }

    #[test]
    fn answers_to_every_spelling_of_alt_arrow() {
        let left = |m| KeyEvent::new(KeyCode::Left, m);
        assert!(is_word_left(&left(KeyModifiers::ALT)));
        assert!(is_word_left(&left(KeyModifiers::CONTROL)));
        // The Meta bit some terminals report the same keypress with.
        assert!(is_word_left(&left(KeyModifiers::SUPER)));
        assert!(is_word_left(&left(KeyModifiers::META)));
        assert!(is_word_left(&KeyEvent::new(
            KeyCode::Char('b'),
            KeyModifiers::ALT
        )));
        assert!(is_word_right(&KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::ALT
        )));
        assert!(is_word_right(&KeyEvent::new(
            KeyCode::Char('f'),
            KeyModifiers::ALT
        )));
        assert!(is_delete_word_left(&KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::ALT
        )));

        assert!(!is_word_left(&left(KeyModifiers::NONE)));
        assert!(!is_word_right(&KeyEvent::new(
            KeyCode::Char('f'),
            KeyModifiers::NONE
        )));
        assert!(!is_delete_word_left(&KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE
        )));
        assert!(is_typed(&KeyEvent::new(
            KeyCode::Char('f'),
            KeyModifiers::NONE
        )));
        assert!(!is_typed(&KeyEvent::new(
            KeyCode::Char('f'),
            KeyModifiers::ALT
        )));
    }
}

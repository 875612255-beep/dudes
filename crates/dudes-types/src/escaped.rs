//! Text from elsewhere, made safe to print to a terminal.
//!
//! Here rather than in either binary because both print such text: the CLI prints names, reasons
//! and paths from the daemon or a file, and the daemon prints paths from its environment when it
//! starts or fails to.

use core::fmt::{self, Write as _};

/// Text from the daemon or a file, for a terminal. C0 and C1 controls, DEL, bidirectional formatting
/// characters and the separators U+2028 and U+2029 are written as `\u{…}` escapes. Every other
/// character is written as it is.
///
/// A terminal obeys the controls, so a name could otherwise write the operator's clipboard with
/// OSC 52 or start a forged row with a newline, and a bidirectional override would reorder what
/// the operator reads.
pub struct Escaped<'a>(pub &'a str);

impl fmt::Display for Escaped<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for c in self.0.chars() {
            if c.is_control()
                || matches!(
                    c,
                    '\u{061c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{2028}'
                        | '\u{2029}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
            {
                write!(f, "{}", c.escape_unicode())?;
            } else {
                f.write_char(c)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use super::*;

    #[test]
    fn escaped_line_separators_keep_their_neighbours() {
        assert_eq!(
            Escaped("left\u{2028}middle\u{2029}right é").to_string(),
            r"left\u{2028}middle\u{2029}right é"
        );
    }

    #[test]
    fn every_control_and_bidirectional_format_character_is_escaped() {
        assert_eq!(
            Escaped("\u{0}\u{1b}\n\u{7f}\u{85}\u{9b}\u{61c}\u{200e}\u{200f}\u{202a}\u{202e}\u{2066}\u{2069}é")
                .to_string(),
            r"\u{0}\u{1b}\u{a}\u{7f}\u{85}\u{9b}\u{61c}\u{200e}\u{200f}\u{202a}\u{202e}\u{2066}\u{2069}é"
        );
    }
}

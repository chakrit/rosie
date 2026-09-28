//! Quoting for bash, used by the `--sh` export and the plan file's hint comments.
//!
//! A quoted word never contains a raw control character, so it always stays on one line:
//! a commented-out command cannot spill an executable line, and a TOML comment cannot
//! end early. Nor does it contain a hidden character, so the script the user inspects
//! before running it with sudo shows what it runs.

use std::ops::RangeInclusive;

/// Characters a word may hold and still be written bare, in any position of a command.
/// `=` is left out because `NAME=value` first in a command is an assignment.
fn is_bare(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_@%+:,./-".contains(c)
}

/// Bash reserved words, which bash reads as syntax when one starts a command unquoted.
const RESERVED_WORDS: &[&str] = &[
    "case", "coproc", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in",
    "select", "then", "time", "until", "while",
];

/// Characters that display as nothing, or reorder or break the text around them:
/// Unicode's default-ignorable code points, its format characters (category Cf), and the
/// line and paragraph separators.
const HIDDEN: &[RangeInclusive<char>] = &[
    '\u{ad}'..='\u{ad}',
    '\u{34f}'..='\u{34f}',
    '\u{600}'..='\u{605}',
    '\u{61c}'..='\u{61c}',
    '\u{6dd}'..='\u{6dd}',
    '\u{70f}'..='\u{70f}',
    '\u{890}'..='\u{891}',
    '\u{8e2}'..='\u{8e2}',
    '\u{115f}'..='\u{1160}',
    '\u{17b4}'..='\u{17b5}',
    '\u{180b}'..='\u{180f}',
    '\u{200b}'..='\u{200f}',
    '\u{2028}'..='\u{202e}',
    '\u{2060}'..='\u{206f}',
    '\u{3164}'..='\u{3164}',
    '\u{fe00}'..='\u{fe0f}',
    '\u{feff}'..='\u{feff}',
    '\u{ffa0}'..='\u{ffa0}',
    '\u{fff0}'..='\u{fffb}',
    '\u{110bd}'..='\u{110bd}',
    '\u{110cd}'..='\u{110cd}',
    '\u{13430}'..='\u{1343f}',
    '\u{1bca0}'..='\u{1bca3}',
    '\u{1d173}'..='\u{1d17a}',
    '\u{e0000}'..='\u{e0fff}',
];

/// Whether a character must be written as an escape rather than as itself.
fn needs_escape(c: char) -> bool {
    c.is_control() || HIDDEN.iter().any(|range| range.contains(&c))
}

/// One bash word that expands to exactly `text`, in any position of a command.
///
/// Printable runs go in single quotes, with `'` written as `'\''`. Control and hidden
/// characters go byte by byte in `$'\ooo'` octal escapes, which the bash 3.2 that macOS
/// ships reads.
pub fn word(text: &str) -> String {
    let bare = !text.is_empty() && text.chars().all(is_bare) && !RESERVED_WORDS.contains(&text);
    if bare {
        return text.to_owned();
    }

    let mut quoted = String::new();
    let mut in_quotes = false;
    for c in text.chars() {
        match (needs_escape(c), in_quotes) {
            (true, true) => {
                quoted.push('\'');
                in_quotes = false;
                push_octal_escape(&mut quoted, c);
            }
            (true, false) => push_octal_escape(&mut quoted, c),
            (false, false) => {
                quoted.push('\'');
                in_quotes = true;
                push_quoted_char(&mut quoted, c);
            }
            (false, true) => push_quoted_char(&mut quoted, c),
        }
    }

    match (in_quotes, quoted.is_empty()) {
        (true, _) => quoted.push('\''),
        (false, true) => quoted.push_str("''"),
        (false, false) => {}
    }
    quoted
}

/// Text for a `#` comment, with control and hidden characters shown as escapes instead
/// of written.
pub fn comment_text(text: &str) -> String {
    text.chars()
        .map(|c| match needs_escape(c) {
            true => c.escape_default().to_string(),
            false => c.to_string(),
        })
        .collect()
}

/// Writes a character inside an open single-quoted run.
fn push_quoted_char(quoted: &mut String, c: char) {
    match c {
        '\'' => quoted.push_str("'\\''"),
        _ => quoted.push(c),
    }
}

/// Writes a control or hidden character as `$'\ooo'`, one escape per UTF-8 byte.
fn push_octal_escape(quoted: &mut String, c: char) {
    let mut buffer = [0; 4];
    quoted.push_str("$'");
    for byte in c.encode_utf8(&mut buffer).bytes() {
        quoted.push_str(&format!("\\{byte:03o}"));
    }
    quoted.push('\'');
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    /// What bash expands `word(text)` to, by printing its single argument.
    fn bash_expansion(text: &str) -> String {
        let script = format!("printf '%s' {}", word(text));
        let output = Command::new("/bin/bash")
            .arg("-c")
            .arg(&script)
            .env_clear()
            .output()
            .expect("run /bin/bash");
        assert!(output.status.success(), "bash rejected: {script}");
        String::from_utf8(output.stdout).expect("utf-8 output")
    }

    #[test]
    fn plain_words_stay_bare() {
        assert_eq!(
            word("/Users/me/src/app/node_modules"),
            "/Users/me/src/app/node_modules"
        );
        assert_eq!(word("--force"), "--force");
    }

    #[test]
    fn hostile_paths_expand_back_to_themselves() {
        let hostile = [
            "",
            "/w/my app/target",
            "/w/it's/a \"quote\"",
            "/w/$HOME/`id`/$(id)/*",
            "/w/new\nline",
            "/w/trailing newline\n",
            "/w/tab\tand\rcr\u{1b}[31m",
            "/w/semi;colon&amp|pipe>out<in",
            "/w/back\\slash!bang~tilde#hash",
            "/w/ünïcødé/日本",
            "/w/c1\u{85}next",
            HIDDEN,
            "FOO=1",
            "time",
            "%1",
            "'",
            "''\n'",
        ];

        for text in hostile {
            assert_eq!(bash_expansion(text), text, "quoted as {}", word(text));
        }
    }

    /// Characters that display as nothing, or reorder or break the text around them.
    const HIDDEN: &str = "/w/\u{202e}gpj.sh\u{202c}/zero\u{200b}width\u{2060}\u{feff}/line\u{2028}para\u{2029}/soft\u{ad}hyphen/tag\u{e0041}/isolate\u{2066}x\u{2069}";

    fn is_shown_truthfully(text: &str) -> bool {
        text.chars().all(|c| c.is_ascii_graphic() || c == ' ')
    }

    #[test]
    fn quoted_words_never_hold_a_control_or_hidden_character() {
        let quoted = word(&format!("/w/new\nline\r\u{7f}\u{85}{HIDDEN}"));

        assert!(is_shown_truthfully(&quoted), "{quoted:?}");
    }

    #[test]
    fn comment_text_stays_on_one_line() {
        let shown = comment_text("Remove \"X\"\nrm -rf ~");

        assert_eq!(shown, "Remove \"X\"\\nrm -rf ~");
    }

    #[test]
    fn comment_text_shows_hidden_characters_as_escapes() {
        let shown = comment_text(HIDDEN);

        assert!(is_shown_truthfully(&shown), "{shown:?}");
    }

    #[test]
    fn visible_unicode_stays_readable() {
        assert_eq!(word("/w/日本/ünï"), "'/w/日本/ünï'");
        assert_eq!(comment_text("日本 ünï"), "日本 ünï");
    }
}

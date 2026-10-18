//! The look of what rosie prints (`docs/spec/cli.md#output`): every stderr line's marker
//! in the first column, the column where `inquire` puts its `?` and `indicatif` its
//! spinner; the plan listing on stdout; and one palette for all of them.
//!
//! Color is decided once per stream, from whether it is a terminal and the user's
//! `NO_COLOR`; nothing here asks `console` or the environment, so a [`Palette`] is the
//! whole decision.

use console::Style;
use indicatif::ProgressStyle;
use inquire::ui::{Color, RenderConfig, StyleSheet, Styled};

/// What kind of line a stderr line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Note,
    Done,
    Error,
    Warning,
    Skipped,
}

impl Mark {
    pub fn glyph(self) -> &'static str {
        match self {
            Mark::Note => "·",
            Mark::Done => "✓",
            Mark::Error => "✗",
            Mark::Warning => "!",
            Mark::Skipped => "-",
        }
    }

    fn style(self) -> Style {
        match self {
            Mark::Note => Style::new().dim(),
            Mark::Done => Style::new().green(),
            Mark::Error => Style::new().red(),
            Mark::Warning | Mark::Skipped => Style::new().yellow(),
        }
    }
}

/// How loudly a plan listing note says why its item is not cleaned
/// (`docs/spec/plan.md#plan-listing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Left alone as asked: keep, skipped.
    Calm,
    /// Waits on the user: a conflict, a running process.
    Caution,
    /// Refused by the safety gate: outside the roots.
    Alarm,
}

impl Tone {
    fn style(self) -> Style {
        match self {
            Tone::Calm => Style::new().dim(),
            Tone::Caution => Style::new().yellow(),
            Tone::Alarm => Style::new().red(),
        }
    }
}

/// One stderr line before it is styled: its mark and its text, which may span several
/// lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub mark: Mark,
    pub text: String,
}

impl Line {
    pub fn new(mark: Mark, text: impl Into<String>) -> Self {
        Line {
            mark,
            text: text.into(),
        }
    }
}

/// The user's `NO_COLOR` (<https://no-color.org>): set and not empty turns color off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorChoice {
    Auto,
    Never,
}

/// How one stream's lines, spinners, and prompts look: colored or plain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    color: bool,
}

/// The spinner frames, as `indicatif`'s default spinner draws them; the last is shown
/// once it finishes.
const SPINNER_FRAMES: [&str; 9] = ["⠁", "⠂", "⠄", "⡀", "⢀", "⠠", "⠐", "⠈", " "];

/// Continuation lines of a multi-line text start under the text, past the marker.
const CONTINUATION: &str = "\n  ";

impl Palette {
    /// Colored only when the stream is a terminal and the user did not opt out.
    pub fn for_stream(is_terminal: bool, choice: ColorChoice) -> Palette {
        let color = match (is_terminal, choice) {
            (true, ColorChoice::Auto) => true,
            (true, ColorChoice::Never) | (false, _) => false,
        };
        Palette { color }
    }

    pub fn plain() -> Palette {
        Palette { color: false }
    }

    /// `line`'s marker, a space, then its text; each further line of the text indented
    /// under the first.
    pub fn line(self, line: &Line) -> String {
        let mark = self.paint(line.mark.style(), line.mark.glyph());
        let text = line.text.replace('\n', CONTINUATION);
        format!("{mark} {text}")
    }

    /// A folder heading, in the color `ls` gives folders.
    pub fn folder(self, text: &str) -> String {
        self.paint(Style::new().blue(), text)
    }

    /// A note's status words, as loud as `tone`.
    pub fn tone(self, tone: Tone, text: &str) -> String {
        self.paint(tone.style(), text)
    }

    /// A figure that should catch the eye.
    pub fn strong(self, text: &str) -> String {
        self.paint(Style::new().bold(), text)
    }

    /// Detail the eye may skip: a listing's rules and notes.
    pub fn quiet(self, text: &str) -> String {
        self.paint(Style::new().dim(), text)
    }

    /// What a hint does, shown above its command.
    pub fn hint_label(self, text: &str) -> String {
        self.paint(Style::new().dim(), text)
    }

    /// A command the user can type: the color `inquire` gives an answer.
    pub fn hint_command(self, text: &str) -> String {
        self.paint(Style::new().cyan(), text)
    }

    /// A spinner style for `template`, which must start with `{spinner}` so the spinner
    /// takes the marker column.
    pub fn spinner(self, template: &str) -> ProgressStyle {
        let frames: Vec<String> = SPINNER_FRAMES
            .iter()
            .map(|frame| self.paint(Style::new().cyan(), frame))
            .collect();
        let frames: Vec<&str> = frames.iter().map(String::as_str).collect();
        ProgressStyle::with_template(template)
            .unwrap_or_else(|error| unreachable!("spinner templates are fixed and valid: {error}"))
            .tick_strings(&frames)
    }

    /// `inquire`'s look: `?` in the marker column, answers and the highlighted option in
    /// the hint-command color.
    pub fn prompts(self) -> RenderConfig<'static> {
        match self.color {
            false => RenderConfig::empty(),
            true => RenderConfig::default_colored()
                .with_prompt_prefix(Styled::new("?").with_fg(Color::DarkGreen))
                .with_answered_prompt_prefix(Styled::new("?").with_fg(Color::DarkGreen))
                .with_answer(StyleSheet::new().with_fg(Color::DarkCyan))
                .with_highlighted_option_prefix(Styled::new(">").with_fg(Color::DarkCyan))
                .with_selected_option(Some(StyleSheet::new().with_fg(Color::DarkCyan)))
                .with_selected_checkbox(Styled::new("[x]").with_fg(Color::DarkGreen))
                .with_help_message(StyleSheet::new().with_fg(Color::DarkGrey)),
        }
    }

    fn paint(self, style: Style, text: &str) -> String {
        match self.color {
            true => style.force_styling(true).apply_to(text).to_string(),
            false => text.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_only_a_terminal_whose_user_did_not_opt_out() {
        let decisions = [
            (true, ColorChoice::Auto, true),
            (true, ColorChoice::Never, false),
            (false, ColorChoice::Auto, false),
            (false, ColorChoice::Never, false),
        ];
        for (terminal, choice, colored) in decisions {
            let line = Palette::for_stream(terminal, choice).line(&Line::new(Mark::Done, "x"));

            assert_eq!(line.contains('\u{1b}'), colored, "{terminal} {choice:?}");
        }
    }

    #[test]
    fn a_multi_line_text_continues_under_the_text() {
        let line = Palette::plain().line(&Line::new(Mark::Error, "first\nsecond"));

        assert_eq!(line, "✗ first\n  second");
    }
}

//! Patterns drawn in the side panel rather than written in the editor.
//!
//! A graphical pattern is a name and a row of beats, and it earns its keep by
//! being *the same thing* a list literal is. That used to be a resemblance —
//! the panel's rows were seeded into the environment as `Value::List`s. Now it
//! is literal: the panel writes `patterns.scree` in the project folder, one
//! `let` per row, and every program in the project has that file folded into it
//! the way an explicit `use` would fold in any other.
//!
//! So this module is a translation, both ways. [`to_source`] writes what the
//! panel holds; [`from_source`] reads a file back into rows the grid can draw.
//! Everything between the two is the ordinary language: the parser reads those
//! `let`s, the lowerer builds the lists, and nothing downstream — `to_pattern`,
//! the scheduler, the lanes — has any idea a grid was involved.

use crate::diagnostic::{Diagnostic, Stage};
use crate::lang::{self, NoteName};
use crate::parser::parser::{parse, Expr, ListItem, ScreeItem};

/// What the panel's file is called, in every project.
pub const FILE: &str = "patterns.scree";

/// What a step is. Pitches carry a MIDI note number; the other two are the same
/// silence and bare hit the language spells `` ` `` and `\`.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StepKind {
    Rest,
    Trigger,
    Pitch { note: f64 },
}

/// The length a step has when nothing says otherwise — one grid cell.
fn unit_length() -> f64 {
    1.0
}

/// One step: what it is, and how many grid cells it covers.
///
/// `length` is in cells rather than bars, which is what makes the grid and
/// the language agree without either doing arithmetic: a note drawn four cells
/// wide is written `;4`, and the cells in a row always sum to the resolution
/// the row was drawn at.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
pub struct GraphicalStep {
    #[serde(flatten)]
    pub kind: StepKind,
    #[serde(default = "unit_length")]
    pub length: f64,
}

impl GraphicalStep {
    /// A step of one cell, which is every step the flat grid ever drew.
    pub fn new(kind: StepKind) -> GraphicalStep {
        GraphicalStep { kind, length: unit_length() }
    }

    pub fn sized(kind: StepKind, length: f64) -> GraphicalStep {
        GraphicalStep { kind, length }
    }
}

/// Which editor a pattern is drawn in.
///
/// Stored rather than inferred because an empty pattern — which is what every
/// new one is — has no pitches or triggers to infer from, and a composer that
/// opened new patterns in the wrong editor every time would be maddening.
#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Pitched notes on a piano roll.
    #[default]
    Roll,
    /// Bare triggers, for an instrument that takes no pitch.
    Drums,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Roll => "roll",
            Mode::Drums => "drums",
        }
    }

    fn parse(s: &str) -> Option<Mode> {
        match s {
            "roll" => Some(Mode::Roll),
            "drums" => Some(Mode::Drums),
            _ => None,
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
pub struct GraphicalPattern {
    pub name: String,
    pub steps: Vec<GraphicalStep>,
    #[serde(default)]
    pub mode: Mode,
}

impl GraphicalPattern {
    /// How many cells wide the grid is: the lengths always sum to it.
    ///
    /// Derived rather than stored, because storing it would let it disagree
    /// with the row it describes — and a resolution that contradicts the
    /// lengths is not recoverable from, whereas this can only ever be right.
    pub fn resolution(&self) -> f64 {
        self.steps.iter().map(|s| s.length).sum()
    }
}

/// The header the panel's file carries.
///
/// It is a generated file that people will open — it is scree, it sits in their
/// project, and the project panel lists it — so it has to say what it is and
/// what will happen to anything else they put in it.
const HEADER: &str = "\
// Patterns drawn in the side panel.
//
// The panel rewrites this file whenever a row changes, so anything else kept
// here will be lost. Every file in this project can name these patterns
// without importing them.
";

/// The marker that carries what the file cannot otherwise say.
///
/// Which editor drew a row is not expressible in scree — it is not a property
/// of the pattern, it is a property of how someone chose to look at it. A
/// comment is where that belongs: the file stays entirely ordinary scree, the
/// compiler never sees this, and a row whose marker is missing or unreadable
/// still opens, just in whichever editor its contents suggest.
const MODE_MARKER: &str = "// composer: ";

impl GraphicalPattern {
    /// One row, as the `let` the language would have been given, under the
    /// comment that says which editor drew it.
    fn to_source(&self) -> String {
        let steps: Vec<String> = self.steps.iter().map(step_source).collect();
        format!(
            "{MODE_MARKER}{}\nlet {} = [{}]",
            self.mode.as_str(),
            self.name,
            steps.join(", "),
        )
    }
}

/// Every row, as a file.
pub fn to_source(patterns: &[GraphicalPattern]) -> String {
    let mut out = String::from(HEADER);
    for pattern in patterns {
        out.push('\n');
        out.push_str(&pattern.to_source());
        out.push('\n');
    }
    out
}

/// One beat, spelled the way it would be written by hand: a note name where
/// there is one, so the file reads as music rather than as MIDI.
fn step_source(step: &GraphicalStep) -> String {
    let value = match &step.kind {
        StepKind::Rest => "`".to_string(),
        StepKind::Trigger => "\\".to_string(),
        StepKind::Pitch { note } => note_source(*note),
    };
    // A length of one is the default, so writing it would be noise — and a row
    // drawn at no particular resolution then reads exactly as it always did.
    if step.length == 1.0 {
        return value;
    }
    format!("{value};{}", format_number(step.length))
}

fn note_source(note: f64) -> String {
    /// Sharps rather than flats, matching `lang::note`'s own spelling.
    const NAMES: [&str; 12] = [
        "c", "cs", "d", "ds", "e", "f", "fs", "g", "gs", "a", "as", "b",
    ];

    // Only whole notes inside the range note names cover; anything else is a
    // number, which the language reads just as well.
    let whole = note.fract() == 0.0 && note >= 12.0 && note <= 127.0;
    if !whole {
        return format_number(note);
    }

    let midi = note as i64;
    format!("{}{}", NAMES[(midi % 12) as usize], midi / 12 - 1)
}

/// A number without a trailing `.0`, since the file is read by people.
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    format!("{n}")
}

/// Read a patterns file back into rows.
///
/// Anything the grid cannot draw is passed over rather than refused: the file
/// is ordinary scree and somebody may well have written something in it that a
/// row of cells has no way to show. It still compiles — the whole file is
/// included in every program — it simply is not drawn.
///
/// A file that does not parse *is* refused, because the alternative is a panel
/// that silently shows nothing and then overwrites what it could not read.
pub fn from_source(code: &str) -> Result<Vec<GraphicalPattern>, Diagnostic> {
    let items = parse(code.to_string())?;
    let modes = modes_in(code);

    let mut patterns = Vec::new();
    for item in &items {
        let ScreeItem::Let { name, value: Expr::List(steps) } = item else {
            continue;
        };
        let mut octave = None;
        let Some(steps) = steps
            .iter()
            .map(|step| parse_step(step, &mut octave))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let name = name.0.clone();
        // A hand-written row has no marker, so fall back to what it looks like:
        // anything pitched is a roll, and bare hits are drums.
        let mode = modes.get(&name).copied().unwrap_or_else(|| infer_mode(&steps));
        patterns.push(GraphicalPattern { name, steps, mode });
    }

    Ok(patterns)
}

/// Which editor each row was drawn in, read off the comments.
///
/// A text pass rather than part of the parse, because the lexer drops comments
/// before the parser ever sees them — and it should, since this is not part of
/// the program. A marker applies to the next `let` below it, which is the only
/// arrangement [`GraphicalPattern::to_source`] ever writes.
fn modes_in(code: &str) -> std::collections::HashMap<String, Mode> {
    let mut out = std::collections::HashMap::new();
    let mut pending: Option<Mode> = None;

    for line in code.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(MODE_MARKER) {
            // An unreadable marker is not an error: the row still opens, it
            // just opens wherever its contents suggest.
            pending = Mode::parse(rest.trim());
            continue;
        }
        if let Some(rest) = line.strip_prefix("let ") {
            if let Some(mode) = pending.take() {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.insert(name, mode);
                }
            }
            continue;
        }
        // Anything between a marker and its `let` breaks the association: the
        // marker was for something else, or for nothing.
        if !line.is_empty() && !line.starts_with("//") {
            pending = None;
        }
    }
    out
}

/// Which editor a row without a marker belongs in.
fn infer_mode(steps: &[GraphicalStep]) -> Mode {
    if steps.iter().any(|s| matches!(s.kind, StepKind::Pitch { .. })) {
        Mode::Roll
    } else if steps.iter().any(|s| matches!(s.kind, StepKind::Trigger)) {
        Mode::Drums
    } else {
        Mode::default()
    }
}

/// One element of a list, as a beat — or `None` when it is something a cell
/// cannot hold, like a nested list or an expression.
/// `octave` is the register the language keeps while reading a sequence — set
/// by every note that spells one, read by every note that does not. The panel
/// has to keep it too: a row written `[a1, a, a, a]` is four `a1` cells, and a
/// grid that could not read it would leave the row undrawn for no reason the
/// person looking at the file could see.
fn parse_step(item: &ListItem, octave: &mut Option<i32>) -> Option<GraphicalStep> {
    // A length is drawable now — it is how wide the note is — but only a plain
    // number is. `[c4;beats]` is a length the grid cannot show a cell for,
    // since it does not know what `beats` is; the row is passed over rather
    // than guessed at.
    //
    // Written note values fall out here too, and must. The grid is cells, which
    // are shares of a row — a `;q` is a length of time and has no cell count to
    // be. Reading one as the number 1 would silently restripe the rhythm and
    // then write the flattened version back over what was there.
    let length = match item.length.as_deref() {
        None => 1.0,
        Some(Expr::Num(n)) if *n > 0.0 && n.is_finite() => *n,
        Some(_) => return None,
    };

    let kind = match &item.value {
        Expr::Rest => StepKind::Rest,
        Expr::Trigger => StepKind::Trigger,
        Expr::Num(n) => StepKind::Pitch { note: *n },
        // A note name is a plain variable to the parser; the lowerer only reads
        // it as a pitch when nothing else has that name, and so do we.
        Expr::Var(name) => match lang::note(&name.0) {
            NoteName::Note { midi, octave: spelled } => {
                *octave = Some(spelled);
                StepKind::Pitch { note: midi }
            }
            // A written value is also a note letter in one case — `e` — and the
            // language refuses that step rather than choosing. Undrawable
            // either way, so the row simply is not shown.
            NoteName::PitchClass(offset) if lang::duration(&name.0).is_none() => {
                StepKind::Pitch { note: lang::in_octave(offset, (*octave)?) }
            }
            _ => return None,
        },
        _ => return None,
    };
    Some(GraphicalStep::sized(kind, length))
}

/// Refuse a name the panel could not write and read back.
///
/// The panel enforces this while you type, so a failure here means something
/// got past it — worth an error rather than a file with an unusable name in it.
pub fn check_names(patterns: &[GraphicalPattern]) -> Result<(), Diagnostic> {
    for (i, p) in patterns.iter().enumerate() {
        let mut chars = p.name.chars();
        let ok = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ok {
            return Err(Diagnostic::message(
                Stage::Import,
                format!(
                    "'{}' is not a usable pattern name: start with a letter or \
                     underscore, then letters, digits or underscores",
                    p.name
                ),
            ));
        }
        if patterns[..i].iter().any(|q| q.name == p.name) {
            return Err(Diagnostic::message(
                Stage::Import,
                format!("two graphical patterns are both named '{}'", p.name),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lowerer::lower::lower;
    use crate::pattern::pattern::{Pattern, Step};

    fn pat(name: &str, steps: Vec<GraphicalStep>) -> GraphicalPattern {
        GraphicalPattern { name: name.to_string(), steps, mode: Mode::default() }
    }

    fn pitch(note: f64) -> GraphicalStep {
        GraphicalStep::new(StepKind::Pitch { note })
    }

    /// The claim this module rests on: a drawn row and the list a person would
    /// have typed are the same program.
    #[test]
    fn a_drawn_pattern_is_the_list_it_stands_for() {
        let source = to_source(&[pat(
            "riff",
            vec![pitch(60.0), GraphicalStep::new(StepKind::Rest), GraphicalStep::new(StepKind::Trigger)],
        )]);

        let items = parse(format!("fn kick(f) = sin(f)\n{source}\nplay(riff, kick)\n"))
            .expect("the written file should parse");
        let lowered = lower(&items).expect("and lower");

        assert_eq!(
            lowered.bindings[0].pattern,
            Pattern::seq(vec![Step::Value(60.0), Step::Rest, Step::Value(1.0)]),
        );
    }

    /// Written as music: the file is meant to be read, and `c4` says what `60`
    /// only means.
    #[test]
    fn pitches_are_written_as_note_names() {
        let source = to_source(&[pat("riff", vec![pitch(60.0), pitch(61.0), pitch(127.0)])]);
        assert!(source.contains("let riff = [c4, cs4, g9]"), "got:\n{source}");
    }

    /// Except where there is no name to write: the panel takes a MIDI number,
    /// and a number is what comes back out.
    #[test]
    fn unnameable_pitches_stay_numbers() {
        let source = to_source(&[pat("odd", vec![pitch(60.5), pitch(4.0)])]);
        assert!(source.contains("let odd = [60.5, 4]"), "got:\n{source}");
    }

    /// What the panel writes, the panel reads.
    #[test]
    fn a_written_file_reads_back_the_same() {
        let patterns = vec![
            pat("hats", vec![GraphicalStep::new(StepKind::Trigger), GraphicalStep::new(StepKind::Rest)]),
            pat("riff", vec![pitch(60.0), pitch(60.5), GraphicalStep::new(StepKind::Rest)]),
            pat("empty", vec![]),
        ];
        assert_eq!(from_source(&to_source(&patterns)).unwrap(), patterns);
    }

    /// And what a person writes, the panel reads: both spellings of a pitch,
    /// and both of a step.
    #[test]
    fn a_hand_written_file_is_read_too() {
        let read = from_source("let hats = [\\, `, ef3, 62]\n").expect("should read");
        assert_eq!(
            read,
            vec![pat(
                "hats",
                vec![
                    GraphicalStep::new(StepKind::Trigger),
                    GraphicalStep::new(StepKind::Rest),
                    pitch(51.0),
                    pitch(62.0),
                ]
            )]
        );
    }

    /// A row that carries its octave reads as the notes it stands for. The
    /// panel keeps the same register the language does, so a hand-written
    /// melody is drawable rather than passed over for a rule it obeys.
    #[test]
    fn a_hand_written_row_may_carry_its_octave() {
        let read = from_source("let riff = [c4, ef, g, c5]\n").expect("should read");
        assert_eq!(
            read,
            vec![pat("riff", vec![pitch(60.0), pitch(63.0), pitch(67.0), pitch(72.0)])]
        );
        // Drawing over it spells every octave out again: a grid is cells, and
        // has nowhere to keep a register. Sharps, because the cells are
        // semitones and only one spelling can come back out of a number.
        assert!(
            to_source(&read).contains("let riff = [c4, ds4, g4, c5]"),
            "got {}", to_source(&read),
        );
    }

    /// A row whose first note has no octave has no register to read, so there
    /// is nothing to draw and the row is passed over — the same answer the
    /// language gives, which is an error rather than a default octave.
    #[test]
    fn a_row_that_never_spells_an_octave_is_passed_over() {
        let read = from_source("let hats = [\\, `]\nlet riff = [a, b, c]\n").expect("should read");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].name, "hats");
    }

    /// A file may hold more than a grid can draw. It still compiles — every
    /// program in the project includes the whole of it — so the panel passes
    /// over what it cannot show rather than refusing the file.
    #[test]
    fn what_the_grid_cannot_draw_is_passed_over() {
        let read = from_source(
            "let hats = [\\, `]\nfn kick(f) = sin(f)\nlet nested = [[1, 2], 3]\nlet n = 4\n",
        )
        .expect("should read");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].name, "hats");
    }

    /// A file that does not parse is refused, and says why: the panel showing
    /// nothing and then writing over it is how work disappears.
    #[test]
    fn a_broken_file_is_refused() {
        let err = from_source("let hats = [\\, `\nfn oops(").expect_err("should not read");
        assert!(matches!(err.stage, Stage::Parse | Stage::Lex), "{err}");
    }

    /// Empty in, empty out — and a file that is only a header, so a project
    /// with no patterns still has an honest file rather than a stale one.
    #[test]
    fn no_patterns_is_a_file_with_no_patterns() {
        let source = to_source(&[]);
        assert_eq!(from_source(&source).unwrap(), Vec::new());
        assert!(source.starts_with("// Patterns drawn"));
    }

    #[test]
    fn bad_names_are_refused() {
        assert!(check_names(&[pat("2fast", vec![])]).is_err());
        assert!(check_names(&[pat("has space", vec![])]).is_err());
        assert!(check_names(&[pat("", vec![])]).is_err());
        assert!(check_names(&[pat("hats_2", vec![])]).is_ok());
    }

    #[test]
    fn duplicate_names_are_refused() {
        let err = check_names(&[pat("hats", vec![]), pat("hats", vec![])])
            .expect_err("duplicates must not be written");
        assert!(err.message.contains("hats"), "{err}");
    }

    /// The exact shape `src/patterns.ts` sends. Worth pinning: the two sides
    /// only ever meet over this JSON, and a renamed tag would fail at the one
    /// moment it cannot be caught — someone's eval, mid-performance.
    #[test]
    fn the_wire_format_is_what_the_panel_sends() {
        let json = r#"[{"name":"hats","steps":[
            {"kind":"trigger"},{"kind":"pitch","note":54},{"kind":"rest"}]}]"#;
        let patterns: Vec<GraphicalPattern> = serde_json::from_str(json).expect("wire format");

        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].name, "hats");

        // All the way through: the JSON the panel sent, written as a file,
        // read as a program, and played as the rhythm that was drawn.
        let source = to_source(&patterns);
        let items = parse(format!("fn tone(f) = sin(f)\n{source}\nplay(hats, tone)\n"))
            .expect("should parse");
        let lowered = lower(&items).expect("should lower");
        assert_eq!(
            lowered.bindings[0].pattern,
            Pattern::seq(vec![Step::Value(1.0), Step::Value(54.0), Step::Rest]),
        );
    }

    /// And the shape it reads back, which the panel deserializes.
    #[test]
    fn rows_serialize_for_the_panel() {
        let json = serde_json::to_string(&[pat("hats", vec![GraphicalStep::new(StepKind::Trigger), pitch(54.0)])])
            .expect("should serialize");
        assert_eq!(
            json,
            r#"[{"name":"hats","steps":[{"kind":"trigger","length":1.0},{"kind":"pitch","note":54.0,"length":1.0}],"mode":"roll"}]"#
        );
    }
}

#[cfg(test)]
mod length_tests {
    use super::*;
    use crate::pattern::pattern::Pattern;

    /// A length is a note's width, so the grid draws it rather than passing
    /// the row over — the composer's whole output depends on this.
    #[test]
    fn a_row_with_lengths_is_drawn() {
        let read = from_source("let riff = [c4;3, e4]\n").expect("should read");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].steps[0].length, 3.0);
        assert_eq!(read[0].steps[1].length, 1.0, "an absent `;` is one cell");
        assert_eq!(read[0].resolution(), 4.0, "the cells sum to the grid width");
    }

    /// A length the grid has no cell for is still refused: it cannot show a
    /// width it cannot compute, and guessing would be written back as fact.
    #[test]
    fn a_row_with_a_computed_length_is_not_drawn() {
        let read = from_source("let beats = 3\nlet riff = [c4;beats, e4]\nlet hats = [\\, `]\n")
            .expect("the file should still read");
        assert_eq!(read.len(), 1, "only the drawable row: {read:?}");
        assert_eq!(read[0].name, "hats");
    }

    /// And what it cannot draw, it still compiles — the whole file is folded
    /// into every program in the project whether the grid shows it or not.
    #[test]
    fn a_row_with_lengths_still_plays() {
        use crate::lowerer::lower::lower;

        let src = "fn tone(n) = sin(n)\nlet riff = [c4;3, e4]\nplay(riff, tone)\n";
        let items = parse(src.to_string()).expect("should parse");
        let lowered = lower(&items).expect("should lower");
        let Pattern::Steps(slots) = &lowered.bindings[0].pattern else {
            panic!("expected a sequence");
        };
        assert_eq!(slots[0].length, 3.0);
    }
}

/// The file format the composer reads and writes.
#[cfg(test)]
mod composer_format_tests {
    use super::*;

    fn pat(name: &str, mode: Mode, steps: Vec<GraphicalStep>) -> GraphicalPattern {
        GraphicalPattern { name: name.to_string(), steps, mode }
    }

    fn pitch(note: f64, length: f64) -> GraphicalStep {
        GraphicalStep::sized(StepKind::Pitch { note }, length)
    }

    /// A drawn note's width is written as the `;` the language reads.
    #[test]
    fn widths_are_written_as_lengths() {
        let src = to_source(&[pat(
            "riff",
            Mode::Roll,
            vec![pitch(60.0, 2.0), GraphicalStep::sized(StepKind::Rest, 2.0), pitch(64.0, 4.0)],
        )]);
        assert!(src.contains("let riff = [c4;2, `;2, e4;4]"), "got:\n{src}");
    }

    /// A one-cell step writes no `;` at all, so a row drawn at no particular
    /// resolution reads exactly as it did before the composer existed.
    #[test]
    fn one_cell_steps_stay_bare() {
        let src = to_source(&[pat(
            "hats",
            Mode::Drums,
            vec![GraphicalStep::new(StepKind::Trigger), GraphicalStep::new(StepKind::Rest)],
        )]);
        assert!(src.contains("let hats = [\\, `]"), "got:\n{src}");
    }

    /// Which editor drew a row survives the trip through the file.
    #[test]
    fn the_mode_round_trips() {
        let rows = vec![
            pat("riff", Mode::Roll, vec![pitch(60.0, 4.0)]),
            pat("hats", Mode::Drums, vec![GraphicalStep::sized(StepKind::Trigger, 2.0)]),
        ];
        assert_eq!(from_source(&to_source(&rows)).unwrap(), rows);
    }

    /// The marker is a comment, so it is not part of the program — the file
    /// still compiles to exactly the pattern that was drawn.
    #[test]
    fn the_marker_is_invisible_to_the_compiler() {
        use crate::lowerer::lower::lower;
        use crate::pattern::pattern::Pattern;

        let src = to_source(&[pat("riff", Mode::Roll, vec![pitch(60.0, 3.0), pitch(64.0, 1.0)])]);
        let items = parse(format!("fn tone(n) = sin(n)\n{src}\nplay(riff, tone)\n"))
            .expect("should parse");
        let lowered = lower(&items).expect("should lower");
        let Pattern::Steps(slots) = &lowered.bindings[0].pattern else {
            panic!("expected a sequence");
        };
        assert_eq!(slots[0].length, 3.0);
        assert_eq!(slots[1].length, 1.0);
    }

    /// A hand-written row has no marker, so the mode comes from what is in it.
    /// Pitches are a roll; bare hits are drums.
    #[test]
    fn a_row_without_a_marker_infers_its_mode() {
        let read = from_source("let riff = [c4, e4]\nlet hats = [\\, `]\n").expect("should read");
        assert_eq!(read[0].mode, Mode::Roll);
        assert_eq!(read[1].mode, Mode::Drums);
    }

    /// An unreadable marker is not an error. The row still opens — the marker
    /// is a convenience, and losing it should never lose the pattern.
    #[test]
    fn an_unreadable_marker_falls_back_to_inference() {
        let read = from_source("// composer: xylophone\nlet hats = [\\, `]\n")
            .expect("should read");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].mode, Mode::Drums);
    }

    /// A marker only speaks for the `let` directly under it, so one left
    /// stranded by an edit cannot claim a row it was never about.
    #[test]
    fn a_stranded_marker_claims_nothing() {
        let read = from_source("// composer: drums\nfn tone(n) = sin(n)\nlet riff = [c4]\n")
            .expect("should read");
        assert_eq!(read[0].mode, Mode::Roll, "inferred, not the stranded marker");
    }

    /// The grid's width is the sum of its cells, which is why it is derived
    /// rather than stored: it cannot disagree with the row it describes.
    #[test]
    fn resolution_is_the_sum_of_the_cells() {
        let row = pat("riff", Mode::Roll, vec![pitch(60.0, 4.0), pitch(64.0, 12.0)]);
        assert_eq!(row.resolution(), 16.0);
    }

    /// A row written in note values is not a grid row — cells are shares, and a
    /// `;q` is a length of time with no cell count to be. It is passed over
    /// rather than flattened, so the panel never rewrites it as the wrong
    /// rhythm. The same holds for a bare `q`, which is a value in the step
    /// position rather than a note.
    ///
    /// The cost is that the panel does not *show* such a row, and writing from
    /// the panel drops it — so metrical patterns belong in a file of their own
    /// until the grid learns to draw them.
    #[test]
    fn a_row_written_in_note_values_is_not_a_grid_row() {
        for src in ["let riff = [c4;q, e4, g4]\n", "let hits = [q, q, q]\n"] {
            let rows = from_source(src).expect("the file still parses");
            assert!(rows.is_empty(), "{src} should not have been read as a grid: {rows:?}");
        }
    }

    /// JSON from a panel that predates lengths still reads: an absent length is
    /// one cell, and an absent mode is a roll.
    #[test]
    fn the_old_wire_format_still_deserializes() {
        let json = r#"[{"name":"hats","steps":[{"kind":"trigger"},{"kind":"rest"}]}]"#;
        let rows: Vec<GraphicalPattern> = serde_json::from_str(json).expect("should read");
        assert_eq!(rows[0].steps[0].length, 1.0);
        assert_eq!(rows[0].mode, Mode::Roll);
    }
}

//! What the editor is told when a program does not compile.
//!
//! Every pass between the source text and a running graph refuses in its own
//! vocabulary, and until now all of them arrived at the frontend as a bare
//! string that nothing displayed. A `Diagnostic` is that refusal in a shape the
//! editor can act on: which pass rejected the program, what it said, and — when
//! the pass still knows — where in the source to point.

use serde::Serialize;
use std::fmt;

/// Which pass refused the program.
///
/// Worth carrying separately from the message because the passes fail at
/// different distances from the text: a `Parse` error is at a character, a
/// `Realize` error is about a graph the source only implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    /// Reading characters into tokens.
    Lex,
    /// Reading tokens into a syntax tree.
    Parse,
    /// Finding the files a program's `use` items name, and folding their
    /// definitions into it.
    Import,
    /// Turning the tree into a graph and pattern bindings.
    Lower,
    /// Building the audio graph the engine runs.
    Realize,
    /// Raised while a pattern was playing rather than while it compiled: an
    /// instrument that only fails once a note asks it for a voice.
    Scheduler,
    /// Not the program's fault: the engine or a lock was in a bad way.
    Engine,
}

impl Stage {
    /// The pass named the way it is worth reading in a message.
    pub fn label(self) -> &'static str {
        match self {
            Stage::Lex | Stage::Parse => "syntax error",
            Stage::Import => "import error",
            Stage::Lower => "compile error",
            Stage::Realize => "audio graph error",
            Stage::Scheduler => "playback error",
            Stage::Engine => "engine error",
        }
    }
}

/// Whether a diagnostic stopped the program or merely has something to say
/// about it.
///
/// Every diagnostic was an [`Error`](Severity::Error) until MIDI arrived, and
/// the reason MIDI needed the other one is worth keeping: a program names the
/// port it plays to (`midiout("deluge")`), and the port it names is not always
/// plugged in. Refusing to compile there would mean a piece written for a rack
/// could not be edited on a train, which is the same trade `input(channel)`
/// already settled by being silence on a channel the device does not have.
///
/// So a warning is a diagnostic that rode the ordinary path to the problems
/// panel and changed nothing else: the eval that raised it still published its
/// graph and its patterns, and the run still counts as having succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The program did not run. Every diagnostic returned as an `Err` is one
    /// of these, which is why it is the default — a constructor that says
    /// nothing about severity is being called on a failure path.
    #[default]
    Error,
    /// The program ran, and this is something it is worth knowing about it.
    Warning,
}

/// One reason a program did not run.
///
/// `line` and `column` are 1-based, and both absent together: a pass either
/// knows the position or it does not. Only the front of the pipeline does —
/// the lowerer walks an AST that carries no spans, so its diagnostics are
/// message-only, and the editor is built to show them that way.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub stage: Stage,
    /// Whether this stopped the program. Serialized always rather than skipped
    /// when it is the default, because the frontend mirrors this type field for
    /// field and a missing key there is a shape it would have to guess at.
    pub severity: Severity,
    pub message: String,
    /// 1-based line in the source that was compiled.
    pub line: Option<usize>,
    /// 1-based column, counted in characters.
    pub column: Option<usize>,
    /// That line of source, verbatim, so the panel can show the code it is
    /// complaining about without the frontend re-deriving it from a tab whose
    /// contents may have moved on since the run.
    pub snippet: Option<String>,
    /// The file the position is in, absolute, when it is not the one that was
    /// run. `None` means the program itself — which is every diagnostic a
    /// program without imports can produce.
    pub file: Option<String>,
}

/// One line, for Rust-side readers — test failures, logs. The editor renders
/// the fields itself and never sees this.
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.severity == Severity::Warning {
            write!(f, "warning: ")?;
        }
        write!(f, "{}", self.stage.label())?;
        if let Some(file) = &self.file {
            write!(f, " in {file}")?;
        }
        if let (Some(line), Some(column)) = (self.line, self.column) {
            write!(f, " at line {line}, column {column}")?;
        }
        write!(f, ": {}", self.message)
    }
}

impl Diagnostic {
    /// A diagnostic with no position, for the passes that have none.
    pub fn message(stage: Stage, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            stage,
            severity: Severity::Error,
            message: message.into(),
            line: None,
            column: None,
            snippet: None,
            file: None,
        }
    }

    /// A diagnostic that did not stop anything.
    ///
    /// Positionless like [`message`](Diagnostic::message), and for the same
    /// reason: the passes that raise warnings are the ones downstream of the
    /// parser, which walk a tree carrying no spans.
    pub fn warning(stage: Stage, message: impl Into<String>) -> Diagnostic {
        Diagnostic { severity: Severity::Warning, ..Diagnostic::message(stage, message) }
    }

    /// Point a diagnostic at `offset` in `source`, unless it already knows a
    /// better place.
    ///
    /// An error raised while resolving a `use` belongs on the `use`; one raised
    /// by parsing the file that `use` names already belongs where it landed,
    /// and must not be dragged back out of it.
    pub fn or_at(self, source: &str, offset: usize) -> Diagnostic {
        if self.line.is_some() {
            return self;
        }
        let (file, severity) = (self.file, self.severity);
        // Severity rides across because `at` rebuilds the whole diagnostic and
        // knows only how to make failures. A warning that has been given a
        // position is still a warning.
        Diagnostic { file, severity, ..Diagnostic::at(self.stage, self.message, source, offset) }
    }

    /// Say which file a diagnostic is in, unless it already says.
    ///
    /// `None` is the file that was run, which the editor is already showing and
    /// so never needs naming.
    pub fn or_in(mut self, file: Option<&std::path::Path>) -> Diagnostic {
        if self.file.is_none() {
            self.file = file.map(|p| p.display().to_string());
        }
        self
    }

    /// A diagnostic pointing at a byte offset into `source`.
    ///
    /// Offsets past the end of the text land on the last line rather than
    /// dropping the position: "unexpected end of file" is exactly the error
    /// most in need of somewhere to point.
    pub fn at(
        stage: Stage,
        message: impl Into<String>,
        source: &str,
        offset: usize,
    ) -> Diagnostic {
        let offset = offset.min(source.len());
        // Byte offsets can land inside a multi-byte character when the source
        // has one; walking back to the boundary keeps the slicing below sound.
        let offset = (0..=offset)
            .rev()
            .find(|&i| source.is_char_boundary(i))
            .unwrap_or(0);

        let before = &source[..offset];
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let line_end = source[offset..]
            .find('\n')
            .map_or(source.len(), |i| offset + i);

        Diagnostic {
            stage,
            severity: Severity::Error,
            message: message.into(),
            line: Some(before.matches('\n').count() + 1),
            column: Some(source[line_start..offset].chars().count() + 1),
            snippet: Some(source[line_start..line_end].trim_end_matches('\r').to_string()),
            file: None,
        }
    }
}

use std::rc::Rc;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use crate::scree_graph::environment::{Env, FunctionDef, Value};
use crate::scree_graph::graph::ScreeGraph;
use crate::scree_graph::ugen_nodes::{NodeId, NodeInput, NodeKind, UGenNode};
use crate::parser::parser::ScreeItem;
use crate::pattern::patterns::{Binding, ChoiceGroup};
use crate::samples::Samples;
use crate::scheduler::clock::Meter;
#[cfg(test)]
use crate::scheduler::clock::{DEFAULT_CPS, DEFAULT_QUARTERS_PER_BAR};

pub struct Lowerer {
    pub env: Env,
    pub graph: ScreeGraph,
    pub depth: usize,
    pub bindings: Vec<Binding>,
    /// Bars after the origin that the next `play` should start at.
    ///
    /// Zero at the top level; `.then` raises it while inlining the function it
    /// was handed, which is the whole of "start when the last one finished".
    pub play_start: f64,
    /// Seeded per eval, so `choice`, `scramble` and the `rand` family differ
    /// each time. `seed` replaces it with a fixed one, which is the whole of
    /// how a lucky roll gets kept.
    pub rng: SmallRng,
    /// The buffers this program's `load` calls name, decoded before lowering
    /// began. Empty for a program that names no file — and for every caller
    /// that has no business reading one.
    pub samples: Samples,
    /// One entry per `wthen`, `rthen` or `maybe`, which its arms' bindings
    /// refer to by index. Empty for a program that never chooses.
    pub choices: Vec<ChoiceGroup>,
    /// The time signature this program is being lowered against.
    ///
    /// Two things need it, and both are conversions *out of* beats: a pattern
    /// written in note values has to know how many beats fill a bar before it
    /// can say what fraction of one it is, and `bpm` answers in bars per
    /// second. Everything else here already counts bars and never asks.
    pub meter: Meter,
    /// The octave in force while a written sequence is being lowered: set by
    /// every step that spells one, read by every note that does not, so
    /// `[a1;q, a, a, a]` is four `a1`s.
    ///
    /// A field rather than a parameter because the register belongs to the
    /// *written* sequence — `Expr::List` opens and closes it — while the note
    /// that reads it is resolved several frames down in `Expr::Var`, past the
    /// arithmetic and the calls in between, none of which have any business
    /// knowing about octaves.
    ///
    /// `None` outside a sequence, and that is what keeps an octave-less note an
    /// error everywhere else rather than a silent pitch.
    pub octave: Option<i32>,
}

/// One eval produces two artifacts: the persistent graph, which is crossfaded
/// into the engine's slot, and the pattern bindings, which go to the scheduler.
pub struct Lowered {
    pub graph: ScreeGraph,
    pub bindings: Vec<Binding>,
    pub choices: Vec<ChoiceGroup>,
}

/// A fresh RNG for one eval.
///
/// Drawn from the system source rather than the clock: two evals a keystroke
/// apart used to be able to land in the same nanosecond and repeat themselves,
/// which reads as a broken `choice` rather than as bad luck.
pub fn fresh_rng() -> SmallRng {
    SmallRng::from_rng(&mut rand::rng())
}

/// Lower a program that names no audio files.
///
/// Only tests reach this: the app always has a sample cache to hand and goes
/// through `lower_with_samples`, passing an empty set when the program names
/// no file. Kept because most of what there is to test about lowering has
/// nothing to do with buffers, and threading an empty one through every case
/// would say otherwise.
#[cfg(test)]
pub fn lower(items: &Vec<ScreeItem>) -> Result<Lowered, String> {
    lower_inner(items, None, DEFAULT_BEAT_SECS, Meter::default(), Samples::default())
}

/// The beat every caller that has no transport to ask gets: the default
/// tempo's, so `qvs` is bound to something musical in a test rather than left
/// out and reported as an unbound name.
#[cfg(test)]
pub const DEFAULT_BEAT_SECS: f64 = 1.0 / (DEFAULT_CPS * DEFAULT_QUARTERS_PER_BAR);

/// Lower a program in a given time signature, at the default tempo.
///
/// Only tests reach this, and only the ones about rhythm: the signature is
/// what a bar is, so a test that a three-beat pass fills a 3/4 bar has no
/// other way to say which bar it means.
#[cfg(test)]
pub fn lower_in_meter(items: &Vec<ScreeItem>, meter: Meter) -> Result<Lowered, String> {
    lower_inner(items, None, DEFAULT_BEAT_SECS, meter, Samples::default())
}

/// Lower a program with buffers but no transport, at the default tempo.
///
/// Only tests reach this, for the same reason they reach `lower`: what there is
/// to test about a `load` has nothing to do with what the clock is doing. The
/// app goes through `lower_at_tempo`.
#[cfg(test)]
pub fn lower_with_samples(items: &Vec<ScreeItem>, samples: Samples) -> Result<Lowered, String> {
    lower_inner(items, None, DEFAULT_BEAT_SECS, Meter::default(), samples)
}

/// Lower a program against the tempo the transport is running at, so `qvs` and
/// `qvh` in the persistent graph mean the beat the patterns are on.
///
/// Only `run_code` calls this — it is the one caller with an engine to ask.
/// The tests go through `lower` or `lower_with_samples` and get the default
/// tempo, which is the honest answer when there is no transport in the picture.
///
/// The graph is lowered once per eval, so the tempo here is the tempo *then*:
/// dragging the transport afterwards moves the patterns and leaves the graph
/// where it was until the next eval. A voice has no such gap — it is lowered
/// per note, and reads the tempo as it is at that note.
pub fn lower_at_tempo(
    items: &Vec<ScreeItem>,
    samples: Samples,
    beat_secs: f64,
    meter: Meter,
) -> Result<Lowered, String> {
    lower_inner(items, None, beat_secs, meter, samples)
}

/// Lower a single scheduler voice.
///
/// `dur` (the note length in seconds) is pre-bound as an ordinary number, so an
/// instrument can shape itself against the note it is playing — `env(a, d, s,
/// r, dur)`. Outside a voice the name is simply unbound.
///
/// `beat_secs` is the quarter note of the clock *this* note is played on — the
/// transport's, divided by the speed its pattern runs at — and is bound here as
/// `qvs`. Unlike `dur` it is bound everywhere, because a tempo exists whether or
/// not a note is sounding.
///
/// `samples` comes from the eval that published the instruments, so an
/// instrument that reads a buffer builds here without touching a disk.
pub fn lower_voice(
    items: &Vec<ScreeItem>,
    dur: f64,
    beat_secs: f64,
    meter: Meter,
    samples: Samples,
) -> Result<Lowered, String> {
    lower_inner(items, Some(dur), beat_secs, meter, samples)
}

fn lower_inner(
    items: &Vec<ScreeItem>,
    dur: Option<f64>,
    beat_secs: f64,
    meter: Meter,
    samples: Samples,
) -> Result<Lowered, String> {
    let mut lw = Lowerer {
        env: Env::new(),
        graph: ScreeGraph::default(),
        depth: 0,
        bindings: Vec::new(),
        play_start: 0.0,
        rng: fresh_rng(),
        samples,
        choices: Vec::new(),
        meter,
        octave: None,
    };

    if let Some(dur) = dur {
        lw.env.define("dur", Value::Number(dur));
    }

    // The beat, written the two ways a signal wants it: as a length for
    // everything that takes seconds — `env`, `perc`, `line`, the delays — and as
    // a frequency for everything that takes hertz. `qvh` is `1 / qvs` and could
    // be written out every time, but an oscillator synced to the beat is the
    // whole point of having these, and `sin(qvh * 2)` says what it does where
    // `sin(1 / (qvs / 2))` does not.
    //
    // A tempo of zero or worse would poison every frequency computed from here,
    // so a beat that is not a positive, finite length leaves both names unbound
    // — `unbound name: qvs` is a diagnostic somebody can act on, and silence
    // full of NaNs is not. Neither the clock nor `accel` can produce one today;
    // this is what keeps that true if either learns how.
    if beat_secs.is_finite() && beat_secs > 0.0 {
        lw.env.define("qvs", Value::Number(beat_secs));
        lw.env.define("qvh", Value::Number(1.0 / beat_secs));
    }

    for item in items {
        lw.item(item)?;
    }

    Ok(Lowered { graph: lw.graph, bindings: lw.bindings, choices: lw.choices })
}

/// Lower a program that is only expected to produce a graph.
#[cfg(test)]
pub fn lower_graph(items: &Vec<ScreeItem>) -> Result<ScreeGraph, String> {
    lower(items).map(|l| l.graph)
}

impl Lowerer {

    fn item(&mut self, item: &ScreeItem) -> Result<(), String> {
        match item {
            ScreeItem::Function { name, params, body } => {
                self.env.define(&name.0.as_str(), Value::Function(
                    Rc::new(
                        FunctionDef { params: params.to_vec(), body: body.clone() }
                    )
                ));
                
                Ok(())
            }

            ScreeItem::Let { name, value } => {
                let v = self.expr(value)?;
                self.env.define(&name.0.as_str(), v);
                Ok(())
            }

            ScreeItem::Expr(e) => {
                let v = self.expr(e)?;
                if let Value::Signal(id) = v {
                    self.add_to_output(id);
                }
                Ok(())
            }

            ScreeItem::Call { func, args } => {
                let v = self.call(func, args)?;
                if let Value::Signal(id) = v {
                    self.add_to_output(id);
                }
                Ok(())
            }


            _ => Ok (())
        }
    }

    pub fn push_node(&mut self, kind: NodeKind, inputs: Vec<NodeInput>) -> NodeId {
        self.graph.nodes.push(UGenNode { kind, inputs, span: None });
        NodeId(self.graph.nodes.len() - 1)
    }

    pub fn add_to_output(&mut self, id: NodeId) {
        self.graph.output = Some(match self.graph.output {
            None => id,
            Some(prev) => self
                .push_node(NodeKind::Add, vec![NodeInput::Node(prev), NodeInput::Node(id)])
        })
    }
}
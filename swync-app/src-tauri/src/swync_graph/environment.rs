use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use fundsp::wave::Wave;

use crate::lang::Beats;
use crate::swync_graph::ugen_nodes::NodeId;
use crate::parser::parser::{Expr, Param};

#[derive(Clone)]
pub enum Value {
    Number(f64),
    Signal(NodeId),
    Function(Rc<FunctionDef>),
    List(Rc<Vec<Item>>),
    /// Patterns that sound at once rather than in turn — what `stack` builds.
    ///
    /// A variant of its own because nothing about the shape of a list says
    /// whether it is a sequence or a chord: `[[a, b], [c, d]]` is already two
    /// groups played one after the other, so layering needs a mark rather than
    /// a nesting. Only patterns read it; everywhere else it is an error, since
    /// there is no sensible `len` or index of two things happening together.
    Stack(Rc<Vec<Value>>),
    /// A loaded audio file, as `load` answers with it. Not a signal — nothing
    /// comes out of a buffer until `sample` reads it at a position.
    Buffer(Arc<Wave>),
    /// A silent step. Only meaningful inside a pattern.
    Rest,
    /// A sounding step carrying no value. Only meaningful inside a pattern.
    Trigger,
    /// A written note value — `q`, `e`, `h`. Only meaningful inside a pattern,
    /// where it says how long a step is rather than what sounds.
    ///
    /// In the value position it is a [`Trigger`](Value::Trigger) of that
    /// length, so `[q, q, q]` is three quarter-note hits: a duration alone
    /// carries no pitch, which is exactly what a trigger already means.
    Duration(Beats),
    /// `t`, marking a group as a tuplet. A marker rather than a number for the
    /// same reason `Rest` is a variant rather than a value: it means one thing
    /// in one position and is an error everywhere else.
    Tuplet,
    /// A speed to run a pattern at, from `accel`. Only `play`'s rate takes one.
    ///
    /// Not a number, because a curve is not one; and not a signal, because
    /// nothing in the audio graph can reach the scheduler — it works a
    /// lookahead ahead of the audio clock, so it needs the rate for a stretch
    /// of time the graph has not rendered yet. A rate the lowerer can hand over
    /// whole is the shape that fits between them.
    Rate(crate::pattern::rate::Rate),
    /// What a `play` call evaluates to: a handle onto the bindings it made.
    ///
    /// `ends_at` is the cycle, counted from the pattern origin, at which the
    /// section falls silent — `None` for plain `play`, which never does. It is
    /// what `.then` needs to know when to start what follows.
    ///
    /// The rest of it is what the combinators beyond `.then` need. `starts_at`
    /// is where the section opened, so `.with` can lay something alongside it
    /// rather than after it. `first..last` is the slice of `Lowerer::bindings`
    /// the section wrote, so `.take` and `.stop` can reach back and shorten
    /// what is already on the timeline — the two of them are the only things
    /// here that edit a binding after the fact. And `template` is the single
    /// `play` a handle came from, which is where `.then_fill` gets the
    /// instrument it is a fill *for*; a group of several plays leaves it
    /// `None`, because then there is no one instrument to inherit.
    ///
    /// `chain_first` is the odd one out: every other field describes the *last*
    /// link, but this one remembers where the whole chain began. `.then` and
    /// its family deliberately narrow `first..last` to the section they just
    /// wrote, so that `.take` after a `.then` cuts what followed and not what
    /// came before — which leaves nothing able to say "all of this", and that
    /// is exactly what `.loop` repeats. A fresh `play` starts a chain, so there
    /// it is `first`; every combinator passes its receiver's along.
    Play {
        starts_at: f64,
        ends_at: Option<f64>,
        first: usize,
        last: usize,
        chain_first: usize,
        template: Option<usize>,
    },
}

/// What a `;` said, which is one of two different things.
///
/// A bare number is a *share*: the sequence still fills exactly one cycle and
/// the numbers divide it between them, so only their ratio matters. A written
/// note value is a *duration*: the sequence is as long as its values add up to,
/// and a cycle has nothing to do with it. The two cannot be reconciled — one
/// says "twice as long as its neighbour", the other "one beat, whatever the
/// neighbours are" — so the distinction is kept in the type and a sequence
/// holding both is refused rather than having one of them silently win.
#[derive(Clone, Copy, PartialEq)]
pub enum Length {
    /// `;2` — a share of the sequence, relative to its siblings.
    Ratio(f64),
    /// `;q` — a written value, in beats.
    Beats(Beats),
    /// `;t` — this group is a tuplet. Carries no number: the span it is played
    /// in follows from what the group holds.
    Tuplet,
}

/// One element of a list, and the length `;` gave it.
///
/// The length rides along on the element instead of being desugared into
/// repeated copies, because the two are not the same thing: `[c4;3, e4]` is one
/// note held for three quarters of a cycle, and `[c4, c4, c4, e4]` is three
/// strikes. Only whoever reads the list knows which of those it wants — a
/// pattern sustains, a lane repeats, and `len` ignores it entirely and counts
/// the elements that were written.
#[derive(Clone)]
pub struct Item {
    pub value: Value,
    pub length: Option<Length>,
}

impl Item {
    /// An element with no length — every element of a list that predates `;`,
    /// and every one built by a builtin rather than written down.
    pub fn plain(value: Value) -> Item {
        Item { value, length: None }
    }

    /// The list a builtin returns, where lengths never apply.
    pub fn all(values: impl IntoIterator<Item = Value>) -> Rc<Vec<Item>> {
        Rc::new(values.into_iter().map(Item::plain).collect())
    }
}

pub struct FunctionDef {
    pub params: Vec<Param>,
    pub body: Expr
}

pub struct Env {
    scopes: Vec<HashMap<String, Value>>
}

impl Env {
    pub fn new() -> Env {
        Env {
            scopes: vec![HashMap::new()]
        }
    }

    pub fn define(&mut self, name: &str, value: Value) {
        self.scopes
            .last_mut()
            .expect("scope stack is never empty")
            .insert(name.to_string(), value);
    }

    pub fn lookup(&self, name: &str) -> Option<Value> {
        self.scopes.iter().rev()
            .find_map(|scope| scope.get(name))
            .cloned()
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop();
        debug_assert!(!self.scopes.is_empty(), "popped the global scope");
    }
}
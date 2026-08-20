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
    /// A list marked to be passed whole: `'[2, 3]`, or `list(2, 3)`.
    ///
    /// A variant of its own for the same reason [`Stack`](Value::Stack) is one.
    /// Nothing about the shape of a list says whether the brackets are a
    /// sequence to read through or the value itself, and in a `play` lane both
    /// are meaningful: `div: [2, 3]` gives the first note 2 and the second 3,
    /// while `div: '[2, 3]` gives every note both. So the difference has to be
    /// a mark rather than a nesting.
    ///
    /// The numbers are held flat rather than as `Item`s because a lane carries
    /// them to the scheduler thread, which builds the voice: whatever crosses
    /// has to be plain data, and `Item` holds a `Value` that may be an `Rc`.
    /// That is also why the items are checked to be numbers here, at the quote,
    /// rather than being discovered to be something else one note at a time.
    Quoted(Rc<Vec<f64>>),
    /// Where a pattern's notes are being sent, as `midiout` answers with it.
    ///
    /// Opaque, like a [`Buffer`](Value::Buffer): nothing may be done with one
    /// except hand it to a `play` in the slot an instrument would otherwise
    /// fill. It carries the port as it was *written* rather than as it
    /// resolved, because which ports exist is a fact about tonight and the
    /// value may outlive an unplugging — see `midi::out::Destination`.
    Destination(crate::midi::out::Destination),
    /// Notes arriving from outside, as `midiin` answers with them.
    ///
    /// The mirror of [`Destination`](Value::Destination), and it goes in the
    /// slot the other one does not: a destination is what *plays* a pattern,
    /// and a source is the pattern itself. So `play(midiin("keys"), lead)`
    /// reads as what it is, and the method form `midiin("keys").play(lead)`
    /// falls out of it — `a.f(b)` is `f(a, b)`, and nothing had to be added
    /// for that.
    ///
    /// It carries the slot rather than the port as written, because unlike a
    /// destination there is nothing left to resolve: a slot is handed out at
    /// lowering time and is fixed for the life of the process.
    Source(crate::swync_graph::environment::Source),
    /// A slider in the panel, as `slider` answers with it.
    ///
    /// Both a signal and a number, and it has to be both. As a **signal** it is
    /// the node the graph reads at audio rate, which is what makes dragging one
    /// audible without recompiling anything — that is the ordinary use, and it
    /// reaches the graph through `as_input` like any other signal. A site that
    /// asks "is this audio?" by matching the *variant* has to ask
    /// `expr::signal_node` instead, or it silently drops a slider: the output
    /// rule and the `for` loop both did, and what that sounded like was a line
    /// that did nothing.
    /// As a **number** it is where the slider was standing when the program
    /// compiled, which is the only answer available where the language demands
    /// a compile-time number: a pattern's rate, a `;` length, anything that
    /// folds during lowering. Those readings are baked in, and are marked as
    /// such on the way past so the panel can say so — see
    /// [`crate::controls::Slider::baked`].
    ///
    /// A variant of its own rather than a `Signal` with a number attached,
    /// because the two are wanted in different places and a value that
    /// silently picked one would pick wrong: `slider("level") * 0.5` is a
    /// multiply in the graph, not a constant folded at compile time, and
    /// nothing about the expression says so except which variant this is.
    ///
    /// It carries the slot as well as the node because the slot is what
    /// outlives the graph — it is the session's memory of where this control
    /// is, and it is what a reading-as-a-number has to mark.
    Slider { node: NodeId, slot: usize, at: f64 },
    /// A loaded audio file, as `load` answers with it. Not a signal — nothing
    /// comes out of a buffer until `sample` reads it at a position.
    Buffer(Arc<Wave>),
    /// An `enum` itself, as the bare `Scale` in `Scale.major` resolves to.
    ///
    /// It lives in the environment like any other binding rather than in a
    /// table beside it, which is what gives it scoping and shadowing for free —
    /// and, more to the point, is what lets the collision check be "is this name
    /// already bound?" rather than a second question asked of a second place.
    ///
    /// Nothing may be done with one except reach through it for a member. That
    /// is a refusal every consumer already makes, since a type is not a number,
    /// a list or a signal.
    EnumType(Rc<EnumDef>),
    /// One member of one enum: `Scale.major`.
    ///
    /// Opaque, and carrying its value rather than being it. The two halves are
    /// both load-bearing and pull opposite ways — identity is what `==` compares,
    /// so that `Section.verse == Section.chorus` is a question about which tag
    /// this is and not about what the tags happen to hold; while the value is
    /// what the member is *for* wherever data is wanted, so that
    /// `61.scale(Scale.major)` reads the offsets without the writer having to
    /// unwrap them. Collapsing either half into the other loses one of the two
    /// things enums were added to do.
    ///
    /// The member is held as an index into `def.members` rather than by name, so
    /// that a value is a pointer and a number however long the names are.
    Enum { def: Rc<EnumDef>, member: usize },
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

/// A keyboard, as `midiin` names one.
///
/// Plain numbers, because this ends up inside a `Binding` and crosses to the
/// scheduler thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    /// Which port, as `midi::input` interned it.
    pub slot: usize,
    /// `None` is every channel, which is what a keyboard on its own means.
    pub channel: Option<u8>,
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

/// An `enum` as the program can use it: its name, and its members in the order
/// they were written, each with the value it was given.
///
/// The members' values are evaluated once, where the enum is declared, rather
/// than at each mention. That is what makes a member a constant: `enum R { x =
/// rand() }` draws one number and every `R.x` is that number, which is the only
/// reading under which two mentions of one name are the same value.
pub struct EnumDef {
    /// As the program knows it — `Scale`, or `kit::Scale` once imported.
    pub name: String,
    pub members: Vec<EnumMemberDef>,
}

pub struct EnumMemberDef {
    pub name: String,
    /// What the member stands for where data is wanted, or `None` for a tag
    /// that stands only for itself.
    pub value: Option<Value>,
}

impl EnumDef {
    /// The index of a member by name, for `Scale.major`.
    pub fn member(&self, name: &str) -> Option<usize> {
        self.members.iter().position(|m| m.name == name)
    }

    /// The members' names, for a message that has to say what was on offer.
    pub fn member_names(&self) -> String {
        self.members
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// How the program writes this enum's name, without the module prefix
    /// expansion gave it.
    pub fn written(&self) -> &str {
        written(&self.name)
    }
}

/// A definition's name as the file that mentions it writes it, without the
/// module prefix expansion filed it under.
///
/// Every message about an enum quotes this rather than the filed name: a reader
/// who wrote `use kit::Scale` and then `Scale.majr` has never seen the spelling
/// `kit::Scale`, and an error naming it would be about a program they did not
/// write.
pub fn written(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
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
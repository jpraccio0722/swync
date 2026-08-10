use chumsky::span::SimpleSpan;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub usize);


#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NodeKind {
    Add, ADSR, Afollow, Allpass, Allpole,
    Bandpass, Bandrez, Bell, Biquad, Brown, Butterpass,
    Chorus, Clip, ClipTo,
    Dcblock, Declick, Delay, Div, DsfSaw, DsfSquare,
    Env,
    Fir3, Follow,
    Hammond, Highpass, Highpole, Highshelf, Hold, Impulse,
    Limiter, Line, Lorenz, Lowpass, Lowpole, Lowrez, Lowshelf,
    Mls, MlsBits, Moog, Morph, Mul, Neg,
    Noise, Notch,
    Organ,
    Peak, Perc, Pink, Pinkpass, Pluck, PolyPulse, PolySaw, PolySquare, Pulse,
    Ramp, Resonator, Reverb, Reverb2, Reverb3, Reverb4, Rossler,
    Sample, Saw, Sin, SoftSaw, Square, Sub,
    Tap, Tick, Triangle,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeInput {
    Const(f64),
    Node(NodeId)
}

#[derive(Clone, Debug, PartialEq)]
pub struct UGenNode {
    pub kind: NodeKind,
    pub inputs: Vec<NodeInput>,
    pub span: Option<SimpleSpan>
}
use std::sync::Arc;

use fundsp::wave::Wave;

use crate::swync_graph::ugen_nodes::{NodeId, UGenNode};

/// A lowered program: nodes, which of them is the output, and the buffers the
/// nodes read from.
///
/// Samples sit beside the nodes rather than inside them because a `NodeKind` is
/// a plain C-like enum — it is written into a `static` table in `lang`, which a
/// variant holding an `Arc` could not be. A `Sample` node names its buffer by
/// index into this table, as a construction-time constant like any other.
#[derive(Clone, Default)]
pub struct SwyncGraph {
    pub nodes: Vec<UGenNode>,
    pub output: Option<NodeId>,
    /// Buffers the graph's `Sample` nodes read, in the order they were first
    /// named. Cloning one is an `Arc` bump, not a copy of the audio.
    pub samples: Vec<Arc<Wave>>,
}

impl SwyncGraph {
    /// Record a buffer and answer with the index a `Sample` node names it by.
    /// The same buffer asked for twice is one entry, so a break chopped sixteen
    /// ways is stored once.
    pub fn intern_sample(&mut self, wave: Arc<Wave>) -> usize {
        match self.samples.iter().position(|w| Arc::ptr_eq(w, &wave)) {
            Some(i) => i,
            None => {
                self.samples.push(wave);
                self.samples.len() - 1
            }
        }
    }
}

/// Hand-written because `Wave` has no `Debug`, and because printing one would
/// be printing every sample in it — a break is several million numbers, and a
/// graph is something you dump while debugging something else.
impl std::fmt::Debug for SwyncGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwyncGraph")
            .field("nodes", &self.nodes)
            .field("output", &self.output)
            .field("samples", &format_args!("[{} buffers]", self.samples.len()))
            .finish()
    }
}

/// Hand-written for the same reason: `Wave` has no `PartialEq`, and comparing
/// two by content would be comparing the audio. Two graphs hold the same buffer
/// when they hold the same buffer — identity, not equality.
impl PartialEq for SwyncGraph {
    fn eq(&self, other: &Self) -> bool {
        self.nodes == other.nodes
            && self.output == other.output
            && self.samples.len() == other.samples.len()
            && self
                .samples
                .iter()
                .zip(&other.samples)
                .all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

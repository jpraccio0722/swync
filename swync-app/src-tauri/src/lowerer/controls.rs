//! `slider(name, lo = 0, hi = 1, start = lo)` — a control in the panel, named
//! where it is used.
//!
//! Intercepted before its arguments are evaluated, for the reason `load` and
//! `midiout` are: the name is a string, and a string is not a value in this
//! language. `Expr::Str` is refused everywhere else precisely so that text
//! cannot be carried around and mistaken for something that sounds, so the
//! names that take one read it off the syntax instead.
//!
//! The other half of what happens here is that **nothing is decided here**.
//! The shape of a slider — its range, where it starts, which slot it has — is
//! settled by [`crate::controls::declare_in`] walking the program's syntax
//! before lowering begins, because an instrument's body is not lowered until a
//! note plays and the panel cannot wait for one. So this looks the name up and
//! agrees with what is already on the panel, and only falls back to declaring
//! one itself where no pass has run at all, which is a test lowering a program
//! directly.
//!
//! That is also what makes "the first declaration wins" true of the *sound*
//! and not merely of the warning: a second `slider("cutoff", 0, 1)` written
//! with a different range lowers to the range the first one asked for, because
//! what it reads is the table rather than its own arguments.

use crate::controls;
use crate::lowerer::lower::Lowerer;
use crate::parser::parser::Arg;
use crate::swync_graph::environment::Value;
use crate::swync_graph::ugen_nodes::{NodeInput, NodeKind};

impl Lowerer {
    /// True when this call declares a control in the panel.
    pub fn is_slider(name: &str) -> bool {
        name == controls::SLIDER
    }

    /// `slider("cutoff")`, `slider("cutoff", 200, 5000)`,
    /// `slider("cutoff", 200, 5000, 800)`.
    ///
    /// The node holds the **slot** and nothing else, which is the same trick
    /// `cc` plays with a port and for the same reason: a node reads its
    /// control on the audio callback and cannot hash a name, so the name
    /// becomes a small integer here and never appears in the graph again.
    /// Unlike `cc` it carries no range either — what the slot holds is already
    /// in the slider's own units, because the panel knows the range it drew.
    pub fn slider(&mut self, args: &[Arg], piped: Option<Value>) -> Result<Value, String> {
        // A piped receiver is refused rather than folded in with the
        // arguments, and there is nothing it could have been: the first
        // parameter is the name, the name is written in quotes, and a string
        // is never a value — so whatever arrived here came from somewhere the
        // name cannot come from.
        if piped.is_some() {
            return Err(format!(
                "{}: the name is written at the call rather than chained into it — \
                 {}(\"cutoff\", 200, 5000)",
                controls::SLIDER, controls::SLIDER));
        }

        let decl = controls::parse(args)?;

        // What the panel already has for this name, which is what the program
        // declared first. Only a program nothing has walked — a test lowering
        // directly — falls through to declaring it here.
        let slot = match controls::shape_of(&decl.name) {
            Some((slot, ..)) => slot,
            // Every slot taken, when this answers `None`. The pass has already
            // said so in the problems panel, and what is left to decide here
            // is what the program *sounds* like: silence at this one control
            // and everything else unchanged. Refusing to compile would take a
            // whole piece down over a sixty-fifth slider.
            None => controls::declare(&decl.name, decl.lo, decl.hi, decl.start)
                .unwrap_or(controls::NO_SLOT),
        };

        let node = self.push_node(NodeKind::Slider, vec![NodeInput::Const(slot as f64)]);
        Ok(Value::Slider { node, slot, at: controls::position(slot) as f64 })
    }
}

#[cfg(test)]
mod tests {
    use crate::controls::{self, exclusive, set};
    use crate::lowerer::lower::lower;
    use crate::parser::parser::parse;
    use crate::pattern::rate::Rate;
    use crate::swync_graph::graph::SwyncGraph;
    use crate::swync_graph::ugen_nodes::{NodeInput, NodeKind};

    fn lower_src(src: &str) -> Result<SwyncGraph, String> {
        let items = parse(src.to_string()).expect("parse failed");
        // The pass a real run makes before lowering, so these read the way the
        // app does rather than the way only a test would.
        controls::declare_in(&items);
        lower(&items).map(|l| l.graph)
    }

    fn err(src: &str) -> String {
        lower_src(src).expect_err("this program should not have compiled")
    }

    fn slots(g: &SwyncGraph) -> Vec<NodeInput> {
        g.nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Slider)
            .map(|n| n.inputs[0].clone())
            .collect()
    }

    /// The ordinary use: a node in the graph, read at audio rate, so a drag is
    /// heard without anything being compiled again.
    #[test]
    fn a_slider_in_a_signal_position_is_a_node_the_graph_reads() {
        let _controls = exclusive();
        let g = lower_src("lowpass(saw(110), slider(\"cutoff\", 200, 5000, 800), 1)\n").unwrap();

        assert_eq!(slots(&g), vec![NodeInput::Const(0.0)]);
        assert!(!controls::baked_by_name("cutoff"), "a signal reading is not a baked one");
    }

    /// The whole point of the node: arithmetic on a slider builds a graph
    /// rather than folding to the number it happens to be standing at.
    #[test]
    fn arithmetic_on_a_slider_stays_in_the_graph() {
        let _controls = exclusive();
        let g = lower_src("sin(220) * slider(\"level\") * 0.5\n").unwrap();

        assert_eq!(slots(&g), vec![NodeInput::Const(0.0)]);
        assert_eq!(g.nodes.iter().filter(|n| n.kind == NodeKind::Mul).count(), 2);
        assert!(!controls::baked_by_name("level"));
    }

    /// A math builtin folds during lowering, so there is no node for a
    /// changing value to be part of and the position it stands at is the only
    /// answer available. What is owed is saying so, which is the mark.
    #[test]
    fn a_slider_where_only_a_number_will_do_is_the_position_it_stands_at() {
        let _controls = exclusive();
        let g = lower_src("sin(slider(\"pitch\", 40, 80, 69).m2h)\n").unwrap();

        assert_eq!(g.nodes.iter().filter(|n| n.kind == NodeKind::Sin).count(), 1);
        assert!(controls::baked_by_name("pitch"), "a number reading should be marked");
    }

    /// Read as a number in one place and as a signal in another, it is still
    /// one slider — and it is marked, because a drag that is heard in one
    /// place and not the other is a drag the panel has to warn about.
    #[test]
    fn a_slider_read_both_ways_is_marked_as_the_baked_one_it_partly_is() {
        let _controls = exclusive();
        lower_src("sin(220) * slider(\"level\", 0, 1, 0.5) + sin(slider(\"level\").m2h)\n")
            .unwrap();
        assert!(controls::baked_by_name("level"));
    }

    /// One name is one slider wherever it is written, and the range the first
    /// one asked for is the range both of them get — which has to be true of
    /// the graph and not only of the warning in the problems panel.
    #[test]
    fn a_name_written_twice_lowers_to_one_slot() {
        let _controls = exclusive();
        let g = lower_src(
            "lowpass(saw(110), slider(\"cutoff\", 200, 5000), 1) * slider(\"cutoff\", 0, 1)\n")
            .unwrap();

        assert_eq!(slots(&g), vec![NodeInput::Const(0.0), NodeInput::Const(0.0)]);
        assert_eq!(
            controls::shape_of("cutoff").map(|(_, lo, hi, _)| (lo, hi)),
            Some((200.0, 5000.0)));
    }

    /// Two names are two controls, which is the thing that makes naming them
    /// worth anything.
    #[test]
    fn two_names_are_two_slots() {
        let _controls = exclusive();
        let g = lower_src("sin(220) * slider(\"a\") + sin(330) * slider(\"b\")\n").unwrap();
        assert_eq!(slots(&g), vec![NodeInput::Const(0.0), NodeInput::Const(1.0)]);
    }

    /// Where a session has already been dialled in, the number baked into a
    /// program is where the slider *is* rather than where the program says it
    /// starts — otherwise a re-eval would quietly undo the last five minutes.
    #[test]
    fn a_baked_reading_takes_the_position_rather_than_the_starting_point() {
        let _controls = exclusive();
        let src = "fn tone(n) = sin(n.m2h)\nplay([c4], tone, slider(\"speed\", 1, 4, 1))\n"
            .to_string();
        let items = parse(src).expect("parse failed");
        controls::declare_in(&items);
        set("speed", 3.0);

        let lowered = lower(&items).expect("lowering failed");
        assert!(matches!(lowered.bindings[0].rate, Rate::Fixed(r) if r == 3.0),
                "got: {:?}", lowered.bindings[0].rate);
    }

    #[test]
    fn a_slider_needs_a_name_written_in_quotes() {
        let _controls = exclusive();
        let message = err("let name = 1\nsin(220) * slider(name)\n");
        assert!(message.contains("written in quotes"), "got: {message}");
    }

    /// The panel is drawn before the program runs, so a range it would have to
    /// run the program to know is one it cannot draw.
    #[test]
    fn a_range_that_has_to_be_worked_out_is_refused() {
        let _controls = exclusive();
        let message = err("let top = 5000\nsin(slider(\"cutoff\", 200, top))\n");
        assert!(message.contains("written out as a number"), "got: {message}");
    }

    #[test]
    fn a_slider_cannot_be_chained_into() {
        let _controls = exclusive();
        let message = err("sin(220 >> slider(200, 5000))\n");
        assert!(message.contains("written at the call"), "got: {message}");
    }
}

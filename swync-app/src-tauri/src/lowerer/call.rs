use std::rc::Rc;

use crate::{swync_graph::{environment::Value, ugen_nodes::NodeKind}, lowerer::lower::Lowerer, parser::parser::{Arg, Ident}};


impl Lowerer {
    pub fn call(&mut self, func: &Ident, args: &[Arg]) -> Result<Value, String> {
        self.call_with(func, args, None)
    }

    pub fn call_with(&mut self, func: &Ident, args: &[Arg], piped: Option<Value>) -> Result<Value, String> {
        // Before evaluation: play needs the instrument argument syntactically,
        // and load needs its path the same way.
        if Lowerer::is_play(&func.0) {
            return self.play(&func.0, args, piped);
        }
        if Lowerer::is_load(&func.0) {
            return self.load(args, piped);
        }
        // And `midiout`, which reads a port's name off the syntax for exactly
        // the reason `load` reads a path off it — as do the three MIDI-in
        // readers and `midiin` itself.
        if Lowerer::is_midiout(&func.0) {
            return self.midiout(args, piped);
        }
        if Lowerer::is_midiin(&func.0) {
            return self.midiin(args, piped);
        }
        if Lowerer::is_midi_control(&func.0) {
            return self.midi_control(&func.0, args, piped);
        }
        if Lowerer::is_midiclock(&func.0) {
            return self.midiclock(args, piped);
        }
        // And `slider`, whose name is read off the syntax for the same reason
        // a port's is — with the addition that the panel has already read it,
        // before any of this ran. See `lowerer::controls`.
        if Lowerer::is_slider(&func.0) {
            return self.slider(args, piped);
        }

        // And the arrangement combinators, for a reason of the same shape: a
        // section has to be lowered where it is *placed*, with `play_start`
        // already moved there. Evaluating `.then(playn(...))` on the way in
        // would write those notes at the origin, leaving `.then` a fixup to
        // perform instead of a section to place — and leaving the meaning of a
        // play dependent on whether something later reached in and moved it.
        // `section_builtin` settles the arguments that can be settled.
        if Lowerer::is_section(&func.0) {
            return self.section_builtin(&func.0, args, piped);
        }

        // `play_all` is one of them in this respect: a section named among its
        // arguments has to be inlined rather than evaluated, and a name that
        // has already been evaluated is a `fn` value with nowhere left to put
        // its notes. It differs only in *where* — the group's own start, which
        // is where everything is already.
        if Lowerer::is_play_all(&func.0) {
            return self.play_all(args, piped);
        }

        let mut arg_vals: Vec<Value> = Vec::with_capacity(args.len() + 1);
        let mut named: Vec<(String, Value)> = Vec::new();

        if let Some(p) = piped {
            arg_vals.push(p);
        }

        for arg in args {
            let v = self.expr(&arg.value)?;
            match &arg.name {
                // Allowing them to interleave would make "which parameter does
                // this fill?" depend on names declared elsewhere.
                None if !named.is_empty() => return Err(format!(
                    "{}: positional arguments must come before named ones", func.0)),
                None => arg_vals.push(v),
                Some(name) => named.push((name.0.clone(), v)),
            }
        }

        // Only user functions have parameter names to match against. Checking
        // here means a name aimed at a UGen fails loudly instead of being
        // silently dropped by a builtin that never looks for one.
        if !named.is_empty() && !matches!(self.env.lookup(&func.0), Some(Value::Function(_))) {
            return Err(format!(
                "{}: named arguments only work on a user `fn`", func.0));
        }

        // Before the UGen table: `sample` takes a buffer where the table's own
        // lowering would put every argument through `as_input`.
        if Lowerer::is_buffer_builtin(&func.0) {
            return self.buffer_builtin(&func.0, &arg_vals);
        }

        // `accel` is in no table for the same reason: it answers with a rate,
        // which is neither a number the math builtins could fold nor a node the
        // UGen table could push.
        if Lowerer::is_rate(&func.0) {
            return self.rate_builtin(&arg_vals);
        }

        if let Some(v) = self.list_builtin(&func.0, &arg_vals)? {
            return Ok(v);
        }

        if let Some(v) = self.math_builtin(&func.0, &arg_vals)? {
            return Ok(v);
        }

        if let Some(v) = self.random_builtin(&func.0, &arg_vals)? {
            return Ok(v);
        }

        if let Some((kind, arity)) = self.builtin(&func.0) {
            if arg_vals.len() != arity {
               return Err(format!("{} expects {} inputs, got {}",
                                   func.0, arity, arg_vals.len()));
            }
            let inputs = arg_vals.into_iter()
                .map(|v| self.as_input(v))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Value::Signal(self.push_node(kind, inputs)));
        }

        let Some(Value::Function(def)) = self.env.lookup(&func.0) else {
            return Err(format!("{} is not a function", func.0));
        };

        self.apply_named(&func.0, def, arg_vals, named)
    }

    /// Inline a user function against already-evaluated arguments.
    ///
    /// Split out of `call_with` so list builtins that take a function — `filter`
    /// — can invoke one without having the original `Expr`s.
    pub fn apply(
        &mut self,
        name: &str,
        def: Rc<crate::swync_graph::environment::FunctionDef>,
        arg_vals: Vec<Value>,
    ) -> Result<Value, String> {
        self.apply_named(name, def, arg_vals, Vec::new())
    }

    /// `apply`, plus arguments matched to parameters by name.
    ///
    /// Defaults are still evaluated inside the pushed scope and in declaration
    /// order, so `fn bass(n, cut = n.m2h * 4)` sees `n` — which is why names are
    /// resolved here rather than flattened into positions by the caller.
    pub fn apply_named(
        &mut self,
        name: &str,
        def: Rc<crate::swync_graph::environment::FunctionDef>,
        arg_vals: Vec<Value>,
        named: Vec<(String, Value)>,
    ) -> Result<Value, String> {
        if arg_vals.len() > def.params.len() {
            return Err(format!("{} expects at most {} args, got {}",
                               name, def.params.len(), arg_vals.len()));
        }
        for (key, _) in &named {
            if !def.params.iter().any(|p| p.name.0 == *key) {
                return Err(format!("{} has no parameter named '{}'", name, key));
            }
        }
        if self.depth >= 64 {
            return Err(format!("call depth exceeded inlining {} (recursive fn?)", name));
        }

        self.depth += 1;
        self.env.push_scope();
        // A body is not part of the sequence that called it. Without this a
        // bare note inside a function would take its octave from whichever
        // list the call happens to sit in, which is a spooky action the author
        // of the function cannot see and the author of the list did not write.
        let outer_octave = self.octave.take();
        let result = (|| {
            for (i, param) in def.params.iter().enumerate() {
                let by_name = named.iter().find(|(k, _)| *k == param.name.0).map(|(_, v)| v);
                let v = match (arg_vals.get(i), by_name, &param.default) {
                    (Some(_), Some(_), _) => return Err(format!(
                        "{}: '{}' given both positionally and by name", name, param.name.0)),
                    (Some(v), None, _) => v.clone(),
                    (None, Some(v), _) => v.clone(),
                    (None, None, Some(d)) => self.expr(d)?,
                    (None, None, None) => return Err(format!(
                        "{}: missing argument '{}'", name, param.name.0)),
                };
                self.env.define(&param.name.0, v);
            }
            self.expr(&def.body)
        })();
        self.octave = outer_octave;
        self.env.pop_scope();
        self.depth -= 1;
        result
    }

    /// Resolve a UGen name to its node kind and arity.
    ///
    /// Both come from the `lang::UGENS` table, which the editor also serves to
    /// the frontend for completion — so a UGen cannot be callable without also
    /// being completable, and its arity cannot disagree with its documented
    /// parameter list.
    fn builtin(&self, func: &str) -> Option<(NodeKind, usize)> {
        crate::lang::ugen(func).map(|u| (u.kind, u.params.len()))
    }
}

#[cfg(test)]
mod tests {
    use crate::lowerer::lower::lower;
    use crate::parser::parser::parse;

    fn lower_err(src: &str) -> String {
        let items = parse(src.to_string()).expect("parse failed");
        match lower(&items) {
            Err(e) => e,
            Ok(_) => panic!("expected {src} to fail lowering"),
        }
    }

    /// The arity message is derived from the table now; it must still name the
    /// same count the hand-written match arm enforced.
    #[test]
    fn wrong_arity_reports_the_expected_count() {
        assert_eq!(lower_err("lowpass(sin(220), 400)\n"), "lowpass expects 3 inputs, got 2");
        assert_eq!(lower_err("sin(220, 330)\n"), "sin expects 1 inputs, got 2");
        assert_eq!(lower_err("noise(1)\n"), "noise expects 0 inputs, got 1");
    }

    /// A name in neither table is still an ordinary unbound-function error.
    #[test]
    fn unknown_names_are_not_functions() {
        assert_eq!(lower_err("lowpas(sin(220), 400, 1)\n"), "lowpas is not a function");
    }

    // ---- named arguments ----

    /// Lower `defs` then `expr`, and read back the constant `expr` folded to.
    /// Wrapping in `sin` is the codebase's usual way to make a folded number
    /// observable in the graph.
    fn constant(defs: &str, expr: &str) -> f64 {
        use crate::swync_graph::ugen_nodes::NodeInput;
        let src = format!("{defs}sin({expr})\n");
        let items = parse(src.clone()).expect("parse failed");
        let g = crate::lowerer::lower::lower_graph(&items)
            .unwrap_or_else(|e| panic!("{src}: {e}"));
        match g.nodes[0].inputs[0] {
            NodeInput::Const(v) => v,
            _ => panic!("expected a folded constant"),
        }
    }

    #[test]
    fn a_named_argument_binds_to_its_parameter() {
        assert_eq!(constant("fn f(a, b) = a * 10 + b\n", "f(1, b: 2)"), 12.0);
    }

    /// Order at the call site is free, because names carry the binding.
    #[test]
    fn named_arguments_may_be_written_in_any_order() {
        assert_eq!(
            constant("fn f(a, b, c) = a * 100 + b * 10 + c\n", "f(1, c: 3, b: 2)"),
            123.0
        );
    }

    /// The property the whole lane design rests on: a parameter no argument
    /// filled still gets its default, evaluated where earlier params are bound.
    #[test]
    fn defaults_still_see_earlier_parameters() {
        assert_eq!(constant("fn f(a, b = a * 2, c = b + 1) = c\n", "f(5)"), 11.0);
        assert_eq!(constant("fn f(a, b = a * 2, c = b + 1) = c\n", "f(5, b: 1)"), 2.0);
    }

    #[test]
    fn an_unknown_named_argument_is_an_error() {
        let err = lower_err("fn f(a) = a\nf(1, nope: 2)\n");
        assert!(err.contains("no parameter named 'nope'"), "got: {err}");
    }

    #[test]
    fn filling_one_parameter_twice_is_an_error() {
        let err = lower_err("fn f(a, b) = a + b\nf(1, 2, b: 3)\n");
        assert!(err.contains("both positionally and by name"), "got: {err}");
    }

    #[test]
    fn a_positional_argument_after_a_named_one_is_an_error() {
        let err = lower_err("fn f(a, b) = a + b\nf(a: 1, 2)\n");
        assert!(err.contains("must come before named"), "got: {err}");
    }

    /// UGens and builtins have no parameter names to bind against, so a name
    /// aimed at one has to fail rather than be quietly dropped.
    #[test]
    fn builtins_reject_named_arguments() {
        let err = lower_err("sin(freq: 440)\n");
        assert!(err.contains("user `fn`"), "got: {err}");
        let err = lower_err("max(1, b: 2)\n");
        assert!(err.contains("user `fn`"), "got: {err}");
    }

    /// A missing argument is still missing when the name given is a different
    /// parameter's.
    #[test]
    fn a_gap_left_by_names_is_still_reported() {
        let err = lower_err("fn f(a, b) = a + b\nf(b: 2)\n");
        assert!(err.contains("missing argument 'a'"), "got: {err}");
    }

    /// Every UGen in the table really lowers, at its documented arity.
    #[test]
    fn every_ugen_lowers_at_its_documented_arity() {
        for u in crate::lang::UGENS {
            let args = vec!["1"; u.params.len()].join(", ");
            let src = format!("{}({})\n", u.name, args);
            let items = parse(src.clone()).unwrap_or_else(|e| panic!("{src} failed to parse: {e}"));
            assert!(lower(&items).is_ok(), "{src} failed to lower");
        }
    }
}

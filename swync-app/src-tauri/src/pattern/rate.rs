//! How fast a pattern runs, as a function of when.
//!
//! A rate is a change of clock, and the only thing `Pattern::Fast` ever needed
//! of it was two maps: *outer* bar time — the clock everything is scheduled
//! against — to the *inner* time the pattern is written in, and back again. A
//! constant rate does both by multiplying. Anything that varies has to
//! integrate instead, because the inner time reached at some moment is the
//! whole history of the rate up to it and not the rate there.
//!
//! That integral is why the shapes here are a closed enum rather than a
//! callback. Both directions have to be *exact*: the inverse places every
//! onset the query answers with, and the lowerer needs it a second time to say
//! how many bars a bounded section lasts, long before anything is scheduled.
//! A numeric integration would answer both a little differently each time it
//! was asked, and a section whose length disagrees with its own last note is a
//! gap in an arrangement.

/// The rate a pattern runs at.
///
/// Two things use this, and they ask opposite questions. `Pattern::Fast` asks
/// *where are we in the pattern at this bar* — [`phase`](Rate::phase), the
/// integral. The lowerer asks *how many bars until the pattern has run this
/// far* — [`unphase`](Rate::unphase), its inverse, which is what gives a
/// bounded section a length.
///
/// **Only a varying rate is anchored.** `Fixed` ignores the anchor it is
/// handed: a constant rate is periodic, so nothing about the sound says where
/// it started, and patterns are deliberately laid on one grid running from the
/// clock's origin — a `.then` cuts a window onto that grid rather than starting
/// a pattern of its own. Anchoring a constant rate would shift every existing
/// arrangement off that grid to no audible end. A curve has a real beginning,
/// on the other hand: `accel(1, 2, 8)` means eight bars from *this section's*
/// first note, so it is measured from where its window opens and starts again
/// each time that window comes back around.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rate {
    /// One speed throughout — every `rate` written before `accel` existed.
    Fixed(f64),
    /// A straight line in rate: `from` to `to` over `bars` bars, holding at
    /// `to` after that. Held rather than continuing, because `play` loops
    /// forever and a rate that kept climbing would end at a smear.
    Accel { from: f64, to: f64, bars: f64 },
}

impl Rate {
    /// A line in rate, reduced to a `Fixed` wherever it is not really a curve.
    ///
    /// Both degenerate cases fold rather than fail. Equal ends *are* a constant
    /// rate, and a curve given no time to happen in is its own endpoint — the
    /// same reading `line` gives a zero duration. Folding them here is what
    /// lets the rest of this file divide by `bars` and by `to - from`
    /// without asking first.
    pub fn accel(from: f64, to: f64, bars: f64) -> Rate {
        if !bars.is_finite() || bars <= 0.0 || from == to {
            return Rate::Fixed(to);
        }
        Rate::Accel { from, to, bars }
    }

    /// Is this a speed a pattern can actually be played at?
    ///
    /// Zero stops time and a negative runs it backwards; neither has a phase
    /// this can invert, and both used to be caught by `Fast`'s own guard. The
    /// lowerer refuses them at the call site, where there is somewhere to say
    /// so — this is what `query` falls back on for a pattern built any other
    /// way.
    pub fn is_usable(&self) -> bool {
        match self {
            Rate::Fixed(r) => *r > 0.0 && r.is_finite(),
            Rate::Accel { from, to, bars } => {
                *from > 0.0 && *to > 0.0 && from.is_finite() && to.is_finite() && *bars > 0.0
            }
        }
    }

    /// Nothing to do: the pattern already runs at the clock's own speed, so
    /// `play` leaves it unwrapped rather than adding a `Fast` that multiplies
    /// by one.
    pub fn is_unit(&self) -> bool {
        matches!(self, Rate::Fixed(r) if *r == 1.0)
    }

    /// The inner time the pattern has reached by outer bar `at`.
    ///
    /// For a curve this is ∫r from the anchor, which for a straight line in
    /// rate is the area of a trapezium — `from * c` plus the triangle the
    /// change adds. Outside the curve it stays linear at the rate in force
    /// there, so phase is continuous and strictly increasing over the whole
    /// line: before the anchor at `from`, after the curve at `to`. Time before
    /// a section opens is not usually asked for, but a plain `play` joins a
    /// performance already in progress and is queried from wherever the clock
    /// is, which can be a moment before the origin it counts from.
    pub fn phase(&self, at: f64, anchor: f64) -> f64 {
        match self {
            Rate::Fixed(r) => at * r,
            Rate::Accel { from, to, bars } => {
                let c = at - anchor;
                if c <= 0.0 {
                    c * from
                } else if c >= *bars {
                    (from + to) * bars / 2.0 + (c - bars) * to
                } else {
                    from * c + (to - from) * c * c / (2.0 * bars)
                }
            }
        }
    }

    /// The speed the pattern is running at *at* outer bar `at` — the slope of
    /// [`phase`](Rate::phase) rather than its value.
    ///
    /// Neither of the two questions this type was built for; it is what a voice
    /// asks. A note wanting to shape itself in beats needs the beat it is being
    /// played on, and under a curve that is a different length for every note.
    /// Piecewise exactly as `phase` is — flat at `from` before the anchor and at
    /// `to` past the curve's end — so the beat a note is given is the beat its
    /// own placement was worked out from.
    pub fn at(&self, at: f64, anchor: f64) -> f64 {
        match self {
            Rate::Fixed(r) => *r,
            Rate::Accel { from, to, bars } => {
                let c = at - anchor;
                if c <= 0.0 {
                    *from
                } else if c >= *bars {
                    *to
                } else {
                    from + (to - from) * c / bars
                }
            }
        }
    }

    /// The outer bar at which the pattern reaches inner time `phase` — the
    /// inverse of [`phase`](Rate::phase), and where every onset a query found
    /// is actually played.
    ///
    /// The middle branch inverts a quadratic, written as `2p / (from + √…)`
    /// rather than the schoolbook `(-from + √…) / 2k`. The two are the same
    /// root, but the schoolbook form divides by a `k` that goes to zero as the
    /// curve flattens while its numerator cancels to nothing — losing every
    /// digit exactly where a gentle accelerando lives. This form has no `k` in
    /// it to divide by and degrades into `p / from`, which is the right answer
    /// for a curve that is not curving.
    ///
    /// The discriminant cannot go negative here: it is the rate at that point
    /// squared, and this branch is only reached inside the curve, where the
    /// rate is between two positive ends.
    pub fn unphase(&self, phase: f64, anchor: f64) -> f64 {
        match self {
            Rate::Fixed(r) => phase / r,
            Rate::Accel { from, to, bars } => {
                let full = (from + to) * bars / 2.0;
                let c = if phase <= 0.0 {
                    phase / from
                } else if phase >= full {
                    bars + (phase - full) / to
                } else {
                    let k = (to - from) / (2.0 * bars);
                    2.0 * phase / (from + (from * from + 4.0 * k * phase).sqrt())
                };
                anchor + c
            }
        }
    }
}

impl From<f64> for Rate {
    fn from(r: f64) -> Rate {
        Rate::Fixed(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two directions have to agree, or an onset is played somewhere the
    /// pattern did not put it. Checked across the whole shape: before the
    /// curve, inside it, and out past its end.
    #[test]
    fn phase_and_unphase_are_inverses() {
        let rates = [
            Rate::Fixed(1.0),
            Rate::Fixed(0.25),
            Rate::accel(1.0, 2.0, 8.0),
            Rate::accel(2.0, 0.5, 3.0),
            // Barely a curve at all, which is where the naive inverse fails.
            Rate::accel(1.0, 1.0000001, 4.0),
        ];
        for rate in rates {
            for c in [-2.0, -0.001, 0.0, 0.5, 1.0, 2.5, 7.99, 8.0, 12.0, 400.0] {
                let there = rate.phase(c + 5.0, 5.0);
                let back = rate.unphase(there, 5.0);
                assert!(
                    (back - (c + 5.0)).abs() < 1e-9,
                    "{rate:?} at {c}: phase {there}, back to {back}"
                );
            }
        }
    }

    /// A fixed rate is exactly what it was before this type existed: multiply
    /// going in, divide coming out, and the anchor makes no difference.
    #[test]
    fn a_fixed_rate_ignores_its_anchor() {
        let rate = Rate::Fixed(2.0);
        assert_eq!(rate.phase(3.0, 0.0), 6.0);
        assert_eq!(rate.phase(3.0, 100.0), 6.0);
        assert_eq!(rate.unphase(6.0, 0.0), 3.0);
        assert_eq!(rate.unphase(6.0, 100.0), 3.0);
    }

    /// The trapezium: a run from 1x to 3x over 4 bars covers the 8 bars of
    /// pattern that the average of the two rates says it should, and holds that
    /// end rate afterwards.
    #[test]
    fn a_curve_covers_the_area_under_it() {
        let rate = Rate::accel(1.0, 3.0, 4.0);

        assert_eq!(rate.phase(0.0, 0.0), 0.0);
        // Halfway along, the rate is 2x and the area is 1*2 + 2/2 = 3.
        assert!((rate.phase(2.0, 0.0) - 3.0).abs() < 1e-12);
        assert!((rate.phase(4.0, 0.0) - 8.0).abs() < 1e-12);
        // Past the end it runs on at 3x.
        assert!((rate.phase(6.0, 0.0) - 14.0).abs() < 1e-12);
    }

    /// What the lowerer asks: how long does a section of `n` passes last? A
    /// pattern accelerating from 1x to 3x gets eight passes done in the four
    /// bars the same eight would have taken five and a bit of at 1.5x.
    #[test]
    fn a_curve_says_how_long_a_section_lasts() {
        let rate = Rate::accel(1.0, 3.0, 4.0);
        assert!((rate.unphase(8.0, 0.0) - 4.0).abs() < 1e-12);
        // And a fixed rate still just divides.
        assert_eq!(Rate::Fixed(2.0).unphase(8.0, 0.0), 4.0);
    }

    /// The speed at a moment is the slope of the phase there, which is what a
    /// voice is told when it asks how long a beat is. Checked against a
    /// difference quotient rather than against the formula written a second
    /// time, so the two really are the same curve.
    #[test]
    fn the_rate_at_a_moment_is_the_slope_of_its_phase() {
        let rate = Rate::accel(1.0, 3.0, 4.0);
        let h = 1e-6;
        for c in [0.5, 1.0, 2.0, 3.9] {
            let slope = (rate.phase(c + h, 0.0) - rate.phase(c - h, 0.0)) / (2.0 * h);
            assert!((rate.at(c, 0.0) - slope).abs() < 1e-5, "at {c}: {} vs {slope}", rate.at(c, 0.0));
        }
        // Flat outside the curve, at either end's speed.
        assert_eq!(rate.at(-1.0, 0.0), 1.0);
        assert_eq!(rate.at(0.0, 0.0), 1.0);
        assert_eq!(rate.at(4.0, 0.0), 3.0);
        assert_eq!(rate.at(100.0, 0.0), 3.0);
        // And a fixed rate is that rate wherever it is asked.
        assert_eq!(Rate::Fixed(2.0).at(7.0, 3.0), 2.0);
    }

    /// Degenerate curves are constants, which is what keeps every division in
    /// here safe rather than guarded.
    #[test]
    fn a_curve_that_does_not_curve_is_a_fixed_rate() {
        assert_eq!(Rate::accel(2.0, 2.0, 4.0), Rate::Fixed(2.0));
        assert_eq!(Rate::accel(1.0, 2.0, 0.0), Rate::Fixed(2.0));
        assert_eq!(Rate::accel(1.0, 2.0, f64::NAN), Rate::Fixed(2.0));
    }

    /// Rates a pattern cannot run at. Zero and below stop or reverse time; the
    /// lowerer refuses them, and `query` needs to recognise them too.
    #[test]
    fn a_stopped_or_reversed_rate_is_not_usable() {
        assert!(Rate::Fixed(0.5).is_usable());
        assert!(!Rate::Fixed(0.0).is_usable());
        assert!(!Rate::Fixed(-1.0).is_usable());
        assert!(!Rate::Fixed(f64::INFINITY).is_usable());
        assert!(Rate::accel(1.0, 4.0, 2.0).is_usable());
        assert!(!Rate::accel(1.0, 0.0, 2.0).is_usable());
        assert!(!Rate::accel(0.0, 1.0, 2.0).is_usable());
    }

    /// Monotonic everywhere, which is what makes the inverse a function at all
    /// — and what lets `onsets_before` keep counting in inner time.
    #[test]
    fn phase_never_goes_backwards() {
        let rate = Rate::accel(0.5, 4.0, 6.0);
        let mut last = f64::NEG_INFINITY;
        for i in -50..500 {
            let p = rate.phase(i as f64 / 10.0, 0.0);
            assert!(p > last, "phase went backwards at {i}");
            last = p;
        }
    }
}

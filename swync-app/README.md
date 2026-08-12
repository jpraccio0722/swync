# Swync

A live coding language and environment, written in Rust, wrapping much of
[fundsp's](https://github.com/SamiPerttu/fundsp) functionality. 

```bash
npm install && npm run tauri dev
```

`Cmd/Ctrl + ,` evaluates the current tab

`Cmd/Ctrl + .` stops audio.

The play button is lit green while the engine is holding a program, and goes
dark when it is stopped — so what is running is readable at a glance, from
across a room. While a take is being recorded the red record button is the
light instead, since the two would otherwise be making the same claim twice.


## Imports

A file can use another file's definitions. The spelling is Rust's:

```rust
use kit                      // the module, as kit::kick, kit::snare
use kit as k                 // the same, as k::kick
use kit::kick                // one name, on its own
use kit::{kick, hat as tick} // several, renamed where you like
use kit::*                   // everything the file defines
```

**A `use` does not run the file it names.** Its `fn`s and `let`s come across;
Top level expressions and `play`s in the imported file do not run.

Imports resolve relative to the file on disk, and read what is saved: save a
module before playing a file that uses it.

Because a path is a route rather than a name, renaming or moving a file in the
project panel would change what every `use` of it means. So the panel corrects
them: the paths are rewritten to point where the file went, and nothing else in
those files is touched. A whole-module import keeps the name it was bound
under, so the body of the importing file still reads the same —

```rust
use drums               // rename drums.swync to percussion.swync, and this
use percussion as drums // becomes this, so every drums::kick still resolves
```

A `use` path only goes downward: there is no way to name a folder above the one
you are in. Drag a module somewhere its importer cannot reach and no rewrite is
honest, so the move still happens and the imports that broke are named in the
problems panel.

Everything else follows from names. An imported `fn` is an instrument like any
other, qualified or not:

```rust
play([\, `, \, `], kit::kick)
```


## Libraries

A library is a folder of modules somebody else wrote. Install one and every
project on the machine can `use` it — the spelling does not change, because an
installed library is a folder a `use` resolves in like any other:

```rust
use kit               // kit::kick, kit::hat
use kit::drums::*     // however you would write it for a file in the project
```

**File ▸ Install Library…** takes a `.swyncpack` — a zip with a name on it —
and the **Libraries** tab in the left panel lists what is installed. Each row
can be revealed on disk, removed, or **vendored**: copied into the project, so
the project stops depending on what happens to be installed here. Hand that
folder to somebody, or check it into a repository, and it still plays.

A `use` is answered by the first of these that has the file, and the project is
always asked in full before either store:

| | |
|---|---|
| beside the file | as it always was |
| `.swync/libraries/` | the copies this project carries |
| the machine's store | everything installed |

So a `kit.swync` in the project wins over a `kit` that is installed, and
installing something can never change what a project that already worked means.
The Libraries tab says so on any row a project file is hiding.

### Making one

**File ▸ Export as Library…** packs the project up. What is packed is named by a
`swync-library.json` beside the settings file, written for you on the first
export and yours to edit after:

```json
{
  "name": "kit",
  "version": "1.2.0",
  "author": "Somebody",
  "description": "Nine drums and the samples for them."
}
```

**A library owns one name, and packs exactly what is under it** — `kit.swync`
and `kit/`, with whatever samples are in there. That one rule is what makes
installing safe: two libraries can never write the same file, so there is never
a question of which one won, and no version resolution behind it to get wrong.
Installing over a library already there says what is installed and what is
arriving, and replaces it only if you say so.

Exporting refuses anything that would not survive the trip to another machine —
an absolute `load` path, a path that reaches out of the library, a `use` at the
top of it that names something the library does not carry. All three work
perfectly on the machine they were written on, which is why they are worth
catching there.


## Drawn patterns

The right-hand panel draws patterns on a grid: click a cell to walk it through
rest → trigger → pitch. Each row has a name, and that name is what the editor
plays:

```rust
play(hats, hat)
```

Rows are saved with the project, in a `patterns.swync` beside your files:

```rust
let hats = [\, `, \, `]
let riff = [c4, e4, `, g4]
```

That file is folded into every program the project runs, so a drawn pattern
needs no `use` — it is simply in scope.

Being a real file, it goes both ways: open it in a tab, edit it by hand, save,
and the panel redraws from what you wrote — including a row that
[carries its octave](#notes-and-octaves), like `[a1, a, a, a]`. Draw in the
panel and the file is rewritten, with every octave spelled out: a grid is cells
and has no register to write down. Anything else you keep in that file is lost
the next time a row changes.


## Samples

`load` reads an audio file into a buffer. The path is relative to the file it is
written in, exactly like a `use` path, and any format symphonia reads will do —
wav, mp3, flac, ogg:

```rust
let amen = load("breaks/amen.wav")
```

That holds inside an imported module too: a path written in `kit/drums.swync`
is relative to `kit/`, wherever the file that imported it lives. It is what lets
a library carry its own samples.

A buffer is not audio. Nothing comes out of it until `sample` reads it **at a
position**, where 0 is the start of the buffer and 1 is the end:

```rust
sample(amen, ramp(1 / amen.secs))       // forwards, at its own speed
```

That is the whole interface. There is no play, no stop, no rate and no reverse,
because a position is a signal and everything those would do is arithmetic on
it:

```rust
sample(amen, 1 - ramp(1 / amen.secs))       // backwards
sample(amen, ramp(2 / amen.secs))           // twice as fast, an octave up
sample(amen, ramp(0.5 / amen.secs))         // half speed
sample(amen, ramp(4 / amen.secs) * 0.25)    // the first quarter, looping four times a pass
sample(amen, 0.5 + ramp(4 / amen.secs) * 0.25)  // the third quarter
sample(amen, ramp(1 / amen.secs) >> hold(16, 0))  // stuttered into sixteen steps
```

`ramp(f)` is the phasor: a rising 0 to 1, `f` times a second. `secs` is how long
the buffer is, so `1 / amen.secs` is the frequency that reads it exactly once —
and any multiple of that is a speed. Reading outside 0..1 is silence rather than
a held edge, so a position that overshoots goes quiet instead of clicking.

### A portion, once

One thing on that list is easy to write wrong, and it is the most common one:
**a portion of the buffer, at the buffer's own speed.** `ramp(...) * 0.25` reads
the first quarter, but the phasor still takes the whole buffer to get there, so
it reads that quarter at a quarter speed. Reading a portion properly means
scaling the frequency to match the span — and then saying the span twice, once
in each.

`slice` is that written for you. Name the two ends and it draws the line between
them:

```rust
slice(amen, 0, 0.25)        // the first quarter, once, at its own speed
slice(amen, 0.5, 0.75)      // the third quarter
slice(amen, 0.5, 0.25)      // that quarter backwards
slice(amen, 0, 1)           // the whole break, once, no loop
```

Both ends are numbers written in the source rather than signals, and both are
refused outside 0..1 — a slice you can point at is a slice that cannot silently
read nothing.

A fourth argument is the **rate**, a multiple of that own speed:

```rust
slice(amen, 0, 0.25, 2)     // the first quarter in half the time, an octave up
slice(amen, 0, 0.25, 0.5)   // in twice the time, an octave down
slice(amen, 0.5, 0.25, 2)   // backwards, twice as fast
```

Reading faster is reading fewer of the buffer's samples per second of output,
so a rate is a pitch as much as a tempo — there is no way to have one without
the other here. It has to be above zero: at zero the reader would stop rather
than slow, which is the silent DC offset the whole builtin exists to avoid.
Backwards is the ends written the other way round, not a negative rate.

All of it is exactly `sample` with the phasor filled in:

```rust
slice(amen, 0.25, 0.5)
sample(amen, line(0.25, 0.5, 0.25 * amen.secs))   // the same graph

slice(amen, 0.25, 0.5, 2)
sample(amen, line(0.25, 0.5, 0.25 * amen.secs / 2))   // and the same again
```

Which means the one thing to know about it is `line`'s: it **holds** on the last
sample of the slice rather than falling silent, so a portion ending anywhere but
the end of the buffer leaves a DC offset behind it. An envelope is what ends the
note — the same thing that ends every other voice:

```rust
fn slam() = slice(load("door.wav"), 0, 0.15) * perc(0.001, 0.2)
```

A slice ending at `1` needs no such care: past the buffer is already silence.

Reach for `sample` when the position or the speed has to **move** — scrubbing,
stuttering, anything modulated. Reach for `slice` when you know the ends and the
rate when you write them, which is most drums.

Chopping from a pattern is the same arithmetic with the portion as a lane:

```rust
// Sixteenths of the break, played as a pattern. `at` is where in the buffer
// this note starts; the phasor covers one sixteenth from there.
fn chop(n, at = 0) =
  sample(load("breaks/amen.wav"), at + ramp(n) * 0.0625) * perc(0.001, 0.2)

play([\, \, \, \, \, \, \, \], chop, 1,
     at: [0, 0.25, 0.5, 0.0625, 0.75, 0.5, 0.125, 0.875])
```

A lane value is a number by the time the note is built, so it is a `slice` end
like any other — which is the same chop with the phasor left to the language,
and at the break's own speed rather than `n`'s:

```rust
fn chop(n, at = 0) =
  slice(load("breaks/amen.wav"), at, at + 0.0625) * perc(0.001, 0.2)
```

**An instrument names its own file.** A `fn` sees only other `fn`s — a top-level
`let` is the persistent graph's, not a voice's — so `let amen = load(...)` above
a `fn` that reads `amen` will not compile. Write the `load` inside the
instrument, as above. It costs nothing: one path is one buffer however many
`load`s name it, so the inline spelling shares the same audio as everything
else that reads that file.

Three more things worth knowing:

- **Every file is decoded once, before the program runs.** A path is found by
  reading the program, not by running it, which is why it has to be written out
  rather than assembled — `load(name)` will not compile. It also means no note
  ever waits on a disk, and re-evaluating costs nothing however long the file
  is.
- **A buffer is stored once** however many times it is read, so chopping a break
  sixteen ways is sixteen readers over one copy of the audio.
- **Reading interpolates** (cubic), so a break holds up away from its own speed
  rather than aliasing.

`channels` says how many channels a file has, and `sample`'s optional third
argument picks one — it defaults to 0 and wraps, so asking a mono file for
channel 1 gives you the mono back rather than an error:

```rust
let stereo = load("pad.wav")
let pos = ramp(1 / stereo.secs)

// Both channels of a stereo file, summed. An instrument is mono, so this is
// where a stereo file stops being stereo — use `pan:` on the `play` to place
// the voice.
(sample(stereo, pos, 0) + sample(stereo, pos, 1)) * 0.5
```


## Audio in and out

Which devices the sound comes from and goes to are in the **Settings** tab of
the right-hand panel, and both are remembered for this machine rather than for
the piece — an interface is on a desk, not in a project somebody hands you.

**Output** is the system's own choice unless you pick a device. Picking one
takes effect immediately: the graph is moved onto it, and if it opens at a
different sample rate everything that counts in frames — the beat, the pattern
voices, the recorder — is moved with it, so the music keeps its place and its
pitch. It cannot be changed while a take is running, because a WAV names one
rate in its header for the whole file.

**Input** is off until you choose a device, and off is a real default rather
than an oversight: an app that opened a microphone the first time it ran would
ask for a permission nobody asked it to want, and put an open mic in a room
with speakers in it. Choose one and its channels become signals:

```rust
input(0)                        // the first channel, as it arrives
input(0).lowpass(800, 1)        // through a filter
input(0) + input(0).reverb(10, 3, 0.5) * 0.3
```

`input(0)` is the first channel, `input(1)` the second, and so on up the
device's count. It is a signal like any other, so it filters, delays and sums
exactly as an oscillator does — and it works inside an instrument, so a pattern
can play the room:

```rust
fn stab() {
    input(0) * perc(0.001, 0.15)
}

play([\, \, `, \], stab)
```

Nothing about it is special-cased, which also means nothing protects you from
the obvious: `input(0)` played through speakers that the microphone can hear is
a feedback loop, and it will find the room's resonance faster than you can
reach the fader. Headphones, or a `* 0.1` while you find out what you have.

Two things are worth knowing:

- **Both devices have to run at the same sample rate.** They feed one graph and
  a graph is rendered at one rate, so an input that cannot run at whatever the
  output opened at is refused, by name, rather than opened and quietly
  resampled — which would be a pitch error with nothing to point at its cause.
  On a Mac, Audio MIDI Setup is where the two are matched.
- **A channel the device does not have is silence**, not an error. A piece
  written against an eight-in interface still opens, compiles and runs on the
  laptop it is edited on.

Two meters sit beside the transport in the title bar. **`out`** is always
there: two bars, taken from the same block the device is filled from, so what
it shows is what the room hears — the master fader included, and turning red
when it clips. **`in`** appears once an input device is chosen, one bar per
channel, so you can see which `input(n)` is which before writing anything.

Both are peak meters over the last tenth of a second rather than levels at the
instant they are drawn: most samples of most signals are nowhere near the peak,
and a meter you are setting a gain by has to catch the transient rather than
average it away.

If the settings panel ever reports frames arriving late or being dropped, the
two devices are not keeping step with each other, and a larger buffer size on
either one usually settles it.

## Recording

The **record** button sits beside play and stop. Pressing it **plays the file
and records it** — one gesture, because what you want on disk is the piece from
its first note, and pressing play and then record loses the attack. Everything
the engine is making goes to the file: the persistent graph, the pattern
voices, and the master fader as you move it. What you get is the performance as
it was heard, evals and all — there is no offline render, because half of what
a live-coded piece is is *when* you changed it.

While a take runs the button is a stop square carrying its length, so it is
also the clock. Press it and the music stops and the file is finished and
saved — the fade-out is recorded too, so a take ends on the release of its last
note rather than on a click.

Press record again for a second take.

If the file will not compile, the take is closed straight away and the problems
panel says why — a red button recording the silence after a typo helps nobody.
The empty file is left where it was made rather than removed.

Where the file goes and what it is written as are in the **Settings** tab of
the right-hand panel, decided before a take rather than at the moment you press
record:

- **Output folder.** The open project's own folder unless you choose another.
  A folder you choose is remembered for every project on this machine — it is a
  path into *this* disk, so it lives with the app rather than in the project's
  `swync-project.json`, which is meant to be shared.
- **File type.** `WAV — 16-bit` is what everything can read. `WAV — 32-bit
  float` is exactly what the engine rendered, headroom and all, and is what to
  record if the take is going to be mixed. Nothing else is offered, because
  nothing else can be written.

A recording is named for the project and the moment it was played — `Night
Piece 2026-08-08 21-14-03.wav` — and nothing is ever recorded over: a name
already on the disk gets a number rather than being replaced.

Two things are worth knowing about the file:

- It is written as you play, and its header is brought up to date once a
  second. A session that ends in a crash still leaves a playable take.
- A recording is a WAV, so it stops at four gigabytes — about six hours of
  16-bit stereo. At that point the take is closed properly and the problems
  panel says so.

A finished take is listed at the bottom of the settings panel, with a Reveal
that opens it in the Finder. And since it is an ordinary audio file, the next
program can `load` it.

## Notes and octaves

A pitch is written as a letter, an optional `s` for sharp or `f` for flat, and
an octave: `c4` is middle C and MIDI 60, `af3` is A flat below it, `gs9` is the
top of the range. Octaves run 0 to 9, and enharmonics are allowed — `bs3` is
`c4`, `cf4` is `b3`. Flats are `f` rather than `b` because `b` is already a note
and `db3` would collide with the `db` builtin.

A note is a plain number once it is read, so everything numeric works on it:
`c4.oct(1)`, `c4.semi(7)`, `c4 + 12`, `c4.m2h`.

**The octave carries.** Inside a sequence, a note written without one takes the
octave of the last note that spelled one, the same way a written value carries
its length:

```rust
play([a1;q, a, a, a], bass)              // four a1s
play([c4;q, ef, g, c5, ef, g], lead)     // two arpeggios, an octave apart
```

Only a spelled octave moves the register, so the octave of any note is found by
reading backwards to the nearest digit. A step that *computes* its pitch —
`c4.oct(1)`, `n + 12` — lands somewhere the text never says, so it carries
nothing, and a bound name shadows a bare letter exactly as it shadows `c4`.

A group takes the octave in with it and gives it back at the closing bracket,
which is the rule a written value already follows: in
`[c4;q, [g, b, d5];t, g]` that last `g` is g4, not g5.

There is nothing to carry outside a sequence, so a bare letter there is an
error rather than a guess — which is what keeps `f`, `a` and `e` usable as
ordinary parameter names.

**`e` is the one collision.** It is the eighth note and also the note E. In a
sequence that is already in an octave either could be meant, so the step is
refused instead of quietly becoming one of them:

```rust
play([c4;q, e, g], lead)      // refused: is `e` an eighth, or is it E?
play([c4;q, e4, g], lead)     // the note
play([c4;q, \;e, g], lead)    // a pitchless hit, an eighth long
```

Nowhere else is `e` ambiguous. With no octave in force it is the eighth it has
always been, and after a `;` it is a length and never a pitch.


## Rhythm in note values

Two words do all the work here, and they are not the same word:

- A **bar** is the grid. It is four beats in 4/4, three in 3/4, and it is what
  `take`, `at`, `then_after` and `quantize` count. The transport's own
  signature says how long it is.
- A **pass** is one trip through a pattern. A bare list is one pass and fills
  the bar exactly, in any signature.

A list divides its bar by *ratio*: `[220;2, 330, 440]` is a half and two
quarters, and `[220;8, 330;4, 440;4]` is the same rhythm, because only the
proportions matter. That is how every pattern here works, and it stays that way.

Written note values are the other way of saying it. `w` is a whole note, `h` a
half, `q` a quarter, `e` an eighth and `s` a sixteenth, and a pass written in
them is as long as they add up to rather than always one bar:

```rust
play([c4;q, e4, g4], lead)      // three beats
play([c4;q, e4, g4, c5], lead)  // four quarters — one bar, in 4/4
```

A value carries to the steps after it, so a pass says what it is once — the
same way [an octave carries](#notes-and-octaves), and a melody on one line can
say both once: `[c4;q, ef, g, c5]`. A bare `q` is a hit of that length — a
written value carries no pitch, which is what `\` already means — so
`[q, q, q]` is three quarter-note hits.

**Dots and ties.** `q.dot` is a dotted quarter, and `dot` takes a count for the
rest: `q.dot(2)` is the doubly-dotted quarter, a quarter and an eighth and a
sixteenth. It is a count rather than a repeated `.dot` because each dot adds half
of *the note*, not half of what the last one left. Adding two values ties them —
`h + e` is a half held into an eighth, as one note rather than two — and nothing
else is arithmetic notation can draw on them, so nothing else is allowed:

```rust
play([c4;q.dot, e4;e], lead)     // a dotted quarter and an eighth: two beats
play([c4;h + e, e4;e], lead)     // the same length, spelled as a tie
```

### The signature, and playing against it

The beat is always the quarter note — the one `bpm` counts. How many of them
make a bar is the project's **time signature**, set beside the tempo in the
title bar and remembered in `swync-project.json`. In 4/4 a bar is four beats;
in 3/4 it is three.

That is what decides whether a pass fills its bar or runs against it:

```rust
// In 4/4: three beats against a four-beat bar.
play([c4;q, e4, g4], lead)
```

This pass is three quarters of a bar, so it comes round a beat early and its
downbeat walks — beat 1, then 4, then 3, then 2, back to 1 after three bars.
That is polymeter, and it is exactly what makes the line play against a
four-square drum part.

Set the project to 3/4 and the same line fills the bar, once per bar, with no
walking at all — because the bar moved to meet it. Neither reading is a
special case: a pass fills the bar when its values add up to one, and rotates
when they do not.

Two things the signature deliberately does **not** move. The **quarter note**:
120 bpm is 120 bpm in any meter, so switching to 3/4 shortens the bar rather
than speeding the music up. And **`w`**, the whole note, which is four beats
wherever it is written — in 3/4 that is more than a bar, which is as it should
be.

6/8 and 3/4 are the same length here — six eighths, three quarters — and Swync
does not tell them apart, because what separates them is where the accents
fall and that is yours to write.

That same beat is available inside an instrument as a number, for writing a
sweep or an envelope in note values rather than in seconds — see [Syncing to the
beat](#syncing-to-the-beat).

### Tuplets

A group inside such a sequence is a tuplet, and says so with `;t`:

```rust
play([[c4;q, e4, g4];t, c5;q], lead)     // a quarter triplet, then a quarter
play([[c4;e, d4, e4, f4, g4];t], lead)   // an eighth quintuplet
```

`;t` carries no number because there is none left to carry. The count is what the
group holds, the unit is the shortest value in it, and the span it is played in
is the next lower power of two — three quarters in the time of two, five eighths
in the time of four. Both of those come to a half note, which is why a quarter
triplet and an eighth quintuplet fill the same space.

The contents have to *fill* the division — every slot accounted for, by a note, a
rest, or a longer note covering several:

```rust
play([[c4;h, e4;q];t], lead)             // fine: three quarters, the first two tied
play([[c4;q, e4;e, g4];t], lead)         // refused: four eighths is a half note,
                                         // so there is nothing to compress
```

That last rule is the whole of "which tuplets are acceptable". A group whose
contents come to a plain duration would be played in exactly the time it is
written, so `;t` would be claiming a compression that is not there.

A value set inside a group stops there: `[c4;q, [e4;e, f4, g4];t, c5]` leaves
`c5` a quarter. An octave set inside one stops there too — one rule for both
registers, so a bracket never has to be read twice.

### The two cannot be mixed

A ratio and a written value mean different things by the same `;` — one is a
share of whatever else is in the brackets, the other a length regardless of them
— so a sequence holding both is refused rather than resolved in one of their
favours. The same goes for a lane: a lane is read by note, so a `;` there is how
many notes the value covers, and a length of *time* is not a position.

Drawn patterns are ratios, since the grid is cells — so a drawn row is one pass
filling the bar whatever the signature is, and the grid rules itself into that
many beats. A row written in note values is passed over by the panel rather
than redrawn as the wrong rhythm — keep those in a file of your own.

### A sequence a `for` builds

A `for` whose body is values collects them into a list, and the body may end in
a `;` saying how long one step of that list is:

```rust
play(for i in 0..=4 { f4;e }, lead)          // five eighths on the one note
play(for i in 0..=7 { 60 + i;s }, lead)      // a rising run of sixteenths
```

That is the only way a *generated* sequence reaches note values. Every other
list — `rands`, `walki`, a range — is elements and nothing else, so it can only
ever be read as shares; the `;` is written where a step is written, and a `for`
collecting steps is the one place besides a list literal that there is one.

The length is read inside the loop, so it can be computed from what the step was
made of:

```rust
play(for i in 1..=3 { 220 * i;i }, lead)     // shares 1, 2 and 3 of the bar
```

A loop that turns out to be audio or plays rather than a sequence has no steps
to measure, and a `;` on one of those is refused.


## Function Reference

- [Patterns and Playback](#patterns-and-playback)
- [Samples](#sample-functions)
- [List Functions](#list-functions)
- [Math Functions](#math-functions)
- [Random Numbers](#random-numbers)
- [Oscillators and Sources](#oscillators-and-sources)
- [Noise and Chaos](#noise-and-chaos)
- [Filters](#filters)
- [Envelopes and Dynamics](#envelopes-and-dynamics)
- [Delays and Effects](#delays-and-effects)

Every name here takes its first argument on the left of a dot, so `f(a, b)`,
`a >> f(b)` and `a.f(b)` are three spellings of one call. The editor offers only
the names that suit whatever is in front of the dot.

The notes below are the same ones the editor shows, and are generated from the
same tables. Hold ⌘ and point at a name already written in a file to read its
signature and note where it stands; ⌘-click it to open the whole reference,
searchable, in the right-hand panel.

In the UGen tables an **`→` separates ports from constants**. Everything to its
left is a wired input and may be modulated by another signal; everything to its
right is baked in when the graph is built and must be a compile-time number. So
`tap(signal, delay → min_delay, max_delay)` follows a moving `delay`, while
`delay(signal → time)` will not.

### Patterns and Playback

These are what turn a list into sound. `play` and its variants take the pattern
first, so they chain off one like anything else: `riff.rev.play(bass)`.

An instrument is written in mono. Where it sits is `play`'s business, not the
instrument's, so `pan:` is a lane like any other — read a value per note,
wrapping when it runs out, and free to be a different length from the pattern:

```rust
play([\, `, \, `, \, `, \, `], hat, pan: [-0.8, 0.8])   // alternating, note by note
play([c2, e2, g2], bass, pan: 0)                        // pinned to the centre
```

-1 is hard left, 0 the centre, 1 hard right; anything further clamps. The law is
equal-power, scaled so that the centre is where an unpanned voice already sat —
adding `pan: 0` to a line that was playing changes nothing about how loud it is.
The 3 dB that buys is paid at the extremes instead, where one channel is silent
and the other is a little louder than the instrument wrote, so a hard-panned
voice is worth checking against a limiter.

A pan is sampled once, at the note's onset, and holds for that note. Sweeping
one *within* a note is a different thing and is not in the language yet: the
signal graph an instrument builds is mono throughout, and stereo begins after
it.

| Name | Signature | Notes |
| --- | --- | --- |
| `accel` | `accel(from, to, bars)` | A rate that moves, written where a `play` takes a number: `playn(riff, lead, 8, accel(1, 2, 4))` runs eight passes, speeding up to double over the first four bars and holding there. A straight line in rate, so the pattern covers the area under it — from 1x to 3x over four bars is eight passes in those four bars, not four. Measured from the section's own first note, and run afresh each time a `wthen` window comes round again. `to` below `from` is a ritardando. Both rates must be above zero. See [Speeding up and slowing down](#speeding-up-and-slowing-down). |
| `play` | `play(pattern, instrument, rate?)` | Schedule a pattern on an instrument, forever. The instrument must name a user `fn`. `rate` defaults to 1, and may be an `accel` rather than a number. A list divides the bar evenly unless a step is given a length with `;` — ``[220;2, 330, 440, `;4]`` is a quarter, two eighths and a half of silence — and lengths are relative, so only their ratio matters. A long step is one sustained note, not several. A step may instead be given a written note value — see [Rhythm in note values](#rhythm-in-note-values). Any further parameter is patterned by name — `play(bass, cut: [400, 2000])` — sampled at each note's onset, and lanes may be any length. In a lane a `;` is how many notes the value covers, so it has to be a whole number there. Two names are reserved and reach the note rather than the instrument: `legato:` scales its length, and `pan:` places it across the stereo field. |
| `play_once` | `play_once(pattern, instrument, rate?)` | `play`, stopping after one pass. Started while something is already playing, it begins on the next bar, so the one-shot lands on a downbeat. Re-evaluating fires it again. |
| `playn` | `playn(pattern, instrument, times, rate?)` | `play`, stopping after `times` passes. `rate` follows the count and still defaults to 1 — at rate 2, four passes take two bars. |
| `play_all` | `play_all(section, ...)` | Treat several sections that run at once as one. Each is a no-parameter `fn` named or a play written out, the same two spellings `seq` and `then` take — the difference is that these start together rather than one after another, and the group finishes when the last does. A plain `play` among them never finishes, so nothing may follow. A section piped in from the left must be named, as in `seq`. |
| `then` | `then(play, section)` | Sequence one section after another: `playn(verse, lead, 4).then(chorus)`. The left side must finish, so plain `play` will not do. `section` is a no-parameter `fn` named here or a play written out, and either way its own `play` calls start where this one stops; it is lowered at eval time, not called from the audio thread. A play bound to a `let` is not a section — it already sounded where it was written. |
| `dur` | `dur` | The current note's length in seconds. A binding rather than a function, and bound only inside a voice — pass it to `env`. |
| `qvs` | `qvs` | The quarter note in seconds — 0.5 at the default 120 bpm. A binding rather than a function. See [Syncing to the beat](#syncing-to-the-beat). |
| `qvh` | `qvh` | The quarter note in hertz, which is `1 / qvs` — 2 at the default 120 bpm. See [Syncing to the beat](#syncing-to-the-beat). |

### Syncing to the beat

`qvs` and `qvh` are the beat, as a number, wherever a number can be written.
They are the same quarter note the transport counts and `bpm` converts, in the
two units a signal asks for it in: **`qvs` is a length in seconds** for
everything that takes a time — `env`, `perc`, `line`, `delay` — and **`qvh` is a
frequency in hertz** for everything that takes one.

```rust
fn stab(n) = lowpass(saw(n.m2h), 400 + 3000 * sin(qvh * 2), 3) * perc(0, qvs / 2)
```

Which way round to divide follows from the unit, and the two go opposite ways:
`qvs / 2` is *half as long*, so it is an eighth; `qvh * 2` is *twice as fast*,
so it is also an eighth. Multiply for shorter notes in hertz, divide for them in
seconds. Dots and triplets are the arithmetic they always were — `qvs * 1.5` is
a dotted quarter's worth of delay, `qvh * 3` a quarter triplet.

**Inside an instrument the beat is the one that instrument is being played on.**
A `play` at rate 2 packs two passes into a bar, and its voices are told a beat
half as long to match — so one instrument follows whatever speed it is put at,
and two patterns can run the same instrument at two speeds:

```rust
play(riff, stab)          // sweeps on the eighth
play(riff, stab, 2)       // the same instrument, sweeping twice as fast
```

Under an `accel` this is read afresh for every note, at the speed the curve has
reached by then, so a sweep tightens along with the rhythm.

Outside an instrument — in the persistent graph — the beat is the transport's
own, fixed at the eval that wrote it. **Dragging the tempo moves the patterns and
leaves the graph where it was** until the next eval; play the file again to
bring it along. A voice has no such gap, since it is built afresh per note.

One other thing they do not do: they set a *speed*, not a phase. A voice starts
its shapes at its own note, so an envelope or a sweep written with `qvs` lands
where the note does. An oscillator in the persistent graph runs at the right
rate but from wherever the eval left it, so it will not be on the downbeat.

### Arrangement

`then` is the root of a family. All of them chain from a play, and most work the
same way underneath: the section is lowered *now*, while the program is being
lowered, with its start moved to wherever it belongs. There is no runtime
interpreter and no callback — the scheduler only ever sees bindings that happen
to open later.

Three of them are exceptions, and it is worth knowing which. `take` and `stop`
reach *backwards*, shortening bindings already on the timeline — the only place
in the language that edits a binding after it was written. `wthen` is the one
that genuinely needs the scheduler's help; see [Choice](#choice-and-why-it-is-not-choice)
below.

A *section* is either a no-parameter `fn` named where the section goes, or a
play written out there:

```rust
playn(riff, lead, 4).then(chorus)                  // named
playn(riff, lead, 4).then(playn(riff2, lead, 4))   // written out
```

Every combinator that takes a section takes both spellings, `play_all`
included — `play_all(verse, bassline)` and `play_all(verse(), bassline())` are
the same group, so nothing needs parentheses in one position and not another:

```rust
seq(intro, play_all(verse, bassline))
```

These are the same thing, not two mechanisms. A section argument is the one
place in the language whose expression is *not* evaluated on the way into the
call: it is lowered where the section is placed, with the start already moved
there — and for `play_all`, which moves nothing, that start is simply the one
the group already sits at. So a play written out never sounds at the origin and then gets dragged
forward — it is written where it belongs in the first place, exactly as the
body of a named `fn` is. Write it out when a two-bar variation is not worth a
name; name it when the name says something, or when the same section is used
twice.

A play bound to a `let` is **not** a section, and this is the one place the
difference shows:

```rust
let verse = playn(riff, lead, 4)   // this sounds, here, at the origin
playn(intro, bass, 2).then(verse)  // refused: `verse` was already placed
```

`let` does not defer anything. A `play` sounds where it is written, so by the
time the name holds a value the notes are on the timeline at bar 0, and there
is nothing left for `then` to place — moving them afterwards is exactly the
fixup the mechanism above avoids. What a named play *is* good for is the other
side of the call: it is a handle, so `verse.then(chorus)` chains from it. To
place the same music later instead, name it as a `fn` — `fn verse() = playn(riff,
lead, 4)` — and nothing about the call site changes.

A section captures nothing, because closures do not exist here; whatever `play`
calls it contains are written relative to where the section was placed, so
nesting composes and the offsets add up as the code reads.

**A placed section starts at the top of its pattern**, wherever on the timeline
it lands. That matters when the pass does not divide the bar: seventeen eighths
is 2⅛ bars, and a section of them dropped eight bars in would otherwise open on
its fourteenth note and wrap round to its first — and its lanes with it, so a
volume ramp would start three quarters of the way up. The polymeter above is a
property of a *looping* `play`, which was never placed anywhere and so keeps the
transport's own grid: re-evaluating does not re-phase a line that is already
rotating against the bar. Anything an arrangement puts somewhere begins where it
begins.

Five of them take only a named `fn`: `then_each`, `wthen`, `rthen`,
`shuffle_then` and `maybe`. Each of those has to *run* its section rather than
place it once — per element, per arm, or once more each time round — and a play
written out has already happened by the time it is an argument. `then_n` is not
among them: it re-lowers its section per pass, so both spellings work and a
`rand` inside is drawn afresh either way.

Most of these need the section on their left to **finish**. A plain `play` never
does, which is what `play_once`, `playn`, `take` and `stop` are for.

```rust
playn(riff, lead, 4)
  .then_fill([1, 1, 1, 1])  // one bar of fill, on lead itself
  .then_n(chorus, 2)        // twice through
  .then(outro)
```

Everything here runs once and hands on, so a chain simply stops when it reaches
the end. `loop` is how a chain comes back around instead:

```rust
playn(riff, lead, 3)
  .then_fill([1, 1, 1, 1])  // three bars and a fill …
  .loop(4)                  // … four times through
  .then(chorus)
```

It repeats *the whole chain to its left*, which is the point — a fill is only
worth writing because the groove returns after it, and there is nowhere else to
say "these two together" without naming the pair as a `fn` first. `then_n` is
the other half of the pair: it takes a section and runs it after this one, and
because it lowers it afresh each pass a `rand` inside it lands differently every
time. `loop` copies bindings that already exist, so every pass is the same
music.

`with` lays a section alongside rather than after it, from the same downbeat:

```rust
playn(riff, lead, 4).with(drums).then(chorus)
```

Note that this makes the handle cover both plays, so a `then_fill` cannot follow
it — a fill needs one instrument to be a fill *for*. Chain the fill onto the
play itself.

| Name | Signature | Notes |
| --- | --- | --- |
| `then_after` | `then_after(play, bars, section)` | `then`, with `bars` of silence in between. The gap cannot be negative — `overlap` is how a section starts early. |
| `overlap` | `overlap(play, bars, section)` | `then`, but the section starts `bars` *before* this one ends, so the two really do sound together over the join. Never earlier than the receiver's own start. The chain carries on from whichever ends later. |
| `with` | `with(play, section)` | Run a section alongside this one, from where it **began**. `play_all` opens its sections together as a group; this makes one concurrent with a section already placed, so an arrangement reads in the order it happens. |
| `at` | `at(bar, section)` | Place a section at an absolute bar from the origin. Bars are counted from 0, so `at(8, …)` is the ninth bar — it is a distance from the start, the same number the `then_after`s before it would have added up to. The escape hatch from chaining, for an arrangement whose shape you already know. |
| `seq` | `seq(section, ...)` | Sections one after another without the nesting: `seq(intro, verse, chorus, verse)`. A section piped in from the left — `intro.seq(verse)` — must be a named `fn`, since it settled before `seq` could place it. |
| `then_n` | `then_n(play, section, times)` | A section `times` times, back to back. Lowered afresh each pass — whether the section is named or written out — so a `rand` inside it is a different number every time round, the same rule a voice follows. |
| `loop` | `loop(play, times)` | Everything chained so far, `times` times through. The counterpart to `then_n`: that one names a `fn` and runs it *after* this section, this one takes no section at all, because the section it repeats is the chain it is written on. The whole chain must finish. The passes are copies, so a `rand` in the chain was already spent and every pass is the same music — `then_n` is the one that draws afresh. |
| `then_each` | `then_each(play, list, body)` | One pass per element, with the element passed in: `.then_each([1, 2, 4], faster)` calls `faster(1)`, `faster(2)`, `faster(4)`. `body` takes exactly one parameter, so it is always a named `fn`. Arrangement by list — every list function already builds the shape of a piece, and this is what spends one. |
| `then_fill` | `then_fill(play, pattern, rate?)` | One pass of a pattern on this section's **own** instrument. No `fn` and no second `play`: a fill is played by whoever just played, so the instrument and every lane are inherited and only the pattern is new. Needs a single `play` on the left — a group has no one instrument to fill for. |
| `quantize` | `quantize(play, grid?)` | Round where the chain has reached up to a multiple of `grid` bars (default 1), without shortening what is already playing. |
| `take` | `take(play, bars)` | This section, cut to `bars`. What gives a plain `play` an end. A part that already stops sooner is left alone — a cut is a ceiling, not a length. |
| `stop` | `stop(play)` | Cut everything still open in this section at the moment its last **counted** part finishes. Needs at least one `play_once` or `playn` among them, or there is no moment to stop at. |
| `wthen` | `wthen(play, sections, weights)` | Choose between sections, afresh each time round. Weights are relative and need not sum to 1. The arms are named `fn`s, not plays written out: an arm has to still be runnable when the choice is made. The same holds for `rthen`, `maybe` and `shuffle_then`. |
| `rthen` | `rthen(play, sections)` | `wthen` with every section equally likely. |
| `maybe` | `maybe(play, chance, section)` | A section with probability `chance` (0 to 1), decided afresh each time round. A `wthen` whose other arm is silence. |
| `shuffle_then` | `shuffle_then(play, sections)` | Every section once each, in an order drawn now. The counterpart to `rthen` rather than a variant: a weighted choice may pass a section over for a long time, and this cannot. |

#### `quantize`

`rate` divides into the count — or an `accel` integrates into it — so a
section's length need not be a whole number.
`playn(pat, inst, 3, 2)` is three passes at double speed — 1.5 bars — and
every `.then` after it inherits that half-bar offset for good:

```rust
playn(pat, inst, 3, 2).then(chorus)              // chorus starts at 1.5
playn(pat, inst, 3, 2).quantize().then(chorus)   // chorus starts at 2
```

Quantizing moves where the *next* section starts. It does not shorten what is
already playing, so the three passes still run their full 1.5 bars and the
chorus opens over the tail of them.

The rest it rounds to is **part of the section**, which is what makes it worth
naming one:

```rust
fn slot() = play_once(phrase, lead).quantize(4)   // the phrase, in a four-bar slot
slot().then(slot).loop(2)                         // four slots, on the grid
```

`.loop` repeats the rest along with the notes, so a section padded out to its
slot comes back around at the slot line rather than at its last note. `take`
says a length the same way — `.take(8)` on a six-bar section is six bars of
music and two of silence, since a cut is a ceiling and never lengthens a part.

#### Speeding up and slowing down

A rate need not be one speed. `accel(from, to, bars)` is a straight line in
rate, and goes wherever a rate number goes:

```rust
playn(riff, lead, 8, accel(1, 3, 4))    // eight passes, 1x rising to 3x
play(hats, hat, accel(4, 1, 16))        // a long settle, then 1x forever
```

It is a line in *rate*, so what the pattern covers is the area under it. From 1x
to 3x over four bars averages 2x, which is eight passes in those four bars —
not four, and not six. That is also how a counted section still has a length:
`playn(riff, lead, 8, accel(1, 3, 4))` is exactly four bars long, and `.then`
places what follows at bar 4 like any other section.

The curve is measured from the section's **own** first note, not from the start
of the performance, so it means the same thing wherever the section sits — and a
`wthen` arm accelerates again every time it is drawn. After `bars` it holds at
`to` rather than climbing on, which is what makes it safe on a plain `play` that
never ends.

Notes shorten as the gaps do: a note is as long as the pattern says in the
pattern's own time, so an accelerando tightens the line rather than leaving
notes overlapping the ones after them.

Two limits are worth knowing. The rate must stay above zero at both ends — at
zero a pattern stops rather than slows, and a counted section would never reach
the end it hands on from. And a rate is not a signal: it cannot be an `lfo` or
anything else from the audio graph, because the scheduler works a lookahead
*ahead* of the audio clock and would need values the graph has not produced
yet. `accel` is a shape the scheduler evaluates itself, which is what lets it
place a note before the sound of it exists.

#### Choice, and why it is not `choice`

Everything else here settles while the program is lowered. `wthen`, `rthen` and
`maybe` cannot: a draw that never changes is a draw made once, and the whole
point is that it changes. So these are the one place the scheduler is told
something new.

Every arm is written to the timeline, all starting at the same bar and all
marked as arms of one choice. Which arm actually sounds is decided as the music
reaches it. That has three consequences worth knowing:

- **The block repeats forever**, because a choice with nowhere to come back to
  would be drawn once and never again. `ends_at` is `None` for the same reason,
  so nothing may follow a choice until `take` has given it a length.
- **Every arm must finish**, and the block's period is the longest of them. A
  shorter arm leaves silence rather than pulling the next repetition early, so
  the arms stay interchangeable in time.
- **The draw is a hash of `(seed, choice, repetition)`**, not a running
  generator. The scheduler queries an overlapping lookahead window every pass,
  so the same moment is asked about more than once; a stateful generator would
  answer differently each time and the note would flicker in and out as the
  horizon crept past it. Hashing gives a fresh draw each time round and the same
  draw every time that repetition comes up.

The seed is drawn once per eval, so re-evaluating deals a new hand — and `seed`
pins an arrangement exactly as it pins `choice` and `scramble`.

```rust
playn(riff, lead, 2)
  .wthen([verse, chorus], [0.7, 0.3])  // a new draw every time round
  .take(32)                            // ...for 32 bars
  .then(outro)
```

`shuffle_then` is deliberately not part of this. It draws an order once, at eval
time, like `scramble` — so it has a length, it plays every section exactly once,
and a `.then` may follow it.

### Sample Functions

Reading an audio file. See [Samples](#samples) above for what these are for; the
table is the signatures.

| Name | Signature | Notes |
| --- | --- | --- |
| `load` | `load(path) -> buffer` | Read an audio file into a buffer. The path is relative to the file it is written in, the same way a `use` path is, and must be written out rather than computed — every file is decoded once, before the program runs, so no note ever waits on a disk. Any format symphonia reads: wav, mp3, flac, ogg. Nothing comes out of a buffer until `sample` reads it. |
| `sample` | `sample(buffer, position, channel?) -> signal` | Read a buffer at a position: 0 is the start, 1 is the end, and anything outside that is silence. `position` is a signal, which is where speed, direction and chopping all come from. Cubic interpolation, so it holds up away from its own speed. `channel` defaults to 0 and wraps if the buffer has fewer; it picks which reader is built, so it must be a compile-time number. |
| `slice` | `slice(buffer, start, end, rate?, channel?) -> signal` | Read a portion of a buffer, once: `slice(amen, 0, 0.25)` is the first quarter of the break, at the speed it was recorded at. `start` and `end` are positions like `sample`'s, and both are compile-time numbers rather than signals — a slice is refused outside 0..1, where `sample` would read silence. `start` past `end` plays the portion backwards. `rate` defaults to 1 and multiplies the speed — 2 reads the portion in half the time, an octave up — and must be above zero, since at zero the reader would stop rather than slow. It is `sample` with the phasor written for you, `sample(b, line(start, end, (end - start) * b.secs / rate))`, and exists because that duration is the part that is easy to get wrong. The read holds on the last sample once it arrives, so an envelope is what ends the note; a slice ending at 1 goes quiet on its own. Use `sample` when the position or the speed has to move. |
| `secs` | `secs(buffer) -> number` | How long a buffer is, in seconds. A compile-time number, so it divides into a `ramp` frequency: `ramp(1 / amen.secs)` reads the whole buffer once at its own speed. |
| `channels` | `channels(buffer) -> number` | How many channels a buffer has — 1 for mono, 2 for a stereo file. |

### List Functions

```rust
let riff = [60, 63, 67]
play(riff.rev.rotl(2), bass)
play(riff.push(72).scramble, bass)
riff.rev.play(bass)             // `play` takes a pattern, so it chains too
```

Lists are immutable. List functions generate a new list and leave the existing list intact.

| Name | Signature | Notes |
| --- | --- | --- |
| `len` | `len(list) -> number` | Works on list literals and ranges. Errors on non-lists and on wrong arity. |
| `zip` | `zip(list, ...) -> list` | Variadic. Pairs positionally into rows `[[a0,b0], [a1,b1], …]`. **All arguments must be lists of equal length** — a mismatch is an error, not a silent truncation. Rows carry whatever `Value`s went in, including signals. |
| `rev` | `rev(list) -> list` | Reversed. |
| `palindrome` | `palindrome(list) -> list` | The sequence then its mirror: `[1,2,3]` → `[1,2,3,3,2,1]`. |
| `rotl` | `rotl(list, amount?) -> list` | Rotate left, wrapping. `amount` defaults to 1, and a negative amount rotates the other way — `rotl(l, -1)` is `rotr(l, 1)`. Rotating by the length is the identity. An empty list is returned unchanged. |
| `rotr` | `rotr(list, amount?) -> list` | Rotate right, wrapping. The mirror of `rotl` in every respect. |
| `push` | `push(list, value) -> list` | Appends. Returns a new list; the original is untouched. |
| `pop` | `pop(list) -> list` | Drops the **last** element. Nothing is returned "off the top" — index the list for that. Errors on an empty list. |
| `sort` | `sort(list) -> list` | Ascending. Every element must be a compile-time number. |
| `sum` | `sum(list) -> value` | Folds through the same `combine` the arithmetic operators use, so numbers fold to a constant and **signals emit `Add` nodes** — `sum([sin(110), sin(220)])` is additive synthesis without a `for`. Empty list → `0`. |
| `split` | `split(list, size) -> list` | Chunks of `size` (not split-at-index). A short final chunk is kept. `size` must be a whole number ≥ 1. |
| `map` | `map(list, transform) -> list` | The function applied to every element. It is an ordinary user `fn` of one argument and may answer with anything, so `map` is also the only way to build a **list of signals** — a `for` over audio sums instead of collecting. |
| `filter` | `filter(list, predicate) -> list` | Keeps elements the predicate answers non-zero for. The predicate is an ordinary user `fn`; it must return a compile-time number. |
| `dot` | `dot(value, dots?) -> duration` | Dot a written note value: `q.dot` is a dotted quarter, a quarter and an eighth. `dots` defaults to 1; `q.dot(2)` is the doubly-dotted quarter. A count rather than a repeated `.dot`, because each dot adds half of the note itself rather than half of what the last one left. See [Rhythm in note values](#rhythm-in-note-values). |
| `choice` | `choice(list) -> value` | One random element. Errors on an empty list. |
| `wchoice` | `wchoice(values, weights) -> value` | Weighted random pick. Parallel lists of equal length, like `zip`. Weights must be finite and ≥ 0, and not all zero. |
| `scramble` | `scramble(list) -> list` | Shuffled. |

The last three re-roll on every eval, and draw from the same generator as
[the random numbers](#random-numbers) — so `seed` pins them too.

### Math Functions
| Name | Signature | Notes |
| --- | --- | --- |
| `m2h` | `m2h(note) -> number` | MIDI note to hertz. `69.m2h` is 440, `60.m2h` is 261.63. Equal temperament, A4 = 440. Fractional notes work — that is how you get glides. |
| `h2m` | `h2m(hz) -> number` | The inverse. `440.h2m` is 69; the result may be fractional. Frequency must be above zero. |
| `db` | `db(decibels) -> number` | Decibels to linear amplitude. `0.db` is 1, `(-6).db` is about 0.5. |
| `amp` | `amp(amplitude) -> number` | The inverse. `1.amp` is 0. Amplitude must be above zero. |
| `cents` | `cents(hz, cents) -> number` | Detune a frequency by hundredths of a semitone. `440.cents(1200)` is 880. |
| `bpm` | `bpm(beats) -> number` | Beats per minute to bars per second. The beat is the quarter note; how many make a bar is the project's signature, so `120.bpm` is 0.5 in 4/4 — exactly `DEFAULT_CPS` — and 0.667 in 3/4, where the bar is shorter. |
| `oct` | `oct(note, octaves) -> number` | Transpose by whole octaves. `60.oct(-1)` is 48. |
| `semi` | `semi(note, semitones) -> number` | Transpose by semitones. `60.semi(7)` is 67. |
| `scale` | `scale(note, scale) -> number` | Snap to the **nearest** tone of a scale given as semitone offsets within an octave. `61.scale([0,2,4,5,7,9,11])` is 60. Neighbouring octaves are candidates, so 59 against `[0,4,7]` rises to 60 rather than falling a seventh. Ties snap down. |
| `clamp` | `clamp(x, lo, hi) -> number` | Constrain to `lo..=hi`. An empty range is an error. |
| `norm` | `norm(x, lo, hi) -> number` | Map 0..1 onto `lo..hi`. Values outside 0..1 **extrapolate** — `clamp` first if that is not what you want. |
| `wrap` | `wrap(x, lo, hi) -> number` | Fold back into the range rather than clamping. `13.wrap(0, 12)` is 1. Useful for modular pitch. |
| `round` | `round(x) -> number` | Nearest whole number, halves away from zero. |
| `floor` | `floor(x) -> number` | Round down. |
| `ceil` | `ceil(x) -> number` | Round up. |
| `abs` | `abs(x) -> number` | Magnitude, sign discarded. |
| `pow` | `pow(x, exponent) -> number` | `2.pow(10)` is 1024. For exponential curve shaping. |
| `sqrt` | `sqrt(x) -> number` | `x` must not be negative. |
| `log2` | `log2(x) -> number` | `x` must be above zero. |

### Random Numbers

```rust
play(randis(8, 60, 72), lead)          // eight notes, settled until you eval again
play(riff, bass, cut: rands(4, 400, 2000))
fn snare(n) = noise() * perc(0.001, 0.1) * rand(0.7, 1)   // a new draw per note
```

| Name | Signature | Notes |
| --- | --- | --- |
| `rand` | `rand(lo?, hi?) -> number` | A uniform number. `rand()` draws from 0..1; `rand(lo, hi)` — or `60.rand(72)` — draws from `lo..hi`, `hi` excluded. An empty range is an error. |
| `randi` | `randi(lo, hi) -> number` | A uniform whole number in `lo..hi`, `hi` excluded: `randi(60, 72)` is an octave of notes that never repeats the root. Both bounds must be whole. |
| `coin` | `coin(probability?) -> number` | 1 or 0 at odds you choose. `coin()` is even; `coin(0.25)` answers 1 one time in four. Multiply by it to drop a note. |
| `seed` | `seed(seed) -> number` | Fix every draw made after it, and answer with the seed. Any number will do — `seed(0.5)` is a seed like any other. |
| `gauss` | `gauss(mean?, deviation?) -> number` | Normally distributed: clustered around `mean`, two thirds within one `deviation`. Both default to the standard 0 and 1. **Unbounded** — `clamp` it if a stray value would hurt. |
| `humanize` | `humanize(x, amount) -> number` | `x` plus a normal draw of deviation `amount`, so most nudges are small and a few are not. `0.5.humanize(0.05)` is a velocity that no longer sounds typed in. |
| `expo` | `expo(mean?) -> number` | Exponential, above zero, averaging `mean` (default 1). Short values common, long ones rare — the shape of a wait between events. |
| `tri` | `tri(lo, hi, mode?) -> number` | Triangular over `lo..hi`, peaking at `mode` (the midpoint if omitted). Bounded like `rand` but with a centre. |
| `cauchy` | `cauchy(median?, spread?) -> number` | Heavy-tailed around `median` (default 0). Mostly close in, but far likelier than `gauss` to lurch — which is the point. |
| `pareto` | `pareto(scale?, shape?) -> number` | Power-law at or above `scale` (default 1). A smaller `shape` (default 1) makes big values likelier. |
| `poisson` | `poisson(mean?) -> number` | A whole count averaging `mean` (default 1) — how many things happened, when each was independent. |
| `rands` | `rands(count, lo?, hi?) -> list` | `count` uniform numbers, from 0..1 or from `lo..hi`. A lane in one line. |
| `randis` | `randis(count, lo, hi) -> list` | `count` whole numbers in `lo..hi`, `hi` excluded. |
| `walk` | `walk(count, start, step) -> list` | A random walk: `count` numbers beginning at `start`, each drifting from the one before by up to `step` either way. **Neighbours stay close**, so it moves rather than jumps — which `rands` does not. Good for a cutoff or a pan. |
| `walki` | `walki(count, start, step) -> list` | `walk` in whole steps, so it stays on the semitone grid. `walki(16, 60, 2)` wanders a melody around middle C. `step` must be whole. |
| `choices` | `choices(list, count) -> list` | `count` elements without replacement, in a random order — `choice` several times over may repeat itself, and this cannot. Taking the whole list is `scramble`; asking for more than it holds is an error. |
| `randscale` | `randscale(count, scale, lo?, hi?) -> list` | `count` notes drawn **evenly** from the tones of a scale, given as semitone offsets within an octave, between MIDI `lo` and `hi` (60..72 by default). Unlike snapping a uniform draw with `scale`, every degree is equally likely and nothing lands outside the range. |

### Oscillators and Sources

| Name | Arguments | Notes |
| --- | --- | --- |
| `sin` | `(frequency)` | Sine oscillator. |
| `saw` | `(frequency)` | Bandlimited saw wavetable oscillator. |
| `square` | `(frequency)` | Bandlimited square wavetable oscillator. |
| `triangle` | `(frequency)` | Bandlimited triangle wavetable oscillator. |
| `soft_saw` | `(frequency)` | Soft saw wavetable oscillator. Contains all partials but falls off like a triangle wave. |
| `hammond` | `(frequency)` | Hammond organ wavetable oscillator. Emphasizes the first three partials. |
| `organ` | `(frequency)` | Organ wavetable oscillator. Emphasizes octave partials. |
| `ramp` | `(frequency)` | Rising ramp from 0 to 1 at the given repetition frequency, starting at 0. Not bandlimited — useful as a phasor, not as audio. Its zero is the start of its own period, which is what lets it drive `sample`: `sample(b, ramp(1 / b.secs))` reads a buffer once, end to end. |
| `poly_saw` | `(frequency)` | PolyBLEP saw wave. Fast and fairly bandlimited. |
| `poly_square` | `(frequency)` | PolyBLEP square wave. Fast and fairly bandlimited. |
| `poly_pulse` | `(frequency, width)` | PolyBLEP pulse wave. Fast and fairly bandlimited; `width` in 0..=1 is the duty cycle. |
| `pulse` | `(frequency, width)` | Bandlimited pulse wave oscillator. `width` in 0..=1 is the duty cycle. |
| `dsf_saw` | `(frequency, roughness)` | Saw-like discrete summation formula oscillator. `roughness` in 0..=1 sets how much successive partials are attenuated. |
| `dsf_square` | `(frequency, roughness)` | Square-like discrete summation formula oscillator. `roughness` in 0..=1 sets how much successive partials are attenuated. |
| `impulse` | `()` | A single one followed by silence. Useful for exciting `pluck` or measuring an impulse response. |
| `input` | `(→ channel)` | One channel of the live audio input, counted from 0. Silence until an input device is chosen in the settings panel, and on any channel the chosen device does not have — so a piece written against an interface still runs on the laptop it is edited on. |

### Noise and Chaos
| Name | Arguments | Notes |
| --- | --- | --- |
| `noise` | `()` | White noise. |
| `pink` | `()` | Pink noise: -3 dB per octave. |
| `brown` | `()` | Brown noise: -6 dB per octave. Darker than pink. |
| `mls` | `()` | Maximum length sequence noise: a repeating pseudorandom run of -1 and 1. |
| `mls_bits` | `(→ bits)` | Maximum length sequence noise from an n-bit sequence (1..=31). More bits means a longer period before it repeats. Constant. |
| `lorenz` | `(frequency)` | Lorenz chaotic oscillator. The frequency input has only a slight effect on the output. |
| `rossler` | `(frequency)` | Rossler chaotic oscillator, with peaks at multiples of the frequency input. |

### Filters
| Name | Arguments | Notes |
| --- | --- | --- |
| `lowpass` | `(audio, cutoff, q)` | Resonant lowpass filter. |
| `highpass` | `(audio, cutoff, q)` | Resonant highpass filter. |
| `bandpass` | `(audio, frequency, q)` | Bandpass filter. Keeps frequencies near the center, attenuating either side. |
| `notch` | `(audio, frequency, q)` | Notch filter. Removes a narrow band around the center frequency. |
| `peak` | `(audio, frequency, q)` | Peaking filter. |
| `allpass` | `(audio, frequency, q)` | Allpass filter. Passes all frequencies but shifts their phase around the center frequency. |
| `lowrez` | `(audio, cutoff, q)` | Resonant two-pole lowpass filter. |
| `bandrez` | `(audio, frequency, q)` | Resonant two-pole bandpass filter. |
| `moog` | `(signal, cutoff, q)` | Moog-style resonant lowpass ladder filter. |
| `resonator` | `(audio, frequency, q)` | Constant-gain bandpass resonator. |
| `bell` | `(audio, frequency, q, gain)` | Bell equalizer. Boosts or cuts a band around the center frequency by `gain` (an amplitude multiplier, not dB). |
| `lowshelf` | `(audio, frequency, q, gain)` | Low shelf filter. Scales everything below the center frequency by `gain` (an amplitude multiplier). |
| `highshelf` | `(audio, frequency, q, gain)` | High shelf filter. Scales everything above the center frequency by `gain` (an amplitude multiplier). |
| `morph` | `(signal, frequency, q, morph)` | Filter that morphs continuously between modes: `morph` runs -1 (lowpass) to 0 (peak) to 1 (highpass). |
| `lowpole` | `(audio, cutoff)` | First-order one-pole lowpass. No resonance. |
| `highpole` | `(audio, cutoff)` | First-order one-pole one-zero highpass. No resonance. |
| `allpole` | `(audio, delay)` | First-order allpass filter with a configurable delay at DC, in samples (must be > 0). |
| `butterpass` | `(audio, cutoff)` | Second-order Butterworth lowpass. Maximally flat passband, no resonance control. |
| `pinkpass` | `(signal)` | Pinking filter: -3 dB per octave. Turns white noise into pink. |
| `dcblock` | `(signal)` | Remove DC offset, keeping the signal zero-centered. Cutoff is 10 Hz. |
| `biquad` | `(signal → a1, a2, b0, b1, b2)` | Arbitrary biquad filter with coefficients in normalized form. All five coefficients must be constants. |
| `fir3` | `(signal → gain)` | Three-point symmetric FIR filter, specified by its `gain` (>= 0) at the Nyquist frequency. A gain below 1 gives a gentle lowpass. |

### Envelopes and Dynamics
| Name | Arguments | Notes |
| --- | --- | --- |
| `perc` | `(→ attack, release)` | Self-contained percussive envelope: rise, fall, silence. Needs no note length, so it works in a voice or the persistent graph — and in a voice the shape is measured from the onset and always finishes, so a drum longer than the step it sits on rings on into the next rather than being cut off. Both times are constants. |
| `env` | `(→ attack, decay, sustain, release, duration)` | Time-based ADSR for one-shot voices. Attack, decay and sustain fill `duration`; the release starts where that ends and rings on for its own time past it, so the note is held for `duration` and the voice goes on at least `duration + release` (see [How long a voice lasts](#how-long-a-voice-lasts)). Pass the voice-bound `dur` as the duration — `legato` shortens the held part and leaves the release alone. All arguments are constants. |
| `line` | `(→ start, end, duration)` | One straight segment: `start` to `end` over `duration` seconds, held at `end` after that. Measured from the onset — the note's in a voice, the eval's in the persistent graph — so like `perc` it needs no note length and works in either: `sin(line(880, 220, 0.05))` is a kick's pitch drop. Unlike `perc` it ends at a level rather than at silence, so it holds a voice open past its note only when `end` is 0 and the shape really finishes (see [How long a voice lasts](#how-long-a-voice-lasts)). All three arguments are constants; for a sweep between signals, scale it — `start + (end - start) * line(0, 1, d)`. |
| `adsr` | `(gate → attack, decay, sustain, release)` | Gated ADSR envelope. Rises while the gate is positive, releases when it returns to zero. Times are in seconds; sustain is a level in 0..=1. |
| `follow` | `(signal → response_time)` | Parameter follower. Smooths the signal with the given halfway response time, in seconds. |
| `afollow` | `(signal → attack, release)` | Asymmetric parameter follower. Smooths rising segments over `attack` and falling ones over `release` (halfway response times, in seconds). |
| `limiter` | `(signal → attack, release)` | Look-ahead limiter holding the signal to -1..=1. Look-ahead equals the attack time. Times are constants, in seconds. |
| `clip` | `(signal)` | Hard-clip the signal to -1..=1. |
| `clip_to` | `(signal → minimum, maximum)` | Hard-clip the signal to `minimum`..=`maximum`. Both bounds are constants. |
| `declick` | `(signal)` | Fade the signal in over 10 ms from time zero, suppressing the click at the start of a graph. |

### Delays and Effects
| Name | Arguments | Notes |
| --- | --- | --- |
| `delay` | `(signal → time)` | Fixed delay of `time` seconds, rounded to the nearest sample. The time is a constant — use `tap` for a modulatable delay. |
| `tap` | `(signal, delay → min_delay, max_delay)` | Tapped delay line with cubic interpolation. Unlike `delay`, the delay time is a signal, so it can be modulated — it must stay within the constant `min_delay`..=`max_delay` bounds, in seconds. |
| `tick` | `(signal)` | Single-sample delay. The building block for feedback and comb filters. |
| `hold` | `(signal, frequency → variability)` | Sample-and-hold. Samples the signal at `frequency` Hz; `variability` in 0..=1 jitters the sampling interval and is a constant. |
| `chorus` | `(audio → seed, separation, variation, mod_frequency)` | Five-voice mono chorus, mixed with the dry signal. Stack two with different seeds for stereo. All parameters except the audio input are constants. |
| `pluck` | `(excitation → frequency, gain_per_second, damping)` | Karplus-Strong plucked string. Feed it a burst — `impulse()` or a short noise envelope — as the excitation. Frequency, gain and damping (0..=1) are constants. |
| `reverb` | `(audio → room_size, time, damping)` | Reverb (32-channel FDN). `room_size` is in meters (10 is an average room), `time` is the decay to -60 dB in seconds, `damping` in 0..=1 rolls off the highs. Wet only: `x + reverb(x, 10, 3, 0.5) * 0.2`. All parameters except the audio input are constants. |
| `reverb2` | `(audio → room_size, time, diffusion, modulation, damping_cutoff)` | Hybrid FDN reverb — richer and more expensive than `reverb`. `room_size` is in meters and clamps to 10..=30, `diffusion` in 0..=1 thickens the tail, `modulation` around 1 adds movement (higher goes audibly Doppler), and `damping_cutoff` is the lowpass applied to each loop pass, in hertz. Wet only. All parameters except the audio input are constants. |
| `reverb3` | `(audio → time, diffusion, damping_cutoff)` | Allpass-loop reverb, with no room size — just `time` to -60 dB, `diffusion` in 0..=1, and a `damping_cutoff` in hertz applied to each loop pass. Wet only. All parameters except the audio input are constants. |
| `reverb4` | `(audio → room_size, time)` | Reverb with a slow fade-in, for swells rather than rooms. `room_size` is in meters and is treated as at least 15; below that the delay times stop sounding like a space. Wet only. Both `room_size` and `time` are constants. |

The reverbs and delays are wet only:

```
fn pad(n) = saw(n.m2h) * env(0.3, 0.2, 0.7, 0.4, dur)
fn wet(x) = x + reverb(x, 10, 3, 0.5) * 0.3
```

### How long a voice lasts

A note played from a pattern is one voice, and it is rendered for as long as it
has something left to say — never only for the step it sits on. An instrument
is read for how far past its note the sound can still arrive, and the note gets
that much more room:

- `env` adds its release, which hangs off the end of the note.
- `perc` adds whatever of its shape does not fit inside the note, and nothing
  when it does fit.
- `line` adds the same, but only when it ends at 0 — a line that stops at a
  level is still sounding when it gets there, so room for it would hold the
  note on rather than let it finish.
- `delay` and `tap` add how far they reach back. A `tap` whose delay is a
  signal has no one answer, so its declared `max_delay` stands for it — worth
  keeping honest, since a bound far wider than the modulation holds the voice
  open for a reach it never makes.
- The reverbs add their `time`, the decay to -60 dB.

**These accumulate along the signal path**, which is the point: an `env` into a
`delay` rings out for its release and *then* comes back a delay later, so the
voice needs both. Branches that happen at once count once — a dry signal added
to its own echoes lasts as long as the last echo, not the sum of them.

```
// a 0.5 s note holds this voice open for 0.5 + 0.4 + 0.8 = 1.7 s
fn ping(n) = {
  let dry = sin(n.m2h) * env(0.001, 0.05, 0.2, 0.4, dur)
  dry + delay(dry, 0.8) * 0.5
}
```

Two things are outside this. A voice still ringing is a whole graph still being
rendered, so a long tail against a fast pattern is many voices at once — **the
tail is capped at 10 seconds**, which is well past anything musical and stops a
slipped digit in a `reverb` time from taking the performance down with it. And
feedback built by hand out of `tick` cannot be measured at all, so it holds a
voice open for nothing; give it an envelope that says when it is over.

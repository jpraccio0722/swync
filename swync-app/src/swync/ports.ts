import { invoke } from "@tauri-apps/api/core";

/**
 * What MIDI ports this machine has, for the editor to offer.
 *
 * This exists because of a question the design otherwise has no good answer
 * to: a port is named *in the program* — `midiout("deluge")` — so how does
 * anybody know what to type? The settings panel lists them, but reading a name
 * off a panel, remembering it, and typing it into a different pane is not how
 * any other name in this language is learned. Every other one is completed.
 *
 * So this is [`Symbols`](./symbols) for hardware, and the parallel is exact:
 * the names live somewhere the buffer has never seen, so they have to come
 * from the backend, and completion reads them at the moment a menu opens.
 *
 * What differs is *when* they go stale. A `use` line changes when the document
 * does, so `Symbols` can refresh on an edit. A port list changes when somebody
 * plugs something in, which no edit reports — so this refreshes on a clock
 * instead, and [`STALE_MS`] is how long a cable takes to appear in a menu.
 */

/** One MIDI port, as `midi_ports` reports it. Mirrors `PortInfo` in
 *  `midi/ports.rs`. */
export interface PortInfo {
  /** Its place in the platform's list, which is what `midiout(0)` means. */
  number: number;
  name: string;
}

interface MidiPorts {
  outputs: PortInfo[];
  inputs: PortInfo[];
}

/**
 * How long an answer is treated as current.
 *
 * Short, because the thing it goes stale against is a person plugging a cable
 * in and then reaching for the editor to use it — and long enough that holding
 * a key down inside `midiout("` is one round trip rather than one per
 * keystroke. Enumerating ports talks to the platform, so it is not free, but it
 * is not a disk either.
 */
const STALE_MS = 2000;

/** Cheap enough to run on every keystroke: what makes a document worth asking
 *  the platform about at all. */
const MENTIONS_MIDI = /\bmidiout\b/;

/**
 * A live list of ports for one editor.
 *
 * Mutable and long-lived for the same reason `Symbols` is: completion reads it
 * when a menu opens, and the fetch that fills it finished some time before
 * that or has not finished yet.
 */
export class Ports {
  private outputs: PortInfo[] = [];
  /** When the current answer arrived. Zero before the first one. */
  private fetchedAt = 0;
  private fetching = false;
  /** Bumped per request, so a slow answer cannot overwrite a newer one. */
  private generation = 0;

  /** Every MIDI output, as of the last answer. */
  current(): PortInfo[] {
    return this.outputs;
  }

  /**
   * Look again, unless the last answer is still warm.
   *
   * Safe to call on every keystroke and from inside a completion source: the
   * common case is a clock comparison and a return. Never awaited — a menu
   * that waited for this would be a menu that opened late, and the answer it
   * would have shown is the one the next keystroke shows instead.
   */
  refresh(doc?: string) {
    // A document that never says `midiout` has nothing to ask about. Checked
    // here rather than by the caller so that the update listener and the
    // completion source can both just call this.
    if (doc !== undefined && !MENTIONS_MIDI.test(doc)) return;
    if (this.fetching || Date.now() - this.fetchedAt < STALE_MS) return;

    this.fetching = true;
    const generation = ++this.generation;
    void invoke<MidiPorts>("midi_ports")
      .then((ports) => {
        if (generation !== this.generation) return;
        this.outputs = ports.outputs;
        this.fetchedAt = Date.now();
      })
      .catch(() => {
        // A host that cannot be asked is a machine with no MIDI on it, which
        // is a perfectly ordinary machine. The backend already answers with an
        // empty list rather than failing, so reaching here means the command
        // itself did not land — and the right thing to offer is nothing.
        if (generation === this.generation) this.outputs = [];
      })
      .finally(() => {
        if (generation === this.generation) {
          this.fetching = false;
          // So a failure does not retry a thousand times before the next
          // menu: a miss is as warm as a hit.
          if (this.fetchedAt === 0) this.fetchedAt = Date.now();
        }
      });
  }
}

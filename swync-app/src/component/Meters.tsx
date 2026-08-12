/**
 * The `in` and `out` meters, in the title bar beside the transport.
 *
 * They are here rather than in the settings panel for the same reason record
 * is: what a meter is for is the moment you are playing, and a meter behind a
 * tab is one you find out about afterwards. Levels tell you three things a
 * program cannot — that a signal is arriving at all, that a gain is set
 * somewhere usable, and that the output is clipping — and every one of them is
 * something you want to know *while* it is true.
 *
 * Both are peak meters over the last tenth of a second, not levels at the
 * instant of the poll; see `meter.rs`, which explains why that distinction is
 * the whole design. `out` is taken from the same block the device is filled
 * from, so it shows what the room hears, master fader included.
 */

/** What one poll of `audio_levels` answers with. */
export interface AudioLevels {
  /** Peak per input channel since the last ask. Empty when input is off, so
   *  its length is also how many channels the open device has. */
  input: number[];
  /** Peak per output channel — two, always. */
  output: number[];
  /** Frames the input arrived too late, and frames thrown away to keep the
   *  delay from growing. The settings panel is where these are explained. */
  late: number;
  dropped: number;
}

/** Nothing arriving and nothing leaving, for before the first poll answers. */
export const SILENT: AudioLevels = { input: [], output: [], late: 0, dropped: 0 };

/**
 * A level as a fraction of the bar's width.
 *
 * Levels arrive as amplitude, where half of full scale is 0.5 and is also
 * -6 dB — which is most of the way up a fader and a fifth of the way along a
 * linear bar. So the bar is drawn in decibels, over the 60 dB that is the
 * useful range of a small meter: quiet playing is visible on it, which is the
 * whole reason anybody looks at one.
 */
export function meterWidth(level: number): number {
  if (!(level > 0)) return 0;
  const db = 20 * Math.log10(Math.min(level, 1));
  return Math.max(0, Math.min(1, 1 + db / 60));
}

/** One channel. `title` names it the way a program would. */
function Bar({ level, title }: { level: number; title: string }) {
  return (
    <div
      title={title}
      className="min-h-px flex-1 overflow-hidden rounded-[1px] bg-neutral-800"
    >
      <div
        // Clipping is worth its own colour: it is the one thing on a meter you
        // have to do something about, and on the way in it is happening before
        // any program has touched the signal.
        className={
          "h-full transition-[width] duration-75 " +
          (level >= 0.99 ? "bg-red-500" : "bg-emerald-500/80")
        }
        style={{ width: `${meterWidth(level) * 100}%` }}
      />
    </div>
  );
}

/**
 * One stack of bars under a word, sized to sit beside the transport buttons.
 *
 * The channels flex inside a fixed height, so a stereo interface draws two
 * comfortable bars and an eight-in one draws eight thin ones. Past a handful
 * they stop being readable individually and become a texture — which is still
 * the truth about what is arriving, and the tooltips still say which is which.
 */
function Meter({
  label,
  levels,
  channelName,
  title,
}: {
  label: string;
  levels: number[];
  channelName: (channel: number) => string;
  title: string;
}) {
  // A meter with no channels still draws one dead bar rather than collapsing:
  // a chosen device that is not sending anything is a thing to see, and an
  // empty box beside `out` reads as a rendering bug.
  const bars = levels.length > 0 ? levels : [0];

  return (
    <div className="flex flex-col items-center gap-1" title={title}>
      <div className="flex h-6 w-12 flex-col justify-center gap-px">
        {bars.map((level, channel) => (
          <Bar
            key={channel}
            level={level}
            title={levels.length > 0 ? channelName(channel) : title}
          />
        ))}
      </div>
      <span className="text-xs text-neutral-400">{label}</span>
    </div>
  );
}

interface MetersProps {
  levels: AudioLevels;
  /** Whether an input device has been chosen. The `in` meter is drawn only
   *  then — a permanently dead bar on the majority of machines, which never
   *  open an input, is clutter that says nothing. */
  hasInput: boolean;
}

export function Meters({ levels, hasInput }: MetersProps) {
  return (
    <div className="flex items-center gap-2">
      {hasInput && (
        <Meter
          label="in"
          levels={levels.input}
          channelName={(channel) => `input(${channel})`}
          title="What the audio input is sending, peak over the last moment"
        />
      )}
      <Meter
        label="out"
        levels={levels.output}
        channelName={(channel) => (channel === 0 ? "left" : "right")}
        title="What is going to the audio output — the graph, the patterns and the master fader together"
      />
    </div>
  );
}

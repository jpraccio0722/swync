import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { AudioLevels } from "./component/Meters";

/** What a recording may be written as, as `recording_formats` reports it. */
export interface RecordingFormat {
  /** What `Settings.recordingFormat` is set to. */
  id: string;
  label: string;
  extension: string;
  detail: string;
}

/** The app's own settings — the ones that belong to this machine rather than
 *  to the piece. See `settings.rs` for why the two files are separate. */
export interface Settings {
  /** Where recordings go. Null is the open project's own folder, which is the
   *  default and is what most takes want. */
  recordingDir: string | null;
  recordingFormat: string;
  /** What `input` listens to. Null is off, which is where it starts — see
   *  `settings.rs` for why an app does not open a microphone unasked. */
  inputDevice: DeviceInfo | null;
  /** What the graph plays through. Null is the system's own choice. */
  outputDevice: DeviceInfo | null;
  /** How large the editor's text is, in pixels, as ⌘ and the wheel over it
   *  left it. Null is the editor's own size — a machine that has never been
   *  zoomed on. There is no control for it in this panel: the gesture is the
   *  control, and it is done while reading the thing it changes. */
  editorFontSize: number | null;
  /** How far behind the audio a MIDI message is sent, in milliseconds. Zero
   *  on a machine nobody has lined up by ear. See `midi/out.rs` for what it
   *  is correcting — none of which is knowable from here, which is why it is
   *  a control rather than a calculation. */
  midiOffsetMs: number;
  /** The MIDI input whose clock the transport follows, by port name. Null is
   *  swync's own clock. Here rather than in a project because whether you are
   *  the slave tonight is about the rig, not the music — see `settings.rs`. */
  midiClockSource: string | null;
}

/** How a followed clock is getting on, as `midi_clock_status` answers.
 *  Mirrors `Status` in `midi/follow.rs`. */
export interface ClockStatus {
  status: "internal" | "locked" | "lost" | "missing";
  /** The tempo arriving, when one is. */
  bpm: number | null;
}

/** One MIDI port, as `midi_ports` reports it. */
export interface PortInfo {
  /** Its place in the platform's list, which is what `midiout(0)` means. */
  number: number;
  name: string;
}

/** Every MIDI port on this machine, as `midi_ports` answers. */
export interface MidiPorts {
  outputs: PortInfo[];
  inputs: PortInfo[];
}

/**
 * An audio device, as `devices.rs` names it.
 *
 * Two names, for two jobs. `id` is what the device *is* — stable across
 * unplugging and rebooting where the platform can manage it, and what a
 * remembered choice is matched on, so two identical interfaces on one desk are
 * still two devices. `name` is what it is *called*, which is what a person
 * picks from a list and what a sentence about a missing device has to use.
 */
export interface DeviceInfo {
  id: string;
  name: string;
}

/** Every audio device on this machine, and which are open, as
 *  `audio_devices` answers. */
export interface AudioDevices {
  inputs: DeviceInfo[];
  outputs: DeviceInfo[];
  /** The output actually playing — the system default when nothing has been
   *  chosen. */
  output: DeviceInfo;
  /** The input actually open. Null when input is off, which includes a
   *  remembered device that is not plugged in tonight. */
  input: DeviceInfo | null;
  sampleRate: number;
}

/**
 * How a take is going, as `recording_state` answers.
 *
 * `failure` is the reason it stopped when nothing asked it to — a disk filling
 * up mid-performance. There is no command for that to be the answer to, so it
 * waits here to be collected.
 */
export interface RecordingState {
  recording: boolean;
  path: string | null;
  seconds: number;
  dropped: number;
  failure: string | null;
}

/** A recording that has finished, as `stop_recording` answers. */
export interface FinishedRecording {
  path: string;
  seconds: number;
  /** Frames the writer thread could not keep up with — a gap in the file, and
   *  zero on any machine that was keeping up. */
  dropped: number;
}

interface SettingsPanelProps {
  /** Null until the backend has answered, which is a moment on launch. */
  settings: Settings | null;
  onChange: (settings: Settings) => void;
  /** Every format the recorder can actually write, from the backend's table. */
  formats: RecordingFormat[];
  /** What can be opened, and what is. Null until the backend has answered. */
  devices: AudioDevices | null;
  /** How the followed clock is doing, polled while this panel is open. */
  clock: ClockStatus | null;
  /** What MIDI there is to write to. Null until the backend has answered.
   *  Unlike the audio devices there is nothing here to *choose* — a port is
   *  named in the program — so this list is read rather than picked from. */
  midi: MidiPorts | null;
  /** The same poll the title bar's meters are drawn from. Nothing here draws
   *  a level — the meters are in the header, where you are looking while you
   *  play — but the counts that come with them belong beside the devices they
   *  are about. */
  levels: AudioLevels;
  /** Null turns input off. Rejections come back through `onError`, since a
   *  device that refused to open is the one thing here a person must be told
   *  about rather than left to infer from a meter that never moves. */
  onInputDevice: (device: string | null) => void;
  /** Null returns the output to the system's own choice. */
  onOutputDevice: (device: string | null) => void;
  /** The open project, whose folder is where recordings go by default. */
  projectRoot: string | null;
  /** The take that is running, if one is. */
  recording: { path: string; seconds: number } | null;
  /** The last one that finished, so the panel can say where it went. */
  last: FinishedRecording | null;
  onError: (message: string) => void;
}

/** How far the send offset can be dragged, either way. Mirrors
 *  `MAX_OFFSET_MS` in `midi/out.rs`, which is what actually binds — a quarter
 *  second in each direction covers every converter and every piece of gear
 *  anybody is lining up by ear. */
const MAX_MIDI_OFFSET_MS = 250;

/** A length as a performer counts it. */
export function elapsed(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  const minutes = Math.floor(whole / 60);
  return `${minutes}:${String(whole % 60).padStart(2, "0")}`;
}

/** The last part of a path, which is what a file is called. */
function basename(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/**
 * Which devices the audio comes from and goes to, where recordings go, and
 * what they are written as.
 *
 * The record button is in the title bar with play and stop, because it is a
 * thing you reach for while the music is running. What is left for a panel is
 * everything you decide *before* a take and then forget about — and that is
 * why this is a panel rather than a dialog on the button: a question asked at
 * the moment you press record is a question asked several seconds too late.
 * The devices are here for the same reason. The *meters* are not: they went to
 * the title bar, beside the transport, because what a level is for is the
 * moment you are playing — see `component/Meters.tsx`. What is left of them
 * here is the one sentence a meter cannot say, which is that the two devices
 * are not keeping step with each other.
 *
 * The formats are the backend's own list rather than one written out again
 * here. A dropdown offering something the recorder cannot write would be a
 * promise broken after the performance, which is the worst possible moment to
 * find out.
 *
 * This is one of the right panel's three tabs; the frame around it, including
 * the width and the drag handle, belongs to `RightPanel`.
 */
export function SettingsPanel({
  settings,
  onChange,
  midi,
  clock,
  formats,
  devices,
  levels,
  onInputDevice,
  onOutputDevice,
  projectRoot,
  recording,
  last,
  onError,
}: SettingsPanelProps) {
  if (!settings) {
    // Nothing worth drawing yet, and a folder shown before the file has been
    // read would be a claim about where a recording goes that may be wrong.
    return <div className="h-full" />;
  }

  const chooseFolder = async () => {
    try {
      const selected = await open({ directory: true, multiple: false });
      if (typeof selected === "string") {
        onChange({ ...settings, recordingDir: selected });
      }
    } catch (e) {
      onError(String(e));
    }
  };

  const format = formats.find((f) => f.id === settings.recordingFormat);

  // A device that was chosen and is not open is the one state worth saying out
  // loud: the interface has been unplugged, or something else on the machine
  // has taken it, and a meter that never moves is not an explanation.
  const inputMissing =
    settings.inputDevice !== null && devices !== null && devices.input === null;
  const outputMissing =
    settings.outputDevice !== null &&
    devices !== null &&
    devices.output.id !== settings.outputDevice.id;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto p-3">
      <section>
        <h3 className="text-[11px] font-semibold uppercase tracking-wide text-neutral-500">
          Audio
        </h3>

        <div className="mt-2">
          <label htmlFor="audio-input" className="text-xs text-neutral-400">
            Input
          </label>
          <select
            id="audio-input"
            value={settings.inputDevice?.id ?? ""}
            onChange={(e) => onInputDevice(e.target.value || null)}
            className="mt-1 w-full rounded-md bg-neutral-800 px-2 py-1 text-xs text-neutral-200"
          >
            <option value="">Off</option>
            {devices?.inputs.map((device) => (
              <option key={device.id} value={device.id}>
                {device.name}
              </option>
            ))}
            {/* A remembered device that is not on the machine tonight still
                belongs in the list, or the dropdown would silently show
                something else as the current choice. */}
            {settings.inputDevice !== null &&
              !devices?.inputs.some((d) => d.id === settings.inputDevice!.id) && (
                <option value={settings.inputDevice.id}>
                  {settings.inputDevice.name} (not connected)
                </option>
              )}
          </select>

          {inputMissing ? (
            <p className="mt-1.5 text-[11px] leading-snug text-amber-500/80">
              {settings.inputDevice?.name} is not available, so{" "}
              <code className="text-amber-500/70">input</code> is silent. Plug
              it back in and choose it again.
            </p>
          ) : settings.inputDevice ? (
            <p className="mt-1.5 text-[11px] leading-snug text-neutral-500">
              Listening. The <span className="text-neutral-400">in</span> meter
              beside the transport shows each channel —{" "}
              <code className="text-neutral-400">input(0)</code> is the first,{" "}
              <code className="text-neutral-400">input(1)</code> the second.
            </p>
          ) : (
            <p className="mt-1.5 text-[11px] leading-snug text-neutral-500">
              Nothing is being listened to. Choose a device and{" "}
              <code className="text-neutral-400">input(0)</code> becomes its
              first channel, <code className="text-neutral-400">input(1)</code>{" "}
              its second — a signal like any other, so it filters, delays and
              plays into a pattern the same way.
            </p>
          )}

          {/* Only ever seen when the two devices cannot keep in step — which
              is a buffer size worth looking at, and is invisible otherwise
              because neither one is anybody's command to fail. It stays here
              rather than going to the header with the meters: it is a sentence
              about how the devices are configured, and this is where they are
              configured. */}
          {(levels.late > 0 || levels.dropped > 0) && (
            <p className="mt-1.5 text-[11px] leading-snug text-amber-500/80">
              The input and output devices are not keeping step: {levels.late}{" "}
              frames arrived too late and {levels.dropped} were dropped. A
              larger buffer size on either device usually settles it.
            </p>
          )}
        </div>

        <div className="mt-4">
          <label htmlFor="audio-output" className="text-xs text-neutral-400">
            Output
          </label>
          <select
            id="audio-output"
            value={settings.outputDevice?.id ?? ""}
            onChange={(e) => onOutputDevice(e.target.value || null)}
            // Changing device mid-take would change the rate the file is being
            // written at, which a WAV header cannot say twice. The backend
            // refuses it; this is the same refusal made visible.
            disabled={recording !== null}
            className="mt-1 w-full rounded-md bg-neutral-800 px-2 py-1 text-xs text-neutral-200 disabled:text-neutral-500"
          >
            <option value="">
              {devices
                ? `System default (${devices.output.name})`
                : "System default"}
            </option>
            {devices?.outputs.map((device) => (
              <option key={device.id} value={device.id}>
                {device.name}
              </option>
            ))}
            {settings.outputDevice !== null &&
              !devices?.outputs.some((d) => d.id === settings.outputDevice!.id) && (
                <option value={settings.outputDevice.id}>
                  {settings.outputDevice.name} (not connected)
                </option>
              )}
          </select>

          {recording !== null ? (
            <p className="mt-1.5 text-[11px] leading-snug text-neutral-500">
              Not while a take is running — a recording is written at the rate
              the device gave it.
            </p>
          ) : outputMissing ? (
            <p className="mt-1.5 text-[11px] leading-snug text-amber-500/80">
              {settings.outputDevice?.name} is not available, so this is
              playing through {devices?.output.name} instead.
            </p>
          ) : (
            devices && (
              <p className="mt-1.5 text-[11px] leading-snug text-neutral-500">
                Playing through {devices.output.name} at{" "}
                {Math.round(devices.sampleRate)} Hz. An input has to run at that
                rate too — both feed one graph.
              </p>
            )
          )}
        </div>
      </section>

      {/* Read, not chosen. Every other list in this panel is a control — you
          pick a device and the app opens it — but a MIDI port is named inside
          the program (`midiout("deluge")`), because which synth a part is
          written for belongs to the piece rather than to the desk. See
          `midi/ports.rs`.

          This is not how you are meant to *learn* a port's name: the editor
          offers them inside `midiout("`, which is where you are looking when
          you need one, and is how every other name in the language is found.
          What this is for is the questions the editor cannot answer while you
          are not writing — whether the interface you just plugged in showed
          up, and what its number is. */}
      <section>
        <h3 className="text-[11px] font-semibold uppercase tracking-wide text-neutral-500">
          MIDI
        </h3>

        <div className="mt-2">
          <label className="text-xs text-neutral-400">Outputs</label>
          {midi === null ? (
            <p className="mt-1 text-[11px] text-neutral-600">Looking…</p>
          ) : midi.outputs.length === 0 ? (
            <p className="mt-1 text-[11px] leading-snug text-neutral-500">
              Nothing to send to. Plug in an interface, or turn on a virtual
              bus — the IAC Driver on a Mac, loopMIDI on Windows.
            </p>
          ) : (
            <ul className="mt-1 space-y-0.5">
              {midi.outputs.map((port) => (
                <li key={port.number} className="flex items-baseline gap-2">
                  <span className="w-4 shrink-0 text-right font-mono text-[11px] text-neutral-500">
                    {port.number}
                  </span>
                  <span className="break-all font-mono text-[11px] leading-snug text-neutral-300">
                    {port.name}
                  </span>
                </li>
              ))}
            </ul>
          )}
          <p className="mt-1.5 text-[11px] leading-snug text-neutral-500">
            Name one in a program to play it:{" "}
            <span className="font-mono text-neutral-400">
              play(bass, midiout("{midi?.outputs[0]?.name.split(" ")[0].toLowerCase() ?? "deluge"}"))
            </span>
            . Any part of the name will do, case and all, and so will the number
            beside it. The editor offers these inside{" "}
            <span className="font-mono text-neutral-400">midiout("</span> — you
            should not need to come back here to copy one out.
          </p>
        </div>

        <div className="mt-4">
          <label className="text-xs text-neutral-400">Inputs</label>
          {midi === null ? (
            <p className="mt-1 text-[11px] text-neutral-600">Looking…</p>
          ) : midi.inputs.length === 0 ? (
            <p className="mt-1 text-[11px] leading-snug text-neutral-500">
              Nothing to read from. Plug in a keyboard or a control surface.
            </p>
          ) : (
            <ul className="mt-1 space-y-0.5">
              {midi.inputs.map((port) => (
                <li key={port.number} className="flex items-baseline gap-2">
                  <span className="w-4 shrink-0 text-right font-mono text-[11px] text-neutral-500">
                    {port.number}
                  </span>
                  <span className="break-all font-mono text-[11px] leading-snug text-neutral-300">
                    {port.name}
                  </span>
                </li>
              ))}
            </ul>
          )}
          <p className="mt-1.5 text-[11px] leading-snug text-neutral-500">
            Play one:{" "}
            <span className="font-mono text-neutral-400">
              play(midiin("{midi?.inputs[0]?.name.split(" ")[0].toLowerCase() ?? "keys"}"), lead)
            </span>
            . Read a knob:{" "}
            <span className="font-mono text-neutral-400">cc("push", 74)</span>.
            These are offered in the editor too.
          </p>
        </div>

        {/* Chosen here rather than named in a program, and that is the one
            exception to how every other MIDI port works. Sending clock to a
            synth is about the piece — its arps have to line up with the part
            written for it — so `midiclock("deluge")` is in the program. But
            whether *this* machine is the slave tonight is about the rig: the
            same piece is master in the studio and slave when somebody else's
            box is running the room. See `midi/follow.rs`. */}
        <div className="mt-4">
          <label htmlFor="midi-clock" className="text-xs text-neutral-400">
            Follow clock
          </label>
          <select
            id="midi-clock"
            value={settings.midiClockSource ?? ""}
            onChange={(e) =>
              onChange({ ...settings, midiClockSource: e.target.value || null })
            }
            className="mt-1 w-full rounded-md bg-neutral-800 px-2 py-1 text-xs text-neutral-200"
          >
            <option value="">Internal — swync keeps its own time</option>
            {midi?.inputs.map((port) => (
              <option key={port.number} value={port.name}>
                {port.name}
              </option>
            ))}
            {/* A remembered port that is not plugged in tonight is kept in the
                list rather than silently falling back to Internal, which would
                lose the choice the next time anything was saved. */}
            {settings.midiClockSource !== null &&
              !midi?.inputs.some((p) => p.name === settings.midiClockSource) && (
                <option value={settings.midiClockSource}>
                  {settings.midiClockSource} (not connected)
                </option>
              )}
          </select>
          {clock !== null && settings.midiClockSource !== null && (
            <p
              className={`mt-1.5 text-[11px] leading-snug ${
                clock.status === "locked" ? "text-neutral-500" : "text-amber-500/80"
              }`}
            >
              {clock.status === "locked" ? (
                <>
                  Following at{" "}
                  <span className="font-mono text-neutral-300">
                    {clock.bpm?.toFixed(1) ?? "…"} bpm
                  </span>
                  . The tempo control is theirs while this is on.
                </>
              ) : clock.status === "missing" ? (
                <>That port is not on this machine, so nothing is arriving.</>
              ) : (
                <>
                  No clock arriving. Still playing at the tempo it last saw —
                  silence would be worse — but it is no longer in sync.
                </>
              )}
            </p>
          )}
        </div>

        <div className="mt-4">
          <label htmlFor="midi-offset" className="text-xs text-neutral-400">
            Send offset
          </label>
          <div className="mt-1 flex items-center gap-2">
            <input
              id="midi-offset"
              type="range"
              min={-MAX_MIDI_OFFSET_MS}
              max={MAX_MIDI_OFFSET_MS}
              step={1}
              value={settings.midiOffsetMs}
              onChange={(e) =>
                onChange({ ...settings, midiOffsetMs: Number(e.target.value) })
              }
              className="flex-1"
            />
            <span className="w-14 shrink-0 text-right font-mono text-[11px] text-neutral-300">
              {settings.midiOffsetMs > 0 ? "+" : ""}
              {settings.midiOffsetMs} ms
            </span>
          </div>
          <p className="mt-1.5 text-[11px] leading-snug text-neutral-500">
            Lines gear you are <em>sending</em> to up with what you hear.
            Nothing can work this out for you — it is the converter, the driver and whatever is at
            the far end of the cable — so set it by ear against a sound the app
            is making itself.
          </p>
        </div>
      </section>

      <section>
        <h3 className="text-[11px] font-semibold uppercase tracking-wide text-neutral-500">
          Recording
        </h3>

        <div className="mt-2">
          <label className="text-xs text-neutral-400">Output folder</label>
          <p className="mt-1 break-all font-mono text-[11px] leading-snug text-neutral-300">
            {settings.recordingDir ??
              (projectRoot ? (
                <>
                  {projectRoot}
                  <span className="ml-1 font-sans text-neutral-600">
                    (the project's own folder)
                  </span>
                </>
              ) : (
                // The one state a person has to do something about: with no
                // folder chosen and no project open there is nowhere a
                // recording could go that anybody would find again.
                <span className="font-sans text-amber-500/80">
                  No project is open, so there is nowhere to record to — choose
                  a folder.
                </span>
              ))}
          </p>
          <div className="mt-1.5 flex gap-3 text-[11px] text-neutral-500">
            <button
              onClick={() => void chooseFolder()}
              className="transition-colors hover:text-neutral-200"
            >
              Choose…
            </button>
            {settings.recordingDir && (
              <>
                <button
                  onClick={() => onChange({ ...settings, recordingDir: null })}
                  className="transition-colors hover:text-neutral-200"
                >
                  Use the project's folder
                </button>
                <button
                  onClick={() =>
                    void revealItemInDir(settings.recordingDir!).catch((e) =>
                      onError(String(e)),
                    )
                  }
                  className="transition-colors hover:text-neutral-200"
                >
                  Reveal
                </button>
              </>
            )}
          </div>
        </div>

        <div className="mt-4">
          <label
            htmlFor="recording-format"
            className="text-xs text-neutral-400"
          >
            File type
          </label>
          <select
            id="recording-format"
            value={settings.recordingFormat}
            onChange={(e) =>
              onChange({ ...settings, recordingFormat: e.target.value })
            }
            className="mt-1 w-full rounded-md bg-neutral-800 px-2 py-1 text-xs text-neutral-200"
          >
            {formats.map((f) => (
              <option key={f.id} value={f.id}>
                {f.label}
              </option>
            ))}
          </select>
          {format && (
            <p className="mt-1 text-[11px] leading-snug text-neutral-500">
              {format.detail}
            </p>
          )}
        </div>
      </section>

      <section>
        <h3 className="text-[11px] font-semibold uppercase tracking-wide text-neutral-500">
          {recording ? "Recording now" : "Last recording"}
        </h3>

        {recording ? (
          <div className="mt-2">
            <div className="flex items-center gap-2">
              <span className="h-2 w-2 animate-pulse rounded-full bg-red-500" />
              <span className="font-mono text-xs text-neutral-200">
                {elapsed(recording.seconds)}
              </span>
            </div>
            <p className="mt-1 break-all text-[11px] leading-snug text-neutral-500">
              {basename(recording.path)}
            </p>
          </div>
        ) : last ? (
          <div className="mt-2">
            <p className="break-all font-mono text-[11px] leading-snug text-neutral-300">
              {basename(last.path)}
            </p>
            <p className="mt-0.5 text-[11px] text-neutral-500">
              {elapsed(last.seconds)} long
            </p>
            {/* Only ever seen on a machine that could not keep up with its own
                disk, and worth saying plainly: the file has a gap in it. */}
            {last.dropped > 0 && (
              <p className="mt-0.5 text-[11px] leading-snug text-amber-500/80">
                {last.dropped} frames were dropped — the disk could not keep up,
                so there is a gap in this take.
              </p>
            )}
            <button
              onClick={() =>
                void revealItemInDir(last.path).catch((e) => onError(String(e)))
              }
              className="mt-1 text-[11px] text-neutral-500 transition-colors hover:text-neutral-200"
            >
              Reveal
            </button>
          </div>
        ) : (
          <p className="mt-2 text-[11px] leading-snug text-neutral-500">
            Nothing recorded yet. The record button beside play captures
            everything you hear — the graph, the patterns and the master fader —
            from the moment you press it.
          </p>
        )}
      </section>
    </div>
  );
}

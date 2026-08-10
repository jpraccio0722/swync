import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import PlayIcon from "../assets/play.svg?react";
import StopIcon from "../assets/stop.svg?react";
import RecordIcon from "../assets/record.svg?react";
import { elapsed } from "../SettingsPanel";
import { ValueControl } from "./ValueControl";

const MIN_BPM = 40;
const MAX_BPM = 240;

/**
 * A time signature: how many of what note make a bar.
 *
 * `bottom` is a note value — 4 is a quarter, 8 an eighth — so the pair reads
 * as it is written on a stave.
 */
export interface Meter {
  top: number;
  bottom: number;
}

/**
 * The signatures the menu offers.
 *
 * A list rather than two number fields, because a signature is a choice from a
 * short set of things musicians actually write, and a free pair invites 4/5.
 * The backend refuses one it cannot write anyway; this keeps that refusal off
 * the screen.
 */
const METERS: Meter[] = [
  { top: 4, bottom: 4 },
  { top: 3, bottom: 4 },
  { top: 2, bottom: 4 },
  { top: 5, bottom: 4 },
  { top: 6, bottom: 8 },
  { top: 7, bottom: 8 },
  { top: 12, bottom: 8 },
];

const meterLabel = (m: Meter) => `${m.top}/${m.bottom}`;

/** The engine's global controls, as the backend reports them. */
export interface TransportState {
  /** Beats per minute. The beat is the quarter note, in any signature. */
  bpm: number;
  /** The time signature, which is how many beats make a bar. */
  meter: Meter;
  /** Linear amplitude, 0 to 1. */
  volume: number;
}

interface TransportProps {
  /** Evaluate the active tab and hand the result to the engine. Whether it
   *  compiled is of no interest here — the problems panel has already been
   *  told — so the answer is discarded rather than typed away. */
  play: () => unknown;
  /** Silence everything the engine and scheduler are holding. */
  stop: () => void | Promise<void>;
  /**
   * Where the controls sit, or null until something has said — on launch,
   * before the engine has answered.
   */
  state: TransportState | null;
  /** Report a move, so the open project can remember it. */
  onChange: (state: TransportState) => void;
  /**
   * Whether the engine is holding a program — which is what the lit play
   * button says, and is not the same as whether anything is audible: a
   * program that builds silence is still one the transport is running.
   */
  playing: boolean;
  /**
   * The take that is running, and how long it has been going — null when
   * nothing is being recorded.
   */
  recording: { path: string; seconds: number } | null;
  /** Start a take, or end the one that is running. */
  onToggleRecording: () => void;
}

/**
 * Play, stop, record, tempo and volume, in the title bar.
 *
 * They live here rather than in the transport panel because they are the
 * controls that must stay reachable with every panel shut — the engine keeps
 * running whatever is on screen, and the things that steer it should not be
 * behind a tab. Keyboard players still have ⌘, and ⌘.
 *
 * Record is here for the strongest version of that reason: a take begins
 * because of something you just heard, and a record button behind a tab is one
 * that gets pressed a bar late. It plays as well as records — see
 * `toggleRecording` in `App` for why the two are one gesture. Where the file
 * goes and what it is written as are decided in the settings panel, before any
 * of that — see `SettingsPanel`.
 *
 * Tempo and volume belong to the engine rather than to the program, so their
 * values are never assumed here: they are read from the engine on launch and
 * from the project's own file when one is opened, both of which happen a level
 * up — a setting that is remembered belongs to the thing that remembers it.
 * What is left here is the surface. Each edit goes straight out: sends are
 * cheap enough to fire on every frame of a drag, and the audio side is built to
 * take them mid-performance.
 */
export function Transport({
  play,
  stop,
  state,
  onChange,
  playing,
  recording,
  onToggleRecording,
}: TransportProps) {
  // Green for playing, but not while a take is running: the record button is
  // the light then, and two lit buttons would be two claims about one state.
  // Which of them is on tells you what pressing the other one will do.
  const lit = playing && recording === null;

  const setBpm = useCallback(
    (bpm: number) => {
      if (!state) return;
      onChange({ ...state, bpm });
      invoke("set_tempo", { bpm }).catch((e) => console.error("could not set tempo:", e));
    },
    [state, onChange],
  );

  // Changing the signature changes what a bar is, and therefore what every
  // bare pattern in the program fills. The engine is told at once so the
  // transport is right, but what is already playing was lowered against the
  // old bar and keeps its lengths until the file is played again — the same
  // gap a tempo drag leaves on the persistent graph.
  const setMeter = useCallback(
    (meter: Meter) => {
      if (!state) return;
      onChange({ ...state, meter });
      invoke("set_meter", { top: meter.top, bottom: meter.bottom }).catch((e) =>
        console.error("could not set the time signature:", e),
      );
    },
    [state, onChange],
  );

  // The control works in percent, which is how volume is read and typed; the
  // engine's own scale is linear amplitude.
  const setVolumePercent = useCallback(
    (percent: number) => {
      if (!state) return;
      const volume = percent / 100;
      onChange({ ...state, volume });
      invoke("set_master_volume", { volume }).catch((e) =>
        console.error("could not set volume:", e),
      );
    },
    [state, onChange],
  );

  return (
    <div className="flex items-center gap-1">
      <button
        onClick={() => void play()}
        title={lit ? "Playing — run again (⌘,)" : "Run (⌘,)"}
        aria-pressed={lit}
        className={
          "rounded-md p-1.5 text-xs transition-colors " +
          (lit
            ? "bg-green-950/60 text-green-400 hover:bg-green-900/60 hover:text-green-300"
            : "text-neutral-400 hover:bg-neutral-800 hover:text-blue-400")
        }
      >
        <div className="flex flex-col items-center min-w-10">
          <PlayIcon className="h-6 w-6" />
          play
        </div>
      </button>

      <button
        onClick={() => void stop()}
        title="Stop (⌘.)"
        className="rounded-md p-1.5 text-xs text-neutral-400 transition-colors hover:bg-neutral-800 hover:text-red-400"
      >
        <div className="flex flex-col items-center min-w-10">
          <StopIcon className="h-6 w-6" />
          stop
        </div>
      </button>
      <button
        onClick={onToggleRecording}
        title={
          recording
            ? "Stop — the music stops and the file is finished and saved"
            : "Record — runs the file and captures everything you hear"
        }
        aria-pressed={recording !== null}
        className={
          "rounded-md p-1.5 text-xs transition-colors " +
          (recording
            ? "bg-red-950/60 text-red-400 hover:bg-red-900/60 hover:text-red-300"
            : "text-neutral-400 hover:bg-neutral-800 hover:text-red-400")
        }
      >
        {/* A stop square while a take is running, because that is what
            pressing it now does — it ends the music as well as the file. A
            record dot that stays a record dot reads as an invitation to start
            a second take, which is the one thing this button cannot do while
            one is open. The clock takes the place of the word for the same
            reason: while it runs, how long it has been is what to know. */}
        {recording ? (
          <div className="flex flex-col items-center min-w-10">
            <StopIcon className="h-6 w-6" />
            <span className="font-mono tabular-nums">{elapsed(recording.seconds)}</span>
          </div>
        ) : (
          <div className="flex flex-col items-center min-w-10">
            <RecordIcon className="h-6 w-6" />
            <div>record</div>
          </div>
        )}
      </button>

      {/* Nothing to show until the engine has told us where the controls sit —
          a slider with a made-up value on it would be a lie about the sound. */}
      {state && (
        <>
          <ValueControl
            label="tempo"
            value={state.bpm}
            min={MIN_BPM}
            max={MAX_BPM}
            step={1}
            unit=" bpm"
            onChange={setBpm}
          />

          <label className="flex flex-col items-center gap-1 text-xs text-neutral-400">
            <select
              value={meterLabel(state.meter)}
              onChange={(e) => {
                const next = METERS.find((m) => meterLabel(m) === e.target.value);
                if (next) setMeter(next);
              }}
              title="Time signature — how many beats make a bar"
              className="rounded-md bg-neutral-800 px-1.5 py-0.5 text-neutral-200"
            >
              {/* A project's file is editable by hand, and the backend takes
                  any signature it can write — 9/8 among them. One that is not
                  on this list is still what the project is in, so it is added
                  rather than shown as a blank box. */}
              {(METERS.some((m) => meterLabel(m) === meterLabel(state.meter))
                ? METERS
                : [...METERS, state.meter]
              ).map((m) => (
                <option key={meterLabel(m)} value={meterLabel(m)}>
                  {meterLabel(m)}
                </option>
              ))}
            </select>
            <div>time signature</div>
          </label>
          <ValueControl
            label="volume"
            value={state.volume * 100}
            min={0}
            max={100}
            step={1}
            unit="%"
            onChange={setVolumePercent}
          />
        </>
      )}
    </div>
  );
}

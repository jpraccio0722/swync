import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

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
  /** The open project, whose folder is where recordings go by default. */
  projectRoot: string | null;
  /** The take that is running, if one is. */
  recording: { path: string; seconds: number } | null;
  /** The last one that finished, so the panel can say where it went. */
  last: FinishedRecording | null;
  onError: (message: string) => void;
}

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
 * Where recordings go, and what they are written as.
 *
 * The record button is in the title bar with play and stop, because it is a
 * thing you reach for while the music is running. What is left for a panel is
 * everything you decide *before* a take and then forget about — and that is
 * why this is a panel rather than a dialog on the button: a question asked at
 * the moment you press record is a question asked several seconds too late.
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
  formats,
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

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto p-3">
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

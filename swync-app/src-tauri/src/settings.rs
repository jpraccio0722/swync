//! What the app remembers, as against what a project does.
//!
//! `project.rs` holds the other half of this, and the split is the point. A
//! project's file travels with the piece — its tempo and its signature mean
//! the same thing on anybody's machine, so they belong beside the files and
//! are meant to be shared. What is here does not: an output folder is a path
//! into one disk, and a project carrying `/Users/someone/Music` in it would be
//! a project that records into nowhere the moment it is opened by anyone else.
//!
//! So this is one file in the app's config directory, beside the remembered
//! session, and it is the same for every project opened on this machine.
//!
//! Nothing in it is required, and a file that cannot be read is a first run
//! rather than a failure — the same answer the session file gives, and for the
//! same reason: none of this is worth refusing to start over, and every field
//! has a default the app would have used anyway.

use crate::devices::DeviceInfo;
use crate::recorder::Format;

use std::path::{Path, PathBuf};

/// What the settings file is called, in the app's config directory.
pub const FILE: &str = "settings.json";

/// The app's own settings.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Where recordings are written.
    ///
    /// `None` means the project's own folder, which is the useful default: a
    /// take belongs with the piece it is of, and it is somewhere a project
    /// already has permission to write. It is not resolved to a real path
    /// here — a folder chosen once should not be quietly re-pointed when a
    /// different project is opened.
    #[serde(default)]
    pub recording_dir: Option<String>,
    /// What a recording is written as.
    #[serde(default)]
    pub recording_format: Format,
    /// Which device `input` listens to, by the name the machine calls it.
    ///
    /// `None` is off, and off is where it starts. An app that opened the
    /// default microphone the first time it ran would ask for permission
    /// nobody had asked it to want, and would put an open mic into a room with
    /// speakers in it — which is a feedback loop that arrives before any of
    /// this has been read.
    ///
    /// Both halves of [`DeviceInfo`] are kept: the id is what the device is
    /// found again by, and the name is what a panel says about one that cannot
    /// be found — "the Scarlett is not connected" is a sentence about a device
    /// there is nothing left to ask.
    ///
    /// A device that is not plugged in tonight is kept rather than forgotten,
    /// for the same reason a recording folder on an unmounted drive is: it is
    /// still the answer, and dropping it would lose it for good the next time
    /// anything else was saved.
    #[serde(default)]
    pub input_device: Option<DeviceInfo>,
    /// Which device the graph plays through. `None` is the system's own
    /// choice, which is what the app did before this could be chosen and is
    /// what it falls back to when a remembered device is gone.
    #[serde(default)]
    pub output_device: Option<DeviceInfo>,
    /// How large the editor's text is, in pixels — what ⌘ and the wheel over
    /// the editor set.
    ///
    /// It is here rather than in a project because it is about a pair of eyes
    /// and a screen: a size chosen for a laptop on a dark stage would be an
    /// odd thing to arrive with a piece somebody else opened. `None` is the
    /// editor's own size, which is where every machine starts and what the
    /// frontend supplies — the number itself is the editor's business, and a
    /// default written down twice is one that can disagree with itself.
    #[serde(default)]
    pub editor_font_size: Option<f32>,
}

impl Settings {
    /// Where a recording made now should be written, given the project that is
    /// open — `None` when there is none.
    ///
    /// The refusal is worth its own sentence because it is the one state a
    /// person has to do something about: with no folder chosen and no project
    /// open, there is nowhere a file could go that anybody would find again.
    pub fn recording_dir_for(&self, project: Option<&str>) -> Result<PathBuf, String> {
        if let Some(dir) = &self.recording_dir {
            return Ok(PathBuf::from(dir));
        }
        match project {
            Some(root) => Ok(PathBuf::from(root)),
            None => Err(
                "there is nowhere to put the recording: open a project, or choose \
                 an output folder in the settings panel"
                    .to_string(),
            ),
        }
    }
}

/// Read the app's settings, or the defaults if there are none to read.
pub fn read(path: &Path) -> Settings {
    match std::fs::read_to_string(path) {
        // A file half-written by a crash, or written by a version that has
        // since changed the shape, is a first run rather than a failure.
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

/// Write them, making the config directory if this is the first time.
pub fn write(path: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "swync-settings-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("should create a temp folder");
        dir
    }

    #[test]
    fn settings_that_have_never_been_written_are_the_defaults() {
        let root = temp("first-run");
        let settings = read(&root.join(FILE));
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.recording_format, Format::Wav16);
        assert_eq!(settings.recording_dir, None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn what_is_written_is_what_is_read_back() {
        let root = temp("round-trip");
        let path = root.join("nested").join(FILE);
        let settings = Settings {
            recording_dir: Some(root.display().to_string()),
            recording_format: Format::Wav32,
            input_device: Some(DeviceInfo {
                id: "coreaudio:AppleUSBAudioEngine:2i2".to_string(),
                name: "Scarlett 2i2 USB".to_string(),
            }),
            output_device: Some(DeviceInfo {
                id: "coreaudio:BuiltInHeadphoneOutputDevice".to_string(),
                name: "External Headphones".to_string(),
            }),
            editor_font_size: Some(18.0),
        };
        write(&path, &settings).expect("should write");
        assert_eq!(read(&path), settings);
        std::fs::remove_dir_all(&root).ok();
    }

    /// It is JSON in a folder anybody can open, so it can be nonsense by the
    /// time it is read — and none of it is worth refusing to start over.
    #[test]
    fn a_settings_file_that_cannot_be_read_is_a_first_run_rather_than_a_failure() {
        let root = temp("nonsense");
        let path = root.join(FILE);

        std::fs::write(&path, "{ this is not json").expect("should write");
        assert_eq!(read(&path), Settings::default());

        std::fs::remove_dir_all(&root).ok();
    }

    /// A folder on a drive that is not plugged in today is still the folder
    /// that was chosen, and is kept rather than quietly forgotten — the
    /// recording that cannot be started says which path it could not write to,
    /// which is something a person can act on. Dropping the setting instead
    /// would lose it for good the next time anything else was saved.
    #[test]
    fn a_folder_that_is_not_there_today_is_still_remembered() {
        let root = temp("unmounted");
        let path = root.join(FILE);
        let gone = root.join("moved-away").display().to_string();

        write(&path, &Settings {
            recording_dir: Some(gone.clone()),
            recording_format: Format::Wav16,
            ..Settings::default()
        })
        .expect("should write");
        assert_eq!(read(&path).recording_dir, Some(gone));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_recording_with_no_folder_chosen_lands_in_the_project() {
        let settings = Settings::default();
        assert_eq!(
            settings.recording_dir_for(Some("/tmp/piece")).expect("should answer"),
            PathBuf::from("/tmp/piece")
        );

        let err = settings.recording_dir_for(None).expect_err("should refuse");
        assert!(err.contains("choose an output folder"), "got {err}");
    }

    /// A folder that was chosen on purpose beats the project's, and keeps
    /// beating it when a different project is opened.
    #[test]
    fn a_chosen_folder_is_where_recordings_go_whatever_project_is_open() {
        let settings = Settings {
            recording_dir: Some("/tmp/takes".to_string()),
            recording_format: Format::Wav16,
            ..Settings::default()
        };
        assert_eq!(
            settings.recording_dir_for(Some("/tmp/piece")).expect("should answer"),
            PathBuf::from("/tmp/takes")
        );
        assert_eq!(
            settings.recording_dir_for(None).expect("should answer"),
            PathBuf::from("/tmp/takes")
        );
    }
}

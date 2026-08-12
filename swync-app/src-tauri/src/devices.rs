//! Naming an audio device, so that the same one can be found again.
//!
//! A device has two names and they are for different things. What the host
//! *calls* it — "Scarlett 2i2 USB", "MacBook Pro Microphone" — is what a
//! person picks from a list. What identifies it is a `cpal::DeviceId`, which
//! the platform keeps stable across disconnections and reboots where it can,
//! and which cpal documents as the thing to persist.
//!
//! Both are kept, and the split is why. The id is what a remembered choice is
//! matched on: two identical interfaces on one desk have the same name, and a
//! device that is renamed is still the device. The name is what is *said* — in
//! the picker, and in "the Scarlett is not connected", which is a sentence
//! about a device that cannot be looked up to be asked what it is called.

use cpal::traits::{DeviceTrait, HostTrait};

/// What `cpal` is asked to wait for a stream to start.
///
/// The alternative it offers is `None`, which means *wait indefinitely*, and
/// indefinitely is a real duration on a device that will not start. Passing a
/// bound is right whether or not it is honoured — and it is documented as one
/// not every backend honours. CoreAudio does not, which is why
/// [`ANSWER_TIMEOUT`] exists as well and is the one that actually holds.
pub const START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a caller waits to be told how opening a device went.
///
/// Not how long the open is *given* — the thread that owns the stream carries
/// on either way. This is how long the answer is waited for, and it is bounded
/// because [`START_TIMEOUT`] turns out not to bind: measured against a device
/// another application was holding, and against one whose microphone
/// permission was never granted, both took nine minutes to refuse.
///
/// Opening a device is a command the editor is waiting on, so what those nine
/// minutes would otherwise be is a settings panel that has stopped answering.
/// Twelve seconds is longer than every device that works takes — the ones
/// measured took a tenth of a second — and long enough for a permission dialog
/// to be read and clicked.
pub const ANSWER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

/// A device as the settings file remembers it and the picker shows it.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// What it is, for finding it again.
    pub id: String,
    /// What it is called, for saying so.
    pub name: String,
}

/// What to call a device, and how to find it again — or `None` for one that
/// has been unplugged between being listed and being asked.
pub fn describe(device: &cpal::Device) -> Option<DeviceInfo> {
    Some(DeviceInfo { id: device.id().ok()?.to_string(), name: device.to_string() })
}

/// Every audio input on this machine.
///
/// A host that cannot be asked is an empty list rather than a failure: what
/// the panel then offers is nothing but "off", which is the truth about what
/// can be opened.
pub fn inputs() -> Vec<DeviceInfo> {
    match cpal::default_host().input_devices() {
        Ok(devices) => devices.filter_map(|device| describe(&device)).collect(),
        Err(_) => Vec::new(),
    }
}

/// Every audio output on this machine.
pub fn outputs() -> Vec<DeviceInfo> {
    match cpal::default_host().output_devices() {
        Ok(devices) => devices.filter_map(|device| describe(&device)).collect(),
        Err(_) => Vec::new(),
    }
}

/// The input that was chosen, by the id it was chosen under.
pub fn input(id: &str) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    host.input_devices()
        .map_err(|e| format!("could not list the audio inputs: {e}"))?
        .find(|device| device.id().map(|found| found.to_string() == id).unwrap_or(false))
        .ok_or_else(|| "that audio input is not on this machine".to_string())
}

/// The output that was chosen, by the id it was chosen under.
pub fn output(id: &str) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    host.output_devices()
        .map_err(|e| format!("could not list the audio outputs: {e}"))?
        .find(|device| device.id().map(|found| found.to_string() == id).unwrap_or(false))
        .ok_or_else(|| "that audio output is not on this machine".to_string())
}

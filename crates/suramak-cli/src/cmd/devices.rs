//! `devices` — lists input audio devices (name + default marker) when built with
//! the `capture` feature; otherwise reports that recording support is not compiled
//! in. Either way this is a status report: exit 0.

use anyhow::Result;
use suramak_asr::capture;
use suramak_asr::AsrError;

pub fn run() -> Result<()> {
    match capture::list_input_devices_detailed() {
        Ok(devices) => {
            if devices.is_empty() {
                println!("no input devices found");
            } else {
                println!("input devices:");
                for device in devices {
                    if device.is_default {
                        println!("  - {} (default)", device.name);
                    } else {
                        println!("  - {}", device.name);
                    }
                }
            }
            println!(
                "(system audio taps: `System audio` in the app's Settings, macOS builds only)"
            );
            Ok(())
        }
        Err(AsrError::CaptureUnsupported) => {
            println!("built without recording support (feature `capture`)");
            println!("rebuild with: cargo build --release -p suramak-cli --features capture");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

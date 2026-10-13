//! `devices` — lists input audio devices when built with the `capture` feature;
//! otherwise reports that capture is not compiled in (still exit 0: it is a status report).

use anyhow::Result;
use suramak_asr::capture;
use suramak_asr::AsrError;

pub fn run() -> Result<()> {
    match capture::list_input_devices() {
        Ok(devices) => {
            if devices.is_empty() {
                println!("no input devices found");
            } else {
                println!("input devices:");
                for name in devices {
                    println!("  - {name}");
                }
            }
            Ok(())
        }
        Err(AsrError::CaptureUnsupported) => {
            println!(
                "device enumeration is not available in this build: the `capture` feature is off"
            );
            println!("rebuild with: cargo build --release -p suramak-cli --features capture");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

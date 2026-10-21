//! Link the macOS Swift runtime rpath into the CLI binary.
//!
//! The release build compiles the CLI with `speech-transcriber` (the same Swift
//! bridge as the app), which makes the binary depend on
//! `@rpath/libswift_Concurrency.dylib`. macOS's other Swift dylibs are
//! referenced by absolute path, but the back-deployed concurrency runtime is
//! not — without `/usr/lib/swift` on the rpath list dyld aborts the process at
//! launch. The app crate carries the same line for its binary; see
//! `crates/suramak-ui/build.rs` for the full story (v0.3.0 shipped broken
//! without it).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,/usr/lib/swift");
}

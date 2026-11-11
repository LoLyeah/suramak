//! Link the macOS runtime rpaths the CLI binary needs when it is shipped in the
//! app bundle.
//!
//! **Swift runtime (M10B).** The release build compiles the CLI with
//! `speech-transcriber` (the same Swift bridge as the app), which makes the
//! binary depend on `@rpath/libswift_Concurrency.dylib`. macOS's other Swift
//! dylibs are referenced by absolute path, but the back-deployed concurrency
//! runtime is not — without `/usr/lib/swift` on the rpath list dyld aborts the
//! process at launch. The app crate carries the same line for its binary; see
//! `crates/suramak-ui/build.rs` for the full story (v0.3.0 shipped broken
//! without it).
//!
//! **Parakeet (M16F).** With the `parakeet` feature the CLI links
//! `@rpath/libsherpa-onnx-c-api.dylib`. In the packaged app (`make-app.sh` copies
//! the CLI into `Contents/MacOS`) that library lives in `Contents/Frameworks`,
//! so the binary needs `@executable_path/../Frameworks` exactly like the app
//! binary. Emitted only for macOS builds with the feature on; every other build
//! links exactly as before.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,/usr/lib/swift");
    if std::env::var_os("CARGO_FEATURE_PARAKEET").is_some() {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path/../Frameworks");
    }
}

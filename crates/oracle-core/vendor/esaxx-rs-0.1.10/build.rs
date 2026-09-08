// NOTICE: This file has been modified from the original esaxx-rs 0.1.10 as
// published on crates.io. The change is the `static_crt` call below, which was
// an unconditional `true` upstream. Nothing else in this vendored copy is
// changed. This notice is required by Apache-2.0 section 4(b); the crate's own
// LICENSE ships beside this file and the reason for the change, along with the
// condition for dropping the patch, is recorded in the workspace Cargo.toml
// and in THIRD_PARTY.md.

#[cfg(feature = "cpp")]
#[cfg(not(target_os = "macos"))]
fn main() {
    cc::Build::new()
        .cpp(true)
        .flag("-std=c++11")
        // Windows: dynamic CRT (/MD) to match ort_sys prebuilt binaries.
        // Other non-macOS (Linux): static CRT is fine.
        .static_crt(cfg!(not(target_os = "windows")))
        .file("src/esaxx.cpp")
        .include("src")
        .compile("esaxx");
}

#[cfg(feature = "cpp")]
#[cfg(target_os = "macos")]
fn main() {
    cc::Build::new()
        .cpp(true)
        .flag("-std=c++11")
        .flag("-stdlib=libc++")
        .static_crt(true)
        .file("src/esaxx.cpp")
        .include("src")
        .compile("esaxx");
}

#[cfg(not(feature = "cpp"))]
fn main() {}

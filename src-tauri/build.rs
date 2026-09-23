fn main() {
    tauri_build::build();
    link_windows_resource_into_test_binaries();
}

/// Pass tauri-build's compiled Windows resource to the integration-test
/// targets too.
///
/// `tauri_build::build()` embeds icon, version info and the app manifest —
/// whose only content is the dependency on
/// `Microsoft.Windows.Common-Controls` 6.0
/// (`tauri-build/src/windows-app-manifest.xml`) — through
/// `embed-resource::compile`, which emits **`cargo:rustc-link-arg-bins`
/// only**: the app binary gets it, test executables get nothing. Measured
/// on this machine (Windows 11, build 26200): `System32\comctl32.dll` is
/// version 5.82 and exports no `TaskDialogIndirect`, while the WinSxS v6
/// copies that do export it are only reached through that manifest
/// dependency. A test binary that links the code pulling
/// `TaskDialogIndirect` therefore died before `main` with
/// `STATUS_ENTRYPOINT_NOT_FOUND` (0xC0000139) — no output, no panic, just a
/// non-zero exit — the first time a test referenced `tauri::test`'s mock
/// app (`tests/asset_scope.rs`, the only target in this package that
/// does; the unit tests under `src/` need none of it).
///
/// `cargo:rustc-link-arg-tests` reaches exactly those `[[test]]` targets
/// — Cargo validates the key against the package's test targets, which is
/// why the resource stays out of the unit-test harnesses (the all-targets
/// key would pass the file to the binary twice, and `CVTRES` dies on the
/// duplicated VERSION resource: measured, `CVT1100 duplicate resource`).
/// The path is the `compile()` above's output; on MSVC it is `.res`
/// content under a library name ("`.res`es are linkable under MSVC as
/// well as normal libraries", `embed-resource/src/windows_msvc.rs`), and a
/// toolchain that names its artifact differently simply does not match
/// the file and embeds nothing here.
fn link_windows_resource_into_test_binaries() {
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    let resource = std::path::Path::new(&out_dir).join("resource.lib");
    if resource.is_file() {
        println!("cargo:rustc-link-arg-tests={}", resource.display());
    }
}

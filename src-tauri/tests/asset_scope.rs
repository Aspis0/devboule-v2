//! The asset protocol's scope, checked with bytes against the folder the
//! daemon really writes — the trap this test exists for: the static scope
//! in `tauri.conf.json` is written against `$CACHE`, while the natural
//! instinct is `$APPLOCALDATA` (Tauri's *app* data dir, which carries the
//! identifier `com.devboule.desktop`). The daemon's runtime dir carries no
//! identifier (`%LOCALAPPDATA%\Devboule`, `RuntimePaths::from_env` — the
//! same rule the spawned process runs), so a scope spelled against the
//! wrong base covers a folder nothing writes and every preview is a 403.
//!
//! Everything asserted here goes through Tauri's own machinery: the app is
//! built from this crate's real `tauri.conf.json` (`generate_context!`), so
//! the pattern under test is the one production expands, with the same
//! `$CACHE` resolver and the same glob options `Scope::new` sets.

use devboule_daemon::RuntimePaths;
use tauri::Manager;

#[test]
fn the_asset_scope_covers_the_daemons_real_previews_folder_and_only_it() {
    let app = tauri::test::mock_builder()
        .build(tauri::generate_context!())
        .expect("a mock app from this crate's own tauri.conf.json");
    let scope = app.asset_protocol_scope();

    // One config entry, two spellings of it: the literal glob Tauri parsed
    // from `tauri.conf.json`, and the canonicalized one (`\\?\C:\…`) that
    // `is_allowed` matches a resolved request against — `push_pattern`
    // adds both, canonicalizing the longest prefix that exists
    // (`canonicalize_parent`, `tauri/scope/fs.rs`), so the canonical
    // spelling is present even while the `previews` folder does not exist
    // yet: it climbs to the runtime dir (or `%LOCALAPPDATA%` on a machine
    // that has never run) and re-appends the tail. Both must name the
    // folder the daemon writes — spelled byte for byte by the daemon's own
    // rule below.
    let runtime = RuntimePaths::from_env().expect("LOCALAPPDATA is set on Windows");
    let previews = runtime.dir.join("previews");
    let suffix = format!("{}\\*", previews.display());
    let patterns = scope.allowed_patterns();
    assert_eq!(
        patterns.len(),
        2,
        "the literal and the canonical spelling: {:?}",
        patterns.iter().map(|p| p.as_str()).collect::<Vec<_>>()
    );
    for pattern in &patterns {
        assert!(
            pattern.as_str().ends_with(&suffix),
            "pattern {:?} does not concede {}",
            pattern.as_str(),
            suffix
        );
    }

    // Tauri's own resolution of `$CACHE`, byte for byte against the
    // daemon's own rule — this equality is what makes the static glob
    // cover the folder the daemon writes to.
    let cache = app.path().cache_dir().expect("$CACHE resolves");
    assert_eq!(
        cache.join("Devboule"),
        runtime.dir,
        "$CACHE/Devboule is where RuntimePaths::from_env puts the runtime dir; \
         if these ever diverge the scope covers nothing"
    );

    // The trap, on the other side: `$APPLOCALDATA` is cache + identifier and
    // is NOT the daemon's directory — a scope written against it would be
    // green as configuration and dead at the first fetch.
    let app_local = app
        .path()
        .app_local_data_dir()
        .expect("$APPLOCALDATA resolves");
    assert_ne!(
        app_local, runtime.dir,
        "$APPLOCALDATA carries the identifier; the daemon's folder does not"
    );

    // The path-level claims, negative first: everything the scope must NOT
    // open, spelled the way a wrong scope would have spelled it.
    for outside in [
        // What `$APPLOCALDATA/previews/*` would have covered.
        app_local.join("previews").join("ab12.png"),
        // The runtime dir's siblings and the attachments store.
        runtime.dir.join("attachments").join("s.a.1").join("ab.png"),
        // The scope is one level (`require_literal_separator`): the daemon
        // writes flat, and a subfolder is outside it.
        previews.join("sub").join("ab12.png"),
        // The workspace itself — the whole point of staging a copy.
        std::env::current_dir()
            .expect("cwd")
            .join("tauri.conf.json"),
    ] {
        assert!(
            !scope.is_allowed(&outside),
            "outside the concession: {}",
            outside.display()
        );
    }

    // And the byte-level claim, on a copy that actually exists: the daemon's
    // real folder, a real file in it, read back through the same
    // `is_allowed` (which canonicalizes a path it can resolve — the
    // spelling the glob was built from and the spelling on disk must agree).
    let existed_before = previews.exists();
    std::fs::create_dir_all(&previews).expect("create the daemon's previews folder");
    // A name this run owns, opened `create_new` (review-images §7): a fixed
    // name with `write` would truncate whatever a real daemon staged
    // under it, and `create_new` turns a collision into a red test
    // instead of a lost copy.
    let copy = previews.join(format!(
        "imgs-scope-selftest-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&copy)
            .expect("stage a throwaway copy, never truncating a real one");
        file.write_all(b"scope self-test")
            .expect("write the throwaway");
    }
    assert!(
        scope.is_allowed(&copy),
        "the staged copy itself must be inside the concession: {}",
        copy.display()
    );
    // Cleanup removes only what this test wrote, and the folder only if it
    // created it: this is the daemon's live directory, not a fixture.
    std::fs::remove_file(&copy).expect("remove the throwaway copy");
    if !existed_before {
        let _ = std::fs::remove_dir(&previews);
    }
}

/// The override the static scope cannot spell, closed by the app's own
/// concession instead of left declared: `DEVBOULE_RUNTIME_DIR` moves the
/// daemon's copies out of `$CACHE/Devboule` while `tauri.conf.json` stays
/// where it was, so at start the app concedes the `previews` folder of
/// the runtime dir it actually resolves (`devboule_lib::concede_previews_of`)
/// — the same function `run` calls with `RuntimePaths::from_env`. This
/// test drives it over a directory that is NOT this environment's default
/// and measures the whole width of the grant: its copy (real file,
/// canonicalized request) is inside, and attachments, a subfolder, and the
/// runtime dir itself are not.
#[test]
fn a_runtime_dir_override_gets_the_same_flat_concession_as_the_default() {
    let app = tauri::test::mock_builder()
        .build(tauri::generate_context!())
        .expect("a mock app from this crate's own tauri.conf.json");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let override_dir = std::env::temp_dir().join(format!(
        "devboule-other-runtime-{}-{stamp}",
        std::process::id()
    ));
    devboule_lib::concede_previews_of(&app, &override_dir);

    let scope = app.asset_protocol_scope();
    let previews = override_dir.join("previews");
    std::fs::create_dir_all(&previews).expect("the override's previews folder");
    let copy = previews.join(format!("ab12-{stamp}.png"));
    std::fs::write(&copy, b"override copy").expect("a real file to canonicalize");

    assert!(
        scope.is_allowed(&copy),
        "a copy staged under the override must be inside the concession: {}",
        copy.display()
    );
    assert!(
        !scope.is_allowed(override_dir.join("attachments").join("ab.png")),
        "only previews is conceded, never the sibling folders"
    );
    assert!(
        !scope.is_allowed(previews.join("sub").join("x.png")),
        "the concession is one level, like the static scope"
    );
    assert!(
        !scope.is_allowed(override_dir.join("ab.png")),
        "the runtime dir itself is not conceded, only its previews"
    );

    let _ = std::fs::remove_dir_all(&override_dir);
}

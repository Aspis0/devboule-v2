use devboule_daemon::{DaemonError, RuntimePaths};

fn main() {
    // This target is listed with `required-features = ["server"]` in the
    // manifest. Keep the guard here too so a direct target invocation cannot
    // accidentally turn the client library into an in-process server.
    #[cfg(not(feature = "server"))]
    compile_error!("devboule-daemon binary requires the `server` feature");

    #[cfg(feature = "server")]
    match devboule_daemon::run() {
        Ok(()) => {}
        Err(DaemonError::AlreadyRunning) => {
            // A second daemon is not an error: the running one already owns
            // the sessions and journal. Say so, then exit 0 so a caller that
            // checks exit codes sees "nothing to do", not "failure".
            eprintln!(
                "Devboule is already running: another daemon owns this user's runtime folder. \
                 Nothing was started; use the running Devboule app."
            );
            std::process::exit(0);
        }
        Err(DaemonError::Io(error)) if error.raw_os_error() == Some(5) => {
            // ERROR_ACCESS_DENIED: a different failure than a live second
            // daemon, and it needs a different action from the user.
            let runtime_dir = RuntimePaths::from_env()
                .map(|paths| paths.dir.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "<runtime folder could not be determined>".to_string());
            eprintln!(
                "Devboule cannot access its runtime folder at {runtime_dir} (permission denied). \
                 Check the folder's permissions, then start Devboule again."
            );
            std::process::exit(1);
        }
        Err(error) => {
            let runtime_dir = RuntimePaths::from_env()
                .map(|paths| paths.dir.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "<runtime folder could not be determined>".to_string());
            eprintln!(
                "Devboule could not start: {error}. Runtime folder: {runtime_dir}. \
                 That folder holds your session history, so do not delete it to \
                 work around this. Report the error instead."
            );
            std::process::exit(1);
        }
    }
}

fn main() {
    // This target is listed with `required-features = ["server"]` in the
    // manifest. Keep the guard here too so a direct target invocation cannot
    // accidentally turn the client library into an in-process server.
    #[cfg(not(feature = "server"))]
    compile_error!("devboule-daemon binary requires the `server` feature");

    #[cfg(feature = "server")]
    match devboule_daemon::run() {
        Ok(()) => {}
        Err(devboule_daemon::DaemonError::AlreadyRunning) => {
            std::process::exit(0);
        }
        Err(error) => {
            eprintln!("devboule-daemon: {error}");
            std::process::exit(1);
        }
    }
}

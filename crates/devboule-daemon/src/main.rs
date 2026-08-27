fn main() {
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

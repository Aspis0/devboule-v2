//! The Unix PTY round trip: a child behind a real pseudoterminal reads what
//! is written, resizing the grid reaches the terminal, and the child's own
//! exit code comes back to the daemon. The Windows twin (ConPTY's source
//! selection and its DSR stall) lives in `portable_pty_tests.rs`.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use portable_pty::{CommandBuilder, PtySize};

/// Read until `needle` shows up, bounded: a reader that never sees it must
/// fail the test, not hang the suite. The reader is drained on its own
/// thread so the wait is a timeout on the channel.
fn read_until(reader: Box<dyn Read + Send>, needle: &[u8]) -> Vec<u8> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut seen = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    seen.extend_from_slice(&buffer[..count]);
                    if seen.windows(needle.len()).any(|window| window == needle) {
                        break;
                    }
                }
            }
        }
        let _ = tx.send(seen);
    });
    rx.recv_timeout(Duration::from_secs(15))
        .expect("the pty answered within 15s")
}

#[test]
fn a_pty_echoes_a_written_line_reports_its_grid_and_the_exit_code() {
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("the pty opens");
    let mut builder = CommandBuilder::new("/bin/sh");
    builder.args(["-c", "read line; echo \"got:$line\"; exit 7"]);
    let mut child = pair.slave.spawn_command(builder).expect("the shell spawns");
    drop(pair.slave);

    let mut writer = pair.master.take_writer().expect("the writer exists");
    pair.master
        .resize(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("the grid resizes");
    let resized = pair.master.get_size().expect("the grid reports its size");
    assert_eq!(
        (resized.rows, resized.cols),
        (40, 120),
        "the resize reached the master"
    );

    let reader = pair.master.try_clone_reader().expect("the reader exists");
    writer.write_all(b"hello\n").expect("the line is written");
    writer.flush().expect("the line is flushed");

    let seen = read_until(reader, b"got:hello");
    let text = String::from_utf8_lossy(&seen);
    assert!(
        text.contains("got:hello"),
        "the shell read the written line and echoed it: {text:?}"
    );

    let status = child.wait().expect("the child is reaped");
    assert_eq!(status.exit_code(), 7, "the child's own exit code");
}

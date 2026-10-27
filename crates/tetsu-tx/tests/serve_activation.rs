//! `tetsu-tx serve` adopts the socket systemd passes in, checked with
//! `systemd-socket-activate`. A missing tool fails the test, as in boot_e2e.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const NAME: &str = "org.tetsu.tx.v1";

fn call_get_info(path: &std::path::Path) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut stream = loop {
        match UnixStream::connect(path) {
            Ok(s) => break s,
            Err(e) => {
                assert!(Instant::now() < deadline, "cannot connect: {e}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(b"{\"method\":\"org.varlink.service.GetInfo\"}\0")
        .unwrap();
    let mut reply = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).expect("reply before timeout");
        if byte[0] == 0 {
            break;
        }
        reply.push(byte[0]);
    }
    serde_json::from_slice(&reply).expect("reply is JSON")
}

#[test]
fn serve_adopts_the_socket_systemd_passes_and_leaves_it() {
    let tool = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .chain(["/usr/bin".into(), "/usr/lib/systemd".into()])
        .map(|d| d.join("systemd-socket-activate"))
        .find(|p| p.is_file());
    let tool = tool.expect(
        "systemd-socket-activate not found: install the `systemd` package to run this test",
    );
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join(NAME);
    let mut child = Command::new(tool)
        .arg("-l")
        .arg(&socket)
        .arg(format!("--fdname={NAME}"))
        .arg(env!("CARGO_BIN_EXE_tetsu-tx"))
        .arg("serve")
        .arg("--socket")
        .arg(&socket)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn systemd-socket-activate");

    let info = call_get_info(&socket);
    let interfaces = info["parameters"]["interfaces"]
        .as_array()
        .unwrap_or_else(|| panic!("no interfaces in {info}"));
    assert!(interfaces.iter().any(|i| i == NAME), "{info}");

    // Without --accept the activator execs the service in its own process.
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(status.success(), "the service was not running");
    child.wait().expect("wait for the service");
    assert!(
        socket.exists(),
        "the adopted socket must not be unlinked by the service"
    );
}

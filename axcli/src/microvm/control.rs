use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use crate::ioctl;

static CONTROL_SERVER: OnceLock<ControlServer> = OnceLock::new();

struct ControlServer {
    listener: UnixListener,
    socket_path: String,
    instance_fd: i32,
    instance_id: usize,
    max_vcpus: u8,
    desired_vcpus: AtomicU8,
}

unsafe impl Sync for ControlServer {}
unsafe impl Send for ControlServer {}

pub fn start_control_socket(
    instance_fd: i32,
    instance_id: usize,
    default_vcpus: u8,
    max_vcpus: u8,
) -> Result<(), String> {
    let socket_path = std::env::var("AXCLI_MICROVM_CONTROL_SOCKET")
        .unwrap_or_else(|_| format!("/tmp/eqvisor-microvm-{}.sock", instance_id));
    match fs::remove_file(&socket_path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => {
            return Err(format!(
                "Failed to remove stale control socket {}: {}",
                socket_path, err
            ));
        }
    }

    let listener = UnixListener::bind(&socket_path)
        .map_err(|err| format!("Failed to bind control socket {}: {}", socket_path, err))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o666))
        .map_err(|err| format!("Failed to chmod control socket {}: {}", socket_path, err))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("Failed to set control socket nonblocking: {}", err))?;

    let server = ControlServer {
        listener,
        socket_path: socket_path.clone(),
        instance_fd,
        instance_id,
        max_vcpus,
        desired_vcpus: AtomicU8::new(default_vcpus),
    };

    CONTROL_SERVER
        .set(server)
        .map_err(|_| "microVM control socket already initialized".to_string())?;
    info!(
        "microVM control socket listening path={} instance={} default_vcpus={} max_vcpus={}",
        socket_path, instance_id, default_vcpus, max_vcpus
    );
    Ok(())
}

pub fn poll_control_once() {
    let Some(server) = CONTROL_SERVER.get() else {
        return;
    };

    loop {
        match server.listener.accept() {
            Ok((stream, _addr)) => handle_client(server, stream),
            Err(err) if err.kind() == ErrorKind::WouldBlock => break,
            Err(err) => {
                warn!(
                    "microVM control socket accept failed path={}: {}",
                    server.socket_path, err
                );
                break;
            }
        }
    }
}

fn handle_client(server: &ControlServer, mut stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    let mut buf = [0u8; 256];
    let request_len = match stream.read(&mut buf) {
        Ok(0) => {
            let _ = writeln!(stream, "ERR empty command");
            return;
        }
        Ok(n) => n,
        Err(err) => {
            let _ = writeln!(stream, "ERR read failed: {}", err);
            return;
        }
    };
    let request = String::from_utf8_lossy(&buf[..request_len]);

    match execute_command(server, request.trim()) {
        Ok(response) => {
            let _ = writeln!(stream, "OK {}", response);
        }
        Err(err) => {
            let _ = writeln!(stream, "ERR {}", err);
        }
    }
}

fn execute_command(server: &ControlServer, request: &str) -> Result<String, String> {
    let mut parts = request.split_whitespace();
    let Some(command) = parts.next() else {
        return Err("empty command".to_string());
    };

    match command {
        "resize-vcpu" | "set-vcpu" => {
            let count = parts
                .next()
                .ok_or_else(|| "missing vCPU count".to_string())?
                .parse::<u8>()
                .map_err(|err| format!("invalid vCPU count: {}", err))?;
            if count == 0 || count > server.max_vcpus {
                return Err(format!(
                    "vCPU count {} outside allowed range 1..={}",
                    count, server.max_vcpus
                ));
            }
            ioctl::ioctl_set_instance_vcpu_count(
                server.instance_fd,
                server.instance_id as u64,
                count as u32,
            )?;
            server.desired_vcpus.store(count, Ordering::Release);
            info!(
                "microVM control resize-vcpu instance={} desired={} max={}",
                server.instance_id, count, server.max_vcpus
            );
            Ok(format!(
                "desired_vcpus={} max_vcpus={}",
                count, server.max_vcpus
            ))
        }
        "get-vcpu" | "status" => {
            let desired = server.desired_vcpus.load(Ordering::Acquire);
            Ok(format!(
                "desired_vcpus={} max_vcpus={}",
                desired, server.max_vcpus
            ))
        }
        _ => Err(format!("unknown command '{}'", command)),
    }
}

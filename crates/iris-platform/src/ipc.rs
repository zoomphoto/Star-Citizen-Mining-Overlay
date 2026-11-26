use crossbeam_channel::{unbounded, Receiver};
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcCommand {
    OpenSettings,
    ToggleOverlay,
    ToggleDebugOverlay,
    TimedOverlay,
    InteractiveOverlay,
    ClearBreadcrumbs,
    Quit,
}

impl IpcCommand {
    pub fn as_wire(&self) -> &'static str {
        match self {
            IpcCommand::OpenSettings => "settings",
            IpcCommand::ToggleOverlay => "toggle",
            IpcCommand::ToggleDebugOverlay => "debug",
            IpcCommand::TimedOverlay => "timed",
            IpcCommand::InteractiveOverlay => "interactive",
            IpcCommand::ClearBreadcrumbs => "clear_breadcrumbs",
            IpcCommand::Quit => "quit",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s.trim() {
            "settings" => Some(IpcCommand::OpenSettings),
            "toggle" => Some(IpcCommand::ToggleOverlay),
            "debug" => Some(IpcCommand::ToggleDebugOverlay),
            "timed" => Some(IpcCommand::TimedOverlay),
            "interactive" => Some(IpcCommand::InteractiveOverlay),
            "clear_breadcrumbs" => Some(IpcCommand::ClearBreadcrumbs),
            "quit" => Some(IpcCommand::Quit),
            _ => None,
        }
    }
}

/// Cross-platform single-instance lock & local loopback IPC (`127.0.0.1:<ephemeral>` + token in `Data/instance.addr`).
///
/// Enables `sc-overlay --toggle`, `--timed`, `--interactive`, `--settings`, and `--quit` to signal
/// an already-running instance on both Windows 11 and Bazzite Linux (KDE / GNOME / Gamescope).
pub struct SingleInstanceIpc {
    addr_file: PathBuf,
    rx: Receiver<IpcCommand>,
}

impl SingleInstanceIpc {
    pub fn acquire_or_send(data_dir: &Path, command_if_running: IpcCommand) -> Result<Self, bool> {
        let _ = fs::create_dir_all(data_dir);
        let addr_file = data_dir.join("instance.addr");

        if let Some((port, token)) = Self::read_addr_file(&addr_file) {
            if Self::send_command(port, &token, command_if_running) {
                return Err(true);
            }
        }

        let listener = match TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)) {
            Ok(l) => l,
            Err(_) => return Err(false),
        };
        let port = match listener.local_addr() {
            Ok(a) => a.port(),
            Err(_) => return Err(false),
        };

        let token = format!("{:08x}-{:08x}", std::process::id(), port as u32 ^ 0x5C_07_E2_A1);
        let _ = fs::write(&addr_file, format!("{}:{}\n", port, token));

        let (tx, rx) = unbounded();
        let expected_token = token.clone();
        thread::Builder::new()
            .name("sc-overlay-ipc".to_string())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                    let mut buf = [0u8; 256];
                    let mut s = stream;
                    if let Ok(n) = s.read(&mut buf) {
                        if let Ok(line) = std::str::from_utf8(&buf[..n]) {
                            if let Some((tok, cmd_str)) = line.trim().split_once(':') {
                                if tok == expected_token {
                                    if let Some(cmd) = IpcCommand::from_wire(cmd_str) {
                                        let _ = s.write_all(b"ok\n");
                                        let _ = tx.send(cmd);
                                    }
                                }
                            }
                        }
                    }
                }
            })
            .ok();

        Ok(Self { addr_file, rx })
    }

    pub fn try_recv(&self) -> Option<IpcCommand> {
        self.rx.try_recv().ok()
    }

    fn read_addr_file(path: &Path) -> Option<(u16, String)> {
        let content = fs::read_to_string(path).ok()?;
        let (port_str, token) = content.trim().split_once(':')?;
        let port: u16 = port_str.parse().ok()?;
        Some((port, token.to_string()))
    }

    fn send_command(port: u16, token: &str, cmd: IpcCommand) -> bool {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let Ok(mut stream) =
            TcpStream::connect_timeout(&addr.into(), Duration::from_millis(350))
        else {
            return false;
        };
        let _ = stream.set_write_timeout(Some(Duration::from_millis(350)));
        let _ = stream.set_read_timeout(Some(Duration::from_millis(350)));
        let payload = format!("{}:{}\n", token, cmd.as_wire());
        if stream.write_all(payload.as_bytes()).is_err() {
            return false;
        }
        let mut resp = [0u8; 16];
        match stream.read(&mut resp) {
            Ok(n) => std::str::from_utf8(&resp[..n])
                .map(|s| s.trim() == "ok")
                .unwrap_or(false),
            Err(_) => false,
        }
    }
}

impl Drop for SingleInstanceIpc {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.addr_file);
    }
}


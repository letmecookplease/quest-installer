use crate::{AppError, Result};
use serde::Serialize;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::Command,
    time::timeout,
};

pub const PORT: u16 = 5038;
pub const SHORT: Duration = Duration::from_secs(25);
pub const TRANSFER: Duration = Duration::from_secs(60 * 30);

#[derive(Clone)]
pub struct Adb {
    executable: PathBuf,
    home: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Device {
    pub serial: String,
    pub status: String,
    pub name: String,
    pub wireless: bool,
}

impl Adb {
    pub fn new(executable: PathBuf, home: PathBuf) -> Self {
        Self { executable, home }
    }

    pub fn command(&self) -> Command {
        let mut cmd = command(&self.executable);
        cmd.args(["-P", "5038"])
            .env_remove("ADB_SERVER_SOCKET")
            .env_remove("ANDROID_ADB_SERVER_ADDRESS")
            .env("ANDROID_ADB_SERVER_PORT", PORT.to_string())
            .env("ANDROID_USER_HOME", &self.home)
            .env("ADB_MDNS_AUTO_CONNECT", "0");
        cmd
    }

    pub async fn run(&self, args: &[&str], duration: Duration) -> Result<String> {
        output(self.command().args(args), duration).await
    }

    pub fn target(&self, serial: &str) -> Result<Command> {
        if serial.is_empty()
            || serial.len() > 255
            || !serial
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._:-[]%".contains(&c))
        {
            return Err(AppError::new("input", "Invalid headset serial."));
        }
        let mut cmd = self.command();
        cmd.args(["-s", serial]);
        Ok(cmd)
    }

    pub async fn shell(&self, serial: &str, script: &str) -> Result<String> {
        output(self.target(serial)?.args(["shell", "-T", script]), SHORT).await
    }

    pub async fn install_apk(
        &self,
        serial: &str,
        apk: &Path,
        mut on_bytes: impl FnMut(u64, u64),
    ) -> Result<String> {
        let file = tokio::fs::File::open(apk).await?;
        let size = file.metadata().await?.len();
        if size == 0 {
            return Err(AppError::new("apk", "The APK is empty."));
        }
        let command = format!("cmd package install -r -d -S {size} -");
        let mut child = self
            .target(serial)?
            .args(["shell", "-T", &command])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::new("adb", "Could not open the APK transfer stream."))?;
        let write = async move {
            let mut file = file.take(size);
            let mut buffer = vec![0; 256 * 1024];
            let mut sent = 0;
            let mut last_update = Instant::now();
            on_bytes(0, size);
            loop {
                let read = file.read(&mut buffer).await?;
                if read == 0 {
                    break;
                }
                stdin.write_all(&buffer[..read]).await?;
                sent += read as u64;
                if sent < size && last_update.elapsed() >= Duration::from_millis(200) {
                    on_bytes(sent, size);
                    last_update = Instant::now();
                }
            }
            drop(stdin);
            if sent != size {
                return Err(AppError::new(
                    "apk",
                    "The APK ended before all bytes could be sent.",
                ));
            }
            on_bytes(sent, size);
            Ok(())
        };
        let (sent, reply) = timeout(TRANSFER, async {
            tokio::join!(write, child.wait_with_output())
        })
        .await
        .map_err(|_| AppError::new("timeout", "The APK install timed out."))?;
        let reply = check_output(reply?)?;
        sent?;
        Ok(reply)
    }

    pub async fn track(&self) -> Result<TcpStream> {
        self.run(&["start-server"], SHORT).await?;
        let mut socket = timeout(SHORT, TcpStream::connect(("127.0.0.1", PORT)))
            .await
            .map_err(|_| AppError::new("timeout", "ADB server did not respond."))??;
        let request = "host:track-devices-l";
        socket
            .write_all(format!("{:04x}{request}", request.len()).as_bytes())
            .await?;
        let mut status = [0; 4];
        timeout(SHORT, socket.read_exact(&mut status))
            .await
            .map_err(|_| AppError::new("timeout", "ADB device tracking did not respond."))??;
        if &status != b"OKAY" {
            return Err(AppError::new("adb", "ADB did not start device tracking."));
        }
        Ok(socket)
    }
}

pub async fn next_devices(socket: &mut TcpStream) -> Result<Vec<Device>> {
    let mut length = [0; 4];
    socket.read_exact(&mut length).await?;
    let length = std::str::from_utf8(&length)
        .ok()
        .and_then(|s| usize::from_str_radix(s, 16).ok())
        .ok_or_else(|| AppError::new("adb", "ADB sent an invalid device list."))?;
    let mut frame = vec![0; length];
    socket.read_exact(&mut frame).await?;
    Ok(parse_devices(&String::from_utf8_lossy(&frame)))
}

pub fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields.next()?;
            let status = fields.next()?;
            let wireless = serial.contains(':') || serial.ends_with("._adb-tls-connect._tcp");
            let name = fields
                .find_map(|part| part.strip_prefix("model:"))
                .map(|model| model.replace('_', " "))
                .unwrap_or_else(|| serial.to_owned());
            Some(Device {
                serial: serial.to_owned(),
                status: status.to_owned(),
                name,
                wireless,
            })
        })
        .collect()
}

pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.kill_on_drop(true).stdin(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd
}

pub async fn output(cmd: &mut Command, duration: Duration) -> Result<String> {
    let output = timeout(duration, cmd.output())
        .await
        .map_err(|_| AppError::new("timeout", "The operation timed out."))??;
    check_output(output)
}

fn check_output(output: std::process::Output) -> Result<String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout.trim(), stderr.trim())
        .trim()
        .to_owned();
    if !output.status.success() || raw.starts_with("Failure [") {
        let (code, message) = if raw.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE") {
            (
                "signature",
                "The installed app was signed with a different key.",
            )
        } else if raw.contains("INSTALL_FAILED_INSUFFICIENT_STORAGE")
            || raw.contains("No space left on device")
        {
            ("storage", "The headset is out of space.")
        } else if raw.contains("Permission denied") {
            (
                "permission",
                "The headset denied access to this file or directory.",
            )
        } else if raw.contains("unauthorized") {
            (
                "device",
                "Authorize debugging on the headset and try again.",
            )
        } else {
            ("adb", "The command failed.")
        };
        return Err(AppError::with_raw(code, message, raw));
    }
    Ok(stdout.trim_end_matches(['\r', '\n']).to_owned())
}

pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

pub fn local_endpoint(value: &str) -> Result<SocketAddr> {
    let address: SocketAddr = value.parse().map_err(|_| {
        AppError::new(
            "input",
            "Enter a headset IP address and port, such as 192.168.1.20:5555.",
        )
    })?;
    let local = match address.ip() {
        std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
        std::net::IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
    };
    if !local || address.port() == 0 {
        return Err(AppError::new(
            "input",
            "Use an address on your local network.",
        ));
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_device_states_and_quotes_shell_paths() {
        let devices = parse_devices("usb1 device product:panther model:Quest_3 transport_id:1\n192.168.0.4:5555 unauthorized transport_id:2\n");
        assert_eq!(devices[0].name, "Quest 3");
        assert!(!devices[0].wireless);
        assert_eq!(devices[1].status, "unauthorized");
        assert!(devices[1].wireless);
        assert_eq!(quote("a'b"), "'a'\"'\"'b'");
        assert!(local_endpoint("192.168.0.4:5555").is_ok());
        assert!(local_endpoint("8.8.8.8:5555").is_err());
    }
}

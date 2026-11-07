//! Bluetooth Classic RFCOMM (SPP) link to the printer, via a raw Linux socket.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};

use crate::protocol::{take_frames, Encoder, Frame, Status, RSP_FIRMWARE, RSP_SERIAL, RSP_STATUS};

const AF_BLUETOOTH: libc::c_int = 31;
const BTPROTO_RFCOMM: libc::c_int = 3;

#[repr(C)]
struct SockaddrRc {
    family: libc::sa_family_t,
    bdaddr: [u8; 6],
    channel: u8,
}

fn parse_addr(addr: &str) -> anyhow::Result<[u8; 6]> {
    let parts: Vec<u8> = addr
        .split(':')
        .map(|p| u8::from_str_radix(p, 16))
        .collect::<Result<_, _>>()
        .with_context(|| format!("bad Bluetooth address {addr}"))?;
    let mut b: [u8; 6] = parts
        .try_into()
        .map_err(|_| anyhow::anyhow!("bad Bluetooth address {addr}"))?;
    b.reverse(); // bdaddr_t is little-endian
    Ok(b)
}

/// Errors worth retrying: the printer or BlueZ is still closing the previous link.
fn is_transient(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>()
        .and_then(|e| e.raw_os_error())
        .is_some_and(|c| {
            [
                libc::EBUSY,
                libc::ECONNREFUSED,
                libc::ECONNRESET,
                libc::EAGAIN,
                libc::EHOSTDOWN,
            ]
            .contains(&c)
        })
}

#[derive(Default, Debug)]
pub struct Info {
    pub serial: Option<String>,
    pub firmware: Option<String>,
    pub status: Option<Status>,
}

pub struct Printer {
    sock: std::fs::File,
    buf: Vec<u8>,
    pub enc: Encoder,
}

impl Printer {
    /// Connect, retrying while the previous link is still being torn down.
    pub fn connect(addr: &str, channel: u8) -> anyhow::Result<Printer> {
        let mut attempt = 0;
        loop {
            match Self::connect_once(addr, channel) {
                Err(e) if attempt < 5 && is_transient(&e) => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(1000));
                }
                r => return r,
            }
        }
    }

    fn connect_once(addr: &str, channel: u8) -> anyhow::Result<Printer> {
        let sa = SockaddrRc {
            family: AF_BLUETOOTH as libc::sa_family_t,
            bdaddr: parse_addr(addr)?,
            channel,
        };
        unsafe {
            let fd = libc::socket(
                AF_BLUETOOTH,
                libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                BTPROTO_RFCOMM,
            );
            if fd < 0 {
                bail!(
                    "cannot create a Bluetooth socket: {}",
                    std::io::Error::last_os_error()
                );
            }
            let fd = OwnedFd::from_raw_fd(fd);
            let r = libc::connect(
                fd.as_raw_fd(),
                &sa as *const SockaddrRc as *const libc::sockaddr,
                std::mem::size_of::<SockaddrRc>() as libc::socklen_t,
            );
            if r < 0 {
                let err = std::io::Error::last_os_error();
                return Err(anyhow::Error::new(err).context(format!(
                    "cannot connect to {addr} (is the printer on and in range?)"
                )));
            }
            Ok(Printer {
                sock: std::fs::File::from(fd),
                buf: Vec::new(),
                enc: Encoder::default(),
            })
        }
    }

    pub fn send(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.sock.write_all(data).context("Bluetooth write failed")
    }

    /// Read frames for up to `wait`.
    fn read_frames(&mut self, wait: Duration) -> anyhow::Result<Vec<Frame>> {
        let end = Instant::now() + wait;
        let mut frames = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let mut pfd = libc::pollfd {
                fd: self.sock.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let n = unsafe { libc::poll(&mut pfd, 1, left.as_millis().max(1) as libc::c_int) };
            if n <= 0 {
                break;
            }
            let got = self
                .sock
                .read(&mut chunk)
                .context("Bluetooth read failed")?;
            if got == 0 {
                break;
            }
            self.buf.extend_from_slice(&chunk[..got]);
            frames.extend(take_frames(&mut self.buf));
        }
        Ok(frames)
    }

    fn collect(&mut self, wait: Duration, info: &mut Info) -> anyhow::Result<()> {
        for f in self.read_frames(wait)? {
            if RSP_STATUS.contains(&f.cmd) {
                if let Some(st) = Status::parse(&f.payload) {
                    info.status = Some(st);
                }
            } else if f.cmd == RSP_FIRMWARE {
                info.firmware = Some(f.payload.iter().map(|b| format!("{b:02x}")).collect());
            } else if f.cmd == RSP_SERIAL {
                info.serial = Some(String::from_utf8_lossy(&f.payload).into_owned());
            }
        }
        Ok(())
    }

    /// Ask for serial number and firmware, like the phone app does after connecting.
    pub fn handshake(&mut self) -> anyhow::Result<Info> {
        let mut info = Info::default();
        let f = self.enc.get_serial();
        self.send(&f)?;
        std::thread::sleep(Duration::from_millis(150));
        let f = self.enc.get_firmware();
        self.send(&f)?;
        self.collect(Duration::from_millis(1500), &mut info)?;
        Ok(info)
    }

    pub fn status(&mut self, wait: Duration) -> anyhow::Result<Option<Status>> {
        let f = self.enc.get_status();
        self.send(&f)?;
        let mut info = Info::default();
        self.collect(wait, &mut info)?;
        Ok(info.status)
    }
}

/// Nearby devices whose name looks like an HP4 (uses bluetoothctl).
pub fn scan(seconds: u32) -> anyhow::Result<Vec<(String, String)>> {
    use std::process::Command;
    Command::new("bluetoothctl")
        .args(["--timeout", &seconds.to_string(), "scan", "on"])
        .output()
        .context("running bluetoothctl")?;
    let out = Command::new("bluetoothctl")
        .arg("devices")
        .output()
        .context("running bluetoothctl")?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("Device ")?;
            let (addr, name) = rest.split_once(' ')?;
            crate::protocol::BT_NAMES
                .iter()
                .any(|n| name.to_lowercase().contains(&n.to_lowercase()))
                .then(|| (addr.to_string(), name.to_string()))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn address_is_reversed() {
        assert_eq!(
            super::parse_addr("AA:BB:CC:DD:EE:01").unwrap(),
            [0x01, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA]
        );
        assert!(super::parse_addr("AA:BB").is_err());
    }
}

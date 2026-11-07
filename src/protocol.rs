//! HP4 wire protocol (see docs/PROTOCOL.md).
//!
//! Every message, in both directions, is one frame:
//! `0x64 | cmd | seq | len u16 LE | payload | 00 00 00 00 | 0x9B`.

use serde::Serialize;

pub const HEAD: u8 = 0x64;
pub const TAIL: u8 = 0x9B;

const CMD_RASTER: u8 = 0x00;
const CMD_FEED: u8 = 0x02;
const CMD_SPEC_FEED: u8 = 0x03;
const CMD_FEED_BACK: u8 = 0x04;
const CMD_AUTO_OFF: u8 = 0x08;
const CMD_DENSITY: u8 = 0x09;
const CMD_SPEED: u8 = 0x0A;
const CMD_GET_STATUS: u8 = 0x10;
const CMD_GET_FIRMWARE: u8 = 0x11;
const CMD_GET_SERIAL: u8 = 0x12;

pub const RSP_STATUS: [u8; 3] = [0x80, 0x81, 0xFF];
pub const RSP_FIRMWARE: u8 = 0xF1;
pub const RSP_SERIAL: u8 = 0xF2;

pub const DOTS_PER_MM: f32 = 8.0;
pub const HEAD_DOTS: usize = 96;
pub const ROW_BYTES: usize = HEAD_DOTS / 8;
const ROWS_PER_PACKET: usize = 24;

pub const SPEED: u8 = 25;
const TAPE_FIRST_FEED_BACK: u16 = 24;
const TAPE_FEED: u16 = 40;
const TAPE_LAST_FEED: u16 = 24;
const GAP_SEARCH_DISTANCE: u16 = 800;

/// Bluetooth names the printer advertises.
pub const BT_NAMES: [&str; 4] = ["HP4", "DS-702", "KD1", "LP4"];

pub fn density_level(name: &str) -> Option<u8> {
    match name {
        "light" => Some(5),
        "normal" => Some(9),
        "dark" => Some(13),
        n => n.parse().ok().filter(|v| *v < 16),
    }
}

/// Auto power-off choices; the printer counts in units of 10 minutes, 0 = never.
pub fn auto_off_units(name: &str) -> Option<u8> {
    match name {
        "never" => Some(0),
        "10m" => Some(1),
        "30m" => Some(3),
        "1h" => Some(6),
        "3h" => Some(18),
        "5h" => Some(30),
        "10h" => Some(60),
        n => {
            let mins: u32 = n.strip_suffix('m')?.parse().ok()?;
            (mins.is_multiple_of(10) && mins / 10 < 256).then_some((mins / 10) as u8)
        }
    }
}

/// Builds frames with the printer's 6-bit sequence counter.
#[derive(Default)]
pub struct Encoder {
    seq: u8,
}

impl Encoder {
    pub fn frame(&mut self, cmd: u8, payload: &[u8]) -> Vec<u8> {
        self.seq = (self.seq + 1) & 0x3F;
        let mut f = Vec::with_capacity(payload.len() + 10);
        f.extend([HEAD, cmd, self.seq]);
        f.extend((payload.len() as u16).to_le_bytes());
        f.extend(payload);
        f.extend([0, 0, 0, 0, TAIL]);
        f
    }

    fn u16_cmd(&mut self, cmd: u8, v: u16) -> Vec<u8> {
        if v == 0 {
            return Vec::new();
        }
        self.frame(cmd, &v.to_le_bytes())
    }

    pub fn feed(&mut self, dots: u16) -> Vec<u8> {
        self.u16_cmd(CMD_FEED, dots)
    }

    pub fn auto_off(&mut self, units: u8) -> Vec<u8> {
        self.frame(CMD_AUTO_OFF, &[units])
    }

    pub fn get_status(&mut self) -> Vec<u8> {
        self.frame(CMD_GET_STATUS, &[])
    }

    pub fn get_serial(&mut self) -> Vec<u8> {
        self.frame(CMD_GET_SERIAL, &[])
    }

    pub fn get_firmware(&mut self) -> Vec<u8> {
        self.frame(CMD_GET_FIRMWARE, &[])
    }

    fn spec_feed(&mut self, mode: u8) -> Vec<u8> {
        let [lo, hi] = GAP_SEARCH_DISTANCE.to_le_bytes();
        self.frame(CMD_SPEC_FEED, &[mode, lo, hi])
    }

    fn raster(&mut self, rows: &[u8]) -> Vec<u8> {
        rows.chunks(ROWS_PER_PACKET * ROW_BYTES)
            .flat_map(|chunk| self.frame(CMD_RASTER, chunk))
            .collect()
    }

    /// Continuous tape: pull back, print, push the label out past the cutter.
    pub fn tape_job(&mut self, rows: &[u8], density: u8, first: bool, last: bool) -> Vec<u8> {
        let mut out = self.frame(CMD_SPEED, &[SPEED]);
        out.extend(self.frame(CMD_DENSITY, &[density]));
        if first {
            out.extend(self.u16_cmd(CMD_FEED_BACK, TAPE_FIRST_FEED_BACK));
        }
        out.extend(self.raster(rows));
        out.extend(self.feed(TAPE_FEED));
        if last {
            out.extend(self.feed(TAPE_LAST_FEED));
        }
        out
    }

    /// Die-cut labels: the printer finds the gaps itself.
    pub fn gap_job(
        &mut self,
        rows: &[u8],
        density: u8,
        lead_feed: u16,
        first: bool,
        last: bool,
    ) -> Vec<u8> {
        let mut out = self.frame(CMD_SPEED, &[SPEED]);
        out.extend(self.frame(CMD_DENSITY, &[density]));
        if first {
            out.extend(self.spec_feed(2));
        }
        out.extend(self.feed(lead_feed));
        out.extend(self.raster(rows));
        out.extend(self.spec_feed(if last { 1 } else { 0 }));
        out
    }
}

/// A parsed incoming frame.
pub struct Frame {
    pub cmd: u8,
    pub payload: Vec<u8>,
}

/// Take complete frames off the front of `buf`, leaving any partial frame in it.
pub fn take_frames(buf: &mut Vec<u8>) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut i = 0;
    loop {
        let Some(start) = buf[i..].iter().position(|&b| b == HEAD).map(|p| p + i) else {
            buf.clear();
            break;
        };
        if buf.len() - start < 10 {
            buf.drain(..start);
            break;
        }
        let len = u16::from_le_bytes([buf[start + 3], buf[start + 4]]) as usize;
        let end = start + len + 10;
        if end > buf.len() {
            buf.drain(..start);
            break;
        }
        if buf[end - 1] != TAIL {
            i = start + 1;
            continue;
        }
        frames.push(Frame {
            cmd: buf[start + 1],
            payload: buf[start + 5..start + 5 + len].to_vec(),
        });
        i = end;
    }
    frames
}

const STATUS_BITS: [&str; 16] = [
    "on",
    "bt_connected",
    "charging",
    "printing",
    "suspended",
    "usb_busy",
    "bt_busy",
    "motor_out_of_sync",
    "cover_open",
    "low_battery",
    "overheating",
    "paper_out",
    "print_allowed",
    "valid_paper",
    "upgrading",
    "paper_loaded",
];

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub flags: std::collections::BTreeMap<&'static str, bool>,
    pub battery_pct: Option<u8>,
    pub auto_off_min: Option<u32>,
    pub density: Option<u8>,
    pub speed: Option<u8>,
}

impl Status {
    pub fn parse(p: &[u8]) -> Option<Status> {
        if p.len() < 7 {
            return None;
        }
        let bits = u16::from_le_bytes([p[2], p[3]]);
        let flags = STATUS_BITS
            .iter()
            .enumerate()
            .map(|(i, n)| (*n, bits >> i & 1 == 1))
            .collect();
        let battery_pct = match p.get(7) {
            Some(&b) => Some(b),
            None => {
                let volts =
                    i16::from_le_bytes([p[0], p[1]]) as f32 * 3340.0 * 280.0 / 409600.0 / 1000.0;
                Some(battery_from_voltage(volts, bits >> 2 & 1 == 1))
            }
        };
        Some(Status {
            flags,
            battery_pct,
            auto_off_min: Some(p[5] as u32 * 10),
            density: Some(p[4]),
            speed: Some(p[6]),
        })
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }

    pub fn problems(&self) -> Vec<&'static str> {
        ["cover_open", "paper_out", "low_battery", "overheating"]
            .into_iter()
            .filter(|n| self.flag(n))
            .collect()
    }
}

fn battery_from_voltage(volts: f32, charging: bool) -> u8 {
    const IDLE: [(f32, u8); 12] = [
        (8.21, 100),
        (8.155, 90),
        (8.02, 80),
        (7.885, 70),
        (7.695, 60),
        (7.48, 50),
        (7.32, 40),
        (7.175, 30),
        (7.085, 20),
        (6.98, 10),
        (6.84, 5),
        (6.665, 2),
    ];
    const CHARGING: [(f32, u8); 12] = [
        (8.38, 100),
        (8.325, 90),
        (8.2, 80),
        (8.065, 70),
        (7.875, 60),
        (7.53, 50),
        (7.415, 40),
        (7.33, 30),
        (7.24, 20),
        (7.115, 10),
        (7.0, 5),
        (6.88, 2),
    ];
    let table = if charging { &CHARGING } else { &IDLE };
    table
        .iter()
        .find(|(t, _)| volts >= *t)
        .map_or(1, |(_, p)| *p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_layout() {
        let f = Encoder::default().frame(CMD_SPEED, &[25]);
        assert_eq!(f, [0x64, 0x0A, 0x01, 0x01, 0x00, 0x19, 0, 0, 0, 0, 0x9B]);
    }

    #[test]
    fn splits_frames_and_keeps_partial() {
        let f = Encoder::default().frame(CMD_SPEED, &[25]);
        let mut buf = [&[0x00, 0x01][..], &f, &f[..4]].concat();
        let frames = take_frames(&mut buf);
        assert_eq!(frames.len(), 1);
        assert_eq!(
            (frames[0].cmd, frames[0].payload.as_slice()),
            (0x0A, &[25u8][..])
        );
        assert_eq!(buf, &f[..4]);
    }

    #[test]
    fn parses_status_seen_on_hardware() {
        // 0xFF report from a real HP4: on, bt connected, print allowed, 30 min, battery 100
        let st = Status::parse(&[0xac, 0x10, 0x03, 0x10, 0x09, 0x03, 0x32, 0x64]).unwrap();
        assert!(st.flag("on") && st.flag("print_allowed") && !st.flag("paper_out"));
        assert_eq!(
            (st.battery_pct, st.auto_off_min, st.density),
            (Some(100), Some(30), Some(9))
        );
        assert!(st.problems().is_empty());
    }

    #[test]
    fn auto_off_names() {
        assert_eq!(auto_off_units("never"), Some(0));
        assert_eq!(auto_off_units("1h"), Some(6));
        assert_eq!(auto_off_units("120m"), Some(12));
        assert_eq!(auto_off_units("15m"), None);
    }
}

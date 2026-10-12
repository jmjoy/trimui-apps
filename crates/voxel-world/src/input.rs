use anyhow::{Context, Result, ensure};
use glam::Vec2;
use std::{
    fs::File,
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};
pub const A: usize = 1;
pub const B: usize = 0;
pub const X: usize = 3;
pub const Y: usize = 2;
pub const L: usize = 4;
pub const R: usize = 5;
pub const SELECT: usize = 6;
pub const START: usize = 7;
const CODES: [u16; 11] = [304, 305, 307, 308, 310, 311, 314, 315, 316, 317, 318];
#[derive(Default, Clone)]
pub struct Frame {
    pub pressed: [bool; 11],
    pub held: [bool; 11],
    pub movement: Vec2,
    pub look: Vec2,
    pub dpad: Vec2,
    pub dig: bool,
    pub place: bool,
}
pub struct Gamepad {
    file: File,
    buttons: [bool; 512],
    axes: [i16; 64],
    axis_map: [u8; 64],
    button_map: [u16; 512],
    axis_count: usize,
    button_count: usize,
}
fn stick(v: i16) -> f32 {
    let x = (v as f32 / 32767.0).clamp(-1.0, 1.0);
    if x.abs() < 0.18 {
        0.0
    } else {
        x.signum() * (x.abs() - 0.18) / 0.82
    }
}
fn trigger(value: i16, held: bool) -> bool {
    let value = (value as f32 + 32767.0) / 65534.0;
    value > if held { 0.45 } else { 0.55 }
}
impl Gamepad {
    pub fn open(path: &str) -> Result<Self> {
        let file = File::options()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .with_context(|| format!("Opening controller {path}"))?;
        let mut pad = Self {
            file,
            buttons: [false; 512],
            axes: [0; 64],
            axis_map: [0; 64],
            button_map: [0; 512],
            axis_count: 0,
            button_count: 0,
        };
        let mut axes = 0u8;
        let mut buttons = 0u8;
        // Linux joystick read-only ioctl requests; buffers match the UAPI sizes.
        unsafe {
            ensure!(
                libc::ioctl(pad.file.as_raw_fd(), 0x80016a11u64, &mut axes) >= 0,
                "Reading joystick axes: {}",
                io::Error::last_os_error()
            );
            ensure!(
                libc::ioctl(pad.file.as_raw_fd(), 0x80016a12u64, &mut buttons) >= 0,
                "Reading joystick buttons: {}",
                io::Error::last_os_error()
            );
            ensure!(
                libc::ioctl(
                    pad.file.as_raw_fd(),
                    0x80406a32u64,
                    pad.axis_map.as_mut_ptr()
                ) >= 0,
                "Reading joystick axis map: {}",
                io::Error::last_os_error()
            );
            ensure!(
                libc::ioctl(
                    pad.file.as_raw_fd(),
                    0x84006a34u64,
                    pad.button_map.as_mut_ptr()
                ) >= 0,
                "Reading joystick button map: {}",
                io::Error::last_os_error()
            );
        }
        pad.axis_count = usize::from(axes).min(64);
        pad.button_count = usize::from(buttons).min(512);
        for (i, code) in pad.axis_map[..pad.axis_count].iter().enumerate() {
            if *code == 2 || *code == 5 {
                pad.axes[i] = -32767;
            }
        }
        log::info!(
            "Controller: {path}; axes {:?}; buttons {:?}",
            &pad.axis_map[..pad.axis_count],
            &pad.button_map[..pad.button_count]
        );
        Ok(pad)
    }
    fn axis(&self, code: u8) -> i16 {
        self.axis_map[..self.axis_count]
            .iter()
            .position(|&c| c == code)
            .map_or(if code == 2 || code == 5 { -32767 } else { 0 }, |i| {
                self.axes[i]
            })
    }
    pub fn update(&mut self, previous: &Frame) -> Result<Frame> {
        let mut frame = Frame::default();
        loop {
            let mut bytes = [0; 8];
            match self.file.read(&mut bytes) {
                Ok(8) => {
                    let value = i16::from_ne_bytes([bytes[4], bytes[5]]);
                    let initial = bytes[6] & 0x80 != 0;
                    let kind = bytes[6] & 0x7f;
                    let i = bytes[7] as usize;
                    if kind == 1 && i < self.button_count {
                        if !initial
                            && value != 0
                            && !self.buttons[i]
                            && let Some(c) = CODES.iter().position(|&c| c == self.button_map[i])
                        {
                            frame.pressed[c] = true;
                        }
                        self.buttons[i] = value != 0;
                    } else if kind == 2 && i < self.axis_count {
                        self.axes[i] = value;
                    }
                }
                Ok(0) => anyhow::bail!("Controller disconnected"),
                Ok(_) => anyhow::bail!("Incomplete joystick event"),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        for (c, code) in CODES.iter().enumerate() {
            frame.held[c] = self.button_map[..self.button_count]
                .iter()
                .position(|b| b == code)
                .is_some_and(|i| self.buttons[i]);
        }
        frame.movement = Vec2::new(stick(self.axis(0)), stick(self.axis(1)));
        frame.look = Vec2::new(stick(self.axis(3)), stick(self.axis(4)));
        frame.dpad = Vec2::new(self.axis(16).signum() as f32, self.axis(17).signum() as f32);
        frame.dig = trigger(self.axis(5), previous.dig);
        frame.place = trigger(self.axis(2), previous.place);
        Ok(frame)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drift_and_trigger_hysteresis() {
        assert_eq!(stick(4199), 0.0);
        assert_eq!(stick(-2708), 0.0);
        assert_eq!(stick(i16::MIN), -1.0);
        assert!(!trigger(-32767, false));
        assert!(trigger(32767, false));
        assert!(trigger(0, true));
        assert!(!trigger(0, false));
    }
}

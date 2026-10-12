use anyhow::{Context, Result};
use std::{
    fs::File,
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
};

// The virtual controller uses Xbox button indices, while the handheld's
// printed labels follow Nintendo: A is on the right, B is at the bottom.
pub const BUTTON_A: usize = 1;
pub const BUTTON_B: usize = 0;
pub const BUTTON_X: usize = 3;
pub const BUTTON_Y: usize = 2;
pub const BUTTON_START: usize = 7;

pub struct Gamepad {
    file: File,
    pub buttons: [bool; 32],
    axes: [i16; 8],
}

impl Gamepad {
    pub fn open(path: &str) -> Result<Self> {
        let file = File::options()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .with_context(|| format!("Opening controller {path}"))?;
        log::info!(
            "Controller: {path}; A = confirm, B = cancel, X = undo, Y = legal target, START = menu, SELECT + START = exit"
        );
        Ok(Self {
            file,
            buttons: [false; 32],
            axes: [0; 8],
        })
    }

    pub fn update(&mut self) -> Result<[bool; 32]> {
        let mut pressed = [false; 32];
        loop {
            // Linux joystick API has a fixed 8-byte native-endian js_event.
            let mut bytes = [0u8; 8];
            match self.file.read(&mut bytes) {
                Ok(8) => {
                    let value = i16::from_ne_bytes([bytes[4], bytes[5]]);
                    let initial = bytes[6] & 0x80 != 0;
                    let kind = bytes[6] & !0x80;
                    let number = bytes[7] as usize;
                    if kind == 1 && number < self.buttons.len() {
                        pressed[number] |= !initial && value != 0 && !self.buttons[number];
                        self.buttons[number] = value != 0;
                    } else if kind == 2 && number < self.axes.len() {
                        self.axes[number] = value;
                    }
                }
                Ok(0) => anyhow::bail!("Controller disconnected"),
                Ok(_) => anyhow::bail!("Incomplete Linux joystick event"),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(pressed)
    }

    pub fn direction(&self) -> [f32; 2] {
        let normalize = |v: i16| {
            let f = v as f32 / 32767.0;
            if f.abs() < 0.15 {
                0.0
            } else {
                f.clamp(-1.0, 1.0)
            }
        };
        [0, 1].map(|axis| {
            let stick = normalize(self.axes[axis]);
            let dpad = normalize(self.axes[axis + 6]);
            if dpad != 0.0 { dpad } else { stick }
        })
    }

    pub fn exit_requested(&self) -> bool {
        self.buttons[6] && self.buttons[7]
    }
}

/// One square immediately, then a short delay followed by held-key repeat.
#[derive(Default)]
pub struct DirectionRepeat {
    held: [i8; 2],
    next: std::time::Duration,
}

impl DirectionRepeat {
    pub fn update(&mut self, direction: [f32; 2], now: std::time::Duration) -> Option<[i8; 2]> {
        let mut digital = direction.map(|v| {
            if v > 0.5 {
                1
            } else if v < -0.5 {
                -1
            } else {
                0
            }
        });
        // A diagonal stick position should move along one board axis at a time.
        if digital[0] != 0 && digital[1] != 0 {
            if direction[0].abs() > direction[1].abs() {
                digital[1] = 0;
            } else {
                digital[0] = 0;
            }
        }
        if digital != self.held {
            self.held = digital;
            self.next = now + std::time::Duration::from_millis(300);
            return (digital != [0, 0]).then_some(digital);
        }
        if digital != [0, 0] && now >= self.next {
            self.next = now + std::time::Duration::from_millis(105);
            return Some(digital);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn held_direction_repeats_and_release_stops_it() {
        let mut repeat = DirectionRepeat::default();
        assert_eq!(repeat.update([1.0, 0.0], Duration::ZERO), Some([1, 0]));
        assert_eq!(repeat.update([1.0, 0.0], Duration::from_millis(299)), None);
        assert_eq!(
            repeat.update([1.0, 0.0], Duration::from_millis(300)),
            Some([1, 0])
        );
        assert_eq!(repeat.update([0.0, 0.0], Duration::from_millis(301)), None);
        assert_eq!(repeat.update([0.0, 0.0], Duration::from_secs(1)), None);
        assert_eq!(
            repeat.update([0.0, -1.0], Duration::from_secs(2)),
            Some([0, -1])
        );
    }
}

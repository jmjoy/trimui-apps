//! Linux joystick (`/dev/input/js0`) reader producing per-frame button sets.
//!
//! The D-pad hat axes and the analog L2/R2 triggers are folded into virtual
//! buttons so every physical control can be bound to an action.
use anyhow::{Context, Result, ensure};
use glam::Vec2;
use std::{
    fs::File,
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Button {
    A,
    B,
    X,
    Y,
    L,
    R,
    L2,
    R2,
    Up,
    Down,
    Left,
    Right,
    Select,
    Start,
    Menu,
}

impl Button {
    pub const ALL: [Button; 15] = [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::L,
        Button::R,
        Button::L2,
        Button::R2,
        Button::Up,
        Button::Down,
        Button::Left,
        Button::Right,
        Button::Select,
        Button::Start,
        Button::Menu,
    ];
    /// Stable identifier used in the configuration file.
    pub fn key(self) -> &'static str {
        match self {
            Button::A => "a",
            Button::B => "b",
            Button::X => "x",
            Button::Y => "y",
            Button::L => "l",
            Button::R => "r",
            Button::L2 => "l2",
            Button::R2 => "r2",
            Button::Up => "up",
            Button::Down => "down",
            Button::Left => "left",
            Button::Right => "right",
            Button::Select => "select",
            Button::Start => "start",
            Button::Menu => "menu",
        }
    }
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.key() == key)
    }
    /// Label printed on the handheld.
    pub fn label(self) -> &'static str {
        match self {
            Button::A => "A",
            Button::B => "B",
            Button::X => "X",
            Button::Y => "Y",
            Button::L => "L",
            Button::R => "R",
            Button::L2 => "L2",
            Button::R2 => "R2",
            Button::Up => "↑",
            Button::Down => "↓",
            Button::Left => "←",
            Button::Right => "→",
            Button::Select => "SELECT",
            Button::Start => "START",
            Button::Menu => "MENU",
        }
    }
    /// evdev key code reported through the joystick button map.
    fn code(self) -> Option<u16> {
        Some(match self {
            Button::B => 304,
            Button::A => 305,
            Button::Y => 307,
            Button::X => 308,
            Button::L => 310,
            Button::R => 311,
            Button::Select => 314,
            Button::Start => 315,
            Button::Menu => 316,
            _ => return None,
        })
    }
    fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// Small set of buttons.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Buttons(u16);
impl Buttons {
    pub fn contains(self, b: Button) -> bool {
        self.0 & b.bit() != 0
    }
    pub fn insert(&mut self, b: Button) {
        self.0 |= b.bit();
    }
    pub fn remove(&mut self, b: Button) {
        self.0 &= !b.bit();
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn iter(self) -> impl Iterator<Item = Button> {
        Button::ALL.into_iter().filter(move |b| self.contains(*b))
    }
    pub fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl FromIterator<Button> for Buttons {
    fn from_iter<T: IntoIterator<Item = Button>>(iter: T) -> Self {
        let mut set = Self::default();
        for b in iter {
            set.insert(b);
        }
        set
    }
}

#[derive(Default, Clone)]
pub struct Frame {
    pub held: Buttons,
    /// Buttons that went down since the previous frame, including taps that
    /// were pressed and released between two polls.
    pub pressed: Buttons,
    pub movement: Vec2,
    pub look: Vec2,
}
impl Frame {
    pub fn held(&self, b: Button) -> bool {
        self.held.contains(b)
    }
    pub fn pressed(&self, b: Button) -> bool {
        self.pressed.contains(b)
    }
    /// D-pad direction, y pointing down like the stick axes.
    pub fn dpad(&self) -> Vec2 {
        let axis =
            |neg, pos| f32::from(u8::from(self.held(pos))) - f32::from(u8::from(self.held(neg)));
        Vec2::new(
            axis(Button::Left, Button::Right),
            axis(Button::Up, Button::Down),
        )
    }
    #[cfg(test)]
    pub fn press(buttons: &[Button]) -> Self {
        let set: Buttons = buttons.iter().copied().collect();
        Self {
            held: set,
            pressed: set,
            ..Default::default()
        }
    }
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
    fn button(&self, code: u16) -> Option<Button> {
        Button::ALL.into_iter().find(|b| b.code() == Some(code))
    }
    pub fn update(&mut self, previous: &Frame) -> Result<Frame> {
        let mut taps = Buttons::default();
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
                            && let Some(b) = self.button(self.button_map[i])
                        {
                            taps.insert(b);
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
        let mut held = Buttons::default();
        for (i, &code) in self.button_map[..self.button_count].iter().enumerate() {
            if self.buttons[i]
                && let Some(b) = self.button(code)
            {
                held.insert(b);
            }
        }
        for (b, code) in [(Button::L2, 2), (Button::R2, 5)] {
            if trigger(self.axis(code), previous.held(b)) {
                held.insert(b);
            }
        }
        let (hx, hy) = (self.axis(16), self.axis(17));
        for (b, on) in [
            (Button::Left, hx < 0),
            (Button::Right, hx > 0),
            (Button::Up, hy < 0),
            (Button::Down, hy > 0),
        ] {
            if on {
                held.insert(b);
            }
        }
        Ok(Frame {
            held,
            pressed: held.difference(previous.held).union(taps),
            movement: Vec2::new(stick(self.axis(0)), stick(self.axis(1))),
            look: Vec2::new(stick(self.axis(3)), stick(self.axis(4))),
        })
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
    #[test]
    fn button_keys_and_sets() {
        for b in Button::ALL {
            assert_eq!(Button::from_key(b.key()), Some(b));
        }
        assert_eq!(Button::from_key("l3"), None);
        let set: Buttons = [Button::A, Button::Menu].into_iter().collect();
        assert!(set.contains(Button::Menu) && !set.contains(Button::B));
        assert_eq!(set.iter().collect::<Vec<_>>(), [Button::A, Button::Menu]);
        let frame = Frame {
            held: [Button::Up, Button::Right].into_iter().collect(),
            ..Default::default()
        };
        assert_eq!(frame.dpad(), Vec2::new(1.0, -1.0));
    }
}

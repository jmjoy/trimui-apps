//! Game actions, their rebindable buttons and timing gestures.
//!
//! Defaults follow the Bedrock Edition controller layout, mapped by the
//! handheld's physical positions (A right, B bottom, X top, Y left).
//! START (pause), SELECT + START (save and exit), menu A/B and both sticks
//! are fixed and never stored here.
use super::device::{Button, Frame};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Action {
    Jump,
    Sneak,
    Inventory,
    PickBlock,
    Attack,
    Use,
    HotbarPrev,
    HotbarNext,
    ToggleSneak,
    ToggleSprint,
}

impl Action {
    pub const ALL: [Action; 10] = [
        Action::Jump,
        Action::Sneak,
        Action::Attack,
        Action::Use,
        Action::HotbarPrev,
        Action::HotbarNext,
        Action::Inventory,
        Action::PickBlock,
        Action::ToggleSneak,
        Action::ToggleSprint,
    ];
    pub const COUNT: usize = Self::ALL.len();
    pub fn key(self) -> &'static str {
        match self {
            Action::Jump => "jump",
            Action::Sneak => "sneak",
            Action::Inventory => "inventory",
            Action::PickBlock => "pick_block",
            Action::Attack => "attack",
            Action::Use => "use",
            Action::HotbarPrev => "hotbar_prev",
            Action::HotbarNext => "hotbar_next",
            Action::ToggleSneak => "toggle_sneak",
            Action::ToggleSprint => "toggle_sprint",
        }
    }
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == key)
    }
    pub fn label(self) -> &'static str {
        match self {
            Action::Jump => "跳跃 / 上升（双击切换飞行）",
            Action::Sneak => "潜行 / 下降（按住）",
            Action::Inventory => "方块库",
            Action::PickBlock => "选取所看方块",
            Action::Attack => "攻击 / 挖掘",
            Action::Use => "使用 / 放置",
            Action::HotbarPrev => "快捷栏上一格",
            Action::HotbarNext => "快捷栏下一格",
            Action::ToggleSneak => "潜行开关",
            Action::ToggleSprint => "疾跑开关",
        }
    }
    /// Short label for HUD hints.
    pub fn hint(self) -> &'static str {
        match self {
            Action::Jump => "跳跃",
            Action::Sneak => "潜行",
            Action::Inventory => "方块库",
            Action::PickBlock => "选取",
            Action::Attack => "挖掘",
            Action::Use => "放置",
            Action::HotbarPrev | Action::HotbarNext => "切换",
            Action::ToggleSneak => "潜行开关",
            Action::ToggleSprint => "疾跑开关",
        }
    }
    pub fn default_button(self) -> Button {
        match self {
            Action::Jump => Button::B,
            Action::Sneak => Button::A,
            Action::Inventory => Button::X,
            Action::PickBlock => Button::Y,
            Action::Attack => Button::R2,
            Action::Use => Button::L2,
            Action::HotbarPrev => Button::L,
            Action::HotbarNext => Button::R,
            Action::ToggleSneak => Button::Left,
            Action::ToggleSprint => Button::Right,
        }
    }
    fn index(self) -> usize {
        Self::ALL.iter().position(|a| *a == self).unwrap()
    }
}

/// START is reserved for pausing (and SELECT + START for exit).
pub fn bindable(b: Button) -> bool {
    b != Button::Start
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(
    from = "BTreeMap<String, Option<String>>",
    into = "BTreeMap<String, Option<String>>"
)]
pub struct Bindings {
    map: [Option<Button>; Action::COUNT],
}

impl Default for Bindings {
    fn default() -> Self {
        let mut map = [None; Action::COUNT];
        for a in Action::ALL {
            map[a.index()] = Some(a.default_button());
        }
        Self { map }
    }
}

impl Bindings {
    pub fn get(&self, a: Action) -> Option<Button> {
        self.map[a.index()]
    }
    pub fn held(&self, frame: &Frame, a: Action) -> bool {
        self.get(a).is_some_and(|b| frame.held(b))
    }
    pub fn pressed(&self, frame: &Frame, a: Action) -> bool {
        self.get(a).is_some_and(|b| frame.pressed(b))
    }
    pub fn action_for(&self, b: Button) -> Option<Action> {
        Action::ALL.into_iter().find(|a| self.get(*a) == Some(b))
    }
    /// Binds `b` to `a`. An action already using `b` receives `a`'s previous
    /// button, so no control is ever lost; that action is returned.
    pub fn bind(&mut self, a: Action, b: Button) -> Option<Action> {
        if !bindable(b) || self.get(a) == Some(b) {
            return None;
        }
        let previous = self.get(a);
        let other = self.action_for(b);
        if let Some(other) = other {
            self.map[other.index()] = previous;
        }
        self.map[a.index()] = Some(b);
        other
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn label(&self, a: Action) -> &'static str {
        self.get(a).map_or("未绑定", Button::label)
    }
}

impl From<BTreeMap<String, Option<String>>> for Bindings {
    /// Unknown names are ignored; missing actions get their default button
    /// unless another action already uses it; duplicates keep the first.
    fn from(stored: BTreeMap<String, Option<String>>) -> Self {
        let mut map = [None; Action::COUNT];
        let mut used = Vec::new();
        let mut missing = Vec::new();
        for a in Action::ALL {
            match stored.get(a.key()) {
                Some(Some(key)) => match Button::from_key(key) {
                    Some(b) if bindable(b) && !used.contains(&b) => {
                        map[a.index()] = Some(b);
                        used.push(b);
                    }
                    _ => log::warn!("Ignoring invalid binding {}={key}", a.key()),
                },
                Some(None) => {}
                None => missing.push(a),
            }
        }
        for a in missing {
            let b = a.default_button();
            if used.contains(&b) {
                log::warn!("New action {} left unbound: {} is in use", a.key(), b.key());
            } else {
                map[a.index()] = Some(b);
                used.push(b);
            }
        }
        Self { map }
    }
}

impl From<Bindings> for BTreeMap<String, Option<String>> {
    fn from(bindings: Bindings) -> Self {
        Action::ALL
            .into_iter()
            .map(|a| {
                (
                    a.key().to_owned(),
                    bindings.get(a).map(|b| b.key().to_owned()),
                )
            })
            .collect()
    }
}

/// Double-tap jump and double-push forward detection.
#[derive(Default)]
pub struct Gestures {
    last_jump: Option<f64>,
    last_push: Option<f64>,
    forward: bool,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct GestureEvents {
    pub double_jump: bool,
    pub double_push: bool,
}

impl Gestures {
    pub const WINDOW: f64 = 0.3;
    /// `forward` is the stick's forward deflection in 0..=1.
    pub fn update(&mut self, jump_pressed: bool, forward: f32, now: f64) -> GestureEvents {
        let mut events = GestureEvents::default();
        if jump_pressed {
            if self.last_jump.is_some_and(|t| now - t <= Self::WINDOW) {
                events.double_jump = true;
                self.last_jump = None;
            } else {
                self.last_jump = Some(now);
            }
        }
        // Hysteresis keeps stick noise from producing extra pushes.
        if !self.forward && forward > 0.85 {
            self.forward = true;
            if self.last_push.is_some_and(|t| now - t <= Self::WINDOW) {
                events.double_push = true;
                self.last_push = None;
            } else {
                self.last_push = Some(now);
            }
        } else if self.forward && forward < 0.35 {
            self.forward = false;
        }
        events
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_bedrock_layout_by_position() {
        let b = Bindings::default();
        let expect = [
            (Action::Jump, Button::B),
            (Action::Sneak, Button::A),
            (Action::Inventory, Button::X),
            (Action::PickBlock, Button::Y),
            (Action::Attack, Button::R2),
            (Action::Use, Button::L2),
            (Action::HotbarPrev, Button::L),
            (Action::HotbarNext, Button::R),
            (Action::ToggleSneak, Button::Left),
            (Action::ToggleSprint, Button::Right),
        ];
        for (a, button) in expect {
            assert_eq!(b.get(a), Some(button), "{a:?}");
        }
        assert!(b.action_for(Button::Start).is_none());
        assert!(b.action_for(Button::Select).is_none());
        let mut keys: Vec<_> = Action::ALL.iter().map(|a| a.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), Action::COUNT);
    }

    #[test]
    fn rebinding_swaps_conflicts_and_resets() {
        let mut b = Bindings::default();
        assert_eq!(b.bind(Action::Jump, Button::R), Some(Action::HotbarNext));
        assert_eq!(b.get(Action::Jump), Some(Button::R));
        assert_eq!(b.get(Action::HotbarNext), Some(Button::B));
        assert_eq!(b.bind(Action::Jump, Button::Menu), None);
        assert_eq!(b.get(Action::Jump), Some(Button::Menu));
        assert_eq!(b.bind(Action::Jump, Button::Start), None);
        assert_eq!(b.get(Action::Jump), Some(Button::Menu));
        b.reset();
        assert_eq!(b, Bindings::default());
    }

    #[test]
    fn serialization_round_trips_and_repairs() {
        let mut b = Bindings::default();
        b.bind(Action::Use, Button::Select);
        let json = serde_json::to_string(&b).unwrap();
        assert!(json.contains("\"use\":\"select\""), "{json}");
        let back: Bindings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
        // Unknown actions and buttons are dropped, duplicates keep the first
        // action, and a missing action takes its default when free.
        let repaired: Bindings = serde_json::from_str(
            r#"{"jump":"r2","attack":"r2","use":"start","sneak":null,"future":"a","pick_block":"l3"}"#,
        )
        .unwrap();
        assert_eq!(repaired.get(Action::Jump), Some(Button::R2));
        assert_eq!(repaired.get(Action::Attack), None);
        assert_eq!(repaired.get(Action::Use), None);
        assert_eq!(repaired.get(Action::Sneak), None);
        assert_eq!(repaired.get(Action::PickBlock), None);
        assert_eq!(repaired.get(Action::Inventory), Some(Button::X));
    }

    #[test]
    fn gesture_windows() {
        let mut g = Gestures::default();
        assert_eq!(g.update(true, 0.0, 0.0), GestureEvents::default());
        assert!(g.update(true, 0.0, 0.25).double_jump);
        // A third tap starts a new pair instead of toggling again.
        assert!(!g.update(true, 0.0, 0.4).double_jump);
        assert!(!g.update(true, 0.0, 0.8).double_jump);
        g.reset();
        assert!(!g.update(false, 1.0, 1.0).double_push);
        assert!(
            !g.update(false, 0.9, 1.05).double_push,
            "held, not a new push"
        );
        g.update(false, 0.0, 1.1);
        assert!(g.update(false, 1.0, 1.2).double_push);
        g.update(false, 0.0, 2.0);
        g.update(false, 1.0, 2.1);
        g.update(false, 0.0, 2.2);
        assert!(!g.update(false, 1.0, 2.6).double_push, "outside the window");
    }
}

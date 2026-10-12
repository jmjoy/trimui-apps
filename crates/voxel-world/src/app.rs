use crate::{
    audio::{Audio, Sound},
    input::{self, Frame},
    player::Player,
    save::{SaveWorker, Settings, Snapshot},
    world::{NAMES, World, inside},
};
use anyhow::Result;
use glam::{Vec2, Vec3};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Copy, PartialEq)]
pub enum Screen {
    Title,
    Playing,
    Pause,
    Palette,
    ConfirmNew,
}
pub struct App {
    pub world: World,
    pub player: Player,
    pub settings: Settings,
    pub screen: Screen,
    pub cursor: usize,
    pub status: String,
    pub epoch: u64,
    pub audio: Audio,
    pub save: SaveWorker,
    pub save_allowed: bool,
    pub has_save: bool,
    clock: f64,
    status_until: f64,
    autosave: f64,
    accumulator: f32,
    edit_after: f64,
    nav_axis: Vec2,
    nav_after: f64,
    return_screen: Screen,
    armed: bool,
}
fn new_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
impl App {
    pub fn new(dir: PathBuf, seed: Option<u64>) -> Self {
        Self::with_audio(dir, seed, Audio::new())
    }
    fn with_audio(dir: PathBuf, seed: Option<u64>, audio: Audio) -> Self {
        let loaded = Snapshot::load(&dir);
        let has_save = dir.join("world.dat").exists();
        let (world, player, settings, error) = match loaded {
            Ok(Some(s)) => (s.world, s.player, s.settings, None),
            other => {
                let w = World::generate(seed.unwrap_or_else(new_seed));
                let p = Player::new(&w);
                let error = other
                    .err()
                    .map(|e| format!("存档读取失败，原文件已保留：{e:#}"));
                (w, p, Settings::default(), error)
            }
        };
        log::info!(
            "World ready: seed={}, spawn={:?}, digest={:016x}",
            world.seed,
            world.spawn(),
            crate::save::digest(&world.blocks)
        );
        let mut app = Self {
            world,
            player,
            settings,
            screen: Screen::Title,
            cursor: 0,
            status: error.clone().unwrap_or_default(),
            epoch: 1,
            audio,
            save: SaveWorker::new(dir),
            save_allowed: error.is_none(),
            has_save,
            clock: 0.0,
            status_until: if error.is_some() { f64::INFINITY } else { 0.0 },
            autosave: 0.0,
            accumulator: 0.0,
            edit_after: 0.0,
            nav_axis: Vec2::ZERO,
            nav_after: 0.0,
            return_screen: Screen::Title,
            armed: false,
        };
        app.update_audio();
        app
    }
    fn update_audio(&mut self) {
        self.audio.settings.effects_volume = self.settings.volume;
        self.audio.settings.music_enabled = false;
        self.audio.update();
    }
    fn message(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.status_until = self.clock + 5.0;
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            world: self.world.clone(),
            player: self.player.clone(),
            settings: self.settings.clone(),
        }
    }
    pub fn request_save(&mut self) {
        if !self.save_allowed {
            return;
        }
        if self.save.busy {
            self.message("正在保存，请稍候");
            return;
        }
        let snapshot = self.snapshot();
        match self.save.request(snapshot) {
            Ok(()) => {
                self.autosave = self.clock;
                self.has_save = true;
                self.message("正在保存世界…");
            }
            Err(e) => self.message(format!("保存失败：{e}")),
        }
    }
    pub fn finish(&mut self) -> Result<()> {
        if self.save_allowed {
            self.save.finish()?;
            let snapshot = self.snapshot();
            self.save.request(snapshot)?;
            self.save.finish()?;
        }
        Ok(())
    }
    fn play(&mut self) {
        if !self.save_allowed {
            self.message("存档损坏，请选择新建世界并确认");
            return;
        }
        self.screen = Screen::Playing;
        self.accumulator = 0.0;
        self.armed = false;
        log::info!("Game started: seed={}", self.world.seed);
    }
    fn rebuild(&mut self) {
        if let Err(e) = self.save.finish() {
            self.message(format!("保存失败，未重建：{e}"));
            return;
        }
        self.world = World::generate(new_seed());
        self.player = Player::new(&self.world);
        self.epoch += 1;
        self.save_allowed = true;
        self.play();
        self.request_save();
        log::info!("New world: seed={}", self.world.seed);
    }
    fn open_pause(&mut self) {
        self.screen = Screen::Pause;
        self.cursor = 0;
        self.accumulator = 0.0;
        self.request_save();
        log::info!("Paused");
    }
    fn adjust(&mut self, step: i32) {
        match self.cursor {
            2 => self.settings.view_distance = (self.settings.view_distance + step).clamp(2, 4),
            3 => {
                self.settings.sensitivity =
                    (self.settings.sensitivity + step as f32 * 0.1).clamp(0.5, 2.0)
            }
            4 => {
                self.settings.volume = (self.settings.volume + step as f32 * 0.1).clamp(0.0, 1.0);
                self.update_audio();
            }
            _ => {}
        }
        log::info!(
            "Settings: view={}, sensitivity={:.1}, volume={:.0}%",
            self.settings.view_distance,
            self.settings.sensitivity,
            self.settings.volume * 100.0
        );
    }
    fn confirm(&mut self) -> bool {
        match self.screen {
            Screen::Title => match self.cursor {
                0 => self.play(),
                1 => {
                    if self.has_save {
                        self.return_screen = Screen::Title;
                        self.screen = Screen::ConfirmNew;
                        self.cursor = 0;
                    } else {
                        self.rebuild();
                    }
                }
                _ => return true,
            },
            Screen::Pause => match self.cursor {
                0 => self.play(),
                1 => self.request_save(),
                2..=4 => self.adjust(1),
                5 => {
                    self.return_screen = Screen::Pause;
                    self.screen = Screen::ConfirmNew;
                    self.cursor = 0;
                }
                _ => return true,
            },
            Screen::Palette => {
                self.settings.selected = self.cursor as u8 + 1;
                self.screen = Screen::Playing;
                self.armed = false;
                log::info!("Block selected: {}", self.settings.selected);
            }
            Screen::ConfirmNew => {
                if self.cursor == 1 {
                    self.rebuild();
                } else {
                    self.screen = self.return_screen;
                    self.cursor = 0;
                }
            }
            Screen::Playing => {}
        }
        false
    }
    pub fn update(&mut self, input: &Frame, dt: f32, now: f64) -> bool {
        self.clock = now;
        if let Some(result) = self.save.poll() {
            match result {
                Ok(()) => self.message("世界已保存"),
                Err(e) => {
                    log::error!("Save failed: {e}");
                    self.message(format!("保存失败，旧存档已保留：{e}"));
                }
            }
        }
        if now > self.status_until {
            self.status.clear();
        }
        if input.held[input::SELECT] && input.held[input::START] {
            log::info!("Exit requested by SELECT + START");
            return true;
        }
        if self.screen == Screen::Playing {
            if input.pressed[input::START] {
                self.open_pause();
                return false;
            }
            if input.pressed[input::Y] {
                self.screen = Screen::Palette;
                self.cursor = self.settings.selected as usize - 1;
                self.accumulator = 0.0;
                log::info!("Palette opened");
                return false;
            }
            if input.pressed[input::X] {
                self.player.flying = !self.player.flying;
                self.player.velocity = 0.0;
                self.message(if self.player.flying {
                    "飞行模式：B 上升 · A 下降"
                } else {
                    "步行模式：B 跳跃"
                });
                log::info!("Flight: {}", self.player.flying);
            }
            for (button, step) in [(input::L, -1), (input::R, 1)] {
                if input.pressed[button] {
                    self.settings.selected =
                        ((i32::from(self.settings.selected) - 1 + step).rem_euclid(8) + 1) as u8;
                    self.audio.play(Sound::Select);
                    log::info!("Block selected: {}", self.settings.selected);
                }
            }
            if !input.dig
                && !input.place
                && ![input::A, input::B, input::START, input::Y]
                    .iter()
                    .any(|&b| input.held[b])
            {
                self.armed = true;
            }
            self.accumulator = (self.accumulator + dt.min(0.1)).min(0.1);
            while self.accumulator >= 1.0 / 60.0 {
                self.player
                    .look(input.look, 1.0 / 60.0, self.settings.sensitivity);
                if self.player.tick(
                    &self.world,
                    input.movement,
                    self.armed && input.held[input::B],
                    self.armed && input.held[input::A],
                    1.0 / 60.0,
                ) {
                    self.audio.play(Sound::Jump);
                    log::info!("Jump");
                }
                self.accumulator -= 1.0 / 60.0;
            }
            if self.armed && now >= self.edit_after && (input.dig || input.place) {
                self.edit_after = now + 0.18;
                if let Some(hit) =
                    self.world
                        .raycast(self.player.eye(), self.player.direction(), 6.0)
                {
                    if input.dig {
                        let block = self.world.get(hit.block);
                        if self.world.set(hit.block, 0) {
                            self.audio.play(Sound::Place(true));
                            log::info!("Block broken: {:?}, {}", hit.block, block);
                        } else {
                            self.message("基岩无法破坏");
                        }
                    } else {
                        let target = hit.block + hit.normal;
                        if inside(target)
                            && hit.normal != glam::IVec3::ZERO
                            && self.world.get(target) == 0
                            && !self.player.overlaps(target)
                        {
                            if self.world.set(target, self.settings.selected) {
                                self.audio.play(Sound::Place(false));
                                log::info!(
                                    "Block placed: {:?}, {}",
                                    target,
                                    self.settings.selected
                                );
                            }
                        } else {
                            self.message("此处无法放置方块");
                        }
                    }
                }
            }
            if now - self.autosave >= 30.0 && !self.save.busy {
                self.request_save();
            }
            return false;
        }
        if input.pressed[input::B] || (input.pressed[input::START] && self.screen != Screen::Title)
        {
            match self.screen {
                Screen::Pause | Screen::Palette => self.play(),
                Screen::ConfirmNew => {
                    self.screen = self.return_screen;
                    self.cursor = 0;
                }
                _ => {}
            }
            return false;
        }
        let direction = if input.dpad != Vec2::ZERO {
            input.dpad
        } else {
            input.movement
        };
        let axis = if direction.abs().max_element() > 0.5 {
            if direction.x.abs() > direction.y.abs() {
                Vec2::new(direction.x.signum(), 0.0)
            } else {
                Vec2::new(0.0, direction.y.signum())
            }
        } else {
            Vec2::ZERO
        };
        if axis != self.nav_axis || now >= self.nav_after {
            if axis != Vec2::ZERO {
                let count = match self.screen {
                    Screen::Title => 3,
                    Screen::Pause => 7,
                    Screen::Palette => 8,
                    Screen::ConfirmNew => 2,
                    Screen::Playing => 1,
                };
                if self.screen == Screen::Pause && axis.x != 0.0 {
                    self.adjust(axis.x as i32);
                } else {
                    let step = if axis.y != 0.0 { axis.y } else { axis.x } as i32;
                    self.cursor = (self.cursor as i32 + step).rem_euclid(count) as usize;
                }
                self.nav_after = now + if axis != self.nav_axis { 0.3 } else { 0.105 };
            }
            self.nav_axis = axis;
        }
        if input.pressed[input::A] || (input.pressed[input::START] && self.screen == Screen::Title)
        {
            return self.confirm();
        }
        false
    }
    pub fn target(&self) -> Option<crate::world::Hit> {
        self.world
            .raycast(self.player.eye(), self.player.direction(), 6.0)
    }
    pub fn block_name(&self) -> &'static str {
        NAMES[self.settings.selected as usize]
    }
    pub fn camera_position(&self) -> Vec3 {
        self.player.eye()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controller_edit_pause_and_corrupt_save_protection() {
        let dir = std::env::temp_dir().join(format!("voxel-app-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::with_audio(dir.clone(), Some(42), Audio::silent());
        let mut frame = Frame::default();
        frame.pressed[input::A] = true;
        app.update(&frame, 0.016, 0.0);
        assert!(app.screen == Screen::Playing);
        app.player.pitch = -1.0;
        app.update(&Frame::default(), 0.016, 0.1);
        let hit = app.target().unwrap();
        let old = app.world.get(hit.block);
        frame = Frame {
            dig: true,
            ..Default::default()
        };
        app.update(&frame, 0.016, 0.2);
        assert_eq!(app.world.get(hit.block), 0);
        assert!(old != 0);
        app.update(&Frame::default(), 0.016, 0.3);
        frame = Frame {
            place: true,
            ..Default::default()
        };
        app.update(&frame, 0.016, 0.5);
        assert_eq!(app.world.get(hit.block), 1);
        let player = app.player.position;
        frame = Frame::default();
        frame.pressed[input::START] = true;
        app.update(&frame, 0.016, 0.6);
        assert!(app.screen == Screen::Pause);
        frame = Frame {
            movement: Vec2::new(1.0, 0.0),
            ..Default::default()
        };
        app.update(&frame, 1.0, 0.7);
        assert_eq!(player, app.player.position);
        app.player.flying = true;
        frame = Frame::default();
        frame.pressed[input::B] = true;
        frame.held[input::B] = true;
        app.update(&frame, 1.0 / 60.0, 0.8);
        frame.pressed[input::B] = false;
        app.update(&frame, 1.0 / 60.0, 0.9);
        assert_eq!(player, app.player.position, "menu cancel must not ascend");
        app.update(&Frame::default(), 1.0 / 60.0, 1.0);
        app.update(&frame, 1.0 / 60.0, 1.1);
        assert!(app.player.position.y > player.y);
        app.finish().unwrap();
        drop(app);
        let path = dir.join("world.dat");
        std::fs::write(&path, b"corrupt").unwrap();
        let mut app = App::with_audio(dir.clone(), Some(43), Audio::silent());
        frame = Frame::default();
        frame.pressed[input::A] = true;
        app.update(&frame, 0.016, 0.0);
        assert!(app.screen == Screen::Title);
        app.finish().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
        app.cursor = 1;
        app.update(&frame, 0.016, 0.1);
        assert!(app.screen == Screen::ConfirmNew);
        assert_eq!(app.cursor, 0);
        app.update(&frame, 0.016, 0.2);
        assert!(app.screen == Screen::Title);
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

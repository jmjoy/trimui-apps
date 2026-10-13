use crate::render::sky;
use crate::{
    audio::{Audio, Kind, Material, Sound},
    config::{self, Config, ConfigStore, Loaded},
    input::{Action, Button, Frame, Gestures, bindable},
    jobs::Pool,
    player::{Intent, Player},
    save::{self, Level, SaveJob, SaveWorker, WorldDir, region::RegionStore},
    world::{self, World, block, light::Lighter, stream::Stream},
};
use anyhow::Result;
use glam::{Vec2, Vec3};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Title,
    Playing,
    Pause,
    Settings,
    Controls,
    ConfirmReset,
    Palette,
    ConfirmNew,
}

/// One row of a menu: a label and an optional value shown on the right.
pub struct MenuItem {
    pub label: String,
    pub value: Option<String>,
}
impl MenuItem {
    fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: None,
        }
    }
    fn value(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: Some(value.into()),
        }
    }
}
pub struct Menu {
    pub title: &'static str,
    pub subtitle: String,
    pub items: Vec<MenuItem>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SettingRow {
    ViewDistance,
    SmoothLighting,
    FancyLeaves,
    Clouds,
    Sensitivity,
    InvertY,
    Master,
    Effects,
    AllCores,
    DebugOverlay,
    Controls,
    Back,
}
const SETTINGS: [SettingRow; 12] = [
    SettingRow::ViewDistance,
    SettingRow::SmoothLighting,
    SettingRow::FancyLeaves,
    SettingRow::Clouds,
    SettingRow::Sensitivity,
    SettingRow::InvertY,
    SettingRow::Master,
    SettingRow::Effects,
    SettingRow::AllCores,
    SettingRow::DebugOverlay,
    SettingRow::Controls,
    SettingRow::Back,
];
const LISTEN_SECONDS: f64 = 5.0;
/// Horizontal blocks walked per footstep sound.
const STEP_DISTANCE: f32 = 1.7;

fn settings_index(row: SettingRow) -> usize {
    SETTINGS.iter().position(|r| *r == row).unwrap()
}

pub struct App {
    pub world: World,
    pub player: Player,
    /// Creative hotbar (block ids) and the selected slot, saved per world.
    pub hotbar: [u8; save::HOTBAR],
    pub slot: usize,
    pub stream: Stream,
    pub lighter: Lighter,
    /// World time in ticks; advances while playing.
    pub time: f64,
    pub config: Config,
    pub screen: Screen,
    pub cursor: usize,
    pub status: String,
    pub epoch: u64,
    pub audio: Audio,
    pub save: SaveWorker,
    pub save_allowed: bool,
    pub has_save: bool,
    /// The debug overlay was forced on from the command line.
    pub overlay_forced: bool,
    /// Rebinding: the action waiting for a button and the time limit.
    pub listening: Option<(Action, f64)>,
    pub sneak_latched: bool,
    pub sprint_latched: bool,
    /// Smoothed field-of-view widening while sprinting, in degrees.
    pub fov_boost: f32,
    config_store: ConfigStore,
    /// Benchmark sessions never read or write the save directory.
    persistent: bool,
    world_dir: WorldDir,
    /// The player's position was checked against loaded terrain.
    placed: bool,
    /// Background save started because far columns wait for it.
    quiet_save: bool,
    far_save_after: f64,
    clock: f64,
    status_until: f64,
    autosave: f64,
    accumulator: f32,
    edit_after: f64,
    nav_axis: Vec2,
    nav_after: f64,
    return_screen: Screen,
    armed: bool,
    gestures: Gestures,
    double_sprint: bool,
    step_distance: f32,
}
fn new_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
impl App {
    pub fn new(
        dir: PathBuf,
        config_dir: PathBuf,
        seed: Option<u64>,
        persistent: bool,
        pool: Arc<Pool>,
    ) -> Self {
        Self::with_audio(dir, config_dir, seed, Audio::new(), persistent, pool)
    }
    fn with_audio(
        dir: PathBuf,
        config_dir: PathBuf,
        seed: Option<u64>,
        audio: Audio,
        persistent: bool,
        pool: Arc<Pool>,
    ) -> Self {
        let legacy = persistent
            && save::retire_legacy(&dir).unwrap_or_else(|e| {
                log::error!("Could not rename the legacy save: {e:#}");
                false
            });
        let world_dir = WorldDir::new(&dir, 0);
        let loaded = if persistent {
            world_dir.read_level()
        } else {
            Ok(None)
        };
        let has_save = persistent && world_dir.exists();
        let (level, error) = match loaded {
            Ok(level) => (level, None),
            Err(e) => (None, Some(format!("存档读取失败，原文件已保留：{e:#}"))),
        };
        let world = World::new(
            level
                .as_ref()
                .map_or_else(|| seed.unwrap_or_else(new_seed), |l| l.seed),
        );
        let player = level
            .as_ref()
            .map_or_else(|| Player::new(&world), |l| l.player.clone());
        let hotbar = level
            .as_ref()
            .map_or_else(save::default_hotbar, |l| l.hotbar);
        let slot = level.as_ref().map_or(0, |l| usize::from(l.slot));
        let time = level.as_ref().map_or_else(save::default_time, |l| l.time);
        // A damaged level never has its region files read or written.
        let store = if persistent && error.is_none() {
            Arc::new(RegionStore::new(world_dir.regions()))
        } else {
            Arc::new(RegionStore::memory())
        };
        if level.is_some() {
            let (columns, digest) =
                save::region::digest(&world_dir.regions()).unwrap_or_else(|e| {
                    log::error!("Region scan failed: {e:#}");
                    (0, 0)
                });
            log::info!(
                "World loaded: seed={}, position={:?}, slot={slot}, columns={columns}, digest={digest:016x}",
                world.seed,
                player.position
            );
        }
        let (config_store, config, config_warning) = if persistent {
            let (store, loaded) = ConfigStore::load(&config_dir);
            match loaded {
                Loaded::Existing(c) => (store, c, None),
                Loaded::Fresh(c) => (store, c, None),
                Loaded::Recovered(c, warning) => (store, c, Some(warning)),
            }
        } else {
            let c = Config::default();
            (ConfigStore::memory(&c), c, None)
        };
        log::info!(
            "World ready: seed={}, spawn={:?}, player={:?}",
            world.seed,
            world.spawn(),
            player.position
        );
        let legacy_note =
            legacy.then(|| "旧版存档已保留为 save/world.v1.dat，已创建新的无限世界".to_owned());
        let status = error
            .clone()
            .or(config_warning)
            .or(legacy_note)
            .unwrap_or_default();
        let save = SaveWorker::new(
            (persistent && error.is_none()).then(|| world_dir.clone()),
            store.clone(),
        );
        let mut app = Self {
            world,
            player,
            hotbar,
            slot,
            lighter: Lighter::new(pool.clone()),
            time,
            stream: Stream::new(pool, store),
            config,
            screen: Screen::Title,
            cursor: 0,
            status_until: if status.is_empty() {
                0.0
            } else if error.is_some() {
                f64::INFINITY
            } else {
                8.0
            },
            status,
            epoch: 1,
            audio,
            save,
            save_allowed: error.is_none(),
            has_save,
            overlay_forced: false,
            listening: None,
            sneak_latched: false,
            sprint_latched: false,
            fov_boost: 0.0,
            config_store,
            persistent,
            world_dir,
            placed: false,
            quiet_save: false,
            far_save_after: 0.0,
            clock: 0.0,
            autosave: 0.0,
            accumulator: 0.0,
            edit_after: 0.0,
            nav_axis: Vec2::ZERO,
            nav_after: 0.0,
            return_screen: Screen::Title,
            armed: false,
            gestures: Gestures::default(),
            double_sprint: false,
            step_distance: 1.0,
        };
        app.update_audio();
        // Persist the settings file right away when it was missing or reset.
        app.save_config();
        // The columns around the player load and light before the first
        // frame, so a world can be entered without waiting.
        let start = std::time::Instant::now();
        let center = world::column_at(app.player.position);
        app.stream.prime(&mut app.world, center, 2);
        Lighter::light_now(&mut app.world);
        log::info!(
            "Terrain primed: {} columns in {:.0} ms",
            app.world.len(),
            start.elapsed().as_secs_f64() * 1000.0
        );
        app
    }
    fn update_audio(&mut self) {
        self.audio
            .set_levels(self.config.audio.master, self.config.audio.effects);
    }
    fn message(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.status_until = self.clock + 5.0;
    }
    fn save_config(&mut self) {
        if let Some(e) = self.config_store.save(&self.config) {
            self.message(format!("设置保存失败：{e}"));
        }
    }
    pub fn want_all_cores(&self) -> bool {
        self.config.advanced.all_cores
    }
    pub fn show_overlay(&self) -> bool {
        self.overlay_forced || self.config.advanced.debug_overlay
    }
    /// Level metadata and the raw records of every modified column.
    fn save_job(&self) -> SaveJob {
        SaveJob {
            level: Level {
                version: save::LEVEL_VERSION,
                name: "世界 1".into(),
                seed: self.world.seed,
                block_table: save::BLOCK_TABLE,
                player: self.player.clone(),
                hotbar: self.hotbar,
                time: self.time,
                slot: self.slot as u8,
            },
            columns: self
                .world
                .columns()
                .filter(|c| c.dirty())
                .map(|c| (c.pos, c.changes, c.encode()))
                .collect(),
        }
    }
    fn start_save(&mut self, quiet: bool) -> bool {
        if !self.save_allowed || !self.persistent || self.save.busy {
            return false;
        }
        let job = self.save_job();
        match self.save.request(job) {
            Ok(()) => {
                self.autosave = self.clock;
                self.has_save = true;
                self.quiet_save = quiet;
                true
            }
            Err(e) => {
                self.message(format!("保存失败：{e}"));
                false
            }
        }
    }
    pub fn request_save(&mut self) {
        if self.save.busy && self.persistent && self.save_allowed {
            self.message("正在保存，请稍候");
        } else if self.start_save(false) {
            self.message("正在保存世界…");
        }
    }
    /// Marks columns as saved; edits made after the copy stay dirty.
    fn saved(&mut self, result: std::result::Result<save::Saved, String>, quiet: bool) {
        match result {
            Ok(saved) => {
                for (pos, changes) in saved.columns {
                    if let Some(c) = self.world.columns_mut().find(|c| c.pos == pos) {
                        c.saved_changes = c.saved_changes.max(changes);
                    }
                }
                log::debug!("Save acknowledged: digest={:016x}", saved.digest);
                if !quiet {
                    self.message(format!("世界已保存 · {} 个已修改区块柱", saved.stored));
                }
            }
            Err(e) => {
                log::error!("Save failed: {e}");
                self.message(format!("保存失败，旧存档已保留：{e}"));
            }
        }
    }
    pub fn finish(&mut self) -> Result<()> {
        self.save_config();
        let config = self.config_store.flush();
        if self.save_allowed && self.persistent {
            let quiet = self.quiet_save;
            if let Some(result) = self.save.wait() {
                self.saved(result, quiet);
            }
            self.start_save(true);
            match self.save.wait() {
                Some(Ok(saved)) => self.saved(Ok(saved), true),
                Some(Err(e)) => anyhow::bail!("Saving world: {e}"),
                None => anyhow::bail!("Final save did not start"),
            }
        }
        config
    }
    /// Whether the terrain around the player is loaded; physics and edits
    /// wait for it.
    pub fn terrain_ready(&self) -> bool {
        self.placed
            && self
                .world
                .area_loaded(world::column_at(self.player.position), 1)
    }
    fn play(&mut self) {
        if !self.save_allowed {
            self.message("存档损坏，请选择新建世界并确认");
            return;
        }
        if matches!(
            self.screen,
            Screen::Settings | Screen::Controls | Screen::ConfirmReset
        ) {
            self.save_config();
        }
        self.screen = Screen::Playing;
        self.listening = None;
        self.accumulator = 0.0;
        self.armed = false;
        self.gestures.reset();
        log::info!("Game started: seed={}", self.world.seed);
    }
    /// Enters the world without the title screen (tests and screenshots).
    pub fn start_world(&mut self) {
        self.play();
    }
    /// Moves the player; physics waits until the terrain there is loaded.
    pub fn teleport(&mut self, feet: Vec3) {
        self.player.position = feet;
        self.player.velocity = 0.0;
        self.placed = false;
        log::info!("Teleported to {feet:?}");
    }
    /// Builds a platform showing every kind of block model in front of the
    /// player (rendering checks with throw-away saves).
    pub fn build_showcase(&mut self) {
        use block::*;
        use glam::IVec3;
        let base = self.player.position.floor().as_ivec3() - IVec3::new(0, 1, 0);
        let w = &mut self.world;
        let at = |x: i32, y: i32, z: i32| base + IVec3::new(x, y, z);
        for z in -14..=2 {
            for x in -8..=8 {
                w.set(at(x, 0, z), STONE);
                for y in 1..8 {
                    w.set(at(x, y, z), AIR);
                }
            }
        }
        // A pool of water with a lava pit beside it.
        for z in -6..=-4 {
            for x in -6..=-3 {
                for y in -2..=0 {
                    w.set(at(x, y, z), WATER);
                }
                w.set(at(x, -3, z), SAND);
            }
        }
        w.set(at(-1, 0, -5), LAVA);
        // Plants on grass, sand and farmland.
        let plants = [
            TALL_GRASS,
            DANDELION,
            POPPY,
            OAK_SAPLING,
            BIRCH_SAPLING,
            SPRUCE_SAPLING,
        ];
        for (i, plant) in plants.into_iter().enumerate() {
            let x = i as i32 * 2 - 6;
            w.set(at(x, 0, -9), GRASS);
            w.place_block(at(x, 1, -9), plant, 0);
        }
        for (x, plant) in [(6, SUGAR_CANE), (7, CACTUS)] {
            w.set(at(x, 0, -9), SAND);
            for y in 1..=3 {
                w.place_block(at(x, y, -9), plant, 0);
            }
        }
        // A wall with glass, leaves, ice, a ladder, a door and torches.
        let wall = [
            GLASS,
            OAK_LEAVES,
            BIRCH_LEAVES,
            SPRUCE_LEAVES,
            ICE,
            GLOWSTONE,
        ];
        for x in -7..=7 {
            for y in 1..=4 {
                w.set(at(x, y, -12), STONE_BRICKS);
            }
        }
        for (i, id) in wall.into_iter().enumerate() {
            let x = i as i32 * 2 - 6;
            w.set(at(x, 2, -12), id);
            w.set(at(x, 3, -12), id);
        }
        w.place_block(at(6, 1, -11), LADDER, 0);
        w.place_block(at(6, 2, -11), LADDER, 0);
        w.place_block(at(4, 1, -11), DOOR, 2);
        w.place_block(at(-7, 3, -11), TORCH, torch_state(Face::South).unwrap_or(0));
        w.place_block(at(0, 1, -7), TORCH, 0);
        w.place_block(at(2, 1, -6), SNOW_LAYER, 0);
        w.place_block(at(3, 1, -6), SNOW_LAYER, 0);
        w.place_block(at(-3, 1, -8), CHEST, 2);
        w.place_block(at(-2, 1, -8), FURNACE, 2);
        w.place_block(at(-1, 1, -8), CRAFTING_TABLE, 2);
        self.player.position = at(0, 1, 2).as_vec3() + glam::Vec3::new(0.5, 0.0, 0.5);
        self.player.yaw = 0.0;
        self.player.pitch = -0.35;
        self.player.flying = true;
        log::info!("Showcase built at {base}");
    }
    /// Starts an unattended benchmark flight in the current world.
    pub fn start_bench(&mut self) {
        self.play();
        self.player.flying = true;
        log::info!("Benchmark started");
    }
    fn rebuild(&mut self) {
        if let Some(Err(e)) = self.save.wait() {
            self.message(format!("保存失败，未重建：{e}"));
            return;
        }
        if self.persistent
            && let Err(e) = self.world_dir.retire()
        {
            self.message(format!("无法备份旧世界，未重建：{e:#}"));
            return;
        }
        self.world = World::new(new_seed());
        self.player = Player::new(&self.world);
        self.epoch += 1;
        let store = Arc::new(if self.persistent {
            RegionStore::new(self.world_dir.regions())
        } else {
            RegionStore::memory()
        });
        self.save = SaveWorker::new(
            self.persistent.then(|| self.world_dir.clone()),
            store.clone(),
        );
        self.stream.reset(store, self.epoch);
        self.lighter.reset(self.epoch);
        self.time = save::default_time();
        self.placed = false;
        let center = world::column_at(self.player.position);
        self.stream.prime(&mut self.world, center, 2);
        Lighter::light_now(&mut self.world);
        self.save_allowed = true;
        self.play();
        self.request_save();
        log::info!("New world: seed={}", self.world.seed);
    }
    fn open(&mut self, screen: Screen, cursor: usize) {
        self.screen = screen;
        self.cursor = cursor;
        self.listening = None;
    }
    fn open_pause(&mut self) {
        self.open(Screen::Pause, 0);
        self.accumulator = 0.0;
        self.request_save();
        log::info!("Paused");
    }
    /// The menu shown for the current screen (not used while playing).
    pub fn menu(&self) -> Menu {
        let on_off = |v: bool| if v { "开" } else { "关" };
        match self.screen {
            Screen::Title => Menu {
                title: "方块世界",
                subtitle: "一块一块，造出你的世界".into(),
                items: vec![
                    MenuItem::new(if self.has_save {
                        "继续世界"
                    } else {
                        "开始游戏"
                    }),
                    MenuItem::new("新建世界"),
                    MenuItem::new("退出"),
                ],
            },
            Screen::Pause => Menu {
                title: "暂停",
                subtitle: format!("种子 {} · 无限世界", self.world.seed),
                items: vec![
                    MenuItem::new("继续探索"),
                    MenuItem::new("立即保存"),
                    MenuItem::new("设置…"),
                    MenuItem::new("重建世界…"),
                    MenuItem::new("保存并退出"),
                ],
            },
            Screen::Settings => Menu {
                title: "设置",
                subtitle: "←/→ 调整   设置对所有世界生效".into(),
                items: SETTINGS
                    .iter()
                    .map(|row| match row {
                        SettingRow::SmoothLighting => {
                            MenuItem::value("平滑光照", on_off(self.config.video.smooth_lighting))
                        }
                        SettingRow::FancyLeaves => MenuItem::value(
                            "树叶",
                            if self.config.video.fancy_leaves {
                                "精致"
                            } else {
                                "快速"
                            },
                        ),
                        SettingRow::Clouds => {
                            MenuItem::value("云", on_off(self.config.video.clouds))
                        }
                        SettingRow::ViewDistance => MenuItem::value(
                            "视距",
                            format!("{} 区块", self.config.video.view_distance),
                        ),
                        SettingRow::Sensitivity => MenuItem::value(
                            "视角灵敏度",
                            format!("{:.1}", self.config.controls.sensitivity),
                        ),
                        SettingRow::InvertY => {
                            MenuItem::value("反转视角上下", on_off(self.config.controls.invert_y))
                        }
                        SettingRow::Master => MenuItem::value(
                            "主音量",
                            format!("{:.0}%", self.config.audio.master * 100.0),
                        ),
                        SettingRow::Effects => MenuItem::value(
                            "音效音量",
                            format!("{:.0}%", self.config.audio.effects * 100.0),
                        ),
                        SettingRow::AllCores => MenuItem::value(
                            "启用全部 CPU 核心",
                            on_off(self.config.advanced.all_cores),
                        ),
                        SettingRow::DebugOverlay => {
                            MenuItem::value("调试信息", on_off(self.config.advanced.debug_overlay))
                        }
                        SettingRow::Controls => MenuItem::new("按键设置…"),
                        SettingRow::Back => MenuItem::new("返回"),
                    })
                    .collect(),
            },
            Screen::Controls => {
                let mut items: Vec<_> = Action::ALL
                    .iter()
                    .map(|&a| {
                        let value = match self.listening {
                            Some((listen, until)) if listen == a => {
                                format!("请按键…{:.0}", (until - self.clock).max(0.0).ceil())
                            }
                            _ => self.config.controls.bindings.label(a).to_owned(),
                        };
                        MenuItem::value(a.label(), value)
                    })
                    .collect();
                items.push(MenuItem::new("恢复默认键位…"));
                items.push(MenuItem::new("返回"));
                Menu {
                    title: "按键设置",
                    subtitle: if self.listening.is_some() {
                        "按下新按键绑定；START 取消；冲突的动作会互换按键".into()
                    } else {
                        "A 修改   START 暂停、SELECT+START 退出与菜单 A/B 固定".into()
                    },
                    items,
                }
            }
            Screen::ConfirmReset => Menu {
                title: "恢复默认键位",
                subtitle: "所有动作将恢复为默认按键。".into(),
                items: vec![MenuItem::new("取消"), MenuItem::new("确认恢复")],
            },
            Screen::ConfirmNew => Menu {
                title: "新建世界",
                subtitle: "这会替换当前建筑与存档。确认重新开始？".into(),
                items: vec![MenuItem::new("保留当前世界"), MenuItem::new("确认重建")],
            },
            Screen::Palette => {
                let palette = block::palette();
                Menu {
                    title: "方块库",
                    subtitle: format!(
                        "{} · 放入快捷栏第 {} 格",
                        palette
                            .get(self.cursor)
                            .map_or("", |&id| block::def(id).name),
                        self.slot + 1
                    ),
                    items: palette
                        .iter()
                        .map(|&id| MenuItem::new(block::def(id).name))
                        .collect(),
                }
            }
            Screen::Playing => Menu {
                title: "",
                subtitle: String::new(),
                items: Vec::new(),
            },
        }
    }
    fn adjust(&mut self, step: i32) {
        let Some(&row) = SETTINGS.get(self.cursor) else {
            return;
        };
        let c = &mut self.config;
        match row {
            SettingRow::SmoothLighting => c.video.smooth_lighting = !c.video.smooth_lighting,
            SettingRow::FancyLeaves => c.video.fancy_leaves = !c.video.fancy_leaves,
            SettingRow::Clouds => c.video.clouds = !c.video.clouds,
            SettingRow::ViewDistance => {
                c.video.view_distance = (c.video.view_distance + step)
                    .clamp(*config::VIEW_DISTANCE.start(), *config::VIEW_DISTANCE.end())
            }
            SettingRow::Sensitivity => {
                c.controls.sensitivity = (c.controls.sensitivity + step as f32 * 0.1)
                    .clamp(*config::SENSITIVITY.start(), *config::SENSITIVITY.end())
            }
            SettingRow::InvertY => c.controls.invert_y = !c.controls.invert_y,
            SettingRow::Master => {
                c.audio.master = (c.audio.master + step as f32 * 0.1).clamp(0.0, 1.0)
            }
            SettingRow::Effects => {
                c.audio.effects = (c.audio.effects + step as f32 * 0.1).clamp(0.0, 1.0)
            }
            SettingRow::AllCores => c.advanced.all_cores = !c.advanced.all_cores,
            SettingRow::DebugOverlay => c.advanced.debug_overlay = !c.advanced.debug_overlay,
            SettingRow::Controls | SettingRow::Back => return,
        }
        c.sanitize();
        self.update_audio();
        log::info!(
            "Settings: view={}, smooth={}, fancy_leaves={}, clouds={}, sensitivity={:.1}, invert_y={}, master={:.0}%, effects={:.0}%, all_cores={}, debug_overlay={}",
            self.config.video.view_distance,
            self.config.video.smooth_lighting,
            self.config.video.fancy_leaves,
            self.config.video.clouds,
            self.config.controls.sensitivity,
            self.config.controls.invert_y,
            self.config.audio.master * 100.0,
            self.config.audio.effects * 100.0,
            self.config.advanced.all_cores,
            self.config.advanced.debug_overlay
        );
    }
    fn back(&mut self) {
        match self.screen {
            Screen::Pause | Screen::Palette => self.play(),
            Screen::Settings => {
                self.save_config();
                self.open(Screen::Pause, 2);
            }
            Screen::Controls => {
                self.save_config();
                self.open(Screen::Settings, settings_index(SettingRow::Controls));
            }
            Screen::ConfirmReset => self.open(Screen::Controls, Action::COUNT),
            Screen::ConfirmNew => self.open(self.return_screen, 0),
            Screen::Title | Screen::Playing => {}
        }
    }
    fn confirm(&mut self) -> bool {
        match self.screen {
            Screen::Title => match self.cursor {
                0 => self.play(),
                1 => {
                    if self.has_save {
                        self.return_screen = Screen::Title;
                        self.open(Screen::ConfirmNew, 0);
                    } else {
                        self.rebuild();
                    }
                }
                _ => return true,
            },
            Screen::Pause => match self.cursor {
                0 => self.play(),
                1 => self.request_save(),
                2 => self.open(Screen::Settings, 0),
                3 => {
                    self.return_screen = Screen::Pause;
                    self.open(Screen::ConfirmNew, 0);
                }
                _ => return true,
            },
            Screen::Settings => match SETTINGS[self.cursor] {
                SettingRow::Controls => self.open(Screen::Controls, 0),
                SettingRow::Back => self.back(),
                _ => self.adjust(1),
            },
            Screen::Controls => {
                if let Some(&action) = Action::ALL.get(self.cursor) {
                    self.listening = Some((action, self.clock + LISTEN_SECONDS));
                    self.audio.play(Sound::UiClick);
                    log::info!("Rebinding {}", action.key());
                } else if self.cursor == Action::COUNT {
                    self.open(Screen::ConfirmReset, 0);
                } else {
                    self.back();
                }
            }
            Screen::ConfirmReset => {
                if self.cursor == 1 {
                    self.config.controls.bindings.reset();
                    self.save_config();
                    self.message("已恢复默认键位");
                    log::info!("Bindings reset to defaults");
                }
                self.open(Screen::Controls, Action::COUNT);
            }
            Screen::Palette => {
                if let Some(&id) = block::palette().get(self.cursor) {
                    self.hotbar[self.slot] = id;
                    log::info!(
                        "Palette pick: {} into slot {}",
                        block::def(id).key,
                        self.slot + 1
                    );
                }
                self.play();
            }
            Screen::ConfirmNew => {
                if self.cursor == 1 {
                    self.rebuild();
                } else {
                    self.back();
                }
            }
            Screen::Playing => {}
        }
        false
    }
    /// Rebinding mode consumes every button press except START.
    fn listen(&mut self, input: &Frame, action: Action, until: f64) {
        if input.pressed(Button::Start) || self.clock >= until {
            self.listening = None;
            log::info!("Rebinding cancelled");
            return;
        }
        let Some(button) = input.pressed.iter().find(|b| bindable(*b)) else {
            return;
        };
        let swapped = self.config.controls.bindings.bind(action, button);
        self.listening = None;
        self.audio.play(Sound::UiClick);
        log::info!(
            "Binding: {} = {}{}",
            action.key(),
            button.key(),
            swapped.map_or(String::new(), |s| format!(" (swapped with {})", s.key()))
        );
        match swapped {
            Some(other) => self.message(format!(
                "「{}」改为 {}，「{}」改为 {}",
                action.label(),
                button.label(),
                other.label(),
                self.config.controls.bindings.label(other)
            )),
            None => self.message(format!("「{}」改为 {}", action.label(), button.label())),
        }
    }
    fn update_playing(&mut self, input: &Frame, dt: f32, now: f64) {
        let bindings = self.config.controls.bindings.clone();
        if input.pressed(Button::Start) {
            self.audio.play(Sound::UiClick);
            self.open_pause();
            return;
        }
        if input.held.is_empty() {
            self.armed = true;
        }
        let armed = self.armed;
        let pressed = |a| armed && bindings.pressed(input, a);
        let held = |a| armed && bindings.held(input, a);
        if pressed(Action::Inventory) {
            self.audio.play(Sound::UiClick);
            let cursor = block::palette()
                .iter()
                .position(|&id| id == self.selected())
                .unwrap_or(0);
            self.open(Screen::Palette, cursor);
            self.accumulator = 0.0;
            log::info!("Palette opened");
            return;
        }
        let gestures =
            self.gestures
                .update(pressed(Action::Jump), (-input.movement.y).max(0.0), now);
        if gestures.double_jump {
            self.player.flying = !self.player.flying;
            self.player.velocity = 0.0;
            self.message(if self.player.flying {
                "飞行：按住跳跃上升，按住潜行下降；双击跳跃落地"
            } else {
                "步行模式"
            });
            log::info!("Flight: {}", self.player.flying);
        }
        if pressed(Action::ToggleSneak) {
            self.sneak_latched = !self.sneak_latched;
            self.message(if self.sneak_latched {
                "潜行：开"
            } else {
                "潜行：关"
            });
            log::info!("Sneak toggle: {}", self.sneak_latched);
        }
        if pressed(Action::ToggleSprint) {
            self.sprint_latched = !self.sprint_latched;
            self.message(if self.sprint_latched {
                "疾跑：开（向前移动时疾跑）"
            } else {
                "疾跑：关"
            });
            log::info!("Sprint toggle: {}", self.sprint_latched);
        }
        // A quick double push forward sprints until the stick is released.
        if gestures.double_push {
            self.double_sprint = true;
            log::info!("Sprint: double push");
        }
        if input.movement.y > -0.3 {
            self.double_sprint = false;
        }
        for (action, step) in [(Action::HotbarPrev, -1), (Action::HotbarNext, 1)] {
            if pressed(action) {
                self.slot = (self.slot as i32 + step).rem_euclid(save::HOTBAR as i32) as usize;
                // Like Bedrock, scrolling the hotbar is silent.
                log::info!(
                    "Block selected: {} ({})",
                    self.slot + 1,
                    block::def(self.selected()).key
                );
            }
        }
        if pressed(Action::PickBlock)
            && let Some(hit) = self.target()
        {
            let id = self.world.get(hit.block);
            if block::def(id).creative {
                // Select it if it is already in the hotbar, else replace
                // the current slot (creative pick block).
                match self.hotbar.iter().position(|&b| b == id) {
                    Some(slot) => self.slot = slot,
                    None => self.hotbar[self.slot] = id,
                }
                log::info!("Block picked: {id}");
            }
        }
        let sneak_held = held(Action::Sneak);
        let intent = Intent {
            axis: input.movement,
            jump: held(Action::Jump),
            sneak: if self.player.flying {
                sneak_held
            } else {
                sneak_held || self.sneak_latched
            },
            sprint: self.sprint_latched || self.double_sprint,
        };
        let mut look = input.look;
        if self.config.controls.invert_y {
            look.y = -look.y;
        }
        // Terrain still streaming in: look around but do not move or edit.
        if !self.terrain_ready() {
            self.player
                .look(look, dt.min(0.1), self.config.controls.sensitivity);
            self.accumulator = 0.0;
            return;
        }
        self.time += f64::from(dt.min(0.1)) * sky::TICKS_PER_SECOND;
        self.accumulator = (self.accumulator + dt.min(0.1)).min(0.1);
        let mut landed = 0.0f32;
        while self.accumulator >= 1.0 / 60.0 {
            self.player
                .look(look, 1.0 / 60.0, self.config.controls.sensitivity);
            // Jumping is silent, as in the original game.
            if self.player.tick(&self.world, intent, 1.0 / 60.0) {
                log::info!("Jump");
            }
            landed = landed.max(self.player.landed);
            if self.player.grounded && !self.player.flying {
                self.step_distance += self.player.moved;
            }
            let boost = if self.player.sprinting { 8.0 } else { 0.0 };
            self.fov_boost += (boost - self.fov_boost) * (1.0f32 / 60.0 * 10.0);
            self.accumulator -= 1.0 / 60.0;
        }
        self.audio.set_listener(self.player.eye(), self.player.yaw);
        self.movement_sounds(landed);
        let dig = held(Action::Attack);
        let place = held(Action::Use);
        if now >= self.edit_after
            && (dig || place)
            && let Some(hit) = self.target()
        {
            self.edit_after = now + 0.18;
            if dig {
                let block = self.world.get(hit.block);
                if let Some(removed) = self.world.break_block(hit.block) {
                    self.audio.play_at(
                        Sound::Block(Kind::Break, world::material(block)),
                        hit.block.as_vec3() + 0.5,
                        1.0,
                        1.0,
                    );
                    log::info!("Block broken: {:?}, {}", hit.block, block);
                    for &(p, id) in &removed[1..] {
                        log::info!("Block dropped: {p:?}, {id}");
                    }
                } else {
                    self.message("基岩无法破坏");
                }
            } else {
                let id = self.selected();
                match self.world.placement(&hit, id, self.player.yaw) {
                    Ok((target, _))
                        if block::solid(id)
                            && (self.player.overlaps(target)
                                || (id == block::DOOR
                                    && self.player.overlaps(target + glam::IVec3::Y))) =>
                    {
                        self.message("此处无法放置方块");
                    }
                    Ok((target, state)) => {
                        if self.world.place_block(target, id, state) {
                            self.audio.play_at(
                                Sound::Block(Kind::Place, world::material(id)),
                                target.as_vec3() + 0.5,
                                1.0,
                                1.0,
                            );
                            log::info!("Block placed: {:?}, {}", target, id);
                        }
                    }
                    Err(reason) => self.message(reason),
                }
            }
        }
        if now - self.autosave >= 30.0 && !self.save.busy {
            self.request_save();
        }
    }
    /// Material of the block the player stands on, checking the footprint
    /// corners when the centre overhangs an edge.
    fn ground_material(&self) -> Option<Material> {
        let p = self.player.position - Vec3::Y * 0.05;
        [
            (0.0, 0.0),
            (0.3, 0.3),
            (-0.3, 0.3),
            (0.3, -0.3),
            (-0.3, -0.3),
        ]
        .into_iter()
        .map(|(dx, dz)| {
            self.world
                .get((p + Vec3::new(dx, 0.0, dz)).floor().as_ivec3())
        })
        .find(|&b| b != 0)
        .map(world::material)
    }
    fn movement_sounds(&mut self, landed: f32) {
        let feet = self.player.position;
        if landed > 3.0
            && let Some(m) = self.ground_material()
        {
            self.step_distance = 0.0;
            let volume = (0.8 + landed / 8.0).min(2.0);
            self.audio
                .play_at(Sound::Block(Kind::Step, m), feet, volume, 0.85);
            if landed > 12.0 {
                self.audio
                    .play_at(Sound::Block(Kind::Place, m), feet, 0.6, 0.75);
            }
        }
        if self.step_distance >= STEP_DISTANCE {
            self.step_distance %= STEP_DISTANCE;
            if let Some(m) = self.ground_material() {
                let volume = if self.player.sneaking { 0.45 } else { 1.0 };
                self.audio
                    .play_at(Sound::Block(Kind::Step, m), feet, volume, 1.0);
            }
        }
    }
    /// Streams columns around the player and saves far modified ones.
    fn update_world(&mut self, now: f64) {
        let center = world::column_at(self.player.position);
        let facing = Vec2::new(self.player.yaw.sin(), -self.player.yaw.cos());
        let dirty_far = self.stream.update(
            &mut self.world,
            center,
            facing,
            self.config.video.view_distance,
        );
        self.lighter.update(&mut self.world, center);
        // Modified columns are only dropped once saved.
        if dirty_far > 0 && now >= self.far_save_after && self.start_save(true) {
            self.far_save_after = now + 5.0;
        }
        if !self.placed && self.world.area_loaded(center, 1) {
            self.placed = true;
            if self.player.unstuck(&self.world) {
                log::info!("Player moved out of terrain to {:?}", self.player.position);
            }
        }
        for warning in self.stream.store().take_warnings() {
            self.status = warning;
            self.status_until = now + 10.0;
        }
    }
    pub fn update(&mut self, input: &Frame, dt: f32, now: f64) -> bool {
        self.clock = now;
        if let Some(result) = self.save.poll() {
            let quiet = self.quiet_save;
            self.saved(result, quiet);
        }
        self.update_world(now);
        if now > self.status_until {
            self.status.clear();
        }
        if input.held(Button::Select) && input.held(Button::Start) {
            log::info!("Exit requested by SELECT + START");
            return true;
        }
        if self.screen == Screen::Playing {
            self.update_playing(input, dt, now);
            return false;
        }
        if let Some((action, until)) = self.listening {
            self.listen(input, action, until);
            return false;
        }
        if input.pressed(Button::B)
            || (input.pressed(Button::Start) && self.screen != Screen::Title)
        {
            self.audio.play(Sound::UiBack);
            if input.pressed(Button::Start) && self.screen != Screen::ConfirmNew {
                self.play();
            } else {
                self.back();
            }
            return false;
        }
        let direction = if input.dpad() != Vec2::ZERO {
            input.dpad()
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
                let count = self.menu().items.len().max(1);
                self.audio.play(Sound::UiMove);
                if self.screen == Screen::Settings && axis.x != 0.0 {
                    self.adjust(axis.x as i32);
                } else {
                    let step = if axis.y == 0.0 {
                        axis.x as i32
                    } else if self.screen == Screen::Palette {
                        axis.y as i32 * crate::ui::PALETTE_COLUMNS as i32
                    } else {
                        axis.y as i32
                    };
                    self.cursor = (self.cursor as i32 + step).rem_euclid(count as i32) as usize;
                }
                self.nav_after = now + if axis != self.nav_axis { 0.3 } else { 0.105 };
            }
            self.nav_axis = axis;
        }
        if input.pressed(Button::A)
            || (input.pressed(Button::Start) && self.screen == Screen::Title)
        {
            self.audio.play(Sound::UiClick);
            return self.confirm();
        }
        false
    }
    pub fn target(&self) -> Option<crate::world::Hit> {
        self.world
            .raycast(self.player.eye(), self.player.direction(), 6.0)
    }
    pub fn selected(&self) -> u8 {
        self.hotbar[self.slot]
    }
    pub fn block_name(&self) -> &'static str {
        block::def(self.selected()).name
    }
    pub fn camera_position(&self) -> Vec3 {
        self.player.eye()
    }
    pub fn fov_degrees(&self) -> f32 {
        70.0 + self.fov_boost
    }
    /// HUD hint line generated from the current bindings.
    pub fn hints(&self) -> String {
        let b = &self.config.controls.bindings;
        let mut parts = Vec::new();
        for a in [Action::Attack, Action::Use] {
            if let Some(button) = b.get(a) {
                parts.push(format!("{} {}", button.label(), a.hint()));
            }
        }
        match (b.get(Action::HotbarPrev), b.get(Action::HotbarNext)) {
            (Some(p), Some(n)) => parts.push(format!("{}/{} 切换", p.label(), n.label())),
            (Some(x), None) | (None, Some(x)) => parts.push(format!("{} 切换", x.label())),
            (None, None) => {}
        }
        if let Some(button) = b.get(Action::Jump) {
            parts.push(format!("{} 跳跃·双击飞行", button.label()));
        }
        for a in [Action::Sneak, Action::Inventory, Action::PickBlock] {
            if let Some(button) = b.get(a) {
                parts.push(format!("{} {}", button.label(), a.hint()));
            }
        }
        parts.push("START 菜单".into());
        parts.join("   ")
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("voxel-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }
    fn app(dir: &std::path::Path, seed: u64) -> App {
        App::with_audio(
            dir.join("save"),
            dir.join("config"),
            Some(seed),
            Audio::silent(),
            true,
            Arc::new(Pool::new(2, 2)),
        )
    }
    /// Runs idle frames until every requested column has arrived.
    fn settle(app: &mut App, now: f64) {
        for _ in 0..3000 {
            app.update(&Frame::default(), 0.0, now);
            let r = world::stream::request_radius(app.config.video.view_distance);
            if app.stream.pending() == 0
                && app
                    .world
                    .area_loaded(world::column_at(app.player.position), r)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("terrain did not load");
    }
    fn press(app: &mut App, buttons: &[Button], now: f64) {
        app.update(&Frame::press(buttons), 1.0 / 60.0, now);
        app.update(&Frame::default(), 1.0 / 60.0, now + 0.01);
    }
    fn trigger(button: Button) -> Frame {
        Frame::press(&[button])
    }

    #[test]
    fn controller_edit_pause_and_corrupt_save_protection() {
        let dir = temp("edit");
        let mut app = app(&dir, 42);
        press(&mut app, &[Button::A], 0.0);
        assert!(app.screen == Screen::Playing);
        app.player.pitch = -1.0;
        app.update(&Frame::default(), 0.016, 0.1);
        let hit = app.target().unwrap();
        let old = app.world.get(hit.block);
        app.update(&trigger(Button::R2), 0.016, 0.2);
        assert_eq!(app.world.get(hit.block), 0);
        assert!(old != 0);
        app.update(&Frame::default(), 0.016, 0.3);
        app.update(&trigger(Button::L2), 0.016, 0.5);
        assert_eq!(app.world.get(hit.block), 1);
        let player = app.player.position;
        press(&mut app, &[Button::Start], 0.6);
        assert!(app.screen == Screen::Pause);
        let frame = Frame {
            movement: Vec2::new(1.0, 0.0),
            ..Default::default()
        };
        app.update(&frame, 1.0, 0.7);
        assert_eq!(player, app.player.position);
        // B in a menu goes back without making the player jump or ascend.
        app.player.flying = true;
        let b = Frame::press(&[Button::B]);
        app.update(&b, 1.0 / 60.0, 0.8);
        assert!(app.screen == Screen::Playing);
        let hold_b = Frame {
            held: b.held,
            ..Default::default()
        };
        app.update(&hold_b, 1.0 / 60.0, 0.9);
        assert_eq!(player, app.player.position, "menu cancel must not ascend");
        app.update(&Frame::default(), 1.0 / 60.0, 1.0);
        app.update(&hold_b, 1.0 / 60.0, 1.1);
        assert!(app.player.position.y > player.y);
        app.finish().unwrap();
        drop(app);
        let path = dir.join("save/worlds/0/level.json");
        std::fs::write(&path, b"corrupt").unwrap();
        let mut app = self::app(&dir, 43);
        press(&mut app, &[Button::A], 0.0);
        assert!(app.screen == Screen::Title);
        app.finish().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
        app.cursor = 1;
        press(&mut app, &[Button::A], 0.1);
        assert!(app.screen == Screen::ConfirmNew);
        assert_eq!(app.cursor, 0);
        press(&mut app, &[Button::A], 0.2);
        assert!(app.screen == Screen::Title);
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bedrock_controls_double_tap_flight_toggles_and_pick() {
        let dir = temp("controls");
        let mut app = app(&dir, 42);
        press(&mut app, &[Button::A], 0.0);
        for _ in 0..60 {
            app.update(&Frame::default(), 1.0 / 60.0, 0.1);
        }
        assert!(!app.player.flying);
        press(&mut app, &[Button::B], 1.0);
        press(&mut app, &[Button::B], 1.2);
        assert!(app.player.flying, "double tap B toggles flight");
        press(&mut app, &[Button::B], 2.0);
        press(&mut app, &[Button::B], 2.6);
        assert!(app.player.flying, "taps outside the window do not toggle");
        press(&mut app, &[Button::Left], 3.0);
        assert!(app.sneak_latched);
        press(&mut app, &[Button::Right], 3.1);
        assert!(app.sprint_latched);
        press(&mut app, &[Button::R], 3.2);
        assert_eq!((app.slot, app.selected()), (1, block::DIRT));
        press(&mut app, &[Button::L], 3.3);
        press(&mut app, &[Button::L], 3.31);
        assert_eq!(app.slot, 8, "the hotbar wraps around");
        press(&mut app, &[Button::R], 3.32);
        // Y selects a hotbar slot holding the block, or replaces the slot.
        app.player.flying = false;
        app.player.pitch = -1.5;
        let below = app.target().unwrap().block;
        app.world.set(below, block::OAK_PLANKS);
        press(&mut app, &[Button::Y], 3.4);
        assert_eq!((app.slot, app.selected()), (4, block::OAK_PLANKS));
        app.world.set(below, block::STONE_BRICKS);
        press(&mut app, &[Button::Y], 3.41);
        assert_eq!((app.slot, app.selected()), (4, block::STONE_BRICKS));
        press(&mut app, &[Button::X], 3.5);
        assert!(app.screen == Screen::Palette);
        let palette = block::palette();
        assert_eq!(palette[app.cursor], block::STONE_BRICKS);
        // Down moves one grid row; A puts the block into the slot.
        press(&mut app, &[Button::Down], 3.55);
        let expect = palette[(palette
            .iter()
            .position(|&b| b == block::STONE_BRICKS)
            .unwrap()
            + crate::ui::PALETTE_COLUMNS)
            % palette.len()];
        press(&mut app, &[Button::A], 3.56);
        assert!(app.screen == Screen::Playing);
        assert_eq!(app.selected(), expect);
        press(&mut app, &[Button::X], 3.57);
        press(&mut app, &[Button::B], 3.6);
        assert!(app.screen == Screen::Playing);
        // SELECT + START always exits, even after rebinding SELECT.
        app.config
            .controls
            .bindings
            .bind(Action::Jump, Button::Select);
        assert!(app.update(&Frame::press(&[Button::Select, Button::Start]), 0.016, 4.0));
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rebinding_menu_swaps_resets_and_persists() {
        let dir = temp("rebind");
        let mut app = app(&dir, 42);
        press(&mut app, &[Button::A], 0.0);
        press(&mut app, &[Button::Start], 0.1);
        // Let the pause save finish so its message cannot replace others.
        while app.save.busy {
            app.update(&Frame::default(), 0.0, 0.1);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.cursor = 2;
        press(&mut app, &[Button::A], 0.2);
        assert!(app.screen == Screen::Settings);
        app.cursor = settings_index(SettingRow::Controls);
        press(&mut app, &[Button::A], 0.3);
        assert!(app.screen == Screen::Controls);
        // Jump is the first action; bind it to R, which HotbarNext used.
        press(&mut app, &[Button::A], 0.4);
        assert!(app.listening.is_some());
        press(&mut app, &[Button::R], 0.5);
        assert!(app.listening.is_none());
        let b = &app.config.controls.bindings;
        assert_eq!(b.get(Action::Jump), Some(Button::R));
        assert_eq!(b.get(Action::HotbarNext), Some(Button::B));
        assert!(app.status.contains("快捷栏下一格"));
        // START cancels listening; the listener also times out.
        press(&mut app, &[Button::A], 0.6);
        press(&mut app, &[Button::Start], 0.7);
        assert!(app.listening.is_none() && app.screen == Screen::Controls);
        press(&mut app, &[Button::A], 0.8);
        app.update(&Frame::default(), 0.016, 0.8 + LISTEN_SECONDS + 0.1);
        assert!(app.listening.is_none());
        assert_eq!(
            app.config.controls.bindings.get(Action::Jump),
            Some(Button::R)
        );
        press(&mut app, &[Button::B], 7.0);
        assert!(app.screen == Screen::Settings);
        app.config_store.flush().unwrap();
        let saved = std::fs::read_to_string(dir.join("config/settings.json")).unwrap();
        assert!(saved.contains("\"jump\": \"r\""), "{saved}");
        // Reset requires confirmation.
        assert_eq!(app.cursor, settings_index(SettingRow::Controls));
        press(&mut app, &[Button::A], 7.1);
        app.cursor = Action::COUNT;
        press(&mut app, &[Button::A], 7.2);
        assert!(app.screen == Screen::ConfirmReset);
        press(&mut app, &[Button::A], 7.3);
        assert_eq!(
            app.config.controls.bindings.get(Action::Jump),
            Some(Button::R)
        );
        press(&mut app, &[Button::A], 7.4);
        app.cursor = 1;
        press(&mut app, &[Button::A], 7.5);
        assert_eq!(
            app.config.controls.bindings,
            crate::input::Bindings::default()
        );
        app.finish().unwrap();
        drop(app);
        let app = self::app(&dir, 42);
        assert_eq!(
            app.config.controls.bindings,
            crate::input::Bindings::default()
        );
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_adjust_with_dpad_and_hints_follow_bindings() {
        let dir = temp("settings");
        let mut app = app(&dir, 42);
        app.open(Screen::Settings, 0);
        assert_eq!(app.config.video.view_distance, 4);
        press(&mut app, &[Button::Right], 0.0);
        press(&mut app, &[Button::Right], 0.5);
        press(&mut app, &[Button::Right], 1.0);
        assert_eq!(app.config.video.view_distance, 6);
        app.cursor = settings_index(SettingRow::InvertY);
        press(&mut app, &[Button::A], 1.5);
        assert!(app.config.controls.invert_y);
        app.cursor = settings_index(SettingRow::SmoothLighting);
        press(&mut app, &[Button::A], 1.6);
        assert!(!app.config.video.smooth_lighting);
        assert!(app.hints().contains("R2 挖掘"));
        app.config
            .controls
            .bindings
            .bind(Action::Attack, Button::Menu);
        assert!(app.hints().contains("MENU 挖掘"));
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sounds_follow_materials_and_jumping_is_silent() {
        let dir = temp("sounds");
        let mut app = app(&dir, 42);
        // A flat grass strip east of the spawn point keeps the walk stable.
        let spawn = app.player.position.floor().as_ivec3();
        let floor = spawn.y - 1;
        for x in spawn.x - 8..spawn.x + 22 {
            for z in spawn.z - 3..spawn.z + 4 {
                app.world.set(glam::IVec3::new(x, floor, z), 1);
                for y in floor + 1..floor + 5 {
                    app.world.set(glam::IVec3::new(x, y, z), 0);
                }
            }
        }
        press(&mut app, &[Button::A], 0.0);
        assert_eq!(app.audio.played, [Sound::UiClick]);
        for i in 0..120 {
            app.update(&Frame::default(), 1.0 / 60.0, 0.1 + i as f64 / 60.0);
        }
        app.audio.played.clear();
        // A jump on flat ground: no sound when leaving, a step when landing.
        let floor = app.player.position.y;
        press(&mut app, &[Button::B], 3.0);
        assert!(app.player.position.y > floor);
        assert!(app.audio.played.is_empty(), "{:?}", app.audio.played);
        for i in 0..90 {
            app.update(&Frame::default(), 1.0 / 60.0, 3.1 + i as f64 / 60.0);
        }
        assert_eq!(
            app.audio.played,
            [Sound::Block(Kind::Step, Material::Grass)]
        );
        // Walking plays footsteps at a steady rate.
        app.audio.played.clear();
        let walk = Frame {
            movement: Vec2::new(0.0, -1.0),
            ..Default::default()
        };
        app.player.yaw = std::f32::consts::FRAC_PI_2;
        for i in 0..120 {
            app.update(&walk, 1.0 / 60.0, 5.0 + i as f64 / 60.0);
        }
        let steps = app.audio.played.len();
        assert!(
            (3..=8).contains(&steps),
            "{steps} steps in 2 s: {:?}",
            app.audio.played
        );
        // Digging and placing planks use their materials.
        app.audio.played.clear();
        app.player.pitch = -1.2;
        app.update(&Frame::default(), 1.0 / 60.0, 8.0);
        let hit = app.target().unwrap();
        let block = app.world.get(hit.block);
        app.update(&trigger(Button::R2), 1.0 / 60.0, 8.1);
        app.update(&Frame::default(), 1.0 / 60.0, 8.2);
        app.hotbar[app.slot] = block::OAK_PLANKS;
        app.update(&trigger(Button::L2), 1.0 / 60.0, 8.5);
        let played: Vec<_> = app
            .audio
            .played
            .iter()
            .filter(|s| !matches!(s, Sound::Block(Kind::Step, _)))
            .copied()
            .collect();
        assert_eq!(
            played,
            [
                Sound::Block(Kind::Break, world::material(block)),
                Sound::Block(Kind::Place, Material::Wood)
            ]
        );
        // Hotbar scrolling is silent; menus click.
        app.audio.played.clear();
        press(&mut app, &[Button::R], 9.0);
        assert!(app.audio.played.is_empty());
        press(&mut app, &[Button::Start], 9.1);
        press(&mut app, &[Button::Down], 9.2);
        press(&mut app, &[Button::B], 9.3);
        assert_eq!(
            app.audio.played,
            [Sound::UiClick, Sound::UiMove, Sound::UiBack]
        );
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn far_edits_persist_and_physics_waits_for_terrain() {
        let dir = temp("stream");
        std::fs::create_dir_all(dir.join("save")).unwrap();
        std::fs::write(dir.join("save/world.dat"), b"v1").unwrap();
        let mut app = app(&dir, 77);
        assert!(app.status.contains("world.v1.dat"), "{}", app.status);
        assert_eq!(std::fs::read(dir.join("save/world.v1.dat")).unwrap(), b"v1");
        app.config.video.view_distance = 2;
        press(&mut app, &[Button::A], 0.0);
        // Teleport far away: nothing is loaded there yet, so the player is
        // frozen even while pushing the stick.
        app.player.position = Vec3::new(-1000.5, 120.0, 2000.5);
        app.player.flying = false;
        let start = app.player.position;
        let walk = Frame {
            movement: Vec2::new(0.0, -1.0),
            ..Default::default()
        };
        app.update(&walk, 1.0 / 60.0, 1.0);
        assert_eq!(app.player.position, start);
        assert!(!app.terrain_ready());
        settle(&mut app, 1.1);
        assert!(app.terrain_ready());
        // Origin columns are far now and were unloaded.
        assert!(!app.world.loaded(glam::IVec2::ZERO));
        for i in 0..240 {
            app.update(&Frame::default(), 1.0 / 60.0, 2.0 + i as f64 / 60.0);
        }
        assert!(app.player.grounded, "fell onto far terrain");
        let p = app.player.position.floor().as_ivec3() + glam::IVec3::new(2, 0, 0);
        assert!(app.world.set(p, 8));
        app.finish().unwrap();
        assert!(dir.join("save/worlds/0/level.json").exists());
        drop(app);
        let mut app = self::app(&dir, 1);
        assert_eq!(app.world.seed, 77, "seed comes from the level");
        assert!(app.status.is_empty(), "{}", app.status);
        assert!(
            app.terrain_ready() || {
                app.update(&Frame::default(), 0.0, 0.0);
                app.terrain_ready()
            }
        );
        assert_eq!(app.world.get(p), 8, "edit survives reload");
        assert!(!app.world.column(world::column_of(p)).unwrap().dirty());
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rebuilding_keeps_a_backup_and_starts_fresh() {
        let dir = temp("rebuild");
        let mut app = app(&dir, 5);
        press(&mut app, &[Button::A], 0.0);
        let p = app.player.position.floor().as_ivec3() + glam::IVec3::new(1, 3, 0);
        assert!(app.world.set(p, 8));
        app.finish().unwrap();
        let old_seed = app.world.seed;
        press(&mut app, &[Button::Start], 1.0);
        app.cursor = 3;
        press(&mut app, &[Button::A], 1.1);
        assert!(app.screen == Screen::ConfirmNew);
        app.cursor = 1;
        press(&mut app, &[Button::A], 1.2);
        assert!(app.screen == Screen::Playing);
        assert_ne!(app.world.seed, old_seed);
        assert!(dir.join("save/worlds/0.bak/region").exists());
        app.finish().unwrap();
        drop(app);
        let app = self::app(&dir, 1);
        assert_ne!(app.world.seed, old_seed);
        assert_ne!(app.world.get(p), 8);
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn oriented_blocks_face_the_player_and_hotbar_persists() {
        let dir = temp("facing");
        let mut app = app(&dir, 9);
        press(&mut app, &[Button::A], 0.0);
        for i in 0..90 {
            app.update(&Frame::default(), 1.0 / 60.0, 0.1 + i as f64 / 60.0);
        }
        app.hotbar[2] = block::FURNACE;
        app.slot = 2;
        // Looking north and down at the ground in front.
        app.player.yaw = 0.0;
        app.player.pitch = -0.9;
        app.update(&Frame::default(), 1.0 / 60.0, 2.0);
        let hit = app.target().unwrap();
        app.update(&trigger(Button::L2), 1.0 / 60.0, 2.1);
        let placed = hit.block + hit.normal;
        assert_eq!(app.world.get(placed), block::FURNACE);
        assert_eq!(
            block::facing_face(app.world.state(placed)),
            block::Face::South,
            "front turns to the player"
        );
        app.finish().unwrap();
        drop(app);
        let app = self::app(&dir, 1);
        assert_eq!((app.slot, app.hotbar[2]), (2, block::FURNACE));
        assert_eq!(
            block::facing_face(app.world.state(placed)),
            block::Face::South
        );
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn showcase_lamps_light_the_floor() {
        let dir = temp("showcase");
        let mut app = app(&dir, 42);
        app.start_world();
        let base = app.player.position.floor().as_ivec3() - glam::IVec3::Y;
        app.world
            .generate_area(world::column_of(base + glam::IVec3::new(0, 0, -6)), 3);
        app.build_showcase();
        let at = |x, y, z| base + glam::IVec3::new(x, y, z);
        let near = app.world.light(at(1, 1, -7));
        let lava = app.world.light(at(-1, 1, -5));
        assert!(near.unwrap().1 >= 12, "{near:?}");
        let p = at(1, 0, -7);
        let key = world::section_of(p);
        let mesh = crate::mesh::Job::new(&app.world, key, 1, crate::mesh::STYLE_SMOOTH)
            .unwrap()
            .build();
        let local = (p - key * 16) * 16;
        let top: Vec<_> = mesh
            .vertices
            .iter()
            .filter(|v| {
                i32::from(v.pos[1]) == local.y + 16
                    && (i32::from(v.pos[0]) - local.x - 8).abs() <= 8
                    && (i32::from(v.pos[2]) - local.z - 8).abs() <= 8
            })
            .map(|v| v.info)
            .collect();
        assert!(!top.is_empty());
        assert!(top.iter().all(|info| info[2] >= 44), "{top:?}");
        assert!(lava.unwrap().1 >= 13, "{lava:?}");
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

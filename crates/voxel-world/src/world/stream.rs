//! Column streaming around the player.
//!
//! Columns within `view + 2` are requested: a section is meshed once its
//! 3×3 columns are lit, and lighting a column needs its 3×3 neighbours.
//! Clean columns beyond `view + 4` are dropped. Loading reads the region
//! store first and generates from the seed otherwise, on the worker pool.
use super::{Column, World, terrain};
use crate::{
    jobs::{Kind, Pool},
    save::region::RegionStore,
};
use glam::{IVec2, Vec2};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::Instant,
};

/// Hard cap on resident columns (view distance 6 keeps up to 21² = 441).
pub const MAX_COLUMNS: usize = 480;
/// Extra rings kept loaded beyond the request radius before unloading.
const HYSTERESIS: i32 = 2;

struct Loaded {
    column: Column,
    epoch: u64,
    from_disk: bool,
    ms: f32,
}

/// Generation statistics since the last report.
#[derive(Default, Clone, Copy, Debug)]
pub struct StreamStats {
    pub generated: u32,
    pub read: u32,
    pub gen_ms_sum: f32,
    pub gen_ms_max: f32,
}

/// Reads a stored column or generates it; never fails.
pub fn load_column(store: &RegionStore, seed: u64, pos: IVec2) -> (Column, bool) {
    match store.load(pos) {
        Some(column) => (column, true),
        None => (terrain::generate(seed, pos), false),
    }
}

pub fn request_radius(view: i32) -> i32 {
    view + 2
}
fn keep_radius(view: i32) -> i32 {
    request_radius(view) + HYSTERESIS
}

pub struct Stream {
    pool: Arc<Pool>,
    store: Arc<RegionStore>,
    sender: Sender<Loaded>,
    receiver: Receiver<Loaded>,
    pending: HashSet<IVec2>,
    epoch: u64,
    stats: StreamStats,
}

impl Stream {
    pub fn new(pool: Arc<Pool>, store: Arc<RegionStore>) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            pool,
            store,
            sender,
            receiver,
            pending: HashSet::new(),
            epoch: 0,
            stats: StreamStats::default(),
        }
    }
    pub fn store(&self) -> &Arc<RegionStore> {
        &self.store
    }
    /// Switches to another world; results for the previous one are dropped.
    pub fn reset(&mut self, store: Arc<RegionStore>, epoch: u64) {
        self.store = store;
        self.epoch = epoch;
        self.pending.clear();
        while self.receiver.try_recv().is_ok() {}
    }
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
    pub fn take_stats(&mut self) -> StreamStats {
        std::mem::take(&mut self.stats)
    }
    /// Loads the columns around `center` on the calling thread.
    pub fn prime(&mut self, world: &mut World, center: IVec2, r: i32) {
        for dz in -r..=r {
            for dx in -r..=r {
                let c = center + IVec2::new(dx, dz);
                if !world.loaded(c) {
                    let (column, _) = load_column(&self.store, world.seed, c);
                    world.insert(column);
                }
            }
        }
    }
    /// Inserts finished columns, drops far clean ones and requests missing
    /// ones. `facing` is the horizontal view direction for priorities.
    /// Returns how many dirty columns wait outside the keep radius.
    pub fn update(&mut self, world: &mut World, center: IVec2, facing: Vec2, view: i32) -> usize {
        let keep = keep_radius(view);
        let far = |c: IVec2| (c - center).abs().max_element() > keep;
        while let Ok(loaded) = self.receiver.try_recv() {
            let pos = loaded.column.pos;
            if loaded.epoch != self.epoch {
                continue;
            }
            self.pending.remove(&pos);
            if loaded.from_disk {
                self.stats.read += 1;
            } else {
                self.stats.generated += 1;
                self.stats.gen_ms_sum += loaded.ms;
                self.stats.gen_ms_max = self.stats.gen_ms_max.max(loaded.ms);
            }
            if !far(pos) && !world.loaded(pos) {
                world.insert(loaded.column);
            }
        }
        // Unload clean columns that are too far, then enforce the cap.
        let mut dirty_far = 0;
        let mut drop: Vec<IVec2> = Vec::new();
        for column in world.columns() {
            if far(column.pos) {
                if column.dirty() {
                    dirty_far += 1;
                } else {
                    drop.push(column.pos);
                }
            }
        }
        for c in drop {
            world.remove(c);
        }
        if world.len() > MAX_COLUMNS {
            let mut clean: Vec<_> = world
                .columns()
                .filter(|c| !c.dirty())
                .map(|c| ((c.pos - center).length_squared(), c.pos))
                .collect();
            clean.sort_unstable_by_key(|(d, c)| std::cmp::Reverse((*d, c.x, c.y)));
            for (_, c) in clean.into_iter().take(world.len() - MAX_COLUMNS) {
                world.remove(c);
            }
        }
        let budget = self.pool.active() * 2 + 2;
        if self.pending.len() >= budget {
            return dirty_far;
        }
        let r = request_radius(view);
        let mut wanted = Vec::new();
        for dz in -r..=r {
            for dx in -r..=r {
                let c = center + IVec2::new(dx, dz);
                if world.loaded(c)
                    || self.pending.contains(&c)
                    || world.len() + wanted.len() >= MAX_COLUMNS
                {
                    continue;
                }
                wanted.push((priority(IVec2::new(dx, dz), facing), c));
            }
        }
        wanted.sort_unstable_by_key(|(p, c)| (*p, c.x, c.y));
        for (p, c) in wanted
            .into_iter()
            .take((budget - self.pending.len()).min(4))
        {
            self.pending.insert(c);
            let (store, sender, seed, epoch) = (
                self.store.clone(),
                self.sender.clone(),
                world.seed,
                self.epoch,
            );
            self.pool.submit(Kind::Generate, p, move || {
                let start = Instant::now();
                let (column, from_disk) = load_column(&store, seed, c);
                let _ = sender.send(Loaded {
                    column,
                    epoch,
                    from_disk,
                    ms: start.elapsed().as_secs_f32() * 1000.0,
                });
            });
        }
        dirty_far
    }
}

/// Lower is sooner: distance squared, with columns behind the camera later.
pub fn priority(offset: IVec2, facing: Vec2) -> i64 {
    let d = i64::from(offset.length_squared());
    let behind = offset != IVec2::ZERO && offset.as_vec2().dot(facing) < 0.0;
    d * 2 + if behind { 6 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn settle(stream: &mut Stream, world: &mut World, center: IVec2, view: i32) {
        for _ in 0..2000 {
            stream.update(world, center, Vec2::Y, view);
            if stream.pending() == 0 && world.area_loaded(center, request_radius(view)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("streaming did not settle");
    }

    #[test]
    fn loads_square_and_unloads_with_hysteresis() {
        let pool = Arc::new(Pool::new(2, 2));
        let mut stream = Stream::new(pool, Arc::new(RegionStore::memory()));
        let mut world = World::new(3);
        settle(&mut stream, &mut world, IVec2::ZERO, 2);
        assert_eq!(world.len(), 81);
        // Moving two columns keeps everything (within keep radius 6).
        settle(&mut stream, &mut world, IVec2::new(2, 0), 2);
        assert!(world.loaded(IVec2::new(-3, 0)));
        // Moving further drops the far side but keeps a dirty column.
        assert!(world.set(glam::IVec3::new(-40, 100, 0), 1));
        settle(&mut stream, &mut world, IVec2::new(6, 0), 2);
        assert!(!world.loaded(IVec2::new(-3, 1)));
        assert!(
            world.loaded(IVec2::new(-3, 0)),
            "dirty column waits for save"
        );
        assert!(!world.loaded(IVec2::new(-1, 4)), "beyond the keep radius");
        let dirty = stream.update(&mut world, IVec2::new(6, 0), Vec2::Y, 2);
        assert_eq!(dirty, 1);
        world
            .columns_mut()
            .for_each(|c| c.saved_changes = c.changes);
        stream.update(&mut world, IVec2::new(6, 0), Vec2::Y, 2);
        assert!(!world.loaded(IVec2::new(-3, 0)));
    }

    #[test]
    fn stored_columns_win_over_generation_and_old_epochs_are_dropped() {
        let pool = Arc::new(Pool::new(1, 1));
        let store = Arc::new(RegionStore::memory());
        let mut edited = terrain::generate(3, IVec2::new(1, 0));
        edited.put(0, 120, 0, 8);
        store.store(&[(edited.pos, edited.encode())]).unwrap();
        let mut stream = Stream::new(pool, store.clone());
        let mut world = World::new(3);
        settle(&mut stream, &mut world, IVec2::ZERO, 1);
        assert_eq!(world.column(IVec2::new(1, 0)).unwrap().get(0, 120, 0), 8);
        let stats = stream.take_stats();
        assert_eq!((stats.read, stats.generated), (1, 48));
        // Results requested before a reset are ignored.
        let mut world = World::new(3);
        stream.update(&mut world, IVec2::new(50, 50), Vec2::Y, 1);
        stream.reset(store, 9);
        std::thread::sleep(Duration::from_millis(200));
        stream.update(&mut world, IVec2::new(-50, -50), Vec2::Y, 0);
        assert!(!world.loaded(IVec2::new(50, 50)));
    }

    #[test]
    fn nearest_and_front_first() {
        let f = Vec2::new(0.0, -1.0);
        assert!(priority(IVec2::ZERO, f) < priority(IVec2::new(0, -1), f));
        assert!(priority(IVec2::new(0, -2), f) < priority(IVec2::new(0, 2), f));
        assert!(priority(IVec2::new(0, -1), f) < priority(IVec2::new(0, -3), f));
    }
}

//! Sky light and block light.
//!
//! A column's light depends only on the blocks of the surrounding 3×3
//! columns: light fades by at least one level per block, so nothing from
//! further away can reach it. That makes the full computation a pure
//! function of a snapshot, run on the worker pool. Edits update light
//! incrementally on the game thread (removal flood followed by refill).
use super::{
    CHUNK, HEIGHT, World, block,
    chunk::{COLUMN_CELLS, FULL_SKY, SECTIONS, Section, light_index},
    column_of,
};
use crate::jobs::{Kind, Pool};
use glam::{IVec2, IVec3};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, OnceLock,
        mpsc::{self, Receiver, Sender},
    },
    time::Instant,
};

pub const MAX: u8 = 15;

/// Opacity and emission of every block id.
fn tables() -> &'static ([u8; 256], [u8; 256]) {
    static TABLES: OnceLock<([u8; 256], [u8; 256])> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut opacity = [MAX; 256];
        let mut emission = [0; 256];
        for id in 0..block::COUNT {
            let def = block::def(id as u8);
            opacity[id] = def.opacity;
            emission[id] = def.light;
        }
        opacity[0] = 0;
        (opacity, emission)
    })
}
pub fn opacity(id: u8) -> u8 {
    tables().0[usize::from(id)]
}
pub fn emission(id: u8) -> u8 {
    tables().1[usize::from(id)]
}

/// Blocks of the 3×3 columns around `pos`, in row order (z, then x).
pub struct Snapshot {
    pub pos: IVec2,
    sections: Vec<[Option<Arc<Section>>; SECTIONS]>,
    /// Change counters of the nine columns when copied.
    pub versions: [u64; 9],
}

impl Snapshot {
    /// `None` unless all nine columns are loaded.
    pub fn new(world: &World, pos: IVec2) -> Option<Self> {
        let mut sections = Vec::with_capacity(9);
        let mut versions = [0; 9];
        for (i, version) in versions.iter_mut().enumerate() {
            let column = world.column(pos + IVec2::new(i as i32 % 3 - 1, i as i32 / 3 - 1))?;
            sections.push(std::array::from_fn(|s| column.section_arc(s)));
            *version = column.changes;
        }
        Some(Self {
            pos,
            sections,
            versions,
        })
    }
    /// Whether the nine columns are unchanged since the snapshot.
    pub fn current(&self, world: &World) -> bool {
        self.versions.iter().enumerate().all(|(i, v)| {
            world
                .column(self.pos + IVec2::new(i as i32 % 3 - 1, i as i32 / 3 - 1))
                .is_some_and(|c| c.changes == *v)
        })
    }
}

/// Light fades by one level per block, so a cell 15 or more blocks outside
/// the column cannot reach it: a 14-block margin gives exact results.
const MARGIN: usize = 14;
const W: usize = 16 + 2 * MARGIN;

/// Full light of the centre column of `snapshot`.
pub fn compute(snapshot: &Snapshot) -> Vec<u8> {
    let (opacity_of, emission_of) = tables();
    let layer = W * W;
    // Highest block anywhere in the region; above it is open sky, and block
    // light can rise at most 14 more.
    let top_section = snapshot
        .sections
        .iter()
        .flat_map(|c| {
            c.iter()
                .enumerate()
                .filter(|(_, s)| s.is_some())
                .map(|(i, _)| i)
        })
        .max();
    let Some(top_section) = top_section else {
        return vec![FULL_SKY; COLUMN_CELLS];
    };
    let mut h = ((top_section + 1) * 16 + 16).min(HEIGHT as usize);
    let mut opacity = vec![0u8; layer * h];
    let mut emitters = Vec::new();
    let mut highest = 0;
    for (c, sections) in snapshot.sections.iter().enumerate() {
        // Region coordinates of this column's local 0; only part of the
        // outer columns lies inside the margin.
        let x0 = (c % 3) as isize * 16 - 16 + MARGIN as isize;
        let z0 = (c / 3) as isize * 16 - 16 + MARGIN as isize;
        let span = |o: isize| (-o).max(0) as usize..(W as isize - o).min(16) as usize;
        let (xs, zs) = (span(x0), span(z0));
        for (s, section) in sections.iter().enumerate() {
            let Some(section) = section else { continue };
            for ly in 0..16 {
                let y = s * 16 + ly;
                if y >= h {
                    break;
                }
                for lz in zs.clone() {
                    let src = &section.blocks[ly << 8 | lz << 4..][xs.clone()];
                    let start = y * layer
                        + (z0 + lz as isize) as usize * W
                        + (x0 + xs.start as isize) as usize;
                    let dst = &mut opacity[start..start + src.len()];
                    let mut any = 0u8;
                    for (d, &id) in dst.iter_mut().zip(src) {
                        *d = opacity_of[usize::from(id)];
                        any |= id;
                    }
                    if any == 0 {
                        continue;
                    }
                    highest = highest.max(y);
                    for (k, &id) in src.iter().enumerate() {
                        let e = emission_of[usize::from(id)];
                        if e > 0 {
                            emitters.push(((start + k) as u32, e));
                        }
                    }
                }
            }
        }
    }
    let lamp_reach = if emitters.is_empty() { 1 } else { 15 };
    h = h
        .min(highest + lamp_reach + 1)
        .max(highest + 2)
        .min(HEIGHT as usize);
    opacity.truncate(layer * h);
    opacity.resize(layer * h, 0);
    // Sky light: straight down, layer by layer, then flood sideways.
    let mut sky = vec![0u8; layer * h];
    let mut column = vec![MAX; layer];
    // Highest non-clear cell per (x, z); above it everything is full sky.
    let mut top = vec![0usize; layer];
    for y in (0..h).rev() {
        let base = y * layer;
        for (i, s) in column.iter_mut().enumerate() {
            if *s == 0 {
                continue;
            }
            let o = opacity[base + i];
            if o > 0 && top[i] == 0 {
                top[i] = y;
            }
            *s = if o >= MAX { 0 } else { s.saturating_sub(o) };
            sky[base + i] = *s;
        }
    }
    // A cell can only seed the flood if it or a neighbour is below its
    // column's top.
    let ceiling: Vec<usize> = (0..layer)
        .map(|i| {
            let (x, z) = (i % W, i / W);
            let mut c = top[i];
            if x > 0 {
                c = c.max(top[i - 1]);
            }
            if x + 1 < W {
                c = c.max(top[i + 1]);
            }
            if z > 0 {
                c = c.max(top[i - W]);
            }
            if z + 1 < W {
                c = c.max(top[i + W]);
            }
            c + 1
        })
        .collect();
    let mut queue: VecDeque<u32> = VecDeque::with_capacity(4096);
    // Above the highest block every cell and its neighbours are full sky.
    for y in 0..(highest + 2).min(h) {
        for z in 0..W {
            for x in 0..W {
                let i = y * layer + z * W + x;
                let s = sky[i];
                if s <= 1 || y > ceiling[z * W + x] {
                    continue;
                }
                let lower = |j: usize| sky[j] + 1 < s && opacity[j] < MAX;
                if (x > 0 && lower(i - 1))
                    || (x + 1 < W && lower(i + 1))
                    || (z > 0 && lower(i - W))
                    || (z + 1 < W && lower(i + W))
                    || (y > 0 && lower(i - layer))
                    || (y + 1 < h && lower(i + layer))
                {
                    queue.push_back(i as u32);
                }
            }
        }
    }
    flood(&mut sky, &opacity, h, &mut queue, true);
    let mut light = Vec::new();
    if !emitters.is_empty() {
        light = vec![0u8; layer * h];
        for &(i, e) in &emitters {
            light[i as usize] = e;
            queue.push_back(i);
        }
        flood(&mut light, &opacity, h, &mut queue, false);
    }
    let mut out = vec![FULL_SKY; COLUMN_CELLS];
    for y in 0..h {
        for z in 0..16 {
            let row = y * layer + (z + MARGIN) * W + MARGIN;
            for x in 0..16 {
                let i = row + x;
                let block = light.get(i).copied().unwrap_or(0);
                out[light_index(x as i32, y as i32, z as i32)] = sky[i] << 4 | block;
            }
        }
    }
    out
}

/// Breadth-first spread inside the snapshot region.
fn flood(level: &mut [u8], opacity: &[u8], h: usize, queue: &mut VecDeque<u32>, sky: bool) {
    let layer = W * W;
    while let Some(i) = queue.pop_front() {
        let i = i as usize;
        let l = level[i];
        if l <= 1 {
            continue;
        }
        let (x, z, y) = (i % W, (i / W) % W, i / layer);
        let mut visit = |j: usize, down: bool| {
            let o = opacity[j];
            if o >= MAX {
                return;
            }
            let next = if sky && down && l == MAX && o == 0 {
                MAX
            } else {
                l.saturating_sub(o.max(1))
            };
            if level[j] < next {
                level[j] = next;
                queue.push_back(j as u32);
            }
        };
        if x > 0 {
            visit(i - 1, false);
        }
        if x + 1 < W {
            visit(i + 1, false);
        }
        if z > 0 {
            visit(i - W, false);
        }
        if z + 1 < W {
            visit(i + W, false);
        }
        if y > 0 {
            visit(i - layer, true);
        }
        if y + 1 < h {
            visit(i + layer, false);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    Sky,
    Block,
}
impl Channel {
    fn get(self, packed: u8) -> u8 {
        match self {
            Channel::Sky => packed >> 4,
            Channel::Block => packed & 15,
        }
    }
    fn put(self, packed: u8, value: u8) -> u8 {
        match self {
            Channel::Sky => (packed & 15) | value << 4,
            Channel::Block => (packed & 0xF0) | value,
        }
    }
}

const DIRECTIONS: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Z,
    IVec3::NEG_Z,
    IVec3::Y,
    IVec3::NEG_Y,
];

/// Packed light at `p`: full sky above the world, `None` below it or in
/// columns that are unloaded or not lit yet.
pub fn get(world: &World, p: IVec3) -> Option<u8> {
    if p.y >= HEIGHT {
        return Some(FULL_SKY);
    }
    if p.y < 0 {
        return None;
    }
    let column = world.columns.get(&column_of(p))?;
    column
        .lit
        .then(|| column.light(p.x.rem_euclid(CHUNK), p.y, p.z.rem_euclid(CHUNK)))
}

fn set(world: &mut World, p: IVec3, value: u8, changed: &mut Vec<IVec3>) {
    if !(0..HEIGHT).contains(&p.y) {
        return;
    }
    if let Some(column) = world.columns.get_mut(&column_of(p))
        && column.lit
        && column.set_light(p.x.rem_euclid(CHUNK), p.y, p.z.rem_euclid(CHUNK), value)
    {
        changed.push(p);
    }
}

fn relight(world: &mut World, p: IVec3, channel: Channel, changed: &mut Vec<IVec3>) {
    let level = |w: &World, q: IVec3| get(w, q).map(|v| channel.get(v));
    let write = |w: &mut World, q: IVec3, l: u8, changed: &mut Vec<IVec3>| {
        if let Some(v) = get(w, q) {
            set(w, q, channel.put(v, l), changed);
        }
    };
    let mut remove = VecDeque::new();
    let mut add = VecDeque::new();
    if let Some(old) = level(world, p) {
        write(world, p, 0, changed);
        remove.push_back((p, old));
    }
    while let Some((q, l)) = remove.pop_front() {
        for d in DIRECTIONS {
            let n = q + d;
            if n.y >= HEIGHT {
                add.push_back(n);
                continue;
            }
            let Some(ln) = level(world, n) else { continue };
            if ln == 0 {
                continue;
            }
            let direct = channel == Channel::Sky && d == IVec3::NEG_Y && l == MAX && ln == MAX;
            if ln < l || direct {
                write(world, n, 0, changed);
                remove.push_back((n, ln));
            } else {
                add.push_back(n);
            }
        }
    }
    if channel == Channel::Block {
        let e = emission(world.get(p));
        if e > 0 {
            write(world, p, e, changed);
            add.push_back(p);
        }
    }
    for d in DIRECTIONS {
        add.push_back(p + d);
    }
    while let Some(q) = add.pop_front() {
        let Some(l) = level(world, q) else { continue };
        if l <= 1 {
            continue;
        }
        for d in DIRECTIONS {
            let n = q + d;
            if n.y >= HEIGHT {
                continue;
            }
            let Some(ln) = level(world, n) else { continue };
            let o = opacity(world.get(n));
            if o >= MAX {
                continue;
            }
            let next = if channel == Channel::Sky && d == IVec3::NEG_Y && l == MAX && o == 0 {
                MAX
            } else {
                l.saturating_sub(o.max(1))
            };
            if ln < next {
                write(world, n, next, changed);
                add.push_back(n);
            }
        }
    }
}

/// Updates light after the block at `p` changed; returns changed cells.
pub fn block_changed(world: &mut World, p: IVec3) -> Vec<IVec3> {
    let mut changed = Vec::new();
    if get(world, p).is_none() {
        return changed;
    }
    relight(world, p, Channel::Sky, &mut changed);
    relight(world, p, Channel::Block, &mut changed);
    changed
}

/// Sections whose meshes read any of `cells` (meshes sample a one-block
/// halo for smooth lighting).
pub fn affected_sections(cells: &[IVec3]) -> HashSet<IVec3> {
    let mut keys = HashSet::new();
    for c in cells {
        let range = |v: i32| {
            let (a, b) = ((v - 1).div_euclid(CHUNK), (v + 1).div_euclid(CHUNK));
            a..=b
        };
        for kx in range(c.x) {
            for ky in range(c.y) {
                if !(0..SECTIONS as i32).contains(&ky) {
                    continue;
                }
                for kz in range(c.z) {
                    keys.insert(IVec3::new(kx, ky, kz));
                }
            }
        }
    }
    keys
}

struct Lit {
    snapshot: Snapshot,
    light: Vec<u8>,
    epoch: u64,
    ms: f32,
}

/// Schedules full light computations for loaded columns.
pub struct Lighter {
    pool: Arc<Pool>,
    sender: Sender<Lit>,
    receiver: Receiver<Lit>,
    pending: HashMap<IVec2, ()>,
    epoch: u64,
    pub computed: u32,
    pub ms_sum: f32,
}

impl Lighter {
    pub fn new(pool: Arc<Pool>) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            pool,
            sender,
            receiver,
            pending: HashMap::new(),
            epoch: 0,
            computed: 0,
            ms_sum: 0.0,
        }
    }
    pub fn reset(&mut self, epoch: u64) {
        self.epoch = epoch;
        self.pending.clear();
        while self.receiver.try_recv().is_ok() {}
    }
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
    /// Columns lit and their average compute time since the last call.
    pub fn take_stats(&mut self) -> (u32, f32) {
        let n = std::mem::take(&mut self.computed);
        let ms = std::mem::take(&mut self.ms_sum);
        (n, if n > 0 { ms / n as f32 } else { 0.0 })
    }
    /// Installs finished light and requests it for unlit columns whose
    /// neighbours are loaded, nearest first.
    pub fn update(&mut self, world: &mut World, center: IVec2) {
        while let Ok(lit) = self.receiver.try_recv() {
            if lit.epoch != self.epoch {
                continue;
            }
            let pos = lit.snapshot.pos;
            self.pending.remove(&pos);
            // Edits since the snapshot invalidate the result; retry later.
            if !lit.snapshot.current(world) {
                continue;
            }
            self.computed += 1;
            self.ms_sum += lit.ms;
            let revisions: Vec<u64> = (0..SECTIONS).map(|_| world.bump()).collect();
            if let Some(column) = world.columns.get_mut(&pos) {
                column.replace_light(lit.light);
                column.revisions.copy_from_slice(&revisions);
            }
        }
        let budget = self.pool.active() * 2 + 2;
        if self.pending.len() >= budget {
            return;
        }
        let mut wanted: Vec<_> = world
            .columns()
            .filter(|c| !c.lit && !self.pending.contains_key(&c.pos))
            .map(|c| ((c.pos - center).length_squared(), c.pos))
            .collect();
        wanted.sort_unstable_by_key(|(d, c)| (*d, c.x, c.y));
        for (d, pos) in wanted {
            if self.pending.len() >= budget {
                break;
            }
            let Some(snapshot) = Snapshot::new(world, pos) else {
                continue;
            };
            self.pending.insert(pos, ());
            let (sender, epoch) = (self.sender.clone(), self.epoch);
            self.pool
                .submit(Kind::Light, i64::from(d) * 2 + 1, move || {
                    let start = Instant::now();
                    let light = compute(&snapshot);
                    let _ = sender.send(Lit {
                        snapshot,
                        light,
                        epoch,
                        ms: start.elapsed().as_secs_f32() * 1000.0,
                    });
                });
        }
    }
    /// Computes light for every column whose neighbours are loaded, on the
    /// calling thread (tests and the first frames of a world).
    pub fn light_now(world: &mut World) {
        let ready: Vec<_> = world.columns().filter(|c| !c.lit).map(|c| c.pos).collect();
        for pos in ready {
            if let Some(snapshot) = Snapshot::new(world, pos) {
                let light = compute(&snapshot);
                if let Some(column) = world.columns.get_mut(&pos) {
                    column.replace_light(light);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::{GLASS, GLOWSTONE, OAK_LEAVES, STONE};

    /// Stone floor at y = 40 over 5×5 columns; the middle 3×3 are lit.
    fn flat() -> World {
        let mut w = World::new(0);
        for dz in -2..=2 {
            for dx in -2..=2 {
                let mut column = crate::world::Column::empty(IVec2::new(dx, dz));
                for z in 0..16 {
                    for x in 0..16 {
                        column.put(x, 40, z, STONE);
                    }
                }
                w.insert(column);
            }
        }
        Lighter::light_now(&mut w);
        w
    }
    fn sky(w: &World, p: IVec3) -> u8 {
        get(w, p).unwrap() >> 4
    }
    fn blk(w: &World, p: IVec3) -> u8 {
        get(w, p).unwrap() & 15
    }
    /// Recomputes everything from scratch for comparison.
    fn reference(w: &World) -> World {
        let mut r = World::new(0);
        for c in w.columns() {
            let mut copy = c.clone();
            copy.lit = false;
            r.insert(copy);
        }
        Lighter::light_now(&mut r);
        r
    }
    fn assert_matches_reference(w: &World, area: std::ops::RangeInclusive<i32>) {
        let r = reference(w);
        for y in 30..60 {
            for z in area.clone() {
                for x in area.clone() {
                    let p = IVec3::new(x, y, z);
                    assert_eq!(get(w, p), get(&r, p), "at {p}");
                }
            }
        }
    }

    #[test]
    fn open_sky_roofs_and_falloff() {
        let mut w = flat();
        let p = IVec3::new(5, 41, 5);
        assert_eq!(sky(&w, p), 15);
        assert_eq!(sky(&w, IVec3::new(5, 40, 5)), 0, "inside stone");
        assert_eq!(sky(&w, IVec3::new(5, 39, 5)), 0, "below the floor");
        // A 5×5 roof at y = 44 darkens the centre below it by distance.
        for z in 3..8 {
            for x in 3..8 {
                w.set(IVec3::new(x, 44, z), STONE);
            }
        }
        assert_eq!(sky(&w, p), 12, "two blocks in from the edge and one down");
        assert_eq!(sky(&w, IVec3::new(5, 43, 3)), 14);
        assert_matches_reference(&w, -2..=14);
        // Leaves only filter one level per block.
        w.set(IVec3::new(10, 45, 10), OAK_LEAVES);
        assert_eq!(sky(&w, IVec3::new(10, 44, 10)), 14);
        assert_matches_reference(&w, -2..=14);
    }

    #[test]
    fn block_light_spreads_and_is_removed() {
        let mut w = flat();
        let lamp = IVec3::new(8, 41, 8);
        assert!(w.set(lamp, GLOWSTONE));
        assert_eq!(blk(&w, lamp), 15);
        assert_eq!(blk(&w, lamp + IVec3::X), 14);
        assert_eq!(blk(&w, lamp + IVec3::new(3, 2, -1)), 9);
        // Crosses into the neighbouring column.
        assert_eq!(blk(&w, IVec3::new(-2, 41, 8)), 5);
        assert_matches_reference(&w, -6..=22);
        // Walls block it; glass does not.
        w.set(lamp + IVec3::new(2, 0, 0), STONE);
        assert_eq!(blk(&w, lamp + IVec3::new(2, 0, 0)), 0);
        assert_eq!(blk(&w, lamp + IVec3::new(3, 0, 0)), 10, "around the wall");
        w.set(lamp + IVec3::new(0, 0, 2), GLASS);
        assert_eq!(blk(&w, lamp + IVec3::new(0, 0, 2)), 13);
        assert_matches_reference(&w, -6..=22);
        assert!(w.set(lamp, 0));
        assert!((0..30).all(|d| blk(&w, lamp + IVec3::new(d - 15, 0, 0)) == 0));
        assert_matches_reference(&w, -6..=22);
    }

    #[test]
    fn digging_down_lets_sky_in_and_affected_sections_cover_halo() {
        let mut w = flat();
        for y in (30..41).rev() {
            w.set(IVec3::new(4, y, 4), 0);
        }
        // Not stone below 40 in this world: the shaft is lit straight down.
        assert_eq!(sky(&w, IVec3::new(4, 30, 4)), 15);
        assert_matches_reference(&w, -4..=12);
        let keys = affected_sections(&[IVec3::new(15, 16, 0)]);
        assert!(keys.contains(&IVec3::new(0, 1, 0)));
        assert!(keys.contains(&IVec3::new(1, 0, -1)));
        assert_eq!(keys.len(), 8);
        assert_eq!(affected_sections(&[IVec3::new(8, 8, 8)]).len(), 1);
    }

    #[test]
    fn unlit_columns_are_skipped_and_scheduler_installs_results() {
        let mut w = World::new(0);
        for dz in -2..=2 {
            for dx in -2..=2 {
                w.insert(crate::world::Column::empty(IVec2::new(dx, dz)));
            }
        }
        assert!(get(&w, IVec3::new(3, 50, 3)).is_none());
        assert!(block_changed(&mut w, IVec3::new(3, 50, 3)).is_empty());
        let mut lighter = Lighter::new(Arc::new(Pool::new(2, 2)));
        for _ in 0..500 {
            lighter.update(&mut w, IVec2::ZERO);
            if w.columns().filter(|c| c.lit).count() == 9 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Only columns with all neighbours loaded can be lit.
        assert_eq!(w.columns().filter(|c| c.lit).count(), 9);
        assert!(w.column(IVec2::new(2, 2)).is_some_and(|c| !c.lit));
        assert_eq!(get(&w, IVec3::new(3, 50, 3)), Some(FULL_SKY));
        let (n, ms) = lighter.take_stats();
        assert!(n == 9 && ms >= 0.0);
        assert_eq!(lighter.take_stats().0, 0);
        // A result computed before an edit is discarded.
        let mut w2 = World::new(0);
        w2.empty_area(IVec2::ZERO, 2);
        let snapshot = Snapshot::new(&w2, IVec2::ZERO).unwrap();
        w2.set(IVec3::new(1, 1, 1), STONE);
        assert!(!snapshot.current(&w2));
    }

    #[test]
    #[ignore = "timing probe"]
    fn timing_probe() {
        let mut w = World::new(42);
        for dz in -1..=1 {
            for dx in -1..=1 {
                w.insert(crate::world::terrain::generate(42, IVec2::new(dx, dz)));
            }
        }
        let snapshot = Snapshot::new(&w, IVec2::ZERO).unwrap();
        let start = Instant::now();
        for _ in 0..20 {
            std::hint::black_box(compute(&snapshot));
        }
        eprintln!(
            "compute: {:.2} ms",
            start.elapsed().as_secs_f64() * 1000.0 / 20.0
        );
    }
}

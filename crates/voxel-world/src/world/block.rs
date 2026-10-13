//! Data-driven block registry.
//!
//! Ids 0–9 match the first release so existing region files stay valid;
//! new blocks are only ever appended. Names and textures are original.
// The full block set is defined up front; mining, lighting, fluids and
// crafting read the remaining properties as they are implemented.
#![allow(dead_code)]
use crate::audio::Material;

/// Texture tiles; each is one layer of the block texture array.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(u8)]
pub enum Tex {
    GrassTop,
    GrassSide,
    SnowyGrassSide,
    Dirt,
    Stone,
    Cobblestone,
    MossyCobblestone,
    Bedrock,
    Sand,
    SandstoneTop,
    SandstoneSide,
    SandstoneBottom,
    Gravel,
    Clay,
    Snow,
    Ice,
    Water,
    Lava,
    Obsidian,
    CoalOre,
    IronOre,
    GoldOre,
    DiamondOre,
    OakLog,
    OakLogTop,
    BirchLog,
    BirchLogTop,
    SpruceLog,
    SpruceLogTop,
    OakLeaves,
    BirchLeaves,
    SpruceLeaves,
    OakPlanks,
    BirchPlanks,
    SprucePlanks,
    CraftingTop,
    CraftingSide,
    CraftingFront,
    FurnaceFront,
    FurnaceFrontLit,
    FurnaceSide,
    FurnaceTop,
    ChestTop,
    ChestSide,
    ChestFront,
    Glass,
    Bricks,
    StoneBricks,
    WoolWhite,
    WoolRed,
    WoolYellow,
    WoolBlue,
    WoolGreen,
    WoolBlack,
    Glowstone,
    Torch,
    OakSapling,
    BirchSapling,
    SpruceSapling,
    TallGrass,
    Dandelion,
    Poppy,
    CactusTop,
    CactusSide,
    CactusBottom,
    SugarCane,
    Farmland,
    FarmlandWet,
    Wheat0,
    Wheat1,
    Wheat2,
    Wheat3,
    Ladder,
    DoorLower,
    DoorUpper,
    BedHead,
    BedFoot,
    BedSide,
    IronBlock,
    GoldBlock,
    DiamondBlock,
    Bookshelf,
}
pub const TEX_COUNT: usize = Tex::Bookshelf as usize + 1;

impl Tex {
    pub fn all() -> impl Iterator<Item = Tex> {
        // SAFETY: Tex is a fieldless #[repr(u8)] enum with contiguous
        // discriminants 0..TEX_COUNT.
        (0..TEX_COUNT as u8).map(|i| unsafe { std::mem::transmute::<u8, Tex>(i) })
    }
    pub fn layer(self) -> u32 {
        self as u32
    }
}

/// How a block is drawn and whether it hides neighbouring faces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Render {
    Air,
    /// Full opaque cube.
    Opaque,
    /// Cube with transparent holes; drawn opaque with "fast leaves".
    Leaves,
    /// Cube with fully transparent pixels (glass).
    Cutout,
    /// Semi-transparent cube (ice).
    Translucent,
    Liquid,
    /// Two crossed quads (plants).
    Cross,
    Torch,
    Cactus,
    Ladder,
    Door,
    Bed,
    /// Thin layer on top of a block (snow).
    Layer,
    Farmland,
    Crop,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    None,
    Pickaxe,
    Axe,
    Shovel,
    Shears,
}

#[derive(Clone, Copy, Debug)]
pub enum Faces {
    All(Tex),
    /// Top, side, bottom.
    Column(Tex, Tex, Tex),
    /// Front, side, top, bottom; the front follows the facing state.
    Front(Tex, Tex, Tex, Tex),
}

/// Cube faces in mesher order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    East,
    West,
    Up,
    Down,
    South,
    North,
}
impl Face {
    /// Face from an axis (0 = x, 1 = y, 2 = z) and the sign of its normal.
    pub fn from_axis(axis: usize, positive: bool) -> Self {
        match (axis, positive) {
            (0, true) => Face::East,
            (0, false) => Face::West,
            (1, true) => Face::Up,
            (1, false) => Face::Down,
            (_, true) => Face::South,
            (_, false) => Face::North,
        }
    }
}

/// Horizontal facing stored in the low two bits of the state byte.
pub fn facing_face(state: u8) -> Face {
    match state & 3 {
        0 => Face::North,
        1 => Face::East,
        2 => Face::South,
        _ => Face::West,
    }
}
/// Facing for a block placed by a player looking along `yaw`: the front
/// turns towards the player.
pub fn facing_towards_player(yaw: f32) -> u8 {
    let quarter = (yaw / std::f32::consts::FRAC_PI_2).round() as i32;
    // yaw 0 looks north (−Z), so the front faces south.
    match quarter.rem_euclid(4) {
        0 => 2,
        1 => 3,
        2 => 0,
        _ => 1,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BlockDef {
    pub key: &'static str,
    pub name: &'static str,
    pub render: Render,
    pub faces: Faces,
    pub material: Option<Material>,
    /// Seconds-scale hardness; negative values cannot be broken.
    pub hardness: f32,
    pub tool: Tool,
    /// Minimum tool tier needed for a drop (0 = any).
    pub tier: u8,
    pub light: u8,
    /// Light lost when passing through, 0–15.
    pub opacity: u8,
    /// Offered in the creative palette.
    pub creative: bool,
}

const fn block(
    key: &'static str,
    name: &'static str,
    render: Render,
    faces: Faces,
    material: Material,
    hardness: f32,
) -> BlockDef {
    let opacity = match render {
        Render::Opaque => 15,
        Render::Leaves | Render::Translucent => 1,
        Render::Liquid => 2,
        _ => 0,
    };
    BlockDef {
        key,
        name,
        render,
        faces,
        material: Some(material),
        hardness,
        tool: Tool::None,
        tier: 0,
        light: 0,
        opacity,
        // Blocks that need special placement (two-block beds, crops on
        // farmland) are offered once their mechanics exist.
        creative: !matches!(
            render,
            Render::Air | Render::Bed | Render::Farmland | Render::Crop
        ),
    }
}
impl BlockDef {
    const fn tool(mut self, tool: Tool, tier: u8) -> Self {
        self.tool = tool;
        self.tier = tier;
        self
    }
    const fn light(mut self, light: u8) -> Self {
        self.light = light;
        self
    }
    const fn hidden(mut self) -> Self {
        self.creative = false;
        self
    }
    const fn silent(mut self) -> Self {
        self.material = None;
        self
    }
    const fn opacity(mut self, opacity: u8) -> Self {
        self.opacity = opacity;
        self
    }
}

pub const AIR: u8 = 0;
pub const GRASS: u8 = 1;
pub const DIRT: u8 = 2;
pub const STONE: u8 = 3;
pub const SAND: u8 = 4;
pub const OAK_LOG: u8 = 5;
pub const OAK_LEAVES: u8 = 6;
pub const OAK_PLANKS: u8 = 7;
pub const BRICKS: u8 = 8;
pub const BEDROCK: u8 = 9;
pub const COBBLESTONE: u8 = 10;
pub const MOSSY_COBBLESTONE: u8 = 11;
pub const GRAVEL: u8 = 12;
pub const SANDSTONE: u8 = 13;
pub const CLAY: u8 = 14;
pub const SNOW: u8 = 15;
pub const SNOWY_GRASS: u8 = 16;
pub const ICE: u8 = 17;
pub const WATER: u8 = 18;
pub const LAVA: u8 = 19;
pub const OBSIDIAN: u8 = 20;
pub const COAL_ORE: u8 = 21;
pub const IRON_ORE: u8 = 22;
pub const GOLD_ORE: u8 = 23;
pub const DIAMOND_ORE: u8 = 24;
pub const BIRCH_LOG: u8 = 25;
pub const BIRCH_LEAVES: u8 = 26;
pub const BIRCH_PLANKS: u8 = 27;
pub const SPRUCE_LOG: u8 = 28;
pub const SPRUCE_LEAVES: u8 = 29;
pub const SPRUCE_PLANKS: u8 = 30;
pub const CRAFTING_TABLE: u8 = 31;
pub const FURNACE: u8 = 32;
pub const FURNACE_LIT: u8 = 33;
pub const CHEST: u8 = 34;
pub const GLASS: u8 = 35;
pub const STONE_BRICKS: u8 = 36;
pub const WHITE_WOOL: u8 = 37;
pub const RED_WOOL: u8 = 38;
pub const YELLOW_WOOL: u8 = 39;
pub const BLUE_WOOL: u8 = 40;
pub const GREEN_WOOL: u8 = 41;
pub const BLACK_WOOL: u8 = 42;
pub const GLOWSTONE: u8 = 43;
pub const IRON_BLOCK: u8 = 44;
pub const GOLD_BLOCK: u8 = 45;
pub const DIAMOND_BLOCK: u8 = 46;
pub const BOOKSHELF: u8 = 47;
pub const TORCH: u8 = 48;
pub const OAK_SAPLING: u8 = 49;
pub const BIRCH_SAPLING: u8 = 50;
pub const SPRUCE_SAPLING: u8 = 51;
pub const TALL_GRASS: u8 = 52;
pub const DANDELION: u8 = 53;
pub const POPPY: u8 = 54;
pub const CACTUS: u8 = 55;
pub const SUGAR_CANE: u8 = 56;
pub const FARMLAND: u8 = 57;
pub const WHEAT: u8 = 58;
pub const LADDER: u8 = 59;
pub const DOOR: u8 = 60;
pub const BED: u8 = 61;
pub const SNOW_LAYER: u8 = 62;

use Faces::{All, Column, Front};
use Material as M;
use Render as R;
use Tex as T;
use Tool::{Axe, Pickaxe, Shears, Shovel};

pub static BLOCKS: [BlockDef; 63] = [
    block("air", "空气", R::Air, All(T::Stone), M::Stone, 0.0)
        .silent()
        .hidden(),
    block(
        "grass",
        "草方块",
        R::Opaque,
        Column(T::GrassTop, T::GrassSide, T::Dirt),
        M::Grass,
        0.6,
    )
    .tool(Shovel, 0),
    block("dirt", "泥土", R::Opaque, All(T::Dirt), M::Dirt, 0.5).tool(Shovel, 0),
    block("stone", "石头", R::Opaque, All(T::Stone), M::Stone, 1.5).tool(Pickaxe, 1),
    block("sand", "沙子", R::Opaque, All(T::Sand), M::Sand, 0.5).tool(Shovel, 0),
    block(
        "oak_log",
        "橡木原木",
        R::Opaque,
        Column(T::OakLogTop, T::OakLog, T::OakLogTop),
        M::Wood,
        2.0,
    )
    .tool(Axe, 0),
    block(
        "oak_leaves",
        "橡树树叶",
        R::Leaves,
        All(T::OakLeaves),
        M::Plant,
        0.2,
    )
    .tool(Shears, 0),
    block(
        "oak_planks",
        "橡木木板",
        R::Opaque,
        All(T::OakPlanks),
        M::Wood,
        2.0,
    )
    .tool(Axe, 0),
    block("bricks", "红砖块", R::Opaque, All(T::Bricks), M::Stone, 2.0).tool(Pickaxe, 1),
    block(
        "bedrock",
        "基岩",
        R::Opaque,
        All(T::Bedrock),
        M::Stone,
        -1.0,
    )
    .hidden(),
    block(
        "cobblestone",
        "圆石",
        R::Opaque,
        All(T::Cobblestone),
        M::Stone,
        2.0,
    )
    .tool(Pickaxe, 1),
    block(
        "mossy_cobblestone",
        "苔石",
        R::Opaque,
        All(T::MossyCobblestone),
        M::Stone,
        2.0,
    )
    .tool(Pickaxe, 1),
    block("gravel", "砂砾", R::Opaque, All(T::Gravel), M::Gravel, 0.6).tool(Shovel, 0),
    block(
        "sandstone",
        "砂岩",
        R::Opaque,
        Column(T::SandstoneTop, T::SandstoneSide, T::SandstoneBottom),
        M::Stone,
        0.8,
    )
    .tool(Pickaxe, 1),
    block("clay", "黏土块", R::Opaque, All(T::Clay), M::Dirt, 0.6).tool(Shovel, 0),
    block("snow", "雪块", R::Opaque, All(T::Snow), M::Snow, 0.2).tool(Shovel, 0),
    block(
        "snowy_grass",
        "雪地草方块",
        R::Opaque,
        Column(T::Snow, T::SnowyGrassSide, T::Dirt),
        M::Snow,
        0.6,
    )
    .tool(Shovel, 0),
    block("ice", "冰", R::Translucent, All(T::Ice), M::Glass, 0.5).tool(Pickaxe, 0),
    // Sunlight fades one level per block of water, so shallow seas stay lit.
    block("water", "水", R::Liquid, All(T::Water), M::Stone, -1.0)
        .silent()
        .opacity(1),
    block("lava", "岩浆", R::Liquid, All(T::Lava), M::Stone, -1.0)
        .silent()
        .light(15),
    block(
        "obsidian",
        "黑曜石",
        R::Opaque,
        All(T::Obsidian),
        M::Stone,
        50.0,
    )
    .tool(Pickaxe, 4),
    block(
        "coal_ore",
        "煤矿石",
        R::Opaque,
        All(T::CoalOre),
        M::Stone,
        3.0,
    )
    .tool(Pickaxe, 1),
    block(
        "iron_ore",
        "铁矿石",
        R::Opaque,
        All(T::IronOre),
        M::Stone,
        3.0,
    )
    .tool(Pickaxe, 2),
    block(
        "gold_ore",
        "金矿石",
        R::Opaque,
        All(T::GoldOre),
        M::Stone,
        3.0,
    )
    .tool(Pickaxe, 3),
    block(
        "diamond_ore",
        "钻石矿石",
        R::Opaque,
        All(T::DiamondOre),
        M::Stone,
        3.0,
    )
    .tool(Pickaxe, 3),
    block(
        "birch_log",
        "白桦原木",
        R::Opaque,
        Column(T::BirchLogTop, T::BirchLog, T::BirchLogTop),
        M::Wood,
        2.0,
    )
    .tool(Axe, 0),
    block(
        "birch_leaves",
        "白桦树叶",
        R::Leaves,
        All(T::BirchLeaves),
        M::Plant,
        0.2,
    )
    .tool(Shears, 0),
    block(
        "birch_planks",
        "白桦木板",
        R::Opaque,
        All(T::BirchPlanks),
        M::Wood,
        2.0,
    )
    .tool(Axe, 0),
    block(
        "spruce_log",
        "云杉原木",
        R::Opaque,
        Column(T::SpruceLogTop, T::SpruceLog, T::SpruceLogTop),
        M::Wood,
        2.0,
    )
    .tool(Axe, 0),
    block(
        "spruce_leaves",
        "云杉树叶",
        R::Leaves,
        All(T::SpruceLeaves),
        M::Plant,
        0.2,
    )
    .tool(Shears, 0),
    block(
        "spruce_planks",
        "云杉木板",
        R::Opaque,
        All(T::SprucePlanks),
        M::Wood,
        2.0,
    )
    .tool(Axe, 0),
    block(
        "crafting_table",
        "工作台",
        R::Opaque,
        Front(
            T::CraftingFront,
            T::CraftingSide,
            T::CraftingTop,
            T::OakPlanks,
        ),
        M::Wood,
        2.5,
    )
    .tool(Axe, 0),
    block(
        "furnace",
        "熔炉",
        R::Opaque,
        Front(
            T::FurnaceFront,
            T::FurnaceSide,
            T::FurnaceTop,
            T::FurnaceTop,
        ),
        M::Stone,
        3.5,
    )
    .tool(Pickaxe, 1),
    block(
        "furnace_lit",
        "燃烧的熔炉",
        R::Opaque,
        Front(
            T::FurnaceFrontLit,
            T::FurnaceSide,
            T::FurnaceTop,
            T::FurnaceTop,
        ),
        M::Stone,
        3.5,
    )
    .tool(Pickaxe, 1)
    .light(13)
    .hidden(),
    block(
        "chest",
        "箱子",
        R::Opaque,
        Front(T::ChestFront, T::ChestSide, T::ChestTop, T::ChestTop),
        M::Wood,
        2.5,
    )
    .tool(Axe, 0),
    block("glass", "玻璃", R::Cutout, All(T::Glass), M::Glass, 0.3),
    block(
        "stone_bricks",
        "石砖",
        R::Opaque,
        All(T::StoneBricks),
        M::Stone,
        1.5,
    )
    .tool(Pickaxe, 1),
    block(
        "white_wool",
        "白色羊毛",
        R::Opaque,
        All(T::WoolWhite),
        M::Cloth,
        0.8,
    )
    .tool(Shears, 0),
    block(
        "red_wool",
        "红色羊毛",
        R::Opaque,
        All(T::WoolRed),
        M::Cloth,
        0.8,
    )
    .tool(Shears, 0),
    block(
        "yellow_wool",
        "黄色羊毛",
        R::Opaque,
        All(T::WoolYellow),
        M::Cloth,
        0.8,
    )
    .tool(Shears, 0),
    block(
        "blue_wool",
        "蓝色羊毛",
        R::Opaque,
        All(T::WoolBlue),
        M::Cloth,
        0.8,
    )
    .tool(Shears, 0),
    block(
        "green_wool",
        "绿色羊毛",
        R::Opaque,
        All(T::WoolGreen),
        M::Cloth,
        0.8,
    )
    .tool(Shears, 0),
    block(
        "black_wool",
        "黑色羊毛",
        R::Opaque,
        All(T::WoolBlack),
        M::Cloth,
        0.8,
    )
    .tool(Shears, 0),
    block(
        "glowstone",
        "萤石灯",
        R::Opaque,
        All(T::Glowstone),
        M::Glass,
        0.3,
    )
    .light(15),
    block(
        "iron_block",
        "铁块",
        R::Opaque,
        All(T::IronBlock),
        M::Stone,
        5.0,
    )
    .tool(Pickaxe, 2),
    block(
        "gold_block",
        "金块",
        R::Opaque,
        All(T::GoldBlock),
        M::Stone,
        3.0,
    )
    .tool(Pickaxe, 3),
    block(
        "diamond_block",
        "钻石块",
        R::Opaque,
        All(T::DiamondBlock),
        M::Stone,
        5.0,
    )
    .tool(Pickaxe, 3),
    block(
        "bookshelf",
        "书架",
        R::Opaque,
        Column(T::OakPlanks, T::Bookshelf, T::OakPlanks),
        M::Wood,
        1.5,
    )
    .tool(Axe, 0),
    block("torch", "火把", R::Torch, All(T::Torch), M::Wood, 0.0).light(14),
    block(
        "oak_sapling",
        "橡树树苗",
        R::Cross,
        All(T::OakSapling),
        M::Plant,
        0.0,
    ),
    block(
        "birch_sapling",
        "白桦树苗",
        R::Cross,
        All(T::BirchSapling),
        M::Plant,
        0.0,
    ),
    block(
        "spruce_sapling",
        "云杉树苗",
        R::Cross,
        All(T::SpruceSapling),
        M::Plant,
        0.0,
    ),
    block(
        "tall_grass",
        "草丛",
        R::Cross,
        All(T::TallGrass),
        M::Plant,
        0.0,
    )
    .tool(Shears, 0),
    block(
        "dandelion",
        "黄花",
        R::Cross,
        All(T::Dandelion),
        M::Plant,
        0.0,
    ),
    block("poppy", "红花", R::Cross, All(T::Poppy), M::Plant, 0.0),
    block(
        "cactus",
        "仙人掌",
        R::Cactus,
        Column(T::CactusTop, T::CactusSide, T::CactusBottom),
        M::Cloth,
        0.4,
    ),
    block(
        "sugar_cane",
        "甘蔗",
        R::Cross,
        All(T::SugarCane),
        M::Plant,
        0.0,
    ),
    block(
        "farmland",
        "耕地",
        R::Farmland,
        Column(T::Farmland, T::Dirt, T::Dirt),
        M::Dirt,
        0.6,
    )
    .tool(Shovel, 0)
    .hidden(),
    block("wheat", "小麦", R::Crop, All(T::Wheat3), M::Plant, 0.0).hidden(),
    block("ladder", "梯子", R::Ladder, All(T::Ladder), M::Wood, 0.4).tool(Axe, 0),
    block("door", "木门", R::Door, All(T::DoorLower), M::Wood, 3.0).tool(Axe, 0),
    block(
        "bed",
        "床",
        R::Bed,
        Column(T::BedFoot, T::BedSide, T::OakPlanks),
        M::Cloth,
        0.2,
    )
    .hidden(),
    block("snow_layer", "薄雪", R::Layer, All(T::Snow), M::Snow, 0.1).tool(Shovel, 0),
];

pub const COUNT: usize = BLOCKS.len();
const _: () = assert!(COUNT < 256 && TEX_COUNT < 256);

pub fn def(id: u8) -> &'static BlockDef {
    BLOCKS.get(usize::from(id)).unwrap_or(&BLOCKS[0])
}
/// Whether the block hides the face of a neighbour that touches it.
pub fn occludes(id: u8) -> bool {
    matches!(def(id).render, Render::Opaque | Render::Leaves)
}
/// Whether entities collide with the block.
pub fn solid(id: u8) -> bool {
    !matches!(
        def(id).render,
        Render::Air
            | Render::Liquid
            | Render::Cross
            | Render::Torch
            | Render::Crop
            | Render::Ladder
    ) && id != SNOW_LAYER
}
/// Whether the crosshair can select the block.
pub fn targetable(id: u8) -> bool {
    !matches!(def(id).render, Render::Air | Render::Liquid)
}
/// Rendered as a plain cube (all faces, full size).
pub fn is_cube(id: u8) -> bool {
    matches!(
        def(id).render,
        Render::Opaque | Render::Leaves | Render::Cutout | Render::Translucent
    )
}
/// Texture of one face, honouring the facing state of oriented blocks.
pub fn face_texture(id: u8, state: u8, face: Face) -> Tex {
    match def(id).faces {
        Faces::All(t) => t,
        Faces::Column(top, side, bottom) => match face {
            Face::Up => top,
            Face::Down => bottom,
            _ => side,
        },
        Faces::Front(front, side, top, bottom) => match face {
            Face::Up => top,
            Face::Down => bottom,
            f if f == facing_face(state) => front,
            _ => side,
        },
    }
}
/// Blocks offered in the creative palette, in registry order.
pub fn palette() -> Vec<u8> {
    (0..COUNT as u8).filter(|&id| def(id).creative).collect()
}

/// Render pass of a block's faces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pass {
    Opaque = 0,
    /// Alpha-tested: fully transparent pixels are discarded.
    Cutout = 1,
    /// Blended after everything else, far to near.
    Translucent = 2,
}
pub fn pass(id: u8, fancy_leaves: bool) -> Pass {
    match def(id).render {
        Render::Leaves if fancy_leaves => Pass::Cutout,
        Render::Cutout
        | Render::Cross
        | Render::Torch
        | Render::Cactus
        | Render::Ladder
        | Render::Door
        | Render::Bed
        | Render::Crop => Pass::Cutout,
        Render::Translucent => Pass::Translucent,
        Render::Liquid if id == WATER => Pass::Translucent,
        _ => Pass::Opaque,
    }
}
/// Whether `neighbour` hides the face of a block touching it. Leaves only
/// hide faces when drawn without holes ("fast leaves").
pub fn hides(neighbour: u8, fancy_leaves: bool) -> bool {
    match def(neighbour).render {
        Render::Opaque => true,
        Render::Leaves => !fancy_leaves,
        _ => false,
    }
}
pub fn liquid(id: u8) -> bool {
    def(id).render == Render::Liquid
}
/// Blocks a placed block may replace.
pub fn replaceable(id: u8) -> bool {
    matches!(id, AIR | WATER | LAVA | TALL_GRASS | SNOW_LAYER)
}
/// Liquid state: level 0 (source) to 7 in bits 0–2, falling in bit 3.
pub fn liquid_level(state: u8) -> u8 {
    state & 7
}
pub fn liquid_falling(state: u8) -> bool {
    state & 8 != 0
}
/// Surface height of a liquid cell in 1/16 blocks (when nothing of the
/// same liquid lies above it).
pub fn liquid_height(state: u8) -> i32 {
    if liquid_falling(state) {
        14
    } else {
        [14, 12, 11, 9, 7, 6, 4, 2][usize::from(liquid_level(state))]
    }
}

/// Index of a horizontal face as stored in facing states (0 N, 1 E, 2 S, 3 W).
pub fn facing_index(face: Face) -> Option<u8> {
    match face {
        Face::North => Some(0),
        Face::East => Some(1),
        Face::South => Some(2),
        Face::West => Some(3),
        _ => None,
    }
}
/// Direction a player looking along `yaw` faces.
pub fn player_facing(yaw: f32) -> u8 {
    (facing_towards_player(yaw) + 2) & 3
}
impl Face {
    pub fn from_normal(n: glam::IVec3) -> Option<Self> {
        Some(match (n.x, n.y, n.z) {
            (1, 0, 0) => Face::East,
            (-1, 0, 0) => Face::West,
            (0, 1, 0) => Face::Up,
            (0, -1, 0) => Face::Down,
            (0, 0, 1) => Face::South,
            (0, 0, -1) => Face::North,
            _ => return None,
        })
    }
    pub fn normal(self) -> glam::IVec3 {
        match self {
            Face::East => glam::IVec3::X,
            Face::West => glam::IVec3::NEG_X,
            Face::Up => glam::IVec3::Y,
            Face::Down => glam::IVec3::NEG_Y,
            Face::South => glam::IVec3::Z,
            Face::North => glam::IVec3::NEG_Z,
        }
    }
    /// The face after turning a north-facing model to `facing` (0–3).
    pub fn rotated(self, facing: u8) -> Self {
        let mut f = self;
        for _ in 0..facing & 3 {
            f = match f {
                Face::North => Face::East,
                Face::East => Face::South,
                Face::South => Face::West,
                Face::West => Face::North,
                other => other,
            };
        }
        f
    }
}

/// Torch state: 0 standing, 1–4 on a wall pointing north/east/south/west.
pub fn torch_state(pointing: Face) -> Option<u8> {
    facing_index(pointing).map(|i| i + 1)
}
pub fn torch_pointing(state: u8) -> Option<u8> {
    (1..=4).contains(&state).then(|| state - 1)
}
/// Door state: facing in bits 0–1, upper half in bit 2, open in bit 3.
pub const DOOR_UPPER: u8 = 4;
pub const DOOR_OPEN: u8 = 8;
/// Bed state: facing in bits 0–1, head half in bit 2.
pub const BED_HEAD: u8 = 4;

/// Turns a point of a north-facing model (1/16 units) to `facing`.
pub fn rotate_xz(x: i32, z: i32, facing: u8) -> (i32, i32) {
    match facing & 3 {
        0 => (x, z),
        1 => (16 - z, x),
        2 => (16 - x, 16 - z),
        _ => (z, 16 - x),
    }
}
fn rotate_box(min: [i32; 3], max: [i32; 3], facing: u8) -> ([i32; 3], [i32; 3]) {
    let (ax, az) = rotate_xz(min[0], min[2], facing);
    let (bx, bz) = rotate_xz(max[0], max[2], facing);
    (
        [ax.min(bx), min[1], az.min(bz)],
        [ax.max(bx), max[1], az.max(bz)],
    )
}

/// Selection and ray-test box in 1/16 block units.
pub fn bounds(id: u8, state: u8) -> ([i32; 3], [i32; 3]) {
    let full = ([0, 0, 0], [16, 16, 16]);
    match def(id).render {
        Render::Cross if id == SUGAR_CANE => ([2, 0, 2], [14, 16, 14]),
        Render::Cross => ([2, 0, 2], [14, 13, 14]),
        Render::Torch => match torch_pointing(state) {
            None => ([6, 0, 6], [10, 10, 10]),
            Some(f) => rotate_box([5, 3, 8], [11, 14, 16], f),
        },
        Render::Cactus => ([1, 0, 1], [15, 16, 15]),
        Render::Ladder => rotate_box([0, 0, 13], [16, 16, 16], state & 3),
        Render::Door if state & DOOR_OPEN != 0 => rotate_box([0, 0, 0], [3, 16, 16], state & 3),
        Render::Door => rotate_box([0, 0, 13], [16, 16, 16], state & 3),
        Render::Bed => ([0, 0, 0], [16, 9, 16]),
        Render::Layer => ([0, 0, 0], [16, 2, 16]),
        Render::Farmland => ([0, 0, 0], [16, 15, 16]),
        Render::Crop => ([0, 0, 0], [16, 4 + 2 * i32::from(state.min(6)), 16]),
        _ => full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_consistent() {
        let mut keys: Vec<_> = BLOCKS.iter().map(|b| b.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), COUNT, "duplicate keys");
        for (id, b) in BLOCKS.iter().enumerate().skip(1) {
            assert!(!b.name.is_empty() && !b.key.is_empty(), "{id}");
            assert!(b.light <= 15 && b.opacity <= 15);
            if b.render != Render::Liquid {
                assert!(b.material.is_some(), "{} has no sound", b.key);
            }
        }
        // First-release ids must never move.
        let legacy = [
            (GRASS, "grass"),
            (DIRT, "dirt"),
            (STONE, "stone"),
            (SAND, "sand"),
            (OAK_LOG, "oak_log"),
            (OAK_LEAVES, "oak_leaves"),
            (OAK_PLANKS, "oak_planks"),
            (BRICKS, "bricks"),
            (BEDROCK, "bedrock"),
        ];
        for (id, key) in legacy {
            assert_eq!(def(id).key, key);
        }
        assert_eq!(def(SNOW_LAYER).key, "snow_layer");
        assert_eq!(def(250).key, "air", "unknown ids read as air");
        assert_eq!(Tex::all().count(), TEX_COUNT);
        assert!(Tex::all().enumerate().all(|(i, t)| t.layer() as usize == i));
        let palette = palette();
        assert!(palette.len() >= 55, "{}", palette.len());
        for hidden in [AIR, BEDROCK, FURNACE_LIT, FARMLAND, WHEAT, BED] {
            assert!(!palette.contains(&hidden), "{}", def(hidden).key);
        }
        for shown in [GLASS, WATER, TORCH, TALL_GRASS, LADDER, DOOR, SNOW_LAYER] {
            assert!(palette.contains(&shown), "{}", def(shown).key);
        }
    }

    #[test]
    fn faces_follow_columns_and_facing() {
        assert_eq!(face_texture(GRASS, 0, Face::Up), Tex::GrassTop);
        assert_eq!(face_texture(GRASS, 0, Face::North), Tex::GrassSide);
        assert_eq!(face_texture(GRASS, 0, Face::Down), Tex::Dirt);
        for (state, front) in [
            (0, Face::North),
            (1, Face::East),
            (2, Face::South),
            (3, Face::West),
        ] {
            assert_eq!(face_texture(FURNACE, state, front), Tex::FurnaceFront);
            assert_eq!(face_texture(FURNACE, state, Face::Up), Tex::FurnaceTop);
            let other = if front == Face::North {
                Face::South
            } else {
                Face::North
            };
            assert_eq!(face_texture(FURNACE, state, other), Tex::FurnaceSide);
        }
        // Looking north (yaw 0) the front faces south, towards the player.
        assert_eq!(facing_face(facing_towards_player(0.0)), Face::South);
        assert_eq!(
            facing_face(facing_towards_player(std::f32::consts::FRAC_PI_2)),
            Face::West
        );
        assert_eq!(
            facing_face(facing_towards_player(std::f32::consts::PI)),
            Face::North
        );
        assert_eq!(Face::from_axis(0, true), Face::East);
        assert_eq!(Face::from_axis(2, false), Face::North);
    }

    #[test]
    fn collision_and_selection_rules() {
        assert!(solid(STONE) && solid(OAK_LEAVES) && solid(GLASS) && solid(CACTUS));
        assert!(!solid(AIR) && !solid(WATER) && !solid(TORCH) && !solid(TALL_GRASS));
        assert!(targetable(TORCH) && !targetable(WATER) && !targetable(AIR));
        assert!(occludes(STONE) && occludes(OAK_LEAVES) && !occludes(GLASS) && !occludes(TORCH));
        assert_eq!(def(GLOWSTONE).light, 15);
        assert_eq!(def(STONE).opacity, 15);
        assert!(def(BEDROCK).hardness < 0.0);
    }

    #[test]
    fn passes_liquids_and_model_bounds() {
        assert_eq!(pass(STONE, true), Pass::Opaque);
        assert_eq!(pass(OAK_LEAVES, false), Pass::Opaque);
        assert_eq!(pass(OAK_LEAVES, true), Pass::Cutout);
        assert_eq!(pass(GLASS, false), Pass::Cutout);
        assert_eq!(pass(TORCH, false), Pass::Cutout);
        assert_eq!(pass(WATER, false), Pass::Translucent);
        assert_eq!(pass(ICE, false), Pass::Translucent);
        assert_eq!(pass(LAVA, false), Pass::Opaque);
        assert!(hides(STONE, true) && hides(OAK_LEAVES, false) && !hides(OAK_LEAVES, true));
        assert!(!hides(WATER, false) && !hides(GLASS, false) && !hides(TORCH, false));
        assert_eq!(liquid_height(0), 14);
        assert!(liquid_height(7) < liquid_height(1));
        assert_eq!(liquid_height(8 | 5), 14, "falling liquid is tall");
        assert!(replaceable(WATER) && replaceable(TALL_GRASS) && !replaceable(STONE));
        assert_eq!(bounds(STONE, 0), ([0, 0, 0], [16, 16, 16]));
        assert_eq!(bounds(TORCH, 0), ([6, 0, 6], [10, 10, 10]));
        // A torch pointing east hangs on the wall to its west.
        let (min, max) = bounds(TORCH, torch_state(Face::East).unwrap());
        assert_eq!((min[0], max[0]), (0, 8), "{min:?} {max:?}");
        // Ladders and doors turn with their facing.
        assert_eq!(bounds(LADDER, 0).0[2], 13);
        assert_eq!(bounds(LADDER, 2).1[2], 3);
        let (min, max) = bounds(DOOR, 1);
        assert_eq!((min[0], max[0], min[2], max[2]), (0, 3, 0, 16));
        assert_eq!(rotate_xz(0, 16, 1), (0, 0));
        assert_eq!(Face::North.rotated(1), Face::East);
        assert_eq!(Face::West.rotated(3), Face::South);
        assert_eq!(Face::from_normal(glam::IVec3::NEG_Z), Some(Face::North));
        assert_eq!(player_facing(0.0), 0, "looking north");
        assert_eq!(
            player_facing(std::f32::consts::FRAC_PI_2),
            1,
            "looking east"
        );
    }
}

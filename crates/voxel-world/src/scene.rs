use crate::{
    app::App,
    jobs::{Kind, Pool},
    mesh::{
        self, GROUP_ORDER, GROUPS, Job, Mesh, STYLE_FANCY_LEAVES, STYLE_SMOOTH, VISIBLE_ALL, Vertex,
    },
    render::{
        arena::{Arena, Slot},
        clouds, sky, textures,
    },
    world::{self, CHUNK, World, block, chunk},
};
use bytemuck::{Pod, Zeroable};
use glam::{IVec2, IVec3, Mat4, Vec3};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::Instant,
};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Camera {
    /// View-projection relative to the camera's block.
    matrix: [[f32; 4]; 4],
    /// Inverse rotation-only view-projection, for sky directions.
    inverse: [[f32; 4]; 4],
    /// Eye position inside its block.
    eye: [f32; 4],
    /// Fog colour and end distance.
    fog: [f32; 4],
    /// Zenith colour and daylight factor.
    zenith: [f32; 4],
    /// Horizon colour and star visibility.
    horizon: [f32; 4],
    /// Sun direction and day phase.
    sun: [f32; 4],
    /// Basis of the sun and moon discs; `sun_right.w` is the dusk glow,
    /// `sun_up.w` the distance where fog starts.
    sun_right: [f32; 4],
    sun_up: [f32; 4],
    /// Rotation-only view-projection, for directions at infinity.
    rotation: [[f32; 4]; 4],
}
/// Where a section's mesh lives on the GPU.
enum Storage {
    /// Inside the shared pages (16-bit indices).
    Arena(Slot),
    /// Rare huge meshes with 32-bit indices get their own buffers.
    Own(wgpu::Buffer, wgpu::Buffer),
}
struct GpuMesh {
    /// `None` for sections without visible faces.
    storage: Option<Storage>,
    /// Index ranges per pass and face group.
    ranges: [[(u32, u32); GROUPS]; 3],
    /// Face-to-face connectivity through the section.
    visibility: u64,
    revision: u64,
    style: u32,
}
/// Most sections drawn in one frame (view distance 6 needs about 1 400).
const MAX_DRAWS: usize = 4096;
/// Columns (Chebyshev distance) drawn with see-through leaves.
const FANCY_RADIUS: i32 = 1;
/// Indirect draw records per frame (all passes).
const MAX_ARGS: usize = 32 * 1024;
const ARGS_SIZE: u64 = std::mem::size_of::<wgpu::util::DrawIndexedIndirectArgs>() as u64;

/// Consecutive indirect draws that use the same buffer page.
#[derive(Clone, Copy, Debug)]
struct Batch {
    page: usize,
    first: u32,
    count: u32,
}
/// Origin slot of the cloud layer, after the section slots.
const CLOUD_SLOT: usize = MAX_DRAWS;

struct CloudMesh {
    anchor: IVec2,
    radius: i32,
    buffer: Option<wgpu::Buffer>,
    vertices: u32,
}
const SKY_GRID: u32 = 24 * 14;
const STARS: u32 = 360;

struct SkyPipelines {
    gradient: wgpu::RenderPipeline,
    discs: wgpu::RenderPipeline,
    stars: wgpu::RenderPipeline,
    clouds: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
}
impl SkyPipelines {
    fn new(device: &wgpu::Device, uniform: &wgpu::Buffer) -> Self {
        let module = shader(device, "sky", include_str!("sky.wgsl"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky bindings"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let cloud_vertex = wgpu::vertex_attr_array![0 => Float32x3];
        let cloud_origin = wgpu::vertex_attr_array![1 => Float32x4];
        let cloud_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &cloud_vertex,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &cloud_origin,
            }),
        ];
        let make = |label: &str,
                    vs: &str,
                    fs: &str,
                    blend: Option<wgpu::BlendState>,
                    buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
                    depth: wgpu::DepthStencilState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: Some(depth),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // The sky is drawn after opaque terrain at the far plane, so only
        // uncovered pixels are shaded.
        let behind = || depth_state(false, wgpu::CompareFunction::LessEqual);
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        Self {
            gradient: make(
                "sky gradient",
                "vs_gradient",
                "fs_gradient",
                None,
                &[],
                behind(),
            ),
            discs: make("sun and moon", "vs_disc", "fs_disc", None, &[], behind()),
            stars: make("stars", "vs_star", "fs_star", blend, &[], behind()),
            // Clouds write depth so water behind them stays hidden.
            clouds: make(
                "clouds",
                "vs_cloud",
                "fs_cloud",
                blend,
                &cloud_buffers,
                depth_state(true, wgpu::CompareFunction::Less),
            ),
            group: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sky bindings"),
                layout: &layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                }],
            }),
        }
    }
}

pub struct Scene {
    passes: [wgpu::RenderPipeline; 3],
    sky: SkyPipelines,
    stars: bool,
    /// Sky and clouds are hidden while the camera is inside a liquid.
    open_sky: bool,
    clouds: Option<CloudMesh>,
    blit: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    blit_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    origins: wgpu::Buffer,
    fullscreen: wgpu::Buffer,
    color: wgpu::TextureView,
    _color_texture: wgpu::Texture,
    depth: wgpu::TextureView,
    meshes: HashMap<IVec3, GpuMesh>,
    arena: Arena,
    /// One multi-draw per batch; `None` when the device cannot draw
    /// indirectly with instance offsets (sections are then drawn one by one).
    indirect: Option<wgpu::Buffer>,
    batches: [Vec<Batch>; 3],
    /// Draw slots of meshes outside the arena, per pass.
    singles: [Vec<usize>; 3],
    pending: HashMap<IVec3, (u64, u64)>,
    /// Sections to draw this frame, nearest first.
    draws: Vec<IVec3>,
    pool: Arc<Pool>,
    results: Receiver<Mesh>,
    sender: Sender<Mesh>,
    epoch: u64,
    fog: [f32; 4],
    /// Relative view-projection and the camera block it is relative to.
    pub matrix: Mat4,
    pub origin: IVec3,
    /// Camera position in world space (face culling).
    eye: Vec3,
    frustum: Frustum,
}
/// View frustum as six planes (normal · p + d ≥ 0 inside).
#[derive(Clone, Copy)]
pub struct Frustum([glam::Vec4; 6]);
impl Frustum {
    /// Planes of a clip-space matrix with a 0..1 depth range.
    pub fn new(m: Mat4) -> Self {
        let r = |i: usize| m.row(i);
        let planes = [
            r(3) + r(0),
            r(3) - r(0),
            r(3) + r(1),
            r(3) - r(1),
            r(2),
            r(3) - r(2),
        ];
        Self(planes.map(|p| p / p.truncate().length().max(1e-6)))
    }
    /// Whether any part of the 16³ section box at `origin` may be visible.
    pub fn section(&self, origin: Vec3) -> bool {
        let max = origin + Vec3::splat(CHUNK as f32);
        self.0.iter().all(|p| {
            // The corner furthest along the plane normal.
            let v = Vec3::select(p.truncate().cmpge(Vec3::ZERO), max, origin);
            p.truncate().dot(v) + p.w >= 0.0
        })
    }
}
fn shader(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}
fn depth_state(write: bool, compare: wgpu::CompareFunction) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth24Plus,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: Default::default(),
        bias: Default::default(),
    }
}
/// Bind group layout and the opaque, cutout and translucent pipelines.
fn terrain_pipelines(device: &wgpu::Device) -> (wgpu::BindGroupLayout, [wgpu::RenderPipeline; 3]) {
    let module = shader(device, "voxel shader", include_str!("voxel.wgsl"));
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("terrain bindings"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("terrain"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let vertex = wgpu::vertex_attr_array![0 => Uint16x4, 1 => Sint16x2, 2 => Uint8x4];
    let instance = wgpu::vertex_attr_array![3 => Float32x4];
    let buffers = [
        Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &vertex,
        }),
        Some(wgpu::VertexBufferLayout {
            array_stride: 16,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &instance,
        }),
    ];
    let make = |label: &str,
                fs: &str,
                blend: Option<wgpu::BlendState>,
                cull: Option<wgpu::Face>,
                depth_write: bool| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: cull,
                ..Default::default()
            },
            depth_stencil: Some(depth_state(depth_write, wgpu::CompareFunction::Less)),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    let back = Some(wgpu::Face::Back);
    (
        layout,
        [
            make("opaque terrain", "fs_main", None, back, true),
            // Plants and ladders are single quads seen from both sides.
            make("cutout terrain", "fs_cutout", None, None, true),
            // Water surfaces are seen from below too; blended faces do not
            // write depth so everything behind them still shows.
            make(
                "translucent terrain",
                "fs_translucent",
                Some(wgpu::BlendState::ALPHA_BLENDING),
                None,
                false,
            ),
        ],
    )
}
fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    format: wgpu::TextureFormat,
    depth: Option<wgpu::DepthStencilState>,
    buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
) -> wgpu::RenderPipeline {
    let module = shader(device, label, source);
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        primitive: Default::default(),
        depth_stencil: depth,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}
impl Scene {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        pool: Arc<Pool>,
    ) -> Self {
        let (sender, results) = mpsc::channel();
        let (terrain_layout, passes) = terrain_pipelines(device);
        let blit_attributes = wgpu::vertex_attr_array![0 => Float32x2];
        let blit = fullscreen_pipeline(
            device,
            "scene upscale",
            include_str!("blit.wgsl"),
            format,
            None,
            &[Some(wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &blit_attributes,
            })],
        );
        let origins = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("section origins"),
            size: ((MAX_DRAWS + 1) * 16) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&Camera::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let tiles = textures::all();
        let levels = textures::mip_chain(&tiles);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("procedural pixel blocks"),
            size: wgpu::Extent3d {
                width: textures::TILE as u32,
                height: textures::TILE as u32,
                depth_or_array_layers: tiles.len() as u32,
            },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, pixels) in levels.iter().enumerate() {
            let size = (textures::TILE >> level) as u32;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 4),
                    rows_per_image: Some(size),
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: tiles.len() as u32,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            // Distant blocks sample smaller mips: less shimmer and less
            // texture bandwidth on the tile-based GPU.
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene bindings"),
            layout: &terrain_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let sky = SkyPipelines::new(device, &uniform);
        let color_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("540p scene"),
            size: wgpu::Extent3d {
                width: 960,
                height: 540,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color = color_texture.create_view(&Default::default());
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("scene depth"),
                size: color_texture.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth24Plus,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let filter = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let blit_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("upscale bindings"),
            layout: &blit.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&filter),
                },
            ],
        });
        let fullscreen = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fullscreen triangle"),
            contents: bytemuck::cast_slice(&[[-1.0f32, -1.0], [3.0, -1.0], [-1.0, 3.0]]),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            passes,
            sky,
            stars: false,
            open_sky: true,
            clouds: None,
            blit,
            group,
            blit_group,
            uniform,
            origins,
            fullscreen,
            color,
            _color_texture: color_texture,
            depth,
            meshes: HashMap::new(),
            arena: Arena::default(),
            indirect: device
                .features()
                .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
                .then(|| {
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("section draws"),
                        size: MAX_ARGS as u64 * ARGS_SIZE,
                        usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    })
                }),
            batches: Default::default(),
            singles: Default::default(),
            pending: HashMap::new(),
            draws: Vec::new(),
            pool,
            results,
            sender,
            epoch: 0,
            fog: [0.7, 0.8, 0.95, 64.0],
            matrix: Mat4::IDENTITY,
            origin: IVec3::ZERO,
            eye: Vec3::ZERO,
            frustum: Frustum::new(Mat4::IDENTITY),
        }
    }
    /// Sections with a GPU mesh (including empty ones).
    pub fn loaded(&self) -> usize {
        self.meshes.len()
    }
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
    /// MiB of vertex and index data in the shared pages, and page count.
    pub fn arena_usage(&self) -> (f32, f32, usize) {
        self.arena.usage()
    }
    /// Sections drawn and triangles submitted per pass in the last frame.
    pub fn draw_stats(&self) -> (usize, [u32; 3]) {
        let mut tris = [0u32; 3];
        for key in &self.draws {
            if let Some(m) = self.meshes.get(key) {
                let mask = facing_groups(*key, self.eye);
                for (p, t) in tris.iter_mut().enumerate() {
                    *t += draw_ranges(&m.ranges[p], mask)
                        .iter()
                        .map(|r| r.1)
                        .sum::<u32>()
                        / 3;
                }
            }
        }
        (self.draws.len(), tris)
    }
    /// Uploads finished meshes and schedules new ones; returns upload ms.
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, app: &App) -> f32 {
        if self.epoch != app.epoch {
            for (_, m) in self.meshes.drain() {
                if let Some(Storage::Arena(slot)) = m.storage {
                    self.arena.free(slot);
                }
            }
            self.pending.clear();
            while self.results.try_recv().is_ok() {}
            self.epoch = app.epoch;
        }
        let world = &app.world;
        let eye = app.camera_position();
        let origin = eye.floor().as_ivec3();
        let center = world::column_at(eye);
        let radius = app.config.video.view_distance;
        let video = &app.config.video;
        let base_style = if video.smooth_lighting {
            STYLE_SMOOTH
        } else {
            0
        };
        let fancy = video.fancy_leaves;
        // Alpha-tested leaves are expensive on the tile-based GPU, so only
        // the columns around the player get see-through leaves.
        let style_of = |pos: IVec2| {
            if fancy && (pos - center).abs().max_element() <= FANCY_RADIUS {
                base_style | STYLE_FANCY_LEAVES
            } else {
                base_style
            }
        };
        let nearby =
            |key: IVec3| (IVec2::new(key.x, key.z) - center).abs().max_element() <= radius + 1;
        let arena = &mut self.arena;
        self.meshes.retain(|&key, m| {
            let keep = nearby(key) && world.section_revision(key).is_some();
            if !keep && let Some(Storage::Arena(slot)) = m.storage {
                arena.free(slot);
            }
            keep
        });
        let upload_start = Instant::now();
        for _ in 0..6 {
            if upload_start.elapsed().as_secs_f32() > 0.0015 {
                break;
            }
            let Ok(mesh) = self.results.try_recv() else {
                break;
            };
            if self.pending.get(&mesh.key) == Some(&(mesh.revision, mesh.epoch)) {
                self.pending.remove(&mesh.key);
            }
            if mesh.epoch != app.epoch
                || world.section_revision(mesh.key) != Some(mesh.revision)
                || !nearby(mesh.key)
            {
                continue;
            }
            let storage = if mesh.indices.is_empty() {
                None
            } else {
                let slot = match &mesh.indices {
                    mesh::Indices::U16(idx) => self.arena.upload(
                        device,
                        queue,
                        bytemuck::cast_slice(&mesh.vertices),
                        mesh.vertices.len() as u32,
                        idx,
                    ),
                    mesh::Indices::U32(_) => None,
                };
                Some(match slot {
                    Some(slot) => Storage::Arena(slot),
                    None => Storage::Own(
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("section vertices"),
                            contents: bytemuck::cast_slice(&mesh.vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        }),
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("section indices"),
                            contents: mesh.indices.bytes(),
                            usage: wgpu::BufferUsages::INDEX,
                        }),
                    ),
                })
            };
            self.replace_mesh(
                mesh.key,
                GpuMesh {
                    storage,
                    ranges: mesh.ranges,
                    visibility: mesh.visibility,
                    revision: mesh.revision,
                    style: mesh.style,
                },
            );
        }
        let upload_ms = upload_start.elapsed().as_secs_f32() * 1000.0;
        let far = (radius as f32 + 2.0) * 16.0;
        let projection =
            Mat4::perspective_rh(app.fov_degrees().to_radians(), 16.0 / 9.0, 0.05, far + 16.0);
        let direction = app.player.direction();
        let fract = eye - origin.as_vec3();
        self.matrix = projection * Mat4::look_to_rh(fract, direction, Vec3::Y);
        self.frustum = Frustum::new(self.matrix);
        self.origin = origin;
        self.eye = eye;
        let relative = |key: IVec3| (key * CHUNK - origin).as_vec3();
        let eye_section = world::section_of(origin);
        let mut candidates = Vec::new();
        for column in world.columns() {
            let offset = column.pos - center;
            if offset.abs().max_element() > radius {
                continue;
            }
            let style = style_of(column.pos);
            for s in 0..chunk::SECTIONS {
                let key = IVec3::new(column.pos.x, s as i32, column.pos.y);
                let revision = column.revisions[s];
                if self.pending.contains_key(&key)
                    || self.meshes.get(&key).is_some_and(|m| {
                        m.revision == revision && (m.style == style || m.storage.is_none())
                    })
                {
                    continue;
                }
                if column.is_empty(s) || enclosed(world, key) {
                    // Nothing to draw; record it without a background job.
                    if let Some(GpuMesh {
                        storage: Some(Storage::Arena(slot)),
                        ..
                    }) = self.meshes.insert(
                        key,
                        GpuMesh {
                            storage: None,
                            ranges: [[(0, 0); GROUPS]; 3],
                            // Empty sections are open, enclosed ones closed.
                            visibility: if column.is_empty(s) { VISIBLE_ALL } else { 0 },
                            revision,
                            style,
                        },
                    ) {
                        self.arena.free(slot);
                    }
                    continue;
                }
                let d = key - eye_section;
                let stale = self.meshes.contains_key(&key);
                let priority = i64::from(offset.length_squared()) * 2
                    + i64::from(d.y * d.y) / 2
                    + 3
                    + if self.frustum.section(relative(key)) { 0 } else { 64 }
                    // Replacing a drawable mesh is less urgent than a hole.
                    + if stale { 24 } else { 0 };
                candidates.push((priority, key));
            }
        }
        candidates.sort_unstable_by_key(|(p, k)| (*p, k.x, k.y, k.z));
        let budget = self.pool.active() * 2 + 1;
        let mut submitted = 0;
        for (priority, key) in candidates {
            if submitted >= 3 || self.pending.len() >= budget {
                break;
            }
            // Needs all neighbouring columns lit; skip until they are.
            let Some(job) = Job::new(world, key, app.epoch, style_of(IVec2::new(key.x, key.z)))
            else {
                continue;
            };
            self.pending.insert(key, (job.revision, job.epoch));
            let sender = self.sender.clone();
            self.pool.submit(Kind::Mesh, priority, move || {
                let _ = sender.send(job.build());
            });
            submitted += 1;
        }
        // Sections reachable from the camera through open space and inside
        // the view, nearest first, with camera-relative origins.
        let reachable = self.reachable(eye_section, origin, center, radius, direction);
        let mut draws: Vec<(i32, IVec3)> = reachable
            .into_iter()
            .filter(|key| self.meshes.get(key).is_some_and(|m| m.storage.is_some()))
            .map(|key| ((key * CHUNK + 8 - origin).length_squared(), key))
            .collect();
        draws.sort_unstable_by_key(|(d, k)| (*d, k.x, k.y, k.z));
        draws.truncate(MAX_DRAWS);
        self.draws = draws.into_iter().map(|(_, k)| k).collect();
        let origins: Vec<[f32; 4]> = self
            .draws
            .iter()
            .map(|&k| relative(k).extend(0.0).to_array())
            .collect();
        if !origins.is_empty() {
            queue.write_buffer(&self.origins, 0, bytemuck::cast_slice(&origins));
        }
        self.build_batches(queue);
        let sky = sky::sky(app.time);
        let sun_right = sky.sun.cross(Vec3::Z).normalize();
        let rotation = projection * Mat4::look_to_rh(Vec3::ZERO, direction, Vec3::Y);
        self.fog = [sky.fog.x, sky.fog.y, sky.fog.z, radius as f32 * 16.0];
        let mut fog_start = self.fog[3] * 0.6;
        // Inside water or lava the world closes in and the sky disappears;
        // a negative start tints even the closest blocks.
        let submerged = submerged(world, eye);
        self.open_sky = submerged.is_none();
        match submerged {
            Some(block::WATER) => {
                let c = Vec3::new(0.06, 0.2, 0.42) * (0.3 + 0.7 * sky.daylight);
                self.fog = [c.x, c.y, c.z, 14.0];
                fog_start = -6.0;
            }
            Some(_) => {
                self.fog = [0.85, 0.32, 0.05, 2.5];
                fog_start = -1.0;
            }
            None => {}
        }
        self.update_clouds(device, queue, app, origin, radius);
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&Camera {
                matrix: self.matrix.to_cols_array_2d(),
                inverse: rotation.inverse().to_cols_array_2d(),
                eye: fract
                    .extend((app.time / sky::TICKS_PER_SECOND % 3600.0) as f32)
                    .to_array(),
                fog: self.fog,
                zenith: sky.zenith.extend(sky.daylight).to_array(),
                horizon: sky.horizon.extend(sky.stars).to_array(),
                sun: sky.sun.extend(sky.phase).to_array(),
                sun_right: sun_right.extend(sky.dusk).to_array(),
                sun_up: sun_right.cross(sky.sun).extend(fog_start).to_array(),
                rotation: rotation.to_cols_array_2d(),
            }),
        );
        self.stars = sky.stars > 0.01;
        upload_ms
    }
    pub fn prepare(&self, encoder: &mut wgpu::CommandEncoder) {
        let [r, g, b, _] = self.fog.map(f64::from);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voxel world"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, &self.group, &[]);
        // Section origins are selected with the instance index.
        pass.set_vertex_buffer(1, self.origins.slice(..));
        // Opaque and cutout geometry near to far, so hidden pixels are
        // rejected early.
        for p in 0..2 {
            pass.set_pipeline(&self.passes[p]);
            self.draw_pass(&mut pass, p);
        }
        if self.open_sky {
            pass.set_bind_group(0, &self.sky.group, &[]);
            pass.set_pipeline(&self.sky.gradient);
            pass.draw(0..SKY_GRID * 6, 0..1);
            if self.stars {
                pass.set_pipeline(&self.sky.stars);
                pass.draw(0..6, 0..STARS);
            }
            pass.set_pipeline(&self.sky.discs);
            pass.draw(0..6, 0..2);
            if let Some(CloudMesh {
                buffer: Some(buffer),
                vertices,
                ..
            }) = &self.clouds
            {
                let at = (CLOUD_SLOT * 16) as u64;
                pass.set_pipeline(&self.sky.clouds);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.set_vertex_buffer(1, self.origins.slice(at..at + 16));
                pass.draw(0..*vertices, 0..1);
            }
        }
        // Blended geometry far to near.
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_vertex_buffer(1, self.origins.slice(..));
        pass.set_pipeline(&self.passes[2]);
        self.draw_pass(&mut pass, 2);
    }
    /// Breadth-first walk from the camera's section: a neighbour is entered
    /// through a face only if the current section connects the face it was
    /// entered from to that face, the step leads away from the camera and
    /// the neighbour is in the frustum. Sections hidden behind solid rock
    /// (most caves seen from the surface) are never reached.
    fn reachable(
        &self,
        start: IVec3,
        origin: IVec3,
        center: IVec2,
        radius: i32,
        direction: Vec3,
    ) -> Vec<IVec3> {
        use crate::world::block::Face;
        let faces = [
            Face::East,
            Face::West,
            Face::Up,
            Face::Down,
            Face::South,
            Face::North,
        ];
        let in_range = |k: IVec3| {
            (0..chunk::SECTIONS as i32).contains(&k.y)
                && (IVec2::new(k.x, k.z) - center).abs().max_element() <= radius
        };
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        // Above or below the world the walk starts from every section in
        // the eye's column that exists.
        let start = start.with_y(start.y.clamp(0, chunk::SECTIONS as i32 - 1));
        seen.insert(start);
        queue.push_back((start, None::<Face>, 0u8));
        while let Some((key, entered, dirs)) = queue.pop_front() {
            out.push(key);
            // Unmeshed sections (still loading) are treated as open so that
            // terrain beyond them appears once they arrive.
            let vis = self.meshes.get(&key).map_or(VISIBLE_ALL, |m| m.visibility);
            for face in faces {
                let n = face.normal();
                // Never walk back towards the camera.
                if dirs & (1 << opposite(face) as u8) != 0 {
                    continue;
                }
                if let Some(from) = entered
                    && !mesh::connected(vis, from, face)
                {
                    continue;
                }
                let next = key + n;
                if !in_range(next) || seen.contains(&next) {
                    continue;
                }
                // Skip sections entirely behind the camera.
                let to_centre = (next * CHUNK + 8 - origin).as_vec3();
                if to_centre.dot(direction) < -14.0
                    || !self.frustum.section((next * CHUNK - origin).as_vec3())
                {
                    continue;
                }
                seen.insert(next);
                queue.push_back((next, Some(opposite(face)), dirs | 1 << face as u8));
            }
        }
        out
    }
    /// Records the indirect draws of this frame: opaque and cutout grouped
    /// by page (near to far within a page), translucent strictly far to near.
    fn build_batches(&mut self, queue: &wgpu::Queue) {
        for p in 0..3 {
            self.batches[p].clear();
            self.singles[p].clear();
        }
        let Some(buffer) = &self.indirect else {
            return;
        };
        let mut args: Vec<wgpu::util::DrawIndexedIndirectArgs> = Vec::new();
        for p in 0..3 {
            let mut slots: Vec<usize> = (0..self.draws.len()).collect();
            if p == 2 {
                slots.reverse();
            } else {
                slots.sort_by_key(|&slot| {
                    match self
                        .meshes
                        .get(&self.draws[slot])
                        .and_then(|m| m.storage.as_ref())
                    {
                        Some(Storage::Arena(s)) => s.page,
                        _ => usize::MAX,
                    }
                });
            }
            for slot in slots {
                let key = self.draws[slot];
                let Some(mesh) = self.meshes.get(&key) else {
                    continue;
                };
                let s = match &mesh.storage {
                    Some(Storage::Arena(s)) => *s,
                    Some(Storage::Own(..)) => {
                        self.singles[p].push(slot);
                        continue;
                    }
                    None => continue,
                };
                let mask = if p == 2 {
                    u8::MAX
                } else {
                    facing_groups(key, self.eye)
                };
                for (start, count) in draw_ranges(&mesh.ranges[p], mask) {
                    if args.len() >= MAX_ARGS {
                        break;
                    }
                    let first = args.len() as u32;
                    args.push(wgpu::util::DrawIndexedIndirectArgs {
                        index_count: count,
                        instance_count: 1,
                        first_index: s.first_index + start,
                        base_vertex: s.base_vertex as i32,
                        first_instance: slot as u32,
                    });
                    match self.batches[p].last_mut() {
                        Some(b) if b.page == s.page && b.first + b.count == first => b.count += 1,
                        _ => self.batches[p].push(Batch {
                            page: s.page,
                            first,
                            count: 1,
                        }),
                    }
                }
            }
        }
        if !args.is_empty() {
            let bytes: Vec<u8> = args.iter().flat_map(|a| a.as_bytes().to_vec()).collect();
            queue.write_buffer(buffer, 0, &bytes);
        }
    }
    /// Draws pass `p` of every visible section.
    fn draw_pass(&self, pass: &mut wgpu::RenderPass<'_>, p: usize) {
        let Some(buffer) = &self.indirect else {
            let mut bound = None;
            let slots: Box<dyn Iterator<Item = (usize, &IVec3)>> = if p == 2 {
                Box::new(self.draws.iter().enumerate().rev())
            } else {
                Box::new(self.draws.iter().enumerate())
            };
            for (slot, key) in slots {
                self.draw_section(pass, slot, key, p, &mut bound);
            }
            return;
        };
        for b in &self.batches[p] {
            let page = &self.arena.pages[b.page];
            pass.set_vertex_buffer(0, page.vertices.slice(..));
            pass.set_index_buffer(page.indices.slice(..), wgpu::IndexFormat::Uint16);
            pass.multi_draw_indexed_indirect(buffer, u64::from(b.first) * ARGS_SIZE, b.count);
        }
        let mut bound = None;
        for &slot in &self.singles[p] {
            self.draw_section(pass, slot, &self.draws[slot], p, &mut bound);
        }
    }
    fn replace_mesh(&mut self, key: IVec3, mesh: GpuMesh) {
        if let Some(GpuMesh {
            storage: Some(Storage::Arena(slot)),
            ..
        }) = self.meshes.insert(key, mesh)
        {
            self.arena.free(slot);
        }
    }
    /// Draws one pass of a section; `bound` remembers the buffers in use
    /// (a page index, or `usize::MAX` for a mesh with its own buffers).
    fn draw_section(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        slot: usize,
        key: &IVec3,
        p: usize,
        bound: &mut Option<usize>,
    ) {
        let Some(mesh) = self.meshes.get(key) else {
            return;
        };
        let Some(storage) = &mesh.storage else {
            return;
        };
        // Translucent faces are double-sided; only cull solid passes.
        let mask = if p == 2 {
            u8::MAX
        } else {
            facing_groups(*key, self.eye)
        };
        let ranges = draw_ranges(&mesh.ranges[p], mask);
        if ranges.is_empty() {
            return;
        }
        let instance = slot as u32..slot as u32 + 1;
        match storage {
            Storage::Arena(s) => {
                if *bound != Some(s.page) {
                    let page = &self.arena.pages[s.page];
                    pass.set_vertex_buffer(0, page.vertices.slice(..));
                    pass.set_index_buffer(page.indices.slice(..), wgpu::IndexFormat::Uint16);
                    *bound = Some(s.page);
                }
                for (start, count) in ranges {
                    let first = s.first_index + start;
                    pass.draw_indexed(first..first + count, s.base_vertex as i32, instance.clone());
                }
            }
            Storage::Own(vertices, indices) => {
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                *bound = Some(usize::MAX);
                for (start, count) in ranges {
                    pass.draw_indexed(start..start + count, 0, instance.clone());
                }
            }
        }
    }
    /// Rebuilds the cloud mesh when the camera reaches another cloud cell
    /// and places it relative to the camera.
    fn update_clouds(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        app: &App,
        origin: IVec3,
        radius: i32,
    ) {
        if !app.config.video.clouds {
            self.clouds = None;
            return;
        }
        let eye = app.camera_position().as_dvec3();
        let drift = app.time * clouds::SPEED;
        let anchor = clouds::cell_at(eye.x, eye.z, drift);
        let cells = ((radius + 2) * 16) / clouds::CELL + 2;
        if self
            .clouds
            .as_ref()
            .is_none_or(|c| c.anchor != anchor || c.radius != cells)
        {
            let vertices = clouds::build(anchor, cells);
            let buffer = (!vertices.is_empty()).then(|| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("clouds"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
            });
            self.clouds = Some(CloudMesh {
                anchor,
                radius: cells,
                buffer,
                vertices: vertices.len() as u32,
            });
        }
        let corner = glam::DVec3::new(
            f64::from(anchor.x * clouds::CELL) + drift,
            f64::from(clouds::HEIGHT),
            f64::from(anchor.y * clouds::CELL),
        ) - origin.as_dvec3();
        queue.write_buffer(
            &self.origins,
            (CLOUD_SLOT * 16) as u64,
            bytemuck::cast_slice(&[corner.as_vec3().extend(0.0).to_array()]),
        );
    }
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.blit);
        pass.set_bind_group(0, &self.blit_group, &[]);
        pass.set_vertex_buffer(0, self.fullscreen.slice(..));
        pass.draw(0..3, 0..1);
    }
}
/// Face groups of a section that can face the camera: an east face is only
/// visible from east of its plane, so it is skipped when the eye is west of
/// the whole section. Bit 6 (other faces) is always set.
fn facing_groups(key: IVec3, eye: Vec3) -> u8 {
    let min = (key * CHUNK).as_vec3();
    let max = min + CHUNK as f32;
    let mut mask = 1 << 6;
    for (bit, visible) in [
        (0, eye.x > min.x),
        (1, eye.x < max.x),
        (2, eye.y > min.y),
        (3, eye.y < max.y),
        (4, eye.z > min.z),
        (5, eye.z < max.z),
    ] {
        if visible {
            mask |= 1 << bit;
        }
    }
    mask
}
/// Index ranges of the groups in `mask`, merging neighbouring groups.
fn draw_ranges(ranges: &[(u32, u32); GROUPS], mask: u8) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::with_capacity(4);
    for g in GROUP_ORDER {
        let (start, count) = ranges[g];
        if count == 0 || mask & (1 << g) == 0 {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.0 + last.1 == start => last.1 += count,
            _ => out.push((start, count)),
        }
    }
    out
}
fn opposite(face: crate::world::block::Face) -> crate::world::block::Face {
    use crate::world::block::Face::*;
    match face {
        East => West,
        West => East,
        Up => Down,
        Down => Up,
        South => North,
        North => South,
    }
}
/// The liquid the eye is inside, below its surface.
fn submerged(world: &World, eye: Vec3) -> Option<u8> {
    let p = eye.floor().as_ivec3();
    let id = world.get(p);
    if !block::liquid(id) {
        return None;
    }
    let height = if world.get(p + IVec3::Y) == id {
        16
    } else {
        block::liquid_height(world.state(p))
    };
    ((eye.y - p.y as f32) * 16.0 < height as f32).then_some(id)
}
/// A full section whose six neighbours are full has no visible faces.
fn enclosed(world: &World, key: IVec3) -> bool {
    let full = |k: IVec3| {
        if k.y < 0 {
            return true;
        }
        if k.y >= chunk::SECTIONS as i32 {
            return false;
        }
        world
            .column(IVec2::new(k.x, k.z))
            .and_then(|c| c.section(k.y as usize))
            .is_some_and(|s| s.opaque() == chunk::CELLS)
    };
    full(key)
        && [
            IVec3::X,
            IVec3::NEG_X,
            IVec3::Y,
            IVec3::NEG_Y,
            IVec3::Z,
            IVec3::NEG_Z,
        ]
        .into_iter()
        .all(|d| full(key + d))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eye_below_a_liquid_surface_is_submerged() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 1);
        w.set(IVec3::new(2, 40, 2), block::WATER);
        w.set(IVec3::new(2, 41, 2), block::WATER);
        w.set_with_state(IVec3::new(5, 40, 5), block::LAVA, 4);
        assert_eq!(
            submerged(&w, Vec3::new(2.5, 40.95, 2.5)),
            Some(block::WATER)
        );
        assert_eq!(submerged(&w, Vec3::new(2.5, 41.5, 2.5)), Some(block::WATER));
        assert_eq!(
            submerged(&w, Vec3::new(2.5, 41.95, 2.5)),
            None,
            "above the top surface"
        );
        assert_eq!(submerged(&w, Vec3::new(5.5, 40.2, 5.5)), Some(block::LAVA));
        assert_eq!(
            submerged(&w, Vec3::new(5.5, 40.6, 5.5)),
            None,
            "level 4 is 7/16 high"
        );
        assert_eq!(submerged(&w, Vec3::new(8.5, 40.5, 8.5)), None);
    }
    #[test]
    fn back_facing_groups_are_skipped_and_ranges_merge() {
        // Eye east of, above and north of the section.
        let mask = facing_groups(IVec3::new(0, 1, 0), Vec3::new(20.0, 40.0, -3.0));
        assert_eq!(mask, 1 << 0 | 1 << 2 | 1 << 5 | 1 << 6);
        // Inside the section every group may face the camera.
        assert_eq!(facing_groups(IVec3::ZERO, Vec3::splat(8.0)), 0x7f);
        // Storage order east, up, south, other, north, down, west.
        let mut ranges = [(0, 0); GROUPS];
        let mut at = 0;
        for g in GROUP_ORDER {
            ranges[g] = (at, 6);
            at += 6;
        }
        assert_eq!(draw_ranges(&ranges, 0x7f), [(0, 42)]);
        // East, up, south and other are one range; west, down, north too.
        assert_eq!(
            draw_ranges(&ranges, 1 | 1 << 2 | 1 << 4 | 1 << 6),
            [(0, 24)]
        );
        assert_eq!(
            draw_ranges(&ranges, 1 << 1 | 1 << 3 | 1 << 5 | 1 << 6),
            [(18, 24)]
        );
        assert_eq!(
            draw_ranges(&ranges, 1 | 1 << 3 | 1 << 6),
            [(0, 6), (18, 6), (30, 6)]
        );
        assert_eq!(draw_ranges(&ranges, 1 << 3), [(30, 6)]);
    }
    #[test]
    fn frustum_rejects_chunks_behind_camera() {
        let m = Mat4::perspective_rh(70.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        let f = Frustum::new(m);
        assert!(f.section(Vec3::new(-8.0, -8.0, -32.0)));
        assert!(!f.section(Vec3::new(-8.0, -8.0, 16.0)), "behind");
        assert!(f.section(Vec3::new(-8.0, -8.0, -8.0)), "around the eye");
        assert!(
            !f.section(Vec3::new(-8.0, -8.0, -150.0)),
            "beyond the far plane"
        );
        assert!(
            !f.section(Vec3::new(200.0, -8.0, -40.0)),
            "far to the right"
        );
        // Rotated view: looking along +X.
        let f = Frustum::new(m * Mat4::look_to_rh(Vec3::ZERO, Vec3::X, Vec3::Y));
        assert!(f.section(Vec3::new(30.0, -8.0, -8.0)));
        assert!(!f.section(Vec3::new(-46.0, -8.0, -8.0)));
    }
    #[test]
    #[ignore = "requires a Vulkan adapter"]
    fn gpu_scene_smoke() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: wgpu::Limits::downlevel_defaults(),
            ..Default::default()
        }))
        .unwrap();
        let mut scene = Scene::new(
            &device,
            &queue,
            wgpu::TextureFormat::Rgba8Unorm,
            Arc::new(Pool::new(1, 1)),
        );
        let mut world = World::new(0);
        world.empty_area(IVec2::ZERO, 2);
        world.set(IVec3::new(8, 8, 8), 1);
        world.set(IVec3::new(8, 8, 7), 3);
        let mesh = Job::new(&world, IVec3::ZERO, 1, STYLE_SMOOTH)
            .unwrap()
            .build();
        scene.meshes.insert(
            IVec3::ZERO,
            GpuMesh {
                storage: Some(Storage::Own(
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: None,
                        contents: bytemuck::cast_slice(&mesh.vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: None,
                        contents: bytemuck::cast_slice(&mesh.indices.to_vec()),
                        usage: wgpu::BufferUsages::INDEX,
                    }),
                )),
                ranges: mesh.ranges,
                visibility: mesh.visibility,
                revision: 1,
                style: 1,
            },
        );
        scene.draws = vec![IVec3::ZERO];
        let eye = Vec3::new(8.5, 9.7, 11.0);
        scene.eye = eye;
        let projection = Mat4::perspective_rh(70.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        scene.matrix = projection * Mat4::look_at_rh(eye, Vec3::splat(8.5), Vec3::Y);
        let noon = sky::sky(6_000.0);
        queue.write_buffer(&scene.origins, 0, bytemuck::cast_slice(&[[0.0f32; 4]]));
        queue.write_buffer(
            &scene.uniform,
            0,
            bytemuck::bytes_of(&Camera {
                matrix: scene.matrix.to_cols_array_2d(),
                inverse: (projection
                    * Mat4::look_at_rh(Vec3::ZERO, Vec3::splat(8.5) - eye, Vec3::Y))
                .inverse()
                .to_cols_array_2d(),
                eye: eye.extend(1.0).to_array(),
                fog: [noon.fog.x, noon.fog.y, noon.fog.z, 48.0],
                zenith: noon.zenith.extend(noon.daylight).to_array(),
                horizon: noon.horizon.extend(noon.stars).to_array(),
                sun: noon.sun.extend(noon.phase).to_array(),
                sun_right: noon.sun.cross(Vec3::Z).normalize().extend(0.0).to_array(),
                sun_up: Vec3::Y.extend(28.0).to_array(),
                rotation: (projection
                    * Mat4::look_at_rh(Vec3::ZERO, Vec3::splat(8.5) - eye, Vec3::Y))
                .to_cols_array_2d(),
            }),
        );
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 3840 * 540,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        scene.prepare(&mut encoder);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &scene._color_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(3840),
                    rows_per_image: Some(540),
                },
            },
            scene._color_texture.size(),
        );
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).unwrap();
        });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = buffer.slice(..).get_mapped_range().unwrap();
        let center = &bytes[(270 * 960 + 480) * 4..][..4];
        assert!(
            center[0] > center[1] && center[1] > center[2],
            "cube center={center:?}"
        );
        let green = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[1] > p[0] && p[1] > p[2])
            .count();
        assert!(green > 500, "missing grass top: {green} green pixels");
        let sky = &bytes[0..4];
        assert!(sky[2] > sky[1] && sky[1] > sky[0], "sky={sky:?}");
    }
}

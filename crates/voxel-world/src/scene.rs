use crate::{
    app::App,
    jobs::{Kind, Pool},
    mesh::{Job, Mesh, Vertex},
    render::{sky, textures},
    world::{self, CHUNK, World, chunk},
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
    /// Basis of the sun and moon discs.
    sun_right: [f32; 4],
    sun_up: [f32; 4],
    /// Rotation-only view-projection, for directions at infinity.
    rotation: [[f32; 4]; 4],
}
struct GpuMesh {
    /// `None` for sections without visible faces.
    buffers: Option<(wgpu::Buffer, wgpu::Buffer, wgpu::IndexFormat)>,
    count: u32,
    revision: u64,
    style: u32,
}
/// Most sections drawn in one frame (view distance 6 needs about 1 400).
const MAX_DRAWS: usize = 4096;
const SKY_GRID: u32 = 24 * 14;
const STARS: u32 = 360;

struct SkyPipelines {
    gradient: wgpu::RenderPipeline,
    discs: wgpu::RenderPipeline,
    stars: wgpu::RenderPipeline,
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
        let make = |label: &str, vs: &str, fs: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
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
                // Drawn after opaque terrain at the far plane, so only
                // uncovered pixels are shaded.
                depth_stencil: Some(depth_state(false, wgpu::CompareFunction::LessEqual)),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            gradient: make("sky gradient", "vs_gradient", "fs_gradient", None),
            discs: make("sun and moon", "vs_disc", "fs_disc", None),
            stars: make(
                "stars",
                "vs_star",
                "fs_star",
                Some(wgpu::BlendState::ALPHA_BLENDING),
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
    pipeline: wgpu::RenderPipeline,
    sky: SkyPipelines,
    stars: bool,
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
}
pub fn visible(matrix: Mat4, origin: Vec3) -> bool {
    let points: Vec<_> = (0..8)
        .map(|i| {
            matrix
                * (origin
                    + Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32)
                        * 16.0)
                    .extend(1.0)
        })
        .collect();
    !(points.iter().all(|p| p.x < -p.w)
        || points.iter().all(|p| p.x > p.w)
        || points.iter().all(|p| p.y < -p.w)
        || points.iter().all(|p| p.y > p.w)
        || points.iter().all(|p| p.z < 0.0)
        || points.iter().all(|p| p.z > p.w))
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
fn voxel_pipeline(device: &wgpu::Device) -> wgpu::RenderPipeline {
    let module = shader(device, "voxel shader", include_str!("voxel.wgsl"));
    let vertex = wgpu::vertex_attr_array![0 => Uint16x4, 1 => Sint16x2, 2 => Uint8x4];
    let instance = wgpu::vertex_attr_array![3 => Float32x4];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("voxel scene"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[
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
            ],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(depth_state(true, wgpu::CompareFunction::Less)),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
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
        let pipeline = voxel_pipeline(device);
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
            size: (MAX_DRAWS * 16) as u64,
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
            layout: &pipeline.get_bind_group_layout(0),
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
            pipeline,
            sky,
            stars: false,
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
            pending: HashMap::new(),
            draws: Vec::new(),
            pool,
            results,
            sender,
            epoch: 0,
            fog: [0.7, 0.8, 0.95, 64.0],
            matrix: Mat4::IDENTITY,
            origin: IVec3::ZERO,
        }
    }
    /// Sections with a GPU mesh (including empty ones).
    pub fn loaded(&self) -> usize {
        self.meshes.len()
    }
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
    /// Uploads finished meshes and schedules new ones; returns upload ms.
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, app: &App) -> f32 {
        if self.epoch != app.epoch {
            self.meshes.clear();
            self.pending.clear();
            while self.results.try_recv().is_ok() {}
            self.epoch = app.epoch;
        }
        let world = &app.world;
        let smooth = app.config.video.smooth_lighting;
        let style = u32::from(smooth);
        let eye = app.camera_position();
        let origin = eye.floor().as_ivec3();
        let center = world::column_at(eye);
        let radius = app.config.video.view_distance;
        let nearby =
            |key: IVec3| (IVec2::new(key.x, key.z) - center).abs().max_element() <= radius + 1;
        self.meshes
            .retain(|&key, _| nearby(key) && world.section_revision(key).is_some());
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
            let buffers = (!mesh.indices.is_empty()).then(|| {
                (
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
                    mesh.indices.format(),
                )
            });
            self.meshes.insert(
                mesh.key,
                GpuMesh {
                    buffers,
                    count: mesh.indices.len() as u32,
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
        self.origin = origin;
        let relative = |key: IVec3| (key * CHUNK - origin).as_vec3();
        let eye_section = world::section_of(origin);
        let mut candidates = Vec::new();
        for column in world.columns() {
            let offset = column.pos - center;
            if offset.abs().max_element() > radius {
                continue;
            }
            for s in 0..chunk::SECTIONS {
                let key = IVec3::new(column.pos.x, s as i32, column.pos.y);
                let revision = column.revisions[s];
                if self.pending.contains_key(&key)
                    || self.meshes.get(&key).is_some_and(|m| {
                        m.revision == revision && (m.style == style || m.count == 0)
                    })
                {
                    continue;
                }
                if column.is_empty(s) || enclosed(world, key) {
                    // Nothing to draw; record it without a background job.
                    self.meshes.insert(
                        key,
                        GpuMesh {
                            buffers: None,
                            count: 0,
                            revision,
                            style,
                        },
                    );
                    continue;
                }
                let d = key - eye_section;
                let stale = self.meshes.contains_key(&key);
                let priority = i64::from(offset.length_squared()) * 2
                    + i64::from(d.y * d.y) / 2
                    + 3
                    + if visible(self.matrix, relative(key)) { 0 } else { 64 }
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
            let Some(job) = Job::new(world, key, app.epoch, style, smooth) else {
                continue;
            };
            self.pending.insert(key, (job.revision, job.epoch));
            let sender = self.sender.clone();
            self.pool.submit(Kind::Mesh, priority, move || {
                let _ = sender.send(job.build());
            });
            submitted += 1;
        }
        // Visible sections, nearest first, with camera-relative origins.
        let mut draws: Vec<(i32, IVec3)> = self
            .meshes
            .iter()
            .filter(|(key, mesh)| mesh.buffers.is_some() && visible(self.matrix, relative(**key)))
            .map(|(key, _)| ((*key * CHUNK + 8 - origin).length_squared(), *key))
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
        let sky = sky::sky(app.time);
        let sun_right = sky.sun.cross(Vec3::Z).normalize();
        let rotation = projection * Mat4::look_to_rh(Vec3::ZERO, direction, Vec3::Y);
        self.fog = [sky.fog.x, sky.fog.y, sky.fog.z, radius as f32 * 16.0];
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&Camera {
                matrix: self.matrix.to_cols_array_2d(),
                inverse: rotation.inverse().to_cols_array_2d(),
                eye: fract.extend(1.0).to_array(),
                fog: self.fog,
                zenith: sky.zenith.extend(sky.daylight).to_array(),
                horizon: sky.horizon.extend(sky.stars).to_array(),
                sun: sky.sun.extend(sky.phase).to_array(),
                sun_right: sun_right.extend(sky.dusk).to_array(),
                sun_up: sun_right.cross(sky.sun).extend(0.0).to_array(),
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        for (slot, key) in self.draws.iter().enumerate() {
            let Some(mesh) = self.meshes.get(key) else {
                continue;
            };
            let Some((vertices, indices, format)) = &mesh.buffers else {
                continue;
            };
            let at = (slot * 16) as u64;
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_vertex_buffer(1, self.origins.slice(at..at + 16));
            pass.set_index_buffer(indices.slice(..), *format);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
        pass.set_bind_group(0, &self.sky.group, &[]);
        pass.set_pipeline(&self.sky.gradient);
        pass.draw(0..SKY_GRID * 6, 0..1);
        if self.stars {
            pass.set_pipeline(&self.sky.stars);
            pass.draw(0..6, 0..STARS);
        }
        pass.set_pipeline(&self.sky.discs);
        pass.draw(0..6, 0..2);
    }
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.blit);
        pass.set_bind_group(0, &self.blit_group, &[]);
        pass.set_vertex_buffer(0, self.fullscreen.slice(..));
        pass.draw(0..3, 0..1);
    }
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
            .is_some_and(|s| s.count() == chunk::CELLS)
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
    fn frustum_rejects_chunks_behind_camera() {
        let m = Mat4::perspective_rh(70.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        assert!(visible(m, Vec3::new(-8.0, -8.0, -32.0)));
        assert!(!visible(m, Vec3::new(-8.0, -8.0, 16.0)));
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
        let mesh = Job::new(&world, IVec3::ZERO, 1, 1, true).unwrap().build();
        scene.meshes.insert(
            IVec3::ZERO,
            GpuMesh {
                buffers: Some((
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: None,
                        contents: bytemuck::cast_slice(&mesh.vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: None,
                        contents: mesh.indices.bytes(),
                        usage: wgpu::BufferUsages::INDEX,
                    }),
                    mesh.indices.format(),
                )),
                count: mesh.indices.len() as u32,
                revision: 1,
                style: 1,
            },
        );
        scene.draws = vec![IVec3::ZERO];
        let eye = Vec3::new(8.5, 9.7, 11.0);
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
                sun_up: Vec3::Y.extend(0.0).to_array(),
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

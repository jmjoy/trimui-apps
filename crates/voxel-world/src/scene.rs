use crate::{
    app::App,
    mesh::{Job, Vertex, Worker},
    world::{self, CHUNK, COLORS},
};
use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Mat4, Vec3};
use std::collections::HashMap;
use wgpu::util::DeviceExt;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Camera {
    matrix: [[f32; 4]; 4],
    eye: [f32; 4],
    fog: [f32; 4],
}
struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    revision: u64,
}
pub struct Scene {
    pipeline: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    blit_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    fullscreen: wgpu::Buffer,
    color: wgpu::TextureView,
    _color_texture: wgpu::Texture,
    depth: wgpu::TextureView,
    meshes: HashMap<usize, GpuMesh>,
    pending: HashMap<usize, (u64, u64)>,
    worker: Worker,
    epoch: u64,
    pub matrix: Mat4,
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
fn pipeline(
    device: &wgpu::Device,
    source: &str,
    format: wgpu::TextureFormat,
    voxel: bool,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("voxel shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2,2=>Uint32,3=>Float32];
    let blit_attributes = wgpu::vertex_attr_array![0=>Float32x2];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if voxel {
            "voxel scene"
        } else {
            "scene upscale"
        }),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: if voxel {
                    std::mem::size_of::<Vertex>() as u64
                } else {
                    8
                },
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: if voxel { &attributes } else { &blit_attributes },
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: if voxel { Some(wgpu::Face::Back) } else { None },
            ..Default::default()
        },
        depth_stencil: if voxel {
            Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            })
        } else {
            None
        },
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}
impl Scene {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let pipeline = pipeline(
            device,
            include_str!("voxel.wgsl"),
            wgpu::TextureFormat::Rgba8Unorm,
            true,
        );
        let blit = pipeline_fn(device, format);
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&Camera::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("procedural pixel blocks"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 12,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut pixels = Vec::new();
        for layer in 0..12 {
            for y in 0..16u32 {
                for x in 0..16u32 {
                    let block = if layer == 10 {
                        if y < 3 { 1 } else { 2 }
                    } else if layer == 11 {
                        7
                    } else {
                        layer
                    };
                    let base = COLORS[block];
                    let n = (((x * 17 + y * 113 + layer as u32 * 71).wrapping_mul(1103515245)
                        >> 16)
                        % 25) as i32
                        - 12;
                    let pattern = match layer {
                        5 if x % 5 == 0 => -25,
                        7 if y % 4 == 0 || (x + (y / 4) * 7) % 16 == 0 => -28,
                        8 if y % 5 == 0 || (x + (y / 5) * 8) % 16 == 0 => 35,
                        11 if x.min(15 - x).min(y.min(15 - y)) % 3 == 0 => -25,
                        _ => 0,
                    };
                    for c in base {
                        pixels.push((i32::from(c) + n + pattern).clamp(0, 255) as u8);
                    }
                    pixels.push(255);
                }
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(64),
                rows_per_image: Some(16),
            },
            texture.size(),
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
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
            blit,
            group,
            blit_group,
            uniform,
            fullscreen,
            color,
            _color_texture: color_texture,
            depth,
            meshes: HashMap::new(),
            pending: HashMap::new(),
            worker: Worker::new(),
            epoch: 0,
            matrix: Mat4::IDENTITY,
        }
    }
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, app: &App) {
        if self.epoch != app.epoch {
            self.meshes.clear();
            self.pending.clear();
            self.epoch = app.epoch;
        }
        let eye = app.camera_position();
        let center = app.player.position.floor().as_ivec3() / CHUNK;
        let radius = app.settings.view_distance;
        let nearby = |id| {
            let c = world::chunk_pos(id);
            (c.x - center.x).abs() <= radius + 1 && (c.z - center.z).abs() <= radius + 1
        };
        self.meshes.retain(|&id, _| nearby(id));
        for _ in 0..2 {
            let Ok(mesh) = self.worker.receiver.try_recv() else {
                break;
            };
            if self.pending.get(&mesh.id) == Some(&(mesh.revision, mesh.epoch)) {
                self.pending.remove(&mesh.id);
            }
            if mesh.epoch != app.epoch
                || mesh.revision != app.world.revisions[mesh.id]
                || !nearby(mesh.id)
            {
                continue;
            }
            let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            self.meshes.insert(
                mesh.id,
                GpuMesh {
                    vertices,
                    indices,
                    count: mesh.indices.len() as u32,
                    revision: mesh.revision,
                },
            );
        }
        self.matrix = Mat4::perspective_rh(
            70.0f32.to_radians(),
            16.0 / 9.0,
            0.05,
            (radius as f32 + 2.0) * 16.0,
        ) * Mat4::look_to_rh(eye, app.player.direction(), Vec3::Y);
        let mut candidates = Vec::new();
        for y in 0..4 {
            for z in (center.z - radius).max(0)..=(center.z + radius).min(15) {
                for x in (center.x - radius).max(0)..=(center.x + radius).min(15) {
                    let c = IVec3::new(x, y, z);
                    let id = world::chunk_id(c);
                    if !self.pending.contains_key(&id)
                        && self
                            .meshes
                            .get(&id)
                            .is_none_or(|m| m.revision != app.world.revisions[id])
                    {
                        let d = c - center;
                        let priority = d.x * d.x
                            + d.z * d.z
                            + d.y * d.y
                            + if visible(self.matrix, (c * CHUNK).as_vec3()) {
                                0
                            } else {
                                128
                            };
                        candidates.push((priority, id));
                    }
                }
            }
        }
        candidates.sort_unstable();
        for (_, id) in candidates.into_iter().take(2) {
            let job = Job::new(&app.world, id, app.epoch);
            let key = (job.revision, job.epoch);
            if self.worker.sender.try_send(job).is_ok() {
                self.pending.insert(id, key);
            } else {
                break;
            }
        }
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&Camera {
                matrix: self.matrix.to_cols_array_2d(),
                eye: eye.extend(1.0).to_array(),
                fog: [0.52, 0.73, 0.88, radius as f32 * 16.0],
            }),
        );
    }
    pub fn prepare(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voxel world"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.52,
                        g: 0.73,
                        b: 0.88,
                        a: 1.0,
                    }),
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
        for (&id, mesh) in &self.meshes {
            if mesh.count == 0 || !visible(self.matrix, (world::chunk_pos(id) * CHUNK).as_vec3()) {
                continue;
            }
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
    }
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.blit);
        pass.set_bind_group(0, &self.blit_group, &[]);
        pass.set_vertex_buffer(0, self.fullscreen.slice(..));
        pass.draw(0..3, 0..1);
    }
}
fn pipeline_fn(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    pipeline(device, include_str!("blit.wgsl"), format, false)
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
        let mut scene = Scene::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        let mut world = world::World {
            seed: 0,
            blocks: vec![0; world::COUNT],
            revisions: vec![1; world::CHUNKS],
        };
        world.set(IVec3::new(8, 8, 8), 1);
        world.set(IVec3::new(8, 8, 7), 3);
        let mesh = Job::new(&world, 0, 1).build();
        scene.meshes.insert(
            0,
            GpuMesh {
                vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&mesh.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&mesh.indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                count: mesh.indices.len() as u32,
                revision: 1,
            },
        );
        let eye = Vec3::new(8.5, 9.7, 11.0);
        scene.matrix = Mat4::perspective_rh(70.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0)
            * Mat4::look_at_rh(eye, Vec3::splat(8.5), Vec3::Y);
        queue.write_buffer(
            &scene.uniform,
            0,
            bytemuck::bytes_of(&Camera {
                matrix: scene.matrix.to_cols_array_2d(),
                eye: eye.extend(1.0).to_array(),
                fog: [0.52, 0.73, 0.88, 48.0],
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

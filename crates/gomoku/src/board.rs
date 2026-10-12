use crate::{
    game::{Outcome, Position},
    session::Session,
};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Debug)]
pub struct BoardLayout {
    pub rect: egui::Rect,
}
impl BoardLayout {
    pub fn origin(self) -> egui::Pos2 {
        self.rect.min + egui::Vec2::splat(self.rect.width() * 0.085)
    }
    pub fn spacing(self) -> f32 {
        self.rect.width() * 0.83 / 14.0
    }
    #[cfg(test)]
    pub fn position(self, pointer: egui::Pos2) -> Option<Position> {
        let p = (pointer - self.origin()) / self.spacing();
        let x = p.x.round();
        let y = p.y.round();
        if !(0.0..=14.0).contains(&x) || !(0.0..=14.0).contains(&y) {
            return None;
        }
        if (p.x - x).abs() > 0.46 || (p.y - y).abs() > 0.46 {
            return None;
        }
        Some(Position::new(x as usize, y as usize))
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
struct Scene {
    screen: [f32; 4],
    board: [f32; 4],
    marks: [f32; 4],
    animation: [f32; 4],
    winning: [f32; 4],
    cells: [[u32; 4]; 57],
}
impl Scene {
    fn new(
        size: [u32; 2],
        dpi: f32,
        layout: BoardLayout,
        session: &Session,
        hover: Option<Position>,
        elapsed: f32,
    ) -> Self {
        let mut scene = Self::zeroed();
        scene.screen = [size[0] as f32, size[1] as f32, dpi, 1.0];
        scene.board = [
            layout.rect.min.x * dpi,
            layout.rect.min.y * dpi,
            layout.rect.width() * dpi,
            0.0,
        ];
        scene.marks = [
            session
                .game
                .history
                .last()
                .map_or(-1.0, |m| m.pos.index() as f32),
            session.hint.map_or(-1.0, |p| p.index() as f32),
            hover.map_or(-1.0, |p| p.index() as f32),
            session.human as u8 as f32,
        ];
        scene.animation[0] = elapsed;
        scene.winning = [-1.0; 4];
        if let Outcome::Won { line, .. } = &session.game.outcome
            && let (Some(a), Some(b)) = (line.first(), line.last())
        {
            scene.winning = [a.x as f32, a.y as f32, b.x as f32, b.y as f32];
        }
        for (i, &stone) in session.game.board.cells.iter().enumerate() {
            scene.cells[i / 4][i % 4] = u32::from(stone);
        }
        scene
    }
}

struct BoardCache {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    size: [u32; 2],
}

pub struct BoardRenderer {
    pipeline: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    cache: Option<BoardCache>,
    scene: Option<Scene>,
    dirty: bool,
    vertices: wgpu::Buffer,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}
impl BoardRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let pipeline = create_pipeline(
            device,
            format,
            include_str!("board.wgsl"),
            "wooden gomoku board",
        );
        let blit = create_pipeline(
            device,
            format,
            include_str!("board-blit.wgsl"),
            "cached gomoku board",
        );
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("board fullscreen triangle"),
            contents: bytemuck::cast_slice(&[[-1.0_f32, -1.0], [3.0, -1.0], [-1.0, 3.0]]),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("board scene"),
            contents: bytemuck::bytes_of(&Scene::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("board scene bind group"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            pipeline,
            blit,
            cache: None,
            scene: None,
            dirty: true,
            vertices,
            buffer,
            bind_group,
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: [u32; 2],
        layout: BoardLayout,
        session: &Session,
        hover: Option<Position>,
        elapsed: f32,
        format: wgpu::TextureFormat,
    ) {
        let mut scene = Scene::new(size, 1.0, layout, session, hover, elapsed.min(0.18));
        scene.screen[3] = f32::from(format.is_srgb());
        if self.cache.as_ref().is_none_or(|cache| cache.size != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("cached wooden board"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cached board bind group"),
                layout: &self.blit.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            self.cache = Some(BoardCache {
                _texture: texture,
                view,
                bind_group,
                size,
            });
            self.dirty = true;
        }
        if self.scene.as_ref() != Some(&scene) {
            queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&scene));
            self.scene = Some(scene);
            self.dirty = true;
        }
    }

    // Rebuild only when stones, marks, layout, or the short placement animation change.
    pub fn prepare(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.dirty {
            return;
        }
        let cache = self
            .cache
            .as_ref()
            .expect("update the board before rendering");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("refresh wooden board cache"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &cache.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
        self.dirty = false;
    }

    #[cfg(test)]
    pub fn paint(&mut self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        self.prepare(encoder);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("board"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        self.paint_in_pass(&mut pass);
    }

    pub fn paint_in_pass(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.blit);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_bind_group(
            0,
            &self
                .cache
                .as_ref()
                .expect("prepare the board before rendering")
                .bind_group,
            &[],
        );
        pass.draw(0..3, 0..1);
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    source: &str,
    label: &str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2],
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
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::SIZE;

    #[test]
    #[ignore = "requires the PNG captured by just input-test gomoku"]
    fn device_capture_contains_board_and_both_stones() {
        let path = std::env::var("GOMOKU_DEVICE_SMOKE_PNG")
            .expect("Set GOMOKU_DEVICE_SMOKE_PNG to the input-test game.png");
        let log =
            std::fs::read_to_string(std::path::Path::new(&path).with_file_name("input-test.log"))
                .expect("Keep input-test.log beside the captured game.png");
        let decoder =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut bytes).unwrap();
        assert_eq!((frame.width, frame.height), (1280, 720));
        assert_eq!(frame.color_type, png::ColorType::Rgba);
        let sample = |x: usize, y: usize| {
            let i = (y * 1280 + x) * 4;
            [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
        };
        // Use the logged opening moves: a time-limited AI may choose different replies.
        // Sample stone bodies away from the last-move dot and the cursor border.
        let stone = |pattern: &str| {
            let coordinate = log
                .lines()
                .find_map(|line| line.split_once(pattern).map(|(_, rest)| rest))
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap();
            let x = usize::from(coordinate.as_bytes()[0] - b'A');
            let y = SIZE - coordinate[1..].parse::<usize>().unwrap();
            let step = 688.0 * 0.83 / 14.0;
            let origin = 16.0 + 688.0 * 0.085;
            sample(
                (origin + x as f32 * step + 6.0).round() as usize,
                (origin + y as f32 * step + 6.0).round() as usize,
            )
        };
        let wood = sample(130, 130);
        let black = stone("Move 1: Black ");
        let white = stone("Move 2: White ");
        assert!(
            wood[0] > 160 && wood[0] > wood[1] && wood[1] > wood[2],
            "wood={wood:?}"
        );
        assert!(
            black[0] < 120 && black[1] < 120 && black[3] == 255,
            "black={black:?}"
        );
        assert!(
            white[0] > 160 && white[1] > 160 && white[3] == 255,
            "white={white:?}"
        );

        // The setup capture has no stones covering the five star points.
        let setup_path = std::path::Path::new(&path).with_file_name("setup.png");
        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(setup_path).unwrap(),
        ));
        let mut reader = decoder.read_info().unwrap();
        let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut bytes).unwrap();
        assert_eq!((frame.width, frame.height), (1280, 720));
        assert_eq!(frame.color_type, png::ColorType::Rgba);
        let intersection = |x: usize, y: usize| {
            let step = 688.0 * 0.83 / 14.0;
            let origin = 16.0 + 688.0 * 0.085;
            let px = (origin + x as f32 * step).round() as usize;
            let py = (origin + y as f32 * step).round() as usize;
            let i = (py * 1280 + px) * 4;
            [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
        };
        for (x, y) in [(3, 3), (11, 3), (7, 7), (3, 11), (11, 11)] {
            let pixel = intersection(x, y);
            assert!(
                pixel[0] < 100 && pixel[1] < 75 && pixel[3] == 255,
                "missing star at ({x}, {y}): {pixel:?}"
            );
        }
        for (x, y) in [(7, 3), (3, 7), (7, 11), (11, 7)] {
            let pixel = intersection(x, y);
            assert!(pixel[0] > 105, "unexpected star at ({x}, {y}): {pixel:?}");
        }
    }

    #[test]
    fn picking_uses_the_rendered_grid_and_rejects_margins() {
        let layout = BoardLayout {
            rect: egui::Rect::from_min_size(egui::pos2(31.0, 80.0), egui::Vec2::splat(600.0)),
        };
        for pos in Position::all() {
            let point = layout.origin() + egui::vec2(pos.x as f32, pos.y as f32) * layout.spacing();
            assert_eq!(layout.position(point), Some(pos));
        }
        assert_eq!(layout.position(layout.rect.min), None);
        assert_eq!(
            layout.position(layout.origin() + egui::Vec2::splat(layout.spacing() * 0.5)),
            None
        );
    }
    #[test]
    #[ignore = "requires a Vulkan/Metal/DX12 adapter; run cargo test gpu_board_smoke -- --ignored"]
    fn gpu_board_smoke() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
            let mut renderer = BoardRenderer::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
            let size = 512;
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("smoke board"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: (size * size * 4) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut session = Session::default();
            for pos in [
                Position::new(7, 7),
                Position::new(8, 7),
                Position::new(6, 6),
                Position::new(8, 8),
            ] {
                session.game.play(pos).unwrap();
            }
            session.hint = Some(Position::new(5, 5));
            let layout = BoardLayout {
                rect: egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::Vec2::splat(472.0)),
            };
            renderer.update(
                &device,
                &queue,
                [size, size],
                layout,
                &session,
                None,
                1.0,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            renderer.paint(&mut encoder, &texture.create_view(&Default::default()));
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(size * 4),
                        rows_per_image: Some(size),
                    },
                },
                texture.size(),
            );
            queue.submit([encoder.finish()]);
            let (sender, receiver) = std::sync::mpsc::channel();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                sender.send(r).unwrap();
            });
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            receiver.recv().unwrap().unwrap();
            let data = buffer.slice(..).get_mapped_range().unwrap();
            let sample = |point: egui::Pos2| {
                let i = (point.y.round() as usize * size as usize + point.x.round() as usize) * 4;
                [data[i], data[i + 1], data[i + 2], data[i + 3]]
            };
            let origin = layout.origin();
            let step = layout.spacing();
            let black = sample(origin + egui::vec2(7.0, 7.0) * step);
            let white = sample(origin + egui::vec2(8.0, 7.0) * step);
            assert!(
                black[0] < 100 && black[1] < 100 && black[3] == 255,
                "black={black:?}"
            );
            assert!(white[0] > 180 && white[3] == 255, "white={white:?}");
            // The center star is covered by the black stone in this snapshot.
            for (x, y) in [(3.0, 3.0), (11.0, 3.0), (3.0, 11.0), (11.0, 11.0)] {
                let pixel = sample(origin + egui::vec2(x, y) * step);
                assert!(
                    pixel[0] < 100 && pixel[1] < 75 && pixel[3] == 255,
                    "missing star at ({x}, {y}): {pixel:?}"
                );
            }
            if let Ok(path) = std::env::var("GOMOKU_SMOKE_PNG") {
                let file = std::fs::File::create(path).unwrap();
                let mut encoder = png::Encoder::new(file, size, size);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(&data)
                    .unwrap();
            }
        });
    }
}

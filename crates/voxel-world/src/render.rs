use crate::{
    app::App,
    display::{DirectDisplay, Target},
    scene::Scene,
    ui,
};
use anyhow::{Context, Result, ensure};
use std::{fs::File, io::BufWriter, path::Path, time::Duration};

pub struct Renderer {
    egui: egui::Context,
    painter: egui_wgpu::Renderer,
    scene: Scene,
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let egui = egui::Context::default();
        egui.set_pixels_per_point(1.0);

        install_cjk_font(&egui)?;
        Ok(Self {
            scene: Scene::new(device, queue, format),
            egui,
            painter: egui_wgpu::Renderer::new(device, format, Default::default()),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &Target,
        display: &DirectDisplay,
        app: &App,
        elapsed: f64,
        fps: f64,
        capture: Option<&Path>,
    ) -> Result<()> {
        let width = display.extent.width;
        let height = display.extent.height;
        self.scene.update(device, queue, app);
        let output = self.egui.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width as f32, height as f32),
                )),
                time: Some(elapsed),
                ..Default::default()
            },
            |ui| ui::draw(ui, app, fps, self.scene.matrix),
        );
        let jobs = self.egui.tessellate(output.shapes, output.pixels_per_point);
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.painter.update_texture(device, queue, *id, delta);
            }
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("voxel-world frame"),
        });
        let callbacks = self
            .painter
            .update_buffers(device, queue, &mut encoder, &jobs, &screen);
        self.scene.prepare(&mut encoder);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voxel-world board and controls"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let mut pass = pass.forget_lifetime();
            self.scene.paint(&mut pass);
            self.painter.render(&mut pass, &jobs, &screen);
        }
        let row_bytes = (width * 4).div_ceil(256) * 256;
        let readback = if capture.is_some() {
            ensure!(
                display.can_capture,
                "Display does not support screenshot readback"
            );
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("screenshot readback"),
                size: row_bytes as u64 * height as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &target.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            Some(buffer)
        } else {
            None
        };
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: &target.texture,
                selector: None,
                state: wgpu::TextureUses::PRESENT,
            }),
        );
        // Preserve wgpu-demo's workaround for the PowerVR timeline wait bug.
        display.prepare_submit(queue)?;
        let submission = queue.submit(callbacks.into_iter().chain([encoder.finish()]));
        device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(5)),
        })?;
        for id in &output.textures_delta.free {
            self.painter.free_texture(id);
        }
        if let (Some(path), Some(buffer)) = (capture, readback) {
            save_capture(
                device,
                &buffer,
                path,
                width,
                height,
                row_bytes,
                display.format,
            )?;
        }
        Ok(())
    }
}

fn install_cjk_font(ctx: &egui::Context) -> Result<()> {
    let configured = std::env::var("CHINESE_CHESS_FONT").ok();
    let candidates = [
        configured.as_deref().unwrap_or(""),
        "/usr/trimui/res/full.ttf",
        "/usr/trimui/res/regular.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ];
    let path = candidates
        .iter()
        .find(|path| Path::new(path).is_file())
        .context("No Chinese font found; set CHINESE_CHESS_FONT to a CJK TTF/TTC")?;
    let bytes = std::fs::read(path).with_context(|| format!("Reading font {path}"))?;
    let mut definitions = egui::FontDefinitions::default();
    definitions
        .font_data
        .insert("cjk".into(), egui::FontData::from_owned(bytes).into());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        definitions
            .families
            .entry(family)
            .or_default()
            .insert(0, "cjk".into());
    }
    ctx.set_fonts(definitions);
    log::info!("Chinese font: {path}");
    Ok(())
}
fn save_capture(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    path: &Path,
    width: u32,
    height: u32,
    row_bytes: u32,
    format: wgpu::TextureFormat,
) -> Result<()> {
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(5)),
    })?;
    receiver.recv_timeout(Duration::from_secs(5))??;
    let bytes = buffer.slice(..).get_mapped_range()?;
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for row in bytes.chunks_exact(row_bytes as usize) {
        for pixel in row[..width as usize * 4].as_chunks::<4>().0 {
            if format == wgpu::TextureFormat::Bgra8Unorm {
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            } else {
                rgba.extend_from_slice(pixel);
            }
        }
    }
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    drop(bytes);
    buffer.unmap();
    log::info!("Screenshot: {}", path.display());
    Ok(())
}

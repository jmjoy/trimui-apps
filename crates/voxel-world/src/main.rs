mod app;
mod audio;
mod display;
mod input;
mod mesh;
mod player;
mod render;
mod save;
mod scene;
mod ui;
mod world;
use anyhow::{Context, Result};
use clap::Parser;
use std::{
    path::PathBuf,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Parser)]
#[command(
    name = "trimui-voxel-world",
    version,
    about = "Creative voxel sandbox for TrimUI Smart Pro",
    after_help = "Left stick: move; right stick: look; R2: break; L2: place; L/R: blocks; B: jump/ascend; A: descend; X: flight; Y: palette; START: pause; SELECT + START: save and exit."
)]
struct Options {
    #[arg(long,default_value_t=0.0,value_parser=parse_duration)]
    seconds: f64,
    #[arg(long)]
    probe: bool,
    #[arg(long)]
    capture: Option<PathBuf>,
    #[arg(long,default_value_t=2.0,value_parser=parse_duration)]
    capture_after: f64,
    #[arg(long, default_value = "/dev/input/js0")]
    controller: String,
    /// Directory for the single world save; isolated directories are useful for testing
    #[arg(long, default_value = "save")]
    save_dir: PathBuf,
    /// Terrain seed when there is no existing world
    #[arg(long)]
    seed: Option<u64>,
}
fn parse_duration(s: &str) -> std::result::Result<f64, String> {
    let n = s.parse::<f64>().map_err(|e| e.to_string())?;
    if n.is_finite() && n >= 0.0 {
        Ok(n)
    } else {
        Err("Duration must be finite and non-negative".into())
    }
}
fn main() -> ExitCode {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,trimui_voxel_world=info"),
    )
    .init();
    match run(Options::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
fn run(options: Options) -> Result<()> {
    let running = Arc::new(AtomicBool::new(true));
    let flag = running.clone();
    ctrlc::set_handler(move || flag.store(false, Ordering::Relaxed))?;
    let instance = display::create_instance()?;
    let adapter = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN))
        .into_iter()
        .next()
        .context("No Vulkan GPU found")?;
    log::info!("GPU: {:?}", adapter.get_info());
    let surface = display::DisplaySurface::new(&instance, &adapter)?;
    if options.probe {
        log::info!("Native surface probe passed; no frames presented");
        return Ok(());
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("TrimUI Voxel World"),
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))?;
    let display = display::DirectDisplay::new(surface, &device)?;
    let mut renderer = render::Renderer::new(&device, &queue, display.format)?;
    let mut gamepad = input::Gamepad::open(&options.controller)?;
    let mut input = input::Frame::default();
    let mut app = app::App::new(options.save_dir, options.seed);
    let start = Instant::now();
    let mut previous = start;
    let mut sample = start;
    let mut sample_frames = 0;
    let mut frames = 0;
    let mut fps = 0.0;
    let mut captured = false;
    let result = (|| -> Result<()> {
        while running.load(Ordering::Relaxed) {
            let now = Instant::now();
            let elapsed = start.elapsed().as_secs_f64();
            let dt = (now - previous).as_secs_f32();
            previous = now;
            if options.seconds > 0.0 && elapsed >= options.seconds {
                break;
            }
            input = gamepad.update(&input)?;
            if app.update(&input, dt, elapsed) {
                break;
            }
            let index = display.acquire().context("Acquiring display image")?;
            let capture = if !captured && elapsed >= options.capture_after {
                options.capture.as_deref()
            } else {
                None
            };
            renderer.draw(
                &device,
                &queue,
                &display.targets[index],
                &display,
                &app,
                elapsed,
                fps,
                capture,
            )?;
            captured |= capture.is_some();
            display.present(index).context("Presenting voxel world")?;
            if frames == 0 {
                log::info!("First frame: present completed");
            }
            frames += 1;
            sample_frames += 1;
            if sample.elapsed() >= Duration::from_secs(1) {
                fps = sample_frames as f64 / sample.elapsed().as_secs_f64();
                log::info!(
                    "Presented {frames} frames, {fps:.1} FPS; position={:?}, yaw={:.3}, pitch={:.3}, flight={}",
                    app.player.position,
                    app.player.yaw,
                    app.player.pitch,
                    app.player.flying
                );
                sample = Instant::now();
                sample_frames = 0;
            }
        }
        Ok(())
    })();
    let saved = app.finish();
    log::info!(
        "Finished: {frames} frames in {:.2}s",
        start.elapsed().as_secs_f64()
    );
    result?;
    saved
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_validation_and_save_isolation() {
        for s in ["NaN", "inf", "-1", "bad"] {
            assert!(Options::try_parse_from(["game", "--seconds", s]).is_err());
        }
        let o = Options::try_parse_from(["game", "--seed", "42", "--save-dir", "/tmp/isolated"])
            .unwrap();
        assert_eq!(o.seed, Some(42));
        assert_eq!(o.save_dir, PathBuf::from("/tmp/isolated"));
    }
}

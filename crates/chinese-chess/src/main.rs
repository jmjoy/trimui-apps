mod ai;
mod app;
mod audio;
mod display;
mod game;
mod input;
mod render;
mod ui;

use anyhow::{Context, Result};
use app::{Action, App};
use clap::Parser;
use display::{DirectDisplay, DisplaySurface};
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
    name = "trimui-chinese-chess",
    version,
    about = "Chinese chess for TrimUI Smart Pro",
    after_help = "D-pad/left stick: navigate; A (right): confirm; B (bottom): cancel; X (top): undo; Y (left): legal target; START: menu; SELECT + START: exit."
)]
struct Options {
    /// Run for N seconds (0 means unlimited)
    #[arg(long, value_name = "N", default_value_t = 0.0, value_parser = parse_duration)]
    seconds: f64,

    /// Only validate the native display surface, without presenting frames
    #[arg(long)]
    probe: bool,

    /// Save a screenshot to a PNG file
    #[arg(long, value_name = "PNG")]
    capture: Option<PathBuf>,

    /// Wait N seconds before capturing the screenshot
    #[arg(long, value_name = "N", default_value_t = 2.0, value_parser = parse_duration)]
    capture_after: f64,

    /// Linux joystick device path
    #[arg(long, value_name = "PATH", default_value = "/dev/input/js0")]
    controller: String,
}

fn parse_duration(value: &str) -> std::result::Result<f64, String> {
    let duration: f64 = value.parse().map_err(|error| format!("{error}"))?;
    if duration.is_finite() && duration >= 0.0 {
        Ok(duration)
    } else {
        Err("Duration must be a finite, non-negative number".into())
    }
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,trimui_chinese_chess=info"),
    )
    .init();
    match run(Options::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(options: Options) -> Result<()> {
    let running = Arc::new(AtomicBool::new(true));
    let signal_flag = running.clone();
    ctrlc::set_handler(move || signal_flag.store(false, Ordering::Relaxed))?;
    let instance = display::create_instance()?;
    let adapter = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN))
        .into_iter()
        .next()
        .context("No Vulkan GPU found")?;
    log::info!("GPU: {:?}", adapter.get_info());
    let surface = DisplaySurface::new(&instance, &adapter)?;
    if options.probe {
        log::info!("Native surface probe passed; no frames presented");
        return Ok(());
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("TrimUI Chinese Chess"),
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))?;
    let display = DirectDisplay::new(surface, &device)?;
    let mut renderer = render::Renderer::new(&device, display.format)?;
    let mut gamepad = input::Gamepad::open(&options.controller)?;
    let mut repeat = input::DirectionRepeat::default();
    let mut app = App::new();
    let start = Instant::now();
    let mut sample_start = start;
    let mut sample_frames = 0u64;
    let mut frames = 0u64;
    let mut fps = 0.0;
    let mut captured = false;
    // Cache legal moves until a move/undo/reset; UI refresh does no search.
    let mut legal_moves = app.game.legal_moves();
    let mut history_len = 0;
    while running.load(Ordering::Relaxed) {
        let elapsed = start.elapsed();
        if options.seconds > 0.0 && elapsed.as_secs_f64() >= options.seconds {
            break;
        }
        app.poll_ai();
        let pressed = gamepad.update()?;
        if gamepad.exit_requested() {
            log::info!("Exit requested by SELECT + START");
            break;
        }
        if let Some(direction) = repeat.update(gamepad.direction(), elapsed) {
            app.action(Action::Direction(direction));
        }
        let mut exit = false;
        for (button, action) in [
            (input::BUTTON_START, Action::Menu),
            (input::BUTTON_B, Action::Cancel),
            (input::BUTTON_A, Action::Confirm),
            (input::BUTTON_X, Action::Undo),
            (input::BUTTON_Y, Action::NextTarget),
        ] {
            if pressed[button] {
                exit |= app.action(action);
            }
        }
        if exit {
            log::info!("Exit requested by menu");
            break;
        }
        if history_len != app.game.history().len() {
            history_len = app.game.history().len();
            legal_moves = app.game.legal_moves();
        }
        let index = display.acquire().context("Acquiring display image")?;
        let capture = if !captured && elapsed.as_secs_f64() >= options.capture_after {
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
            &legal_moves,
            elapsed.as_secs_f64(),
            fps,
            capture,
        )?;
        captured |= capture.is_some();
        display.present(index).context("Presenting chess board")?;
        if frames == 0 {
            log::info!("First frame: present completed");
        }
        frames += 1;
        sample_frames += 1;
        if sample_start.elapsed() >= Duration::from_secs(1) {
            fps = sample_frames as f64 / sample_start.elapsed().as_secs_f64();
            log::info!("Presented {frames} frames, {fps:.1} FPS");
            sample_start = Instant::now();
            sample_frames = 0;
        }
    }
    log::info!(
        "Finished: {frames} frames in {:.2}s",
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

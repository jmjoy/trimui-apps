mod app;
mod audio;
mod bench;
mod config;
mod display;
mod input;
mod jobs;
mod mesh;
mod player;
mod render;
mod save;
mod scene;
mod system;
mod ui;
mod world;
use anyhow::{Context, Result};
use clap::Parser;
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use system::{CpuManager, Telemetry};

#[derive(Parser)]
#[command(
    name = "trimui-voxel-world",
    version,
    about = "Voxel sandbox for TrimUI Smart Pro",
    after_help = "Default controls (rebindable in Settings): left stick move (double push forward to sprint); right stick look; R2 attack/break; L2 use/place; L/R hotbar; B jump, double tap to fly; A sneak/descend; X block palette; Y pick block; D-pad left/right toggle sneak/sprint; START pause; SELECT + START save and exit."
)]
struct Options {
    /// Stop after N seconds (0 = unlimited)
    #[arg(long,default_value_t=0.0,value_parser=parse_duration)]
    seconds: f64,
    /// Only verify the native display surface
    #[arg(long)]
    probe: bool,
    /// Write one screenshot PNG after --capture-after seconds; every SIGUSR1
    /// also writes PATH-N.png (default /tmp/voxel-world-shot-N.png)
    #[arg(long)]
    capture: Option<PathBuf>,
    #[arg(long,default_value_t=2.0,value_parser=parse_duration)]
    capture_after: f64,
    #[arg(long, default_value = "/dev/input/js0")]
    controller: String,
    /// Directory for the world save; isolated directories are useful for testing
    #[arg(long, default_value = "save")]
    save_dir: PathBuf,
    /// Directory for the global settings file (settings.json)
    #[arg(long, default_value = "config")]
    config_dir: PathBuf,
    /// Terrain seed when there is no existing world
    #[arg(long)]
    seed: Option<u64>,
    /// Fly a fixed route for N seconds (after a warm-up) and print statistics;
    /// uses a fresh world and never touches the save directory
    #[arg(long, value_parser = parse_duration)]
    bench: Option<f64>,
    /// World time in ticks for this session (0 sunrise, 6000 noon,
    /// 12000 sunset, 18000 midnight); benchmarks default to noon
    #[arg(long, value_parser = parse_duration)]
    time: Option<f64>,
    /// Skip the title screen and enter the world directly
    #[arg(long)]
    start_world: bool,
    /// Benchmark flight speed in blocks per second
    #[arg(long, default_value_t = 9.0, value_parser = parse_duration)]
    bench_speed: f64,
    /// Show the debug overlay for this session
    #[arg(long)]
    debug_overlay: bool,
    /// Leave CPU hotplug state as the system configured it for this session
    #[arg(long)]
    no_all_cores: bool,
    /// Write every synthesised sound as WAV into DIR and exit (no display needed)
    #[arg(long, value_name = "DIR")]
    dump_sounds: Option<PathBuf>,
    /// Write the texture sheet and icon atlas as PNG into DIR and exit
    #[arg(long, value_name = "DIR")]
    dump_atlas: Option<PathBuf>,
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

static SHOT_REQUESTED: AtomicBool = AtomicBool::new(false);
extern "C" fn request_shot(_: libc::c_int) {
    // Only an atomic store: async-signal-safe.
    SHOT_REQUESTED.store(true, Ordering::Relaxed);
}
/// `PATH-N.png` next to the configured capture path.
fn numbered_capture(base: Option<&Path>, n: u32) -> PathBuf {
    let base = base.unwrap_or(Path::new("/tmp/voxel-world-shot.png"));
    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("voxel-world-shot");
    base.with_file_name(format!("{stem}-{n}.png"))
}

fn log_system(cpu: &CpuManager, telemetry: &Telemetry) {
    log::info!(
        "System: present {}, online {}, governor {}, freq {} MHz, temp {} C, parallelism {}",
        system::format_cpu_list(cpu.present()),
        telemetry.online_label(),
        telemetry.governor,
        telemetry.freq_label(),
        telemetry.temp_label(),
        std::thread::available_parallelism().map_or(0, |n| n.get())
    );
}

fn run(options: Options) -> Result<()> {
    if let Some(dir) = &options.dump_sounds {
        audio::dump_sounds(dir, 48_000)?;
        return Ok(());
    }
    if let Some(dir) = &options.dump_atlas {
        return render::dump_atlas(dir);
    }
    let running = Arc::new(AtomicBool::new(true));
    let flag = running.clone();
    ctrlc::set_handler(move || flag.store(false, Ordering::Relaxed))?;
    // SAFETY: the handler only performs an atomic store.
    unsafe {
        libc::signal(
            libc::SIGUSR1,
            request_shot as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
    }
    let cpu_root = Path::new(system::CPU_ROOT);
    let thermal_root = Path::new(system::THERMAL_ROOT);
    let mut cpu = CpuManager::new(cpu_root);
    log_system(
        &cpu,
        &Telemetry::sample(cpu_root, thermal_root, cpu.present()),
    );
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
    let mut gamepad = input::Gamepad::open(&options.controller)?;
    let mut input = input::Frame::default();
    let bench_seconds = options.bench;
    // Threads for every core that may come online; the active count
    // follows the CPU option once the configuration is loaded.
    let threads = cpu.present().len().saturating_sub(1).clamp(1, 3);
    let pool = Arc::new(jobs::Pool::new(threads, 1));
    let mut app = app::App::new(
        options.save_dir,
        options.config_dir,
        if bench_seconds.is_some() {
            Some(options.seed.unwrap_or(42))
        } else {
            options.seed
        },
        bench_seconds.is_none(),
        pool.clone(),
    );
    app.overlay_forced = options.debug_overlay;
    if let Some(time) = options.time.or(bench_seconds.map(|_| 6_000.0)) {
        app.time = time;
    }
    let want_cores = |app: &app::App| app.want_all_cores() && !options.no_all_cores;
    cpu.set_enabled(want_cores(&app));
    pool.set_active(cpu.worker_budget());
    let mut renderer = render::Renderer::new(&device, &queue, display.format, pool.clone())?;
    let bench_origin = app.player.position.with_y(100.0);
    if bench_seconds.is_some() {
        app.start_bench();
    } else if options.start_world {
        app.start_world();
    }
    let limit = match bench_seconds {
        Some(b) => bench::WARMUP + b,
        None => options.seconds,
    };
    let mut stats = bench::Stats::default();
    let mut diag = ui::Diagnostics {
        cpu_state: cpu.state(),
        workers_active: pool.active(),
        workers_total: pool.threads(),
        ..Default::default()
    };
    let start = Instant::now();
    let mut previous = start;
    let mut sample = start;
    let mut sample_frames = 0;
    let mut sample_ms = (0.0f32, 0.0f32);
    let mut frames = 0u64;
    let mut samples = 0u32;
    let mut captured = false;
    let mut shots = 0u32;
    let mut shot_path = PathBuf::new();
    let mut present_ms = 0.0f32;
    // Screenshot readback and PNG encoding stall a frame; keep it out of stats.
    let mut skip_stat = false;
    let result = (|| -> Result<()> {
        while running.load(Ordering::Relaxed) {
            let now = Instant::now();
            let elapsed = start.elapsed().as_secs_f64();
            let dt = (now - previous).as_secs_f32();
            previous = now;
            if limit > 0.0 && elapsed >= limit {
                break;
            }
            if frames > 0 {
                sample_ms.0 += dt * 1000.0;
                sample_ms.1 = sample_ms.1.max(dt * 1000.0);
            }
            cpu.poll(now);
            if want_cores(&app) != cpu.enabled() {
                cpu.set_enabled(want_cores(&app));
                pool.set_active(cpu.worker_budget());
                diag.cpu_state = cpu.state();
                diag.workers_active = pool.active();
            }
            let update_start = Instant::now();
            input = gamepad.update(&input)?;
            if bench_seconds.is_some() {
                let (position, yaw, pitch) =
                    bench::route(bench_origin, elapsed, options.bench_speed);
                app.player.position = position;
                app.player.yaw = yaw;
                app.player.pitch = pitch;
                // Only SELECT + START is honoured during an unattended benchmark.
                let exit = [input::Button::Select, input::Button::Start];
                input = input::Frame {
                    held: input.held.iter().filter(|b| exit.contains(b)).collect(),
                    ..Default::default()
                };
            }
            if app.update(&input, dt, elapsed) {
                break;
            }
            let update_ms = update_start.elapsed().as_secs_f32() * 1000.0;
            let index = display.acquire().context("Acquiring display image")?;
            let capture = if SHOT_REQUESTED.swap(false, Ordering::Relaxed) {
                shots += 1;
                shot_path = numbered_capture(options.capture.as_deref(), shots);
                Some(shot_path.as_path())
            } else if !captured && elapsed >= options.capture_after {
                captured = options.capture.is_some();
                options.capture.as_deref()
            } else {
                None
            };
            let cost = renderer.draw(
                &device,
                &queue,
                &display.targets[index],
                &display,
                &app,
                elapsed,
                &diag,
                capture,
            )?;
            let present_start = Instant::now();
            display.present(index).context("Presenting voxel world")?;
            if frames == 0 {
                log::info!("First frame: present completed");
            }
            if bench_seconds.is_some() && elapsed >= bench::WARMUP && frames > 0 && !skip_stat {
                stats.record(
                    dt * 1000.0,
                    bench::Timings {
                        update: update_ms,
                        upload: cost.upload,
                        render: cost.render,
                        present: present_ms,
                    },
                );
            }
            present_ms = present_start.elapsed().as_secs_f32() * 1000.0;
            skip_stat = capture.is_some();
            frames += 1;
            sample_frames += 1;
            if sample.elapsed() >= Duration::from_secs(1) {
                let seconds = sample.elapsed().as_secs_f64();
                diag.fps = sample_frames as f64 / seconds;
                diag.frame_avg_ms = sample_ms.0 / sample_frames.max(1) as f32;
                diag.frame_worst_ms = sample_ms.1;
                diag.telemetry = Telemetry::sample(cpu_root, thermal_root, cpu.present());
                pool.set_active(cpu.worker_budget());
                diag.cpu_state = cpu.state();
                diag.workers_active = pool.active();
                diag.queued = pool.queued();
                diag.workers_running = pool.running();
                (diag.loaded, diag.pending) = renderer.scene_stats();
                diag.columns = app.world.len();
                diag.sections_mb = app.world.columns().map(|c| c.allocated()).sum::<usize>() as f32
                    * (3 * world::chunk::CELLS) as f32
                    / 1_048_576.0;
                diag.streaming = app.stream.pending();
                let stream_stats = app.stream.take_stats();
                diag.lighting = app.lighter.pending();
                let (lit, light_ms) = app.lighter.take_stats();
                if lit > 0 {
                    diag.light_ms = light_ms;
                }
                if stream_stats.generated > 0 {
                    diag.gen_ms = stream_stats.gen_ms_sum / stream_stats.generated as f32;
                }
                log::info!(
                    "Presented {frames} frames, {:.1} FPS, worst {:.1} ms; cpus {} @ {} MHz; position={:?}, yaw={:.3}, pitch={:.3}, flight={}",
                    diag.fps,
                    diag.frame_worst_ms,
                    diag.telemetry.online_label(),
                    diag.telemetry.freq_label(),
                    app.player.position,
                    app.player.yaw,
                    app.player.pitch,
                    app.player.flying
                );
                samples += 1;
                // Accumulate ten seconds of callback statistics per report.
                if samples.is_multiple_of(10) {
                    diag.audio = app.audio.take_stats();
                    log::info!(
                        "Audio callback: {} buffers x {:.0} frames, avg {:.0} us, max {:.0} us, load {:.2}%, voices peak {}",
                        diag.audio.callbacks,
                        diag.audio.frames_per_callback,
                        diag.audio.avg_us,
                        diag.audio.max_us,
                        diag.audio.load * 100.0,
                        diag.audio.voices_peak
                    );
                }
                sample = Instant::now();
                sample_frames = 0;
                sample_ms = (0.0, 0.0);
            }
        }
        Ok(())
    })();
    let saved = app.finish();
    log::info!(
        "Finished: {frames} frames in {:.2}s",
        start.elapsed().as_secs_f64()
    );
    if bench_seconds.is_some() && result.is_ok() {
        let (avg, low, worst, t) = stats.summary();
        let telemetry = Telemetry::sample(cpu_root, thermal_root, cpu.present());
        log::info!(
            "Bench result: frames={} avg_fps={avg:.1} p1_fps={low:.1} worst_ms={worst:.1} update_ms={:.2} upload_ms={:.2} render_ms={:.2} present_ms={:.2} rss_mb={:.1} columns={} distance={:.0} cpus={} workers={} governor={} freq_mhz={} temp_c={}",
            stats.frames(),
            t.update,
            t.upload,
            t.render,
            t.present,
            telemetry.rss_mb,
            app.world.len(),
            app.player.position.distance(bench_origin),
            telemetry.online_label(),
            pool.active(),
            telemetry.governor,
            telemetry.freq_label(),
            telemetry.temp_label()
        );
    }
    drop(renderer);
    drop(pool);
    cpu.restore();
    log::info!(
        "CPU cores restored: online {}",
        system::format_cpu_list(&cpu.online())
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
            assert!(Options::try_parse_from(["game", "--bench", s]).is_err());
        }
        let o = Options::try_parse_from(["game", "--seed", "42", "--save-dir", "/tmp/isolated"])
            .unwrap();
        assert_eq!(o.seed, Some(42));
        assert_eq!(o.save_dir, PathBuf::from("/tmp/isolated"));
        assert_eq!(o.config_dir, PathBuf::from("config"));
        assert!(o.dump_sounds.is_none() && !o.start_world && o.time.is_none());
        let o = Options::try_parse_from(["game", "--start-world", "--time", "18000"]).unwrap();
        assert!(o.start_world && o.time == Some(18_000.0));
        let o = Options::try_parse_from(["game", "--dump-sounds", "/tmp/s"]).unwrap();
        assert_eq!(o.dump_sounds, Some(PathBuf::from("/tmp/s")));
        assert!(o.bench.is_none() && !o.debug_overlay && !o.no_all_cores);
        let o =
            Options::try_parse_from(["game", "--bench", "40", "--debug-overlay", "--no-all-cores"])
                .unwrap();
        assert_eq!(o.bench, Some(40.0));
        assert!(o.debug_overlay && o.no_all_cores);
        assert_eq!(
            numbered_capture(Some(Path::new("/tmp/game.png")), 2),
            PathBuf::from("/tmp/game-2.png")
        );
        assert_eq!(
            numbered_capture(None, 1),
            PathBuf::from("/tmp/voxel-world-shot-1.png")
        );
    }
}

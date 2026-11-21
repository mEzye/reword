#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod launcher;
mod motion;
mod platform;

use fs2::FileExt;
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};
use std::{
    cell::RefCell,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

slint::include_modules!();

#[derive(Clone, Copy)]
struct Palette {
    background: slint::Color,
    accent: slint::Color,
}

fn parse_color(value: &str) -> Result<slint::Color, &'static str> {
    let digits = value.trim().trim_start_matches('#');
    if !digits.is_ascii() {
        return Err("Введи колір у форматі #RRGGBB");
    }
    // The previous WPF prototype saved #AARRGGBB. Preserve that format too.
    let rgb = match digits.len() {
        6 => digits,
        8 => &digits[2..],
        _ => return Err("Введи колір у форматі #RRGGBB"),
    };
    let rgb = u32::from_str_radix(rgb, 16).map_err(|_| "Введи колір у форматі #RRGGBB")?;
    Ok(slint::Color::from_rgb_u8(
        (rgb >> 16) as u8,
        (rgb >> 8) as u8,
        rgb as u8,
    ))
}

fn hex(color: slint::Color) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        color.red(),
        color.green(),
        color.blue()
    )
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            background: parse_color("#202C3A").unwrap(),
            accent: parse_color("#FFB482").unwrap(),
        }
    }
}

impl Palette {
    fn load(path: &Path) -> Self {
        let mut palette = Self::default();
        if let Ok(text) = fs::read_to_string(path) {
            for line in text.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    if let Ok(color) = parse_color(value) {
                        match key.trim() {
                            "background" => palette.background = color,
                            "accent" => palette.accent = color,
                            _ => {}
                        }
                    }
                }
            }
        }
        palette
    }
    fn save(self, path: &Path) -> std::io::Result<()> {
        let temporary = path.with_extension("ini.tmp");
        fs::write(
            &temporary,
            format!(
                "background={}\naccent={}\n",
                hex(self.background),
                hex(self.accent)
            ),
        )?;
        fs::rename(temporary, path)
    }
}

struct Animation {
    start: Instant,
    close: Option<Instant>,
    preview: bool,
    first_frame: u32,
    first_visible_logged: bool,
    last_render: Option<Instant>,
    worst_frame_gap: Duration,
}

thread_local! {
    static CONTROLLER: RefCell<std::rc::Weak<App>> = RefCell::new(std::rc::Weak::new());
    static GRAPHICS_FAILURE: RefCell<Option<String>> = const { RefCell::new(None) };
}

struct App {
    overlay: Overlay,
    preferences: Preferences,
    palette: RefCell<Palette>,
    animation: RefCell<Option<Animation>>,
    bars: Rc<VecModel<WaveBar>>,
    animator: Timer,
    config: PathBuf,
    log: RefCell<File>,
    rendered: Rc<std::cell::Cell<u32>>,
    prepared: std::cell::Cell<bool>,
    warming: std::cell::Cell<bool>,
    ready_timer: Timer,
    pending_open: std::cell::Cell<Option<bool>>,
    diagnostics: bool,
}

impl App {
    fn log(&self, message: &str) {
        let _ = writeln!(self.log.borrow_mut(), "{message}");
    }
    fn set_palette(&self, palette: Palette) {
        self.palette.replace(palette);
        self.overlay.set_panel_color(palette.background);
        self.overlay.set_accent_color(palette.accent);
        self.preferences.set_panel_color(palette.background);
        self.preferences.set_accent_color(palette.accent);
        self.preferences
            .set_background_hex(hex(palette.background).into());
        self.preferences.set_accent_hex(hex(palette.accent).into());
    }
    fn show_preferences(&self) {
        if let Err(error) = self.preferences.show() {
            self.log(&format!("Settings: {error}"));
        }
    }
    fn update_frame(&self, time: f32, exit: f32) {
        let frame = motion::frame(time, exit);
        self.overlay.set_shell_width(frame.width);
        self.overlay.set_shell_height(frame.height);
        self.overlay.set_visibility(frame.alpha);
        self.overlay.set_offset_y(frame.offset_y);
        self.overlay.set_mic_x(frame.mic_x);
        self.overlay.set_mic_scale(frame.mic_scale);
        self.overlay.set_mic_alpha(frame.mic_alpha);
        for (i, bar) in frame.bars.iter().enumerate() {
            self.bars.set_row_data(
                i,
                WaveBar {
                    x: bar.x,
                    height: bar.height,
                    alpha: bar.alpha,
                },
            );
        }
    }
    fn open(self: &Rc<Self>, preview: bool) {
        if GRAPHICS_FAILURE.with(|failure| failure.borrow().is_some()) {
            return;
        }
        if !preview
            && self
                .animation
                .borrow()
                .as_ref()
                .is_some_and(|a| !a.preview && a.close.is_none())
        {
            return;
        }
        if !self.prepared.get() {
            self.pending_open.set(Some(preview));
            self.prepare_graphics();
            return;
        }
        self.update_frame(0.0, 1.0);
        platform::position_overlay(self.overlay.window());
        self.animation.replace(Some(Animation {
            start: Instant::now(),
            close: None,
            preview,
            first_frame: self.rendered.get(),
            first_visible_logged: false,
            last_render: None,
            worst_frame_gap: Duration::ZERO,
        }));
        self.log(&format!(
            "resident resume: {}",
            platform::resident_description(self.overlay.window())
        ));
        self.log(if preview {
            "animation: preview"
        } else {
            "hotkey: pressed"
        });
        let weak = Rc::downgrade(self);
        self.animator
            .start(TimerMode::Repeated, Duration::from_millis(8), move || {
                if let Some(app) = weak.upgrade() {
                    app.tick();
                }
            });
    }
    fn prepare_graphics(self: &Rc<Self>) {
        if GRAPHICS_FAILURE.with(|failure| failure.borrow().is_some()) {
            return;
        }
        if self.prepared.get() || self.warming.replace(true) {
            return;
        }
        let start = Instant::now();
        if let Err(error) = self.overlay.show() {
            self.warming.set(false);
            self.log(&format!("GPU warmup: {error}"));
            GRAPHICS_FAILURE.with(|failure| {
                failure.replace(Some(error.to_string()));
            });
            let _ = slint::quit_event_loop();
            return;
        }
        platform::position_overlay(self.overlay.window());
        platform::keep_non_interactive(self.overlay.window());
        // Compile the primitive, image and transparency pipelines before the visible
        // entrance. Snapshots target an offscreen texture and are not presented.
        for time in [0.125, 1.5] {
            self.update_frame(time, 1.0);
            if let Err(error) = self.overlay.window().take_snapshot() {
                self.log(&format!("GPU warmup snapshot: {error}"));
                GRAPHICS_FAILURE.with(|failure| {
                    failure.replace(Some(error.to_string()));
                });
                let _ = slint::quit_event_loop();
                return;
            }
        }
        // Warm the actual composition target as well as the snapshot target.
        // This alpha rounds to zero on the final 8-bit surface, while the GPU
        // still prepares the image and primitive pipelines.
        self.update_frame(1.5, 1.0);
        self.overlay.set_visibility(0.0001);
        self.overlay.window().request_redraw();
        self.log(&format!(
            "GPU warmup: {:.2}s",
            start.elapsed().as_secs_f32()
        ));
        let weak = Rc::downgrade(self);
        self.ready_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(250),
            move || {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                app.warming.set(false);
                app.prepared.set(true);
                app.overlay.set_visibility(0.0);
                app.log(&format!(
                    "resident ready: {}; GPU retained; click-through; event-driven idle",
                    platform::resident_description(app.overlay.window())
                ));
                if let Err(error) = launcher::mark_ready() {
                    GRAPHICS_FAILURE.with(|failure| {
                        failure.replace(Some(error.to_string()));
                    });
                    let _ = slint::quit_event_loop();
                    return;
                }
                if let Some(preview) = app.pending_open.take() {
                    app.open(preview);
                }
            },
        );
    }
    fn release(&self) {
        if self.pending_open.get() == Some(false) {
            self.pending_open.set(None);
        }
        if let Some(animation) = self.animation.borrow_mut().as_mut() {
            if !animation.preview && animation.close.is_none() {
                animation.close = Some(Instant::now());
                self.log("hotkey: released");
            }
        }
    }
    fn tick(self: &Rc<Self>) {
        let mut animations = self.animation.borrow_mut();
        let Some(animation) = animations.as_mut() else {
            return;
        };
        let time = animation.start.elapsed().as_secs_f32();
        if animation.preview && time >= 5.0 && animation.close.is_none() {
            animation.close = Some(Instant::now());
        }
        #[cfg(windows)]
        if !animation.preview
            && time > 0.05
            && !platform::hotkey_held()
            && animation.close.is_none()
        {
            animation.close = Some(Instant::now());
            self.log("hotkey: released");
        }
        let exit = animation.close.map_or(1.0, |close| {
            1.0 - motion::ease(close.elapsed().as_secs_f32() / 0.5)
        });
        if exit <= 0.0 {
            self.log(&format!(
                "animation: {} frames in {:.2}s; worst frame gap {:.1}ms",
                self.rendered.get().saturating_sub(animation.first_frame),
                time,
                animation.worst_frame_gap.as_secs_f64() * 1000.0,
            ));
            // Keep the native window and device. Clear once, then stop drawing.
            self.overlay.set_visibility(0.0);
            self.animator.stop();
            *animations = None;
            self.log(&format!(
                "resident parked: {}; frame timer stopped",
                platform::resident_description(self.overlay.window())
            ));
            if self.diagnostics {
                let weak = Rc::downgrade(self);
                Timer::single_shot(Duration::from_millis(200), move || {
                    let Some(app) = weak.upgrade() else {
                        return;
                    };
                    let baseline = app.rendered.get();
                    let weak = Rc::downgrade(&app);
                    Timer::single_shot(Duration::from_secs(1), move || {
                        let Some(app) = weak.upgrade() else {
                            return;
                        };
                        if app.animation.borrow().is_none() {
                            app.log(&format!(
                                "idle verification: {} new frames in 1s; {}",
                                app.rendered.get().saturating_sub(baseline),
                                platform::resident_description(app.overlay.window())
                            ));
                        }
                    });
                });
            }
            return;
        }
        let source_time = animation.close.map_or(time, |close| {
            close.duration_since(animation.start).as_secs_f32()
        });
        self.update_frame(source_time, exit);
    }
    fn capture(window: &slint::Window, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = window.take_snapshot()?;
        let file = BufWriter::new(File::create(path)?);
        let mut encoder = png::Encoder::new(file, snapshot.width(), snapshot.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()?
            .write_image_data(snapshot.as_bytes())?;
        Ok(())
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let directory = std::env::var_os("REWORD_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or(
            std::env::current_exe()?
                .parent()
                .ok_or("No executable directory")?
                .to_path_buf(),
        );
    fs::create_dir_all(&directory)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".reword.lock"))?;
    if lock.try_lock_exclusive().is_err() {
        return Err("Reword is already running. Open its tray menu.".into());
    }
    let mut log = File::create(directory.join("reword.log"))?;
    let renderer = launcher::worker_renderer().unwrap_or(launcher::Renderer::Vulkan);
    writeln!(
        log,
        "Reword {} | Rust + Slint 1.19 | renderer={}",
        env!("CARGO_PKG_VERSION"),
        renderer.name()
    )?;
    if renderer == launcher::Renderer::Vulkan {
        writeln!(
            log,
            "Vulkan API cap: 1.1; legacy Intel descriptor-pool fix enabled"
        )?;
    }
    writeln!(
        log,
        "GPU power preference: {}",
        std::env::var("WGPU_POWER_PREF").unwrap_or_default()
    )?;

    let selector = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.slint_name().into());
    let selector = match renderer {
        launcher::Renderer::Vulkan => selector.require_vulkan(),
        launcher::Renderer::OpenGl => selector.require_opengl_with_version(3, 3),
    };
    let creating_overlay = Rc::new(std::cell::Cell::new(true));
    let flag = creating_overlay.clone();
    selector
        .with_winit_window_attributes_hook(move |attributes| {
            platform::window_attributes(attributes, flag.get())
        })
        .select()?;

    let overlay = Overlay::new()?;
    let _ = overlay.window();
    creating_overlay.set(false);
    let preferences = Preferences::new()?;
    let about = About::new()?;
    let tray = RewordTray::new()?;
    let bars = Rc::new(VecModel::from(vec![WaveBar::default(); motion::BAR_COUNT]));
    overlay.set_bars(ModelRc::from(bars.clone()));
    let config = directory.join("colors.ini");
    let palette = Palette::load(&config);
    let rendered = Rc::new(std::cell::Cell::new(0_u32));
    let app = Rc::new(App {
        overlay,
        preferences,
        palette: RefCell::new(palette),
        animation: RefCell::new(None),
        bars,
        animator: Timer::default(),
        config,
        log: RefCell::new(log),
        rendered,
        prepared: std::cell::Cell::new(false),
        warming: std::cell::Cell::new(false),
        ready_timer: Timer::default(),
        pending_open: std::cell::Cell::new(None),
        diagnostics: args.iter().any(|arg| arg == "--smoke-test"),
    });
    CONTROLLER.with(|controller| {
        controller.replace(Rc::downgrade(&app));
    });
    let weak = Rc::downgrade(&app);
    let _ = app
        .overlay
        .window()
        .set_rendering_notifier(move |state, api| {
            if matches!(state, slint::RenderingState::RenderingSetup) {
                if let Some(app) = weak.upgrade() {
                    app.log(&format!(
                        "GPU setup: resident device initialized; {}",
                        platform::graphics_description(api)
                    ));
                }
                return;
            }
            if !matches!(state, slint::RenderingState::AfterRendering) {
                return;
            }
            let Some(app) = weak.upgrade() else {
                return;
            };
            app.rendered.set(app.rendered.get().saturating_add(1));
            if let Some(animation) = app.animation.borrow_mut().as_mut() {
                if app.overlay.get_visibility() > 0.01 {
                    let now = Instant::now();
                    if !animation.first_visible_logged {
                        animation.first_visible_logged = true;
                        app.log(&format!(
                            "first visible render: {:.1}ms",
                            animation.start.elapsed().as_secs_f64() * 1000.0
                        ));
                    }
                    if let Some(last) = animation.last_render.replace(now) {
                        animation.worst_frame_gap =
                            animation.worst_frame_gap.max(now.duration_since(last));
                    }
                }
            }
        });
    app.set_palette(palette);
    platform::setup_overlay(app.overlay.window());

    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyH);
    let hotkey_manager = GlobalHotKeyManager::new().and_then(|manager| {
        manager.register(hotkey)?;
        Ok(manager)
    });
    match &hotkey_manager {
        Ok(_) => app.log("Ctrl+Alt+H: registered"),
        Err(error) => {
            app.log(&format!("Hotkey unavailable: {error}"));
            app.preferences
                .set_message(format!("Не вдалося зареєструвати Ctrl+Alt+H: {error}").into());
            app.show_preferences();
        }
    }
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == hotkey.id() {
            let _ = slint::invoke_from_event_loop(move || {
                CONTROLLER.with(|controller| {
                    if let Some(app) = controller.borrow().upgrade() {
                        match event.state {
                            HotKeyState::Pressed => app.open(false),
                            HotKeyState::Released => app.release(),
                        }
                    }
                });
            });
        }
    }));
    let weak = Rc::downgrade(&app);
    tray.on_preview(move || {
        if let Some(app) = weak.upgrade() {
            app.open(true);
        }
    });
    let weak = Rc::downgrade(&app);
    tray.on_preferences(move || {
        if let Some(app) = weak.upgrade() {
            app.show_preferences();
        }
    });
    tray.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    let about_weak = about.as_weak();
    tray.on_about(move || {
        if let Some(about) = about_weak.upgrade() {
            let _ = about.show();
        }
    });
    let weak = Rc::downgrade(&app);
    app.preferences.on_preview(move || {
        if let Some(app) = weak.upgrade() {
            app.open(true);
        }
    });
    let weak = Rc::downgrade(&app);
    app.preferences.on_apply(move |background, accent| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        match parse_color(&background).and_then(|background| {
            Ok(Palette {
                background,
                accent: parse_color(&accent)?,
            })
        }) {
            Ok(palette) => match palette.save(&app.config) {
                Ok(()) => {
                    app.set_palette(palette);
                    app.preferences.set_message("Кольори збережено".into());
                    app.open(true);
                }
                Err(error) => app
                    .preferences
                    .set_message(format!("Не вдалося зберегти: {error}").into()),
            },
            Err(error) => app.preferences.set_message(error.into()),
        }
    });
    let weak = Rc::downgrade(&app);
    app.preferences.on_preset(move |index| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let (background, accent) = match index {
            1 => ("#172D2B", "#92E2C0"),
            2 => ("#F1EBE3", "#425C75"),
            _ => ("#202C3A", "#FFB482"),
        };
        let palette = Palette {
            background: parse_color(background).unwrap(),
            accent: parse_color(accent).unwrap(),
        };
        match palette.save(&app.config) {
            Ok(()) => {
                app.set_palette(palette);
                app.preferences.set_message("Кольори збережено".into());
                app.open(true);
            }
            Err(error) => app
                .preferences
                .set_message(format!("Не вдалося зберегти: {error}").into()),
        }
    });

    let capture_timer = Timer::default();
    let startup_timer = Timer::default();
    let smoke_timer = Rc::new(Timer::default());
    let replay_timer = Rc::new(Timer::default());
    if args.iter().any(|a| a == "--capture-frames") {
        fs::create_dir_all(directory.join("preview-rust"))?;
        app.overlay.show()?;
        let weak = Rc::downgrade(&app);
        let index = Rc::new(RefCell::new(0_usize));
        capture_timer.start(TimerMode::Repeated, Duration::from_millis(130), move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            const TIMES: [u32; 10] = [0, 75, 125, 200, 250, 375, 500, 625, 750, 1500];
            let mut i = index.borrow_mut();
            if *i == TIMES.len() {
                app.show_preferences();
                *i += 1;
                return;
            }
            if *i > TIMES.len() {
                if let Err(error) = App::capture(
                    app.preferences.window(),
                    &directory.join("preview-rust/preferences.png"),
                ) {
                    app.log(&format!("Settings snapshot failed: {error}"));
                }
                let _ = slint::quit_event_loop();
                return;
            }
            let ms = TIMES[*i];
            app.update_frame(ms as f32 / 1000.0, 1.0);
            match App::capture(
                app.overlay.window(),
                &directory.join("preview-rust").join(format!("{ms:04}.png")),
            ) {
                Ok(()) => app.log(&format!("GPU snapshot: {ms}ms")),
                Err(error) => app.log(&format!("GPU snapshot failed: {error}")),
            }
            *i += 1;
        });
    } else {
        if args.iter().any(|a| a == "--settings") {
            app.show_preferences();
        }
        let preview = args.iter().any(|a| a == "--preview");
        let smoke = args.iter().any(|a| a == "--smoke-test");
        let weak = Rc::downgrade(&app);
        let replay = replay_timer.clone();
        let finish = smoke_timer.clone();
        startup_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(10),
            move || {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                app.prepare_graphics();
                if preview || smoke {
                    app.open(true);
                }
                if smoke {
                    let weak = Rc::downgrade(&app);
                    replay.start(TimerMode::SingleShot, Duration::from_secs(6), move || {
                        if let Some(app) = weak.upgrade() {
                            app.open(true);
                        }
                    });
                    finish.start(TimerMode::SingleShot, Duration::from_secs(15), || {
                        let _ = slint::quit_event_loop();
                    });
                }
            },
        );
    }
    // All handles and the lock stay alive until the explicit tray Quit action.
    slint::run_event_loop_until_quit()?;
    if let Some(error) = GRAPHICS_FAILURE.with(|failure| failure.take()) {
        return Err(error.into());
    }
    drop(hotkey_manager);
    drop(lock);
    Ok(())
}

fn worker_main() {
    platform::prefer_integrated_gpu();
    let panic_text = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let panic_log = panic_text.clone();
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut text) = panic_log.lock() {
            *text = Some(info.to_string());
        }
        previous_hook(info);
    }));
    let error = match std::panic::catch_unwind(run) {
        Ok(Ok(())) => return,
        Ok(Err(error)) => format!("{error:#}"),
        Err(_) => panic_text
            .lock()
            .ok()
            .and_then(|mut text| text.take())
            .unwrap_or_else(|| "Невідома помилка ініціалізації графіки".into()),
    };
    {
        let directory = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        if let Some(directory) = directory {
            let _ = fs::write(directory.join("error.log"), format!("{error}\n"));
        }
        eprintln!("Reword: {error}");
        std::process::exit(1);
    }
}

fn main() {
    if let Some(renderer) = launcher::worker_renderer() {
        if renderer == launcher::Renderer::Vulkan
            && std::env::args().any(|arg| arg == "--simulate-vulkan-crash")
        {
            std::process::exit(0xC0000005_u32 as i32);
        }
        worker_main();
        return;
    }
    match launcher::supervise() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            platform::show_startup_error(&error.to_string());
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_previous_wpf_colors_and_rejects_invalid_input() {
        assert_eq!(hex(parse_color("#FF202C3A").unwrap()), "#202C3A");
        assert_eq!(hex(parse_color("#ffb482").unwrap()), "#FFB482");
        assert!(parse_color("#ZZZZZZ").is_err());
        assert!(parse_color("#123").is_err());
    }
}

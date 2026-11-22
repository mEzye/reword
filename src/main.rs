#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod hotkeys;
mod launcher;
mod motion;
mod palettes;
mod platform;
mod speech;
mod speeds;

use fs2::FileExt;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
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

impl Palette {
    fn from_scheme(scheme: &palettes::Scheme) -> Self {
        Self {
            background: parse_color(&scheme.background).unwrap(),
            accent: parse_color(&scheme.accent).unwrap(),
        }
    }
    fn load(path: &Path, mut palette: Self, library: &palettes::Library) -> (Self, Option<usize>) {
        let mut selected = None;
        if let Ok(text) = fs::read_to_string(path) {
            for line in text.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    if key.trim() == "scheme" {
                        selected = library
                            .schemes
                            .iter()
                            .position(|scheme| scheme.name == value.trim());
                        continue;
                    }
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
        if let Some(index) = selected {
            palette = Self::from_scheme(&library.schemes[index]);
        }
        (palette, selected)
    }
    fn save(self, path: &Path, name: Option<&str>) -> std::io::Result<()> {
        let temporary = path.with_extension("ini.tmp");
        fs::write(
            &temporary,
            format!(
                "{}background={}\naccent={}\n",
                name.map(|name| format!("scheme={name}\n"))
                    .unwrap_or_default(),
                hex(self.background),
                hex(self.accent)
            ),
        )?;
        fs::rename(temporary, path)
    }
}

struct Animation {
    start: Instant,
    playback: motion::Playback,
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
    tray: slint::Weak<RewordTray>,
    hotkey_manager: RefCell<Option<GlobalHotKeyManager>>,
    current_hotkey: std::cell::Cell<HotKey>,
    hotkey_registered: std::cell::Cell<bool>,
    voice: RefCell<Option<speech::Voice>>,
    voice_generation: std::cell::Cell<u64>,
    dictating: std::cell::Cell<bool>,
    voice_level: std::cell::Cell<f32>,
    palette: RefCell<Palette>,
    palettes: RefCell<palettes::Library>,
    speeds: std::cell::Cell<speeds::Speeds>,
    speed_save_timer: Timer,
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
    fn start_voice(self: &Rc<Self>) {
        self.stop_recording();
        let settings = speech::Settings::load(&self.config.with_file_name("speech.ini"));
        if !self.config.with_file_name("speech.ini").exists() {
            if let Err(error) = settings.save(&self.config.with_file_name("speech.ini")) {
                self.log(&error);
            }
        }
        self.preferences
            .set_voice_mode(if settings.mode == "server" { 1 } else { 0 });
        self.preferences
            .set_voice_device(match settings.device.as_str() {
                "cuda" => 1,
                "auto" => 2,
                _ => 0,
            });
        self.preferences.set_server_url(settings.url.clone().into());
        self.preferences
            .set_server_token(settings.token.clone().into());
        self.preferences.set_voice_threads(settings.threads as i32);
        let generation = self.voice_generation.get() + 1;
        self.voice_generation.set(generation);
        self.voice.take();
        let directory = self.config.parent().unwrap().to_path_buf();
        let voice = speech::Voice::spawn(settings, directory, move |event| {
            let _ = slint::invoke_from_event_loop(move || {
                CONTROLLER.with(|controller| {
                    if let Some(app) = controller.borrow().upgrade() {
                        if app.voice_generation.get() == generation {
                            app.voice_event(event);
                        }
                    }
                });
            });
        });
        self.voice.replace(Some(voice));
    }
    fn voice_event(&self, event: speech::Event) {
        match event {
            speech::Event::Status(status) => self.preferences.set_voice_status(status.into()),
            speech::Event::Preview(text) => {
                self.preferences.set_transcript(text.into());
            }
            speech::Event::Error(error) => {
                self.log(&format!("Speech error: {error}"));
                self.preferences.set_voice_status(error.into());
            }
            speech::Event::Transcript {
                text,
                inserted,
                elapsed,
            } => {
                self.preferences.set_transcript(text.into());
                self.log(&format!("Speech complete: {elapsed:.2}s"));
                self.preferences.set_voice_status(
                    match inserted {
                        Ok(()) => format!("Вставлено · {elapsed:.1} с"),
                        Err(error) => error,
                    }
                    .into(),
                );
            }
        }
    }
    fn start_recording(&self) {
        if !self.dictating.replace(true) {
            if let Some(voice) = self.voice.borrow().as_ref() {
                voice.start();
            }
        }
    }
    fn stop_recording(&self) {
        if self.dictating.replace(false) {
            if let Some(voice) = self.voice.borrow().as_ref() {
                voice.stop();
            }
        }
    }
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
        self.preferences
            .set_palette_index(self.palette_index(palette));
    }
    fn palette_index(&self, palette: Palette) -> i32 {
        let library = self.palettes.borrow();
        let background = hex(palette.background);
        let accent = hex(palette.accent);
        let selected = self.preferences.get_palette_index();
        if let Some(scheme) = usize::try_from(selected)
            .ok()
            .and_then(|index| library.schemes.get(index))
        {
            if scheme.background == background && scheme.accent == accent {
                return selected;
            }
        }
        library.index(&background, &accent)
    }
    fn refresh_palette_names(&self) {
        let mut names: Vec<slint::SharedString> = self
            .palettes
            .borrow()
            .schemes
            .iter()
            .map(|scheme| scheme.name.clone().into())
            .collect();
        names.push("Власна".into());
        self.preferences
            .set_palette_names(ModelRc::new(VecModel::from(names)));
    }
    fn save_palette(&self, palette: Palette) -> std::io::Result<()> {
        let index = self.palette_index(palette);
        let library = self.palettes.borrow();
        let name = usize::try_from(index)
            .ok()
            .and_then(|index| library.schemes.get(index))
            .map(|scheme| scheme.name.as_str());
        palette.save(&self.config, name)
    }
    fn show_preferences(&self) {
        if let Err(error) = self.preferences.show() {
            self.log(&format!("Settings: {error}"));
        }
    }
    fn apply_hotkey(self: &Rc<Self>, input: &str) {
        let replacement = match hotkeys::parse(input) {
            Ok(hotkey) => hotkey,
            Err(error) => {
                self.preferences.set_hotkey_message(error.into());
                self.preferences
                    .set_hotkey_text(hotkeys::friendly(self.current_hotkey.get()).into());
                return;
            }
        };
        let previous = self.current_hotkey.get();
        self.stop_recording();
        self.pending_open.set(None);
        if let Some(animation) = self.animation.borrow_mut().as_mut() {
            animation.preview = false;
            animation
                .playback
                .set_closing(animation.start.elapsed().as_secs_f32(), true);
        }
        let path = self.config.with_file_name("hotkeys.ini");
        if replacement == previous {
            let mut registered_now = false;
            if !self.hotkey_registered.get() {
                let manager = self.hotkey_manager.borrow();
                let Some(manager) = manager.as_ref() else {
                    self.preferences.set_hotkey_message(
                        "Менеджер гарячих клавіш недоступний. Перезапусти програму.".into(),
                    );
                    self.preferences
                        .set_hotkey_text(hotkeys::friendly(previous).into());
                    return;
                };
                if let Err(error) = manager.register(previous) {
                    self.preferences
                        .set_hotkey_message(format!("Комбінацію не зареєстровано: {error}").into());
                    self.preferences
                        .set_hotkey_text(hotkeys::friendly(previous).into());
                    return;
                }
                registered_now = true;
            }
            if let Err(error) = hotkeys::save(&path, replacement) {
                if registered_now {
                    if let Some(manager) = self.hotkey_manager.borrow().as_ref() {
                        let _ = manager.unregister(previous);
                    }
                }
                self.preferences.set_hotkey_message(error.into());
                self.preferences
                    .set_hotkey_text(hotkeys::friendly(previous).into());
                return;
            }
            if registered_now {
                self.hotkey_registered.set(true);
            }
            self.preferences
                .set_hotkey_text(hotkeys::friendly(replacement).into());
            self.preferences
                .set_hotkey_message("Гарячу клавішу збережено".into());
            return;
        }
        let manager = self.hotkey_manager.borrow();
        let Some(manager) = manager.as_ref() else {
            self.preferences.set_hotkey_message(
                "Менеджер гарячих клавіш недоступний. Перезапусти програму.".into(),
            );
            self.preferences
                .set_hotkey_text(hotkeys::friendly(previous).into());
            return;
        };
        if let Err(error) = manager.register(replacement) {
            self.preferences.set_hotkey_message(
                format!("Комбінацію не зареєстровано: {error}. Попередня лишилася активною.")
                    .into(),
            );
            self.preferences
                .set_hotkey_text(hotkeys::friendly(previous).into());
            return;
        }
        if let Err(error) = hotkeys::save(&path, replacement) {
            let _ = manager.unregister(replacement);
            self.preferences.set_hotkey_message(
                format!("Не вдалося зберегти комбінацію: {error}. Попередня лишилася активною.")
                    .into(),
            );
            self.preferences
                .set_hotkey_text(hotkeys::friendly(previous).into());
            return;
        }
        self.current_hotkey.set(replacement);
        if self.hotkey_registered.get() {
            if let Err(error) = manager.unregister(previous) {
                self.log(&format!("Could not unregister previous hotkey: {error}"));
            }
        }
        self.hotkey_registered.set(true);
        self.preferences
            .set_hotkey_text(hotkeys::friendly(replacement).into());
        if let Some(tray) = self.tray.upgrade() {
            tray.set_hotkey_tooltip(format!("Reword · {}", hotkeys::friendly(replacement)).into());
        }
        self.preferences
            .set_hotkey_message("Гарячу клавішу збережено".into());
        self.log(&format!(
            "Hotkey changed: {}",
            hotkeys::canonical(replacement)
        ));
    }
    fn set_speeds(&self, speeds: speeds::Speeds) {
        self.speeds.set(speeds);
        self.preferences.set_open_speed(speeds.entrance);
        self.preferences.set_close_speed(speeds.exit);
        self.preferences.set_master_speed(speeds.master);
        self.preferences
            .set_open_speed_text(speeds::display_multiplier(speeds.entrance).into());
        self.preferences
            .set_close_speed_text(speeds::display_multiplier(speeds.exit).into());
        self.preferences
            .set_master_speed_text(speeds::display_multiplier(speeds.master).into());
        if let Some(animation) = self.animation.borrow_mut().as_mut() {
            let (open, close) = speeds.effective();
            animation
                .playback
                .set_speeds(animation.start.elapsed().as_secs_f32(), open, close);
        }
    }
    fn change_speeds(self: &Rc<Self>, speeds: speeds::Speeds) {
        self.set_speeds(speeds);
        let weak = Rc::downgrade(self);
        self.speed_save_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(250),
            move || {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                match app
                    .speeds
                    .get()
                    .save(&app.config.with_file_name("animation.ini"))
                {
                    Ok(()) => app.preferences.set_message("Швидкість збережено".into()),
                    Err(error) => app
                        .preferences
                        .set_message(format!("Не вдалося зберегти швидкість: {error}").into()),
                }
            },
        );
    }
    fn update_frame(&self, time: f32, exit: f32) {
        self.apply_frame(motion::frame(time, exit));
    }
    fn apply_frame(&self, frame: motion::Frame) {
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
        if !self.prepared.get() {
            self.pending_open.set(Some(preview));
            self.prepare_graphics();
            return;
        }
        if !preview {
            if let Some(animation) = self.animation.borrow_mut().as_mut() {
                let was_closing = animation.playback.is_closing();
                animation.preview = false;
                animation
                    .playback
                    .set_closing(animation.start.elapsed().as_secs_f32(), false);
                if was_closing {
                    self.log("hotkey: pressed; animation reversed toward open");
                }
                return;
            }
        }
        self.update_frame(0.0, 1.0);
        platform::position_overlay(self.overlay.window());
        let mut playback = motion::Playback::new();
        let (open, close) = self.speeds.get().effective();
        playback.set_speeds(0.0, open, close);
        self.animation.replace(Some(Animation {
            start: Instant::now(),
            playback,
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
        self.stop_recording();
        if self.pending_open.get() == Some(false) {
            self.pending_open.set(None);
        }
        if let Some(animation) = self.animation.borrow_mut().as_mut() {
            if !animation.preview && !animation.playback.is_closing() {
                animation
                    .playback
                    .set_closing(animation.start.elapsed().as_secs_f32(), true);
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
        if animation.preview && time >= 5.0 && !animation.playback.is_closing() {
            animation.playback.set_closing(time, true);
        }
        #[cfg(windows)]
        if !animation.preview
            && time > 0.05
            && !platform::hotkey_held(self.current_hotkey.get())
            && !animation.playback.is_closing()
        {
            self.stop_recording();
            animation.playback.set_closing(time, true);
            self.log("hotkey: released");
        }
        let (source_time, exit) = animation.playback.sample(time);
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
        let mut frame = motion::frame_with_stream(source_time, exit, time);
        if !animation.preview {
            let volume = self
                .voice
                .borrow()
                .as_ref()
                .map_or(0.0, speech::Voice::level);
            let volume = self.voice_level.get() + (volume - self.voice_level.get()) * 0.18;
            self.voice_level.set(volume);
            for bar in &mut frame.bars {
                bar.height *= 0.2 + volume * 0.8;
            }
        }
        self.apply_frame(frame);
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
    overlay
        .window()
        .set_size(slint::LogicalSize::new(224.0, 90.0));
    creating_overlay.set(false);
    let preferences = Preferences::new()?;
    let about = About::new()?;
    let tray = RewordTray::new()?;
    let bars = Rc::new(VecModel::from(vec![WaveBar::default(); motion::BAR_COUNT]));
    overlay.set_bars(ModelRc::from(bars.clone()));
    let config = directory.join("colors.ini");
    let palettes = palettes::Library::load_or_create(&directory.join("palettes.ini"))?;
    let (palette, selected_scheme) = Palette::load(
        &config,
        Palette::from_scheme(palettes.default_scheme()),
        &palettes,
    );
    let speeds = speeds::Speeds::load(&directory.join("animation.ini"));
    if let Err(error) = speeds.save(&directory.join("animation.ini")) {
        writeln!(log, "Animation settings migration/save: {error}")?;
    }
    let rendered = Rc::new(std::cell::Cell::new(0_u32));
    let app = Rc::new(App {
        overlay,
        preferences,
        tray: tray.as_weak(),
        hotkey_manager: RefCell::new(None),
        current_hotkey: std::cell::Cell::new(hotkeys::parse(hotkeys::DEFAULT).unwrap()),
        hotkey_registered: std::cell::Cell::new(false),
        voice: RefCell::new(None),
        voice_generation: std::cell::Cell::new(0),
        dictating: std::cell::Cell::new(false),
        voice_level: std::cell::Cell::new(0.0),
        palette: RefCell::new(palette),
        palettes: RefCell::new(palettes),
        speeds: std::cell::Cell::new(speeds),
        speed_save_timer: Timer::default(),
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
    app.refresh_palette_names();
    if let Some(index) = selected_scheme {
        app.preferences.set_palette_index(index as i32);
    }
    app.set_palette(palette);
    if let Err(error) = app.save_palette(palette) {
        app.log(&format!("Active color scheme migration/save: {error}"));
    }
    app.set_speeds(speeds);
    platform::setup_overlay(app.overlay.window());

    let hotkey_path = app.config.with_file_name("hotkeys.ini");
    let hotkey = match hotkeys::load(&hotkey_path) {
        Ok(hotkey) => hotkey,
        Err(error) => {
            app.log(&format!("Hotkey configuration: {error}; using default"));
            let default = hotkeys::parse(hotkeys::DEFAULT).unwrap();
            if !hotkey_path.exists() {
                if let Err(save_error) = hotkeys::save(&hotkey_path, default) {
                    app.log(&format!("Hotkey default save failed: {save_error}"));
                }
            }
            default
        }
    };
    app.current_hotkey.set(hotkey);
    app.preferences
        .set_hotkey_text(hotkeys::friendly(hotkey).into());
    app.preferences.set_hotkey_message("".into());
    tray.set_hotkey_tooltip(format!("Reword · {}", hotkeys::friendly(hotkey)).into());
    match GlobalHotKeyManager::new() {
        Ok(manager) => {
            match manager.register(hotkey) {
                Ok(()) => {
                    app.hotkey_registered.set(true);
                    app.log(&format!("{}: registered", hotkeys::friendly(hotkey)));
                }
                Err(error) => {
                    app.log(&format!("Hotkey unavailable: {error}"));
                    app.preferences.set_hotkey_message(
                        format!(
                            "Не вдалося зареєструвати {}: {error}",
                            hotkeys::friendly(hotkey)
                        )
                        .into(),
                    );
                    app.show_preferences();
                }
            }
            app.hotkey_manager.replace(Some(manager));
        }
        Err(error) => {
            app.log(&format!("Hotkey manager unavailable: {error}"));
            app.preferences
                .set_hotkey_message(format!("Менеджер гарячих клавіш недоступний: {error}").into());
            app.show_preferences();
        }
    }
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        let _ = slint::invoke_from_event_loop(move || {
            CONTROLLER.with(|controller| {
                if let Some(app) = controller.borrow().upgrade() {
                    if event.id == app.current_hotkey.get().id() {
                        match event.state {
                            HotKeyState::Pressed => {
                                app.start_recording();
                                app.open(false);
                            }
                            HotKeyState::Released => app.release(),
                        }
                    }
                }
            });
        });
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
    let weak = Rc::downgrade(&app);
    app.preferences
        .on_apply_voice(move |mode, device, url, token, threads| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let mut settings = speech::Settings::load(&app.config.with_file_name("speech.ini"));
            settings.mode = if mode == 1 { "server" } else { "local" }.into();
            settings.device = match device {
                1 => "cuda",
                2 => "auto",
                _ => "cpu",
            }
            .into();
            settings.url = url.trim().trim_end_matches('/').into();
            settings.token = token.trim().into();
            settings.threads = threads.clamp(1, 32) as usize;
            match settings.save(&app.config.with_file_name("speech.ini")) {
                Ok(()) => app.start_voice(),
                Err(error) => app.preferences.set_voice_status(error.into()),
            }
        });
    let weak = Rc::downgrade(&app);
    app.preferences.on_apply_hotkey(move |input| {
        if let Some(app) = weak.upgrade() {
            app.apply_hotkey(&input);
        }
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
    app.preferences.on_color_edited(move |index, text| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        if let Ok(color) = parse_color(&text) {
            match index {
                0 => app.preferences.set_panel_color(color),
                1 => app.preferences.set_accent_color(color),
                _ => {}
            }
        }
        let index = parse_color(&app.preferences.get_background_hex())
            .and_then(|background| {
                Ok(app.palette_index(Palette {
                    background,
                    accent: parse_color(&app.preferences.get_accent_hex())?,
                }))
            })
            .unwrap_or(app.palettes.borrow().schemes.len() as i32);
        app.preferences.set_palette_index(index);
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
            Ok(palette) => match app.save_palette(palette) {
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
        let palette = usize::try_from(index).ok().and_then(|index| {
            app.palettes
                .borrow()
                .schemes
                .get(index)
                .map(Palette::from_scheme)
        });
        let Some(palette) = palette else {
            return;
        };
        match app.save_palette(palette) {
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

    let weak = Rc::downgrade(&app);
    app.preferences
        .on_save_scheme(move |name, background, accent| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let palette = match parse_color(&background).and_then(|background| {
                Ok(Palette {
                    background,
                    accent: parse_color(&accent)?,
                })
            }) {
                Ok(palette) => palette,
                Err(error) => {
                    app.preferences.set_message(error.into());
                    return;
                }
            };
            let mut library = app.palettes.borrow().clone();
            if let Err(error) =
                library.upsert(&name, &hex(palette.background), &hex(palette.accent))
            {
                app.preferences.set_message(error.into());
                return;
            }
            if let Err(error) = library.save(&app.config.with_file_name("palettes.ini")) {
                app.preferences
                    .set_message(format!("Не вдалося зберегти схему: {error}").into());
                return;
            }
            app.palettes.replace(library);
            app.refresh_palette_names();
            let index = app
                .palettes
                .borrow()
                .schemes
                .iter()
                .position(|scheme| scheme.name == name.trim())
                .unwrap();
            app.preferences.set_palette_index(index as i32);
            app.set_palette(palette);
            app.preferences.set_scheme_name("".into());
            match app.save_palette(palette) {
                Ok(()) => app.preferences.set_message("Схему збережено".into()),
                Err(error) => app.preferences.set_message(
                    format!("Схему збережено; не вдалося зберегти активні кольори: {error}").into(),
                ),
            }
        });

    let weak = Rc::downgrade(&app);
    app.preferences
        .on_speed_changed(move |entrance, exit, master| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            app.change_speeds(speeds::Speeds::new(entrance, exit, master));
        });
    let weak = Rc::downgrade(&app);
    app.preferences.on_speed_edited(move |index, text| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        match speeds::parse_multiplier(&text) {
            Ok(value) => {
                let mut speeds = app.speeds.get();
                match index {
                    0 => speeds.entrance = value,
                    1 => speeds.exit = value,
                    2 => speeds.master = value,
                    _ => return,
                }
                app.change_speeds(speeds);
            }
            Err(message) => {
                app.set_speeds(app.speeds.get());
                app.preferences.set_message(message.into());
            }
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
        if !args.iter().any(|a| a == "--smoke-test") {
            app.start_voice();
        }
        if args.iter().any(|a| a == "--voice-settings") {
            app.show_preferences();
        }
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
    app.hotkey_manager.borrow_mut().take();
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
    if std::env::args().any(|arg| arg == "--speech-server") {
        if let Err(error) = speech::server::run() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(path) = std::env::args().find_map(|arg| {
        arg.strip_prefix("--transcribe-file=")
            .or_else(|| arg.strip_prefix("--transcribe-stream-file="))
            .map(PathBuf::from)
    }) {
        let base = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let result = if std::env::args().any(|arg| arg.starts_with("--transcribe-stream-file=")) {
            speech::transcribe_stream_file(&path, &base)
        } else {
            speech::transcribe_file(&path, &base)
        };
        match result {
            Ok(result) => {
                if let Some(path) = std::env::args()
                    .find_map(|arg| arg.strip_prefix("--output=").map(PathBuf::from))
                {
                    fs::write(path, result.to_string()).unwrap();
                } else {
                    println!("{result}");
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
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

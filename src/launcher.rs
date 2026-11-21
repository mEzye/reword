use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    Vulkan,
    OpenGl,
}

impl Renderer {
    pub fn name(self) -> &'static str {
        match self {
            Self::Vulkan => "vulkan",
            Self::OpenGl => "opengl",
        }
    }
    pub fn slint_name(self) -> &'static str {
        match self {
            Self::Vulkan => "femtovg-wgpu",
            Self::OpenGl => "femtovg",
        }
    }
}

pub fn worker_renderer() -> Option<Renderer> {
    std::env::args().find_map(|arg| match arg.as_str() {
        "--renderer-worker=vulkan" => Some(Renderer::Vulkan),
        "--renderer-worker=opengl" => Some(Renderer::OpenGl),
        _ => None,
    })
}

pub fn directory() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(std::env::var_os("REWORD_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or(
            std::env::current_exe()?
                .parent()
                .ok_or("No executable directory")?
                .to_path_buf(),
        ))
}

pub fn mark_ready() -> std::io::Result<()> {
    if let Some(path) = std::env::args().find_map(|arg| {
        arg.strip_prefix("--renderer-ready-file=")
            .map(PathBuf::from)
    }) {
        fs::write(
            path,
            format!(
                "{}\npid={}\n",
                worker_renderer().unwrap().name(),
                std::process::id()
            ),
        )?;
    }
    Ok(())
}

pub fn supervise() -> Result<i32, Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let directory = directory()?;
    fs::create_dir_all(&directory)?;
    let guard = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".reword-launcher.lock"))?;
    guard
        .try_lock_exclusive()
        .map_err(|_| "Reword is already running. Open its tray menu.")?;
    let mut log = File::create(directory.join("launcher.log"))?;
    writeln!(
        log,
        "Reword {} | Vulkan <=1.1 -> OpenGL | GPU renderers only",
        env!("CARGO_PKG_VERSION")
    )?;
    let ready = directory.join(format!(".renderer-ready-{}.txt", std::process::id()));
    let order: &[Renderer] = if args.iter().any(|a| a == "--opengl") {
        &[Renderer::OpenGl]
    } else {
        &[Renderer::Vulkan, Renderer::OpenGl]
    };
    for &renderer in order {
        if ready.exists() {
            fs::remove_file(&ready)?;
        }
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(&args)
            .arg(format!("--renderer-worker={}", renderer.name()))
            .arg(format!("--renderer-ready-file={}", ready.display()))
            .env_remove("WGPU_ADAPTER_NAME")
            .stdout(Stdio::null())
            .stderr(File::create(
                directory.join(format!("renderer-{}.stderr.log", renderer.name())),
            )?);
        if renderer == Renderer::Vulkan {
            command.env("WGPU_BACKEND", "vulkan");
        }
        let mut child = command.spawn()?;
        writeln!(
            log,
            "Starting {}; worker pid={}",
            renderer.name(),
            child.id()
        )?;
        let started = Instant::now();
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if ready.exists() {
                writeln!(
                    log,
                    "{} ready after {:.2}s; supervisor sleeping until exit",
                    renderer.name(),
                    started.elapsed().as_secs_f64()
                )?;
                break child.wait()?;
            }
            if started.elapsed() > Duration::from_secs(25) {
                timed_out = true;
                let _ = child.kill();
                break child.wait()?;
            }
            thread::sleep(Duration::from_millis(50));
        };
        writeln!(
            log,
            "{} exited: {:?}; startup_timeout={timed_out}",
            renderer.name(),
            status.code()
        )?;
        if ready.exists() {
            let _ = fs::remove_file(&ready);
        }
        if status.success() && !timed_out {
            return Ok(0);
        }
        for (source, extension) in [("reword.log", "log"), ("error.log", "error.log")] {
            let source = directory.join(source);
            if source.exists() {
                let _ = fs::copy(
                    source,
                    directory.join(format!("renderer-{}.{}", renderer.name(), extension)),
                );
            }
        }
        if renderer == Renderer::Vulkan {
            writeln!(
                log,
                "Vulkan could not run this overlay; retrying OpenGL in a fresh process"
            )?;
        }
    }
    Err("Не вдалося запустити ні Vulkan, ні OpenGL. Причини збережені в launcher.log, renderer-vulkan.log та renderer-opengl.log.".into())
}

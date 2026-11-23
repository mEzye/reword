use fs2::FileExt;
use std::{
    fs::{self, OpenOptions},
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
    let scope = crate::runtime::scope(&directory)?;
    let guard = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(scope.join("launcher.lock"))?;
    guard
        .try_lock_exclusive()
        .map_err(|_| "Reword is already running. Open its tray menu.")?;
    let session = std::env::var_os("REWORD_RUNTIME_SESSION").ok_or("No runtime session")?;
    let ready = PathBuf::from(session).join(format!(".renderer-ready-{}.txt", std::process::id()));
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
            .stderr(Stdio::piped());
        if renderer == Renderer::Vulkan {
            command.env("WGPU_BACKEND", "vulkan");
        }
        let mut child = command.spawn()?;
        let stderr = child
            .stderr
            .take()
            .map(|pipe| crate::runtime::drain(pipe, false));
        let started = Instant::now();
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if ready.exists() {
                break child.wait()?;
            }
            if started.elapsed() > Duration::from_secs(25) {
                timed_out = true;
                let _ = child.kill();
                break child.wait()?;
            }
            thread::sleep(Duration::from_millis(50));
        };
        if let Some(stderr) = stderr {
            let _ = stderr.join();
        }
        if ready.exists() {
            let _ = fs::remove_file(&ready);
        }
        if status.success() && !timed_out {
            return Ok(0);
        }
        crate::runtime::log(
            false,
            &format!(
                "{} exited: {:?}; startup timeout: {timed_out}",
                renderer.name(),
                status.code()
            ),
        );
        if renderer == Renderer::Vulkan {
            crate::runtime::log(false, "Vulkan failed; retrying OpenGL");
        }
    }
    Err(format!(
        "Не вдалося запустити ні Vulkan, ні OpenGL. Журнал: {}",
        crate::runtime::log_path(false).display()
    )
    .into())
}

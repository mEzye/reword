mod audio;
mod live_output;
mod native;
pub mod server;
mod streaming;

use enigo::Keyboard;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub mode: String,
    pub device: String,
    pub url: String,
    pub token: String,
    pub threads: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: "local".into(),
            device: "cpu".into(),
            url: "http://127.0.0.1:8178".into(),
            token: String::new(),
            threads: 2,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        let mut settings = Self::default();
        if let Ok(text) = fs::read_to_string(path) {
            for line in text.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    match key.trim() {
                        "mode" if ["local", "server"].contains(&value.trim()) => {
                            settings.mode = value.trim().into()
                        }
                        "device" if ["cpu", "cuda", "auto"].contains(&value.trim()) => {
                            settings.device = value.trim().into()
                        }
                        "url" => settings.url = value.trim().trim_end_matches('/').into(),
                        "token" => settings.token = value.trim().into(),
                        "threads" => {
                            settings.threads =
                                value.trim().parse::<usize>().unwrap_or(2).clamp(1, 32)
                        }
                        _ => {}
                    }
                }
            }
        }
        settings
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if self.mode == "server" && self.token.trim().is_empty() {
            return Err("Введи ключ сервера".into());
        }
        if self.mode == "server"
            && !(self.url.starts_with("http://") || self.url.starts_with("https://"))
        {
            return Err("Адреса має починатися з http:// або https://".into());
        }
        if self.url.contains(['\r', '\n']) || self.token.contains(['\r', '\n']) {
            return Err("Адреса або ключ містить перенесення рядка".into());
        }
        let temp = path.with_extension("ini.tmp");
        fs::write(
            &temp,
            format!(
                "mode={}\ndevice={}\nurl={}\ntoken={}\nthreads={}\n",
                self.mode, self.device, self.url, self.token, self.threads
            ),
        )
        .map_err(|e| e.to_string())?;
        fs::rename(temp, path).map_err(|e| e.to_string())
    }
}

#[derive(Debug)]
pub enum Event {
    Status(String),
    Error(String),
    Preview(String),
    Transcript {
        text: String,
        inserted: Result<(), String>,
        elapsed: f64,
    },
}

enum CaptureCommand {
    Start(usize),
    Stop,
    Shutdown,
}
pub struct Voice {
    commands: mpsc::Sender<CaptureCommand>,
    level: Arc<AtomicU32>,
    cancelled: Arc<AtomicBool>,
    jobs: Arc<streaming::Mailbox>,
}

pub fn target() -> usize {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as usize
    }
    #[cfg(not(windows))]
    {
        0
    }
}

pub fn insert(text: &str, expected: usize) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(expected as *mut std::ffi::c_void, &mut pid);
        }
        if expected == 0 || expected != target() || pid == std::process::id() {
            return Err(
                "Фокус змінився; текст доступний у полі останнього транскрипту в Налаштуваннях"
                    .into(),
            );
        }
    }
    let _ = expected;
    let mut keyboard = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| e.to_string())?;
    // Windows Enigo submits Unicode packets in one batch, rather than simulating
    // letter keys / shortcuts. Keep the physical push-to-talk modifiers intact.
    keyboard.text(text).map_err(|e| e.to_string())
}

impl Voice {
    pub fn spawn(
        settings: Settings,
        directory: PathBuf,
        notify: impl Fn(Event) + Send + Sync + 'static,
    ) -> Self {
        let notify = Arc::new(notify);
        let jobs = Arc::new(streaming::Mailbox::default());
        let worker_jobs = jobs.clone();
        let active = Arc::new(AtomicU64::new(0));
        let worker_active = active.clone();
        let report = notify.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        thread::spawn(move || {
            let mut client = Client::new(settings, directory, cancellation.clone());
            report(Event::Status("Завантаження моделі…".into()));
            match client.prepare() {
                Ok(backend) => report(Event::Status(format!("Готово · {backend}"))),
                Err(error) => report(Event::Error(error)),
            }
            let mut decoder = streaming::Decoder::default();
            let mut output = live_output::LiveOutput::default();
            let mut output_id = 0;
            let mut output_error = None;
            while let Some(job) = worker_jobs.take() {
                if cancellation.load(Ordering::Relaxed) {
                    break;
                }
                if job.finished {
                    report(Event::Status("Завершення розпізнавання…".into()));
                }
                let start = Instant::now();
                if output_id != job.id {
                    output = live_output::LiveOutput::default();
                    output_id = job.id;
                    output_error = None;
                }
                let result = decoder.update(
                    &job,
                    |audio| client.transcribe(audio),
                    |text, committed| {
                        if !cancellation.load(Ordering::Relaxed) {
                            if let Err(error) = output.update(&text, &committed, false, |suffix| {
                                insert(suffix, job.target)
                            }) {
                                if output_error.is_none() {
                                    report(Event::Error(error.clone()));
                                    output_error = Some(error);
                                }
                            }
                        }
                        if !cancellation.load(Ordering::Relaxed)
                            && worker_active.load(Ordering::Relaxed) == job.id
                        {
                            report(Event::Preview(text));
                        }
                    },
                );
                if !job.finished {
                    if let Err(error) = result {
                        report(Event::Error(error));
                    }
                    continue;
                }
                match result {
                    Ok(text) if !text.is_empty() => {
                        if cancellation.load(Ordering::Relaxed) {
                            break;
                        }
                        let inserted =
                            output.update(&text, &text, true, |suffix| insert(suffix, job.target));
                        report(Event::Transcript {
                            text,
                            inserted,
                            elapsed: start.elapsed().as_secs_f64(),
                        });
                    }
                    Ok(_) => report(Event::Status("Мовлення не розпізнано".into())),
                    Err(error) => report(Event::Error(error)),
                }
            }
        });
        let (commands, receiver) = mpsc::channel();
        let level = Arc::new(AtomicU32::new(0f32.to_bits()));
        let volume = level.clone();
        let capture_jobs = jobs.clone();
        thread::spawn(move || {
            let mut capture = None;
            let mut target = 0;
            let mut id = 0_u64;
            let mut next_preview = Instant::now();
            loop {
                let command = match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if Instant::now() >= next_preview {
                            next_preview = Instant::now() + Duration::from_millis(800);
                            if let Some(stream) = capture.as_ref() {
                                match audio::Capture::snapshot(stream) {
                                    Ok(samples) if samples.len() >= audio::RATE => {
                                        let _ = capture_jobs.publish(streaming::Job {
                                            id,
                                            target,
                                            samples,
                                            finished: false,
                                        });
                                    }
                                    Ok(_) => {}
                                    Err(error) => {
                                        capture = None;
                                        volume.store(0f32.to_bits(), Ordering::Relaxed);
                                        notify(Event::Error(error));
                                    }
                                }
                            }
                        }
                        continue;
                    }
                };
                match command {
                    CaptureCommand::Start(window) => {
                        if capture.is_some() {
                            continue;
                        }
                        if !capture_jobs.can_start() {
                            notify(Event::Error(
                                "Дочекайся завершення попередніх записів".into(),
                            ));
                            continue;
                        }
                        target = window;
                        match audio::Capture::start(volume.clone()) {
                            Ok(stream) => {
                                id += 1;
                                active.store(id, Ordering::Relaxed);
                                capture = Some(stream);
                                next_preview = Instant::now() + Duration::from_secs(1);
                                notify(Event::Preview(String::new()));
                                notify(Event::Status("Запис…".into()));
                            }
                            Err(error) => notify(Event::Error(error)),
                        }
                    }
                    CaptureCommand::Stop => {
                        if let Some(stream) = capture.take() {
                            match stream.finish() {
                                Ok(samples) if samples.len() < audio::RATE / 5 => {
                                    notify(Event::Status("Запис закороткий".into()))
                                }
                                Ok(samples) => {
                                    let power = samples.iter().map(|v| v * v).sum::<f32>()
                                        / samples.len() as f32;
                                    if power < 0.000001 {
                                        notify(Event::Status("Тиша".into()));
                                    } else if capture_jobs
                                        .publish(streaming::Job {
                                            id,
                                            samples,
                                            target,
                                            finished: true,
                                        })
                                        .is_err()
                                    {
                                        notify(Event::Error(
                                            "Черга розпізнавання заповнена".into(),
                                        ));
                                    }
                                }
                                Err(error) => notify(Event::Error(error)),
                            }
                        }
                        volume.store(0f32.to_bits(), Ordering::Relaxed);
                    }
                    CaptureCommand::Shutdown => break,
                }
            }
        });
        Self {
            commands,
            level,
            cancelled,
            jobs,
        }
    }
    pub fn start(&self) {
        let _ = self.commands.send(CaptureCommand::Start(target()));
    }
    pub fn stop(&self) {
        let _ = self.commands.send(CaptureCommand::Stop);
    }
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.jobs.close();
        let _ = self.commands.send(CaptureCommand::Shutdown);
    }
}

struct Client {
    backend: String,
    settings: Settings,
    child: Option<Child>,
    stderr: Option<thread::JoinHandle<()>>,
    ready: Option<PathBuf>,
    local_token: Option<PathBuf>,
    url: String,
    token: String,
    agent: ureq::Agent,
    cancelled: Arc<AtomicBool>,
}

impl Client {
    fn new(settings: Settings, _directory: PathBuf, cancelled: Arc<AtomicBool>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(180)))
            .timeout_connect(Some(Duration::from_secs(5)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            url: settings.url.clone(),
            token: settings.token.clone(),
            settings,
            child: None,
            stderr: None,
            ready: None,
            local_token: None,
            agent,
            cancelled,
            backend: String::new(),
        }
    }
    fn prepare(&mut self) -> Result<String, String> {
        match self.prepare_once() {
            Err(_)
                if self.settings.mode == "local"
                    && self.settings.device == "auto"
                    && !self.cancelled.load(Ordering::Relaxed) =>
            {
                self.fallback_cpu();
                self.prepare_once()
            }
            result => result,
        }
    }
    fn prepare_once(&mut self) -> Result<String, String> {
        if self.settings.mode == "local" {
            let alive = if let Some(child) = self.child.as_mut() {
                child.try_wait().map_err(|e| e.to_string())?.is_none()
            } else {
                false
            };
            if !alive {
                self.stop_child();
                let key = server::random_token()?;
                let suffix = &key[..16];
                let runtime = std::env::var_os("REWORD_RUNTIME_SESSION")
                    .map(PathBuf::from)
                    .ok_or("No runtime session")?;
                let ready = runtime.join(format!(".speech-ready-{suffix}.json"));
                let token_path = runtime.join(format!(".speech-{suffix}.token"));
                fs::write(&token_path, &key).map_err(|e| e.to_string())?;
                self.ready = Some(ready.clone());
                self.local_token = Some(token_path.clone());
                let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
                command
                    .arg("--speech-server")
                    .arg("--engine-child")
                    .arg("--exit-on-stdin-close")
                    .arg("--listen=127.0.0.1:0")
                    .arg(format!("--device={}", self.settings.device))
                    .arg(format!("--threads={}", self.settings.threads))
                    .arg(format!("--ready-file={}", ready.display()))
                    .arg(format!("--token-file={}", token_path.display()))
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    command.creation_flags(0x08000000);
                }
                let mut child = command.spawn().map_err(|e| e.to_string())?;
                self.stderr = child
                    .stderr
                    .take()
                    .map(|pipe| crate::runtime::drain(pipe, true));
                self.child = Some(child);
                self.token = key;
                let started = Instant::now();
                loop {
                    if self.cancelled.load(Ordering::Relaxed) {
                        return Err("Завантаження скасовано".into());
                    }
                    if let Ok(text) = fs::read_to_string(&ready) {
                        let value: serde_json::Value =
                            serde_json::from_str(&text).map_err(|e| e.to_string())?;
                        self.url = value["url"]
                            .as_str()
                            .ok_or("Speech server returned no URL")?
                            .into();
                        break;
                    }
                    if self
                        .child
                        .as_mut()
                        .unwrap()
                        .try_wait()
                        .map_err(|e| e.to_string())?
                        .is_some()
                    {
                        if let Some(stderr) = self.stderr.take() {
                            let _ = stderr.join();
                        }
                        let tail = crate::runtime::tail(true);
                        return Err(format!("Рушій голосу не запустився: {tail}"));
                    }
                    if started.elapsed() > Duration::from_secs(180) {
                        if let Some(mut child) = self.child.take() {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        return Err("Модель не завантажилася за 3 хвилини".into());
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
        let response = self
            .agent
            .get(format!("{}/health", self.url))
            .header("Authorization", format!("Bearer {}", self.token))
            .call()
            .map_err(|e| e.to_string())?;
        let value = json_response(response)?;
        if value["protocol"] != 1 || value["ready"] != true {
            return Err("Несумісний сервер голосу".into());
        }
        self.backend = value["backend"].as_str().unwrap_or("сервер").into();
        Ok(self.backend.clone())
    }
    fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        self.prepare()?;
        match self.request_transcription(samples) {
            Err(_)
                if self.settings.mode == "local"
                    && self.settings.device == "auto"
                    && !self.cancelled.load(Ordering::Relaxed) =>
            {
                self.fallback_cpu();
                self.prepare()?;
                self.request_transcription(samples)
            }
            result => result,
        }
    }
    fn fallback_cpu(&mut self) {
        crate::runtime::log(true, "Speech GPU failed; retrying on CPU");
        self.stop_child();
        self.settings.device = "cpu".into();
    }
    fn stop_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(stderr) = self.stderr.take() {
            let _ = stderr.join();
        }
        for path in [self.ready.take(), self.local_token.take()]
            .into_iter()
            .flatten()
        {
            let _ = fs::remove_file(path);
        }
    }
    fn request_transcription(&mut self, samples: &[f32]) -> Result<String, String> {
        let response = self
            .agent
            .post(format!("{}/transcribe", self.url))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "audio/wav")
            .send(audio::wav(samples))
            .map_err(|e| e.to_string())?;
        let value = json_response(response)?;
        value["text"]
            .as_str()
            .map(str::to_owned)
            .ok_or("Сервер не повернув текст".into())
    }
}

fn json_response(
    mut response: ureq::http::Response<ureq::Body>,
) -> Result<serde_json::Value, String> {
    let status = response.status();
    let value: serde_json::Value = response.body_mut().read_json().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(value["error"]
            .as_str()
            .unwrap_or("Помилка сервера голосу")
            .into());
    }
    Ok(value)
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop_child();
    }
}

pub fn transcribe_file(path: &Path, base: &Path) -> Result<serde_json::Value, String> {
    let settings = Settings::load(&base.join("speech.ini"));
    let samples = audio::read_wav(&fs::read(path).map_err(|e| e.to_string())?)?;
    let mut client = Client::new(settings, base.into(), Arc::new(AtomicBool::new(false)));
    let start = Instant::now();
    let text = client.transcribe(&samples)?;
    Ok(
        serde_json::json!({"text":text,"elapsed_seconds":start.elapsed().as_secs_f64(),"backend":client.backend}),
    )
}

/// Feed a WAV at microphone speed through the same coalescing/phrase decoder.
/// This exercises live inference without injecting text or using a microphone.
pub fn transcribe_stream_file(path: &Path, base: &Path) -> Result<serde_json::Value, String> {
    let mut settings = Settings::load(&base.join("speech.ini"));
    if let Some(device) =
        std::env::args().find_map(|arg| arg.strip_prefix("--device=").map(str::to_owned))
    {
        if !["cpu", "cuda", "auto"].contains(&device.as_str()) {
            return Err("Device must be cpu, cuda or auto".into());
        }
        settings.device = device;
    }
    let samples = audio::read_wav(&fs::read(path).map_err(|e| e.to_string())?)?;
    let duration = samples.len() as f64 / audio::RATE as f64;
    let mut client = Client::new(settings, base.into(), Arc::new(AtomicBool::new(false)));
    client.prepare()?;
    let mailbox = Arc::new(streaming::Mailbox::default());
    let producer = mailbox.clone();
    let start = Instant::now();
    let capture = thread::spawn(move || {
        let mut end = audio::RATE;
        while end < samples.len() {
            let due = start + Duration::from_secs_f64(end as f64 / audio::RATE as f64);
            thread::sleep(due.saturating_duration_since(Instant::now()));
            if producer
                .publish(streaming::Job {
                    id: 1,
                    target: 0,
                    samples: samples[..end].to_vec(),
                    finished: false,
                })
                .is_err()
            {
                return;
            }
            end += audio::RATE * 4 / 5;
        }
        let due = start + Duration::from_secs_f64(duration);
        thread::sleep(due.saturating_duration_since(Instant::now()));
        let _ = producer.publish(streaming::Job {
            id: 1,
            target: 0,
            samples,
            finished: true,
        });
    });
    let mut decoder = streaming::Decoder::default();
    let mut previews = Vec::new();
    let mut insertions = Vec::new();
    let mut output = live_output::LiveOutput::default();
    let mut insertion_error = None;
    let result = loop {
        let job = mailbox.take().ok_or("Stream closed")?;
        let text = decoder.update(&job, |audio| client.transcribe(audio), |text, committed| {
            let elapsed = start.elapsed().as_secs_f64();
            previews.push(serde_json::json!({"at_seconds":elapsed,"before_release":elapsed < duration,"text":text}));
            if let Err(error) = output.update(&text, &committed, false, |suffix| {
                insertions.push(serde_json::json!({"at_seconds":elapsed,"before_release":elapsed < duration,"text":suffix}));
                Ok(())
            }) { insertion_error = Some(error); }
        });
        match text {
            Err(error) => break Err(error),
            Ok(text) if job.finished => break Ok(text),
            _ => {}
        }
    };
    mailbox.close();
    let _ = capture.join();
    let text = result?;
    let final_elapsed = start.elapsed().as_secs_f64();
    if let Err(error) = output.update(&text, &text, true, |suffix| {
        insertions.push(
            serde_json::json!({"at_seconds":final_elapsed,"before_release":false,"text":suffix}),
        );
        Ok(())
    }) {
        insertion_error = Some(error);
    }
    Ok(serde_json::json!({
        "text":text,"backend":client.backend,"audio_seconds":duration,
        "elapsed_seconds":start.elapsed().as_secs_f64(),
        "after_release_seconds":(start.elapsed().as_secs_f64() - duration).max(0.0),
        "previews":previews,
        "insertions":insertions,"inserted_text":output.written(),"insertion_error":insertion_error,
    }))
}

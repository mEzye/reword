use super::{audio, native::Engine};
use std::{fs, io::Read, path::Path, time::Instant};
use tiny_http::{Header, Method, Response, StatusCode};

enum Runner {
    Native(Engine),
    Managed(super::Client),
}
impl Runner {
    fn backend(&self) -> &str {
        match self {
            Self::Native(engine) => &engine.backend,
            Self::Managed(client) => &client.backend,
        }
    }
    fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        match self {
            Self::Native(engine) => engine.transcribe(samples),
            Self::Managed(client) => client.transcribe(samples),
        }
    }
}

pub fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|v| format!("{v:02x}")).collect())
}

fn argument(name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    std::env::args().find_map(|arg| arg.strip_prefix(&prefix).map(str::to_owned))
}

fn respond(request: tiny_http::Request, status: u16, value: serde_json::Value) {
    let response = Response::from_string(value.to_string())
        .with_status_code(StatusCode(status))
        .with_header(
            Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap(),
        );
    let _ = request.respond(response);
}

pub fn run() -> Result<(), String> {
    if std::env::args().any(|arg| arg == "--exit-on-stdin-close") {
        std::thread::spawn(|| {
            let _ = std::io::stdin().lock().read_to_end(&mut Vec::new());
            std::process::exit(0);
        });
    }
    let base = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .unwrap()
        .to_path_buf();
    let listen = argument("--listen").unwrap_or_else(|| "127.0.0.1:8178".into());
    let device = argument("--device").unwrap_or_else(|| "cpu".into());
    if !["auto", "cpu", "cuda"].contains(&device.as_str()) {
        return Err("Device must be cpu, cuda or auto".into());
    }
    let threads = argument("--threads")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let token_path = argument("--token-file")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| base.join("speech-server.token"));
    let token = match fs::read_to_string(&token_path) {
        Ok(token) if !token.trim().is_empty() => token.trim().to_owned(),
        _ => {
            let token = random_token()?;
            fs::write(&token_path, &token).map_err(|e| e.to_string())?;
            token
        }
    };
    let started = Instant::now();
    eprintln!("Loading Parakeet UK v3 ({device})...");
    let mut engine = if device == "auto" && !std::env::args().any(|arg| arg == "--engine-child") {
        let settings = super::Settings {
            device: "auto".into(),
            threads,
            ..Default::default()
        };
        let mut client = super::Client::new(
            settings,
            base.clone(),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        client.prepare()?;
        Runner::Managed(client)
    } else {
        Runner::Native(Engine::load(&base, &device, threads)?)
    };
    eprintln!(
        "Model ready on {} in {:.2}s",
        engine.backend(),
        started.elapsed().as_secs_f64()
    );
    let server = tiny_http::Server::http(&listen).map_err(|e| e.to_string())?;
    let address = server.server_addr().to_ip().ok_or("No TCP listener")?;
    let url = format!("http://{address}");
    if let Some(path) = argument("--ready-file") {
        fs::write(
            Path::new(&path),
            serde_json::json!({"url":url,"backend":engine.backend()}).to_string(),
        )
        .map_err(|e| e.to_string())?;
    }
    eprintln!("Speech server: {url}; token file: {}", token_path.display());
    // One resident model, serialized inference. Concurrent clients queue here;
    // the native library does not permit concurrent compute on one model.
    for mut request in server.incoming_requests() {
        let authorized = request.headers().iter().any(|header| {
            header.field.equiv("Authorization")
                && header.value.as_str() == format!("Bearer {token}")
        });
        if !authorized {
            respond(
                request,
                401,
                serde_json::json!({"error":"Invalid server token"}),
            );
            continue;
        }
        if request.method() == &Method::Get && request.url() == "/health" {
            respond(
                request,
                200,
                serde_json::json!({"ready":true,"model":"parakeet-uk-v3-q8","backend":engine.backend(),"protocol":1}),
            );
            continue;
        }
        if request.method() != &Method::Post || request.url() != "/transcribe" {
            respond(
                request,
                404,
                serde_json::json!({"error":"Unknown endpoint"}),
            );
            continue;
        }
        const LIMIT: usize = audio::RATE * audio::MAX_SECONDS * 2 + 4096;
        if request.body_length().is_some_and(|length| length > LIMIT) {
            respond(
                request,
                413,
                serde_json::json!({"error":"Audio too large (maximum 120 seconds)"}),
            );
            continue;
        }
        let mut bytes = Vec::new();
        if let Err(error) = request
            .as_reader()
            .take((LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
        {
            respond(request, 400, serde_json::json!({"error":error.to_string()}));
            continue;
        }
        if bytes.len() > LIMIT {
            respond(request, 413, serde_json::json!({"error":"Audio too large"}));
            continue;
        }
        let samples = match audio::read_wav(&bytes) {
            Ok(samples) => samples,
            Err(error) => {
                respond(request, 400, serde_json::json!({"error":error}));
                continue;
            }
        };
        let start = Instant::now();
        match engine.transcribe(&samples) {
            Ok(text) => respond(
                request,
                200,
                serde_json::json!({"text":text,"elapsed_seconds":start.elapsed().as_secs_f64(),"backend":engine.backend()}),
            ),
            Err(error) => respond(request, 500, serde_json::json!({"error":error})),
        }
    }
    Ok(())
}

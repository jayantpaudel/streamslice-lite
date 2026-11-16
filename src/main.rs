#![cfg_attr(windows, windows_subsystem = "windows")]
mod ai;
mod media;
mod setup;

use anyhow::{bail, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Path as ApiPath, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    fs,
    process::Command,
    sync::{mpsc, watch, Mutex, Notify, OwnedSemaphorePermit, RwLock, Semaphore},
    task::JoinHandle,
};
use tower_http::services::ServeDir;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_JOBS: usize = 3;
#[derive(Clone)]
struct Tools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    streamlink: PathBuf,
}
#[derive(Clone, Default, Serialize)]
struct Doctor {
    checking: bool,
    ffmpeg: bool,
    ffprobe: bool,
    streamlink: bool,
    x264: bool,
    subtitles: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    source: String,
    quality: String,
    preset: String,
    layout: String,
    seconds: u32,
    burn: bool,
    language: String,
    glossary: String,
    interval: u64,
    daily_limit: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            source: String::new(),
            quality: "best".into(),
            preset: "quality".into(),
            layout: "fit".into(),
            seconds: 30,
            burn: true,
            language: "auto".into(),
            glossary: String::new(),
            interval: 300,
            daily_limit: 12,
        }
    }
}
impl Settings {
    fn validate(&self) -> Result<()> {
        if !["best", "360p", "480p", "720p", "1080p"].contains(&self.quality.as_str()) {
            bail!("Invalid capture quality");
        }
        validate_export(&self.preset, &self.layout)?;
        if ![15, 30, 45, 60].contains(&self.seconds)
            || ![60, 120, 300].contains(&self.interval)
            || !(1..=120).contains(&self.daily_limit)
        {
            bail!("Invalid duration, interval or daily limit");
        }
        if self.language != "auto"
            && !(self.language.len() == 2 && self.language.bytes().all(|b| b.is_ascii_lowercase()))
        {
            bail!("Use auto or a two-letter language code");
        }
        if self.glossary.chars().count() > 300
            || self.glossary.chars().any(char::is_control)
            || self.source.len() > 32768
        {
            bail!("Settings exceed safe bounds");
        }
        Ok(())
    }
}
fn validate_export(preset: &str, layout: &str) -> Result<()> {
    if !["copy", "balanced", "quality"].contains(&preset)
        || !["original", "fit", "crop"].contains(&layout)
    {
        bail!("Invalid export preset/layout");
    }
    Ok(())
}
#[derive(Clone, Default, Serialize)]
struct CaptureView {
    active: bool,
    source: String,
    note: String,
    session: String,
}
struct Capture {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}
#[derive(Clone, Serialize, Deserialize)]
struct JobView {
    id: String,
    stage: String,
    message: String,
    title: String,
    preset: String,
    created: u64,
    video: Option<String>,
    srt: Option<String>,
    progress: Option<f64>,
    duration: Option<f64>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipRequest {
    mode: String,
    seconds: u32,
    preset: String,
    layout: String,
    #[serde(default)]
    burn: bool,
    #[serde(default)]
    consent: bool,
}
struct Job {
    id: String,
    dir: PathBuf,
    req: ClipRequest,
    cancel: Arc<Notify>,
    snapshot: Option<media::Snapshot>,
    _permit: OwnedSemaphorePermit,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Usage {
    day: u64,
    windows: u32,
}
struct App {
    root: PathBuf,
    token: String,
    origin: String,
    tools: RwLock<Tools>,
    doctor: RwLock<Doctor>,
    settings: RwLock<Settings>,
    usage: Mutex<Usage>,
    key: Mutex<Option<String>>,
    client: reqwest::Client,
    gate: Mutex<()>,
    disk: Mutex<()>,
    inspection: Mutex<()>,
    capture: Mutex<Option<Capture>>,
    view: Mutex<CaptureView>,
    jobs: Mutex<Vec<JobView>>,
    cancels: Mutex<HashMap<String, Arc<Notify>>>,
    slots: Arc<Semaphore>,
    tx: mpsc::Sender<Job>,
    consent: AtomicBool,
    paused: AtomicBool,
    automatic: AtomicBool,
    setup: Mutex<setup::Progress>,
    shutdown: Notify,
    recent: Mutex<Vec<(String, f64, f64)>>,
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn id() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("OS randomness unavailable");
    b.iter().map(|b| format!("{b:02x}")).collect()
}
fn command(exe: &Path) -> Command {
    let mut c = Command::new(exe);
    c.kill_on_drop(true).env_remove("GROQ_API_KEY");
    #[cfg(windows)]
    c.creation_flags(0x08000000 | 0x00004000);
    c
}
async fn atomic_json<T: Serialize>(p: &Path, data: &T) -> Result<()> {
    let tmp = p.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(data)?).await?;
    fs::rename(tmp, p).await?;
    Ok(())
}
fn find_exe(root: &Path, name: &str, depth: u8) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    for e in std::fs::read_dir(root).ok()?.take(5000).flatten() {
        let t = e.file_type().ok()?;
        if t.is_file() && e.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
            return Some(e.path());
        }
        if t.is_dir() {
            if let Some(p) = find_exe(&e.path(), name, depth - 1) {
                return Some(p);
            }
        }
    }
    None
}
fn discover(root: &Path) -> Tools {
    let pick = |name: &str, var: &str, folder: &str| {
        if let Some(v) = std::env::var_os(var) {
            return PathBuf::from(v);
        }
        let file = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.into()
        };
        if let Some(bin) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
        {
            let p = bin.join("tools").join(&file);
            if p.is_file() {
                return p;
            }
        }
        find_exe(&root.join("dependencies").join(folder), &file, 6)
            .unwrap_or_else(|| PathBuf::from(file))
    };
    Tools {
        ffmpeg: pick("ffmpeg", "FFMPEG_PATH", "ffmpeg"),
        ffprobe: pick("ffprobe", "FFPROBE_PATH", "ffmpeg"),
        streamlink: pick("streamlink", "STREAMLINK_PATH", "streamlink"),
    }
}
async fn check(exe: &Path, arg: &str) -> Option<String> {
    let mut c = command(exe);
    c.arg(arg).stdin(Stdio::null()).stderr(Stdio::null());
    match tokio::time::timeout(Duration::from_secs(25), c.output()).await {
        Ok(Ok(o)) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).into()),
        _ => None,
    }
}
async fn refresh(a: &App) {
    let _lock = a.inspection.lock().await;
    a.doctor.write().await.checking = true;
    let t = discover(&a.root);
    let ff = check(&t.ffmpeg, "-version").await.is_some();
    let fp = check(&t.ffprobe, "-version").await.is_some();
    let sl = check(&t.streamlink, "--version").await.is_some();
    let enc = if ff {
        check(&t.ffmpeg, "-encoders").await.unwrap_or_default()
    } else {
        String::new()
    };
    let filters = if ff {
        check(&t.ffmpeg, "-filters").await.unwrap_or_default()
    } else {
        String::new()
    };
    *a.tools.write().await = t;
    *a.doctor.write().await = Doctor {
        checking: false,
        ffmpeg: ff,
        ffprobe: fp,
        streamlink: sl,
        x264: enc.contains("libx264"),
        subtitles: filters.contains("subtitles"),
    };
}
async fn root() -> Result<(PathBuf, std::fs::File)> {
    use fs2::FileExt;
    let p = std::env::var_os("STREAMSLICE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("StreamSliceLite"))
        })
        .unwrap_or(std::env::current_dir()?.join("StreamSliceLite-data"));
    fs::create_dir_all(&p).await?;
    let marker = p.join(".streamslice-root");
    if !marker.exists() {
        if fs::read_dir(&p).await?.next_entry().await?.is_some() {
            bail!("Data folder is not an empty/dedicated StreamSlice folder");
        }
        fs::write(marker, "StreamSlice Lite\n").await?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(p.join("instance.lock"))?;
    lock.try_lock_exclusive()
        .context("StreamSlice is already running; use the existing dashboard")?;
    let work = p.join("work");
    if work.exists() {
        fs::remove_dir_all(&work).await?;
    }
    fs::create_dir_all(work).await?;
    fs::create_dir_all(p.join("exports")).await?;
    Ok((p, lock))
}
#[derive(Debug)]
struct ApiError(anyhow::Error);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":format!("{:#}",self.0)})),
        )
            .into_response()
    }
}
fn api<T: Serialize>(r: Result<T>) -> std::result::Result<Json<T>, ApiError> {
    r.map(Json).map_err(ApiError)
}
async fn host(State(a): State<Arc<App>>, req: Request, next: Next) -> Response {
    if req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        != Some(a.origin.trim_start_matches("http://"))
        || req
            .headers()
            .get(header::ORIGIN)
            .is_some_and(|v| v.to_str().ok() != Some(&a.origin))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut r = next.run(req).await;
    r.headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    r.headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    r.headers_mut()
        .insert("referrer-policy", "no-referrer".parse().unwrap());
    r.headers_mut().insert("content-security-policy","default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; media-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'".parse().unwrap());
    r
}
async fn auth(State(a): State<Arc<App>>, req: Request, next: Next) -> Response {
    if req
        .headers()
        .get("x-streamslice-token")
        .and_then(|v| v.to_str().ok())
        != Some(&a.token)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(req).await
}
async fn index(State(a): State<Arc<App>>) -> Html<String> {
    Html(include_str!("../ui/index.html").replace("__TOKEN__", &a.token))
}
async fn state(State(a): State<Arc<App>>) -> Json<serde_json::Value> {
    let seg = {
        let _g = a.disk.lock().await;
        media::segments(&a.root.join("buffer"))
            .await
            .unwrap_or_default()
    };
    let buffered = seg
        .first()
        .zip(seg.last())
        .map(|(f, l)| (l.end - f.start).max(0.0))
        .unwrap_or(0.0);
    let mut usage = a.usage.lock().await.clone();
    if usage.day != now() / 86400 {
        usage = Usage {
            day: now() / 86400,
            windows: 0,
        };
    }
    let settings = a.settings.read().await.clone();
    let doctor = a.doctor.read().await.clone();
    let view = a.view.lock().await.clone();
    let jobs = a.jobs.lock().await.clone();
    let setup = a.setup.lock().await.clone();
    let ready = a.key.lock().await.is_some();
    Json(
        serde_json::json!({"version":VERSION,"settings":settings,"doctor":doctor,"capture":view,"buffer_seconds":buffered,"jobs":jobs,"setup":setup,"ai_ready":ready,"ai_paused":a.paused.load(Ordering::Relaxed),"automatic":a.automatic.load(Ordering::Relaxed),"usage":usage,"windows":cfg!(windows),"exports":a.root.join("exports"),"disk_free":fs2::available_space(&a.root).ok()}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    source: String,
    quality: String,
    authorized: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirm {
    confirm: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Key {
    #[serde(default)]
    key: String,
    #[serde(default)]
    clear: bool,
}
async fn start(
    State(a): State<Arc<App>>,
    Json(r): Json<Start>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    api(media::start(&a, r)
        .await
        .map(|_| serde_json::json!({"ok":true})))
}
async fn stop(State(a): State<Arc<App>>) -> Json<serde_json::Value> {
    a.automatic.store(false, Ordering::Relaxed);
    media::stop(&a).await;
    Json(serde_json::json!({"ok":true}))
}
async fn enqueue(
    State(a): State<Arc<App>>,
    Json(r): Json<ClipRequest>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    api(media::enqueue(&a, r, false)
        .await
        .map(|id| serde_json::json!({"id":id})))
}
async fn selftest(
    State(a): State<Arc<App>>,
    Json(c): Json<Confirm>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    api(async {
        if !c.confirm {
            bail!("Confirm the local synthetic CPU test first");
        }
        media::enqueue(
            &a,
            ClipRequest {
                mode: "selftest".into(),
                seconds: 15,
                preset: "balanced".into(),
                layout: "fit".into(),
                burn: true,
                consent: false,
            },
            true,
        )
        .await
        .map(|id| serde_json::json!({"id":id}))
    }
    .await)
}
async fn cancel(
    State(a): State<Arc<App>>,
    ApiPath(id): ApiPath<String>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    api(async {
        let map = a.cancels.lock().await;
        map.get(&id)
            .context("Job already finished or does not exist")?
            .notify_one();
        Ok(serde_json::json!({"ok":true}))
    }
    .await)
}
async fn save_settings(
    State(a): State<Arc<App>>,
    Json(p): Json<Settings>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    api(async {
        p.validate()?;
        let mut old = a.settings.write().await;
        atomic_json(&a.root.join("settings.json"), &p).await?;
        *old = p;
        Ok(serde_json::json!({"ok":true}))
    }
    .await)
}
async fn key(
    State(a): State<Arc<App>>,
    Json(r): Json<Key>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    api(async {
        if r.clear {
            a.consent.store(false, Ordering::Relaxed);
            a.automatic.store(false, Ordering::Relaxed);
            *a.key.lock().await = None;
        } else {
            let k = r.key.trim();
            if !(12..=512).contains(&k.len()) || !k.bytes().all(|v| v.is_ascii_graphic()) {
                bail!("Invalid key format");
            }
            *a.key.lock().await = Some(k.into());
            a.paused.store(false, Ordering::Relaxed);
        }
        Ok(serde_json::json!({"ok":true}))
    }
    .await)
}
async fn consent(State(a): State<Arc<App>>, Json(r): Json<Confirm>) -> Json<serde_json::Value> {
    a.consent.store(r.confirm, Ordering::Relaxed);
    if !r.confirm {
        a.automatic.store(false, Ordering::Relaxed);
    }
    Json(serde_json::json!({"ok":true}))
}
async fn automatic(
    State(a): State<Arc<App>>,
    Json(r): Json<Confirm>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    api(async {
        if r.confirm
            && (!a.consent.load(Ordering::Relaxed)
                || a.key.lock().await.is_none()
                || !a.view.lock().await.active
                || a.paused.load(Ordering::Relaxed))
        {
            bail!("Start capture, add a key and allow cloud audio first; AI must not be paused");
        }
        a.automatic.store(r.confirm, Ordering::Relaxed);
        Ok(serde_json::json!({"ok":true}))
    }
    .await)
}
async fn install(
    State(a): State<Arc<App>>,
    Json(r): Json<Confirm>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    api(setup::start(a, r.confirm)
        .await
        .map(|_| serde_json::json!({"ok":true})))
}
async fn recheck(State(a): State<Arc<App>>) -> Json<serde_json::Value> {
    refresh(&a).await;
    Json(serde_json::json!({"ok":true}))
}
async fn quit(State(a): State<Arc<App>>) -> Json<serde_json::Value> {
    a.shutdown.notify_one();
    Json(serde_json::json!({"ok":true}))
}
fn browser(url: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("rundll32.exe")
        .env_remove("GROQ_API_KEY")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open")
        .env_remove("GROQ_API_KEY")
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
fn main() {
    let r = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|rt| rt.block_on(run()));
    if let Err(e) = r {
        eprintln!("{e:#}");
        #[cfg(windows)]
        if !std::env::args().any(|a| a == "--no-browser") {
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::*;
                let msg: Vec<u16> = format!("StreamSlice could not start:\n{e:#}")
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                let title: Vec<u16> = "StreamSlice Lite".encode_utf16().chain(Some(0)).collect();
                MessageBoxW(
                    std::ptr::null_mut(),
                    msg.as_ptr(),
                    title.as_ptr(),
                    MB_OK | MB_ICONERROR,
                );
            }
        }
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let (root, _instance) = root().await?;
    let settings: Settings = fs::read(root.join("settings.json"))
        .await
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .filter(|v: &Settings| v.validate().is_ok())
        .unwrap_or_default();
    let usage = fs::read(root.join("usage.json"))
        .await
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let (tx, rx) = mpsc::channel(MAX_JOBS);
    let tools = discover(&root);
    let a = Arc::new(App {
        root,
        token: format!("{}{}", id(), id()),
        origin: origin.clone(),
        tools: RwLock::new(tools),
        doctor: RwLock::new(Doctor {
            checking: true,
            ..Default::default()
        }),
        settings: RwLock::new(settings),
        usage: Mutex::new(usage),
        key: Mutex::new(None),
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .build()?,
        gate: Mutex::new(()),
        disk: Mutex::new(()),
        inspection: Mutex::new(()),
        capture: Mutex::new(None),
        view: Mutex::new(CaptureView::default()),
        jobs: Mutex::new(vec![]),
        cancels: Mutex::new(HashMap::new()),
        slots: Arc::new(Semaphore::new(MAX_JOBS)),
        tx,
        consent: AtomicBool::new(false),
        paused: AtomicBool::new(false),
        automatic: AtomicBool::new(false),
        setup: Mutex::new(setup::Progress::default()),
        shutdown: Notify::new(),
        recent: Mutex::new(vec![]),
    });
    let mut entries = fs::read_dir(a.root.join("exports")).await?;
    let mut history = vec![];
    while let Some(e) = entries.next_entry().await? {
        if e.path().extension().is_some_and(|x| x == "json") {
            if let Ok(b) = fs::read(e.path()).await {
                if let Ok(j) = serde_json::from_slice::<JobView>(&b) {
                    history.push(j);
                }
            }
        }
    }
    history.sort_by(|a, b| b.created.cmp(&a.created));
    history.truncate(50);
    *a.jobs.lock().await = history;
    atomic_json(
        &a.root.join("run.json"),
        &serde_json::json!({"origin":origin,"version":VERSION,"pid":std::process::id()}),
    )
    .await?;
    let inspection = a.clone();
    tokio::spawn(async move {
        refresh(&inspection).await;
    });
    let worker = tokio::spawn(media::worker(a.clone(), rx));
    let auto = tokio::spawn(media::auto_loop(a.clone()));
    let endpoints = Router::new()
        .route("/state", get(state))
        .route("/start", post(start))
        .route("/stop", post(stop))
        .route("/jobs", post(enqueue))
        .route("/jobs/{id}/cancel", post(cancel))
        .route("/selftest", post(selftest))
        .route("/settings", post(save_settings))
        .route("/key", post(key))
        .route("/consent", post(consent))
        .route("/auto", post(automatic))
        .route("/setup", post(install))
        .route("/recheck", post(recheck))
        .route("/quit", post(quit))
        .route_layer(middleware::from_fn_with_state(a.clone(), auth));
    let app = Router::new()
        .route("/", get(index))
        .nest("/api", endpoints)
        .nest_service("/media", ServeDir::new(a.root.join("exports")))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(a.clone(), host))
        .with_state(a.clone());
    println!("StreamSlice {VERSION}\nDashboard: {origin}");
    if !std::env::args().any(|v| v == "--no-browser") {
        browser(&origin);
    }
    let end = a.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=end.shutdown.notified()=>{}}
            end.automatic.store(false, Ordering::Relaxed);
            media::stop(&end).await;
        })
        .await?;
    auto.abort();
    worker.abort();
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_are_quality_first() {
        let p = Settings::default();
        p.validate().unwrap();
        assert_eq!(p.quality, "best");
        assert_eq!(p.preset, "quality");
        assert_eq!(p.daily_limit, 12);
    }
    #[test]
    fn rejects_filter_injection() {
        assert!(validate_export("quality", "crop;movie=bad").is_err());
    }
    #[test]
    fn checks_settings_limits() {
        let mut p = Settings::default();
        p.daily_limit = 0;
        assert!(p.validate().is_err());
    }
    #[test]
    fn ids_are_safe() {
        let s = id();
        assert_eq!(s.len(), 32);
        assert!(s.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}

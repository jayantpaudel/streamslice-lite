use super::*;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
#[derive(Clone, Debug)]
pub(crate) struct Segment {
    pub name: String,
    pub start: f64,
    pub end: f64,
}
#[derive(Clone)]
pub(crate) struct Snapshot {
    end: f64,
    session: String,
}
#[derive(Clone, Debug, PartialEq)]
enum Source {
    Twitch(String),
    Local(PathBuf),
}
fn twitch(s: &str) -> Result<String> {
    let u = reqwest::Url::parse(s)?;
    let n = u.path().trim_matches('/');
    if u.scheme() != "https"
        || !matches!(u.host_str(), Some("twitch.tv" | "www.twitch.tv"))
        || u.port().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || n.is_empty()
        || n.len() > 25
        || !n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || ["videos", "directory", "search", "settings"].contains(&n)
    {
        bail!("Use a normal HTTPS Twitch live-channel URL without credentials or query strings");
    }
    Ok(format!("https://www.twitch.tv/{}", n.to_lowercase()))
}
fn source(s: &str) -> Result<Source> {
    let s = s.trim().trim_matches('"');
    if !s.is_empty() && s.len() <= 25 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Ok(Source::Twitch(twitch(&format!(
            "https://www.twitch.tv/{s}"
        ))?));
    }
    if s.starts_with("https://") || s.starts_with("http://") {
        return Ok(Source::Twitch(twitch(s)?));
    }
    let p = PathBuf::from(s);
    if !p.is_absolute() || !p.is_file() {
        bail!("Enter a Twitch channel or absolute path to your own video");
    }
    Ok(Source::Local(p.canonicalize()?))
}
pub(crate) fn safe_segment(s: &str) -> bool {
    s.len() == 12
        && s.as_bytes()[9..] == *b".ts"
        && s.as_bytes()[..9].iter().all(u8::is_ascii_digit)
}
pub(crate) async fn segments(dir: &Path) -> Result<Vec<Segment>> {
    let data = match fs::read(dir.join("segments.csv")).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    if data.len() > 128 * 1024 {
        bail!("Oversized capture manifest");
    }
    let mut csv = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(data.as_slice());
    let mut out = vec![];
    for r in csv.records().flatten() {
        let (Some(n), Some(l), Some(r)) = (r.get(0), r.get(1), r.get(2)) else {
            continue;
        };
        let (Ok(start), Ok(end)) = (l.parse::<f64>(), r.parse::<f64>()) else {
            continue;
        };
        if safe_segment(n)
            && start.is_finite()
            && end.is_finite()
            && start >= 0.0
            && end > start
            && fs::metadata(dir.join(n)).await.is_ok_and(|m| m.is_file())
        {
            out.push(Segment {
                name: n.into(),
                start,
                end,
            });
        }
    }
    Ok(out)
}
async fn prune(a: &App) -> Result<String> {
    let _g = a.disk.lock().await;
    let dir = a.root.join("buffer");
    let list = segments(&dir).await?;
    let min = list
        .get(list.len().saturating_sub(30))
        .map(|s| s.name.as_str());
    let mut bytes = 0u64;
    let mut entries = fs::read_dir(&dir).await?;
    while let Some(e) = entries.next_entry().await? {
        let n = e.file_name().to_string_lossy().to_string();
        if safe_segment(&n) {
            if min.is_some_and(|m| n.as_str() < m) {
                fs::remove_file(e.path()).await?;
            } else {
                bytes = bytes.saturating_add(e.metadata().await?.len());
            }
        } else if ["capture.log", "streamlink.log"].contains(&n.as_str())
            && e.metadata().await?.len() > 2 * 1024 * 1024
        {
            std::fs::OpenOptions::new()
                .write(true)
                .open(e.path())?
                .set_len(0)?;
        }
    }
    for old in list.iter().take(list.len().saturating_sub(3)) {
        if bytes <= 192 * 1024 * 1024 {
            break;
        }
        let p = dir.join(&old.name);
        if let Ok(m) = fs::metadata(&p).await {
            if fs::remove_file(p).await.is_ok() {
                bytes = bytes.saturating_sub(m.len());
            }
        }
    }
    if bytes > 256 * 1024 * 1024 {
        bail!("256 MiB buffer safety limit exceeded; choose a lower source bitrate");
    }
    Ok(list.last().map(|s| s.name.clone()).unwrap_or_default())
}
pub(crate) async fn start(a: &Arc<App>, r: Start) -> Result<()> {
    let _gate = a.gate.lock().await;
    if a.setup.lock().await.running {
        bail!("Wait for helper setup");
    }
    if !r.authorized {
        bail!("Confirm ownership/permission to capture and reuse this source");
    }
    if fs2::available_space(&a.root)? < 512 * 1024 * 1024 {
        bail!("Free at least 512 MiB of disk space");
    }
    let d = a.doctor.read().await.clone();
    if d.checking || !d.ffmpeg || !d.ffprobe {
        bail!("Wait for checks or install FFmpeg/ffprobe");
    }
    let src = source(&r.source)?;
    if matches!(src, Source::Twitch(_)) && !d.streamlink {
        bail!("Install the complete Streamlink portable runtime");
    }
    let quality = match r.quality.as_str() {
        "best" => "best",
        "360p" => "360p,480p,worst",
        "480p" => "480p,360p,worst",
        "720p" => "720p,480p,360p,worst",
        "1080p" => "1080p,1080p60,720p,480p,worst",
        _ => bail!("Invalid quality"),
    }
    .to_string();
    let mut capture = a.capture.lock().await;
    if capture.as_ref().is_some_and(|c| !c.task.is_finished()) {
        bail!("Capture already running");
    }
    if let Some(c) = capture.take() {
        let _ = c.task.await;
    }
    let dir = a.root.join("buffer");
    {
        let _g = a.disk.lock().await;
        if dir.exists() {
            fs::remove_dir_all(&dir).await?;
        }
        fs::create_dir_all(&dir).await?;
    }
    {
        let mut p = a.settings.write().await;
        p.source = r.source.clone();
        p.quality = r.quality;
        atomic_json(&a.root.join("settings.json"), &*p).await?;
    }
    *a.view.lock().await = CaptureView {
        active: true,
        source: r.source,
        note: "Starting; wait 15–30 seconds for completed segments.".into(),
        session: id(),
    };
    a.automatic.store(false, Ordering::Relaxed);
    let (tx, rx) = watch::channel(false);
    let app = a.clone();
    let task = tokio::spawn(async move {
        let r = supervise(&app, src, quality, rx).await;
        let mut v = app.view.lock().await;
        v.active = false;
        v.note = match r {
            Ok(()) => "Stopped; completed footage remains until the next Start.".into(),
            Err(e) => format!("Capture stopped: {e:#}"),
        };
        app.automatic.store(false, Ordering::Relaxed);
    });
    *capture = Some(Capture { stop: tx, task });
    Ok(())
}
pub(crate) async fn stop(a: &App) {
    if let Some(mut c) = a.capture.lock().await.take() {
        let _ = c.stop.send(true);
        if tokio::time::timeout(Duration::from_secs(12), &mut c.task)
            .await
            .is_err()
        {
            c.task.abort();
            let _ = c.task.await;
        }
    }
    a.view.lock().await.active = false;
}
async fn supervise(a: &App, src: Source, q: String, mut stop: watch::Receiver<bool>) -> Result<()> {
    for attempt in 0..=3usize {
        let r = capture(a, src.clone(), &q, stop.clone()).await;
        if *stop.borrow() || matches!(src, Source::Local(_)) {
            return r;
        }
        if r.as_ref()
            .err()
            .is_some_and(|e| e.to_string().contains("safety limit"))
        {
            return r;
        }
        if attempt == 3 {
            bail!("Reconnects exhausted; inspect buffer logs");
        }
        let delay = [5, 15, 30][attempt];
        a.view.lock().await.note = format!("Disconnected; reconnect {}/3 in {delay}s", attempt + 1);
        tokio::select! {_=stop.changed()=>return Ok(()),_=tokio::time::sleep(Duration::from_secs(delay))=>{}}
        {
            let _g = a.disk.lock().await;
            let dir = a.root.join("buffer");
            fs::remove_dir_all(&dir).await?;
            fs::create_dir_all(dir).await?;
        }
        a.view.lock().await.session = id();
    }
    Ok(())
}
async fn capture(a: &App, src: Source, q: &str, mut stop: watch::Receiver<bool>) -> Result<()> {
    let t = a.tools.read().await.clone();
    let dir = a.root.join("buffer");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("capture.log"))?;
    let mut ff = command(&t.ffmpeg);
    ff.current_dir(&dir).args([
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "warning",
        "-nostats",
        "-fflags",
        "+genpts",
        "-threads",
        "1",
    ]);
    let mut link = None;
    if let Source::Twitch(url) = &src {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("streamlink.log"))?;
        let mut sl = command(&t.streamlink);
        sl.args([
            "--loglevel",
            "warning",
            "--stdout",
            "--ringbuffer-size",
            "8M",
            "--ffmpeg-ffmpeg",
        ])
        .arg(&t.ffmpeg)
        .args([
            "--retry-streams",
            "5",
            "--retry-max",
            "6",
            "--retry-open",
            "3",
            url,
            q,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log));
        link = Some(sl.spawn()?);
        ff.args(["-i", "pipe:0"]).stdin(Stdio::piped());
    } else if let Source::Local(p) = &src {
        ff.arg("-re").arg("-i").arg(p).stdin(Stdio::null());
    }
    ff.args([
        "-map",
        "0:v:0",
        "-map",
        "0:a:0?",
        "-c",
        "copy",
        "-f",
        "segment",
        "-segment_format",
        "mpegts",
        "-segment_time",
        "6",
        "-segment_list",
        "segments.csv",
        "-segment_list_type",
        "csv",
        "-segment_list_size",
        "32",
        "-segment_list_flags",
        "+live",
        "-reset_timestamps",
        "1",
        "%09d.ts",
    ])
    .stdout(Stdio::null())
    .stderr(Stdio::from(log));
    let mut child = ff.spawn()?;
    let forward = if let Some(sl) = link.as_mut() {
        let mut input = sl.stdout.take().context("Missing source pipe")?;
        let mut output = child.stdin.take().context("Missing capture pipe")?;
        Some(tokio::spawn(async move {
            let _ = tokio::io::copy(&mut input, &mut output).await;
        }))
    } else {
        None
    };
    let mut waiting = Box::pin(child.wait());
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    let mut last = String::new();
    let mut seen = tokio::time::Instant::now();
    let result: Result<()> = loop {
        tokio::select! {r=&mut waiting=>break r.map_err(anyhow::Error::from).and_then(|s|if s.success(){Ok(())}else{Err(anyhow::anyhow!("FFmpeg capture failed ({s}); inspect buffer/capture.log"))}),_=stop.changed()=>break Ok(()),_=tick.tick()=>{match prune(a).await{Ok(n)=>{if n!=last{last=n;seen=tokio::time::Instant::now();}if seen.elapsed()>Duration::from_secs(120){break Err(anyhow::anyhow!("No new segments for 120 seconds"));}},Err(e)=>break Err(e)}}}
    };
    drop(waiting);
    let _ = child.start_kill();
    let _ = child.wait().await;
    if let Some(mut sl) = link {
        let _ = sl.start_kill();
        let _ = sl.wait().await;
    }
    if let Some(f) = forward {
        f.abort();
    }
    result
}
async fn pin(a: &App, dir: &Path, secs: f64) -> Result<Snapshot> {
    let _g = a.disk.lock().await;
    let root = a.root.join("buffer");
    let list = segments(&root).await?;
    let last = list
        .last()
        .context("No completed footage yet; wait for the buffer")?;
    let mut manifest = String::new();
    for s in list.iter().filter(|s| s.end > last.end - secs) {
        fs::copy(root.join(&s.name), dir.join(&s.name)).await?;
        manifest.push_str(&format!("file '{}'\n", s.name));
    }
    fs::write(dir.join("concat.txt"), manifest).await?;
    Ok(Snapshot {
        end: last.end,
        session: a.view.lock().await.session.clone(),
    })
}
pub(crate) async fn update(a: &App, id: &str, stage: &str, msg: &str) {
    if let Some(v) = a.jobs.lock().await.iter_mut().find(|v| v.id == id) {
        v.stage = stage.into();
        v.message = msg.into();
    }
}
pub(crate) async fn enqueue(a: &Arc<App>, mut r: ClipRequest, test: bool) -> Result<String> {
    let _gate = a.gate.lock().await;
    if a.setup.lock().await.running {
        bail!("Wait for helper setup");
    }
    if ![15, 30, 45, 60].contains(&r.seconds)
        || (!test && !["save", "ai"].contains(&r.mode.as_str()))
    {
        bail!("Invalid mode/duration");
    }
    validate_export(&r.preset, &r.layout)?;
    if r.mode == "ai"
        && (!r.consent
            || !a.consent.load(Ordering::Relaxed)
            || a.key.lock().await.is_none()
            || a.paused.load(Ordering::Relaxed))
    {
        bail!("Add a key and allow cloud audio; AI must not be paused");
    }
    if r.mode == "save" {
        r.burn = false;
    }
    if r.preset == "copy" {
        r.layout = "original".into();
        if r.burn {
            bail!("Burned captions require an encoded preset");
        }
    }
    let d = a.doctor.read().await;
    if !d.ffmpeg
        || !d.ffprobe
        || d.checking
        || (r.preset != "copy" && !d.x264)
        || (r.burn && !d.subtitles)
    {
        bail!("Required helper/encoder/caption support missing; install or recheck helpers");
    }
    drop(d);
    if fs2::available_space(&a.root)? < 512 * 1024 * 1024 {
        bail!("Less than 512 MiB disk space remains");
    }
    let permit = a
        .slots
        .clone()
        .try_acquire_owned()
        .context("Three jobs already active/queued; wait or cancel one")?;
    let id = id();
    let dir = a.root.join("work").join(&id);
    fs::create_dir_all(&dir).await?;
    let snapshot = if test {
        None
    } else {
        match pin(
            a,
            &dir,
            if r.mode == "ai" {
                90.0
            } else {
                r.seconds as f64
            },
        )
        .await
        {
            Ok(s) => Some(s),
            Err(e) => {
                let _ = fs::remove_dir_all(&dir).await;
                return Err(e);
            }
        }
    };
    let cancel = Arc::new(Notify::new());
    let view = JobView {
        id: id.clone(),
        stage: "queued".into(),
        message: "Source pinned safely on disk".into(),
        title: if test {
            "Local PC self-test".into()
        } else {
            "Recent footage".into()
        },
        preset: r.preset.clone(),
        created: now(),
        video: None,
        srt: None,
        progress: None,
        duration: None,
    };
    {
        let mut v = a.jobs.lock().await;
        v.insert(0, view);
        v.truncate(50);
    }
    a.cancels.lock().await.insert(id.clone(), cancel.clone());
    let j = Job {
        id: id.clone(),
        dir: dir.clone(),
        req: r,
        cancel,
        snapshot,
        _permit: permit,
    };
    if a.tx.try_send(j).is_err() {
        a.cancels.lock().await.remove(&id);
        let _ = fs::remove_dir_all(dir).await;
        bail!("Worker unavailable");
    }
    Ok(id)
}
pub(crate) async fn worker(a: Arc<App>, mut rx: mpsc::Receiver<Job>) {
    while let Some(j) = rx.recv().await {
        tokio::select! {biased;_=j.cancel.notified()=>update(&a,&j.id,"cancelled","Cancelled; cloud requests already sent cannot be undone.").await,r=process(&a,&j)=>{if let Err(e)=r{update(&a,&j.id,"failed",&format!("{e:#}")).await;}}}
        a.cancels.lock().await.remove(&j.id);
        if let Ok(mut files) = fs::read_dir(&j.dir).await {
            while let Ok(Some(f)) = files.next_entry().await {
                if f.path().extension().is_none_or(|x| x != "log") {
                    let _ = fs::remove_file(f.path()).await;
                }
            }
        }
    }
}
pub(crate) async fn auto_loop(a: Arc<App>) {
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut last = tokio::time::Instant::now();
    loop {
        tick.tick().await;
        if !a.automatic.load(Ordering::Relaxed) || !a.view.lock().await.active {
            last = tokio::time::Instant::now();
            continue;
        }
        let p = a.settings.read().await.clone();
        if last.elapsed().as_secs() < p.interval {
            continue;
        }
        last = tokio::time::Instant::now();
        let r = ClipRequest {
            mode: "ai".into(),
            seconds: p.seconds,
            preset: p.preset.clone(),
            layout: p.layout,
            burn: p.burn && p.preset != "copy",
            consent: true,
        };
        if let Err(e) = enqueue(&a, r, false).await {
            a.view.lock().await.note = format!("Automatic sample skipped: {e}");
        }
    }
}
fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}
async fn tail(p: &Path) -> String {
    let r = async {
        let mut f = fs::File::open(p).await?;
        let n = f.metadata().await?.len();
        f.seek(std::io::SeekFrom::Start(n.saturating_sub(8192)))
            .await?;
        let mut b = vec![];
        f.take(8192).read_to_end(&mut b).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&b).into_owned())
    }
    .await;
    r.unwrap_or_default()
}
async fn ff(a: &App, dir: &Path, stage: &str, argv: &[String]) -> Result<()> {
    let t = a.tools.read().await.clone();
    let logpath = dir.join(format!("{stage}.log"));
    let progress = dir.join(format!("progress-{stage}.txt"));
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&logpath)?;
    let mut c = command(&t.ffmpeg);
    c.current_dir(dir)
        .args([
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-loglevel",
            "warning",
            "-y",
            "-threads",
            "1",
            "-filter_threads",
            "1",
            "-filter_complex_threads",
            "1",
            "-progress",
        ])
        .arg(&progress)
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    let mut child = c.spawn()?;
    let mut waiting = Box::pin(child.wait());
    let timeout = tokio::time::sleep(Duration::from_secs(1800));
    tokio::pin!(timeout);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let id = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let status = loop {
        tokio::select! {r=&mut waiting=>break r?,_=&mut timeout=>bail!("Encoding timed out after 30 minutes; choose a faster preset/shorter clip"),_=tick.tick()=>{if stage=="export"{let text=tail(&progress).await;let us=text.lines().filter_map(|l|l.strip_prefix("out_time_us=")).filter_map(|s|s.parse::<f64>().ok()).next_back();if let Some(us)=us{if us.is_finite()&&us>=0.0{if let Some(v)=a.jobs.lock().await.iter_mut().find(|v|v.id==id){if let Some(d)=v.duration{v.progress=Some((us/1e6/d.max(0.1)).clamp(0.0,0.995));}}}}}if fs::metadata(&logpath).await.is_ok_and(|m|m.len()>2*1024*1024){let _=std::fs::OpenOptions::new().write(true).open(&logpath).and_then(|f|f.set_len(0));}}}
    };
    if !status.success() {
        bail!(
            "FFmpeg {stage} failed ({status}). {}\nLog: {}",
            tail(&logpath).await,
            logpath.display()
        );
    }
    Ok(())
}
struct Video {
    seconds: f64,
    width: u32,
    height: u32,
    fps: f64,
}
pub(crate) fn rate(s: &str) -> f64 {
    let r = s
        .split_once('/')
        .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?))
        .or_else(|| s.parse::<f64>().ok())
        .unwrap_or(30.0);
    if r.is_finite() && (1.0..=240.0).contains(&r) {
        r
    } else {
        30.0
    }
}
async fn probe(a: &App, path: &Path) -> Result<Video> {
    let t = a.tools.read().await.clone();
    let mut c = command(&t.ffprobe);
    c.args([
        "-v",
        "error",
        "-show_entries",
        "format=duration:stream=codec_type,width,height,avg_frame_rate,r_frame_rate",
        "-of",
        "json",
    ])
    .arg(path)
    .stdin(Stdio::null())
    .stderr(Stdio::null());
    let o = tokio::time::timeout(Duration::from_secs(30), c.output()).await??;
    if !o.status.success() {
        bail!("Cannot probe video");
    }
    let v: serde_json::Value = serde_json::from_slice(&o.stdout)?;
    let seconds = v
        .pointer("/format/duration")
        .and_then(|v| v.as_str())
        .context("Missing duration")?
        .parse::<f64>()?;
    let s = v
        .get("streams")
        .and_then(|v| v.as_array())
        .and_then(|v| {
            v.iter()
                .find(|s| s.get("codec_type").and_then(|v| v.as_str()) == Some("video"))
        })
        .context("No video stream")?;
    let width = s.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let height = s.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let raw = s
        .get("avg_frame_rate")
        .and_then(|v| v.as_str())
        .filter(|v| *v != "0/0")
        .or_else(|| s.get("r_frame_rate").and_then(|v| v.as_str()))
        .unwrap_or("30/1");
    if !seconds.is_finite()
        || !(0.5..=240.0).contains(&seconds)
        || width < 2
        || height < 2
        || width > 16384
        || height > 16384
    {
        bail!("Invalid media dimensions/duration");
    }
    Ok(Video {
        seconds,
        width,
        height,
        fps: rate(raw),
    })
}
pub(crate) fn size(p: &str, l: &str, w: u32, h: u32) -> (u32, u32) {
    if p == "copy" {
        return (w, h);
    }
    let edge = if p == "quality" { 1080 } else { 720 };
    if l == "square" {
        (edge, edge)
    } else {
        (edge, edge * 16 / 9)
    }
}
fn filter(_layout: &str, w: u32, h: u32, fps: f64) -> String {
    // Fit the entire DISPLAY aspect ratio, including anamorphic sample aspect ratio.
    // There is deliberately no crop filter and no forced aspect-ratio stretching.
    format!("scale=w='max(2,trunc(min({w},{h}*dar)/2)*2)':h='max(2,trunc(min({h},{w}/dar)/2)*2)':flags=lanczos,setsar=1,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black,fps={fps:.6}")
}
async fn budget(a: &App) -> Result<()> {
    let limit = a.settings.read().await.daily_limit;
    let mut u = a.usage.lock().await;
    if u.day != now() / 86400 {
        *u = Usage {
            day: now() / 86400,
            windows: 0,
        };
    }
    if u.windows >= limit {
        a.automatic.store(false, Ordering::Relaxed);
        bail!("Daily UTC AI-window guard reached; this is not a financial spending cap");
    }
    u.windows += 1;
    atomic_json(&a.root.join("usage.json"), &*u).await
}
async fn process(a: &App, j: &Job) -> Result<()> {
    let test = j.req.mode == "selftest";
    update(
        a,
        &j.id,
        "preparing",
        if test {
            "Generating a local synthetic test; no cloud requests."
        } else {
            "Remuxing pinned source without re-encoding."
        },
    )
    .await;
    if test {
        ff(
            a,
            &j.dir,
            "generate",
            &args(&[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=15",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-t",
                "3",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-threads",
                "1",
                "-c:a",
                "aac",
                "input.mkv",
            ]),
        )
        .await?;
    } else {
        ff(
            a,
            &j.dir,
            "merge",
            &args(&[
                "-fflags",
                "+genpts",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                "concat.txt",
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-c",
                "copy",
                "-avoid_negative_ts",
                "make_zero",
                "input.mkv",
            ]),
        )
        .await?;
    }
    let info = probe(a, &j.dir.join("input.mkv")).await?;
    let mut start = (info.seconds - j.req.seconds as f64).max(0.0);
    let mut duration = info.seconds.min(j.req.seconds as f64);
    let mut title = if test {
        "Local media self-test passed".into()
    } else {
        "Recent footage".into()
    };
    let mut cues = vec![];
    let mut offset = 0.0;
    if j.req.mode == "ai" {
        budget(a).await?;
        let window = info.seconds.min(90.0);
        let base = (info.seconds - window).max(0.0);
        update(
            a,
            &j.id,
            "transcribing",
            "Uploading only the allowed ≤90-second mono audio window.",
        )
        .await;
        ff(
            a,
            &j.dir,
            "audio",
            &args(&[
                "-ss",
                &format!("{base:.3}"),
                "-i",
                "input.mkv",
                "-t",
                &format!("{window:.3}"),
                "-vn",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-c:a",
                "flac",
                "-threads",
                "1",
                "candidate.flac",
            ]),
        )
        .await?;
        let t = ai::transcribe(a, &j.dir.join("candidate.flac"), window).await?;
        update(
            a,
            &j.id,
            "ranking",
            "Choosing a complete spoken highlight; the model cannot see video.",
        )
        .await;
        let d = ai::rank(a, &t, window, j.req.seconds as f64).await?;
        if !d.keep {
            update(
                a,
                &j.id,
                "skipped",
                "No strong complete spoken highlight; manual Save remains available.",
            )
            .await;
            return Ok(());
        }
        start = base + d.start;
        duration = d.duration;
        offset = d.start;
        title = if d.title.is_empty() {
            "AI-selected highlight".into()
        } else {
            d.title
        };
        cues = t.cues;
        if let Some(s) = &j.snapshot {
            let left = s.end - info.seconds + start;
            let right = left + duration;
            if a.recent.lock().await.iter().any(|(session, l, r)| {
                session == &s.session
                    && (right.min(*r) - left.max(*l)).max(0.0) > duration.min(r - l) * 0.65
            }) {
                update(
                    a,
                    &j.id,
                    "skipped",
                    "Overlaps a recent AI highlight; duplicate skipped.",
                )
                .await;
                return Ok(());
            }
        }
    } else if test {
        start = 0.0;
        duration = info.seconds.min(3.0);
        cues = vec![
            ai::Cue {
                start: 0.05,
                end: 1.4,
                text: "Your PC can render clips".into(),
            },
            ai::Cue {
                start: 1.5,
                end: 2.9,
                text: "Video audio captions OK".into(),
            },
        ];
    }
    if let Some(v) = a.jobs.lock().await.iter_mut().find(|v| v.id == j.id) {
        v.duration = Some(duration);
    }
    let (w, h) = size(&j.req.preset, &j.req.layout, info.width, info.height);
    let subtitles = ai::srt(&cues, offset, duration);
    update(
        a,
        &j.id,
        "exporting",
        if j.req.preset == "quality" {
            "CRF 17 Quality: one background CPU thread; allow extra time."
        } else if j.req.preset == "copy" {
            "Original-quality copy; keyframe-approximate cuts."
        } else {
            "Balanced encoding on one background CPU thread."
        },
    )
    .await;
    let mut argv = args(&[
        "-ss",
        &format!("{start:.3}"),
        "-i",
        "input.mkv",
        "-t",
        &format!("{duration:.3}"),
        "-map",
        "0:v:0",
        "-map",
        "0:a:0?",
        "-map_metadata",
        "-1",
    ]);
    if j.req.preset == "copy" {
        argv.extend(args(&["-c", "copy"]));
    } else {
        let fps = info.fps.min(if j.req.preset == "quality" {
            60.0
        } else {
            30.0
        });
        let mut f = filter(&j.req.layout, w, h, fps);
        if j.req.burn {
            if subtitles.is_empty() {
                bail!("No reliable captions to burn; export without burning");
            }
            fs::write(
                j.dir.join("captions.ass"),
                ai::ass(&cues, offset, duration, w, h),
            )
            .await?;
            f.push_str(",subtitles=filename=captions.ass");
        }
        argv.extend(args(&[
            "-vf",
            &f,
            "-c:v",
            "libx264",
            "-preset",
            if j.req.preset == "quality" {
                "medium"
            } else {
                "veryfast"
            },
            "-crf",
            if j.req.preset == "quality" {
                "17"
            } else {
                "21"
            },
            "-x264-params",
            "rc-lookahead=10:sync-lookahead=0:ref=2:bframes=2",
            "-pix_fmt",
            "yuv420p",
            "-threads",
            "1",
            "-c:a",
            "aac",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-b:a",
            if j.req.preset == "quality" {
                "192k"
            } else {
                "128k"
            },
            "-af",
            "loudnorm=I=-16:TP=-1.5:LRA=11",
        ]));
    }
    argv.extend(args(&[
        "-avoid_negative_ts",
        "make_zero",
        "-movflags",
        "+faststart",
        "-f",
        "mp4",
        "render.mp4",
    ]));
    ff(a, &j.dir, "export", &argv).await?;
    let _ = probe(a, &j.dir.join("render.mp4")).await?;
    fs::rename(
        j.dir.join("render.mp4"),
        a.root.join("exports").join(format!("{}.mp4", j.id)),
    )
    .await?;
    let srt = if !subtitles.is_empty() {
        fs::write(
            a.root.join("exports").join(format!("{}.srt", j.id)),
            subtitles,
        )
        .await?;
        Some(format!("/media/{}.srt", j.id))
    } else {
        None
    };
    let view = JobView {
        id: j.id.clone(),
        stage: "done".into(),
        message: if test {
            "Local video/audio/encoder/caption test passed; Twitch/cloud still need your own check."
                .into()
        } else if j.req.preset == "copy" {
            "Copy complete; cut/caption timing may vary by a keyframe.".into()
        } else {
            format!("{w}×{h} H.264/AAC export complete. Review framing and captions.")
        },
        title,
        preset: j.req.preset.clone(),
        created: now(),
        video: Some(format!("/media/{}.mp4", j.id)),
        srt,
        progress: Some(1.0),
        duration: Some(duration),
    };
    atomic_json(
        &a.root.join("exports").join(format!("{}.json", j.id)),
        &view,
    )
    .await?;
    if let Some(v) = a.jobs.lock().await.iter_mut().find(|v| v.id == j.id) {
        *v = view;
    }
    if j.req.mode == "ai" {
        if let Some(s) = &j.snapshot {
            let left = s.end - info.seconds + start;
            let mut recent = a.recent.lock().await;
            recent.push((s.session.clone(), left, left + duration));
            if recent.len() > 50 {
                recent.remove(0);
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_names() {
        assert!(safe_segment("000000001.ts"));
        for s in ["../one.ts", "000000001.ts/evil", "C:/one.ts"] {
            assert!(!safe_segment(s));
        }
    }
    #[test]
    fn urls() {
        assert_eq!(
            twitch("https://twitch.tv/Example").unwrap(),
            "https://www.twitch.tv/example"
        );
        for s in [
            "http://twitch.tv/a",
            "https://twitch.tv/videos/1",
            "https://evil.test/a",
            "https://twitch.tv/a?x=1",
        ] {
            assert!(twitch(s).is_err());
        }
    }
    #[test]
    fn rates() {
        assert!((rate("60000/1001") - 59.94).abs() < 0.01);
        assert_eq!(rate("0/0"), 30.0);
    }
    #[test]
    fn dimensions() {
        assert_eq!(size("quality", "vertical", 1920, 1080), (1080, 1920));
        assert_eq!(size("balanced", "vertical", 1920, 1080), (720, 1280));
        assert_eq!(size("copy", "original", 640, 360), (640, 360));
        assert_eq!(size("quality", "square", 1920, 1080), (1080, 1080));
        assert_eq!(size("balanced", "square", 1920, 1080), (720, 720));
    }
    #[tokio::test]
    async fn evicted_segments() {
        let p = std::env::temp_dir().join(format!("ss-test-{}", id()));
        fs::create_dir_all(&p).await.unwrap();
        fs::write(
            p.join("segments.csv"),
            "000000001.ts,0,6\n000000002.ts,6,12\n",
        )
        .await
        .unwrap();
        fs::write(p.join("000000002.ts"), b"test").await.unwrap();
        assert_eq!(segments(&p).await.unwrap().len(), 1);
        fs::remove_dir_all(p).await.unwrap();
    }
}

#[cfg(test)]
mod no_crop_filter_tests {
    use super::*;
    #[test]
    fn fitting_never_uses_a_crop_filter() {
        for l in ["vertical", "square"] {
            let (w, h) = size("quality", l, 1920, 1080);
            let f = filter(l, w, h, 59.94);
            assert!(!f.contains("crop="));
            assert!(f.contains("pad="));
            assert!(f.contains("dar"));
            assert!(f.contains("setsar=1"));
        }
    }
}

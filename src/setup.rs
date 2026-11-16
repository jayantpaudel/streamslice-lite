use super::*;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
#[derive(Clone, Default, Serialize)]
pub(crate) struct Progress {
    pub running: bool,
    pub stage: String,
    pub bytes: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
}
struct Asset {
    name: &'static str,
    url: &'static str,
    hash: &'static str,
    required: &'static [&'static str],
}
const ASSETS:[Asset;2]=[
Asset{name:"ffmpeg",url:"https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip",hash:"60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba",required:&["ffmpeg.exe","ffprobe.exe"]},
Asset{name:"streamlink",url:"https://github.com/streamlink/windows-builds/releases/download/8.6.3-1/streamlink-8.6.3-1-py315-x86_64.zip",hash:"3b1f1470d4b135080a8107ca2917379ca3ff5acadbcdc215568b79eea1450497",required:&["streamlink.exe"]}];
pub(crate) async fn start(a: Arc<App>, confirm: bool) -> Result<()> {
    let gate = a.gate.lock().await;
    if !confirm {
        bail!("Confirm the ~200 MB helper download");
    }
    if !cfg!(windows) {
        bail!("Portable setup is Windows-only; use your package manager on other OSes");
    }
    if a.doctor.read().await.checking {
        bail!("Wait for the initial helper check");
    }
    if a.view.lock().await.active || a.slots.available_permits() != MAX_JOBS {
        bail!("Stop capture and finish/cancel jobs first");
    }
    if fs2::available_space(&a.root)? < 1536 * 1024 * 1024 {
        bail!("Setup needs 1.5 GiB free disk space");
    }
    {
        let mut p = a.setup.lock().await;
        if p.running {
            bail!("Setup already running");
        }
        *p = Progress {
            running: true,
            stage: "Preparing verified downloads".into(),
            ..Default::default()
        };
    }
    drop(gate);
    tokio::spawn(async move {
        let r = install(&a).await;
        let mut p = a.setup.lock().await;
        p.running = false;
        match r {
            Ok(()) => {
                p.stage = "Helpers ready".into();
                p.error = None;
            }
            Err(e) => {
                p.stage = "Setup stopped safely".into();
                p.error = Some(format!("{e:#}"));
            }
        }
    });
    Ok(())
}
fn host(s: &str) -> bool {
    matches!(
        s,
        "github.com" | "release-assets.githubusercontent.com" | "objects.githubusercontent.com"
    )
}
async fn install(a: &App) -> Result<()> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(1800))
        .user_agent("StreamSliceLite/0.3 portable setup")
        .redirect(reqwest::redirect::Policy::custom(|r| {
            if r.previous().len() > 6
                || r.url().scheme() != "https"
                || !r.url().host_str().is_some_and(host)
            {
                r.error("Unexpected download redirect")
            } else {
                r.follow()
            }
        }))
        .build()?;
    let base = a.root.join("dependencies");
    fs::create_dir_all(&base).await?;
    for asset in &ASSETS {
        // Explicit repair installs both pinned distributions, even if PATH contains other versions.
        let path = base.join(format!("{}.download", asset.name));
        {
            let mut p = a.setup.lock().await;
            p.stage = format!("Downloading {}", asset.name);
            p.bytes = 0;
            p.total = None;
        }
        let mut r = client.get(asset.url).send().await?.error_for_status()?;
        if r.content_length().is_some_and(|v| v > 180 * 1024 * 1024) {
            bail!("Unexpected archive size");
        }
        a.setup.lock().await.total = r.content_length();
        let mut f = fs::File::create(&path).await?;
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        while let Some(b) = r.chunk().await? {
            bytes = bytes.checked_add(b.len() as u64).context("Size overflow")?;
            if bytes > 180 * 1024 * 1024 {
                bail!("Download exceeded 180 MiB");
            }
            hash.update(&b);
            f.write_all(&b).await?;
            a.setup.lock().await.bytes = bytes;
        }
        f.flush().await?;
        drop(f);
        if format!("{:x}", hash.finalize()) != asset.hash {
            let _ = fs::remove_file(path).await;
            bail!(
                "{} SHA-256 mismatch; nothing extracted or executed",
                asset.name
            );
        }
        a.setup.lock().await.stage = format!("Verified hash; extracting {}", asset.name);
        let temp = base.join(format!(".{}-{}", asset.name, id()));
        let archive = path.clone();
        let dir = temp.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || extract(&archive, &dir)).await? {
            let _ = fs::remove_dir_all(&temp).await;
            return Err(e);
        }
        for name in asset.required {
            if find_exe(&temp, name, 6).is_none() {
                bail!("Expected executable missing: {name}");
            }
        }
        let target = base.join(asset.name);
        let backup = base.join(format!(".old-{}-{}", asset.name, id()));
        if target.exists() {
            fs::rename(&target, &backup).await?;
        }
        if let Err(e) = fs::rename(&temp, &target).await {
            if backup.exists() {
                let _ = fs::rename(&backup, &target).await;
            }
            return Err(e.into());
        }
        refresh(a).await;
        let d = a.doctor.read().await.clone();
        let ready = if asset.name == "ffmpeg" {
            d.ffmpeg && d.ffprobe && d.x264 && d.subtitles
        } else {
            d.streamlink
        };
        if !ready {
            let _ = fs::rename(&target, base.join(format!(".failed-{}", id()))).await;
            if backup.exists() {
                let _ = fs::rename(&backup, &target).await;
            }
            refresh(a).await;
            bail!("Verified {} could not run. Check OS/security diagnostics; do not disable protection.",asset.name);
        }
        if backup.exists() {
            let _ = fs::remove_dir_all(backup).await;
        }
        let _ = fs::remove_file(path).await;
    }
    Ok(())
}
fn safe_path(s: &str) -> Result<PathBuf> {
    if s.is_empty() || s.len() > 1024 || s.contains('\\') || s.contains(':') || s.starts_with('/') {
        bail!("Unsafe archive name");
    }
    let p = Path::new(s);
    if p.components().any(|c| {
        !matches!(
            c,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    }) {
        bail!("Unsafe archive path");
    }
    Ok(p.into())
}
fn extract(archive: &Path, target: &Path) -> Result<()> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    if z.len() > 20000 {
        bail!("Too many archive entries");
    }
    std::fs::create_dir_all(target)?;
    let mut total = 0u64;
    for i in 0..z.len() {
        let mut e = z.by_index(i)?;
        let name = e.name().to_string();
        let p = safe_path(&name)?;
        if e.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            bail!("Archive symlinks rejected");
        }
        if e.is_dir() {
            std::fs::create_dir_all(target.join(p))?;
            continue;
        }
        if name.to_ascii_lowercase().ends_with("/ffplay.exe") {
            continue;
        }
        let size = e.size();
        total = total.checked_add(size).context("Size overflow")?;
        if total > 1024 * 1024 * 1024 || size > 300 * 1024 * 1024 {
            bail!("Unpacked size cap exceeded");
        }
        let p = target.join(p);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(p)?;
        if std::io::copy(&mut (&mut e).take(size + 1), &mut f)? != size {
            bail!("Archive size mismatch");
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn traversal() {
        for s in ["../x", "/x", "C:/x", "a\\b", "a/../../b"] {
            assert!(safe_path(s).is_err());
        }
        assert!(safe_path("ffmpeg/bin/ffmpeg.exe").is_ok());
    }
    #[test]
    fn redirects() {
        assert!(host("release-assets.githubusercontent.com"));
        assert!(!host("github.com.evil.test"));
    }
    #[test]
    fn hashes() {
        for a in &ASSETS {
            assert_eq!(a.hash.len(), 64);
            assert!(a.hash.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }
}

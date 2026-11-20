//! Thumbnails of show art. Some providers (Plex) link artwork at full size,
//! often several megabytes, for a thumbnail 80 pixels tall. Conflux downloads
//! each image once, shrinks it, and serves the copy from `/api/art/<key>`:
//! about 12 KB each instead of megabytes on a phone.
//!
//! Thumbnails are made ahead of time for what's on now and next (`prepare`,
//! after each guide refresh), so nobody waits for a download, and kept on
//! disk (`CacheDirectory=` in the systemd unit, or `CONFLUX_ART_DIR`), so a
//! restart or deploy doesn't fetch them all again. Browsers keep each one for
//! a week.
//!
//! Only images that Conflux itself lists in a guide can be fetched (a request
//! names a key, never a URL), and only over https from a named host, so a
//! guide entry can't point the server at itself or its network.

use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};
use axum::body::Bytes;
use futures::StreamExt;
use image::imageops::FilterType;
use image::{DynamicImage, ImageReader, Limits};
use reqwest::{Client, redirect};
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::model::Program;
use crate::providers::USER_AGENT;

/// The guide's 56×80 thumbnail at 3× pixel density.
pub const WIDTH: u32 = 168;
pub const HEIGHT: u32 = 240;
const MAX_DOWNLOAD: usize = 20 << 20;
/// In memory: room for about 5,000 thumbnails.
const CACHE_BYTES: usize = 64 << 20;
/// On disk, thumbnails are deleted this long after they're made (and made
/// again if they're still needed).
const KEEP_FOR: Duration = Duration::from_secs(30 * 24 * 3600);
const PRUNE_EVERY: Duration = Duration::from_secs(24 * 3600);
/// A failed image isn't tried again for this long; the app shows the
/// channel logo meanwhile.
const RETRY_AFTER: Duration = Duration::from_secs(3600);
/// Downloads and resizes at once.
const AT_ONCE: usize = 4;
/// Of those, how many making thumbnails ahead of time may use, so requests
/// from the app still get through.
const AHEAD_AT_ONCE: usize = 2;
/// Thumbnails made ahead of time per channel: what's on now and next.
const AHEAD_PER_CHANNEL: usize = 2;

pub struct Art {
    http: Client,
    /// Where thumbnails are kept between restarts, if anywhere.
    dir: Option<PathBuf>,
    /// provider id -> key -> original URL, from that provider's last guide.
    sources: RwLock<HashMap<String, HashMap<u64, String>>>,
    cache: Mutex<Cache>,
    gate: Semaphore,
    /// Keys waiting to be made ahead of time, and whether a run is going.
    queued: Mutex<Vec<u64>>,
    preparing: AtomicBool,
    pruned_at: Mutex<Option<Instant>>,
}

#[derive(Default)]
struct Cache {
    done: HashMap<u64, Bytes>,
    /// Oldest first, for eviction.
    order: VecDeque<u64>,
    bytes: usize,
    failed: HashMap<u64, Instant>,
}

impl Cache {
    fn put(&mut self, key: u64, b: Bytes) {
        self.bytes += b.len();
        match self.done.insert(key, b) {
            Some(old) => self.bytes -= old.len(),
            None => self.order.push_back(key),
        }
        while self.bytes > CACHE_BYTES {
            let Some(k) = self.order.pop_front() else { break };
            if let Some(old) = self.done.remove(&k) {
                self.bytes -= old.len();
            }
        }
    }

    fn fail(&mut self, key: u64) {
        self.failed.retain(|_, at| at.elapsed() < RETRY_AFTER);
        self.failed.insert(key, Instant::now());
    }
}

enum Lookup {
    Hit(Bytes),
    Failed,
    Miss,
}

impl Art {
    /// Thumbnails in memory only, or also kept in `dir`.
    pub fn new(dir: Option<PathBuf>) -> Art {
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .redirect(redirect::Policy::custom(|a| {
                if a.previous().len() < 5 && fetchable(a.url().as_str()) {
                    a.follow()
                } else {
                    a.stop()
                }
            }))
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(8))
            .build()
            .expect("http client");
        let dir = dir.filter(|d| match writable(d) {
            Ok(()) => {
                tracing::info!("art: thumbnails kept in {}", d.display());
                true
            }
            Err(e) => {
                tracing::warn!("art: can't keep thumbnails in {}, memory only: {e:#}", d.display());
                false
            }
        });
        Art {
            http,
            dir,
            sources: RwLock::new(HashMap::new()),
            cache: Mutex::new(Cache::default()),
            gate: Semaphore::new(AT_ONCE),
            queued: Mutex::new(Vec::new()),
            preparing: AtomicBool::new(false),
            pruned_at: Mutex::new(None),
        }
    }

    /// `CONFLUX_ART_DIR`, else `art` in the cache directory systemd gives the
    /// service (`CacheDirectory=`), else memory only.
    pub fn from_env() -> Art {
        let dir = std::env::var_os("CONFLUX_ART_DIR")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                let dirs = std::env::var("CACHE_DIRECTORY").ok()?;
                let first = dirs.split(':').next().filter(|d| !d.is_empty())?;
                Some(Path::new(first).join("art"))
            });
        Art::new(dir)
    }

    /// Point each program's image at a shrunken copy, replacing what this
    /// provider's last guide listed. Images that can't be fetched safely are
    /// dropped, so the app shows the channel logo instead. Returns the keys
    /// worth making now: each channel's current and next shows.
    pub async fn adopt(&self, provider: &str, guide: &mut HashMap<String, Vec<Program>>) -> Vec<u64> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut urls = HashMap::new();
        let mut soon = Vec::new();
        for programs in guide.values_mut() {
            let mut upcoming = 0;
            for p in programs.iter_mut() {
                let Some(url) = p.image.take() else { continue };
                if !fetchable(&url) {
                    continue;
                }
                let key = key_of(&url);
                p.image = Some(path_of(key));
                urls.insert(key, url);
                if p.stop > now && upcoming < AHEAD_PER_CHANNEL {
                    upcoming += 1;
                    soon.push(key);
                }
            }
        }
        self.sources.write().await.insert(provider.to_string(), urls);
        soon.sort_unstable();
        soon.dedup();
        soon
    }

    /// Make thumbnails for `keys` ahead of time, a few at once, so the art is
    /// ready before anyone opens the guide. Providers refresh their guides at
    /// the same moment, so if a run is already going the keys are queued for
    /// it rather than starting a second one.
    pub async fn prepare(self: Arc<Self>, keys: Vec<u64>) {
        self.queued.lock().await.extend(keys);
        loop {
            if self.preparing.swap(true, Ordering::AcqRel) {
                return; // the run that's going takes these too
            }
            self.prune().await;
            loop {
                let batch = {
                    let mut q = self.queued.lock().await;
                    q.sort_unstable();
                    q.dedup();
                    std::mem::take(&mut *q)
                };
                if batch.is_empty() {
                    break;
                }
                self.make_ahead(batch).await;
            }
            self.preparing.store(false, Ordering::Release);
            // Keys queued after the last batch was taken but before the run
            // ended would otherwise wait for the next refresh.
            if self.queued.lock().await.is_empty() {
                return;
            }
        }
    }

    async fn make_ahead(self: &Arc<Self>, keys: Vec<u64>) {
        let started = Instant::now();
        let total = keys.len();
        let ready = futures::stream::iter(keys)
            .map(|k| {
                let me = Arc::clone(self);
                async move { me.thumb(k).await.is_some() }
            })
            .buffer_unordered(AHEAD_AT_ONCE)
            .filter(|ok| std::future::ready(*ok))
            .count()
            .await;
        tracing::info!(
            "art: {ready} of {total} thumbnails for now and next ready ({:.0?})",
            started.elapsed()
        );
    }

    /// The shrunken image for `key` (16 hex digits), made on first request.
    /// None if the key isn't in a current guide or the image is unusable.
    pub async fn get(&self, key: &str) -> Option<Bytes> {
        self.thumb(parse_key(key)?).await
    }

    async fn thumb(&self, key: u64) -> Option<Bytes> {
        match self.lookup(key).await {
            Lookup::Hit(b) => return Some(b),
            Lookup::Failed => return None,
            Lookup::Miss => {}
        }
        // On disk from before a restart; served even before the first guide
        // refresh lists it again.
        if let Some(b) = self.read_disk(key).await {
            self.cache.lock().await.put(key, b.clone());
            return Some(b);
        }
        let url = self.sources.read().await.values().find_map(|m| m.get(&key).cloned())?;
        let _permit = self.gate.acquire().await.ok()?;
        // Another request may have made it while this one waited.
        if let Lookup::Hit(b) = self.lookup(key).await {
            return Some(b);
        }
        match self.make(&url).await {
            Ok(jpeg) => {
                let b = Bytes::from(jpeg);
                self.keep(key, b.clone()).await;
                Some(b)
            }
            Err(e) => {
                tracing::debug!("art: {url}: {e:#}");
                self.cache.lock().await.fail(key);
                None
            }
        }
    }

    async fn lookup(&self, key: u64) -> Lookup {
        let c = self.cache.lock().await;
        if let Some(b) = c.done.get(&key) {
            Lookup::Hit(b.clone())
        } else if c.failed.get(&key).is_some_and(|at| at.elapsed() < RETRY_AFTER) {
            Lookup::Failed
        } else {
            Lookup::Miss
        }
    }

    /// In memory, and on disk if there is one.
    async fn keep(&self, key: u64, b: Bytes) {
        if let Some(path) = self.path(key) {
            let tmp = path.with_extension("tmp");
            let write = async {
                tokio::fs::write(&tmp, &b).await?;
                tokio::fs::rename(&tmp, &path).await
            };
            if let Err(e) = write.await {
                tracing::warn!("art: couldn't save {}: {e}", path.display());
            }
        }
        self.cache.lock().await.put(key, b);
    }

    async fn read_disk(&self, key: u64) -> Option<Bytes> {
        let b = tokio::fs::read(self.path(key)?).await.ok()?;
        (!b.is_empty()).then(|| Bytes::from(b))
    }

    fn path(&self, key: u64) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("{key:016x}.jpg")))
    }

    /// Delete thumbnails on disk older than `KEEP_FOR`, at most once a day.
    async fn prune(&self) {
        let Some(dir) = self.dir.clone() else { return };
        {
            let mut at = self.pruned_at.lock().await;
            if at.is_some_and(|t| t.elapsed() < PRUNE_EVERY) {
                return;
            }
            *at = Some(Instant::now());
        }
        match tokio::task::spawn_blocking(move || prune(&dir, KEEP_FOR)).await {
            Ok(Ok(0)) => {}
            Ok(Ok(n)) => tracing::info!("art: deleted {n} old thumbnails"),
            Ok(Err(e)) => tracing::warn!("art: couldn't clear old thumbnails: {e:#}"),
            Err(e) => tracing::warn!("art: couldn't clear old thumbnails: {e}"),
        }
    }

    async fn make(&self, url: &str) -> Result<Vec<u8>> {
        let mut resp = self.http.get(url).send().await?.error_for_status()?;
        if resp.content_length().is_some_and(|n| n > MAX_DOWNLOAD as u64) {
            bail!("over {} MB", MAX_DOWNLOAD >> 20);
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_DOWNLOAD {
                bail!("over {} MB", MAX_DOWNLOAD >> 20);
            }
        }
        tokio::task::spawn_blocking(move || shrink(&body)).await?
    }
}

fn writable(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(".probe");
    std::fs::write(&probe, b"ok")?;
    std::fs::remove_file(&probe)?;
    Ok(())
}

/// Remove thumbnails (and half-written files) in `dir` last written more
/// than `age` ago. Returns how many.
fn prune(dir: &Path, age: Duration) -> Result<usize> {
    let mut n = 0;
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let ours = path.extension().is_some_and(|e| e == "jpg" || e == "tmp");
        let old = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|e| e > age));
        if ours && old && std::fs::remove_file(&path).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

/// https from a named host (no IP addresses, no localhost) on the default
/// port.
pub fn fetchable(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    u.scheme() == "https"
        && u.port().is_none()
        && u.username().is_empty()
        && u.password().is_none()
        && u.domain().is_some_and(|d| {
            let d = d.trim_end_matches('.');
            d.contains('.') && d != "localhost" && !d.ends_with(".localhost")
        })
}

/// How `shrink` draws thumbnails. Part of every key, so changing it gives
/// thumbnails new addresses and browsers and the disk cache drop the old ones.
const STYLE: &str = "fit-wide-over-blur-1";

/// FNV-1a of `STYLE` and the URL: stable across restarts and Rust versions,
/// so browsers can keep their cached copies.
pub fn key_of(url: &str) -> u64 {
    STYLE
        .bytes()
        .chain(*b"\n")
        .chain(url.bytes())
        .fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

pub fn path_of(key: u64) -> String {
    format!("/api/art/{key:016x}")
}

fn parse_key(s: &str) -> Option<u64> {
    if s.len() != 16 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(s, 16).ok()
}

/// Decode an image and make a WIDTH×HEIGHT JPEG of it. Tall images are
/// cropped to fill, as the guide's `object-fit: cover` would. Wide ones (most
/// of Roku's art is 16:9) would lose their sides and titles that way, so the
/// whole image is shown across the middle, over a blurred, darkened fill of
/// itself.
pub fn shrink(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_000);
    limits.max_image_height = Some(16_000);
    limits.max_alloc = Some(512 << 20);
    reader.limits(limits);
    let img = reader.decode()?;
    let (w, h) = (img.width().max(1), img.height().max(1));
    let img = if w > h {
        let fit_h = ((f64::from(h) * f64::from(WIDTH) / f64::from(w)).round() as u32).clamp(1, HEIGHT);
        let front = scale(&img, WIDTH, fit_h).to_rgb8();
        // Blurring the small front image is cheap; the fill only needs colour.
        let mut back = cover(&DynamicImage::ImageRgb8(front.clone()), WIDTH, HEIGHT).to_rgb8();
        back = image::imageops::fast_blur(&back, 12.0);
        for px in back.pixels_mut() {
            px.0 = px.0.map(|c| (u16::from(c) * 3 / 5) as u8);
        }
        image::imageops::overlay(&mut back, &front, 0, i64::from((HEIGHT - fit_h) / 2));
        back
    } else {
        cover(&img, WIDTH, HEIGHT).to_rgb8()
    };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode_image(&img)?;
    Ok(out)
}

/// `img` scaled to fill `w`×`h` and cropped to it, centred.
fn cover(img: &DynamicImage, w: u32, h: u32) -> DynamicImage {
    let (iw, ih) = (img.width().max(1), img.height().max(1));
    let k = (f64::from(w) / f64::from(iw)).max(f64::from(h) / f64::from(ih));
    let sw = ((f64::from(iw) * k).round() as u32).max(w);
    let sh = ((f64::from(ih) * k).round() as u32).max(h);
    scale(img, sw, sh).crop_imm((sw - w) / 2, (sh - h) / 2, w, h)
}

/// Shrinking uses the fast thumbnail filter; growing smooths.
fn scale(img: &DynamicImage, w: u32, h: u32) -> DynamicImage {
    if w < img.width() || h < img.height() {
        img.thumbnail_exact(w, h)
    } else {
        img.resize_exact(w, h, FilterType::Triangle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(image: &str) -> Program {
        at(u64::MAX - 1, image)
    }

    fn at(start: u64, image: &str) -> Program {
        Program {
            start,
            stop: start.saturating_add(1),
            title: "Show".into(),
            subtitle: None,
            description: None,
            rating: None,
            image: Some(image.into()),
        }
    }

    #[test]
    fn only_https_from_named_hosts() {
        assert!(fetchable("https://metadata-static.plex.tv/a/gracenote/x.jpg"));
        assert!(!fetchable("http://metadata-static.plex.tv/x.jpg"));
        assert!(!fetchable("https://169.254.169.254/latest"));
        assert!(!fetchable("https://[::1]/x.jpg"));
        assert!(!fetchable("https://localhost/x.jpg"));
        assert!(!fetchable("https://a.localhost./x.jpg"));
        assert!(!fetchable("https://intranet/x.jpg"));
        assert!(!fetchable("https://example.com:8443/x.jpg"));
        assert!(!fetchable("https://user@example.com/x.jpg"));
        assert!(!fetchable("javascript:alert(1)"));
    }

    #[test]
    fn keys_are_stable_hex() {
        let k = key_of("https://example.com/a.jpg");
        assert_eq!(key_of("https://example.com/a.jpg"), k);
        assert_ne!(key_of("https://example.com/b.jpg"), k);
        let path = path_of(k);
        assert_eq!(parse_key(path.strip_prefix("/api/art/").unwrap()), Some(k));
        assert_eq!(parse_key("+123456789abcdef"), None);
        assert_eq!(parse_key("../../etc/passwd"), None);
    }

    #[tokio::test]
    async fn adopt_rewrites_and_drops_unsafe_images() {
        let art = Art::new(None);
        let mut guide = HashMap::from([(
            "plex:live:1".to_string(),
            vec![program("https://example.com/a.jpg"), program("http://10.0.0.1/a.jpg")],
        )]);
        let soon = art.adopt("plex", &mut guide).await;
        assert_eq!(soon, [key_of("https://example.com/a.jpg")]);
        let ps = &guide["plex:live:1"];
        assert_eq!(ps[0].image.as_deref(), Some(path_of(key_of("https://example.com/a.jpg")).as_str()));
        assert_eq!(ps[1].image, None);
        assert_eq!(art.get("0000000000000000").await, None, "not in any guide");
        assert_eq!(art.get("nonsense").await, None);
    }

    #[test]
    fn shrinks_to_fill_the_thumbnail() {
        for (w, h) in [(2000, 3000), (1920, 1080), (40, 30)] {
            let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
            let mut src = Vec::new();
            image::DynamicImage::ImageRgb8(img)
                .write_to(&mut Cursor::new(&mut src), image::ImageFormat::Png)
                .unwrap();
            let out = shrink(&src).unwrap();
            let thumb = image::load_from_memory(&out).unwrap();
            assert_eq!((thumb.width(), thumb.height()), (WIDTH, HEIGHT), "{w}x{h}");
            assert!(out.len() < 40_000, "{w}x{h}: {} bytes", out.len());
        }
        assert!(shrink(b"<html>not an image</html>").is_err());
    }

    #[test]
    fn wide_images_are_shown_whole_over_a_blur() {
        // Blue, with a green stripe down the left edge that a crop would cut off.
        let img = image::RgbImage::from_fn(1920, 1080, |x, _| {
            if x < 96 { image::Rgb([0, 200, 0]) } else { image::Rgb([0, 0, 200]) }
        });
        let mut src = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut src), image::ImageFormat::Png)
            .unwrap();
        let thumb = image::load_from_memory(&shrink(&src).unwrap()).unwrap().to_rgb8();
        assert_eq!(thumb.dimensions(), (WIDTH, HEIGHT));
        let [r, g, b] = thumb.get_pixel(2, HEIGHT / 2).0;
        assert!(g > 150 && r < 60 && b < 60, "left edge of the middle: {r},{g},{b}");
        let [_, _, b] = thumb.get_pixel(WIDTH - 3, HEIGHT / 2).0;
        assert!(b > 150, "right edge of the middle is blue");
        // Above and below the picture is the darker fill.
        let [r, g, b] = thumb.get_pixel(WIDTH / 2, 4).0;
        assert!(r.max(g).max(b) < 150, "fill is darkened: {r},{g},{b}");
    }

    #[tokio::test]
    async fn makes_now_and_next_ahead_of_time() {
        let art = Art::new(None);
        let url = |n: u32| format!("https://example.com/{n}.jpg");
        let mut guide = HashMap::from([(
            "plex:live:1".to_string(),
            vec![at(0, &url(0)), at(u64::MAX - 9, &url(1)), at(u64::MAX - 7, &url(2)), at(u64::MAX - 5, &url(3))],
        )]);
        let soon = art.adopt("plex", &mut guide).await;
        let mut want = vec![key_of(&url(1)), key_of(&url(2))];
        want.sort_unstable();
        assert_eq!(soon, want, "not the past one, not the third");
    }

    #[tokio::test]
    async fn a_second_provider_is_queued_not_skipped() {
        let dir = scratch("queue");
        let art = Arc::new(Art::new(Some(dir.clone())));
        let (a, b) = (key_of("https://example.com/a.jpg"), key_of("https://example.com/b.jpg"));
        // Saved thumbnails, so making them is just a read from disk.
        art.keep(a, Bytes::from_static(b"a")).await;
        art.keep(b, Bytes::from_static(b"b")).await;
        art.cache.lock().await.done.clear();
        art.preparing.store(true, Ordering::Release); // as if Plex's run were going
        Arc::clone(&art).prepare(vec![b]).await; // Roku's keys wait in the queue
        assert_eq!(*art.queued.lock().await, [b]);
        art.preparing.store(false, Ordering::Release);
        Arc::clone(&art).prepare(vec![a]).await;
        let c = art.cache.lock().await;
        assert!(c.done.contains_key(&a) && c.done.contains_key(&b), "both made");
        drop(c);
        assert!(art.queued.lock().await.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("conflux-art-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[tokio::test]
    async fn thumbnails_outlive_a_restart() {
        let dir = scratch("restart");
        let key = key_of("https://example.com/a.jpg");
        Art::new(Some(dir.clone())).keep(key, Bytes::from_static(b"jpeg")).await;
        let after = Art::new(Some(dir.clone()));
        assert_eq!(after.get(&format!("{key:016x}")).await.as_deref(), Some(&b"jpeg"[..]), "not in any guide yet, but saved");
        assert!(!dir.join(format!("{key:016x}.tmp")).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_thumbnails_are_deleted() {
        let dir = scratch("prune");
        std::fs::create_dir_all(&dir).unwrap();
        let old = SystemTime::now() - Duration::from_secs(40 * 24 * 3600);
        for name in ["old.jpg", "old.tmp", "old.txt"] {
            let f = std::fs::File::create(dir.join(name)).unwrap();
            f.set_modified(old).unwrap();
        }
        std::fs::write(dir.join("new.jpg"), b"x").unwrap();
        assert_eq!(prune(&dir, KEEP_FOR).unwrap(), 2);
        let mut left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        left.sort();
        assert_eq!(left, ["new.jpg", "old.txt"], "only our own old files go");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_dir_that_cant_be_written_means_memory_only() {
        let file = scratch("file");
        std::fs::write(&file, b"not a dir").unwrap();
        assert!(Art::new(Some(file.join("art"))).dir.is_none());
        std::fs::remove_file(&file).unwrap();
    }
}

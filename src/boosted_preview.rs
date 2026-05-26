use anyhow::{Context, Result, bail};
use image::AnimationDecoder;
use image::codecs::gif::GifDecoder;
use image::imageops::FilterType;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::constants::HTTP_REQUEST_TIMEOUT;

static PREVIEW_CLIENT: OnceLock<Client> = OnceLock::new();
const MAX_PREVIEW_FRAMES: usize = 16;
const BOOSTED_PREVIEW_MAX_DIMENSION: u32 = 96;
const MIN_PREVIEW_MAX_DIMENSION: u32 = 16;
const MAX_PREVIEW_MAX_DIMENSION: u32 = 192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoostedPreviewKind {
    Creature,
    Boss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoostedPreviewLoadPhase {
    StaticPlaceholder,
    Animated,
}

#[derive(Debug, Clone)]
pub struct BoostedPreviewFrame {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

#[derive(Debug, Clone)]
pub struct BoostedPreviewData {
    pub url: String,
    pub frames: Vec<BoostedPreviewFrame>,
}

pub async fn fetch_boosted_preview_cached(
    url: String,
    cache_dir: PathBuf,
) -> Result<BoostedPreviewData> {
    fetch_preview_cached(
        url,
        MAX_PREVIEW_FRAMES,
        BOOSTED_PREVIEW_MAX_DIMENSION,
        cache_dir,
    )
    .await
}

pub async fn fetch_boosted_preview_cached_as(
    fetch_url: String,
    display_url: String,
    cache_dir: PathBuf,
) -> Result<BoostedPreviewData> {
    let mut preview = fetch_boosted_preview_cached(fetch_url, cache_dir).await?;
    preview.url = display_url;
    Ok(preview)
}

pub async fn fetch_static_preview_cached(
    url: String,
    max_dimension: u32,
    cache_dir: PathBuf,
) -> Result<BoostedPreviewData> {
    fetch_preview_cached(url, 1, max_dimension, cache_dir).await
}

pub async fn fetch_static_preview_cached_as(
    fetch_url: String,
    display_url: String,
    max_dimension: u32,
    cache_dir: PathBuf,
) -> Result<BoostedPreviewData> {
    let mut preview = fetch_static_preview_cached(fetch_url, max_dimension, cache_dir).await?;
    preview.url = display_url;
    Ok(preview)
}

pub async fn fetch_boosted_static_preview_cached_as(
    fetch_url: String,
    display_url: String,
    cache_dir: PathBuf,
) -> Result<BoostedPreviewData> {
    fetch_static_preview_cached_as(
        fetch_url,
        display_url,
        BOOSTED_PREVIEW_MAX_DIMENSION,
        cache_dir,
    )
    .await
}

async fn fetch_preview_cached(
    url: String,
    max_frames: usize,
    max_dimension: u32,
    cache_dir: PathBuf,
) -> Result<BoostedPreviewData> {
    let cache_key = preview_cache_key(&url, max_frames, max_dimension);
    let cache_path = cache_dir.join(&cache_key);
    let cached_url = url.clone();
    if let Ok(Some(frames)) =
        tokio::task::spawn_blocking(move || load_cached_preview(&cache_path)).await?
    {
        return Ok(BoostedPreviewData {
            url: cached_url,
            frames,
        });
    }

    let preview = fetch_preview(url, max_frames, max_dimension).await?;
    let cache_path = cache_dir.join(preview_cache_key(&preview.url, max_frames, max_dimension));
    let frames_to_cache = preview.frames.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let _ = save_cached_preview(&cache_path, &frames_to_cache);
    });

    Ok(preview)
}

async fn fetch_preview(
    url: String,
    max_frames: usize,
    max_dimension: u32,
) -> Result<BoostedPreviewData> {
    let client = PREVIEW_CLIENT
        .get_or_init(|| {
            Client::builder()
                .timeout(HTTP_REQUEST_TIMEOUT)
                .user_agent("PenultimaLauncher/boosted-preview")
                .build()
                .expect("preview HTTP client should build")
        })
        .clone();

    let bytes = client
        .get(&url)
        .send()
        .await
        .context("failed to fetch boosted preview")?
        .error_for_status()
        .context("boosted preview returned an error")?
        .bytes()
        .await
        .context("failed to read boosted preview bytes")?;

    let frames = tokio::task::spawn_blocking(move || {
        decode_preview_frames(&bytes, max_frames, max_dimension)
    })
    .await
    .context("preview decode task failed")??;
    Ok(BoostedPreviewData { url, frames })
}

fn decode_preview_frames(
    bytes: &[u8],
    max_frames: usize,
    max_dimension: u32,
) -> Result<Vec<BoostedPreviewFrame>> {
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return decode_gif_frames(bytes, max_frames, max_dimension);
    }

    let image = resize_preview_image(
        image::load_from_memory(bytes)
            .context("boosted preview is not a supported image format")?
            .into_rgba8(),
        max_dimension,
    );
    let (width, height) = image.dimensions();

    Ok(vec![BoostedPreviewFrame {
        size: [width as usize, height as usize],
        rgba: image.into_raw(),
        delay_ms: 220,
    }])
}

fn decode_gif_frames(
    bytes: &[u8],
    max_frames: usize,
    max_dimension: u32,
) -> Result<Vec<BoostedPreviewFrame>> {
    let cursor = Cursor::new(bytes.to_vec());
    let decoder =
        GifDecoder::new(BufReader::new(cursor)).context("failed to decode boosted GIF")?;
    let frame_limit = max_frames.clamp(1, MAX_PREVIEW_FRAMES);
    let mut decoded = Vec::with_capacity(frame_limit);

    for frame in decoder.into_frames() {
        let frame = frame.context("failed to decode boosted GIF frame")?;
        let delay = frame.delay();
        let (numerator, denominator) = delay.numer_denom_ms();
        let delay_ms = if denominator == 0 {
            120
        } else {
            (numerator / denominator).clamp(80, 600)
        };
        let image = resize_preview_image(frame.into_buffer(), max_dimension);
        let (width, height) = image.dimensions();

        decoded.push(BoostedPreviewFrame {
            size: [width as usize, height as usize],
            rgba: image.into_raw(),
            delay_ms,
        });

        if decoded.len() >= frame_limit {
            break;
        }
    }

    if decoded.is_empty() {
        bail!("boosted GIF had no frames");
    }

    Ok(decoded)
}

fn resize_preview_image(image: image::RgbaImage, max_dimension: u32) -> image::RgbaImage {
    let max_dimension = max_dimension.clamp(MIN_PREVIEW_MAX_DIMENSION, MAX_PREVIEW_MAX_DIMENSION);
    let (width, height) = image.dimensions();
    let longest = width.max(height);
    if longest <= max_dimension {
        return image;
    }

    let scale = max_dimension as f32 / longest as f32;
    let resized_width = ((width as f32 * scale).round() as u32).max(1);
    let resized_height = ((height as f32 * scale).round() as u32).max(1);
    image::imageops::resize(&image, resized_width, resized_height, FilterType::Nearest)
}

fn preview_cache_key(url: &str, max_frames: usize, max_dimension: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    hasher.update([0]);
    hasher.update(max_frames.to_le_bytes());
    hasher.update(max_dimension.to_le_bytes());
    let hash = hasher.finalize();
    format!("{:x}", hash)
}

fn load_cached_preview(path: &Path) -> Result<Option<Vec<BoostedPreviewFrame>>> {
    let manifest_path = path.join("manifest.json");
    if !manifest_path.exists() {
        return Ok(None);
    }

    let manifest_raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read cached preview {}", manifest_path.display()))?;
    let manifest: CachedPreviewManifest = serde_json::from_str(&manifest_raw)
        .with_context(|| format!("failed to parse cached preview {}", manifest_path.display()))?;

    let mut frames = Vec::with_capacity(manifest.frames.len());
    for frame in manifest.frames {
        let frame_path = path.join(&frame.file);
        let image = image::open(&frame_path)
            .with_context(|| format!("failed to load cached preview {}", frame_path.display()))?
            .into_rgba8();
        let (width, height) = image.dimensions();
        frames.push(BoostedPreviewFrame {
            size: [width as usize, height as usize],
            rgba: image.into_raw(),
            delay_ms: frame.delay_ms,
        });
    }

    Ok((!frames.is_empty()).then_some(frames))
}

fn save_cached_preview(path: &Path, frames: &[BoostedPreviewFrame]) -> Result<()> {
    if frames.is_empty() {
        return Ok(());
    }

    fs::create_dir_all(path)?;
    let mut manifest = CachedPreviewManifest { frames: Vec::new() };

    for (index, frame) in frames.iter().enumerate() {
        let file = format!("frame-{index:02}.png");
        let frame_path = path.join(&file);
        image::save_buffer_with_format(
            &frame_path,
            &frame.rgba,
            frame.size[0] as u32,
            frame.size[1] as u32,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .with_context(|| format!("failed to save cached preview {}", frame_path.display()))?;

        manifest.frames.push(CachedPreviewFrame {
            file,
            delay_ms: frame.delay_ms,
        });
    }

    fs::write(
        path.join("manifest.json"),
        serde_json::to_string(&manifest)?,
    )?;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedPreviewManifest {
    frames: Vec<CachedPreviewFrame>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedPreviewFrame {
    file: String,
    delay_ms: u32,
}

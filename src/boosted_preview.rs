use anyhow::{Context, Result, bail};
use image::AnimationDecoder;
use image::codecs::gif::GifDecoder;
use reqwest::Client;
use std::io::{BufReader, Cursor};
use std::sync::OnceLock;

use crate::constants::HTTP_REQUEST_TIMEOUT;

static PREVIEW_CLIENT: OnceLock<Client> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoostedPreviewKind {
    Creature,
    Boss,
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

pub async fn fetch_boosted_preview(url: String) -> Result<BoostedPreviewData> {
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

    let frames = decode_preview_frames(&bytes)?;
    Ok(BoostedPreviewData { url, frames })
}

fn decode_preview_frames(bytes: &[u8]) -> Result<Vec<BoostedPreviewFrame>> {
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return decode_gif_frames(bytes);
    }

    let image = image::load_from_memory(bytes)
        .context("boosted preview is not a supported image format")?
        .into_rgba8();
    let (width, height) = image.dimensions();

    Ok(vec![BoostedPreviewFrame {
        size: [width as usize, height as usize],
        rgba: image.into_raw(),
        delay_ms: 220,
    }])
}

fn decode_gif_frames(bytes: &[u8]) -> Result<Vec<BoostedPreviewFrame>> {
    let cursor = Cursor::new(bytes.to_vec());
    let decoder =
        GifDecoder::new(BufReader::new(cursor)).context("failed to decode boosted GIF")?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .context("failed to collect boosted GIF frames")?;

    let decoded: Vec<BoostedPreviewFrame> = frames
        .into_iter()
        .map(|frame| {
            let delay = frame.delay();
            let (numerator, denominator) = delay.numer_denom_ms();
            let delay_ms = if denominator == 0 {
                120
            } else {
                (numerator / denominator).clamp(60, 600)
            };
            let image = frame.into_buffer();
            let (width, height) = image.dimensions();

            BoostedPreviewFrame {
                size: [width as usize, height as usize],
                rgba: image.into_raw(),
                delay_ms,
            }
        })
        .collect();

    if decoded.is_empty() {
        bail!("boosted GIF had no frames");
    }

    Ok(decoded)
}

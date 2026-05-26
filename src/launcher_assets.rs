use anyhow::{Context, Result};
use reqwest::Client;
use std::fs;
use std::path::{Path, PathBuf};

use crate::constants::{HTTP_REQUEST_TIMEOUT, LAUNCHER_ASSET_BASE_URL};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LauncherAssetKind {
    Background,
    Logo,
    SplashLogo,
}

impl LauncherAssetKind {
    pub fn filename(self) -> &'static str {
        match self {
            Self::Background => "background.jpg",
            Self::Logo => "logo.png",
            Self::SplashLogo => "splash-logo.png",
        }
    }

    pub fn texture_name(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Logo => "logo",
            Self::SplashLogo => "splash-logo",
        }
    }

    pub fn max_dimension(self) -> u32 {
        match self {
            Self::Background => 1024,
            Self::Logo => 360,
            Self::SplashLogo => 512,
        }
    }

    fn url(self) -> String {
        format!(
            "{}/{}",
            LAUNCHER_ASSET_BASE_URL.trim_end_matches('/'),
            self.filename()
        )
    }
}

pub const LAUNCHER_ASSETS: [LauncherAssetKind; 3] = [
    LauncherAssetKind::Background,
    LauncherAssetKind::Logo,
    LauncherAssetKind::SplashLogo,
];

pub fn asset_cache_dir(state_path: &Path) -> PathBuf {
    state_path.join("launcher-assets")
}

pub fn asset_cache_path(state_path: &Path, kind: LauncherAssetKind) -> PathBuf {
    asset_cache_dir(state_path).join(kind.filename())
}

pub fn load_cached_asset(state_path: &Path, kind: LauncherAssetKind) -> Option<Vec<u8>> {
    fs::read(asset_cache_path(state_path, kind)).ok()
}

pub async fn fetch_and_cache_asset(
    client: Client,
    state_path: PathBuf,
    kind: LauncherAssetKind,
) -> Result<Vec<u8>> {
    let bytes = client
        .get(kind.url())
        .send()
        .await
        .context("failed to fetch launcher asset")?
        .error_for_status()
        .context("launcher asset returned an error")?
        .bytes()
        .await
        .context("failed to read launcher asset bytes")?
        .to_vec();

    let cache_path = asset_cache_path(&state_path, kind);
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&cache_path, &bytes)
        .with_context(|| format!("failed to write {}", cache_path.display()))?;

    Ok(bytes)
}

pub fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(HTTP_REQUEST_TIMEOUT)
        .user_agent("PenultimaLauncher/assets")
        .build()
        .context("failed to build launcher asset client")
}

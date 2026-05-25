// Mensagens que podem ser enviadas ao launcher
use crate::boosted_preview::{BoostedPreviewData, BoostedPreviewKind};
use crate::website_status::WebsiteStatus;
use std::path::PathBuf;

#[derive(Debug)]
pub enum LauncherMessage {
    LaunchGame,
    LaunchOtClient(PathBuf),
    #[allow(dead_code)]
    CheckForUpdates,
    UpdateAvailable(String),
    DownloadComplete,
    DownloadProgress(f32),
    VersionUpdated(String),
    ClientVersionUpdated(String),
    SetStatus(String),
    SetProcessing(bool),
    Error(String),
    SetTempMessage(String),
    PingResult(Option<u32>), // Resultado do ping do servidor
    WebsiteStatusLoaded(WebsiteStatus),
    WebsiteStatusError(String),
    BoostedPreviewLoaded(BoostedPreviewKind, BoostedPreviewData),
    BoostedPreviewError(BoostedPreviewKind, String, String),
    OfferPreviewLoaded(BoostedPreviewData),
    OfferPreviewError(String, String),
    RestartLauncherForUpdate,
}

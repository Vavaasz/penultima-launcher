// Mensagens que podem ser enviadas ao launcher
#[derive(Debug)]
pub enum LauncherMessage {
    LaunchGame,
    #[allow(dead_code)]
    CheckForUpdates,
    UpdateAvailable(String),
    DownloadComplete,
    DownloadProgress(f32),
    VersionUpdated(String),
    SetStatus(String),
    SetProcessing(bool),
    Error(String),
    SetTempMessage(String),
}

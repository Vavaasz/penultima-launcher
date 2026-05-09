// Mensagens que podem ser enviadas ao launcher
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
    RestartLauncherForUpdate,
}

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use log::info;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use zip::ZipArchive;

use crate::constants::{
    DOWNLOADS_METADATA_URL, HTTP_DOWNLOAD_TIMEOUT, HTTP_REQUEST_TIMEOUT, WEBSITE_BASE_URL,
};
use crate::message_system::LauncherMessage;

const STATUS_REPORT_INTERVAL: Duration = Duration::from_millis(250);
const LAUNCHER_ZIP_FILE_NAME: &str = "Penultima-Launcher.zip";

#[derive(Debug, Deserialize)]
struct DownloadsMetadata {
    launcher: Option<LauncherRelease>,
}

#[derive(Debug, Deserialize)]
struct LauncherRelease {
    version: Option<String>,
    zip: String,
    sha256: String,
    size: Option<u64>,
    exe_sha256: Option<String>,
}

pub struct LauncherUpdateManager {
    download_path: PathBuf,
    state_path: PathBuf,
}

impl LauncherUpdateManager {
    pub fn new(download_path: PathBuf, state_path: PathBuf) -> Self {
        Self {
            download_path,
            state_path,
        }
    }

    pub async fn update_launcher(
        &self,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<()> {
        self.run_update(message_sender, true).await.map(|_| ())
    }

    pub async fn update_launcher_if_available(
        &self,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<bool> {
        self.run_update(message_sender, false).await
    }

    async fn run_update(
        &self,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
        notify_when_current: bool,
    ) -> Result<bool> {
        send_message(
            &message_sender,
            LauncherMessage::SetStatus("Verificando update do launcher...".to_string()),
        )?;
        send_message(&message_sender, LauncherMessage::SetProcessing(true))?;
        send_message(&message_sender, LauncherMessage::DownloadProgress(0.0))?;

        let metadata_client = reqwest::Client::builder()
            .timeout(HTTP_REQUEST_TIMEOUT)
            .build()
            .context("Falha ao inicializar cliente HTTP do updater do launcher")?;
        let metadata = fetch_downloads_metadata(&metadata_client).await?;

        let release = metadata
            .launcher
            .ok_or_else(|| anyhow!("Metadata remota nao contem release do launcher"))?;

        if !self.should_install_release(&release, notify_when_current)? {
            if notify_when_current {
                send_message(
                    &message_sender,
                    LauncherMessage::SetTempMessage("Launcher ja esta atualizado".to_string()),
                )?;
                send_message(&message_sender, LauncherMessage::DownloadProgress(1.0))?;
                send_message(&message_sender, LauncherMessage::SetProcessing(false))?;
            }
            return Ok(false);
        }

        let update_dir = self.state_path.join("launcher-update");
        fs::create_dir_all(&update_dir)
            .with_context(|| format!("Falha ao criar {}", update_dir.display()))?;
        fs::create_dir_all(&self.download_path)
            .with_context(|| format!("Falha ao criar {}", self.download_path.display()))?;

        let zip_url = resolve_download_url(&release.zip)?;
        let zip_path = self.download_path.join(LAUNCHER_ZIP_FILE_NAME);
        let download_client = reqwest::Client::builder()
            .timeout(HTTP_DOWNLOAD_TIMEOUT)
            .build()
            .context("Falha ao inicializar download do updater do launcher")?;

        send_message(
            &message_sender,
            LauncherMessage::SetStatus("Baixando update do launcher...".to_string()),
        )?;
        download_to_path_with_progress(
            &download_client,
            &zip_url,
            &zip_path,
            &message_sender,
            "Baixando update do launcher",
            0.0,
            0.72,
        )
        .await?;

        if let Some(expected_size) = release.size {
            let actual_size = zip_path
                .metadata()
                .with_context(|| format!("Falha ao ler {}", zip_path.display()))?
                .len();
            if actual_size != expected_size {
                return Err(anyhow!(
                    "Tamanho invalido para {} (esperado {}, obtido {})",
                    zip_path.display(),
                    expected_size,
                    actual_size
                ));
            }
        }

        verify_hash(&zip_path, &release.sha256)?;

        send_message(
            &message_sender,
            LauncherMessage::SetStatus("Preparando update do launcher...".to_string()),
        )?;
        send_message(&message_sender, LauncherMessage::DownloadProgress(0.82))?;

        let current_exe = std::env::current_exe().context("Falha ao localizar launcher atual")?;
        let staged_exe = extract_launcher_executable(&zip_path, &update_dir, &current_exe)?;

        if let Some(expected_exe_hash) = &release.exe_sha256 {
            verify_hash(&staged_exe, expected_exe_hash)?;
        }

        spawn_replacement_helper(&update_dir, &staged_exe, &current_exe)?;

        send_message(
            &message_sender,
            LauncherMessage::SetStatus("Update pronto. Reiniciando launcher...".to_string()),
        )?;
        send_message(&message_sender, LauncherMessage::DownloadProgress(1.0))?;
        send_message(&message_sender, LauncherMessage::RestartLauncherForUpdate)?;

        Ok(true)
    }

    fn should_install_release(
        &self,
        release: &LauncherRelease,
        install_when_ambiguous: bool,
    ) -> Result<bool> {
        let Some(remote_version) = release.version.as_deref() else {
            if let Some(expected_exe_hash) = release.exe_sha256.as_deref() {
                return self.current_exe_hash_differs(expected_exe_hash);
            }

            return Ok(install_when_ambiguous);
        };

        let remote_version = parse_version(remote_version)?;
        let current_version = parse_version(env!("CARGO_PKG_VERSION"))?;

        if remote_version > current_version {
            return Ok(true);
        }

        if remote_version < current_version {
            return Ok(false);
        }

        if !install_when_ambiguous {
            return Ok(false);
        }

        let Some(expected_exe_hash) = release.exe_sha256.as_deref() else {
            return Ok(false);
        };

        self.current_exe_hash_differs(expected_exe_hash)
    }

    fn current_exe_hash_differs(&self, expected_exe_hash: &str) -> Result<bool> {
        match std::env::current_exe() {
            Ok(current_exe) if current_exe.exists() => {
                Ok(hash_file(&current_exe)? != expected_exe_hash.to_ascii_lowercase())
            }
            _ => Ok(true),
        }
    }
}

async fn fetch_downloads_metadata(http_client: &reqwest::Client) -> Result<DownloadsMetadata> {
    let response = http_client
        .get(DOWNLOADS_METADATA_URL)
        .send()
        .await
        .with_context(|| format!("Falha ao baixar {}", DOWNLOADS_METADATA_URL))?
        .error_for_status()
        .with_context(|| format!("Metadata rejeitada por {}", DOWNLOADS_METADATA_URL))?;

    let metadata_raw = response.text().await.context("Falha ao ler metadata")?;
    let metadata_raw = metadata_raw.trim_start_matches('\u{feff}');
    serde_json::from_str::<DownloadsMetadata>(metadata_raw)
        .context("Metadata de downloads invalida")
}

async fn download_to_path_with_progress(
    http_client: &reqwest::Client,
    url: &str,
    destination: &Path,
    message_sender: &mpsc::UnboundedSender<LauncherMessage>,
    status_prefix: &str,
    progress_start: f32,
    progress_end: f32,
) -> Result<()> {
    if destination.exists() {
        fs::remove_file(destination)
            .with_context(|| format!("Falha ao remover {}", destination.display()))?;
    }

    let response = http_client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Falha ao iniciar download de {}", url))?
        .error_for_status()
        .with_context(|| format!("Download rejeitado por {}", url))?;

    let total_bytes = response.content_length();
    let mut stream = response.bytes_stream();
    let mut file = File::create(destination)
        .with_context(|| format!("Falha ao criar {}", destination.display()))?;
    let started_at = Instant::now();
    let mut downloaded_bytes = 0u64;
    let mut last_report_at = Instant::now()
        .checked_sub(STATUS_REPORT_INTERVAL)
        .unwrap_or_else(Instant::now);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("Erro ao ler dados de {}", url))?;
        file.write_all(&chunk)?;
        downloaded_bytes += chunk.len() as u64;

        let should_report = last_report_at.elapsed() >= STATUS_REPORT_INTERVAL
            || total_bytes == Some(downloaded_bytes);
        if should_report {
            report_download_progress(
                message_sender,
                status_prefix,
                downloaded_bytes,
                total_bytes,
                started_at,
                progress_start,
                progress_end,
            )?;
            last_report_at = Instant::now();
        }
    }

    file.flush()?;
    report_download_progress(
        message_sender,
        status_prefix,
        downloaded_bytes,
        total_bytes,
        started_at,
        progress_start,
        progress_end,
    )?;

    Ok(())
}

fn extract_launcher_executable(
    zip_path: &Path,
    update_dir: &Path,
    current_exe: &Path,
) -> Result<PathBuf> {
    let current_name = current_exe
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "penultima-launcher.exe".to_string());
    let mut accepted_names = vec![current_name.clone()];
    if current_name != "penultima-launcher.exe" {
        accepted_names.push("penultima-launcher.exe".to_string());
    }

    let zip_file =
        File::open(zip_path).with_context(|| format!("Falha ao abrir {}", zip_path.display()))?;
    let mut archive = ZipArchive::new(zip_file).context("Falha ao ler ZIP do launcher")?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("Falha ao ler entrada {} do ZIP", index))?;
        if entry.is_dir() {
            continue;
        }

        let entry_name = Path::new(entry.name())
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase());
        if !accepted_names
            .iter()
            .any(|accepted_name| entry_name.as_deref() == Some(accepted_name.as_str()))
        {
            continue;
        }

        let staged_exe = update_dir.join(format!("{}.new", current_name));
        if staged_exe.exists() {
            fs::remove_file(&staged_exe)
                .with_context(|| format!("Falha ao remover {}", staged_exe.display()))?;
        }

        let mut output = BufWriter::new(
            File::create(&staged_exe)
                .with_context(|| format!("Falha ao criar {}", staged_exe.display()))?,
        );
        std::io::copy(&mut entry, &mut output).with_context(|| {
            format!(
                "Falha ao extrair {} para {}",
                entry.name(),
                staged_exe.display()
            )
        })?;
        output.flush()?;

        return Ok(staged_exe);
    }

    Err(anyhow!(
        "O ZIP do launcher nao contem {}",
        accepted_names.join(" ou ")
    ))
}

#[cfg(windows)]
fn spawn_replacement_helper(
    update_dir: &Path,
    staged_exe: &Path,
    current_exe: &Path,
) -> Result<()> {
    let helper_path = update_dir.join("apply-launcher-update.ps1");
    fs::write(&helper_path, replacement_helper_script())
        .with_context(|| format!("Falha ao criar {}", helper_path.display()))?;

    let working_directory = current_exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(&helper_path)
        .arg("-TargetProcessId")
        .arg(std::process::id().to_string())
        .arg("-Source")
        .arg(staged_exe)
        .arg("-Destination")
        .arg(current_exe)
        .arg("-WorkingDirectory")
        .arg(&working_directory)
        .spawn()
        .context("Falha ao iniciar helper de update do launcher")?;

    info!(
        "Helper de update iniciado para substituir {} por {}",
        current_exe.display(),
        staged_exe.display()
    );

    Ok(())
}

#[cfg(not(windows))]
fn spawn_replacement_helper(
    _update_dir: &Path,
    _staged_exe: &Path,
    _current_exe: &Path,
) -> Result<()> {
    Err(anyhow!(
        "Update automatico do launcher esta disponivel apenas no Windows"
    ))
}

fn replacement_helper_script() -> &'static str {
    r#"
param(
  [int]$TargetProcessId,
  [string]$Source,
  [string]$Destination,
  [string]$WorkingDirectory
)

$ErrorActionPreference = "Stop"
$backup = "${Destination}.old"
$logPath = "${Destination}.update.log"

try {
  $deadline = (Get-Date).AddSeconds(90)
  while ((Get-Date) -lt $deadline -and (Get-Process -Id $TargetProcessId -ErrorAction SilentlyContinue)) {
    Start-Sleep -Milliseconds 250
  }

  if (Get-Process -Id $TargetProcessId -ErrorAction SilentlyContinue) {
    throw "Timed out waiting for launcher process $TargetProcessId to exit."
  }

  Start-Sleep -Milliseconds 500

  Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue

  if (Test-Path -LiteralPath $Destination) {
    Move-Item -LiteralPath $Destination -Destination $backup -Force
  }

  Move-Item -LiteralPath $Source -Destination $Destination -Force
  Start-Process -FilePath $Destination -WorkingDirectory $WorkingDirectory
  Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
} catch {
  $message = "$(Get-Date -Format o) Launcher update failed: $($_.Exception.Message)"
  try {
    Add-Content -LiteralPath $logPath -Value $message
  } catch {
  }

  if ((-not (Test-Path -LiteralPath $Destination)) -and (Test-Path -LiteralPath $backup)) {
    Move-Item -LiteralPath $backup -Destination $Destination -Force
  }

  if (Test-Path -LiteralPath $Destination) {
    Start-Process -FilePath $Destination -WorkingDirectory $WorkingDirectory
  }

  exit 1
}
"#
}

fn resolve_download_url(path: &str) -> Result<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Metadata do launcher contem URL vazia"));
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Ok(trimmed.to_string());
    }

    Ok(format!(
        "{}/{}",
        WEBSITE_BASE_URL.trim_end_matches('/'),
        trimmed.trim_start_matches('/')
    ))
}

fn parse_version(value: &str) -> Result<Version> {
    let normalized = value.trim().trim_start_matches('v');
    Version::parse(normalized).with_context(|| format!("Versao do launcher invalida: {}", value))
}

fn report_download_progress(
    sender: &mpsc::UnboundedSender<LauncherMessage>,
    status_prefix: &str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    started_at: Instant,
    progress_start: f32,
    progress_end: f32,
) -> Result<()> {
    let elapsed_secs = started_at.elapsed().as_secs_f64().max(0.1);
    let bytes_per_second = downloaded_bytes as f64 / elapsed_secs;
    let status = if let Some(total_bytes) = total_bytes {
        format!(
            "{}... {} / {} ({}/s)",
            status_prefix,
            format_bytes(downloaded_bytes),
            format_bytes(total_bytes),
            format_bytes(bytes_per_second as u64)
        )
    } else {
        format!(
            "{}... {} ({}/s)",
            status_prefix,
            format_bytes(downloaded_bytes),
            format_bytes(bytes_per_second as u64)
        )
    };

    send_message(sender, LauncherMessage::SetStatus(status))?;

    if let Some(total_bytes) = total_bytes {
        if total_bytes > 0 {
            let fraction = downloaded_bytes as f32 / total_bytes as f32;
            let progress =
                progress_start + fraction.clamp(0.0, 1.0) * (progress_end - progress_start);
            send_message(sender, LauncherMessage::DownloadProgress(progress))?;
        }
    }

    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];

    let mut value = bytes as f64;
    let mut unit_index = 0usize;
    while value >= 1024.0 && unit_index < UNITS.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[unit_index])
    } else {
        format!("{value:.1} {}", UNITS[unit_index])
    }
}

fn verify_hash(path: &Path, expected_hash: &str) -> Result<()> {
    let actual_hash = hash_file(path)?;
    let expected = expected_hash.to_ascii_lowercase();
    if actual_hash != expected {
        return Err(anyhow!(
            "Hash invalido para {} (esperado {}, obtido {})",
            path.display(),
            expected,
            actual_hash
        ));
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("Falha ao abrir {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(bytes_to_hex(&hasher.finalize()))
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(hex_nibble(byte >> 4));
        output.push(hex_nibble(byte & 0x0f));
    }
    output
}

fn hex_nibble(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'a' + (value - 10)) as char,
        _ => unreachable!(),
    }
}

fn send_message(
    sender: &mpsc::UnboundedSender<LauncherMessage>,
    message: LauncherMessage,
) -> Result<()> {
    sender
        .send(message)
        .map_err(|error| anyhow!("Falha ao enviar mensagem para a UI: {}", error))
}

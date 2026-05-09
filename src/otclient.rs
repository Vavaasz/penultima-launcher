use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use log::info;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use zip::ZipArchive;

use crate::constants::{HTTP_DOWNLOAD_TIMEOUT, OTCRP_LAUNCHER_BOOTSTRAP_URL};
use crate::message_system::LauncherMessage;
use crate::tokio::sync::mpsc;

const OTCLIENT_LAUNCHER_EXE: &str = "OTCLauncher.exe";
const OTCLIENT_REQUIRED_FILES: &[&str] = &[OTCLIENT_LAUNCHER_EXE, "cacert.pem"];

pub async fn ensure_otcrp_launcher(
    install_path: PathBuf,
    message_sender: mpsc::UnboundedSender<LauncherMessage>,
) -> Result<PathBuf> {
    fs::create_dir_all(&install_path)
        .with_context(|| format!("Falha ao criar {}", install_path.display()))?;

    let launcher_path = install_path.join(OTCLIENT_LAUNCHER_EXE);
    if OTCLIENT_REQUIRED_FILES
        .iter()
        .all(|file_name| install_path.join(file_name).exists())
    {
        return Ok(launcher_path);
    }

    send_message(
        &message_sender,
        LauncherMessage::SetStatus("Baixando OTClient Redemption...".to_string()),
    )?;
    send_message(&message_sender, LauncherMessage::DownloadProgress(0.0))?;

    let archive_path = install_path.join("launcher-bootstrap.zip.download");
    download_bootstrap(&archive_path, &message_sender).await?;

    send_message(
        &message_sender,
        LauncherMessage::SetStatus("Instalando OTClient Redemption...".to_string()),
    )?;
    send_message(&message_sender, LauncherMessage::DownloadProgress(0.85))?;

    extract_bootstrap(&archive_path, &install_path)?;
    if archive_path.exists() {
        fs::remove_file(&archive_path).ok();
    }

    if !launcher_path.exists() {
        return Err(anyhow!(
            "OTCLauncher.exe nao foi encontrado apos extrair o bootstrap"
        ));
    }

    send_message(&message_sender, LauncherMessage::DownloadProgress(1.0))?;
    Ok(launcher_path)
}

async fn download_bootstrap(
    archive_path: &Path,
    message_sender: &mpsc::UnboundedSender<LauncherMessage>,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_DOWNLOAD_TIMEOUT)
        .build()
        .context("Falha ao inicializar cliente HTTP do OTClient")?;

    let response = client
        .get(OTCRP_LAUNCHER_BOOTSTRAP_URL)
        .send()
        .await
        .context("Falha ao baixar bootstrap do OTClient")?
        .error_for_status()
        .context("Bootstrap do OTClient retornou erro HTTP")?;

    let total_bytes = response.content_length();
    let started_at = Instant::now();
    let mut downloaded_bytes = 0u64;
    let mut stream = response.bytes_stream();
    let mut file = BufWriter::new(
        File::create(archive_path)
            .with_context(|| format!("Falha ao criar {}", archive_path.display()))?,
    );

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Falha ao ler dados do bootstrap do OTClient")?;
        file.write_all(&chunk)?;
        downloaded_bytes += chunk.len() as u64;

        if let Some(total) = total_bytes {
            if total > 0 {
                let progress = ((downloaded_bytes as f32 / total as f32) * 0.80).clamp(0.0, 0.80);
                send_message(message_sender, LauncherMessage::DownloadProgress(progress))?;
            }
        }
    }
    file.flush()?;

    info!(
        "Bootstrap OTClient baixado: {} bytes em {:.1}s",
        downloaded_bytes,
        started_at.elapsed().as_secs_f32()
    );

    Ok(())
}

fn extract_bootstrap(archive_path: &Path, install_path: &Path) -> Result<()> {
    let archive_file = File::open(archive_path)
        .with_context(|| format!("Falha ao abrir {}", archive_path.display()))?;
    let mut archive = ZipArchive::new(archive_file).context("Bootstrap OTClient ZIP invalido")?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("Falha ao ler entrada {} do bootstrap OTClient", index))?;

        let Some(relative_path) = safe_zip_path(entry.name()) else {
            continue;
        };

        let destination = install_path.join(relative_path);
        if entry.is_dir() {
            fs::create_dir_all(&destination)?;
            continue;
        }

        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }

        let temp_path = destination.with_extension("download");
        {
            let mut output = BufWriter::new(
                File::create(&temp_path)
                    .with_context(|| format!("Falha ao criar {}", temp_path.display()))?,
            );
            std::io::copy(&mut entry, &mut output)?;
            output.flush()?;
        }

        if destination.exists() {
            fs::remove_file(&destination)?;
        }
        fs::rename(&temp_path, &destination).or_else(|_| {
            fs::copy(&temp_path, &destination)?;
            fs::remove_file(&temp_path)?;
            Ok::<(), std::io::Error>(())
        })?;
    }

    Ok(())
}

fn safe_zip_path(name: &str) -> Option<PathBuf> {
    let normalized = name.replace('\\', "/");
    let mut path = PathBuf::new();

    for part in normalized.split('/').filter(|part| !part.is_empty()) {
        if part == "." || part == ".." {
            return None;
        }
        path.push(part);
    }

    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path)
    }
}

fn send_message(
    message_sender: &mpsc::UnboundedSender<LauncherMessage>,
    message: LauncherMessage,
) -> Result<()> {
    message_sender
        .send(message)
        .map_err(|_| anyhow!("Canal de mensagens do launcher encerrado"))
}

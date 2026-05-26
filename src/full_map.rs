use anyhow::{Context, Result, anyhow};
use flate2::read::ZlibDecoder;
use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use zip::ZipArchive;

use crate::constants::{
    DOWNLOADS_METADATA_URL, FULL_MINIMAP_ARCHIVE_URL, FULL_MINIMAP_URL_ENV, HTTP_DOWNLOAD_TIMEOUT,
    HTTP_REQUEST_TIMEOUT, WEBSITE_BASE_URL,
};
use crate::message_system::LauncherMessage;
use crate::tokio::sync::mpsc;

const FULL_MINIMAP_ZIP_NAME: &str = "Penultima-Full-Minimap.zip";
const DOWNLOAD_PROGRESS_END: f32 = 0.82;
const EXTRACT_PROGRESS_END: f32 = 0.98;
const STATUS_REPORT_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FullMinimapInstallStats {
    pub files: usize,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FullMapInstallRoot {
    Minimap,
    Assets,
}

#[derive(Debug)]
struct FullMapArchiveEntry {
    index: usize,
    root: FullMapInstallRoot,
    relative_path: PathBuf,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct DownloadsMetadata {
    full_minimap: Option<FullMinimapRelease>,
}

#[derive(Debug, Deserialize)]
struct FullMinimapRelease {
    zip: String,
    sha256: Option<String>,
    size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FullMinimapDownload {
    url: String,
    expected_sha256: Option<String>,
    expected_size: Option<u64>,
}

pub async fn download_and_install_full_minimap(
    download_path: PathBuf,
    game_path: PathBuf,
    message_sender: mpsc::UnboundedSender<LauncherMessage>,
) -> Result<FullMinimapInstallStats> {
    fs::create_dir_all(&download_path)
        .with_context(|| format!("Nao foi possivel criar {}", download_path.display()))?;
    fs::create_dir_all(&game_path)
        .with_context(|| format!("Nao foi possivel criar {}", game_path.display()))?;

    send_message(
        &message_sender,
        LauncherMessage::SetStatus("Verificando pacote do full/custom map...".to_string()),
    )?;

    let metadata_client = reqwest::Client::builder()
        .connect_timeout(HTTP_REQUEST_TIMEOUT)
        .timeout(HTTP_REQUEST_TIMEOUT)
        .build()
        .context("Falha ao preparar metadata do full map")?;
    let download_source = resolve_full_minimap_download(&metadata_client).await?;

    download_and_install_full_minimap_from_source(
        download_source,
        download_path,
        game_path,
        message_sender,
    )
    .await
}

async fn download_and_install_full_minimap_from_url(
    url: String,
    download_path: PathBuf,
    game_path: PathBuf,
    message_sender: mpsc::UnboundedSender<LauncherMessage>,
) -> Result<FullMinimapInstallStats> {
    download_and_install_full_minimap_from_source(
        FullMinimapDownload {
            url,
            expected_sha256: None,
            expected_size: None,
        },
        download_path,
        game_path,
        message_sender,
    )
    .await
}

async fn download_and_install_full_minimap_from_source(
    download_source: FullMinimapDownload,
    download_path: PathBuf,
    game_path: PathBuf,
    message_sender: mpsc::UnboundedSender<LauncherMessage>,
) -> Result<FullMinimapInstallStats> {
    fs::create_dir_all(&download_path)
        .with_context(|| format!("Nao foi possivel criar {}", download_path.display()))?;
    fs::create_dir_all(&game_path)
        .with_context(|| format!("Nao foi possivel criar {}", game_path.display()))?;

    let archive_path = download_path.join(FULL_MINIMAP_ZIP_NAME);

    send_message(
        &message_sender,
        LauncherMessage::SetStatus("Baixando full map e custom map...".to_string()),
    )?;
    send_message(&message_sender, LauncherMessage::DownloadProgress(0.0))?;

    let http_client = reqwest::Client::builder()
        .connect_timeout(HTTP_REQUEST_TIMEOUT)
        .timeout(HTTP_DOWNLOAD_TIMEOUT)
        .build()
        .context("Falha ao preparar download do full map")?;

    download_to_path(
        &http_client,
        &download_source.url,
        &archive_path,
        &message_sender,
        "Baixando full map e custom map",
    )
    .await?;

    if let Some(expected_size) = download_source.expected_size {
        verify_download_size(&archive_path, expected_size)?;
    }

    if let Some(expected_sha256) = download_source.expected_sha256.as_deref() {
        verify_hash(&archive_path, expected_sha256)?;
    }

    send_message(
        &message_sender,
        LauncherMessage::SetStatus("Instalando full map e custom map...".to_string()),
    )?;

    let install_archive_path = archive_path.clone();
    let install_game_path = game_path.clone();
    let install_sender = message_sender.clone();
    let stats = tokio::task::spawn_blocking(move || {
        install_full_minimap_from_zip(
            &install_archive_path,
            &install_game_path,
            Some(&install_sender),
        )
    })
    .await
    .context("Falha ao aguardar instalacao do full map")??;

    send_message(&message_sender, LauncherMessage::DownloadProgress(1.0))?;
    send_message(
        &message_sender,
        LauncherMessage::SetTempMessage(format!(
            "Full/custom map instalado: {} arquivos ({})",
            stats.files,
            format_bytes(stats.bytes)
        )),
    )?;
    send_message(&message_sender, LauncherMessage::SetProcessing(false))?;

    Ok(stats)
}

pub fn install_full_minimap_from_zip(
    archive_path: &Path,
    game_path: &Path,
    message_sender: Option<&mpsc::UnboundedSender<LauncherMessage>>,
) -> Result<FullMinimapInstallStats> {
    let archive_file = File::open(archive_path)
        .with_context(|| format!("Falha ao abrir {}", archive_path.display()))?;
    let mut archive = ZipArchive::new(archive_file).context("Falha ao ler ZIP do full map")?;

    let mut entries = Vec::new();
    let mut total_bytes = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("Falha ao ler entrada {} do ZIP", index))?;
        if entry.is_dir() {
            continue;
        }

        let Some((root, relative_path)) = full_map_archive_path(entry.name()) else {
            continue;
        };

        if root == FullMapInstallRoot::Minimap {
            let entry_name = entry.name().to_string();
            validate_minimap_archive_entry(&entry_name, &mut entry)?;
        }

        total_bytes += entry.size();
        entries.push(FullMapArchiveEntry {
            index,
            root,
            relative_path,
            size: entry.size(),
        });
    }

    if entries.is_empty() {
        return Err(anyhow!("O ZIP do full map nao contem arquivos de mapa"));
    }

    let minimap_dir = game_path.join("minimap");
    let assets_dir = game_path.join("assets");
    fs::create_dir_all(&minimap_dir)
        .with_context(|| format!("Nao foi possivel criar {}", minimap_dir.display()))?;
    cleanup_stale_full_map_minimap(&minimap_dir, &entries)?;
    if entries
        .iter()
        .any(|entry| entry.root == FullMapInstallRoot::Assets)
    {
        fs::create_dir_all(&assets_dir)
            .with_context(|| format!("Nao foi possivel criar {}", assets_dir.display()))?;
        cleanup_stale_full_map_assets(&assets_dir, &entries)?;
    }

    let mut created_directories = HashSet::new();
    let mut extracted_files = 0usize;
    let mut extracted_bytes = 0u64;
    let mut last_report_at = Instant::now()
        .checked_sub(STATUS_REPORT_INTERVAL)
        .unwrap_or_else(Instant::now);

    for (position, entry_to_install) in entries.iter().enumerate() {
        let mut entry = archive.by_index(entry_to_install.index).with_context(|| {
            format!("Falha ao reabrir entrada {} do ZIP", entry_to_install.index)
        })?;
        let destination_root = match entry_to_install.root {
            FullMapInstallRoot::Minimap => &minimap_dir,
            FullMapInstallRoot::Assets => &assets_dir,
        };
        let destination = destination_root.join(&entry_to_install.relative_path);
        let is_catalog_entry = entry_to_install.root == FullMapInstallRoot::Assets
            && entry_to_install
                .relative_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("catalog-content.json"));

        if let Some(parent) = destination.parent() {
            if created_directories.insert(parent.to_path_buf()) {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Falha ao criar {}", parent.display()))?;
            }
        }

        if is_catalog_entry {
            merge_catalog_content_from_archive(&destination, &mut entry)?;
        } else {
            let temp_path = destination.with_extension(format!(
                "{}fullmap.tmp",
                destination
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(|value| format!("{value}."))
                    .unwrap_or_default()
            ));
            if temp_path.exists() {
                fs::remove_file(&temp_path)?;
            }

            {
                let mut output = BufWriter::with_capacity(
                    256 * 1024,
                    File::create(&temp_path)
                        .with_context(|| format!("Falha ao criar {}", temp_path.display()))?,
                );
                std::io::copy(&mut entry, &mut output).with_context(|| {
                    format!(
                        "Falha ao extrair {} para {}",
                        entry_to_install.relative_path.display(),
                        destination.display()
                    )
                })?;
                output.flush()?;
            }

            replace_file(&temp_path, &destination)?;
        }
        extracted_files += 1;
        extracted_bytes += entry_to_install.size;

        if let Some(sender) = message_sender {
            let should_report =
                last_report_at.elapsed() >= STATUS_REPORT_INTERVAL || position + 1 == entries.len();
            if should_report {
                let fraction = if total_bytes == 0 {
                    1.0
                } else {
                    (extracted_bytes as f32 / total_bytes as f32).clamp(0.0, 1.0)
                };
                let progress = DOWNLOAD_PROGRESS_END
                    + fraction * (EXTRACT_PROGRESS_END - DOWNLOAD_PROGRESS_END);
                send_message(sender, LauncherMessage::DownloadProgress(progress))?;
                send_message(
                    sender,
                    LauncherMessage::SetStatus(format!(
                        "Instalando full/custom map... {}/{} arquivos",
                        extracted_files,
                        entries.len()
                    )),
                )?;
                last_report_at = Instant::now();
            }
        }
    }

    Ok(FullMinimapInstallStats {
        files: extracted_files,
        bytes: extracted_bytes,
    })
}

async fn resolve_full_minimap_download(
    http_client: &reqwest::Client,
) -> Result<FullMinimapDownload> {
    if let Some(override_url) = env::var(FULL_MINIMAP_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Ok(FullMinimapDownload {
            url: override_url,
            expected_sha256: None,
            expected_size: None,
        });
    }

    match fetch_downloads_metadata(http_client).await {
        Ok(metadata) => {
            let release = metadata
                .full_minimap
                .ok_or_else(|| anyhow!("Metadata remota nao contem full_minimap"))?;
            Ok(FullMinimapDownload {
                url: resolve_download_url(&release.zip)?,
                expected_sha256: release.sha256,
                expected_size: release.size,
            })
        }
        Err(_) => Ok(FullMinimapDownload {
            url: FULL_MINIMAP_ARCHIVE_URL.to_string(),
            expected_sha256: None,
            expected_size: None,
        }),
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

fn resolve_download_url(path_or_url: &str) -> Result<String> {
    let trimmed = path_or_url.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Metadata do full map contem URL vazia"));
    }

    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        return Ok(trimmed.to_string());
    }

    Ok(format!(
        "{}/{}",
        WEBSITE_BASE_URL.trim_end_matches('/'),
        trimmed.trim_start_matches('/')
    ))
}

fn verify_download_size(path: &Path, expected_size: u64) -> Result<()> {
    let actual_size = path
        .metadata()
        .with_context(|| format!("Falha ao ler {}", path.display()))?
        .len();
    if actual_size != expected_size {
        return Err(anyhow!(
            "Tamanho invalido para {} (esperado {}, obtido {})",
            path.display(),
            expected_size,
            actual_size
        ));
    }

    Ok(())
}

fn verify_hash(path: &Path, expected_sha256: &str) -> Result<()> {
    let actual_sha256 = hash_file(path)?;
    let expected_sha256 = expected_sha256.trim();
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(anyhow!(
            "Hash invalido para {} (esperado {}, obtido {})",
            path.display(),
            expected_sha256,
            actual_sha256
        ));
    }

    Ok(())
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = BufReader::new(
        File::open(path).with_context(|| format!("Falha ao abrir {}", path.display()))?,
    );
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("Falha ao ler {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

async fn download_to_path(
    http_client: &reqwest::Client,
    url: &str,
    destination: &Path,
    message_sender: &mpsc::UnboundedSender<LauncherMessage>,
    status_prefix: &str,
) -> Result<()> {
    if destination.exists() {
        fs::remove_file(destination)?;
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
    )?;

    Ok(())
}

fn report_download_progress(
    sender: &mpsc::UnboundedSender<LauncherMessage>,
    status_prefix: &str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    started_at: Instant,
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
            send_message(
                sender,
                LauncherMessage::DownloadProgress(fraction.clamp(0.0, 1.0) * DOWNLOAD_PROGRESS_END),
            )?;
        }
    }

    Ok(())
}

fn full_map_archive_path(entry_name: &str) -> Option<(FullMapInstallRoot, PathBuf)> {
    let normalized = entry_name.replace('\\', "/");
    let mut clean_parts = Vec::new();
    for part in normalized.split('/').filter(|part| !part.is_empty()) {
        let path = Path::new(part);
        if path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return None;
        }
        if part == "." {
            continue;
        }
        clean_parts.push(part);
    }

    if clean_parts.is_empty() {
        return None;
    }

    let filename = clean_parts.last()?;
    if let Some(start_index) = clean_parts
        .iter()
        .position(|part| part.eq_ignore_ascii_case("assets"))
        .map(|index| index + 1)
    {
        return archive_relative_from_parts(
            FullMapInstallRoot::Assets,
            &clean_parts,
            start_index,
            is_asset_map_file(filename),
        );
    }

    if let Some(start_index) = clean_parts
        .iter()
        .position(|part| part.eq_ignore_ascii_case("minimap"))
        .map(|index| index + 1)
    {
        return archive_relative_from_parts(
            FullMapInstallRoot::Minimap,
            &clean_parts,
            start_index,
            is_client_minimap_file(filename) || filename.eq_ignore_ascii_case(".gitkeep"),
        );
    }

    if is_client_minimap_file(filename) {
        return Some((FullMapInstallRoot::Minimap, PathBuf::from(filename)));
    }

    if is_asset_map_file(filename) {
        return Some((FullMapInstallRoot::Assets, PathBuf::from(filename)));
    }

    None
}

fn archive_relative_from_parts(
    root: FullMapInstallRoot,
    clean_parts: &[&str],
    start_index: usize,
    allowed: bool,
) -> Option<(FullMapInstallRoot, PathBuf)> {
    if !allowed || start_index >= clean_parts.len() {
        return None;
    }

    let mut relative_path = PathBuf::new();
    for part in &clean_parts[start_index..] {
        relative_path.push(part);
    }

    (!relative_path.as_os_str().is_empty()).then_some((root, relative_path))
}

fn validate_minimap_archive_entry<R: Read>(
    entry_name: &str,
    entry: &mut zip::read::ZipFile<R>,
) -> Result<()> {
    let filename = Path::new(entry_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(entry_name);
    if !is_client_minimap_file(filename) {
        return Ok(());
    }

    let mut header = [0u8; 26];
    entry.read_exact(&mut header).with_context(|| {
        format!(
            "Minimap invalido em {}: nao foi possivel ler cabecalho PNG",
            entry_name
        )
    })?;

    let is_png = header.starts_with(b"\x89PNG\r\n\x1a\n") && &header[12..16] == b"IHDR";
    let is_indexed_png = is_png && header[24] == 8 && header[25] == 3;
    if !is_indexed_png {
        return Err(anyhow!(
            "Minimap invalido em {}: PNG precisa ser indexed/paletted para o cliente",
            entry_name
        ));
    }

    Ok(())
}

fn merge_catalog_content_from_archive<R: Read>(
    catalog_path: &Path,
    entry: &mut zip::read::ZipFile<R>,
) -> Result<()> {
    let mut archive_catalog = Vec::new();
    entry
        .read_to_end(&mut archive_catalog)
        .context("Falha ao ler catalog-content.json do full map")?;

    if !catalog_path.exists() {
        return write_atomic_bytes(catalog_path, &archive_catalog);
    }

    let archive_catalog_value = match serde_json::from_slice::<serde_json::Value>(&archive_catalog)
    {
        Ok(value) => value,
        Err(_) => return write_atomic_bytes(catalog_path, &archive_catalog),
    };

    let Some(mut current_catalog_value) = select_existing_catalog_for_merge(catalog_path)? else {
        return write_atomic_bytes(catalog_path, &archive_catalog);
    };

    let Some(archive_entries) = map_catalog_entries(&archive_catalog_value) else {
        return write_atomic_bytes(catalog_path, &archive_catalog);
    };
    if archive_entries.is_empty() {
        return Ok(());
    }

    let Some(current_entries) = current_catalog_value.as_array_mut() else {
        return write_atomic_bytes(catalog_path, &archive_catalog);
    };

    let mut replaced_types = HashSet::new();
    for current_entry in current_entries.iter_mut() {
        let Some(entry_type) = map_catalog_type(current_entry) else {
            continue;
        };
        if let Some(replacement) = archive_entries.get(&entry_type) {
            *current_entry = replacement.clone();
            replaced_types.insert(entry_type);
        }
    }

    for (entry_type, replacement) in archive_entries {
        if !replaced_types.contains(&entry_type) {
            current_entries.push(replacement);
        }
    }

    write_catalog_value(catalog_path, &current_catalog_value)
}

fn map_catalog_entries(value: &serde_json::Value) -> Option<HashMap<String, serde_json::Value>> {
    let entries = value.as_array()?;
    let mut map_entries = HashMap::new();
    for entry in entries {
        let Some(entry_type) = map_catalog_type(entry) else {
            continue;
        };
        map_entries.insert(entry_type, entry.clone());
    }
    Some(map_entries)
}

fn select_existing_catalog_for_merge(catalog_path: &Path) -> Result<Option<serde_json::Value>> {
    let assets_dir = catalog_path.parent().unwrap_or_else(|| Path::new("."));
    let mut candidates = Vec::new();
    for (order, candidate_path) in catalog_candidate_paths(catalog_path)?
        .into_iter()
        .enumerate()
    {
        let Ok(raw) = fs::read_to_string(&candidate_path) else {
            continue;
        };
        let raw = raw.trim_start_matches('\u{feff}');
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        let missing_files = count_missing_non_map_catalog_files(assets_dir, &value);
        candidates.push((missing_files, order, value));
    }

    candidates.sort_by_key(|(missing_files, order, _)| (*missing_files, *order));
    Ok(candidates.into_iter().next().map(|(_, _, value)| value))
}

fn catalog_candidate_paths(catalog_path: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    if seen.insert(catalog_path.to_path_buf()) {
        paths.push(catalog_path.to_path_buf());
    }

    let Some(parent) = catalog_path.parent() else {
        return Ok(paths);
    };

    for entry in
        fs::read_dir(parent).with_context(|| format!("Falha ao listar {}", parent.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let filename = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if filename.starts_with("catalog-content.json")
            && filename.contains("bak")
            && seen.insert(entry.path())
        {
            paths.push(entry.path());
        }
    }

    Ok(paths)
}

fn count_missing_non_map_catalog_files(assets_dir: &Path, value: &serde_json::Value) -> usize {
    let Some(entries) = value.as_array() else {
        return usize::MAX;
    };

    entries
        .iter()
        .filter(|entry| map_catalog_type(entry).is_none())
        .filter_map(|entry| entry.get("file").and_then(|file| file.as_str()))
        .filter(|file| !assets_dir.join(file).exists())
        .count()
}

fn map_catalog_type(value: &serde_json::Value) -> Option<String> {
    let entry_type = value.get("type")?.as_str()?;
    matches!(entry_type, "map" | "staticdata" | "staticmapdata").then(|| entry_type.to_string())
}

fn write_catalog_value(destination: &Path, value: &serde_json::Value) -> Result<()> {
    let temp_path = destination.with_extension("json.fullmap.tmp");
    if temp_path.exists() {
        fs::remove_file(&temp_path)?;
    }

    {
        let mut output = BufWriter::with_capacity(
            256 * 1024,
            File::create(&temp_path)
                .with_context(|| format!("Falha ao criar {}", temp_path.display()))?,
        );
        serde_json::to_writer_pretty(&mut output, value)
            .with_context(|| format!("Falha ao escrever {}", temp_path.display()))?;
        output.write_all(b"\n")?;
        output.flush()?;
    }

    replace_file(&temp_path, destination)
}

fn write_atomic_bytes(destination: &Path, bytes: &[u8]) -> Result<()> {
    let temp_path = destination.with_extension("fullmap.tmp");
    if temp_path.exists() {
        fs::remove_file(&temp_path)?;
    }

    {
        let mut output = BufWriter::with_capacity(
            256 * 1024,
            File::create(&temp_path)
                .with_context(|| format!("Falha ao criar {}", temp_path.display()))?,
        );
        output.write_all(bytes)?;
        output.flush()?;
    }

    replace_file(&temp_path, destination)
}

fn is_client_minimap_file(filename: &str) -> bool {
    is_client_minimap_color_file(filename)
}

fn is_client_minimap_color_file(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    lower.starts_with("minimap_color_")
}

fn is_client_waypoint_cost_file(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    lower.starts_with("minimap_waypointcost_")
}

fn is_asset_map_file(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    lower == "catalog-content.json"
        || lower.starts_with("subarea-")
        || lower.starts_with("minimap-")
        || lower.starts_with("satellite-")
        || lower.starts_with("map-")
        || lower.starts_with("staticdata-")
        || lower.starts_with("staticmapdata-")
}

fn cleanup_stale_full_map_assets(assets_dir: &Path, entries: &[FullMapArchiveEntry]) -> Result<()> {
    if !assets_dir.exists() {
        return Ok(());
    }

    let archive_asset_filenames: HashSet<String> = entries
        .iter()
        .filter(|entry| entry.root == FullMapInstallRoot::Assets)
        .filter_map(|entry| entry.relative_path.file_name())
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .collect();

    for entry in fs::read_dir(assets_dir)
        .with_context(|| format!("Falha ao listar {}", assets_dir.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }

        let filename = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if !is_cleanup_candidate_asset(&filename) || archive_asset_filenames.contains(&filename) {
            continue;
        }

        fs::remove_file(entry.path())
            .with_context(|| format!("Falha ao remover asset antigo {}", entry.path().display()))?;
    }

    Ok(())
}

fn cleanup_stale_full_map_minimap(
    minimap_dir: &Path,
    entries: &[FullMapArchiveEntry],
) -> Result<()> {
    if !minimap_dir.exists() {
        return Ok(());
    }

    let archive_minimap_filenames: HashSet<String> = entries
        .iter()
        .filter(|entry| entry.root == FullMapInstallRoot::Minimap)
        .filter_map(|entry| entry.relative_path.file_name())
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .collect();

    for entry in fs::read_dir(minimap_dir)
        .with_context(|| format!("Falha ao listar {}", minimap_dir.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }

        let path = entry.path();
        let filename = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if is_client_waypoint_cost_file(&filename) {
            if matches!(is_broken_waypoint_cost_file(&path), Ok(true)) {
                fs::remove_file(&path).with_context(|| {
                    format!("Falha ao remover waypoint-cost invalido {}", path.display())
                })?;
            }
            continue;
        }

        if !is_client_minimap_color_file(&filename) || archive_minimap_filenames.contains(&filename)
        {
            continue;
        }

        fs::remove_file(&path)
            .with_context(|| format!("Falha ao remover minimap antigo {}", path.display()))?;
    }

    Ok(())
}

fn is_broken_waypoint_cost_file(path: &Path) -> Result<bool> {
    let data = fs::read(path).with_context(|| format!("Falha ao ler {}", path.display()))?;
    Ok(classify_broken_waypoint_cost_png(&data).unwrap_or(false))
}

fn classify_broken_waypoint_cost_png(data: &[u8]) -> Option<bool> {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(false);
    }

    let mut offset = 8usize;
    let mut width = None;
    let mut height = None;
    let mut bit_depth = None;
    let mut color_type = None;
    let mut interlace = None;
    let mut idat = Vec::new();

    while offset.checked_add(8)? <= data.len() {
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().ok()?) as usize;
        let chunk_type = &data[offset + 4..offset + 8];
        offset += 8;

        let payload_end = offset.checked_add(length)?;
        let chunk_end = payload_end.checked_add(4)?;
        if chunk_end > data.len() {
            return None;
        }
        let payload = &data[offset..payload_end];

        match chunk_type {
            b"IHDR" if payload.len() >= 13 => {
                width = Some(u32::from_be_bytes(payload[0..4].try_into().ok()?));
                height = Some(u32::from_be_bytes(payload[4..8].try_into().ok()?));
                bit_depth = Some(payload[8]);
                color_type = Some(payload[9]);
                interlace = Some(payload[12]);
            }
            b"IDAT" => idat.extend_from_slice(payload),
            b"IEND" => break,
            _ => {}
        }

        offset = chunk_end;
    }

    let width = width?;
    let height = height?;
    if bit_depth? != 8 || color_type? != 3 {
        return Some(true);
    }
    if interlace? != 0 || width == 0 || height == 0 || width > 4096 || height > 4096 {
        return None;
    }
    if idat.is_empty() {
        return None;
    }

    let mut decoder = ZlibDecoder::new(idat.as_slice());
    let mut decoded = Vec::new();
    if decoder.read_to_end(&mut decoded).is_err() {
        return None;
    }

    indexed_png_scanlines_are_all_zero(width as usize, height as usize, &decoded)
}

fn indexed_png_scanlines_are_all_zero(width: usize, height: usize, decoded: &[u8]) -> Option<bool> {
    let row_size = width.checked_add(1)?;
    let expected_size = row_size.checked_mul(height)?;
    if decoded.len() < expected_size {
        return None;
    }

    let mut previous = vec![0u8; width];
    let mut current = vec![0u8; width];
    let mut all_zero = true;

    for y in 0..height {
        let row_offset = y.checked_mul(row_size)?;
        let filter = decoded[row_offset];
        current.copy_from_slice(&decoded[row_offset + 1..row_offset + 1 + width]);

        for x in 0..width {
            let left = if x == 0 { 0 } else { current[x - 1] };
            let up = previous[x];
            let upper_left = if x == 0 { 0 } else { previous[x - 1] };
            let predictor = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((left as u16 + up as u16) / 2) as u8,
                4 => paeth_predictor(left, up, upper_left),
                _ => return None,
            };
            current[x] = current[x].wrapping_add(predictor);
            if current[x] != 0 {
                all_zero = false;
            }
        }

        previous.copy_from_slice(&current);
    }

    Some(all_zero)
}

fn paeth_predictor(left: u8, up: u8, upper_left: u8) -> u8 {
    let left = left as i32;
    let up = up as i32;
    let upper_left = upper_left as i32;
    let estimate = left + up - upper_left;
    let left_distance = (estimate - left).abs();
    let up_distance = (estimate - up).abs();
    let upper_left_distance = (estimate - upper_left).abs();

    if left_distance <= up_distance && left_distance <= upper_left_distance {
        left as u8
    } else if up_distance <= upper_left_distance {
        up as u8
    } else {
        upper_left as u8
    }
}

fn is_cleanup_candidate_asset(filename: &str) -> bool {
    (filename.starts_with("subarea-") && filename.ends_with(".bmp.lzma"))
        || (filename.starts_with("minimap-") && filename.ends_with(".bmp.lzma"))
        || (filename.starts_with("satellite-") && filename.ends_with(".bmp.lzma"))
        || (filename.starts_with("map-") && filename.ends_with(".dat"))
        || (filename.starts_with("staticdata-")
            && (filename.ends_with(".dat") || filename.ends_with(".dat.lzma")))
        || (filename.starts_with("staticmapdata-")
            && (filename.ends_with(".dat") || filename.ends_with(".dat.lzma")))
}

fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(source, destination).or_else(|_| {
        fs::copy(source, destination)?;
        fs::remove_file(source)?;
        Ok(())
    })
}

fn send_message(
    sender: &mpsc::UnboundedSender<LauncherMessage>,
    message: LauncherMessage,
) -> Result<()> {
    sender
        .send(message)
        .map_err(|_| anyhow!("Falha ao comunicar progresso do full map"))
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

#[cfg(test)]
mod tests {
    use super::{
        FullMapInstallRoot, download_and_install_full_minimap_from_url, full_map_archive_path,
        hash_file, install_full_minimap_from_zip, resolve_download_url, verify_hash,
    };
    use crate::tokio::sync::mpsc;
    use flate2::{Compression, write::ZlibEncoder};
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use zip::ZipWriter;

    #[test]
    fn strips_minimap_archive_prefixes() {
        assert_eq!(
            full_map_archive_path("minimap/Minimap_Color_0_0_7.png"),
            Some((
                FullMapInstallRoot::Minimap,
                PathBuf::from("Minimap_Color_0_0_7.png")
            ))
        );
        assert_eq!(
            full_map_archive_path("Penultima-Full-Minimap/minimap/Minimap_Color_0_0_7.png"),
            Some((
                FullMapInstallRoot::Minimap,
                PathBuf::from("Minimap_Color_0_0_7.png")
            ))
        );
        assert_eq!(
            full_map_archive_path("Minimap_Color_0_0_7.png"),
            Some((
                FullMapInstallRoot::Minimap,
                PathBuf::from("Minimap_Color_0_0_7.png")
            ))
        );
    }

    #[test]
    fn routes_map_assets_to_assets_directory() {
        assert_eq!(
            full_map_archive_path("assets/minimap-32-0001-0002-07-hash.bmp.lzma"),
            Some((
                FullMapInstallRoot::Assets,
                PathBuf::from("minimap-32-0001-0002-07-hash.bmp.lzma")
            ))
        );
        assert_eq!(
            full_map_archive_path("world/staticmapdata-hash.dat"),
            Some((
                FullMapInstallRoot::Assets,
                PathBuf::from("staticmapdata-hash.dat")
            ))
        );
        assert_eq!(
            full_map_archive_path("assets/subarea-0001-hash.bmp.lzma"),
            Some((
                FullMapInstallRoot::Assets,
                PathBuf::from("subarea-0001-hash.bmp.lzma")
            ))
        );
        assert_eq!(
            full_map_archive_path("assets/catalog-content.json"),
            Some((
                FullMapInstallRoot::Assets,
                PathBuf::from("catalog-content.json")
            ))
        );
    }

    #[test]
    fn rejects_unsafe_minimap_paths() {
        assert_eq!(full_map_archive_path("../escape.png"), None);
        assert_eq!(full_map_archive_path("minimap/../escape.png"), None);
        assert_eq!(full_map_archive_path("C:/escape.png"), None);
        assert_eq!(
            full_map_archive_path("minimap/Minimap_WaypointCost_0_0_7.png"),
            None
        );
        assert_eq!(full_map_archive_path("assets/not-a-map.txt"), None);
    }

    #[test]
    fn resolves_metadata_download_paths() {
        assert_eq!(
            resolve_download_url("downloads/Penultima-Full-Minimap.zip?sha256=abc").unwrap(),
            "https://ultimaotserv.online/downloads/Penultima-Full-Minimap.zip?sha256=abc"
        );
        assert_eq!(
            resolve_download_url("/downloads/Penultima-Full-Minimap.zip").unwrap(),
            "https://ultimaotserv.online/downloads/Penultima-Full-Minimap.zip"
        );
        assert_eq!(
            resolve_download_url("https://cdn.example/map.zip").unwrap(),
            "https://cdn.example/map.zip"
        );
        assert!(resolve_download_url(" ").is_err());
    }

    #[test]
    fn verifies_download_hash_case_insensitively() {
        let root = temp_dir("full-minimap-hash");
        let file_path = root.join("map.zip");
        fs::write(&file_path, b"full-map").unwrap();

        let hash = hash_file(&file_path).unwrap();
        verify_hash(&file_path, &hash.to_ascii_uppercase()).unwrap();
        assert!(verify_hash(&file_path, "0000").is_err());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn installs_full_minimap_into_game_minimap_folder() {
        let root = temp_dir("full-minimap-install");
        let archive_path = root.join("map.zip");
        let game_path = root.join("game");
        let assets_dir = game_path.join("assets");
        let minimap_dir = game_path.join("minimap");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::create_dir_all(&minimap_dir).unwrap();
        fs::write(minimap_dir.join("Minimap_Color_stale.png"), b"stale-color").unwrap();
        fs::write(
            minimap_dir.join("Minimap_WaypointCost_bad.png"),
            waypoint_cost_png(0),
        )
        .unwrap();
        fs::write(
            minimap_dir.join("Minimap_WaypointCost_keep.png"),
            waypoint_cost_png(1),
        )
        .unwrap();
        fs::write(minimap_dir.join("notes.txt"), b"keep").unwrap();
        fs::write(assets_dir.join("map-bad.dat"), b"bad-map").unwrap();
        fs::write(assets_dir.join("staticdata-bad.dat"), b"bad-static").unwrap();
        fs::write(assets_dir.join("minimap-bad.bmp.lzma"), b"bad-minimap").unwrap();
        fs::write(assets_dir.join("satellite-bad.bmp.lzma"), b"bad-satellite").unwrap();
        fs::write(assets_dir.join("subarea-bad.bmp.lzma"), b"bad-subarea").unwrap();
        fs::write(assets_dir.join("custom.txt"), b"keep").unwrap();
        create_zip(
            &archive_path,
            &[
                ("minimap/Minimap_Color_0_0_7.png", indexed_png_bytes()),
                (
                    "assets/minimap-32-0001-0002-07-hash.bmp.lzma",
                    b"asset-minimap".as_slice(),
                ),
                ("assets/map-good.dat", b"good-map".as_slice()),
                ("assets/staticdata-good.dat", b"good-static".as_slice()),
                ("assets/catalog-content.json", b"catalog".as_slice()),
                ("staticmapdata-hash.dat", b"static-map".as_slice()),
                ("../escape.txt", b"escape".as_slice()),
            ],
        );

        let stats = install_full_minimap_from_zip(&archive_path, &game_path, None).unwrap();

        assert_eq!(stats.files, 6);
        assert_eq!(
            fs::read(game_path.join("minimap").join("Minimap_Color_0_0_7.png")).unwrap(),
            indexed_png_bytes()
        );
        assert_eq!(
            fs::read(
                game_path
                    .join("assets")
                    .join("minimap-32-0001-0002-07-hash.bmp.lzma")
            )
            .unwrap(),
            b"asset-minimap"
        );
        assert_eq!(
            fs::read(game_path.join("assets").join("staticmapdata-hash.dat")).unwrap(),
            b"static-map"
        );
        assert_eq!(
            fs::read(game_path.join("assets").join("map-good.dat")).unwrap(),
            b"good-map"
        );
        assert_eq!(
            fs::read(game_path.join("assets").join("staticdata-good.dat")).unwrap(),
            b"good-static"
        );
        assert_eq!(
            fs::read(game_path.join("assets").join("catalog-content.json")).unwrap(),
            b"catalog"
        );
        assert!(!game_path.join("assets").join("map-bad.dat").exists());
        assert!(!game_path.join("assets").join("staticdata-bad.dat").exists());
        assert!(
            !game_path
                .join("assets")
                .join("minimap-bad.bmp.lzma")
                .exists()
        );
        assert!(
            !game_path
                .join("assets")
                .join("satellite-bad.bmp.lzma")
                .exists()
        );
        assert!(
            !game_path
                .join("assets")
                .join("subarea-bad.bmp.lzma")
                .exists()
        );
        assert_eq!(
            fs::read(game_path.join("assets").join("custom.txt")).unwrap(),
            b"keep"
        );
        assert!(
            !game_path
                .join("minimap")
                .join("Minimap_Color_stale.png")
                .exists()
        );
        assert!(
            !game_path
                .join("minimap")
                .join("Minimap_WaypointCost_bad.png")
                .exists()
        );
        assert!(
            game_path
                .join("minimap")
                .join("Minimap_WaypointCost_keep.png")
                .exists()
        );
        assert_eq!(
            fs::read(game_path.join("minimap").join("notes.txt")).unwrap(),
            b"keep"
        );
        assert!(!game_path.join("escape.txt").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn minimap_only_install_preserves_assets_and_valid_waypoint_cache() {
        let root = temp_dir("full-minimap-minimap-only");
        let archive_path = root.join("map.zip");
        let game_path = root.join("game");
        let assets_dir = game_path.join("assets");
        let minimap_dir = game_path.join("minimap");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::create_dir_all(&minimap_dir).unwrap();
        fs::write(assets_dir.join("map-existing.dat"), b"keep-map").unwrap();
        fs::write(
            minimap_dir.join("Minimap_WaypointCost_keep.png"),
            waypoint_cost_png(1),
        )
        .unwrap();
        fs::write(
            minimap_dir.join("Minimap_WaypointCost_bad.png"),
            waypoint_cost_png(0),
        )
        .unwrap();
        create_zip(
            &archive_path,
            &[("minimap/Minimap_Color_1_2_7.png", indexed_png_bytes())],
        );

        install_full_minimap_from_zip(&archive_path, &game_path, None).unwrap();

        assert_eq!(
            fs::read(assets_dir.join("map-existing.dat")).unwrap(),
            b"keep-map"
        );
        assert!(minimap_dir.join("Minimap_WaypointCost_keep.png").exists());
        assert!(!minimap_dir.join("Minimap_WaypointCost_bad.png").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn full_map_catalog_merge_preserves_existing_sprite_entries() {
        let root = temp_dir("full-minimap-catalog-merge");
        let archive_path = root.join("map.zip");
        let game_path = root.join("game");
        let assets_dir = game_path.join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(
            assets_dir.join("catalog-content.json"),
            r#"[
  {"type":"sprite","file":"sprites-installed.bmp.lzma"},
  {"type":"map","file":"map-bad.dat"},
  {"type":"staticdata","file":"staticdata-old.dat"},
  {"type":"appearances","file":"appearances-installed.dat"}
]"#,
        )
        .unwrap();
        create_zip(
            &archive_path,
            &[
                ("minimap/Minimap_Color_0_0_7.png", indexed_png_bytes()),
                (
                    "assets/catalog-content.json",
                    br#"[
  {"type":"sprite","file":"sprites-newer-missing.bmp.lzma"},
  {"type":"map","file":"map-good.dat"},
  {"type":"staticdata","file":"staticdata-good.dat"},
  {"type":"staticmapdata","file":"staticmapdata-good.dat"}
]"#,
                ),
                ("assets/map-good.dat", b"good-map".as_slice()),
                ("assets/staticdata-good.dat", b"good-static".as_slice()),
                (
                    "assets/staticmapdata-good.dat",
                    b"good-static-map".as_slice(),
                ),
            ],
        );

        install_full_minimap_from_zip(&archive_path, &game_path, None).unwrap();

        let catalog_raw = fs::read_to_string(assets_dir.join("catalog-content.json")).unwrap();
        let catalog = serde_json::from_str::<serde_json::Value>(&catalog_raw).unwrap();
        let entries = catalog.as_array().unwrap();
        assert!(entries.iter().any(|entry| {
            entry.get("type").and_then(|value| value.as_str()) == Some("sprite")
                && entry.get("file").and_then(|value| value.as_str())
                    == Some("sprites-installed.bmp.lzma")
        }));
        assert!(!entries.iter().any(|entry| {
            entry.get("file").and_then(|value| value.as_str())
                == Some("sprites-newer-missing.bmp.lzma")
        }));
        assert!(entries.iter().any(|entry| {
            entry.get("type").and_then(|value| value.as_str()) == Some("map")
                && entry.get("file").and_then(|value| value.as_str()) == Some("map-good.dat")
        }));
        assert!(entries.iter().any(|entry| {
            entry.get("type").and_then(|value| value.as_str()) == Some("staticdata")
                && entry.get("file").and_then(|value| value.as_str()) == Some("staticdata-good.dat")
        }));
        assert!(entries.iter().any(|entry| {
            entry.get("type").and_then(|value| value.as_str()) == Some("staticmapdata")
                && entry.get("file").and_then(|value| value.as_str())
                    == Some("staticmapdata-good.dat")
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn full_map_catalog_merge_uses_backup_when_current_references_missing_assets() {
        let root = temp_dir("full-minimap-catalog-backup");
        let archive_path = root.join("map.zip");
        let game_path = root.join("game");
        let assets_dir = game_path.join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(assets_dir.join("sprites-installed.bmp.lzma"), b"sprite").unwrap();
        fs::write(
            assets_dir.join("catalog-content.json"),
            r#"[
  {"type":"sprite","file":"sprites-newer-missing.bmp.lzma"},
  {"type":"map","file":"map-bad.dat"}
]"#,
        )
        .unwrap();
        fs::write(
            assets_dir.join("catalog-content.json-bak"),
            r#"[
  {"type":"sprite","file":"sprites-installed.bmp.lzma"},
  {"type":"map","file":"map-old.dat"}
]"#,
        )
        .unwrap();
        create_zip(
            &archive_path,
            &[
                ("minimap/Minimap_Color_0_0_7.png", indexed_png_bytes()),
                (
                    "assets/catalog-content.json",
                    br#"[
  {"type":"sprite","file":"sprites-newer-missing.bmp.lzma"},
  {"type":"map","file":"map-good.dat"}
]"#,
                ),
                ("assets/map-good.dat", b"good-map".as_slice()),
            ],
        );

        install_full_minimap_from_zip(&archive_path, &game_path, None).unwrap();

        let catalog_raw = fs::read_to_string(assets_dir.join("catalog-content.json")).unwrap();
        let catalog = serde_json::from_str::<serde_json::Value>(&catalog_raw).unwrap();
        let entries = catalog.as_array().unwrap();
        assert!(entries.iter().any(|entry| {
            entry.get("type").and_then(|value| value.as_str()) == Some("sprite")
                && entry.get("file").and_then(|value| value.as_str())
                    == Some("sprites-installed.bmp.lzma")
        }));
        assert!(!entries.iter().any(|entry| {
            entry.get("file").and_then(|value| value.as_str())
                == Some("sprites-newer-missing.bmp.lzma")
        }));
        assert!(entries.iter().any(|entry| {
            entry.get("type").and_then(|value| value.as_str()) == Some("map")
                && entry.get("file").and_then(|value| value.as_str()) == Some("map-good.dat")
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn downloads_and_installs_full_minimap_from_http() {
        let root = temp_dir("full-minimap-download");
        let source_zip = root.join("source.zip");
        let download_path = root.join("downloads");
        let game_path = root.join("game");
        create_zip(
            &source_zip,
            &[("minimap/Minimap_Color_1_2_7.png", indexed_png_bytes())],
        );
        let zip_bytes = fs::read(&source_zip).unwrap();
        let url = serve_zip_once(zip_bytes).await;
        let (tx, _rx) = mpsc::unbounded_channel();

        let stats =
            download_and_install_full_minimap_from_url(url, download_path, game_path.clone(), tx)
                .await
                .unwrap();

        assert_eq!(stats.files, 1);
        assert_eq!(
            fs::read(game_path.join("minimap").join("Minimap_Color_1_2_7.png")).unwrap(),
            indexed_png_bytes()
        );

        let _ = fs::remove_dir_all(root);
    }

    fn create_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, contents) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap();
    }

    fn indexed_png_bytes() -> &'static [u8] {
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x03\x00\x00\x00"
    }

    fn waypoint_cost_png(index: u8) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&[0, index]).unwrap();
        let idat = encoder.finish().unwrap();

        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        append_png_chunk(
            &mut png,
            b"IHDR",
            &[
                0, 0, 0, 1, // width
                0, 0, 0, 1, // height
                8, 3, 0, 0, 0,
            ],
        );
        append_png_chunk(&mut png, b"PLTE", &[0, 0, 0, 255, 255, 255]);
        append_png_chunk(&mut png, b"IDAT", &idat);
        append_png_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn append_png_chunk(png: &mut Vec<u8>, chunk_type: &[u8; 4], payload: &[u8]) {
        png.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        png.extend_from_slice(chunk_type);
        png.extend_from_slice(payload);
        png.extend_from_slice(&0u32.to_be_bytes());
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    async fn serve_zip_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer).await.unwrap();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/zip\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        });

        format!("http://{address}/Penultima-Full-Minimap.zip")
    }
}

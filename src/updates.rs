use anyhow::{Context, Result, anyhow};
use futures_util::{StreamExt, stream};
use log::info;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::client_version::ClientVersionManager;
use crate::constants::{
    CLIENT_ASSET_MANIFEST_HASH_URL, CLIENT_ASSET_MANIFEST_URL, CLIENT_GITHUB_ARCHIVE_URL,
    CLIENT_GITHUB_RAW_BASE_URL, CLIENT_PACKAGE_MANIFEST_URL, CLIENT_PACKAGE_VERSION_URL,
    HTTP_DOWNLOAD_TIMEOUT, HTTP_REQUEST_TIMEOUT,
};
use crate::message_system::LauncherMessage;
use crate::tokio::sync::mpsc;
use std::time::{Duration, Instant};
use zip::ZipArchive;

const BULK_ARCHIVE_FILE_THRESHOLD: usize = 1_500;
const BULK_ARCHIVE_BYTE_THRESHOLD: u64 = 128 * 1024 * 1024;
const MANIFEST_DOWNLOAD_CONCURRENCY: usize = 16;
const MANIFEST_DOWNLOAD_PROGRESS_END: f32 = 0.98;
const ARCHIVE_DOWNLOAD_PROGRESS_END: f32 = 0.82;
const ARCHIVE_EXTRACTION_PROGRESS_END: f32 = 0.98;
const STATUS_REPORT_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Deserialize)]
struct PackageManifest {
    version: String,
    #[serde(default)]
    files: Vec<PackageFile>,
}

#[derive(Clone, Debug, Deserialize)]
struct PackageFile {
    url: String,
    localfile: String,
    #[serde(default)]
    packedhash: Option<String>,
    #[serde(default)]
    packedsize: Option<u64>,
    #[serde(default)]
    unpackedhash: Option<String>,
    #[serde(default)]
    unpackedsize: Option<u64>,
    #[serde(default)]
    unpack: Option<bool>,
    #[serde(default)]
    bootstrap_only: bool,
}

impl PackageFile {
    fn should_unpack(&self) -> bool {
        self.unpack.unwrap_or(self.url.ends_with(".lzma"))
    }

    fn target_path(&self, game_path: &Path) -> PathBuf {
        game_path.join(&self.localfile)
    }

    fn manifest_matches(&self, previous: &PackageFile) -> bool {
        self.url == previous.url
            && self.localfile == previous.localfile
            && self.packedhash == previous.packedhash
            && self.packedsize == previous.packedsize
            && self.unpackedhash == previous.unpackedhash
            && self.unpackedsize == previous.unpackedsize
            && self.unpack == previous.unpack
            && self.bootstrap_only == previous.bootstrap_only
    }

    fn expected_local_size(&self) -> Option<u64> {
        if self.should_unpack() {
            self.unpackedsize
        } else {
            self.packedsize
        }
    }
}

struct RemoteMetadata {
    package_raw: String,
    package_manifest: PackageManifest,
    package_version: String,
    assets_raw: String,
    assets_hash: String,
}

pub struct UpdateManager {
    download_path: PathBuf,
    game_path: PathBuf,
    state_path: PathBuf,
}

impl UpdateManager {
    pub fn new(download_path: PathBuf, game_path: PathBuf, state_path: PathBuf) -> Self {
        Self {
            download_path,
            game_path,
            state_path,
        }
    }

    pub fn load_current_version(state_path: &PathBuf, game_path: &PathBuf) -> Result<String> {
        if let Some(version) = read_metadata_file(state_path, game_path, "package.json.version")? {
            return Ok(version.trim().to_string());
        }

        if let Some(version) = read_metadata_file(state_path, game_path, "version.txt")? {
            return Ok(version.trim().to_string());
        }

        if let Some(manifest_raw) = read_metadata_file(state_path, game_path, "package.json")? {
            let manifest: PackageManifest = serde_json::from_str(&manifest_raw)?;
            return Ok(manifest.version);
        }

        Ok("0.0.0".to_string())
    }

    pub async fn check_initial_updates(
        game_path: &PathBuf,
        state_path: &PathBuf,
    ) -> Result<bool, reqwest::Error> {
        info!("Verificando cliente declarado em: {:?}", game_path);

        if let Err(error) = fs::create_dir_all(game_path) {
            info!("Falha ao garantir diretorio do jogo: {}", error);
            return Ok(true);
        }

        let client_exists = game_path.join("bin").join("client.exe").exists();
        if !client_exists {
            info!("client.exe nao encontrado. Atualizacao necessaria.");
            return Ok(true);
        }

        let (package_raw, remote_assets_hash) = match tokio::try_join!(
            fetch_text(CLIENT_PACKAGE_MANIFEST_URL),
            fetch_text(CLIENT_ASSET_MANIFEST_HASH_URL)
        ) {
            Ok(result) => result,
            Err(error) => {
                info!("Falha ao obter manifestos remotos: {}", error);
                return Ok(false);
            }
        };

        let local_package =
            read_metadata_file(state_path, game_path, "package.json").unwrap_or_default();
        let local_assets_hash =
            read_metadata_file(state_path, game_path, "assets.json.sha256").unwrap_or_default();

        Ok(
            local_package.unwrap_or_default().trim() != package_raw.trim()
                || local_assets_hash.unwrap_or_default().trim() != remote_assets_hash.trim(),
        )
    }

    pub async fn check_for_updates(
        &self,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
        disable_auto_start: bool,
    ) -> Result<()> {
        self.run_update(message_sender, disable_auto_start, false)
            .await
    }

    pub async fn force_refresh(
        &self,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
        disable_auto_start: bool,
    ) -> Result<()> {
        self.run_update(message_sender, disable_auto_start, true)
            .await
    }

    async fn run_update(
        &self,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
        disable_auto_start: bool,
        force: bool,
    ) -> Result<()> {
        send_message(
            &message_sender,
            LauncherMessage::SetStatus("Verificando arquivos do cliente...".to_string()),
        )?;
        send_message(&message_sender, LauncherMessage::SetProcessing(true))?;
        send_message(&message_sender, LauncherMessage::DownloadProgress(0.0))?;

        let remote = self.fetch_remote_metadata().await?;
        let download_client = reqwest::Client::builder()
            .timeout(HTTP_DOWNLOAD_TIMEOUT)
            .build()
            .context("Falha ao inicializar cliente HTTP do updater")?;

        let local_package = read_metadata_file(&self.state_path, &self.game_path, "package.json")?
            .unwrap_or_default();
        let local_assets_hash =
            read_metadata_file(&self.state_path, &self.game_path, "assets.json.sha256")?
                .unwrap_or_default();

        let package_changed = force || local_package.trim() != remote.package_raw.trim();
        let assets_changed = force || local_assets_hash.trim() != remote.assets_hash.trim();
        let has_local_sync_state =
            !local_package.trim().is_empty() && !local_assets_hash.trim().is_empty();

        let files_to_update = if force || package_changed {
            self.collect_changed_files(&remote.package_manifest, force)?
        } else {
            Vec::new()
        };
        let use_archive_install =
            self.should_use_archive_install(&files_to_update, force, has_local_sync_state);

        if files_to_update.is_empty() && !assets_changed {
            info!("Cliente ja esta sincronizado com o manifesto remoto");
            self.persist_metadata(&remote)?;
            self.refresh_versions(&message_sender, &remote.package_version)?;
            send_message(
                &message_sender,
                LauncherMessage::SetStatus(format!(
                    "Cliente ja esta atualizado ({})",
                    remote.package_version
                )),
            )?;
            send_message(&message_sender, LauncherMessage::DownloadProgress(1.0))?;
            send_message(&message_sender, LauncherMessage::SetProcessing(false))?;
            return Ok(());
        }

        if files_to_update.is_empty() {
            send_message(
                &message_sender,
                LauncherMessage::SetStatus("Sincronizando manifestos do cliente...".to_string()),
            )?;
        } else {
            send_message(
                &message_sender,
                LauncherMessage::SetStatus(format!(
                    "Atualizando {} arquivo(s) do cliente...",
                    files_to_update.len()
                )),
            )?;
        }

        if use_archive_install {
            self.install_from_archive(&download_client, &message_sender, &remote.package_manifest)
                .await?;
        } else {
            self.download_manifest_files(&download_client, &files_to_update, &message_sender)
                .await?;
        }

        self.persist_metadata(&remote)?;
        self.refresh_versions(&message_sender, &remote.package_version)?;

        send_message(&message_sender, LauncherMessage::DownloadProgress(1.0))?;
        send_message(
            &message_sender,
            LauncherMessage::SetStatus("Atualizacao concluida. Pronto para jogar.".to_string()),
        )?;
        send_message(&message_sender, LauncherMessage::SetProcessing(false))?;
        send_message(&message_sender, LauncherMessage::DownloadComplete)?;

        if !disable_auto_start {
            send_message(&message_sender, LauncherMessage::LaunchGame)?;
        }

        Ok(())
    }

    async fn fetch_remote_metadata(&self) -> Result<RemoteMetadata> {
        let (package_raw, package_version, assets_raw, assets_hash) = tokio::try_join!(
            fetch_text(CLIENT_PACKAGE_MANIFEST_URL),
            fetch_text(CLIENT_PACKAGE_VERSION_URL),
            fetch_text(CLIENT_ASSET_MANIFEST_URL),
            fetch_text(CLIENT_ASSET_MANIFEST_HASH_URL)
        )
        .context("Falha ao baixar metadados remotos do cliente")?;

        let package_manifest: PackageManifest =
            serde_json::from_str(&package_raw).context("package.json remoto invalido")?;

        Ok(RemoteMetadata {
            package_raw,
            package_manifest,
            package_version: package_version.trim().to_string(),
            assets_raw,
            assets_hash: assets_hash.trim().to_string(),
        })
    }

    fn collect_changed_files(
        &self,
        manifest: &PackageManifest,
        force: bool,
    ) -> Result<Vec<PackageFile>> {
        let mut changed_files = Vec::new();
        let previous_manifest = if force {
            None
        } else {
            self.load_local_manifest()?
        };
        let previous_files: HashMap<String, PackageFile> = previous_manifest
            .map(|manifest| {
                manifest
                    .files
                    .into_iter()
                    .map(|file| (file.localfile.clone(), file))
                    .collect()
            })
            .unwrap_or_default();

        for file in &manifest.files {
            if force && file.bootstrap_only && file.target_path(&self.game_path).exists() {
                info!(
                    "Preservando arquivo bootstrap existente durante atualizacao forcada: {}",
                    file.localfile
                );
                continue;
            }

            if force || self.file_needs_update_fast(file, previous_files.get(&file.localfile))? {
                changed_files.push(file.clone());
            }
        }

        Ok(changed_files)
    }

    fn should_use_archive_install(
        &self,
        files_to_update: &[PackageFile],
        force: bool,
        has_local_sync_state: bool,
    ) -> bool {
        if files_to_update.is_empty() {
            return false;
        }

        let client_missing = !self.game_path.join("bin").join("client.exe").exists();
        let total_download_bytes = files_to_update
            .iter()
            .map(|file| file.packedsize.unwrap_or_default())
            .sum::<u64>();

        force
            || client_missing
            || !has_local_sync_state
            || files_to_update.len() >= BULK_ARCHIVE_FILE_THRESHOLD
            || total_download_bytes >= BULK_ARCHIVE_BYTE_THRESHOLD
    }

    fn load_local_manifest(&self) -> Result<Option<PackageManifest>> {
        let Some(manifest_raw) =
            read_metadata_file(&self.state_path, &self.game_path, "package.json")?
        else {
            return Ok(None);
        };

        let manifest: PackageManifest =
            serde_json::from_str(&manifest_raw).context("package.json local invalido")?;
        Ok(Some(manifest))
    }

    fn file_needs_update_fast(
        &self,
        file: &PackageFile,
        previous_file: Option<&PackageFile>,
    ) -> Result<bool> {
        let target_path = file.target_path(&self.game_path);
        if file.bootstrap_only {
            return Ok(!target_path.exists());
        }
        if !target_path.exists() {
            return Ok(true);
        }

        if let Some(previous_file) = previous_file {
            if file.manifest_matches(previous_file) {
                if let Some(expected_size) = file.expected_local_size() {
                    let current_size = target_path
                        .metadata()
                        .map(|meta| meta.len())
                        .unwrap_or_default();
                    if current_size != expected_size {
                        return Ok(true);
                    }
                }

                return Ok(false);
            }
        }

        self.file_needs_update(file)
    }

    fn file_needs_update(&self, file: &PackageFile) -> Result<bool> {
        let target_path = file.target_path(&self.game_path);
        if file.bootstrap_only {
            return Ok(!target_path.exists());
        }
        if !target_path.exists() {
            return Ok(true);
        }

        if file.should_unpack() {
            if let Some(expected_size) = file.unpackedsize {
                if target_path
                    .metadata()
                    .map(|meta| meta.len())
                    .unwrap_or_default()
                    != expected_size
                {
                    return Ok(true);
                }
            }

            if let Some(expected_hash) = &file.unpackedhash {
                return Ok(hash_file(&target_path)? != expected_hash.to_ascii_lowercase());
            }
        } else {
            if let Some(expected_size) = file.packedsize {
                if target_path
                    .metadata()
                    .map(|meta| meta.len())
                    .unwrap_or_default()
                    != expected_size
                {
                    return Ok(true);
                }
            }

            if let Some(expected_hash) = &file.packedhash {
                return Ok(hash_file(&target_path)? != expected_hash.to_ascii_lowercase());
            }
        }

        Ok(false)
    }

    async fn download_manifest_files(
        &self,
        http_client: &reqwest::Client,
        files_to_update: &[PackageFile],
        message_sender: &mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<()> {
        let total = files_to_update.len();
        if total == 0 {
            return Ok(());
        }

        send_message(
            message_sender,
            LauncherMessage::SetStatus(format!(
                "Baixando arquivos do cliente em paralelo ({MANIFEST_DOWNLOAD_CONCURRENCY} conexoes)..."
            )),
        )?;

        let started_at = Instant::now();
        let mut completed = 0usize;
        let mut last_report_at = Instant::now()
            .checked_sub(STATUS_REPORT_INTERVAL)
            .unwrap_or_else(Instant::now);
        let mut downloads = stream::iter(files_to_update.iter().cloned().map(|file| async move {
            let file_name = display_file_name(&file.localfile);
            self.download_manifest_file(http_client, &file)
                .await
                .with_context(|| format!("Falha ao atualizar {}", file.localfile))?;
            Ok::<String, anyhow::Error>(file_name)
        }))
        .buffer_unordered(MANIFEST_DOWNLOAD_CONCURRENCY);

        while let Some(result) = downloads.next().await {
            let file_name = result?;
            completed += 1;

            let should_report =
                last_report_at.elapsed() >= STATUS_REPORT_INTERVAL || completed == total;
            if should_report {
                let elapsed_secs = started_at.elapsed().as_secs_f64().max(0.1);
                let files_per_second = completed as f64 / elapsed_secs;
                let progress = (completed as f32 / total as f32) * MANIFEST_DOWNLOAD_PROGRESS_END;

                send_message(
                    message_sender,
                    LauncherMessage::SetStatus(format!(
                        "Atualizando arquivos do cliente... {completed}/{total} ({files_per_second:.1} arquivos/s, ultimo: {file_name})"
                    )),
                )?;
                send_message(
                    message_sender,
                    LauncherMessage::DownloadProgress(progress.min(MANIFEST_DOWNLOAD_PROGRESS_END)),
                )?;
                last_report_at = Instant::now();
            }
        }

        Ok(())
    }

    async fn download_manifest_file(
        &self,
        http_client: &reqwest::Client,
        file: &PackageFile,
    ) -> Result<()> {
        let target_path = file.target_path(&self.game_path);
        let packed_temp_path = temporary_path(
            &target_path,
            if file.should_unpack() {
                "packed"
            } else {
                "download"
            },
        );
        let unpacked_temp_path = temporary_path(&target_path, "part");

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Falha ao criar diretorio {}", parent.display()))?;
        }

        download_to_path(http_client, &build_raw_url(&file.url), &packed_temp_path).await?;

        if file.should_unpack() {
            if let Some(expected_hash) = &file.packedhash {
                verify_hash(&packed_temp_path, expected_hash)?;
            }

            if unpacked_temp_path.exists() {
                fs::remove_file(&unpacked_temp_path)?;
            }

            let mut packed_file = BufReader::new(
                File::open(&packed_temp_path)
                    .with_context(|| format!("Falha ao abrir {}", packed_temp_path.display()))?,
            );
            let mut unpacked_file = File::create(&unpacked_temp_path).with_context(|| {
                format!(
                    "Falha ao criar arquivo temporario {}",
                    unpacked_temp_path.display()
                )
            })?;
            lzma_rs::lzma_decompress(&mut packed_file, &mut unpacked_file)
                .context("Falha ao descompactar arquivo LZMA")?;
            unpacked_file.flush()?;

            if let Some(expected_hash) = &file.unpackedhash {
                verify_hash(&unpacked_temp_path, expected_hash)?;
            }

            replace_file(&unpacked_temp_path, &target_path)?;
            if packed_temp_path.exists() {
                fs::remove_file(&packed_temp_path)?;
            }
        } else {
            if let Some(expected_hash) = &file.packedhash {
                verify_hash(&packed_temp_path, expected_hash)?;
            }

            replace_file(&packed_temp_path, &target_path)?;
        }

        Ok(())
    }

    async fn install_from_archive(
        &self,
        http_client: &reqwest::Client,
        message_sender: &mpsc::UnboundedSender<LauncherMessage>,
        manifest: &PackageManifest,
    ) -> Result<()> {
        fs::create_dir_all(&self.state_path)?;
        fs::create_dir_all(&self.game_path)?;

        let archive_path = self.state_path.join("client-feed.bootstrap.zip");

        send_message(
            message_sender,
            LauncherMessage::SetStatus("Baixando pacote completo do cliente...".to_string()),
        )?;

        download_to_path_with_progress(
            http_client,
            CLIENT_GITHUB_ARCHIVE_URL,
            &archive_path,
            Some(message_sender),
            "Baixando pacote completo do cliente",
            0.0,
            ARCHIVE_DOWNLOAD_PROGRESS_END,
        )
        .await?;

        send_message(
            message_sender,
            LauncherMessage::SetStatus("Extraindo pacote completo do cliente...".to_string()),
        )?;

        let extraction_result = extract_client_archive(
            &archive_path,
            &self.game_path,
            message_sender,
            &archive_preserved_bootstrap_paths(manifest, &self.game_path),
            manifest,
        );

        if archive_path.exists() {
            let _ = fs::remove_file(&archive_path);
        }

        extraction_result
    }

    fn persist_metadata(&self, remote: &RemoteMetadata) -> Result<()> {
        fs::create_dir_all(&self.state_path)?;
        fs::write(self.package_manifest_path(), &remote.package_raw)?;
        fs::write(
            self.package_version_path(),
            format!("{}\n", remote.package_version),
        )?;
        fs::write(self.asset_manifest_path(), &remote.assets_raw)?;
        fs::write(
            self.asset_manifest_hash_path(),
            format!("{}\n", remote.assets_hash),
        )?;
        fs::write(
            self.state_path.join("version.txt"),
            format!("{}\n", remote.package_version),
        )?;
        self.remove_legacy_metadata_files()?;
        Ok(())
    }

    fn refresh_versions(
        &self,
        message_sender: &mpsc::UnboundedSender<LauncherMessage>,
        version: &str,
    ) -> Result<()> {
        send_message(
            message_sender,
            LauncherMessage::VersionUpdated(version.to_string()),
        )?;

        if let Some(client_version) =
            ClientVersionManager::load_client_version(&self.download_path, &self.game_path)
        {
            send_message(
                message_sender,
                LauncherMessage::ClientVersionUpdated(client_version),
            )?;
        } else {
            send_message(
                message_sender,
                LauncherMessage::ClientVersionUpdated(version.to_string()),
            )?;
        }

        Ok(())
    }

    fn package_manifest_path(&self) -> PathBuf {
        self.state_path.join("package.json")
    }

    fn package_version_path(&self) -> PathBuf {
        self.state_path.join("package.json.version")
    }

    fn asset_manifest_path(&self) -> PathBuf {
        self.state_path.join("assets.json")
    }

    fn asset_manifest_hash_path(&self) -> PathBuf {
        self.state_path.join("assets.json.sha256")
    }

    fn remove_legacy_metadata_files(&self) -> Result<()> {
        for file_name in [
            "package.json",
            "package.json.version",
            "assets.json",
            "assets.json.sha256",
            "version.txt",
        ] {
            let legacy_path = self.game_path.join(file_name);
            if legacy_path.exists() {
                let _ = fs::remove_file(legacy_path);
            }
        }
        Ok(())
    }
}

fn read_metadata_file(
    state_path: &Path,
    game_path: &Path,
    file_name: &str,
) -> Result<Option<String>> {
    for candidate in [state_path.join(file_name), game_path.join(file_name)] {
        if candidate.exists() {
            return Ok(Some(fs::read_to_string(candidate)?));
        }
    }

    Ok(None)
}

fn send_message(
    sender: &mpsc::UnboundedSender<LauncherMessage>,
    message: LauncherMessage,
) -> Result<()> {
    sender
        .send(message)
        .map_err(|error| anyhow!("Falha ao enviar mensagem para a UI: {}", error))
}

async fn fetch_text(url: &str) -> Result<String, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(HTTP_REQUEST_TIMEOUT)
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await
}

async fn download_to_path(
    http_client: &reqwest::Client,
    url: &str,
    destination: &Path,
) -> Result<()> {
    download_to_path_with_progress(http_client, url, destination, None, "", 0.0, 0.0).await
}

async fn download_to_path_with_progress(
    http_client: &reqwest::Client,
    url: &str,
    destination: &Path,
    message_sender: Option<&mpsc::UnboundedSender<LauncherMessage>>,
    status_prefix: &str,
    progress_start: f32,
    progress_end: f32,
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

        if let Some(sender) = message_sender {
            let should_report = last_report_at.elapsed() >= STATUS_REPORT_INTERVAL
                || total_bytes == Some(downloaded_bytes);
            if should_report {
                report_download_progress(
                    sender,
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
    }

    file.flush()?;

    if let Some(sender) = message_sender {
        report_download_progress(
            sender,
            status_prefix,
            downloaded_bytes,
            total_bytes,
            started_at,
            progress_start,
            progress_end,
        )?;
    }

    Ok(())
}

fn build_raw_url(relative_path: &str) -> String {
    format!(
        "{}/{}",
        CLIENT_GITHUB_RAW_BASE_URL.trim_end_matches('/'),
        relative_path.replace('\\', "/")
    )
}

fn temporary_path(target_path: &Path, suffix: &str) -> PathBuf {
    let file_name = target_path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    target_path.with_file_name(format!("{file_name}.{suffix}.tmp"))
}

fn display_file_name(path: &str) -> String {
    path.replace('\\', "/")
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn extract_client_archive(
    archive_path: &Path,
    game_path: &Path,
    message_sender: &mpsc::UnboundedSender<LauncherMessage>,
    preserved_paths: &HashSet<String>,
    manifest: &PackageManifest,
) -> Result<()> {
    let archive_file = File::open(archive_path)
        .with_context(|| format!("Falha ao abrir {}", archive_path.display()))?;
    let mut archive =
        ZipArchive::new(archive_file).context("Falha ao ler o pacote ZIP do cliente")?;

    let manifest_files_by_url: HashMap<String, PackageFile> = manifest
        .files
        .iter()
        .cloned()
        .map(|file| (normalize_manifest_path(&file.url), file))
        .collect();
    let mut relevant_entries = Vec::new();
    let mut found_urls = HashSet::new();
    let mut total_bytes = 0u64;

    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .with_context(|| format!("Falha ao ler entrada {} do ZIP", index))?;
        if entry.is_dir() {
            continue;
        }

        let Some(relative_path) = archive_relative_path(entry.name()) else {
            continue;
        };
        let archive_key = archive_path_key(&relative_path);
        let Some(file) = manifest_files_by_url.get(&archive_key) else {
            continue;
        };
        if preserved_paths.contains(&normalize_manifest_path(&file.localfile)) {
            continue;
        }

        let destination = file.target_path(game_path);
        total_bytes += entry.size();
        found_urls.insert(archive_key);
        relevant_entries.push((
            index,
            relative_path,
            destination,
            entry.size(),
            file.clone(),
        ));
    }

    let missing_file_count = manifest
        .files
        .iter()
        .filter(|file| !preserved_paths.contains(&normalize_manifest_path(&file.localfile)))
        .filter(|file| !found_urls.contains(&normalize_manifest_path(&file.url)))
        .count();
    let missing_files: Vec<String> = manifest
        .files
        .iter()
        .filter(|file| !preserved_paths.contains(&normalize_manifest_path(&file.localfile)))
        .filter(|file| !found_urls.contains(&normalize_manifest_path(&file.url)))
        .take(5)
        .map(|file| file.url.clone())
        .collect();

    if !missing_files.is_empty() {
        return Err(anyhow!(
            "O pacote completo do cliente nao contem {} arquivo(s) do manifesto; primeiros: {}",
            missing_file_count,
            missing_files.join(", ")
        ));
    }

    if relevant_entries.is_empty() {
        return Err(anyhow!(
            "O pacote completo do cliente nao contem arquivos instalaveis"
        ));
    }

    let mut extracted_bytes = 0u64;
    let mut created_directories = HashSet::new();
    let mut last_report_at = Instant::now()
        .checked_sub(STATUS_REPORT_INTERVAL)
        .unwrap_or_else(Instant::now);

    for (position, (index, relative_path, destination, entry_size, file)) in
        relevant_entries.iter().enumerate()
    {
        let mut entry = archive
            .by_index(*index)
            .with_context(|| format!("Falha ao reabrir entrada {} do ZIP", index))?;

        if let Some(parent) = destination.parent() {
            if created_directories.insert(parent.to_path_buf()) {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Falha ao criar diretorio {}", parent.display()))?;
            }
        }

        let temp_path = temporary_path(destination, "archive");
        if temp_path.exists() {
            fs::remove_file(&temp_path)?;
        }

        if file.should_unpack() {
            let output = BufWriter::with_capacity(
                256 * 1024,
                File::create(&temp_path)
                    .with_context(|| format!("Falha ao criar {}", temp_path.display()))?,
            );
            let mut output = HashingWriter::new(output);
            let mut packed_reader = BufReader::new(&mut entry);
            lzma_rs::lzma_decompress(&mut packed_reader, &mut output).with_context(|| {
                format!(
                    "Falha ao descompactar {} para {}",
                    relative_path.display(),
                    destination.display()
                )
            })?;
            output.flush()?;
            let (output, actual_hash, actual_size) = output.finish();
            drop(output);

            verify_size_value(destination, file.unpackedsize, actual_size)?;
            if let Some(expected_hash) = &file.unpackedhash {
                verify_hash_value(destination, expected_hash, &actual_hash)?;
            }
        } else {
            let output = BufWriter::with_capacity(
                256 * 1024,
                File::create(&temp_path)
                    .with_context(|| format!("Falha ao criar {}", temp_path.display()))?,
            );
            let mut output = HashingWriter::new(output);
            std::io::copy(&mut entry, &mut output).with_context(|| {
                format!(
                    "Falha ao extrair {} para {}",
                    relative_path.display(),
                    destination.display()
                )
            })?;
            output.flush()?;
            let (output, actual_hash, actual_size) = output.finish();
            drop(output);

            verify_size_value(destination, file.packedsize, actual_size)?;
            if let Some(expected_hash) = &file.packedhash {
                verify_hash_value(destination, expected_hash, &actual_hash)?;
            }
        }

        replace_file(&temp_path, destination)?;
        extracted_bytes += *entry_size;

        let should_report = last_report_at.elapsed() >= STATUS_REPORT_INTERVAL
            || position + 1 == relevant_entries.len();
        if should_report {
            let progress = if total_bytes == 0 {
                ARCHIVE_EXTRACTION_PROGRESS_END
            } else {
                let fraction = extracted_bytes as f32 / total_bytes as f32;
                ARCHIVE_DOWNLOAD_PROGRESS_END
                    + fraction.clamp(0.0, 1.0)
                        * (ARCHIVE_EXTRACTION_PROGRESS_END - ARCHIVE_DOWNLOAD_PROGRESS_END)
            };

            send_message(
                message_sender,
                LauncherMessage::SetStatus(format!(
                    "Extraindo pacote completo do cliente... {}/{} arquivos",
                    position + 1,
                    relevant_entries.len()
                )),
            )?;
            send_message(
                message_sender,
                LauncherMessage::DownloadProgress(progress.min(ARCHIVE_EXTRACTION_PROGRESS_END)),
            )?;
            last_report_at = Instant::now();
        }
    }

    Ok(())
}

fn archive_preserved_bootstrap_paths(
    manifest: &PackageManifest,
    game_path: &Path,
) -> HashSet<String> {
    manifest
        .files
        .iter()
        .filter(|file| file.bootstrap_only && file.target_path(game_path).exists())
        .map(|file| normalize_manifest_path(&file.localfile))
        .collect()
}

fn archive_relative_path(entry_name: &str) -> Option<PathBuf> {
    let normalized = entry_name.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }

    let payload_starts_at_root = matches!(
        parts.first().copied(),
        Some(
            "assets"
                | "bin"
                | "conf"
                | "sounds"
                | "package.json"
                | "package.json.version"
                | "assets.json"
                | "assets.json.sha256"
        )
    );

    let start_index = if payload_starts_at_root { 0 } else { 1 };
    if start_index >= parts.len() {
        return None;
    }

    let mut relative_path = PathBuf::new();
    for part in &parts[start_index..] {
        if *part == "." {
            continue;
        }
        if *part == ".." {
            return None;
        }
        relative_path.push(part);
    }

    if relative_path.as_os_str().is_empty() {
        None
    } else {
        Some(relative_path)
    }
}

fn archive_path_key(relative_path: &Path) -> String {
    normalize_manifest_path(&relative_path.to_string_lossy())
}

fn normalize_manifest_path(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

#[cfg(test)]
fn archive_destination_for(
    relative_path: &Path,
    game_path: &Path,
    state_path: &Path,
) -> Option<PathBuf> {
    let normalized = relative_path.to_string_lossy().replace('\\', "/");

    match normalized.as_str() {
        "package.json" | "package.json.version" | "assets.json" | "assets.json.sha256" => {
            Some(state_path.join(relative_path.file_name()?))
        }
        _ if normalized.starts_with("assets/")
            || normalized.starts_with("bin/")
            || normalized.starts_with("conf/")
            || normalized.starts_with("sounds/") =>
        {
            Some(game_path.join(relative_path))
        }
        _ => None,
    }
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
    if status_prefix.is_empty() {
        return Ok(());
    }

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

struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
    bytes_written: u64,
}

impl<W> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            bytes_written: 0,
        }
    }

    fn finish(self) -> (W, String, u64) {
        (
            self.inner,
            bytes_to_hex(&self.hasher.finalize()),
            self.bytes_written,
        )
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.hasher.update(&buffer[..written]);
        self.bytes_written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn verify_size_value(path: &Path, expected_size: Option<u64>, actual_size: u64) -> Result<()> {
    if let Some(expected_size) = expected_size {
        if actual_size != expected_size {
            return Err(anyhow!(
                "Tamanho invalido para {} (esperado {}, obtido {})",
                path.display(),
                expected_size,
                actual_size
            ));
        }
    }

    Ok(())
}

fn verify_hash_value(path: &Path, expected_hash: &str, actual_hash: &str) -> Result<()> {
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

fn verify_hash(path: &Path, expected_hash: &str) -> Result<()> {
    let actual_hash = hash_file(path)?;
    verify_hash_value(path, expected_hash, &actual_hash)
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

#[cfg(test)]
mod tests {
    use super::{
        BULK_ARCHIVE_BYTE_THRESHOLD, PackageFile, PackageManifest, UpdateManager,
        archive_destination_for, archive_relative_path, extract_client_archive,
    };
    use crate::tokio::sync::mpsc;
    use std::collections::HashSet;
    use std::fs;
    use std::io::{Cursor, Write};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    #[test]
    fn compressed_files_unpack_by_default() {
        let file = PackageFile {
            url: "bin/client.exe.lzma".to_string(),
            localfile: "bin/client.exe".to_string(),
            packedhash: None,
            packedsize: None,
            unpackedhash: None,
            unpackedsize: None,
            unpack: None,
            bootstrap_only: false,
        };

        assert!(file.should_unpack());
    }

    #[test]
    fn explicit_unpack_false_is_respected() {
        let file = PackageFile {
            url: "sounds/catalog-sound.json".to_string(),
            localfile: "sounds/catalog-sound.json".to_string(),
            packedhash: None,
            packedsize: None,
            unpackedhash: None,
            unpackedsize: None,
            unpack: Some(false),
            bootstrap_only: false,
        };

        assert!(!file.should_unpack());
    }

    #[test]
    fn archive_relative_path_strips_top_level_directory() {
        let relative =
            archive_relative_path("Vavaasz-penultima-client-123/assets/test.dat").unwrap();
        assert_eq!(relative, PathBuf::from("assets").join("test.dat"));
    }

    #[test]
    fn archive_relative_path_accepts_archives_without_top_level_directory() {
        let relative = archive_relative_path("assets/test.dat").unwrap();
        assert_eq!(relative, PathBuf::from("assets").join("test.dat"));
    }

    #[test]
    fn archive_relative_path_accepts_conf_without_top_level_directory() {
        let relative = archive_relative_path("conf/config.ini").unwrap();
        assert_eq!(relative, PathBuf::from("conf").join("config.ini"));
    }

    #[test]
    fn archive_destination_routes_metadata_to_state_dir() {
        let destination = archive_destination_for(
            Path::new("package.json"),
            Path::new("D:/game"),
            Path::new("D:/state"),
        )
        .unwrap();
        assert_eq!(destination, PathBuf::from("D:/state").join("package.json"));
    }

    #[test]
    fn archive_destination_routes_runtime_files_to_game_dir() {
        let destination = archive_destination_for(
            Path::new("bin/client.exe"),
            Path::new("D:/game"),
            Path::new("D:/state"),
        )
        .unwrap();
        assert_eq!(
            destination,
            PathBuf::from("D:/game").join("bin").join("client.exe")
        );
    }

    #[test]
    fn archive_destination_routes_conf_files_to_game_dir() {
        let destination = archive_destination_for(
            Path::new("conf/config.ini"),
            Path::new("D:/game"),
            Path::new("D:/state"),
        )
        .unwrap();
        assert_eq!(
            destination,
            PathBuf::from("D:/game").join("conf").join("config.ini")
        );
    }

    #[test]
    fn archive_extract_decompresses_manifest_lzma_entries() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("penultima-launcher-test-{unique}"));
        let game_path = root.join("game");
        let archive_path = root.join("client.zip");
        fs::create_dir_all(&root).unwrap();

        let client_bytes = b"fast-client-binary";
        let mut packed_client = Vec::new();
        lzma_rs::lzma_compress(&mut Cursor::new(client_bytes), &mut packed_client).unwrap();

        let archive_file = fs::File::create(&archive_path).unwrap();
        let mut archive = ZipWriter::new(archive_file);
        archive
            .start_file(
                "penultima-client-main/bin/client.exe.lzma",
                SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(&packed_client).unwrap();
        archive.finish().unwrap();

        let manifest = PackageManifest {
            version: "test".to_string(),
            files: vec![PackageFile {
                url: "bin/client.exe.lzma".to_string(),
                localfile: "bin/client.exe".to_string(),
                packedhash: None,
                packedsize: Some(packed_client.len() as u64),
                unpackedhash: None,
                unpackedsize: Some(client_bytes.len() as u64),
                unpack: None,
                bootstrap_only: false,
            }],
        };
        let (sender, _receiver) = mpsc::unbounded_channel();

        extract_client_archive(
            &archive_path,
            &game_path,
            &sender,
            &HashSet::new(),
            &manifest,
        )
        .unwrap();

        assert_eq!(
            fs::read(game_path.join("bin").join("client.exe")).unwrap(),
            client_bytes
        );
        assert!(!game_path.join("bin").join("client.exe.lzma").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archive_install_is_used_when_launcher_state_is_missing() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("penultima-launcher-test-{unique}"));
        let game_path = root.join("game");
        let state_path = root.join("state");
        let download_path = root.join("downloads");

        fs::create_dir_all(game_path.join("bin")).unwrap();
        fs::write(game_path.join("bin").join("client.exe"), b"test").unwrap();

        let manager = UpdateManager::new(download_path, game_path, state_path);
        let files = vec![PackageFile {
            url: "bin/client.exe".to_string(),
            localfile: "bin/client.exe".to_string(),
            packedhash: None,
            packedsize: Some(4),
            unpackedhash: None,
            unpackedsize: None,
            unpack: Some(false),
            bootstrap_only: false,
        }];

        assert!(manager.should_use_archive_install(&files, false, false));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn small_synced_update_uses_parallel_file_downloads() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("penultima-launcher-test-{unique}"));
        let game_path = root.join("game");
        let state_path = root.join("state");
        let download_path = root.join("downloads");

        fs::create_dir_all(game_path.join("bin")).unwrap();
        fs::write(game_path.join("bin").join("client.exe"), b"test").unwrap();

        let manager = UpdateManager::new(download_path, game_path, state_path);
        let files = vec![PackageFile {
            url: "assets/catalog-content.json".to_string(),
            localfile: "assets/catalog-content.json".to_string(),
            packedhash: None,
            packedsize: Some(4),
            unpackedhash: None,
            unpackedsize: None,
            unpack: Some(false),
            bootstrap_only: false,
        }];

        assert!(!manager.should_use_archive_install(&files, false, true));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn large_synced_update_uses_archive_install() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("penultima-launcher-test-{unique}"));
        let game_path = root.join("game");
        let state_path = root.join("state");
        let download_path = root.join("downloads");

        fs::create_dir_all(game_path.join("bin")).unwrap();
        fs::write(game_path.join("bin").join("client.exe"), b"test").unwrap();

        let manager = UpdateManager::new(download_path, game_path, state_path);
        let files = vec![PackageFile {
            url: "assets/big.dat".to_string(),
            localfile: "assets/big.dat".to_string(),
            packedhash: None,
            packedsize: Some(BULK_ARCHIVE_BYTE_THRESHOLD),
            unpackedhash: None,
            unpackedsize: None,
            unpack: Some(false),
            bootstrap_only: false,
        }];

        assert!(manager.should_use_archive_install(&files, false, true));

        let _ = fs::remove_dir_all(root);
    }
}

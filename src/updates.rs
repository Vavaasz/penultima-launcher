use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use log::info;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use crate::client_version::ClientVersionManager;
use crate::constants::{
    CLIENT_ASSET_MANIFEST_HASH_URL, CLIENT_ASSET_MANIFEST_URL, CLIENT_GITHUB_RAW_BASE_URL,
    CLIENT_PACKAGE_MANIFEST_URL, CLIENT_PACKAGE_VERSION_URL, HTTP_REQUEST_TIMEOUT,
};
use crate::message_system::LauncherMessage;
use crate::tokio::sync::mpsc;

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
        if let Some(version) =
            read_metadata_file(state_path, game_path, "package.json.version")?
        {
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
            info!("Falha ao garantir diretório do jogo: {}", error);
            return Ok(true);
        }

        let client_exists = game_path.join("bin").join("client.exe").exists();
        if !client_exists {
            info!("client.exe não encontrado. Atualização necessária.");
            return Ok(true);
        }

        let package_raw = match fetch_text(CLIENT_PACKAGE_MANIFEST_URL).await {
            Ok(text) => text,
            Err(error) => {
                info!("Falha ao obter package.json remoto: {}", error);
                return Ok(false);
            }
        };

        let remote_assets_hash = match fetch_text(CLIENT_ASSET_MANIFEST_HASH_URL).await {
            Ok(text) => text,
            Err(error) => {
                info!("Falha ao obter assets.json.sha256 remoto: {}", error);
                return Ok(false);
            }
        };

        let local_package =
            read_metadata_file(state_path, game_path, "package.json").unwrap_or_default();
        let local_assets_hash =
            read_metadata_file(state_path, game_path, "assets.json.sha256").unwrap_or_default();

        Ok(local_package
            .unwrap_or_default()
            .trim()
            != package_raw.trim()
            || local_assets_hash
                .unwrap_or_default()
                .trim()
                != remote_assets_hash.trim())
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
        let local_package =
            read_metadata_file(&self.state_path, &self.game_path, "package.json")?.unwrap_or_default();
        let local_assets_hash =
            read_metadata_file(&self.state_path, &self.game_path, "assets.json.sha256")?
                .unwrap_or_default();

        let package_changed = force || local_package.trim() != remote.package_raw.trim();
        let assets_changed = force || local_assets_hash.trim() != remote.assets_hash.trim();

        let files_to_update = if force || package_changed {
            self.collect_changed_files(&remote.package_manifest, force)?
        } else {
            Vec::new()
        };

        if files_to_update.is_empty() && !assets_changed {
            info!("Cliente já está sincronizado com o manifesto remoto");
            self.persist_metadata(&remote)?;
            self.refresh_versions(&message_sender, &remote.package_version)?;
            send_message(
                &message_sender,
                LauncherMessage::SetStatus(format!(
                    "Cliente já está atualizado ({})",
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

        for (index, file) in files_to_update.iter().enumerate() {
            self.download_manifest_file(file, index + 1, files_to_update.len(), &message_sender)
                .await?;
        }

        self.persist_metadata(&remote)?;
        self.refresh_versions(&message_sender, &remote.package_version)?;

        send_message(&message_sender, LauncherMessage::DownloadProgress(1.0))?;
        send_message(
            &message_sender,
            LauncherMessage::SetStatus("Atualização concluída. Pronto para jogar.".to_string()),
        )?;
        send_message(&message_sender, LauncherMessage::SetProcessing(false))?;
        send_message(&message_sender, LauncherMessage::DownloadComplete)?;

        if !disable_auto_start {
            send_message(&message_sender, LauncherMessage::LaunchGame)?;
        }

        Ok(())
    }

    async fn fetch_remote_metadata(&self) -> Result<RemoteMetadata> {
        let package_raw = fetch_text(CLIENT_PACKAGE_MANIFEST_URL)
            .await
            .context("Falha ao baixar package.json")?;
        let package_manifest: PackageManifest =
            serde_json::from_str(&package_raw).context("package.json remoto inválido")?;
        let package_version = fetch_text(CLIENT_PACKAGE_VERSION_URL)
            .await
            .context("Falha ao baixar package.json.version")?;
        let assets_raw = fetch_text(CLIENT_ASSET_MANIFEST_URL)
            .await
            .context("Falha ao baixar assets.json")?;
        let assets_hash = fetch_text(CLIENT_ASSET_MANIFEST_HASH_URL)
            .await
            .context("Falha ao baixar assets.json.sha256")?;

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

        for file in &manifest.files {
            if force || self.file_needs_update(file)? {
                changed_files.push(file.clone());
            }
        }

        Ok(changed_files)
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

    async fn download_manifest_file(
        &self,
        file: &PackageFile,
        index: usize,
        total: usize,
        message_sender: &mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<()> {
        let target_path = file.target_path(&self.game_path);
        let file_name = target_path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| file.localfile.clone());
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
                .with_context(|| format!("Falha ao criar diretório {}", parent.display()))?;
        }

        send_message(
            message_sender,
            LauncherMessage::SetStatus(format!("Atualizando {}/{}: {}", index, total, file_name)),
        )?;

        let progress = if total == 0 {
            1.0
        } else {
            ((index - 1) as f32 / total as f32).min(0.99)
        };
        send_message(message_sender, LauncherMessage::DownloadProgress(progress))?;

        download_to_path(&build_raw_url(&file.url), &packed_temp_path).await?;

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
                    "Falha ao criar arquivo temporário {}",
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

async fn download_to_path(url: &str, destination: &Path) -> Result<()> {
    if destination.exists() {
        fs::remove_file(destination)?;
    }

    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Falha ao iniciar download de {}", url))?
        .error_for_status()
        .with_context(|| format!("Download rejeitado por {}", url))?;

    let mut stream = response.bytes_stream();
    let mut file = File::create(destination)
        .with_context(|| format!("Falha ao criar {}", destination.display()))?;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("Erro ao ler dados de {}", url))?;
        file.write_all(&chunk)?;
    }

    file.flush()?;
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

fn verify_hash(path: &Path, expected_hash: &str) -> Result<()> {
    let actual_hash = hash_file(path)?;
    let expected = expected_hash.to_ascii_lowercase();
    if actual_hash != expected {
        return Err(anyhow!(
            "Hash inválido para {} (esperado {}, obtido {})",
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

#[cfg(test)]
mod tests {
    use super::PackageFile;

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
}

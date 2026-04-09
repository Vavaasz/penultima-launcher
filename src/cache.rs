use crate::LauncherMessage;
use crate::tokio::sync::mpsc;
use anyhow::Result;
use log::info;
use serde::{Deserialize, Serialize};
use serde_json;
use std::fs;
use std::path::PathBuf;
use tokio;
use tokio::time::Duration;

/// Estrutura para gerenciar as operações de cache do launcher
pub struct CacheManager {
    /// Caminho para o diretório de download
    download_path: PathBuf,
    /// Caminho para o diretório do jogo
    game_path: PathBuf,
    /// Caminho para o estado persistido do launcher
    state_path: PathBuf,
}

#[derive(Serialize, Deserialize)]
pub struct UserSettings {
    pub disable_auto_start: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            disable_auto_start: true,
        }
    }
}

impl CacheManager {
    /// Cria uma nova instância do CacheManager
    pub fn new(download_path: PathBuf, game_path: PathBuf, state_path: PathBuf) -> Self {
        Self {
            download_path,
            game_path,
            state_path,
        }
    }

    pub fn save_user_settings(&self, settings: &UserSettings) -> Result<()> {
        fs::create_dir_all(&self.state_path)?;
        let settings_path = self.state_path.join("settings.json");
        let json = serde_json::to_string_pretty(settings)?;
        fs::write(&settings_path, json)?;
        Ok(())
    }

    pub fn load_user_settings(&self) -> Result<UserSettings> {
        let settings_path = self.state_path.join("settings.json");
        if settings_path.exists() {
            let json = fs::read_to_string(&settings_path)?;
            let settings = serde_json::from_str(&json)?;
            Ok(settings)
        } else {
            Ok(UserSettings::default())
        }
    }

    /// Limpa todos os diretórios de cache
    pub async fn clean_cache(
        &self,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<f64> {
        // Iniciar com 20% de progresso para feedback visual
        let _ = message_sender.send(LauncherMessage::DownloadProgress(0.2));
        let _ = message_sender.send(LauncherMessage::SetStatus(
            "Limpando diretório de download...".to_string(),
        ));

        // Limpar diretório de download padrão
        let mut total_cleaned_mb: f64 = 0.0;
        let download_size = self.clean_directory(
            &self.download_path,
            "diretório de download",
            &message_sender,
        )?;
        total_cleaned_mb += download_size;

        // Atualiza o progresso após limpar o primeiro diretório
        let _ = message_sender.send(LauncherMessage::DownloadProgress(0.5));
        let _ = message_sender.send(LauncherMessage::SetStatus(
            "Limpando cache do jogo...".to_string(),
        ));

        for custom_cache_path in [
            self.game_path.join("Penultima").join("cache"),
            self.game_path.join("UltimaOT").join("cache"),
        ] {
            let custom_size =
                self.clean_directory(&custom_cache_path, "cache personalizado", &message_sender)?;
            total_cleaned_mb += custom_size;
        }

        // Atualiza o progresso para indicar conclusão
        let _ = message_sender.send(LauncherMessage::DownloadProgress(1.0));
        let _ = message_sender.send(LauncherMessage::SetStatus(format!(
            "Cache limpo com sucesso! ({:.2} MB liberados)",
            total_cleaned_mb
        )));

        // Pequeno delay para que o usuário veja a mensagem de sucesso
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let _ = message_sender.send(LauncherMessage::SetProcessing(false));
        let _ = message_sender.send(LauncherMessage::SetStatus("Pronto para jogar".to_string()));

        Ok(total_cleaned_mb)
    }

    /// Limpa um diretório específico e o recria
    fn clean_directory(
        &self,
        path: &PathBuf,
        description: &str,
        message_sender: &mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<f64> {
        if path.exists() {
            info!("Limpando {}: {:?}", description, path);

            // Calcular o tamanho do diretório antes de deletar
            let dir_size = self.get_directory_size(path).unwrap_or(0) as f64 / (1024.0 * 1024.0);

            if let Err(e) = fs::remove_dir_all(path) {
                let error_msg = format!("Erro ao limpar {}: {}", description, e);
                let _ = message_sender.send(LauncherMessage::Error(error_msg.clone()));
                return Err(anyhow::anyhow!(error_msg));
            }

            if let Err(e) = fs::create_dir_all(path) {
                let error_msg = format!("Erro ao recriar diretório para {}: {}", description, e);
                let _ = message_sender.send(LauncherMessage::Error(error_msg.clone()));
                return Err(anyhow::anyhow!(error_msg));
            }

            info!(
                "{} limpo com sucesso (liberados {:.2} MB)",
                description, dir_size
            );
            Ok(dir_size)
        } else {
            // Se o diretório não existir, apenas crie-o
            if let Err(e) = fs::create_dir_all(path) {
                let error_msg = format!("Erro ao criar diretório para {}: {}", description, e);
                let _ = message_sender.send(LauncherMessage::Error(error_msg.clone()));
                return Err(anyhow::anyhow!(error_msg));
            }

            info!("Diretório para {} criado", description);
            Ok(0.0)
        }
    }

    // Função para calcular o tamanho de um diretório
    fn get_directory_size(&self, path: &PathBuf) -> Result<u64> {
        let mut total_size: u64 = 0;

        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_dir() {
                    total_size += self.get_directory_size(&path)?;
                } else {
                    total_size += path.metadata()?.len();
                }
            }
        }

        Ok(total_size)
    }
}

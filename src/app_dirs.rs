use crate::constants::*;
use anyhow::{Context, Result};
use log::{error, info};
use std::env;
use std::fs;
use std::path::PathBuf;

/// Estrutura para gerenciar os diretórios da aplicação
pub struct AppDirs {
    pub base_dir: PathBuf, // Diretório base único
    pub state_path: PathBuf, // Estado interno do launcher
    pub download_path: PathBuf, // Subdiretório para downloads
    pub game_path: PathBuf,     // Subdiretório para arquivos do jogo
}

impl AppDirs {
    /// Obtém o diretório base que o egui já está criando
    pub fn get_base_dir() -> Option<PathBuf> {
        match env::var("APPDATA") {
            Ok(appdata) => {
                let base_dir = PathBuf::from(appdata).join(APP_DATA_DIR);
                Some(base_dir)
            }
            Err(_) => {
                if let Some(home) = dirs::home_dir() {
                    let base_dir = home.join(HOME_DIR);
                    Some(base_dir)
                } else {
                    None
                }
            }
        }
    }

    /// Inicializa os diretórios da aplicação, criando-os se necessário
    pub fn init() -> Result<Self> {
        let base_dir =
            Self::get_base_dir().context("Não foi possível obter o diretório base da aplicação")?;

        let state_path = base_dir.join("state");
        let download_path = base_dir.join("downloads");
        let game_path = base_dir.join("game");

        fs::create_dir_all(&base_dir).context("Não foi possível criar diretório base")?;
        fs::create_dir_all(&state_path)
            .context("Não foi possível criar diretório interno do launcher")?;
        fs::create_dir_all(&download_path)
            .context("Não foi possível criar diretório de download")?;
        fs::create_dir_all(&game_path).context("Não foi possível criar diretório do jogo")?;

        Ok(Self {
            base_dir,
            state_path,
            download_path,
            game_path,
        })
    }

    /// Retorna o caminho para o arquivo de sinal usado para comunicação entre instâncias
    pub fn get_signal_file_path() -> Option<PathBuf> {
        Self::get_base_dir().map(|dir| dir.join("show.signal"))
    }

    /// Obtem todos os caminhos de client.exe no diretório do jogo
    pub fn find_client_paths(&self) -> Vec<PathBuf> {
        let direct_client = self.game_path.join("bin").join("client.exe");
        if direct_client.exists() {
            info!("Encontrado client.exe direto: {}", direct_client.display());
            return vec![direct_client];
        }

        let glob_pattern = self.game_path.join("*/bin/client.exe");

        match glob::glob(glob_pattern.to_str().unwrap_or("")) {
            Ok(paths) => {
                let valid_paths: Vec<PathBuf> = paths.filter_map(Result::ok).collect();

                info!("Encontrados {} caminhos para client.exe", valid_paths.len());
                for (i, path) in valid_paths.iter().enumerate() {
                    info!("  [{}]: {} ", i, path.display());
                }

                valid_paths
            }
            Err(e) => {
                error!("Erro ao buscar caminhos do cliente: {}", e);
                Vec::new()
            }
        }
    }

    /// Obtém o caminho para o arquivo de versão
    pub fn get_version_file_path(&self) -> PathBuf {
        self.state_path.join("version.txt")
    }
}

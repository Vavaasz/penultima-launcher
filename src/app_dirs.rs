use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::fs;
use std::path::PathBuf;

/// Estrutura para gerenciar os diretórios da aplicação
pub struct AppDirs {
    pub download_path: PathBuf,
    pub game_path: PathBuf,
}

impl AppDirs {
    /// Obtém os diretórios do projeto usando o ProjectDirs
    pub fn get_project_dirs() -> Option<ProjectDirs> {
        ProjectDirs::from(
            "com.arcadiaot.launcher",
            "Arcadia-Organization",
            "ArcadiaOT-Launcher",
        )
    }

    /// Inicializa os diretórios da aplicação, criando-os se necessário
    pub fn init() -> Result<Self> {
        let app_dirs =
            Self::get_project_dirs().context("Não foi possível criar diretórios da aplicação")?;

        let download_path = app_dirs.cache_dir().to_path_buf();
        let game_path = app_dirs.data_dir().to_path_buf();

        fs::create_dir_all(&download_path)
            .context("Não foi possível criar diretório de download")?;
        fs::create_dir_all(&game_path).context("Não foi possível criar diretório do jogo")?;

        Ok(Self {
            download_path,
            game_path,
        })
    }

    /// Retorna o caminho para o arquivo de sinal usado para comunicação entre instâncias
    pub fn get_signal_file_path() -> Option<PathBuf> {
        Self::get_project_dirs().map(|dirs| dirs.data_dir().join("show.signal"))
    }

    /// Obtem todos os caminhos de client.exe no diretório do jogo
    pub fn find_client_paths(&self) -> Vec<PathBuf> {
        let glob_pattern = self.game_path.join("*/bin/client.exe");

        match glob::glob(glob_pattern.to_str().unwrap_or("")) {
            Ok(paths) => {
                let valid_paths: Vec<PathBuf> = paths.filter_map(Result::ok).collect();

                println!("Encontrados {} caminhos para client.exe", valid_paths.len());
                for (i, path) in valid_paths.iter().enumerate() {
                    println!("  [{}]: {} ", i, path.display());
                }

                valid_paths
            }
            Err(e) => {
                eprintln!("Erro ao buscar caminhos do cliente: {}", e);
                Vec::new()
            }
        }
    }

    /// Obtém o caminho para o arquivo de versão
    pub fn get_version_file_path(&self) -> PathBuf {
        self.game_path.join("version.txt")
    }
}

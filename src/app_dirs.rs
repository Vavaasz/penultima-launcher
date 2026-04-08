use crate::constants::*;
use anyhow::{Context, Result};
use log::{error, info};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Estrutura para gerenciar os diretórios da aplicação
pub struct AppDirs {
    #[allow(dead_code)]
    pub base_dir: PathBuf, // Diretório base único
    pub download_path: PathBuf, // Subdiretório para downloads
    pub game_path: PathBuf,     // Subdiretório para arquivos do jogo
}

impl AppDirs {
    fn is_client_runtime_ready(root: &Path) -> bool {
        REQUIRED_CLIENT_RUNTIME_FILES
            .iter()
            .all(|relative_path| root.join(relative_path).exists())
    }

    fn select_preferred_game_path<I>(candidates: I) -> Option<PathBuf>
    where
        I: IntoIterator<Item = PathBuf>,
    {
        candidates
            .into_iter()
            .find(|candidate| Self::is_client_runtime_ready(candidate))
    }

    pub fn external_game_path() -> Option<PathBuf> {
        let candidates: Vec<PathBuf> = EXTERNAL_GAME_PATHS.iter().map(PathBuf::from).collect();
        for candidate in &candidates {
            if candidate.join("bin").join("client.exe").exists()
                && !Self::is_client_runtime_ready(candidate)
            {
                info!(
                    "Ignorando cliente externo incompleto sem runtime Qt completo: {}",
                    candidate.display()
                );
            }
        }

        Self::select_preferred_game_path(candidates)
    }

    /// Obtém o diretório base que o egui já está criando
    pub fn get_base_dir() -> Option<PathBuf> {
        // Obter %APPDATA% no Windows onde o egui cria a pasta "ArcadiaOT Launcher"
        match env::var("APPDATA") {
            Ok(appdata) => {
                let base_dir = Path::new(&appdata).join(APP_DATA_DIR);
                Some(base_dir)
            }
            Err(_) => {
                // Fallback para outros sistemas operacionais
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

        // Criar subdiretórios dentro do diretório base único
        let download_path = base_dir.join("downloads");
        let game_path = if let Some(external_game_path) = Self::external_game_path() {
            info!(
                "Cliente externo detectado, usando game_path: {}",
                external_game_path.display()
            );
            external_game_path
        } else {
            base_dir.join("game")
        };

        fs::create_dir_all(&base_dir).context("Não foi possível criar diretório base")?;
        fs::create_dir_all(&download_path)
            .context("Não foi possível criar diretório de download")?;
        fs::create_dir_all(&game_path).context("Não foi possível criar diretório do jogo")?;

        Ok(Self {
            base_dir,
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
        self.game_path.join("version.txt")
    }
}

#[cfg(test)]
mod tests {
    use super::AppDirs;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn create_client_root(base_dir: &Path, folder_name: &str, files: &[&str]) -> PathBuf {
        let root = base_dir.join(folder_name);
        for relative_path in files {
            let full_path = root.join(relative_path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full_path, folder_name).unwrap();
        }
        root
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_when_client_runtime_is_incomplete() {
        let dir = temp_dir("launcher-client-runtime");
        let incomplete = create_client_root(
            &dir,
            "original",
            &[
                r"bin\client.exe",
                r"bin\Qt6Core.dll",
                r"bin\QtWebEngineProcess.exe",
                r"bin\qt.conf",
            ],
        );
        let complete = create_client_root(
            &dir,
            "local",
            &[
                r"bin\client.exe",
                r"bin\Qt6Core.dll",
                r"bin\Qt6WebEngineCore.dll",
                r"bin\QtWebEngineProcess.exe",
                r"bin\qt.conf",
            ],
        );

        assert!(!AppDirs::is_client_runtime_ready(&incomplete));
        assert!(AppDirs::is_client_runtime_ready(&complete));
    }

    #[test]
    fn prefers_runtime_ready_client_root() {
        let dir = temp_dir("launcher-client-selection");
        let incomplete = create_client_root(
            &dir,
            "original",
            &[
                r"bin\client.exe",
                r"bin\Qt6Core.dll",
                r"bin\QtWebEngineProcess.exe",
                r"bin\qt.conf",
            ],
        );
        let complete = create_client_root(
            &dir,
            "local",
            &[
                r"bin\client.exe",
                r"bin\Qt6Core.dll",
                r"bin\Qt6WebEngineCore.dll",
                r"bin\QtWebEngineProcess.exe",
                r"bin\qt.conf",
            ],
        );

        let selected = AppDirs::select_preferred_game_path([incomplete.clone(), complete.clone()]);

        assert_eq!(selected, Some(complete));
    }

    #[test]
    fn keeps_the_first_runtime_ready_client_root_when_multiple_are_valid() {
        let dir = temp_dir("launcher-client-order");
        let preferred = create_client_root(
            &dir,
            "original",
            &[
                r"bin\client.exe",
                r"bin\Qt6Core.dll",
                r"bin\Qt6WebEngineCore.dll",
                r"bin\QtWebEngineProcess.exe",
                r"bin\qt.conf",
            ],
        );
        let fallback = create_client_root(
            &dir,
            "local",
            &[
                r"bin\client.exe",
                r"bin\Qt6Core.dll",
                r"bin\Qt6WebEngineCore.dll",
                r"bin\QtWebEngineProcess.exe",
                r"bin\qt.conf",
            ],
        );

        let selected = AppDirs::select_preferred_game_path([preferred.clone(), fallback]);

        assert_eq!(selected, Some(preferred));
    }
}

use crate::app_dirs::AppDirs;
use regex::Regex;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

/// Módulo responsável por gerenciar a versão do cliente
pub struct ClientVersionManager;

impl ClientVersionManager {
    /// Carrega a versão do client.exe a partir dos caminhos disponíveis
    pub fn load_client_version(download_path: &PathBuf, game_path: &PathBuf) -> Option<String> {
        let app_dirs = AppDirs {
            base_dir: PathBuf::new(),
            state_path: PathBuf::new(),
            download_path: download_path.clone(),
            game_path: game_path.clone(),
        };

        let client_paths = app_dirs.find_client_paths();
        if let Some(client_path) = client_paths.first() {
            Self::get_file_version(client_path)
        } else {
            None
        }
    }

    /// Obtém a versão do Tibia Client lendo diretamente do binário
    pub fn get_file_version(file_path: &Path) -> Option<String> {
        // Ler o arquivo binário
        let file = File::open(file_path).ok()?;
        let mut reader = BufReader::new(file);
        let mut buffer = Vec::new();

        // Ler todo o conteúdo do arquivo
        reader.read_to_end(&mut buffer).ok()?;

        // Converter para string (ignorando caracteres inválidos)
        let content = String::from_utf8_lossy(&buffer);

        // Procurar por "Tibia Client"
        if let Some(pos) = content.find("Tibia Client") {
            // Procurar por padrão de versão após "Tibia Client"
            let search_area = &content[pos..std::cmp::min(pos + 200, content.len())];

            // Regex para encontrar versão no formato X.X.X ou X.XX
            if let Ok(version_regex) = Regex::new(r"\b(\d+\.\d+(?:\.\d+)?)\b") {
                if let Some(captures) = version_regex.find(search_area) {
                    return Some(captures.as_str().to_string());
                }
            }

            // Fallback: procurar por números após "Tibia Client"
            if let Ok(numbers_regex) = Regex::new(r"\b(\d{1,2}\.\d{1,2})\b") {
                if let Some(captures) = numbers_regex.find(search_area) {
                    return Some(captures.as_str().to_string());
                }
            }
        }

        // Se não encontrar "Tibia Client", tentar procurar por padrões de versão comuns
        let version_patterns = [
            r"Version\s+(\d+\.\d+(?:\.\d+)?)",
            r"v(\d+\.\d+(?:\.\d+)?)",
            r"(\d+\.\d{2})",
        ];

        for pattern in &version_patterns {
            if let Ok(regex) = Regex::new(pattern) {
                if let Some(captures) = regex.captures(&content) {
                    if let Some(version) = captures.get(1) {
                        return Some(version.as_str().to_string());
                    }
                }
            }
        }

        None
    }
}

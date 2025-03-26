use crate::tokio::sync::mpsc;
use crate::LauncherMessage;
use anyhow::{Context, Result};
use futures_util::StreamExt;
use glob::glob;
use log::info;
use reqwest;
use reqwest::Error;
use semver::Version;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

/// Estrutura para gerenciar as operações de atualização do jogo
pub struct UpdateManager {
    /// Caminho para o diretório de download
    download_path: PathBuf,
    /// Caminho para o diretório do jogo
    game_path: PathBuf,
}

impl UpdateManager {
    /// Cria uma nova instância do UpdateManager
    pub fn new(download_path: PathBuf, game_path: PathBuf) -> Self {
        Self {
            download_path,
            game_path,
        }
    }

    /// Busca a versão mais recente no GitHub
    async fn fetch_github_version() -> Result<String> {
        println!("Iniciando verificação de versão no GitHub...");
        let url = "https://raw.githubusercontent.com/Arcadia-OT/arcadia-client/main/version.txt";
        println!("Conectando a: {}", url);

        // Criar um cliente com timeout
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;

        // Fazer a requisição
        match client.get(url).send().await {
            Ok(response) => {
                let status = response.status();
                println!("Status HTTP: {}", status);

                if status.is_success() {
                    match response.text().await {
                        Ok(version) => {
                            let version = version.trim().to_string();
                            println!("Versão no GitHub: {}", version);
                            Ok(version)
                        }
                        Err(e) => {
                            println!("Erro ao ler resposta: {}", e);
                            Err(anyhow::anyhow!("Erro ao ler resposta do servidor: {}", e))
                        }
                    }
                } else {
                    println!("Resposta HTTP não foi bem-sucedida: {}", status);
                    Err(anyhow::anyhow!(
                        "Erro ao verificar versão: Servidor retornou {}",
                        status
                    ))
                }
            }
            Err(e) => {
                // Tratamento específico para timeout e outros erros de conexão
                if e.is_timeout() {
                    println!("Timeout na conexão com o servidor");
                    Err(anyhow::anyhow!(
                        "Tempo de conexão esgotado. Verifique sua internet."
                    ))
                } else if e.is_connect() {
                    println!("Falha na conexão com o servidor: {}", e);
                    Err(anyhow::anyhow!(
                        "Não foi possível se conectar ao servidor. Verifique sua internet."
                    ))
                } else {
                    println!("Erro na requisição: {}", e);
                    Err(anyhow::anyhow!("Erro ao verificar versão: {}", e))
                }
            }
        }
    }

    /// Carrega a versão atual do jogo
    pub fn load_current_version(game_path: &PathBuf) -> Result<String> {
        let version_file = game_path.join("version.txt");
        if version_file.exists() {
            let mut content = String::new();
            File::open(version_file)?.read_to_string(&mut content)?;
            Ok(content.trim().to_string())
        } else {
            Ok("0.0.0".to_string())
        }
    }

    /// Verifica se a versão atual precisa ser atualizada
    pub fn version_needs_update(current: &str, latest: &str) -> bool {
        match (Version::parse(current), Version::parse(latest)) {
            (Ok(current), Ok(latest)) => latest > current,
            _ => true, // Se não conseguir parsear alguma versão, assume que precisa atualizar
        }
    }

    /// Verifica se há atualizações disponíveis para o jogo
    pub async fn check_for_updates(
        &self,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<()> {
        // Atualizar o status para indicar o início da verificação
        message_sender.send(LauncherMessage::SetStatus(
            "Verificando atualizações...".to_string(),
        ))?;
        message_sender.send(LauncherMessage::SetProcessing(true))?;
        message_sender.send(LauncherMessage::DownloadProgress(0.2))?; // Progresso inicial

        println!("Buscando versão mais recente...");
        // Buscar versão do GitHub
        let latest_version = match Self::fetch_github_version().await {
            Ok(version) => version,
            Err(e) => {
                println!("Erro ao buscar versão: {}", e);
                message_sender.send(LauncherMessage::SetStatus(format!(
                    "Erro ao verificar versão: {}",
                    e
                )))?;
                message_sender.send(LauncherMessage::SetProcessing(false))?;
                return Err(e);
            }
        };

        message_sender.send(LauncherMessage::SetStatus(format!(
            "Versão mais recente: {}",
            latest_version
        )))?;
        message_sender.send(LauncherMessage::DownloadProgress(0.4))?;

        println!("Verificando versão local...");
        // Obter versão local
        let current_version =
            if let Ok(content) = fs::read_to_string(self.game_path.join("version.txt")) {
                content.trim().to_string()
            } else {
                println!("Arquivo de versão não encontrado, usando 0.0.0");
                "0.0.0".to_string()
            };

        println!(
            "Versão atual: {}, Versão mais recente: {}",
            current_version, latest_version
        );
        message_sender.send(LauncherMessage::VersionUpdated(current_version.clone()))?;
        message_sender.send(LauncherMessage::DownloadProgress(0.6))?;

        // Comparar versões
        if Self::version_needs_update(&current_version, &latest_version) {
            println!("Atualização necessária. Iniciando download...");
            message_sender.send(LauncherMessage::SetStatus(format!(
                "Nova versão disponível: {} → {}",
                current_version, latest_version
            )))?;
            message_sender.send(LauncherMessage::DownloadProgress(0.8))?;

            // Pequena pausa para que o usuário veja a mensagem
            tokio::time::sleep(Duration::from_millis(1500)).await;

            // Iniciar download
            self.download_release(
                &format!(
                    "https://github.com/Arcadia-OT/arcadia-client/releases/download/{}/ArcadiaOT.zip",
                    latest_version
                ),
                &latest_version,
                message_sender.clone(),
            ).await?;
        } else {
            println!("O jogo já está atualizado.");
            message_sender.send(LauncherMessage::SetStatus(format!(
                "Jogo já está na versão mais recente ({})",
                current_version
            )))?;
            message_sender.send(LauncherMessage::DownloadProgress(1.0))?;

            // Pequena pausa para o usuário ver a mensagem
            tokio::time::sleep(Duration::from_millis(1500)).await;

            message_sender.send(LauncherMessage::SetStatus("Pronto para jogar".to_string()))?;
            message_sender.send(LauncherMessage::SetProcessing(false))?;
        }

        Ok(())
    }

    /// Força a atualização do jogo, limpando diretórios e baixando tudo novamente
    pub async fn force_refresh(
        &self,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<()> {
        message_sender.send(LauncherMessage::SetStatus(
            "Limpando diretórios...".to_string(),
        ))?;

        // Limpar diretório de download
        if self.download_path.exists() {
            info!("Limpando diretório de download: {:?}", self.download_path);
            fs::remove_dir_all(&self.download_path)?;
            fs::create_dir_all(&self.download_path)?;
        }

        // Limpar diretório do jogo
        if self.game_path.exists() {
            info!("Limpando diretório do jogo: {:?}", self.game_path);
            fs::remove_dir_all(&self.game_path)?;
            fs::create_dir_all(&self.game_path)?;
        }

        message_sender.send(LauncherMessage::SetStatus(
            "Iniciando download limpo...".to_string(),
        ))?;

        // Chamar check_for_updates para baixar tudo novamente
        self.check_for_updates(message_sender).await
    }

    /// Baixa e instala uma versão do jogo
    pub async fn download_release(
        &self,
        url: &str,
        version: &str,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<()> {
        message_sender.send(LauncherMessage::SetProcessing(true))?;
        message_sender.send(LauncherMessage::SetStatus(
            "Iniciando download...".to_string(),
        ))?;
        message_sender.send(LauncherMessage::DownloadProgress(0.0))?;

        info!("Iniciando download de: {}", url);

        // Criar cliente HTTP
        let client = reqwest::Client::new();

        // Iniciar download
        let res = client
            .get(url)
            .send()
            .await
            .context("Falha ao fazer requisição HTTP")?;

        if !res.status().is_success() {
            return Err(anyhow::anyhow!("Erro no download: Status {}", res.status()));
        }

        let total_size = res.content_length().unwrap_or(0);
        info!("Tamanho total do arquivo: {} bytes", total_size);

        // Preparar arquivo de saída
        let zip_path = self.download_path.join(format!("game-{}.zip", version));
        info!("Salvando arquivo em: {:?}", zip_path);

        let mut file = File::create(&zip_path).context("Falha ao criar arquivo zip")?;

        let mut downloaded = 0u64;

        // Stream do download
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Falha ao ler chunk do download")?;
            file.write_all(&chunk)
                .context("Falha ao escrever chunk no arquivo")?;
            downloaded += chunk.len() as u64;

            if total_size > 0 {
                let progress = (downloaded as f32 / total_size as f32).min(1.0);
                message_sender.send(LauncherMessage::DownloadProgress(progress))?;
                message_sender.send(LauncherMessage::SetStatus(format!(
                    "Baixando... {:.1}%",
                    progress * 100.0
                )))?;
            }
        }

        // Garantir que o arquivo foi escrito completamente
        file.flush()
            .context("Falha ao finalizar escrita do arquivo")?;
        drop(file);

        info!("Download completo. Tamanho baixado: {} bytes", downloaded);
        message_sender
            .send(LauncherMessage::SetStatus(
                "Verificando arquivo...".to_string(),
            ))
            .context("Falha ao enviar status de verificação")?;

        // Verificar se o arquivo zip é válido
        let file = File::open(&zip_path).context("Falha ao abrir arquivo zip para verificação")?;
        let archive = zip::ZipArchive::new(file).context("Arquivo zip inválido")?;
        info!("Arquivo zip válido com {} arquivos", archive.len());
        drop(archive);

        message_sender
            .send(LauncherMessage::SetStatus(
                "Extraindo arquivos...".to_string(),
            ))
            .context("Falha ao enviar status de extração")?;

        // Extrair o arquivo ZIP
        message_sender.send(LauncherMessage::SetStatus(
            "Preparando extração...".to_string(),
        ))?;

        let total_files = {
            let zip_file_temp =
                File::open(&zip_path).context("Falha ao abrir arquivo ZIP para contagem")?;
            let archive_temp = zip::ZipArchive::new(zip_file_temp)
                .context("Falha ao ler arquivo ZIP para contagem")?;
            archive_temp.len()
        };

        message_sender.send(LauncherMessage::SetStatus(format!(
            "Extraindo {} arquivos...",
            total_files
        )))?;

        let zip_file = File::open(&zip_path).context("Falha ao abrir arquivo ZIP")?;
        let mut archive = zip::ZipArchive::new(zip_file).context("Falha ao ler arquivo ZIP")?;

        // Criar diretório de extração
        fs::create_dir_all(&self.game_path).context("Falha ao criar diretório de extração")?;

        // Extrair todos os arquivos
        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .context("Falha ao acessar arquivo no ZIP")?;
            let outpath = self.game_path.join(file.name());

            // Atualizar progresso a cada 10 arquivos
            if i % 10 == 0 {
                let progress = (i as f32 / total_files as f32).min(1.0);
                message_sender.send(LauncherMessage::DownloadProgress(progress))?;
                message_sender.send(LauncherMessage::SetStatus(format!(
                    "Extraindo arquivo {}/{}...",
                    i + 1,
                    total_files
                )))?;
            }

            if file.name().ends_with('/') {
                fs::create_dir_all(&outpath).context("Falha ao criar diretório de extração")?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        fs::create_dir_all(p).context("Falha ao criar diretório pai")?;
                    }
                }
                let mut outfile =
                    File::create(&outpath).context("Falha ao criar arquivo de saída")?;
                std::io::copy(&mut file, &mut outfile).context("Falha ao extrair arquivo")?;
            }
        }

        // Limpar arquivo zip após extração
        fs::remove_file(&zip_path).context("Falha ao remover arquivo zip temporário")?;

        // Salvar nova versão
        fs::write(self.game_path.join("version.txt"), &version)?;

        message_sender
            .send(LauncherMessage::VersionUpdated(version.to_string()))
            .context("Falha ao enviar versão")?;
        message_sender
            .send(LauncherMessage::SetStatus(
                "Download concluído!".to_string(),
            ))
            .context("Falha ao enviar status final")?;
        message_sender
            .send(LauncherMessage::DownloadProgress(1.0))
            .context("Falha ao enviar progresso final")?;
        message_sender.send(LauncherMessage::SetProcessing(false))?;
        message_sender.send(LauncherMessage::SetStatus(
            "Atualização concluída. Pronto para jogar.".to_string(),
        ))?;
        message_sender.send(LauncherMessage::DownloadComplete)?;

        // Verificar se o cliente foi extraído corretamente
        let client_exe_pattern = format!("{}/*/bin/client.exe", self.game_path.display());
        let client_exe_exists = glob(&client_exe_pattern)
            .map(|entries| entries.filter_map(Result::ok).next().is_some())
            .unwrap_or(false);

        if client_exe_exists {
            message_sender.send(LauncherMessage::SetStatus(
                "Atualização completa! Pronto para jogar.".to_string(),
            ))?;

            // Iniciar automaticamente o jogo após a atualização bem-sucedida
            message_sender.send(LauncherMessage::LaunchGame)?;
        } else {
            message_sender.send(LauncherMessage::Error(
                "Atualização concluída, mas client.exe não foi encontrado!".to_string(),
            ))?;
        }

        info!("Processo de download e extração concluído com sucesso!");
        Ok(())
    }

    pub async fn check_initial_updates(game_path: &PathBuf) -> Result<bool, Error> {
        println!("Verificando arquivos do jogo em: {:?}", game_path);

        // Criar diretório do jogo se não existir
        if !game_path.exists() {
            println!("Diretório do jogo não existe. Criando...");
            if let Err(e) = fs::create_dir_all(game_path) {
                println!("Erro ao criar diretório do jogo: {}", e);
                return Ok(true); // Precisa atualizar
            }
        }

        // Verificar se os arquivos principais do jogo existem
        let client_exe_pattern = format!("{}/*/bin/client.exe", game_path.display());
        println!("Buscando client.exe com padrão: {}", client_exe_pattern);

        let client_exe_exists = glob(&client_exe_pattern)
            .map(|entries| {
                let paths: Vec<_> = entries.filter_map(Result::ok).collect();
                if !paths.is_empty() {
                    println!("Encontrados {} arquivos client.exe:", paths.len());
                    for (i, path) in paths.iter().enumerate() {
                        println!("  [{}]: {}", i, path.display());
                    }
                    true
                } else {
                    println!("Nenhum client.exe encontrado");
                    false
                }
            })
            .unwrap_or_else(|e| {
                println!("Erro ao buscar client.exe: {}", e);
                false
            });

        if !client_exe_exists {
            println!("Client.exe não encontrado. Atualização necessária.");
            return Ok(true); // Precisa atualizar se não encontrar o cliente
        }

        // Buscar a versão mais recente do GitHub
        println!("Verificando versão mais recente no GitHub...");
        let latest_version_result = UpdateManager::fetch_github_version().await;

        if let Err(e) = &latest_version_result {
            println!("Erro ao buscar versão do GitHub: {}", e);
            // Em caso de erro ao buscar, verificamos se pelo menos temos os arquivos locais
            return Ok(!client_exe_exists);
        }

        let latest_version = latest_version_result.unwrap();

        // Obter versão atual do arquivo version.txt local
        let version_file_path = game_path.join("version.txt");
        println!("Verificando arquivo de versão: {:?}", version_file_path);

        let current_version = if version_file_path.exists() {
            match fs::read_to_string(&version_file_path) {
                Ok(content) => {
                    let version = content.trim().to_string();
                    println!("Versão atual lida do arquivo: {}", version);
                    version
                }
                Err(e) => {
                    println!("Erro ao ler arquivo de versão: {}", e);
                    "0.0.0".to_string()
                }
            }
        } else {
            println!("Arquivo version.txt não encontrado.");
            "0.0.0".to_string()
        };

        println!(
            "Versão atual: {}, Versão mais recente: {}",
            current_version, latest_version
        );

        // Verifica se há necessidade de atualização
        let needs_update = Self::version_needs_update(&current_version, &latest_version);

        println!("Necessita atualização? {}", needs_update);

        Ok(needs_update)
    }
}

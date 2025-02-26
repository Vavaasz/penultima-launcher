#![windows_subsystem = "windows"]

use anyhow::{Context, Result};
use directories::ProjectDirs;
use egui::IconData;
use eframe::Frame;
use eframe::egui;
use futures_util::StreamExt;
use glob::glob;
use image;
use log::{error, info, warn};
use reqwest;
use reqwest::Error;
use semver::Version;
use single_instance::SingleInstance;
use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItemBuilder},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use winapi::um::winuser::{
    FindWindowW, RedrawWindow, SetForegroundWindow, SetWindowPos, ShowWindow, HWND_NOTOPMOST, HWND_TOPMOST,
    RDW_ALLCHILDREN, RDW_INVALIDATE, RDW_UPDATENOW, RDW_FRAME, SWP_NOMOVE, SWP_NOSIZE, SW_RESTORE, SW_SHOW,
    SW_HIDE,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use crate::tokio::sync::mpsc;
use clap::Parser;
use zip::ZipArchive;

mod proxy;
mod system;

// Mensagens que podem ser enviadas ao launcher
#[derive(Debug)]
enum LauncherMessage {
    LaunchGame,
    CheckForUpdates,
    UpdateAvailable(String),
    DownloadComplete,
    DownloadProgress(f32),
    VersionUpdated(String),
    SetStatus(String),
    SetProcessing(bool),
    Error(String),
}

struct WindowState {
    visible: bool,
    last_check: Instant,
    last_show: Instant,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            visible: true, // Inicia com a janela visível
            last_check: Instant::now(),
            last_show: Instant::now(),
        }
    }
}

// Estrutura para rastrear o status de cada serviço do proxy
struct ProxyStatus {
    login_running: bool,
    game_running: bool,
    http_running: bool,
    https_running: bool,
}

impl Default for ProxyStatus {
    fn default() -> Self {
        Self {
            login_running: false,
            game_running: false,
            http_running: false,
            https_running: false,
        }
    }
}

struct GameLauncher {
    status: String,
    progress: f32,
    download_path: PathBuf,
    game_path: PathBuf,
    current_version: Option<String>,
    update_sender: Option<mpsc::UnboundedSender<()>>,
    message_receiver: Option<mpsc::UnboundedReceiver<LauncherMessage>>,
    message_sender: Option<mpsc::UnboundedSender<LauncherMessage>>,
    is_processing: bool,
    download_completed: bool,
    game_process: Option<Child>,
    active_clients: Vec<Child>, // Lista de clients ativos
    max_clients: usize,         // Número máximo de clients
    window_state: Arc<Mutex<WindowState>>,
    needs_repaint: Arc<AtomicBool>,
    tray_icon: Option<TrayIcon>,
    initialized: bool,
    // Campo para rastrear o status dos serviços
    proxy_status: ProxyStatus,
}

impl Default for GameLauncher {
    fn default() -> Self {
        let app_dirs =
            Self::get_project_dirs().expect("Não foi possível criar diretórios da aplicação");
        let download_path = app_dirs.cache_dir().to_path_buf();
        let game_path = app_dirs.data_dir().to_path_buf();

        fs::create_dir_all(&download_path).expect("Não foi possível criar diretório de download");
        fs::create_dir_all(&game_path).expect("Não foi possível criar diretório do jogo");

        let mut launcher = Self {
            status: "Verificando atualizações...".to_string(),
            progress: 0.0,
            download_path,
            game_path,
            current_version: None,
            update_sender: None,
            message_receiver: None,
            message_sender: None, // Inicializa como None
            is_processing: true,  // Inicia como processando para mostrar o indicador de carregamento
            download_completed: false,
            game_process: None,
            active_clients: Vec::new(),
            max_clients: 3,
            window_state: Arc::new(Mutex::new(WindowState::default())),
            needs_repaint: Arc::new(AtomicBool::new(false)),
            tray_icon: None,
            initialized: false,
            proxy_status: ProxyStatus::default(),
        };

        if let Ok(version) = GameLauncher::load_current_version(&launcher.game_path) {
            launcher.current_version = Some(version);
        }

        launcher
    }
}

impl GameLauncher {
    fn get_project_dirs() -> Option<ProjectDirs> {
        ProjectDirs::from(
            "com.arcadiaot.launcher",
            "Arcadia-Organization",
            "ArcadiaOT-Launcher",
        )
    }

    async fn fetch_github_version() -> Result<String> {
        let client = reqwest::Client::new();
        let url = "https://raw.githubusercontent.com/Arcadia-OT/arcadia-client/refs/heads/main/version.txt";
        let response = client.get(url).send().await?;

        if response.status().is_success() {
            let version = response.text().await?;
            Ok(version.trim().to_string())
        } else {
            Err(anyhow::anyhow!("Falha ao buscar versão do GitHub"))
        }
    }

    fn load_current_version(game_path: &PathBuf) -> Result<String> {
        let version_file = game_path.join("version.txt");
        if version_file.exists() {
            let mut content = String::new();
            File::open(version_file)?.read_to_string(&mut content)?;
            Ok(content.trim().to_string())
        } else {
            Ok("0.0.0".to_string())
        }
    }

    fn version_needs_update(current: &str, latest: &str) -> bool {
        match (Version::parse(current), Version::parse(latest)) {
            (Ok(current), Ok(latest)) => latest > current,
            _ => true, // Se não conseguir parsear alguma versão, assume que precisa atualizar
        }
    }

    async fn check_initial_updates(game_path: &PathBuf) -> Result<bool, Error> {
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
        let latest_version_result = Self::fetch_github_version().await;
        
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
                },
                Err(e) => {
                    println!("Erro ao ler arquivo de versão: {}", e);
                    "0.0.0".to_string()
                }
            }
        } else {
            println!("Arquivo version.txt não encontrado.");
            "0.0.0".to_string()
        };
        
        println!("Versão atual: {}, Versão mais recente: {}", current_version, latest_version);
        
        // Verifica se há necessidade de atualização
        let needs_update = Self::version_needs_update(&current_version, &latest_version);
        
        println!("Necessita atualização? {}", needs_update);
        
        Ok(needs_update)
    }

    async fn download_release(
        url: &str,
        version: &str,
        download_path: PathBuf,
        game_path: PathBuf,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<()> {
        message_sender.send(LauncherMessage::SetProcessing(true))?;
        message_sender.send(LauncherMessage::SetStatus(
            "Iniciando download...".to_string(),
        ))?;
        message_sender.send(LauncherMessage::DownloadProgress(0.0))?;

        println!("Iniciando download de: {}", url);

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
        println!("Tamanho total do arquivo: {} bytes", total_size);

        // Preparar arquivo de saída
        let zip_path = download_path.join(format!("game-{}.zip", version));
        println!("Salvando arquivo em: {:?}", zip_path);

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
                let progress = (downloaded as f32 / total_size as f32).min(1.0); // Adiciona o método min para evitar progresso maior que 1.0
                message_sender.send(LauncherMessage::DownloadProgress(progress))?;
                message_sender.send(LauncherMessage::SetStatus(format!(
                    "Baixando... {:.1}%",
                    progress * 100.0
                )))?;
            }

            // Removido o println fora do escopo
        }

        // Garantir que o arquivo foi escrito completamente
        file.flush()
            .context("Falha ao finalizar escrita do arquivo")?;
        drop(file);

        println!("Download completo. Tamanho baixado: {} bytes", downloaded);
        message_sender
            .send(LauncherMessage::SetStatus(
                "Verificando arquivo...".to_string(),
            ))
            .context("Falha ao enviar status de verificação")?;

        // Verificar se o arquivo zip é válido
        let file = File::open(&zip_path).context("Falha ao abrir arquivo zip para verificação")?;
        let archive = zip::ZipArchive::new(file).context("Arquivo zip inválido")?;
        println!("Arquivo zip válido com {} arquivos", archive.len());
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
            let zip_file_temp = std::fs::File::open(&zip_path).context("Falha ao abrir arquivo ZIP para contagem")?;
            let archive_temp = zip::ZipArchive::new(zip_file_temp).context("Falha ao ler arquivo ZIP para contagem")?;
            archive_temp.len()
        };
        
        message_sender.send(LauncherMessage::SetStatus(
            format!("Extraindo {} arquivos...", total_files),
        ))?;
        
        let zip_file = std::fs::File::open(&zip_path).context("Falha ao abrir arquivo ZIP")?;
        let mut archive = zip::ZipArchive::new(zip_file).context("Falha ao ler arquivo ZIP")?;

        // Criar diretório de extração
        fs::create_dir_all(&game_path).context("Falha ao criar diretório de extração")?;

        // Extrair todos os arquivos
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).context("Falha ao acessar arquivo no ZIP")?;
            let outpath = game_path.join(file.name());

            // Atualizar progresso a cada 10 arquivos
            if i % 10 == 0 {
                let progress = (i as f32 / total_files as f32).min(1.0); // Adiciona o método min para evitar progresso maior que 1.0
                message_sender.send(LauncherMessage::DownloadProgress(progress))?;
                message_sender.send(LauncherMessage::SetStatus(
                    format!("Extraindo arquivo {}/{}...", i+1, total_files),
                ))?;
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
                    fs::File::create(&outpath).context("Falha ao criar arquivo de saída")?;
                std::io::copy(&mut file, &mut outfile)
                    .context("Falha ao extrair arquivo")?;
            }
        }

        // Limpar arquivo zip após extração
        fs::remove_file(&zip_path).context("Falha ao remover arquivo zip temporário")?;

        // Salvar nova versão
        fs::write(game_path.join("version.txt"), &version)?;

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
        let client_exe_pattern = format!("{}/*/bin/client.exe", game_path.display());
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

        println!("Processo de download e extração concluído com sucesso!");
        Ok(())
    }

    async fn check_for_updates(
        download_path: PathBuf,
        game_path: PathBuf,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<()> {
        message_sender.send(LauncherMessage::SetStatus(
            "Verificando atualizações...".to_string(),
        ))?;
        message_sender.send(LauncherMessage::SetProcessing(true))?;

        let latest_version = Self::fetch_github_version().await?;

        let current_version = if let Ok(content) = fs::read_to_string(game_path.join("version.txt"))
        {
            content.trim().to_string()
        } else {
            "0.0.0".to_string()
        };

        message_sender.send(LauncherMessage::VersionUpdated(current_version.clone()))?;

        if Self::version_needs_update(&current_version, &latest_version) {
            Self::download_release(
                &format!(
                    "https://github.com/Arcadia-OT/arcadia-client/releases/download/{}/ArcadiaOT.zip",
                    latest_version
                ),
                &latest_version,
                download_path,
                game_path,
                message_sender.clone(),
            )
            .await?;
        } else {
            message_sender.send(LauncherMessage::SetStatus(
                "Jogo já está na última versão".to_string(),
            ))?;
            message_sender.send(LauncherMessage::SetProcessing(false))?;
        }

        Ok(())
    }

    async fn force_refresh(
        download_path: PathBuf,
        game_path: PathBuf,
        message_sender: mpsc::UnboundedSender<LauncherMessage>,
    ) -> Result<()> {
        message_sender.send(LauncherMessage::SetStatus(
            "Limpando diretórios...".to_string(),
        ))?;

        // Limpar diretório de download
        if download_path.exists() {
            println!("Limpando diretório de download: {:?}", download_path);
            fs::remove_dir_all(&download_path)?;
            fs::create_dir_all(&download_path)?;
        }

        // Limpar diretório do jogo
        if game_path.exists() {
            println!("Limpando diretório do jogo: {:?}", game_path);
            fs::remove_dir_all(&game_path)?;
            fs::create_dir_all(&game_path)?;
        }

        message_sender.send(LauncherMessage::SetStatus(
            "Iniciando download limpo...".to_string(),
        ))?;

        // Chamar check_for_updates para baixar tudo novamente
        Self::check_for_updates(download_path, game_path, message_sender).await
    }

    fn setup_update_channel(&mut self) {
        let (update_tx, mut update_rx) = mpsc::unbounded_channel();
        let (message_tx, message_rx) = mpsc::unbounded_channel();

        self.update_sender = Some(update_tx);
        self.message_receiver = Some(message_rx);
        self.message_sender = Some(message_tx.clone()); // Armazenar o sender

        let download_path = self.download_path.clone();
        let game_path = self.game_path.clone();
        let message_tx = message_tx.clone();

        tokio::spawn(async move {
            while let Some(_) = update_rx.recv().await {
                match Self::check_for_updates(
                    download_path.clone(),
                    game_path.clone(),
                    message_tx.clone(),
                )
                    .await
                {
                    Ok(_) => (),
                    Err(e) => {
                        if let Err(send_err) =
                            message_tx.send(LauncherMessage::Error(format!("Erro: {:#}", e)))
                        {
                            println!("Erro ao enviar mensagem de erro: {:?}", send_err);
                            // Não use break aqui; continue rodando
                        }
                    }
                }
            }
            println!("Canal de atualização encerrado");
        });
    }

    fn has_game_files(&self) -> bool {
        // Verifica se existe algum diretório com client.exe
        glob(&format!("{}/*/bin/client.exe", self.game_path.display()))
            .map(|entries| entries.filter_map(Result::ok).next().is_some())
            .unwrap_or(false)
    }

    fn can_play(&self) -> bool {
        self.has_game_files() && !self.is_processing
    }

    fn launch_game(&mut self) -> Result<()> {
        println!("Tentando iniciar o jogo...");
        self.status = "Iniciando o jogo...".to_string();
        self.is_processing = true; // Ativa o indicador de carregamento
        
        // Verifica se o jogo já está rodando
        if self.is_game_running() {
            println!("O jogo já está em execução!");
            self.is_processing = false; // Desativa o indicador se houver erro
            return Err(anyhow::anyhow!("O jogo já está em execução"));
        }

        // Procura o client.exe em subdiretórios da pasta data
        let glob_pattern = format!("{}/*/bin/client.exe", self.game_path.display());
        println!("Buscando client.exe com padrão: {}", glob_pattern);
        
        let entries = glob(&glob_pattern)
            .context("Falha ao procurar client.exe")?;

        let client_paths: Vec<_> = entries.filter_map(Result::ok).collect();
        
        if client_paths.is_empty() {
            println!("client.exe não foi encontrado!");
            self.is_processing = false; // Desativa o indicador se houver erro
            return Err(anyhow::anyhow!("client.exe não encontrado"));
        }
        
        println!("Encontrados {} caminhos para client.exe", client_paths.len());
        for (i, path) in client_paths.iter().enumerate() {
            println!("  [{}]: {}", i, path.display());
        }
        
        let client_path = &client_paths[0];
        println!("Usando client.exe: {}", client_path.display());

        println!("Diretório de trabalho: {}", client_path.parent().unwrap().display());
        
        let process = Command::new(&client_path)
            .current_dir(client_path.parent().unwrap())
            .spawn()
            .context("Falha ao iniciar o jogo")?;

        println!("Processo iniciado com sucesso: {:?}", process.id());
        self.game_process = Some(process);
        
        // Atualiza o status
        self.status = "Jogo em execução...".to_string();
        
        Ok(())
    }

    fn launch_client(&mut self) -> Result<()> {
        // Verifica se já atingiu o limite de clients
        if self.active_clients.len() >= self.max_clients {
            return Err(anyhow::anyhow!("Número máximo de clients atingido"));
        }

        // Procura o client.exe em subdiretórios da pasta data
        let entries = glob(&format!("{}/*/bin/client.exe", self.game_path.display()))
            .context("Falha ao procurar client.exe")?;

        let client_path = entries
            .filter_map(Result::ok)
            .next()
            .ok_or_else(|| anyhow::anyhow!("client.exe não encontrado"))?;

        let process = Command::new(&client_path)
            .current_dir(client_path.parent().unwrap())
            .spawn()
            .context("Falha ao iniciar o jogo")?;

        self.active_clients.push(process);
        self.needs_repaint.store(true, Ordering::SeqCst);

        Ok(())
    }

    fn is_game_running(&mut self) -> bool {
        if let Some(process) = &mut self.game_process {
            match process.try_wait() {
                Ok(None) => true, // Processo ainda está rodando
                Ok(Some(_)) => {
                    // Processo terminou
                    self.game_process = None;
                    if self.status == "Jogo em execução..." {
                        self.status = "Pronto para jogar".to_string();
                    }
                    // Garantir que o launcher não fique no estado "processando" após o jogo terminar
                    self.is_processing = false;
                    
                    // Reexibir o launcher quando o jogo fechar
                    {
                        let mut state = self.window_state.lock().unwrap();
                        state.visible = true;
                        state.last_show = Instant::now();
                    }
                    return false;
                }
                Err(_) => {
                    // Erro ao verificar processo
                    self.game_process = None;
                    if self.status == "Jogo em execução..." {
                        self.status = "Pronto para jogar".to_string();
                    }
                    // Garantir que o launcher não fique no estado "processando" após o jogo terminar
                    self.is_processing = false;
                    
                    // Reexibir o launcher quando o jogo fechar
                    {
                        let mut state = self.window_state.lock().unwrap();
                        state.visible = true;
                        state.last_show = Instant::now();
                    }
                    return false;
                }
            }
        } else {
            false
        }
    }

    fn terminate_process(process: &mut Child) {
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, false, process.id());
            if let Ok(handle) = handle {
                let _ = TerminateProcess(handle, 0);
                let _ = process.kill(); // Tenta matar o processo também usando o método padrão
            }
        }
    }

    fn terminate_all_processes(&mut self) {
        // Fecha todos os clients ativos
        for client in self.active_clients.iter_mut() {
            Self::terminate_process(client);
        }
        self.active_clients.clear();

        // Fecha o processo principal se estiver rodando
        if let Some(process) = &mut self.game_process {
            Self::terminate_process(process);
            self.game_process = None;
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        static LAST_CHECK: AtomicI64 = AtomicI64::new(0);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        let last = LAST_CHECK.load(Ordering::Relaxed);
        
        // Verificar o status do proxy a cada 5 segundos
        if now - last >= 60 {
            // Criar uma nova configuração de proxy para verificar o status
            let config = proxy::ProxyConfig::default();
            self.update_proxy_status(&config);
            LAST_CHECK.store(now, Ordering::Relaxed);
        }

        if !self.initialized {

            self.initialized = true;

            // Garantir que o canal de mensagens esteja configurado antes de verificar atualizações
            if self.message_sender.is_none() {
                println!("Configurando canais de mensagem...");
                self.setup_update_channel();
            }

            let game_path = self.game_path.clone();
            let window_state = self.window_state.clone();
            let needs_repaint = self.needs_repaint.clone();
            let message_sender = self.message_sender.clone();

            tokio::spawn(async move {
                // Atualizar o status para "Verificando atualizações"
                if let Some(sender) = message_sender.clone() {
                    let _ = sender.send(LauncherMessage::SetStatus("Verificando atualizações...".to_string()));
                    let _ = sender.send(LauncherMessage::SetProcessing(true));
                }
                
                println!("Verificando atualizações iniciais...");
                match GameLauncher::check_initial_updates(&game_path).await {
                    Ok(needs_update) => {
                        if needs_update {
                            println!("Atualização encontrada! Mostrando launcher...");
                            // Mostrar janela do launcher pois precisa atualizar
                            if let Some(sender) = message_sender.clone() {
                                sender
                                    .send(LauncherMessage::SetStatus(
                                        "Nova versão disponível. Aguardando download...".to_string(),
                                    ))
                                    .ok();
                                
                                sender
                                    .send(LauncherMessage::UpdateAvailable(
                                        "Nova versão disponível".to_string(),
                                    ))
                                    .ok();
                                    
                                sender.send(LauncherMessage::SetProcessing(false)).ok();
                            }
                        } else {
                            println!("Nenhuma atualização encontrada! Iniciando o cliente diretamente...");
                            
                            // Atualizar o status para "Iniciando o Cliente"
                            if let Some(sender) = message_sender.clone() {
                                sender.send(LauncherMessage::SetStatus("Iniciando o Cliente...".to_string())).ok();
                            }
                            
                            // Pequeno delay para que o usuário veja a mensagem
                            tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;
                            
                            // Nenhuma atualização necessária, iniciar o jogo diretamente
                            if let Some(sender) = message_sender {
                                println!("Enviando mensagem LaunchGame...");
                                match sender.send(LauncherMessage::LaunchGame) {
                                    Ok(_) => println!("Mensagem LaunchGame enviada com sucesso"),
                                    Err(e) => println!("Erro ao enviar mensagem LaunchGame: {:?}", e)
                                }
                            } else {
                                println!("Sender é None, não foi possível enviar a mensagem LaunchGame");
                            }
                        }
                    }
                    Err(e) => {
                        println!("Erro ao verificar atualizações: {}", e);
                        // Em caso de erro, mostrar o launcher para o usuário
                        show_window(&window_state);
                        needs_repaint.store(true, Ordering::SeqCst);
                        if let Some(sender) = message_sender {
                            sender
                                .send(LauncherMessage::Error(format!(
                                    "Erro ao verificar atualizações: {}",
                                    e
                                )))
                                .ok();
                        }
                    }
                }
            });
        }

        // Definir o tamanho desejado da janela
        let desired_size = egui::Vec2::new(400.0, 500.0);
        if ctx.available_rect().size() != desired_size {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(desired_size));
        }

        // Reduzir a taxa de atualização quando não houver interação
        if !ctx.input(|i| i.pointer.any_pressed() || i.pointer.any_released()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // Limpa clients que terminaram
        self.active_clients.retain_mut(|client| {
            match client.try_wait() {
                Ok(None) => true, // Processo ainda está rodando
                _ => false,       // Processo terminou ou erro
            }
        });

        // Atualiza status do jogo principal
        if let Some(process) = &mut self.game_process {
            match process.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    // Processo terminou ou erro
                    self.game_process = None;
                    if self.status == "Jogo em execução..." {
                        self.status = "Pronto para jogar".to_string();
                    }
                    // Garantir que o launcher não fique no estado "processando" após o jogo terminar
                    self.is_processing = false;
                    
                    // Reexibir o launcher quando o jogo fechar
                    {
                        let mut state = self.window_state.lock().unwrap();
                        state.visible = true;
                        state.last_show = Instant::now();
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                }
                _ => {}
            }
        }

        // Verifica se é necessário reexibir a interface
        if self.needs_repaint.load(Ordering::SeqCst) {
            println!("Solicitando repintura imediata...");
            self.needs_repaint.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        }

        // Verifica se a janela deve estar visível
        let (is_visible, recently_shown) = {
            let state = self.window_state.lock().unwrap();
            (state.visible, state.last_show.elapsed() < std::time::Duration::from_secs(2))
        };

        // Só esconde a janela se ela estiver marcada como invisível E não tiver sido mostrada recentemente
        let should_hide = !is_visible && !recently_shown && self.initialized;

        if should_hide {
            // Esconder a janela via Windows API
            hide_window(&self.window_state);
            return;
        }

        // Configurar canais se ainda não existirem
        if self.update_sender.is_none() {
            self.setup_update_channel();
        }

        if let Some(receiver) = &mut self.message_receiver {
            // Coletar todas as mensagens disponíveis em um vetor
            let mut messages = Vec::new();
            while let Ok(message) = receiver.try_recv() {
                messages.push(message);
            }

            for message in messages {
                match message {
                    LauncherMessage::LaunchGame => {
                        if let Err(e) = self.launch_game() {
                            self.status = format!("Erro ao iniciar o jogo: {}", e);
                            self.is_processing = false;
                        } else {
                            // O status já é definido dentro do método launch_game
                            // Esconde a janela principal
                            {
                                let mut state = self.window_state.lock().unwrap();
                                state.visible = false;
                            }
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                            ctx.request_repaint();
                        }
                    }
                    LauncherMessage::CheckForUpdates => {
                        if let Some(sender) = &self.update_sender {
                            let _ = sender.send(());
                        }
                    }
                    LauncherMessage::UpdateAvailable(version) => {
                        self.status = format!("Nova versão disponível: {}", version);
                    }
                    LauncherMessage::DownloadComplete => {
                        self.download_completed = true;
                    }
                    LauncherMessage::DownloadProgress(progress) => {
                        self.progress = progress;
                    }
                    LauncherMessage::VersionUpdated(version) => {
                        self.current_version = Some(version);
                    }
                    LauncherMessage::SetStatus(status) => self.status = status,
                    LauncherMessage::SetProcessing(processing) => self.is_processing = processing,
                    LauncherMessage::Error(error) => {
                        self.status = format!("Erro: {}", error);
                        self.is_processing = false;
                    }
                }
            }
        }

        // Configurar tema escuro moderno
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);

        // Ajustar apenas a sombra da janela, que é acessível
        style.visuals.window_shadow = egui::Shadow {
            offset: [0, 20], // Sombra deslocada 20 pixels para baixo
            blur: style.visuals.window_shadow.blur,
            spread: style.visuals.window_shadow.spread,
            color: style.visuals.window_shadow.color,
        };
        ctx.set_style(style);

        // Pegar o tamanho da janela para responsividade
        let available_size = ctx.available_rect().size();

        // Renderizar o painel central usando a função dedicada
        self.render_central_panel(ctx, available_size);

        // Solicitar repaint para manter a UI atualizada
        ctx.request_repaint();
    }

    fn render_central_panel(&mut self, ctx: &egui::Context, available_size: egui::Vec2) {
        let button_width = 300.0;
        let button_height = 40.0;
        let spacing_between_buttons = 10.0;
        let container_width = button_width + 40.0;
        let footer_height = 35.0; // Aumentando altura do rodapé
        
        // Primeiro, renderize o conteúdo principal
        egui::CentralPanel::default().show(ctx, |ui| {
            // Cria um layout vertical geral com espaço para o rodapé
            let main_content_height = ui.available_height() - footer_height;
            
            // Área principal de conteúdo
            egui::containers::Frame::none()
                .show(ui, |ui| {
                    ui.set_min_height(main_content_height);
                    
                    ui.vertical_centered(|ui| {
                        // Título com estilo
                        ui.add_space(30.0); // Aumentado para dar mais espaço no topo
                        ui.heading(
                            egui::RichText::new("ArcadiaOT Launcher")
                                .size((available_size.x * 0.05).max(24.0))
                                .strong()
                                .color(egui::Color32::from_rgb(100, 200, 255)),
                        );
            
                        ui.add_space(20.0); // Aumentado espaço entre título e próxima seção

                        // Indicador de carregamento
                        if self.is_processing {
                            // Reservar espaço para o indicador
                            let indicator_height = 120.0; // Aumentado para dar mais espaço ao círculo maior
                            let response = ui.allocate_space(egui::Vec2::new(available_size.x, indicator_height));
                            let rect = response.1;
                            let center = rect.center();
                            
                            let time = ui.input(|i| i.time);
                            let angle = (time * 2.0) % std::f64::consts::TAU;
                            let radius = 35.0; // Aumentado de 20.0 para 35.0
            
                            // Fundo do círculo
                            ui.painter().circle_filled(
                                center,
                                radius,
                                egui::Color32::from_rgb(30, 30, 30),
                            );
            
                            // Desenhar círculo animado de pontos
                            let num_points = 10; // Aumentado de 8 para 10 pontos
                            for i in 0..num_points {
                                let point_angle = angle as f32 + i as f32 * std::f32::consts::TAU / num_points as f32;
                                let x = center.x + radius * point_angle.cos();
                                let y = center.y + radius * point_angle.sin();
                                let point_pos = egui::Pos2::new(x, y);
                                let point_size = 3.5 + 3.0 * ((angle as f32 * 2.0 + i as f32 * 0.5) % std::f32::consts::TAU).sin(); // Aumentado tamanho dos pontos
                                ui.painter().circle_filled(
                                    point_pos,
                                    point_size,
                                    egui::Color32::from_rgb(0, 120, 210),
                                );
                            }
            
                            // Mensagem de status abaixo do círculo
                            ui.allocate_ui_at_rect(
                                egui::Rect::from_min_size(
                                    egui::Pos2::new(rect.left(), rect.bottom() + 15.0), // Aumentado o espaço para texto
                                    egui::Vec2::new(rect.width(), 25.0), // Aumentado a altura para texto maior
                                ),
                                |ui| {
                                    ui.centered_and_justified(|ui| {
                                        ui.label(egui::RichText::new(&self.status).size(18.0).color(egui::Color32::WHITE)); // Aumentado tamanho do texto
                                    });
                                },
                            );
            
                            ui.ctx().request_repaint(); // Mantém a animação ativa
                        }
                        
                        // Espaço dinâmico para empurrar os botões para baixo quando não há indicador de carregamento
                        if !self.is_processing {
                            ui.add_space(available_size.y * 0.25); // Ajustado para posicionar melhor quando não há indicador
                        } else {
                            ui.add_space(available_size.y * 0.10); // Espaço menor quando há indicador
                        }

                        // Centralizar os botões manualmente
                        let available_width = ui.available_width();
                        let indent = (available_width - button_width) / 2.0;

                        if !self.is_processing && !self.has_game_files() {
                            // Botão Baixar
                            ui.horizontal(|ui| {
                                ui.add_space(indent);
                                if ui
                                    .add_sized(
                                        [button_width, button_height],
                                        egui::Button::new(
                                            egui::RichText::new("BAIXAR JOGO")
                                                .size(20.0)
                                                .strong()
                                                .color(egui::Color32::BLACK),
                                        )
                                        .fill(egui::Color32::from_rgb(100, 200, 255))
                                        .corner_radius(8.0),
                                    )
                                    .clicked()
                                {
                                    if let Some(sender) = &self.update_sender {
                                        let _ = sender.send(());
                                    }
                                }
                            });
                        } else if !self.is_processing && self.can_play() {
                            // Botão Jogar - sem espaço adicional antes do primeiro botão
                            ui.horizontal(|ui| {
                                ui.add_space(indent);
                                let is_running = self.is_game_running();
                                let button = ui
                                    .add_sized(
                                        [button_width, button_height],
                                        egui::Button::new(
                                            egui::RichText::new("JOGAR AGORA").size(20.0).color(
                                                if is_running {
                                                    egui::Color32::GRAY
                                                } else {
                                                    egui::Color32::BLACK
                                                },
                                            ),
                                        )
                                        .fill(if is_running {
                                            egui::Color32::from_rgb(150, 150, 150)
                                        } else {
                                            egui::Color32::from_rgb(100, 200, 255)
                                        })
                                        .corner_radius(8.0),
                                    )
                                    .clicked();

                                if button && !is_running {
                                    if let Err(e) = self.launch_game() {
                                        self.status = format!("Erro ao iniciar o jogo: {}", e);
                                    } else {
                                        // O status já é definido dentro do método launch_game
                                        // Esconde a janela principal
                                        {
                                            let mut state = self.window_state.lock().unwrap();
                                            state.visible = false;
                                        }
                                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                                        ctx.request_repaint();
                                    }
                                }
                            });

                            // Espaço entre botões
                            ui.add_space(spacing_between_buttons);

                            // Botão Atualizar
                            ui.horizontal(|ui| {
                                ui.add_space(indent);
                                if ui
                                    .add_sized(
                                        [button_width, button_height],
                                        egui::Button::new(
                                            egui::RichText::new("VERIFICAR ATUALIZAÇÕES").size(16.0),
                                        )
                                        .corner_radius(8.0),
                                    )
                                    .clicked()
                                {
                                    if let Some(sender) = &self.update_sender {
                                        let _ = sender.send(());
                                    }
                                }
                            });

                            // Espaço entre botões
                            ui.add_space(spacing_between_buttons);

                            // Botão Refresh
                            ui.horizontal(|ui| {
                                ui.add_space(indent);
                                if ui
                                    .add_sized(
                                        [button_width, button_height],
                                        egui::Button::new(
                                            egui::RichText::new("🔄 FORÇAR ATUALIZAÇÃO").size(16.0),
                                        )
                                        .fill(egui::Color32::from_rgb(65, 105, 225))
                                        .corner_radius(8.0),
                                    )
                                    .clicked()
                                {
                                    if let Some(_sender) = &self.update_sender {
                                        // Criar novo canal para force_refresh
                                        let (message_tx, message_rx) = mpsc::unbounded_channel();
                                        self.message_receiver = Some(message_rx);

                                        let download_path = self.download_path.clone();
                                        let game_path = self.game_path.clone();

                                        // Spawn força refresh task
                                        tokio::spawn(async move {
                                            if let Err(e) = GameLauncher::force_refresh(
                                                download_path,
                                                game_path,
                                                message_tx.clone(),
                                            )
                                            .await
                                            {
                                                println!("Erro no refresh: {:#}", e);
                                                let _ = message_tx
                                                    .send(LauncherMessage::Error(format!("Erro: {:#}", e)));
                                            }
                                        });
                                    }
                                }
                            });
                        }
                    });
                });
        });
        
        // Agora, adicione o rodapé como um painel separado na parte inferior da tela
        // Este método garante que ele cubra toda a largura da janela
        egui::TopBottomPanel::bottom("footer_panel")
            .exact_height(footer_height)
            .frame(
                egui::Frame::none()
                    .fill(egui::Color32::from_rgba_premultiplied(30, 30, 30, 200))
                    .inner_margin(egui::Margin::symmetric(15, 5)) // Margem horizontal de 15px, vertical de 5px
                    .outer_margin(egui::Margin::ZERO)
                    .stroke(egui::Stroke::NONE)
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Versão à esquerda
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        if let Some(version) = &self.current_version {
                            ui.label(
                                egui::RichText::new(format!("Versão {}", version))
                                    .color(egui::Color32::from_rgb(180, 180, 180))
                                    .size(14.0),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new("Versão não instalada")
                                    .color(egui::Color32::from_rgb(180, 180, 180))
                                    .size(14.0),
                            );
                        }
                    });
                    
                    // Espaço flexível
                    ui.add_space(10.0);
                    
                    // Função para desenhar um círculo colorido
                    let draw_status_circle = |ui: &mut egui::Ui, is_running: bool, label: &str| {
                        let circle_color = if is_running {
                            egui::Color32::from_rgb(0, 180, 0) // Verde
                        } else {
                            egui::Color32::from_rgb(180, 0, 0) // Vermelho
                        };
                        
                        ui.horizontal(|ui| {
                            // Reservando espaço para o círculo
                            let circle_size = 8.0;
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(circle_size, circle_size), egui::Sense::hover());
                            
                            // Desenhando o círculo
                            ui.painter().circle_filled(
                                rect.center(),
                                circle_size / 2.0,
                                circle_color,
                            );
                            
                            ui.add_space(4.0); // Espaço entre o círculo e o texto
                            ui.label(egui::RichText::new(label).color(egui::Color32::from_rgb(160, 160, 160)).size(12.0));
                        });
                        ui.add_space(5.0);
                    };
                    
                    // Mostrar status Login
                    draw_status_circle(ui, self.proxy_status.login_running, "Login");
                    
                    // Mostrar status Game
                    draw_status_circle(ui, self.proxy_status.game_running, "Game");
                    
                    // Mostrar status HTTP
                    draw_status_circle(ui, self.proxy_status.http_running, "HTTP");
                    
                    // Mostrar status HTTPS
                    draw_status_circle(ui, self.proxy_status.https_running, "HTTPS");
                });
            });
    }

    // Método para verificar se um serviço está rodando em determinada porta
    fn check_service_status(host: &str, port: u16) -> bool {
        // Definindo um timeout para não travar a UI
        let socket_addr = match format!("{}:{}", host, port).parse::<std::net::SocketAddr>() {
            Ok(addr) => addr,
            Err(_) => return false,
        };
        
        // Usar um timeout curto para não travar a UI
        let timeout = Duration::from_millis(500);
        let start = Instant::now();
        
        match TcpStream::connect_timeout(&socket_addr, timeout) {
            Ok(_) => {
                // Serviço respondeu com sucesso
                true
            },
            Err(_) => {
                // Verificar se o timeout foi excedido
                if start.elapsed() >= timeout {
                    // Timeout excedido, consideramos que o serviço não está rodando
                    false
                } else {
                    // Erro de conexão, serviço não está rodando
                    false
                }
            }
        }
    }
    
    // Método para atualizar o status de todos os serviços do proxy
    fn update_proxy_status(&mut self, config: &proxy::ProxyConfig) {
        // Para o login e game, verificamos a conexão com o servidor remoto
        self.proxy_status.login_running = Self::check_service_status(&config.game_host, config.login_port);
        self.proxy_status.game_running = Self::check_service_status(&config.game_host, config.game_port);
        
        // Para HTTP e HTTPS, verificamos o proxy local (127.0.0.1)
        self.proxy_status.http_running = Self::check_service_status("127.0.0.1", config.http_port);
        self.proxy_status.https_running = Self::check_service_status("127.0.0.1", config.https_port);
    }
}

impl eframe::App for GameLauncher {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.update(ctx, frame);
    }
}

impl Drop for GameLauncher {
    fn drop(&mut self) {
        self.terminate_all_processes();
    }
}

fn show_window(window_state: &Arc<Mutex<WindowState>>) {
    unsafe {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use std::ptr::null_mut;

        let title: Vec<u16> = OsStr::new("ArcadiaOT Launcher")
            .encode_wide()
            .chain(Some(0))
            .collect();

        println!("Tentando encontrar a janela...");
        let hwnd = FindWindowW(null_mut(), title.as_ptr());
        if !hwnd.is_null() {
            println!("Janela encontrada, restaurando...");
            
            // Traz a janela para frente
            SetForegroundWindow(hwnd);
            SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            ShowWindow(hwnd, SW_RESTORE);
            ShowWindow(hwnd, SW_SHOW);

            // Atualiza o estado
            let mut state = window_state.lock().unwrap();
            state.visible = true;
            state.last_show = Instant::now();
        } else {
            println!("Janela não encontrada!");
        }
    }
}

fn hide_window(window_state: &Arc<Mutex<WindowState>>) {
    unsafe {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use std::ptr::null_mut;

        let title: Vec<u16> = OsStr::new("ArcadiaOT Launcher")
            .encode_wide()
            .chain(Some(0))
            .collect();

        let hwnd = FindWindowW(null_mut(), title.as_ptr());
        if !hwnd.is_null() {
            ShowWindow(hwnd, winapi::um::winuser::SW_HIDE);

            // Atualiza o estado
            let mut state = window_state.lock().unwrap();
            state.visible = false;
        } else {
            println!("Janela não encontrada!");
        }
    }
}

fn load_icon() -> Option<Arc<IconData>> {
    let icon = include_bytes!("../assets/icon.ico");
    let (icon_rgba, icon_width, icon_height) = {
        let image = image::load_from_memory(icon).ok()?;
        let image = image.into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };

    Some(Arc::new(IconData {
        rgba: icon_rgba,
        width: icon_width as _,
        height: icon_height as _,
    }))
}

// Estrutura para os argumentos de linha de comando
#[derive(Parser, Debug)]
#[clap(name = "Game Launcher", about = "Launcher para ArcadiaOT")]
struct Args {
    /// Mostra o console do launcher
    #[clap(long, short = 'c')]
    console: bool,
}

// Função para alocar e mostrar um novo console
fn show_console() {
    unsafe {
        use winapi::um::consoleapi::AllocConsole;
        AllocConsole();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Analisar argumentos de linha de comando
    let args = Args::parse();

    // Mostrar console se a flag estiver presente
    if args.console {
        show_console();
    }

    // Configurar o nível de log para debug
    std::env::set_var("RUST_LOG", "info");
    env_logger::init();

    // Configurar prioridade alta para o processo
    if let Err(e) = system::set_process_priority(true) {
        warn!("[MAIN] Erro ao configurar prioridade do processo: {}", e);
    }

    // Verificar se o launcher já está rodando
    let instance = SingleInstance::new("arcadiaot-launcher").unwrap();
    if !instance.is_single() {
        // Se já estiver rodando, enviar sinal para mostrar a janela
        if let Some(proj_dirs) = GameLauncher::get_project_dirs() {
            let signal_file = proj_dirs.data_dir().join("show.signal");
            std::fs::write(signal_file, "show")?;
        }
        println!("O launcher já está em execução. Ativando a janela existente...");
        std::process::exit(0);
    }

    // Configurar os diretórios do aplicativo
    let project_dirs =
        GameLauncher::get_project_dirs().context("Falha ao obter diretórios do projeto")?;

    let cache_dir = project_dirs.cache_dir();
    let data_dir = project_dirs.data_dir();

    println!("Diretório de download: {:?}", cache_dir);
    println!("Diretório do jogo: {:?}", data_dir);

    // Criar diretórios se não existirem
    fs::create_dir_all(cache_dir).context("Falha ao criar diretório de cache")?;
    fs::create_dir_all(data_dir).context("Falha ao criar diretório de dados")?;

    // Configurar o ícone da bandeja
    println!("Criando ícone na bandeja...");
    let icon = include_bytes!("../assets/icon.ico");
    let (icon_rgba, icon_width, icon_height) = {
        let image = image::load_from_memory(icon)
            .context("Falha ao carregar ícone")?
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };

    let icon =
        Icon::from_rgba(icon_rgba, icon_width, icon_height).context("Falha ao criar ícone")?;

    let tray_menu = Menu::new();
    let restore_id = MenuId::new("restore");
    let quit_id = MenuId::new("quit");

    // Criar os itens do menu
    let restore_item = MenuItemBuilder::new()
        .text("Abrir")
        .id(restore_id.clone())
        .enabled(true)
        .build();
    let quit_item = MenuItemBuilder::new()
        .text("Sair")
        .id(quit_id.clone())
        .enabled(true)
        .build();

    // Adicionar os itens ao menu
    tray_menu
        .append(&restore_item)
        .context("Falha ao adicionar item Abrir")?;
    tray_menu
        .append(&quit_item)
        .context("Falha ao adicionar item Sair")?;

    let tray_icon = TrayIconBuilder::new()
        .with_tooltip("ArcadiaOT Launcher")
        .with_icon(icon)
        .with_menu(Box::new(tray_menu))
        .build()
        .context("Falha ao criar ícone na system tray")?;

    println!("Ícone criado com sucesso!");

    let config = Arc::new(proxy::ProxyConfig::default());

    // Iniciar o proxy em uma nova task
    println!("Iniciando proxy do jogo...");
    let proxy_config = config.clone();
    tokio::spawn(async move {
        if let Err(e) = proxy::run_proxy(proxy_config).await {
            eprintln!("Erro ao executar o proxy: {}", e);
        }
    });

    // Esperar um pouco para os serviços iniciarem
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let window_state = Arc::new(Mutex::new(WindowState {
        visible: true, // Inicia com a janela visível
        last_check: Instant::now(),
        last_show: Instant::now(),
    }));

    // Configurar eventos do menu
    let menu_channel = MenuEvent::receiver();
    let restore_id_clone = restore_id.clone();
    let quit_id_clone = quit_id.clone();
    let window_state_menu = window_state.clone();
    std::thread::spawn(move || {
        while let Ok(event) = menu_channel.recv() {
            if event.id == restore_id_clone {
                println!("Menu Abrir clicado!");
                show_window(&window_state_menu);
            } else if event.id == quit_id_clone {
                println!("Menu Sair clicado!");
                std::process::exit(0);
            }
        }
    });

    // Thread para monitorar tentativas de nova instância
    let window_state_monitor = window_state.clone();
    std::thread::spawn(move || {
        loop {
            // Verifica se há tentativa de nova instância a cada 2 segundos
            let should_check = {
                let state = window_state_monitor.lock().unwrap();
                state.last_check.elapsed() >= std::time::Duration::from_secs(1)
            };

            if should_check {
                // Atualiza o timestamp fora do bloco principal para evitar deadlock
                {
                    let mut state = window_state_monitor.lock().unwrap();
                    state.last_check = Instant::now();
                }

                if let Some(proj_dirs) = GameLauncher::get_project_dirs() {
                    let signal_file = proj_dirs.data_dir().join("show.signal");
                    if signal_file.exists() {
                        let _ = std::fs::remove_file(&signal_file);
                        show_window(&window_state_monitor);
                    }
                }
            }

            // Dorme por mais tempo para reduzir consumo de CPU
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });

    // Configurar eventos do ícone
    let window_state_icon = window_state.clone();
    let event_channel = TrayIconEvent::receiver();
    std::thread::spawn(move || {
        while let Ok(event) = event_channel.recv() {
            if let TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = event
            {
                if button == MouseButton::Left && button_state == MouseButtonState::Up {
                    println!("Ícone clicado! Tornando janela visível...");
                    show_window(&window_state_icon);
                }
            }
        }
    });

    let native_options = eframe::NativeOptions {
        persist_window: false,
        centered: true,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 500.0]) // Define o tamanho inicial explicitamente
            .with_visible(true) // Inicia o launcher visível
            .with_resizable(false) // Impede o redimensionamento
            .with_maximized(false)
            .with_maximize_button(false) // Desativa botão de maximizar
            .with_title("ArcadiaOT Launcher")
            .with_decorations(true)
            .with_transparent(false)
            .with_active(true) // Ativa a janela ao iniciar
            .with_position([0.0, 0.0])
            .with_icon(load_icon().unwrap_or_else(|| {
                Arc::new(IconData {
                    rgba: Vec::new(),
                    width: 0,
                    height: 0,
                })
            })),
        ..Default::default()
    };

    // Iniciar o aplicativo
    eframe::run_native(
        "ArcadiaOT Launcher",
        native_options.clone(),
        Box::new(|cc| {
            // Configurar fonte padrão
            let mut style = (*cc.egui_ctx.style()).clone();
            style.text_styles = [
                (
                    egui::TextStyle::Heading,
                    egui::FontId::new(30.0, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Body,
                    egui::FontId::new(18.0, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Button,
                    egui::FontId::new(18.0, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Small,
                    egui::FontId::new(14.0, egui::FontFamily::Proportional),
                ),
            ]
            .into();
            cc.egui_ctx.set_style(style);

            let mut launcher = GameLauncher::default();
            launcher.window_state = window_state;
            launcher.tray_icon = Some(tray_icon);
            launcher.initialized = false;
            let config_clone = config.clone();
            launcher.update_proxy_status(&config_clone);
            Ok(Box::new(launcher))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Erro ao iniciar o launcher: {}", e))?;

    Ok(())
}

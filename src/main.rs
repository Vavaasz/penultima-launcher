#![windows_subsystem = "windows"]

use crate::tokio::sync::mpsc;
use anyhow::{Context, Result};
use clap::Parser;
use eframe::egui;
use image;
use log::{info, warn};
use std::fs::{self};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio;
use windows::Win32::Foundation::HWND;

mod app_dirs;
mod cache;
mod cli;
mod client_version;
mod config_modal;
mod constants;
mod game_client;
mod instance_manager;
mod logger;
mod message_system;
mod proxy;
mod proxy_status;
mod system;
mod tray_manager;
mod ui_components;
mod updates;
mod window_manager;

// Importações diretas dos novos módulos
use app_dirs::AppDirs;
use cli::{Args, show_console};
use client_version::ClientVersionManager;
use config_modal::ConfigModal;
use constants::*;
use game_client::{GameClient, WindowState};
use instance_manager::InstanceManager;
use message_system::LauncherMessage;
use proxy_status::ProxyStatus;
use tray_manager::{TrayAction, TrayManager};
use window_manager::WindowManager;

struct GameLauncher {
    status: String,
    progress: f32,
    download_path: PathBuf,
    game_path: PathBuf,
    state_path: PathBuf,
    current_version: Option<String>,
    update_sender: Option<mpsc::UnboundedSender<()>>,
    message_receiver: Option<mpsc::UnboundedReceiver<LauncherMessage>>,
    message_sender: Option<mpsc::UnboundedSender<LauncherMessage>>,
    is_processing: bool,
    download_completed: bool,
    game_client: GameClient,
    window_state: Arc<Mutex<WindowState>>,
    needs_repaint: Arc<AtomicBool>,
    initialized: bool,
    auto_hide: bool, // Flag para controlar o auto-hide do launcher
    proxy_status: ProxyStatus,
    temp_message_time: Option<Instant>, // Momento em que uma mensagem temporária foi definida
    is_alert_message: bool,             // Flag para mensagens de alerta que devem ser destacadas
    is_closing_attempted: bool, // Nova flag para indicar que o usuário tentou fechar a janela
    window_manager: Option<WindowManager>, // Gerenciador de janela
    background_texture: Option<egui::TextureHandle>, // Nova propriedade para o papel de parede
    logo_texture: Option<egui::TextureHandle>, // Nova propriedade para o logo
    show_footer: bool,          // Nova variável para controlar a visibilidade do rodapé
    show_force_update_modal: bool, // Nova variável para controlar a visibilidade do modal de confirmação
    disable_auto_start: bool,      // Nova variável para controlar o início automático
    config_modal: Option<ConfigModal>, // Novo campo para o modal de configuração
    launcher_version: String,      // Nova variável para armazenar a versão do launcher
    client_version: Option<String>, // Nova variável para armazenar a versão do client.exe
    server_ping: Option<u32>,      // Nova variável para armazenar o ping do servidor
    last_ping_check: Option<Instant>, // Momento da última verificação de ping
    was_hidden: bool, // Controla transição de visibilidade para otimizar CPU quando minimizado
    tray_manager: Option<TrayManager>,
}

impl Default for GameLauncher {
    fn default() -> Self {
        let app_dirs =
            AppDirs::init().expect("Não foi possível inicializar diretórios da aplicação");
        let download_path = app_dirs.download_path.clone();
        let game_path = app_dirs.game_path.clone();
        let state_path = app_dirs.state_path.clone();
        // Usar AppDirs::get_version_file_path para obter o caminho do arquivo de versão
        let version_file_path = app_dirs.get_version_file_path();
        info!("Caminho do arquivo de versão: {:?}", version_file_path);

        // Usar AppDirs::find_client_paths para listar os clients disponíveis
        let available_clients = app_dirs.find_client_paths();
        info!("Clientes disponíveis: {}", available_clients.len());

        // Criar GameClient com número máximo específico de clientes
        let game_client = GameClient::default();

        // Carregar configurações do usuário
        let cache_manager = cache::CacheManager::new(
            download_path.clone(),
            game_path.clone(),
            state_path.clone(),
        );
        let disable_auto_start = cache_manager
            .load_user_settings()
            .map(|settings| settings.disable_auto_start)
            .unwrap_or(true);

        let mut launcher = Self {
            status: "Verificando atualizações...".to_string(),
            progress: 0.0,
            download_path: download_path.clone(),
            game_path: game_path.clone(),
            state_path: state_path.clone(),
            current_version: None,
            update_sender: None,
            message_receiver: None,
            message_sender: None,
            is_processing: false,
            download_completed: false,
            game_client,
            window_state: Arc::new(Mutex::new(WindowState::default())),
            needs_repaint: Arc::new(AtomicBool::new(false)),
            initialized: false,
            auto_hide: false, // O launcher só vai para a tray quando o usuário pedir
            proxy_status: ProxyStatus::new(), // Usar o construtor explícito
            temp_message_time: None,
            is_alert_message: false,
            is_closing_attempted: false,
            window_manager: None,
            background_texture: None,
            logo_texture: None,             // Inicializar o logo como None
            show_footer: false,             // Rodapé desabilitado por padrão
            show_force_update_modal: false, // Modal de confirmação desabilitado por padrão
            disable_auto_start,
            config_modal: None, // Inicializar o modal de configuração como None
            launcher_version: env!("CARGO_PKG_VERSION").to_string(), // Versão do launcher do Cargo.toml
            client_version: None,
            server_ping: None,     // Inicializar ping como None
            last_ping_check: None, // Inicializar última verificação como None
            was_hidden: false,
            tray_manager: None,
        };

        // Carregar versão do client.exe
        launcher.load_client_version();

        if let Ok(version) =
            updates::UpdateManager::load_current_version(&launcher.state_path, &launcher.game_path)
        {
            launcher.current_version = Some(version);
        }

        launcher
    }
}

impl GameLauncher {
    /// Carrega a versão do client.exe
    fn load_client_version(&mut self) {
        self.client_version =
            ClientVersionManager::load_client_version(&self.download_path, &self.game_path);
    }

    fn tray_manager_mut(&mut self) -> Option<&mut TrayManager> {
        self.tray_manager.as_mut()
    }

    fn has_hidden_clients(&self) -> bool {
        self.tray_manager
            .as_ref()
            .map(|tray_manager| tray_manager.has_hidden_clients())
            .unwrap_or(false)
    }

    fn hide_launcher_to_tray(&mut self, _ctx: &egui::Context) {
        {
            let mut state = self.window_state.lock().unwrap();
            state.visible = false;
        }

        if let Some(window_manager) = &self.window_manager {
            window_manager.hide_window();
        }

        if let Some(tray_manager) = self.tray_manager_mut() {
            tray_manager.show_launcher_icon();
        }

        self.was_hidden = true;
    }

    fn restore_launcher_from_tray(&mut self, ctx: &egui::Context) {
        {
            let mut state = self.window_state.lock().unwrap();
            state.visible = true;
            state.last_show = Instant::now();
        }

        if let Some(window_manager) = &self.window_manager {
            window_manager.show_window();
        }

        if let Some(tray_manager) = self.tray_manager_mut() {
            tray_manager.hide_launcher_icon();
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn restore_all_clients_from_tray(&mut self) {
        let hwnds = self
            .tray_manager_mut()
            .map(|tray_manager| tray_manager.restore_all_hidden_clients())
            .unwrap_or_default();
        let restored = GameClient::restore_windows(&hwnds);

        if restored > 0 {
            self.status = "Clientes restaurados da system tray".to_string();
            self.temp_message_time = Some(Instant::now());
            self.is_alert_message = false;
        }
    }

    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        let actions = self
            .tray_manager
            .as_ref()
            .map(|tray_manager| tray_manager.process_events())
            .unwrap_or_default();

        for action in actions {
            match action {
                TrayAction::ShowLauncher => self.restore_launcher_from_tray(ctx),
                TrayAction::RestoreAllClients => self.restore_all_clients_from_tray(),
                TrayAction::RestoreClient(hwnd_raw) => {
                    let hwnd = HWND(hwnd_raw as *mut _);
                    if GameClient::restore_window(hwnd) {
                        if let Some(tray_manager) = self.tray_manager_mut() {
                            tray_manager.remove_hidden_client(hwnd);
                        }
                        self.status = "Cliente restaurado da system tray".to_string();
                        self.temp_message_time = Some(Instant::now());
                        self.is_alert_message = false;
                    }
                }
                TrayAction::QuitLauncher => {
                    self.restore_all_clients_from_tray();
                    std::process::exit(0);
                }
            }
        }
    }

    /// Verifica o ping do servidor usando TCP customizado
    fn check_server_ping(&mut self) {
        let now = Instant::now();

        // Se o message_sender não estiver disponível, não fazer ping ainda
        if self.message_sender.is_none() {
            return;
        }

        // Verificar se já passou tempo suficiente desde a última verificação
        // Para o primeiro ping (quando last_ping_check é None), executar imediatamente
        if let Some(last_check) = self.last_ping_check {
            if now.duration_since(last_check) < PING_CHECK_INTERVAL {
                return;
            }
        }

        // Atualizar o momento da última verificação
        self.last_ping_check = Some(now);

        // Executar ping TCP customizado de forma não-bloqueante
        if let Some(message_sender) = &self.message_sender {
            let sender = message_sender.clone();

            tokio::spawn(async move {
                let mut ping_times = Vec::new();

                // Fazer 4 pings para calcular a média
                for _ in 0..4 {
                    match Self::tcp_ping_server().await {
                        Ok(duration) => {
                            ping_times.push(duration);
                        }
                        Err(_) => {
                            // Ignorar falhas individuais
                        }
                    }
                }

                // Calcular a média dos pings bem-sucedidos
                let ping_result = if !ping_times.is_empty() {
                    Some(ping_times.iter().sum::<u32>() / ping_times.len() as u32)
                } else {
                    None
                };

                // Enviar resultado via canal de mensagens
                let _ = sender.send(LauncherMessage::PingResult(ping_result));
            });
        }
    }

    /// Cria um pacote TCP customizado seguindo o protocolo especificado
    fn create_packet(command_text: &str) -> Vec<u8> {
        let command = command_text.as_bytes();
        let length = PING_PROTOCOL_SIZE + command.len(); // 2 = 255,255 do protocolo

        let mut packet = Vec::new();
        packet.push(length as u8 & 0xff);
        packet.push((length >> 8) as u8 & 0xff);
        packet.push(255);
        packet.push(255);
        packet.extend_from_slice(command);

        packet
    }

    /// Executa ping TCP customizado para o servidor
    async fn tcp_ping_server() -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let start = Instant::now();

        // Conectar ao servidor
        let mut stream = TcpStream::connect(get_ping_server_address()).await?;

        // Criar pacote de ping
        let packet = Self::create_packet("info");

        // Enviar pacote
        stream.write_all(&packet).await?;

        // Ler resposta
        let mut buffer = vec![0; NETWORK_BUFFER_SIZE];
        let bytes_read = stream.read(&mut buffer).await?;

        let duration = start.elapsed().as_millis() as u32;

        // Apenas confirmar que houve resposta (não fazer parse do XML)
        if bytes_read > 0 {
            info!("Resposta recebida: {} bytes em {}ms", bytes_read, duration);
        }

        Ok(duration)
    }

    fn setup_update_channel(&mut self) {
        let (update_tx, mut update_rx) = mpsc::unbounded_channel();
        let (message_tx, message_rx) = mpsc::unbounded_channel();

        self.update_sender = Some(update_tx);
        self.message_receiver = Some(message_rx);
        self.message_sender = Some(message_tx.clone()); // Armazenar o sender

        let download_path = self.download_path.clone();
        let game_path = self.game_path.clone();
        let state_path = self.state_path.clone();
        let disable_auto_start = self.disable_auto_start;
        let message_tx = message_tx.clone();

        tokio::spawn(async move {
            while let Some(_) = update_rx.recv().await {
                // Criar instância do UpdateManager
                let update_manager = updates::UpdateManager::new(
                    download_path.clone(),
                    game_path.clone(),
                    state_path.clone(),
                );
                match update_manager
                    .check_for_updates(message_tx.clone(), disable_auto_start)
                    .await
                {
                    Ok(_) => (),
                    Err(e) => {
                        if let Err(send_err) =
                            message_tx.send(LauncherMessage::Error(format!("Erro: {:#}", e)))
                        {
                            info!("Erro ao enviar mensagem de erro: {:?}", send_err);
                            // Não use break aqui; continue rodando
                        }
                    }
                }
            }
            info!("Canal de atualização encerrado");
        });
    }

    fn launch_game(&mut self, ctx: &egui::Context) -> Result<()> {
        info!("Tentando iniciar o jogo...");
        self.status = "Iniciando o cliente...".to_string();
        self.is_processing = true;
        ConfigModal::ensure_default_config(&self.game_path)?;

        // Usar o GameClient para iniciar o jogo principal
        match self.game_client.launch_main_client(&self.game_path) {
            Ok(_) => {
                // Atualiza o status
                self.status = "Cliente em execução".to_string();

                // Desativa o processamento após iniciar o jogo
                self.is_processing = false;

                // Esconde a janela principal apenas se auto_hide estiver ativado
                if self.auto_hide {
                    self.hide_launcher_to_tray(ctx);
                }

                Ok(())
            }
            Err(e) => {
                self.is_processing = false;
                Err(e)
            }
        }
    }

    fn launch_client(&mut self) -> Result<()> {
        ConfigModal::ensure_default_config(&self.game_path)?;

        // Usar o GameClient para iniciar um cliente adicional
        match self.game_client.launch_additional_client(&self.game_path) {
            Ok(_) => {
                // Atualiza o status com o número total de clientes
                self.status = "Cliente adicional iniciado".to_string();
                self.status = "Cliente adicional iniciado".to_string();
                self.temp_message_time = Some(Instant::now());
                self.is_alert_message = false;
                self.needs_repaint.store(true, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn minimize_to_tray(&mut self, ctx: &egui::Context) {
        let hidden_clients = match self
            .game_client
            .minimize_declared_clients_to_tray(&self.game_path)
        {
            Ok(hidden_windows) => {
                if let Some(tray_manager) = self.tray_manager_mut() {
                    if let Err(error) = tray_manager.register_hidden_clients(&hidden_windows) {
                        self.status = format!("Erro ao criar ícones da tray: {}", error);
                        self.temp_message_time = Some(Instant::now());
                        self.is_alert_message = true;
                        ctx.request_repaint();
                        return;
                    }
                }
                hidden_windows.len()
            }
            Err(error) => {
                self.status = format!("Erro ao localizar clients: {}", error);
                self.temp_message_time = Some(Instant::now());
                self.is_alert_message = true;
                ctx.request_repaint();
                return;
            }
        };

        self.hide_launcher_to_tray(ctx);

        self.status = if hidden_clients > 0 {
            "Launcher e clientes enviados para a system tray".to_string()
        } else {
            "Launcher enviado para a system tray".to_string()
        };
        self.temp_message_time = Some(Instant::now());
        self.is_alert_message = false;
        ctx.request_repaint();
    }

    fn is_game_running(&mut self) -> bool {
        let is_running = self.game_client.is_main_client_running();

        // Se o jogo não está mais rodando mas estava antes, atualize o estado da janela
        if !is_running && self.status.starts_with("Cliente em execução") {
            self.status = "Pronto para jogar".to_string();
            self.is_processing = false;

            // Reexibir o launcher quando o jogo fechar
            {
                let mut state = self.window_state.lock().unwrap();
                state.visible = true;
                state.last_show = Instant::now();
            }
        }

        is_running
    }

    fn terminate_all_processes(&mut self) {
        self.game_client.terminate_all_processes();
    }

    fn custom_update(&mut self, ctx: &egui::Context) {
        // === Fast path: quando a janela está escondida no tray, fazer apenas trabalho essencial ===
        self.handle_tray_events(ctx);
        if let Some(tray_manager) = self.tray_manager_mut() {
            tray_manager.cleanup_hidden_clients();
        }

        let (is_visible, recently_shown) = {
            let state = self.window_state.lock().unwrap();
            (
                state.visible,
                state.last_show.elapsed() < Duration::from_secs(2),
            )
        };
        let should_hide = !is_visible && !recently_shown && self.initialized;

        if should_hide {
            // Transição para escondido: executar hide apenas uma vez
            if !self.was_hidden {
                self.was_hidden = true;
                if let Some(wm) = &self.window_manager {
                    wm.hide_window();
                }
            }

            // Trabalho essencial mínimo quando escondido:

            // 1. Drenar canal de mensagens (necessário para detectar comandos)
            if let Some(receiver) = &mut self.message_receiver {
                while let Ok(message) = receiver.try_recv() {
                    match message {
                        LauncherMessage::PingResult(ping) => {
                            self.server_ping = ping;
                            self.last_ping_check = Some(Instant::now());
                        }
                        LauncherMessage::SetStatus(status) => {
                            self.status = status;
                        }
                        LauncherMessage::SetProcessing(processing) => {
                            self.is_processing = processing;
                        }
                        LauncherMessage::Error(error) => {
                            self.status = error;
                            self.is_processing = false;
                        }
                        LauncherMessage::VersionUpdated(version) => {
                            self.current_version = Some(version);
                        }
                        LauncherMessage::ClientVersionUpdated(version) => {
                            self.client_version = Some(version);
                        }
                        LauncherMessage::DownloadComplete => {
                            self.download_completed = true;
                        }
                        LauncherMessage::DownloadProgress(progress) => {
                            self.progress = progress;
                        }
                        _ => {} // Outras mensagens processadas quando visível
                    }
                }
            }

            // 2. Verificar se o processo principal do jogo terminou (para re-mostrar a janela)
            if !self.game_client.is_main_client_running()
                && (self.status.contains("Cliente principal") || self.status.contains("Cliente em"))
            {
                self.status = "Pronto para jogar".to_string();
                self.is_processing = false;
                self.restore_launcher_from_tray(ctx);
                ctx.request_repaint();
                return;
            }

            // 3. Limpar clientes adicionais que terminaram
            self.game_client.update_additional_clients();

            // 4. Verificar ping do servidor (async, leve)
            self.check_server_ping();

            // 5. Agendar próximo wake-up com intervalo longo para economizar CPU
            let hidden_interval = self
                .tray_manager
                .as_ref()
                .map(|tray_manager| {
                    if tray_manager.should_poll_aggressively() {
                        TRAY_POLL_INTERVAL
                    } else {
                        HIDDEN_REPAINT_INTERVAL
                    }
                })
                .unwrap_or(HIDDEN_REPAINT_INTERVAL);
            ctx.request_repaint_after(hidden_interval);

            return; // Pular toda renderização e trabalho não-essencial
        }

        // Transição de escondido → visível
        if self.was_hidden {
            self.was_hidden = false;
            ctx.request_repaint();
        }

        // === Caminho normal: janela visível ===

        // Verificar ping do servidor periodicamente
        self.check_server_ping();

        // Definir o tamanho desejado da janela
        // Verificar se a tecla F1 foi pressionada para alternar a visibilidade do rodapé
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.show_footer = !self.show_footer;
            ctx.request_repaint();
        }

        // Verificar se devemos atualizar o status do proxy usando should_update
        // if self.proxy_status.should_update() {
        //     let config = proxy::ProxyConfig::default();
        //     self.proxy_status.update_status(&config);

        //     let active_services = self.proxy_status.active_services_count();
        //     info!("Serviços de proxy ativos: {}/4", active_services);
        // }

        if !self.initialized {
            self.initialized = true;

            // Garantir que o canal de mensagens esteja configurado antes de verificar atualizações
            if self.message_sender.is_none() {
                info!("Configurando canais de mensagem...");
                self.setup_update_channel();
            }

            let game_path = self.game_path.clone();
            let download_path = self.download_path.clone();
            let state_path = self.state_path.clone();
            let window_state = self.window_state.clone();
            let needs_repaint = self.needs_repaint.clone();
            let message_sender = self.message_sender.clone();
            let disable_auto_start = self.disable_auto_start; // Capturar o estado do checkbox

            tokio::spawn(async move {
                // Atualizar o status para "Verificando atualizações"
                if let Some(sender) = message_sender.clone() {
                    let _ = sender.send(LauncherMessage::SetStatus(
                        "Verificando atualizações...".to_string(),
                    ));
                    let _ = sender.send(LauncherMessage::SetProcessing(true));
                }

                info!("Verificando atualizações iniciais...");
                match updates::UpdateManager::check_initial_updates(&game_path, &state_path).await {
                    Ok(needs_update) => {
                        if needs_update {
                            info!("Atualização encontrada! Mostrando launcher...");
                            // Mostrar janela do launcher pois precisa atualizar
                            if let Some(sender) = message_sender.clone() {
                                sender
                                    .send(LauncherMessage::SetStatus(
                                        "Nova versão disponível. Iniciando download...".to_string(),
                                    ))
                                    .ok();

                                sender
                                    .send(LauncherMessage::UpdateAvailable(
                                        "Nova versão disponível".to_string(),
                                    ))
                                    .ok();

                                sender.send(LauncherMessage::SetProcessing(true)).ok();

                                // Iniciar o download automaticamente
                                let game_path = game_path.clone();
                                let state_path = state_path.clone();
                                let message_tx = sender.clone();

                                tokio::spawn(async move {
                                    let update_manager = updates::UpdateManager::new(
                                        download_path,
                                        game_path,
                                        state_path,
                                    );
                                    if let Err(e) = update_manager
                                        .check_for_updates(message_tx, disable_auto_start)
                                        .await
                                    {
                                        info!("Erro ao iniciar download automático: {}", e);
                                    }
                                });
                            }
                        } else {
                            info!("Nenhuma atualização encontrada!");

                            // Só inicia automaticamente se disable_auto_start for false
                            if !disable_auto_start {
                                info!("Iniciando o cliente automaticamente...");
                                // Atualizar o status para "Iniciando o Cliente"
                                if let Some(sender) = message_sender.clone() {
                                    sender
                                        .send(LauncherMessage::SetStatus(
                                            "Iniciando o Cliente...".to_string(),
                                        ))
                                        .ok();
                                }

                                // Pequeno delay para que o usuário veja a mensagem
                                tokio::time::sleep(Duration::from_millis(5000)).await;

                                // Iniciar o jogo
                                if let Some(sender) = message_sender {
                                    sender.send(LauncherMessage::LaunchGame).ok();
                                }
                            } else {
                                info!("Início automático desativado pelo usuário");
                                if let Some(sender) = message_sender {
                                    sender
                                        .send(LauncherMessage::SetStatus(
                                            "Pronto para jogar".to_string(),
                                        ))
                                        .ok();
                                    sender.send(LauncherMessage::SetProcessing(false)).ok();
                                }
                            }
                        }
                    }
                    Err(e) => {
                        info!("Erro ao verificar atualizações: {}", e);
                        // Em caso de erro, mostrar o launcher para o usuário
                        game_client::show_window(&window_state);
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

        // Verificar se há uma mensagem temporária que deve ser limpa
        if let Some(time) = self.temp_message_time {
            // Tempo diferente para mensagens de alerta (8 segundos) e mensagens normais (5 segundos)
            let timeout_duration = if self.is_alert_message {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(5)
            };

            // Limpa mensagens temporárias após o timeout
            if time.elapsed() > timeout_duration {
                info!("Limpando mensagem temporária: {}", self.status);

                // Limpar a mensagem temporária, incluindo a de fechamento
                self.temp_message_time = None;
                self.is_alert_message = false;

                // Se ainda estamos com a flag de fechamento ativa, apenas desativá-la
                // sem acionar novamente a mensagem
                if self.is_closing_attempted {
                    info!("Desativando flag de tentativa de fechamento após exibição temporária");
                    self.is_closing_attempted = false;
                }

                // Se o usuário tentou fechar a janela, mas agora não há mais clientes, podemos resetar a flag
                if self.temp_message_time.is_none() && !self.is_closing_attempted {
                    let (has_main, additional_count) = self.game_client.sync_client_state();
                    // Atualizar para o status normal de acordo com o estado dos clientes
                    self.status = if has_main || additional_count > 0 {
                        if has_main {
                            if additional_count == 0 {
                                "Cliente em execução".to_string()
                            } else {
                                format!("Clientes em execução")
                            }
                        } else {
                            "Clientes em execução".to_string()
                        }
                    } else {
                        "Pronto para jogar".to_string()
                    };
                }
                ctx.request_repaint();
            }
        }

        // Reduzir a taxa de atualização quando não houver interação
        if !ctx.input(|i| i.pointer.any_pressed() || i.pointer.any_released()) {
            // Se estiver mostrando um alerta, solicita repaint com maior frequência
            if self.is_alert_message {
                ctx.request_repaint_after(Duration::from_millis(100));
            } else {
                ctx.request_repaint_after(IDLE_REPAINT_INTERVAL);
            }
        }

        // Atualiza clients que terminaram
        self.game_client.update_additional_clients();

        // Atualizar status com base nos clientes ativos e o cliente principal
        let is_game_running = self.is_game_running();
        let (_, additional_count) = self.game_client.sync_client_state();

        // Se o usuário tentou fechar a janela, mas agora não há mais clientes, podemos resetar a flag
        if self.is_closing_attempted && !is_game_running && additional_count == 0 {
            info!(
                "Todos os clientes foram fechados após tentativa de fechamento. Resetando flags."
            );
            self.is_closing_attempted = false;
            self.is_alert_message = false;
            self.temp_message_time = None;
            self.status = "Pronto para jogar".to_string();
            ctx.request_repaint();
        }

        // Não atualizar o status se houver uma mensagem temporária ou tentativa de fechamento
        if !self.temp_message_time.is_some() && !self.is_closing_attempted {
            if is_game_running && additional_count > 0 {
                // Cliente principal e clientes adicionais em execução
                self.status = "Clientes em execução".to_string();
            } else if is_game_running {
                // Apenas cliente principal em execução
                self.status = "Cliente em execução".to_string();
            } else if additional_count > 0 {
                // Apenas clientes adicionais em execução
                self.status = "Clientes em execução".to_string();
            } else if self.status.contains("em execução") {
                // Nenhum cliente em execução, mas o status ainda indica que estão
                self.status = "Pronto para jogar".to_string();
            }
        }

        // Atualiza status do jogo principal
        if !self.game_client.is_main_client_running() && self.status.contains("em execu") {
            self.status = "Pronto para jogar".to_string();
            self.is_processing = false;
            self.restore_launcher_from_tray(ctx);
        }

        // Verifica se é necessário reexibir a interface
        if self.needs_repaint.load(Ordering::SeqCst) {
            info!("Solicitando repintura imediata...");
            self.needs_repaint.store(false, Ordering::SeqCst);
            ctx.request_repaint();
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

            // Se houver alguma mensagem, solicitar repintura da UI
            if !messages.is_empty() {
                // Processar as mensagens
                for message in messages {
                    match message {
                        LauncherMessage::LaunchGame => {
                            if let Err(e) = self.launch_game(ctx) {
                                self.status = format!("Erro ao iniciar o jogo: {}", e);
                            }
                        }
                        LauncherMessage::CheckForUpdates => {
                            info!("Processando CheckForUpdates");
                            if let Some(sender) = &self.update_sender {
                                if let Err(e) = sender.send(()) {
                                    info!(
                                        "Erro ao enviar mensagem para verificar atualizações: {:?}",
                                        e
                                    );
                                }
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
                        LauncherMessage::ClientVersionUpdated(version) => {
                            // Atualizar a versão do cliente na UI
                            self.client_version = Some(version);
                        }
                        LauncherMessage::SetStatus(status) => {
                            self.status = status;
                        }
                        LauncherMessage::SetProcessing(processing) => {
                            self.is_processing = processing;
                        }
                        LauncherMessage::Error(error) => {
                            self.status = error;
                            self.is_processing = false;
                        }
                        LauncherMessage::SetTempMessage(message) => {
                            self.status = message.clone();
                            self.temp_message_time = Some(Instant::now());
                            // Verifica se é um alerta específico
                            if message.contains("Feche todos os clientes antes de sair") {
                                self.is_alert_message = true;
                            } else {
                                self.is_alert_message = false;
                            }
                            info!("Mensagem temporária definida via channel: {}", message);
                        }
                        LauncherMessage::PingResult(ping) => {
                            self.server_ping = ping;
                            self.last_ping_check = Some(Instant::now());
                        }
                    }
                }
                ctx.request_repaint();
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

        // Renderizar o modal de confirmação para Forçar Atualização
        if self.show_force_update_modal {
            egui::Window::new("Forçar Atualização")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .fixed_size([320.0, 140.0])
                .frame(egui::Frame::window(&ctx.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 250)))
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new("Tem certeza que deseja forçar a atualização do cliente?")
                                .size(13.0)
                                .color(egui::Color32::from_rgb(160, 160, 160))
                        );

                        ui.label(
                            egui::RichText::new("Isso irá baixar a versão mais recente, mesmo que você já tenha a versão atual.")
                                .size(13.0)
                                .color(egui::Color32::from_rgb(140, 140, 140))
                        );

                        ui.add_space(15.0);

                        ui.horizontal(|ui| {
                            // Botão Cancelar à esquerda
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                if ui.add_sized(
                                    [90.0, 28.0],
                                    egui::Button::new(
                                        egui::RichText::new("Cancelar")
                                            .size(13.0)
                                            .color(egui::Color32::from_rgb(200, 200, 200))
                                    )
                                        .fill(egui::Color32::from_rgba_unmultiplied(45, 45, 45, 255))
                                        .corner_radius(2.0)
                                        .stroke(egui::Stroke::NONE),
                                ).clicked() {
                                    self.show_force_update_modal = false;
                                }
                            });

                            // Espaço flexível entre os botões
                            ui.with_layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight), |ui| {
                                ui.allocate_space(ui.available_size());
                            });

                            // Botão Confirmar à direita
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add_sized(
                                    [90.0, 28.0],
                                    egui::Button::new(
                                        egui::RichText::new("Confirmar")
                                            .size(13.0)
                                            .color(egui::Color32::BLACK)
                                    )
                                        .fill(egui::Color32::from_rgb(76, 175, 80))
                                        .corner_radius(2.0)
                                        .stroke(egui::Stroke::NONE),
                                ).clicked() {
                                    self.show_force_update_modal = false;

                                    // Iniciar a atualização forçada
                                    let (tx, rx) = mpsc::unbounded_channel();
                                    self.message_receiver = Some(rx);
                                    self.status = "Iniciando atualização forçada...".to_string();
                                    self.is_processing = true;
                                    self.progress = 0.0;
                                    ctx.request_repaint();

                                    let download_path = self.download_path.clone();
                                    let game_path = self.game_path.clone();
                                    let state_path = self.state_path.clone();
                                    let disable_auto_start = self.disable_auto_start;
                                    let update_manager = updates::UpdateManager::new(
                                        download_path,
                                        game_path,
                                        state_path,
                                    );

                                    tokio::spawn(async move {
                                        match update_manager.force_refresh(tx.clone(), disable_auto_start).await {
                                            Ok(_) => {
                                                info!("Atualização forçada concluída com sucesso");
                                            }
                                            Err(e) => {
                                                info!("Erro durante atualização forçada: {}", e);
                                                let _ = tx.send(LauncherMessage::SetStatus(format!(
                                                    "Erro na atualização forçada: {}",
                                                    e
                                                )));
                                                let _ = tx.send(LauncherMessage::SetProcessing(false));
                                            }
                                        }
                                    });
                                }
                            });
                        });
                    });
                });
        }

        // Inicializar o modal de configuração se necessário
        if self.config_modal.is_none() {
            self.config_modal = Some(ConfigModal::new(self.game_path.clone()));
        }

        // Verificar tecla de atalho para o modal de configuração
        if let Some(config_modal) = &mut self.config_modal {
            config_modal.check_hotkey(ctx);
        }
    }

    fn load_background(&mut self, ctx: &egui::Context) {
        // Carregar o papel de parede
        if let Ok(image_data) = image::load_from_memory(include_bytes!(
            "../../UniServerZ/www/templates/tibiacom/images/header/background-artwork.jpg"
        )) {
            let image = image_data.into_rgba8();
            let (width, height) = image.dimensions();
            let rgba = image.into_raw();

            // Criar textura do egui
            let texture =
                egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);

            // Armazenar a textura
            self.background_texture =
                Some(ctx.load_texture("background", texture, egui::TextureOptions::LINEAR));

            info!("Papel de parede carregado em {}x{}", width, height);
        } else {
            info!("Não foi possível carregar o papel de parede");
        }

        // Carregar o logo
        if let Ok(logo_data) = image::load_from_memory(include_bytes!(
            "../assets/penultima-phoenix.png"
        )) {
            let logo = logo_data.into_rgba8();

            let (width, height) = logo.dimensions();
            let rgba = logo.into_raw();

            // Criar textura do egui para o logo
            let texture =
                egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);

            // Armazenar a textura do logo
            self.logo_texture =
                Some(ctx.load_texture("logo", texture, egui::TextureOptions::LINEAR));

            info!("Logo carregado em {}x{}", width, height);
        } else {
            info!("Não foi possível carregar o logo");
        }
    }

    fn render_central_panel(&mut self, ctx: &egui::Context, available_size: egui::Vec2) {
        // Renderizar todos os componentes de UI
        ui_components::render_all_components(self, ctx, available_size);

        // Renderizar o modal de configuração
        if let Some(config_modal) = &mut self.config_modal {
            config_modal.render(ctx);
        }
    }
}

impl Drop for GameLauncher {
    fn drop(&mut self) {
        self.terminate_all_processes();
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

    // No início do seu main.rs depois de inicializar o logger
    logger::initialize(args.console);

    // Configurar prioridade alta para o processo
    if let Err(e) = system::set_process_priority(true) {
        warn!("[MAIN] Erro ao configurar prioridade do processo: {}", e);
    }

    // Inicializar o gerenciador de instância
    let mut instance_manager = InstanceManager::new(INSTANCE_NAME);

    // Verificar se o launcher já está rodando
    if !instance_manager.ensure_single_instance()? {
        // Se já estiver rodando, enviar sinal para mostrar a janela
        let _app_dirs = AppDirs::init().context("Falha ao inicializar diretórios da aplicação")?;

        let signal_path = AppDirs::get_signal_file_path()
            .context("Falha ao resolver o arquivo de sinal do launcher")?;
        info!("Caminho do arquivo de sinal: {:?}", signal_path);
        instance_manager.signal_running_instance(&signal_path)?;
        std::process::exit(0);
    }

    // Inicializar os diretórios da aplicação
    let app_dirs = AppDirs::init().context("Falha ao inicializar diretórios da aplicação")?;

    info!("Diretório de download: {:?}", app_dirs.download_path);
    info!("Diretório do jogo: {:?}", app_dirs.game_path);
    info!("Diretório de estado: {:?}", app_dirs.state_path);

    // Criar diretórios se não existirem
    fs::create_dir_all(&app_dirs.download_path).context("Falha ao criar diretório de cache")?;
    fs::create_dir_all(&app_dirs.game_path).context("Falha ao criar diretório de dados")?;
    fs::create_dir_all(&app_dirs.state_path).context("Falha ao criar diretório interno")?;

    // Clonando o caminho para evitar erros de movimento
    let signal_path =
        AppDirs::get_signal_file_path().context("Falha ao resolver o arquivo de sinal")?;

    // Criar o gerenciador de janelas
    let window_manager = WindowManager::new();
    let window_state = window_manager.window_state.clone();

    // Usar show_window do window_manager
    window_manager.show_window();

    // Configurar o ícone da bandeja usando o TrayManager
    let tracked_client_pids = Arc::new(Mutex::new(Vec::new()));
    let mut tray_manager = TrayManager::new();
    tray_manager.setup(window_state.clone())?;

    // Usar load_window_icon para carregar o ícone
    if let Some(icon_data) = TrayManager::load_window_icon() {
        info!(
            "Ícone carregado com sucesso: {}x{}",
            icon_data.width, icon_data.height
        );
    }

    // Iniciar o monitor de sinal para exibição da janela
    instance_manager.start_signal_monitor(signal_path, window_state.clone());

    // let config = Arc::new(proxy::ProxyConfig::default());

    // // Iniciar o proxy em uma nova task
    // info!("Iniciando proxy do jogo...");
    // let proxy_config = config.clone();
    // tokio::spawn(async move {
    //     if let Err(e) = proxy::run_proxy(proxy_config).await {
    //         einfo!("Erro ao executar o proxy: {}", e);
    //     }
    // });

    // Esperar um pouco para os serviços iniciarem
    tokio::time::sleep(STARTUP_DELAY).await;

    // Iniciar o aplicativo
    eframe::run_native(
        APP_NAME,
        WindowManager::get_native_options(),
        Box::new(move |cc| {
            // Configurar fonte padrão
            let mut style = (*cc.egui_ctx.style()).clone();
            style.text_styles = [
                (
                    egui::TextStyle::Heading,
                    egui::FontId::new(FONT_SIZE_HEADING, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Body,
                    egui::FontId::new(FONT_SIZE_BODY, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Button,
                    egui::FontId::new(FONT_SIZE_BUTTON, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Small,
                    egui::FontId::new(FONT_SIZE_SMALL, egui::FontFamily::Proportional),
                ),
            ]
            .into();
            style.spacing.item_spacing = egui::vec2(10.0, 10.0);
            style.visuals.window_shadow = egui::Shadow {
                offset: [0, 20],
                blur: style.visuals.window_shadow.blur,
                spread: style.visuals.window_shadow.spread,
                color: style.visuals.window_shadow.color,
            };
            cc.egui_ctx.set_style(style);

            let mut launcher = GameLauncher::default();
            launcher.game_client = GameClient::new(MAX_CLIENTS, tracked_client_pids.clone());
            launcher.window_state = window_state;
            launcher.initialized = false;
            launcher.auto_hide = args.auto_hide;
            // let config_clone = config.clone();
            // launcher.proxy_status.update_status(&config_clone);
            launcher.window_manager = Some(window_manager);
            launcher.tray_manager = Some(tray_manager);

            // Carregar o papel de parede
            launcher.load_background(&cc.egui_ctx);

            Ok(Box::new(launcher))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Erro ao iniciar o launcher: {}", e))?;

    Ok(())
}

// Implementação do eframe::App para GameLauncher
impl eframe::App for GameLauncher {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Interceptar evento de fechamento
        if ctx.input(|i| i.viewport().close_requested()) {
            info!("Evento de fechamento detectado!");
            if self.has_hidden_clients() {
                self.restore_all_clients_from_tray();
                self.restore_launcher_from_tray(ctx);
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.status = "Clientes restaurados antes de fechar o launcher".to_string();
                self.temp_message_time = Some(Instant::now());
                self.is_alert_message = false;
                ctx.request_repaint();
                return;
            }

            // Verifica se há clientes ativos
            let (has_main, additional_count) = self.game_client.sync_client_state();
            if has_main || additional_count > 0 {
                info!(
                    "Há clientes ativos: {} clientes adicionais, main: {}",
                    additional_count, has_main
                );
                // Salvar a mensagem atual para debug
                let old_status = self.status.clone();

                // Definir mensagem temporária e marcar tentativa de fechamento
                self.status = "Feche todos os clientes antes de sair!".to_string();
                self.temp_message_time = Some(Instant::now());
                self.is_alert_message = true;
                self.is_closing_attempted = true; // Marcar que o usuário tentou fechar a janela

                info!("Status alterado de '{}' para '{}'", old_status, self.status);

                // Forçar repaint imediato da UI - usando múltiplos métodos para garantir
                self.needs_repaint.store(true, Ordering::SeqCst);
                ctx.request_repaint();

                // Impede o fechamento mantendo a janela aberta
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else {
                info!("Nenhum cliente ativo, permitindo fechamento");
                // Permite o fechamento
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        // Chamada para o método de atualização personalizado
        self.custom_update(ctx);
    }
}

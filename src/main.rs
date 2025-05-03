#![windows_subsystem = "windows"]

use crate::tokio::sync::mpsc;
use anyhow::{Context, Result};
use clap::Parser;
use eframe::egui;
use log::{info, warn};
use std::fs::{self};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio;
use image;

mod app_dirs;
mod cache;
mod cli;
mod game_client;
mod instance_manager;
mod message_system;
mod proxy;
mod proxy_status;
mod system;
mod tray_manager;
mod updates;
mod window_manager;
mod logger;

// Importações diretas dos novos módulos
use app_dirs::AppDirs;
use cli::{show_console, Args};
use game_client::WindowState;
use instance_manager::InstanceManager;
use message_system::LauncherMessage;
use proxy_status::ProxyStatus;
use tray_manager::TrayManager;
use window_manager::WindowManager;

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
    game_client: game_client::GameClient, // Novo campo para gerenciar os clientes
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
    show_footer: bool, // Nova variável para controlar a visibilidade do rodapé
    show_force_update_modal: bool, // Nova variável para controlar a visibilidade do modal de confirmação
    disable_auto_start: bool, // Nova variável para controlar o início automático
}

impl Default for GameLauncher {
    fn default() -> Self {
        let app_dirs =
            AppDirs::init().expect("Não foi possível inicializar diretórios da aplicação");
        let download_path = app_dirs.download_path.clone();
        let game_path = app_dirs.game_path.clone();
        
        // Usar AppDirs::get_version_file_path para obter o caminho do arquivo de versão
        let version_file_path = app_dirs.get_version_file_path();
        info!("Caminho do arquivo de versão: {:?}", version_file_path);
        
        // Usar AppDirs::find_client_paths para listar os clients disponíveis
        let available_clients = app_dirs.find_client_paths();
        info!("Clientes disponíveis: {}", available_clients.len());

        // Criar GameClient com número máximo específico de clientes
        let game_client = game_client::GameClient::new(3);

        // Carregar configurações do usuário
        let cache_manager = cache::CacheManager::new(download_path.clone(), game_path.clone());
        let disable_auto_start = cache_manager
            .load_user_settings()
            .map(|settings| settings.disable_auto_start)
            .unwrap_or(false);

        let mut launcher = Self {
            status: "Verificando atualizações...".to_string(),
            progress: 0.0,
            download_path,
            game_path,
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
            auto_hide: false, // Por padrão, o launcher não se esconde
            proxy_status: ProxyStatus::new(), // Usar o construtor explícito
            temp_message_time: None,
            is_alert_message: false,
            is_closing_attempted: false,
            window_manager: None,
            background_texture: None,
            logo_texture: None, // Inicializar o logo como None
            show_footer: false, // Rodapé desabilitado por padrão
            show_force_update_modal: false, // Modal de confirmação desabilitado por padrão
            disable_auto_start,
        };

        if let Ok(version) = updates::UpdateManager::load_current_version(&launcher.game_path) {
            launcher.current_version = Some(version);
        }

        launcher
    }
}

impl GameLauncher {

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
                // Criar instância do UpdateManager
                let update_manager =
                    updates::UpdateManager::new(download_path.clone(), game_path.clone());
                match update_manager.check_for_updates(message_tx.clone()).await {
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

        // Usar o GameClient para iniciar o jogo principal
        match self.game_client.launch_main_client(&self.game_path) {
            Ok(_) => {
                // Atualiza o status
                self.status = "Cliente principal em execução...".to_string();

                // Desativa o processamento após iniciar o jogo
                self.is_processing = false;

                // Esconde a janela principal apenas se auto_hide estiver ativado
                if self.auto_hide {
                    {
                        let mut state = self.window_state.lock().unwrap();
                        state.visible = false;
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    ctx.request_repaint();
                }

                Ok(())
            }
            Err(e) => {
                self.is_processing = false;
                Err(e)
            }
        }
    }

    fn launch_client(&mut self, _ctx: &egui::Context) -> Result<()> {
        // Usar o GameClient para iniciar um cliente adicional
        match self.game_client.launch_additional_client(&self.game_path) {
            Ok(_) => {
                // Atualiza o status com o número total de clientes
                let (_has_main, additional_count) = self.game_client.get_clients_count();
                self.status = format!("Cliente em execução ({})...", additional_count);
                self.needs_repaint.store(true, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => Err(e),
        }
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
        // Definir o tamanho desejado da janela
        let desired_size = egui::Vec2::new(800.0, 450.0);
        if ctx.available_rect().size() != desired_size {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(desired_size));
        }
        
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
                match updates::UpdateManager::check_initial_updates(&game_path).await {
                    Ok(needs_update) => {
                        if needs_update {
                            info!("Atualização encontrada! Mostrando launcher...");
                            // Mostrar janela do launcher pois precisa atualizar
                            if let Some(sender) = message_sender.clone() {
                                sender
                                    .send(LauncherMessage::SetStatus(
                                        "Nova versão disponível. Iniciando download..."
                                            .to_string(),
                                    ))
                                    .ok();

                                sender
                                    .send(LauncherMessage::UpdateAvailable(
                                        "Nova versão disponível".to_string(),
                                    ))
                                    .ok();

                                sender.send(LauncherMessage::SetProcessing(true)).ok();
                                
                                // Iniciar o download automaticamente
                                let download_path = game_path.clone();
                                let game_path = game_path.clone();
                                let message_tx = sender.clone();
                                
                                tokio::spawn(async move {
                                    let update_manager = updates::UpdateManager::new(download_path, game_path);
                                    if let Err(e) = update_manager.check_for_updates(message_tx).await {
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
                                    sender.send(LauncherMessage::SetStatus(
                                        "Pronto para jogar".to_string(),
                                    )).ok();
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
                    info!(
                        "Desativando flag de tentativa de fechamento após exibição temporária"
                    );
                    self.is_closing_attempted = false;
                }

                // Se o usuário tentou fechar a janela, mas agora não há mais clientes, podemos resetar a flag
                if self.temp_message_time.is_none() && !self.is_closing_attempted {
                    let (has_main, additional_count) = self.game_client.get_clients_count();
                    // Atualizar para o status normal de acordo com o estado dos clientes
                    self.status = if has_main || additional_count > 0 {
                        if has_main {
                            if additional_count == 0 {
                                "Cliente principal em execução".to_string()
                            } else {
                                format!(
                                    "Cliente principal e {} adicionais em execução",
                                    additional_count
                                )
                            }
                        } else {
                            format!("{} clientes adicionais em execução", additional_count)
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
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }

        // Atualiza clients que terminaram
        self.game_client.update_additional_clients();

        // Atualizar status com base nos clientes ativos e o cliente principal
        let is_game_running = self.is_game_running();
        let (_, additional_count) = self.game_client.get_clients_count();

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
                self.status = format!("Cliente principal e {} adicional(is)...", additional_count);
            } else if is_game_running {
                // Apenas cliente principal em execução
                self.status = "Cliente principal em execução...".to_string();
            } else if additional_count > 0 {
                // Apenas clientes adicionais em execução
                self.status = format!("{} cliente(s) adicional(is)...", additional_count);
            } else if self.status.contains("em execução") {
                // Nenhum cliente em execução, mas o status ainda indica que estão
                self.status = "Pronto para jogar".to_string();
            }
        }

        // Atualiza status do jogo principal
        if let Some(process) = &mut self.game_client.game_process {
            match process.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    // Processo terminou ou erro
                    self.game_client.game_process = None;
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
            info!("Solicitando repintura imediata...");
            self.needs_repaint.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        }

        // Verifica se a janela deve estar visível
        let (is_visible, recently_shown) = {
            let state = self.window_state.lock().unwrap();
            (
                state.visible,
                state.last_show.elapsed() < Duration::from_secs(2),
            )
        };

        // Só esconde a janela se ela estiver marcada como invisível E não tiver sido mostrada recentemente
        let should_hide = !is_visible && !recently_shown && self.initialized;

        if should_hide {
            // Esconder a janela via window_manager
            if let Some(window_manager) = &self.window_manager {
                window_manager.hide_window();
            }
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
                                    let update_manager = updates::UpdateManager::new(download_path, game_path);

                                    tokio::spawn(async move {
                                        match update_manager.force_refresh(tx.clone()).await {
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
    }

    fn load_background(&mut self, ctx: &egui::Context) {
        // Carregar o papel de parede
        if let Ok(image_data) = image::load_from_memory(include_bytes!("../assets/background-artwork.png")) {
            // Converter para RGBA8
            let image = image_data.into_rgba8();
            
            // Redimensionar para o tamanho da janela (400x500)
            let resized_image = image::imageops::resize(
                &image,
                800, // Largura da janela
                450, // Altura da janela
                image::imageops::FilterType::Lanczos3, // Algoritmo de alta qualidade para redimensionamento
            );
            
            let (width, height) = resized_image.dimensions();
            let rgba = resized_image.into_raw();
            
            // Criar textura do egui
            let texture = egui::ColorImage::from_rgba_unmultiplied(
                [width as usize, height as usize],
                &rgba,
            );
            
            // Armazenar a textura
            self.background_texture = Some(ctx.load_texture(
                "background",
                texture,
                egui::TextureOptions::default(),
            ));
            
            info!("Papel de parede carregado e redimensionado para {}x{}", width, height);
        } else {
            info!("Não foi possível carregar o papel de parede");
        }

        // Carregar o logo
        if let Ok(logo_data) = image::load_from_memory(include_bytes!("../assets/arcadia_launcher_logo.png")) {
            let logo = logo_data.into_rgba8();
            
            // Redimensionar o logo para o tamanho exato desejado
            let resized_logo = image::imageops::resize(
                &logo,
                215, // Largura exata
                150,  // Altura exata
                image::imageops::FilterType::Lanczos3, // Alta qualidade para o logo
            );
            
            let (width, height) = resized_logo.dimensions();
            let rgba = resized_logo.into_raw();
            
            // Criar textura do egui para o logo
            let texture = egui::ColorImage::from_rgba_unmultiplied(
                [width as usize, height as usize],
                &rgba,
            );
            
            // Armazenar a textura do logo
            self.logo_texture = Some(ctx.load_texture(
                "logo",
                texture,
                egui::TextureOptions::default(),
            ));
            
            info!("Logo carregado e redimensionado para {}x{}", width, height);
        } else {
            info!("Não foi possível carregar o logo");
        }
    }

    fn render_central_panel(&mut self, ctx: &egui::Context, available_size: egui::Vec2) {
        let button_width = 200.0;
        let button_height = 40.0;
        let spacing_between_buttons = 6.0;
        let footer_height = if self.show_footer { 35.0 } else { 0.0 };

        // Primeiro, renderize o conteúdo principal com um fundo preto sólido para evitar flash
        egui::CentralPanel::default()
            .frame(egui::Frame::new()
                .fill(egui::Color32::from_rgb(0, 0, 0))
                .inner_margin(egui::Margin::ZERO)
                .outer_margin(egui::Margin::ZERO))
            .show(ctx, |ui| {
            // Cria um layout vertical geral com espaço para o rodapé
            let main_content_height = ui.available_height() - footer_height;

                // Área principal de conteúdo - removido o Frame adicional
                ui.set_min_height(main_content_height);

                // Renderizar o papel de parede se estiver carregado
                if let Some(texture) = &self.background_texture {
                    // Obter o tamanho disponível para o papel de parede
                    let available_rect = ui.max_rect();
                    
                    // Desenhar a imagem cobrindo toda a área
                    ui.painter().image(
                        texture.id(),
                        available_rect,
                        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                    
                    // Adicionar overlay escuro por cima da imagem
                    ui.painter().rect_filled(
                        available_rect,
                        0.0,
                        egui::Color32::from_black_alpha(153),
                    );
                }

                ui.vertical_centered(|ui| {
                    // Título com estilo - substituído pelo logo
                    ui.add_space(35.0);
                    
                    if let Some(logo) = &self.logo_texture {
                        // Tamanho fixo para o logo
                        let final_size = egui::vec2(215.0, 150.0);
                        
                        ui.add(egui::Image::new(egui::ImageSource::Texture(
                            egui::load::SizedTexture::new(logo.id(), final_size)
                        )));
                    }

                    ui.add_space(10.0);

                    // Indicador de carregamento ou status
                    if self.is_processing
                        || !self.game_client.get_clients_count().1.eq(&0)
                        || self.game_client.get_clients_count().0
                        || self.temp_message_time.is_some()
                    {
                        // Reservar espaço para o indicador ou status
                        let indicator_height = 45.0;
                        let response = ui.allocate_space(egui::Vec2::new(available_size.x, indicator_height));
                        let rect = response.1;
                        let center = rect.center();

                        // Mostrar animação apenas quando estiver processando ou com clientes ativos
                        let (has_main, additional_count) = self.game_client.get_clients_count();
                        if self.is_processing || has_main || additional_count > 0 {
                            let time = ui.input(|i| i.time) as f32;
                            let angle = (time * 2.0) % std::f32::consts::TAU;
                            let radius = 30.0;

                            // Desenhar círculo animado de pontos
                            let num_points = 10;
                            for i in 0..num_points {
                                let point_angle = angle + (i as f32 * std::f32::consts::TAU / num_points as f32);
                                let x = center.x + radius * point_angle.cos();
                                let y = center.y + radius * point_angle.sin();
                                let point_pos = egui::Pos2::new(x, y);
                                let point_size = 3.5_f32 + 3.0 * ((angle * 2.0 + i as f32 * 0.5) % std::f32::consts::TAU).sin();
                                
                                ui.painter().circle_filled(
                                    point_pos,
                                    point_size,
                                    egui::Color32::ORANGE,
                                );
                            }

                            // Solicitar repaint apenas se a animação estiver ativa
                            if self.is_processing || has_main || additional_count > 0 {
                                ctx.request_repaint_after(Duration::from_millis(50));
                            }
                        }

                        ui.add_space(10.0);

                        // Mensagem de status
                        ui.allocate_ui_with_layout(
                            egui::Vec2::new(rect.width(), 25.0),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(&self.status)
                                        .size(20.0)
                                        .color(
                                    if self.is_alert_message {
                                                egui::Color32::from_rgb(255, 100, 100) // Vermelho para alertas
                                            } else if self.temp_message_time.is_some() {
                                                egui::Color32::from_rgb(100, 255, 100) // Verde para sucesso
                                    } else {
                                                egui::Color32::from_rgb(220, 220, 220) // Branco para normal
                                            }
                                        )
                                        .strong()
                                );
                            },
                        );
                    }

                    // Espaço dinâmico para empurrar os botões para baixo quando não há indicador de carregamento
                    let (has_main, additional_count) = self.game_client.get_clients_count();
                    if !self.is_processing
                        && additional_count == 0
                        && !has_main
                        && self.temp_message_time.is_none()
                    {
                        ui.add_space(available_size.y * 0.04);
                    } else {
                        ui.add_space(available_size.y * 0.01);
                    }

                    // Centralizar os botões manualmente
                    let available_width = ui.available_width();
                    let indent = (available_width - button_width) / 2.0;

                    let is_game_running = self.is_game_running();
                    let (_, has_additional_clients) = self.game_client.get_clients_count();
                    let has_additional_clients = has_additional_clients > 0;

                    if self.is_processing {
                        // Não mostrar botões quando estiver processando
                    } else if is_game_running || has_additional_clients {
                        // Mostra APENAS o botão NOVO CLIENTE quando o jogo principal ou clientes adicionais estão rodando
                        ui.horizontal(|ui| {
                            ui.add_space(indent);
                            let (_, additional_count) = self.game_client.get_clients_count();
                            let max_clients = self.game_client.max_clients;
                            let can_launch = additional_count < max_clients;
                            
                            if ui.add_sized(
                                    [button_width, button_height],
                                    egui::Button::new(
                                    egui::RichText::new(format!("▶ Cliente Adicional ({}/{})", additional_count, max_clients))
                                        .size(15.0)
                                        .color(if can_launch {
                                            if ui.ui_contains_pointer() {
                                                egui::Color32::BLACK
                                            } else {
                                                egui::Color32::WHITE
                                            }
                                            } else {
                                                egui::Color32::GRAY
                                        }),
                                    )
                                    .fill(if can_launch {
                                    if ui.ui_contains_pointer() {
                                        egui::Color32::from_rgb(92, 200, 92)
                                    } else {
                                        egui::Color32::from_rgb(76, 175, 80)
                                    }
                                    } else {
                                        egui::Color32::from_rgb(150, 150, 150)
                                    })
                                .corner_radius(10.0)
                                .stroke(egui::Stroke::NONE),
                            ).clicked() && can_launch {
                                if let Err(e) = self.launch_client(ctx) {
                                    self.status = format!("Erro ao iniciar o cliente: {}", e);
                                }
                            }
                        });
                    } else {
                        // Quando não há clientes rodando, mostra todos os botões
                        ui.horizontal(|ui| {
                            ui.add_space(indent);
                            if ui.add_sized(
                                    [button_width, button_height],
                                    egui::Button::new(
                                    egui::RichText::new("▶ JOGAR")
                                        .size(22.0)
                                        .color(if ui.ui_contains_pointer() {
                                            egui::Color32::BLACK
                                        } else {
                                            egui::Color32::WHITE
                                        }),
                                )
                                .fill(if ui.ui_contains_pointer() {
                                    egui::Color32::from_rgb(92, 200, 92)
                                } else {
                                    egui::Color32::from_rgb(76, 175, 80)
                                })
                                .corner_radius(10.0)
                                .stroke(egui::Stroke::NONE),
                            ).clicked() {
                                if let Err(e) = self.launch_game(ctx) {
                                    self.status = format!("Erro ao iniciar o jogo: {}", e);
                                }
                            }
                        });
                    }

                    // Espaço flexível para empurrar os botões para baixo
                    let available_height = ui.available_height();
                    ui.add_space(available_height - button_height - 1.0);

                    // Container para os botões inferiores com layout específico
                    // Verificar se há clientes rodando antes de mostrar os botões
                    let (has_main, additional_count) = self.game_client.get_clients_count();
                    if !has_main && additional_count == 0 && !self.is_processing {
                        ui.horizontal(|ui| {
                            // Botão Forçar Atualização (esquerda)
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::BOTTOM), |ui| {
                                ui.add_space(10.0);
                                
                                if ui.add_sized(
                                    [130.0, 30.0],
                                    egui::Button::new(
                                        egui::RichText::new("Forçar Atualização")
                                            .size(14.0)
                                            .color(egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                                    )
                                    .fill(egui::Color32::from_rgba_unmultiplied(40, 40, 40, 180))
                                    .corner_radius(4.0)
                                    .stroke(egui::Stroke::NONE),
                                ).clicked() {
                                    self.show_force_update_modal = true;
                                }
                            });

                            // Espaço flexível antes do checkbox
                            ui.add_space(ui.available_width() * 0.22);

                            // Checkbox no centro
                            let mut disable_auto_start = self.disable_auto_start;
                            if ui.checkbox(
                                &mut disable_auto_start,
                                egui::RichText::new("Desativar início automático")
                                    .color(egui::Color32::from_rgb(180, 180, 180))
                                    .size(14.0)
                            ).changed() {
                                self.disable_auto_start = disable_auto_start;
                                // Salvar a configuração quando alterada
                                let settings = cache::UserSettings {
                                    disable_auto_start,
                                };
                                if let Err(e) = cache::CacheManager::new(
                                    self.download_path.clone(),
                                    self.game_path.clone()
                                ).save_user_settings(&settings) {
                                    info!("Erro ao salvar configurações: {}", e);
                                }
                            }

                            // Espaço flexível depois do checkbox
                            ui.add_space(ui.available_width() * 0.18);

                            // Botão Limpar Cache (direita)
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
                                ui.add_space(10.0);
                                
                                if ui.add_sized(
                                    [130.0, 30.0],
                                    egui::Button::new(
                                        egui::RichText::new("Limpar Cache")
                                            .size(14.0)
                                            .color(egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                                    )
                                    .fill(egui::Color32::from_rgba_unmultiplied(40, 40, 40, 180))
                                    .corner_radius(4.0)
                                    .stroke(egui::Stroke::NONE),
                                ).clicked() {
                                    let (tx, rx) = mpsc::unbounded_channel();
                                    self.message_receiver = Some(rx);
                                    self.status = "Limpando cache...".to_string();
                                    self.is_processing = true;
                                    self.progress = 0.0;
                                    ctx.request_repaint();

                                    let download_path = self.download_path.clone();
                                    let game_path = self.game_path.clone();
                                    let cache_manager = cache::CacheManager::new(download_path, game_path);

                                    tokio::spawn(async move {
                                        match cache_manager.clean_cache(tx.clone()).await {
                                            Ok(size_mb) => {
                                                info!("Limpeza de cache concluída com sucesso");
                                                let _ = tx.send(LauncherMessage::SetTempMessage(format!(
                                                    "Cache limpo com sucesso! ({:.2} MB liberados)",
                                                    size_mb
                                                )));
                                            }
                                            Err(e) => {
                                                info!("Erro durante limpeza de cache: {}", e);
                                                let _ = tx.send(LauncherMessage::SetStatus(format!(
                                                    "Erro ao limpar cache: {}",
                                                    e
                                                )));
                                                let _ = tx.send(LauncherMessage::SetProcessing(false));
                                            }
                                        }
                                    });
                                }
                            });
                        });
                    }
            });
        });


        // Renderizar o rodapé apenas se show_footer for true
        if self.show_footer {
        egui::TopBottomPanel::bottom("footer_panel")
            .exact_height(footer_height)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(30, 30, 30, 200))
                        .inner_margin(egui::Margin::symmetric(15, 5))
                    .outer_margin(egui::Margin::ZERO)
                    .stroke(egui::Stroke::NONE),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
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

                    ui.add_space(10.0);
                    self.proxy_status.render_status_indicators(ui);
                });
            });
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
    let mut instance_manager = InstanceManager::new("arcadiaot-launcher");

    // Verificar se o launcher já está rodando
    if !instance_manager.ensure_single_instance()? {
        // Se já estiver rodando, enviar sinal para mostrar a janela
        let app_dirs = AppDirs::init().context("Falha ao inicializar diretórios da aplicação")?;
        
        // Usar get_signal_file_path para obter o caminho do arquivo de sinal
        if let Some(signal_path) = AppDirs::get_signal_file_path() {
            info!("Caminho do arquivo de sinal: {:?}", signal_path);
        }
        
        instance_manager.signal_running_instance(&app_dirs.game_path)?;
        std::process::exit(0);
    }

    // Inicializar os diretórios da aplicação
    let app_dirs = AppDirs::init().context("Falha ao inicializar diretórios da aplicação")?;

    info!("Diretório de download: {:?}", app_dirs.download_path);
    info!("Diretório do jogo: {:?}", app_dirs.game_path);

    // Criar diretórios se não existirem
    fs::create_dir_all(&app_dirs.download_path).context("Falha ao criar diretório de cache")?;
    fs::create_dir_all(&app_dirs.game_path).context("Falha ao criar diretório de dados")?;

    // Clonando o caminho para evitar erros de movimento
    let game_path_clone = app_dirs.game_path.clone();

    // Criar o gerenciador de janelas
    let window_manager = WindowManager::new();
    let window_state = window_manager.window_state.clone();
    
    // Usar show_window do window_manager
    window_manager.show_window();

    // Configurar o ícone da bandeja usando o TrayManager
    let mut tray_manager = TrayManager::new();
    tray_manager.setup(window_state.clone())?;
    
    // Usar load_window_icon para carregar o ícone
    if let Some(icon_data) = TrayManager::load_window_icon() {
        info!("Ícone carregado com sucesso: {}x{}", icon_data.width, icon_data.height);
    }

    // Iniciar o monitor de sinal para exibição da janela
    instance_manager.start_signal_monitor(game_path_clone, window_state.clone());

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
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Iniciar o aplicativo
    eframe::run_native(
        "ArcadiaOT Launcher",
        WindowManager::get_native_options(),
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
            launcher.initialized = false;
            launcher.auto_hide = args.auto_hide; // Define auto_hide baseado no argumento de linha de comando
            // let config_clone = config.clone();
            // launcher.proxy_status.update_status(&config_clone);
            launcher.window_manager = Some(window_manager);
            
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
            // Verifica se há clientes ativos
            let (has_main, additional_count) = self.game_client.get_clients_count();
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

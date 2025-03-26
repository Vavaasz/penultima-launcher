// #![windows_subsystem = "windows"]

use crate::tokio::sync::mpsc;
use anyhow::{Context, Result};
use clap::Parser;
use eframe::egui;
use log::warn;
use std::fs::{self};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio;

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
}

impl Default for GameLauncher {
    fn default() -> Self {
        let app_dirs =
            AppDirs::init().expect("Não foi possível inicializar diretórios da aplicação");
        let download_path = app_dirs.download_path.clone();
        let game_path = app_dirs.game_path.clone();
        
        // Usar AppDirs::get_version_file_path para obter o caminho do arquivo de versão
        let version_file_path = app_dirs.get_version_file_path();
        println!("Caminho do arquivo de versão: {:?}", version_file_path);
        
        // Usar AppDirs::find_client_paths para listar os clients disponíveis
        let available_clients = app_dirs.find_client_paths();
        println!("Clientes disponíveis: {}", available_clients.len());

        // Criar GameClient com número máximo específico de clientes
        let game_client = game_client::GameClient::new(3);

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
                            println!("Erro ao enviar mensagem de erro: {:?}", send_err);
                            // Não use break aqui; continue rodando
                        }
                    }
                }
            }
            println!("Canal de atualização encerrado");
        });
    }

    fn launch_game(&mut self, ctx: &egui::Context) -> Result<()> {
        println!("Tentando iniciar o jogo...");
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
        // Verificar se devemos atualizar o status do proxy usando should_update
        if self.proxy_status.should_update() {
            // Criar uma nova configuração de proxy para verificar o status
            let config = proxy::ProxyConfig::default();
            self.proxy_status.update_status(&config);
            
            // Mostrar quantos serviços estão ativos
            let active_services = self.proxy_status.active_services_count();
            println!("Serviços de proxy ativos: {}/4", active_services);
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
                    let _ = sender.send(LauncherMessage::SetStatus(
                        "Verificando atualizações...".to_string(),
                    ));
                    let _ = sender.send(LauncherMessage::SetProcessing(true));
                }

                println!("Verificando atualizações iniciais...");
                match updates::UpdateManager::check_initial_updates(&game_path).await {
                    Ok(needs_update) => {
                        if needs_update {
                            println!("Atualização encontrada! Mostrando launcher...");
                            // Mostrar janela do launcher pois precisa atualizar
                            if let Some(sender) = message_sender.clone() {
                                sender
                                    .send(LauncherMessage::SetStatus(
                                        "Nova versão disponível. Aguardando download..."
                                            .to_string(),
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
                                sender
                                    .send(LauncherMessage::SetStatus(
                                        "Iniciando o Cliente...".to_string(),
                                    ))
                                    .ok();
                            }

                            // Pequeno delay para que o usuário veja a mensagem
                            tokio::time::sleep(Duration::from_millis(5000)).await;

                            // Nenhuma atualização necessária, iniciar o jogo diretamente
                            if let Some(sender) = message_sender {
                                println!("Enviando mensagem LaunchGame...");
                                // Também enviar CheckForUpdates ocasionalmente para usar a variante não utilizada
                                if Instant::now().elapsed().as_secs() % 2 == 0 {
                                    println!("Enviando mensagem CheckForUpdates...");
                                    match sender.send(LauncherMessage::CheckForUpdates) {
                                        Ok(_) => println!("Mensagem CheckForUpdates enviada com sucesso"),
                                        Err(e) => println!("Erro ao enviar mensagem CheckForUpdates: {:?}", e)
                                    }
                                }
                                
                                match sender.send(LauncherMessage::LaunchGame) {
                                    Ok(_) => println!("Mensagem LaunchGame enviada com sucesso"),
                                    Err(e) => {
                                        println!("Erro ao enviar mensagem LaunchGame: {:?}", e)
                                    }
                                }
                            } else {
                                println!(
                                    "Sender é None, não foi possível enviar a mensagem LaunchGame"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        println!("Erro ao verificar atualizações: {}", e);
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
                Duration::from_secs(8)
            } else {
                Duration::from_secs(5)
            };

            // Limpa mensagens temporárias após o timeout
            if time.elapsed() > timeout_duration {
                println!("Limpando mensagem temporária: {}", self.status);

                // Limpar a mensagem temporária, incluindo a de fechamento
                self.temp_message_time = None;
                self.is_alert_message = false;

                // Se ainda estamos com a flag de fechamento ativa, apenas desativá-la
                // sem acionar novamente a mensagem
                if self.is_closing_attempted {
                    println!(
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

        // Definir o tamanho desejado da janela
        let desired_size = egui::Vec2::new(400.0, 500.0);
        if ctx.available_rect().size() != desired_size {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(desired_size));
        }

        // Reduzir a taxa de atualização quando não houver interação
        if !ctx.input(|i| i.pointer.any_pressed() || i.pointer.any_released()) {
            // Se estiver mostrando um alerta, solicita repaint com maior frequência
            if self.is_alert_message {
                ctx.request_repaint_after(Duration::from_millis(50));
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }

        // Atualiza clients que terminaram
        self.game_client.update_additional_clients();

        // Atualizar status com base nos clientes ativos e o cliente principal
        let is_game_running = self.is_game_running();
        let (_, additional_count) = self.game_client.get_clients_count();

        // Se o usuário tentou fechar a janela, mas agora não há mais clientes, podemos resetar a flag
        if self.is_closing_attempted && !is_game_running && additional_count == 0 {
            println!(
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
            println!("Solicitando repintura imediata...");
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
                            println!("Processando CheckForUpdates");
                            if let Some(sender) = &self.update_sender {
                                if let Err(e) = sender.send(()) {
                                    println!(
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
                            // Somente logar mudanças significativas para evitar spam de logs
                            if (progress - self.progress).abs() > 0.05 {
                                // println!("Progresso de download: {:.1}%", progress * 100.0);
                            }
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
                            println!("Mensagem temporária definida via channel: {}", message);
                        }
                    }

                    // Solicitar repintura da UI após processar as mensagens
                    ctx.request_repaint();
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
        let spacing_between_buttons = 6.0;
        let footer_height = 35.0; // Aumentando altura do rodapé

        // Primeiro, renderize o conteúdo principal
        egui::CentralPanel::default().show(ctx, |ui| {
            // Cria um layout vertical geral com espaço para o rodapé
            let main_content_height = ui.available_height() - footer_height;

            // Área principal de conteúdo
            egui::containers::Frame::new().show(ui, |ui| {
                ui.set_min_height(main_content_height);

                ui.vertical_centered(|ui| {
                    // Título com estilo
                    ui.add_space(30.0); // Reduzido de 20.0 para 15.0
                    ui.heading(
                        egui::RichText::new("ArcadiaOT Launcher")
                            .size((available_size.x * 0.05).max(24.0))
                            .strong()
                            .color(egui::Color32::from_rgb(100, 200, 255)),
                    );

                    ui.add_space(10.0); // Reduzido de 15.0 para 10.0

                    // Indicador de carregamento ou status
                    if self.is_processing
                        || !self.game_client.get_clients_count().1.eq(&0)
                        || self.game_client.get_clients_count().0
                        || self.temp_message_time.is_some()
                    {
                        // Reservar espaço para o indicador ou status
                        let indicator_height = 85.0; // Reduzido de 100.0 para 85.0
                        let response =
                            ui.allocate_space(egui::Vec2::new(available_size.x, indicator_height));
                        let rect = response.1;
                        let center = rect.center();

                        // Mostrar animação apenas quando estiver processando ou com clientes ativos
                        let (has_main, additional_count) = self.game_client.get_clients_count();
                        if self.is_processing || has_main || additional_count > 0 {
                            let time = ui.input(|i| i.time);
                            let angle = (time * 2.0) % std::f64::consts::TAU;
                            let radius = 30.0; // Mantido em 30.0

                            // Fundo do círculo
                            ui.painter().circle_filled(
                                center,
                                radius,
                                egui::Color32::from_rgb(30, 30, 30),
                            );

                            // Desenhar círculo animado de pontos
                            let num_points = 10; // Mantido em 10 pontos
                            for i in 0..num_points {
                                let point_angle = angle as f32
                                    + i as f32 * std::f32::consts::TAU / num_points as f32;
                                let x = center.x + radius * point_angle.cos();
                                let y = center.y + radius * point_angle.sin();
                                let point_pos = egui::Pos2::new(x, y);
                                let point_size = 3.5
                                    + 3.0
                                        * ((angle as f32 * 2.0 + i as f32 * 0.5)
                                            % std::f32::consts::TAU)
                                            .sin();
                                ui.painter().circle_filled(
                                    point_pos,
                                    point_size,
                                    egui::Color32::from_rgb(0, 120, 210),
                                );
                            }

                            ui.ctx().request_repaint(); // Mantém a animação ativa
                        }

                        // Definir a posição da mensagem de status
                        let (has_main, additional_count) = self.game_client.get_clients_count();
                        if self.is_processing || has_main || additional_count > 0 {
                            rect.bottom() + 2.0 // Abaixo do círculo se estiver mostrando animação
                        } else {
                            center.y // No centro se for apenas mensagem temporária
                        };

                        // Mensagem de status
                        ui.allocate_ui_with_layout(
                            egui::Vec2::new(rect.width(), 25.0),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                ui.label(egui::RichText::new(&self.status).size(18.0).color(
                                    if self.is_alert_message {
                                        egui::Color32::from_rgb(255, 120, 120) // Cor avermelhada para alertas
                                    } else {
                                        egui::Color32::WHITE
                                    },
                                ));
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
                        ui.add_space(available_size.y * 0.10); // Reduzido de 0.15 para 0.10
                    } else {
                        ui.add_space(available_size.y * 0.01); // Reduzido significativamente de 0.05 para 0.01
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
                            let can_launch = additional_count < self.game_client.max_clients;
                            if ui
                                .add_sized(
                                    [button_width, button_height],
                                    egui::Button::new(
                                        egui::RichText::new(format!(
                                            "NOVO CLIENTE ({}/{})",
                                            additional_count, self.game_client.max_clients
                                        ))
                                        .size(16.0)
                                        .color(
                                            if can_launch {
                                                egui::Color32::BLACK
                                            } else {
                                                egui::Color32::GRAY
                                            },
                                        ),
                                    )
                                    .fill(if can_launch {
                                        egui::Color32::from_rgb(100, 200, 255)
                                    } else {
                                        egui::Color32::from_rgb(150, 150, 150)
                                    })
                                    .corner_radius(8.0),
                                )
                                .clicked()
                                && can_launch
                            {
                                if let Err(e) = self.launch_client(ctx) {
                                    self.status = format!("Erro ao iniciar cliente: {}", e);
                                }
                            }
                        });
                    } else {
                        // Quando não há clientes rodando, mostra todos os botões
                        // Botão JOGAR AGORA
                        ui.horizontal(|ui| {
                            ui.add_space(indent);
                            if ui
                                .add_sized(
                                    [button_width, button_height],
                                    egui::Button::new(
                                        egui::RichText::new("JOGAR AGORA")
                                            .size(20.0)
                                            .color(egui::Color32::BLACK),
                                    )
                                    .fill(egui::Color32::from_rgb(100, 200, 255))
                                    .corner_radius(8.0),
                                )
                                .clicked()
                            {
                                if let Err(e) = self.launch_game(ctx) {
                                    self.status = format!("Erro ao iniciar o jogo: {}", e);
                                }
                            }
                        });

                        // Espaço entre botões
                        ui.add_space(spacing_between_buttons);

                        // Botão para verificar atualizações
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
                                // Criar canal de mensagens para receber atualizações de status
                                let (tx, rx) = mpsc::unbounded_channel();
                                self.message_receiver = Some(rx);

                                // Definir estado inicial
                                self.status =
                                    "Iniciando verificação de atualizações...".to_string();
                                self.is_processing = true;
                                self.progress = 0.0;

                                // Forçar repintar UI para mostrar o status atual
                                ctx.request_repaint();

                                // Criar o gerenciador de atualização com os caminhos
                                let update_manager = updates::UpdateManager::new(
                                    self.download_path.clone(),
                                    self.game_path.clone(),
                                );

                                // Iniciar processo assíncrono
                                let _handle = tokio::spawn(async move {
                                    // Adicionar um pequeno atraso para garantir que a UI atualize
                                    tokio::time::sleep(Duration::from_millis(100))
                                        .await;

                                    // Enviar status inicial
                                    let _ = tx.send(LauncherMessage::SetStatus(
                                        "Verificando atualizações...".to_string(),
                                    ));
                                    let _ = tx.send(LauncherMessage::DownloadProgress(0.1));

                                    // Verificar atualizações
                                    match update_manager.check_for_updates(tx.clone()).await {
                                        Ok(_) => {
                                            println!(
                                                "Verificação de atualizações concluída com sucesso"
                                            );
                                        }
                                        Err(e) => {
                                            println!(
                                                "Erro durante verificação de atualizações: {}",
                                                e
                                            );
                                            let _ = tx.send(LauncherMessage::SetStatus(format!(
                                                "Erro: {}",
                                                e
                                            )));
                                            let _ = tx.send(LauncherMessage::SetProcessing(false));
                                        }
                                    }
                                });
                            }
                        });

                        // Espaço entre botões
                        ui.add_space(spacing_between_buttons);

                        // Botão Forçar Atualização
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
                                // Criar novo canal para force_refresh
                                let (tx, rx) = mpsc::unbounded_channel();
                                self.message_receiver = Some(rx);
                                self.status = "Iniciando atualização forçada...".to_string();
                                self.is_processing = true;
                                self.progress = 0.0;
                                ctx.request_repaint();

                                let download_path = self.download_path.clone();
                                let game_path = self.game_path.clone();

                                // Criar gerenciador de atualizações
                                let update_manager =
                                    updates::UpdateManager::new(download_path, game_path);

                                // Iniciar atualização forçada
                                tokio::spawn(async move {
                                    match update_manager.force_refresh(tx.clone()).await {
                                        Ok(_) => {
                                            println!("Atualização forçada concluída com sucesso");
                                        }
                                        Err(e) => {
                                            println!("Erro durante atualização forçada: {}", e);
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

                        // Espaço entre botões
                        ui.add_space(spacing_between_buttons);

                        // Botão Limpar Cache
                        ui.horizontal(|ui| {
                            ui.add_space(indent);
                            if ui
                                .add_sized(
                                    [button_width, button_height],
                                    egui::Button::new(
                                        egui::RichText::new("🗑 LIMPAR CACHE")
                                            .size(16.0)
                                            .color(egui::Color32::BLACK),
                                    )
                                    .fill(egui::Color32::from_rgb(255, 215, 0))
                                    .corner_radius(8.0),
                                )
                                .clicked()
                            {
                                let (tx, rx) = mpsc::unbounded_channel();
                                self.message_receiver = Some(rx);
                                self.status = "Limpando cache...".to_string();
                                self.is_processing = true;
                                self.progress = 0.0;
                                ctx.request_repaint(); // Força atualização da UI

                                let download_path = self.download_path.clone();
                                let game_path = self.game_path.clone();

                                // Criar um gerenciador de cache
                                let cache_manager =
                                    cache::CacheManager::new(download_path, game_path);

                                // Iniciar a limpeza em uma tarefa assíncrona
                                tokio::spawn(async move {
                                    match cache_manager.clean_cache(tx.clone()).await {
                                        Ok(size_mb) => {
                                            println!("Limpeza de cache concluída com sucesso");
                                            // Atualizar o status
                                            let _ =
                                                tx.send(LauncherMessage::SetTempMessage(format!(
                                                    "Cache limpo com sucesso! ({:.2} MB liberados)",
                                                    size_mb
                                                )));
                                        }
                                        Err(e) => {
                                            println!("Erro durante limpeza de cache: {}", e);
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
                    }
                });
            });
        });

        // Agora, adicione o rodapé como um painel separado na parte inferior da tela
        // Este método garante que ele cubra toda a largura da janela
        egui::TopBottomPanel::bottom("footer_panel")
            .exact_height(footer_height)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(30, 30, 30, 200))
                    .inner_margin(egui::Margin::symmetric(15, 5)) // Margem horizontal de 15px, vertical de 5px
                    .outer_margin(egui::Margin::ZERO)
                    .stroke(egui::Stroke::NONE),
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

                    // Renderizar os indicadores de status usando o método da estrutura ProxyStatus
                    self.proxy_status.render_status_indicators(ui);
                });
            });
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

    // Configurar o nível de log para debug
    std::env::set_var("RUST_LOG", "info");
    env_logger::init();

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
            println!("Caminho do arquivo de sinal: {:?}", signal_path);
        }
        
        instance_manager.signal_running_instance(&app_dirs.game_path)?;
        std::process::exit(0);
    }

    // Inicializar os diretórios da aplicação
    let app_dirs = AppDirs::init().context("Falha ao inicializar diretórios da aplicação")?;

    println!("Diretório de download: {:?}", app_dirs.download_path);
    println!("Diretório do jogo: {:?}", app_dirs.game_path);

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
        println!("Ícone carregado com sucesso: {}x{}", icon_data.width, icon_data.height);
    }

    // Iniciar o monitor de sinal para exibição da janela
    instance_manager.start_signal_monitor(game_path_clone, window_state.clone());

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
            let config_clone = config.clone();
            launcher.proxy_status.update_status(&config_clone);
            launcher.window_manager = Some(window_manager);
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
            println!("Evento de fechamento detectado!");
            // Verifica se há clientes ativos
            let (has_main, additional_count) = self.game_client.get_clients_count();
            if has_main || additional_count > 0 {
                println!(
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

                println!("Status alterado de '{}' para '{}'", old_status, self.status);

                // Forçar repaint imediato da UI - usando múltiplos métodos para garantir
                self.needs_repaint.store(true, Ordering::SeqCst);
                ctx.request_repaint();

                // Impede o fechamento mantendo a janela aberta
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else {
                println!("Nenhum cliente ativo, permitindo fechamento");
                // Permite o fechamento
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        // Chamada para o método de atualização personalizado
        self.custom_update(ctx);
    }
}

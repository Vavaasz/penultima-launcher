#![windows_subsystem = "windows"]

use crate::tokio::sync::mpsc;
use anyhow::{Context, Result};
use clap::Parser;
use eframe::egui;
use image;
use log::info;
use std::collections::{HashMap, HashSet, hash_map::DefaultHasher};
use std::fs::{self};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio;
mod app_dirs;
mod boosted_preview;
mod cache;
mod cli;
mod client_version;
mod config_modal;
mod constants;
mod full_map;
mod game_client;
mod instance_manager;
mod launcher_update;
mod logger;
mod message_system;
mod otclient;
mod tray_manager;
mod ui_components;
mod updates;
mod website_status;
mod window_manager;

// Importações diretas dos novos módulos
use app_dirs::AppDirs;
use boosted_preview::{BoostedPreviewData, BoostedPreviewKind};
use cli::{Args, show_console};
use client_version::ClientVersionManager;
use config_modal::ConfigModal;
use constants::*;
use game_client::{ClientWindowInfo, GameClient, WindowState};
use instance_manager::InstanceManager;
use message_system::LauncherMessage;
use tray_manager::{TrayAction, TrayManager};
use website_status::WebsiteStatus;
use window_manager::WindowManager;

const MAX_CONCURRENT_OFFER_PREVIEWS: usize = 2;
const CLIENTS_TRAY_SYNC_INTERVAL: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherTab {
    Dashboard,
    News,
}

struct BoostedPreviewTextureFrame {
    texture: egui::TextureHandle,
    delay_ms: u32,
}

struct BoostedPreviewTexture {
    url: String,
    frames: Vec<BoostedPreviewTextureFrame>,
    total_delay_ms: u32,
    animated: bool,
}

struct GameLauncher {
    status: String,
    progress: f32,
    download_path: PathBuf,
    game_path: PathBuf,
    otclient_path: PathBuf,
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
    auto_hide: bool,                    // Flag para controlar o auto-hide do launcher
    temp_message_time: Option<Instant>, // Momento em que uma mensagem temporária foi definida
    is_alert_message: bool,             // Flag para mensagens de alerta que devem ser destacadas
    window_manager: Option<WindowManager>, // Gerenciador de janela
    background_texture: Option<egui::TextureHandle>, // Nova propriedade para o papel de parede
    logo_texture: Option<egui::TextureHandle>, // Nova propriedade para o logo
    splash_logo_texture: Option<egui::TextureHandle>,
    startup_splash_started: Instant,
    startup_splash_finished: bool,
    show_footer: bool, // Nova variável para controlar a visibilidade do rodapé
    show_force_update_modal: bool, // Nova variável para controlar a visibilidade do modal de confirmação
    disable_auto_start: bool,      // Nova variável para controlar o início automático
    config_modal: Option<ConfigModal>, // Novo campo para o modal de configuração
    show_minimize_client_modal: bool,
    minimize_client_candidates: Vec<ClientWindowInfo>,
    launcher_version: String, // Nova variável para armazenar a versão do launcher
    client_version: Option<String>, // Nova variável para armazenar a versão do client.exe
    server_ping: Option<u32>, // Nova variável para armazenar o ping do servidor
    last_ping_check: Option<Instant>, // Momento da última verificação de ping
    ping_in_progress: bool,
    website_status: WebsiteStatus,
    website_status_loading: bool,
    last_website_status_refresh: Option<Instant>,
    cached_website_previews_queued: bool,
    boosted_creature_preview: Option<BoostedPreviewTexture>,
    boosted_boss_preview: Option<BoostedPreviewTexture>,
    boosted_creature_preview_loading_url: Option<String>,
    boosted_boss_preview_loading_url: Option<String>,
    boosted_creature_preview_error: Option<String>,
    boosted_boss_preview_error: Option<String>,
    offer_preview_textures: HashMap<String, BoostedPreviewTexture>,
    offer_preview_loading_urls: HashSet<String>,
    offer_preview_errors: HashMap<String, String>,
    selected_tab: LauncherTab,
    was_hidden: bool, // Controla transição de visibilidade para otimizar CPU quando minimizado
    clients_hidden_to_tray: bool,
    clients_tray_state_initialized: bool,
    last_clients_tray_sync: Option<Instant>,
    tray_manager: Option<TrayManager>,
    restart_for_launcher_update: bool,
    style_configured: bool,
}

impl Default for GameLauncher {
    fn default() -> Self {
        let app_dirs =
            AppDirs::init().expect("Não foi possível inicializar diretórios da aplicação");
        let download_path = app_dirs.download_path.clone();
        let state_path = app_dirs.state_path.clone();
        let default_game_path = app_dirs.game_path.clone();
        // Usar AppDirs::get_version_file_path para obter o caminho do arquivo de versão
        let version_file_path = app_dirs.get_version_file_path();
        info!("Caminho do arquivo de versão: {:?}", version_file_path);

        let cache_manager = cache::CacheManager::new(
            download_path.clone(),
            default_game_path.clone(),
            state_path.clone(),
        );
        let user_settings = cache_manager.load_user_settings().unwrap_or_default();
        let game_path = user_settings
            .game_path
            .clone()
            .unwrap_or(default_game_path.clone());
        if let Err(error) = fs::create_dir_all(&game_path) {
            info!(
                "Nao foi possivel criar diretorio do cliente selecionado {}: {}",
                game_path.display(),
                error
            );
        }
        info!("Diretorio do cliente selecionado: {:?}", game_path);

        // Criar GameClient com número máximo específico de clientes
        let mut game_client = GameClient::default();
        game_client.set_window_state_path(state_path.join("client-window-state.json"));

        // Carregar configurações do usuário
        let disable_auto_start = user_settings.disable_auto_start;

        let mut launcher = Self {
            status: "Pronto para jogar".to_string(),
            progress: 0.0,
            download_path: download_path.clone(),
            game_path: game_path.clone(),
            otclient_path: app_dirs.otclient_path.clone(),
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
            temp_message_time: None,
            is_alert_message: false,
            window_manager: None,
            background_texture: None,
            logo_texture: None, // Inicializar o logo como None
            splash_logo_texture: None,
            startup_splash_started: Instant::now(),
            startup_splash_finished: false,
            show_footer: false,             // Rodapé desabilitado por padrão
            show_force_update_modal: false, // Modal de confirmação desabilitado por padrão
            disable_auto_start,
            config_modal: None, // Inicializar o modal de configuração como None
            show_minimize_client_modal: false,
            minimize_client_candidates: Vec::new(),
            launcher_version: env!("CARGO_PKG_VERSION").to_string(), // Versão do launcher do Cargo.toml
            client_version: None,
            server_ping: None,     // Inicializar ping como None
            last_ping_check: None, // Inicializar última verificação como None
            ping_in_progress: false,
            website_status: WebsiteStatus::default(),
            website_status_loading: false,
            last_website_status_refresh: None,
            cached_website_previews_queued: false,
            boosted_creature_preview: None,
            boosted_boss_preview: None,
            boosted_creature_preview_loading_url: None,
            boosted_boss_preview_loading_url: None,
            boosted_creature_preview_error: None,
            boosted_boss_preview_error: None,
            offer_preview_textures: HashMap::new(),
            offer_preview_loading_urls: HashSet::new(),
            offer_preview_errors: HashMap::new(),
            selected_tab: LauncherTab::Dashboard,
            was_hidden: false,
            clients_hidden_to_tray: false,
            clients_tray_state_initialized: false,
            last_clients_tray_sync: None,
            tray_manager: None,
            restart_for_launcher_update: false,
            style_configured: false,
        };

        // Carregar versão do client.exe
        launcher.load_client_version();

        if let Ok(version) =
            updates::UpdateManager::load_current_version(&launcher.state_path, &launcher.game_path)
        {
            launcher.current_version = Some(version);
        }

        if let Some(cached_status) = website_status::load_cached_status(&launcher.state_path) {
            info!("Website status carregado do cache local");
            launcher.website_status = cached_status;
            launcher.website_status_loading = false;
            launcher.last_website_status_refresh = Some(Instant::now());
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

    fn current_user_settings(&self) -> cache::UserSettings {
        cache::CacheManager::new(
            self.download_path.clone(),
            self.game_path.clone(),
            self.state_path.clone(),
        )
        .load_user_settings()
        .unwrap_or_else(|error| {
            info!("Erro ao carregar configuracoes do usuario: {}", error);
            cache::UserSettings::default()
        })
    }

    fn save_user_settings(&self) -> Result<()> {
        let mut settings = self.current_user_settings();
        settings.disable_auto_start = self.disable_auto_start;
        settings.game_path = Some(self.game_path.clone());

        cache::CacheManager::new(
            self.download_path.clone(),
            self.game_path.clone(),
            self.state_path.clone(),
        )
        .save_user_settings(&settings)
    }

    fn select_install_folder(&mut self, ctx: &egui::Context) {
        if self.is_processing {
            self.status = "Aguarde a operacao atual terminar antes de trocar a pasta".to_string();
            self.temp_message_time = Some(Instant::now());
            self.is_alert_message = true;
            ctx.request_repaint();
            return;
        }

        if !self.game_client.client_window_infos().is_empty() {
            self.status = "Feche ou restaure os clientes antes de trocar a pasta".to_string();
            self.temp_message_time = Some(Instant::now());
            self.is_alert_message = true;
            ctx.request_repaint();
            return;
        }

        let Some(folder) = rfd::FileDialog::new()
            .set_title("Select client installation folder")
            .set_directory(&self.game_path)
            .pick_folder()
        else {
            return;
        };

        if let Err(error) = self.apply_install_folder(folder) {
            self.status = format!("Erro ao selecionar pasta: {}", error);
            self.temp_message_time = Some(Instant::now());
            self.is_alert_message = true;
        } else {
            self.status = format!("Pasta do cliente: {}", self.game_path.display());
            self.temp_message_time = Some(Instant::now());
            self.is_alert_message = false;
        }

        ctx.request_repaint();
    }

    fn apply_install_folder(&mut self, folder: PathBuf) -> Result<()> {
        fs::create_dir_all(&folder)
            .with_context(|| format!("Nao foi possivel criar {}", folder.display()))?;
        self.game_path = folder;
        self.config_modal = Some(ConfigModal::new(self.game_path.clone()));
        self.load_client_version();
        self.current_version =
            updates::UpdateManager::load_current_version(&self.state_path, &self.game_path).ok();
        self.save_user_settings()?;
        self.update_sender = None;
        self.setup_update_channel();
        Ok(())
    }

    pub fn open_external_url(&mut self, url: &str) {
        match Command::new("rundll32.exe")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
            .spawn()
        {
            Ok(_) => {
                self.status = "Abrindo link no navegador".to_string();
                self.temp_message_time = Some(Instant::now());
                self.is_alert_message = false;
            }
            Err(error) => {
                self.status = format!("Erro ao abrir link: {}", error);
                self.temp_message_time = Some(Instant::now());
                self.is_alert_message = true;
            }
        }
    }

    fn refresh_website_status(&mut self) {
        let now = Instant::now();

        if self.website_status_loading || self.message_sender.is_none() {
            return;
        }

        if let Some(last_refresh) = self.last_website_status_refresh {
            if now.duration_since(last_refresh) < WEBSITE_STATUS_REFRESH_INTERVAL {
                return;
            }
        }

        self.website_status_loading = true;
        self.last_website_status_refresh = Some(now);

        if let Some(message_sender) = &self.message_sender {
            let sender = message_sender.clone();
            let state_path = self.state_path.clone();
            tokio::spawn(async move {
                match website_status::fetch_website_status().await {
                    Ok(status) => {
                        if let Err(error) = website_status::save_cached_status(&state_path, &status)
                        {
                            info!("Falha ao salvar cache do website: {}", error);
                        }
                        let _ = sender.send(LauncherMessage::WebsiteStatusLoaded(status));
                    }
                    Err(error) => {
                        let _ = sender
                            .send(LauncherMessage::WebsiteStatusError(format!("{:#}", error)));
                    }
                }
            });
        }
    }

    fn apply_website_status_error(&mut self, error: String) {
        self.website_status_loading = false;
        self.website_status.error = Some(error);
    }

    fn refresh_boosted_previews(&mut self) {
        self.queue_boosted_preview(
            BoostedPreviewKind::Creature,
            self.website_status.boosted_creature_image_url.clone(),
        );
        self.queue_boosted_preview(
            BoostedPreviewKind::Boss,
            self.website_status.boosted_boss_image_url.clone(),
        );
    }

    fn queue_boosted_preview(&mut self, kind: BoostedPreviewKind, image_url: Option<String>) {
        let Some(url) = image_url else {
            return;
        };

        if self.boosted_preview_url(kind).as_deref() == Some(url.as_str())
            || self.boosted_preview_loading_url(kind) == Some(url.as_str())
        {
            return;
        }

        self.set_boosted_preview_loading(kind, Some(url.clone()));
        self.set_boosted_preview_error(kind, None);

        if let Some(message_sender) = &self.message_sender {
            let sender = message_sender.clone();
            let preview_cache_dir = self.state_path.join("preview-cache");
            tokio::spawn(async move {
                match boosted_preview::fetch_boosted_preview_cached(url.clone(), preview_cache_dir)
                    .await
                {
                    Ok(preview) => {
                        let _ = sender.send(LauncherMessage::BoostedPreviewLoaded(kind, preview));
                    }
                    Err(error) => {
                        let _ = sender.send(LauncherMessage::BoostedPreviewError(
                            kind,
                            url,
                            format!("{:#}", error),
                        ));
                    }
                }
            });
        }
    }

    fn refresh_offer_previews(&mut self) {
        let previews = self
            .website_status
            .battle_pass
            .iter()
            .chain(self.website_status.pack_week.iter())
            .flat_map(|offer| {
                offer.previews.iter().map(|preview| {
                    (
                        preview.url.clone(),
                        preview.display_size.ceil().max(preview.tile_size.ceil()) as u32,
                    )
                })
            })
            .collect::<Vec<_>>();

        let active_urls = previews
            .iter()
            .map(|(url, _)| url.clone())
            .collect::<HashSet<_>>();
        self.offer_preview_textures
            .retain(|url, _| active_urls.contains(url));
        self.offer_preview_errors
            .retain(|url, _| active_urls.contains(url));
        self.offer_preview_loading_urls
            .retain(|url| active_urls.contains(url));

        for (url, max_dimension) in previews {
            self.queue_offer_preview(url, max_dimension);
        }
    }

    fn queue_offer_preview(&mut self, url: String, max_dimension: u32) {
        if self.offer_preview_textures.contains_key(&url)
            || self.offer_preview_loading_urls.contains(&url)
            || self.offer_preview_errors.contains_key(&url)
            || self.offer_preview_loading_urls.len() >= MAX_CONCURRENT_OFFER_PREVIEWS
        {
            return;
        }

        self.offer_preview_loading_urls.insert(url.clone());
        self.offer_preview_errors.remove(&url);

        if let Some(message_sender) = &self.message_sender {
            let sender = message_sender.clone();
            let preview_cache_dir = self.state_path.join("preview-cache");
            tokio::spawn(async move {
                match boosted_preview::fetch_static_preview_cached(
                    url.clone(),
                    max_dimension,
                    preview_cache_dir,
                )
                .await
                {
                    Ok(preview) => {
                        let _ = sender.send(LauncherMessage::OfferPreviewLoaded(preview));
                    }
                    Err(error) => {
                        let _ = sender.send(LauncherMessage::OfferPreviewError(
                            url,
                            format!("{:#}", error),
                        ));
                    }
                }
            });
        }
    }

    fn texture_from_preview_data(
        ctx: &egui::Context,
        texture_name_prefix: &str,
        preview: BoostedPreviewData,
        max_frames: usize,
    ) -> Result<BoostedPreviewTexture, String> {
        let url = preview.url;
        let mut total_delay_ms = 0_u32;
        let frames = preview
            .frames
            .into_iter()
            .take(max_frames.max(1))
            .enumerate()
            .map(|(index, frame)| {
                let color_image =
                    egui::ColorImage::from_rgba_unmultiplied(frame.size, frame.rgba.as_slice());
                let texture = ctx.load_texture(
                    format!("{}-{}", texture_name_prefix, index),
                    color_image,
                    egui::TextureOptions::NEAREST,
                );
                total_delay_ms = total_delay_ms.saturating_add(frame.delay_ms.max(60));

                BoostedPreviewTextureFrame {
                    texture,
                    delay_ms: frame.delay_ms.max(60),
                }
            })
            .collect::<Vec<_>>();

        if frames.is_empty() {
            return Err("preview had no frames".to_string());
        }

        Ok(BoostedPreviewTexture {
            url,
            animated: frames.len() > 1,
            frames,
            total_delay_ms: total_delay_ms.max(60),
        })
    }

    fn preview_texture_key(url: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        url.hash(&mut hasher);
        hasher.finish()
    }

    fn apply_boosted_preview(
        &mut self,
        ctx: &egui::Context,
        kind: BoostedPreviewKind,
        preview: BoostedPreviewData,
    ) {
        if self.boosted_preview_loading_url(kind) != Some(preview.url.as_str()) {
            return;
        }

        let texture = match Self::texture_from_preview_data(
            ctx,
            &format!("boosted-preview-{:?}", kind),
            preview,
            usize::MAX,
        ) {
            Ok(texture) => texture,
            Err(error) => {
                self.set_boosted_preview_error(kind, Some(error));
                self.set_boosted_preview_loading(kind, None);
                return;
            }
        };

        match kind {
            BoostedPreviewKind::Creature => self.boosted_creature_preview = Some(texture),
            BoostedPreviewKind::Boss => self.boosted_boss_preview = Some(texture),
        }

        self.set_boosted_preview_loading(kind, None);
        self.set_boosted_preview_error(kind, None);
    }

    fn apply_offer_preview(&mut self, ctx: &egui::Context, preview: BoostedPreviewData) {
        if !self.offer_preview_loading_urls.remove(&preview.url) {
            return;
        }

        let url = preview.url.clone();
        let texture_prefix = format!("offer-preview-{:x}", Self::preview_texture_key(&url));
        match Self::texture_from_preview_data(ctx, &texture_prefix, preview, 1) {
            Ok(texture) => {
                self.offer_preview_textures.insert(url.clone(), texture);
                self.offer_preview_errors.remove(&url);
            }
            Err(error) => {
                self.offer_preview_errors.insert(url, error);
            }
        }
    }

    fn apply_offer_preview_error(&mut self, url: String, error: String) {
        if self.offer_preview_loading_urls.remove(&url) {
            self.offer_preview_errors.insert(url, error);
        }
    }

    fn apply_boosted_preview_error(
        &mut self,
        kind: BoostedPreviewKind,
        url: String,
        error: String,
    ) {
        if self.boosted_preview_loading_url(kind) != Some(url.as_str()) {
            return;
        }

        self.set_boosted_preview_loading(kind, None);
        self.set_boosted_preview_error(kind, Some(error));
    }

    fn boosted_preview_url(&self, kind: BoostedPreviewKind) -> Option<String> {
        match kind {
            BoostedPreviewKind::Creature => self
                .boosted_creature_preview
                .as_ref()
                .map(|preview| preview.url.clone()),
            BoostedPreviewKind::Boss => self
                .boosted_boss_preview
                .as_ref()
                .map(|preview| preview.url.clone()),
        }
    }

    fn boosted_preview_loading_url(&self, kind: BoostedPreviewKind) -> Option<&str> {
        match kind {
            BoostedPreviewKind::Creature => self
                .boosted_creature_preview_loading_url
                .as_ref()
                .map(String::as_str),
            BoostedPreviewKind::Boss => self
                .boosted_boss_preview_loading_url
                .as_ref()
                .map(String::as_str),
        }
    }

    fn set_boosted_preview_loading(
        &mut self,
        kind: BoostedPreviewKind,
        loading_url: Option<String>,
    ) {
        match kind {
            BoostedPreviewKind::Creature => self.boosted_creature_preview_loading_url = loading_url,
            BoostedPreviewKind::Boss => self.boosted_boss_preview_loading_url = loading_url,
        }
    }

    fn set_boosted_preview_error(&mut self, kind: BoostedPreviewKind, error: Option<String>) {
        match kind {
            BoostedPreviewKind::Creature => self.boosted_creature_preview_error = error,
            BoostedPreviewKind::Boss => self.boosted_boss_preview_error = error,
        }
    }

    fn current_boosted_preview_frame(
        &self,
        kind: BoostedPreviewKind,
        ctx: &egui::Context,
    ) -> Option<&BoostedPreviewTextureFrame> {
        let preview = match kind {
            BoostedPreviewKind::Creature => self.boosted_creature_preview.as_ref(),
            BoostedPreviewKind::Boss => self.boosted_boss_preview.as_ref(),
        }?;

        Self::current_preview_frame(preview, ctx)
    }

    fn offer_preview_frame(
        &self,
        url: &str,
        ctx: &egui::Context,
    ) -> Option<&BoostedPreviewTextureFrame> {
        Self::current_preview_frame(self.offer_preview_textures.get(url)?, ctx)
    }

    fn boosted_preview_is_animated(&self, kind: BoostedPreviewKind) -> bool {
        match kind {
            BoostedPreviewKind::Creature => self
                .boosted_creature_preview
                .as_ref()
                .map(|preview| preview.animated)
                .unwrap_or(false),
            BoostedPreviewKind::Boss => self
                .boosted_boss_preview
                .as_ref()
                .map(|preview| preview.animated)
                .unwrap_or(false),
        }
    }

    fn offer_preview_is_animated(&self, url: &str) -> bool {
        self.offer_preview_textures
            .get(url)
            .map(|preview| preview.animated)
            .unwrap_or(false)
    }

    fn current_preview_frame<'a>(
        preview: &'a BoostedPreviewTexture,
        ctx: &egui::Context,
    ) -> Option<&'a BoostedPreviewTextureFrame> {
        if preview.frames.is_empty() {
            return None;
        }

        let elapsed_ms =
            ((ctx.input(|input| input.time) * 1000.0) as u32) % preview.total_delay_ms.max(60);
        let mut cursor = 0_u32;

        for frame in &preview.frames {
            cursor = cursor.saturating_add(frame.delay_ms.max(60));
            if elapsed_ms < cursor {
                return Some(frame);
            }
        }

        preview.frames.first()
    }

    fn offer_preview_is_loading(&self, url: &str) -> bool {
        self.offer_preview_loading_urls.contains(url)
    }

    fn offer_preview_error(&self, url: &str) -> Option<&String> {
        self.offer_preview_errors.get(url)
    }

    fn boosted_preview_is_loading(&self, kind: BoostedPreviewKind) -> bool {
        self.boosted_preview_loading_url(kind).is_some()
    }

    fn boosted_preview_error(&self, kind: BoostedPreviewKind) -> Option<&String> {
        match kind {
            BoostedPreviewKind::Creature => self.boosted_creature_preview_error.as_ref(),
            BoostedPreviewKind::Boss => self.boosted_boss_preview_error.as_ref(),
        }
    }

    fn tray_manager_mut(&mut self) -> Option<&mut TrayManager> {
        self.tray_manager.as_mut()
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

    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        let actions = self
            .tray_manager
            .as_ref()
            .map(|tray_manager| tray_manager.process_events())
            .unwrap_or_default();

        for action in actions {
            match action {
                TrayAction::ShowLauncher => self.restore_launcher_from_tray(ctx),
                TrayAction::RestoreClients => self.restore_clients_from_tray(ctx),
                TrayAction::RestoreClient(pid) => self.restore_client_from_tray(ctx, pid),
                TrayAction::MinimizeClients => self.open_minimize_client_selector(ctx),
                TrayAction::QuitLauncher => std::process::exit(0),
            }
        }
    }

    fn refresh_hidden_client_restore_menu(&mut self) -> usize {
        let hidden_clients = self.game_client.hidden_client_window_infos();
        let hidden_count = hidden_clients.len();

        if let Some(tray_manager) = self.tray_manager_mut() {
            tray_manager.update_hidden_client_entries(&hidden_clients);
        }

        self.last_clients_tray_sync = Some(Instant::now());
        hidden_count
    }

    fn set_clients_tray_icon_visible(&mut self, visible: bool) {
        if let Some(tray_manager) = self.tray_manager_mut() {
            if visible {
                tray_manager.show_clients_icon();
            } else {
                tray_manager.hide_clients_icon();
            }
        }
    }

    fn open_minimize_client_selector(&mut self, ctx: &egui::Context) {
        self.minimize_client_candidates = self.game_client.client_window_infos();

        if self.minimize_client_candidates.is_empty() {
            self.show_minimize_client_modal = false;
            self.status = "Nenhuma janela de cliente encontrada".to_string();
            self.temp_message_time = Some(Instant::now());
            self.is_alert_message = false;
        } else {
            self.show_minimize_client_modal = true;
        }

        ctx.request_repaint();
    }

    fn refresh_client_selector_candidates(&mut self) {
        if self.show_minimize_client_modal {
            self.minimize_client_candidates = self.game_client.client_window_infos();
            if self.minimize_client_candidates.is_empty() {
                self.show_minimize_client_modal = false;
            }
        }
    }

    fn minimize_client_to_tray(&mut self, ctx: &egui::Context, pid: u32) {
        let minimized_client = self.game_client.minimize_client_to_tray(pid);
        let hidden_client_count = self.refresh_hidden_client_restore_menu();
        self.clients_hidden_to_tray = hidden_client_count > 0;
        self.set_clients_tray_icon_visible(hidden_client_count > 0);

        self.status = if let Some(client) = minimized_client {
            format!(
                "Cliente {} enviado para a system tray",
                client.character_name
            )
        } else {
            "Cliente nao encontrado".to_string()
        };
        self.temp_message_time = Some(Instant::now());
        self.is_alert_message = false;
        self.refresh_client_selector_candidates();
        ctx.request_repaint();
    }

    fn minimize_clients_to_tray(&mut self, ctx: &egui::Context) {
        let hidden_count = self.game_client.minimize_clients_to_tray();
        let hidden_client_count = self.refresh_hidden_client_restore_menu();

        if hidden_count > 0 || hidden_client_count > 0 {
            self.clients_hidden_to_tray = true;
            self.set_clients_tray_icon_visible(true);
            self.status = if hidden_count == 1 {
                "Cliente enviado para a system tray".to_string()
            } else if hidden_count > 1 {
                format!("{} clientes enviados para a system tray", hidden_count)
            } else {
                "Clientes ja estao na system tray".to_string()
            };
        } else if self.game_client.has_tracked_clients() {
            self.clients_hidden_to_tray = false;
            self.set_clients_tray_icon_visible(false);
            self.status = "Nenhuma janela de cliente encontrada".to_string();
        } else {
            self.clients_hidden_to_tray = false;
            self.set_clients_tray_icon_visible(false);
            self.status = "Nenhum cliente aberto".to_string();
        }

        self.temp_message_time = Some(Instant::now());
        self.is_alert_message = false;
        self.refresh_client_selector_candidates();
        ctx.request_repaint();
    }

    fn restore_clients_from_tray(&mut self, ctx: &egui::Context) {
        let restored_count = self.game_client.restore_clients_from_tray();
        let hidden_client_count = self.refresh_hidden_client_restore_menu();

        if restored_count > 0 {
            self.clients_hidden_to_tray = hidden_client_count > 0;
            self.set_clients_tray_icon_visible(hidden_client_count > 0);
            self.status = if restored_count == 1 {
                "Cliente restaurado".to_string()
            } else {
                format!("{} clientes restaurados", restored_count)
            };
        } else if self.game_client.has_tracked_clients() {
            self.clients_hidden_to_tray = hidden_client_count > 0;
            self.set_clients_tray_icon_visible(hidden_client_count > 0);
            self.status = "Nenhuma janela de cliente encontrada".to_string();
        } else {
            self.clients_hidden_to_tray = false;
            self.set_clients_tray_icon_visible(false);
            self.status = "Nenhum cliente aberto".to_string();
        }

        self.temp_message_time = Some(Instant::now());
        self.is_alert_message = false;
        self.refresh_client_selector_candidates();
        ctx.request_repaint();
    }

    fn restore_client_from_tray(&mut self, ctx: &egui::Context, pid: u32) {
        let restored_client = self.game_client.restore_client_from_tray(pid);
        let hidden_client_count = self.refresh_hidden_client_restore_menu();
        self.clients_hidden_to_tray = hidden_client_count > 0;
        self.set_clients_tray_icon_visible(hidden_client_count > 0);

        self.status = if let Some(client) = restored_client {
            format!("Cliente {} restaurado", client.character_name)
        } else if self.game_client.has_tracked_clients() {
            "Cliente nao encontrado na system tray".to_string()
        } else {
            "Nenhum cliente aberto".to_string()
        };

        self.temp_message_time = Some(Instant::now());
        self.is_alert_message = false;
        self.refresh_client_selector_candidates();
        ctx.request_repaint();
    }

    fn initialize_clients_tray_state(&mut self) {
        if self.clients_tray_state_initialized {
            return;
        }

        self.clients_tray_state_initialized = true;
        let hidden_client_count = self.refresh_hidden_client_restore_menu();
        self.clients_hidden_to_tray = hidden_client_count > 0;
        self.last_clients_tray_sync = Some(Instant::now());
        self.set_clients_tray_icon_visible(hidden_client_count > 0);
    }

    fn sync_clients_tray_state(&mut self) {
        if !self.clients_hidden_to_tray {
            return;
        }

        let now = Instant::now();
        if let Some(last_sync) = self.last_clients_tray_sync {
            if now.duration_since(last_sync) < CLIENTS_TRAY_SYNC_INTERVAL {
                return;
            }
        }

        let hidden_client_count = self.refresh_hidden_client_restore_menu();
        self.clients_hidden_to_tray = hidden_client_count > 0;
        self.last_clients_tray_sync = Some(now);
        self.set_clients_tray_icon_visible(hidden_client_count > 0);
    }

    /// Verifica o ping do servidor usando TCP customizado
    fn check_server_ping(&mut self) {
        let now = Instant::now();

        // Se o message_sender não estiver disponível, não fazer ping ainda
        if self.message_sender.is_none() {
            return;
        }

        if self.ping_in_progress {
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
        self.ping_in_progress = true;

        // Executar ping TCP customizado de forma não-bloqueante
        if let Some(message_sender) = &self.message_sender {
            let sender = message_sender.clone();

            tokio::spawn(async move {
                let mut ping_times = Vec::new();

                // Fazer 3 pings curtos para calcular a media sem deixar a UI presa.
                for _ in 0..3 {
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
        use tokio::time::timeout;

        let start = Instant::now();

        // Conectar ao servidor
        let mut stream = timeout(
            PING_REQUEST_TIMEOUT,
            TcpStream::connect(get_ping_server_address()),
        )
        .await??;

        // Criar pacote de ping
        let packet = Self::create_packet("info");

        // Enviar pacote
        timeout(PING_REQUEST_TIMEOUT, stream.write_all(&packet)).await??;

        // Ler resposta
        let mut buffer = vec![0; NETWORK_BUFFER_SIZE];
        let bytes_read = timeout(PING_REQUEST_TIMEOUT, stream.read(&mut buffer)).await??;

        let duration = start.elapsed().as_millis() as u32;

        // Apenas confirmar que houve resposta (não fazer parse do XML)
        if bytes_read > 0 {
            info!("Resposta recebida: {} bytes em {}ms", bytes_read, duration);
        } else {
            return Err("empty ping response".into());
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

    fn ensure_message_sender(&mut self) -> mpsc::UnboundedSender<LauncherMessage> {
        if self.message_sender.is_none() {
            self.setup_update_channel();
        }

        self.message_sender
            .clone()
            .expect("message channel should be initialized")
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

    fn prepare_otclient(&mut self, ctx: &egui::Context) -> Result<()> {
        let sender = self
            .message_sender
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Canal de mensagens do launcher indisponivel"))?;

        self.status = "Preparando OTClient Redemption...".to_string();
        self.is_processing = true;
        self.progress = 0.0;
        self.temp_message_time = None;
        self.is_alert_message = false;
        ctx.request_repaint();

        let install_path = self.otclient_path.clone();
        tokio::spawn(async move {
            match otclient::ensure_otcrp_launcher(install_path, sender.clone()).await {
                Ok(launcher_path) => {
                    let _ = sender.send(LauncherMessage::LaunchOtClient(launcher_path));
                }
                Err(error) => {
                    let _ = sender.send(LauncherMessage::Error(format!(
                        "Erro ao preparar OTClient: {:#}",
                        error
                    )));
                }
            }
        });

        Ok(())
    }

    fn launch_otclient_executable(
        &mut self,
        ctx: &egui::Context,
        launcher_path: PathBuf,
    ) -> Result<()> {
        self.game_client.launch_otclient_launcher(&launcher_path)?;
        self.status = "OTClient em execucao".to_string();
        self.is_processing = false;
        self.temp_message_time = Some(Instant::now());
        self.is_alert_message = false;
        self.needs_repaint.store(true, Ordering::SeqCst);

        if self.auto_hide {
            self.hide_launcher_to_tray(ctx);
        }

        Ok(())
    }

    pub fn start_launcher_update(&mut self, ctx: &egui::Context) {
        let tx = self.ensure_message_sender();
        self.status = "Atualizando launcher...".to_string();
        self.is_processing = true;
        self.progress = 0.0;
        self.temp_message_time = None;
        self.is_alert_message = false;
        ctx.request_repaint();

        let update_manager = launcher_update::LauncherUpdateManager::new(
            self.download_path.clone(),
            self.state_path.clone(),
        );

        tokio::spawn(async move {
            if let Err(error) = update_manager.update_launcher(tx.clone()).await {
                info!("Erro durante update do launcher: {}", error);
                let _ = tx.send(LauncherMessage::SetStatus(format!(
                    "Erro ao atualizar launcher: {:#}",
                    error
                )));
                let _ = tx.send(LauncherMessage::SetProcessing(false));
            }
        });
    }

    fn restart_launcher_for_update(&mut self, ctx: &egui::Context) {
        self.restart_for_launcher_update = true;
        self.status = "Reiniciando launcher para aplicar update...".to_string();
        self.is_processing = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }

    fn minimize_to_tray(&mut self, ctx: &egui::Context) {
        self.hide_launcher_to_tray(ctx);
        self.status = "Launcher enviado para a system tray".to_string();
        self.temp_message_time = Some(Instant::now());
        self.is_alert_message = false;
        ctx.request_repaint();
    }

    fn render_minimize_client_modal(&mut self, ctx: &egui::Context) {
        if !self.show_minimize_client_modal {
            return;
        }

        let candidates = self.minimize_client_candidates.clone();
        let mut minimize_pid = None;
        let mut restore_pid = None;
        let mut minimize_all = false;
        let mut restore_all = false;
        let mut refresh = false;
        let mut cancel = false;

        egui::Window::new("Minimizar/Restaurar clientes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([520.0, 340.0])
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 250)),
            )
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Clientes")
                            .size(14.0)
                            .color(egui::Color32::from_rgb(200, 200, 200)),
                    );
                    ui.add_space(6.0);

                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            if candidates.is_empty() {
                                ui.label(
                                    egui::RichText::new("Nenhuma janela de cliente encontrada")
                                        .size(13.0)
                                        .color(egui::Color32::from_rgb(160, 160, 160)),
                                );
                            }

                            for client in &candidates {
                                let mut label =
                                    format!("{} (PID {})", client.character_name, client.pid);
                                if client.title != client.character_name {
                                    label.push_str(&format!(" - {}", client.title));
                                }

                                egui::Frame::new()
                                    .fill(egui::Color32::from_rgba_unmultiplied(35, 38, 46, 220))
                                    .corner_radius(4.0)
                                    .inner_margin(egui::Margin::symmetric(8, 6))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.vertical(|ui| {
                                                ui.label(
                                                    egui::RichText::new(label)
                                                        .size(13.0)
                                                        .color(egui::Color32::from_rgb(
                                                            220, 220, 220,
                                                        )),
                                                );
                                                ui.label(
                                                    egui::RichText::new(if client.visible {
                                                        "Visivel"
                                                    } else {
                                                        "Na system tray"
                                                    })
                                                    .size(11.0)
                                                    .color(egui::Color32::from_rgb(
                                                        155, 165, 180,
                                                    )),
                                                );
                                            });

                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    let action_label = if client.visible {
                                                        "Minimizar"
                                                    } else {
                                                        "Restaurar"
                                                    };
                                                    if ui
                                                        .add_sized(
                                                            [94.0, 28.0],
                                                            egui::Button::new(
                                                                egui::RichText::new(action_label)
                                                                    .size(13.0)
                                                                    .color(
                                                                        egui::Color32::from_rgb(
                                                                            220, 220, 220,
                                                                        ),
                                                                    ),
                                                            )
                                                            .fill(
                                                                egui::Color32::from_rgba_unmultiplied(
                                                                    45, 45, 45, 255,
                                                                ),
                                                            )
                                                            .corner_radius(2.0)
                                                            .stroke(egui::Stroke::NONE),
                                                        )
                                                        .clicked()
                                                    {
                                                        if client.visible {
                                                            minimize_pid = Some(client.pid);
                                                        } else {
                                                            restore_pid = Some(client.pid);
                                                        }
                                                    }
                                                },
                                            );
                                        });
                                    });
                                ui.add_space(6.0);
                            }
                        });

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized(
                                [80.0, 28.0],
                                egui::Button::new(
                                    egui::RichText::new("Atualizar")
                                        .size(13.0)
                                        .color(egui::Color32::from_rgb(220, 220, 220)),
                                )
                                .fill(egui::Color32::from_rgba_unmultiplied(45, 45, 45, 255))
                                .corner_radius(2.0)
                                .stroke(egui::Stroke::NONE),
                            )
                            .clicked()
                        {
                            refresh = true;
                        }

                        if ui
                            .add_sized(
                                [120.0, 28.0],
                                egui::Button::new(
                                    egui::RichText::new("Min. visiveis")
                                        .size(13.0)
                                        .color(egui::Color32::from_rgb(220, 220, 220)),
                                )
                                .fill(egui::Color32::from_rgba_unmultiplied(45, 45, 45, 255))
                                .corner_radius(2.0)
                                .stroke(egui::Stroke::NONE),
                            )
                            .clicked()
                        {
                            minimize_all = true;
                        }

                        if ui
                            .add_sized(
                                [120.0, 28.0],
                                egui::Button::new(
                                    egui::RichText::new("Rest. tray")
                                        .size(13.0)
                                        .color(egui::Color32::from_rgb(220, 220, 220)),
                                )
                                .fill(egui::Color32::from_rgba_unmultiplied(45, 45, 45, 255))
                                .corner_radius(2.0)
                                .stroke(egui::Stroke::NONE),
                            )
                            .clicked()
                        {
                            restore_all = true;
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_sized(
                                    [80.0, 28.0],
                                    egui::Button::new(
                                        egui::RichText::new("Cancelar")
                                            .size(13.0)
                                            .color(egui::Color32::from_rgb(220, 220, 220)),
                                    )
                                    .fill(egui::Color32::from_rgba_unmultiplied(45, 45, 45, 255))
                                    .corner_radius(2.0)
                                    .stroke(egui::Stroke::NONE),
                                )
                                .clicked()
                            {
                                cancel = true;
                            }
                        });
                    });
                });
            });

        if let Some(pid) = minimize_pid {
            self.minimize_client_to_tray(ctx, pid);
        } else if let Some(pid) = restore_pid {
            self.restore_client_from_tray(ctx, pid);
        } else if minimize_all {
            self.minimize_clients_to_tray(ctx);
        } else if restore_all {
            self.restore_clients_from_tray(ctx);
        } else if refresh {
            self.minimize_client_candidates = self.game_client.client_window_infos();
            ctx.request_repaint();
        } else if cancel {
            self.show_minimize_client_modal = false;
            ctx.request_repaint();
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
        self.clients_hidden_to_tray = false;
        if let Some(tray_manager) = self.tray_manager_mut() {
            tray_manager.hide_clients_icon();
        }
    }

    fn configure_style_once(&mut self, ctx: &egui::Context) {
        if self.style_configured {
            return;
        }

        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.visuals.window_shadow = egui::Shadow {
            offset: [0, 20],
            blur: style.visuals.window_shadow.blur,
            spread: style.visuals.window_shadow.spread,
            color: style.visuals.window_shadow.color,
        };
        ctx.set_style(style);
        self.style_configured = true;
    }

    fn custom_update(&mut self, ctx: &egui::Context) {
        // === Fast path: quando a janela está escondida no tray, fazer apenas trabalho essencial ===
        self.handle_tray_events(ctx);

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
            let mut should_restart_for_launcher_update = false;
            let mut pending_otclient_launch = None;
            let mut messages = Vec::new();
            if let Some(receiver) = &mut self.message_receiver {
                while let Ok(message) = receiver.try_recv() {
                    messages.push(message);
                }
            }

            for message in messages {
                match message {
                    LauncherMessage::LaunchOtClient(path) => {
                        pending_otclient_launch = Some(path);
                    }
                    LauncherMessage::PingResult(ping) => {
                        self.server_ping = ping;
                        self.last_ping_check = Some(Instant::now());
                        self.ping_in_progress = false;
                    }
                    LauncherMessage::WebsiteStatusLoaded(status) => {
                        if let Err(error) =
                            website_status::save_cached_status(&self.state_path, &status)
                        {
                            info!("Falha ao salvar cache do website: {}", error);
                        }
                        self.website_status = status;
                        self.website_status_loading = false;
                        self.cached_website_previews_queued = false;
                    }
                    LauncherMessage::WebsiteStatusError(error) => {
                        self.website_status_loading = false;
                        self.website_status.error = Some(error);
                    }
                    LauncherMessage::BoostedPreviewLoaded(kind, preview) => {
                        self.apply_boosted_preview(ctx, kind, preview);
                    }
                    LauncherMessage::BoostedPreviewError(kind, url, error) => {
                        let _ = url;
                        match kind {
                            BoostedPreviewKind::Creature => {
                                self.boosted_creature_preview_loading_url = None;
                                self.boosted_creature_preview_error = Some(error);
                            }
                            BoostedPreviewKind::Boss => {
                                self.boosted_boss_preview_loading_url = None;
                                self.boosted_boss_preview_error = Some(error);
                            }
                        }
                    }
                    LauncherMessage::OfferPreviewLoaded(preview) => {
                        self.apply_offer_preview(ctx, preview);
                    }
                    LauncherMessage::OfferPreviewError(url, error) => {
                        if self.offer_preview_loading_urls.remove(&url) {
                            self.offer_preview_errors.insert(url, error);
                        }
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
                    LauncherMessage::RestartLauncherForUpdate => {
                        should_restart_for_launcher_update = true;
                    }
                    _ => {} // Outras mensagens processadas quando visível
                }
            }
            if let Some(path) = pending_otclient_launch {
                if let Err(error) = self.launch_otclient_executable(ctx, path) {
                    self.status = format!("Erro ao iniciar OTClient: {}", error);
                    self.is_processing = false;
                }
                ctx.request_repaint();
                return;
            }
            if should_restart_for_launcher_update {
                self.restart_launcher_for_update(ctx);
                return;
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
            self.sync_clients_tray_state();

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

        if !self.startup_splash_finished {
            self.render_central_panel(ctx, ctx.available_rect().size());
            return;
        }

        self.initialize_clients_tray_state();
        self.sync_clients_tray_state();

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

            if false {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(6)).await;
                    info!("Verificando atualizacoes do cliente em segundo plano...");
                    if false {
                        // Atualizar o status para "Verificando atualizações"
                        if let Some(sender) = message_sender.clone() {
                            let _ = sender.send(LauncherMessage::SetStatus(
                                "Verificando atualizações...".to_string(),
                            ));
                            let _ = sender.send(LauncherMessage::SetProcessing(true));
                        }

                        info!("Verificando atualizações iniciais...");
                        if let Some(sender) = message_sender.clone() {
                            let launcher_update_manager =
                                launcher_update::LauncherUpdateManager::new(
                                    download_path.clone(),
                                    state_path.clone(),
                                );

                            match launcher_update_manager
                                .update_launcher_if_available(sender.clone())
                                .await
                            {
                                Ok(true) => {
                                    info!(
                                        "Update do launcher encontrado; reiniciando para aplicar"
                                    );
                                    return;
                                }
                                Ok(false) => {
                                    info!("Launcher ja esta atualizado");
                                }
                                Err(error) => {
                                    info!(
                                        "Falha ao verificar update automatico do launcher: {:#}",
                                        error
                                    );
                                }
                            }
                        }

                        if let Some(sender) = message_sender.clone() {
                            let _ = sender.send(LauncherMessage::SetStatus(
                                "Verificando atualizacoes...".to_string(),
                            ));
                            let _ = sender.send(LauncherMessage::DownloadProgress(0.0));
                            let _ = sender.send(LauncherMessage::SetProcessing(true));
                        }
                    }

                    match updates::UpdateManager::check_initial_updates(&game_path, &state_path)
                        .await
                    {
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
                                            .check_for_updates(
                                                message_tx.clone(),
                                                disable_auto_start,
                                            )
                                            .await
                                        {
                                            let _ =
                                                message_tx.send(LauncherMessage::Error(format!(
                                                    "Erro ao iniciar download automático: {:#}",
                                                    e
                                                )));
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
                                    tokio::time::sleep(Duration::from_millis(800)).await;

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
            self.status = "Pronto para jogar".to_string();
            self.is_processing = false;
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

                // Limpar a mensagem temporária
                self.temp_message_time = None;
                self.is_alert_message = false;

                let (has_main, additional_count) = self.game_client.sync_client_state();
                // Atualizar para o status normal de acordo com o estado dos clientes
                self.status = if has_main || additional_count > 0 {
                    if has_main {
                        if additional_count == 0 {
                            "Cliente em execução".to_string()
                        } else {
                            "Clientes em execução".to_string()
                        }
                    } else {
                        "Clientes em execução".to_string()
                    }
                } else {
                    "Pronto para jogar".to_string()
                };
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

        // Atualizar status com base nos clientes ativos e o cliente principal
        let (is_game_running, additional_count) = self.game_client.sync_client_state();
        self.sync_clients_tray_state();

        // Não atualizar o status se houver uma mensagem temporária
        if !self.temp_message_time.is_some() {
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
        if !is_game_running && self.status.contains("em execu") {
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
        self.refresh_website_status();
        if !self.cached_website_previews_queued && self.website_status.fetched_at.is_some() {
            self.cached_website_previews_queued = true;
            self.refresh_boosted_previews();
            self.refresh_offer_previews();
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
                        LauncherMessage::LaunchOtClient(path) => {
                            if let Err(e) = self.launch_otclient_executable(ctx, path) {
                                self.status = format!("Erro ao iniciar OTClient: {}", e);
                                self.is_processing = false;
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
                            self.is_alert_message = false;
                            info!("Mensagem temporária definida via channel: {}", message);
                        }
                        LauncherMessage::PingResult(ping) => {
                            self.server_ping = ping;
                            self.last_ping_check = Some(Instant::now());
                            self.ping_in_progress = false;
                        }
                        LauncherMessage::WebsiteStatusLoaded(status) => {
                            if let Err(error) =
                                website_status::save_cached_status(&self.state_path, &status)
                            {
                                info!("Falha ao salvar cache do website: {}", error);
                            }
                            self.website_status = status;
                            self.website_status_loading = false;
                            self.cached_website_previews_queued = true;
                            self.refresh_boosted_previews();
                            self.refresh_offer_previews();
                        }
                        LauncherMessage::WebsiteStatusError(error) => {
                            self.apply_website_status_error(error);
                        }
                        LauncherMessage::BoostedPreviewLoaded(kind, preview) => {
                            self.apply_boosted_preview(ctx, kind, preview);
                        }
                        LauncherMessage::BoostedPreviewError(kind, url, error) => {
                            self.apply_boosted_preview_error(kind, url, error);
                        }
                        LauncherMessage::OfferPreviewLoaded(preview) => {
                            self.apply_offer_preview(ctx, preview);
                            self.refresh_offer_previews();
                        }
                        LauncherMessage::OfferPreviewError(url, error) => {
                            self.apply_offer_preview_error(url, error);
                            self.refresh_offer_previews();
                        }
                        LauncherMessage::RestartLauncherForUpdate => {
                            self.restart_launcher_for_update(ctx);
                        }
                    }
                }
                ctx.request_repaint();
            }
        }

        self.configure_style_once(ctx);
        /*
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
        */

        // Pegar o tamanho da janela para responsividade
        let available_size = ctx.available_rect().size();

        // Renderizar o painel central usando a função dedicada
        self.render_central_panel(ctx, available_size);
        self.render_minimize_client_modal(ctx);

        // Renderizar o modal de confirmação para Forçar Atualização
        if self.show_force_update_modal {
            egui::Window::new("Force Update")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .fixed_size([320.0, 140.0])
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 250)),
                )
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new("Baixar novamente os arquivos do cliente?")
                                .size(13.0)
                                .color(egui::Color32::from_rgb(160, 160, 160)),
                        );

                        ui.label(
                            egui::RichText::new(
                                "O clientoptions.json existente sera mantido intacto.",
                            )
                            .size(13.0)
                            .color(egui::Color32::from_rgb(140, 140, 140)),
                        );

                        ui.add_space(15.0);

                        ui.horizontal(|ui| {
                            // Botão Cancelar à esquerda
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized(
                                            [90.0, 28.0],
                                            egui::Button::new(
                                                egui::RichText::new("Cancelar")
                                                    .size(13.0)
                                                    .color(egui::Color32::from_rgb(200, 200, 200)),
                                            )
                                            .fill(egui::Color32::from_rgba_unmultiplied(
                                                45, 45, 45, 255,
                                            ))
                                            .corner_radius(2.0)
                                            .stroke(egui::Stroke::NONE),
                                        )
                                        .clicked()
                                    {
                                        self.show_force_update_modal = false;
                                    }
                                },
                            );

                            // Espaço flexível entre os botões
                            ui.with_layout(
                                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                                |ui| {
                                    ui.allocate_space(ui.available_size());
                                },
                            );

                            // Botão Confirmar à direita
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized(
                                            [90.0, 28.0],
                                            egui::Button::new(
                                                egui::RichText::new("Confirmar")
                                                    .size(13.0)
                                                    .color(egui::Color32::BLACK),
                                            )
                                            .fill(egui::Color32::from_rgb(76, 175, 80))
                                            .corner_radius(2.0)
                                            .stroke(egui::Stroke::NONE),
                                        )
                                        .clicked()
                                    {
                                        self.show_force_update_modal = false;

                                        // Iniciar a atualização forçada
                                        let tx = self.ensure_message_sender();
                                        self.status = "Iniciando Force Update...".to_string();
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
                                            match update_manager
                                                .force_refresh(tx.clone(), disable_auto_start)
                                                .await
                                            {
                                                Ok(_) => {
                                                    info!(
                                                        "Atualização forçada concluída com sucesso"
                                                    );
                                                }
                                                Err(e) => {
                                                    info!(
                                                        "Erro durante atualização forçada: {}",
                                                        e
                                                    );
                                                    let _ = tx.send(LauncherMessage::SetStatus(
                                                        format!(
                                                            "Erro na atualização forçada: {}",
                                                            e
                                                        ),
                                                    ));
                                                    let _ = tx.send(
                                                        LauncherMessage::SetProcessing(false),
                                                    );
                                                }
                                            }
                                        });
                                    }
                                },
                            );
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

    fn load_texture_from_memory(
        ctx: &egui::Context,
        name: &str,
        bytes: &[u8],
        max_dimension: Option<u32>,
        options: egui::TextureOptions,
    ) -> Option<egui::TextureHandle> {
        let image_data = image::load_from_memory(bytes).ok()?;
        let mut image = image_data.into_rgba8();
        if let Some(max_dimension) = max_dimension {
            let (width, height) = image.dimensions();
            let longest = width.max(height);
            if longest > max_dimension {
                let scale = max_dimension as f32 / longest as f32;
                let resized_width = ((width as f32 * scale).round() as u32).max(1);
                let resized_height = ((height as f32 * scale).round() as u32).max(1);
                image = image::imageops::resize(
                    &image,
                    resized_width,
                    resized_height,
                    image::imageops::FilterType::Lanczos3,
                );
            }
        }

        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        let texture =
            egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
        info!("Textura {} carregada em {}x{}", name, width, height);
        Some(ctx.load_texture(name, texture, options))
    }

    fn load_background(&mut self, ctx: &egui::Context) {
        self.background_texture = Self::load_texture_from_memory(
            ctx,
            "background",
            include_bytes!("../assets/website-background.jpg"),
            Some(1024),
            egui::TextureOptions::LINEAR,
        );
        self.logo_texture = Self::load_texture_from_memory(
            ctx,
            "logo",
            include_bytes!("../assets/penultima-phoenix.png"),
            Some(360),
            egui::TextureOptions::LINEAR,
        );
        self.splash_logo_texture = Self::load_texture_from_memory(
            ctx,
            "splash-logo",
            include_bytes!("../assets/ultima-website-logo.png"),
            Some(512),
            egui::TextureOptions::LINEAR,
        );
    }

    /*

        // Carregar o papel de parede
        if let Ok(image_data) =
            image::load_from_memory(include_bytes!("../assets/website-background.jpg"))
        {
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
        if let Ok(logo_data) =
            image::load_from_memory(include_bytes!("../assets/penultima-phoenix.png"))
        {
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

        if let Ok(logo_data) =
            image::load_from_memory(include_bytes!("../assets/ultima-website-logo.png"))
        {
            let logo = logo_data.into_rgba8();
            let (width, height) = logo.dimensions();
            let rgba = logo.into_raw();
            let texture =
                egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);

            self.splash_logo_texture =
                Some(ctx.load_texture("splash-logo", texture, egui::TextureOptions::LINEAR));

            info!("Logo de abertura carregado em {}x{}", width, height);
        } else {
            info!("NÃ£o foi possÃ­vel carregar o logo de abertura");
        }
    }

        */
    fn render_central_panel(&mut self, ctx: &egui::Context, available_size: egui::Vec2) {
        if !self.startup_splash_finished {
            ui_components::render_startup_splash(self, ctx, available_size);
            return;
        }

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
            launcher.game_client = GameClient::new();
            launcher
                .game_client
                .set_window_state_path(launcher.state_path.join("client-window-state.json"));
            launcher.window_state = window_state;
            launcher.initialized = false;
            launcher.auto_hide = args.auto_hide;
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
            if self.restart_for_launcher_update {
                info!("Fechando launcher para aplicar update automatico");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Chamada para o método de atualização personalizado
        self.custom_update(ctx);
    }
}

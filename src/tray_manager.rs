use crate::game_client::WindowState;
use crate::window_manager::WindowManager;
use anyhow::{Context, Result};
use eframe::egui::IconData;
use image;
use std::sync::{Arc, Mutex};
use std::thread;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItemBuilder},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

pub struct TrayManager {
    pub tray_icon: Option<TrayIcon>,
    restore_id: MenuId,
    quit_id: MenuId,
}

impl TrayManager {
    pub fn new() -> Self {
        Self {
            tray_icon: None,
            restore_id: MenuId::new("restore"),
            quit_id: MenuId::new("quit"),
        }
    }

    // Configurar ícone da bandeja e eventos
    pub fn setup(&mut self, window_state: Arc<Mutex<WindowState>>) -> Result<()> {
        println!("Criando ícone na bandeja...");

        // Carregar o ícone
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
        let restore_id = self.restore_id.clone();
        let quit_id = self.quit_id.clone();

        // Criar os itens do menu
        let restore_item = MenuItemBuilder::new()
            .text("Abrir")
            .id(restore_id)
            .enabled(true)
            .build();

        let quit_item = MenuItemBuilder::new()
            .text("Sair")
            .id(quit_id)
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

        self.tray_icon = Some(tray_icon);

        println!("Ícone criado com sucesso!");

        // Configurar eventos do menu
        self.setup_menu_events(window_state.clone());

        // Configurar eventos do ícone
        self.setup_icon_events(window_state);

        Ok(())
    }

    // Configurar eventos do menu
    fn setup_menu_events(&self, window_state: Arc<Mutex<WindowState>>) {
        let menu_channel = MenuEvent::receiver();
        let restore_id = self.restore_id.clone();
        let quit_id = self.quit_id.clone();
        let window_state_menu = window_state.clone();

        thread::spawn(move || {
            // Criar uma instância de WindowManager para usar seu método show_window
            let window_manager = WindowManager {
                window_state: window_state_menu.clone(),
                needs_repaint: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            };
            
            while let Ok(event) = menu_channel.recv() {
                if event.id == restore_id {
                    println!("Menu Abrir clicado!");
                    // Usar o método do WindowManager em vez da função independente
                    window_manager.show_window();
                } else if event.id == quit_id {
                    println!("Menu Sair clicado!");
                    std::process::exit(0);
                }
            }
        });
    }

    // Configurar eventos do ícone
    fn setup_icon_events(&self, window_state: Arc<Mutex<WindowState>>) {
        let window_state_icon = window_state.clone();
        let event_channel = TrayIconEvent::receiver();

        thread::spawn(move || {
            // Criar uma instância de WindowManager para usar seu método show_window
            let window_manager = WindowManager {
                window_state: window_state_icon.clone(),
                needs_repaint: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            };

            while let Ok(event) = event_channel.recv() {
                if let TrayIconEvent::Click {
                    button,
                    button_state,
                    ..
                } = event
                {
                    if button == MouseButton::Left && button_state == MouseButtonState::Up {
                        println!("Ícone clicado! Tornando janela visível...");
                        // Usar o método do WindowManager em vez da função independente
                        window_manager.show_window();
                    }
                }
            }
        });
    }

    // Função para carregar o ícone do aplicativo para a janela
    pub fn load_window_icon() -> Option<Arc<IconData>> {
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
}

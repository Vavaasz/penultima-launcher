use crate::constants::*;
use crate::game_client::{GameClient, WindowState};
use crate::window_manager::WindowManager;
use anyhow::{Context, Result};
use eframe::egui::IconData;
use log::info;
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
    tracked_pids: Arc<Mutex<Vec<u32>>>,
}

impl TrayManager {
    pub fn new(tracked_pids: Arc<Mutex<Vec<u32>>>) -> Self {
        Self {
            tray_icon: None,
            restore_id: MenuId::new("restore"),
            quit_id: MenuId::new("quit"),
            tracked_pids,
        }
    }

    pub fn setup(&mut self, window_state: Arc<Mutex<WindowState>>) -> Result<()> {
        info!("Criando Ã­cone na bandeja...");

        let icon = include_bytes!("../assets/ultima-logo.ico");
        let (icon_rgba, icon_width, icon_height) = {
            let image = image::load_from_memory(icon)
                .context("Falha ao carregar Ã­cone")?
                .into_rgba8();
            let (width, height) = image.dimensions();
            (image.into_raw(), width, height)
        };

        let icon =
            Icon::from_rgba(icon_rgba, icon_width, icon_height).context("Falha ao criar Ã­cone")?;

        let tray_menu = Menu::new();
        let restore_id = self.restore_id.clone();
        let quit_id = self.quit_id.clone();

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

        tray_menu
            .append(&restore_item)
            .context("Falha ao adicionar item Abrir")?;
        tray_menu
            .append(&quit_item)
            .context("Falha ao adicionar item Sair")?;

        let tray_icon = TrayIconBuilder::new()
            .with_tooltip(APP_NAME)
            .with_icon(icon)
            .with_menu(Box::new(tray_menu))
            .build()
            .context("Falha ao criar Ã­cone na system tray")?;

        self.tray_icon = Some(tray_icon);

        self.setup_menu_events(window_state.clone(), self.tracked_pids.clone());
        self.setup_icon_events(window_state, self.tracked_pids.clone());

        Ok(())
    }

    fn setup_menu_events(
        &self,
        window_state: Arc<Mutex<WindowState>>,
        tracked_pids: Arc<Mutex<Vec<u32>>>,
    ) {
        let menu_channel = MenuEvent::receiver();
        let restore_id = self.restore_id.clone();
        let quit_id = self.quit_id.clone();

        thread::spawn(move || {
            let window_manager = WindowManager {
                window_state,
                needs_repaint: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            };

            while let Ok(event) = menu_channel.recv() {
                if event.id == restore_id {
                    window_manager.show_window();
                    let restored = GameClient::restore_all_from_tray(&tracked_pids);
                    info!("Tray restore executado para {} janela(s) de client", restored);
                } else if event.id == quit_id {
                    let restored = GameClient::restore_all_from_tray(&tracked_pids);
                    info!("Tray quit restaurou {} janela(s) de client", restored);
                    std::process::exit(0);
                }
            }
        });
    }

    fn setup_icon_events(
        &self,
        window_state: Arc<Mutex<WindowState>>,
        tracked_pids: Arc<Mutex<Vec<u32>>>,
    ) {
        let event_channel = TrayIconEvent::receiver();

        thread::spawn(move || {
            let window_manager = WindowManager {
                window_state,
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
                        window_manager.show_window();
                        let restored = GameClient::restore_all_from_tray(&tracked_pids);
                        info!("Tray click restaurou {} janela(s) de client", restored);
                    }
                }
            }
        });
    }

    pub fn load_window_icon() -> Option<Arc<IconData>> {
        let icon = include_bytes!("../assets/ultima-logo.ico");
        let (icon_rgba, icon_width, icon_height) = {
            let image = image::load_from_memory(icon).ok()?.into_rgba8();
            let (width, height) = image.dimensions();
            (image.into_raw(), width, height)
        };

        Some(Arc::new(IconData {
            rgba: icon_rgba,
            width: icon_width as _,
            height: icon_height as _,
        }))
    }
}

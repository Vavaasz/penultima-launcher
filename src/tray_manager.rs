use anyhow::{Context, Result};
use eframe::egui::IconData;
use image;
use std::sync::{
    Arc,
    mpsc::{self, Receiver, Sender},
};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent, TrayIconId,
    menu::{Menu, MenuEvent, MenuId, MenuItemBuilder},
};

use crate::constants::APP_NAME;
use crate::game_client::{WindowState, show_window};

pub enum TrayAction {
    ShowLauncher,
    QuitLauncher,
}

pub struct TrayManager {
    launcher_icon: Option<TrayIcon>,
    launcher_icon_id: TrayIconId,
    launcher_restore_id: MenuId,
    launcher_quit_id: MenuId,
    launcher_icon_visible: bool,
    action_sender: Sender<TrayAction>,
    action_receiver: Receiver<TrayAction>,
}

impl TrayManager {
    pub fn new() -> Self {
        let (action_sender, action_receiver) = mpsc::channel();
        Self {
            launcher_icon: None,
            launcher_icon_id: TrayIconId::new("launcher-main"),
            launcher_restore_id: MenuId::new("launcher-restore"),
            launcher_quit_id: MenuId::new("launcher-quit"),
            launcher_icon_visible: false,
            action_sender,
            action_receiver,
        }
    }

    pub fn setup(&mut self, window_state: Arc<std::sync::Mutex<WindowState>>) -> Result<()> {
        let tray_menu = Menu::new();

        let restore_item = MenuItemBuilder::new()
            .text("Abrir launcher")
            .id(self.launcher_restore_id.clone())
            .enabled(true)
            .build();

        let quit_item = MenuItemBuilder::new()
            .text("Sair")
            .id(self.launcher_quit_id.clone())
            .enabled(true)
            .build();

        tray_menu
            .append(&restore_item)
            .context("Falha ao adicionar item Abrir launcher")?;
        tray_menu
            .append(&quit_item)
            .context("Falha ao adicionar item Sair")?;

        let tray_icon = TrayIconBuilder::new()
            .with_id(self.launcher_icon_id.clone())
            .with_tooltip(APP_NAME)
            .with_icon(Self::load_tray_icon()?)
            .with_menu(Box::new(tray_menu))
            .with_menu_on_left_click(false)
            .build()
            .context("Falha ao criar icone principal da system tray")?;

        tray_icon
            .set_visible(false)
            .context("Falha ao ocultar icone principal da tray")?;

        self.launcher_icon = Some(tray_icon);
        self.launcher_icon_visible = false;
        self.install_event_handlers(window_state);
        Ok(())
    }

    pub fn show_launcher_icon(&mut self) {
        if let Some(icon) = &self.launcher_icon {
            let _ = icon.set_tooltip(Some(APP_NAME));
            let _ = icon.set_visible(true);
            self.launcher_icon_visible = true;
        }
    }

    pub fn hide_launcher_icon(&mut self) {
        if let Some(icon) = &self.launcher_icon {
            let _ = icon.set_visible(false);
            self.launcher_icon_visible = false;
        }
    }

    pub fn should_poll_aggressively(&self) -> bool {
        self.launcher_icon_visible
    }

    pub fn process_events(&self) -> Vec<TrayAction> {
        let mut actions = Vec::new();

        while let Ok(action) = self.action_receiver.try_recv() {
            actions.push(action);
        }

        actions
    }

    fn install_event_handlers(&self, window_state: Arc<std::sync::Mutex<WindowState>>) {
        let menu_sender = self.action_sender.clone();
        let menu_window_state = Arc::clone(&window_state);
        let launcher_restore_id = self.launcher_restore_id.clone();
        let launcher_quit_id = self.launcher_quit_id.clone();

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == launcher_restore_id {
                show_window(&menu_window_state);
                let _ = menu_sender.send(TrayAction::ShowLauncher);
                return;
            }

            if event.id == launcher_quit_id {
                let _ = menu_sender.send(TrayAction::QuitLauncher);
            }
        }));

        let tray_sender = self.action_sender.clone();
        let tray_window_state = Arc::clone(&window_state);
        let launcher_icon_id = self.launcher_icon_id.clone();

        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| match event {
            TrayIconEvent::Click {
                id,
                button,
                button_state,
                ..
            } if button == MouseButton::Left && button_state == MouseButtonState::Up => {
                if id == launcher_icon_id {
                    show_window(&tray_window_state);
                    let _ = tray_sender.send(TrayAction::ShowLauncher);
                }
            }
            TrayIconEvent::DoubleClick { id, button, .. } if button == MouseButton::Left => {
                if id == launcher_icon_id {
                    show_window(&tray_window_state);
                    let _ = tray_sender.send(TrayAction::ShowLauncher);
                }
            }
            _ => {}
        }));
    }

    fn load_tray_icon() -> Result<Icon> {
        let icon = include_bytes!("../assets/penultima-phoenix.ico");
        let (icon_rgba, icon_width, icon_height) = {
            let image = image::load_from_memory(icon)
                .context("Falha ao carregar icone da tray")?
                .into_rgba8();
            let (width, height) = image.dimensions();
            (image.into_raw(), width, height)
        };

        Icon::from_rgba(icon_rgba, icon_width, icon_height).context("Falha ao criar icone da tray")
    }

    pub fn load_window_icon() -> Option<Arc<IconData>> {
        let icon = include_bytes!("../assets/penultima-phoenix.ico");
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

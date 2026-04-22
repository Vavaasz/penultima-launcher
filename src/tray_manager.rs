use anyhow::{Context, Result};
use eframe::egui::IconData;
use image;
use log::warn;
use std::sync::{
    Arc,
    mpsc::{self, Receiver, Sender},
};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent, TrayIconId,
    menu::{Menu, MenuEvent, MenuId, MenuItem, MenuItemBuilder, Submenu, SubmenuBuilder},
};

use crate::constants::APP_NAME;
use crate::game_client::{ClientWindowInfo, WindowState, show_window};

pub enum TrayAction {
    ShowLauncher,
    RestoreClients,
    RestoreClient(u32),
    MinimizeClients,
    QuitLauncher,
}

const CLIENT_RESTORE_ID_PREFIX: &str = "client-restore-pid-";

pub struct TrayManager {
    launcher_icon: Option<TrayIcon>,
    client_restore_submenu: Option<Submenu>,
    client_restore_items: Vec<MenuItem>,
    launcher_icon_id: TrayIconId,
    launcher_restore_id: MenuId,
    client_restore_id: MenuId,
    client_restore_submenu_id: MenuId,
    client_minimize_id: MenuId,
    launcher_quit_id: MenuId,
    launcher_icon_visible: bool,
    clients_icon_visible: bool,
    action_sender: Sender<TrayAction>,
    action_receiver: Receiver<TrayAction>,
}

impl TrayManager {
    pub fn new() -> Self {
        let (action_sender, action_receiver) = mpsc::channel();
        Self {
            launcher_icon: None,
            client_restore_submenu: None,
            client_restore_items: Vec::new(),
            launcher_icon_id: TrayIconId::new("launcher-main"),
            launcher_restore_id: MenuId::new("launcher-restore"),
            client_restore_id: MenuId::new("client-restore"),
            client_restore_submenu_id: MenuId::new("client-restore-one"),
            client_minimize_id: MenuId::new("client-minimize"),
            launcher_quit_id: MenuId::new("launcher-quit"),
            launcher_icon_visible: false,
            clients_icon_visible: false,
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

        let restore_clients_item = MenuItemBuilder::new()
            .text("Restaurar clientes")
            .id(self.client_restore_id.clone())
            .enabled(true)
            .build();

        let restore_client_submenu = SubmenuBuilder::new()
            .text("Restaurar cliente")
            .id(self.client_restore_submenu_id.clone())
            .enabled(false)
            .build()
            .context("Falha ao criar submenu Restaurar cliente")?;

        let minimize_clients_item = MenuItemBuilder::new()
            .text("Minimizar clientes")
            .id(self.client_minimize_id.clone())
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
            .append(&restore_clients_item)
            .context("Falha ao adicionar item Restaurar clientes")?;
        tray_menu
            .append(&restore_client_submenu)
            .context("Falha ao adicionar submenu Restaurar cliente")?;
        tray_menu
            .append(&minimize_clients_item)
            .context("Falha ao adicionar item Minimizar clientes")?;
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
        self.client_restore_submenu = Some(restore_client_submenu);
        self.client_restore_items.clear();
        self.launcher_icon_visible = false;
        self.clients_icon_visible = false;
        self.install_event_handlers(window_state);
        Ok(())
    }

    pub fn show_launcher_icon(&mut self) {
        self.launcher_icon_visible = true;
        self.update_icon_state();
    }

    pub fn hide_launcher_icon(&mut self) {
        self.launcher_icon_visible = false;
        self.update_icon_state();
    }

    pub fn show_clients_icon(&mut self) {
        self.clients_icon_visible = true;
        self.update_icon_state();
    }

    pub fn hide_clients_icon(&mut self) {
        self.clients_icon_visible = false;
        self.update_icon_state();
    }

    pub fn should_poll_aggressively(&self) -> bool {
        self.launcher_icon_visible || self.clients_icon_visible
    }

    pub fn process_events(&self) -> Vec<TrayAction> {
        let mut actions = Vec::new();

        while let Ok(action) = self.action_receiver.try_recv() {
            actions.push(action);
        }

        actions
    }

    pub fn update_hidden_client_entries(&mut self, clients: &[ClientWindowInfo]) {
        let Some(restore_submenu) = self.client_restore_submenu.clone() else {
            return;
        };

        for item in self.client_restore_items.drain(..) {
            if let Err(error) = restore_submenu.remove(&item) {
                warn!("Falha ao remover item de cliente da tray: {}", error);
            }
        }

        if clients.is_empty() {
            restore_submenu.set_enabled(false);
            return;
        }

        for client in clients {
            let item = MenuItemBuilder::new()
                .text(format!(
                    "Restaurar {}",
                    escape_menu_label(&client.character_name)
                ))
                .id(client_restore_menu_id(client.pid))
                .enabled(true)
                .build();

            if let Err(error) = restore_submenu.append(&item) {
                warn!(
                    "Falha ao adicionar item de cliente {} na tray: {}",
                    client.pid, error
                );
                continue;
            }

            self.client_restore_items.push(item);
        }

        restore_submenu.set_enabled(true);
    }

    fn install_event_handlers(&self, window_state: Arc<std::sync::Mutex<WindowState>>) {
        let menu_sender = self.action_sender.clone();
        let menu_window_state = Arc::clone(&window_state);
        let launcher_restore_id = self.launcher_restore_id.clone();
        let client_restore_id = self.client_restore_id.clone();
        let client_minimize_id = self.client_minimize_id.clone();
        let launcher_quit_id = self.launcher_quit_id.clone();

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == launcher_restore_id {
                show_window(&menu_window_state);
                let _ = menu_sender.send(TrayAction::ShowLauncher);
                return;
            }

            if event.id == client_restore_id {
                let _ = menu_sender.send(TrayAction::RestoreClients);
                return;
            }

            if let Some(pid) = pid_from_client_restore_menu_id(&event.id) {
                let _ = menu_sender.send(TrayAction::RestoreClient(pid));
                return;
            }

            if event.id == client_minimize_id {
                let _ = menu_sender.send(TrayAction::MinimizeClients);
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

    fn update_icon_state(&mut self) {
        let Some(icon) = &self.launcher_icon else {
            return;
        };

        let tooltip = if self.launcher_icon_visible && self.clients_icon_visible {
            "Penultima Launcher - launcher e clientes na tray"
        } else if self.clients_icon_visible {
            "Penultima Launcher - clientes na tray"
        } else {
            APP_NAME
        };

        let _ = icon.set_tooltip(Some(tooltip));
        let _ = icon.set_visible(self.launcher_icon_visible || self.clients_icon_visible);
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

fn client_restore_menu_id(pid: u32) -> MenuId {
    MenuId::new(format!("{}{}", CLIENT_RESTORE_ID_PREFIX, pid))
}

fn pid_from_client_restore_menu_id(id: &MenuId) -> Option<u32> {
    id.as_ref()
        .strip_prefix(CLIENT_RESTORE_ID_PREFIX)
        .and_then(|pid| pid.parse().ok())
}

fn escape_menu_label(label: &str) -> String {
    label.replace('&', "&&")
}

#[cfg(test)]
mod tests {
    use super::{client_restore_menu_id, escape_menu_label, pid_from_client_restore_menu_id};
    use tray_icon::menu::MenuId;

    #[test]
    fn parses_dynamic_client_restore_menu_ids() {
        let id = client_restore_menu_id(12345);
        assert_eq!(pid_from_client_restore_menu_id(&id), Some(12345));
        assert_eq!(
            pid_from_client_restore_menu_id(&MenuId::new("client-restore")),
            None
        );
    }

    #[test]
    fn escapes_ampersands_in_menu_labels() {
        assert_eq!(escape_menu_label("A&B"), "A&&B");
    }
}

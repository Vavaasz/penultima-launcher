use anyhow::{Context, Result};
use eframe::egui::IconData;
use image;
use log::info;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, Sender},
};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent, TrayIconId,
    menu::{Menu, MenuEvent, MenuId, MenuItemBuilder},
};
use windows::Win32::Foundation::HWND;

use crate::constants::APP_NAME;
use crate::game_client::{ClientWindowInfo, GameClient, WindowState, show_window};

pub enum TrayAction {
    ShowLauncher,
    RestoreAllClients,
    RestoreClient(isize),
    QuitLauncher,
}

struct ClientTrayEntry {
    hwnd: HWND,
    icon_id: TrayIconId,
    _tray_icon: TrayIcon,
}

#[derive(Clone)]
struct HiddenClientEventEntry {
    hwnd_raw: isize,
    icon_id: TrayIconId,
}

pub struct TrayManager {
    launcher_icon: Option<TrayIcon>,
    launcher_icon_id: TrayIconId,
    launcher_restore_id: MenuId,
    launcher_restore_clients_id: MenuId,
    launcher_quit_id: MenuId,
    launcher_icon_visible: bool,
    hidden_clients: HashMap<isize, ClientTrayEntry>,
    hidden_client_lookup: Arc<Mutex<HashMap<isize, HiddenClientEventEntry>>>,
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
            launcher_restore_clients_id: MenuId::new("launcher-restore-clients"),
            launcher_quit_id: MenuId::new("launcher-quit"),
            launcher_icon_visible: false,
            hidden_clients: HashMap::new(),
            hidden_client_lookup: Arc::new(Mutex::new(HashMap::new())),
            action_sender,
            action_receiver,
        }
    }

    pub fn setup(&mut self, window_state: Arc<Mutex<WindowState>>) -> Result<()> {
        let tray_menu = Menu::new();

        let restore_item = MenuItemBuilder::new()
            .text("Abrir launcher")
            .id(self.launcher_restore_id.clone())
            .enabled(true)
            .build();

        let restore_clients_item = MenuItemBuilder::new()
            .text("Restaurar clientes")
            .id(self.launcher_restore_clients_id.clone())
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
            .append(&quit_item)
            .context("Falha ao adicionar item Sair")?;

        let tray_icon = TrayIconBuilder::new()
            .with_id(self.launcher_icon_id.clone())
            .with_tooltip(APP_NAME)
            .with_icon(Self::load_tray_icon()?)
            .with_menu(Box::new(tray_menu))
            .with_menu_on_left_click(false)
            .build()
            .context("Falha ao criar ícone principal da system tray")?;

        tray_icon
            .set_visible(false)
            .context("Falha ao ocultar ícone principal da tray")?;

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

    pub fn register_hidden_clients(&mut self, windows: &[ClientWindowInfo]) -> Result<usize> {
        let mut added = 0usize;

        for window in windows {
            let key = window.hwnd.0 as isize;
            if self.hidden_clients.contains_key(&key) {
                continue;
            }

            let icon_id = TrayIconId::new(format!("client-{}", key));
            let tray_icon = TrayIconBuilder::new()
                .with_id(icon_id.clone())
                .with_tooltip(window.display_name.clone())
                .with_icon(Self::load_tray_icon()?)
                .with_menu_on_left_click(false)
                .build()
                .with_context(|| {
                    format!(
                        "Falha ao criar ícone de tray para a janela {}",
                        window.display_name
                    )
                })?;

            info!(
                "Cliente enviado para a tray: pid={} title='{}'",
                window.pid, window.window_title
            );

            self.hidden_clients.insert(
                key,
                ClientTrayEntry {
                    hwnd: window.hwnd,
                    icon_id,
                    _tray_icon: tray_icon,
                },
            );
            added += 1;
        }

        self.sync_hidden_client_lookup();
        Ok(added)
    }

    pub fn remove_hidden_client(&mut self, hwnd: HWND) -> bool {
        let removed = self.hidden_clients.remove(&(hwnd.0 as isize)).is_some();
        if removed {
            self.sync_hidden_client_lookup();
        }
        removed
    }

    pub fn restore_all_hidden_clients(&mut self) -> Vec<HWND> {
        let hwnds: Vec<HWND> = self
            .hidden_clients
            .values()
            .map(|entry| entry.hwnd)
            .collect();
        self.hidden_clients.clear();
        self.sync_hidden_client_lookup();
        hwnds
    }

    pub fn cleanup_hidden_clients(&mut self) {
        self.hidden_clients.retain(|_, entry| {
            let keep = GameClient::is_window_hidden(entry.hwnd);
            if !keep {
                info!(
                    "Removendo ícone stale da tray para janela {:?}",
                    entry.hwnd.0
                );
            }
            keep
        });
        self.sync_hidden_client_lookup();
    }

    pub fn has_hidden_clients(&self) -> bool {
        !self.hidden_clients.is_empty()
    }

    pub fn should_poll_aggressively(&self) -> bool {
        self.launcher_icon_visible || self.has_hidden_clients()
    }

    pub fn process_events(&self) -> Vec<TrayAction> {
        let mut actions = Vec::new();

        while let Ok(action) = self.action_receiver.try_recv() {
            actions.push(action);
        }

        actions
    }

    fn install_event_handlers(&self, window_state: Arc<Mutex<WindowState>>) {
        let menu_sender = self.action_sender.clone();
        let menu_window_state = Arc::clone(&window_state);
        let menu_hidden_clients = Arc::clone(&self.hidden_client_lookup);
        let launcher_restore_id = self.launcher_restore_id.clone();
        let launcher_restore_clients_id = self.launcher_restore_clients_id.clone();
        let launcher_quit_id = self.launcher_quit_id.clone();

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == launcher_restore_id {
                show_window(&menu_window_state);
                let _ = menu_sender.send(TrayAction::ShowLauncher);
                return;
            }

            if event.id == launcher_restore_clients_id {
                let hwnds = Self::collect_hidden_client_hwnds(&menu_hidden_clients);
                if !hwnds.is_empty() {
                    let _ = GameClient::restore_windows(&hwnds);
                }
                let _ = menu_sender.send(TrayAction::RestoreAllClients);
                return;
            }

            if event.id == launcher_quit_id {
                let hwnds = Self::collect_hidden_client_hwnds(&menu_hidden_clients);
                if !hwnds.is_empty() {
                    let _ = GameClient::restore_windows(&hwnds);
                }
                std::process::exit(0);
            }
        }));

        let tray_sender = self.action_sender.clone();
        let tray_window_state = Arc::clone(&window_state);
        let tray_hidden_clients = Arc::clone(&self.hidden_client_lookup);
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
                } else if let Some(hwnd) =
                    Self::find_hidden_client_hwnd_by_icon(&tray_hidden_clients, &id)
                {
                    let _ = GameClient::restore_window(hwnd);
                    let _ = tray_sender.send(TrayAction::RestoreClient(hwnd.0 as isize));
                }
            }
            TrayIconEvent::DoubleClick { id, button, .. } if button == MouseButton::Left => {
                if id == launcher_icon_id {
                    show_window(&tray_window_state);
                    let _ = tray_sender.send(TrayAction::ShowLauncher);
                } else if let Some(hwnd) =
                    Self::find_hidden_client_hwnd_by_icon(&tray_hidden_clients, &id)
                {
                    let _ = GameClient::restore_window(hwnd);
                    let _ = tray_sender.send(TrayAction::RestoreClient(hwnd.0 as isize));
                }
            }
            _ => {}
        }));
    }

    fn sync_hidden_client_lookup(&self) {
        let next_lookup = self
            .hidden_clients
            .iter()
            .map(|(key, entry)| {
                (
                    *key,
                    HiddenClientEventEntry {
                        hwnd_raw: entry.hwnd.0 as isize,
                        icon_id: entry.icon_id.clone(),
                    },
                )
            })
            .collect();
        *self.hidden_client_lookup.lock().unwrap() = next_lookup;
    }

    fn collect_hidden_client_hwnds(
        hidden_client_lookup: &Arc<Mutex<HashMap<isize, HiddenClientEventEntry>>>,
    ) -> Vec<HWND> {
        hidden_client_lookup
            .lock()
            .unwrap()
            .values()
            .map(|entry| HWND(entry.hwnd_raw as *mut _))
            .collect()
    }

    fn find_hidden_client_hwnd_by_icon(
        hidden_client_lookup: &Arc<Mutex<HashMap<isize, HiddenClientEventEntry>>>,
        icon_id: &TrayIconId,
    ) -> Option<HWND> {
        hidden_client_lookup
            .lock()
            .unwrap()
            .values()
            .find(|entry| &entry.icon_id == icon_id)
            .map(|entry| HWND(entry.hwnd_raw as *mut _))
    }

    fn load_tray_icon() -> Result<Icon> {
        let icon = include_bytes!("../assets/penultima-phoenix.ico");
        let (icon_rgba, icon_width, icon_height) = {
            let image = image::load_from_memory(icon)
                .context("Falha ao carregar ícone da tray")?
                .into_rgba8();
            let (width, height) = image.dimensions();
            (image.into_raw(), width, height)
        };

        Icon::from_rgba(icon_rgba, icon_width, icon_height).context("Falha ao criar ícone da tray")
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

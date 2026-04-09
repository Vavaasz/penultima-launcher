use crate::constants::*;
use crate::game_client::WindowState;
use eframe::egui;
use egui::IconData;
use image;
use log::info;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use winapi::um::winuser::{
    FindWindowW, HWND_NOTOPMOST, HWND_TOPMOST, IsIconic, IsWindowVisible, SW_HIDE, SW_RESTORE,
    SW_SHOW, SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetWindowPos, ShowWindow,
};

pub struct WindowManager {
    pub window_state: Arc<Mutex<WindowState>>,
    pub needs_repaint: Arc<AtomicBool>,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            window_state: Arc::new(Mutex::new(WindowState::default())),
            needs_repaint: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Carrega o ícone da aplicação
    pub fn load_icon() -> Option<Arc<IconData>> {
        let icon = include_bytes!("../assets/penultima-phoenix.ico");
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

    /// Mostra a janela e a traz para o primeiro plano
    pub fn show_window(&self) {
        unsafe {
            use std::ffi::OsStr;
            use std::os::windows::ffi::OsStrExt;
            use std::ptr::null_mut;

            let title: Vec<u16> = OsStr::new(APP_NAME).encode_wide().chain(Some(0)).collect();

            let hwnd = FindWindowW(null_mut(), title.as_ptr());
            if !hwnd.is_null() {
                // Verifica se a janela já está visível e não está minimizada
                let is_visible = IsWindowVisible(hwnd) != 0;
                let is_minimized = IsIconic(hwnd) != 0;

                // Só executa a restauração se a janela estiver invisível ou minimizada
                if !is_visible || is_minimized {
                    info!("Janela encontrada, restaurando...");

                    // Traz a janela para frente
                    SetForegroundWindow(hwnd);
                    SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
                    SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
                    ShowWindow(hwnd, SW_RESTORE);
                    ShowWindow(hwnd, SW_SHOW);

                    // Atualiza o estado
                    let mut state = self.window_state.lock().unwrap();
                    state.visible = true;
                    state.last_show = Instant::now();

                    // Solicita repintura da UI
                    self.needs_repaint.store(true, Ordering::SeqCst);
                } else {
                    // Janela já está visível, apenas traz para frente sem logs desnecessários
                    SetForegroundWindow(hwnd);
                }
            } else {
                info!("Janela não encontrada!");
            }
        }
    }

    /// Esconde a janela
    pub fn hide_window(&self) {
        unsafe {
            use std::ffi::OsStr;
            use std::os::windows::ffi::OsStrExt;
            use std::ptr::null_mut;

            let title: Vec<u16> = OsStr::new(APP_NAME).encode_wide().chain(Some(0)).collect();

            let hwnd = FindWindowW(null_mut(), title.as_ptr());
            if !hwnd.is_null() {
                ShowWindow(hwnd, SW_HIDE);

                // Atualiza o estado
                let mut state = self.window_state.lock().unwrap();
                state.visible = false;
            } else {
                info!("Janela não encontrada!");
            }
        }
    }

    /// Configura as opções nativas da janela para o eframe
    pub fn get_native_options() -> eframe::NativeOptions {
        eframe::NativeOptions {
            persist_window: false,
            centered: true, // Isto já está correto
            vsync: true,
            renderer: eframe::Renderer::Glow,
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([WINDOW_SIZE.0, WINDOW_SIZE.1])
                .with_visible(true)
                .with_resizable(false)
                .with_maximized(false)
                .with_maximize_button(false)
                .with_title(APP_NAME)
                .with_decorations(true)
                .with_transparent(false)
                .with_active(true)
                // Remova esta linha ou substitua por .with_position(None)
                // .with_position([0.0, 0.0])
                .with_icon(Self::load_icon().unwrap_or_else(|| {
                    Arc::new(IconData {
                        rgba: Vec::new(),
                        width: 0,
                        height: 0,
                    })
                })),
            ..Default::default()
        }
    }
}

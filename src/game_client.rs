use std::os::windows::process::CommandExt;
use anyhow::{Context, Result};
use glob::glob;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use log::info;
use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
use crate::constants::*;

// Estrutura para rastrear o estado da janela
pub struct WindowState {
    pub visible: bool,
    pub last_check: Instant,
    pub last_show: Instant,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            visible: true, // Inicia com a janela visível
            last_check: Instant::now(),
            last_show: Instant::now(),
        }
    }
}

pub struct GameClient {
    pub game_process: Option<Child>,
    pub active_clients: Vec<Child>, // Lista de clients ativos
    pub max_clients: usize,         // Número máximo de clients
}

impl Default for GameClient {
    fn default() -> Self {
        Self {
            game_process: None,
            active_clients: Vec::new(),
            max_clients: 3,
        }
    }
}

impl GameClient {
    pub fn new(max_clients: usize) -> Self {
        Self {
            game_process: None,
            active_clients: Vec::new(),
            max_clients,
        }
    }

    pub fn launch_main_client(&mut self, game_path: &PathBuf) -> Result<()> {
        info!("Tentando iniciar o jogo...");

        // Procura o client.exe em subdiretórios da pasta data
        let glob_pattern = format!("{}/*/bin/client.exe", game_path.display());
        info!("Buscando client.exe com padrão: {}", glob_pattern);

        let entries = glob(&glob_pattern).context("Falha ao procurar client.exe")?;

        let client_paths: Vec<_> = entries.filter_map(Result::ok).collect();

        if client_paths.is_empty() {
            info!("client.exe não foi encontrado!");
            return Err(anyhow::anyhow!("client.exe não encontrado"));
        }

        info!(
            "Encontrados {} caminhos para client.exe",
            client_paths.len()
        );
        for (i, path) in client_paths.iter().enumerate() {
            info!("  [{}]: {}", i, path.display());
        }

        let client_path = &client_paths[0];
        info!("Usando client.exe: {}", client_path.display());

        info!(
            "Diretório de trabalho: {}",
            client_path.parent().unwrap().display()
        );

        // Definir constante para alta prioridade (Windows)
        const HIGH_PRIORITY_CLASS: u32 = 0x00000080;

        let process = Command::new(&client_path)
            .current_dir(client_path.parent().unwrap())
            .creation_flags(HIGH_PRIORITY_CLASS) // Definir prioridade alta
            .spawn()
            .context("Falha ao iniciar o jogo")?;

        info!("Processo iniciado com sucesso com prioridade alta: {:?}", process.id());

        // Armazenar o processo do jogo principal
        self.game_process = Some(process);

        Ok(())
    }

    pub fn launch_additional_client(&mut self, game_path: &PathBuf) -> Result<()> {
        // Verifica se já atingiu o limite de clients
        if self.active_clients.len() >= self.max_clients {
            return Err(anyhow::anyhow!("Número máximo de clients atingido"));
        }

        // Procura o client.exe em subdiretórios da pasta data
        let entries = glob(&format!("{}/*/bin/client.exe", game_path.display()))
            .context("Falha ao procurar client.exe")?;

        let client_path = entries
            .filter_map(Result::ok)
            .next()
            .ok_or_else(|| anyhow::anyhow!("client.exe não encontrado"))?;

        let process = Command::new(&client_path)
            .current_dir(client_path.parent().unwrap())
            .spawn()
            .context("Falha ao iniciar o jogo")?;

        self.active_clients.push(process);

        Ok(())
    }

    pub fn is_main_client_running(&mut self) -> bool {
        if let Some(process) = &mut self.game_process {
            match process.try_wait() {
                Ok(None) => {
                    true // Processo ainda está rodando
                }
                Ok(Some(_)) | Err(_) => {
                    // Processo terminou ou erro
                    self.game_process = None;
                    false
                }
            }
        } else {
            false
        }
    }

    pub fn update_additional_clients(&mut self) {
        // Limpa clients que terminaram
        self.active_clients.retain_mut(|client| {
            match client.try_wait() {
                Ok(None) => true, // Processo ainda está rodando
                _ => false,       // Processo terminou ou erro
            }
        });
    }

    pub fn terminate_process(process: &mut Child) {
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, false, process.id());
            if let Ok(handle) = handle {
                let _ = TerminateProcess(handle, 0);
                let _ = process.kill(); // Tenta matar o processo também usando o método padrão
            }
        }
    }

    pub fn terminate_all_processes(&mut self) {
        // Fecha todos os clients ativos
        for client in self.active_clients.iter_mut() {
            Self::terminate_process(client);
        }
        self.active_clients.clear();

        // Fecha o processo principal se estiver rodando
        if let Some(process) = &mut self.game_process {
            Self::terminate_process(process);
            self.game_process = None;
        }
    }

    pub fn get_clients_count(&self) -> (bool, usize) {
        (self.game_process.is_some(), self.active_clients.len())
    }
}

// Função para mostrar a janela do launcher quando minimizada
pub fn show_window(window_state: &Arc<Mutex<WindowState>>) {
    unsafe {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use std::ptr::null_mut;
        use winapi::um::winuser::{
            FindWindowW, SetForegroundWindow, SetWindowPos, ShowWindow, HWND_NOTOPMOST,
            HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE, SW_RESTORE, SW_SHOW, IsWindowVisible, IsIconic,
        };

        let title: Vec<u16> = OsStr::new(APP_NAME)
            .encode_wide()
            .chain(Some(0))
            .collect();

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
                let mut state = window_state.lock().unwrap();
                state.visible = true;
                state.last_show = Instant::now();
            } else {
                // Janela já está visível, apenas traz para frente sem logs desnecessários
                SetForegroundWindow(hwnd);
            }
        } else {
            info!("Janela não encontrada!");
        }
    }
}

// A função hide_window foi removida pois é duplicada em window_manager.rs
// Use WindowManager::hide_window() em vez disso

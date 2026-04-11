use anyhow::{Context, Result, anyhow};
use glob::glob;
use log::info;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::null;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use winapi::um::shellapi::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{GetExitCodeProcess, GetProcessId, WaitForSingleObject};

pub struct WindowState {
    pub visible: bool,
    pub last_show: Instant,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            visible: true,
            last_show: Instant::now(),
        }
    }
}

struct ProcessHandle {
    pid: u32,
    handle: HANDLE,
}

impl ProcessHandle {
    fn spawn(client_path: &Path) -> Result<Self> {
        let file_wide = wide_null(client_path.as_os_str());
        let workdir = client_path
            .parent()
            .ok_or_else(|| anyhow!("client.exe sem diretorio pai"))?;
        let workdir_wide = wide_null(workdir.as_os_str());
        let mut exec_info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        exec_info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        exec_info.fMask = SEE_MASK_NOCLOSEPROCESS;
        exec_info.lpVerb = null();
        exec_info.lpFile = file_wide.as_ptr();
        exec_info.lpDirectory = workdir_wide.as_ptr();
        exec_info.nShow = winapi::um::winuser::SW_SHOWNORMAL;

        unsafe {
            if ShellExecuteExW(&mut exec_info) == 0 {
                return Err(anyhow!("ShellExecuteExW falhou ao iniciar o client.exe"));
            }
        }

        let handle = HANDLE(exec_info.hProcess.cast());
        if handle.is_invalid() {
            return Err(anyhow!("ShellExecuteExW nao retornou handle do processo"));
        }

        let pid = unsafe { GetProcessId(handle) };
        if pid == 0 {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(anyhow!("Nao foi possivel obter o PID do client.exe"));
        }

        Ok(Self { pid, handle })
    }

    fn is_running(&self) -> bool {
        unsafe {
            if WaitForSingleObject(self.handle, 0) == WAIT_TIMEOUT {
                return true;
            }

            let mut exit_code = 0u32;
            GetExitCodeProcess(self.handle, &mut exit_code)
                .map(|_| exit_code == STILL_ACTIVE.0 as u32)
                .unwrap_or(false)
        }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

pub struct GameClient {
    game_process: Option<ProcessHandle>,
    active_clients: Vec<ProcessHandle>,
    pub max_clients: usize,
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

    pub fn find_client_path(game_path: &PathBuf) -> Result<PathBuf> {
        let direct_client = game_path.join("bin").join("client.exe");
        if direct_client.exists() {
            return Ok(direct_client);
        }

        let glob_pattern = format!("{}/*/bin/client.exe", game_path.display());
        let entries = glob(&glob_pattern).context("Falha ao procurar client.exe")?;
        entries
            .filter_map(Result::ok)
            .next()
            .ok_or_else(|| anyhow!("client.exe nao encontrado"))
    }

    pub fn launch_main_client(&mut self, game_path: &PathBuf) -> Result<()> {
        if self.is_main_client_running() {
            return Err(anyhow!("O cliente principal ja esta em execucao"));
        }

        let client_path = Self::find_client_path(game_path)?;
        info!("Usando client.exe: {}", client_path.display());

        let process =
            ProcessHandle::spawn(&client_path).context("Falha ao iniciar o client.exe")?;
        info!("Processo principal iniciado com PID {}", process.pid);

        self.game_process = Some(process);
        Ok(())
    }

    pub fn launch_additional_client(&mut self, game_path: &PathBuf) -> Result<()> {
        self.update_additional_clients();
        if self.active_clients.len() >= self.max_clients {
            return Err(anyhow!("Numero maximo de clients atingido"));
        }

        let client_path = Self::find_client_path(game_path)?;
        let process =
            ProcessHandle::spawn(&client_path).context("Falha ao iniciar client adicional")?;

        info!("Cliente adicional iniciado com PID {}", process.pid);
        self.active_clients.push(process);
        Ok(())
    }

    pub fn is_main_client_running(&mut self) -> bool {
        match &self.game_process {
            Some(process) if process.is_running() => true,
            Some(_) => {
                self.game_process = None;
                false
            }
            None => false,
        }
    }

    pub fn update_additional_clients(&mut self) {
        self.active_clients.retain(|client| client.is_running());
    }

    pub fn terminate_all_processes(&mut self) {
        self.active_clients.clear();
        self.game_process = None;
    }

    pub fn get_clients_count(&self) -> (bool, usize) {
        (self.game_process.is_some(), self.active_clients.len())
    }

    pub fn sync_client_state(&mut self) -> (bool, usize) {
        let has_main = self.is_main_client_running();
        self.update_additional_clients();
        (has_main, self.active_clients.len())
    }
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

pub fn show_window(window_state: &Arc<Mutex<WindowState>>) {
    unsafe {
        use std::ptr::null_mut;
        use winapi::um::winuser::{
            FindWindowW, HWND_NOTOPMOST, HWND_TOPMOST, IsIconic, IsWindowVisible, SW_RESTORE,
            SW_SHOW, SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetWindowPos, ShowWindow,
        };

        let title: Vec<u16> = OsStr::new(crate::constants::APP_NAME)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let hwnd = FindWindowW(null_mut(), title.as_ptr());
        if !hwnd.is_null() {
            let is_visible = IsWindowVisible(hwnd) != 0;
            let is_minimized = IsIconic(hwnd) != 0;

            if !is_visible || is_minimized {
                SetForegroundWindow(hwnd);
                SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
                SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
                ShowWindow(hwnd, SW_RESTORE);
                ShowWindow(hwnd, SW_SHOW);

                let mut state = window_state.lock().unwrap();
                state.visible = true;
                state.last_show = Instant::now();
            } else {
                SetForegroundWindow(hwnd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GameClient;
    use std::path::PathBuf;

    #[test]
    fn finds_direct_client_path() {
        let root = std::env::temp_dir().join("penultima-find-client-test");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let client = bin.join("client.exe");
        std::fs::write(&client, b"test").unwrap();

        let found = GameClient::find_client_path(&PathBuf::from(&root)).unwrap();
        assert_eq!(found, client);

        let _ = std::fs::remove_dir_all(root);
    }
}

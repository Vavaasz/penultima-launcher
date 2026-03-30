use crate::constants::*;
use anyhow::{anyhow, Context, Result};
use glob::glob;
use log::info;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, STILL_ACTIVE, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessId, SetPriorityClass, TerminateProcess, WaitForSingleObject,
    HIGH_PRIORITY_CLASS,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, SetWindowPos,
    ShowWindow, HWND_NOTOPMOST, HWND_TOPMOST, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWNORMAL,
    SWP_NOMOVE, SWP_NOSIZE,
};
use winapi::um::shellapi::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

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
    fn spawn_elevated(client_path: &PathBuf) -> Result<Self> {
        let file_wide = wide_null(client_path.as_os_str());
        let workdir = client_path
            .parent()
            .ok_or_else(|| anyhow!("client.exe sem diretÃ³rio pai"))?;
        let workdir_wide = wide_null(workdir.as_os_str());
        let verb_wide = wide_null(OsStr::new("runas"));

        let mut exec_info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        exec_info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        exec_info.fMask = SEE_MASK_NOCLOSEPROCESS;
        exec_info.lpVerb = verb_wide.as_ptr();
        exec_info.lpFile = file_wide.as_ptr();
        exec_info.lpDirectory = workdir_wide.as_ptr();
        exec_info.nShow = SW_SHOWNORMAL.0;

        unsafe {
            if ShellExecuteExW(&mut exec_info) == 0 {
                return Err(anyhow!("ShellExecuteExW falhou ao solicitar elevaÃ§Ã£o"));
            }
        }

        let handle = HANDLE(exec_info.hProcess.cast());
        if handle.is_invalid() {
            return Err(anyhow!("ShellExecuteExW nÃ£o retornou handle do processo"));
        }

        let pid = unsafe { GetProcessId(handle) };
        if pid == 0 {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(anyhow!("NÃ£o foi possÃ­vel obter o PID do client.exe"));
        }

        let process = Self {
            pid,
            handle,
        };
        process.set_high_priority();
        Ok(process)
    }

    fn set_high_priority(&self) {
        unsafe {
            let _ = SetPriorityClass(self.handle, HIGH_PRIORITY_CLASS);
        }
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

    fn terminate(&self) {
        unsafe {
            let _ = TerminateProcess(self.handle, 0);
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
    tracked_pids: Arc<Mutex<Vec<u32>>>,
}

impl Default for GameClient {
    fn default() -> Self {
        Self {
            game_process: None,
            active_clients: Vec::new(),
            max_clients: 3,
            tracked_pids: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl GameClient {
    pub fn new(max_clients: usize, tracked_pids: Arc<Mutex<Vec<u32>>>) -> Self {
        Self {
            game_process: None,
            active_clients: Vec::new(),
            max_clients,
            tracked_pids,
        }
    }

    fn sync_tracked_pids(&self) {
        let mut pids = Vec::new();
        if let Some(process) = &self.game_process {
            pids.push(process.pid);
        }
        pids.extend(self.active_clients.iter().map(|client| client.pid));
        *self.tracked_pids.lock().unwrap() = pids;
    }

    fn find_client_path(game_path: &PathBuf) -> Result<PathBuf> {
        let direct_client = game_path.join("bin").join("client.exe");
        if direct_client.exists() {
            return Ok(direct_client);
        }

        let glob_pattern = format!("{}/*/bin/client.exe", game_path.display());
        let entries = glob(&glob_pattern).context("Falha ao procurar client.exe")?;
        entries
            .filter_map(Result::ok)
            .next()
            .ok_or_else(|| anyhow!("client.exe nÃ£o encontrado"))
    }

    pub fn launch_main_client(&mut self, game_path: &PathBuf) -> Result<()> {
        info!("Tentando iniciar o jogo...");
        let client_path = Self::find_client_path(game_path)?;
        info!("Usando client.exe: {}", client_path.display());

        let process = ProcessHandle::spawn_elevated(&client_path)
            .context("Falha ao iniciar o client.exe em modo administrador")?;
        info!("Processo iniciado com PID {}", process.pid);

        self.game_process = Some(process);
        self.sync_tracked_pids();
        Ok(())
    }

    pub fn launch_additional_client(&mut self, game_path: &PathBuf) -> Result<()> {
        if self.active_clients.len() >= self.max_clients {
            return Err(anyhow!("NÃºmero mÃ¡ximo de clients atingido"));
        }

        let client_path = Self::find_client_path(game_path)?;
        let process = ProcessHandle::spawn_elevated(&client_path)
            .context("Falha ao iniciar client adicional em modo administrador")?;

        self.active_clients.push(process);
        self.sync_tracked_pids();
        Ok(())
    }

    pub fn is_main_client_running(&mut self) -> bool {
        match &self.game_process {
            Some(process) if process.is_running() => true,
            Some(_) => {
                self.game_process = None;
                self.sync_tracked_pids();
                false
            }
            None => false,
        }
    }

    pub fn update_additional_clients(&mut self) {
        let previous_len = self.active_clients.len();
        self.active_clients.retain(|client| client.is_running());
        if self.active_clients.len() != previous_len {
            self.sync_tracked_pids();
        }
    }

    pub fn terminate_all_processes(&mut self) {
        for client in &self.active_clients {
            client.terminate();
        }
        self.active_clients.clear();

        if let Some(process) = &self.game_process {
            process.terminate();
        }
        self.game_process = None;
        self.sync_tracked_pids();
    }

    pub fn get_clients_count(&self) -> (bool, usize) {
        (self.game_process.is_some(), self.active_clients.len())
    }

    pub fn minimize_all_to_tray(&mut self) -> usize {
        self.sync_tracked_pids();
        let mut pids = self.tracked_pids.lock().unwrap().clone();
        let mut changed = Self::apply_window_visibility_to_pids(&pids, true);
        if changed == 0 {
            pids = Self::find_all_client_process_ids();
            changed = Self::apply_window_visibility_to_pids(&pids, true);
        }
        changed
    }

    pub fn restore_all_from_tray(tracked_pids: &Arc<Mutex<Vec<u32>>>) -> usize {
        let mut pids = tracked_pids.lock().unwrap().clone();
        let mut changed = Self::apply_window_visibility_to_pids(&pids, false);
        if changed == 0 {
            pids = Self::find_all_client_process_ids();
            changed = Self::apply_window_visibility_to_pids(&pids, false);
        }
        changed
    }

    pub fn restore_all_from_tray_for_launcher(&self) -> usize {
        Self::restore_all_from_tray(&self.tracked_pids)
    }

    fn find_all_client_process_ids() -> Vec<u32> {
        let mut pids = Vec::new();

        unsafe {
            let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
                Ok(handle) => handle,
                Err(_) => return pids,
            };

            let mut entry = PROCESSENTRY32W::default();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    let exe_name_end = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let exe_name = String::from_utf16_lossy(&entry.szExeFile[..exe_name_end]);

                    if exe_name.eq_ignore_ascii_case("client.exe") {
                        pids.push(entry.th32ProcessID);
                    }

                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
        }

        pids
    }

    fn apply_window_visibility_to_pids(pids: &[u32], hide: bool) -> usize {
        unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let context = unsafe { &mut *(lparam.0 as *mut WindowEnumerationContext) };
            let mut window_pid = 0u32;
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut window_pid));
            }

            if !context.pids.contains(&window_pid) {
                return BOOL(1);
            }

            if context.hide {
                if unsafe { IsWindowVisible(hwnd) }.as_bool() {
                    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
                    context.changed += 1;
                }
            } else {
                let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
                let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
                let _ = unsafe { SetForegroundWindow(hwnd) };
                let _ = unsafe {
                    SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
                };
                let _ = unsafe {
                    SetWindowPos(
                        hwnd,
                        Some(HWND_NOTOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE,
                    )
                };
                context.changed += 1;
            }

            BOOL(1)
        }

        if pids.is_empty() {
            return 0;
        }

        let mut context = WindowEnumerationContext {
            pids: pids.to_vec(),
            hide,
            changed: 0,
        };

        unsafe {
            let _ = EnumWindows(
                Some(enum_windows_proc),
                LPARAM(&mut context as *mut WindowEnumerationContext as isize),
            );
        }

        context.changed
    }
}

struct WindowEnumerationContext {
    pids: Vec<u32>,
    hide: bool,
    changed: usize,
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

pub fn show_window(window_state: &Arc<Mutex<WindowState>>) {
    unsafe {
        use std::ptr::null_mut;
        use winapi::um::winuser::{
            FindWindowW, HWND_NOTOPMOST, HWND_TOPMOST, IsIconic, IsWindowVisible, SWP_NOMOVE,
            SWP_NOSIZE, SW_RESTORE, SW_SHOW, SetForegroundWindow, SetWindowPos, ShowWindow,
        };

        let title: Vec<u16> = OsStr::new(APP_NAME).encode_wide().chain(Some(0)).collect();
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

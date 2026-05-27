use anyhow::{Context, Result, anyhow};
use glob::glob;
use log::info;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::null;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winapi::shared::minwindef::{BOOL, DWORD, LPARAM, TRUE};
use winapi::shared::windef::{HWND, RECT};
use winapi::um::handleapi::{CloseHandle as CloseRawHandle, INVALID_HANDLE_VALUE};
use winapi::um::processthreadsapi::OpenProcess;
use winapi::um::shellapi::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use winapi::um::tlhelp32::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use winapi::um::winbase::QueryFullProcessImageNameW;
use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;
use winapi::um::winuser::{
    EnumWindows, GetWindowPlacement, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_HIDE, SW_RESTORE, SW_SHOW,
    SW_SHOWMAXIMIZED, SW_SHOWNORMAL, SetForegroundWindow, SetWindowPlacement, ShowWindow,
    WINDOWPLACEMENT,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{GetExitCodeProcess, GetProcessId, WaitForSingleObject};

const CLIENT_WINDOW_TITLE_PREFIX: &str = "Tibia - ";
const PROD_CLIENT_LAUNCHER_EXE: &str = "client_launcher.exe";
const OTCLIENT_LAUNCHER_EXE: &str = "OTCLauncher.exe";
const CLIENT_STATE_CACHE_DURATION: Duration = Duration::from_millis(2000);
const PROCESS_IMAGE_BUFFER_LEN: usize = 32768;

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
    fn spawn(executable_path: &Path) -> Result<Self> {
        match Self::spawn_with_command(executable_path) {
            Ok(process) => return Ok(process),
            Err(error) => {
                info!(
                    "Command::spawn falhou para {}; tentando ShellExecuteExW: {:#}",
                    executable_path.display(),
                    error
                );
            }
        }

        Self::spawn_with_shell_execute(executable_path)
    }

    fn spawn_with_command(executable_path: &Path) -> Result<Self> {
        let workdir = executable_path
            .parent()
            .ok_or_else(|| anyhow!("executavel sem diretorio pai"))?;
        let child = Command::new(executable_path)
            .current_dir(workdir)
            .spawn()
            .context("CreateProcess falhou ao iniciar o executavel")?;
        let pid = child.id();
        let handle = HANDLE(child.as_raw_handle().cast());
        std::mem::forget(child);

        if handle.is_invalid() {
            return Err(anyhow!("CreateProcess nao retornou handle do processo"));
        }

        Ok(Self { pid, handle })
    }

    fn spawn_with_shell_execute(executable_path: &Path) -> Result<Self> {
        let file_wide = wide_null(executable_path.as_os_str());
        let workdir = executable_path
            .parent()
            .ok_or_else(|| anyhow!("executavel sem diretorio pai"))?;
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
                return Err(anyhow!("ShellExecuteExW falhou ao iniciar o executavel"));
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
            return Err(anyhow!("Nao foi possivel obter o PID do processo"));
        }

        Ok(Self { pid, handle })
    }

    fn exited_within(&self, timeout: Duration) -> Option<u32> {
        unsafe {
            if WaitForSingleObject(
                self.handle,
                timeout.as_millis().min(u32::MAX as u128) as u32,
            ) == WAIT_TIMEOUT
            {
                return None;
            }

            let mut exit_code = 0u32;
            GetExitCodeProcess(self.handle, &mut exit_code).ok()?;
            Some(exit_code)
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
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

struct ClientWindowSearch {
    pids: HashSet<u32>,
    windows: Vec<HWND>,
}

struct AllClientWindowSearch {
    windows: Vec<HWND>,
}

struct ClientWindow {
    hwnd: HWND,
    info: ClientWindowInfo,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct ClientWindowState {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    maximized: bool,
}

impl ClientWindowState {
    fn is_valid(self) -> bool {
        self.width >= 640 && self.height >= 480
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientWindowInfo {
    pub pid: u32,
    pub title: String,
    pub character_name: String,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningClientProcessInfo {
    pub pid: u32,
    pub executable_path: PathBuf,
}

unsafe extern "system" fn enum_client_windows(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = unsafe { &mut *(lparam as *mut ClientWindowSearch) };
    let mut pid = 0u32;

    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
        if search.pids.contains(&pid) && GetWindowTextLengthW(hwnd) > 0 {
            search.windows.push(hwnd);
        }
    }

    TRUE
}

unsafe extern "system" fn enum_all_windows(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = unsafe { &mut *(lparam as *mut AllClientWindowSearch) };

    unsafe {
        if GetWindowTextLengthW(hwnd) > 0 {
            search.windows.push(hwnd);
        }
    }

    TRUE
}

pub struct GameClient {
    game_process: Option<ProcessHandle>,
    active_clients: Vec<ProcessHandle>,
    window_state_path: Option<PathBuf>,
    last_window_state: Option<ClientWindowState>,
    pending_window_restore_pids: HashSet<u32>,
    last_sync_state: Option<(Instant, bool, usize)>,
}

impl Default for GameClient {
    fn default() -> Self {
        Self {
            game_process: None,
            active_clients: Vec::new(),
            window_state_path: None,
            last_window_state: None,
            pending_window_restore_pids: HashSet::new(),
            last_sync_state: None,
        }
    }
}

impl GameClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_window_state_path(&mut self, path: PathBuf) {
        self.window_state_path = Some(path);
    }

    pub fn find_client_path(game_path: &PathBuf) -> Result<PathBuf> {
        let direct_client = game_path.join("bin").join("client.exe");
        if direct_client.exists() {
            return Ok(direct_client);
        }

        let glob_pattern = format!("{}/*/bin/client.exe", game_path.display());
        let entries = glob(&glob_pattern).context("Falha ao procurar client.exe")?;
        if let Some(path) = entries.filter_map(Result::ok).next() {
            return Ok(path);
        }

        let direct_launcher = game_path.join("bin").join(PROD_CLIENT_LAUNCHER_EXE);
        if direct_launcher.exists() {
            return Ok(direct_launcher);
        }

        let launcher_glob_pattern =
            format!("{}/*/bin/{}", game_path.display(), PROD_CLIENT_LAUNCHER_EXE);
        let launcher_entries =
            glob(&launcher_glob_pattern).context("Falha ao procurar client_launcher.exe")?;
        launcher_entries
            .filter_map(Result::ok)
            .next()
            .ok_or_else(|| anyhow!("client.exe nao encontrado"))
    }

    pub fn running_client_processes_for_game_path(
        game_path: &Path,
    ) -> Vec<RunningClientProcessInfo> {
        running_client_processes_for_game_path(game_path)
    }

    pub fn has_client_processes_for_game_path(game_path: &Path) -> bool {
        !Self::running_client_processes_for_game_path(game_path).is_empty()
    }

    pub fn launch_main_client(&mut self, game_path: &PathBuf) -> Result<()> {
        if self.is_main_client_running() {
            return Err(anyhow!("O cliente principal ja esta em execucao"));
        }

        let running_clients = Self::running_client_processes_for_game_path(game_path);
        if !running_clients.is_empty() {
            let pids = running_clients
                .iter()
                .map(|process| process.pid.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(anyhow!("O cliente ja esta em execucao (PIDs: {pids})"));
        }

        let client_path = Self::find_client_path(game_path)?;
        info!("Usando executavel do cliente: {}", client_path.display());

        let process = ProcessHandle::spawn(&client_path).context("Falha ao iniciar o cliente")?;
        if let Some(exit_code) = process.exited_within(Duration::from_millis(1200)) {
            return Err(anyhow!(
                "O cliente fechou logo apos iniciar (codigo {}). Use Force Update e tente novamente.",
                exit_code
            ));
        }
        info!("Processo principal iniciado com PID {}", process.pid);

        self.pending_window_restore_pids.insert(process.pid);
        self.game_process = Some(process);
        self.invalidate_client_state_cache();
        Ok(())
    }

    pub fn launch_additional_client(&mut self, game_path: &PathBuf) -> Result<()> {
        self.sync_client_state_now();

        let client_path = Self::find_client_path(game_path)?;
        let process =
            ProcessHandle::spawn(&client_path).context("Falha ao iniciar client adicional")?;
        if let Some(exit_code) = process.exited_within(Duration::from_millis(1200)) {
            return Err(anyhow!(
                "O cliente adicional fechou logo apos iniciar (codigo {}). Use Force Update e tente novamente.",
                exit_code
            ));
        }

        info!("Cliente adicional iniciado com PID {}", process.pid);
        self.pending_window_restore_pids.insert(process.pid);
        self.active_clients.push(process);
        self.invalidate_client_state_cache();
        Ok(())
    }

    pub fn launch_otclient_launcher(&mut self, launcher_path: &PathBuf) -> Result<()> {
        self.sync_client_state_now();

        if !launcher_path.exists() {
            return Err(anyhow!("OTCLauncher.exe nao encontrado"));
        }

        let executable_name = launcher_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if !executable_name.eq_ignore_ascii_case(OTCLIENT_LAUNCHER_EXE) {
            return Err(anyhow!(
                "OTClient deve ser iniciado por {}, nao por {}",
                OTCLIENT_LAUNCHER_EXE,
                launcher_path.display()
            ));
        }

        let process = ProcessHandle::spawn(launcher_path).context("Falha ao iniciar OTClient")?;
        if let Some(exit_code) = process.exited_within(Duration::from_millis(1200)) {
            return Err(anyhow!(
                "OTClient fechou logo apos iniciar (codigo {}).",
                exit_code
            ));
        }
        info!("OTClient launcher iniciado com PID {}", process.pid);
        self.pending_window_restore_pids.insert(process.pid);
        self.active_clients.push(process);
        self.invalidate_client_state_cache();
        Ok(())
    }

    pub fn is_main_client_running(&mut self) -> bool {
        match &self.game_process {
            Some(process) if process.is_running() => true,
            Some(process) => {
                self.pending_window_restore_pids.remove(&process.pid);
                self.game_process = None;
                false
            }
            None => false,
        }
    }

    pub fn update_additional_clients(&mut self) {
        let closed_pids: Vec<u32> = self
            .active_clients
            .iter()
            .filter(|client| !client.is_running())
            .map(|client| client.pid)
            .collect();
        let had_closed_clients = !closed_pids.is_empty();
        for pid in closed_pids {
            self.pending_window_restore_pids.remove(&pid);
        }
        self.active_clients.retain(|client| client.is_running());
        if had_closed_clients {
            self.invalidate_client_state_cache();
        }
    }

    pub fn terminate_all_processes(&mut self) {
        self.active_clients.clear();
        self.game_process = None;
        self.pending_window_restore_pids.clear();
        self.invalidate_client_state_cache();
    }

    pub fn get_clients_count(&self) -> (bool, usize) {
        (self.game_process.is_some(), self.active_clients.len())
    }

    pub fn sync_client_state(&mut self) -> (bool, usize) {
        if let Some((synced_at, has_main, additional_count)) = self.last_sync_state {
            if synced_at.elapsed() < CLIENT_STATE_CACHE_DURATION {
                return (has_main, additional_count);
            }
        }

        self.sync_client_state_now()
    }

    pub fn sync_client_state_now(&mut self) -> (bool, usize) {
        let has_main = self.is_main_client_running();
        self.update_additional_clients();
        self.restore_pending_client_window_states();
        self.remember_current_client_window_state();
        let state = (has_main, self.active_clients.len());
        self.last_sync_state = Some((Instant::now(), state.0, state.1));
        state
    }

    pub fn has_tracked_clients(&mut self) -> bool {
        let (has_main, additional_count) = self.sync_client_state();
        has_main || additional_count > 0
    }

    pub fn visible_client_window_infos(&mut self) -> Vec<ClientWindowInfo> {
        self.sync_client_state_now();
        unique_client_window_infos(
            self.all_client_window_details()
                .into_iter()
                .filter(|client_window| client_window.info.visible)
                .map(|client_window| client_window.info)
                .collect(),
        )
    }

    pub fn minimize_clients_to_tray(&mut self) -> usize {
        self.minimize_client_windows_to_tray(None)
    }

    pub fn minimize_client_to_tray(&mut self, pid: u32) -> Option<ClientWindowInfo> {
        let minimized_count = self.minimize_client_windows_to_tray(Some(pid));
        if minimized_count == 0 {
            None
        } else {
            self.all_client_window_infos()
                .into_iter()
                .find(|client| client.pid == pid)
        }
    }

    fn minimize_client_windows_to_tray(&mut self, pid: Option<u32>) -> usize {
        self.sync_client_state_now();
        let mut hidden_pids = HashSet::new();
        for client_window in self.all_client_window_details() {
            if pid.is_some_and(|target_pid| client_window.info.pid != target_pid) {
                continue;
            }

            unsafe {
                if client_window.info.visible {
                    ShowWindow(client_window.hwnd, SW_HIDE);
                    hidden_pids.insert(client_window.info.pid);
                }
            }
        }

        hidden_pids.len()
    }

    pub fn restore_clients_from_tray(&mut self) -> usize {
        self.sync_client_state_now();

        let mut restored_pids = HashSet::new();
        for client_window in self.all_client_window_details() {
            if client_window.info.visible {
                continue;
            }

            unsafe {
                ShowWindow(client_window.hwnd, SW_RESTORE);
                ShowWindow(client_window.hwnd, SW_SHOW);
                SetForegroundWindow(client_window.hwnd);
            }
            restored_pids.insert(client_window.info.pid);
        }

        restored_pids.len()
    }

    pub fn restore_client_from_tray(&mut self, pid: u32) -> Option<ClientWindowInfo> {
        self.sync_client_state_now();

        let mut restored_client = None;
        for client_window in self.all_client_window_details() {
            if client_window.info.pid != pid {
                continue;
            }

            unsafe {
                ShowWindow(client_window.hwnd, SW_RESTORE);
                ShowWindow(client_window.hwnd, SW_SHOW);
                SetForegroundWindow(client_window.hwnd);
            }

            if restored_client.is_none() {
                restored_client = Some(client_window.info);
            }
        }

        restored_client
    }

    pub fn hidden_client_window_infos(&mut self) -> Vec<ClientWindowInfo> {
        self.sync_client_state_now();
        unique_client_window_infos(
            self.all_client_window_details()
                .into_iter()
                .filter(|client_window| !client_window.info.visible)
                .map(|client_window| client_window.info)
                .collect(),
        )
    }

    pub fn client_window_infos(&mut self) -> Vec<ClientWindowInfo> {
        self.sync_client_state_now();
        self.all_client_window_infos()
    }

    fn invalidate_client_state_cache(&mut self) {
        self.last_sync_state = None;
    }

    fn all_client_window_infos(&self) -> Vec<ClientWindowInfo> {
        unique_client_window_infos(
            self.all_client_window_details()
                .into_iter()
                .map(|client_window| client_window.info)
                .collect(),
        )
    }

    fn all_client_window_details(&self) -> Vec<ClientWindow> {
        let tracked_pids = self.tracked_client_pids();
        let mut search = AllClientWindowSearch {
            windows: Vec::new(),
        };

        unsafe {
            EnumWindows(
                Some(enum_all_windows),
                &mut search as *mut AllClientWindowSearch as LPARAM,
            );
        }

        search
            .windows
            .into_iter()
            .filter_map(client_window_from_hwnd)
            .filter(|client_window| {
                tracked_pids.contains(&client_window.info.pid)
                    || is_client_window_title(&client_window.info.title)
            })
            .collect()
    }

    fn tracked_client_pids(&self) -> HashSet<u32> {
        let mut pids = HashSet::new();
        if let Some(process) = &self.game_process {
            pids.insert(process.pid);
        }

        pids.extend(self.active_clients.iter().map(|client| client.pid));
        pids
    }

    fn tracked_client_windows(&self) -> Vec<HWND> {
        let pids = self.tracked_client_pids();
        if pids.is_empty() {
            return Vec::new();
        }

        let mut search = ClientWindowSearch {
            pids,
            windows: Vec::new(),
        };

        unsafe {
            EnumWindows(
                Some(enum_client_windows),
                &mut search as *mut ClientWindowSearch as LPARAM,
            );
        }

        search.windows
    }

    fn tracked_client_window_details(&self) -> Vec<ClientWindow> {
        self.tracked_client_windows()
            .into_iter()
            .filter_map(client_window_from_hwnd)
            .collect()
    }

    fn remember_current_client_window_state(&mut self) {
        if self.window_state_path.is_none() {
            return;
        }

        let Some(state) = self
            .tracked_client_window_details()
            .into_iter()
            .find(|client_window| {
                client_window.info.visible
                    && !self
                        .pending_window_restore_pids
                        .contains(&client_window.info.pid)
            })
            .and_then(|client_window| client_window_state_from_hwnd(client_window.hwnd))
        else {
            return;
        };

        self.save_client_window_state(state);
    }

    fn restore_pending_client_window_states(&mut self) {
        if self.pending_window_restore_pids.is_empty() {
            return;
        }

        let Some(state) = self.load_client_window_state() else {
            self.pending_window_restore_pids.clear();
            return;
        };

        let mut restored_pids = Vec::new();
        for client_window in self.tracked_client_window_details() {
            if !self
                .pending_window_restore_pids
                .contains(&client_window.info.pid)
            {
                continue;
            }

            if apply_client_window_state(client_window.hwnd, state) {
                restored_pids.push(client_window.info.pid);
            }
        }

        for pid in restored_pids {
            self.pending_window_restore_pids.remove(&pid);
        }
    }

    fn load_client_window_state(&mut self) -> Option<ClientWindowState> {
        if let Some(state) = self.last_window_state.filter(|state| state.is_valid()) {
            return Some(state);
        }

        let path = self.window_state_path.as_ref()?;
        let data = fs::read(path).ok()?;
        let state = serde_json::from_slice::<ClientWindowState>(&data).ok()?;
        if !state.is_valid() {
            return None;
        }

        self.last_window_state = Some(state);
        Some(state)
    }

    fn save_client_window_state(&mut self, state: ClientWindowState) {
        if !state.is_valid() || self.last_window_state == Some(state) {
            return;
        }

        let Some(path) = self.window_state_path.as_ref() else {
            return;
        };

        if let Some(parent) = path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                info!("Nao foi possivel criar diretorio de estado da janela: {error}");
                return;
            }
        }

        match serde_json::to_vec_pretty(&state) {
            Ok(data) => {
                if let Err(error) = fs::write(path, data) {
                    info!("Nao foi possivel salvar tamanho da janela do cliente: {error}");
                } else {
                    self.last_window_state = Some(state);
                }
            }
            Err(error) => {
                info!("Nao foi possivel serializar tamanho da janela do cliente: {error}");
            }
        }
    }
}

fn client_window_from_hwnd(hwnd: HWND) -> Option<ClientWindow> {
    let mut pid = 0u32;

    let (title, visible) = unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
        (window_title(hwnd)?, IsWindowVisible(hwnd) != 0)
    };

    if pid == 0 {
        return None;
    }

    let character_name = character_name_from_window_title(&title, pid);

    Some(ClientWindow {
        hwnd,
        info: ClientWindowInfo {
            pid,
            title,
            character_name,
            visible,
        },
    })
}

fn window_title(hwnd: HWND) -> Option<String> {
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return None;
    }

    let mut buffer = vec![0u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if copied <= 0 {
        return None;
    }

    let title = String::from_utf16_lossy(&buffer[..copied as usize])
        .trim()
        .to_string();
    if title.is_empty() { None } else { Some(title) }
}

fn client_window_state_from_hwnd(hwnd: HWND) -> Option<ClientWindowState> {
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return None;
        }

        let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
        placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd, &mut placement) == 0 {
            return None;
        }

        let rect = placement.rcNormalPosition;
        let state = ClientWindowState {
            left: rect.left,
            top: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
            maximized: placement.showCmd == SW_SHOWMAXIMIZED as u32,
        };

        state.is_valid().then_some(state)
    }
}

fn apply_client_window_state(hwnd: HWND, state: ClientWindowState) -> bool {
    if !state.is_valid() {
        return false;
    }

    unsafe {
        let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
        placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd, &mut placement) == 0 {
            return false;
        }

        placement.rcNormalPosition = RECT {
            left: state.left,
            top: state.top,
            right: state.left + state.width,
            bottom: state.top + state.height,
        };
        placement.showCmd = if state.maximized {
            SW_SHOWMAXIMIZED as u32
        } else {
            SW_SHOWNORMAL as u32
        };

        SetWindowPlacement(hwnd, &placement) != 0
    }
}

pub fn character_name_from_window_title(title: &str, pid: u32) -> String {
    let trimmed = title.trim();
    if let Some(character_name) = trimmed
        .strip_prefix(CLIENT_WINDOW_TITLE_PREFIX.trim_end())
        .map(str::trim)
    {
        if !character_name.is_empty() {
            return character_name.to_string();
        }
    }

    if trimmed.is_empty() || trimmed == CLIENT_WINDOW_TITLE_PREFIX.trim_end() {
        return format!("Cliente {}", pid);
    }

    trimmed.to_string()
}

fn is_client_window_title(title: &str) -> bool {
    let trimmed = title.trim();
    trimmed.eq_ignore_ascii_case("Tibia")
        || trimmed == CLIENT_WINDOW_TITLE_PREFIX.trim_end()
        || trimmed.starts_with(CLIENT_WINDOW_TITLE_PREFIX)
}

fn client_window_title_rank(title: &str) -> usize {
    title
        .trim()
        .strip_prefix(CLIENT_WINDOW_TITLE_PREFIX)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|_| 0)
        .unwrap_or(1)
}

fn unique_client_window_infos(mut infos: Vec<ClientWindowInfo>) -> Vec<ClientWindowInfo> {
    infos.sort_by(|a, b| {
        client_window_title_rank(&a.title)
            .cmp(&client_window_title_rank(&b.title))
            .then_with(|| a.pid.cmp(&b.pid))
    });

    let mut seen_pids = HashSet::new();
    infos.retain(|info| seen_pids.insert(info.pid));

    infos.sort_by(|a, b| {
        a.character_name
            .to_lowercase()
            .cmp(&b.character_name.to_lowercase())
            .then_with(|| a.pid.cmp(&b.pid))
    });

    infos
}

fn running_client_processes_for_game_path(game_path: &Path) -> Vec<RunningClientProcessInfo> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }

    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as DWORD;
    let mut processes = Vec::new();

    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) != 0 };
    while has_entry {
        let executable_name = wide_fixed_to_string(&entry.szExeFile);
        if is_client_process_executable_name(&executable_name) {
            if let Some(executable_path) = query_process_image_path(entry.th32ProcessID) {
                if path_is_within_root(&executable_path, game_path) {
                    processes.push(RunningClientProcessInfo {
                        pid: entry.th32ProcessID,
                        executable_path,
                    });
                }
            }
        }

        has_entry = unsafe { Process32NextW(snapshot, &mut entry) != 0 };
    }

    unsafe {
        CloseRawHandle(snapshot);
    }

    processes.sort_by_key(|process| process.pid);
    processes
}

fn query_process_image_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }

        let mut buffer = vec![0u16; PROCESS_IMAGE_BUFFER_LEN];
        let mut len = buffer.len() as DWORD;
        let success = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut len);
        CloseRawHandle(handle);

        if success == 0 || len == 0 {
            return None;
        }

        Some(PathBuf::from(OsString::from_wide(&buffer[..len as usize])))
    }
}

fn wide_fixed_to_string(value: &[u16]) -> String {
    let len = value
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(value.len());
    OsString::from_wide(&value[..len])
        .to_string_lossy()
        .into_owned()
}

fn is_client_process_executable_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("client.exe") || name.eq_ignore_ascii_case(PROD_CLIENT_LAUNCHER_EXE)
}

fn path_is_within_root(path: &Path, root: &Path) -> bool {
    let path = comparable_path(path);
    let root = comparable_path(root);
    path == root || path.starts_with(&format!("{root}\\"))
}

fn comparable_path(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
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
    use super::{
        ClientWindowInfo, GameClient, character_name_from_window_title,
        is_client_process_executable_name, is_client_window_title, path_is_within_root,
    };
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

    #[test]
    fn prefers_direct_client_when_launcher_is_also_available() {
        let root = std::env::temp_dir().join("penultima-find-client-launcher-test");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let client = bin.join("client.exe");
        let launcher = bin.join("client_launcher.exe");
        std::fs::write(&client, b"client").unwrap();
        std::fs::write(&launcher, b"launcher").unwrap();

        let found = GameClient::find_client_path(&PathBuf::from(&root)).unwrap();
        assert_eq!(found, client);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn prefers_nested_client_when_launcher_is_also_available() {
        let root = std::env::temp_dir().join("penultima-find-nested-client-launcher-test");
        let bin = root.join("Penultima").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let client = bin.join("client.exe");
        let launcher = bin.join("client_launcher.exe");
        std::fs::write(&client, b"client").unwrap();
        std::fs::write(&launcher, b"launcher").unwrap();

        let found = GameClient::find_client_path(&PathBuf::from(&root)).unwrap();
        assert_eq!(found, client);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_character_name_from_tibia_window_title() {
        assert_eq!(
            character_name_from_window_title("Tibia - Waldir", 123),
            "Waldir"
        );
        assert_eq!(
            character_name_from_window_title("  Tibia -  Mage Name  ", 123),
            "Mage Name"
        );
    }

    #[test]
    fn falls_back_to_title_or_pid_for_client_name() {
        assert_eq!(
            character_name_from_window_title("Custom Client Title", 55),
            "Custom Client Title"
        );
        assert_eq!(
            character_name_from_window_title("Tibia - ", 55),
            "Cliente 55"
        );
        assert_eq!(character_name_from_window_title("", 55), "Cliente 55");
    }

    #[test]
    fn detects_tibia_client_window_titles() {
        assert!(is_client_window_title("Tibia"));
        assert!(is_client_window_title("Tibia - Waldir"));
        assert!(!is_client_window_title("Other Window"));
    }

    #[test]
    fn detects_client_process_executable_names() {
        assert!(is_client_process_executable_name("client.exe"));
        assert!(is_client_process_executable_name("CLIENT_LAUNCHER.EXE"));
        assert!(!is_client_process_executable_name("penultima-launcher.exe"));
    }

    #[test]
    fn matches_process_path_inside_game_root() {
        let root = PathBuf::from(r"C:\Users\Waldir\AppData\Roaming\Penultima Launcher\game");
        assert!(path_is_within_root(
            &root.join("bin").join("client.exe"),
            &root
        ));
        assert!(!path_is_within_root(
            &PathBuf::from(r"D:\Server\Tibia 15.23.bf9553-original-windows\bin\client.exe"),
            &root
        ));
    }

    #[test]
    fn unique_client_infos_prefers_tibia_character_title() {
        let infos = super::unique_client_window_infos(vec![
            ClientWindowInfo {
                pid: 10,
                title: "Other Window".to_string(),
                character_name: "Other Window".to_string(),
                visible: false,
            },
            ClientWindowInfo {
                pid: 10,
                title: "Tibia - Waldir".to_string(),
                character_name: "Waldir".to_string(),
                visible: false,
            },
            ClientWindowInfo {
                pid: 20,
                title: "Tibia - Alice".to_string(),
                character_name: "Alice".to_string(),
                visible: false,
            },
        ]);

        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].character_name, "Alice");
        assert_eq!(infos[1].character_name, "Waldir");
    }
}

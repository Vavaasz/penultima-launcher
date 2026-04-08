use anyhow::{Context, Result, anyhow};
use glob::glob;
use log::info;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::null;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use winapi::um::shellapi::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, STILL_ACTIVE, WAIT_TIMEOUT};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessId, HIGH_PRIORITY_CLASS, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, SetPriorityClass,
    TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HWND_NOTOPMOST,
    HWND_TOPMOST, IsIconic, IsWindow, IsWindowVisible, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWNORMAL,
    SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetWindowPos, ShowWindow,
};
use windows::core::{BOOL, PWSTR};

use crate::constants::TRAY_OFFLINE_NAME;

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

#[derive(Clone, Debug)]
pub struct ClientWindowInfo {
    pub hwnd: HWND,
    pub pid: u32,
    pub window_title: String,
    pub display_name: String,
}

struct ProcessHandle {
    pid: u32,
    handle: HANDLE,
}

impl ProcessHandle {
    fn spawn(client_path: &PathBuf) -> Result<Self> {
        let file_wide = wide_null(client_path.as_os_str());
        let workdir = client_path
            .parent()
            .ok_or_else(|| anyhow!("client.exe sem diretório pai"))?;
        let workdir_wide = wide_null(workdir.as_os_str());
        let mut exec_info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        exec_info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        exec_info.fMask = SEE_MASK_NOCLOSEPROCESS;
        exec_info.lpVerb = null();
        exec_info.lpFile = file_wide.as_ptr();
        exec_info.lpDirectory = workdir_wide.as_ptr();
        exec_info.nShow = SW_SHOWNORMAL.0;

        unsafe {
            if ShellExecuteExW(&mut exec_info) == 0 {
                return Err(anyhow!("ShellExecuteExW falhou ao solicitar elevação"));
            }
        }

        let handle = HANDLE(exec_info.hProcess.cast());
        if handle.is_invalid() {
            return Err(anyhow!("ShellExecuteExW não retornou handle do processo"));
        }

        let pid = unsafe { GetProcessId(handle) };
        if pid == 0 {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(anyhow!("Não foi possível obter o PID do client.exe"));
        }

        let process = Self { pid, handle };
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
            .ok_or_else(|| anyhow!("client.exe não encontrado"))
    }

    pub fn launch_main_client(&mut self, game_path: &PathBuf) -> Result<()> {
        if self.is_main_client_running() {
            return Err(anyhow!("O cliente principal jÃ¡ estÃ¡ em execuÃ§Ã£o"));
        }

        let client_path = Self::find_client_path(game_path)?;
        info!("Usando client.exe: {}", client_path.display());

        let process =
            ProcessHandle::spawn(&client_path).context("Falha ao iniciar o client.exe")?;
        info!("Processo principal iniciado com PID {}", process.pid);

        self.game_process = Some(process);
        self.sync_tracked_pids();
        Ok(())
    }

    pub fn launch_additional_client(&mut self, game_path: &PathBuf) -> Result<()> {
        self.update_additional_clients();
        if self.active_clients.len() >= self.max_clients {
            return Err(anyhow!("Número máximo de clients atingido"));
        }

        let client_path = Self::find_client_path(game_path)?;
        let process =
            ProcessHandle::spawn(&client_path).context("Falha ao iniciar client adicional")?;

        info!("Cliente adicional iniciado com PID {}", process.pid);
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

    pub fn sync_client_state(&mut self) -> (bool, usize) {
        let has_main = self.is_main_client_running();
        self.update_additional_clients();
        (has_main, self.active_clients.len())
    }

    pub fn minimize_declared_clients_to_tray(
        &mut self,
        game_path: &PathBuf,
    ) -> Result<Vec<ClientWindowInfo>> {
        let windows = self.find_declared_client_windows(game_path)?;
        for window in &windows {
            unsafe {
                let _ = ShowWindow(window.hwnd, SW_HIDE);
            }
        }
        Ok(windows)
    }

    pub fn find_declared_client_windows(
        &mut self,
        game_path: &PathBuf,
    ) -> Result<Vec<ClientWindowInfo>> {
        self.sync_tracked_pids();
        let pids = self.collect_scoped_client_process_ids(game_path)?;
        Ok(Self::enumerate_visible_windows(&pids))
    }

    pub fn restore_windows(hwnds: &[HWND]) -> usize {
        hwnds
            .iter()
            .filter(|hwnd| Self::restore_window(**hwnd))
            .count()
    }

    pub fn restore_window(hwnd: HWND) -> bool {
        if !Self::is_window_alive(hwnd) {
            return false;
        }

        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE,
            );
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE,
            );
        }

        true
    }

    pub fn is_window_alive(hwnd: HWND) -> bool {
        unsafe { IsWindow(Some(hwnd)).as_bool() }
    }

    pub fn is_window_hidden(hwnd: HWND) -> bool {
        Self::is_window_alive(hwnd) && unsafe { !IsWindowVisible(hwnd).as_bool() }
    }

    fn collect_scoped_client_process_ids(&self, game_path: &PathBuf) -> Result<Vec<u32>> {
        let declared_client = Self::find_client_path(game_path)?;
        let declared_bin = declared_client
            .parent()
            .ok_or_else(|| anyhow!("client.exe sem diretório pai"))?;

        let mut pids: HashSet<u32> = self.tracked_pids.lock().unwrap().iter().copied().collect();

        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
                .context("Falha ao criar snapshot de processos")?;

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
                        if let Some(process_path) =
                            Self::get_process_executable_path(entry.th32ProcessID)
                        {
                            if Self::matches_declared_bin(&process_path, declared_bin) {
                                pids.insert(entry.th32ProcessID);
                            }
                        }
                    }

                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }

            let _ = CloseHandle(snapshot);
        }

        Ok(pids.into_iter().collect())
    }

    fn matches_declared_bin(process_path: &Path, declared_bin: &Path) -> bool {
        let process_parent = process_path.parent();
        match process_parent {
            Some(parent) => normalized_path(parent) == normalized_path(declared_bin),
            None => false,
        }
    }

    fn get_process_executable_path(pid: u32) -> Option<PathBuf> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buffer = vec![0u16; 32768];
            let mut size = buffer.len() as u32;
            let result = QueryFullProcessImageNameW(
                handle,
                windows::Win32::System::Threading::PROCESS_NAME_FORMAT(0),
                PWSTR(buffer.as_mut_ptr()),
                &mut size,
            )
            .is_ok();
            let _ = CloseHandle(handle);

            if !result || size == 0 {
                return None;
            }

            Some(PathBuf::from(String::from_utf16_lossy(
                &buffer[..size as usize],
            )))
        }
    }

    fn enumerate_visible_windows(pids: &[u32]) -> Vec<ClientWindowInfo> {
        unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let context = unsafe { &mut *(lparam.0 as *mut WindowEnumerationContext) };
            let mut window_pid = 0u32;
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut window_pid));
            }

            if !context.pids.contains(&window_pid) {
                return BOOL(1);
            }

            let is_visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
            let is_minimized = unsafe { IsIconic(hwnd) }.as_bool();
            if !is_visible && !is_minimized {
                return BOOL(1);
            }

            let title = read_window_title(hwnd);
            context.windows.push(ClientWindowInfo {
                hwnd,
                pid: window_pid,
                display_name: display_name_from_window_title(&title),
                window_title: title,
            });

            BOOL(1)
        }

        if pids.is_empty() {
            return Vec::new();
        }

        let mut context = WindowEnumerationContext {
            pids: pids.iter().copied().collect(),
            windows: Vec::new(),
        };

        unsafe {
            let _ = EnumWindows(
                Some(enum_windows_proc),
                LPARAM(&mut context as *mut WindowEnumerationContext as isize),
            );
        }

        context.windows
    }
}

struct WindowEnumerationContext {
    pids: HashSet<u32>,
    windows: Vec<ClientWindowInfo>,
}

fn read_window_title(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }

        let mut buffer = vec![0u16; len as usize + 1];
        let read = GetWindowTextW(hwnd, &mut buffer);
        if read <= 0 {
            return String::new();
        }

        String::from_utf16_lossy(&buffer[..read as usize])
            .trim()
            .to_string()
    }
}

fn display_name_from_window_title(title: &str) -> String {
    let normalized = title.trim();
    if normalized.is_empty() {
        return TRAY_OFFLINE_NAME.to_string();
    }

    for segment in normalized
        .split(" - ")
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
    {
        let lowered_segment = segment.to_ascii_lowercase();
        if !is_generic_title(&lowered_segment) {
            return segment.to_string();
        }
    }

    let lowered = normalized.to_ascii_lowercase();
    if is_generic_title(&lowered) {
        TRAY_OFFLINE_NAME.to_string()
    } else {
        normalized.to_string()
    }
}

fn is_generic_title(lowered: &str) -> bool {
    let generic_markers = [
        "client",
        "launcher",
        "otclient",
        "ultima",
        "penultima",
        "tibia",
        "logged off",
    ];

    generic_markers
        .iter()
        .any(|marker| lowered == *marker || lowered.contains(&format!("{marker} ")))
}

fn normalized_path(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('/', "\\")
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
    use super::display_name_from_window_title;
    use crate::constants::TRAY_OFFLINE_NAME;

    #[test]
    fn falls_back_to_default_name_for_generic_titles() {
        assert_eq!(display_name_from_window_title("client"), TRAY_OFFLINE_NAME);
        assert_eq!(
            display_name_from_window_title("Penultima Launcher"),
            TRAY_OFFLINE_NAME
        );
    }

    #[test]
    fn keeps_character_name_titles() {
        assert_eq!(
            display_name_from_window_title("Knight Sample"),
            "Knight Sample"
        );
    }
}

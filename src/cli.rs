use clap::Parser;

/// Estrutura para os argumentos de linha de comando
#[derive(Parser, Debug)]
#[clap(name = "Penultima Launcher", about = "Launcher para Penultima Server")]
pub struct Args {
    /// Mostra o console do launcher
    #[clap(long, short = 'c')]
    pub console: bool,

    /// Ativa o auto-hide do launcher quando um cliente é iniciado
    #[clap(long, short = 'a')]
    pub auto_hide: bool,

    /// Runs the client updater once and exits.
    #[clap(long)]
    pub update_client_once: bool,

    /// Downloads and installs the full map once and exits.
    #[clap(long)]
    pub full_map_once: bool,

    /// Downloads/prepares the OTClient partner launcher once and exits.
    #[clap(long)]
    pub prepare_otclient_once: bool,

    /// Launches the selected 15.23 client once and exits.
    #[clap(long)]
    pub launch_client_once: bool,

    /// Launches N selected 15.23 clients once and exits.
    #[clap(long, default_value_t = 0)]
    pub launch_client_count: u8,

    /// Uses a separate single-instance lock for local smoke tests.
    #[clap(long)]
    pub instance_suffix: Option<String>,
}

impl Args {
    pub fn has_headless_task(&self) -> bool {
        self.update_client_once
            || self.full_map_once
            || self.prepare_otclient_once
            || self.launch_client_once
            || self.launch_client_count > 0
    }
}

/// Função para alocar e mostrar um novo console
pub fn show_console() {
    unsafe {
        use winapi::um::consoleapi::AllocConsole;
        AllocConsole();
    }
}

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
}

/// Função para alocar e mostrar um novo console
pub fn show_console() {
    unsafe {
        use winapi::um::consoleapi::AllocConsole;
        AllocConsole();
    }
}

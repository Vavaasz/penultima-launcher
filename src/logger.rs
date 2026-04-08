use crate::constants::*;
use log::{Level, LevelFilter, Metadata, Record, SetLoggerError};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

// Um logger personalizado que pode ser ativado ou desativado
pub struct AppLogger {
    // Flag atômica para controlar se o logging está ativo
    enabled: AtomicBool,
    // Nível mínimo de log a ser exibido (como um AtomicU8)
    level: AtomicU8,
}

// Função auxiliar para converter u8 para Level
fn u8_to_level(value: u8) -> Level {
    match value {
        1 => Level::Error,
        2 => Level::Warn,
        3 => Level::Info,
        4 => Level::Debug,
        5 => Level::Trace,
        _ => Level::Info, // Valor padrão em caso de número inválido
    }
}

// Função auxiliar para converter Level para u8
fn level_to_u8(level: Level) -> u8 {
    match level {
        Level::Error => 1,
        Level::Warn => 2,
        Level::Info => 3,
        Level::Debug => 4,
        Level::Trace => 5,
    }
}

// Implementação do singleton do logger
static LOGGER: AppLogger = AppLogger {
    enabled: AtomicBool::new(false),
    level: AtomicU8::new(3), // 3 corresponde a Level::Info
};

impl log::Log for AppLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Verifica se o logging está habilitado e se o nível é suficiente
        let current_level = u8_to_level(self.level.load(Ordering::Relaxed));

        self.enabled.load(Ordering::Relaxed) && metadata.level() <= current_level
    }

    fn log(&self, record: &Record) {
        // Só processa o log se estiver habilitado
        if self.enabled(&record.metadata()) {
            // Formata a mensagem com timestamp, nível e módulo
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            println!(
                "[{}] {} [{}] {}",
                timestamp,
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {
        // Sem buffer para flush nesta implementação simples
    }
}

/// Inicializa o logger com configurações específicas
pub fn init(enable: bool, level: Level) -> Result<(), SetLoggerError> {
    // Define o logger global
    log::set_logger(&LOGGER).map(|()| {
        // Define o nível máximo de log
        log::set_max_level(LevelFilter::Trace);
        // Define o estado inicial (ativado/desativado)
        LOGGER.enabled.store(enable, Ordering::Relaxed);
        LOGGER.level.store(level_to_u8(level), Ordering::Relaxed);
    })
}

/// Habilita ou desabilita o logging em tempo de execução
#[allow(dead_code)]
pub fn set_enabled(enable: bool) {
    LOGGER.enabled.store(enable, Ordering::Relaxed);
}

/// Verifica se o logging está habilitado
#[allow(dead_code)]
pub fn is_enabled() -> bool {
    LOGGER.enabled.load(Ordering::Relaxed)
}

/// Define o nível de log em tempo de execução
#[allow(dead_code)]
pub fn set_level(level: Level) {
    LOGGER.level.store(level_to_u8(level), Ordering::Relaxed);
}

/// Retorna o nível de log atual
#[allow(dead_code)]
pub fn get_level() -> Level {
    u8_to_level(LOGGER.level.load(Ordering::Relaxed))
}

/// Inicializa o logger baseado na presença de #[windows_subsystem = "windows"]
/// e/ou flag de console
#[allow(dead_code)]
pub fn initialize(force_console: bool) {
    // Detecção de ambiente GUI/console
    let is_gui_mode = std::env::var("RUST_LOG").is_err() && !force_console;

    // Define se o logging está habilitado
    let enable_logging = !is_gui_mode || force_console;

    // Inicializa o logger com as configurações determinadas
    if let Err(e) = init(enable_logging, Level::Info) {
        eprintln!("Erro ao inicializar o logger: {}", e);
    }

    if enable_logging {
        log::info!("Logger inicializado com sucesso!");
    }
}

/// Escreve um log no arquivo de log (pode ser usado mesmo quando o console está desabilitado)
#[allow(dead_code)]
pub fn log_to_file(level: Level, message: &str) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let app_dirs = crate::app_dirs::AppDirs::init().ok();
    if let Some(dirs) = app_dirs {
        let log_path = dirs.game_path.join(LOG_FILENAME);

        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let log_entry = format!("[{}] {} {}\n", timestamp, level, message);

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;

        file.write_all(log_entry.as_bytes())?;
    }

    Ok(())
}

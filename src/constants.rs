//! Arquivo centralizado para todas as configurações, URLs, IPs e constantes do projeto
//! Este arquivo facilita a manutenção e modificação de valores hardcoded

use std::time::Duration;

// ================================
// CONFIGURAÇÕES DE SERVIDOR
// ================================

/// IP do servidor de ping principal
pub const PING_SERVER_IP: &str = "145.223.94.22";

/// Porta do servidor de ping
pub const PING_SERVER_PORT: u16 = 7171;

/// IP do servidor de jogo (proxy)
pub const GAME_SERVER_IP: &str = "145.223.94.22";

/// Host web para login
pub const WEB_LOGIN_HOST: &str = "ultimaotserv.online";

/// Porta HTTPS para conexões seguras
pub const HTTPS_PORT: u16 = 8443;

// ================================
// URLS E ENDPOINTS
// ================================

/// URL base do feed publico do cliente hospedado no site
pub const CLIENT_GITHUB_RAW_BASE_URL: &str =
    "https://raw.githubusercontent.com/Vavaasz/penultima-client/main";

/// Host publico usado para resolver arquivos publicados no site
pub const WEBSITE_BASE_URL: &str = "https://ultimaotserv.online";

/// Public Discord invite shown by the website header
pub const DISCORD_URL: &str = "https://discord.gg/ytNUWGbjg6";

/// Website changelog page used by the launcher news tab
pub const CHANGELOG_URL: &str = "https://ultimaotserv.online/?changelog";
pub const LAUNCHER_ASSET_BASE_URL: &str = "https://ultimaotserv.online/downloads/launcher-assets";

/// Website event calendar page used by the launcher dashboard
pub const EVENT_CALENDAR_URL: &str = "https://ultimaotserv.online/?eventcalendar";

/// Website battle pass page used by the launcher dashboard
pub const BATTLE_PASS_URL: &str = "https://ultimaotserv.online/?bpass";

/// Website weekly package page used by the launcher dashboard
pub const PACK_WEEK_URL: &str = "https://ultimaotserv.online/?packweek";

/// Website investor page used by the launcher dashboard
pub const INVESTMENT_URL: &str = "https://ultimaotserv.online/?investment";

/// Metadata publica dos downloads publicados no site
pub const DOWNLOADS_METADATA_URL: &str =
    "https://ultimaotserv.online/downloads/penultima-downloads.json";

/// Arquivo ZIP do feed completo do cliente para bootstrap rapido
pub const CLIENT_GITHUB_ARCHIVE_URL: &str =
    "https://codeload.github.com/Vavaasz/penultima-client/zip/refs/heads/main";

/// Optional full minimap package installed into the selected client folder.
pub const FULL_MINIMAP_ARCHIVE_URL: &str =
    "https://ultimaotserv.online/downloads/Penultima-Full-Minimap.zip";

/// Local test override for the full minimap package URL.
pub const FULL_MINIMAP_URL_ENV: &str = "PENULTIMA_FULL_MINIMAP_URL";

/// Slug do Partner Launcher OTCRP usado pelo Penultima
pub const OTCRP_PARTNER_SLUG: &str = "penultima";

/// Pagina de download do Partner Launcher OTCRP do Penultima
pub const OTCRP_LAUNCHER_DOWNLOAD_PAGE_URL: &str = "https://www.otcrp.com/downloads/penultima";

/// ZIP do Partner Launcher OTCRP do Penultima
pub const OTCRP_PARTNER_LAUNCHER_ZIP_URL: &str =
    "https://www.otcrp.com/downloads/penultima/launcher.zip";

/// Manifesto principal do cliente
pub const CLIENT_PACKAGE_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/Vavaasz/penultima-client/main/package.json";

/// Arquivo com a versão publicada do cliente
pub const CLIENT_PACKAGE_VERSION_URL: &str =
    "https://raw.githubusercontent.com/Vavaasz/penultima-client/main/package.json.version";

/// Manifesto de assets do cliente
pub const CLIENT_ASSET_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/Vavaasz/penultima-client/main/assets.json";

/// Hash do manifesto de assets
pub const CLIENT_ASSET_MANIFEST_HASH_URL: &str =
    "https://raw.githubusercontent.com/Vavaasz/penultima-client/main/assets.json.sha256";

/// URLs predefinidas para servidores
pub const PREDEFINED_SERVERS: [&str; 3] = [
    "https://ultimaotserv.online/login.php",
    "http://ultimaotserv.online/login.php",
    "http://www.ultimaotserv.online/login.php",
];

/// URLs individuais para compatibilidade
pub const PREDEFINED_LOGIN_URL_HTTPS: &str = "https://ultimaotserv.online/login.php";
pub const PREDEFINED_LOGIN_URL_HTTP_8080: &str = "http://ultimaotserv.online/login.php";
pub const PREDEFINED_LOGIN_URL_HTTP: &str = "http://www.ultimaotserv.online/login.php";

/// URL de exemplo para hint text
pub const EXAMPLE_SERVER_URL: &str = "https://ultimaotserv.online/login.php";

// ================================
// CONFIGURAÇÕES DO LAUNCHER
// ================================

/// Nome do aplicativo
pub const APP_NAME: &str = "Penultima Launcher";

/// Nome do processo/instância
pub const INSTANCE_NAME: &str = "penultima-launcher";

/// Nome do arquivo de log
pub const LOG_FILENAME: &str = "launcher.log";

/// Nome do diretório base no AppData
pub const APP_DATA_DIR: &str = "Penultima Launcher";

/// Nome do diretório home no Linux/Mac
pub const HOME_DIR: &str = ".penultima-launcher";
pub const REQUIRED_CLIENT_RUNTIME_FILES: &[&str] = &[
    r"bin\client.exe",
    r"bin\Qt6Core.dll",
    r"bin\Qt6WebEngineCore.dll",
    r"bin\QtWebEngineProcess.exe",
    r"bin\qt.conf",
];
pub const TRAY_OFFLINE_NAME: &str = "Penultima Server";

// ================================
// CONFIGURAÇÕES DE PROXY
// ================================

/// Porta padrão para login do proxy
pub const DEFAULT_LOGIN_PORT: u16 = 7171;

/// Porta padrão para jogo do proxy
pub const DEFAULT_GAME_PORT: u16 = 7172;

/// Porta padrão HTTP do proxy
pub const DEFAULT_HTTP_PORT: u16 = 80;

/// Porta padrão HTTPS do proxy
pub const DEFAULT_HTTPS_PORT: u16 = 443;

/// IP local para bind do proxy
pub const LOCALHOST_IP: &str = "127.0.0.1";

// ================================
// TIMEOUTS E DURAÇÕES
// ================================

/// Timeout para conexões do proxy
pub const PROXY_TIMEOUT: Duration = Duration::from_secs(5);

/// Timeout para atividade do proxy
pub const PROXY_ACTIVITY_TIMEOUT: Duration = Duration::from_secs(15);

/// Timeout para leitura do proxy
pub const PROXY_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Intervalo entre verificações de ping (30 segundos)
pub const PING_CHECK_INTERVAL: Duration = Duration::from_secs(30);
pub const PING_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Timeout para requisições HTTP
pub const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const HTTP_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub const WEBSITE_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub const STARTUP_SPLASH_DURATION: Duration = Duration::from_millis(1200);

/// Intervalo para verificação de instâncias
pub const INSTANCE_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// Timeout para verificação de status de proxy
pub const PROXY_STATUS_TIMEOUT: Duration = Duration::from_millis(500);

/// Intervalo para atualização de status de proxy
pub const PROXY_STATUS_UPDATE_INTERVAL: Duration = Duration::from_secs(60);

/// Intervalo de repaint quando a janela está escondida no tray sem interação pendente
pub const HIDDEN_REPAINT_INTERVAL: Duration = Duration::from_secs(2);

/// Intervalo de polling enquanto há ícones ativos na system tray
pub const TRAY_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Intervalo de repaint quando a janela está visível mas ociosa
pub const IDLE_REPAINT_INTERVAL: Duration = Duration::from_millis(33);

/// Duração mínima entre exibições de mensagens
pub const MESSAGE_DISPLAY_INTERVAL: Duration = Duration::from_secs(2);

/// Timeout para operações de atualização
pub const UPDATE_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Intervalo de sleep para threads
pub const THREAD_SLEEP_INTERVAL: Duration = Duration::from_secs(1);

/// Atraso de inicialização
pub const STARTUP_DELAY: Duration = Duration::from_millis(500);

/// Timeout para verificação de serviços
pub const SERVICE_CHECK_TIMEOUT: Duration = Duration::from_millis(500);

/// Duração de exibição de mensagens na UI
pub const UI_MESSAGE_DISPLAY_DURATION: Duration = Duration::from_secs(2);

// ================================
// CONFIGURAÇÕES DE UI
// ================================

/// Tamanho desejado da janela principal
pub const MAIN_WINDOW_SIZE: (f32, f32) = (1240.0, 800.0);
pub const MIN_WINDOW_SIZE: (f32, f32) = (420.0, 520.0);

/// Tamanho da janela (alias para compatibilidade)
pub const WINDOW_SIZE: (f32, f32) = MAIN_WINDOW_SIZE;

/// Tamanho do logo redimensionado
pub const LOGO_SIZE: (f32, f32) = (180.0, 126.0);

/// Altura do rodapé
pub const FOOTER_HEIGHT: f32 = 35.0;

/// Largura padrão dos botões
pub const BUTTON_WIDTH: f32 = 200.0;

/// Altura padrão dos botões
pub const BUTTON_HEIGHT: f32 = 40.0;

/// Altura do indicador de carregamento
pub const LOADING_INDICATOR_HEIGHT: f32 = 45.0;

/// Tamanhos de fonte
pub const FONT_SIZE_HEADING: f32 = 30.0;
pub const FONT_SIZE_BODY: f32 = 18.0;
pub const FONT_SIZE_BUTTON: f32 = 18.0;
pub const FONT_SIZE_SMALL: f32 = 14.0;

/// Raio do indicador de carregamento
pub const LOADING_INDICATOR_RADIUS: f32 = 30.0;

/// Número de pontos no indicador de carregamento
pub const LOADING_INDICATOR_POINTS: usize = 10;

/// Tamanho dos pontos do indicador
pub const LOADING_POINT_SIZE: f32 = 3.5;

/// Tamanho do círculo de status
pub const STATUS_CIRCLE_SIZE: f32 = 8.0;

// ================================
// CONFIGURAÇÕES DE PING
// ================================

/// Limite para ping excelente (verde)
pub const PING_EXCELLENT_THRESHOLD: u32 = 50;

/// Limite para ping bom (amarelo)
pub const PING_GOOD_THRESHOLD: u32 = 100;

// Acima deste valor será considerado ping ruim (vermelho)

// ================================
// CONFIGURAÇÕES DE SISTEMA
// ================================

/// Classe de alta prioridade do Windows
pub const HIGH_PRIORITY_CLASS: u32 = 0x00000080;

/// Tamanho do buffer para operações de rede
pub const NETWORK_BUFFER_SIZE: usize = 4096;

/// Tamanho do protocolo de ping (2 bytes para 255,255)
pub const PING_PROTOCOL_SIZE: usize = 2;

/// Área de busca para versão do cliente
pub const VERSION_SEARCH_AREA_SIZE: usize = 200;

// ================================
// CONFIGURAÇÕES DE CACHE
// ================================

/// Progresso inicial para limpeza de cache
pub const CACHE_CLEANUP_INITIAL_PROGRESS: f32 = 0.2;

/// Progresso intermediário para limpeza de cache
pub const CACHE_CLEANUP_INTERMEDIATE_PROGRESS: f32 = 0.5;

/// Progresso final para limpeza de cache
pub const CACHE_CLEANUP_FINAL_PROGRESS: f32 = 1.0;

/// Divisor para conversão de bytes para MB
pub const BYTES_TO_MB_DIVISOR: f64 = 1024.0 * 1024.0;

// ================================
// CONFIGURAÇÕES DE RENDERIZAÇÃO
// ================================

/// Cor principal do layout
pub const ACCENT_PRIMARY_RGB: (u8, u8, u8) = (234, 182, 76);

/// Cor secundária do layout
pub const ACCENT_SECONDARY_RGB: (u8, u8, u8) = (110, 146, 255);

/// Cor escura do layout
pub const SURFACE_RGB: (u8, u8, u8) = (12, 16, 26);

// ================================
// REGEX PATTERNS
// ================================

/// Pattern para busca de versão em arquivos
pub const VERSION_REGEX_PATTERN: &str = r"\b(\d{1,2}\.\d{1,2})\b";

// ================================
// FUNÇÕES AUXILIARES
// ================================

/// Retorna o tamanho do buffer baseado na porta
pub fn get_buffer_size(port: u16) -> usize {
    match port {
        443 => NETWORK_BUFFER_SIZE,
        _ => NETWORK_BUFFER_SIZE,
    }
}

/// Retorna a URL completa do servidor de ping
pub fn get_ping_server_address() -> String {
    format!("{}:{}", PING_SERVER_IP, PING_SERVER_PORT)
}

/// Retorna o endereço de bind para uma porta específica
pub fn get_bind_address(port: u16) -> String {
    format!("{}:{}", LOCALHOST_IP, port)
}

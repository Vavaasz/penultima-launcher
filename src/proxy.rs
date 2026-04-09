use crate::constants::*;
use anyhow::Result;
use log::{error, info, warn};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};
use tokio_native_tls::TlsConnector as TokioTlsConnector;

// Constantes de configuração
const TIMEOUT_DURATION: Duration = PROXY_TIMEOUT;
const ACTIVITY_TIMEOUT: Duration = PROXY_ACTIVITY_TIMEOUT;
const READ_TIMEOUT: Duration = PROXY_READ_TIMEOUT;

/// Configuração do proxy com portas e hosts de destino.
pub struct ProxyConfig {
    pub login_port: u16,   // Porta do servidor de login (ex: 7171)
    pub game_port: u16,    // Porta do servidor de jogo (ex: 7172)
    pub http_port: u16,    // Porta HTTP (ex: 80)
    pub https_port: u16,   // Porta HTTPS (ex: 443)
    pub game_host: String, // IP ou hostname do servidor de jogo
    pub web_host: String,  // Hostname do servidor web
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            login_port: DEFAULT_LOGIN_PORT,
            game_port: DEFAULT_GAME_PORT,
            http_port: DEFAULT_HTTP_PORT,
            https_port: DEFAULT_HTTPS_PORT,
            game_host: GAME_SERVER_IP.to_string(),
            web_host: WEB_LOGIN_HOST.to_string(),
        }
    }
}

/// Estrutura para rastrear estatísticas de conexão.
struct ConnectionStats {
    bytes_received: usize,
    bytes_sent: usize,
    start_time: std::time::Instant,
}

impl ConnectionStats {
    /// Cria uma nova instância de ConnectionStats.
    fn new() -> Self {
        Self {
            bytes_received: 0,
            bytes_sent: 0,
            start_time: std::time::Instant::now(),
        }
    }

    /// Registra as estatísticas da conexão no log.
    fn log_stats(&self, client_addr: SocketAddr, target_host: &str, target_port: u16) {
        let duration = self.start_time.elapsed();
        info!(
            "[PROXY] Estatísticas da conexão - Cliente: {}, Servidor: {}:{} - Duração: {:.2}s, Bytes Recebidos: {}, Bytes Enviados: {}, Taxa: {:.2} KB/s",
            client_addr,
            target_host,
            target_port,
            duration.as_secs_f64(),
            self.bytes_received,
            self.bytes_sent,
            (self.bytes_received + self.bytes_sent) as f64 / 1024.0 / duration.as_secs_f64()
        );
    }
}

/// Determina o tamanho do buffer com base na porta de destino.
fn get_buffer_size(port: u16) -> usize {
    match port {
        7171 | 7172 => 65535, // Buffer pequeno para jogos (baixa latência)
        80 | 443 => 4096,     // Buffer maior para HTTP/HTTPS (maior throughput)
        _ => 4096,            // Tamanho padrão para outras portas
    }
}

/// Manipula uma conexão genérica, redirecionando para TCP ou HTTPS conforme a porta.
async fn handle_connection(
    client_stream: TcpStream,
    target_host: String,
    target_port: u16,
) -> Result<()> {
    // Configurar o client_stream com parâmetros de socket
    let client_stream = configure_tcp_stream(client_stream).await?;

    // Obter o endereço do cliente
    let client_addr = client_stream.peer_addr()?;

    // Estrutura para rastrear estatísticas da conexão
    let mut stats = ConnectionStats::new();

    // Verificar a porta para decidir o tipo de conexão
    if target_port == 80 {
        handle_https_connection(client_stream, &target_host, client_addr, &mut stats).await?;
    } else {
        handle_tcp_connection(
            client_stream,
            &target_host,
            target_port,
            client_addr,
            &mut stats,
        )
        .await?;
    }

    // Registrar as estatísticas da conexão
    stats.log_stats(client_addr, &target_host, target_port);
    Ok(())
}

#[allow(dead_code)]
async fn configure_tcp_stream(stream: TcpStream) -> Result<TcpStream> {
    // Ativar TCP_NODELAY para evitar atrasos no envio de pequenos pacotes
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// Manipula conexões HTTPS com suporte a TLS.
#[allow(dead_code)]
async fn handle_https_connection(
    mut client_stream: TcpStream,
    target_host: &str,
    client_addr: SocketAddr,
    stats: &mut ConnectionStats,
) -> Result<()> {
    info!("[PROXY] Estabelecendo conexão HTTPS com {}", target_host);

    // Configurar o client_stream com timeout
    client_stream = configure_tcp_stream(client_stream).await?;

    // Conectar-se ao servidor com timeout
    let server_stream = timeout(
        TIMEOUT_DURATION,
        TcpStream::connect(format!("{}:8443", target_host)),
    )
    .await??;
    let server_stream = configure_tcp_stream(server_stream).await?;

    // Usar um TlsConnector com configuração segura
    let mut builder = native_tls::TlsConnector::builder();
    builder
        .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
        .max_protocol_version(None); // Permite TLS 1.3 se disponível

    let native_connector = builder.build()?;
    let connector = TokioTlsConnector::from(native_connector);

    // Estabelecer conexão TLS com timeout
    let mut server_stream = timeout(
        TIMEOUT_DURATION,
        connector.connect(target_host, server_stream),
    )
    .await??;

    info!("[PROXY] Conexão HTTPS estabelecida para {}", client_addr);

    // Buffer para a requisição
    let mut buffer = vec![0u8; get_buffer_size(443)];

    // 1. Ler a requisição do cliente
    let n = match timeout(TIMEOUT_DURATION, client_stream.read(&mut buffer)).await {
        Ok(Ok(0)) => {
            // info!("[PROXY] Cliente {} não enviou dados", client_addr);
            return Ok(());
        }
        Ok(Ok(n)) => n,
        Ok(Err(_e)) => {
            // warn!("[PROXY] Erro ao ler do cliente {}: {}", client_addr, e);
            return Ok(());
        }
        Err(_e) => {
            // warn!("[PROXY] Erro ao ler do cliente {}: {}", client_addr, e);
            return Ok(());
        }
    };

    // 2. Processar e enviar a requisição para o servidor
    if let Ok(request) = String::from_utf8(buffer[..n].to_vec()) {
        let modified_request = modify_http_request(&request, target_host);

        if let Err(_e) = timeout(
            TIMEOUT_DURATION,
            server_stream.write_all(modified_request.as_bytes()),
        )
        .await
        {
            // debug!("[PROXY] Erro ao enviar para o servidor {}: {}", client_addr, e);
            return Ok(());
        }
        stats.bytes_sent += modified_request.len();

        // 3. Ler e enviar a resposta do servidor para o cliente com timeout
        loop {
            match timeout(READ_TIMEOUT, server_stream.read(&mut buffer)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    if let Err(_e) = client_stream.write_all(&buffer[..n]).await {
                        // warn!("[PROXY] Erro ao enviar resposta para {}: {}", client_addr, e);
                        break;
                    }
                    stats.bytes_received += n;
                }
                Ok(Err(_e)) => {
                    // warn!("[PROXY] Erro ao ler resposta do servidor para {}: {}", client_addr, e);
                    break;
                }
                Err(_e) => {
                    // warn!("[PROXY] Erro ao ler resposta do servidor para {}: {}", client_addr, e);
                    break;
                }
            }
        }
    }

    // 4. Fechar as conexões imediatamente após enviar a resposta
    let _ = client_stream.shutdown().await;
    let _ = server_stream.shutdown().await;

    info!(
        "[PROXY] Conexão finalizada - Cliente: {}, Servidor: {}:{} (Enviados: {} bytes, Recebidos: {} bytes)",
        client_addr, target_host, 443, stats.bytes_sent, stats.bytes_received
    );

    Ok(())
}

#[allow(dead_code)]
async fn handle_tcp_connection(
    client_stream: TcpStream,
    target_host: &str,
    target_port: u16,
    client_addr: SocketAddr,
    stats: &mut ConnectionStats,
) -> Result<()> {
    // Configurar o client_stream
    let mut client_stream = configure_tcp_stream(client_stream).await?;

    // Conectar-se ao servidor e configurar o server_stream
    let server_stream = TcpStream::connect(format!("{}:{}", target_host, target_port)).await?;
    let mut server_stream = configure_tcp_stream(server_stream).await?;

    // Buffers dinâmicos para cliente e servidor
    let buffer_size = get_buffer_size(target_port);
    let mut client_buffer = vec![0u8; buffer_size];
    let mut server_buffer = vec![0u8; buffer_size];

    let mut last_activity = std::time::Instant::now();

    // Loop de transferência de dados
    loop {
        tokio::select! {
            result = client_stream.read(&mut client_buffer) => {
                match result {
                    Ok(0) => break, // Conexão fechada pelo cliente
                    Ok(n) => {
                        last_activity = std::time::Instant::now();
                        match server_stream.write_all(&client_buffer[..n]).await {
                            Ok(_) => {
                                if let Err(e) = server_stream.flush().await {
                                    warn!("[PROXY] Erro ao fazer flush dos dados para o servidor: {}", e);
                                    break;
                                }
                                stats.bytes_sent += n;
                            }
                            Err(e) => {
                                warn!("[PROXY] Erro ao enviar dados para o servidor: {}", e);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("[PROXY] Erro ao ler dados do cliente: {}", e);
                        break;
                    }
                }
            }
            result = server_stream.read(&mut server_buffer) => {
                match result {
                    Ok(0) => break, // Conexão fechada pelo servidor
                    Ok(n) => {
                        last_activity = std::time::Instant::now();
                        match client_stream.write_all(&server_buffer[..n]).await {
                            Ok(_) => {
                                if let Err(e) = client_stream.flush().await {
                                    warn!("[PROXY] Erro ao fazer flush dos dados para o cliente: {}", e);
                                    break;
                                }
                                stats.bytes_received += n;
                            }
                            Err(e) => {
                                warn!("[PROXY] Erro ao enviar dados para o cliente: {}", e);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("[PROXY] Erro ao ler dados do servidor: {}", e);
                        break;
                    }
                }
            }
            _ = sleep(Duration::from_secs(1)) => {
                if last_activity.elapsed() > ACTIVITY_TIMEOUT {
                    warn!("[PROXY] Timeout por inatividade");
                    break;
                }
            }
        }
    }

    info!(
        "[PROXY] Conexão TCP encerrada - Cliente: {}, Servidor: {}:{}",
        client_addr, target_host, target_port
    );
    Ok(())
}

/// Modifica requisições HTTP para redirecionar corretamente.
fn modify_http_request(request: &str, target_host: &str) -> String {
    request
        .lines()
        .map(|line| {
            if line.starts_with("POST ") {
                "POST /login HTTP/1.1".to_string()
            } else if line.starts_with("Host:") {
                format!("Host: {}", target_host)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n\r\n"
}

/// Executa o proxy, escutando em todas as portas configuradas.
#[allow(dead_code)]
pub async fn run_proxy(config: Arc<ProxyConfig>) -> Result<()> {
    // Configura os listeners para cada porta
    let login_listener = TcpListener::bind(format!("127.0.0.1:{}", config.login_port)).await?;
    let game_listener = TcpListener::bind(format!("127.0.0.1:{}", config.game_port)).await?;
    let http_listener = TcpListener::bind(format!("127.0.0.1:{}", config.http_port)).await?;
    let https_listener = TcpListener::bind(format!("127.0.0.1:{}", config.https_port)).await?;

    // Logs iniciais
    info!("Login server: {}:{}", config.game_host, config.login_port);
    info!("Game server: {}:{}", config.game_host, config.game_port);
    info!("HTTP server: {}:{}", config.web_host, config.http_port);
    info!("HTTPS server: {}:{}", config.web_host, config.https_port);

    // Loop principal para aceitar conexões
    loop {
        tokio::select! {
            Ok((stream, _)) = login_listener.accept() => {
                let config = config.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, config.game_host.clone(), config.login_port).await {
                        error!("[PROXY] Erro na conexão de login: {}", e);
                    }
                });
            }
            Ok((stream, _)) = game_listener.accept() => {
                let config = config.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, config.game_host.clone(), config.game_port).await {
                        error!("[PROXY] Erro na conexão do jogo: {}", e);
                    }
                });
            }
            Ok((stream, _)) = http_listener.accept() => {
                let config = config.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, config.web_host.clone(), config.http_port).await {
                        error!("[PROXY] Erro na conexão HTTP: {}", e);
                    }
                });
            }
            Ok((stream, _)) = https_listener.accept() => {
                let config = config.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, config.web_host.clone(), config.https_port).await {
                        error!("[PROXY] Erro na conexão HTTPS: {}", e);
                    }
                });
            }
        }
    }
}

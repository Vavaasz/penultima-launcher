use crate::constants::*;
use crate::proxy::ProxyConfig;
use std::net::TcpStream;
use std::time::{Duration, Instant};

// Estrutura para rastrear o status de cada serviço do proxy
#[derive(Debug, Clone)]
pub struct ProxyStatus {
    pub login_running: bool,
    pub game_running: bool,
    pub http_running: bool,
    pub https_running: bool,
    pub last_check: Instant,
}

impl Default for ProxyStatus {
    fn default() -> Self {
        Self {
            login_running: false,
            game_running: false,
            http_running: false,
            https_running: false,
            last_check: Instant::now(),
        }
    }
}

#[allow(dead_code)]
impl ProxyStatus {
    pub fn new() -> Self {
        Self::default()
    }

    // Método para verificar se um serviço está rodando em determinada porta
    pub fn check_service_status(host: &str, port: u16) -> bool {
        // Definindo um timeout para não travar a UI
        let socket_addr = match format!("{}:{}", host, port).parse::<std::net::SocketAddr>() {
            Ok(addr) => addr,
            Err(_) => return false,
        };

        // Usar um timeout curto para não travar a UI
        let timeout = SERVICE_CHECK_TIMEOUT;
        let start = Instant::now();

        match TcpStream::connect_timeout(&socket_addr, timeout) {
            Ok(_) => {
                // Serviço respondeu com sucesso
                true
            }
            Err(_) => {
                // Verificar se o timeout foi excedido
                if start.elapsed() >= timeout {
                    // Timeout excedido, consideramos que o serviço não está rodando
                    false
                } else {
                    // Erro de conexão, serviço não está rodando
                    false
                }
            }
        }
    }

    // Método para atualizar o status de todos os serviços do proxy
    pub fn update_status(&mut self, config: &ProxyConfig) {
        // Para o login e game, verificamos a conexão com o servidor remoto
        self.login_running = Self::check_service_status(&config.game_host, config.login_port);
        self.game_running = Self::check_service_status(&config.game_host, config.game_port);

        // Para HTTP e HTTPS, verificamos o proxy local (127.0.0.1)
        self.http_running = Self::check_service_status("127.0.0.1", config.http_port);
        self.https_running = Self::check_service_status("127.0.0.1", config.https_port);

        // Atualizar o timestamp
        self.last_check = Instant::now();
    }

    // Verifica se é hora de atualizar o status novamente (a cada 60 segundos)
    pub fn should_update(&self) -> bool {
        self.last_check.elapsed() >= Duration::from_secs(60)
    }

    // Retorna o número total de serviços ativos
    pub fn active_services_count(&self) -> u8 {
        let mut count = 0;

        if self.login_running {
            count += 1;
        }
        if self.game_running {
            count += 1;
        }
        if self.http_running {
            count += 1;
        }
        if self.https_running {
            count += 1;
        }

        count
    }

    // Renderiza os indicadores de status
    pub fn render_status_indicators(&self, ui: &mut egui::Ui) {
        // Função para desenhar um círculo colorido
        let draw_status_circle = |ui: &mut egui::Ui, is_running: bool, label: &str| {
            let circle_color = if is_running {
                egui::Color32::from_rgb(0, 180, 0) // Verde
            } else {
                egui::Color32::from_rgb(180, 0, 0) // Vermelho
            };

            ui.horizontal(|ui| {
                // Reservando espaço para o círculo
                let circle_size = 8.0;
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(circle_size, circle_size),
                    egui::Sense::hover(),
                );

                // Desenhando o círculo
                ui.painter()
                    .circle_filled(rect.center(), circle_size / 2.0, circle_color);

                ui.add_space(4.0); // Espaço entre o círculo e o texto
                ui.label(
                    egui::RichText::new(label)
                        .color(egui::Color32::from_rgb(160, 160, 160))
                        .size(12.0),
                );
            });
            ui.add_space(5.0);
        };

        // Mostrar status Login
        draw_status_circle(ui, self.login_running, "Login");

        // Mostrar status Game
        draw_status_circle(ui, self.game_running, "Game");

        // Mostrar status HTTP
        draw_status_circle(ui, self.http_running, "HTTP");

        // Mostrar status HTTPS
        draw_status_circle(ui, self.https_running, "HTTPS");
    }
}

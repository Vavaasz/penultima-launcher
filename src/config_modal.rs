use crate::constants::*;
use anyhow::{Context, Result};
use eframe::egui;
use log::info;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// Opções predefinidas de servidores
const SERVIDORES_PREDEFINIDOS: [&str; 3] = [
    PREDEFINED_LOGIN_URL_HTTPS,
    PREDEFINED_LOGIN_URL_HTTP_8080,
    PREDEFINED_LOGIN_URL_HTTP,
];
const DEFAULT_CONFIG_INI: &str = include_str!("../assets/default-config.ini");

// Estrutura para representar as configurações do jogo
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GameConfig {
    pub baixa_latencia: bool,
    pub otimizacoes_graficas: bool,
    pub servidor_selecionado: usize, // Índice do servidor predefinido (0, 1, 2)
    pub servidor_personalizado: String, // Valor personalizado se servidor_selecionado == 3
    pub usar_servidor_personalizado: bool, // Se verdadeiro, usa o valor personalizado
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            baixa_latencia: false,
            otimizacoes_graficas: false,
            servidor_selecionado: 0,
            servidor_personalizado: String::new(),
            usar_servidor_personalizado: false,
        }
    }
}

// Estrutura para gerenciar o modal de configuração
pub struct ConfigModal {
    // Estado do modal
    pub show_modal: bool,
    // Configurações atuais
    config: Arc<Mutex<GameConfig>>,
    // Caminho para o arquivo de configuração
    config_path: PathBuf,
    // Estado temporário para edição
    temp_config: GameConfig,
}

impl ConfigModal {
    fn direct_config_path(game_path: &Path) -> PathBuf {
        game_path.join("conf").join("config.ini")
    }

    fn legacy_config_path(game_path: &Path) -> PathBuf {
        game_path.join("ArcadiaOT").join("conf").join("config.ini")
    }

    fn resolve_config_path(game_path: &Path) -> PathBuf {
        let direct_config = Self::direct_config_path(game_path);
        if direct_config.exists() {
            return direct_config;
        }

        let legacy_config = Self::legacy_config_path(game_path);
        if legacy_config.exists() {
            return legacy_config;
        }

        direct_config
    }

    pub fn ensure_default_config(game_path: &PathBuf) -> Result<PathBuf> {
        let config_path = Self::resolve_config_path(game_path);
        if config_path.exists() {
            return Ok(config_path);
        }

        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent).context("Falha ao criar diretório de configuração")?;
        }

        fs::write(&config_path, DEFAULT_CONFIG_INI)
            .context("Falha ao gravar config.ini padrão")?;
        Ok(config_path)
    }

    // Cria uma nova instância do ConfigModal
    pub fn new(game_path: PathBuf) -> Self {
        let config_path = Self::ensure_default_config(&game_path)
            .unwrap_or_else(|_| Self::resolve_config_path(&game_path));

        info!("Caminho do arquivo de configuração: {:?}", config_path);

        // Carregar configuração existente ou criar padrão
        let config = match Self::load_config(&config_path) {
            Ok(config) => {
                info!("Configuração carregada com sucesso");
                config
            }
            Err(e) => {
                info!("Erro ao carregar configuração, usando padrão: {}", e);
                GameConfig::default()
            }
        };

        let temp_config = config.clone();

        Self {
            show_modal: false,
            config: Arc::new(Mutex::new(config)),
            config_path,
            temp_config,
        }
    }

    // Carrega a configuração do arquivo
    fn load_config(config_path: &PathBuf) -> Result<GameConfig> {
        if config_path.exists() {
            let content =
                fs::read_to_string(config_path).context("Falha ao ler arquivo de configuração")?;

            // Parse do arquivo .ini
            let mut config = GameConfig::default();
            let mut current_section = String::new();

            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with(';') {
                    continue;
                }

                // Verificar se é uma seção
                if line.starts_with('[') && line.ends_with(']') {
                    current_section = line[1..line.len() - 1].to_string();
                    continue;
                }

                // Processar apenas configurações da seção URLS e GRAPHICS
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();

                    match (current_section.as_str(), key) {
                        ("URLS", "loginWebService") | ("URLS", "clientWebService") => {
                            // Verificar se o valor corresponde a um dos servidores predefinidos
                            if let Some(index) =
                                SERVIDORES_PREDEFINIDOS.iter().position(|&s| s == value)
                            {
                                config.servidor_selecionado = index;
                                config.usar_servidor_personalizado = false;
                            } else {
                                // Se não corresponder a nenhum predefinido, definir como personalizado
                                config.servidor_personalizado = value.to_string();
                                config.usar_servidor_personalizado = true;
                            }
                        }
                        ("GRAPHICS", "renderLoopType") => {
                            // Definir otimizações com base no renderLoopType
                            config.otimizacoes_graficas = value != "basic";
                        }
                        ("MOVEMENT", "minPercentageOfCurrentMovementBeforeSendNext") => {
                            // Se < 1.0, está em modo de baixa latência
                            if let Ok(val) = value.parse::<f32>() {
                                config.baixa_latencia = val < 1.0;
                            }
                        }
                        _ => {}
                    }
                }
            }

            Ok(config)
        } else {
            // Se o arquivo não existe, retorna configuração padrão
            info!("Arquivo de configuração não existe. Usando valores padrão.");
            Ok(GameConfig::default())
        }
    }

    // Salva a configuração no arquivo
    fn save_config(&self) -> Result<()> {
        if !self.config_path.exists() {
            if let Some(parent) = self.config_path.parent() {
                fs::create_dir_all(parent)
                    .context("Falha ao criar diretório da configuração do cliente")?;
            }
            fs::write(&self.config_path, DEFAULT_CONFIG_INI)
                .context("Falha ao recriar config.ini padrão")?;
        }

        // Ler o conteúdo existente
        let content = fs::read_to_string(&self.config_path)
            .context("Falha ao ler arquivo de configuração existente")?;

        let config = self.config.lock().unwrap();

        // Obter o valor do servidor a ser usado
        let server_url = if config.usar_servidor_personalizado {
            &config.servidor_personalizado
        } else if config.servidor_selecionado < SERVIDORES_PREDEFINIDOS.len() {
            SERVIDORES_PREDEFINIDOS[config.servidor_selecionado]
        } else {
            SERVIDORES_PREDEFINIDOS[0] // Fallback para o primeiro servidor
        };

        // Substituir as linhas relevantes
        let mut new_content = String::new();
        let mut current_section = String::new();

        for line in content.lines() {
            let trimmed = line.trim();

            // Detectar seção
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                current_section = trimmed[1..trimmed.len() - 1].to_string();
                new_content.push_str(line);
                new_content.push('\n');
                continue;
            }

            // Processar linhas conforme a seção
            if let Some((key, _)) = trimmed.split_once('=') {
                let key = key.trim();

                match (current_section.as_str(), key) {
                    ("URLS", "loginWebService") => {
                        new_content.push_str(&format!("{}={}", key, server_url));
                        new_content.push('\n');
                    }
                    ("URLS", "clientWebService") => {
                        new_content.push_str(&format!("{}={}", key, server_url));
                        new_content.push('\n');
                    }
                    ("GRAPHICS", "renderLoopType") => {
                        let render_type = if config.otimizacoes_graficas {
                            "optimized"
                        } else {
                            "basic"
                        };
                        new_content.push_str(&format!("{}={}", key, render_type));
                        new_content.push('\n');
                    }
                    ("MOVEMENT", "minPercentageOfCurrentMovementBeforeSendNext") => {
                        let value = if config.baixa_latencia { "0.7" } else { "1.0" };
                        new_content.push_str(&format!("{}={}", key, value));
                        new_content.push('\n');
                    }
                    _ => {
                        // Manter outras linhas inalteradas
                        new_content.push_str(line);
                        new_content.push('\n');
                    }
                }
            } else {
                // Manter linhas sem '=' inalteradas (comentários, linhas em branco, etc.)
                new_content.push_str(line);
                new_content.push('\n');
            }
        }

        // Escrever de volta ao arquivo
        fs::write(&self.config_path, new_content.trim())
            .context("Falha ao salvar arquivo de configuração")?;

        info!("Configuração salva com sucesso em {:?}", self.config_path);

        Ok(())
    }

    // Verifica se a tecla de atalho foi pressionada
    pub fn check_hotkey(&mut self, ctx: &egui::Context) {
        // Shift+F2 como tecla de atalho para abrir/fechar o modal
        if ctx.input(|i| i.key_pressed(egui::Key::F2) && i.modifiers.shift) {
            // Alternar a visibilidade do modal (toggle)
            self.show_modal = !self.show_modal;

            // Se o modal estiver abrindo, inicializar o estado temporário
            if self.show_modal {
                self.temp_config = self.config.lock().unwrap().clone();
            }

            ctx.request_repaint();
        }

        // Fechar o modal quando ESC é pressionado, se estiver aberto
        if self.show_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_modal = false;
            ctx.request_repaint();
        }
    }

    // Renderiza o modal de configuração
    pub fn render(&mut self, ctx: &egui::Context) {
        if !self.show_modal {
            return;
        }

        egui::Window::new("Configurações do Cliente")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([400.0, 300.0])
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 250)),
            )
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(5.0);
                    ui.label(
                        egui::RichText::new("Configurações do Cliente")
                            .size(16.0)
                            .color(egui::Color32::from_rgb(220, 220, 220))
                            .strong(),
                    );

                    ui.add_space(15.0);

                    // Seção de Otimizações
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new("Otimizações")
                                .size(14.0)
                                .color(egui::Color32::from_rgb(180, 180, 180)),
                        );

                        ui.checkbox(
                            &mut self.temp_config.baixa_latencia,
                            egui::RichText::new("Modo de Baixa Latência")
                                .size(13.0)
                                .color(egui::Color32::from_rgb(160, 160, 160)),
                        );

                        ui.checkbox(
                            &mut self.temp_config.otimizacoes_graficas,
                            egui::RichText::new("Otimizações Gráficas")
                                .size(13.0)
                                .color(egui::Color32::from_rgb(160, 160, 160)),
                        );
                    });

                    ui.add_space(10.0);

                    // Seção de Servidor
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new("Configuração de Servidor")
                                .size(14.0)
                                .color(egui::Color32::from_rgb(180, 180, 180)),
                        );

                        // Opções predefinidas
                        ui.vertical(|ui| {
                            for (index, _servidor) in SERVIDORES_PREDEFINIDOS.iter().enumerate() {
                                let servidor_name = match index {
                                    0 => "Servidor Principal",
                                    1 => "Servidor local P/8080",
                                    2 => "Servidor local P/80",
                                    _ => "Outro Servidor",
                                };

                                if ui
                                    .radio_value(
                                        &mut self.temp_config.servidor_selecionado,
                                        index,
                                        egui::RichText::new(servidor_name)
                                            .size(13.0)
                                            .color(egui::Color32::from_rgb(160, 160, 160)),
                                    )
                                    .clicked()
                                {
                                    self.temp_config.usar_servidor_personalizado = false;
                                }
                            }

                            // Opção customizada
                            if ui
                                .radio_value(
                                    &mut self.temp_config.usar_servidor_personalizado,
                                    true,
                                    egui::RichText::new("Servidor Personalizado")
                                        .size(13.0)
                                        .color(egui::Color32::from_rgb(160, 160, 160)),
                                )
                                .clicked()
                            {
                                self.temp_config.usar_servidor_personalizado = true;
                            }

                            // Mostrar campo de texto apenas se servidor personalizado estiver selecionado
                            if self.temp_config.usar_servidor_personalizado {
                                ui.add_enabled_ui(true, |ui| {
                                    ui.add_sized(
                                        [360.0, 24.0],
                                        egui::TextEdit::singleline(
                                            &mut self.temp_config.servidor_personalizado,
                                        )
                                        .hint_text("https://exemplo.com/login"),
                                    );
                                });
                            } else {
                                ui.add_enabled_ui(false, |ui| {
                                    ui.add_sized(
                                        [360.0, 24.0],
                                        egui::TextEdit::singleline(
                                            &mut self.temp_config.servidor_personalizado,
                                        )
                                        .hint_text("https://exemplo.com/login"),
                                    );
                                });
                            }
                        });
                    });

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        // Botão Cancelar à esquerda
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            if ui
                                .add_sized(
                                    [90.0, 28.0],
                                    egui::Button::new(
                                        egui::RichText::new("Cancelar")
                                            .size(13.0)
                                            .color(egui::Color32::from_rgb(200, 200, 200)),
                                    )
                                    .fill(egui::Color32::from_rgba_unmultiplied(45, 45, 45, 255))
                                    .corner_radius(2.0)
                                    .stroke(egui::Stroke::NONE),
                                )
                                .clicked()
                            {
                                self.show_modal = false;
                            }
                        });

                        // Espaço flexível entre os botões
                        ui.with_layout(
                            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                            |ui| {
                                ui.allocate_space(ui.available_size());
                            },
                        );

                        // Botão Salvar à direita
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_sized(
                                    [90.0, 28.0],
                                    egui::Button::new(
                                        egui::RichText::new("Salvar")
                                            .size(13.0)
                                            .color(egui::Color32::BLACK),
                                    )
                                    .fill(egui::Color32::from_rgb(76, 175, 80))
                                    .corner_radius(2.0)
                                    .stroke(egui::Stroke::NONE),
                                )
                                .clicked()
                            {
                                // Validar configurações antes de salvar
                                let is_valid = if self.temp_config.usar_servidor_personalizado {
                                    !self.temp_config.servidor_personalizado.trim().is_empty()
                                } else {
                                    self.temp_config.servidor_selecionado
                                        < SERVIDORES_PREDEFINIDOS.len()
                                };

                                if is_valid {
                                    // Atualizar a configuração com os valores temporários
                                    {
                                        let mut config = self.config.lock().unwrap();
                                        *config = self.temp_config.clone();
                                    }

                                    // Salvar no arquivo
                                    if let Err(e) = self.save_config() {
                                        info!("Erro ao salvar configuração: {}", e);
                                    }

                                    self.show_modal = false;
                                }
                            }
                        });
                    });
                });
            });
    }

    // Retorna uma referência ao estado atual da configuração
    #[allow(dead_code)]
    pub fn get_config(&self) -> Arc<Mutex<GameConfig>> {
        self.config.clone()
    }
}

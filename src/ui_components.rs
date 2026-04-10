use eframe::egui;
use log::info;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

use crate::GameLauncher;
use crate::cache;
use crate::constants::{
    ACCENT_PRIMARY_RGB, ACCENT_SECONDARY_RGB, PING_EXCELLENT_THRESHOLD, PING_GOOD_THRESHOLD,
    SURFACE_RGB,
};
use crate::message_system::LauncherMessage;

fn primary_color() -> egui::Color32 {
    egui::Color32::from_rgb(
        ACCENT_SECONDARY_RGB.0,
        ACCENT_SECONDARY_RGB.1,
        ACCENT_SECONDARY_RGB.2,
    )
}

fn accent_color() -> egui::Color32 {
    egui::Color32::from_rgb(
        ACCENT_PRIMARY_RGB.0,
        ACCENT_PRIMARY_RGB.1,
        ACCENT_PRIMARY_RGB.2,
    )
}

fn surface_color(alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(SURFACE_RGB.0, SURFACE_RGB.1, SURFACE_RGB.2, alpha)
}

fn muted_text() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(214, 222, 242, 190)
}

fn panel_frame(fill: egui::Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(24.0)
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 18),
        ))
        .inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 18,
            bottom: 18,
        })
}

fn metric_row(
    ui: &mut egui::Ui,
    label: &str,
    value: impl Into<String>,
    value_color: egui::Color32,
) {
    let value = value.into();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(11.0)
                .color(egui::Color32::from_rgba_unmultiplied(166, 178, 208, 170))
                .strong(),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(value)
                .size(13.5)
                .color(value_color)
                .strong(),
        );
    });
}

fn utility_button(ui: &mut egui::Ui, label: &str, width: f32) -> egui::Response {
    ui.add_sized(
        [width, 34.0],
        egui::Button::new(
            egui::RichText::new(label)
                .size(13.0)
                .color(egui::Color32::WHITE)
                .strong(),
        )
        .fill(surface_color(228))
        .corner_radius(14.0)
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 18),
        )),
    )
}

fn pill_button(
    ui: &mut egui::Ui,
    label: &str,
    width: f32,
    height: f32,
    fill: egui::Color32,
) -> egui::Response {
    ui.add_sized(
        [width, height],
        egui::Button::new(
            egui::RichText::new(label)
                .size(16.0)
                .color(egui::Color32::WHITE)
                .strong(),
        )
        .fill(fill)
        .corner_radius(16.0)
        .stroke(egui::Stroke::NONE),
    )
}

pub fn render_all_components(
    launcher: &mut GameLauncher,
    ctx: &egui::Context,
    available_size: egui::Vec2,
) {
    let footer_height = if launcher.show_footer { 35.0 } else { 0.0 };
    if launcher.show_footer {
        launcher.render_footer_impl(ctx, footer_height);
    }

    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(0, 0, 0))
                .inner_margin(egui::Margin::ZERO)
                .outer_margin(egui::Margin::ZERO),
        )
        .show(ctx, |ui| {
            launcher.render_background_impl(ui);

            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(18.0, 0.0);

                let side_height = (available_size.y - footer_height - 36.0).max(300.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(250.0, side_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        panel_frame(egui::Color32::from_rgba_unmultiplied(8, 12, 22, 220)).show(
                            ui,
                            |ui| {
                                ui.set_min_height(side_height);
                                ui.label(
                                    egui::RichText::new("PENULTIMA CONTROL")
                                        .size(11.0)
                                        .color(accent_color())
                                        .strong(),
                                );
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new("Launcher")
                                        .size(28.0)
                                        .color(egui::Color32::WHITE)
                                        .strong(),
                                );
                                ui.label(
                                    egui::RichText::new(
                                        "Status do cliente, sincronizacao rapida e acesso direto ao jogo.",
                                    )
                                    .size(13.5)
                                    .color(muted_text()),
                                );

                                ui.add_space(18.0);
                                launcher.render_version_panel_impl(ui);
                                ui.add_space(14.0);
                                launcher.render_ping_panel_impl(ui);

                                ui.add_space(14.0);
                                panel_frame(egui::Color32::from_rgba_unmultiplied(15, 22, 36, 188))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new("CLIENTE")
                                                .size(10.5)
                                                .color(primary_color())
                                                .strong(),
                                        );
                                        ui.add_space(8.0);

                                        let headline = if launcher.current_version.is_some()
                                            && launcher.client_version.is_some()
                                        {
                                            "Cliente detectado"
                                        } else if launcher.current_version.is_some() {
                                            "Arquivos do jogo presentes"
                                        } else {
                                            "Instalacao pendente"
                                        };

                                        ui.label(
                                            egui::RichText::new(headline)
                                                .size(18.0)
                                                .color(egui::Color32::WHITE)
                                                .strong(),
                                        );
                                        ui.label(
                                            egui::RichText::new(
                                                "Use JOGAR quando o client estiver pronto ou force a sincronizacao quando precisar limpar diferencas.",
                                            )
                                            .size(12.5)
                                            .color(muted_text()),
                                        );
                                    });

                                let spacer = (ui.available_height() - 54.0).max(16.0);
                                ui.add_space(spacer);

                                if !launcher.is_processing {
                                    ui.horizontal_centered(|ui| {
                                        if pill_button(
                                            ui,
                                            "MINIMIZAR NO TRAY",
                                            190.0,
                                            34.0,
                                            egui::Color32::from_rgba_unmultiplied(24, 32, 48, 228),
                                        )
                                        .clicked()
                                        {
                                            launcher.minimize_to_tray(ctx);
                                        }
                                    });
                                }
                            },
                        );
                    },
                );

                ui.allocate_ui_with_layout(
                    ui.available_size(),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        panel_frame(egui::Color32::from_rgba_unmultiplied(7, 10, 19, 196)).show(
                            ui,
                            |ui| {
                                let rect = ui.max_rect();
                                ui.painter().circle_filled(
                                    egui::pos2(rect.center().x, rect.top() + 150.0),
                                    150.0,
                                    egui::Color32::from_rgba_unmultiplied(124, 102, 255, 26),
                                );
                                ui.painter().circle_filled(
                                    egui::pos2(rect.center().x + 70.0, rect.top() + 120.0),
                                    95.0,
                                    egui::Color32::from_rgba_unmultiplied(234, 182, 76, 18),
                                );

                                ui.vertical_centered(|ui| {
                                    launcher.render_logo_impl(ui);
                                    launcher.render_loading_indicator_impl(ui, ctx, available_size);
                                    ui.add_space(18.0);
                                    launcher.render_main_buttons_impl(
                                        ui,
                                        ctx,
                                        260.0,
                                        48.0,
                                        available_size,
                                    );
                                    launcher.render_bottom_buttons_impl(ui, ctx, 34.0);
                                });
                            },
                        );
                    },
                );
            });
        });
}

impl GameLauncher {
    pub fn render_background_impl(&self, ui: &mut egui::Ui) {
        let available_rect = ui.max_rect();

        if let Some(texture) = &self.background_texture {
            ui.painter().image(
                texture.id(),
                available_rect,
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            ui.painter()
                .rect_filled(available_rect, 0.0, egui::Color32::from_rgb(4, 6, 12));
        }

        ui.painter().rect_filled(
            available_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(6, 9, 17, 212),
        );
        ui.painter().circle_filled(
            egui::pos2(available_rect.right() - 180.0, available_rect.top() + 110.0),
            170.0,
            egui::Color32::from_rgba_unmultiplied(92, 74, 236, 26),
        );
        ui.painter().circle_filled(
            egui::pos2(
                available_rect.left() + 120.0,
                available_rect.bottom() - 70.0,
            ),
            130.0,
            egui::Color32::from_rgba_unmultiplied(234, 182, 76, 16),
        );
    }

    pub fn render_version_panel_impl(&self, ui: &mut egui::Ui) {
        panel_frame(egui::Color32::from_rgba_unmultiplied(14, 20, 34, 196)).show(ui, |ui| {
            ui.label(
                egui::RichText::new("VERSOES")
                    .size(10.5)
                    .color(primary_color())
                    .strong(),
            );
            ui.add_space(10.0);

            metric_row(
                ui,
                "Launcher",
                format!("v{}", self.launcher_version),
                egui::Color32::WHITE,
            );

            if let Some(version) = &self.current_version {
                metric_row(ui, "Game", format!("v{}", version), egui::Color32::WHITE);
            } else {
                metric_row(
                    ui,
                    "Game",
                    "nao instalado",
                    egui::Color32::from_rgb(255, 163, 122),
                );
            }

            if let Some(client_ver) = &self.client_version {
                metric_row(
                    ui,
                    "Client",
                    format!("v{}", client_ver),
                    egui::Color32::WHITE,
                );
            } else {
                metric_row(
                    ui,
                    "Client",
                    "nao encontrado",
                    egui::Color32::from_rgb(255, 143, 143),
                );
            }
        });
    }

    pub fn render_ping_panel_impl(&self, ui: &mut egui::Ui) {
        let (value, tone, detail) = if let Some(ping) = self.server_ping {
            if ping <= PING_EXCELLENT_THRESHOLD {
                (
                    format!("{} ms", ping),
                    egui::Color32::from_rgb(102, 240, 170),
                    "Rota estavel",
                )
            } else if ping <= PING_GOOD_THRESHOLD {
                (
                    format!("{} ms", ping),
                    egui::Color32::from_rgb(255, 216, 120),
                    "Resposta normal",
                )
            } else {
                (
                    format!("{} ms", ping),
                    egui::Color32::from_rgb(255, 124, 124),
                    "Latencia alta",
                )
            }
        } else if self.last_ping_check.is_some() {
            (
                "Indisponivel".to_string(),
                egui::Color32::from_rgb(255, 124, 124),
                "Sem resposta do servidor",
            )
        } else {
            (
                "Verificando...".to_string(),
                egui::Color32::from_rgb(202, 214, 236),
                "Primeiro teste em andamento",
            )
        };

        panel_frame(egui::Color32::from_rgba_unmultiplied(14, 20, 34, 196)).show(ui, |ui| {
            ui.label(
                egui::RichText::new("SERVIDOR")
                    .size(10.5)
                    .color(accent_color())
                    .strong(),
            );
            ui.add_space(8.0);
            ui.label(egui::RichText::new(value).size(24.0).color(tone).strong());
            ui.label(egui::RichText::new(detail).size(12.5).color(muted_text()));
        });
    }

    pub fn render_logo_impl(&self, ui: &mut egui::Ui) {
        ui.add_space(8.0);

        if let Some(logo) = &self.logo_texture {
            let final_size = egui::vec2(250.0, 176.0);
            ui.add(egui::Image::new(egui::ImageSource::Texture(
                egui::load::SizedTexture::new(logo.id(), final_size),
            )));
        }

        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "Atualize, confirme o status do servidor e abra o client sem ruido visual.",
            )
            .size(14.0)
            .color(muted_text()),
        );
    }

    pub fn render_loading_indicator_impl(
        &self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        _available_size: egui::Vec2,
    ) {
        let status_color = if self.is_alert_message {
            egui::Color32::from_rgb(255, 124, 124)
        } else if self.temp_message_time.is_some() {
            egui::Color32::from_rgb(118, 236, 170)
        } else {
            egui::Color32::WHITE
        };

        panel_frame(egui::Color32::from_rgba_unmultiplied(12, 18, 30, 220)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.is_processing {
                    ui.add(egui::Spinner::new().size(18.0));
                    ctx.request_repaint_after(Duration::from_millis(60));
                } else {
                    ui.label(
                        egui::RichText::new("●")
                            .size(15.0)
                            .color(primary_color())
                            .strong(),
                    );
                }

                let headline = if self.is_processing {
                    "SINCRONIZANDO CLIENTE"
                } else if self.temp_message_time.is_some() {
                    "ULTIMA ACAO"
                } else {
                    "STATUS"
                };

                ui.label(
                    egui::RichText::new(headline)
                        .size(10.5)
                        .color(primary_color())
                        .strong(),
                );
            });

            ui.add_space(8.0);
            ui.add_sized(
                [ui.available_width(), 0.0],
                egui::Label::new(
                    egui::RichText::new(&self.status)
                        .size(15.0)
                        .color(status_color)
                        .strong(),
                )
                .wrap(),
            );

            if self.is_processing || (self.progress > 0.0 && self.progress < 1.0) {
                ui.add_space(10.0);
                ui.add(
                    egui::ProgressBar::new(self.progress.clamp(0.0, 1.0))
                        .desired_width(ui.available_width())
                        .fill(primary_color())
                        .text(format!("{:.0}%", self.progress.clamp(0.0, 1.0) * 100.0)),
                );
            }
        });
    }

    pub fn render_main_buttons_impl(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        button_width: f32,
        button_height: f32,
        _available_size: egui::Vec2,
    ) {
        if self.is_processing {
            return;
        }

        ui.add_space(4.0);
        ui.vertical_centered(|ui| {
            let is_game_running = self.is_game_running();
            let (_, additional_count) = self.game_client.sync_client_state();
            let has_additional_clients = additional_count > 0;

            if is_game_running || has_additional_clients {
                let max_clients = self.game_client.max_clients;
                let can_launch = additional_count < max_clients;
                let response = ui.add_enabled(
                    can_launch,
                    egui::Button::new(
                        egui::RichText::new("ABRIR OUTRO CLIENTE")
                            .size(17.0)
                            .color(egui::Color32::WHITE)
                            .strong(),
                    )
                    .fill(if can_launch {
                        accent_color()
                    } else {
                        egui::Color32::from_rgb(88, 96, 112)
                    })
                    .corner_radius(16.0)
                    .stroke(egui::Stroke::NONE)
                    .min_size(egui::vec2(button_width, button_height)),
                );

                if response.clicked() && can_launch {
                    if let Err(error) = self.launch_client() {
                        self.status = format!("Erro ao iniciar o cliente: {}", error);
                        self.is_alert_message = true;
                    }
                }

                if !can_launch {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Limite de clientes adicionais atingido.")
                            .size(12.5)
                            .color(muted_text()),
                    );
                }
            } else {
                let response = ui.add(
                    egui::Button::new(
                        egui::RichText::new("JOGAR")
                            .size(21.0)
                            .color(egui::Color32::WHITE)
                            .strong(),
                    )
                    .fill(primary_color())
                    .corner_radius(18.0)
                    .stroke(egui::Stroke::NONE)
                    .min_size(egui::vec2(button_width, button_height)),
                );

                if response.clicked() {
                    if let Err(error) = self.launch_game(ctx) {
                        self.status = format!("Erro ao iniciar o jogo: {}", error);
                        self.is_alert_message = true;
                    }
                }
            }
        });
    }

    pub fn render_bottom_buttons_impl(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        _button_height: f32,
    ) {
        let (has_main, additional_count) = self.game_client.sync_client_state();

        if has_main || additional_count > 0 || self.is_processing {
            return;
        }

        ui.add_space(18.0);
        panel_frame(egui::Color32::from_rgba_unmultiplied(10, 15, 26, 196)).show(ui, |ui| {
            ui.label(
                egui::RichText::new("MANUTENCAO")
                    .size(10.5)
                    .color(accent_color())
                    .strong(),
            );
            ui.add_space(12.0);

            ui.horizontal_centered(|ui| {
                if utility_button(ui, "FORCAR ATUALIZACAO", 170.0).clicked() {
                    self.show_force_update_modal = true;
                }

                ui.add_space(10.0);

                if utility_button(ui, "LIMPAR CACHE", 140.0).clicked() {
                    let (tx, rx) = unbounded_channel();
                    self.message_receiver = Some(rx);
                    self.status = "Limpando cache...".to_string();
                    self.is_processing = true;
                    self.progress = 0.0;
                    ctx.request_repaint();

                    let download_path = self.download_path.clone();
                    let game_path = self.game_path.clone();
                    let state_path = self.state_path.clone();
                    let cache_manager =
                        cache::CacheManager::new(download_path, game_path, state_path);

                    tokio::spawn(async move {
                        match cache_manager.clean_cache(tx.clone()).await {
                            Ok(size_mb) => {
                                info!("Limpeza de cache concluida com sucesso");
                                let _ = tx.send(LauncherMessage::SetTempMessage(format!(
                                    "Cache limpo com sucesso! ({:.2} MB liberados)",
                                    size_mb
                                )));
                            }
                            Err(error) => {
                                info!("Erro durante limpeza de cache: {}", error);
                                let _ = tx.send(LauncherMessage::SetStatus(format!(
                                    "Erro ao limpar cache: {}",
                                    error
                                )));
                                let _ = tx.send(LauncherMessage::SetProcessing(false));
                            }
                        }
                    });
                }
            });

            ui.add_space(10.0);

            let mut disable_auto_start = self.disable_auto_start;
            if ui
                .checkbox(
                    &mut disable_auto_start,
                    egui::RichText::new("Desativar inicio automatico")
                        .size(13.0)
                        .color(muted_text()),
                )
                .changed()
            {
                self.disable_auto_start = disable_auto_start;
                let settings = cache::UserSettings { disable_auto_start };
                if let Err(error) = cache::CacheManager::new(
                    self.download_path.clone(),
                    self.game_path.clone(),
                    self.state_path.clone(),
                )
                .save_user_settings(&settings)
                {
                    info!("Erro ao salvar configuracoes: {}", error);
                }
            }
        });
    }

    pub fn render_footer_impl(&self, ctx: &egui::Context, footer_height: f32) {
        if self.show_footer {
            egui::TopBottomPanel::bottom("footer_panel")
                .exact_height(footer_height)
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgba_premultiplied(30, 30, 30, 200))
                        .inner_margin(egui::Margin::symmetric(15, 5))
                        .outer_margin(egui::Margin::ZERO)
                        .stroke(egui::Stroke::NONE),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("Launcher v{}", self.launcher_version))
                                    .color(egui::Color32::from_rgb(180, 180, 180))
                                    .size(12.0),
                            );

                            ui.add_space(15.0);

                            if let Some(version) = &self.current_version {
                                ui.label(
                                    egui::RichText::new(format!("Game v{}", version))
                                        .color(egui::Color32::from_rgb(180, 180, 180))
                                        .size(12.0),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new("Game: nao instalado")
                                        .color(egui::Color32::from_rgb(180, 180, 180))
                                        .size(12.0),
                                );
                            }

                            ui.add_space(15.0);

                            if let Some(client_ver) = &self.client_version {
                                ui.label(
                                    egui::RichText::new(format!("Client v{}", client_ver))
                                        .color(egui::Color32::from_rgb(180, 180, 180))
                                        .size(12.0),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new("Client: nao encontrado")
                                        .color(egui::Color32::from_rgb(180, 180, 180))
                                        .size(12.0),
                                );
                            }
                        });

                        ui.add_space(10.0);
                        self.proxy_status.render_status_indicators(ui);
                    });
                });
        }
    }
}

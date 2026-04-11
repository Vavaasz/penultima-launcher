use eframe::egui;
use log::info;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

use crate::GameLauncher;
use crate::cache;
use crate::constants::{
    ACCENT_PRIMARY_RGB, ACCENT_SECONDARY_RGB, LOGO_SIZE, PING_EXCELLENT_THRESHOLD,
    PING_GOOD_THRESHOLD, SURFACE_RGB,
};
use crate::message_system::LauncherMessage;

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

            egui::Area::new("version_panel".into())
                .fixed_pos(egui::pos2(10.0, 10.0))
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_black_alpha(100))
                        .corner_radius(14.0)
                        .inner_margin(egui::Margin {
                            left: 8,
                            right: 8,
                            top: 5,
                            bottom: 5,
                        })
                        .show(ui, |ui| {
                            launcher.render_version_panel_impl(ui);
                        });
                });

            egui::Area::new("ping_panel".into())
                .fixed_pos(egui::pos2(10.0, 90.0))
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_black_alpha(100))
                        .corner_radius(14.0)
                        .inner_margin(egui::Margin {
                            left: 8,
                            right: 8,
                            top: 5,
                            bottom: 5,
                        })
                        .show(ui, |ui| {
                            launcher.render_ping_panel_impl(ui);
                        });
                });

            ui.vertical_centered(|ui| {
                launcher.render_logo_impl(ui);
                launcher.render_loading_indicator_impl(ui, ctx, available_size);

                let button_width = 200.0;
                let button_height = 40.0;
                launcher.render_main_buttons_impl(
                    ui,
                    ctx,
                    button_width,
                    button_height,
                    available_size,
                );

                launcher.render_bottom_buttons_impl(ui, ctx, button_height);
            });
        });
}

impl GameLauncher {
    pub fn render_background_impl(&self, ui: &mut egui::Ui) {
        if let Some(texture) = &self.background_texture {
            let available_rect = ui.max_rect();

            ui.painter().image(
                texture.id(),
                available_rect,
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0)),
                egui::Color32::WHITE,
            );

            ui.painter().rect_filled(
                available_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(8, 12, 22, 176),
            );
        }
    }

    pub fn render_version_panel_impl(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(
                        egui::RichText::new(format!("Launcher v{}", self.launcher_version))
                            .size(12.0)
                            .color(egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                    ));

                    if let Some(version) = &self.current_version {
                        ui.add(egui::Label::new(
                            egui::RichText::new(format!("Game v{}", version))
                                .size(12.0)
                                .color(egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                        ));
                    } else {
                        ui.add(egui::Label::new(
                            egui::RichText::new("Game: nao instalado")
                                .size(12.0)
                                .color(egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                        ));
                    }

                    if let Some(client_ver) = &self.client_version {
                        ui.add(egui::Label::new(
                            egui::RichText::new(format!("Client v{}", client_ver))
                                .size(12.0)
                                .color(egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                        ));
                    } else {
                        ui.add(egui::Label::new(
                            egui::RichText::new("Client: nao encontrado")
                                .size(12.0)
                                .color(egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                        ));
                    }
                });
            });
        });
    }

    pub fn render_ping_panel_impl(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                ui.vertical(|ui| {
                    if let Some(ping) = self.server_ping {
                        let color = if ping <= PING_EXCELLENT_THRESHOLD {
                            egui::Color32::from_rgb(0, 255, 0)
                        } else if ping <= PING_GOOD_THRESHOLD {
                            egui::Color32::from_rgb(255, 255, 0)
                        } else {
                            egui::Color32::from_rgb(255, 0, 0)
                        };

                        ui.add(egui::Label::new(
                            egui::RichText::new(format!("Ping: {}ms", ping))
                                .size(12.0)
                                .color(color),
                        ));
                    } else if self.last_ping_check.is_some() {
                        ui.add(egui::Label::new(
                            egui::RichText::new("Ping: indisponivel")
                                .size(12.0)
                                .color(egui::Color32::from_rgb(255, 120, 120)),
                        ));
                    } else {
                        ui.add(egui::Label::new(
                            egui::RichText::new("Ping: verificando...")
                                .size(12.0)
                                .color(egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                        ));
                    }
                });
            });
        });
    }

    pub fn render_logo_impl(&self, ui: &mut egui::Ui) {
        ui.add_space(35.0);

        if let Some(logo) = &self.logo_texture {
            let final_size = egui::vec2(LOGO_SIZE.0, LOGO_SIZE.1);

            ui.add(egui::Image::new(egui::ImageSource::Texture(
                egui::load::SizedTexture::new(logo.id(), final_size),
            )));
        }

        ui.add_space(10.0);
    }

    pub fn render_loading_indicator_impl(
        &self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        available_size: egui::Vec2,
    ) {
        if self.is_processing
            || !self.game_client.get_clients_count().1.eq(&0)
            || self.game_client.get_clients_count().0
            || self.temp_message_time.is_some()
        {
            let indicator_height = 45.0;
            let response = ui.allocate_space(egui::Vec2::new(available_size.x, indicator_height));
            let rect = response.1;
            let center = rect.center();

            let (has_main, additional_count) = self.game_client.get_clients_count();
            if self.is_processing || has_main || additional_count > 0 {
                let time = ui.input(|i| i.time) as f32;
                let angle = (time * 2.0) % std::f32::consts::TAU;
                let radius = 30.0;
                let num_points = 10;
                for i in 0..num_points {
                    let point_angle =
                        angle + (i as f32 * std::f32::consts::TAU / num_points as f32);
                    let x = center.x + radius * point_angle.cos();
                    let y = center.y + radius * point_angle.sin();
                    let point_pos = egui::Pos2::new(x, y);
                    let point_size = 3.5_f32
                        + 3.0 * ((angle * 2.0 + i as f32 * 0.5) % std::f32::consts::TAU).sin();

                    ui.painter().circle_filled(
                        point_pos,
                        point_size,
                        egui::Color32::from_rgb(
                            ACCENT_PRIMARY_RGB.0,
                            ACCENT_PRIMARY_RGB.1,
                            ACCENT_PRIMARY_RGB.2,
                        ),
                    );
                }

                if self.is_processing || has_main || additional_count > 0 {
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
            }

            ui.add_space(10.0);

            ui.allocate_ui_with_layout(
                egui::Vec2::new(rect.width(), 25.0),
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    ui.label(
                        egui::RichText::new(&self.status)
                            .size(20.0)
                            .color(if self.is_alert_message {
                                egui::Color32::from_rgb(255, 100, 100)
                            } else if self.temp_message_time.is_some() {
                                egui::Color32::from_rgb(100, 255, 100)
                            } else {
                                egui::Color32::from_rgb(220, 220, 220)
                            })
                            .strong(),
                    );
                },
            );
        }
    }

    pub fn render_main_buttons_impl(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        button_width: f32,
        button_height: f32,
        available_size: egui::Vec2,
    ) {
        let (has_main, additional_count) = self.game_client.sync_client_state();
        if !self.is_processing
            && additional_count == 0
            && !has_main
            && self.temp_message_time.is_none()
        {
            ui.add_space(available_size.y * 0.04);
        } else {
            ui.add_space(available_size.y * 0.01);
        }

        let available_width = ui.available_width();
        let indent = (available_width - button_width) / 2.0;

        let is_game_running = self.is_game_running();
        let (_, has_additional_clients) = self.game_client.sync_client_state();
        let has_additional_clients = has_additional_clients > 0;

        if self.is_processing {
        } else if is_game_running || has_additional_clients {
            ui.horizontal(|ui| {
                ui.add_space(indent);
                let (_, additional_count) = self.game_client.sync_client_state();
                let max_clients = self.game_client.max_clients;
                let can_launch = additional_count < max_clients;

                if ui
                    .add_sized(
                        [button_width, button_height],
                        egui::Button::new(
                            egui::RichText::new("▶ Abrir Outro Cliente")
                                .size(15.0)
                                .color(if can_launch {
                                    if ui.ui_contains_pointer() {
                                        egui::Color32::BLACK
                                    } else {
                                        egui::Color32::WHITE
                                    }
                                } else {
                                    egui::Color32::GRAY
                                }),
                        )
                        .fill(if can_launch {
                            if ui.ui_contains_pointer() {
                                egui::Color32::from_rgb(
                                    ACCENT_PRIMARY_RGB.0,
                                    ACCENT_PRIMARY_RGB.1,
                                    ACCENT_PRIMARY_RGB.2,
                                )
                            } else {
                                egui::Color32::from_rgb(
                                    ACCENT_SECONDARY_RGB.0,
                                    ACCENT_SECONDARY_RGB.1,
                                    ACCENT_SECONDARY_RGB.2,
                                )
                            }
                        } else {
                            egui::Color32::from_rgb(150, 150, 150)
                        })
                        .corner_radius(10.0)
                        .stroke(egui::Stroke::NONE),
                    )
                    .clicked()
                    && can_launch
                {
                    if let Err(error) = self.launch_client() {
                        self.status = format!("Erro ao iniciar o cliente: {}", error);
                    }
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.add_space(indent);
                if ui
                    .add_sized(
                        [button_width, button_height],
                        egui::Button::new(egui::RichText::new("▶ JOGAR").size(22.0).color(
                            if ui.ui_contains_pointer() {
                                egui::Color32::BLACK
                            } else {
                                egui::Color32::WHITE
                            },
                        ))
                        .fill(if ui.ui_contains_pointer() {
                            egui::Color32::from_rgb(
                                ACCENT_PRIMARY_RGB.0,
                                ACCENT_PRIMARY_RGB.1,
                                ACCENT_PRIMARY_RGB.2,
                            )
                        } else {
                            egui::Color32::from_rgb(
                                ACCENT_SECONDARY_RGB.0,
                                ACCENT_SECONDARY_RGB.1,
                                ACCENT_SECONDARY_RGB.2,
                            )
                        })
                        .corner_radius(10.0)
                        .stroke(egui::Stroke::NONE),
                    )
                    .clicked()
                {
                    if let Err(error) = self.launch_game(ctx) {
                        self.status = format!("Erro ao iniciar o jogo: {}", error);
                    }
                }
            });
        }
    }

    pub fn render_bottom_buttons_impl(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        button_height: f32,
    ) {
        let available_height = ui.available_height();
        ui.add_space(available_height - button_height - 1.0);

        let (has_main, additional_count) = self.game_client.sync_client_state();
        if !self.is_processing {
            ui.horizontal_centered(|ui| {
                if ui
                    .add_sized(
                        [150.0, 30.0],
                            egui::Button::new(
                            egui::RichText::new("Minimizar launcher")
                                .size(14.0)
                                .color(egui::Color32::from_rgb(220, 220, 220)),
                        )
                        .fill(egui::Color32::from_rgba_unmultiplied(
                            SURFACE_RGB.0,
                            SURFACE_RGB.1,
                            SURFACE_RGB.2,
                            220,
                        ))
                        .corner_radius(12.0)
                        .stroke(egui::Stroke::NONE),
                    )
                    .clicked()
                {
                    self.minimize_to_tray(ctx);
                }
            });
        }

        if !has_main && additional_count == 0 && !self.is_processing {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::BOTTOM), |ui| {
                    ui.add_space(10.0);

                    if ui
                        .add_sized(
                            [130.0, 30.0],
                            egui::Button::new(
                                egui::RichText::new("Forcar Atualizacao").size(14.0).color(
                                    egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180),
                                ),
                            )
                            .fill(egui::Color32::from_rgba_unmultiplied(40, 40, 40, 180))
                            .corner_radius(4.0)
                            .stroke(egui::Stroke::NONE),
                        )
                        .clicked()
                    {
                        self.show_force_update_modal = true;
                    }
                });

                ui.add_space(ui.available_width() * 0.22);

                let mut disable_auto_start = self.disable_auto_start;
                if ui
                    .checkbox(
                        &mut disable_auto_start,
                        egui::RichText::new("Desativar inicio automatico")
                            .color(egui::Color32::from_rgb(180, 180, 180))
                            .size(14.0),
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

                ui.add_space(ui.available_width() * 0.18);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
                    ui.add_space(10.0);

                    if ui
                        .add_sized(
                            [130.0, 30.0],
                            egui::Button::new(
                                egui::RichText::new("Limpar Cache").size(14.0).color(
                                    egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180),
                                ),
                            )
                            .fill(egui::Color32::from_rgba_unmultiplied(40, 40, 40, 180))
                            .corner_radius(4.0)
                            .stroke(egui::Stroke::NONE),
                        )
                        .clicked()
                    {
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
            });
        }
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

                    });
                });
        }
    }
}

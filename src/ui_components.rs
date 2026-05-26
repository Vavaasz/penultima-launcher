use eframe::egui;
use log::info;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

use crate::GameLauncher;
use crate::LauncherTab;
use crate::boosted_preview::BoostedPreviewKind;
use crate::cache;
use crate::constants::{
    ACCENT_PRIMARY_RGB, ACCENT_SECONDARY_RGB, BATTLE_PASS_URL, CHANGELOG_URL, DISCORD_URL,
    EVENT_CALENDAR_URL, INVESTMENT_URL, LOGO_SIZE, PACK_WEEK_URL, PING_EXCELLENT_THRESHOLD,
    PING_GOOD_THRESHOLD, STARTUP_SPLASH_DURATION, SURFACE_RGB, WEBSITE_BASE_URL,
};
use crate::message_system::LauncherMessage;
use crate::website_status::{EventSummary, OfferPreview, OfferSummary};

const TOP_STATUS_CARD_HEIGHT: f32 = 248.0;
const PREVIEW_REPAINT_MIN_MS: u32 = 180;
const PREVIEW_REPAINT_MAX_MS: u32 = 320;
const SPLASH_REPAINT_MS: u64 = 80;
const STATUS_REPAINT_MS: u64 = 350;
const BACKGROUND_REPAINT_MS: u64 = 140;

fn preview_repaint_delay_ms(delay_ms: u32) -> u64 {
    delay_ms.clamp(PREVIEW_REPAINT_MIN_MS, PREVIEW_REPAINT_MAX_MS) as u64
}

fn panel_fill(alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(SURFACE_RGB.0, SURFACE_RGB.1, SURFACE_RGB.2, alpha)
}

fn accent_fill() -> egui::Color32 {
    egui::Color32::from_rgb(
        ACCENT_PRIMARY_RGB.0,
        ACCENT_PRIMARY_RGB.1,
        ACCENT_PRIMARY_RGB.2,
    )
}

fn smoothstep(value: f32) -> f32 {
    let t = value.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn compact_path_label(path: &str, max_chars: usize) -> String {
    if path.chars().count() <= max_chars {
        return path.to_string();
    }

    let tail_len = max_chars.saturating_sub(3);
    let tail = path
        .chars()
        .rev()
        .take(tail_len)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

fn primary_button(ui: &mut egui::Ui, label: &str, width: f32, height: f32) -> egui::Response {
    ui.add_sized(
        [width, height],
        egui::Button::new(
            egui::RichText::new(label)
                .size(16.0)
                .strong()
                .color(egui::Color32::from_rgb(20, 22, 26)),
        )
        .fill(accent_fill())
        .corner_radius(8.0)
        .stroke(egui::Stroke::NONE),
    )
}

fn secondary_button(ui: &mut egui::Ui, label: &str, width: f32, height: f32) -> egui::Response {
    ui.add_sized(
        [width, height],
        egui::Button::new(
            egui::RichText::new(label)
                .size(16.0)
                .strong()
                .color(egui::Color32::WHITE),
        )
        .fill(egui::Color32::from_rgb(
            ACCENT_SECONDARY_RGB.0,
            ACCENT_SECONDARY_RGB.1,
            ACCENT_SECONDARY_RGB.2,
        ))
        .corner_radius(8.0)
        .stroke(egui::Stroke::NONE),
    )
}

fn small_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    small_button_sized(ui, label, 122.0)
}

fn small_button_sized(ui: &mut egui::Ui, label: &str, width: f32) -> egui::Response {
    ui.add_sized(
        [width, 30.0],
        egui::Button::new(
            egui::RichText::new(label)
                .size(13.0)
                .color(egui::Color32::from_rgb(220, 224, 232)),
        )
        .fill(panel_fill(220))
        .corner_radius(8.0)
        .stroke(egui::Stroke::NONE),
    )
}

fn sidebar_action_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized(
        [138.0, 34.0],
        egui::Button::new(
            egui::RichText::new(label)
                .size(12.5)
                .color(egui::Color32::from_rgb(224, 228, 236)),
        )
        .fill(panel_fill(220))
        .corner_radius(8.0)
        .stroke(egui::Stroke::NONE),
    )
}

fn centered_fixed_row(ui: &mut egui::Ui, row_width: f32, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - row_width) * 0.5).max(0.0));
        add_contents(ui);
    });
}

fn render_card(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    render_card_with_min_height(ui, title, 0.0, add_contents);
}

fn render_card_with_min_height(
    ui: &mut egui::Ui,
    title: &str,
    min_content_height: f32,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(panel_fill(205))
        .corner_radius(8.0)
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 36),
        ))
        .inner_margin(egui::Margin::symmetric(12, 12))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(title)
                    .size(13.0)
                    .strong()
                    .color(egui::Color32::from_rgb(237, 204, 126)),
            );
            ui.add_space(8.0);
            if min_content_height > 0.0 {
                ui.set_min_height(min_content_height);
            }
            add_contents(ui);
            ui.add_space(2.0);
        });
}

fn render_card_with_fixed_height(
    ui: &mut egui::Ui,
    title: &str,
    height: f32,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());

    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.set_width(width);
        ui.set_height(height);

        egui::Frame::new()
            .fill(panel_fill(205))
            .corner_radius(8.0)
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 36),
            ))
            .inner_margin(egui::Margin::symmetric(12, 12))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                ui.set_min_height((height - 24.0).max(0.0));
                ui.label(
                    egui::RichText::new(title)
                        .size(13.0)
                        .strong()
                        .color(egui::Color32::from_rgb(237, 204, 126)),
                );
                ui.add_space(8.0);
                add_contents(ui);
                ui.add_space(2.0);
            });
    });
}

fn value_row(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        ui.label(
            egui::RichText::new(label)
                .size(12.0)
                .color(egui::Color32::from_rgb(160, 170, 185)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value.unwrap_or("Loading..."))
                    .size(13.0)
                    .strong()
                    .color(egui::Color32::from_rgb(225, 230, 238)),
            );
        });
    });
}

fn render_event_summary_tile(
    ui: &mut egui::Ui,
    label: &str,
    event: Option<&EventSummary>,
    empty_text: &str,
) {
    ui.vertical(|ui| {
        ui.set_min_height(70.0);
        ui.label(
            egui::RichText::new(label)
                .size(12.0)
                .strong()
                .color(egui::Color32::from_rgb(237, 204, 126)),
        );
        ui.add_space(4.0);

        if let Some(event) = event {
            ui.label(
                egui::RichText::new(&event.name)
                    .size(13.0)
                    .strong()
                    .color(egui::Color32::from_rgb(225, 230, 238)),
            );
            ui.label(
                egui::RichText::new(&event.window)
                    .size(11.5)
                    .color(egui::Color32::from_rgb(170, 180, 195)),
            );
        } else {
            ui.label(
                egui::RichText::new(empty_text)
                    .size(12.0)
                    .color(egui::Color32::from_rgb(195, 200, 210)),
            );
        }
    });
}

fn preview_tile_rect(ui: &mut egui::Ui, size: f32) -> (egui::Rect, egui::Response) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().rect_filled(
        rect,
        8.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 105),
    );
    ui.painter().rect_stroke(
        rect,
        8.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 34),
        ),
        egui::StrokeKind::Inside,
    );

    (rect, response)
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
            ui.set_min_size(available_size);

            ui.add_space(18.0);
            ui.horizontal_top(|ui| {
                ui.add_space(22.0);

                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    ui.set_min_height((available_size.y - 36.0).max(0.0));
                    launcher.render_launcher_sidebar_impl(ui, ctx, available_size);
                });

                ui.add_space(18.0);

                ui.vertical(|ui| {
                    let content_width = (available_size.x - 370.0).max(560.0);
                    let content_height = (available_size.y - 36.0).max(360.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(content_width, content_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(content_width);
                            ui.set_height(content_height);
                            ui.set_max_height(content_height);
                            launcher.render_site_content_impl(ui, ctx);
                        },
                    );
                });

                ui.add_space(22.0);
            });
        });
}

pub fn render_startup_splash(
    launcher: &mut GameLauncher,
    ctx: &egui::Context,
    available_size: egui::Vec2,
) {
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(0, 0, 0))
                .inner_margin(egui::Margin::ZERO)
                .outer_margin(egui::Margin::ZERO),
        )
        .show(ctx, |ui| {
            launcher.render_background_impl(ui);
            ui.set_min_size(available_size);
            launcher.render_startup_splash_impl(ui, ctx);
        });
}

impl GameLauncher {
    pub fn render_startup_splash_impl(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let elapsed = self.startup_splash_started.elapsed();
        if elapsed >= STARTUP_SPLASH_DURATION {
            self.startup_splash_finished = true;
            ctx.request_repaint();
            return;
        }

        let rect = ui.max_rect();
        let painter = ui.painter();
        let elapsed_secs = elapsed.as_secs_f32();
        let duration_secs = STARTUP_SPLASH_DURATION.as_secs_f32().max(0.1);
        let progress = (elapsed_secs / duration_secs).clamp(0.0, 1.0);
        let fade_in = smoothstep(elapsed_secs / 0.75);
        let fade_out = 1.0 - smoothstep((elapsed_secs - (duration_secs - 0.7)) / 0.7);
        let alpha = (fade_in * fade_out).clamp(0.0, 1.0);
        let center = rect.center() + egui::vec2(0.0, -18.0 * (1.0 - fade_in));
        let pulse = (elapsed_secs * 3.2).sin() * 0.014;
        let scale = 0.78 + 0.22 * fade_in + pulse - 0.04 * smoothstep((progress - 0.78) / 0.22);

        painter.rect_filled(
            rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, (150.0 * alpha) as u8),
        );

        let ring_alpha = (70.0 * alpha) as u8;
        for index in 0..3 {
            let phase = ((elapsed_secs * 0.42) + index as f32 * 0.32).fract();
            let radius = 110.0 + phase * 170.0;
            let ring_opacity = ((1.0 - phase) * ring_alpha as f32) as u8;
            painter.circle_stroke(
                center,
                radius,
                egui::Stroke::new(
                    1.0 + index as f32 * 0.35,
                    egui::Color32::from_rgba_unmultiplied(182, 72, 226, ring_opacity),
                ),
            );
        }

        let logo_texture = self
            .splash_logo_texture
            .as_ref()
            .or(self.logo_texture.as_ref());

        if let Some(logo) = logo_texture {
            let natural_size = logo.size_vec2();
            let max_size = egui::vec2(rect.width().min(390.0), rect.height().min(390.0));
            let fit = (max_size.x / natural_size.x).min(max_size.y / natural_size.y);
            let logo_size = natural_size * fit * scale;
            let logo_rect = egui::Rect::from_center_size(center, logo_size);
            let glow_rect = logo_rect.expand(26.0 + 10.0 * fade_in);

            painter.rect_filled(
                glow_rect,
                18.0,
                egui::Color32::from_rgba_unmultiplied(129, 35, 182, (42.0 * alpha) as u8),
            );
            painter.image(
                logo.id(),
                logo_rect.translate(egui::vec2(0.0, 4.0)),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, (88.0 * alpha) as u8),
            );
            painter.image(
                logo.id(),
                logo_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::from_white_alpha((255.0 * alpha) as u8),
            );

            let sweep_progress = smoothstep((elapsed_secs - 0.65) / 1.35);
            if sweep_progress > 0.0 && sweep_progress < 1.0 {
                let sweep_x = logo_rect.left() + logo_rect.width() * sweep_progress;
                let sweep_rect = egui::Rect::from_min_max(
                    egui::pos2(sweep_x - 24.0, logo_rect.top()),
                    egui::pos2(sweep_x + 18.0, logo_rect.bottom()),
                );
                painter.rect_filled(
                    sweep_rect,
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, (45.0 * alpha) as u8),
                );
            }
        }

        let bar_width = rect.width().min(320.0);
        let bar_rect = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.bottom() - 72.0),
            egui::vec2(bar_width, 3.0),
        );
        painter.rect_filled(
            bar_rect,
            2.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, (28.0 * alpha) as u8),
        );
        let filled = egui::Rect::from_min_size(
            bar_rect.min,
            egui::vec2(bar_rect.width() * smoothstep(progress), bar_rect.height()),
        );
        painter.rect_filled(
            filled,
            2.0,
            egui::Color32::from_rgba_unmultiplied(206, 141, 255, (190.0 * alpha) as u8),
        );

        ctx.request_repaint_after(Duration::from_millis(SPLASH_REPAINT_MS));
    }

    pub fn render_launcher_sidebar_impl(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        available_size: egui::Vec2,
    ) {
        ui.vertical_centered(|ui| {
            self.render_logo_impl(ui);

            if self.is_processing || self.temp_message_time.is_some() {
                self.render_loading_indicator_impl(ui, ctx, egui::vec2(300.0, available_size.y));
            } else {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(&self.status)
                        .size(17.0)
                        .strong()
                        .color(egui::Color32::from_rgb(225, 225, 225)),
                );
                ui.add_space(12.0);
            }

            if self.is_processing {
                ui.add(
                    egui::ProgressBar::new(self.progress.clamp(0.0, 1.0))
                        .desired_width(250.0)
                        .text(format!("{:.0}%", self.progress.clamp(0.0, 1.0) * 100.0)),
                );
                ui.add_space(12.0);
            }

            self.render_launch_buttons_compact_impl(ui, ctx);

            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("In case of crash, \"Force Update\"")
                    .size(12.0)
                    .color(egui::Color32::from_rgb(190, 190, 190)),
            );

            ui.add_space(8.0);
            centered_fixed_row(ui, 284.0, |ui| {
                if sidebar_action_button(ui, "Client Folder").clicked() {
                    self.select_install_folder(ctx);
                }

                if sidebar_action_button(ui, "Full Map").clicked() {
                    self.start_full_minimap_download(ctx);
                }
            });

            ui.add_space(6.0);
            centered_fixed_row(ui, 284.0, |ui| {
                if sidebar_action_button(ui, "Force Update").clicked() {
                    self.trigger_force_update(ctx);
                }

                if sidebar_action_button(ui, "Min Launcher").clicked() {
                    self.minimize_to_tray(ctx);
                }
            });

            ui.add_space(6.0);
            centered_fixed_row(ui, 284.0, |ui| {
                if sidebar_action_button(ui, "Min/Restore Clients").clicked() {
                    self.open_minimize_client_selector(ctx);
                }

                if sidebar_action_button(ui, "Update Launcher").clicked() {
                    self.start_launcher_update(ctx);
                }
            });

            ui.add_space(10.0);
            self.render_external_links_impl(ui);

            ui.add_space(10.0);
            self.render_utility_buttons_compact_impl(ui, ctx);

            ui.add_space(10.0);
            render_card(ui, "Versions", |ui| {
                self.render_version_panel_impl(ui);
            });

            ui.add_space(10.0);
            render_card(ui, "Connection", |ui| {
                self.render_ping_panel_impl(ui);
            });
        });
    }

    pub fn render_site_content_impl(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.selected_tab == LauncherTab::Dashboard, "Dashboard")
                .clicked()
            {
                self.selected_tab = LauncherTab::Dashboard;
            }

            if ui
                .selectable_label(self.selected_tab == LauncherTab::News, "News")
                .clicked()
            {
                self.selected_tab = LauncherTab::News;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_sized(
                        [90.0, 28.0],
                        egui::Button::new(egui::RichText::new("Refresh").size(13.0))
                            .fill(panel_fill(210))
                            .corner_radius(8.0)
                            .stroke(egui::Stroke::NONE),
                    )
                    .clicked()
                {
                    self.last_website_status_refresh = None;
                    self.refresh_website_status();
                }

                let refresh_text = if self.website_status_loading {
                    "Updating website data".to_string()
                } else if let Some(fetched_at) = &self.website_status.fetched_at {
                    format!("Updated {}", fetched_at)
                } else {
                    "Waiting for website data".to_string()
                };

                ui.label(
                    egui::RichText::new(refresh_text)
                        .size(12.0)
                        .color(egui::Color32::from_rgb(180, 190, 205)),
                );
            });
        });

        ui.add_space(12.0);

        let tab_height = ui.available_height().max(260.0);
        match self.selected_tab {
            LauncherTab::Dashboard => self.render_dashboard_tab_impl(ui, tab_height),
            LauncherTab::News => self.render_news_tab_impl(ui, tab_height),
        }
    }

    fn render_dashboard_tab_impl(&mut self, ui: &mut egui::Ui, max_height: f32) {
        if let Some(error) = &self.website_status.error {
            ui.label(
                egui::RichText::new(format!("Website data error: {}", error))
                    .size(12.0)
                    .color(egui::Color32::from_rgb(255, 150, 120)),
            );
            ui.add_space(8.0);
        }

        egui::ScrollArea::vertical()
            .id_salt("dashboard-scroll-v2")
            .auto_shrink([false, false])
            .max_height(max_height)
            .scroll_bar_visibility(
                egui::containers::scroll_area::ScrollBarVisibility::AlwaysVisible,
            )
            .show(ui, |ui| {
                ui.columns(2, |columns| {
                    self.render_boosts_card_impl(&mut columns[0]);
                    self.render_events_card_impl(&mut columns[1]);
                });

                ui.add_space(12.0);

                ui.columns(2, |columns| {
                    let battle_pass = self.website_status.battle_pass.clone();
                    self.render_offer_card_impl(
                        &mut columns[0],
                        "Battle Pass",
                        battle_pass.as_ref(),
                        BATTLE_PASS_URL,
                    );

                    let pack_week = self.website_status.pack_week.clone();
                    self.render_offer_card_impl(
                        &mut columns[1],
                        "Pack Week",
                        pack_week.as_ref(),
                        PACK_WEEK_URL,
                    );
                });

                ui.add_space(12.0);
                self.render_investor_card_impl(ui);
            });
    }

    fn render_news_tab_impl(&mut self, ui: &mut egui::Ui, max_height: f32) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Latest changelog entries")
                    .size(18.0)
                    .strong()
                    .color(egui::Color32::from_rgb(235, 235, 235)),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_sized(
                        [150.0, 30.0],
                        egui::Button::new("Open changelog")
                            .fill(accent_fill())
                            .corner_radius(8.0)
                            .stroke(egui::Stroke::NONE),
                    )
                    .clicked()
                {
                    self.open_external_url(CHANGELOG_URL);
                }
            });
        });

        ui.add_space(10.0);

        if self.website_status.changelogs.is_empty() {
            render_card(ui, "News", |ui| {
                ui.label(
                    egui::RichText::new(if self.website_status_loading {
                        "Loading changelogs from the website..."
                    } else {
                        "No changelog entries were found on the website."
                    })
                    .size(14.0)
                    .color(egui::Color32::from_rgb(200, 205, 215)),
                );
            });
            return;
        }

        egui::ScrollArea::vertical()
            .id_salt("news-scroll-v2")
            .auto_shrink([false, false])
            .max_height((max_height - 52.0).max(180.0))
            .scroll_bar_visibility(
                egui::containers::scroll_area::ScrollBarVisibility::AlwaysVisible,
            )
            .show(ui, |ui| {
                for entry in &self.website_status.changelogs {
                    render_card(
                        ui,
                        &format!("{} - {} - {}", entry.date, entry.kind, entry.area),
                        |ui| {
                            ui.label(
                                egui::RichText::new(&entry.body)
                                    .size(14.0)
                                    .color(egui::Color32::from_rgb(220, 225, 232)),
                            );
                        },
                    );
                    ui.add_space(8.0);
                }
            });
    }

    fn render_boosts_card_impl(&mut self, ui: &mut egui::Ui) {
        let online_players = self
            .website_status
            .online_players
            .map(|count| format!("{} players", count));
        let creature_name = self.website_status.boosted_creature.clone();
        let boss_name = self.website_status.boosted_boss.clone();

        render_card_with_fixed_height(ui, "Today", TOP_STATUS_CARD_HEIGHT, |ui| {
            value_row(ui, "Online", online_players.as_deref());
            ui.add_space(8.0);

            ui.columns(2, |columns| {
                self.render_boosted_preview_tile_impl(
                    &mut columns[0],
                    "Creature",
                    creature_name.as_deref(),
                    BoostedPreviewKind::Creature,
                );
                self.render_boosted_preview_tile_impl(
                    &mut columns[1],
                    "Boss",
                    boss_name.as_deref(),
                    BoostedPreviewKind::Boss,
                );
            });
        });
    }

    fn render_boosted_preview_tile_impl(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        name: Option<&str>,
        kind: BoostedPreviewKind,
    ) {
        ui.vertical_centered(|ui| {
            let preview_size = egui::vec2(82.0, 82.0);
            let (rect, _) = preview_tile_rect(ui, preview_size.x);

            if let Some(frame) = self.current_boosted_preview_frame(kind, ui.ctx()) {
                let texture_size = frame.texture.size_vec2();
                let scale =
                    (preview_size.x / texture_size.x).min(preview_size.y / texture_size.y) * 0.92;
                let image_size = texture_size * scale;
                let image_rect = egui::Rect::from_center_size(rect.center(), image_size);
                ui.painter().image(
                    frame.texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                if self.boosted_preview_is_animated(kind) {
                    ui.ctx().request_repaint_after(Duration::from_millis(
                        preview_repaint_delay_ms(frame.delay_ms),
                    ));
                }
            } else {
                let text = if self.boosted_preview_is_loading(kind) {
                    "Loading"
                } else if self.boosted_preview_error(kind).is_some() {
                    "Preview"
                } else {
                    "Waiting"
                };
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(12.0),
                    egui::Color32::from_rgb(170, 180, 195),
                );
            }

            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(label)
                    .size(12.0)
                    .color(egui::Color32::from_rgb(160, 170, 185)),
            );
            ui.label(
                egui::RichText::new(name.unwrap_or("Loading..."))
                    .size(13.0)
                    .strong()
                    .color(egui::Color32::from_rgb(225, 230, 238)),
            );
        });
    }

    fn render_events_card_impl(&mut self, ui: &mut egui::Ui) {
        let active_event = self.website_status.active_events.first().cloned();
        let upcoming_event = self.website_status.upcoming_events.first().cloned();

        render_card_with_fixed_height(ui, "Events", TOP_STATUS_CARD_HEIGHT, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.link("Calendar").clicked() {
                        self.open_external_url(EVENT_CALENDAR_URL);
                    }
                });
            });

            ui.add_space(2.0);
            ui.columns(2, |columns| {
                render_event_summary_tile(
                    &mut columns[0],
                    "Active",
                    active_event.as_ref(),
                    "No active event.",
                );
                render_event_summary_tile(
                    &mut columns[1],
                    "Next upcoming",
                    upcoming_event.as_ref(),
                    "No upcoming event.",
                );
            });
        });
    }

    fn render_offer_card_impl(
        &mut self,
        ui: &mut egui::Ui,
        title: &str,
        offer: Option<&OfferSummary>,
        url: &str,
    ) {
        render_card(ui, title, |ui| {
            if let Some(offer) = offer {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&offer.title)
                            .size(16.0)
                            .strong()
                            .color(egui::Color32::from_rgb(235, 235, 235)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.link("Website").clicked() {
                            self.open_external_url(url);
                        }
                    });
                });

                if let Some(subtitle) = &offer.subtitle {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(subtitle)
                            .size(12.0)
                            .color(egui::Color32::from_rgb(190, 200, 215)),
                    );
                }

                if !offer.previews.is_empty() {
                    ui.add_space(8.0);
                    self.render_offer_previews_impl(ui, &offer.previews);
                }

                ui.add_space(8.0);
                for (label, value) in offer.facts.iter().take(4) {
                    value_row(ui, label, Some(value));
                }
            } else {
                ui.label(
                    egui::RichText::new(if self.website_status_loading {
                        "Loading from website..."
                    } else {
                        "No website data available."
                    })
                    .size(13.0)
                    .color(egui::Color32::from_rgb(200, 205, 215)),
                );
            }
        });
    }

    fn render_offer_previews_impl(&mut self, ui: &mut egui::Ui, previews: &[OfferPreview]) {
        let spacing = 8.0;
        let large_previews = previews
            .iter()
            .filter(|preview| preview.tile_size > 64.0)
            .collect::<Vec<_>>();
        let item_previews = previews
            .iter()
            .filter(|preview| preview.tile_size <= 64.0)
            .collect::<Vec<_>>();

        if !large_previews.is_empty() {
            self.render_offer_preview_grid_impl(ui, &large_previews, spacing, 3);
        }

        if !large_previews.is_empty() && !item_previews.is_empty() {
            ui.add_space(8.0);
        }

        if !item_previews.is_empty() {
            self.render_offer_preview_grid_impl(ui, &item_previews, spacing, 5);
        }
    }

    fn render_offer_preview_grid_impl(
        &mut self,
        ui: &mut egui::Ui,
        previews: &[&OfferPreview],
        spacing: f32,
        max_columns: usize,
    ) {
        let tile_size = previews
            .first()
            .map(|preview| preview.tile_size)
            .unwrap_or(64.0);
        let columns = ((ui.available_width() + spacing) / (tile_size + spacing))
            .floor()
            .clamp(1.0, max_columns as f32) as usize;

        egui::Grid::new(ui.next_auto_id())
            .num_columns(columns)
            .spacing(egui::vec2(spacing, spacing))
            .show(ui, |ui| {
                for (index, preview) in previews.iter().enumerate() {
                    self.render_offer_preview_tile_impl(ui, preview);
                    if (index + 1) % columns == 0 {
                        ui.end_row();
                    }
                }
            });
    }

    fn render_offer_preview_tile_impl(&mut self, ui: &mut egui::Ui, preview: &OfferPreview) {
        let tile_size = preview.tile_size;
        let (rect, response) = preview_tile_rect(ui, tile_size);

        if let Some(frame) = self.offer_preview_frame(&preview.url, ui.ctx()) {
            let texture_size = frame.texture.size_vec2();
            let display_size = preview.display_size.min(tile_size);
            let scale = (display_size / texture_size.x).min(display_size / texture_size.y);
            let image_size = texture_size * scale;
            let image_rect = egui::Rect::from_center_size(rect.center(), image_size);
            ui.painter().image(
                frame.texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            if self.offer_preview_is_animated(&preview.url) {
                ui.ctx()
                    .request_repaint_after(Duration::from_millis(preview_repaint_delay_ms(
                        frame.delay_ms,
                    )));
            }
        } else {
            let text = if self.offer_preview_is_loading(&preview.url) {
                "Loading"
            } else if self.offer_preview_error(&preview.url).is_some() {
                "Preview"
            } else {
                "Waiting"
            };
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(10.5),
                egui::Color32::from_rgb(170, 180, 195),
            );
        }

        response.on_hover_text(&preview.title);
    }

    fn render_investor_card_impl(&mut self, ui: &mut egui::Ui) {
        let investor = self.website_status.investor.clone();
        let is_loading = self.website_status_loading;

        render_card(ui, "Top Investor", |ui| {
            ui.horizontal(|ui| {
                if let Some(investor) = &investor {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(&investor.name)
                                .size(18.0)
                                .strong()
                                .color(egui::Color32::from_rgb(245, 225, 150)),
                        );
                        value_row(
                            ui,
                            "Invested",
                            Some(&format!("{} Tibia Coins", investor.invested)),
                        );
                        value_row(
                            ui,
                            "Daily coins",
                            Some(&format!("{} Tibia Coins", investor.daily_return)),
                        );
                        value_row(ui, "Next round", investor.remaining.as_deref());
                    });
                } else {
                    ui.label(
                        egui::RichText::new(if is_loading {
                            "Loading investor ranking from the website..."
                        } else {
                            "No active investor found for this cycle."
                        })
                        .size(14.0)
                        .color(egui::Color32::from_rgb(205, 210, 220)),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    if ui
                        .add_sized(
                            [150.0, 30.0],
                            egui::Button::new("Investment")
                                .fill(panel_fill(220))
                                .corner_radius(8.0)
                                .stroke(egui::Stroke::NONE),
                        )
                        .clicked()
                    {
                        self.open_external_url(INVESTMENT_URL);
                    }
                });
            });
        });
    }

    fn render_external_links_impl(&mut self, ui: &mut egui::Ui) {
        centered_fixed_row(ui, 272.0, |ui| {
            if ui
                .add_sized(
                    [132.0, 32.0],
                    egui::Button::new("Website")
                        .fill(panel_fill(220))
                        .corner_radius(8.0)
                        .stroke(egui::Stroke::NONE),
                )
                .clicked()
            {
                self.open_external_url(WEBSITE_BASE_URL);
            }

            if ui
                .add_sized(
                    [132.0, 32.0],
                    egui::Button::new("Discord")
                        .fill(accent_fill())
                        .corner_radius(8.0)
                        .stroke(egui::Stroke::NONE),
                )
                .clicked()
            {
                self.open_external_url(DISCORD_URL);
            }
        });
    }

    fn render_launch_buttons_compact_impl(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let button_width = 250.0;
        let button_height = 40.0;

        let (has_main_client, additional_count) = self.game_client.sync_client_state();
        let has_any_client = has_main_client || additional_count > 0;

        if self.is_processing {
            return;
        }

        if has_any_client {
            if primary_button(ui, "Abrir Outro Cliente", button_width, button_height).clicked() {
                if let Err(error) = self.launch_client() {
                    self.status = format!("Erro ao iniciar o cliente: {}", error);
                }
            }

            ui.add_space(8.0);

            if secondary_button(ui, "Play OTClient", button_width, button_height).clicked() {
                if let Err(error) = self.prepare_otclient(ctx) {
                    self.status = format!("Erro ao preparar OTClient: {}", error);
                    self.is_processing = false;
                }
            }
        } else {
            if primary_button(ui, "Play Client 15.23", button_width, button_height).clicked() {
                if let Err(error) = self.launch_game(ctx) {
                    self.status = format!("Erro ao iniciar o jogo: {}", error);
                }
            }

            ui.add_space(8.0);

            if secondary_button(ui, "Play OTClient", button_width, button_height).clicked() {
                if let Err(error) = self.prepare_otclient(ctx) {
                    self.status = format!("Erro ao preparar OTClient: {}", error);
                    self.is_processing = false;
                }
            }
        }
    }

    fn trigger_force_update(&mut self, ctx: &egui::Context) {
        let (has_main, additional_count) = self.game_client.sync_client_state();
        if has_main || additional_count > 0 {
            self.status = "Feche todos os clientes antes de usar Force Update".to_string();
            self.temp_message_time = Some(std::time::Instant::now());
            self.is_alert_message = true;
            ctx.request_repaint();
        } else {
            self.show_force_update_modal = true;
        }
    }

    fn render_utility_buttons_compact_impl(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        centered_fixed_row(ui, 250.0, |ui| {
            let mut disable_auto_start = self.disable_auto_start;
            if ui
                .checkbox(
                    &mut disable_auto_start,
                    egui::RichText::new("Desativar inicio automatico")
                        .color(egui::Color32::from_rgb(185, 190, 200))
                        .size(13.0),
                )
                .changed()
            {
                self.disable_auto_start = disable_auto_start;
                if let Err(error) = self.save_user_settings() {
                    info!("Erro ao salvar configuracoes: {}", error);
                }
            }
        });

        ui.add_space(6.0);

        centered_fixed_row(ui, 122.0, |ui| {
            if small_button(ui, "Limpar Cache").clicked() {
                self.start_cache_clean(ctx);
            }
        });
    }

    fn start_full_minimap_download(&mut self, ctx: &egui::Context) {
        if self.is_processing {
            self.status = "Aguarde a operacao atual terminar".to_string();
            self.temp_message_time = Some(std::time::Instant::now());
            self.is_alert_message = true;
            ctx.request_repaint();
            return;
        }

        let (tx, rx) = unbounded_channel();
        self.message_receiver = Some(rx);
        self.status = "Baixando full map...".to_string();
        self.is_processing = true;
        self.progress = 0.0;
        ctx.request_repaint();

        let download_path = self.download_path.clone();
        let game_path = self.game_path.clone();

        tokio::spawn(async move {
            match crate::full_map::download_and_install_full_minimap(
                download_path,
                game_path,
                tx.clone(),
            )
            .await
            {
                Ok(stats) => {
                    info!(
                        "Full map instalado com sucesso: {} arquivos, {} bytes",
                        stats.files, stats.bytes
                    );
                }
                Err(error) => {
                    info!("Erro durante download do full map: {}", error);
                    let _ = tx.send(LauncherMessage::SetStatus(format!(
                        "Erro ao baixar full map: {}",
                        error
                    )));
                    let _ = tx.send(LauncherMessage::SetProcessing(false));
                }
            }
        });
    }

    fn start_cache_clean(&mut self, ctx: &egui::Context) {
        let (tx, rx) = unbounded_channel();
        self.message_receiver = Some(rx);
        self.status = "Limpando cache...".to_string();
        self.is_processing = true;
        self.progress = 0.0;
        ctx.request_repaint();

        let download_path = self.download_path.clone();
        let game_path = self.game_path.clone();
        let state_path = self.state_path.clone();
        let cache_manager = cache::CacheManager::new(download_path, game_path, state_path);

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

    pub fn render_background_impl(&self, ui: &mut egui::Ui) {
        if let Some(texture) = &self.background_texture {
            let available_rect = ui.max_rect();
            let time = ui.input(|input| input.time) as f32;
            let drift_x = (time * 0.12).sin() * 10.0;
            let drift_y = (time * 0.09).cos() * 8.0;
            let pulse = ((time * 0.7).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
            let expanded_rect = available_rect
                .expand(22.0)
                .translate(egui::vec2(drift_x, drift_y));

            ui.painter().image(
                texture.id(),
                expanded_rect,
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0)),
                egui::Color32::WHITE,
            );

            let veil_alpha = (24.0 + pulse * 18.0) as u8;
            ui.painter().rect_filled(
                available_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(86, 24, 130, veil_alpha),
            );

            ui.painter().rect_filled(
                available_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(3, 7, 14, 214),
            );

            ui.ctx()
                .request_repaint_after(Duration::from_millis(BACKGROUND_REPAINT_MS));
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

                    let folder = self.game_path.display().to_string();
                    let folder_label = compact_path_label(&folder, 54);
                    ui.add(egui::Label::new(
                        egui::RichText::new(format!("Folder: {}", folder_label))
                            .size(12.0)
                            .color(egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                    ))
                    .on_hover_text(folder);
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
                    } else if self.ping_in_progress {
                        ui.add(egui::Label::new(
                            egui::RichText::new("Ping: verificando...")
                                .size(12.0)
                                .color(egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                        ));
                    } else if self.last_ping_check.is_some() {
                        ui.add(egui::Label::new(
                            egui::RichText::new("Ping: offline")
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
                    ctx.request_repaint_after(Duration::from_millis(STATUS_REPAINT_MS));
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

        let (has_main_client, additional_count) = self.game_client.sync_client_state();
        let has_additional_clients = additional_count > 0;
        let has_any_client = has_main_client || has_additional_clients;

        if self.is_processing {
        } else if has_any_client {
            ui.horizontal(|ui| {
                ui.add_space(indent);

                if ui
                    .add_sized(
                        [button_width, button_height],
                        egui::Button::new(
                            egui::RichText::new("Abrir Outro Cliente").size(15.0).color(
                                if ui.ui_contains_pointer() {
                                    egui::Color32::BLACK
                                } else {
                                    egui::Color32::WHITE
                                },
                            ),
                        )
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
                    if let Err(error) = self.launch_client() {
                        self.status = format!("Erro ao iniciar o cliente: {}", error);
                    }
                }

                ui.add_space(10.0);

                if ui
                    .add_sized(
                        [button_width, button_height],
                        egui::Button::new(egui::RichText::new("Play OTClient").size(15.0).color(
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
                    if let Err(error) = self.prepare_otclient(ctx) {
                        self.status = format!("Erro ao preparar OTClient: {}", error);
                        self.is_processing = false;
                    }
                }
            });
        } else {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    if ui
                        .add_sized(
                            [button_width, button_height],
                            egui::Button::new(
                                egui::RichText::new("Play Client 15.23").size(18.0).color(
                                    if ui.ui_contains_pointer() {
                                        egui::Color32::BLACK
                                    } else {
                                        egui::Color32::WHITE
                                    },
                                ),
                            )
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

                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    if ui
                        .add_sized(
                            [button_width, button_height],
                            egui::Button::new(
                                egui::RichText::new("Play OTClient").size(18.0).color(
                                    if ui.ui_contains_pointer() {
                                        egui::Color32::BLACK
                                    } else {
                                        egui::Color32::WHITE
                                    },
                                ),
                            )
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
                        if let Err(error) = self.prepare_otclient(ctx) {
                            self.status = format!("Erro ao preparar OTClient: {}", error);
                            self.is_processing = false;
                        }
                    }
                });
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
        let help_text_height = 20.0;
        ui.add_space((available_height - button_height - help_text_height - 1.0).max(0.0));

        let (has_main, additional_count) = self.game_client.sync_client_state();
        let has_clients = has_main || additional_count > 0;
        if !self.is_processing {
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("In case of crash, \"Force Update\"")
                        .size(12.0)
                        .color(egui::Color32::from_rgb(190, 190, 190)),
                );
            });
            ui.add_space(4.0);

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

                if ui
                    .add_sized(
                        [180.0, 30.0],
                        egui::Button::new(
                            egui::RichText::new("Minimizar/Restaurar clientes")
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
                    self.open_minimize_client_selector(ctx);
                }

                if ui
                    .add_sized(
                        [130.0, 30.0],
                        egui::Button::new(
                            egui::RichText::new("Force Update")
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
                    if has_clients {
                        self.status =
                            "Feche todos os clientes antes de usar Force Update".to_string();
                        self.temp_message_time = Some(std::time::Instant::now());
                        self.is_alert_message = true;
                        ctx.request_repaint();
                    } else {
                        self.show_force_update_modal = true;
                    }
                }

                if ui
                    .add_sized(
                        [140.0, 30.0],
                        egui::Button::new(
                            egui::RichText::new("Update Launcher")
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
                    self.start_launcher_update(ctx);
                }
            });

            ui.add_space(6.0);

            ui.horizontal_centered(|ui| {
                if ui
                    .add_sized(
                        [150.0, 30.0],
                        egui::Button::new(
                            egui::RichText::new("Client Folder")
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
                    self.select_install_folder(ctx);
                }

                if ui
                    .add_sized(
                        [130.0, 30.0],
                        egui::Button::new(
                            egui::RichText::new("Full Map")
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
                    self.start_full_minimap_download(ctx);
                }
            });
        }

        if !has_main && additional_count == 0 && !self.is_processing {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::BOTTOM), |ui| {
                    ui.add_space(10.0);
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
                    if let Err(error) = self.save_user_settings() {
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

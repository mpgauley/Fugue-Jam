mod library;
mod playback;
mod playlists;

use super::*;

impl eframe::App for MusicApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.check_playback_end();
        self.finish_scan_if_ready();
        self.finish_artwork_loads(ctx);
        self.finish_waveform_loads();
        egui::TopBottomPanel::top("navigation").show(ctx, |ui| {
            let language = self.data.language;
            let mut selected_language = None;
            ui.horizontal(|ui| {
                ui.heading(format!("Fugue Jam · v{PROGRAM_VERSION}"));
                ui.separator();
                for tab in [
                    Tab::NowPlaying,
                    Tab::Library,
                    Tab::Playlists,
                    Tab::Equalization,
                ] {
                    if ui
                        .selectable_label(self.tab == tab, tab.title(language))
                        .clicked()
                    {
                        self.tab = tab;
                    }
                }
                ui.menu_button(tr(language, "Language"), |ui| {
                    for option in Language::ALL {
                        if ui
                            .selectable_label(language == option, option.native_name())
                            .clicked()
                        {
                            selected_language = Some(option);
                            ui.close_menu();
                        }
                    }
                });
            });
            if let Some(language) = selected_language {
                self.data.language = language;
                self.save();
            }
        });
        egui::TopBottomPanel::bottom("player_controls").show(ctx, |ui| {
            let language = self.data.language;
            ui.horizontal(|ui| {
                if ui
                    .button("⏮")
                    .on_hover_text(tr(language, "Previous track"))
                    .clicked()
                {
                    self.previous_track();
                }
                let can_resume = self.paused || self.sink.is_none();
                if ui
                    .button(if can_resume { "▶" } else { "⏸" })
                    .on_hover_text(tr(language, if can_resume { "Play" } else { "Pause" }))
                    .clicked()
                {
                    self.play_pause();
                }
                if ui.button("⏹").on_hover_text(tr(language, "Stop")).clicked() {
                    self.stop_button();
                }
                if ui
                    .button("⏭")
                    .on_hover_text(tr(language, "Next track"))
                    .clicked()
                {
                    self.next_track();
                }
                ui.separator();
                ui.label(if self.current_title.is_empty() {
                    tr(language, "Nothing playing").to_owned()
                } else {
                    self.current_title.clone()
                });
                if let Some(sink) = &self.sink {
                    let elapsed = sink.get_pos();
                    ui.label(format!(
                        "{} / {}",
                        format_duration(elapsed),
                        self.track_duration
                            .map(format_duration)
                            .unwrap_or_else(|| "—:—".to_owned())
                    ));
                }
            });
            ui.horizontal(|ui| {
                ui.label(&self.status);
                if let Some(task) = &self.scan_task {
                    let total = task.total.load(Ordering::Relaxed);
                    let progress = task.progress.load(Ordering::Relaxed);
                    let fraction = if total == 0 {
                        0.0
                    } else {
                        progress as f32 / total as f32
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::ProgressBar::new(fraction).desired_width(210.0).text(
                            if total == 0 {
                                if task.smart_playlist.is_some() {
                                    tr(language, "Scanning library for smart playlist…").to_owned()
                                } else {
                                    tr(language, "Finding files…").to_owned()
                                }
                            } else {
                                format!("{progress}/{total} {}", tr(language, "files"))
                            },
                        ));
                    });
                }
            });
        });
        egui::TopBottomPanel::bottom("track_timeline")
            .exact_height((ctx.screen_rect().height() * 0.10).clamp(56.0, 128.0))
            .show(ctx, |ui| {
                let language = self.data.language;
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), ui.available_height()),
                    egui::Sense::click(),
                );
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 4.0, ui.visuals().faint_bg_color);
                let position = self.sink.as_ref().map(Sink::get_pos).unwrap_or_default();
                let duration = self.track_duration;
                let artwork_path = self
                    .queue_index
                    .and_then(|index| self.data.queue.get(index))
                    .and_then(|entry| {
                        self.library_indices
                            .get(&entry.path)
                            .and_then(|index| self.data.tracks.get(*index))
                    })
                    .and_then(|track| track.artwork_path.clone());
                let artwork = self.load_artwork(artwork_path.as_deref());
                let artwork_size = (rect.height() - 12.0).clamp(32.0, 96.0);
                let artwork_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 8.0, rect.center().y - artwork_size * 0.5),
                    egui::vec2(artwork_size, artwork_size),
                );
                if let Some(texture) = artwork {
                    painter.image(
                        texture.id(),
                        artwork_rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                } else {
                    painter.rect_filled(
                        artwork_rect,
                        4.0,
                        ui.visuals().widgets.noninteractive.bg_fill,
                    );
                    painter.text(
                        artwork_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "♫",
                        egui::TextStyle::Heading.resolve(ui.style()),
                        ui.visuals().text_color(),
                    );
                }
                let fraction = duration
                    .filter(|duration| !duration.is_zero())
                    .map(|duration| {
                        (position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
                    })
                    .unwrap_or(0.0);
                let label = format!(
                    "{}  {} / {}",
                    if self.current_title.is_empty() {
                        tr(language, "Nothing playing")
                    } else {
                        &self.current_title
                    },
                    format_duration(position),
                    duration
                        .map(format_duration)
                        .unwrap_or_else(|| "—:—".to_owned())
                );
                painter.text(
                    egui::pos2(artwork_rect.right() + 10.0, rect.top() + 8.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::TextStyle::Body.resolve(ui.style()),
                    ui.visuals().text_color(),
                );
                let waveform_rect = egui::Rect::from_min_max(
                    egui::pos2(artwork_rect.right() + 10.0, rect.top() + 32.0),
                    egui::pos2(rect.right() - 8.0, rect.bottom() - 8.0),
                );
                let center = waveform_rect.center().y;
                let bar_count = self.waveform.len().max(1);
                let bar_width = waveform_rect.width() / bar_count as f32;
                for (index, peak) in self.waveform.iter().enumerate() {
                    let x = waveform_rect.left() + index as f32 * bar_width;
                    let half_height = peak.clamp(0.02, 1.0) * waveform_rect.height() * 0.48;
                    let color = if index as f32 / bar_count as f32 <= fraction {
                        ui.visuals().selection.bg_fill
                    } else {
                        ui.visuals().text_color().gamma_multiply(0.55)
                    };
                    painter.line_segment(
                        [
                            egui::pos2(x + bar_width * 0.5, center - half_height),
                            egui::pos2(x + bar_width * 0.5, center + half_height),
                        ],
                        egui::Stroke::new(bar_width.max(0.5), color),
                    );
                }
                if duration.is_some() {
                    let x = waveform_rect.left() + waveform_rect.width() * fraction;
                    painter.line_segment(
                        [
                            egui::pos2(x, waveform_rect.top()),
                            egui::pos2(x, waveform_rect.bottom()),
                        ],
                        egui::Stroke::new(2.0_f32, ui.visuals().selection.stroke.color),
                    );
                    if response.clicked()
                        && let Some(pointer) = response.interact_pointer_pos()
                    {
                        let seek_fraction = ((pointer.x - waveform_rect.left())
                            / waveform_rect.width())
                        .clamp(0.0, 1.0);
                        if let (Some(sink), Some(duration)) = (&self.sink, duration) {
                            let target = duration.mul_f32(seek_fraction);
                            match sink.try_seek(target) {
                                Ok(()) => {
                                    self.status = format!("Seeking to {}.", format_duration(target))
                                }
                                Err(error) => self.status = format!("Could not seek: {error}"),
                            }
                        }
                    }
                }
            });
        match self.tab {
            Tab::NowPlaying => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let available = ui.available_size();
                    let divider_width = 6.0;
                    let content_width = (available.x - divider_width).max(0.0);
                    let left_width = content_width * self.now_playing_left_fraction;
                    let split_left = ui.min_rect().left();
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.allocate_ui_with_layout(
                            egui::vec2(left_width, available.y),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| self.render_now_playing_cover(ui),
                        );
                        let (divider_rect, divider_response) = ui.allocate_exact_size(
                            egui::vec2(divider_width, available.y),
                            egui::Sense::drag(),
                        );
                        ui.painter().rect_filled(
                            divider_rect.shrink2(egui::vec2(2.0, 0.0)),
                            1.0,
                            ui.visuals().widgets.noninteractive.bg_stroke.color,
                        );
                        if divider_response.dragged()
                            && let Some(pointer) = divider_response.interact_pointer_pos()
                        {
                            self.now_playing_left_fraction =
                                ((pointer.x - split_left - divider_width * 0.5)
                                    / content_width.max(1.0))
                                .clamp(0.25, 0.9);
                        }
                        if divider_response.drag_stopped() {
                            self.save();
                        }
                        ui.allocate_ui_with_layout(
                            egui::vec2(content_width - left_width, available.y),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| self.render_queue(ui),
                        );
                    });
                });
            }
            Tab::Library => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        ui.set_width(ui.available_width());
                        self.render_library(ui);
                    });
            }
            Tab::Playlists => {
                egui::CentralPanel::default().show(ctx, |ui| self.render_playlists(ui));
            }
            Tab::Equalization => {
                egui::CentralPanel::default().show(ctx, |ui| self.render_equalization(ui));
            }
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

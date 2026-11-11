use super::super::*;

impl MusicApp {
    pub(super) fn render_queue(&mut self, ui: &mut egui::Ui) {
        let language = self.data.language;
        ui.heading(tr(language, "Now playing queue"));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if ui.button(tr(language, "Clear\nqueue")).clicked() {
                self.data.queue.clear();
                self.queue_index = None;
                self.stop();
            }
            if ui.button(tr(language, "Shuffle\nqueue")).clicked() {
                if let Some(current_index) = self
                    .queue_index
                    .filter(|index| *index < self.data.queue.len())
                {
                    let current = self.data.queue.remove(current_index);
                    self.data.queue.shuffle(&mut rand::rng());
                    self.data.queue.insert(0, current);
                    self.queue_index = Some(0);
                } else {
                    self.data.queue.shuffle(&mut rand::rng());
                }
                self.save();
            }
            if ui.button(tr(language, "Customize\ndisplay…")).clicked() {
                self.show_queue_display_options = true;
            }
        });
        let static_playlists: Vec<(usize, String)> = self
            .data
            .playlists
            .iter()
            .enumerate()
            .filter(|(_, playlist)| playlist.smart.is_none())
            .map(|(index, playlist)| (index, playlist.name.clone()))
            .collect();
        let mut action = None;
        let mut play_index = None;
        let queue_display_fields = self.data.queue_display_fields.clone();
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, self.data.queue.len(), |ui, rows| {
                for index in rows {
                    let entry = &self.data.queue[index];
                    let values: Vec<String> = queue_display_fields
                        .iter()
                        .filter_map(|field| entry.display_value(*field))
                        .collect();
                    let title = if values.is_empty() {
                        entry.title.as_str().to_owned()
                    } else {
                        values.join(" — ")
                    };
                    let marker = if self.queue_index == Some(index) {
                        "▶"
                    } else {
                        " "
                    };
                    let response = ui.add_sized(
                        [ui.available_width(), row_height],
                        egui::Label::new(format!("{marker}  {title}"))
                            .halign(egui::Align::Min)
                            .sense(egui::Sense::click()),
                    );
                    if response.clicked() {
                        play_index = Some(index);
                    }
                    response.context_menu(|ui| {
                        if ui.button(tr(language, "Remove Track")).clicked() {
                            action = Some((index, QueueAction::Remove));
                            ui.close_menu();
                        }
                        if ui.button(tr(language, "Play Track Next")).clicked() {
                            action = Some((index, QueueAction::PlayNext));
                            ui.close_menu();
                        }
                        if ui.button(tr(language, "Move Track to the End")).clicked() {
                            action = Some((index, QueueAction::MoveToEnd));
                            ui.close_menu();
                        }
                        ui.menu_button(tr(language, "Add Track to Playlist…"), |ui| {
                            if ui.button(tr(language, "Create new playlist…")).clicked() {
                                action = Some((index, QueueAction::CreatePlaylist));
                                ui.close_menu();
                            }
                            if static_playlists.is_empty() {
                                ui.label(tr(language, "No static playlists yet"));
                            }
                            for (playlist_index, name) in &static_playlists {
                                if ui.button(name).clicked() {
                                    action =
                                        Some((index, QueueAction::AddToPlaylist(*playlist_index)));
                                    ui.close_menu();
                                }
                            }
                        });
                    });
                }
            });
        if self.show_queue_display_options {
            let mut selected: Vec<bool> = QueueDisplayField::ALL
                .iter()
                .map(|field| self.data.queue_display_fields.contains(field))
                .collect();
            let mut display_changed = false;
            egui::Window::new(tr(language, "Customize queue display"))
                .open(&mut self.show_queue_display_options)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(tr(
                        language,
                        "Select the track information to show for each queued track.",
                    ));
                    for (index, field) in QueueDisplayField::ALL.iter().enumerate() {
                        display_changed |= ui
                            .checkbox(&mut selected[index], field.label(language))
                            .changed();
                    }
                    ui.label(tr(
                        language,
                        "When no selected tag has a value, the title is shown.",
                    ));
                });
            if display_changed {
                self.data.queue_display_fields = QueueDisplayField::ALL
                    .iter()
                    .zip(selected)
                    .filter_map(|(field, selected)| selected.then_some(*field))
                    .collect();
                for entry in &mut self.data.queue {
                    if let Some(track) = self
                        .library_indices
                        .get(&entry.path)
                        .and_then(|index| self.data.tracks.get(*index))
                    {
                        entry.update_fields(track, &self.data.queue_display_fields);
                    } else {
                        entry.retain_fields(&self.data.queue_display_fields);
                    }
                }
                self.save();
            }
        }
        if let Some(index) = play_index {
            self.play_queue_index(index);
        }
        if let Some((index, action)) = action {
            match action {
                QueueAction::Remove => self.remove_queue_track(index),
                QueueAction::PlayNext => self.play_queue_track_next(index),
                QueueAction::MoveToEnd => self.move_queue_track_to_end(index),
                QueueAction::AddToPlaylist(playlist_index) => {
                    if let Some(path) = self.data.queue.get(index).map(|entry| entry.path.clone()) {
                        self.add_queue_track_to_playlist(&path, playlist_index);
                    }
                }
                QueueAction::CreatePlaylist => {
                    self.pending_queue_playlist =
                        self.data.queue.get(index).map(|entry| entry.path.clone());
                    self.new_queue_playlist_name.clear();
                }
            }
        }
        if let Some(path) = self.pending_queue_playlist.clone() {
            let mut create = false;
            let mut cancel = false;
            egui::Window::new(tr(language, "Create playlist"))
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(tr(language, "Create a static playlist with this track."));
                    ui.text_edit_singleline(&mut self.new_queue_playlist_name);
                    ui.horizontal(|ui| {
                        if ui.button(tr(language, "Create")).clicked() {
                            create = true;
                        }
                        if ui.button(tr(language, "Cancel")).clicked() {
                            cancel = true;
                        }
                    });
                });
            if create {
                let name = self.new_queue_playlist_name.trim().to_owned();
                if name.is_empty() {
                    self.status = "Enter a name for the playlist.".to_owned();
                } else {
                    self.data.playlists.push(Playlist {
                        name: name.clone(),
                        tracks: vec![path],
                        smart: None,
                    });
                    self.active_playlist = Some(self.data.playlists.len() - 1);
                    self.save();
                    self.status = format!("Created playlist “{name}”.");
                    self.pending_queue_playlist = None;
                }
            }
            if cancel {
                self.pending_queue_playlist = None;
            }
        }
    }

    pub(super) fn render_equalization(&mut self, ui: &mut egui::Ui) {
        let language = self.data.language;
        ui.heading(tr(language, "Equalization"));
        ui.label(format!(
            "{} {} {}",
            self.eq_band_count,
            tr(language, "bands"),
            tr(language, "equalizer controls")
        ));
        let mut controls_changed = ui
            .checkbox(&mut self.eq_enabled, tr(language, "Enable equalizer"))
            .changed();
        egui::ComboBox::from_id_salt("eq_band_count")
            .selected_text(format!("{} {}", self.eq_band_count, tr(language, "bands")))
            .show_ui(ui, |ui| {
                for count in EQ_BAND_COUNTS {
                    if ui
                        .selectable_value(
                            &mut self.eq_band_count,
                            count,
                            format!("{count} {}", tr(language, "bands")),
                        )
                        .clicked()
                    {
                        self.eq_bands = remap_eq_gains(&self.eq_bands, count);
                        self.eq_preset = 0;
                        controls_changed = true;
                    }
                }
            });
        let presets = ["Custom", "Flat", "Bass boost", "Treble boost", "Vocal"];
        egui::ComboBox::from_id_salt("eq_preset")
            .selected_text(tr(language, presets[self.eq_preset.min(presets.len() - 1)]))
            .show_ui(ui, |ui| {
                for (index, preset) in presets.iter().enumerate() {
                    if ui
                        .selectable_value(&mut self.eq_preset, index, tr(language, preset))
                        .clicked()
                    {
                        controls_changed = true;
                        if index > 0 {
                            self.eq_bands = preset_eq_gains(index, self.eq_band_count);
                        }
                    }
                }
            });
        ui.add_space(16.0);
        let mut eq_changed = false;
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                for (index, frequency) in eq_frequencies(self.eq_band_count).iter().enumerate() {
                    let label = format_frequency(*frequency);
                    ui.vertical(|ui| {
                        ui.label(label);
                        eq_changed |= ui
                            .add_enabled(
                                self.eq_enabled,
                                egui::Slider::new(&mut self.eq_bands[index], -12.0..=12.0)
                                    .vertical()
                                    .show_value(true)
                                    .suffix(" dB"),
                            )
                            .changed();
                    });
                }
            });
        });
        if eq_changed {
            self.eq_preset = 0;
            controls_changed = true;
        }
        if controls_changed {
            self.eq_controls.update(&self.eq_bands, self.eq_enabled);
            self.save();
        }
        ui.separator();
        ui.label(tr(
            language,
            "Frequency range: 60 Hz–12 kHz for 5 bands; 30 Hz–18 kHz for 8 bands; 20 Hz–20 kHz for 12–31 bands.",
        ));
        ui.label(tr(
            language,
            "Equalizer changes apply immediately during playback.",
        ));
    }

    pub(super) fn render_now_playing_cover(&mut self, ui: &mut egui::Ui) {
        let language = self.data.language;
        ui.vertical_centered(|ui| {
            ui.heading(tr(language, "Now Playing"));
            ui.add_space(12.0);
            let artwork_path = self
                .queue_index
                .and_then(|index| self.data.queue.get(index))
                .and_then(|entry| {
                    self.data
                        .tracks
                        .iter()
                        .find(|track| track.path == entry.path)
                })
                .and_then(|track| track.artwork_path.clone());
            let title = if self.current_title.is_empty() {
                tr(language, "Nothing playing").to_owned()
            } else {
                self.current_title.clone()
            };
            let available = ui.available_size();
            let title_layout = ui.painter().layout(
                title.clone(),
                egui::TextStyle::Heading.resolve(ui.style()),
                ui.visuals().text_color(),
                available.x,
            );
            let image_height = (available.y - title_layout.size().y - 24.0).max(0.0);
            let image_size = available.x.min(image_height);
            if let Some(texture) = self.load_artwork(artwork_path.as_deref()) {
                ui.add(
                    egui::Image::from_texture(&texture)
                        .fit_to_exact_size(egui::vec2(image_size, image_size)),
                );
            } else {
                ui.add_sized([image_size, image_size], egui::Label::new("♫"));
            }
            ui.add_space(12.0);
            ui.heading(title);
        });
    }
}

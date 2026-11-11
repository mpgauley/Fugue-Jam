use super::super::*;

impl MusicApp {
    pub(super) fn render_playlists(&mut self, ui: &mut egui::Ui) {
        let language = self.data.language;
        self.ensure_playlist_load(ui.ctx());
        self.receive_playlist_rows(ui.ctx());
        let playlist_labels: Vec<(usize, String)> = self
            .data
            .playlists
            .iter()
            .enumerate()
            .map(|(index, playlist)| {
                let mut label = playlist.name.clone();
                if playlist.smart.is_some() {
                    label.push_str(&format!("  ·  {}", tr(language, "smart")));
                } else {
                    label.push_str(&format!(
                        "  ·  {} {}",
                        playlist.tracks.len(),
                        tr(language, "tracks")
                    ));
                }
                (index, label)
            })
            .collect();
        let mut selected_track = None;
        ui.columns(2, |columns| {
            columns[0].heading(tr(language, "Your playlists"));
            for (index, label) in &playlist_labels {
                if columns[0]
                    .selectable_label(self.active_playlist == Some(*index), label)
                    .clicked()
                {
                    self.active_playlist = Some(*index);
                }
            }
            columns[0].separator();
            columns[0].label(tr(language, "Static playlist from selected tracks"));
            columns[0].horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_playlist_name)
                        .hint_text(tr(language, "Playlist name")),
                );
                if ui.button(tr(language, "Create")).clicked() {
                    self.create_static_playlist();
                }
            });
            columns[0].separator();
            columns[0].label(tr(language, "Smart playlist conditions"));
            let mut remove_condition = None;
            for index in 0..self.smart_conditions.len() {
                columns[0].horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt(("smart_field", index))
                        .selected_text(tr(
                            language,
                            match self.smart_conditions[index].field {
                                SmartField::AlbumArtist => "Album artist",
                                SmartField::Album => "Album",
                                SmartField::Genre => "Genre",
                                SmartField::Year => "Year",
                            },
                        ))
                        .show_ui(ui, |ui| {
                            for (field, label) in [
                                (SmartField::AlbumArtist, "Album artist"),
                                (SmartField::Album, "Album"),
                                (SmartField::Genre, "Genre"),
                                (SmartField::Year, "Year"),
                            ] {
                                ui.selectable_value(
                                    &mut self.smart_conditions[index].field,
                                    field,
                                    tr(language, label),
                                );
                            }
                        });
                    egui::ComboBox::from_id_salt(("smart_comparison", index))
                        .selected_text(tr(
                            language,
                            match self.smart_conditions[index].comparison {
                                SmartMatch::Contains => "contains",
                                SmartMatch::DoesNotContain => "does not contain",
                            },
                        ))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.smart_conditions[index].comparison,
                                SmartMatch::Contains,
                                tr(language, "contains"),
                            );
                            ui.selectable_value(
                                &mut self.smart_conditions[index].comparison,
                                SmartMatch::DoesNotContain,
                                tr(language, "does not contain"),
                            );
                        });
                    ui.add(
                        egui::TextEdit::singleline(&mut self.smart_conditions[index].query)
                            .hint_text(tr(language, "Matching value"))
                            .desired_width(100.0),
                    );
                    egui::ComboBox::from_id_salt(("smart_open", index))
                        .selected_text(parenthesis_label(
                            language,
                            "Open",
                            self.smart_conditions[index].open_parentheses,
                        ))
                        .show_ui(ui, |ui| {
                            for count in 0..=3 {
                                ui.selectable_value(
                                    &mut self.smart_conditions[index].open_parentheses,
                                    count,
                                    parenthesis_label(language, "Open", count),
                                );
                            }
                        });
                    egui::ComboBox::from_id_salt(("smart_close", index))
                        .selected_text(parenthesis_label(
                            language,
                            "Close",
                            self.smart_conditions[index].close_parentheses,
                        ))
                        .show_ui(ui, |ui| {
                            for count in 0..=3 {
                                ui.selectable_value(
                                    &mut self.smart_conditions[index].close_parentheses,
                                    count,
                                    parenthesis_label(language, "Close", count),
                                );
                            }
                        });
                    if index < self.smart_operators.len() {
                        egui::ComboBox::from_id_salt(("smart_operator", index))
                            .selected_text(tr(
                                language,
                                match self.smart_operators[index] {
                                    SmartOperator::And => "And",
                                    SmartOperator::Or => "Or",
                                },
                            ))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.smart_operators[index],
                                    SmartOperator::And,
                                    tr(language, "And"),
                                );
                                ui.selectable_value(
                                    &mut self.smart_operators[index],
                                    SmartOperator::Or,
                                    tr(language, "Or"),
                                );
                            });
                    }
                    if self.smart_conditions.len() > 1
                        && ui
                            .small_button(tr(language, "Remove"))
                            .on_hover_text(tr(language, "Remove condition"))
                            .clicked()
                    {
                        remove_condition = Some(index);
                    }
                });
            }
            if let Some(index) = remove_condition {
                self.smart_conditions.remove(index);
                let operator_index = if index < self.smart_operators.len() {
                    index
                } else {
                    self.smart_operators.len().saturating_sub(1)
                };
                if !self.smart_operators.is_empty() {
                    self.smart_operators.remove(operator_index);
                }
            }
            if columns[0].button(tr(language, "Add condition")).clicked() {
                if !self.smart_conditions.is_empty() {
                    self.smart_operators.push(SmartOperator::And);
                }
                self.smart_conditions.push(SmartCondition::default());
            }
            columns[0].horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.smart_playlist_name)
                        .hint_text(tr(language, "Playlist name")),
                );
                if ui
                    .add_enabled(
                        self.scan_task.is_none(),
                        egui::Button::new(tr(language, "Create smart playlist")),
                    )
                    .clicked()
                {
                    self.create_smart_playlist();
                }
            });

            let active_index = self.active_playlist;
            let playlist_loading = self
                .playlist_load
                .as_ref()
                .filter(|task| Some(task.playlist_index) == active_index);
            let loaded_rows = playlist_loading
                .map(|task| Arc::clone(&task.rows))
                .unwrap_or_else(|| Arc::new(Vec::new()));
            let loading_finished = playlist_loading.is_some_and(|task| task.finished);
            let loading_error = playlist_loading.and_then(|task| task.error.clone());
            let active_name = active_index
                .and_then(|index| self.data.playlists.get(index))
                .map(|playlist| playlist.name.clone())
                .unwrap_or_else(|| tr(language, "Playlist tracks").to_owned());
            columns[1].heading(active_name);
            columns[1].horizontal(|ui| {
                if ui
                    .add_enabled(
                        loading_finished && loading_error.is_none(),
                        egui::Button::new(tr(language, "Copy to queue")),
                    )
                    .clicked()
                {
                    self.copy_playlist_to_queue(false);
                }
                if ui
                    .add_enabled(
                        loading_finished && loading_error.is_none(),
                        egui::Button::new(tr(language, "Copy to queue randomly")),
                    )
                    .clicked()
                {
                    self.copy_playlist_to_queue(true);
                }
            });
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(&mut columns[1], 24.0, loaded_rows.len(), |ui, row_range| {
                    for index in row_range {
                        let row = &loaded_rows[index];
                        if ui
                            .selectable_label(false, format!("{}. {}", index + 1, row.title))
                            .clicked()
                        {
                            selected_track = Some(row.path.clone());
                        }
                    }
                });
            if loaded_rows.is_empty() {
                if active_index.is_none() {
                    columns[1].label(tr(language, "Select a playlist to see its tracks."));
                } else if let Some(error) = loading_error {
                    columns[1].label(format!(
                        "{}: {error}",
                        tr(language, "Could not load playlist")
                    ));
                } else if loading_finished {
                    columns[1].label(tr(language, "This playlist has no tracks."));
                } else {
                    columns[1].label(tr(language, "Loading playlist…"));
                }
            } else if let Some(error) = loading_error {
                columns[1].label(format!(
                    "{}: {error}",
                    tr(language, "Could not load playlist")
                ));
            } else if !loading_finished {
                columns[1].label(format!(
                    "{} {} {}",
                    tr(language, "Loading playlist…"),
                    loaded_rows.len(),
                    tr(language, "tracks loaded")
                ));
            }
        });
        if self.active_playlist != self.playlist_load.as_ref().map(|task| task.playlist_index) {
            self.ensure_playlist_load(ui.ctx());
            self.receive_playlist_rows(ui.ctx());
        }
        if let Some(path) = selected_track {
            let entry = self.queue_entry(path);
            self.data.queue.push(entry);
            self.save();
            self.play_queue_index(self.data.queue.len() - 1);
        }
    }
}

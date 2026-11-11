use super::super::*;

impl MusicApp {
    pub(super) fn render_library(&mut self, ui: &mut egui::Ui) {
        let previous_library_view = self.library_view;
        let language = self.data.language;
        let group_by = match self.organize_by.as_str() {
            "Album Artist" => "Album Artist",
            "Genre" => "Genre",
            "Decade" => "Decade",
            _ => "No Grouping",
        };
        let sort_by = match self.sort_by.as_str() {
            "Album Artist" => "Album Artist",
            "Year" => "Year",
            _ => "Album Name",
        };
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.scan_task.is_none(),
                    egui::Button::new(tr(language, "Add music folder…")),
                )
                .clicked()
            {
                self.add_folder();
            }
            if ui
                .add_enabled(
                    self.scan_task.is_none() && !self.data.folders.is_empty(),
                    egui::Button::new(tr(language, "Scan for changes…")),
                )
                .on_hover_text(tr(
                    language,
                    "Choose a folder to discover new files and remove missing files",
                ))
                .clicked()
            {
                self.rescan_folder();
            }
            if ui
                .add_enabled(
                    self.scan_task.is_none() && !self.data.folders.is_empty(),
                    egui::Button::new(tr(language, "Rescan all metadata")),
                )
                .on_hover_text(tr(
                    language,
                    "Refresh tags for all tracks in saved music folders",
                ))
                .clicked()
            {
                self.rescan_all_metadata();
            }
            ui.add_enabled_ui(self.scan_task.is_none(), |ui| {
                ui.checkbox(&mut self.scan_subfolders, tr(language, "Scan subfolders"));
            });
            ui.label(format!(
                "{} {}",
                self.data.tracks.len(),
                tr(language, "tracks")
            ));
        });
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("group_by")
                .selected_text(format!(
                    "{} {}",
                    tr(language, "Group By:"),
                    tr(language, group_by)
                ))
                .show_ui(ui, |ui| {
                    for mode in ["Album Artist", "Genre", "Decade", "No Grouping"] {
                        ui.selectable_value(
                            &mut self.organize_by,
                            mode.to_owned(),
                            tr(language, mode),
                        );
                    }
                });
            egui::ComboBox::from_id_salt("display")
                .selected_text(format!(
                    "{} {}",
                    tr(language, "Display:"),
                    tr(
                        language,
                        match self.library_view {
                            LibraryView::Album => "Album",
                            LibraryView::AlbumArtist => "Album Artist",
                            LibraryView::Tracks => "Tracks",
                        }
                    )
                ))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.library_view,
                        LibraryView::Album,
                        tr(language, "Album"),
                    );
                    ui.selectable_value(
                        &mut self.library_view,
                        LibraryView::Tracks,
                        tr(language, "Tracks"),
                    );
                });
            egui::ComboBox::from_id_salt("sort_by")
                .selected_text(format!(
                    "{} {}",
                    tr(language, "Sort By:"),
                    tr(language, sort_by)
                ))
                .show_ui(ui, |ui| {
                    for mode in ["Album Name", "Album Artist", "Year"] {
                        ui.selectable_value(&mut self.sort_by, mode.to_owned(), tr(language, mode));
                    }
                });
            ui.add(
                egui::Slider::new(&mut self.thumbnail_size, 72.0..=240.0)
                    .text(tr(language, "Thumbnail size"))
                    .suffix(" px"),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text(tr(language, "Search title, artist, album, genre…"))
                        .desired_width(220.0),
                );
            });
        });
        ui.horizontal(|ui| {
            if ui.button(tr(language, "Add selected to queue")).clicked() {
                self.add_selected_to_queue(false);
            }
            if ui.button(tr(language, "Add selected randomly")).clicked() {
                self.add_selected_to_queue(true);
            }
            if ui.button(tr(language, "Play selection")).clicked() {
                self.play_selected_track();
            }
            if ui.button(tr(language, "Select all visible")).clicked() {
                self.selected_paths
                    .extend(self.visible_tracks().iter().map(|track| track.path.clone()));
            }
            if ui.button(tr(language, "Clear selection")).clicked() {
                self.selected_paths.clear();
            }
        });
        if self.library_view != previous_library_view {
            self.save();
        }
        let tracks = self.visible_tracks();
        if tracks.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(tr(
                    language,
                    "Your library is empty. Add a folder containing music files.",
                ));
            });
        } else if self.library_view != LibraryView::Tracks {
            self.render_album_grid(ui, &tracks);
        } else {
            self.render_track_list(ui, &tracks);
        }
    }

    fn render_track_list(&mut self, ui: &mut egui::Ui, tracks: &[Track]) {
        let language = self.data.language;
        ui.horizontal(|ui| {
            ui.add_sized([48.0, 20.0], egui::Label::new(""));
            ui.add_sized([210.0, 20.0], egui::Label::new(tr(language, "Track")));
            ui.add_sized(
                [140.0, 20.0],
                egui::Label::new(tr(language, "Album artist")),
            );
            ui.add_sized([150.0, 20.0], egui::Label::new(tr(language, "Album")));
            ui.add_sized([48.0, 20.0], egui::Label::new(tr(language, "Year")));
            ui.add_sized([58.0, 20.0], egui::Label::new(tr(language, "Type")));
            ui.add_sized([55.0, 20.0], egui::Label::new(tr(language, "Duration")));
        });
        ui.separator();
        let icon_size = (self.thumbnail_size * 0.28).clamp(24.0, 80.0);
        let row_height = icon_size + 24.0;
        egui::ScrollArea::vertical()
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show_rows(ui, row_height, tracks.len(), |ui, row_range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for index in row_range {
                    let track = &tracks[index];
                    let group = track_group_value(track, &self.organize_by);
                    let is_first_in_group = self.organize_by != "No Grouping"
                        && (index == 0
                            || group != track_group_value(&tracks[index - 1], &self.organize_by));
                    let group_label = if group.is_empty() {
                        tr(language, "Unknown")
                    } else {
                        &group
                    };
                    let path = track.path.clone();
                    let texture = self.load_artwork(track.artwork_path.as_deref());
                    let row_response = ui
                        .vertical(|ui| {
                            if is_first_in_group {
                                ui.add_sized(
                                    [320.0, 16.0],
                                    egui::Label::new(egui::RichText::new(group_label).small()),
                                );
                            } else {
                                ui.add_space(16.0);
                            }
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                if let Some(texture) = texture {
                                    ui.add(
                                        egui::Image::from_texture(&texture)
                                            .fit_to_exact_size(egui::vec2(icon_size, icon_size)),
                                    );
                                } else {
                                    ui.add_sized([icon_size, icon_size], egui::Label::new("♫"));
                                }
                                ui.add_sized(
                                    [210.0, icon_size],
                                    egui::Label::new(track.display_title()),
                                );
                                ui.add_sized(
                                    [140.0, icon_size],
                                    egui::Label::new(&track.album_artist),
                                );
                                ui.add_sized([150.0, icon_size], egui::Label::new(&track.album));
                                ui.add_sized(
                                    [48.0, icon_size],
                                    egui::Label::new(
                                        track.year.map(|year| year.to_string()).unwrap_or_default(),
                                    ),
                                );
                                ui.add_sized(
                                    [58.0, icon_size],
                                    egui::Label::new(track.file_type()),
                                );
                                ui.add_sized(
                                    [55.0, icon_size],
                                    egui::Label::new(track.duration_label()),
                                );
                            })
                            .response
                        })
                        .response;
                    let response = ui.interact(
                        row_response.rect,
                        ui.id().with(("track-row", &path)),
                        egui::Sense::click(),
                    );
                    if response.clicked() {
                        if !self.selected_paths.remove(&path) {
                            self.selected_paths.insert(path.clone());
                        }
                    }
                    self.show_track_context_menu(&response, std::slice::from_ref(&path));
                }
            });
    }

    fn render_album_grid(&mut self, ui: &mut egui::Ui, tracks: &[Track]) {
        let language = self.data.language;
        let items = gallery_items(tracks, self.library_view, &self.organize_by, &self.sort_by);
        let mut grouped = std::collections::BTreeMap::<String, Vec<GalleryItem>>::new();
        for item in items {
            let group = if self.organize_by == "No Grouping" {
                String::new()
            } else {
                item.group.clone()
            };
            grouped.entry(group).or_default().push(item);
        }

        let columns = ((ui.available_width() - ui.spacing().scroll.bar_width - 10.0)
            / (self.thumbnail_size + ui.spacing().item_spacing.x))
            .floor()
            .max(1.0) as usize;
        let mut rows = Vec::new();
        for (group, items) in grouped {
            if !group.is_empty() {
                rows.push(GalleryRow::Heading(group));
            }
            for chunk in items.chunks(columns) {
                rows.push(GalleryRow::Items(chunk.to_vec()));
            }
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show_rows(
                ui,
                self.thumbnail_size + ui.spacing().item_spacing.y,
                rows.len(),
                |ui, row_range| {
                    for row_index in row_range {
                        match &rows[row_index] {
                            GalleryRow::Heading(group) => {
                                ui.vertical(|ui| {
                                    ui.heading(group);
                                    ui.add_space((self.thumbnail_size - 16.0).max(0.0));
                                });
                            }
                            GalleryRow::Items(items) => {
                                ui.horizontal(|ui| {
                                    for item in items {
                                        let size = self.thumbnail_size;
                                        let selected = item
                                            .tracks
                                            .iter()
                                            .any(|path| self.selected_paths.contains(path));
                                        let texture =
                                            self.load_artwork(item.artwork_path.as_deref());
                                        let artwork_failed = item
                                            .artwork_path
                                            .as_ref()
                                            .is_some_and(|path| self.artwork_failed.contains(path));
                                        let stroke = if selected {
                                            egui::Stroke::new(
                                                2.0_f32,
                                                inverted_color(
                                                    ui.visuals().widgets.noninteractive.bg_fill,
                                                ),
                                            )
                                        } else {
                                            ui.visuals().widgets.noninteractive.bg_stroke
                                        };
                                        let (rect, _allocated_response) = ui.allocate_exact_size(
                                            egui::vec2(size, size),
                                            egui::Sense::click(),
                                        );
                                        if let Some(texture) = texture {
                                            ui.painter().image(
                                                texture.id(),
                                                rect.shrink(1.0),
                                                egui::Rect::from_min_max(
                                                    egui::Pos2::ZERO,
                                                    egui::pos2(1.0, 1.0),
                                                ),
                                                egui::Color32::WHITE,
                                            );
                                        } else {
                                            ui.painter().text(
                                                rect.center(),
                                                egui::Align2::CENTER_CENTER,
                                                if item.artwork_path.is_some() && !artwork_failed {
                                                    "…"
                                                } else if item.label.is_empty() {
                                                    tr(language, "Unknown")
                                                } else {
                                                    &item.label
                                                },
                                                egui::FontId::proportional(16.0),
                                                ui.visuals().text_color(),
                                            );
                                        }
                                        ui.painter().rect_stroke(
                                            rect,
                                            0.0,
                                            stroke,
                                            egui::StrokeKind::Inside,
                                        );
                                        let card = ui.interact(
                                            rect,
                                            ui.id().with((
                                                "library-card",
                                                &item.label,
                                                &item.tracks,
                                            )),
                                            egui::Sense::click(),
                                        );
                                        if card.clicked() {
                                            let all_selected = item
                                                .tracks
                                                .iter()
                                                .all(|path| self.selected_paths.contains(path));
                                            if all_selected {
                                                for path in &item.tracks {
                                                    self.selected_paths.remove(path);
                                                }
                                            } else {
                                                self.selected_paths
                                                    .extend(item.tracks.iter().cloned());
                                            }
                                        }
                                        self.show_track_context_menu(&card, &item.tracks);
                                    }
                                    ui.add_space(10.0);
                                });
                            }
                        }
                    }
                },
            );
    }

    fn show_track_context_menu(&mut self, response: &egui::Response, paths: &[PathBuf]) {
        let language = self.data.language;
        let selection_changed = paths.iter().any(|path| !self.selected_paths.contains(path));
        if response.secondary_clicked() && selection_changed {
            self.selected_paths.clear();
            self.selected_paths.extend(paths.iter().cloned());
        }
        response.context_menu(|ui| {
            if ui.button(tr(language, "Play selection")).clicked() {
                self.play_selected_track();
                ui.close_menu();
            }
            if ui.button(tr(language, "Add selection to queue")).clicked() {
                self.add_selected_to_queue(false);
                ui.close_menu();
            }
            if ui
                .button(tr(language, "Create playlist from selection"))
                .clicked()
            {
                self.create_playlist_from_selection();
                ui.close_menu();
            }
            if !self
                .data
                .playlists
                .iter()
                .any(|playlist| playlist.smart.is_none())
            {
                ui.label(tr(language, "No static playlists yet"));
            }
            let playlists: Vec<(usize, String)> = self
                .data
                .playlists
                .iter()
                .enumerate()
                .filter(|(_, playlist)| playlist.smart.is_none())
                .map(|(index, playlist)| (index, playlist.name.clone()))
                .collect();
            for (index, name) in playlists {
                if ui
                    .button(format!("{} {name}", tr(language, "Add selection to")))
                    .clicked()
                {
                    self.add_selection_to_playlist(index);
                    ui.close_menu();
                }
            }
        });
    }

    fn visible_tracks(&self) -> Vec<Track> {
        let query = self.filter.to_lowercase();
        let mut tracks: Vec<Track> = self
            .data
            .tracks
            .iter()
            .filter(|track| {
                query.is_empty()
                    || [
                        track.display_title(),
                        &track.album_artist,
                        &track.album,
                        &track.genre,
                    ]
                    .iter()
                    .any(|field| field.to_lowercase().contains(&query))
            })
            .cloned()
            .collect();
        tracks.sort_by_cached_key(|track| {
            (
                track_group_value(track, &self.organize_by).to_lowercase(),
                track_sort_value(track, &self.sort_by),
                track.display_title().to_lowercase(),
            )
        });
        tracks
    }
}

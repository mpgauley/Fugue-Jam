use super::*;

impl MusicApp {
    pub(super) fn add_selection_to_playlist(&mut self, playlist_index: usize) {
        let paths = self.selected_tracks();
        let Some(playlist) = self.data.playlists.get_mut(playlist_index) else {
            return;
        };
        if playlist.smart.is_some() {
            self.status = "Tracks can only be added to static playlists.".to_owned();
            return;
        }
        let count = paths.len();
        for path in paths {
            if !playlist.tracks.contains(&path) {
                playlist.tracks.push(path);
            }
        }
        self.playlist_load = None;
        self.save();
        self.status = format!("Added {count} selected track(s) to the playlist.");
    }

    pub(super) fn create_playlist_from_selection(&mut self) {
        let paths = self.selected_tracks();
        if paths.is_empty() {
            return;
        }
        let mut suffix = self.data.playlists.len() + 1;
        let mut name = format!("Selection {suffix}");
        while self
            .data
            .playlists
            .iter()
            .any(|playlist| playlist.name == name)
        {
            suffix += 1;
            name = format!("Selection {suffix}");
        }
        self.data.playlists.push(Playlist {
            name: name.clone(),
            tracks: paths,
            smart: None,
        });
        self.active_playlist = Some(self.data.playlists.len() - 1);
        self.save();
        self.status = format!("Created playlist “{name}” from selected tracks.");
    }
    pub(super) fn selected_tracks(&self) -> Vec<PathBuf> {
        self.data
            .tracks
            .iter()
            .filter(|track| self.selected_paths.contains(&track.path))
            .map(|track| track.path.clone())
            .collect()
    }

    pub(super) fn add_selected_to_queue(&mut self, random: bool) {
        let mut selected = self.selected_tracks();
        if random {
            selected.shuffle(&mut rand::rng());
        }
        let count = selected.len();
        let entries = selected
            .into_iter()
            .map(|path| self.queue_entry(path))
            .collect::<Vec<_>>();
        self.data.queue.extend(entries);
        self.status = format!("Added {count} selected track(s) to the queue.");
        self.save();
    }

    pub(super) fn play_selected_track(&mut self) {
        let Some(path) = self.selected_tracks().into_iter().next() else {
            self.status = "Select a track to play.".to_owned();
            return;
        };
        if !self.data.queue.iter().any(|entry| entry.path == path) {
            let entry = self.queue_entry(path.clone());
            self.data.queue.push(entry);
            self.save();
        }
        if let Some(index) = self
            .data
            .queue
            .iter()
            .position(|queued| queued.path == path)
        {
            self.play_queue_index(index);
        }
    }

    pub(super) fn ensure_playlist_load(&mut self, ctx: &egui::Context) {
        let Some(index) = self.active_playlist else {
            self.playlist_load = None;
            return;
        };
        if self
            .playlist_load
            .as_ref()
            .is_some_and(|task| task.playlist_index == index)
        {
            return;
        }
        let Some(playlist) = self.data.playlists.get(index).cloned() else {
            self.active_playlist = None;
            self.playlist_load = None;
            return;
        };
        let tracks = Arc::clone(&self.data.tracks);
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            load_playlist_rows(&playlist, &tracks, &sender, &ctx);
        });
        self.playlist_load = Some(PlaylistLoadTask {
            playlist_index: index,
            receiver,
            rows: Arc::new(Vec::new()),
            finished: false,
            error: None,
        });
    }

    pub(super) fn receive_playlist_rows(&mut self, ctx: &egui::Context) {
        let Some(task) = self.playlist_load.as_mut() else {
            return;
        };
        if task.finished {
            return;
        }
        for _ in 0..8 {
            let message = match task.receiver.try_recv() {
                Ok(message) => message,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    task.finished = true;
                    task.error = Some(
                        tr(self.data.language, "Playlist loader stopped unexpectedly.").to_owned(),
                    );
                    break;
                }
            };
            match message {
                PlaylistLoadMessage::Batch(mut rows) => {
                    Arc::make_mut(&mut task.rows).append(&mut rows)
                }
                PlaylistLoadMessage::Finished(result) => {
                    task.finished = true;
                    task.error = result.err();
                    break;
                }
            }
        }
        if !task.finished {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    pub(super) fn copy_playlist_to_queue(&mut self, random: bool) {
        let Some(index) = self.active_playlist else {
            self.status = "Select a playlist first.".to_owned();
            return;
        };
        let Some(task) = self
            .playlist_load
            .as_ref()
            .filter(|task| task.playlist_index == index && task.finished)
        else {
            self.status = tr(self.data.language, "Playlist is still loading.").to_owned();
            return;
        };
        if task.error.is_some() {
            self.status =
                tr(self.data.language, "Playlist loader stopped unexpectedly.").to_owned();
            return;
        }
        let mut paths: Vec<PathBuf> = task.rows.iter().map(|row| row.path.clone()).collect();
        if random {
            paths.shuffle(&mut rand::rng());
        }
        let count = paths.len();
        let entries = paths
            .into_iter()
            .map(|path| self.queue_entry(path))
            .collect::<Vec<_>>();
        self.data.queue.extend(entries);
        self.status = format!(
            "Added {count} playlist track(s) to the queue{}.",
            if random { " in random order" } else { "" }
        );
        self.save();
    }

    pub(super) fn remove_queue_track(&mut self, index: usize) {
        if index >= self.data.queue.len() {
            return;
        }
        if self.queue_index == Some(index) {
            self.stop();
            self.data.queue.remove(index);
        } else {
            self.data.queue.remove(index);
            if self.queue_index.is_some_and(|current| index < current) {
                self.queue_index = self.queue_index.map(|current| current - 1);
            }
        }
        self.save();
    }

    pub(super) fn play_queue_track_next(&mut self, index: usize) {
        if index >= self.data.queue.len() || self.queue_index == Some(index) {
            return;
        }
        let current_index = self.queue_index;
        let track = self.data.queue.remove(index);
        let current_after_removal = current_index.map(|current| {
            if index < current {
                current - 1
            } else {
                current
            }
        });
        let insertion_index = current_after_removal.map_or(0, |current| current + 1);
        self.data.queue.insert(insertion_index, track);
        self.queue_index = current_after_removal.map(|current| {
            if insertion_index <= current {
                current + 1
            } else {
                current
            }
        });
        self.save();
    }

    pub(super) fn move_queue_track_to_end(&mut self, index: usize) {
        if index >= self.data.queue.len() || index + 1 == self.data.queue.len() {
            return;
        }
        let current_index = self.queue_index;
        let track = self.data.queue.remove(index);
        self.data.queue.push(track);
        self.queue_index = current_index.map(|current| {
            if index == current {
                self.data.queue.len() - 1
            } else if index < current {
                current - 1
            } else {
                current
            }
        });
        self.save();
    }

    pub(super) fn add_queue_track_to_playlist(&mut self, path: &Path, playlist_index: usize) {
        let Some(playlist) = self.data.playlists.get_mut(playlist_index) else {
            self.status = "That playlist no longer exists.".to_owned();
            return;
        };
        if playlist.smart.is_some() {
            self.status = "Tracks can only be added to static playlists.".to_owned();
            return;
        }
        let playlist_name = playlist.name.clone();
        let path = path.to_path_buf();
        if !playlist.tracks.contains(&path) {
            playlist.tracks.push(path);
            self.save();
            self.status = format!("Added track to {playlist_name}.");
        } else {
            self.status = format!("Track is already in {playlist_name}.");
        }
    }

    pub(super) fn create_static_playlist(&mut self) {
        let name = self.new_playlist_name.trim().to_owned();
        if name.is_empty() {
            self.status = "Enter a name for the playlist.".to_owned();
            return;
        }
        let paths = self.selected_tracks();
        if paths.is_empty() {
            self.status = "Select one or more library tracks for the playlist.".to_owned();
            return;
        }
        self.data.playlists.push(Playlist {
            name: name.clone(),
            tracks: paths,
            smart: None,
        });
        self.active_playlist = Some(self.data.playlists.len() - 1);
        self.new_playlist_name.clear();
        self.save();
        self.status = format!("Created playlist “{name}”.");
    }

    pub(super) fn create_smart_playlist(&mut self) {
        let name = self.smart_playlist_name.trim().to_owned();
        let query = SmartPlaylistQuery {
            conditions: self.smart_conditions.clone(),
            operators: self.smart_operators.clone(),
        };
        if name.is_empty() || !query.is_valid() {
            self.status = tr(
                self.data.language,
                "Enter a playlist name, values for every condition, and balanced parentheses.",
            )
            .to_owned();
            return;
        }
        self.start_smart_playlist_scan(name, query);
    }
}

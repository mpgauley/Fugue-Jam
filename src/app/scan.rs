use super::*;

impl MusicApp {
    pub(super) fn add_folder(&mut self) {
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.start_folder_scan(folder, false);
    }

    pub(super) fn rescan_folder(&mut self) {
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.start_folder_scan(folder, true);
    }

    pub(super) fn rescan_all_metadata(&mut self) {
        if self.scan_task.is_some() {
            self.status = tr(
                self.data.language,
                "A library scan is already running. Try again when it finishes.",
            )
            .to_owned();
            return;
        }
        let folders = self.data.folders.clone();
        if folders.is_empty() {
            self.status = tr(
                self.data.language,
                "Add a music folder before rescanning metadata.",
            )
            .to_owned();
            return;
        }
        let recurse = self.scan_subfolders;
        let artwork_dir = self
            .data_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("album-art");
        let progress = Arc::new(AtomicUsize::new(0));
        let total = Arc::new(AtomicUsize::new(0));
        let worker_progress = Arc::clone(&progress);
        let worker_total = Arc::clone(&total);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = scan_folders(
                &folders,
                recurse,
                &artwork_dir,
                &worker_progress,
                &worker_total,
                &sender,
            );
            let _ = sender.send(ScanMessage::Finished(result));
        });
        self.status = tr(self.data.language, "Rescanning library metadata…").to_owned();
        self.scan_task = Some(ScanTask {
            folder: PathBuf::new(),
            progress,
            total,
            receiver,
            added_count: 0,
            refreshed_count: 0,
            remove_missing: false,
            recursive: recurse,
            seen_tracks: HashSet::new(),
            added_tracks: HashSet::new(),
            smart_playlist: None,
        });
    }

    pub(super) fn start_folder_scan(&mut self, folder: PathBuf, remove_missing: bool) {
        if self.scan_task.is_some() {
            self.status = "A library scan is already running.".to_owned();
            return;
        }
        if !self.data.folders.contains(&folder) {
            self.data.folders.push(folder.clone());
        }
        let folder_for_worker = folder.clone();
        let recurse = self.scan_subfolders;
        let artwork_dir = self
            .data_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("album-art");
        let progress = Arc::new(AtomicUsize::new(0));
        let total = Arc::new(AtomicUsize::new(0));
        let worker_progress = Arc::clone(&progress);
        let worker_total = Arc::clone(&total);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = scan_folder(
                &folder_for_worker,
                recurse,
                &artwork_dir,
                &worker_progress,
                &worker_total,
                &sender,
            );
            let _ = sender.send(ScanMessage::Finished(result));
        });
        self.status = format!("Scanning {}…", folder.display());
        self.scan_task = Some(ScanTask {
            folder,
            progress,
            total,
            receiver,
            added_count: 0,
            refreshed_count: 0,
            remove_missing,
            recursive: recurse,
            seen_tracks: HashSet::new(),
            added_tracks: HashSet::new(),
            smart_playlist: None,
        });
    }

    pub(super) fn start_smart_playlist_scan(&mut self, name: String, query: SmartPlaylistQuery) {
        if self.scan_task.is_some() {
            self.status = tr(
                self.data.language,
                "A library scan is already running. Try again when it finishes.",
            )
            .to_owned();
            return;
        }
        let folders = self.data.folders.clone();
        let recurse = self.scan_subfolders;
        let artwork_dir = self
            .data_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("album-art");
        let progress = Arc::new(AtomicUsize::new(0));
        let total = Arc::new(AtomicUsize::new(0));
        let worker_progress = Arc::clone(&progress);
        let worker_total = Arc::clone(&total);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = scan_folders(
                &folders,
                recurse,
                &artwork_dir,
                &worker_progress,
                &worker_total,
                &sender,
            );
            let _ = sender.send(ScanMessage::Finished(result));
        });
        self.status = tr(self.data.language, "Scanning library for smart playlist…").to_owned();
        self.scan_task = Some(ScanTask {
            folder: PathBuf::new(),
            progress,
            total,
            receiver,
            added_count: 0,
            refreshed_count: 0,
            remove_missing: false,
            recursive: recurse,
            seen_tracks: HashSet::new(),
            added_tracks: HashSet::new(),
            smart_playlist: Some((name, query)),
        });
    }

    pub(super) fn finish_scan_if_ready(&mut self) {
        let Some(task) = self.scan_task.as_ref() else {
            return;
        };
        let messages: Vec<ScanMessage> = task.receiver.try_iter().take(128).collect();
        let mut added_count = 0;
        let mut refreshed_count = 0;
        let mut finish_result = None;
        for message in messages {
            match message {
                ScanMessage::Total(total) => {
                    if let Some(task) = &self.scan_task {
                        task.total.store(total, Ordering::Relaxed);
                    }
                }
                ScanMessage::Track(track) => {
                    if let Some(task) = &mut self.scan_task {
                        task.seen_tracks.insert(track.path.clone());
                    }
                    for entry in self
                        .data
                        .queue
                        .iter_mut()
                        .filter(|entry| entry.path == track.path)
                    {
                        entry.update_fields(&track, &self.data.queue_display_fields);
                    }
                    if let Some(index) = self.library_indices.get(&track.path).copied() {
                        Arc::make_mut(&mut self.data.tracks)[index] = track;
                        refreshed_count += 1;
                    } else {
                        if let Some(task) = &mut self.scan_task {
                            task.added_tracks.insert(track.path.clone());
                        }
                        self.library_indices
                            .insert(track.path.clone(), self.data.tracks.len());
                        Arc::make_mut(&mut self.data.tracks).push(track);
                        added_count += 1;
                    }
                }
                ScanMessage::Finished(result) => finish_result = Some(result),
            }
        }
        if finish_result.is_some() {
            self.playlist_load = None;
            Arc::make_mut(&mut self.data.tracks)
                .sort_by_key(|track| track.display_title().to_lowercase());
            self.library_indices = self
                .data
                .tracks
                .iter()
                .enumerate()
                .map(|(index, track)| (track.path.clone(), index))
                .collect();
        }
        if let Some(task) = &mut self.scan_task {
            task.added_count += added_count;
            task.refreshed_count += refreshed_count;
        }
        if let Some(result) = finish_result {
            let (
                folder,
                added_count,
                refreshed_count,
                remove_missing,
                recursive,
                seen_tracks,
                added_tracks,
                smart_playlist,
            ) = self
                .scan_task
                .as_ref()
                .map(|task| {
                    (
                        task.folder.clone(),
                        task.added_count,
                        task.refreshed_count,
                        task.remove_missing,
                        task.recursive,
                        task.seen_tracks.clone(),
                        task.added_tracks.clone(),
                        task.smart_playlist.clone(),
                    )
                })
                .unwrap_or_default();
            match result {
                Ok(()) => {
                    if let Some((name, query)) = smart_playlist {
                        self.data.playlists.push(Playlist {
                            name: name.clone(),
                            tracks: Vec::new(),
                            smart: Some(query),
                        });
                        self.active_playlist = Some(self.data.playlists.len() - 1);
                        self.smart_playlist_name.clear();
                        self.smart_conditions = vec![SmartCondition::default()];
                        self.smart_operators.clear();
                        let scanned_count = self
                            .scan_task
                            .as_ref()
                            .map(|task| task.progress.load(Ordering::Relaxed))
                            .unwrap_or_default();
                        self.status = format!(
                            "{} “{name}” — {} {scanned_count} {}.",
                            tr(self.data.language, "Created smart playlist"),
                            tr(self.data.language, "after scanning"),
                            tr(self.data.language, "tracks")
                        );
                        self.save();
                        self.scan_task = None;
                        return;
                    }
                    let (removed_count, moved_count) = if remove_missing {
                        self.remove_missing_tracks(&folder, recursive, &seen_tracks, &added_tracks)
                    } else {
                        (0, 0)
                    };
                    let added_count = added_count.saturating_sub(moved_count);
                    let refreshed_count = refreshed_count + moved_count;
                    self.status = if folder.as_os_str().is_empty() {
                        format!(
                            "{} {added_count} {}, {refreshed_count} {}.",
                            tr(self.data.language, "Metadata rescan complete:"),
                            tr(self.data.language, "new tracks found"),
                            tr(self.data.language, "tracks refreshed")
                        )
                    } else {
                        format!(
                            "Added {added_count}, refreshed {refreshed_count}, and removed {removed_count} missing track(s) from {}.",
                            folder.display(),
                        )
                    };
                    self.save();
                }
                Err(error) => {
                    self.status = format!("Library scan failed: {error}");
                    self.save();
                }
            }
            self.scan_task = None;
        } else if added_count + refreshed_count > 0
            && let Some(task) = &self.scan_task
        {
            self.status = if task.smart_playlist.is_some() {
                format!(
                    "{} {} track(s) loaded",
                    tr(self.data.language, "Scanning library for smart playlist…"),
                    task.progress.load(Ordering::Relaxed)
                )
            } else if task.folder.as_os_str().is_empty() {
                format!(
                    "{} {} track(s) loaded",
                    tr(self.data.language, "Rescanning library metadata…"),
                    task.progress.load(Ordering::Relaxed)
                )
            } else {
                format!(
                    "Scanning {}… {} track(s) loaded",
                    task.folder.display(),
                    task.progress.load(Ordering::Relaxed)
                )
            };
        }
    }

    pub(super) fn remove_missing_tracks(
        &mut self,
        folder: &Path,
        recursive: bool,
        seen_tracks: &HashSet<PathBuf>,
        added_tracks: &HashSet<PathBuf>,
    ) -> (usize, usize) {
        let missing: HashSet<PathBuf> = self
            .data
            .tracks
            .iter()
            .filter(|track| {
                is_in_scan_scope(&track.path, folder, recursive)
                    && !seen_tracks.contains(&track.path)
            })
            .map(|track| track.path.clone())
            .collect();
        if missing.is_empty() {
            return (0, 0);
        }
        let replacements = find_moved_track_paths(&self.data.tracks, &missing, added_tracks);
        for (old_path, new_path) in &replacements {
            for playlist in &mut self.data.playlists {
                for path in &mut playlist.tracks {
                    if path == old_path {
                        path.clone_from(new_path);
                    }
                }
            }
            for entry in &mut self.data.queue {
                if &entry.path == old_path {
                    entry.path.clone_from(new_path);
                    if let Some(track) = self
                        .library_indices
                        .get(new_path)
                        .and_then(|index| self.data.tracks.get(*index))
                    {
                        entry.update_fields(track, &self.data.queue_display_fields);
                    }
                }
            }
            if self.selected_paths.remove(old_path) {
                self.selected_paths.insert(new_path.clone());
            }
        }
        let migrated: HashSet<PathBuf> = replacements.keys().cloned().collect();
        let removed: HashSet<PathBuf> = missing.difference(&migrated).cloned().collect();
        let current_entry = self
            .queue_index
            .and_then(|index| self.data.queue.get(index))
            .cloned();
        if current_entry
            .as_ref()
            .is_some_and(|entry| removed.contains(&entry.path))
        {
            self.stop();
        }
        Arc::make_mut(&mut self.data.tracks).retain(|track| !removed.contains(&track.path));
        self.data.playlists.iter_mut().for_each(|playlist| {
            let mut unique_paths = HashSet::new();
            playlist
                .tracks
                .retain(|path| !removed.contains(path) && unique_paths.insert(path.clone()));
        });
        self.selected_paths.retain(|path| !removed.contains(path));
        self.data
            .queue
            .retain(|entry| !removed.contains(&entry.path));
        if let Some(entry) = current_entry.filter(|entry| !removed.contains(&entry.path)) {
            self.queue_index = self
                .data
                .queue
                .iter()
                .position(|queued| queued.path == entry.path);
        } else if self.sink.is_none() {
            self.queue_index = None;
        }
        self.library_indices = self
            .data
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (track.path.clone(), index))
            .collect();
        (removed.len(), replacements.len())
    }

    pub(super) fn queue_entry(&self, path: PathBuf) -> QueueEntry {
        self.library_indices
            .get(&path)
            .and_then(|index| self.data.tracks.get(*index))
            .map(|track| QueueEntry::from_track(track, &self.data.queue_display_fields))
            .unwrap_or_else(|| QueueEntry::from_path(path))
    }

    pub(super) fn finish_waveform_loads(&mut self) {
        for result in self.waveform_results.try_iter() {
            match result {
                Ok((path, waveform)) if self.waveform_track.as_ref() == Some(&path) => {
                    self.waveform = waveform;
                }
                Ok(_) => {}
                Err((path, error)) if self.waveform_track.as_ref() == Some(&path) => {
                    self.status = format!("Could not generate waveform: {error}");
                }
                Err(_) => {}
            }
        }
    }

    pub(super) fn load_artwork(&mut self, path: Option<&Path>) -> Option<TextureHandle> {
        let path = path?;
        if let Some(texture) = self.texture_cache.get(path) {
            return Some(texture.clone());
        }
        let path = path.to_path_buf();
        if !self.artwork_failed.contains(&path)
            && self.artwork_pending.insert(path.clone())
            && self.artwork_requests.send(path.clone()).is_err()
        {
            self.artwork_pending.remove(&path);
            eprintln!("Album artwork loader is unavailable.");
        }
        None
    }

    pub(super) fn finish_artwork_loads(&mut self, ctx: &egui::Context) {
        for result in self.artwork_results.try_iter().take(8) {
            match result {
                Ok((path, image)) => {
                    self.artwork_pending.remove(&path);
                    let texture = ctx.load_texture(
                        path.display().to_string(),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.texture_cache.insert(path, texture);
                }
                Err((path, error)) => {
                    self.artwork_pending.remove(&path);
                    self.artwork_failed.insert(path.clone());
                    eprintln!("Could not load album art {}: {error}", path.display());
                }
            }
        }
    }
}

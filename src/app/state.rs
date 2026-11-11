use super::*;

impl MusicApp {
    pub(crate) fn new() -> Self {
        let data_path = directories::ProjectDirs::from("org", "FugueJam", "Fugue Jam")
            .map(|dirs| dirs.data_dir().join("library.json"))
            .unwrap_or_else(|| PathBuf::from("fugue-jam-library.json"));
        let (mut data, mut status) = match fs::read(&data_path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(data) => (data, "Library loaded.".to_owned()),
                Err(error) => (
                    LibraryData::default(),
                    format!("Could not read the saved library: {error}"),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
                LibraryData::default(),
                "Add a music folder to start building your library.".to_owned(),
            ),
            Err(error) => (
                LibraryData::default(),
                format!("Could not open the saved library: {error}"),
            ),
        };
        data.eq_band_count = supported_eq_band_count(data.eq_band_count);
        data.eq_bands.resize(data.eq_band_count, 0.0);
        data.eq_bands.truncate(data.eq_band_count);
        data.eq_bands
            .iter_mut()
            .for_each(|gain| *gain = gain.clamp(-12.0, 12.0));
        data.now_playing_left_fraction = data.now_playing_left_fraction.clamp(0.25, 0.9);
        data.library_view = available_library_view(data.library_view);
        let output_device_name = cpal::default_host()
            .default_output_device()
            .and_then(|device| match device.name() {
                Ok(name) => Some(name),
                Err(error) => {
                    eprintln!("Could not identify the default audio output: {error}");
                    None
                }
            });
        let hdmi_output = output_device_name
            .as_deref()
            .is_some_and(|name| name.to_lowercase().contains("hdmi"));
        let (stream, stream_handle) = match OutputStream::try_default() {
            Ok((stream, handle)) => (Some(stream), Some(handle)),
            Err(error) => {
                status = format!("Audio output is unavailable: {error}. {status}");
                (None, None)
            }
        };
        let hdmi_keepalive = if hdmi_output {
            match stream_handle.as_ref() {
                Some(handle) => match Sink::try_new(handle) {
                    Ok(sink) => {
                        sink.append(SilenceSource);
                        sink.pause();
                        status = format!(
                            "HDMI output detected; silent audio will keep the connection active while playback is paused. {status}"
                        );
                        Some(sink)
                    }
                    Err(error) => {
                        status = format!(
                            "HDMI output detected, but silent keep-alive could not be started: {error}. {status}"
                        );
                        None
                    }
                },
                None => {
                    status = format!(
                        "HDMI output detected, but silent keep-alive could not start because audio output is unavailable. {status}"
                    );
                    None
                }
            }
        } else {
            None
        };
        let (artwork_requests, request_receiver) = mpsc::channel::<PathBuf>();
        let (result_sender, artwork_results) =
            mpsc::channel::<Result<(PathBuf, egui::ColorImage), (PathBuf, String)>>();
        thread::spawn(move || {
            while let Ok(path) = request_receiver.recv() {
                let result = decode_artwork(&path)
                    .map(|image| (path.clone(), image))
                    .map_err(|error| (path, error));
                if result_sender.send(result).is_err() {
                    break;
                }
            }
        });
        let (waveform_requests, waveform_request_receiver) = mpsc::channel::<PathBuf>();
        let (waveform_result_sender, waveform_results) =
            mpsc::channel::<Result<(PathBuf, Vec<f32>), (PathBuf, String)>>();
        thread::spawn(move || {
            while let Ok(path) = waveform_request_receiver.recv() {
                let result = decode_waveform(&path)
                    .map(|waveform| (path.clone(), waveform))
                    .map_err(|error| (path, error));
                if waveform_result_sender.send(result).is_err() {
                    break;
                }
            }
        });
        let eq_controls = Arc::new(EqualizerControls::new(&data.eq_bands, data.eq_enabled));
        let library_indices: HashMap<PathBuf, usize> = data
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (track.path.clone(), index))
            .collect();
        data.queue_display_fields
            .retain(|field| QueueDisplayField::ALL.contains(field));
        if data.queue_display_fields.is_empty() {
            data.queue_display_fields = default_queue_display_fields();
        }
        for entry in &mut data.queue {
            if let Some(track) = library_indices
                .get(&entry.path)
                .and_then(|index| data.tracks.get(*index))
            {
                entry.update_fields(track, &data.queue_display_fields);
            } else {
                entry.retain_fields(&data.queue_display_fields);
            }
        }
        let library_view = data.library_view;
        let eq_bands = data.eq_bands.clone();
        let eq_band_count = data.eq_band_count;
        let eq_enabled = data.eq_enabled;
        let now_playing_left_fraction = data.now_playing_left_fraction;
        Self {
            data_path,
            data,
            tab: Tab::NowPlaying,
            selected_paths: HashSet::new(),
            queue_index: None,
            pending_queue_playlist: None,
            new_queue_playlist_name: String::new(),
            show_queue_display_options: false,
            filter: String::new(),
            organize_by: "Album Artist".to_owned(),
            library_view,
            sort_by: "Album Name".to_owned(),
            thumbnail_size: 144.0,
            active_playlist: None,
            playlist_load: None,
            new_playlist_name: String::new(),
            smart_playlist_name: String::new(),
            smart_conditions: vec![SmartCondition::default()],
            smart_operators: Vec::new(),
            status,
            _stream: stream,
            stream_handle,
            sink: None,
            hdmi_keepalive,
            paused: false,
            current_title: String::new(),
            track_duration: None,
            eq_bands,
            eq_band_count,
            eq_enabled,
            eq_preset: 0,
            eq_controls,
            waveform: Vec::new(),
            waveform_track: None,
            waveform_requests,
            waveform_results,
            scan_subfolders: true,
            scan_task: None,
            texture_cache: HashMap::new(),
            artwork_requests,
            artwork_results,
            artwork_pending: HashSet::new(),
            artwork_failed: HashSet::new(),
            library_indices,
            now_playing_left_fraction,
        }
    }

    pub(super) fn save(&mut self) {
        self.data.library_view = self.library_view;
        self.data.eq_band_count = self.eq_band_count;
        self.data.eq_bands.clone_from(&self.eq_bands);
        self.data.eq_enabled = self.eq_enabled;
        self.data.now_playing_left_fraction = self.now_playing_left_fraction;
        let result = (|| -> Result<(), String> {
            if let Some(parent) = self.data_path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let json = serde_json::to_vec_pretty(&self.data).map_err(|error| error.to_string())?;
            fs::write(&self.data_path, json).map_err(|error| error.to_string())
        })();
        if let Err(error) = result {
            self.status = format!("Could not save library: {error}");
        }
    }
}

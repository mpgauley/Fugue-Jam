use std::{
    collections::{HashMap, HashSet, hash_map::DefaultHasher},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::BufReader,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

use eframe::egui::{self, TextureHandle};
use lofty::{
    file::{AudioFile, TaggedFileExt},
    picture::PictureType,
    probe::Probe,
    tag::Accessor,
};
use rand::seq::SliceRandom;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use serde::{Deserialize, Serialize};

const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aiff", "aif", "flac", "m4a", "mp3", "oga", "ogg", "wav",
];
const EQ_FREQUENCIES: [f32; 5] = [60.0, 250.0, 1_000.0, 4_000.0, 12_000.0];
const PROGRAM_VERSION: u32 = 2;

struct Biquad {
    coefficients: [f32; 5],
    state: Vec<(f32, f32)>,
}

impl Biquad {
    fn peaking(frequency: f32, gain_db: f32, sample_rate: u32, channels: u16) -> Self {
        let nyquist = sample_rate as f32 / 2.0;
        let frequency = frequency.min(nyquist * 0.95).max(1.0);
        let omega = std::f32::consts::TAU * frequency / sample_rate as f32;
        let alpha = omega.sin() / (2.0 * 0.8);
        let amplitude = 10.0_f32.powf(gain_db / 40.0);
        let a0 = 1.0 + alpha / amplitude;
        let cosine = omega.cos();
        Self {
            coefficients: [
                (1.0 + alpha * amplitude) / a0,
                (-2.0 * cosine) / a0,
                (1.0 - alpha * amplitude) / a0,
                (-2.0 * cosine) / a0,
                (1.0 - alpha / amplitude) / a0,
            ],
            state: vec![(0.0, 0.0); channels.max(1) as usize],
        }
    }

    fn process(&mut self, sample: f32, channel: usize) -> f32 {
        let [b0, b1, b2, a1, a2] = self.coefficients;
        let (z1, z2) = &mut self.state[channel];
        let output = b0 * sample + *z1;
        *z1 = b1 * sample - a1 * output + *z2;
        *z2 = b2 * sample - a2 * output;
        output
    }
}

struct EqualizedSource<S> {
    input: S,
    filters: Vec<Biquad>,
    channel: usize,
    channels: usize,
}

impl<S: Source<Item = f32>> EqualizedSource<S> {
    fn new(input: S, bands: &[f32; 5], enabled: bool) -> Self {
        let channels = input.channels().max(1) as usize;
        let filters = if enabled {
            EQ_FREQUENCIES
                .iter()
                .zip(bands)
                .filter(|(_, gain)| gain.abs() > f32::EPSILON)
                .map(|(&frequency, &gain)| {
                    Biquad::peaking(frequency, gain, input.sample_rate(), channels as u16)
                })
                .collect()
        } else {
            Vec::new()
        };
        Self {
            input,
            filters,
            channel: 0,
            channels,
        }
    }
}

impl<S: Source<Item = f32>> Iterator for EqualizedSource<S> {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let mut sample = self.input.next()?;
        for filter in &mut self.filters {
            sample = filter.process(sample, self.channel);
        }
        self.channel = (self.channel + 1) % self.channels;
        Some(sample.clamp(-1.0, 1.0))
    }
}

impl<S: Source<Item = f32>> Source for EqualizedSource<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.input.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.input.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.input.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Track {
    path: PathBuf,
    title: String,
    album_artist: String,
    album: String,
    genre: String,
    year: Option<u32>,
    duration_secs: Option<u64>,
    #[serde(default)]
    artwork_path: Option<PathBuf>,
}

impl Track {
    fn display_title(&self) -> &str {
        if self.title.is_empty() {
            self.path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("Unknown track")
        } else {
            &self.title
        }
    }

    fn duration_label(&self) -> String {
        self.duration_secs
            .map(|secs| format!("{}:{:02}", secs / 60, secs % 60))
            .unwrap_or_else(|| "—".to_owned())
    }

    fn file_type(&self) -> String {
        self.path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_uppercase)
            .unwrap_or_else(|| "—".to_owned())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum SmartField {
    AlbumArtist,
    Album,
    Genre,
    Year,
}

impl SmartField {
    fn value<'a>(&self, track: &'a Track) -> String {
        match self {
            Self::AlbumArtist => track.album_artist.clone(),
            Self::Album => track.album.clone(),
            Self::Genre => track.genre.clone(),
            Self::Year => track.year.map(|year| year.to_string()).unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Playlist {
    name: String,
    tracks: Vec<PathBuf>,
    smart: Option<(SmartField, String)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct LibraryData {
    folders: Vec<PathBuf>,
    tracks: Vec<Track>,
    playlists: Vec<Playlist>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    NowPlaying,
    Library,
    Playlists,
    Equalization,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Self::NowPlaying => "Now Playing",
            Self::Library => "Library",
            Self::Playlists => "Playlists",
            Self::Equalization => "Equalization",
        }
    }
}

struct MusicApp {
    data_path: PathBuf,
    data: LibraryData,
    tab: Tab,
    selected_paths: HashSet<PathBuf>,
    queue: Vec<PathBuf>,
    queue_index: Option<usize>,
    filter: String,
    organize_by: String,
    library_view: LibraryView,
    active_playlist: Option<usize>,
    new_playlist_name: String,
    smart_playlist_name: String,
    smart_field: usize,
    smart_match: String,
    status: String,
    _stream: Option<OutputStream>,
    stream_handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
    paused: bool,
    current_title: String,
    track_duration: Option<Duration>,
    eq_bands: [f32; 5],
    eq_enabled: bool,
    eq_preset: usize,
    scan_subfolders: bool,
    scan_task: Option<ScanTask>,
    texture_cache: HashMap<PathBuf, TextureHandle>,
    now_playing_left_fraction: f32,
}

struct ScanTask {
    folder: PathBuf,
    progress: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    receiver: Receiver<Result<Vec<Track>, String>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LibraryView {
    Tracks,
    Albums,
}

impl MusicApp {
    fn new() -> Self {
        let data_path = directories::ProjectDirs::from("org", "FugueJam", "Fugue Jam")
            .map(|dirs| dirs.data_dir().join("library.json"))
            .unwrap_or_else(|| PathBuf::from("fugue-jam-library.json"));
        let (data, mut status) = match fs::read(&data_path) {
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
        let (stream, stream_handle) = match OutputStream::try_default() {
            Ok((stream, handle)) => (Some(stream), Some(handle)),
            Err(error) => {
                status = format!("Audio output is unavailable: {error}. {status}");
                (None, None)
            }
        };
        Self {
            data_path,
            data,
            tab: Tab::NowPlaying,
            selected_paths: HashSet::new(),
            queue: Vec::new(),
            queue_index: None,
            filter: String::new(),
            organize_by: "Album Artist".to_owned(),
            library_view: LibraryView::Tracks,
            active_playlist: None,
            new_playlist_name: String::new(),
            smart_playlist_name: String::new(),
            smart_field: 0,
            smart_match: String::new(),
            status,
            _stream: stream,
            stream_handle,
            sink: None,
            paused: false,
            current_title: String::new(),
            track_duration: None,
            eq_bands: [0.0; 5],
            eq_enabled: true,
            eq_preset: 0,
            scan_subfolders: true,
            scan_task: None,
            texture_cache: HashMap::new(),
            now_playing_left_fraction: 0.75,
        }
    }

    fn save(&mut self) {
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

    fn add_folder(&mut self) {
        if self.scan_task.is_some() {
            return;
        }
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
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
            );
            let _ = sender.send(result);
        });
        self.status = format!("Scanning {}…", folder.display());
        self.scan_task = Some(ScanTask {
            folder,
            progress,
            total,
            receiver,
        });
    }

    fn finish_scan_if_ready(&mut self) {
        let Some(task) = self.scan_task.as_ref() else {
            return;
        };
        match task.receiver.try_recv() {
            Ok(Ok(scanned_tracks)) => {
                let folder = task.folder.clone();
                let mut track_indices: HashMap<PathBuf, usize> = self
                    .data
                    .tracks
                    .iter()
                    .enumerate()
                    .map(|(index, track)| (track.path.clone(), index))
                    .collect();
                let mut added_count = 0;
                let mut refreshed_count = 0;
                for track in scanned_tracks {
                    if let Some(index) = track_indices.get(&track.path).copied() {
                        self.data.tracks[index] = track;
                        refreshed_count += 1;
                    } else {
                        track_indices.insert(track.path.clone(), self.data.tracks.len());
                        self.data.tracks.push(track);
                        added_count += 1;
                    }
                }
                self.data
                    .tracks
                    .sort_by_key(|track| track.display_title().to_lowercase());
                self.status = format!(
                    "Added {added_count} and refreshed {refreshed_count} track(s) from {}.",
                    folder.display()
                );
                self.scan_task = None;
                self.save();
            }
            Ok(Err(error)) => {
                self.status = format!("Could not scan folder: {error}");
                self.scan_task = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status = "The library scan stopped unexpectedly.".to_owned();
                self.scan_task = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    fn load_artwork(&mut self, ctx: &egui::Context, path: Option<&Path>) -> Option<TextureHandle> {
        let path = path?;
        if let Some(texture) = self.texture_cache.get(path) {
            return Some(texture.clone());
        }
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!(
                    "Could not read cached album art {}: {error}",
                    path.display()
                );
                return None;
            }
        };
        let image = match image::load_from_memory(&bytes) {
            Ok(image) => image.to_rgba8(),
            Err(error) => {
                eprintln!(
                    "Could not decode cached album art {}: {error}",
                    path.display()
                );
                return None;
            }
        };
        let size = [image.width() as usize, image.height() as usize];
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
        let texture = ctx.load_texture(
            path.display().to_string(),
            color_image,
            egui::TextureOptions::LINEAR,
        );
        self.texture_cache
            .insert(path.to_path_buf(), texture.clone());
        Some(texture)
    }

    fn add_selection_to_playlist(&mut self, playlist_index: usize) {
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
        self.save();
        self.status = format!("Added {count} selected track(s) to the playlist.");
    }

    fn create_playlist_from_selection(&mut self) {
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
    fn selected_tracks(&self) -> Vec<PathBuf> {
        self.data
            .tracks
            .iter()
            .filter(|track| self.selected_paths.contains(&track.path))
            .map(|track| track.path.clone())
            .collect()
    }

    fn add_selected_to_queue(&mut self, random: bool) {
        let mut selected = self.selected_tracks();
        if random {
            selected.shuffle(&mut rand::rng());
        }
        let count = selected.len();
        self.queue.extend(selected);
        self.status = format!("Added {count} selected track(s) to the queue.");
    }

    fn playlist_tracks(&self, playlist: &Playlist) -> Vec<PathBuf> {
        if let Some((field, query)) = &playlist.smart {
            let needle = query.to_lowercase();
            self.data
                .tracks
                .iter()
                .filter(|track| field.value(track).to_lowercase().contains(&needle))
                .map(|track| track.path.clone())
                .collect()
        } else {
            playlist.tracks.clone()
        }
    }

    fn copy_playlist_to_queue(&mut self, random: bool) {
        let Some(index) = self.active_playlist else {
            self.status = "Select a playlist first.".to_owned();
            return;
        };
        let Some(playlist) = self.data.playlists.get(index) else {
            self.active_playlist = None;
            self.status = "That playlist no longer exists.".to_owned();
            return;
        };
        let mut paths = self.playlist_tracks(playlist);
        if random {
            paths.shuffle(&mut rand::rng());
        }
        let count = paths.len();
        self.queue.extend(paths);
        self.status = format!(
            "Added {count} playlist track(s) to the queue{}.",
            if random { " in random order" } else { "" }
        );
    }

    fn create_static_playlist(&mut self) {
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

    fn create_smart_playlist(&mut self) {
        let name = self.smart_playlist_name.trim().to_owned();
        let query = self.smart_match.trim().to_owned();
        if name.is_empty() || query.is_empty() {
            self.status = "Enter a name and a matching value for the smart playlist.".to_owned();
            return;
        }
        let field = match self.smart_field {
            1 => SmartField::Album,
            2 => SmartField::Genre,
            3 => SmartField::Year,
            _ => SmartField::AlbumArtist,
        };
        self.data.playlists.push(Playlist {
            name: name.clone(),
            tracks: Vec::new(),
            smart: Some((field, query)),
        });
        self.active_playlist = Some(self.data.playlists.len() - 1);
        self.smart_playlist_name.clear();
        self.smart_match.clear();
        self.save();
        self.status = format!("Created smart playlist “{name}”.");
    }

    fn play_queue_index(&mut self, index: usize) {
        let Some(path) = self.queue.get(index).cloned() else {
            self.status = "There is no track at that queue position.".to_owned();
            return;
        };
        let Some(handle) = self.stream_handle.as_ref() else {
            self.status = "Audio output is unavailable on this system.".to_owned();
            return;
        };
        let source = match File::open(&path)
            .map(BufReader::new)
            .map_err(|error| error.to_string())
            .and_then(|reader| Decoder::new(reader).map_err(|error| error.to_string()))
        {
            Ok(source) => source,
            Err(error) => {
                self.status = format!("Could not play {}: {error}", path.display());
                return;
            }
        };
        let duration = source.total_duration();
        let source = EqualizedSource::new(
            source.convert_samples::<f32>(),
            &self.eq_bands,
            self.eq_enabled,
        );
        let sink = match Sink::try_new(handle) {
            Ok(sink) => sink,
            Err(error) => {
                self.status = format!("Could not create audio sink: {error}");
                return;
            }
        };
        sink.append(source);
        self.sink = Some(sink);
        self.queue_index = Some(index);
        self.track_duration = duration;
        self.paused = false;
        self.current_title = self
            .data
            .tracks
            .iter()
            .find(|track| track.path == path)
            .map(|track| format!("{} — {}", track.display_title(), track.album_artist))
            .unwrap_or_else(|| path.display().to_string());
        self.status = format!("Playing {}.", self.current_title);
    }

    fn play_pause(&mut self) {
        let Some(sink) = self.sink.as_ref() else {
            if !self.queue.is_empty() {
                self.play_queue_index(self.queue_index.unwrap_or(0));
            } else {
                self.status = "Add tracks to the queue before playing.".to_owned();
            }
            return;
        };
        if self.paused {
            sink.play();
            self.paused = false;
            self.status = "Playback resumed.".to_owned();
        } else {
            sink.pause();
            self.paused = true;
            self.status = "Playback paused.".to_owned();
        }
    }

    fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self.queue_index = None;
        self.current_title.clear();
        self.track_duration = None;
        self.paused = false;
        self.status = "Playback stopped.".to_owned();
    }

    fn next_track(&mut self) {
        let Some(index) = self.queue_index else {
            if !self.queue.is_empty() {
                self.play_queue_index(0);
            }
            return;
        };
        if index + 1 < self.queue.len() {
            self.play_queue_index(index + 1);
        } else {
            self.stop();
        }
    }

    fn previous_track(&mut self) {
        let Some(index) = self.queue_index else {
            return;
        };
        if index > 0 {
            self.play_queue_index(index - 1);
        } else {
            self.play_queue_index(0);
        }
    }

    fn check_playback_end(&mut self) {
        let ended = self.sink.as_ref().is_some_and(Sink::empty);
        if ended && !self.paused {
            self.next_track();
        }
    }

    fn render_library(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.scan_task.is_none(),
                    egui::Button::new("Add music folder…"),
                )
                .clicked()
            {
                self.add_folder();
            }
            ui.add_enabled_ui(self.scan_task.is_none(), |ui| {
                ui.checkbox(&mut self.scan_subfolders, "Scan subfolders");
            });
            ui.label(format!("{} tracks", self.data.tracks.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("Search title, artist, album, genre…")
                        .desired_width(260.0),
                );
                egui::ComboBox::from_id_salt("library_view")
                    .selected_text(match self.library_view {
                        LibraryView::Tracks => "Tracks",
                        LibraryView::Albums => "Albums",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.library_view,
                            LibraryView::Tracks,
                            "Track view",
                        );
                        ui.selectable_value(
                            &mut self.library_view,
                            LibraryView::Albums,
                            "Album view",
                        );
                    });
                egui::ComboBox::from_id_salt("organize")
                    .selected_text(format!("By {}", self.organize_by))
                    .show_ui(ui, |ui| {
                        for mode in ["Album Artist", "Album", "Genre", "Year"] {
                            ui.selectable_value(&mut self.organize_by, mode.to_owned(), mode);
                        }
                    });
            });
        });
        ui.horizontal(|ui| {
            if ui.button("Add selected to queue").clicked() {
                self.add_selected_to_queue(false);
            }
            if ui.button("Add selected randomly").clicked() {
                self.add_selected_to_queue(true);
            }
            if ui.button("Select all visible").clicked() {
                self.selected_paths
                    .extend(self.visible_tracks().iter().map(|track| track.path.clone()));
            }
            if ui.button("Clear selection").clicked() {
                self.selected_paths.clear();
            }
        });
        let tracks = self.visible_tracks();
        if tracks.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("Your library is empty. Add a folder containing music files.");
            });
        } else if self.library_view == LibraryView::Albums {
            self.render_album_grid(ctx, ui, &tracks);
        } else {
            self.render_track_list(ctx, ui, &tracks);
        }
    }

    fn render_track_list(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, tracks: &[Track]) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_sized([48.0, 20.0], egui::Label::new(""));
                ui.add_sized([210.0, 20.0], egui::Label::new("Track"));
                ui.add_sized([140.0, 20.0], egui::Label::new("Album artist"));
                ui.add_sized([150.0, 20.0], egui::Label::new("Album"));
                ui.add_sized([48.0, 20.0], egui::Label::new("Year"));
                ui.add_sized([58.0, 20.0], egui::Label::new("Type"));
                ui.add_sized([55.0, 20.0], egui::Label::new("Duration"));
            });
            ui.separator();
            let mut previous_group = String::new();
            for track in tracks {
                let group = match self.organize_by.as_str() {
                    "Album" => track.album.clone(),
                    "Genre" => track.genre.clone(),
                    "Year" => track.year.map(|year| year.to_string()).unwrap_or_default(),
                    _ => track.album_artist.clone(),
                };
                if group != previous_group {
                    ui.add_space(5.0);
                    ui.heading(if group.is_empty() {
                        "Unknown".to_owned()
                    } else {
                        group.clone()
                    });
                    previous_group = group;
                }
                let path = track.path.clone();
                let texture = self.load_artwork(ctx, track.artwork_path.as_deref());
                let response = ui
                    .horizontal(|ui| {
                        let mut selected = self.selected_paths.contains(&path);
                        if ui.checkbox(&mut selected, "").changed() {
                            if selected {
                                self.selected_paths.insert(path.clone());
                            } else {
                                self.selected_paths.remove(&path);
                            }
                        }
                        if let Some(texture) = texture {
                            ui.add(
                                egui::Image::from_texture(&texture)
                                    .fit_to_exact_size(egui::vec2(40.0, 40.0)),
                            );
                        } else {
                            ui.add_sized([40.0, 40.0], egui::Label::new("♫"));
                        }
                        if ui
                            .add_sized(
                                [210.0, 40.0],
                                egui::Button::new(track.display_title()).frame(false),
                            )
                            .clicked()
                        {
                            if !self.queue.contains(&path) {
                                self.queue.push(path.clone());
                            }
                            if let Some(index) = self.queue.iter().position(|item| item == &path) {
                                self.play_queue_index(index);
                            }
                        }
                        ui.add_sized([140.0, 40.0], egui::Label::new(&track.album_artist));
                        ui.add_sized([150.0, 40.0], egui::Label::new(&track.album));
                        ui.add_sized(
                            [48.0, 40.0],
                            egui::Label::new(
                                track.year.map(|year| year.to_string()).unwrap_or_default(),
                            ),
                        );
                        ui.add_sized([58.0, 40.0], egui::Label::new(track.file_type()));
                        ui.add_sized([55.0, 40.0], egui::Label::new(track.duration_label()));
                    })
                    .response;
                self.show_track_context_menu(&response, std::slice::from_ref(&path));
                ui.separator();
            }
        });
    }

    fn render_album_grid(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, tracks: &[Track]) {
        let mut albums = std::collections::BTreeMap::<(String, String), Vec<Track>>::new();
        for track in tracks {
            albums
                .entry((track.album_artist.clone(), track.album.clone()))
                .or_default()
                .push(track.clone());
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for ((artist, album), album_tracks) in albums {
                    let Some(representative) = album_tracks.first() else {
                        continue;
                    };
                    let artwork = representative.artwork_path.clone();
                    let paths: Vec<PathBuf> = album_tracks
                        .iter()
                        .map(|track| track.path.clone())
                        .collect();
                    let texture = self.load_artwork(ctx, artwork.as_deref());
                    let card = egui::Frame::group(ui.style())
                        .show(ui, |ui| {
                            ui.set_min_width(168.0);
                            ui.set_max_width(168.0);
                            if let Some(texture) = texture {
                                ui.add(
                                    egui::Image::from_texture(&texture)
                                        .fit_to_exact_size(egui::vec2(144.0, 144.0)),
                                );
                            } else {
                                ui.add_sized([144.0, 144.0], egui::Label::new("♫"));
                            }
                            ui.label(if album.is_empty() {
                                "Unknown album"
                            } else {
                                &album
                            });
                            ui.label(if artist.is_empty() {
                                "Unknown artist"
                            } else {
                                &artist
                            });
                            ui.label(format!("{} tracks", album_tracks.len()));
                        })
                        .response;
                    let card = ui.interact(
                        card.rect,
                        ui.id().with(("album", &artist, &album)),
                        egui::Sense::click(),
                    );
                    if card.clicked() {
                        self.selected_paths.extend(paths.iter().cloned());
                    }
                    self.show_track_context_menu(&card, &paths);
                }
            });
        });
    }

    fn show_track_context_menu(&mut self, response: &egui::Response, paths: &[PathBuf]) {
        let selection_changed = paths.iter().any(|path| !self.selected_paths.contains(path));
        if response.secondary_clicked() && selection_changed {
            self.selected_paths.clear();
            self.selected_paths.extend(paths.iter().cloned());
        }
        response.context_menu(|ui| {
            if ui.button("Add selection to queue").clicked() {
                self.add_selected_to_queue(false);
                ui.close_menu();
            }
            if ui.button("Create playlist from selection").clicked() {
                self.create_playlist_from_selection();
                ui.close_menu();
            }
            if !self
                .data
                .playlists
                .iter()
                .any(|playlist| playlist.smart.is_none())
            {
                ui.label("No static playlists yet");
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
                if ui.button(format!("Add selection to {name}")).clicked() {
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
            let group = match self.organize_by.as_str() {
                "Album" => track.album.to_lowercase(),
                "Genre" => track.genre.to_lowercase(),
                "Year" => track.year.map(|year| year.to_string()).unwrap_or_default(),
                _ => track.album_artist.to_lowercase(),
            };
            (group, track.display_title().to_lowercase())
        });
        tracks
    }

    fn render_playlists(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |columns| {
            columns[0].heading("Your playlists");
            for (index, playlist) in self.data.playlists.iter().enumerate() {
                let label = format!(
                    "{}  ·  {} tracks{}",
                    playlist.name,
                    self.playlist_tracks(playlist).len(),
                    if playlist.smart.is_some() {
                        "  ·  smart"
                    } else {
                        ""
                    }
                );
                if columns[0]
                    .selectable_label(self.active_playlist == Some(index), label)
                    .clicked()
                {
                    self.active_playlist = Some(index);
                }
            }
            columns[0].separator();
            columns[0].label("Static playlist from selected tracks");
            columns[0].horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_playlist_name)
                        .hint_text("Playlist name"),
                );
                if ui.button("Create").clicked() {
                    self.create_static_playlist();
                }
            });
            columns[0].separator();
            columns[0].label("Smart playlist (field contains value)");
            egui::ComboBox::from_id_salt("smart_field")
                .selected_text(["Album artist", "Album", "Genre", "Year"][self.smart_field.min(3)])
                .show_ui(&mut columns[0], |ui| {
                    for (index, field) in ["Album artist", "Album", "Genre", "Year"]
                        .iter()
                        .enumerate()
                    {
                        ui.selectable_value(&mut self.smart_field, index, *field);
                    }
                });
            columns[0].horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.smart_match).hint_text("Matching value"),
                );
            });
            columns[0].horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.smart_playlist_name)
                        .hint_text("Playlist name"),
                );
                if ui.button("Create smart playlist").clicked() {
                    self.create_smart_playlist();
                }
            });

            columns[1].heading(
                self.active_playlist
                    .and_then(|index| self.data.playlists.get(index))
                    .map(|playlist| playlist.name.as_str())
                    .unwrap_or("Playlist tracks"),
            );
            columns[1].horizontal(|ui| {
                if ui.button("Copy to queue").clicked() {
                    self.copy_playlist_to_queue(false);
                }
                if ui.button("Copy to queue randomly").clicked() {
                    self.copy_playlist_to_queue(true);
                }
            });
            let tracks = self
                .active_playlist
                .and_then(|index| self.data.playlists.get(index))
                .map(|playlist| self.playlist_tracks(playlist))
                .unwrap_or_default();
            egui::ScrollArea::vertical().show(&mut columns[1], |ui| {
                for (position, path) in tracks.iter().enumerate() {
                    let label = self
                        .data
                        .tracks
                        .iter()
                        .find(|track| &track.path == path)
                        .map(|track| track.display_title().to_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    if ui
                        .selectable_label(false, format!("{}. {}", position + 1, label))
                        .clicked()
                    {
                        self.queue.push(path.clone());
                        self.play_queue_index(self.queue.len() - 1);
                    }
                }
                if tracks.is_empty() {
                    ui.label("Select a playlist to see its tracks.");
                }
            });
        });
    }

    fn render_queue(&mut self, ui: &mut egui::Ui) {
        ui.heading("Now playing queue");
        ui.horizontal(|ui| {
            if ui.button("Clear queue").clicked() {
                self.queue.clear();
                self.queue_index = None;
                self.stop();
            }
            if ui.button("Shuffle queue").clicked() {
                let current = self
                    .queue_index
                    .and_then(|index| self.queue.get(index).cloned());
                self.queue.shuffle(&mut rand::rng());
                if let Some(current) = current {
                    self.queue_index = self.queue.iter().position(|path| path == &current);
                }
            }
        });
        let queue = self.queue.clone();
        let mut remove_index = None;
        egui::ScrollArea::vertical()
            .max_height(420.0)
            .show(ui, |ui| {
                for (index, path) in queue.iter().enumerate() {
                    let title = self
                        .data
                        .tracks
                        .iter()
                        .find(|track| &track.path == path)
                        .map(|track| track.display_title().to_owned())
                        .unwrap_or_else(|| {
                            path.file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("Unknown")
                                .to_owned()
                        });
                    ui.horizontal(|ui| {
                        let marker = if self.queue_index == Some(index) {
                            "▶"
                        } else {
                            " "
                        };
                        if ui
                            .selectable_label(
                                self.queue_index == Some(index),
                                format!("{marker}  {title}"),
                            )
                            .clicked()
                        {
                            self.play_queue_index(index);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Remove").clicked() {
                                remove_index = Some(index);
                            }
                        });
                    });
                }
            });
        if let Some(index) = remove_index {
            self.queue.remove(index);
            if let Some(current) = self.queue_index {
                if index == current {
                    self.stop();
                } else if index < current {
                    self.queue_index = Some(current - 1);
                }
            }
        }
    }

    fn render_equalization(&mut self, ui: &mut egui::Ui) {
        ui.heading("Equalization");
        ui.label("Five-band equalizer controls");
        ui.checkbox(&mut self.eq_enabled, "Enable equalizer");
        let presets = ["Custom", "Flat", "Bass boost", "Treble boost", "Vocal"];
        egui::ComboBox::from_id_salt("eq_preset")
            .selected_text(presets[self.eq_preset.min(presets.len() - 1)])
            .show_ui(ui, |ui| {
                for (index, preset) in presets.iter().enumerate() {
                    if ui
                        .selectable_value(&mut self.eq_preset, index, *preset)
                        .clicked()
                    {
                        self.eq_bands = match index {
                            1 => [0.0; 5],
                            2 => [6.0, 4.0, 0.0, -1.0, 0.0],
                            3 => [0.0, -1.0, 0.0, 4.0, 6.0],
                            4 => [-2.0, 1.0, 4.0, 2.0, -1.0],
                            _ => self.eq_bands,
                        };
                    }
                }
            });
        ui.add_space(16.0);
        let mut eq_changed = false;
        ui.horizontal(|ui| {
            for (index, label) in ["60 Hz", "250 Hz", "1 kHz", "4 kHz", "12 kHz"]
                .iter()
                .enumerate()
            {
                ui.vertical(|ui| {
                    ui.label(*label);
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
        if eq_changed {
            self.eq_preset = 0;
        }
        ui.separator();
        ui.label("Equalizer changes apply to tracks started after adjusting them.");
    }

    fn render_now_playing_cover(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.heading("Now Playing");
            ui.add_space(12.0);
            let artwork_path = self
                .queue_index
                .and_then(|index| self.queue.get(index))
                .and_then(|path| self.data.tracks.iter().find(|track| &track.path == path))
                .and_then(|track| track.artwork_path.clone());
            if let Some(texture) = self.load_artwork(ctx, artwork_path.as_deref()) {
                let max_side = ui
                    .available_width()
                    .min(ui.available_height() * 0.75)
                    .min(560.0);
                ui.add(
                    egui::Image::from_texture(&texture)
                        .fit_to_exact_size(egui::vec2(max_side, max_side)),
                );
            } else {
                let size = ui
                    .available_width()
                    .min(ui.available_height() * 0.75)
                    .min(560.0);
                ui.add_sized([size, size], egui::Label::new("♫"));
            }
            ui.add_space(12.0);
            ui.heading(if self.current_title.is_empty() {
                "Nothing playing"
            } else {
                &self.current_title
            });
        });
    }
}

impl eframe::App for MusicApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.check_playback_end();
        self.finish_scan_if_ready();
        egui::TopBottomPanel::top("navigation").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("Fugue Jam · v{PROGRAM_VERSION}"));
                ui.separator();
                for tab in [
                    Tab::NowPlaying,
                    Tab::Library,
                    Tab::Playlists,
                    Tab::Equalization,
                ] {
                    if ui.selectable_label(self.tab == tab, tab.title()).clicked() {
                        self.tab = tab;
                    }
                }
            });
        });
        egui::TopBottomPanel::bottom("player_controls").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("⏮").on_hover_text("Previous track").clicked() {
                    self.previous_track();
                }
                let can_resume = self.paused || self.sink.is_none();
                if ui
                    .button(if can_resume { "▶" } else { "⏸" })
                    .on_hover_text(if can_resume { "Play" } else { "Pause" })
                    .clicked()
                {
                    self.play_pause();
                }
                if ui.button("⏹").on_hover_text("Stop").clicked() {
                    self.stop();
                }
                if ui.button("⏭").on_hover_text("Next track").clicked() {
                    self.next_track();
                }
                ui.separator();
                ui.label(if self.current_title.is_empty() {
                    "Nothing playing".to_owned()
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
                                "Finding files…".to_owned()
                            } else {
                                format!("{progress}/{total} files")
                            },
                        ));
                    });
                }
            });
        });
        match self.tab {
            Tab::NowPlaying => {
                let available_width = ctx.available_rect().width();
                let left = egui::SidePanel::left("now_playing_cover")
                    .resizable(true)
                    .default_width(available_width * self.now_playing_left_fraction)
                    .width_range(200.0..=available_width - 200.0)
                    .show(ctx, |ui| self.render_now_playing_cover(ctx, ui));
                self.now_playing_left_fraction =
                    (left.response.rect.width() / available_width).clamp(0.25, 0.9);
                egui::CentralPanel::default().show(ctx, |ui| self.render_queue(ui));
            }
            Tab::Library => {
                egui::CentralPanel::default().show(ctx, |ui| self.render_library(ctx, ui));
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

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn collect_audio_files(
    folder: &Path,
    recursive: bool,
    files: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if recursive && file_type.is_dir() {
            collect_audio_files(&path, recursive, files)?;
        } else if file_type.is_file() && is_supported_audio_file(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn scan_folder(
    folder: &Path,
    recursive: bool,
    artwork_dir: &Path,
    progress: &AtomicUsize,
    total: &AtomicUsize,
) -> Result<Vec<Track>, String> {
    let mut files = Vec::new();
    collect_audio_files(folder, recursive, &mut files).map_err(|error| error.to_string())?;
    files.sort();
    total.store(files.len(), Ordering::Relaxed);
    fs::create_dir_all(artwork_dir).map_err(|error| error.to_string())?;
    let tracks = files
        .into_iter()
        .map(|path| {
            let track = read_track(&path, artwork_dir);
            progress.fetch_add(1, Ordering::Relaxed);
            track
        })
        .collect();
    Ok(tracks)
}

fn is_supported_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            AUDIO_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

fn read_track(path: &Path, artwork_dir: &Path) -> Track {
    let mut track = Track {
        path: path.to_path_buf(),
        title: path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Unknown track")
            .to_owned(),
        ..Track::default()
    };
    let Ok(tagged_file) = Probe::open(path).and_then(|probe| probe.read()) else {
        return track;
    };
    track.duration_secs = Some(tagged_file.properties().duration().as_secs());
    if let Some(tag) = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag())
    {
        let picture = tag
            .get_picture_type(PictureType::CoverFront)
            .or_else(|| tag.pictures().first());
        if let Some(picture) = picture {
            let mut hasher = DefaultHasher::new();
            path.hash(&mut hasher);
            let artwork_path = artwork_dir.join(format!("{:016x}.cover", hasher.finish()));
            if let Err(error) = fs::write(&artwork_path, picture.data()) {
                eprintln!("Could not cache album art from {}: {error}", path.display());
            }
            if artwork_path.exists() {
                track.artwork_path = Some(artwork_path);
            }
        }
        track.title = tag
            .title()
            .map(|value| value.to_string())
            .unwrap_or(track.title);
        track.album_artist = tag
            .get_string(&lofty::tag::ItemKey::AlbumArtist)
            .map(str::to_owned)
            .or_else(|| tag.artist().map(|value| value.into_owned()))
            .unwrap_or_default();
        track.album = tag
            .album()
            .map(|value| value.into_owned())
            .unwrap_or_default();
        track.genre = tag
            .genre()
            .map(|value| value.into_owned())
            .unwrap_or_default();
        track.year = tag.year();
    }
    track
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Fugue Jam")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Fugue Jam",
        options,
        Box::new(|_creation_context| Ok(Box::new(MusicApp::new()))),
    )
}

#[cfg(test)]
mod tests {
    use super::{collect_audio_files, is_supported_audio_file};
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn supported_audio_extensions_are_case_insensitive() {
        assert!(is_supported_audio_file(Path::new("music/track.FLAC")));
        assert!(is_supported_audio_file(Path::new("music/track.ogg")));
        assert!(is_supported_audio_file(Path::new("music/track.m4a")));
        assert!(!is_supported_audio_file(Path::new("music/cover.jpg")));
        assert!(!is_supported_audio_file(Path::new("music/track.opus")));
    }

    #[test]
    fn folder_scanning_can_include_or_skip_subfolders() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("fugue-jam-scan-{}-{nonce}", std::process::id()));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("create scan test folders");
        fs::write(root.join("root.flac"), []).expect("create root track");
        fs::write(nested.join("nested.ogg"), []).expect("create nested track");

        let mut top_level = Vec::new();
        collect_audio_files(&root, false, &mut top_level).expect("scan only root folder");
        assert_eq!(top_level.len(), 1);

        let mut recursive = Vec::new();
        collect_audio_files(&root, true, &mut recursive).expect("scan nested folders");
        assert_eq!(recursive.len(), 2);

        fs::remove_dir_all(root).expect("remove scan test folders");
    }
}

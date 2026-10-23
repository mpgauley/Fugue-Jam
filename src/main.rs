use std::{
    collections::{HashMap, HashSet, hash_map::DefaultHasher},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::BufReader,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use cpal::traits::{DeviceTrait, HostTrait};
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
const EQ_BAND_COUNTS: [usize; 5] = [5, 8, 12, 20, 31];
const PROGRAM_VERSION: u32 = 8;

struct EqualizerControls {
    gains: [AtomicU32; 31],
    band_count: AtomicU32,
    enabled: AtomicBool,
    revision: AtomicU64,
}

impl EqualizerControls {
    fn new(gains: &[f32], enabled: bool) -> Self {
        let mut initial_gains = [0.0; 31];
        let count = gains.len().min(initial_gains.len());
        initial_gains[..count].copy_from_slice(&gains[..count]);
        Self {
            gains: std::array::from_fn(|index| AtomicU32::new(initial_gains[index].to_bits())),
            band_count: AtomicU32::new(gains.len() as u32),
            enabled: AtomicBool::new(enabled),
            revision: AtomicU64::new(0),
        }
    }

    fn update(&self, gains: &[f32], enabled: bool) {
        for (target, gain) in self.gains.iter().zip(gains) {
            target.store(gain.to_bits(), Ordering::Relaxed);
        }
        self.band_count.store(gains.len() as u32, Ordering::Relaxed);
        self.enabled.store(enabled, Ordering::Relaxed);
        self.revision.fetch_add(1, Ordering::Release);
    }

    fn snapshot(&self) -> (Vec<f32>, bool) {
        let count = (self.band_count.load(Ordering::Relaxed) as usize).clamp(5, 31);
        (
            self.gains
                .iter()
                .take(count)
                .map(|gain| f32::from_bits(gain.load(Ordering::Relaxed)))
                .collect(),
            self.enabled.load(Ordering::Relaxed),
        )
    }
}

struct Biquad {
    coefficients: [f32; 5],
    state: Vec<(f32, f32)>,
}

impl Biquad {
    fn peaking(frequency: f32, gain_db: f32, sample_rate: u32, channels: u16) -> Self {
        Self {
            coefficients: Self::coefficients(frequency, gain_db, sample_rate),
            state: vec![(0.0, 0.0); channels.max(1) as usize],
        }
    }

    fn coefficients(frequency: f32, gain_db: f32, sample_rate: u32) -> [f32; 5] {
        if gain_db.abs() < f32::EPSILON {
            return [1.0, 0.0, 0.0, 0.0, 0.0];
        }
        let nyquist = sample_rate as f32 / 2.0;
        let frequency = frequency.min(nyquist * 0.95).max(1.0);
        let omega = std::f32::consts::TAU * frequency / sample_rate as f32;
        let alpha = omega.sin() / (2.0 * 0.8);
        let amplitude = 10.0_f32.powf(gain_db / 40.0);
        let a0 = 1.0 + alpha / amplitude;
        let cosine = omega.cos();
        [
            (1.0 + alpha * amplitude) / a0,
            (-2.0 * cosine) / a0,
            (1.0 - alpha * amplitude) / a0,
            (-2.0 * cosine) / a0,
            (1.0 - alpha / amplitude) / a0,
        ]
    }

    fn set_gain(&mut self, frequency: f32, gain_db: f32, sample_rate: u32) {
        self.coefficients = Self::coefficients(frequency, gain_db, sample_rate);
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
    controls: Arc<EqualizerControls>,
    revision: u64,
}

impl<S: Source<Item = f32>> EqualizedSource<S> {
    fn new(input: S, controls: Arc<EqualizerControls>) -> Self {
        let channels = input.channels().max(1) as usize;
        let (bands, enabled) = controls.snapshot();
        let frequencies = eq_frequencies(bands.len());
        let filters = frequencies
            .iter()
            .zip(&bands)
            .map(|(&frequency, &gain)| {
                Biquad::peaking(
                    frequency,
                    if enabled { gain } else { 0.0 },
                    input.sample_rate(),
                    channels as u16,
                )
            })
            .collect();
        Self {
            input,
            filters,
            channel: 0,
            channels,
            revision: controls.revision.load(Ordering::Acquire),
            controls,
        }
    }
}

impl<S: Source<Item = f32>> Iterator for EqualizedSource<S> {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let mut sample = self.input.next()?;
        if self.channel == 0 {
            let revision = self.controls.revision.load(Ordering::Acquire);
            if revision != self.revision {
                let (bands, enabled) = self.controls.snapshot();
                let frequencies = eq_frequencies(bands.len());
                if frequencies.len() != self.filters.len() {
                    let channels = self.channels as u16;
                    self.filters = frequencies
                        .iter()
                        .zip(&bands)
                        .map(|(&frequency, &gain)| {
                            Biquad::peaking(
                                frequency,
                                if enabled { gain } else { 0.0 },
                                self.input.sample_rate(),
                                channels,
                            )
                        })
                        .collect();
                } else {
                    for ((filter, &frequency), &gain) in
                        self.filters.iter_mut().zip(&frequencies).zip(&bands)
                    {
                        filter.set_gain(
                            frequency,
                            if enabled { gain } else { 0.0 },
                            self.input.sample_rate(),
                        );
                    }
                }
                self.revision = revision;
            }
        }
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

struct SilenceSource;

impl Iterator for SilenceSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        Some(0.0)
    }
}

impl Source for SilenceSource {
    fn current_frame_len(&self) -> Option<usize> {
        Some(4096)
    }

    fn channels(&self) -> u16 {
        2
    }

    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn total_duration(&self) -> Option<Duration> {
        None
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct QueueEntry {
    path: PathBuf,
    title: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredQueueEntry {
    Entry(QueueEntry),
    Path(PathBuf),
}

fn deserialize_queue<'de, D>(deserializer: D) -> Result<Vec<QueueEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let entries = Vec::<StoredQueueEntry>::deserialize(deserializer)?;
    Ok(entries
        .into_iter()
        .map(|entry| match entry {
            StoredQueueEntry::Entry(entry) => entry,
            StoredQueueEntry::Path(path) => QueueEntry {
                title: path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("Unknown")
                    .to_owned(),
                path,
            },
        })
        .collect())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LibraryData {
    folders: Vec<PathBuf>,
    tracks: Vec<Track>,
    playlists: Vec<Playlist>,
    #[serde(default, deserialize_with = "deserialize_queue")]
    queue: Vec<QueueEntry>,
    #[serde(default)]
    library_view: LibraryView,
    #[serde(default = "default_eq_band_count")]
    eq_band_count: usize,
    #[serde(default)]
    eq_bands: Vec<f32>,
    #[serde(default = "default_eq_enabled")]
    eq_enabled: bool,
    #[serde(default = "default_now_playing_left_fraction")]
    now_playing_left_fraction: f32,
}

impl Default for LibraryData {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            tracks: Vec::new(),
            playlists: Vec::new(),
            queue: Vec::new(),
            library_view: LibraryView::default(),
            eq_band_count: default_eq_band_count(),
            eq_bands: Vec::new(),
            eq_enabled: default_eq_enabled(),
            now_playing_left_fraction: default_now_playing_left_fraction(),
        }
    }
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
    queue_index: Option<usize>,
    pending_queue_playlist: Option<PathBuf>,
    new_queue_playlist_name: String,
    filter: String,
    organize_by: String,
    library_view: LibraryView,
    sort_by: String,
    thumbnail_size: f32,
    active_playlist: Option<usize>,
    new_playlist_name: String,
    smart_playlist_name: String,
    smart_field: usize,
    smart_match: String,
    status: String,
    _stream: Option<OutputStream>,
    stream_handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
    hdmi_keepalive: Option<Sink>,
    paused: bool,
    current_title: String,
    track_duration: Option<Duration>,
    eq_bands: Vec<f32>,
    eq_band_count: usize,
    eq_enabled: bool,
    eq_preset: usize,
    eq_controls: Arc<EqualizerControls>,
    waveform: Vec<f32>,
    waveform_track: Option<PathBuf>,
    waveform_requests: Sender<PathBuf>,
    waveform_results: Receiver<Result<(PathBuf, Vec<f32>), (PathBuf, String)>>,
    scan_subfolders: bool,
    scan_task: Option<ScanTask>,
    texture_cache: HashMap<PathBuf, TextureHandle>,
    artwork_requests: Sender<PathBuf>,
    artwork_results: Receiver<Result<(PathBuf, egui::ColorImage), (PathBuf, String)>>,
    artwork_pending: HashSet<PathBuf>,
    artwork_failed: HashSet<PathBuf>,
    library_indices: HashMap<PathBuf, usize>,
    now_playing_left_fraction: f32,
}

struct ScanTask {
    folder: PathBuf,
    progress: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    receiver: Receiver<ScanMessage>,
    added_count: usize,
    refreshed_count: usize,
    remove_missing: bool,
    recursive: bool,
    seen_tracks: HashSet<PathBuf>,
}

enum ScanMessage {
    Total(usize),
    Track(Track),
    Finished(Result<(), String>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum LibraryView {
    Album,
    AlbumArtist,
    #[default]
    Tracks,
}

fn default_eq_band_count() -> usize {
    5
}

fn default_eq_enabled() -> bool {
    true
}

fn default_now_playing_left_fraction() -> f32 {
    0.75
}

fn supported_eq_band_count(count: usize) -> usize {
    if EQ_BAND_COUNTS.contains(&count) {
        count
    } else {
        5
    }
}

fn eq_frequencies(count: usize) -> Vec<f32> {
    if count == 5 {
        return vec![60.0, 250.0, 1_000.0, 4_000.0, 12_000.0];
    }
    let (minimum, maximum): (f32, f32) = if count == 8 {
        (30.0, 18_000.0)
    } else {
        (20.0, 20_000.0)
    };
    (0..count)
        .map(|index| {
            let fraction = index as f32 / (count - 1) as f32;
            minimum * (maximum / minimum).powf(fraction)
        })
        .collect()
}

fn interpolate_gain(frequency: f32, source_frequencies: &[f32], gains: &[f32]) -> f32 {
    if frequency <= source_frequencies[0] {
        return gains[0];
    }
    if frequency >= *source_frequencies.last().unwrap_or(&frequency) {
        return *gains.last().unwrap_or(&0.0);
    }
    let upper = source_frequencies
        .iter()
        .position(|source| *source >= frequency)
        .unwrap_or(source_frequencies.len() - 1);
    let lower = upper - 1;
    let low_log = source_frequencies[lower].ln();
    let high_log = source_frequencies[upper].ln();
    let amount = (frequency.ln() - low_log) / (high_log - low_log);
    gains[lower] + (gains[upper] - gains[lower]) * amount
}

fn remap_eq_gains(gains: &[f32], target_count: usize) -> Vec<f32> {
    let source_frequencies = eq_frequencies(gains.len());
    eq_frequencies(target_count)
        .iter()
        .map(|frequency| interpolate_gain(*frequency, &source_frequencies, gains))
        .collect()
}

fn preset_eq_gains(preset: usize, count: usize) -> Vec<f32> {
    let anchors = match preset {
        2 => [6.0, 4.0, 0.0, -1.0, 0.0],
        3 => [0.0, -1.0, 0.0, 4.0, 6.0],
        4 => [-2.0, 1.0, 4.0, 2.0, -1.0],
        _ => [0.0; 5],
    };
    if count == 5 {
        anchors.to_vec()
    } else {
        remap_eq_gains(&anchors, count)
    }
}

#[derive(Clone)]
struct GalleryItem {
    label: String,
    group: String,
    album_artist: String,
    album: String,
    year: Option<u32>,
    artwork_path: Option<PathBuf>,
    tracks: Vec<PathBuf>,
}

enum GalleryRow {
    Heading(String),
    Items(Vec<GalleryItem>),
}

#[derive(Clone, Copy)]
enum QueueAction {
    Remove,
    PlayNext,
    MoveToEnd,
    AddToPlaylist(usize),
    CreatePlaylist,
}

impl MusicApp {
    fn new() -> Self {
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
        for entry in &mut data.queue {
            if let Some(track) = library_indices
                .get(&entry.path)
                .and_then(|index| data.tracks.get(*index))
            {
                entry.title = track.display_title().to_owned();
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
            filter: String::new(),
            organize_by: "Album Artist".to_owned(),
            library_view,
            sort_by: "Album Name".to_owned(),
            thumbnail_size: 144.0,
            active_playlist: None,
            new_playlist_name: String::new(),
            smart_playlist_name: String::new(),
            smart_field: 0,
            smart_match: String::new(),
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

    fn save(&mut self) {
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

    fn add_folder(&mut self) {
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.start_folder_scan(folder, false);
    }

    fn rescan_folder(&mut self) {
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.start_folder_scan(folder, true);
    }

    fn start_folder_scan(&mut self, folder: PathBuf, remove_missing: bool) {
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
        });
    }

    fn finish_scan_if_ready(&mut self) {
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
                        entry.title = track.display_title().to_owned();
                    }
                    if let Some(index) = self.library_indices.get(&track.path).copied() {
                        self.data.tracks[index] = track;
                        refreshed_count += 1;
                    } else {
                        self.library_indices
                            .insert(track.path.clone(), self.data.tracks.len());
                        self.data.tracks.push(track);
                        added_count += 1;
                    }
                }
                ScanMessage::Finished(result) => finish_result = Some(result),
            }
        }
        if finish_result.is_some() {
            self.data
                .tracks
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
            let (folder, added_count, refreshed_count, remove_missing, recursive, seen_tracks) =
                self.scan_task
                    .as_ref()
                    .map(|task| {
                        (
                            task.folder.clone(),
                            task.added_count,
                            task.refreshed_count,
                            task.remove_missing,
                            task.recursive,
                            task.seen_tracks.clone(),
                        )
                    })
                    .unwrap_or_default();
            match result {
                Ok(()) => {
                    let removed_count = if remove_missing {
                        self.remove_missing_tracks(&folder, recursive, &seen_tracks)
                    } else {
                        0
                    };
                    self.status = format!(
                        "Added {added_count}, refreshed {refreshed_count}, and removed {removed_count} missing track(s) from {}.",
                        folder.display(),
                    );
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
            self.status = format!(
                "Scanning {}… {} track(s) loaded",
                task.folder.display(),
                task.progress.load(Ordering::Relaxed)
            );
        }
    }

    fn remove_missing_tracks(
        &mut self,
        folder: &Path,
        recursive: bool,
        seen_tracks: &HashSet<PathBuf>,
    ) -> usize {
        let removed: HashSet<PathBuf> = self
            .data
            .tracks
            .iter()
            .filter(|track| {
                is_in_scan_scope(&track.path, folder, recursive)
                    && !seen_tracks.contains(&track.path)
            })
            .map(|track| track.path.clone())
            .collect();
        if removed.is_empty() {
            return 0;
        }
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
        self.data
            .tracks
            .retain(|track| !removed.contains(&track.path));
        self.data.playlists.iter_mut().for_each(|playlist| {
            playlist.tracks.retain(|path| !removed.contains(path));
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
        removed.len()
    }

    fn queue_entry(&self, path: PathBuf) -> QueueEntry {
        let title = self
            .library_indices
            .get(&path)
            .and_then(|index| self.data.tracks.get(*index))
            .map(|track| track.display_title().to_owned())
            .or_else(|| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "Unknown".to_owned());
        QueueEntry { path, title }
    }

    fn finish_waveform_loads(&mut self) {
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

    fn load_artwork(&mut self, path: Option<&Path>) -> Option<TextureHandle> {
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

    fn finish_artwork_loads(&mut self, ctx: &egui::Context) {
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
        let entries = selected
            .into_iter()
            .map(|path| self.queue_entry(path))
            .collect::<Vec<_>>();
        self.data.queue.extend(entries);
        self.status = format!("Added {count} selected track(s) to the queue.");
        self.save();
    }

    fn play_selected_track(&mut self) {
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

    fn remove_queue_track(&mut self, index: usize) {
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

    fn play_queue_track_next(&mut self, index: usize) {
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

    fn move_queue_track_to_end(&mut self, index: usize) {
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

    fn add_queue_track_to_playlist(&mut self, path: &Path, playlist_index: usize) {
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
        let Some(path) = self.data.queue.get(index).map(|entry| entry.path.clone()) else {
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
            Arc::clone(&self.eq_controls),
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
        self.waveform.clear();
        self.waveform_track = Some(path.clone());
        if self.waveform_requests.send(path.clone()).is_err() {
            self.status = "Waveform generator is unavailable.".to_owned();
        }
        self.current_title = self
            .data
            .tracks
            .iter()
            .find(|track| track.path == path)
            .map(|track| format!("{} — {}", track.display_title(), track.album_artist))
            .unwrap_or_else(|| path.display().to_string());
        self.status = format!("Playing {}.", self.current_title);
        self.save();
    }

    fn play_pause(&mut self) {
        let Some(sink) = self.sink.as_ref() else {
            if !self.data.queue.is_empty() {
                self.play_queue_index(self.queue_index.unwrap_or(0));
            } else {
                self.status = "Add tracks to the queue before playing.".to_owned();
            }
            return;
        };
        if self.paused {
            sink.play();
            if let Some(keepalive) = &self.hdmi_keepalive {
                keepalive.pause();
            }
            self.paused = false;
            self.status = "Playback resumed.".to_owned();
        } else {
            sink.pause();
            if let Some(keepalive) = &self.hdmi_keepalive {
                keepalive.play();
            }
            self.paused = true;
            self.status = "Playback paused.".to_owned();
        }
    }

    fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        if let Some(keepalive) = &self.hdmi_keepalive {
            keepalive.pause();
        }
        self.queue_index = None;
        self.current_title.clear();
        self.track_duration = None;
        self.paused = false;
        self.waveform.clear();
        self.waveform_track = None;
        self.status = "Playback stopped.".to_owned();
        self.save();
    }

    fn next_track(&mut self) {
        let Some(index) = self.queue_index else {
            if !self.data.queue.is_empty() {
                self.play_queue_index(0);
            }
            return;
        };
        if index + 1 < self.data.queue.len() {
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

    fn render_library(&mut self, ui: &mut egui::Ui) {
        let previous_library_view = self.library_view;
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
            if ui
                .add_enabled(
                    self.scan_task.is_none() && !self.data.folders.is_empty(),
                    egui::Button::new("Scan for changes…"),
                )
                .on_hover_text("Choose a folder to discover new files and remove missing files")
                .clicked()
            {
                self.rescan_folder();
            }
            ui.add_enabled_ui(self.scan_task.is_none(), |ui| {
                ui.checkbox(&mut self.scan_subfolders, "Scan subfolders");
            });
            ui.label(format!("{} tracks", self.data.tracks.len()));
        });
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("group_by")
                .selected_text(format!("Group By: {}", self.organize_by))
                .show_ui(ui, |ui| {
                    for mode in ["Album Artist", "Genre", "Decade", "No Grouping"] {
                        ui.selectable_value(&mut self.organize_by, mode.to_owned(), mode);
                    }
                });
            egui::ComboBox::from_id_salt("display")
                .selected_text(format!(
                    "Display: {}",
                    match self.library_view {
                        LibraryView::Album => "Album",
                        LibraryView::AlbumArtist => "Album Artist",
                        LibraryView::Tracks => "Tracks",
                    }
                ))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.library_view, LibraryView::Album, "Album");
                    ui.selectable_value(
                        &mut self.library_view,
                        LibraryView::AlbumArtist,
                        "Album Artist",
                    );
                    ui.selectable_value(&mut self.library_view, LibraryView::Tracks, "Tracks");
                });
            egui::ComboBox::from_id_salt("sort_by")
                .selected_text(format!("Sort By: {}", self.sort_by))
                .show_ui(ui, |ui| {
                    for mode in ["Album Name", "Album Artist", "Year"] {
                        ui.selectable_value(&mut self.sort_by, mode.to_owned(), mode);
                    }
                });
            ui.add(
                egui::Slider::new(&mut self.thumbnail_size, 72.0..=240.0)
                    .text("Thumbnail size")
                    .suffix(" px"),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("Search title, artist, album, genre…")
                        .desired_width(220.0),
                );
            });
        });
        ui.horizontal(|ui| {
            if ui.button("Add selected to queue").clicked() {
                self.add_selected_to_queue(false);
            }
            if ui.button("Add selected randomly").clicked() {
                self.add_selected_to_queue(true);
            }
            if ui.button("Play selection").clicked() {
                self.play_selected_track();
            }
            if ui.button("Select all visible").clicked() {
                self.selected_paths
                    .extend(self.visible_tracks().iter().map(|track| track.path.clone()));
            }
            if ui.button("Clear selection").clicked() {
                self.selected_paths.clear();
            }
        });
        if self.library_view != previous_library_view {
            self.save();
        }
        let tracks = self.visible_tracks();
        if tracks.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("Your library is empty. Add a folder containing music files.");
            });
        } else if self.library_view != LibraryView::Tracks {
            self.render_album_grid(ui, &tracks);
        } else {
            self.render_track_list(ui, &tracks);
        }
    }

    fn render_track_list(&mut self, ui: &mut egui::Ui, tracks: &[Track]) {
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
                    let group_label = if group.is_empty() { "Unknown" } else { &group };
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
                                                    "Unknown"
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
        let selection_changed = paths.iter().any(|path| !self.selected_paths.contains(path));
        if response.secondary_clicked() && selection_changed {
            self.selected_paths.clear();
            self.selected_paths.extend(paths.iter().cloned());
        }
        response.context_menu(|ui| {
            if ui.button("Play selection").clicked() {
                self.play_selected_track();
                ui.close_menu();
            }
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
            (
                track_group_value(track, &self.organize_by).to_lowercase(),
                track_sort_value(track, &self.sort_by),
                track.display_title().to_lowercase(),
            )
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
                        let entry = self.queue_entry(path.clone());
                        self.data.queue.push(entry);
                        self.save();
                        self.play_queue_index(self.data.queue.len() - 1);
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
                self.data.queue.clear();
                self.queue_index = None;
                self.stop();
            }
            if ui.button("Shuffle queue").clicked() {
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
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
        egui::ScrollArea::vertical().max_height(420.0).show_rows(
            ui,
            row_height,
            self.data.queue.len(),
            |ui, rows| {
                for index in rows {
                    let title = &self.data.queue[index].title;
                    ui.set_min_height(row_height);
                    ui.horizontal(|ui| {
                        let marker = if self.queue_index == Some(index) {
                            "▶"
                        } else {
                            " "
                        };
                        let response = ui.add(
                            egui::Label::new(format!("{marker}  {title}"))
                                .sense(egui::Sense::click()),
                        );
                        if response.clicked() {
                            play_index = Some(index);
                        }
                        response.context_menu(|ui| {
                            if ui.button("Remove Track").clicked() {
                                action = Some((index, QueueAction::Remove));
                                ui.close_menu();
                            }
                            if ui.button("Play Track Next").clicked() {
                                action = Some((index, QueueAction::PlayNext));
                                ui.close_menu();
                            }
                            if ui.button("Move Track to the End").clicked() {
                                action = Some((index, QueueAction::MoveToEnd));
                                ui.close_menu();
                            }
                            ui.menu_button("Add Track to Playlist…", |ui| {
                                if ui.button("Create new playlist…").clicked() {
                                    action = Some((index, QueueAction::CreatePlaylist));
                                    ui.close_menu();
                                }
                                if static_playlists.is_empty() {
                                    ui.label("No static playlists yet");
                                }
                                for (playlist_index, name) in &static_playlists {
                                    if ui.button(name).clicked() {
                                        action = Some((
                                            index,
                                            QueueAction::AddToPlaylist(*playlist_index),
                                        ));
                                        ui.close_menu();
                                    }
                                }
                            });
                        });
                    });
                }
            },
        );
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
            egui::Window::new("Create playlist")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("Create a static playlist with this track.");
                    ui.text_edit_singleline(&mut self.new_queue_playlist_name);
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() {
                            create = true;
                        }
                        if ui.button("Cancel").clicked() {
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

    fn render_equalization(&mut self, ui: &mut egui::Ui) {
        ui.heading("Equalization");
        ui.label(format!("{}-band equalizer controls", self.eq_band_count));
        let mut controls_changed = ui
            .checkbox(&mut self.eq_enabled, "Enable equalizer")
            .changed();
        egui::ComboBox::from_id_salt("eq_band_count")
            .selected_text(format!("{} bands", self.eq_band_count))
            .show_ui(ui, |ui| {
                for count in EQ_BAND_COUNTS {
                    if ui
                        .selectable_value(&mut self.eq_band_count, count, format!("{count} bands"))
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
            .selected_text(presets[self.eq_preset.min(presets.len() - 1)])
            .show_ui(ui, |ui| {
                for (index, preset) in presets.iter().enumerate() {
                    if ui
                        .selectable_value(&mut self.eq_preset, index, *preset)
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
        ui.label("Frequency range: 60 Hz–12 kHz for 5 bands; 30 Hz–18 kHz for 8 bands; 20 Hz–20 kHz for 12–31 bands.");
        ui.label("Equalizer changes apply immediately during playback.");
    }

    fn render_now_playing_cover(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.heading("Now Playing");
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
            if let Some(texture) = self.load_artwork(artwork_path.as_deref()) {
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
        self.finish_artwork_loads(ctx);
        self.finish_waveform_loads();
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
        egui::TopBottomPanel::bottom("track_timeline")
            .exact_height((ctx.screen_rect().height() * 0.10).clamp(56.0, 128.0))
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), ui.available_height()),
                    egui::Sense::click(),
                );
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 4.0, ui.visuals().faint_bg_color);
                let position = self.sink.as_ref().map(Sink::get_pos).unwrap_or_default();
                let duration = self.track_duration;
                let fraction = duration
                    .filter(|duration| !duration.is_zero())
                    .map(|duration| {
                        (position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
                    })
                    .unwrap_or(0.0);
                let label = format!(
                    "{}  {} / {}",
                    if self.current_title.is_empty() {
                        "Nothing playing"
                    } else {
                        &self.current_title
                    },
                    format_duration(position),
                    duration
                        .map(format_duration)
                        .unwrap_or_else(|| "—:—".to_owned())
                );
                painter.text(
                    egui::pos2(rect.left() + 10.0, rect.top() + 8.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::TextStyle::Body.resolve(ui.style()),
                    ui.visuals().text_color(),
                );
                let waveform_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 8.0, rect.top() + 32.0),
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
                        egui::Stroke::new(bar_width.max(1.0), color),
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

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn format_frequency(frequency: f32) -> String {
    if frequency >= 1_000.0 {
        format!("{:.1} kHz", frequency / 1_000.0)
    } else {
        format!("{frequency:.0} Hz")
    }
}

fn is_in_scan_scope(path: &Path, folder: &Path, recursive: bool) -> bool {
    path.starts_with(folder) && (recursive || path.parent() == Some(folder))
}

fn track_group_value(track: &Track, organize_by: &str) -> String {
    match organize_by {
        "Genre" => track.genre.clone(),
        "Decade" => track
            .year
            .map(|year| format!("{}s", year / 10 * 10))
            .unwrap_or_else(|| "Unknown decade".to_owned()),
        "No Grouping" => String::new(),
        _ => track.album_artist.clone(),
    }
}

fn track_sort_value(track: &Track, sort_by: &str) -> (String, u32) {
    match sort_by {
        "Album Artist" => (track.album_artist.to_lowercase(), 0),
        "Year" => (String::new(), track.year.unwrap_or(u32::MAX)),
        _ => (track.album.to_lowercase(), 0),
    }
}

fn inverted_color(color: egui::Color32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        255 - color.r(),
        255 - color.g(),
        255 - color.b(),
        color.a(),
    )
}

fn gallery_items(
    tracks: &[Track],
    display: LibraryView,
    group_by: &str,
    sort_by: &str,
) -> Vec<GalleryItem> {
    let mut entries = std::collections::BTreeMap::<String, Vec<&Track>>::new();
    for track in tracks {
        let key = match display {
            LibraryView::Album => format!("{}\0{}", track.album_artist, track.album),
            LibraryView::AlbumArtist => track.album_artist.clone(),
            LibraryView::Tracks => continue,
        };
        entries.entry(key).or_default().push(track);
    }

    let mut items: Vec<GalleryItem> = entries
        .into_values()
        .filter_map(|album_tracks| {
            let representative = *album_tracks.first()?;
            let label = match display {
                LibraryView::Album => representative.album.clone(),
                LibraryView::AlbumArtist => representative.album_artist.clone(),
                LibraryView::Tracks => return None,
            };
            let group = match group_by {
                "Genre" => {
                    if representative.genre.is_empty() {
                        "Unknown genre".to_owned()
                    } else {
                        representative.genre.clone()
                    }
                }
                "Decade" => track_group_value(representative, "Decade"),
                "No Grouping" => String::new(),
                _ => {
                    if representative.album_artist.is_empty() {
                        "Unknown artist".to_owned()
                    } else {
                        representative.album_artist.clone()
                    }
                }
            };
            Some(GalleryItem {
                label,
                group,
                album_artist: representative.album_artist.clone(),
                album: representative.album.clone(),
                year: representative.year,
                artwork_path: album_tracks
                    .iter()
                    .find_map(|track| track.artwork_path.clone()),
                tracks: album_tracks
                    .iter()
                    .map(|track| track.path.clone())
                    .collect(),
            })
        })
        .collect();
    items.sort_by_cached_key(|item| match sort_by {
        "Album Artist" => (item.album_artist.to_lowercase(), String::new(), 0),
        "Year" => (String::new(), String::new(), item.year.unwrap_or(u32::MAX)),
        _ => (item.album.to_lowercase(), String::new(), 0),
    });
    items
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
    sender: &Sender<ScanMessage>,
) -> Result<(), String> {
    let mut files = Vec::new();
    collect_audio_files(folder, recursive, &mut files).map_err(|error| error.to_string())?;
    files.sort();
    total.store(files.len(), Ordering::Relaxed);
    sender
        .send(ScanMessage::Total(files.len()))
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(artwork_dir).map_err(|error| error.to_string())?;
    for path in files {
        let track = read_track(&path, artwork_dir);
        progress.fetch_add(1, Ordering::Relaxed);
        sender
            .send(ScanMessage::Track(track))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn decode_waveform(path: &Path) -> Result<Vec<f32>, String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    let decoder = Decoder::new(BufReader::new(file)).map_err(|error| error.to_string())?;
    let mut peaks = Vec::new();
    let mut peak = 0.0_f32;
    let mut sample_count = 0_u32;
    for sample in decoder.convert_samples::<f32>() {
        peak = peak.max(sample.abs());
        sample_count += 1;
        if sample_count == 2048 {
            peaks.push(peak);
            peak = 0.0;
            sample_count = 0;
        }
    }
    if sample_count > 0 {
        peaks.push(peak);
    }
    if peaks.len() > 256 {
        let source = std::mem::take(&mut peaks);
        peaks = (0..256)
            .map(|index| {
                let start = index * source.len() / 256;
                let end = ((index + 1) * source.len() / 256).max(start + 1);
                source[start..end.min(source.len())]
                    .iter()
                    .copied()
                    .fold(0.0, f32::max)
            })
            .collect();
    }
    Ok(peaks)
}

fn decode_artwork(path: &Path) -> Result<egui::ColorImage, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let image = image::load_from_memory(&bytes).map_err(|error| error.to_string())?;
    let thumbnail = image.thumbnail(640, 640).to_rgba8();
    let size = [thumbnail.width() as usize, thumbnail.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        size,
        thumbnail.as_raw(),
    ))
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
            picture.data().hash(&mut hasher);
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
    use super::{
        EQ_BAND_COUNTS, LibraryData, LibraryView, QueueEntry, Track, collect_audio_files,
        decode_artwork, eq_frequencies, gallery_items, is_in_scan_scope, is_supported_audio_file,
        remap_eq_gains, track_group_value,
    };
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

    #[test]
    fn artwork_is_decoded_and_resized_for_thumbnails() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("fugue-jam-art-{}/{nonce}.png", std::process::id()));
        fs::create_dir_all(path.parent().expect("artwork test parent"))
            .expect("create artwork test folder");
        image::RgbaImage::new(1200, 600)
            .save(&path)
            .expect("write test artwork");

        let thumbnail = decode_artwork(&path).expect("decode test artwork");
        assert_eq!(thumbnail.size, [640, 320]);

        fs::remove_dir_all(path.parent().expect("artwork test parent"))
            .expect("remove artwork test folder");
    }

    #[test]
    fn decade_grouping_and_gallery_modes_use_expected_items() {
        let tracks = vec![
            Track {
                path: Path::new("music/a.flac").to_path_buf(),
                title: "A".to_owned(),
                album_artist: "Artist".to_owned(),
                album: "First".to_owned(),
                year: Some(1987),
                ..Track::default()
            },
            Track {
                path: Path::new("music/b.flac").to_path_buf(),
                title: "B".to_owned(),
                album_artist: "Artist".to_owned(),
                album: "First".to_owned(),
                year: Some(1988),
                ..Track::default()
            },
            Track {
                path: Path::new("music/c.flac").to_path_buf(),
                title: "C".to_owned(),
                album_artist: "Artist".to_owned(),
                album: "Second".to_owned(),
                year: Some(1992),
                ..Track::default()
            },
        ];

        assert_eq!(track_group_value(&tracks[0], "Decade"), "1980s");
        assert_eq!(
            gallery_items(&tracks, LibraryView::Album, "Decade", "Album Name").len(),
            2
        );
        assert_eq!(
            gallery_items(
                &tracks,
                LibraryView::AlbumArtist,
                "No Grouping",
                "Album Artist"
            )
            .len(),
            1
        );
    }

    #[test]
    fn scan_scope_respects_recursive_setting() {
        let folder = Path::new("/music");
        assert!(is_in_scan_scope(
            Path::new("/music/track.flac"),
            folder,
            false
        ));
        assert!(!is_in_scan_scope(
            Path::new("/music/album/track.flac"),
            folder,
            false
        ));
        assert!(is_in_scan_scope(
            Path::new("/music/album/track.flac"),
            folder,
            true
        ));
        assert!(!is_in_scan_scope(
            Path::new("/music-old/track.flac"),
            folder,
            true
        ));
    }

    #[test]
    fn saved_library_defaults_and_round_trips_queue() {
        let existing_library = r#"{"folders":[],"tracks":[],"playlists":[]}"#;
        let mut library: LibraryData =
            serde_json::from_str(existing_library).expect("load previous library format");
        assert!(library.queue.is_empty());

        let old_queue_library =
            r#"{"folders":[],"tracks":[],"playlists":[],"queue":["/music/track.flac"]}"#;
        let legacy: LibraryData =
            serde_json::from_str(old_queue_library).expect("load path-only queue");
        assert_eq!(legacy.queue[0].path, Path::new("/music/track.flac"));
        assert_eq!(legacy.queue[0].title, "track");

        library.queue.push(QueueEntry {
            path: Path::new("/music/track.flac").to_path_buf(),
            title: "Tagged title".to_owned(),
        });
        let saved = serde_json::to_string(&library).expect("serialize library and queue");
        let restored: LibraryData =
            serde_json::from_str(&saved).expect("restore serialized library");
        assert_eq!(restored.queue[0].path, library.queue[0].path);
        assert_eq!(restored.queue[0].title, "Tagged title");
    }

    #[test]
    fn equalizer_band_options_use_expected_frequency_ranges() {
        assert_eq!(EQ_BAND_COUNTS, [5, 8, 12, 20, 31]);
        for count in EQ_BAND_COUNTS {
            assert_eq!(eq_frequencies(count).len(), count);
        }
        assert_eq!(eq_frequencies(5).first(), Some(&60.0));
        assert_eq!(eq_frequencies(5).last(), Some(&12_000.0));
        assert_eq!(eq_frequencies(8).first(), Some(&30.0));
        assert_eq!(eq_frequencies(8).last(), Some(&18_000.0));
        for count in [12, 20, 31] {
            assert_eq!(eq_frequencies(count).first(), Some(&20.0));
            assert_eq!(eq_frequencies(count).last(), Some(&20_000.0));
        }
        assert_eq!(remap_eq_gains(&[0.0; 5], 31), vec![0.0; 31]);
    }

    #[test]
    fn saved_view_and_equalizer_settings_round_trip() {
        let library = LibraryData {
            library_view: LibraryView::Album,
            eq_band_count: 8,
            eq_bands: vec![1.0; 8],
            eq_enabled: false,
            now_playing_left_fraction: 0.6,
            ..LibraryData::default()
        };
        let serialized = serde_json::to_string(&library).expect("serialize player settings");
        let restored: LibraryData =
            serde_json::from_str(&serialized).expect("restore player settings");
        assert_eq!(restored.library_view, LibraryView::Album);
        assert_eq!(restored.eq_band_count, 8);
        assert_eq!(restored.eq_bands, vec![1.0; 8]);
        assert!(!restored.eq_enabled);
        assert!((restored.now_playing_left_fraction - 0.6).abs() < f32::EPSILON);
    }
}

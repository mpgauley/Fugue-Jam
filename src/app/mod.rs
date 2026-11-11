mod audio;
mod library;
mod playback;
mod playlist;
mod scan;
mod state;
#[cfg(test)]
mod tests;
mod ui;

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use crate::localization::{Language, tr};
use cpal::traits::{DeviceTrait, HostTrait};
use eframe::egui::{self, TextureHandle};
use rand::seq::SliceRandom;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use serde::{Deserialize, Serialize};

use self::{audio::*, library::*};

pub(crate) const PROGRAM_VERSION: u32 = 25;
const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aiff", "aif", "flac", "m4a", "mp3", "oga", "ogg", "wav",
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Track {
    path: PathBuf,
    title: String,
    #[serde(default)]
    artist: String,
    album_artist: String,
    album: String,
    genre: String,
    year: Option<u32>,
    #[serde(default)]
    track_number: Option<String>,
    #[serde(default)]
    disc_number: Option<String>,
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum SmartField {
    #[default]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum SmartMatch {
    #[default]
    Contains,
    DoesNotContain,
}

impl SmartMatch {
    fn matches(self, value: &str, query: &str) -> bool {
        let contains = value.to_lowercase().contains(&query.trim().to_lowercase());
        match self {
            Self::Contains => contains,
            Self::DoesNotContain => !contains,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum SmartOperator {
    #[default]
    And,
    Or,
}

impl SmartOperator {
    fn precedence(self) -> u8 {
        match self {
            Self::And => 2,
            Self::Or => 1,
        }
    }

    fn combine(self, left: bool, right: bool) -> bool {
        match self {
            Self::And => left && right,
            Self::Or => left || right,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SmartCondition {
    field: SmartField,
    query: String,
    #[serde(default)]
    comparison: SmartMatch,
    open_parentheses: u8,
    close_parentheses: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SmartPlaylistQuery {
    conditions: Vec<SmartCondition>,
    operators: Vec<SmartOperator>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredSmartPlaylistQuery {
    Query(SmartPlaylistQuery),
    Legacy((SmartField, String)),
}

fn deserialize_smart_playlist<'de, D>(
    deserializer: D,
) -> Result<Option<SmartPlaylistQuery>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<StoredSmartPlaylistQuery>::deserialize(deserializer).map(|stored| {
        stored.map(|stored| match stored {
            StoredSmartPlaylistQuery::Query(query) => query,
            StoredSmartPlaylistQuery::Legacy((field, query)) => SmartPlaylistQuery {
                conditions: vec![SmartCondition {
                    field,
                    query,
                    ..SmartCondition::default()
                }],
                operators: Vec::new(),
            },
        })
    })
}

impl SmartPlaylistQuery {
    fn is_valid(&self) -> bool {
        if self.conditions.is_empty()
            || self.operators.len() + 1 != self.conditions.len()
            || self
                .conditions
                .iter()
                .any(|condition| condition.query.trim().is_empty())
        {
            return false;
        }
        let mut depth = 0_i32;
        for condition in &self.conditions {
            depth += i32::from(condition.open_parentheses);
            depth -= i32::from(condition.close_parentheses);
            if depth < 0 {
                return false;
            }
        }
        depth == 0
    }

    fn matches(&self, track: &Track) -> bool {
        if !self.is_valid() {
            return false;
        }
        let mut values = Vec::with_capacity(self.conditions.len());
        let mut operators = Vec::new();
        let close_group = |values: &mut Vec<bool>, operators: &mut Vec<Option<SmartOperator>>| {
            if let (Some(right), Some(left), Some(Some(operator))) =
                (values.pop(), values.pop(), operators.pop())
            {
                values.push(operator.combine(left, right));
            }
        };
        for (index, condition) in self.conditions.iter().enumerate() {
            for _ in 0..condition.open_parentheses {
                operators.push(None);
            }
            values.push(
                condition
                    .comparison
                    .matches(&condition.field.value(track), &condition.query),
            );
            for _ in 0..condition.close_parentheses {
                while operators.last().is_some_and(Option::is_some) {
                    close_group(&mut values, &mut operators);
                }
                if operators.pop().is_none() {
                    return false;
                }
            }
            if let Some(&next_operator) = self.operators.get(index) {
                while operators.last().is_some_and(|operator| {
                    operator.is_some_and(|top| top.precedence() >= next_operator.precedence())
                }) {
                    close_group(&mut values, &mut operators);
                }
                operators.push(Some(next_operator));
            }
        }
        while !operators.is_empty() {
            if operators.last().is_some_and(Option::is_none) {
                return false;
            }
            close_group(&mut values, &mut operators);
        }
        values.as_slice() == [true]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Playlist {
    name: String,
    tracks: Vec<PathBuf>,
    #[serde(default, deserialize_with = "deserialize_smart_playlist")]
    smart: Option<SmartPlaylistQuery>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct QueueEntry {
    path: PathBuf,
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    genre: String,
    year: Option<u32>,
    track_number: Option<String>,
    disc_number: Option<String>,
    duration_secs: Option<u64>,
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
            StoredQueueEntry::Path(path) => QueueEntry::from_path(path),
        })
        .collect())
}

impl QueueEntry {
    fn from_path(path: PathBuf) -> Self {
        let title = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Unknown")
            .to_owned();
        Self {
            path,
            title,
            ..Self::default()
        }
    }

    fn from_track(track: &Track, fields: &[QueueDisplayField]) -> Self {
        let mut entry = Self {
            path: track.path.clone(),
            title: track.display_title().to_owned(),
            ..Self::default()
        };
        entry.update_fields(track, fields);
        entry
    }

    fn update_fields(&mut self, track: &Track, fields: &[QueueDisplayField]) {
        self.path.clone_from(&track.path);
        self.title = track.display_title().to_owned();
        self.artist = fields
            .contains(&QueueDisplayField::Artist)
            .then(|| track.artist.clone())
            .unwrap_or_default();
        self.album_artist = fields
            .contains(&QueueDisplayField::AlbumArtist)
            .then(|| track.album_artist.clone())
            .unwrap_or_default();
        self.album = fields
            .contains(&QueueDisplayField::Album)
            .then(|| track.album.clone())
            .unwrap_or_default();
        self.genre = fields
            .contains(&QueueDisplayField::Genre)
            .then(|| track.genre.clone())
            .unwrap_or_default();
        self.year = fields
            .contains(&QueueDisplayField::Year)
            .then_some(track.year)
            .flatten();
        self.track_number = fields
            .contains(&QueueDisplayField::TrackNumber)
            .then(|| track.track_number.clone())
            .flatten();
        self.disc_number = fields
            .contains(&QueueDisplayField::DiscNumber)
            .then(|| track.disc_number.clone())
            .flatten();
        self.duration_secs = fields
            .contains(&QueueDisplayField::TrackLength)
            .then_some(track.duration_secs)
            .flatten();
    }

    fn retain_fields(&mut self, fields: &[QueueDisplayField]) {
        if !fields.contains(&QueueDisplayField::Artist) {
            self.artist.clear();
        }
        if !fields.contains(&QueueDisplayField::AlbumArtist) {
            self.album_artist.clear();
        }
        if !fields.contains(&QueueDisplayField::Album) {
            self.album.clear();
        }
        if !fields.contains(&QueueDisplayField::Genre) {
            self.genre.clear();
        }
        if !fields.contains(&QueueDisplayField::Year) {
            self.year = None;
        }
        if !fields.contains(&QueueDisplayField::TrackNumber) {
            self.track_number = None;
        }
        if !fields.contains(&QueueDisplayField::DiscNumber) {
            self.disc_number = None;
        }
        if !fields.contains(&QueueDisplayField::TrackLength) {
            self.duration_secs = None;
        }
    }

    fn display_value(&self, field: QueueDisplayField) -> Option<String> {
        let value = match field {
            QueueDisplayField::Title => Some(self.title.clone()),
            QueueDisplayField::Artist => Some(self.artist.clone()),
            QueueDisplayField::AlbumArtist => Some(self.album_artist.clone()),
            QueueDisplayField::Album => Some(self.album.clone()),
            QueueDisplayField::Genre => Some(self.genre.clone()),
            QueueDisplayField::Year => self.year.map(|year| year.to_string()),
            QueueDisplayField::TrackNumber => self.track_number.clone(),
            QueueDisplayField::DiscNumber => self.disc_number.clone(),
            QueueDisplayField::TrackLength => self
                .duration_secs
                .map(|secs| format_duration(Duration::from_secs(secs))),
            QueueDisplayField::Composer | QueueDisplayField::Bpm => None,
        }?;
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum QueueDisplayField {
    Title,
    Artist,
    AlbumArtist,
    Album,
    Genre,
    Year,
    TrackNumber,
    DiscNumber,
    TrackLength,
    Composer,
    Bpm,
}

impl QueueDisplayField {
    const ALL: [Self; 9] = [
        Self::Title,
        Self::Artist,
        Self::AlbumArtist,
        Self::Album,
        Self::Genre,
        Self::Year,
        Self::TrackNumber,
        Self::DiscNumber,
        Self::TrackLength,
    ];

    fn label(self, language: Language) -> &'static str {
        match self {
            Self::Title => tr(language, "Title"),
            Self::Artist => tr(language, "Artist"),
            Self::AlbumArtist => tr(language, "Album artist"),
            Self::Album => tr(language, "Album"),
            Self::Genre => tr(language, "Genre"),
            Self::Year => tr(language, "Year"),
            Self::TrackNumber => tr(language, "Track number"),
            Self::DiscNumber => tr(language, "Disc number"),
            Self::TrackLength => tr(language, "Track length"),
            Self::Composer => "Composer",
            Self::Bpm => "BPM",
        }
    }
}

fn default_queue_display_fields() -> Vec<QueueDisplayField> {
    vec![QueueDisplayField::Title]
}

fn stopped_queue_restart_target(
    queue: &[QueueEntry],
    stopped_index: Option<usize>,
) -> Option<usize> {
    stopped_index.filter(|index| *index < queue.len())?;
    (!queue.is_empty()).then_some(0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LibraryData {
    folders: Vec<PathBuf>,
    tracks: Arc<Vec<Track>>,
    playlists: Vec<Playlist>,
    #[serde(default, deserialize_with = "deserialize_queue")]
    queue: Vec<QueueEntry>,
    #[serde(default = "default_queue_display_fields")]
    queue_display_fields: Vec<QueueDisplayField>,
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
    #[serde(default)]
    language: Language,
}

impl Default for LibraryData {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            tracks: Arc::new(Vec::new()),
            playlists: Vec::new(),
            queue: Vec::new(),
            queue_display_fields: default_queue_display_fields(),
            library_view: LibraryView::default(),
            eq_band_count: default_eq_band_count(),
            eq_bands: Vec::new(),
            eq_enabled: default_eq_enabled(),
            now_playing_left_fraction: default_now_playing_left_fraction(),
            language: Language::default(),
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
    fn title(self, language: Language) -> &'static str {
        match self {
            Self::NowPlaying => tr(language, "Now Playing"),
            Self::Library => tr(language, "Library"),
            Self::Playlists => tr(language, "Playlists"),
            Self::Equalization => tr(language, "Equalization"),
        }
    }
}

pub(crate) struct MusicApp {
    data_path: PathBuf,
    data: LibraryData,
    tab: Tab,
    selected_paths: HashSet<PathBuf>,
    queue_index: Option<usize>,
    pending_queue_playlist: Option<PathBuf>,
    new_queue_playlist_name: String,
    show_queue_display_options: bool,
    filter: String,
    organize_by: String,
    library_view: LibraryView,
    sort_by: String,
    thumbnail_size: f32,
    active_playlist: Option<usize>,
    playlist_load: Option<PlaylistLoadTask>,
    new_playlist_name: String,
    smart_playlist_name: String,
    smart_conditions: Vec<SmartCondition>,
    smart_operators: Vec<SmartOperator>,
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
    added_tracks: HashSet<PathBuf>,
    smart_playlist: Option<(String, SmartPlaylistQuery)>,
}

#[derive(Clone)]
struct PlaylistTrackRow {
    path: PathBuf,
    title: String,
}

struct PlaylistLoadTask {
    playlist_index: usize,
    receiver: Receiver<PlaylistLoadMessage>,
    rows: Arc<Vec<PlaylistTrackRow>>,
    finished: bool,
    error: Option<String>,
}

enum PlaylistLoadMessage {
    Batch(Vec<PlaylistTrackRow>),
    Finished(Result<(), String>),
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

fn available_library_view(view: LibraryView) -> LibraryView {
    match view {
        LibraryView::AlbumArtist => LibraryView::Tracks,
        available => available,
    }
}

fn parenthesis_label(language: Language, direction: &'static str, count: u8) -> String {
    if count == 0 {
        tr(language, "None").to_owned()
    } else {
        format!(
            "{} {}",
            tr(language, direction),
            match direction {
                "Open" => "(".repeat(count as usize),
                _ => ")".repeat(count as usize),
            }
        )
    }
}

fn supported_eq_band_count(count: usize) -> usize {
    if EQ_BAND_COUNTS.contains(&count) {
        count
    } else {
        5
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

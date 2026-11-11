use std::{
    collections::{HashMap, HashSet, hash_map::DefaultHasher},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::BufReader,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};

use eframe::egui;
use lofty::{
    file::{AudioFile, TaggedFileExt},
    picture::PictureType,
    probe::Probe,
    tag::Accessor,
};

use super::*;

pub(super) const MAX_WAVEFORM_PEAKS: usize = 2048;

pub(super) fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub(super) fn format_frequency(frequency: f32) -> String {
    if frequency >= 1_000.0 {
        format!("{:.1} kHz", frequency / 1_000.0)
    } else {
        format!("{frequency:.0} Hz")
    }
}

pub(super) fn is_in_scan_scope(path: &Path, folder: &Path, recursive: bool) -> bool {
    path.starts_with(folder) && (recursive || path.parent() == Some(folder))
}

pub(super) fn find_moved_track_paths(
    tracks: &[Track],
    missing_paths: &HashSet<PathBuf>,
    added_paths: &HashSet<PathBuf>,
) -> HashMap<PathBuf, PathBuf> {
    let mut missing_tracks: Vec<&Track> = tracks
        .iter()
        .filter(|track| missing_paths.contains(&track.path))
        .collect();
    missing_tracks.sort_by(|left, right| left.path.cmp(&right.path));

    let mut added_tracks: Vec<&Track> = tracks
        .iter()
        .filter(|track| added_paths.contains(&track.path) && !missing_paths.contains(&track.path))
        .collect();
    added_tracks.sort_by(|left, right| left.path.cmp(&right.path));

    let mut used_added_paths = HashSet::new();
    let mut replacements = HashMap::new();
    for missing in missing_tracks {
        if let Some(added) = added_tracks.iter().find(|added| {
            !used_added_paths.contains(&added.path) && same_track_identity(missing, added)
        }) {
            used_added_paths.insert(added.path.clone());
            replacements.insert(missing.path.clone(), added.path.clone());
        }
    }
    replacements
}

pub(super) fn same_track_identity(left: &Track, right: &Track) -> bool {
    if left.duration_secs.is_none() || left.duration_secs != right.duration_secs {
        return false;
    }
    let normalize = |value: &str| value.trim().to_lowercase();
    let artist = |track: &Track| {
        if track.album_artist.trim().is_empty() {
            normalize(&track.artist)
        } else {
            normalize(&track.album_artist)
        }
    };
    let left_artist = artist(left);
    let right_artist = artist(right);
    let left_album = normalize(&left.album);
    let right_album = normalize(&right.album);
    if left_artist.is_empty()
        || left_artist != right_artist
        || left_album.is_empty()
        || left_album != right_album
    {
        return false;
    }

    if let (Some(left_number), Some(right_number)) = (&left.track_number, &right.track_number)
        && left_number == right_number
    {
        return left.disc_number.is_none()
            || right.disc_number.is_none()
            || left.disc_number == right.disc_number;
    }

    !left.title.trim().is_empty() && normalize(&left.title) == normalize(&right.title)
}

pub(super) fn track_group_value(track: &Track, organize_by: &str) -> String {
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

pub(super) fn track_sort_value(track: &Track, sort_by: &str) -> (String, u32) {
    match sort_by {
        "Album Artist" => (track.album_artist.to_lowercase(), 0),
        "Year" => (String::new(), track.year.unwrap_or(u32::MAX)),
        _ => (track.album.to_lowercase(), 0),
    }
}

pub(super) fn inverted_color(color: egui::Color32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        255 - color.r(),
        255 - color.g(),
        255 - color.b(),
        color.a(),
    )
}

pub(super) fn gallery_items(
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

pub(super) fn collect_audio_files(
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

pub(super) fn load_playlist_rows(
    playlist: &Playlist,
    tracks: &[Track],
    sender: &Sender<PlaylistLoadMessage>,
    ctx: &egui::Context,
) {
    let track_by_path: HashMap<&Path, &Track> = tracks
        .iter()
        .map(|track| (track.path.as_path(), track))
        .collect();
    let mut batch = Vec::with_capacity(64);
    let send_batch = |batch: &mut Vec<PlaylistTrackRow>| {
        if batch.is_empty() {
            return true;
        }
        let rows = std::mem::replace(batch, Vec::with_capacity(64));
        if sender.send(PlaylistLoadMessage::Batch(rows)).is_err() {
            return false;
        }
        ctx.request_repaint();
        true
    };

    match &playlist.smart {
        Some(query) => {
            for track in tracks.iter().filter(|track| query.matches(track)) {
                batch.push(PlaylistTrackRow {
                    path: track.path.clone(),
                    title: track.display_title().to_owned(),
                });
                if batch.len() == 64 && !send_batch(&mut batch) {
                    return;
                }
            }
        }
        None => {
            for path in &playlist.tracks {
                let title = track_by_path
                    .get(path.as_path())
                    .map(|track| track.display_title().to_owned())
                    .unwrap_or_else(|| {
                        path.file_stem()
                            .and_then(|stem| stem.to_str())
                            .map(str::to_owned)
                            .unwrap_or_else(|| path.display().to_string())
                    });
                batch.push(PlaylistTrackRow {
                    path: path.clone(),
                    title,
                });
                if batch.len() == 64 && !send_batch(&mut batch) {
                    return;
                }
            }
        }
    }

    if !send_batch(&mut batch) {
        return;
    }
    let _ = sender.send(PlaylistLoadMessage::Finished(Ok(())));
    ctx.request_repaint();
}

pub(super) fn scan_folder(
    folder: &Path,
    recursive: bool,
    artwork_dir: &Path,
    progress: &AtomicUsize,
    total: &AtomicUsize,
    sender: &Sender<ScanMessage>,
) -> Result<(), String> {
    let folders = [folder.to_path_buf()];
    scan_folders(&folders, recursive, artwork_dir, progress, total, sender)
}

pub(super) fn scan_folders(
    folders: &[PathBuf],
    recursive: bool,
    artwork_dir: &Path,
    progress: &AtomicUsize,
    total: &AtomicUsize,
    sender: &Sender<ScanMessage>,
) -> Result<(), String> {
    let mut files = Vec::new();
    let mut seen_files = HashSet::new();
    for folder in folders {
        let mut folder_files = Vec::new();
        collect_audio_files(folder, recursive, &mut folder_files)
            .map_err(|error| format!("{}: {error}", folder.display()))?;
        for path in folder_files {
            if seen_files.insert(path.clone()) {
                files.push(path);
            }
        }
    }
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

pub(super) fn decode_waveform(path: &Path) -> Result<Vec<f32>, String> {
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
    Ok(reduce_waveform_peaks(peaks))
}

pub(super) fn reduce_waveform_peaks(peaks: Vec<f32>) -> Vec<f32> {
    if peaks.len() <= MAX_WAVEFORM_PEAKS {
        return peaks;
    }
    let source = peaks;
    (0..MAX_WAVEFORM_PEAKS)
        .map(|index| {
            let start = index * source.len() / MAX_WAVEFORM_PEAKS;
            let end = ((index + 1) * source.len() / MAX_WAVEFORM_PEAKS).max(start + 1);
            source[start..end.min(source.len())]
                .iter()
                .copied()
                .fold(0.0, f32::max)
        })
        .collect()
}

pub(super) fn decode_artwork(path: &Path) -> Result<egui::ColorImage, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let image = image::load_from_memory(&bytes).map_err(|error| error.to_string())?;
    let thumbnail = image.thumbnail(640, 640).to_rgba8();
    let size = [thumbnail.width() as usize, thumbnail.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        size,
        thumbnail.as_raw(),
    ))
}

pub(super) fn is_supported_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            AUDIO_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

pub(super) fn read_track(path: &Path, artwork_dir: &Path) -> Track {
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
        track.artist = first_tag_value(&tagged_file, |tag| {
            tag.artist().map(|value| value.into_owned()).or_else(|| {
                tag.get_string(&lofty::tag::ItemKey::TrackArtist)
                    .map(str::to_owned)
            })
        })
        .unwrap_or_default();
        track.album_artist = first_tag_value(&tagged_file, |tag| {
            tag.get_string(&lofty::tag::ItemKey::AlbumArtist)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| track.artist.clone());
        track.album = tag
            .album()
            .map(|value| value.into_owned())
            .unwrap_or_default();
        track.genre = tag
            .genre()
            .map(|value| value.into_owned())
            .unwrap_or_default();
        track.year = tag.year();
        track.track_number = first_tag_value(&tagged_file, |tag| {
            tag.get_string(&lofty::tag::ItemKey::TrackNumber)
                .map(str::to_owned)
                .or_else(|| tag.track().map(|number| number.to_string()))
        });
        track.disc_number = first_tag_value(&tagged_file, |tag| {
            tag.get_string(&lofty::tag::ItemKey::DiscNumber)
                .map(str::to_owned)
                .or_else(|| tag.disk().map(|number| number.to_string()))
        });
    }
    track
}

pub(super) fn first_tag_value(
    tagged_file: &lofty::file::TaggedFile,
    extract: impl Fn(&lofty::tag::Tag) -> Option<String>,
) -> Option<String> {
    first_tag_value_from_tags(
        tagged_file.primary_tag(),
        tagged_file.tags().iter(),
        extract,
    )
}

pub(super) fn first_tag_value_from_tags<'a>(
    primary_tag: Option<&'a lofty::tag::Tag>,
    tags: impl Iterator<Item = &'a lofty::tag::Tag>,
    extract: impl Fn(&lofty::tag::Tag) -> Option<String>,
) -> Option<String> {
    primary_tag
        .into_iter()
        .chain(tags)
        .find_map(|tag| extract(tag).filter(|value| !value.trim().is_empty()))
}

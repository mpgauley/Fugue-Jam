use super::*;
use crate::localization::Language;
use eframe::egui;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicUsize,
    sync::mpsc,
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
fn waveform_resolution_preserves_detail_up_to_2048_peaks() {
    let peaks = vec![0.25; MAX_WAVEFORM_PEAKS];
    assert_eq!(reduce_waveform_peaks(peaks.clone()), peaks);

    let mut high_resolution = vec![0.0; MAX_WAVEFORM_PEAKS * 4];
    high_resolution[400 * 4 + 3] = 0.9;
    let reduced = reduce_waveform_peaks(high_resolution);
    assert_eq!(reduced.len(), MAX_WAVEFORM_PEAKS);
    assert_eq!(reduced[400], 0.9);
}

#[test]
fn folder_scanning_can_include_or_skip_subfolders() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("fugue-jam-scan-{}-{nonce}", std::process::id()));
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
fn playlist_rows_load_in_order_and_are_sent_in_batches() {
    let tracks: Vec<Track> = (0..130)
        .map(|index| Track {
            path: Path::new("music").join(format!("{index}.flac")),
            title: format!("Track {index}"),
            ..Track::default()
        })
        .collect();
    let playlist = Playlist {
        name: "Test".to_owned(),
        tracks: tracks
            .iter()
            .rev()
            .map(|track| track.path.clone())
            .collect(),
        smart: None,
    };
    let (sender, receiver) = mpsc::channel();
    load_playlist_rows(&playlist, &tracks, &sender, &egui::Context::default());

    let mut rows = Vec::new();
    let mut batch_count = 0;
    while let Ok(message) = receiver.recv() {
        match message {
            PlaylistLoadMessage::Batch(batch) => {
                batch_count += 1;
                rows.extend(batch);
            }
            PlaylistLoadMessage::Finished(result) => {
                assert!(result.is_ok());
                break;
            }
        }
    }

    assert_eq!(batch_count, 3);
    assert_eq!(rows.len(), 130);
    assert_eq!(rows[0].title, "Track 129");
    assert_eq!(rows[129].title, "Track 0");
}

#[test]
fn scan_matches_a_new_path_to_a_missing_track_without_matching_other_tracks() {
    let old_track = Track {
        path: Path::new("/music/Old Name.flac").to_path_buf(),
        title: "Old Name".to_owned(),
        album_artist: "Example Artist".to_owned(),
        album: "Example Album".to_owned(),
        track_number: Some("03".to_owned()),
        duration_secs: Some(215),
        ..Track::default()
    };
    let moved_track = Track {
        path: Path::new("/music/renamed/03.flac").to_path_buf(),
        title: "New Title".to_owned(),
        album_artist: "Example Artist".to_owned(),
        album: "Example Album".to_owned(),
        track_number: Some("03".to_owned()),
        duration_secs: Some(215),
        ..Track::default()
    };
    let unrelated_track = Track {
        path: Path::new("/music/renamed/04.flac").to_path_buf(),
        title: "Other Song".to_owned(),
        album_artist: "Different Artist".to_owned(),
        album: "Different Album".to_owned(),
        track_number: Some("04".to_owned()),
        duration_secs: Some(215),
        ..Track::default()
    };
    let tracks = vec![old_track.clone(), moved_track.clone(), unrelated_track];
    let missing = HashSet::from([old_track.path.clone()]);
    let added = HashSet::from([
        moved_track.path.clone(),
        PathBuf::from("/music/renamed/04.flac"),
    ]);

    assert_eq!(
        find_moved_track_paths(&tracks, &missing, &added),
        HashMap::from([(old_track.path, moved_track.path)])
    );
}

#[test]
fn smart_playlist_conditions_support_and_or_precedence_and_parentheses() {
    let track = Track {
        album_artist: "Example Artist".to_owned(),
        genre: "Rock".to_owned(),
        year: Some(1995),
        ..Track::default()
    };
    let grouped = SmartPlaylistQuery {
        conditions: vec![
            SmartCondition {
                field: SmartField::AlbumArtist,
                query: "example".to_owned(),
                open_parentheses: 1,
                ..SmartCondition::default()
            },
            SmartCondition {
                field: SmartField::Genre,
                query: "rock".to_owned(),
                close_parentheses: 1,
                ..SmartCondition::default()
            },
            SmartCondition {
                field: SmartField::Year,
                query: "1990".to_owned(),
                ..SmartCondition::default()
            },
        ],
        operators: vec![SmartOperator::And, SmartOperator::Or],
    };
    assert!(grouped.is_valid());
    assert!(grouped.matches(&track));

    let grouped_or = SmartPlaylistQuery {
        conditions: vec![
            SmartCondition {
                field: SmartField::AlbumArtist,
                query: "missing".to_owned(),
                ..SmartCondition::default()
            },
            SmartCondition {
                field: SmartField::Genre,
                query: "rock".to_owned(),
                open_parentheses: 1,
                ..SmartCondition::default()
            },
            SmartCondition {
                field: SmartField::Year,
                query: "1995".to_owned(),
                close_parentheses: 1,
                ..SmartCondition::default()
            },
        ],
        operators: vec![SmartOperator::And, SmartOperator::Or],
    };
    assert!(!grouped_or.matches(&track));

    let precedence = SmartPlaylistQuery {
        conditions: vec![
            SmartCondition {
                field: SmartField::AlbumArtist,
                query: "missing".to_owned(),
                ..SmartCondition::default()
            },
            SmartCondition {
                field: SmartField::Genre,
                query: "rock".to_owned(),
                ..SmartCondition::default()
            },
            SmartCondition {
                field: SmartField::Year,
                query: "1995".to_owned(),
                ..SmartCondition::default()
            },
        ],
        operators: vec![SmartOperator::Or, SmartOperator::And],
    };
    assert!(precedence.matches(&track));
}

#[test]
fn smart_playlist_rejects_empty_conditions_and_unbalanced_parentheses() {
    let mut query = SmartPlaylistQuery {
        conditions: vec![SmartCondition {
            field: SmartField::Genre,
            query: "rock".to_owned(),
            open_parentheses: 1,
            ..SmartCondition::default()
        }],
        operators: Vec::new(),
    };
    assert!(!query.is_valid());
    assert!(!query.matches(&Track::default()));
    query.conditions[0].close_parentheses = 1;
    assert!(query.is_valid());
    query.conditions[0].query.clear();
    assert!(!query.is_valid());
}

#[test]
fn smart_match_supports_contains_and_does_not_contain() {
    assert!(SmartMatch::Contains.matches("Classic Rock", "rock"));
    assert!(!SmartMatch::DoesNotContain.matches("Classic Rock", "rock"));
    assert!(SmartMatch::DoesNotContain.matches("Jazz", "rock"));
    assert_eq!(SmartMatch::default(), SmartMatch::Contains);
}

#[test]
fn smart_playlist_scan_reports_progress_across_library_folders() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fugue-jam-smart-scan-{}-{nonce}",
        std::process::id()
    ));
    let first = root.join("first");
    let second = root.join("second");
    let artwork = root.join("artwork");
    fs::create_dir_all(&first).expect("create first library folder");
    fs::create_dir_all(&second).expect("create second library folder");
    fs::write(first.join("one.flac"), []).expect("create first audio file");
    fs::write(second.join("two.ogg"), []).expect("create second audio file");

    let (sender, receiver) = mpsc::channel();
    let progress = AtomicUsize::new(0);
    let total = AtomicUsize::new(0);
    scan_folders(&[first, second], true, &artwork, &progress, &total, &sender)
        .expect("scan all library folders");
    assert_eq!(progress.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert_eq!(total.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert!(matches!(
        receiver.recv().expect("receive total"),
        super::ScanMessage::Total(2)
    ));
    assert!(matches!(
        receiver.recv().expect("receive first scanned track"),
        super::ScanMessage::Track(_)
    ));
    assert!(matches!(
        receiver.recv().expect("receive second scanned track"),
        super::ScanMessage::Track(_)
    ));

    fs::remove_dir_all(root).expect("remove smart-playlist scan folders");
}

#[test]
fn legacy_smart_playlist_query_migrates_to_single_condition() {
    let playlist: super::Playlist =
        serde_json::from_str(r#"{"name":"Rock","tracks":[],"smart":["Genre","rock"]}"#)
            .expect("load legacy smart playlist");
    let query = playlist.smart.expect("migrate smart query");
    assert_eq!(query.conditions.len(), 1);
    assert_eq!(query.conditions[0].query, "rock");
    assert!(query.matches(&Track {
        genre: "Classic Rock".to_owned(),
        ..Track::default()
    }));

    let earlier_query: super::Playlist = serde_json::from_str(
            r#"{"name":"Jazz","tracks":[],"smart":{"conditions":[{"field":"Genre","query":"jazz","open_parentheses":0,"close_parentheses":0}],"operators":[]}}"#,
        )
        .expect("load smart playlist without comparison mode");
    assert_eq!(
        earlier_query.smart.expect("load previous query").conditions[0].comparison,
        SmartMatch::Contains
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
    assert_eq!(library.language, Language::English);
    assert!(library.queue.is_empty());
    assert_eq!(library.queue_display_fields, vec![QueueDisplayField::Title]);

    let old_queue_library =
        r#"{"folders":[],"tracks":[],"playlists":[],"queue":["/music/track.flac"]}"#;
    let legacy: LibraryData =
        serde_json::from_str(old_queue_library).expect("load path-only queue");
    assert_eq!(legacy.queue[0].path, Path::new("/music/track.flac"));
    assert_eq!(legacy.queue[0].title, "track");

    library.queue.push(QueueEntry {
        path: Path::new("/music/track.flac").to_path_buf(),
        title: "Tagged title".to_owned(),
        ..QueueEntry::default()
    });
    library.language = Language::French;
    let saved = serde_json::to_string(&library).expect("serialize library and queue");
    let restored: LibraryData = serde_json::from_str(&saved).expect("restore serialized library");
    assert_eq!(restored.language, Language::French);
    assert_eq!(restored.queue[0].path, library.queue[0].path);
    assert_eq!(restored.queue[0].title, "Tagged title");

    let legacy_display_fields =
        r#"{"folders":[],"tracks":[],"playlists":[],"queue_display_fields":["Composer","Bpm"]}"#;
    let migrated: LibraryData = serde_json::from_str(legacy_display_fields)
        .expect("load previously available queue display fields");
    assert_eq!(
        migrated.queue_display_fields,
        vec![QueueDisplayField::Composer, QueueDisplayField::Bpm]
    );
}

#[test]
fn queue_display_fields_show_selected_common_tags() {
    let entry = QueueEntry {
        title: "Song".to_owned(),
        artist: "Artist".to_owned(),
        album: "Album".to_owned(),
        year: Some(2024),
        track_number: Some("7".to_owned()),
        disc_number: Some("2".to_owned()),
        duration_secs: Some(184),
        ..QueueEntry::default()
    };
    assert_eq!(
        entry.display_value(QueueDisplayField::Title).as_deref(),
        Some("Song")
    );
    assert_eq!(
        entry.display_value(QueueDisplayField::Artist).as_deref(),
        Some("Artist")
    );
    assert_eq!(
        entry.display_value(QueueDisplayField::Year).as_deref(),
        Some("2024")
    );
    assert_eq!(
        entry
            .display_value(QueueDisplayField::TrackNumber)
            .as_deref(),
        Some("7")
    );
    assert_eq!(
        entry
            .display_value(QueueDisplayField::DiscNumber)
            .as_deref(),
        Some("2")
    );
    assert_eq!(
        entry
            .display_value(QueueDisplayField::TrackLength)
            .as_deref(),
        Some("3:04")
    );
    assert!(!QueueDisplayField::ALL.contains(&QueueDisplayField::Composer));
    assert!(!QueueDisplayField::ALL.contains(&QueueDisplayField::Bpm));
}

#[test]
fn queue_entries_keep_only_selected_track_metadata() {
    let track = Track {
        path: Path::new("/music/song.flac").to_path_buf(),
        title: "Song".to_owned(),
        artist: "Artist".to_owned(),
        album_artist: "Album Artist".to_owned(),
        album: "Album".to_owned(),
        genre: "Genre".to_owned(),
        year: Some(2024),
        track_number: Some("7".to_owned()),
        disc_number: Some("2".to_owned()),
        duration_secs: Some(184),
        ..Track::default()
    };
    let fields = [QueueDisplayField::Album, QueueDisplayField::TrackLength];
    let mut entry = super::QueueEntry::from_track(&track, &fields);
    assert_eq!(entry.path, track.path);
    assert_eq!(entry.title, "Song");
    assert_eq!(entry.album, "Album");
    assert_eq!(entry.duration_secs, Some(184));
    assert!(entry.artist.is_empty());
    assert!(entry.album_artist.is_empty());
    assert!(entry.genre.is_empty());
    assert_eq!(entry.year, None);
    assert_eq!(entry.track_number, None);
    assert_eq!(entry.disc_number, None);

    entry.update_fields(
        &track,
        &[QueueDisplayField::Artist, QueueDisplayField::Year],
    );
    assert_eq!(entry.artist, "Artist");
    assert_eq!(entry.year, Some(2024));
    assert!(entry.album.is_empty());
    assert_eq!(entry.duration_secs, None);

    entry.album = "Stale album".to_owned();
    entry.retain_fields(&[QueueDisplayField::Title]);
    assert!(entry.album.is_empty());
    assert!(entry.artist.is_empty());
    assert_eq!(entry.title, "Song");
}

#[test]
fn tag_lookup_falls_back_to_other_tags_and_accessor_values() {
    let primary = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
    let mut fallback = lofty::tag::Tag::new(lofty::tag::TagType::VorbisComments);
    lofty::tag::Accessor::set_artist(&mut fallback, "Artist".to_owned());
    lofty::tag::Accessor::set_track(&mut fallback, 7);
    lofty::tag::Accessor::set_disk(&mut fallback, 2);
    let tags = std::iter::once(&fallback);

    assert_eq!(
        first_tag_value_from_tags(Some(&primary), tags, |tag| {
            lofty::tag::Accessor::artist(tag).map(|artist| artist.into_owned())
        }),
        Some("Artist".to_owned())
    );
    assert_eq!(
        first_tag_value_from_tags(Some(&primary), std::iter::once(&fallback), |tag| {
            tag.get_string(&lofty::tag::ItemKey::TrackNumber)
                .map(str::to_owned)
                .or_else(|| lofty::tag::Accessor::track(tag).map(|number| number.to_string()))
        }),
        Some("7".to_owned())
    );
    assert_eq!(
        first_tag_value_from_tags(Some(&primary), std::iter::once(&fallback), |tag| {
            tag.get_string(&lofty::tag::ItemKey::DiscNumber)
                .map(str::to_owned)
                .or_else(|| lofty::tag::Accessor::disk(tag).map(|number| number.to_string()))
        }),
        Some("2".to_owned())
    );
}

#[test]
fn restarting_stopped_queue_selects_first_without_starting_playback() {
    let queue = ["first", "current", "last"].map(|title| QueueEntry {
        title: title.to_owned(),
        ..QueueEntry::default()
    });
    assert_eq!(stopped_queue_restart_target(&queue, Some(1)), Some(0));
    assert_eq!(
        queue
            .iter()
            .map(|entry| entry.title.as_str())
            .collect::<Vec<_>>(),
        ["first", "current", "last"]
    );
    assert_eq!(stopped_queue_restart_target(&queue, None), None);
    assert_eq!(stopped_queue_restart_target(&[], Some(0)), None);
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
        queue_display_fields: vec![QueueDisplayField::Artist, QueueDisplayField::Album],
        eq_band_count: 8,
        eq_bands: vec![1.0; 8],
        eq_enabled: false,
        now_playing_left_fraction: 0.6,
        ..LibraryData::default()
    };
    let serialized = serde_json::to_string(&library).expect("serialize player settings");
    let restored: LibraryData = serde_json::from_str(&serialized).expect("restore player settings");
    assert_eq!(restored.library_view, LibraryView::Album);
    assert_eq!(
        restored.queue_display_fields,
        vec![QueueDisplayField::Artist, QueueDisplayField::Album]
    );
    assert_eq!(restored.eq_band_count, 8);
    assert_eq!(restored.eq_bands, vec![1.0; 8]);
    assert!(!restored.eq_enabled);
    assert!((restored.now_playing_left_fraction - 0.6).abs() < f32::EPSILON);
    assert_eq!(
        available_library_view(LibraryView::AlbumArtist),
        LibraryView::Tracks
    );
}

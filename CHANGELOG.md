# Changelog

Program versions are whole-number release identifiers. The Cargo package uses
the corresponding `N.0.0` version to follow Cargo's version format.
For each future program change, add a new section with the next whole-number
version and update the Cargo package version to `N.0.0`.

## Version 27

- Increased the optional Now Playing queue album-art thumbnail from 36px to 45px.
- Added a 5px gap between queue entries and bolded the Title tag.

## Version 26

- Now Playing queue rows place the first selected tag on the first line and
  any remaining tags on a second line.
- Added optional album art to Customize queue display; when enabled, a small
  cover thumbnail appears beside the queue tags.

## Version 25

- Expanded Linux install packages from Debian/Ubuntu to include RPM packages
  for Fedora, RHEL-compatible distributions, and openSUSE, plus native Arch
  Linux packages.
- The packaging workflow publishes all package formats and a shared SHA-256
  checksum file for version-tagged releases.

## Version 24

- Added a Library button to rescan all saved music folders and refresh track
  metadata in the background.
- The metadata rescan preserves the existing library and playlist entries while
  updating tags and showing scan progress.

## Version 23

- Library update scans remove tracks whose files are no longer found after a
  successful scan.
- When a missing track is recognized at a newly discovered path by its tags,
  album artist, album, track number, and duration, the library entry and its
  static playlist, queue, and selection references are updated instead of
  retaining a duplicate.

## Version 22

- Playlist tracks are resolved in a background worker, so selecting a playlist
  no longer blocks the interface while smart-playlist rules or large static
  playlists are evaluated.
- Playlist rows arrive in batches and render progressively in a virtualized
  list; queue-copy actions become available after loading completes.

## Version 21

- Playlist queue entries now retain only the track path, fallback title, and
  metadata fields selected for queue display.
- Existing queue metadata is trimmed to the selected display fields on load;
  changing the queue display updates metadata from the in-memory library.

## Version 20

- Added per-condition **contains** and **does not contain** matching to smart
  playlists.
- Smart playlist creation now scans saved library folders in the background,
  displays scan progress in the status bar, refreshes library metadata, and
  creates the playlist when scanning completes.

## Version 19

- Expanded smart playlists to support multiple metadata conditions joined with
  AND/OR operators and nested parentheses.
- Existing saved smart playlists migrate to equivalent single-condition rules.

## Version 18

- Added a language selector with English, French, Spanish, German, Simplified
  Chinese, Japanese, Russian, and Korean.
- Localized navigation, library and playlist controls, queue actions, equalizer
  controls, dialogs, playback tooltips, and scan progress labels.
- Saved the selected language in the library settings so it is restored on the
  next launch.

## Version 17

- Scaled Now Playing album artwork to the largest size that fits the cover
  frame, accounting for the track title and preserving the image aspect ratio.

## Version 16

- Increased waveform detail from 256 to 2,048 peak samples.
- Added the current track's album artwork to the left side of the playback
  timeline.

## Version 15

- Added fontconfig-selected UI fallback fonts for Simplified Chinese,
  Traditional Chinese, Korean, and Russian, alongside Japanese.
- Added Noto CJK and Noto core as recommended runtime font packages.

## Version 14

- Added a GitHub Actions workflow to build Debian/Ubuntu install packages for
  Linux x86_64, publish release assets for version tags, and provide checksums.
- Added a desktop launcher entry for installed packages.

## Version 13

- Changed the second Stop press to reset the current queue selection to its
  first track without starting playback.

## Version 12

- Changed the second Stop press to play the first track in the queue without
  moving the stopped track from its existing queue position.

## Version 11

- Changed Stop behavior: the first press stops playback but retains the current
  track; pressing Stop again moves that track to the top of the queue.
- Added track length to the customizable Now Playing queue fields and removed
  composer and BPM from the available choices.
- Improved artist, track number, and disc number tag lookup across all
  available tags and Lofty's format-specific accessors.
- Re-scanning the library refreshes the corrected tag values for existing
  tracks.
- Expanded the Library grid scroll area to fill the available frame.
- Left-aligned Now Playing queue content.

## Version 10

- Added **Customize display…** to the Now Playing queue, with selections for
  title, artist, album artist, album, genre, year, track/disc number, composer,
  and BPM tags.
- Cached the selected metadata in queue entries so queue rows can display
  chosen tags without searching the full library.
- Reads the additional common ID tags when scanning supported audio files.

## Version 9

- Added a Japanese-capable system font fallback through fontconfig for song
  names and other interface text.
- Expanded the Now Playing queue list to fill the available right-side frame
  vertically and horizontally, with full-width row click/context targets.
- Removed **Album Artist** from the Library **Display** menu.

## Version 8

- Reduced each Now Playing queue entry to only its track path and display title,
  retaining compatibility with previously saved path-only queues.
- Eliminated per-row library searches and full queue cloning while rendering.
- Virtualized the queue list so only visible rows are rendered for large queues.

## Version 7

- Preserved the adjustable Now Playing split ratio as the window is resized.
- Saved and restored the last Library view.
- Saved and restored equalizer enablement, selected band count, and gain values.
- Added selectable 5-, 8-, 12-, 20-, and 31-band equalizers with frequency
  ranges of 60 Hz–12 kHz, 30 Hz–18 kHz, or 20 Hz–20 kHz as appropriate.

## Version 6

- Made Library gallery cards fixed square tiles with a strict 1:1 aspect ratio.
- Persisted the Now Playing queue in the saved library and restore it on the
  next launch.
- Removed per-track Remove buttons from the queue and added right-click actions
  to remove, play next, move to the end, or add a track to a static playlist.
- Added queue context-menu options to create a new playlist or choose an
  existing static playlist.
- Prevented queue rows from changing highlight color on mouse hover.
- Made the Library central frame fill the available window width when opened.

## Version 5

- Made equalizer adjustments, presets, and the enable toggle affect active
  playback immediately.
- Added a waveform and playback-progress timeline above the transport controls,
  with click-to-seek support and visible seek errors.
- Detects HDMI default audio outputs and keeps them active with a silent audio
  stream while playback is paused.
- Added **Scan for changes…** to discover new files and remove files no longer
  present under the selected scan folder. Missing tracks are removed only after
  a successful scan and are also removed from static playlists, the queue, and
  the current selection.
- Kept removal scope consistent with the recursive-subfolder setting so scans
  that skip subfolders do not prune those tracks.

## Version 4

- Added fixed, always-visible right-side scrollbars to the Library track and
  gallery views.
- Changed track-view clicks to select tracks; playback is available through
  **Play selection** and the track context menu.
- Added a thumbnail/icon size slider.
- Replaced the library controls with **Group By**, **Display**, and **Sort By**
  options for album artist, genre, decade, albums, artists, and tracks.
- Added group headings to gallery views and inverted selected-card borders.
- Added right-side spacing to gallery rows so the last cover is not clipped.

## Version 3

- Moved album artwork decoding and texture loading to a background worker so
  scrolling and tab navigation remain responsive.
- Virtualized track and album grids, requesting artwork only as visible items
  are rendered and displaying items progressively as scan and artwork results
  arrive.
- Updated the album view to show only cover art, using the album name when no
  cover art is available.

## Version 2

- Displayed the whole-number program version in the app header.
- Added non-blocking library scans with a progress bar in the bottom status bar.
- Added a subfolder scan option, enabled by default.
- Updated existing library tracks and their cached cover art when a folder is
  scanned again.
- Added album artwork extraction and caching for album and track views.
- Added an album view that displays cover-art thumbnails in a grid.
- Added a track list with cover icon, title, album artist, album, year, file
  type, and duration.
- Added library context-menu actions to queue selected tracks, create a
  playlist from the selection, or add selected tracks to a static playlist.
- Reworked Now Playing into an adjustable cover-art and queue split, defaulting
  to 75% cover panel and 25% queue.

## Version 1

- Initial Rust desktop music player with a persistent library, playlists, queue,
  playback controls, and equalization.

# Changelog

Program versions are whole-number release identifiers. The Cargo package uses
the corresponding `N.0.0` version to follow Cargo's version format.
For each future program change, add a new section with the next whole-number
version and update the Cargo package version to `N.0.0`.

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

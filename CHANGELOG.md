# Changelog

Program versions are whole-number release identifiers. The Cargo package uses
the corresponding `N.0.0` version to follow Cargo's version format.
For each future program change, add a new section with the next whole-number
version and update the Cargo package version to `N.0.0`.

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

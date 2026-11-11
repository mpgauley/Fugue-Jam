# Fugue Jam

Fugue Jam is a local-first Linux music player written in Rust. It scans music
folders, reads common embedded tags, and stores the library and playlists in
the user's application data directory.

## Features

- Library browsing and search, with track and album-art grid views, organized by
  album artist, album, genre, or year
- Background artwork loading and virtualized, vertically scrolling library views
- Library controls for grouping, display type, sorting, and thumbnail size
- Uses installed system fonts for Japanese, Simplified and Traditional
  Chinese, Korean, and Russian song titles and other interface text
- Localized interface menus and controls in English, French, Spanish, German,
  Simplified Chinese, Japanese, Russian, and Korean, with the selected language
  saved between launches
- Non-blocking library scanning with progress and recursive subfolder scanning
  enabled by default
- Right-click selected library tracks to queue them or create/add to a static
  playlist
- Persistent music folders, library metadata, static playlists, and smart playlists
- Smart playlists with multiple AND/OR metadata conditions, grouped
  parenthetical expressions, and contains/does-not-contain matching
- Smart playlist creation rescans library folders in the background and shows
  progress in the status bar
- Queue actions for selected library tracks and playlists, in regular or shuffled order
- Persistent Now Playing queue restored when the app reopens
- Virtualized queue rows for responsive browsing of large queues
- Queue context actions for removing, reordering, playing next, and adding tracks
  to static playlists
- Customize queue rows to show common audio tags such as artist, album, genre,
  year, track/disc number, and track length; optionally show a 45-pixel album-art
  thumbnail beside the queue tags
- The Now Playing queue separates tracks with extra spacing and emphasizes the
  Title tag in bold
- Play, pause, stop, previous, and next controls; a second Stop resets the
  queue selection to its first track without starting playback
- High-resolution waveform timeline with playback progress, click-to-seek,
  and current track album art
- Selectable 5-, 8-, 12-, 20-, and 31-band software equalizer with persistent
  settings and bass, treble, and vocal presets that apply immediately during
  playback
- HDMI output keep-alive silence while a track is paused
- Library update scan to find new tracks and remove missing files after a
  successful scan, while updating recognized tracks whose paths have changed
- The Library restores its last-used view
- Playback for MP3, FLAC, WAV, AIFF, AAC, ALAC/M4A, and Ogg Vorbis files
- Embedded tag reading through Lofty, including ID3, Vorbis comments, MP4, RIFF,
  and APE tags where applicable

The supported playback formats are open/common codecs, not every audio format
ever published. Playback availability also depends on the installed system audio
device and its drivers.

## Build and run

Install Rust and the ALSA development headers. On Debian or Ubuntu:

```sh
sudo apt install build-essential pkg-config libasound2-dev fonts-noto-cjk fonts-noto-core
```

Then run:

```sh
cargo run --release
```

## Linux install packages

The **Package Linux** GitHub Actions workflow builds x86_64 installers for
Debian/Ubuntu (`.deb`), Fedora/RHEL/openSUSE (`.rpm`), and Arch Linux
(`.pkg.tar.zst`). When a `v*` version tag is pushed, the packages and their
SHA-256 checksums are uploaded as GitHub Release assets. The workflow can also
be run manually to produce downloadable workflow artifacts.

Install a downloaded package using your distribution's package manager:

```sh
sudo apt install ./fugue-jam_27.0.0-1_amd64.deb   # Debian/Ubuntu
sudo dnf install ./fugue-jam-27.0.0-1.x86_64.rpm # Fedora/RHEL
sudo zypper install ./fugue-jam-27.0.0-1.x86_64.rpm # openSUSE
sudo pacman -U ./fugue-jam-27.0.0-1-x86_64.pkg.tar.zst # Arch Linux
```

The library database is saved under the operating system's per-user application
data directory. Use **Language** in the top navigation to change the interface
language; the selection is saved automatically. Use **Add music folder…** on
the Library tab to scan a folder.
See [CHANGELOG.md](./CHANGELOG.md) for release history.

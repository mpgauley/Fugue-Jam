# Fugue Jam

Fugue Jam is a local-first Linux music player written in Rust. It scans music
folders, reads common embedded tags, and stores the library and playlists in
the user's application data directory.

## Features

- Library browsing and search, with track and album-art grid views, organized by
  album artist, album, genre, or year
- Background artwork loading and virtualized, vertically scrolling library views
- Library controls for grouping, display type, sorting, and thumbnail size
- Non-blocking library scanning with progress and recursive subfolder scanning
  enabled by default
- Right-click selected library tracks to queue them or create/add to a static
  playlist
- Persistent music folders, library metadata, static playlists, and smart playlists
- Queue actions for selected library tracks and playlists, in regular or shuffled order
- Persistent Now Playing queue restored when the app reopens
- Queue context actions for removing, reordering, playing next, and adding tracks
  to static playlists
- Play, pause, stop, previous, and next controls
- Waveform timeline with playback progress and click-to-seek controls
- Five-band software equalizer with bass, treble, and vocal presets that apply
  immediately during playback
- HDMI output keep-alive silence while a track is paused
- Library rescan to find new tracks and remove missing files after a successful
  scan
- Adjustable 75/25 album-art and queue layout in Now Playing
- Playback for MP3, FLAC, WAV, AIFF, AAC, ALAC/M4A, and Ogg Vorbis files
- Embedded tag reading through Lofty, including ID3, Vorbis comments, MP4, RIFF,
  and APE tags where applicable

The supported playback formats are open/common codecs, not every audio format
ever published. Playback availability also depends on the installed system audio
device and its drivers.

## Build and run

Install Rust and the ALSA development headers. On Debian or Ubuntu:

```sh
sudo apt install build-essential pkg-config libasound2-dev
```

Then run:

```sh
cargo run --release
```

The library database is saved under the operating system's per-user application
data directory. Use **Add music folder…** on the Library tab to scan a folder.
See [CHANGELOG.md](./CHANGELOG.md) for release history.

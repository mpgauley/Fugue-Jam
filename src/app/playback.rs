use super::*;

impl MusicApp {
    pub(super) fn play_queue_index(&mut self, index: usize) {
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

    pub(super) fn play_pause(&mut self) {
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

    pub(super) fn stop(&mut self) {
        self.stop_audio();
        self.queue_index = None;
        self.current_title.clear();
        self.track_duration = None;
        self.waveform.clear();
        self.waveform_track = None;
        self.status = "Playback stopped.".to_owned();
        self.save();
    }

    pub(super) fn stop_audio(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        if let Some(keepalive) = &self.hdmi_keepalive {
            keepalive.pause();
        }
        self.paused = false;
    }

    pub(super) fn stop_button(&mut self) {
        if self.sink.is_some() {
            self.stop_audio();
            self.status = "Playback stopped. Press Stop again to restart the queue.".to_owned();
            self.save();
            return;
        }

        let Some(restart_index) = stopped_queue_restart_target(&self.data.queue, self.queue_index)
        else {
            self.status = "There is no stopped track in the queue.".to_owned();
            return;
        };
        let Some(entry) = self.data.queue.get(restart_index) else {
            self.status = "There is no track at the start of the queue.".to_owned();
            return;
        };
        let path = entry.path.clone();
        let title = entry.title.clone();
        let duration_secs = entry.duration_secs;
        self.queue_index = Some(restart_index);
        self.paused = false;
        self.track_duration = duration_secs.map(Duration::from_secs);
        self.waveform.clear();
        self.waveform_track = Some(path.clone());
        let waveform_unavailable = self.waveform_requests.send(path.clone()).is_err();
        self.current_title = self
            .data
            .tracks
            .iter()
            .find(|track| track.path == path)
            .map(|track| format!("{} — {}", track.display_title(), track.album_artist))
            .unwrap_or(title);
        self.status = if waveform_unavailable {
            format!(
                "Queue restarted at {}. Playback remains stopped. Waveform generator is unavailable.",
                self.current_title
            )
        } else {
            format!(
                "Queue restarted at {}. Playback remains stopped.",
                self.current_title
            )
        };
        self.save();
    }

    pub(super) fn next_track(&mut self) {
        let Some(index) = self.queue_index else {
            if !self.data.queue.is_empty() {
                self.play_queue_index(0);
            }
            return;
        };
        if index + 1 < self.data.queue.len() {
            self.play_queue_index(index + 1);
        } else {
            self.stop_audio();
            self.status = "Playback finished.".to_owned();
            self.save();
        }
    }

    pub(super) fn previous_track(&mut self) {
        let Some(index) = self.queue_index else {
            return;
        };
        if index > 0 {
            self.play_queue_index(index - 1);
        } else {
            self.play_queue_index(0);
        }
    }

    pub(super) fn check_playback_end(&mut self) {
        let ended = self.sink.as_ref().is_some_and(Sink::empty);
        if ended && !self.paused {
            self.next_track();
        }
    }
}

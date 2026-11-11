use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};

use rodio::Source;

pub(super) const EQ_BAND_COUNTS: [usize; 5] = [5, 8, 12, 20, 31];

pub(super) struct EqualizerControls {
    gains: [AtomicU32; 31],
    band_count: AtomicU32,
    enabled: AtomicBool,
    revision: AtomicU64,
}

impl EqualizerControls {
    pub(super) fn new(gains: &[f32], enabled: bool) -> Self {
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

    pub(super) fn update(&self, gains: &[f32], enabled: bool) {
        for (target, gain) in self.gains.iter().zip(gains) {
            target.store(gain.to_bits(), Ordering::Relaxed);
        }
        self.band_count.store(gains.len() as u32, Ordering::Relaxed);
        self.enabled.store(enabled, Ordering::Relaxed);
        self.revision.fetch_add(1, Ordering::Release);
    }

    pub(super) fn snapshot(&self) -> (Vec<f32>, bool) {
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

pub(super) struct EqualizedSource<S> {
    input: S,
    filters: Vec<Biquad>,
    channel: usize,
    channels: usize,
    controls: Arc<EqualizerControls>,
    revision: u64,
}

impl<S: Source<Item = f32>> EqualizedSource<S> {
    pub(super) fn new(input: S, controls: Arc<EqualizerControls>) -> Self {
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

pub(super) struct SilenceSource;

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

pub(super) fn eq_frequencies(count: usize) -> Vec<f32> {
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

pub(super) fn interpolate_gain(frequency: f32, source_frequencies: &[f32], gains: &[f32]) -> f32 {
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

pub(super) fn remap_eq_gains(gains: &[f32], target_count: usize) -> Vec<f32> {
    let source_frequencies = eq_frequencies(gains.len());
    eq_frequencies(target_count)
        .iter()
        .map(|frequency| interpolate_gain(*frequency, &source_frequencies, gains))
        .collect()
}

pub(super) fn preset_eq_gains(preset: usize, count: usize) -> Vec<f32> {
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

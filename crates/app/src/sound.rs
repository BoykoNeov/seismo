//! "Listen": a seismogram played through the speakers, sped up (see `session::sound`). The
//! audio device is opened on the first press of a listen button, never before, so screenshots,
//! benchmarks and tests make no sound and need no device.

use std::num::NonZero;

use rodio::buffer::SamplesBuffer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player};
use session::sound::{self, DEFAULT_SPEED, SAMPLE_RATE_HZ};
use sim::seismogram::Seismogram;

/// The audio output, opened when first needed.
enum Device {
    Closed,
    Open(MixerDeviceSink),
    /// Opening failed; not tried again.
    Failed,
}

/// What is playing: whose trace, from where on its clock and how fast, on its own player
/// (dropping the player stops the sound).
struct Playing {
    station: usize,
    from_s: f64,
    speed: f64,
    player: Player,
}

pub(crate) struct Listen {
    /// The speed-up the next sound is made with (× real time).
    pub(crate) speed: f64,
    /// The station picked to listen to; the nearest shown one when `None` or no longer shown.
    pub(crate) station: Option<usize>,
    device: Device,
    playing: Option<Playing>,
    /// Why nothing could be played, for the controls.
    pub(crate) message: Option<String>,
}

impl Default for Listen {
    fn default() -> Self {
        Self {
            speed: DEFAULT_SPEED,
            station: None,
            device: Device::Closed,
            playing: None,
            message: None,
        }
    }
}

impl Listen {
    /// Play `trace` of `station` from `from_s` to `to_s` (its own clock), replacing whatever
    /// was playing.
    pub(crate) fn play(&mut self, station: usize, trace: &Seismogram, from_s: f64, to_s: f64) {
        self.stop();
        if matches!(self.device, Device::Closed) {
            self.device = match DeviceSinkBuilder::open_default_sink() {
                Ok(mut sink) => {
                    sink.log_on_drop(false);
                    Device::Open(sink)
                }
                Err(e) => {
                    tracing::warn!("no sound output: {e}");
                    self.message = Some(format!("No sound output: {e}"));
                    Device::Failed
                }
            };
        }
        let Device::Open(sink) = &self.device else {
            return;
        };
        let samples = sound::audio(trace, from_s, to_s, self.speed, SAMPLE_RATE_HZ);
        if samples.iter().all(|&s| s == 0.0) {
            self.message = Some("This trace is silent.".to_owned());
            return;
        }
        self.message = None;
        let player = Player::connect_new(sink.mixer());
        player.append(SamplesBuffer::new(
            NonZero::<u16>::MIN,
            NonZero::new(SAMPLE_RATE_HZ).expect("a sample rate"),
            samples,
        ));
        self.playing = Some(Playing {
            station,
            from_s,
            speed: self.speed,
            player,
        });
    }

    pub(crate) fn stop(&mut self) {
        self.playing = None;
    }

    /// The station whose trace is playing, if any; forgets a sound that has ended.
    pub(crate) fn playing(&mut self) -> Option<usize> {
        if self.playing.as_ref().is_some_and(|p| p.player.empty()) {
            self.playing = None;
        }
        self.playing.as_ref().map(|p| p.station)
    }

    /// The station playing and where on its trace's clock (s) the sound is now.
    pub(crate) fn playhead(&self) -> Option<(usize, f64)> {
        self.playing.as_ref().map(|p| {
            let played = p.player.get_pos().as_secs_f64();
            (p.station, sound::trace_time(p.from_s, p.speed, played))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run by hand (`cargo test -p app -- --ignored`): opens the real default output and plays
    /// a quarter second of silence through it, so it makes no sound.
    #[test]
    #[ignore = "needs an audio device"]
    fn plays_silence_through_the_default_device() {
        let sink = DeviceSinkBuilder::open_default_sink().expect("an audio output");
        let player = Player::connect_new(sink.mixer());
        player.append(SamplesBuffer::new(
            NonZero::<u16>::MIN,
            NonZero::new(SAMPLE_RATE_HZ).unwrap(),
            vec![0.0f32; SAMPLE_RATE_HZ as usize / 4],
        ));
        assert!(!player.empty());
        player.sleep_until_end();
        assert!(player.empty());
        assert!(
            player.get_pos().as_secs_f64() > 0.2,
            "{:?}",
            player.get_pos()
        );
    }

    #[test]
    fn nothing_plays_until_asked() {
        let mut l = Listen::default();
        assert!(matches!(l.device, Device::Closed));
        assert_eq!(l.playing(), None);
        assert_eq!(l.playhead(), None);
        assert_eq!(l.speed, DEFAULT_SPEED);
    }
}

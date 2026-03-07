use std::mem::MaybeUninit;

use super::{
    fs::{Error, FileHandler, GrainReader},
    state,
};
use embedded_io::SeekFrom;

pub struct Onset<F: FileHandler> {
    pub path: String,
    pub file: F::File,
    pub sample_rate: u32,
    pub pcm_start: u32,
    pub pcm_len: u32,
    pub onset_start: u32,
    pub beat_count: u32,
}

impl<F: FileHandler> Onset<F> {
    pub fn pos(&mut self, fs: &mut F) -> Result<u64, F::Error> {
        Ok(fs.stream_position(&mut self.file)? - self.pcm_start as u64)
    }

    pub fn seek(&mut self, offset: i64, fs: &mut F) -> Result<(), F::Error> {
        fs.seek(
            &mut self.file,
            SeekFrom::Start(self.pcm_start as u64 + offset.rem_euclid(self.pcm_len as i64) as u64),
        )
        .map(|_| ())
    }

    /// read that loops over pcm block
    pub fn read(&mut self, mut bytes: &mut [u8], fs: &mut F) -> Result<(), F::Error> {
        while !bytes.is_empty() {
            let len = bytes.len().min((self.pcm_len as u64 - self.pos(fs)?) as usize);
            let n = fs.read(&mut self.file, &mut bytes[..len])?;
            if n == 0 {
                self.seek(0, fs)?;
            }
            bytes = &mut bytes[n..];
        }
        Ok(())
    }
}

pub struct SignalHandler<F: FileHandler> {
    grain: GrainReader,

    pub ticks_per_beat: u32,
    pub ticks_per_input: u32,
    pub tick: f32,
    pub tempo: f32,

    pub active_state: state::State<Onset<F>>,
}

impl<F: FileHandler> SignalHandler<F> {
    pub fn new() -> Self {
        Self {
            grain: GrainReader::default(),
            ticks_per_beat: 1,
            ticks_per_input: 1,
            tick: 0.,
            tempo: 1.,

            active_state: state::State::default(),
        }
    }

    /// read samples from the active onset into `buffer` until should advance tick
    /// if returning early, returns number of samples read
    pub fn read<const ONSET_COUNT: usize>(&mut self, buffer: &mut [f32], channels: usize, sample_rate: u32, fs: &mut F) -> Result<Option<usize>, F::Error> {
        // FIXME: support alternative channels counts
        assert!(channels == 2, "currently only stereo output is supported");
        for i in 0..buffer.len() / channels {
            let (tick_delta, l, r) = self.grain.read::<ONSET_COUNT, F>(
                self.ticks_per_beat,
                self.tempo,
                &mut self.active_state,
                sample_rate,
                fs,
            )?;
            let l = f32::tanh(l);
            let r = f32::tanh(r);
            buffer[i * channels] += l;
            buffer[i * channels + 1] += r;
            if self.tick.ceil() != { self.tick += tick_delta; self.tick.ceil() } {
                self.tick = self.tick.rem_euclid((self.ticks_per_beat * self.ticks_per_input) as f32);
                return Ok(Some(i * channels))
            }
        }
        Ok(None)
    }

    /// returns (tick, active_state)
    pub fn tick<const ONSET_COUNT: usize>(&mut self, state: state::ModState<state::OnsetEvent<state::Onset>>, fs: &mut F) -> Result<(i32, state::State<()>), Error<F::Error>> {
        if let Some(event) = state.event {
            let uninit: &mut MaybeUninit<state::OnsetEvent<Onset<F>>> = unsafe { core::mem::transmute(&mut self.active_state.event) };
            let mut temp = unsafe { core::mem::replace(uninit, MaybeUninit::uninit()).assume_init() };
            temp = temp.trans::<ONSET_COUNT>(self.ticks_per_beat, event, &mut self.grain, fs)?;
            core::mem::swap(&mut self.active_state.event, &mut temp);
            core::mem::forget(temp);
        } else {
            // advance ticks
            match &mut self.active_state.event {
                state::OnsetEvent::Sync => (),
                state::OnsetEvent::Hold { tick, onset, index } => {
                    *tick += 1;
                    // sync every ticks_per_mod
                    if (self.tick.floor() as i32).rem_euclid(self.ticks_per_input as i32) == 0 {
                        self.grain.fade::<ONSET_COUNT, F>(Some((*index, onset)), fs)?;
                        let offset = (onset.pcm_len as f32 * *tick as f32 / (onset.beat_count * self.ticks_per_beat) as f32) as i64 & !1;
                        onset.seek(onset.onset_start as i64 * 2 + offset, fs)?;
                    }
                }
                state::OnsetEvent::Loop { tick, onset, index, len } => {
                    *tick += 1;
                    // loop on len overflow, else sync every ticks_per_mod
                    let overflow_ticks = tick.rem_euclid(*len as i32);
                    if *tick < 0 || *tick >= *len as i32 {
                        // loop over len **temporal** ticks after onset
                        self.grain.fade::<ONSET_COUNT, F>(Some((*index, onset)), fs)?;
                        let overflow_samples = (onset.sample_rate as i32 * 60 * overflow_ticks) as f32 * self.tempo / self.ticks_per_beat as f32;
                        onset.seek(onset.onset_start as i64 * 2 + overflow_samples as i64, fs)?;
                        *tick = overflow_ticks;
                    } else if (self.tick.floor() as i32).rem_euclid(self.ticks_per_input as i32) == 0 {
                        self.grain.fade::<ONSET_COUNT, F>(Some((*index, onset)), fs)?;
                        let offset = (onset.pcm_len as f32 * *tick  as f32 / (onset.beat_count * self.ticks_per_beat) as f32) as i64 & !1;
                        onset.seek(onset.onset_start as i64 * 2 + offset, fs)?;
                    }
                }
            }
        }
        if let Some(reverse) = state.reverse {
            self.active_state.reverse = reverse;
        }
        if let Some(speed) = state.speed {
            self.active_state.speed = speed.clamp(16f32.recip(), 16.);
        };

        let unit = self.active_state.as_unit();

        Ok((self.tick.floor() as i32, unit))
    }
}

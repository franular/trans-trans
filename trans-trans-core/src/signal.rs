use std::mem::MaybeUninit;

use super::{
    fs::{Error, FileHandler, GrainReader},
    state,
};
use embedded_io::SeekFrom;

pub struct Message<const LAYER_COUNT: usize> {
    pub tick: i32,
    pub(crate) inputs: [Option<state::OnsetInput>; LAYER_COUNT],
}

pub(crate) struct Onset<F: FileHandler> {
    pub path: String,
    pub file: F::File,
    pub pcm_start: u32,
    pub pcm_len: u32,
    pub onset_start: u32,
    pub beat_count: u32,
    pub sample_rate: u32,
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

    pub fn seek_from_start(&mut self, tick: i32, ticks_per_beat: u32, fs: &mut F) -> Result<(), F::Error> {
        let seek_to = self.onset_start as i64 * 2 + ((self.pcm_len as f32 / (self.beat_count * ticks_per_beat) as f32 * tick as f32) as i64 & !1);
        self.seek(seek_to, fs)
    }
}

pub struct SignalHandler<const LAYER_COUNT: usize, F: FileHandler>
where
    [(); LAYER_COUNT + 1]:,
{
    grains: [GrainReader; LAYER_COUNT + 1],
    pub tick: f32,
    pub tempo: f32,
    ticks_per_beat: u32,
    ticks_per_meas: u32,
    active_events: [state::OnsetEvent<state::Modded<Onset<F>>>; LAYER_COUNT + 1],
}

impl<const LAYER_COUNT: usize, F: FileHandler> SignalHandler<LAYER_COUNT, F>
where
    [(); LAYER_COUNT + 1]:,
{
    pub fn new(tempo: f32, ticks_per_beat: u32, ticks_per_input: u32, ticks_per_step: u32, steps_per_meas: u32) -> Self {
        Self {
            grains: core::array::from_fn(|_| GrainReader::default()),
            tick: 0.,
            tempo,
            ticks_per_beat: ticks_per_beat.max(1),
            ticks_per_meas: (ticks_per_step * steps_per_meas).max(ticks_per_input).max(1),
            active_events: core::array::from_fn(|_| state::OnsetEvent::Stop),
        }
    }

    /// read samples from the active onset into `buffer` until should advance global tick
    /// if returning early, returns number of samples read
    pub fn read<const ONSET_COUNT: usize>(&mut self, buffer: &mut [f32], channels: usize, sample_rate: u32, fs: &mut F) -> Result<Option<usize>, F::Error> {
        // FIXME: support alternative channels counts
        assert!(channels == 2, "currently only stereo output is supported");
        for i in 0..buffer.len() / channels {
            for idx in 0..LAYER_COUNT + 1 {
                let (l, r) = self.grains[idx].read::<ONSET_COUNT, F>(
                    &mut self.active_events[idx],
                    sample_rate,
                    fs,
                )?;
                buffer[i * channels] += l;
                buffer[i * channels + 1] += r;
            }
            // apply equal compression to all channels
            let mult = f32::min(
                if buffer[i * channels] == 0. {
                    1.
                } else {
                    buffer[i * channels].tanh() / buffer[i * channels]
                },
                if buffer[i * channels + 1] == 0. {
                    1.
                } else {
                    buffer[i * channels + 1].tanh() / buffer[i * channels + 1]
                },
            );
            buffer[i * channels] *= mult;
            buffer[i * channels + 1] *= mult;
            let tick_delta = self.ticks_per_beat as f32 * self.tempo / (60. * sample_rate as f32);
            if self.tick.ceil() != { self.tick += tick_delta; self.tick.ceil() } {
                self.tick = self.tick.rem_euclid(self.ticks_per_meas as f32);
                return Ok(Some(i * channels))
            }
        }
        Ok(None)
    }

    pub fn tick(&mut self, mut message: state::Message<LAYER_COUNT>, fs: &mut F) -> Result<Message<LAYER_COUNT>, Error<F::Error>> {
        self.ticks_per_beat = message.ticks_per_beat;
        self.ticks_per_meas = message.ticks_per_meas;
        for (local, rx) in self.active_events.iter_mut().zip(message.events.iter_mut()) {
            // FIXME: get fades to not sound like shit please
            // // fade every tick to account for sync, events, gain, etc., and on Stop
            // if let state::OnsetEvent::Hold { onset, .. } | state::OnsetEvent::Loop { onset, .. } = local {
            //     self.grain.fade::<LAYER_COUNT, F>(Some(onset), fs)?;
            // } else if matches!(rx.inner, Some(state::OnsetEvent::Stop)) {
            //     self.grain.fade::<LAYER_COUNT, F>(None, fs)?;
            // }
            // replace event
            let uninit: &mut MaybeUninit<state::OnsetEvent<state::Modded<Onset<F>>>> = unsafe { core::mem::transmute(&mut *local) };
            let mut temp = unsafe { core::mem::replace(uninit, MaybeUninit::uninit()).assume_init() };
            temp = temp.tick(rx, self.ticks_per_beat, fs)?;
            core::mem::swap(local, &mut temp);
            core::mem::forget(temp);
        }
        Ok(Message { tick: self.tick.floor() as i32, inputs: message.inputs })
    }
}

use crate::{signal, state};
use core::fmt::{Debug, Display};
use embedded_io::{ErrorType, ReadExactError, SeekFrom};

/// single-sided grain length in frames
pub(crate) const GRAIN_LEN: usize = 128;
/// window length in frames; equivilent to 4 ms at 48 kHz sample rate
const WINDOW_LEN: usize = 192;
/// single-sided fade length in frames
const FADE_LEN: usize = 512;

#[derive(Debug)]
pub enum Error<E: Debug> {
    BadFormat,
    DataNotFound,
    Other(E),
}

impl<E: Debug + Display> core::fmt::Display for Error<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadFormat => write!(f, "bad format"),
            Self::DataNotFound => write!(f, "data not found"),
            Self::Other(e) => write!(f, "{}", e),
        }
    }
}

impl<E: Debug + Display> core::error::Error for Error<E> {}

impl<E: Debug> From<E> for Error<E> {
    fn from(value: E) -> Self {
        Self::Other(value)
    }
}

pub trait FileHandler: ErrorType {
    type File;

    /// open file handle
    fn open(&mut self, path: &str) -> Result<Self::File, Self::Error>;

    /// clone file handle
    fn try_clone(&mut self, file: &Self::File) -> Result<Self::File, Self::Error>;

    /// close file
    fn close(&mut self, file: &Self::File) -> Result<(), Self::Error>;

    /// Read some bytes from this source into the specified buffer, returning how many bytes were read.
    ///
    /// If no bytes are currently available to read, this function blocks until at least one byte is available.
    ///
    /// If bytes are available, a non-zero amount of bytes is read to the beginning of `buf`, and the amount
    /// is returned. It is not guaranteed that *all* available bytes are returned, it is possible for the
    /// implementation to read an amount of bytes less than `buf.len()` while there are more bytes immediately
    /// available.
    ///
    /// If the reader is at end-of-file (EOF), `Ok(0)` is returned. There is no guarantee that a reader at EOF
    /// will always be so in the future, for example a reader can stop being at EOF if another process appends
    /// more bytes to the underlying file.
    ///
    /// If `buf.len() == 0`, `read` returns without blocking, with either `Ok(0)` or an error.
    /// The `Ok(0)` doesn't indicate EOF, unlike when called with a non-empty buffer.
    fn read(&mut self, file: &mut Self::File, buf: &mut [u8]) -> Result<usize, Self::Error>;

    /// Seek to an offset, in bytes, in a stream.
    fn seek(&mut self, file: &mut Self::File, pos: SeekFrom) -> Result<u64, Self::Error>;

    fn read_exact(
        &mut self,
        file: &mut Self::File,
        buf: &mut [u8],
    ) -> Result<(), ReadExactError<Self::Error>> {
        let mut slice = &mut buf[..];
        while !slice.is_empty() {
            let n = self.read(file, slice)?;
            if n == 0 {
                return Err(ReadExactError::UnexpectedEof);
            }
            slice = &mut slice[n..];
        }
        Ok(())
    }

    /// Returns the current seek position from the start of the stream.
    fn stream_position(&mut self, file: &mut Self::File) -> Result<u64, Self::Error> {
        self.seek(file, SeekFrom::Current(0))
    }
}

struct FadeState {
    ready: bool,
    fade_index: f32,
    window_index: usize,
    /// (sample rate, mods)
    onset_data: Option<(u32, state::Mods)>,
}

pub(crate) struct GrainReader {
    grain: [i16; GRAIN_LEN * 2 + 1],
    grain_index: f32,
    fade: [i16; FADE_LEN * 2 + 1],
    window: [f32; WINDOW_LEN],
    fade_state: Option<FadeState>,
}

impl GrainReader {
    /// assume index with [-buffer.len()-1..buffer.len()-1]
    fn at_interpolated(buffer: &[i16], index: f32) -> f32 {
        let index_a = (index.floor() as i64 + buffer.len() as i64 / 2) as usize;
        let sample_a = buffer[index_a] as f32 / i16::MAX as f32;
        let sample_b = buffer[index_a + 1] as f32 / i16::MAX as f32;
        sample_a * (index.ceil() - index) + sample_b * (index - index.floor())
    }

    /// pan ranges from 0. (full left) to 1. (full right)
    fn mixdown_sample(sample: f32, pan: f32, gain: f32) -> (f32, f32) {
        let l = f32::tanh(sample * gain * (1. - pan));
        let r = f32::tanh(sample * gain * pan);
        (l, r)
    }

    pub fn fade<const ONSET_COUNT: usize, F: FileHandler>(
        &mut self,
        onset: Option<&mut state::Modded<signal::Onset<F>>>,
        fs: &mut F,
    ) -> Result<(), F::Error> {
        if let Some(onset) = onset {
            self.fade_state = Some(FadeState {
                ready: false,
                fade_index: self.grain_index - self.grain_index.floor(),
                window_index: 0,
                onset_data: Some((onset.inner.sample_rate, onset.mods.clone())),
            });
            let seek_to = onset.inner.pos(fs)? as i64 + self.grain_index.floor() as i64 * 2 - GRAIN_LEN as i64 * 4;
            onset.inner.seek(seek_to, fs)?;
            let bytes = bytemuck::cast_slice_mut(&mut self.fade);
            onset.inner.read(bytes, fs)?;
        } else {
            self.fade_state = Some(FadeState {
                ready: false,
                fade_index: self.grain_index - self.grain_index.floor(),
                window_index: 0,
                onset_data: None,
            });
            self.fade.fill(0);
        }
        Ok(())
    }

    /// when called, assume pos() already at center of last grain
    fn fill<F: FileHandler>(
        &mut self,
        onset: &mut signal::Onset<F>,
        fs: &mut F,
    ) -> Result<(), F::Error> {
        if let Some(FadeState { ready, .. }) = self.fade_state.as_mut() && !*ready {
            *ready = true;
            let next_center = onset.pos(fs)? as i64;
            onset.seek(next_center - GRAIN_LEN as i64 * 2, fs)?;
            let bytes = bytemuck::cast_slice_mut(&mut self.grain);
            onset.read(bytes, fs)?;
            onset.seek(next_center, fs)?; // recenter pos()  for next read
            self.grain_index = 0.;
        } else if !(-(GRAIN_LEN as i64)..GRAIN_LEN as i64).contains(&(self.grain_index.floor() as i64)) {
            let next_center = onset.pos(fs)? as i64 + self.grain_index.floor() as i64 * 2;
            onset.seek(next_center - GRAIN_LEN as i64 * 2, fs)?;
            let bytes = bytemuck::cast_slice_mut(&mut self.grain);
            onset.read(bytes, fs)?;
            onset.seek(next_center, fs)?; // recenter pos() for next read
            self.grain_index -= self.grain_index.floor();
        }
        Ok(())
    }

    fn advance_indices<F: FileHandler>(
        &mut self,
        onset: Option<&mut state::Modded<signal::Onset<F>>>,
        sample_rate: u32,
    ) {
        if let Some(onset) = onset {
            let grain_delta = f32::from_bits(
                (onset.mods.speed * onset.inner.sample_rate as f32 / sample_rate as f32).to_bits() | (onset.mods.reverse as u32) << 31,
            );
            self.grain_index += grain_delta;
        }
        // approximates linear delta via derivative of atan() with horizontal asymptote at FADE_LEN
        let fade_delta = |linear: f32, window_index: usize| {
            fn sqr<T: core::ops::Mul + Copy>(num: T) -> T::Output {
                num * num
            }
            linear / (1. + sqr(core::f32::consts::FRAC_PI_2 * linear * window_index as f32 / FADE_LEN as f32))
        };
        if let Some(FadeState { fade_index, window_index, onset_data, .. }) = self.fade_state.as_mut() {
            let linear = if let Some((sr, mods)) = onset_data {
                f32::from_bits((mods.speed * *sr as f32 / sample_rate as f32).to_bits() | (mods.reverse as u32) << 31)
            } else {
                1.
            };
            *fade_index += fade_delta(linear, *window_index);
            *window_index += 1;
            if *window_index >= WINDOW_LEN {
                self.fade_state = None;
            }
        }
    }

    fn with_fade(&self, mut grain_sample: f32, grain_mods: Option<&state::Mods>) -> (f32, f32) {
        let (fl, fr) = if let Some(FadeState { fade_index, window_index, onset_data: ref playback_data, .. }) = self.fade_state {
            let window = self.window[window_index];
            grain_sample *= window;
            if let Some((_, mods)) = playback_data {
                Self::mixdown_sample(
                    Self::at_interpolated(&self.fade, fade_index) * (1. - window),
                    mods.pan,
                    mods.gain,
                )
            } else {
                (0., 0.)
            }
        } else {
            (0., 0.)
        };
        let (gl, gr) = if let Some(mods) = grain_mods {
            Self::mixdown_sample(grain_sample, mods.pan, mods.gain)
            } else {
            (0., 0.)
        };
        (fl + gl, fr + gr)
    }

    /// returns `(left sample, right sample)`
    pub fn read<const ONSET_COUNT: usize, F: FileHandler>(
        &mut self,
        event: &mut state::OnsetEvent<state::Modded<signal::Onset<F>>>,
        sample_rate: u32,
        fs: &mut F,
    ) -> Result<(f32, f32), F::Error> {
        match event {
            state::OnsetEvent::Stop => {
                let (l, r) = self.with_fade(0., None);
                self.advance_indices::<F>(None, sample_rate);
                Ok((l, r))
            },
            state::OnsetEvent::Hold { onset, .. } | state::OnsetEvent::Loop { onset, .. } => {
                self.fill(&mut onset.inner, fs)?;
                let (l, r) = self.with_fade(
                    Self::at_interpolated(&self.grain, self.grain_index),
                    Some(&onset.mods),
                );
                self.advance_indices(Some(onset), sample_rate);
                Ok((l, r))
            }
        }
    }
}

impl Default for GrainReader {
    fn default() -> Self {
        let window = core::array::from_fn(|i| {
            0.5 - 0.5 * f32::cos(core::f32::consts::PI * i as f32 / WINDOW_LEN as f32)
        });
        Self {
            grain: [0; GRAIN_LEN * 2 + 1],
            grain_index: 0.,
            fade: [0; FADE_LEN * 2 + 1],
            window,
            fade_state: None,
        }
    }
}

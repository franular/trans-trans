use super::{
    fs::{Error, FileHandler},
    signal,
};
use embedded_io::ReadExactError;

#[derive(Copy, Clone, PartialEq)]
pub enum Snap {
    /// on tick for ramp; on step for phrase
    Micro,
    /// on onset for ramp; on meas for phrase
    Macro,
}

impl core::fmt::Display for Snap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Micro => write!(f, "micro"),
            Self::Macro => write!(f, "macro"),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum OnsetInput {
    Stop,
    Hold { index: u8 },
    Loop { index: u8, len: u32 },
}

pub enum PhraseInput {
    Stop,
    Hold { index: u8 },
}

pub enum RecordInput {
    Stop,
    Start,
}

struct Buffer<const LAYER_COUNT: usize> {
    onsets: [[Option<OnsetInput>; 2]; LAYER_COUNT],
    phrases: [Option<PhraseInput>; LAYER_COUNT],
    record: Option<RecordInput>,
}

impl<const LAYER_COUNT: usize> Default for Buffer<LAYER_COUNT> {
    fn default() -> Self {
        Self {
            onsets: core::array::from_fn(|_| core::array::from_fn(|_| None)),
            phrases: core::array::from_fn(|_| None),
            record: None,
        }
    }
}

struct Ramp {
    tick: u32,
    base: f32,
    mult: f32,
    delta: f32,
}

impl Ramp {
    fn new() -> Self {
        Ramp { tick: 0, base: 1., mult: 1., delta: 0. }
    }

    fn advance(&mut self, advance_fn: impl Fn(f32, f32) -> f32) {
        self.mult = advance_fn(self.mult, self.delta * self.tick as f32);
        self.tick = 0;
    }

    fn net(&self) -> f32 {
        self.base * self.mult
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Mods {
    pub pan: f32,
    pub gain: f32,
    pub speed: f32,
    pub reverse: bool,
}

impl Default for Mods {
    fn default() -> Self {
        Self { pan: 0.5, gain: 1., speed: 1., reverse: false }
    }
}

#[derive(Debug)]
pub struct Modded<T> {
    pub(crate) inner: T,
    pub(crate) mods: Mods,
}

impl<T: Clone> Clone for Modded<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), mods: self.mods.clone() }
    }
}

impl<T: Default> Default for Modded<T> {
    fn default() -> Self {
        Self { inner: T::default(), mods: Mods::default() }
    }
}

#[derive(Default)]
pub enum OnsetEvent<O> {
    #[default]
    Stop,
    Hold { tick: i32, onset: O, index: u8 },
    Loop { tick: i32, onset: O, index: u8, len: u32 },
}

impl<O: Clone> Clone for OnsetEvent<O> {
    fn clone(&self) -> Self {
        match self {
            Self::Stop => Self::Stop,
            Self::Hold { tick, onset, index } => Self::Hold { tick: *tick, onset: onset.clone(), index: *index },
            Self::Loop { tick, onset, index, len } => Self::Loop { tick: *tick, onset: onset.clone(), index: *index, len: *len },
        }
    }
}

impl<O> core::fmt::Debug for OnsetEvent<O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stop => write!(f, "Stop"),
            Self::Hold { tick, index, .. } => f.debug_struct("Hold").field("tick", tick).field("index", index).finish(),
            Self::Loop { tick, index, len, .. } => f.debug_struct("Loop").field("tick", tick).field("index", index).field("len", len).finish(),
        }
    }
}

impl<F: FileHandler> OnsetEvent<Modded<signal::Onset<F>>> {
    pub fn tick(
        self,
        to: &Modded<Option<OnsetEvent<Onset>>>,
        ticks_per_beat: u32,
        fs: &mut F,
    ) -> Result<Self, Error<F::Error>> {
        match to.inner {
            None => match self {
                OnsetEvent::Stop => Ok(OnsetEvent::Stop),
                OnsetEvent::Hold { mut tick, mut onset, index, .. } => {
                    tick += if to.mods.reverse { -1 } else { 1 };
                    if onset.inner.onset_start.is_none()
                        && !(0..(onset.inner.beat_count * ticks_per_beat) as i32).contains(&tick)
                    {
                        return Ok(OnsetEvent::Stop);
                    }
                    onset.inner.seek_from_start(tick, ticks_per_beat, fs)?;
                    onset.mods = to.mods.clone();
                    Ok(OnsetEvent::Hold { tick, onset, index })
                }
                OnsetEvent::Loop { mut tick, mut onset, index, len, .. } => {
                    tick += if to.mods.reverse { -1 } else { 1 };
                    if !(0..len as i32).contains(&tick) {
                        if onset.inner.onset_start.is_none() {
                            return Ok(OnsetEvent::Stop);
                        }
                        tick = tick.rem_euclid(len as i32);
                    }
                    onset.inner.seek_from_start(tick, ticks_per_beat, fs)?;
                    onset.mods = to.mods.clone();
                    Ok(OnsetEvent::Loop { tick, onset, index, len })
                }
            }
            Some(OnsetEvent::Stop) => {
                match self {
                    OnsetEvent::Stop => (),
                    OnsetEvent::Hold { onset: o, .. } | OnsetEvent::Loop { onset: o, .. } => fs.close(&o.inner.file)?,
                }
                Ok(OnsetEvent::Stop)
            }
            Some(OnsetEvent::Hold { tick, ref onset, index, .. }) => {
                let o = match self {
                    OnsetEvent::Stop => None,
                    OnsetEvent::Hold { onset, .. } | OnsetEvent::Loop { onset, .. } => Some(onset),
                };
                let mut onset = onset.clone().open(o.map(|m| m.inner), fs)?;
                onset.seek_from_start(tick, ticks_per_beat, fs)?;
                Ok(OnsetEvent::Hold { tick, onset: Modded { inner: onset, mods: to.mods.clone() }, index })
            }
            Some(OnsetEvent::Loop { mut tick, ref onset, index, len, .. }) => {
                tick = tick.rem_euclid(len as i32);
                let o = match self {
                    OnsetEvent::Stop => None,
                    OnsetEvent::Hold { onset, .. } | OnsetEvent::Loop { onset, .. } => Some(onset),
                };
                let mut onset = onset.clone().open(o.map(|m| m.inner), fs)?;
                onset.seek_from_start(tick, ticks_per_beat, fs)?;
                Ok(OnsetEvent::Loop { tick, onset: Modded { inner: onset, mods: to.mods.clone() }, index, len })
            }
        }
    }
}

#[derive(Clone)]
pub struct Message<const LAYER_COUNT: usize> {
    pub(crate) ticks_per_beat: u32,
    pub(crate) ticks_per_meas: u32,
    pub(crate) inputs: [Option<OnsetInput>; LAYER_COUNT],
    pub(crate) events: [Modded<Option<OnsetEvent<Onset>>>; LAYER_COUNT],
}

impl<const LAYER_COUNT: usize> Default for Message<LAYER_COUNT> {
    fn default() -> Self {
        Self {
            ticks_per_beat: 1,
            ticks_per_meas: 1,
            inputs: core::array::from_fn(|_| None),
            events: core::array::from_fn(|_| Modded::default()),
        }
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Onset {
    pub path: String,
    pub onset_start: Option<u32>,
    pub beat_count: u32,
}

impl Onset {
    pub(crate) fn open<F: FileHandler>(
        self,
        replace: Option<signal::Onset<F>>,
        fs: &mut F,
    ) -> Result<signal::Onset<F>, Error<F::Error>> {
        if let Some(mut replace) = replace {
            if self.path == replace.path {
                replace.onset_start = self.onset_start;
                return Ok(replace);
            } else {
                fs.close(&replace.file)?;
            }
        }
        let mut file = fs.open(&self.path)?;
        let re_err = |e| match e {
            ReadExactError::UnexpectedEof => Error::DataNotFound,
            ReadExactError::Other(e) => Error::Other(e),
        };
        let assert = |b: bool| if !b { Err(Error::BadFormat) } else { Ok(()) };
        // parse wav looking for metadata and `data` subchunk
        let mut pcm_start = 0;
        let mut pcm_len = 0;
        let mut sample_rate = 0;
        let mut essential_chunks_parsed = 0;
        while essential_chunks_parsed < 3 {
            let mut header = [0u8; 8];
            fs.read_exact(&mut file, &mut header).map_err(re_err)?;
            if &header[0..=3] == b"RIFF" {
                let mut data = [0u8; 4];
                fs.read_exact(&mut file, &mut data).map_err(re_err)?;
                assert(&data[..] == b"WAVE")?;
                essential_chunks_parsed += 1;
            } else if &header[0..=3] == b"fmt " {
                assert(u32::from_le_bytes(header[4..=7].try_into().unwrap()) == 16)?; // `fmt ` chunk size
                let mut data = [0u8; 16];
                fs.read_exact(&mut file, &mut data).map_err(re_err)?;
                assert(u16::from_le_bytes(data[0..=1].try_into().unwrap()) == 1)?; // pcm integer format
                assert(u16::from_le_bytes(data[2..=3].try_into().unwrap()) == 1)?; // 1 channel
                sample_rate = u32::from_le_bytes(data[4..=7].try_into().unwrap());
                assert(u16::from_le_bytes(data[14..=15].try_into().unwrap()) == 16)?; // 16 bits/sample
                essential_chunks_parsed += 1;
            } else if &header[0..=3] == b"data" {
                pcm_start = fs.stream_position(&mut file)? as u32;
                pcm_len = u32::from_le_bytes(header[4..=7].try_into().unwrap());
                essential_chunks_parsed += 1;
            } else {
                let chunk_len = u32::from_le_bytes(header[4..=7].try_into().unwrap());
                fs.seek(&mut file, embedded_io::SeekFrom::Current(chunk_len as i64))?;
            }
        }
        let onset = signal::Onset {
            path: self.path,
            file,
            pcm_start,
            pcm_len,
            onset_start: self.onset_start,
            beat_count: self.beat_count,
            sample_rate,
        };
        Ok(onset)
    }
}

pub struct Phrase<const PHRASE_LEN: usize> {
    events: [Modded<Option<OnsetEvent<Onset>>>; PHRASE_LEN],
    pub start: u32,
    pub len: u32,
}

impl<const PHRASE_LEN: usize> Default for Phrase<PHRASE_LEN> {
    fn default() -> Self {
        Self {
            events: core::array::from_fn(|_| Modded::default()),
            start: 0,
            len: 0,
        }
    }
}

/// running phrase reading from start..start+len ticks (wrapping) of indexed Phrase
pub struct PhraseReader {
    index: u8,
    tick: u32,
}

impl PhraseReader {
    fn start(index: u8) -> Self {
        Self { index, tick: 0 }
    }
}

#[derive(Default)]
enum WriterState {
    #[default]
    Idle,
    Recording { start: u32, len: u32 },
}

pub struct PhraseWriter<const PHRASE_COUNT: usize, const PHRASE_LEN: usize> {
    /// indices of `StateHandler`'s Phrases which is overwritten by `Self::store`
    /// defaults to all Phrases
    store_mask: heapless::Vec<u8, PHRASE_COUNT>,
    store_index: usize,
    queue: heapless::HistoryBuf<Modded<Option<OnsetEvent<Onset>>>, PHRASE_LEN>,
    state: WriterState,
}

impl<const PHRASE_COUNT: usize, const PHRASE_LEN: usize> PhraseWriter<PHRASE_COUNT, PHRASE_LEN> {
    pub fn push(&mut self, event: Modded<Option<OnsetEvent<Onset>>>) {
        self.queue.write(event);
        if let WriterState::Recording { len, .. } = &mut self.state { *len += 1 };
    }

    /// should be called before input tick
    pub fn try_start(&mut self) {
        if matches!(self.state, WriterState::Idle) {
            self.state = WriterState::Recording {
                start: ((self.queue.recent_index().unwrap_or(PHRASE_LEN)) % PHRASE_LEN) as u32,
                len: 0,
            };
        }
    }

    /// should be called before input tick
    pub fn try_store(&mut self, min_len: u32) -> Option<Phrase<PHRASE_LEN>> {
        if let WriterState::Recording { start, mut len } = self.state && len > min_len {
            len = len.min(PHRASE_LEN as u32);
            let phrase = Phrase {
                events: core::array::from_fn(|i| self.queue.as_slice().get(i).cloned().unwrap_or_default()),
                start,
                len,
            };
            self.state = WriterState::Idle;
            return Some(phrase);
        }
        self.state = WriterState::Idle;
        None
    }

    pub fn try_advance(&mut self) -> Option<u8> {
        if self.store_mask.is_empty() {
            return None
        }
        let ret = Some(self.store_mask[self.store_index]);
        self.store_index = (self.store_index + 1) % self.store_mask.len();
        ret
    }
}

impl<const PHRASE_COUNT: usize, const PHRASE_LEN: usize> Default for PhraseWriter<PHRASE_COUNT, PHRASE_LEN> {
    fn default() -> Self {
        Self {
            store_mask: heapless::Vec::from_iter(0..PHRASE_COUNT as u8),
            store_index: Default::default(),
            queue: Default::default(),
            state: Default::default(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Kit<const KIT_LEN: usize> {
    #[serde(with = "serde_arrays")]
    pub onsets: [Option<Onset>; KIT_LEN],
}

pub struct StateHandler<const BANK_COUNT: usize, const LAYER_COUNT: usize, const KIT_COUNT: usize, const KIT_LEN: usize, const PHRASE_COUNT: usize, const PHRASE_LEN: usize> {
    pub layer: u8,
    pub kits: [Option<Kit<KIT_LEN>>; KIT_COUNT],
    kit_indices: [[u8; BANK_COUNT]; LAYER_COUNT],

    pub phrases: [Option<Phrase<PHRASE_LEN>>; PHRASE_COUNT],
    pub phrase_writer: PhraseWriter<PHRASE_COUNT, PHRASE_LEN>,
    pub phrase_snap_start: Snap,
    pub phrase_snap_len: Snap,

    buffer: Buffer<LAYER_COUNT>,
    pub active_onsets: [OnsetEvent<Modded<Onset>>; LAYER_COUNT],
    pub active_phrases: [Option<PhraseReader>; LAYER_COUNT],

    pub pitch_interval: f32,
    pub ramp_snap: Snap,
    gain_ramps: [Ramp; LAYER_COUNT],
    pitch_ramps: [Ramp; LAYER_COUNT],
    widths: [f32; LAYER_COUNT],
    reverse: bool,

    pass_input: bool,
    ticks_per_input: u32,
    ticks_per_beat: u32,
    /// akin to time signature denominator
    ticks_per_step: u32,
    /// akin to time signature numerator
    steps_per_meas: u32,
}

impl<const BANK_COUNT: usize, const LAYER_COUNT: usize, const KIT_COUNT: usize, const KIT_LEN: usize, const PHRASE_COUNT: usize, const PHRASE_LEN: usize> StateHandler<BANK_COUNT, LAYER_COUNT, KIT_COUNT, KIT_LEN, PHRASE_COUNT, PHRASE_LEN> {
    pub fn new(
        phrase_snap_start: Snap,
        phrase_snap_len: Snap,
        pitch_interval: f32,
        ramp_snap: Snap,
        ticks_per_input: u32,
        ticks_per_beat: u32,
        ticks_per_step: u32,
        steps_per_meas: u32,
    ) -> Self {
        Self {
            layer: 0,
            kits: core::array::from_fn(|_| None),
            kit_indices: [[0; BANK_COUNT]; LAYER_COUNT],

            phrases: core::array::from_fn(|_| None),
            phrase_writer: PhraseWriter::default(),
            phrase_snap_start,
            phrase_snap_len,

            buffer: Buffer::default(),
            active_onsets: core::array::from_fn(|_| OnsetEvent::Stop),
            active_phrases: core::array::from_fn(|_| None),

            pitch_interval,
            ramp_snap,
            gain_ramps: core::array::from_fn(|_| Ramp::new()),
            pitch_ramps: core::array::from_fn(|_| Ramp::new()),
            widths: [1.; LAYER_COUNT],
            reverse: false,
            
            pass_input: false,
            ticks_per_input,
            ticks_per_beat,
            ticks_per_step,
            steps_per_meas,
        }
    }

    fn pass_net(&mut self) -> Message<LAYER_COUNT> {
        self.try_pass();
        self.net()
    }

    fn net(&mut self) -> Message<LAYER_COUNT> {
        let mut net = Message {
            ticks_per_beat: self.ticks_per_beat,
            ticks_per_meas: self.ticks_per_step * self.steps_per_meas,
            inputs: self.buffer.onsets.clone().map(|i| i[0].clone()),
            events: core::array::from_fn(|l| Modded { inner: None, mods: match &self.active_onsets[l] {
                OnsetEvent::Hold { onset, .. } | OnsetEvent::Loop { onset, .. } => onset.mods.clone(),
                _ => Mods::default(),
            } }),
        };
        for l in 0..LAYER_COUNT {
            let input_onset = &mut self.buffer.onsets[l];
            let active_onset = &mut self.active_onsets[l];
            let active_phrase = &mut self.active_phrases[l];
            let net = &mut net.events[l];
            if matches!(input_onset[0], Some(OnsetInput::Stop)) && let Some(phrase) = active_phrase {
                // output synced active phrase, if any
                let Phrase { events, start, len } = &self.phrases[phrase.index as usize].as_ref().unwrap();
                let mut phrase_tick_offset = 0;
                let mut onset_tick_offset = 0;
                let s_index = |tick: i32| (*start as i32 + (phrase.tick as i32 - tick).rem_euclid(*len as i32)).rem_euclid(PHRASE_LEN as i32) as usize;
                while onset_tick_offset < *len as i32 && matches!(events[s_index(phrase_tick_offset)].inner, None | Some(OnsetEvent::Stop)) {
                    phrase_tick_offset += 1;
                    onset_tick_offset += if events[s_index(phrase_tick_offset)].mods.reverse { -1 } else { 1 };
                }
                let mut e = events[s_index(onset_tick_offset)].inner.clone();
                match e {
                    Some(OnsetEvent::Hold { ref mut tick, .. }) => *tick += onset_tick_offset,
                    Some(OnsetEvent::Loop { ref mut tick, len, .. }) => *tick = (*tick + onset_tick_offset).rem_euclid(len as i32),
                    _ => (),
                }
                *net = Modded { inner: e, mods: events[(start + phrase.tick) as usize % PHRASE_LEN].mods.clone() };
            } else if matches!(active_onset, OnsetEvent::Stop) && let Some(phrase) = active_phrase {
                // output from active phrase, if any
                let Phrase { events, start, .. } = &self.phrases[phrase.index as usize].as_ref().unwrap();
                *net = events[(start + phrase.tick) as usize % PHRASE_LEN].clone();
            } else if input_onset[0].is_some() {
                // output active onset event
                match active_onset {
                    OnsetEvent::Stop => net.inner = Some(OnsetEvent::Stop),
                    OnsetEvent::Hold { tick, onset, index } => *net = Modded {
                        inner: Some(OnsetEvent::Hold { tick: *tick, onset: onset.inner.clone(), index: *index }),
                        mods: onset.mods.clone(),
                    },
                    OnsetEvent::Loop { tick, onset, index, len } => *net = Modded {
                        inner: Some(OnsetEvent::Loop { tick: *tick, onset: onset.inner.clone(), index: *index, len: *len }),
                        mods: onset.mods.clone(),
                    },
                };
            }
        }
        net
    }

    fn try_pass(&mut self) {
        if !self.pass_input { return };
        for l in 0..LAYER_COUNT {
            let input_onset = &mut self.buffer.onsets[l];
            let input_phrase = &mut self.buffer.phrases[l];
            let active_onset = &mut self.active_onsets[l];
            let active_phrase = &mut self.active_phrases[l];
            // flush phrase input
            if let Some(input) = input_phrase.take() {
                match input {
                    PhraseInput::Stop => {
                        *active_phrase = None;
                        if matches!(active_onset, OnsetEvent::Stop) {
                            input_onset[0] = Some(OnsetInput::Stop)
                        }
                    }
                    PhraseInput::Hold { index } => *active_phrase = if self.phrases[index as usize].is_some() {
                        Some(PhraseReader::start(index))
                    } else {
                        None
                    },
                }
            }
            // pass onset input
            if let Some(input) = input_onset[0].clone() {
                *active_onset = match input {
                    OnsetInput::Stop => OnsetEvent::Stop,
                    OnsetInput::Hold { index } => if let Some(onset) = self.kits[self.kit_indices[l][index as usize * BANK_COUNT / KIT_LEN] as usize].as_ref().and_then(|k| k.onsets[index as usize].as_ref()) {
                        let tick = if let OnsetEvent::Loop { tick, index: i, .. } = active_onset && *i == index {
                            *tick
                        } else {
                            0
                        };
                        let onset = Modded {
                            inner: onset.clone(),
                            mods: Mods {
                                pan: ((index as f32 / (KIT_LEN - 1) as f32) - 0.5) * self.widths[l] + 0.5,
                                gain: self.gain_ramps[l].net(),
                                speed: self.pitch_ramps[l].net(),
                                reverse: self.reverse,
                            },
                        };
                        OnsetEvent::Hold { tick, onset, index }
                    } else {
                        OnsetEvent::Stop
                    }
                    OnsetInput::Loop { index, len } => if let Some(onset) = self.kits[self.kit_indices[l][index as usize * BANK_COUNT / KIT_LEN] as usize].as_ref().and_then(|k| k.onsets[index as usize].as_ref()) {
                        let tick = if let OnsetEvent::Hold { tick, index: i, .. } | OnsetEvent::Loop { tick, index: i, .. } = active_onset && *i == index {
                            tick.rem_euclid(len as i32)
                        } else {
                            0
                        };
                        let onset = Modded {
                            inner: onset.clone(),
                            mods: Mods {
                                pan: ((index as f32 / (KIT_LEN - 1) as f32) - 0.5) * self.widths[l] + 0.5,
                                gain: self.gain_ramps[l].net(),
                                speed: self.pitch_ramps[l].net(),
                                reverse: self.reverse,
                            },
                        };
                        OnsetEvent::Loop { tick, onset, index, len }
                    } else {
                        OnsetEvent::Stop
                    }
                }
            }
        }
        // flush record input
        if let Some(input) = self.buffer.record.take() {
            match input {
                RecordInput::Stop => {
                    if let Some(phrase) = self.phrase_writer.try_store(self.ticks_per_input) && let Some(store_to) = self.phrase_writer.try_advance() {
                        self.phrases[store_to as usize] = Some(phrase);
                        let _ = self.push_phrase(PhraseInput::Hold { index: store_to });
                        if matches!(self.phrase_snap_start,Snap::Macro) {
                            self.mod_phrase_start(1);
                        } else {
                            self.mod_phrase_start(0);
                        }
                        self.mod_phrase_len(0);
                    }
                }
                RecordInput::Start => self.phrase_writer.try_start(),
            }
        }
    }

    pub fn tick(&mut self, message: signal::Message<LAYER_COUNT>) -> Message<LAYER_COUNT> {
        let pass_input = (message.tick + 1).rem_euclid(self.ticks_per_input as i32) == 0;
        if pass_input {
            // rising edge; just before input tick
        } else if self.pass_input {
            // falling edge; after input tick
            for l in 0..LAYER_COUNT {
                // only consume current input when input has been received by SignalHandler
                if self.buffer.onsets[l][0] == message.inputs[l] {
                    self.buffer.onsets[l][0] = self.buffer.onsets[l][1].take();
                }
            }
            if let OnsetEvent::Hold { onset, .. } | OnsetEvent::Loop { onset, .. } = &mut self.active_onsets[self.layer as usize] {
                onset.mods.reverse = self.reverse;
            }
        }
        self.pass_input = pass_input;
        for l in 0..LAYER_COUNT {
            match self.active_onsets[l] {
                OnsetEvent::Stop => (),
                OnsetEvent::Hold { ref mut tick, ref onset, .. } => *tick += if onset.mods.reverse { -1 } else { 1 },
                OnsetEvent::Loop { ref mut tick, ref onset, len, .. } => *tick = (*tick + if onset.mods.reverse { -1 } else { 1 }).rem_euclid(len as i32),
            }
            // flush ramps, width to Modded
            self.gain_ramps[l].tick += 1;
            self.pitch_ramps[l].tick += 1;
            if match self.ramp_snap {
                Snap::Micro => true,
                Snap::Macro => {
                    match self.active_onsets[l] {
                        OnsetEvent::Stop => false,
                        OnsetEvent::Hold { tick, .. } => tick == 0,
                        OnsetEvent::Loop { tick, len, .. } => (tick + 1).rem_euclid(len as i32) == 0,
                    }
                }
            } {
                self.gain_ramps[l].advance(|v, d| (v + d).clamp(0., 2.));
                self.pitch_ramps[l].advance(|v, d| {
                    (v * self.pitch_interval.powf(d)).clamp(self.pitch_interval.powf(-16.), self.pitch_interval.powf(16.))
                });
            }
            if let OnsetEvent::Hold { ref mut onset, index, .. } | OnsetEvent::Loop { ref mut onset, index, .. } = self.active_onsets[l] {
                onset.mods.pan = ((index as f32 / (KIT_LEN - 1) as f32) - 0.5) * self.widths[l] + 0.5;
                onset.mods.gain = self.gain_ramps[l].net();
                onset.mods.speed = self.pitch_ramps[l].net();
            }
        }
        self.try_pass();
        for reader in self.active_phrases.iter_mut().flatten() {
            if let Some(Phrase { len, .. }) = &self.phrases[reader.index as usize] {
                reader.tick = (reader.tick as i32 + 1).rem_euclid(*len as i32) as u32;
            }
        }
        let net = self.net();
        self.phrase_writer.push(net.events[self.layer as usize].clone());
        net
    }

    pub fn push_onset(&mut self, input: OnsetInput) -> Message<LAYER_COUNT> {
        if matches!(input, OnsetInput::Stop)
            && let OnsetEvent::Hold { onset, .. } | OnsetEvent::Loop { onset, .. } = &self.active_onsets[self.layer as usize]
            && onset.inner.onset_start.is_none() {
            return self.pass_net();
        }
        if !matches!(input, OnsetInput::Loop { .. })
            && matches!(self.buffer.onsets[self.layer as usize][0], Some(OnsetInput::Hold { .. } | OnsetInput::Loop { .. }))
        {
            // buffer rapid Stop or Hold event to next tick so the input doesn't seem to disappear
            self.buffer.onsets[self.layer as usize][1] = Some(input);
        } else {
            self.buffer.onsets[self.layer as usize][0] = Some(input);
        }
        self.pass_net()
    }

    pub fn push_phrase(&mut self, input: PhraseInput) -> Message<LAYER_COUNT> {
        self.buffer.phrases[self.layer as usize] = Some(input);
        self.pass_net()
    }

    pub fn push_record(&mut self, input: RecordInput) -> Message<LAYER_COUNT> {
        self.buffer.record = Some(input);
        self.pass_net()
    }

    pub fn base_gain(&mut self, value: f32, layer: u8) -> Message<LAYER_COUNT> {
        self.gain_ramps[layer as usize].base = value + 1.;
        self.pass_net()
    }

    pub fn mult_gain(&mut self, value: f32) -> Message<LAYER_COUNT> {
        self.gain_ramps[self.layer as usize].mult = value + 1.;
        self.pass_net()
    }

    pub fn ramp_gain(&mut self, delta: f32) -> Message<LAYER_COUNT> {
        self.gain_ramps[self.layer as usize].delta = delta / self.ticks_per_beat as f32;
        self.pass_net()
    }

    pub fn base_pitch(&mut self, value: f32, layer: u8) -> Message<LAYER_COUNT> {
        self.pitch_ramps[layer as usize].base = self.pitch_interval.powf(value);
        self.pass_net()
    }

    pub fn mult_pitch(&mut self, value: f32) -> Message<LAYER_COUNT> {
        self.pitch_ramps[self.layer as usize].mult = self.pitch_interval.powf(value);
        self.pass_net()
    }

    pub fn ramp_pitch(&mut self, delta: f32) -> Message<LAYER_COUNT> {
        self.pitch_ramps[self.layer as usize].delta = delta / self.ticks_per_beat as f32;
        self.pass_net()
    }

    pub fn push_width(&mut self, value: f32, layer: u8) -> Message<LAYER_COUNT> {
        self.widths[layer as usize] = value.clamp(0., 1.);
        self.pass_net()
    }

    pub fn push_reverse(&mut self, value: bool) -> Message<LAYER_COUNT> {
        self.reverse = value;
        self.pass_net()
    }

    pub fn set_ticks_per_beat(&mut self, value: u32) -> Message<LAYER_COUNT> {
        self.ticks_per_beat = value.max(1);
        self.pass_net()
    }

    pub fn set_ticks_per_input(&mut self, value: u32) -> Message<LAYER_COUNT> {
        self.ticks_per_input = value.max(1);
        self.ticks_per_step = self.ticks_per_step.max(self.ticks_per_input);
        self.pass_net()
    }

    pub fn set_ticks_per_step(&mut self, value: u32) -> Message<LAYER_COUNT> {
        self.ticks_per_step = value.max(self.ticks_per_input);
        self.pass_net()
    }

    pub fn set_steps_per_meas(&mut self, value: u32) -> Message<LAYER_COUNT> {
        self.steps_per_meas = value.max(1);
        self.pass_net()
    }

    pub fn set_record_mask(&mut self, mask: &[u8]) {
        let mut mask = heapless::Vec::from_slice(mask).unwrap();
        mask.sort();
        self.phrase_writer.store_mask = mask;
        self.phrase_writer.store_index = 0;
    }

    pub fn set_kit_index(&mut self, bank: u8, index: u8) {
        self.kit_indices[self.layer as usize][bank as usize] = index;
    }

    /// for display purposes in external impl
    pub fn get_ticks_per_step(&self) -> u32 {
        self.ticks_per_step
    }

    /// for display purposes in external impl
    pub fn get_ticks_per_beat(&self) -> u32 {
        self.ticks_per_beat
    }

    /// for display purposes in external impl
    pub fn get_record_mask(&self) -> &[u8] {
        &self.phrase_writer.store_mask
    }

    /// for display purposes in external impl
    pub fn get_reader_tick(&self, index: u8) -> Option<u32> {
        self.active_phrases.iter().flatten().find(|r| r.index == index).map(|r| r.tick)
    }

    /// should be called before `StateHandler::tick`
    pub fn mod_phrase_start(&mut self, delta: i32) {
        if let Some(reader) = &self.active_phrases[self.layer as usize] {
            let Phrase { events, start, .. } = self.phrases[reader.index as usize].as_mut().unwrap();
            match self.phrase_snap_start {
                Snap::Micro => {
                    let scale = self.ticks_per_step as i32;
                    *start = ((*start as i32 + delta * scale) / scale * scale).rem_euclid(PHRASE_LEN as i32) as u32;
                }
                Snap::Macro => {
                    // let scale = self.ticks_per_step as i32 * self.steps_per_meas as i32;
                    // *start = ((*start as i32 + delta * scale) / scale * scale).rem_euclid(PHRASE_LEN as i32) as u32;
                    if delta == 0 { return; }
                    if delta.is_negative() && let Some((delta, _)) = events
                        .iter()
                        .rev()
                        .cycle()
                        .skip((-(*start as i32) + 1).rem_euclid(PHRASE_LEN as i32) as usize)
                        .enumerate()
                        .filter(|(_, m)| m.inner.as_ref().is_some_and(|e| !matches!(e, OnsetEvent::Stop)))
                        .nth(delta.unsigned_abs() as usize - 1)
                    {
                        *start = (*start as i32 - delta as i32 - 2).rem_euclid(PHRASE_LEN as i32) as u32;
                    } else if let Some((delta, _)) = events
                        .iter()
                        .cycle()
                        .skip(*start as usize + 1)
                        .enumerate()
                        .filter(|(_, m)| m.inner.as_ref().is_some_and(|e| !matches!(e, OnsetEvent::Stop)))
                        .nth(delta as usize - 1)
                    {
                        *start = (*start + delta as u32 + 1).rem_euclid(PHRASE_LEN as u32);
                    }
                }
            }
        }
    }

    /// should be called before `StateHandler::tick`
    pub fn mod_phrase_len(&mut self, delta: i32) {
        if let Some(reader) = &self.active_phrases[self.layer as usize] {
            let Phrase { len, .. } = self.phrases[reader.index as usize].as_mut().unwrap();
            match self.phrase_snap_len {
                Snap::Micro => {
                    let scale = self.ticks_per_step as i32;
                    *len = (((*len as i32 + delta * scale) / scale).max(1) * scale) as u32;
                }
                Snap::Macro => {
                    let scale = self.ticks_per_step as i32 * self.steps_per_meas as i32;
                    *len = (((*len as i32 + delta * scale) / scale).max(1) * scale) as u32;
                }
            }
        }
    }
}

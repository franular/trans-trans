use super::{
    fs::{Error, FileHandler, GrainReader},
    signal,
};
use embedded_io::ReadExactError;

#[derive(Copy, Clone, PartialEq, Default)]
pub enum Snap {
    #[default]
    Tick,
    Beat,
    Input,
    Onset,
}

impl std::fmt::Display for Snap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tick => write!(f, "tick"),
            Self::Beat => write!(f, "beat"),
            Self::Input => write!(f, "input"),
            Self::Onset => write!(f, "onset"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum OnsetInput {
    Sync,
    Hold { index: u8 },
    Loop { index: u8, len: u32 },
}

pub enum StateInput {
    Reverse(bool),
}

pub enum PhraseInput {
    Push { index: u8 },
    Pop { index: u8 },
}

pub enum RecordInput {
    Start,
    Store,
}

#[derive(Default)]
struct Buffer {
    onset: [Option<OnsetInput>; 2],
    reverse: Option<bool>,
    speed: Option<f32>,
    phrase: Vec<PhraseInput>,
    record: Option<RecordInput>,
}

struct Ramp {
    speed: f32,
    delta: f32,
    snap: Snap,
}

impl Ramp {
    fn advance_speed(&mut self) {
        self.speed *= self.delta;
    }
}

impl Default for Ramp {
    fn default() -> Self {
        Self { speed: 1., delta: 1., snap: Snap::Tick }
    }
}

#[derive(Debug)]
pub enum OnsetEvent<O> {
    Sync,
    Hold { tick: i32, onset: O, index: u8 },
    Loop { tick: i32, onset: O, index: u8, len: u32 },
}

impl From<OnsetInput> for OnsetEvent<()> {
    fn from(value: OnsetInput) -> Self {
        match value {
            OnsetInput::Sync => OnsetEvent::Sync,
            OnsetInput::Hold { index } => OnsetEvent::Hold { tick: 0, onset: (), index },
            OnsetInput::Loop { index, len } => OnsetEvent::Loop { tick: 0, onset: (), index, len },
        }
    }
}

impl OnsetEvent<()> {
    pub fn trans(
        &self,
        event: OnsetInput,
    ) -> Self {
        match event {
            OnsetInput::Sync => OnsetEvent::Sync,
            OnsetInput::Hold { index } => match self {
                OnsetEvent::Sync => OnsetEvent::Hold { tick: 0, onset: (), index },
                OnsetEvent::Hold { tick, index: i, .. } | OnsetEvent::Loop { tick, index: i, .. } => {
                    let tick = if *i == index { *tick } else { 0 };
                    OnsetEvent::Hold { tick, onset: (), index }
                }
            }
            OnsetInput::Loop { index, len } => match self {
                OnsetEvent::Sync => OnsetEvent::Loop { tick: 0, onset: (), index, len },
                OnsetEvent::Hold { tick, index: i, .. } | OnsetEvent::Loop { tick, index: i, .. } => {
                    let tick = if *i == index { tick.rem_euclid(len as i32) } else { 0 };
                    OnsetEvent::Loop { tick, onset: (), index, len }
                }
            }
        }
    }

    pub fn open(
        &self,
        kit: Option<&Kit>,
    ) -> OnsetEvent<Onset> {
        match self {
            OnsetEvent::Sync => OnsetEvent::Sync,
            OnsetEvent::Hold { tick, index, .. } => {
                if let Some(kit) = kit {
                    OnsetEvent::Hold { tick: *tick, onset: kit.onsets[*index as usize].clone(), index: *index }
                } else {
                    OnsetEvent::Sync
                }
            }
            OnsetEvent::Loop { tick, index, len, .. } => {
                if let Some(kit) = kit {
                    OnsetEvent::Loop { tick: *tick, onset: kit.onsets[*index as usize].clone(), index: *index, len: *len }
                } else {
                    OnsetEvent::Sync
                }
                
            }
        }
    }
}

impl Clone for OnsetEvent<()> {
    fn clone(&self) -> Self {
        match self {
            Self::Sync => Self::Sync,
            Self::Hold { tick, index, .. } => Self::Hold { tick: *tick, onset: (), index: *index },
            Self::Loop { tick, index, len, .. } => Self::Loop { tick: *tick, onset: (), index: *index, len: *len },
        }
    }
}

impl<F: FileHandler> OnsetEvent<signal::Onset<F>> {
    pub fn as_unit(&self) -> OnsetEvent<()> {
        match self {
            OnsetEvent::Sync => OnsetEvent::Sync,
            OnsetEvent::Hold { tick, index, .. } => OnsetEvent::Hold { tick: *tick, onset: (), index: *index },
            OnsetEvent::Loop { tick, index, len, .. } => OnsetEvent::Loop { tick: *tick, onset: (), index: *index, len: *len },
        }
    }

    pub fn trans<const ONSET_COUNT: usize>(
        self,
        ticks_per_beat: u32,
        event: OnsetEvent<Onset>,
        grain: &mut GrainReader,
        fs: &mut F,
    ) -> Result<Self, Error<F::Error>> {
        match event {
            OnsetEvent::Sync => match self {
                OnsetEvent::Sync => Ok(OnsetEvent::Sync),
                OnsetEvent::Hold { onset: mut o, index: i, .. } | OnsetEvent::Loop { onset: mut o, index: i, .. } => {
                    grain.fade::<ONSET_COUNT, F>(Some((i, &mut o)), fs)?;
                    fs.close(&o.file)?;
                    Ok(OnsetEvent::Sync)
                }
            }
            OnsetEvent::Hold { tick, onset, index } => match self {
                OnsetEvent::Sync => {
                    grain.fade::<ONSET_COUNT, F>(None, fs)?;
                    Ok(OnsetEvent::Hold { tick, onset: onset.open_seek(ticks_per_beat, tick, None, fs)?, index })
                }
                OnsetEvent::Hold { onset: mut o, index: i, .. } | OnsetEvent::Loop { onset: mut o, index: i, .. } => {
                    grain.fade::<ONSET_COUNT, F>(Some((i, &mut o)), fs)?;
                    Ok(OnsetEvent::Hold { tick, onset: onset.open_seek(ticks_per_beat, tick, Some(o), fs)?, index })
                }
            }
            OnsetEvent::Loop { tick, onset, index, len } => match self {
                OnsetEvent::Sync => {
                    grain.fade::<ONSET_COUNT, F>(None, fs)?;
                    Ok(OnsetEvent::Loop { tick, onset: onset.open_seek(ticks_per_beat, tick, None, fs)?, index, len })
                }
                OnsetEvent::Hold { onset: mut o, index: i, .. } | OnsetEvent::Loop { onset: mut o, index: i, .. } => {
                    grain.fade::<ONSET_COUNT, F>(Some((i, &mut o)), fs)?;
                    Ok(OnsetEvent::Loop { tick, onset: onset.open_seek(ticks_per_beat, tick, Some(o), fs)?, index, len })
                }
            }
        }
    }
}

pub struct ModState<E> {
    pub event: Option<E>,
    pub reverse: Option<bool>,
    pub speed: Option<f32>,
}

impl<E> ModState<E> {
    pub fn default() -> Self {
        Self {
            event: None,
            reverse: None,
            speed: None,
        }
    }
}

impl Clone for ModState<OnsetInput> {
    fn clone(&self) -> Self {
        Self { event: self.event.clone(), reverse: self.reverse, speed: self.speed }
    }
}

impl Clone for ModState<OnsetEvent<()>> {
    fn clone(&self) -> Self {
        Self { event: self.event.clone(), reverse: self.reverse, speed: self.speed }
    }
}

impl ModState<OnsetEvent<Onset>> {
    pub fn as_unit(&self) -> ModState<OnsetEvent<()>> {
        let event = match self.event {
            None => None,
            Some(OnsetEvent::Sync) => Some(OnsetEvent::Sync),
            Some(OnsetEvent::Hold { tick, index, .. }) => Some(OnsetEvent::Hold { tick, onset: (), index }),
            Some(OnsetEvent::Loop { tick, index, len, .. }) => Some(OnsetEvent::Loop { tick, onset: (), index, len }),
        };
        ModState {
            event,
            reverse: self.reverse,
            speed: self.speed,
        }
    }
}

pub struct State<O> {
    pub event: OnsetEvent<O>,
    pub reverse: bool,
    pub speed: f32,
}

impl<O> State<O> {
    pub fn default() -> Self {
        Self {
            event: OnsetEvent::Sync,
            reverse: false,
            speed: 1.,
        }
    }
}

impl Clone for State<()> {
    fn clone(&self) -> Self {
        Self { event: self.event.clone(), reverse: self.reverse, speed: 1. }
    }
}

impl<F: FileHandler> State<signal::Onset<F>> {
    pub fn as_unit(&self) -> State<()> {
        State {
            event: self.event.as_unit(),
            reverse: self.reverse,
            speed: self.speed,
        }
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Onset {
    pub path: String,
    pub start: u32,
    pub beat_count: u32,
}

impl Onset {
    pub fn open_seek<F: FileHandler>(
        self,
        ticks_per_beat: u32,
        tick: i32,
        replace: Option<signal::Onset<F>>,
        fs: &mut F,
    ) -> Result<signal::Onset<F>, Error<F::Error>> {
        if let Some(mut replace) = replace {
            if self.path == replace.path {
                replace.onset_start = self.start;
                let seek_to = replace.onset_start as i64 * 2 + ((replace.pcm_len as f32 / (replace.beat_count * ticks_per_beat) as f32 * tick as f32) as i64 & !1);
                replace.seek(seek_to, fs)?;
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
        let mut onset = signal::Onset {
            path: self.path,
            file,
            sample_rate,
            pcm_start,
            pcm_len,
            onset_start: self.start,
            beat_count: self.beat_count,
        };
        let seek_to = onset.onset_start as i64 * 2 + ((onset.pcm_len as f32 / (onset.beat_count * ticks_per_beat) as f32 * tick as f32) as i64 & !1);
        onset.seek(seek_to, fs)?;
        Ok(onset)
    }
}

pub struct Phrase<const PHRASE_LEN: usize> {
    states: [ModState<OnsetEvent<()>>; PHRASE_LEN],
    pub start: u32,
    pub len: u32,
}

impl<const PHRASE_LEN: usize> Default for Phrase<PHRASE_LEN> {
    fn default() -> Self {
        Self {
            states: core::array::from_fn(|_| ModState::default()),
            start: 0,
            len: 0,
        }
    }
}

/// running phrase reading from start..start+len ticks (wrapping) of indexed Phrase
pub struct PhraseReader {
    pub index: u8,
    pub tick: u32,
}

#[derive(Default)]
enum WriterState {
    #[default]
    Idle,
    Recording { start: u32, len: u32 },
}

struct PhraseWriter<const PHRASE_COUNT: usize, const PHRASE_LEN: usize> {
    /// indices of `StateHandler`'s Phrases which is overwritten by `Self::store`
    /// defaults to all Phrases!
    store_mask: heapless::Vec<u8, PHRASE_COUNT>,
    store_index: usize,
    queue: heapless::HistoryBuf<ModState<OnsetEvent<()>>, PHRASE_LEN>,
    state: WriterState,
    snap_start: Snap,
    snap_len: Snap,
}

impl<const PHRASE_COUNT: usize, const PHRASE_LEN: usize> PhraseWriter<PHRASE_COUNT, PHRASE_LEN> {
    pub fn push(&mut self, state: ModState<OnsetEvent<()>>) {
        self.queue.write(state);
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
                states: core::array::from_fn(|i| self.queue.as_slice().get(i).cloned().unwrap_or(ModState::default())),
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
            snap_start: Default::default(),
            snap_len: Default::default(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Kit {
    pub onsets: Box<[Onset]>,
}

pub struct DefaultSnap {
    pub start: Snap,
    pub len: Snap,
}

/// NOTE: the Phrase at index 0 is overwritten when a recording is stored
pub struct StateHandler<const KIT_COUNT: usize, const PHRASE_COUNT: usize, const PHRASE_LEN: usize>
where
    [(); PHRASE_COUNT + 1]:,
{
    pub kits: [Option<Kit>; KIT_COUNT],
    pub kit_index: u8,

    pub phrase_stack: heapless::Vec<PhraseReader, {PHRASE_COUNT + 1}>,
    phrase_writer: PhraseWriter<PHRASE_COUNT, PHRASE_LEN>,
    pub phrases: [Option<Phrase<PHRASE_LEN>>; PHRASE_COUNT],

    active_state: State<()>,
    input_state: State<()>,

    buffer: Buffer,
    ramp: Ramp,

    pub pass_input: bool,
    pub ticks_per_beat: u32,
    pub ticks_per_input: u32,
}

impl<const KIT_COUNT: usize, const PHRASE_COUNT: usize, const PHRASE_LEN: usize> StateHandler<KIT_COUNT, PHRASE_COUNT, PHRASE_LEN>
where
    [(); PHRASE_COUNT + 1]: Clone,
{
    pub fn new() -> Self {
        Self {
            kits: core::array::from_fn(|_| None),
            kit_index: 0,

            phrase_stack: heapless::Vec::new(),
            phrase_writer: PhraseWriter::default(),
            phrases: core::array::from_fn(|_| None),

            active_state: State::default(),
            input_state: State::default(),

            buffer: Buffer::default(),
            ramp: Ramp::default(),

            pass_input: false,
            ticks_per_beat: 1,
            ticks_per_input: 1,
        }
    }

    pub fn cumulative_mod(&mut self) -> ModState<OnsetEvent<Onset>> {
        let mut state = ModState::default();
        self.flush_speed();
        if self.pass_input {
            self.flush_record();
            self.flush_phrase();
        }
        let mut dont_sync = false;
        if let Some(reader) = self.top_reader() {
            let Phrase { states, start, len } = &self.phrases[reader.index as usize].as_ref().unwrap();
            let s = states[(start + reader.tick) as usize % PHRASE_LEN].clone();
            if matches!(self.buffer.onset[0], Some(OnsetInput::Sync)) {
                // return to phrase as ticked from last onset if input sync
                dont_sync = true;
                let mut tick_offset = 0;
                let s_index = |tick: i32| (*start as i32 + (reader.tick as i32 - tick).rem_euclid(*len as i32)).rem_euclid(PHRASE_LEN as i32) as usize;
                while tick_offset < *len as i32 && states[s_index(tick_offset)].event.is_none() {
                    tick_offset += 1;
                }
                let mut s = states[s_index(tick_offset)].clone();
                match s.event.as_mut() {
                    Some(OnsetEvent::Hold { tick, .. }) => *tick += tick_offset,
                    Some(OnsetEvent::Loop { tick, len, .. }) => *tick = (*tick + tick_offset).rem_euclid(*len as i32),
                    _ => (),
                }
                state.event = s.event.as_ref().map(|e| e.open(self.kits[self.kit_index as usize].as_ref()));
            } else if matches!(self.input_state.event, OnsetEvent::Sync) {
                // current phrase event otherwise
                state.event = s.event.as_ref().map(|e| e.open(self.kits[self.kit_index as usize].as_ref()));
            };
            state.reverse = s.reverse;
            state.speed = s.speed;
        } else if self.buffer.onset[0].is_none() && matches!(self.input_state.event, OnsetEvent::Sync) {
            // FIXME: wait, why is this here?
            self.buffer.onset[0] = Some(OnsetInput::Sync);
        }
        if self.pass_input {
            if let Some(event) = self.buffer.onset[0].clone() && !(dont_sync && matches!(event, OnsetInput::Sync)) {
                state.event = Some(self.active_state.event.trans(event).open(self.kits[self.kit_index as usize].as_ref()));
            }
            if let Some(reverse) = self.buffer.reverse {
                state.reverse = Some(state.reverse.unwrap_or_default() ^ reverse);
            }
        } else {
            state.reverse = Some(state.reverse.unwrap_or_default() ^ self.input_state.reverse);
        }
        if let Some(speed) = self.buffer.speed {
            state.speed = Some(state.speed.unwrap_or(1.) * speed);
        }
        state
    }

    /// returned `ModState` should be sent to `SignalHandler`
    pub fn push_onset(&mut self, input: OnsetInput) -> ModState<OnsetEvent<Onset>> {
        if !matches!(input, OnsetInput::Loop { .. })
            && matches!(self.buffer.onset[0], Some(OnsetInput::Hold { .. }))
            && !matches!(self.input_state.event, OnsetEvent::Loop { .. })
        {
            self.buffer.onset[1] = Some(input.clone());
        } else {
            self.buffer.onset[0] = Some(input.clone());
        }
        self.cumulative_mod()
    }

    /// returned `ModState` should be sent to `SignalHandler`
    pub fn push_state(&mut self, input: StateInput) -> ModState<OnsetEvent<Onset>> {
        match input {
            StateInput::Reverse(v) => self.buffer.reverse = Some(v),
        }
        self.cumulative_mod()
    }

    /// returned `ModState` should be sent to `SignalHandler`
    pub fn push_phrase(&mut self, input: PhraseInput) -> ModState<OnsetEvent<Onset>> {
        match input {
            PhraseInput::Push { index } | PhraseInput::Pop { index } => self.buffer.phrase.retain(|e| match e {
                PhraseInput::Push { index: i } | PhraseInput::Pop { index: i } => *i != index,
            }),
        }
        self.buffer.phrase.push(input);
        self.cumulative_mod()
    }

    /// returned `ModState` should be sent to `SignalHandler`
    pub fn push_record(&mut self, input: RecordInput) -> ModState<OnsetEvent<Onset>> {
        self.buffer.record = Some(input);
        self.cumulative_mod()
    }

    /// returned `ModState` should be sent to `SignalHandler`
    pub fn push_speed_ramp(&mut self, start: Option<f32>, delta: Option<f32>) -> ModState<OnsetEvent<Onset>> {
        if let Some(delta) = delta {
            self.ramp.delta = delta;
        } else if let Some(start) = start {
            self.ramp.speed = start;
        }
        self.cumulative_mod()
    }

    pub fn set_record_mask(&mut self, mask: &[u8]) {
        let mut mask = heapless::Vec::from_slice(mask).unwrap();
        mask.sort();
        self.phrase_writer.store_mask = mask;
        self.phrase_writer.store_index = 0;
    }

    pub fn get_record_mask(&self) -> &[u8] {
        &self.phrase_writer.store_mask
    }

    fn top_reader(&self) -> Option<&PhraseReader> {
        self.phrase_stack.iter().rev().find(|r| self.phrases[r.index as usize].is_some())
    }

    /// for display purposes in external impl
    pub fn get_reader_tick(&self, index: u8) -> Option<u32> {
        self.phrase_stack.iter().rev().find(|r| r.index == index).map(|r| r.tick)
    }

    fn flush_speed(&mut self) {
        self.buffer.speed = Some(self.ramp.speed);
    }

    fn flush_phrase(&mut self) {
        for event in self.buffer.phrase.drain(..).rev() {
            match event {
                PhraseInput::Push { index } => if self.phrases[index as usize].is_some() {
                    self.phrase_stack.retain(|r| r.index != index);
                    let _ = self.phrase_stack.push(PhraseReader { index, tick: 0 });
                }
                PhraseInput::Pop { index } => self.phrase_stack.retain(|r| r.index != index),
            }
        }
    }

    fn flush_record(&mut self) {
        if let Some(event) = self.buffer.record.take() {
            match event {
                RecordInput::Start => self.phrase_writer.try_start(),
                RecordInput::Store => {
                    if let Some(phrase) = self.phrase_writer.try_store(self.ticks_per_input) && let Some(store_to) = self.phrase_writer.try_advance() {
                        self.phrases[store_to as usize] = Some(phrase);
                        let delta_start = if matches!(self.phrase_writer.snap_start, Snap::Onset) {
                            1
                        } else {
                            0
                        };
                        let delta_len = if matches!(self.phrase_writer.snap_len, Snap::Onset) {
                            -1
                        } else {
                            0
                        };
                        self.mod_phrase_start(store_to, delta_start, self.phrase_writer.snap_start);
                        self.mod_phrase_len(store_to, delta_len, self.phrase_writer.snap_len);
                        let _ = self.push_phrase(PhraseInput::Push { index: store_to });
                        self.buffer.speed = None;
                    }
                },
            }
        }
    }

    /// call from `SignalHandler` every tick
    /// pass_input should be true at tick.rem_euclid(ticks_per_input - 1) == 0
    /// returned `ModState` should be sent to `SignalHandler`
    pub fn tick(&mut self, tick: i32, active_state: State<()>, active_mod: ModState<OnsetEvent<()>>) -> ModState<OnsetEvent<Onset>> {
        self.active_state = active_state;
        // advance ramp
        if match self.ramp.snap {
            Snap::Tick => true,
            Snap::Beat => (tick + 1).rem_euclid(self.ticks_per_beat as i32) == 0,
            Snap::Input => (tick + 1).rem_euclid(self.ticks_per_input as i32) == 0,
            Snap::Onset => active_mod.event.as_ref().is_some_and(|e| matches!(e, OnsetEvent::Sync)),
        } {
            self.ramp.advance_speed();
        }
        // advance other
        let pass_input = (tick + 1).rem_euclid(self.ticks_per_input as i32) == 0;
        if pass_input {
            // rising edge; before input tick
            self.flush_record();
            self.flush_phrase();
        } else if self.pass_input {
            // falling edge; after input tick
            // FIXME: is it an issue that this buffer.onset eevnt may not be that received by SignalHandler?
            if let Some(event) = self.buffer.onset[0].take() {
                self.input_state.event = self.active_state.event.trans(event);
            }
            if let Some(reverse) = self.buffer.reverse.take() { self.input_state.reverse = reverse }
            if let Some(speed) = self.buffer.speed.take() { self.input_state.speed = speed }
            self.ramp.speed = 1.;
        }
        self.pass_input = pass_input;
        self.phrase_writer.push(active_mod);
        if let WriterState::Recording { len, .. } = &mut self.phrase_writer.state { *len += 1 }
        let tick_delta = self.tick_delta();
        for reader in self.phrase_stack.iter_mut() {
            if let Some(Phrase { len, .. }) = &self.phrases[reader.index as usize] {
                reader.tick = (reader.tick as i32 + tick_delta).rem_euclid(*len as i32) as u32;
            }
        }
        self.cumulative_mod()
    }

    /// to prevent `PhraseReader`s from reversing their own playback (and how would that interact with
    /// `Record`?), `tick_delta` is dependent only on direct user input
    fn tick_delta(&self) -> i32 {
        if self.input_state.reverse {
            -1
        } else {
            1
        }
    }

    /// quantized to input only
    /// should be called before `StateHandler::tick`
    pub fn set_active_phrase_start(&mut self, input: u32) {
        if let Some(reader) = self.top_reader() {
            let Phrase { start, .. } = self.phrases[reader.index as usize].as_mut().unwrap();
            *start = input * self.ticks_per_input;
        }
    }

    pub fn set_record_start_snap(&mut self, snap: Snap) {
        self.phrase_writer.snap_start = snap;
    }

    pub fn set_record_len_snap(&mut self, snap: Snap) {
        self.phrase_writer.snap_len = snap;
    }

    pub fn set_ramp_snap(&mut self, snap: Snap) {
        self.ramp.snap = snap;
    }

    /// quantized to input only
    /// should be called before `StateHandler::tick`
    pub fn set_active_phrase_len(&mut self, input: u32) {
        if let Some(reader) = self.top_reader() {
            let Phrase { len, .. } = self.phrases[reader.index as usize].as_mut().unwrap();
            *len = input * self.ticks_per_input;
        }
    }

    fn mod_phrase_start(&mut self, index: u8, delta: i32, snap: Snap) {
        let Phrase { states, start, .. } = self.phrases[index as usize].as_mut().unwrap();
        match snap {
            Snap::Tick => *start = (*start as i32 + delta).rem_euclid(PHRASE_LEN as i32) as u32,
            Snap::Beat => *start = ((*start as i32 + delta * self.ticks_per_beat as i32) * self.ticks_per_beat as i32 / self.ticks_per_beat as i32).rem_euclid(PHRASE_LEN as i32) as u32,
            Snap::Input => *start = ((*start as i32 + delta * self.ticks_per_input as i32) * self.ticks_per_input as i32 / self.ticks_per_input as i32).rem_euclid(PHRASE_LEN as i32) as u32,
            Snap::Onset => {
                if delta == 0 { return; }
                if delta.is_negative() && let Some((delta, _)) = states
                    .iter()
                    .rev()
                    .cycle()
                    .skip((-(*start as i32) + 1).rem_euclid(PHRASE_LEN as i32) as usize)
                    .enumerate()
                    .filter(|(_, s)| s.event.as_ref().is_some_and(|s| !matches!(s, OnsetEvent::Sync)))
                    .nth(delta.unsigned_abs() as usize - 1)
                {
                    *start = (*start as i32 - delta as i32 - 2).rem_euclid(PHRASE_LEN as i32) as u32;
                } else if let Some((delta, _)) = states
                    .iter()
                    .cycle()
                    .skip(*start as usize + 1)
                    .enumerate()
                    .filter(|(_, s)| s.event.as_ref().is_some_and(|e| !matches!(e, OnsetEvent::Sync)))
                    .nth(delta as usize - 1)
                {
                    *start = (*start + delta as u32 + 1).rem_euclid(PHRASE_LEN as u32);
                }
            }
        };
    }

    /// should be called before `StateHandler::tick`
    pub fn mod_active_phrase_start(&mut self, delta: i32, snap: Snap) {
        if let Some(reader) = self.top_reader() {
            self.mod_phrase_start(reader.index, delta, snap);
        }
    }

    fn mod_phrase_len(&mut self, index: u8, delta: i32, snap: Snap) {
        let Phrase { states, start, len } = self.phrases[index as usize].as_mut().unwrap();
        match snap {
            Snap::Tick => {
                let new = *len as i32 + delta;
                if new > 0 { *len = new as u32 }
            }
            Snap::Beat => {
                let new = (*len as i32 + delta * self.ticks_per_beat as i32) * self.ticks_per_beat as i32 / self.ticks_per_beat as i32;
                if new > 0 { *len = new as u32 }
            }
            Snap::Input => {
                let new = (*len as i32 + delta * self.ticks_per_input as i32) * self.ticks_per_input as i32 / self.ticks_per_input as i32;
                if new > 0 { *len = new as u32 }
            }
            Snap::Onset => {
                if delta == 0 { return; }
                if delta.is_negative() && let Some((delta, _)) = states
                    .iter()
                    .rev()
                    .cycle()
                    .skip((-((*start + *len) as i32) + 1).rem_euclid(PHRASE_LEN as i32) as usize)
                    .enumerate()
                    .filter(|(_, s)| s.event.as_ref().is_some_and(|s| !matches!(s, OnsetEvent::Sync)))
                    .nth(delta.unsigned_abs() as usize - 1)
                {
                    let new = *len as i32 - delta as i32 - 2;
                    if new > 0 { *len = new as u32 }
                } else if let Some((delta, _)) = states
                    .iter()
                    .cycle()
                    .skip((*start + *len) as usize + 1)
                    .enumerate()
                    .filter(|(_, s)| s.event.as_ref().is_some_and(|e| !matches!(e, OnsetEvent::Sync)))
                    .nth(delta as usize - 1)
                {
                    *len = *len + delta as u32 + 1;
                }
            }
        }
    }

    /// should be called before `StateHandler::tick`
    pub fn mod_active_phrase_len(&mut self, delta: i32, snap: Snap) {
        if let Some(reader) = self.top_reader() {
            self.mod_phrase_len(reader.index, delta, snap);
        }
    }
}

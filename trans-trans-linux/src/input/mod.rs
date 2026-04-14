use crate::audio;
use color_eyre::{Report, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use futures::{FutureExt, StreamExt};

mod tui;

macro_rules! press {
    (+$i:ident) => {
        crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::Modifier(crossterm::event::ModifierKeyCode::$i),
            kind: crossterm::event::KeyEventKind::Press,
            ..
        }
    };
    (-$i:ident) => {
        crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::$i,
            kind: crossterm::event::KeyEventKind::Press,
            ..
        }
    };
    ($c:pat) => {
        crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::Char($c),
            kind: crossterm::event::KeyEventKind::Press,
            ..
        }
    };
}

macro_rules! repeat {
    (+$i:ident) => {
        crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::Modifier(crossterm::event::ModifierKeyCode::$i),
            kind: crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat,
            ..
        }
    };
    (-$i:ident) => {
        crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::$i,
            kind: crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat,
            ..
        }
    };
    ($c:pat) => {
        crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::Char($c),
            kind: crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat,
            ..
        }
    };
}

macro_rules! release {
    (+$i:ident) => {
        crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::Modifier(crossterm::event::ModifierKeyCode::$i),
            kind: crossterm::event::KeyEventKind::Release,
            ..
        }
    };
    (-$i:ident) => {
        crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::$i,
            kind: crossterm::event::KeyEventKind::Release,
            ..
        }
    };
    ($c:pat) => {
        crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::Char($c),
            kind: crossterm::event::KeyEventKind::Release,
            ..
        }
    };
}

const INIT_PHRASE_SNAP_START: ttcore::Snap = ttcore::Snap::Macro;
const INIT_PHRASE_SNAP_LEN: ttcore::Snap = ttcore::Snap::Macro;
const INIT_PITCH_INTERVAL: f32 = 1.0594631;
const INIT_RAMP_SNAP: ttcore::Snap = ttcore::Snap::Macro;
pub const INIT_TEMPO: f32 = 192.;
pub const INIT_TICKS_PER_BEAT: u32 = 16;
pub const INIT_TICKS_PER_INPUT: u32 = 4;
pub const INIT_TICKS_PER_STEP: u32 = 16;
pub const INIT_STEPS_PER_MEAS: u32 = 7;

const BANK_COUNT: usize = 2;
pub const LAYER_COUNT: usize = 2;
const KIT_COUNT: usize = 4;
const KIT_LEN: usize = 8;
const PHRASE_COUNT: usize = 8;
const PHRASE_LEN: usize = 1024;

pub enum Cmd {
    Tick(ttcore::signal::Message<LAYER_COUNT>),
    Midi(midly::MidiMessage),
}

#[derive(Copy, Clone)]
struct Timeout {
    start: std::time::Instant,
    secs: f64,
    overflow: f64,
}

impl Timeout {
    fn secs(secs: f64) -> Self {
        Self {
            start: std::time::Instant::now(),
            secs,
            overflow: 0.,
        }
    }

    fn rem(&self) -> f64 {
        self.secs - self.overflow - self.start.elapsed().as_secs_f64()
    }
}

trait MaybeTimeout {
    fn timeout(&self) -> Option<Timeout>;
}

trait Dialog {
    type Return;

    fn handle_events(&mut self, event: crossterm::event::KeyEvent)
    -> std::io::Result<Self::Return>;
}

struct Scroll<T: ratatui::text::ToLine> {
    list: Vec<T>,
    index: usize,
}

impl<T: std::fmt::Display> Scroll<T> {
    fn dec(&mut self) {
        self.index = self.index.saturating_sub(1);
    }

    fn inc(&mut self) {
        self.index = usize::min(self.index + 1, self.list.len().saturating_sub(1));
    }
}

impl<T: std::fmt::Display> Default for Scroll<T> {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            index: 0,
        }
    }
}

#[derive(Default)]
enum Tab {
    #[default]
    Hosts,
    Devices,
    Farewell(Timeout),
}

struct CpalConfigDialog {
    open: bool,
    tab: Tab,
    hosts: Scroll<cpal::HostId>,
    last_hosts_index: usize,
    devices: Scroll<cpal::DeviceId>,
}

impl CpalConfigDialog {
    fn new() -> Self {
        let mut this = Self {
            open: true,
            tab: Tab::default(),
            hosts: Scroll::default(),
            last_hosts_index: 0,
            devices: Scroll::default(),
        };
        this.tab_to_hosts();
        this
    }

    /// commit change and exit dialog
    fn commit(&mut self, stream: &mut Option<cpal::Stream>) -> Result<()> {
        self.open = false;
        self.tab_to_devices();
        if let Ok(Ok(host)) = self
            .hosts
            .list
            .get(self.hosts.index)
            .ok_or(Report::msg("no host found"))
            .map(|v| cpal::host_from_id(*v))
        && let Some(device) = self
            .devices
            .list
            .get(self.devices.index)
            .and_then(|v| host.device_by_id(v))
        {
            *stream = None; // drop current stream; free AUDIO_HANDLER reference before calling `init_stream`
            let new_stream = audio::init_stream(&device)?;
            new_stream.play()?;
            *stream = Some(new_stream);
        }
        Ok(())
    }

    fn tab_to_hosts(&mut self) {
        self.hosts.list = cpal::available_hosts();
        self.hosts.index = 0;
        self.tab = Tab::Hosts;
    }

    fn tab_to_devices(&mut self) {
        if self.devices.list.is_empty() || self.last_hosts_index != self.hosts.index {
            self.last_hosts_index = self.hosts.index;
            if let Ok(host) = cpal::host_from_id(self.hosts.list[self.hosts.index]) {
                self.devices.list = host
                    .output_devices()
                    .into_iter()
                    .flatten()
                    .flat_map(|v| v.id())
                    .collect::<Vec<_>>();
                self.devices.index = 0;
            }
        }
        self.tab = Tab::Devices;
    }

    fn tab_to_farewell(&mut self) {
        self.tab = Tab::Farewell(Timeout::secs(0.5));
    }
}

impl MaybeTimeout for CpalConfigDialog {
    fn timeout(&self) -> Option<Timeout> {
        if let Tab::Farewell(timeout) = self.tab {
            Some(timeout)
        } else {
            None
        }
    }
}

enum CpalConfigDialogReturn {
    Continue,
    Consumed,
    Escape,
}

impl Dialog for CpalConfigDialog {
    type Return = CpalConfigDialogReturn;

    fn handle_events(
        &mut self,
        event: crossterm::event::KeyEvent,
    ) -> std::io::Result<Self::Return> {
        if let press!(-Esc) = event {
            return Ok(Self::Return::Escape);
        }
        match self.tab {
            Tab::Hosts => match event {
                repeat!(-Up) => self.hosts.dec(),
                repeat!(-Down) => self.hosts.inc(),
                repeat!(-Right) => self.tab_to_devices(),
                _ => return Ok(Self::Return::Continue),
            },
            Tab::Devices => match event {
                repeat!(-Up) => self.devices.dec(),
                repeat!(-Down) => self.devices.inc(),
                repeat!(-Left) => self.tab_to_hosts(),
                repeat!(-Right) => self.tab_to_farewell(),
                _ => return Ok(Self::Return::Continue),
            },
            Tab::Farewell(..) => match event {
                repeat!(-Left) => self.tab_to_devices(),
                _ => return Ok(Self::Return::Continue),
            },
        }
        Ok(Self::Return::Consumed)
    }
}

#[derive(Default)]
struct Throbber {
    step: i32,
    high: bool,
}

#[derive(Default)]
struct NumBuffer {
    num: u32,
    used: bool,
}

impl NumBuffer {
    fn get_num(&mut self) -> u32 {
        self.used = true;
        self.num
    }

    fn push_num(&mut self, digit: u32) {
        if self.used {
            self.num = digit;
            self.used = false;
        } else {
            self.num = self.num * 10 + digit;
        }
    }

    fn pop_num(&mut self) {
        self.num = 0;
        self.used = false;
    }
}

#[derive(Default)]
struct Ramp {
    value: u8,
    delta: u8,
}

pub struct App {
    quit: bool,

    dialog: CpalConfigDialog,
    throbber: Throbber,

    audio_stream: Option<cpal::Stream>,
    audio_tx: std::sync::mpsc::Sender<audio::Cmd>,

    legato: bool,
    sustain: bool,

    onset_downs: Vec<u8>,
    gain_ramp: Ramp,
    pitch_ramp: Ramp,

    num_lock: bool,
    num_buffer: NumBuffer,
    record_mask: Option<Vec<u8>>,
    state_handler: ttcore::state::StateHandler<BANK_COUNT, LAYER_COUNT, KIT_COUNT, KIT_LEN, PHRASE_COUNT, PHRASE_LEN>,
}

impl App {
    pub fn new(
        audio_tx: std::sync::mpsc::Sender<audio::Cmd>,
    ) -> Self {
        let bytes0 = std::fs::read("kits/Small Faces - Rollin Over.ttk").unwrap();
        let bytes1 = std::fs::read("kits/face a.ttk").unwrap();
        let bytes2 = std::fs::read("kits/Dennis Coffey - Scorpio (cd).ttk").unwrap();
        let bytes3 = std::fs::read("kits/James Brown - Cold Sweat (ver.2).ttk").unwrap();
        let mut state_handler = ttcore::state::StateHandler::new(
            INIT_PHRASE_SNAP_START,
            INIT_PHRASE_SNAP_LEN,
            INIT_PITCH_INTERVAL,
            INIT_RAMP_SNAP,
            INIT_TICKS_PER_INPUT,
            INIT_TICKS_PER_BEAT,
            INIT_TICKS_PER_STEP,
            INIT_STEPS_PER_MEAS,
        );
        // FIXME: remove dev defaults
        state_handler.kits = [
            Some(serde_json::from_slice::<ttcore::state::Kit<KIT_LEN>>(&bytes0).unwrap()),
            Some(serde_json::from_slice::<ttcore::state::Kit<KIT_LEN>>(&bytes1).unwrap()),
            Some(serde_json::from_slice::<ttcore::state::Kit<KIT_LEN>>(&bytes2).unwrap()),
            Some(serde_json::from_slice::<ttcore::state::Kit<KIT_LEN>>(&bytes3).unwrap()),
        ];
        Self {
            quit: false,

            dialog: CpalConfigDialog::new(),
            throbber: Throbber::default(),

            audio_stream: None,
            audio_tx,

            legato: false,
            sustain: false,

            onset_downs: Vec::new(),
            gain_ramp: Ramp::default(),
            pitch_ramp: Ramp::default(),

            num_lock: false,
            num_buffer: NumBuffer::default(),
            record_mask: None,
            state_handler,
        }
    }

    fn binary_offset(downs: &[u8], index: u8, count: u8) -> u32 {
        downs
            .iter()
            .skip(1)
            .map(|v| v.checked_sub(index + 1).unwrap_or(v + count - 1 - index))
            .fold(0u32, |acc, v| acc | (1 << v))
    }

    pub fn run(mut self, mut terminal: ratatui::DefaultTerminal, mut input_rx: futures::channel::mpsc::UnboundedReceiver<Cmd>, mut midi_out: midir::MidiOutputConnection) -> Result<()> {
        let mut event_stream = crossterm::event::EventStream::new();
        while !self.quit {
            terminal.draw(|frame| {
                frame.render_widget(&self, frame.area());
            })?;
            self.handle_events(&mut event_stream, &mut input_rx, &mut midi_out)?;
        }
        Ok(())
    }

    fn send(&mut self, msg: ttcore::state::Message<LAYER_COUNT>) -> Result<()> {
        self.audio_tx.send(audio::Cmd::TTMessage(msg))?;
        Ok(())
    }

    fn push_onset(&mut self, index: u8, down: bool) -> Result<()> {
        if down { self.onset_downs.push(index) } else { self.onset_downs.retain(|v| *v != index) }
        if down || !self.legato {
            let msg = if let Some(&index) = self.onset_downs.first() {
                if self.onset_downs.len() > 1 {
                    // push loop start
                    let len = Self::binary_offset(&self.onset_downs, index, LAYER_COUNT as u8);
                    self.state_handler.push_onset(ttcore::OnsetInput::Loop { index, len })
                } else {
                    // push loop stop | jump
                    self.state_handler.push_onset(ttcore::OnsetInput::Hold { index })
                }
            } else {
                // push sync
                self.state_handler.push_onset(ttcore::OnsetInput::Stop)
            };
            self.send(msg)?;
        }
        Ok(())
    }

    fn push_phrase(&mut self, index: u8, down: bool) -> Result<()> {
        if let Some(mask) = self.record_mask.as_mut() {
            if down {
                mask.retain(|i| *i != index);
                mask.push(index);
            }
        } else if down || !self.sustain {
            let input = if down {
                ttcore::PhraseInput::Hold { index }
            } else {
                ttcore::PhraseInput::Stop
            };
            let msg = self.state_handler.push_phrase(input);
            self.send(msg)?;
        }
        Ok(())
    }

    fn push_record(&mut self, down: bool) -> Result<()> {
        let input = if down {
            ttcore::RecordInput::Start
        } else {
            self.set_legato(false)?;
            ttcore::RecordInput::Stop
        };
        let msg = self.state_handler.push_record(input);
        self.send(msg)
    }

    fn set_gain(&mut self, index: u8, down: bool) -> Result<()> {
        if down {
            self.gain_ramp.value |= 1 << index;
            let gain = if self.gain_ramp.value & 1 == 1 {
                -((self.gain_ramp.value >> 1) as i8 & 7)
            } else {
                (self.gain_ramp.value >> 1) as i8 & 7
            } as f32 / 7.;
            let msg = self.state_handler.set_gain(gain);
            self.send(msg)?;
        } else {
            self.gain_ramp.value &= !(1 << index)
        };
        Ok(())
    }

    fn ramp_gain(&mut self, index: u8, down: bool) -> Result<()> {
        if down { self.gain_ramp.delta |= 1 << index } else { self.gain_ramp.delta  &= !(1 << index) };
        let gain = if self.gain_ramp.delta & 1 == 1 {
            -((self.gain_ramp.delta >> 1) as i8 & 7)
        } else {
            (self.gain_ramp.delta >> 1) as i8 & 7
        } as f32 / 7.;
        let msg = self.state_handler.ramp_gain(gain);
        self.send(msg)?;
        Ok(())
    }

    fn set_pitch(&mut self, index: u8, down: bool) -> Result<()> {
        if down {
            self.pitch_ramp.value |= 1 << index;
            let pitch = (if self.pitch_ramp.value & 1 == 1 {
                -((self.pitch_ramp.value >> 1) as i8 & 7)
            } else {
                (self.pitch_ramp.value >> 1) as i8 & 7
            } * 16) as f32 / 7.;
            let msg = self.state_handler.set_pitch(pitch);
            self.send(msg)?;
        } else {
            self.pitch_ramp.value &= !(1 << index)
        };
        Ok(())
    }

    fn ramp_pitch(&mut self, index: u8, down: bool) -> Result<()> {
        if down { self.pitch_ramp.delta |= 1 << index } else { self.pitch_ramp.delta  &= !(1 << index) };
        let pitch = (if self.pitch_ramp.delta & 1 == 1 {
            -((self.pitch_ramp.delta >> 1) as i8 & 7)
        } else {
            (self.pitch_ramp.delta >> 1) as i8 & 7
        } * 16) as f32 / 7.;
        let msg = self.state_handler.ramp_pitch(pitch);
        self.send(msg)?;
        Ok(())
    }

    fn push_kit(&mut self, bank: u8, index: u8) {
        self.state_handler.kit_indices[bank as usize] = index;
    }

    fn set_legato(&mut self, value: bool) -> Result<()> {
        self.legato = value;
        if !self.legato && self.onset_downs.is_empty() {
            let msg = self.state_handler.push_onset(ttcore::OnsetInput::Stop);
            self.send(msg)?;
        }
        Ok(())
    }

    fn key_event(&mut self, key: u8, down: bool) -> Result<()> {
        if self.num_lock {
            match 63 - key {
                4 => if down { self.num_lock = down },
                5 => if down { self.num_buffer.push_num(0) },
                6 => if down { self.num_buffer.pop_num() },
                7 => if down {
                    let msg = self.state_handler.set_steps_per_meas(self.num_buffer.get_num());
                    self.send(msg)?;
                }
                12 => if down { self.num_buffer.push_num(1) },
                13 => if down { self.num_buffer.push_num(2) },
                14 => if down { self.num_buffer.push_num(3) },
                15 => if down {
                    let msg = self.state_handler.set_ticks_per_step(self.num_buffer.get_num());
                    self.send(msg)?;
                }
                20 => if down { self.num_buffer.push_num(4) },
                21 => if down { self.num_buffer.push_num(5) },
                22 => if down { self.num_buffer.push_num(6) },
                23 => if down {
                    let msg = self.state_handler.set_ticks_per_input(self.num_buffer.get_num());
                    self.send(msg)?;
                }
                28 => if down { self.num_buffer.push_num(7) },
                29 => if down { self.num_buffer.push_num(8) },
                30 => if down { self.num_buffer.push_num(9) },
                31 => if down {
                    let msg = self.state_handler.set_ticks_per_beat(self.num_buffer.get_num());
                    self.send(msg)?;
                }
                36 => if down { self.state_handler.phrase_snap_start = ttcore::state::Snap::Micro },
                37 => if down { self.state_handler.phrase_snap_start = ttcore::state::Snap::Macro },
                38 => if down { self.state_handler.phrase_snap_len = ttcore::state::Snap::Micro },
                39 => if down { self.state_handler.phrase_snap_len = ttcore::state::Snap::Macro },
                44 => if down { self.state_handler.mod_phrase_start(-(self.num_buffer.get_num() as i32)) },
                45 => if down { self.state_handler.mod_phrase_start(self.num_buffer.get_num() as i32) },
                46 => if down { self.state_handler.mod_phrase_len(-(self.num_buffer.get_num() as i32)) },
                47 => if down { self.state_handler.mod_phrase_len(self.num_buffer.get_num() as i32) },
                _ => (),
            }
        } else {
            match 63 - key {
                4 => self.num_lock = down,
                10 => self.push_record(down)?,
                11 => self.sustain = down,
                12 => if down { self.set_legato(!self.legato)? },
                13 => if down { self.state_handler.layer = 1 } else { self.state_handler.layer = 0 },
                14 => if down { self.state_handler.ramp_snap = ttcore::state::Snap::Micro },
                15 => if down { self.state_handler.ramp_snap = ttcore::state::Snap::Macro },
                k if (24..32).contains(&k) => self.push_phrase(k-24, down)?,
                k if (32..40).contains(&k) => self.push_onset(k-32,down)?,
                k if (40..44).contains(&k) => self.set_pitch(k-40,down)?,
                k if (44..48).contains(&k) => self.ramp_pitch(3-(k-44),down)?,
                k if (48..52).contains(&k) => self.set_gain(k-48,down)?,
                k if (52..56).contains(&k) => self.ramp_gain(3-(k-52),down)?,
                k if (56..60).contains(&k) => if down { self.push_kit(0,k-56) },
                k if (60..64).contains(&k) => if down { self.push_kit(1,k-60) },
                _ => (),
            }
        }
        Ok(())
    }

    fn handle_events(&mut self, event_stream: &mut crossterm::event::EventStream, input_rx: &mut futures::channel::mpsc::UnboundedReceiver<Cmd>, midi_out: &mut midir::MidiOutputConnection) -> Result<()> {
        let maybe_timeout = match [self.dialog.timeout()]
            .into_iter()
            .enumerate()
            .filter_map(|(i, t)| Some((i, t?)))
            .min_by(|(_, t0), (_, t1)| t0.rem().partial_cmp(&t1.rem()).unwrap())
        {
            Some((index, timeout)) => Some((index, futures_timer::Delay::new(std::time::Duration::from_secs_f64(timeout.rem().max(0.))).fuse())),
            _ => None,
        };
        let mut keyboard_fut = event_stream.next().fuse();
        let mut cmd_fut = input_rx.recv().fuse();
        let mut event_fut = std::pin::pin!(async { futures::select_biased! {
            event = keyboard_fut => {
                if let crossterm::event::Event::Key(event) = event.unwrap()? {
                    if self.dialog.open {
                        match self.dialog.handle_events(event)? {
                            CpalConfigDialogReturn::Continue => (),
                            CpalConfigDialogReturn::Consumed => return Ok::<(), color_eyre::Report>(()),
                            CpalConfigDialogReturn::Escape => {
                                self.dialog.open = false;
                                return Ok(());
                            }
                        }
                    }
                    match event {
                        press!(-Esc) => self.quit = true,
                        press!('`') => self.dialog.open = true,
                        _ => (),
                    }
                }
                Ok(())
            }
            cmd = cmd_fut => {
                match cmd? {
                    Cmd::Tick(msg) => {
                        self.throbber.step = msg.tick / self.state_handler.get_ticks_per_step() as i32;
                        self.throbber.high = msg.tick.rem_euclid(self.state_handler.get_ticks_per_beat() as i32) < self.state_handler.get_ticks_per_beat() as i32 / 3;
                        let msg = self.state_handler.tick(msg);
                        self.audio_tx.send(audio::Cmd::TTMessage(msg))?;
                        // midi clock
                        let event = midly::live::SystemRealtime::TimingClock;
                        midi_out.send(&[event.encode()])?;
                    }
                    Cmd::Midi(msg) => match msg {
                        midly::MidiMessage::NoteOff { key, .. } => self.key_event(key.as_int(), false)?,
                        midly::MidiMessage::NoteOn { key, .. } => self.key_event(key.as_int(), true)?,
                        _ => (),
                    }
                }
                Ok(())
            }
        }}.fuse());
        if let Some((index, mut timeout_fut)) = maybe_timeout {
            let maybe_fut = async { futures::select_biased! {
                v = event_fut => v.map(|_| false),
                _ = timeout_fut => Ok(true),
            }};
            if futures::executor::block_on(maybe_fut)? {
                match index {
                    0 => self.dialog.commit(&mut self.audio_stream)?,
                    _ => unreachable!(),
                }
            }
        } else {
            futures::executor::block_on(event_fut)?;
        }
        Ok(())
    }
}

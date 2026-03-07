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

const SPEED_COUNT: usize = 4;
const KIT_COUNT: usize = 4;
pub const ONSET_COUNT: usize = 8;
const PHRASE_COUNT: usize = 8;
const PHRASE_LEN: usize = 1024;

pub enum Cmd {
    Tick {
        tick: i32,
        active_state: ttcore::state::State<()>,
        active_mod: ttcore::state::ModState<ttcore::state::OnsetEvent<()>>,
        ticks_per_beat: u32,
        ticks_per_input: u32,
    },
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
    high: bool,
}

#[derive(Default)]
struct NumBuffer {
    num: u32,
    used: bool,
    snap: ttcore::state::Snap,
}

impl NumBuffer {
    fn get_num(&mut self, min: u32) -> u32 {
        self.used = true;
        self.num.max(min)
    }

    fn push_num(&mut self, c: char) -> Result<()> {
        if self.used {
            self.num = c.to_string().parse::<u32>()?;
            self.used = false;
        } else {
            self.num = self.num * 10 + c.to_string().parse::<u32>()?;
        }
        Ok(())
    }

    fn pop_num(&mut self) {
        self.num = 0;
        self.used = false;
    }
}

#[derive(Default)]
struct Hold {
    onset: bool,
    phrase: bool,
}

pub struct App {
    quit: bool,

    dialog: CpalConfigDialog,
    throbber: Throbber,

    audio_stream: Option<cpal::Stream>,
    audio_tx: std::sync::mpsc::Sender<audio::Cmd>,

    hold: Hold,

    onset_downs: Vec<u8>,
    speed_starts: [f32; SPEED_COUNT],
    speed_downs: Vec<u8>,

    num_buffer: NumBuffer,
    record_mask: Option<Vec<u8>>,
    state_handler: ttcore::state::StateHandler<KIT_COUNT, PHRASE_COUNT, PHRASE_LEN>,
}

impl App {
    pub fn new(
        audio_tx: std::sync::mpsc::Sender<audio::Cmd>,
    ) -> Self {
        let bytes0 = std::fs::read("kits/Small Faces - Rollin Over.ttk").unwrap();
        let bytes1 = std::fs::read("kits/face a.ttk").unwrap();
        let bytes2 = std::fs::read("kits/Dennis Coffey - Scorpio (cd).ttk").unwrap();
        let bytes3 = std::fs::read("kits/James Brown - Cold Sweat (ver.2).ttk").unwrap();
        let mut state_handler = ttcore::state::StateHandler::new();
        // FIXME: remove dev defaults
        state_handler.set_record_start_snap(ttcore::state::Snap::Onset);
        state_handler.set_record_len_snap(ttcore::state::Snap::Beat);
        state_handler.kits = [
            Some(serde_json::from_slice::<ttcore::state::Kit>(&bytes0).unwrap()),
            Some(serde_json::from_slice::<ttcore::state::Kit>(&bytes1).unwrap()),
            Some(serde_json::from_slice::<ttcore::state::Kit>(&bytes2).unwrap()),
            Some(serde_json::from_slice::<ttcore::state::Kit>(&bytes3).unwrap()),
        ];
        state_handler.ticks_per_beat = 16;
        state_handler.ticks_per_input = 8;
        Self {
            quit: false,

            dialog: CpalConfigDialog::new(),
            throbber: Throbber::default(),

            audio_stream: None,
            audio_tx,

            hold: Hold::default(),

            onset_downs: Vec::new(),
            speed_starts: [0.5, 1.0, 1.5, 2.0], // FIXME: remove dev defaults
            speed_downs: Vec::new(),

            num_buffer: NumBuffer::default(),
            record_mask: None,
            state_handler,
        }
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

    fn binary_offset(&self, downs: &[u8], index: u8, count: u8) -> u32 {
        downs
            .iter()
            .skip(1)
            .map(|v| v.checked_sub(index + 1).unwrap_or(v + count - 1 - index))
            .fold(0u32, |acc, v| acc | (1 << v))
    }

    fn send(&mut self, state: ttcore::state::ModState<ttcore::state::OnsetEvent<ttcore::state::Onset>>) -> Result<()> {
        self.audio_tx.send(audio::Cmd::PushState(state))?;
        Ok(())
    }

    fn push_speed(&mut self, index: u8, down: bool) -> Result<()> {
        if down { self.speed_downs.push(index) } else { self.speed_downs.retain(|v| *v != index) }
        let state = if let Some(&index) = self.speed_downs.first() {
            let mult = if self.speed_downs.len() > 1 {
                let mut offset = self.binary_offset(&self.speed_downs, index, SPEED_COUNT as u8) as i32;
                if (offset >> SPEED_COUNT - 2) & 1 == 1 {
                    offset |= !((1 << SPEED_COUNT - 1) - 1);
                }
                let mult = offset as f32 / 2u32.pow(SPEED_COUNT as u32 - 1) as f32 + 1.;
                Some((mult + 4.)/5.)
            } else {
                None
            };
            self.state_handler.push_speed_ramp(
                Some(self.speed_starts[index as usize]),
                mult,
            )
        } else {
            self.state_handler.push_speed_ramp(None, Some(1.))
        };
        self.send(state)?;
        Ok(())
    }

    fn push_onset(&mut self, index: u8, down: bool) -> Result<()> {
        if down { self.onset_downs.push(index) } else { self.onset_downs.retain(|v| *v != index) }
        let state = if let Some(&index) = self.onset_downs.first() {
            if self.onset_downs.len() > 1 {
                // push loop start
                let len = self.binary_offset(&self.onset_downs, index, ONSET_COUNT as u8);
                self.state_handler.push_onset(ttcore::OnsetInput::Loop { index, len })
            } else {
                // push loop stop | jump
                self.state_handler.push_onset(ttcore::OnsetInput::Hold { index })
            }
        } else {
            if self.hold.onset {
                return Ok(());
            }
            // push sync
            self.state_handler.push_onset(ttcore::OnsetInput::Sync)
        };
        self.send(state)?;
        Ok(())
    }

    fn push_state(&mut self, input: ttcore::StateInput) -> Result<()> {
        let state = self.state_handler.push_state(input);
        self.send(state)?;
        Ok(())
    }

    fn push_phrase(&mut self, index: u8) -> Result<()> {
        if let Some(mask) = self.record_mask.as_mut() {
            mask.retain(|i| *i != index);
            mask.push(index);
        } else {
            let state = self.state_handler.push_phrase(ttcore::PhraseInput::Push { index });
            self.send(state)?;
        }
        Ok(())
    }

    fn pop_phrase(&mut self, index: u8) -> Result<()> {
        if self.record_mask.is_none() && !self.hold.phrase {
            let state = self.state_handler.push_phrase(ttcore::PhraseInput::Pop { index });
            self.send(state)?;
        }
        Ok(())
    }

    fn start_record(&mut self) -> Result<()> {
        let state = self.state_handler.push_record(ttcore::RecordInput::Start);
        self.send(state)?;
        Ok(())
    }

    fn store_record(&mut self) -> Result<()> {
        let state = self.state_handler.push_record(ttcore::RecordInput::Store);
        self.send(state)?;
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
                        // onset inputs
                        press!('c') => self.push_onset(0, true)?,
                        release!('c') => self.push_onset(0, false)?,
                        press!('r') => self.push_onset(1, true)?,
                        release!('r') => self.push_onset(1, false)?,
                        press!('s') => self.push_onset(2, true)?,
                        release!('s') => self.push_onset(2, false)?,
                        press!('t') => self.push_onset(3, true)?,
                        release!('t') => self.push_onset(3, false)?,
                        press!('n') => self.push_onset(4, true)?,
                        release!('n') => self.push_onset(4, false)?,
                        press!('e') => self.push_onset(5, true)?,
                        release!('e') => self.push_onset(5, false)?,
                        press!('i') => self.push_onset(6, true)?,
                        release!('i') => self.push_onset(6, false)?,
                        press!('a') => self.push_onset(7, true)?,
                        release!('a') => self.push_onset(7, false)?,
                        // phrase inputs
                        press!('q') => self.push_phrase(0)?,
                        release!('q') => self.pop_phrase(0)?,
                        press!('j') => self.push_phrase(1)?,
                        release!('j') => self.pop_phrase(1)?,
                        press!('v') => self.push_phrase(2)?,
                        release!('v') => self.pop_phrase(2)?,
                        press!('d') => self.push_phrase(3)?,
                        release!('d') => self.pop_phrase(3)?,
                        press!('h') => self.push_phrase(4)?,
                        release!('h') => self.pop_phrase(4)?,
                        press!('/') => self.push_phrase(5)?,
                        release!('/') => self.pop_phrase(5)?,
                        press!(',') => self.push_phrase(6)?,
                        release!(',') => self.pop_phrase(6)?,
                        press!('.') => self.push_phrase(7)?,
                        release!('.') => self.pop_phrase(7)?,
                        // speed inputs
                        press!('w') => self.push_speed(0, true)?,
                        release!('w') => self.push_speed(0, false)?,
                        press!('l') => self.push_speed(1, true)?,
                        release!('l') => self.push_speed(1, false)?,
                        press!('y') => self.push_speed(2, true)?,
                        release!('y') => self.push_speed(2, false)?,
                        press!('p') => self.push_speed(3, true)?,
                        release!('p') => self.push_speed(3, false)?,
                        // active phrase
                        repeat!(-Delete) => self.state_handler.mod_active_phrase_start(-((self.num_buffer.get_num(1)) as i32), self.num_buffer.snap),
                        press!(-End) => self.state_handler.set_active_phrase_start(self.num_buffer.get_num(1), self.num_buffer.snap),
                        repeat!(-PageDown) => self.state_handler.mod_active_phrase_start((self.num_buffer.get_num(1)) as i32, self.num_buffer.snap),
                        repeat!(-Insert) => self.state_handler.mod_active_phrase_len(-((self.num_buffer.get_num(1)) as i32), self.num_buffer.snap),
                        press!(-Home) => self.state_handler.set_active_phrase_len(self.num_buffer.get_num(1), self.num_buffer.snap),
                        repeat!(-PageUp) => self.state_handler.mod_active_phrase_len((self.num_buffer.get_num(1)) as i32, self.num_buffer.snap),
                        // record
                        press!(' ') => self.start_record()?,
                        release!(' ') => self.store_record()?,
                        press!(+RightShift) => self.record_mask = Some(Vec::new()),
                        release!(+RightShift) => if let Some(mask) = self.record_mask.take() {
                            self.state_handler.set_record_mask(&mask);
                        }
                        // kit
                        press!('k') => self.state_handler.kit_index = 0,
                        press!('x') => self.state_handler.kit_index = 1,
                        press!('g') => self.state_handler.kit_index = 2,
                        press!('m') => self.state_handler.kit_index = 3,
                        // num buffer
                        press!(c) if c.is_ascii_digit() => self.num_buffer.push_num(c)?,
                        press!(-Backspace) => self.num_buffer.pop_num(),
                        // snap
                        press!('f') => self.num_buffer.snap = ttcore::state::Snap::Tick,
                        press!('o') => self.num_buffer.snap = ttcore::state::Snap::Beat,
                        press!('u') => self.num_buffer.snap = ttcore::state::Snap::Input,
                        press!('\'') => self.num_buffer.snap = ttcore::state::Snap::Onset,
                        press!('-') => self.state_handler.set_ramp_snap(self.num_buffer.snap),
                        press!('=') => self.state_handler.set_record_start_snap(self.num_buffer.snap),
                        press!('\\') => self.state_handler.set_record_len_snap(self.num_buffer.snap),
                        press!('[') => self.audio_tx.send(audio::Cmd::TicksPerBeat(self.num_buffer.get_num(1)))?,
                        press!(']') => self.audio_tx.send(audio::Cmd::TicksPerInput(self.num_buffer.get_num(1)))?,
                        press!(-CapsLock) => self.audio_tx.send(audio::Cmd::Tempo(self.num_buffer.get_num(1) as f32))?,
                        // reverse
                        press!(+LeftShift) => self.push_state(ttcore::StateInput::Reverse(true))?,
                        release!(+LeftShift) => self.push_state(ttcore::StateInput::Reverse(false))?,
                        // hold
                        press!(-Enter) => self.hold.phrase = true,
                        release!(-Enter) => self.hold.phrase = false,
                        press!(';') => {
                            self.hold.onset = !self.hold.onset;
                            if !self.hold.onset && self.onset_downs.is_empty() {
                                let state = self.state_handler.push_onset(ttcore::OnsetInput::Sync);
                                self.send(state)?;
                            }
                        },
                        _ => (),
                    }
                }
                Ok(())
            }
            cmd = cmd_fut => {
                match cmd? {
                    Cmd::Tick { tick, active_state, active_mod, ticks_per_beat, ticks_per_input } => {
                        self.throbber.high = tick.rem_euclid(self.state_handler.ticks_per_beat as i32) < self.state_handler.ticks_per_beat as i32 / 4;
                        self.state_handler.ticks_per_beat = ticks_per_beat;
                        self.state_handler.ticks_per_input = ticks_per_input;
                        let state = self.state_handler.tick(tick, active_state, active_mod);
                        self.audio_tx.send(audio::Cmd::PushState(state))?;
                        // midi clock
                        let event = midly::live::SystemRealtime::TimingClock;
                        midi_out.send(&[event.encode()])?;
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

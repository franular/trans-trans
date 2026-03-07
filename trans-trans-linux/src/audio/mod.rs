use crate::input;
use cpal::traits::DeviceTrait;

pub const SAMPLE_RATE: u32 = 48000;
pub static mut AUDIO_HANDLER: Option<AudioHandler> = None;

pub enum Cmd {
    PushState(ttcore::state::ModState<ttcore::state::OnsetEvent<ttcore::state::Onset>>),
    TicksPerBeat(u32),
    TicksPerInput(u32),
    Tempo(f32),
}

pub struct AudioHandler {
    audio_rx: std::sync::mpsc::Receiver<Cmd>,
    input_tx: futures::channel::mpsc::UnboundedSender<crate::input::Cmd>,

    signal_handler: ttcore::signal::SignalHandler<crate::fs::LinuxFileHandler>,
    fs: crate::fs::LinuxFileHandler,
}

impl AudioHandler {
    pub fn new(
        audio_rx: std::sync::mpsc::Receiver<Cmd>,
        input_tx: futures::channel::mpsc::UnboundedSender<crate::input::Cmd>,
    ) -> Self {
        let mut signal_handler = ttcore::signal::SignalHandler::new();
        // FIXME: remove dev defaults
        signal_handler.ticks_per_beat = 16;
        signal_handler.ticks_per_input = 8;
        signal_handler.tempo = 192.;
        Self {
            audio_rx,
            input_tx,
            signal_handler,
            fs: crate::fs::LinuxFileHandler {},
        }
    }

    fn read(
        &mut self,
        data: &mut [f32],
        channels: usize,
    ) -> color_eyre::Result<()> {
        let mut slice = &mut data[..];
        while !slice.is_empty() {
            if let Some(n) = self.signal_handler.read::<{input::ONSET_COUNT}>(slice, channels, SAMPLE_RATE, &mut self.fs)? {
                let mut last_state = ttcore::state::ModState::default();
                // flush buffered commands
                for cmd in self.audio_rx.try_iter() {
                    match cmd {
                        Cmd::PushState(state) => last_state = state,
                        Cmd::TicksPerBeat(ticks_per_beat) => self.signal_handler.ticks_per_beat = ticks_per_beat,
                        Cmd::TicksPerInput(ticks_per_input) => self.signal_handler.ticks_per_input = ticks_per_input,
                        Cmd::Tempo(tempo) => self.signal_handler.tempo = tempo,
                    }
                }
                let active_mod = last_state.as_unit();
                let (tick, active_state) = self.signal_handler.tick::<{input::ONSET_COUNT}>(last_state, &mut self.fs)?;
                self.input_tx.start_send(input::Cmd::Tick {
                    tick,
                    active_state,
                    active_mod,
                    ticks_per_beat: self.signal_handler.ticks_per_beat,
                    ticks_per_input: self.signal_handler.ticks_per_input,
                })?;
                slice = &mut slice[n..];
            } else {
                break;
            }
        }
        Ok(())
    }
}

pub fn init_stream(device: &cpal::Device) -> color_eyre::Result<cpal::Stream> {
    let config = device
        .supported_output_configs()?
        .find(|v| v.channels() == 2 && v.sample_format() == cpal::SampleFormat::F32)
        .ok_or(color_eyre::Report::msg(
            "failed to init desired audio output",
        ))?;
    let config = config.with_sample_rate(SAMPLE_RATE);
    let channels = config.channels();
    let out_fn = move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        static mut LOCAL_AUDIO_HANDLER: Option<AudioHandler> = None;
        let audio_handler = unsafe {
            LOCAL_AUDIO_HANDLER.get_or_insert_with(|| {
                AUDIO_HANDLER
                    .take()
                    .expect("AUDIO_HANDLER must be initialized")
            })
        };
        audio_handler.read(data, channels as usize).expect("audio panic");
    };
    let err_fn = |_| {};
    Ok(device.build_output_stream(&config.into(), out_fn, err_fn, None)?)
}

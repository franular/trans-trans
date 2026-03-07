#![feature(generic_const_exprs)]
#![allow(clippy::precedence, clippy::new_without_default, clippy::too_many_arguments)]
#![allow(incomplete_features, static_mut_refs)]

use color_eyre::Result;

mod audio;
mod fs;
mod input;

fn main() -> Result<()> {
    color_eyre::install()?;

    let log_target = Box::new(std::fs::File::create("/tmp/t4t-linux.txt").unwrap());
    env_logger::builder()
        .target(env_logger::Target::Pipe(log_target))
        .filter(None, log::LevelFilter::Trace)
        .init();

    let (audio_tx, audio_rx) = std::sync::mpsc::channel();
    let (input_tx, input_rx) = futures::channel::mpsc::unbounded();

    unsafe { audio::AUDIO_HANDLER.replace(audio::AudioHandler::new(audio_rx, input_tx)) };
    let app = input::App::new(audio_tx);

    let terminal = ratatui::init();
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle("TRANSCRIBE/TRANSFORM"),
        crossterm::event::PushKeyboardEnhancementFlags(
            // necessary to ignore repeat events
            crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                // necessary to register modifier key events; compatible only with kitty's protocol
                | crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        ),
    )?;

    let app_result = app.run(terminal, input_rx);
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PopKeyboardEnhancementFlags,
    )?;
    ratatui::restore();
    app_result
}

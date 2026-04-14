#![feature(generic_const_exprs)]
#![allow(clippy::precedence, clippy::new_without_default, clippy::too_many_arguments)]
#![allow(incomplete_features, static_mut_refs)]

use std::io::Write;

use color_eyre::Result;
use futures::SinkExt;

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

    let midi_in = midir::MidiInput::new("trans-trans")?;
    // get an input port (read from console if multiple are available)
    let in_ports = midi_in.ports();
    let in_port: &midir::MidiInputPort = match in_ports.len() {
        0 => panic!("no input port found"),
        1 => {
            println!(
                "\nselected only available input port: {}",
                midi_in.port_name(&in_ports[0]).unwrap()
            );
            &in_ports[0]
        }
        _ => {
            println!("\navailable input ports:");
            for (i, p) in in_ports.iter().enumerate() {
                println!("{}: {}", i, midi_in.port_name(p).unwrap());
            }
            print!("select an input port: ");
            std::io::stdout().flush()?;
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            in_ports
                .get(input.trim().parse::<usize>()?)
                .expect("invalid input port selected")
        }
    };

    let midi_out = midir::MidiOutput::new("trans-trans")?;
    // get an output port (read from console if multiple are available)
    let out_ports = midi_out.ports();
    let out_port: &midir::MidiOutputPort = match out_ports.len() {
        0 => panic!("no output port found"),
        1 => {
            println!(
                "selected only available output port: {}",
                midi_out.port_name(&out_ports[0]).unwrap()
            );
            &out_ports[0]
        }
        _ => {
            println!("\navailable output ports:");
            for (i, p) in out_ports.iter().enumerate() {
                println!("{}: {}", i, midi_out.port_name(p).unwrap());
            }
            print!("select output port: ");
            std::io::stdout().flush()?;
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            out_ports
                .get(input.trim().parse::<usize>()?)
                .expect("invalid output port selected")
        }
    };
    let _midi_in = midi_in.connect(
        in_port,
        "trans-trans",
        move |_, message, input_tx: &mut futures::channel::mpsc::UnboundedSender::<input::Cmd>| {
            if let midly::live::LiveEvent::Midi { message, .. } = midly::live::LiveEvent::parse(message).unwrap() {
                futures::executor::block_on(input_tx.send(input::Cmd::Midi(message))).unwrap();
            }
        },
        input_tx.clone(),
    ).expect("failed to connect to midi input");
    let midi_out = midi_out.connect(out_port, "trans-trans").expect("failed to connect to midi output");

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

    let app_result = app.run(terminal, input_rx, midi_out);
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PopKeyboardEnhancementFlags,
    )?;
    ratatui::restore();
    app_result
}

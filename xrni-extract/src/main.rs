#![allow(clippy::uninlined_format_args)]

use std::str::FromStr;
use quick_xml::events::Event;

#[derive(PartialEq)]
enum State {
    None,
    Sample,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir("in")?.flatten() {
        if entry.path().extension().is_some_and(|v| v == std::ffi::OsString::from_str("xrni").unwrap()) {
            let mut zip = zip::ZipArchive::new(std::fs::File::open(entry.path())?)?;

            let mut sample_index = 0;
            let mut sample_name = String::new();
            let mut beat_sync_lines = 0;
            let mut sample_positions = Vec::new();

            if let Ok(xml) = zip.by_name("Instrument.xml") {
                let mut reader = quick_xml::Reader::from_reader(std::io::BufReader::new(xml));
                reader.config_mut().trim_text(true);
                let mut buf = Vec::new();

                let mut state = State::Sample;
                loop {
                    let event = reader.read_event_into(&mut buf)?;
                    match event {
                        Event::Start(elem) => match elem.name().as_ref() {
                            b"Sample" => {
                                sample_index += 1;
                                state = State::Sample
                            },
                            b"Name" => if state == State::Sample && let Event::Text(text) = reader.read_event_into(&mut buf)? {
                                sample_name = text.decode()?.to_string();
                            }
                            b"BeatSyncLines" => if let Event::Text(text) = reader.read_event_into(&mut buf)? {
                                beat_sync_lines = text.decode()?.parse::<u16>()?;
                            }
                            b"SamplePosition" => if let Event::Text(text) = reader.read_event_into(&mut buf)? {
                                sample_positions.push(text.decode()?.parse::<u64>()?);
                            }
                            _ => (),
                        }
                        Event::End(elem) => match elem.name().as_ref() {
                            b"Samples" => break, // stop parsing after all samples
                            b"Sample" => state = State::None,
                            b"Name" => state = State::Sample,
                            b"SliceMarkers" => break, // stop parsing after slices
                            b"SamplePosition" => state = State::Sample,
                            _ => (),
                        }
                        Event::Eof => break,
                        _ => (),
                    }
                }
            } else {
                panic!("no `Instrument.xml` found");
            }
            if sample_index == 0 {
                panic!("no `Sample`s found in `Instrument.xml`");
            }
            println!("Name: {}", sample_name);
            println!("BeatSyncLines: {}", beat_sync_lines);
            println!("SamplePositions:");
            for sp in &sample_positions {
                println!("  {:>8}", sp);
            }

            // extract .flac and convert to .wav in `onsets/breaks` directory
            let mut flac_name = String::new();
            for name in zip.file_names() {
                // of form `SampleData/SampleXX (<SAMPLE NAME>).flac` where XX is sample index (order in XML, i think)
                if name == format!("SampleData/Sample{:02} ({}).flac", sample_index - 1, sample_name) {
                    flac_name = name.to_string();
                }
            }
            let mut flac_reader = claxon::FlacReader::new(zip.by_name(&flac_name)?)?;
            assert!(flac_reader.streaminfo().bits_per_sample == 16);
            let wav_spec = hound::WavSpec {
                channels: 1,
                sample_rate: flac_reader.streaminfo().sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut wav_writer = hound::WavWriter::create(format!("onsets/breaks/{}.wav", sample_name), wav_spec)?;
            let mut frame_reader = flac_reader.blocks();
            let mut block = claxon::Block::empty();
            while let Some(next_block) = frame_reader.read_next_or_eof(block.into_buffer())? {
                block = next_block;
                let mut sample_writer = wav_writer.get_i16_writer(block.duration());
                for i in 0..block.len() / block.channels() {
                    unsafe {
                        sample_writer.write_sample_unchecked(block.sample(0, i) as i16);
                    }
                }
                sample_writer.flush()?;
            }
            wav_writer.finalize()?;
            // save onsets to kit
            let onsets = sample_positions.into_iter().map(|pos| {
                ttcore::state::Onset {
                    path: format!("onsets/breaks/{}.wav", sample_name),
                    start: pos as u32,
                    beat_count: beat_sync_lines as u32,
                }
            }).collect::<Vec<_>>().into_boxed_slice();
            let kit = ttcore::state::Kit { onsets };
            let ttk_file = std::fs::File::create(format!("kits/{}.ttk", sample_name))?;
            serde_json::to_writer_pretty(ttk_file, &kit)?;
        }
    }
    Ok(())
}

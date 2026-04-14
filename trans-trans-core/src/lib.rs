#![feature(generic_const_exprs)]
#![allow(clippy::needless_range_loop, clippy::new_without_default, clippy::too_many_arguments)]
#![allow(incomplete_features)]

pub mod fs;
pub mod signal;
pub mod state;

pub use state::{OnsetInput, PhraseInput, RecordInput, Snap, StateHandler};
pub use signal::SignalHandler;

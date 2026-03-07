#![feature(generic_const_exprs)]
#![allow(clippy::new_without_default, clippy::should_implement_trait, clippy::too_many_arguments)]
#![allow(incomplete_features)]

pub mod fs;
pub mod signal;
pub mod state;

pub use state::{OnsetInput, PhraseInput, RecordInput, StateInput};

//! Headers for the filter's `KEYWORD:value` terms, read in the background.

mod index;
mod scan;

pub use index::HeaderIndex;
pub use scan::{HeaderScanner, Progress};

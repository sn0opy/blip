//! Decoding, stretching, star detection, tilt and export, shared by the desktop app and the
//! WebAssembly build (`web/`).

#[cfg(feature = "solve")]
pub mod astrobin;
#[cfg(feature = "solve")]
pub mod catalog;
#[cfg(feature = "solve")]
pub mod dso;
pub mod export;
mod fits;
pub mod grid;
pub mod image;
pub mod overlay;
pub mod process;
#[cfg(feature = "solve")]
pub mod solve;
pub mod stars;
mod stats;
pub mod tilt;
#[cfg(feature = "solve")]
pub mod wcs;
mod xisf;

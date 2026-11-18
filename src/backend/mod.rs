//! Compositor backends.
//!
//! Only the nested (winit) backend exists so far: Mriya runs as a regular
//! client of the parent session compositor and shows its own window there.
//! A udev/drm backend for running on a TTY comes later.

pub mod element;
pub mod nested;

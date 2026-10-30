//! livenyan: reimplementación nativa del reproductor LiveMaker.
// Dibujo y VM pasan el contexto explícito; se agrupará en structs en el refactor de app.rs/vm.
#![allow(clippy::too_many_arguments)]
pub mod app;
pub mod archive;
pub mod assets;
pub mod audio;
pub mod config;
pub mod db;
pub mod gal;
pub mod game;
pub mod input;
pub mod interpolation;
pub mod lcm;
pub mod lft;
pub mod log_dedup;
pub mod lpm;
pub mod lsb;
pub mod rgba;
pub mod save;
pub mod scale_filter;
pub mod scene;
pub mod slider;
pub mod text;
pub mod text_export;
pub mod util;
pub mod video;
pub mod vm;

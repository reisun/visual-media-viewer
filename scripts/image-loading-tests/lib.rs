#![allow(dead_code)]

// Test the production modules without the Windows video/audio dependencies.
#[path = "../../src/cache.rs"]
mod cache;
#[path = "../../src/file_list.rs"]
mod file_list;
#[path = "../../src/image_decode.rs"]
mod image_decode;
#[path = "../../src/media_overlay.rs"]
mod media_overlay;
#[path = "../../src/shell_integration.rs"]
mod shell_integration;
#[path = "../../src/shell_ui.rs"]
mod shell_ui;
#[path = "../../src/startup.rs"]
mod startup;
#[path = "../../src/title_bar.rs"]
mod title_bar;
#[path = "../../src/wic_decoder.rs"]
mod wic_decoder;

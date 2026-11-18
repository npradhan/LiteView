//! Stand-in used when LiteView is built without the `video` feature.
//! No page is ever recognised as a video then, so none of this is reachable.

use std::path::Path;
use std::time::Duration;

use egui::{ColorImage, TextureHandle};

pub fn thumbnail(_path: &Path, _max_side: u32) -> Result<ColorImage, String> {
    Err("video support is not built in".into())
}

pub enum Player {}

#[allow(dead_code)]
impl Player {
    pub fn open(_path: &Path) -> Result<Player, String> {
        Err("video support is not built in".into())
    }
    pub fn texture(&self) -> Option<&TextureHandle> {
        match *self {}
    }
    pub fn rotation(&self) -> u8 {
        match *self {}
    }
    pub fn duration(&self) -> f64 {
        match *self {}
    }
    pub fn position(&self) -> f64 {
        match *self {}
    }
    pub fn is_paused(&self) -> bool {
        match *self {}
    }
    pub fn has_audio(&self) -> bool {
        match *self {}
    }
    pub fn is_muted(&self) -> bool {
        match *self {}
    }
    pub fn error(&self) -> Option<String> {
        match *self {}
    }
    pub fn toggle_mute(&mut self) {
        match *self {}
    }
    pub fn toggle_pause(&mut self) {
        match *self {}
    }
    pub fn seek(&mut self, _t: f64) {
        match *self {}
    }
    pub fn seek_relative(&mut self, _delta: f64) {
        match *self {}
    }
    pub fn update(&mut self, _ctx: &egui::Context) -> Duration {
        match *self {}
    }
}

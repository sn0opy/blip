//! The message in the bottom-left corner of the image view.

use super::draw;
use eframe::egui::{self, Pos2};
use std::time::Instant;

const FADE: f32 = 0.8;

#[derive(Clone, Copy, PartialEq)]
pub enum StatusKind {
    Message,
    FolderError,
    ReadingHeaders,
}

/// Transient ones fade out after `ttl` seconds.
struct Status {
    kind: StatusKind,
    text: String,
    since: Instant,
    ttl: Option<f32>,
}

#[derive(Default)]
pub struct StatusLine {
    status: Option<Status>,
}

impl StatusLine {
    pub fn set(&mut self, text: String, ttl: Option<f32>) {
        self.show(StatusKind::Message, text, ttl);
    }

    pub fn show(&mut self, kind: StatusKind, text: String, ttl: Option<f32>) {
        self.status = Some(Status {
            kind,
            text,
            since: Instant::now(),
            ttl,
        });
    }

    pub fn clear(&mut self, kind: StatusKind) {
        if self.status.as_ref().is_some_and(|s| s.kind == kind) {
            self.status = None;
        }
    }

    pub fn paint(&mut self, ctx: &egui::Context, painter: &egui::Painter, pos: Pos2) {
        let Some(status) = &self.status else { return };
        let age = status.since.elapsed().as_secs_f32();
        let alpha = match status.ttl {
            Some(ttl) if age >= ttl => 1.0 - ((age - ttl) / FADE).min(1.0),
            _ => 1.0,
        };
        if alpha <= 0.0 {
            self.status = None;
            return;
        }
        draw::text_box(painter, pos, &status.text, alpha);
        match status.ttl {
            Some(ttl) if age < ttl => ctx.request_repaint_after_secs(ttl - age),
            Some(_) => ctx.request_repaint(),
            None => {}
        }
    }
}

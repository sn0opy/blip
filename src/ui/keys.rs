use super::App;
use super::prefs::BinningPref;
use blip_core::process::{BOOST_STEP_PERCENT, MAX_BOOST};
use eframe::egui::{self, Key, Modifiers, Vec2};

impl App {
    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.confirm_delete.is_some() || self.show_help || self.show_settings {
            return;
        }
        let pressed = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        let pressed_cmd = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, key));

        if pressed(Key::Questionmark) || pressed(Key::F1) {
            self.show_help = true;
            return;
        }
        if pressed_cmd(Key::Comma) {
            self.show_settings = true;
            return;
        }
        if pressed(Key::ArrowDown)
            || pressed(Key::ArrowRight)
            || pressed(Key::Space)
            || pressed(Key::J)
        {
            self.step(1);
        }
        if pressed(Key::ArrowUp) || pressed(Key::ArrowLeft) || pressed(Key::K) {
            self.step(-1);
        }
        if pressed(Key::PageDown) {
            self.step(10);
        }
        if pressed(Key::PageUp) {
            self.step(-10);
        }
        if pressed(Key::Home) {
            self.select(0);
        }
        if pressed(Key::End) {
            self.select(usize::MAX);
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            self.confirm_delete = self.current().cloned();
        }
        if pressed(Key::F5) || pressed_cmd(Key::R) {
            self.refresh();
        }
        if pressed(Key::I) {
            self.show_info = !self.show_info;
        }
        if pressed(Key::D) {
            self.show_debug = !self.show_debug;
        }
        if pressed(Key::N) {
            self.params.neutralize = !self.params.neutralize;
            self.reload_all();
        }
        if pressed(Key::Plus) || pressed(Key::Equals) {
            self.change_boost(1);
        }
        if pressed(Key::Minus) {
            self.change_boost(-1);
        }
        if pressed(Key::A) {
            self.toggle_stretch();
        }
        if pressed(Key::C) {
            self.toggle_clip_flash();
        }
        if pressed(Key::S) {
            self.toggle_star_view();
        }
        if pressed(Key::T) {
            self.toggle_tilt_view();
        }
        if pressed(Key::P) {
            self.toggle_sky_view();
        }
        if pressed(Key::G) {
            self.toggle_mosaic();
        }
        if pressed(Key::B) {
            self.toggle_full_res();
        }
        if pressed(Key::Num0) || pressed(Key::F) {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }
        if pressed_cmd(Key::O) || pressed(Key::O) {
            self.pick_folder();
        }
        if pressed_cmd(Key::S) || pressed(Key::E) {
            self.export(ctx);
        }
    }

    fn change_boost(&mut self, delta: i32) {
        let boost = self.boost.saturating_add_signed(delta).min(MAX_BOOST);
        if boost != self.boost {
            self.boost = boost;
            self.reload_all();
        }
        let target = self.params.boosted(boost).target_bg * 100.0;
        let text = match boost {
            0 => format!("Stretch boost off: background {target:.0}% (+/- to change)"),
            _ => format!(
                "Stretch boost +{}%: background {target:.0}% (+/- to change)",
                boost * BOOST_STEP_PERCENT
            ),
        };
        self.set_status(text, Some(2.0));
    }

    fn toggle_full_res(&mut self) {
        if self.prefs.binning == BinningPref::Bin1 {
            self.set_status("Binning is set to 1x1 in the settings".into(), Some(2.5));
            return;
        }
        let was_full = self.loads_full_res();
        self.full_res = !self.full_res;
        let mode = if self.full_res {
            "Full resolution"
        } else {
            "Binned preview"
        };
        self.set_status(mode.into(), Some(2.0));
        if self.loads_full_res() != was_full {
            self.reload_all();
        }
    }

    fn toggle_clip_flash(&mut self) {
        let Some(cur) = self.current().cloned() else {
            return;
        };
        if self.flash_clip.toggle(&cur)
            && self
                .cache
                .get(&cur)
                .is_some_and(|e| e.loaded.clip_overlay.is_none())
        {
            self.set_status("No clipped pixels in this image".into(), Some(2.0));
        }
    }

    fn toggle_star_view(&mut self) {
        if let Some(cur) = self.current().cloned() {
            self.star_view.toggle(&cur);
        }
    }

    fn toggle_tilt_view(&mut self) {
        let Some(cur) = self.current().cloned() else {
            return;
        };
        if self.tilt_view.toggle(&cur) && self.stars.get(&cur).is_some_and(|s| s.tilt.is_none()) {
            self.set_status("Not enough stars to measure tilt".into(), Some(2.5));
        }
    }

    fn toggle_sky_view(&mut self) {
        let Some(cur) = self.current().cloned() else {
            return;
        };
        if !self.sky_view.toggle(&cur) {
            return;
        }
        match self.stars.get(&cur).map(|s| &s.solution) {
            Some(Err(e)) => self.set_status(format!("Plate solve failed: {e}"), Some(4.0)),
            None => self.set_status("Plate solving…".into(), Some(2.0)),
            Some(Ok(_)) => {}
        }
    }

    fn toggle_mosaic(&mut self) {
        let was_full = self.loads_full_res();
        self.mosaic = !self.mosaic;
        let state = if self.mosaic { "on" } else { "off" };
        self.set_status(format!("Corner mosaic {state} (G to toggle)"), Some(2.0));
        if self.loads_full_res() != was_full {
            self.reload_all();
        }
    }
}

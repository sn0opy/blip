use super::App;
use eframe::egui::{self, Key, Modifiers, Vec2};

impl App {
    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.confirm_delete.is_some() || self.show_help {
            return;
        }
        let pressed = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        let pressed_cmd = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, key));

        if pressed(Key::Questionmark) || pressed(Key::F1) {
            self.show_help = true;
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
            let was_full = self.loads_full_res();
            self.full_res = !self.full_res;
            let mode = if self.full_res {
                "Full resolution"
            } else {
                "Binned preview"
            };
            self.set_status(format!("{mode} (B to toggle)"), Some(2.0));
            if self.loads_full_res() != was_full {
                self.reload_all();
            }
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

    fn toggle_clip_flash(&mut self) {
        let cur = self.current().cloned();
        if self.flash_clip.is_some() && self.flash_clip == cur {
            self.flash_clip = None;
            return;
        }
        let Some(cur) = cur else { return };
        if self
            .cache
            .get(&cur)
            .is_some_and(|e| e.loaded.clip_overlay.is_none())
        {
            self.set_status("No clipped pixels in this image".into(), Some(2.0));
        }
        self.flash_clip = Some(cur);
    }

    fn toggle_star_view(&mut self) {
        let cur = self.current().cloned();
        self.star_view = if self.star_view.is_some() && self.star_view == cur {
            None
        } else {
            cur
        };
    }

    fn toggle_tilt_view(&mut self) {
        let cur = self.current().cloned();
        if self.tilt_view.is_some() && self.tilt_view == cur {
            self.tilt_view = None;
            return;
        }
        let Some(cur) = cur else { return };
        if self.stars.get(&cur).is_some_and(|s| s.tilt.is_none()) {
            self.set_status("Not enough stars to measure tilt".into(), Some(2.5));
        }
        self.tilt_view = Some(cur);
    }

    fn toggle_sky_view(&mut self) {
        let cur = self.current().cloned();
        if self.sky_view.is_some() && self.sky_view == cur {
            self.sky_view = None;
            return;
        }
        let Some(cur) = cur else { return };
        match self.stars.get(&cur).map(|s| &s.solution) {
            Some(Err(e)) => self.set_status(format!("Plate solve failed: {e}"), Some(4.0)),
            None => self.set_status("Plate solving…".into(), Some(2.0)),
            Some(Ok(_)) => {}
        }
        self.sky_view = Some(cur);
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

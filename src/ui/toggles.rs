use super::actions::{Action, key_hint, keys};
use super::prefs::BinningPref;
use super::{App, file_name};
use blip_core::process::{BOOST_STEP_PERCENT, MAX_BOOST};
use eframe::egui::Vec2;
use std::path::Path;

impl App {
    pub(super) fn change_boost(&mut self, delta: i32) {
        let boost = self.boost.saturating_add_signed(delta).min(MAX_BOOST);
        if boost != self.boost {
            self.boost = boost;
            self.reload_all();
        }
        let target = self.params.boosted(boost).target_bg * 100.0;
        let state = match boost {
            0 => "off".to_string(),
            _ => format!("+{}%", boost * BOOST_STEP_PERCENT),
        };
        let keys = keys(Action::Boost(1));
        let text = format!("Stretch boost {state}: background {target:.0}% ({keys} to change)");
        self.status.set(text, Some(2.0));
    }

    pub(super) fn toggle_actual_size(&mut self) {
        self.set_actual_size(!self.actual_size);
        self.zoom_status();
    }

    /// Reports only if the view changes.
    pub(super) fn zoom_to_fit(&mut self) {
        let fitted = self.is_fitted();
        self.set_actual_size(false);
        if !fitted {
            self.zoom_status();
        }
    }

    pub(super) fn is_fitted(&self) -> bool {
        !self.actual_size && self.zoom == 1.0 && self.pan == Vec2::ZERO
    }

    fn zoom_status(&mut self) {
        let key = key_hint(Action::ZoomActual);
        let text = if self.actual_size {
            format!("Zoom 1:1 ({key} to fit)")
        } else {
            format!("Zoom to fit ({key} for 1:1)")
        };
        self.status.set(text, Some(2.0));
    }

    pub(super) fn set_actual_size(&mut self, on: bool) {
        self.actual_size = on;
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    pub(super) fn toggle_full_res(&mut self) {
        if self.prefs.binning == BinningPref::Bin1 {
            let text = "Binning is set to 1x1 in the settings".into();
            self.status.set(text, Some(2.5));
            return;
        }
        let was_full = self.loads_full_res();
        self.full_res = !self.full_res;
        let mode = if self.full_res {
            "Full resolution"
        } else {
            "Binned preview"
        };
        self.status.set(mode.into(), Some(2.0));
        if self.loads_full_res() != was_full {
            self.reload_all();
        }
    }

    pub(super) fn toggle_clip_flash(&mut self) {
        let Some(cur) = self.current().cloned() else {
            return;
        };
        if self.flash_clip.toggle(&cur)
            && (self.cache.get(&cur)).is_some_and(|l| l.clip_overlay.is_none())
        {
            let text = "No clipped pixels in this image".into();
            self.status.set(text, Some(2.0));
        }
    }

    pub(super) fn toggle_star_view(&mut self) {
        if let Some(cur) = self.current().cloned() {
            self.star_view.toggle(&cur);
        }
    }

    pub(super) fn toggle_tilt_view(&mut self) {
        let Some(cur) = self.current().cloned() else {
            return;
        };
        if self.tilt_view.toggle(&cur) && self.stars.get(&cur).is_some_and(|s| s.tilt.is_none()) {
            let text = "Not enough stars to measure tilt".into();
            self.status.set(text, Some(2.5));
        }
    }

    pub(super) fn toggle_sky_view(&mut self) {
        let Some(cur) = self.current().cloned() else {
            return;
        };
        if !self.sky_view.toggle(&cur) {
            return;
        }
        match self.stars.get(&cur).map(|s| &s.solution) {
            Some(Err(e)) => self
                .status
                .set(format!("Plate solve failed: {e}"), Some(4.0)),
            None => self.status.set("Plate solving…".into(), Some(2.0)),
            Some(Ok(_)) => {}
        }
    }

    pub(super) fn toggle_mosaic(&mut self) {
        let was_full = self.loads_full_res();
        self.mosaic = !self.mosaic;
        let state = if self.mosaic { "on" } else { "off" };
        let key = key_hint(Action::Mosaic);
        self.status.set(
            format!("Corner mosaic {state} ({key} to toggle)"),
            Some(2.0),
        );
        if self.loads_full_res() != was_full {
            self.reload_all();
        }
    }

    pub(super) fn toggle_align(&mut self) {
        let key = key_hint(Action::Align);
        if self.align_to.take().is_some() {
            self.status
                .set(format!("Star alignment off ({key} to toggle)"), Some(2.0));
            return;
        }
        let Some(reference) = self.current().cloned() else {
            return;
        };
        let name = file_name(&reference);
        let text = match self.stars.get(&reference).map(|s| &s.solution) {
            Some(Err(e)) => return self.reject_reference(&reference, &e.clone()),
            Some(Ok(_)) => format!("Aligning stars to {name} ({key} to toggle)"),
            None => format!("Aligning stars to {name} once plate solved ({key} to toggle)"),
        };
        self.status.set(text, Some(2.0));
        self.align_to = Some(reference);
        self.request_loads();
    }

    pub(super) fn reject_reference(&mut self, reference: &Path, error: &str) {
        let name = file_name(reference);
        let text = format!("Can't align stars to {name}: plate solve failed: {error}");
        self.status.set(text, Some(4.0));
        if self.align_to.as_deref() == Some(reference) {
            self.align_to = None;
        }
    }
}

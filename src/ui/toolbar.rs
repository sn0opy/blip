//! A floating bar of buttons below the image, for use without a keyboard.

use super::actions::{self, Action, Action::*};
use super::{App, ImageToggle, draw};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

const BUTTON: f32 = 36.0;
const ICON: f32 = 20.0;
const PADDING: f32 = 4.0;
const GROUP_GAP: f32 = 13.0;
/// Between the bar and the bottom of the image view.
const MARGIN: f32 = 10.0;

const GROUPS: &[&[Action]] = &[
    &[Step(-1), Step(1)],
    &[ZoomFit, ZoomActual],
    &[AutoStretch, ClipFlash],
    &[Tilt, Sky, Stars, Mosaic],
    &[Reject, Delete],
];

#[derive(Clone, Copy)]
struct State {
    enabled: bool,
    active: bool,
}

/// The bar's sizes in points, each a whole number of physical pixels, so every icon
/// sits on the same pixel grid.
#[derive(Clone, Copy)]
struct Layout {
    pixel: f32,
    button: f32,
    padding: f32,
    group_gap: f32,
}

impl Layout {
    fn new(ppp: f32) -> Self {
        let px = |points: f32| (points * ppp).round().max(1.0);
        let mut button = px(BUTTON);
        // An odd number of pixels can't be centered in an even one, and vice versa.
        let (side, gap) = mosaic_cells(ppp);
        if (button - (3.0 * side + 2.0 * gap)) % 2.0 != 0.0 {
            button += 1.0;
        }
        Self {
            pixel: 1.0 / ppp,
            button: button / ppp,
            padding: px(PADDING) / ppp,
            group_gap: px(GROUP_GAP) / ppp,
        }
    }

    fn size(&self) -> Vec2 {
        let buttons = GROUPS.iter().map(|g| g.len()).sum::<usize>() as f32;
        Vec2::new(
            2.0 * self.padding + buttons * self.button + (GROUPS.len() - 1) as f32 * self.group_gap,
            2.0 * self.padding + self.button,
        )
    }

    fn snap(&self, pos: Pos2) -> Pos2 {
        (pos / self.pixel).round() * self.pixel
    }
}

impl App {
    /// Returns how much of the view's bottom the bar covers. Hidden if it doesn't fit
    /// into the view.
    pub(super) fn toolbar(&mut self, ctx: &egui::Context, view: Rect) -> f32 {
        if !self.prefs.show_toolbar {
            return 0.0;
        }
        let layout = Layout::new(ctx.pixels_per_point());
        let size = layout.size();
        if size.x + 2.0 * MARGIN > view.width() || size.y + 2.0 * MARGIN > view.height() {
            return 0.0;
        }
        let pos = layout.snap(Pos2::new(
            view.center().x - size.x / 2.0,
            view.bottom() - MARGIN - size.y,
        ));
        let clicked = egui::Area::new(egui::Id::unique("toolbar"))
            .order(egui::Order::Middle)
            .fixed_pos(pos)
            .movable(false)
            .show(ctx, |ui| bar(ui, layout, |a| self.state(a)))
            .inner;
        if let Some(action) = clicked {
            self.run(ctx, action);
        }
        size.y + MARGIN
    }

    fn state(&self, action: Action) -> State {
        let current = self.current();
        let has_image = current.is_some();
        let on = |t: &ImageToggle| current.is_some_and(|p| t.is_on(p));
        let zoomable = has_image && !self.mosaic;
        let (enabled, active) = match action {
            Step(d) if d < 0 => (self.list.selected() > 0, false),
            Step(_) => (self.list.selected() + 1 < self.list.files().len(), false),
            ZoomFit => (zoomable, self.is_fitted()),
            ZoomActual => (zoomable, self.actual_size),
            AutoStretch => (has_image, on(&self.auto_stretch)),
            ClipFlash => (has_image, on(&self.flash_clip)),
            Stars => (has_image, on(&self.star_view)),
            Tilt => (has_image, on(&self.tilt_view)),
            Sky => (has_image, on(&self.sky_view)),
            // Stays switchable when there's no image left, so it can't get stuck on.
            Mosaic => (has_image || self.mosaic, self.mosaic),
            Reject => (has_image, current.is_some_and(|p| self.list.is_rejected(p))),
            _ => (has_image, false),
        };
        State {
            enabled,
            active: enabled && active,
        }
    }
}

fn bar(ui: &mut egui::Ui, layout: Layout, state: impl Fn(Action) -> State) -> Option<Action> {
    let (rect, _) = ui.allocate_exact_size(layout.size(), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, draw::PANEL_RADIUS, Color32::from_black_alpha(200));
    let mut clicked = None;
    let mut x = rect.left() + layout.padding;
    for (g, group) in GROUPS.iter().enumerate() {
        if g > 0 {
            // One physical pixel wide, centered on a pixel.
            let sep_x = layout.snap(Pos2::new(x + layout.group_gap / 2.0, 0.0)).x;
            painter.vline(
                sep_x - layout.pixel / 2.0,
                rect.y_range().shrink(12.0),
                Stroke::new(layout.pixel, Color32::from_white_alpha(40)),
            );
            x += layout.group_gap;
        }
        for &action in *group {
            let button_state = state(action);
            let button = Rect::from_min_size(
                Pos2::new(x, rect.top() + layout.padding),
                Vec2::splat(layout.button),
            );
            x += layout.button;
            // Not focusable: the arrow keys and Enter would move focus and press buttons.
            let sense = if button_state.enabled {
                Sense::CLICK
            } else {
                Sense::hover()
            };
            let response = actions::tooltip(
                ui.interact(button, ui.scope_id().with(action), sense),
                action,
            );
            if response.clicked() {
                clicked = Some(action);
            }
            let fill = button_fill(&response, button_state);
            painter.rect_filled(button.shrink(layout.pixel), draw::PANEL_RADIUS, fill);
            let icon_rect = Rect::from_center_size(button.center(), Vec2::splat(ICON));
            let color = icon_color(action, &response, button_state);
            paint_icon(painter, icon_rect, action, color);
        }
    }
    clicked
}

fn button_fill(response: &egui::Response, State { enabled, active }: State) -> Color32 {
    if !enabled {
        Color32::TRANSPARENT
    } else if response.is_pointer_button_down_on() {
        Color32::from_white_alpha(60)
    } else if active {
        Color32::from_white_alpha(45)
    } else if response.hovered() {
        Color32::from_white_alpha(20)
    } else {
        Color32::TRANSPARENT
    }
}

fn icon_color(
    action: Action,
    response: &egui::Response,
    State { enabled, active }: State,
) -> Color32 {
    if !enabled {
        Color32::from_gray(85)
    } else if (action == Delete && response.hovered()) || (action == Reject && active) {
        draw::RED
    } else if active || response.hovered() {
        Color32::WHITE
    } else {
        Color32::from_gray(185)
    }
}

/// Drawn in a square where both axes run from -1 to 1.
fn paint_icon(painter: &egui::Painter, rect: Rect, action: Action, color: Color32) {
    let half = rect.width() / 2.0;
    let p = |x: f32, y: f32| rect.center() + Vec2::new(x, y) * half;
    let stroke = Stroke::new(1.5, color);
    let line = |points: &[(f32, f32)]| {
        let points = points.iter().map(|&(x, y)| p(x, y)).collect();
        painter.add(egui::Shape::line(points, stroke));
    };
    let closed = |points: &[(f32, f32)]| {
        let points = points.iter().map(|&(x, y)| p(x, y)).collect();
        painter.add(egui::Shape::closed_line(points, stroke));
    };
    let dot = |x: f32, y: f32, r: f32| painter.circle_filled(p(x, y), r, color);
    let circle = |x: f32, y: f32, r: f32| painter.circle_stroke(p(x, y), r * half, stroke);

    let profile = |y: fn(f32) -> f32| -> Vec<(f32, f32)> {
        (0..=32)
            .map(|i| {
                let x = -0.85 + 1.7 * i as f32 / 32.0;
                (x, y(x))
            })
            .collect()
    };
    let corner_brackets = || {
        for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            line(&[
                (sx * 0.85, sy * 0.4),
                (sx * 0.85, sy * 0.85),
                (sx * 0.4, sy * 0.85),
            ]);
        }
    };

    match action {
        Step(-1) => line(&[(0.3, -0.65), (-0.35, 0.0), (0.3, 0.65)]),
        Step(1) => line(&[(-0.3, -0.65), (0.35, 0.0), (-0.3, 0.65)]),
        ZoomFit => {
            corner_brackets();
            closed(&[(-0.45, -0.3), (0.45, -0.3), (0.45, 0.3), (-0.45, 0.3)]);
        }
        ZoomActual => {
            corner_brackets();
            for x in [-0.4, 0.58] {
                line(&[(x - 0.14, -0.3), (x, -0.45), (x, 0.45)]);
            }
            dot(0.02, -0.18, 0.9);
            dot(0.02, 0.18, 0.9);
        }
        Tilt => {
            let corners = [(-0.8, -0.6), (0.7, -0.8), (0.85, 0.75), (-0.7, 0.6)];
            closed(&corners);
            line(&[corners[0], corners[2]]);
            line(&[corners[1], corners[3]]);
        }
        Sky => {
            circle(0.0, 0.0, 0.85);
            let meridian = Vec2::new(0.35, 0.85) * half;
            painter.add(egui::Shape::ellipse_stroke(p(0.0, 0.0), meridian, stroke));
            line(&[(-0.85, 0.0), (0.85, 0.0)]);
        }
        AutoStretch => {
            line(&[(-0.85, 0.8), (0.85, 0.8)]);
            line(&profile(|x| 0.8 - 0.95 * (-(x / 0.32).powi(2)).exp()));
            line(&[(-0.85, -0.6), (0.85, -0.6)]);
            line(&[(-0.6, -0.85), (-0.85, -0.6), (-0.6, -0.35)]);
            line(&[(0.6, -0.85), (0.85, -0.6), (0.6, -0.35)]);
        }
        ClipFlash => {
            line(&[(-0.85, 0.8), (0.85, 0.8)]);
            line(&profile(|x| {
                (0.8 - 1.9 * (-(x / 0.36).powi(2)).exp()).max(-0.45)
            }));
            let ceiling = [p(-0.85, -0.72), p(0.85, -0.72)];
            painter.extend(egui::Shape::dashed_line(
                &ceiling,
                stroke,
                0.3 * half,
                0.15 * half,
            ));
        }
        Stars => {
            circle(0.0, 0.4, 0.52);
            dot(0.0, 0.4, 1.6);
            painter.text(
                p(0.0, -0.68),
                egui::Align2::CENTER_CENTER,
                "2.4",
                egui::FontId::proportional(9.0),
                color,
            );
        }
        Mosaic => {
            // Snapped to physical pixels, so rounding doesn't make the cells differ in size.
            let ppp = painter.pixels_per_point();
            let (side, gap) = mosaic_cells(ppp);
            let total = 3.0 * side + 2.0 * gap;
            let origin = ((rect.center().to_vec2() * ppp) - Vec2::splat(total / 2.0)).round();
            for row in 0..3 {
                for col in 0..3 {
                    let offset = Vec2::new(col as f32, row as f32) * (side + gap);
                    let min = (origin + offset) / ppp;
                    let square = Rect::from_min_size(min.to_pos2(), Vec2::splat(side / ppp));
                    painter.rect_filled(square, 0.0, color);
                }
            }
        }
        Reject => {
            line(&[
                (0.1, -0.6),
                (-0.85, -0.6),
                (-0.85, 0.75),
                (0.85, 0.75),
                (0.85, -0.1),
            ]);
            line(&[
                (-0.85, 0.5),
                (-0.35, 0.0),
                (0.05, 0.4),
                (0.35, 0.15),
                (0.85, 0.6),
            ]);
            dot(-0.45, -0.3, 1.2);
            line(&[(0.3, -0.9), (0.9, -0.3)]);
            line(&[(0.3, -0.3), (0.9, -0.9)]);
        }
        Delete => {
            line(&[(-0.8, -0.55), (0.8, -0.55)]);
            line(&[(-0.25, -0.55), (-0.25, -0.82), (0.25, -0.82), (0.25, -0.55)]);
            line(&[(-0.55, -0.4), (-0.45, 0.85), (0.45, 0.85), (0.55, -0.4)]);
            line(&[(-0.15, -0.2), (-0.15, 0.6)]);
            line(&[(0.15, -0.2), (0.15, 0.6)]);
        }
        _ => {}
    }
}

/// The corner mosaic's cell side and gap, in whole physical pixels.
fn mosaic_cells(ppp: f32) -> (f32, f32) {
    let half = ICON / 2.0;
    let side = (0.56 * half * ppp).floor().max(1.0);
    let gap = (0.12 * half * ppp).round().max(1.0);
    (side, gap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_whole_pixels_and_centers_the_mosaic() {
        for ppp in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0] {
            let layout = Layout::new(ppp);
            for points in [layout.button, layout.padding, layout.group_gap] {
                let px = points * ppp;
                assert!((px - px.round()).abs() < 1e-4, "{px} px at {ppp}x");
            }
            let (side, gap) = mosaic_cells(ppp);
            let spare = (layout.button * ppp).round() - (3.0 * side + 2.0 * gap);
            assert!(spare >= 0.0 && spare % 2.0 == 0.0, "{spare} px at {ppp}x");
        }
    }
}

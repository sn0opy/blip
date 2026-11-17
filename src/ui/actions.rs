//! Everything the keyboard and the toolbar can do, with its keys and descriptions. The
//! help, settings, tooltips and status messages all read their key hints from here.

use super::App;
use super::dialogs::Deletion;
use eframe::egui::{self, Key, Modifiers};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Step(isize),
    First,
    Last,
    Delete,
    Reject,
    HandleRejected,
    ZoomFit,
    ZoomActual,
    AutoStretch,
    Boost(i32),
    ClipFlash,
    Stars,
    Tilt,
    Sky,
    Mosaic,
    Align,
    FullRes,
    Neutralize,
    Info,
    Header,
    Debug,
    Export,
    Filter,
    Open,
    Rescan,
    Settings,
    Help,
}

pub struct Binding {
    modifiers: Modifiers,
    key: Key,
    action: Action,
}

const fn key(key: Key, action: Action) -> Binding {
    Binding {
        modifiers: Modifiers::NONE,
        key,
        action,
    }
}

const fn shift(key: Key, action: Action) -> Binding {
    Binding {
        modifiers: Modifiers::SHIFT,
        key,
        action,
    }
}

const fn cmd(key: Key, action: Action) -> Binding {
    Binding {
        modifiers: Modifiers::COMMAND,
        key,
        action,
    }
}

/// A row of the help. The first binding of an action is its key in hints.
pub struct Shortcut {
    pub keys: &'static str,
    pub description: &'static str,
    /// Shorter, for settings, tooltips and buttons. The description if not set.
    name: Option<&'static str>,
    pub bindings: &'static [Binding],
}

const fn row(
    keys: &'static str,
    description: &'static str,
    bindings: &'static [Binding],
) -> Shortcut {
    Shortcut {
        keys,
        description,
        name: None,
        bindings,
    }
}

impl Shortcut {
    const fn named(self, name: &'static str) -> Self {
        Shortcut {
            name: Some(name),
            ..self
        }
    }
}

use Action::*;

pub const SHORTCUTS: &[Shortcut] = &[
    row(
        "↓ → Space J",
        "Next image",
        &[
            key(Key::ArrowRight, Step(1)),
            key(Key::ArrowDown, Step(1)),
            key(Key::Space, Step(1)),
            key(Key::J, Step(1)),
        ],
    ),
    row(
        "↑ ← K",
        "Previous image",
        &[
            key(Key::ArrowLeft, Step(-1)),
            key(Key::ArrowUp, Step(-1)),
            key(Key::K, Step(-1)),
        ],
    ),
    row(
        "PgDn / PgUp",
        "Jump 10 images",
        &[key(Key::PageDown, Step(10)), key(Key::PageUp, Step(-10))],
    ),
    row(
        "Home / End",
        "First / last image",
        &[key(Key::Home, First), key(Key::End, Last)],
    ),
    row(
        "Del / Backspace",
        "Delete file permanently (asks first)",
        &[key(Key::Delete, Delete), key(Key::Backspace, Delete)],
    )
    .named("Delete file"),
    row("X", "Reject or unreject file", &[key(Key::X, Reject)]).named("Reject file"),
    row(
        "Shift+X",
        "Move, trash or delete the rejected files (see Settings)",
        &[shift(Key::X, HandleRejected)],
    )
    .named("Handle rejected files"),
    row("Wheel / drag", "Zoom / pan (kept while blinking)", &[]),
    row(
        "Right-click file",
        "Plate solve, sky links, file actions",
        &[],
    ),
    row(
        "Double-click, F, 0",
        "Reset zoom",
        &[key(Key::F, ZoomFit), key(Key::Num0, ZoomFit)],
    )
    .named("Zoom to fit"),
    row("Z", "Zoom to 1:1", &[key(Key::Z, ZoomActual)]).named("Zoom 1:1"),
    row(
        "A",
        "Toggle auto stretch for this image",
        &[key(Key::A, AutoStretch)],
    )
    .named("Auto stretch"),
    row(
        "+ / -",
        "Stretch harder / softer",
        &[
            key(Key::Plus, Boost(1)),
            key(Key::Equals, Boost(1)),
            key(Key::Minus, Boost(-1)),
        ],
    ),
    row(
        "C",
        "Toggle flashing clipped pixels on this image",
        &[key(Key::C, ClipFlash)],
    )
    .named("Flashing clipped pixels"),
    row(
        "S",
        "Toggle star sizes (HFR) on this image",
        &[key(Key::S, Stars)],
    )
    .named("Star sizes"),
    row(
        "T",
        "Toggle sensor tilt (corner HFR) on this image",
        &[key(Key::T, Tilt)],
    )
    .named("Sensor tilt"),
    row(
        "P",
        "Toggle plate solve annotation on this image",
        &[key(Key::P, Sky)],
    )
    .named("Plate solve annotation"),
    row(
        "G",
        "Toggle corner mosaic (aberration inspection)",
        &[key(Key::G, Mosaic)],
    )
    .named("Corner mosaic"),
    row(
        "L",
        "Toggle star alignment to the current image (by plate solve)",
        &[key(Key::L, Align)],
    )
    .named("Star alignment"),
    row(
        "B",
        "Toggle binned preview / full resolution",
        &[key(Key::B, FullRes)],
    )
    .named("Binning"),
    row(
        "N",
        "Toggle background neutralization (white balance)",
        &[key(Key::N, Neutralize)],
    )
    .named("Background neutralization"),
    row("I", "Toggle info overlay", &[key(Key::I, Info)]).named("Info overlay"),
    row("H", "Show all header entries", &[key(Key::H, Header)]),
    row(
        "D",
        "Toggle debug info (timings, stretch, cache, GPU)",
        &[key(Key::D, Debug)],
    )
    .named("Debug info"),
    row(
        "E, Ctrl+S",
        "Export as PNG / JPEG at full resolution",
        &[key(Key::E, Export), cmd(Key::S, Export)],
    ),
    row(
        "Ctrl+F",
        "Filter files by name or folder",
        &[cmd(Key::F, Filter)],
    ),
    row(
        "O, Ctrl+O",
        "Open folder",
        &[key(Key::O, Open), cmd(Key::O, Open)],
    ),
    row(
        "F5, Ctrl+R",
        "Rescan folder",
        &[key(Key::F5, Rescan), cmd(Key::R, Rescan)],
    ),
    row("Ctrl+,", "Open settings", &[cmd(Key::Comma, Settings)]),
    row(
        "?, F1",
        "This help",
        &[key(Key::Questionmark, Help), key(Key::F1, Help)],
    ),
];

/// `egui::Modifiers::COMMAND` is Cmd there.
const IS_MAC: bool = cfg!(target_os = "macos");

fn bindings() -> impl Iterator<Item = &'static Binding> {
    SHORTCUTS.iter().flat_map(|s| s.bindings)
}

fn binding(action: Action) -> Option<(&'static Shortcut, &'static Binding)> {
    SHORTCUTS.iter().find_map(|s| {
        let b = s.bindings.iter().find(|b| b.action == action)?;
        Some((s, b))
    })
}

pub fn name(action: Action) -> &'static str {
    binding(action).map_or("", |(s, _)| s.name.unwrap_or(s.description))
}

/// A help row's keys, with "Ctrl" named as on this platform.
pub fn platform_keys(keys: &'static str) -> Cow<'static, str> {
    let names = egui::ModifierNames::NAMES;
    if IS_MAC {
        Cow::Owned(keys.replace(names.ctrl, names.mac_cmd))
    } else {
        Cow::Borrowed(keys)
    }
}

/// The action's first key, like "A", "→" or "Ctrl+," ("Cmd+," on macOS).
pub fn key_hint(action: Action) -> String {
    binding(action).map_or_else(String::new, |(_, b)| b.label())
}

impl Binding {
    /// As written in the help, like "→", "PgDn" or "Ctrl+S".
    fn label(&self) -> String {
        let key = match self.key {
            Key::ArrowRight => "→",
            Key::ArrowLeft => "←",
            Key::ArrowDown => "↓",
            Key::ArrowUp => "↑",
            Key::PageDown => "PgDn",
            Key::PageUp => "PgUp",
            Key::Delete => "Del",
            // "=" is the unshifted "+".
            Key::Plus | Key::Equals => "+",
            Key::Minus => "-",
            Key::Comma => ",",
            Key::Questionmark => "?",
            Key::Num0 => "0",
            other => other.name(),
        };
        let names = egui::ModifierNames::NAMES;
        let modifiers = names.format(&self.modifiers, IS_MAC);
        if modifiers.is_empty() {
            key.to_string()
        } else {
            format!("{modifiers}{}{key}", names.concat)
        }
    }
}

/// All keys of the action's help row, like "+ / -".
pub fn keys(action: Action) -> Cow<'static, str> {
    platform_keys(binding(action).map_or("", |(s, _)| s.keys))
}

/// `name (key)`, styled as in the shortcuts dialog.
pub fn label(style: &egui::Style, action: Action) -> egui::text::LayoutJob {
    with_key(style, name(action), &key_hint(action))
}

/// The action's [`label`] as a tooltip, built only while it is shown.
pub fn tooltip(response: egui::Response, action: Action) -> egui::Response {
    tooltip_text(response, name(action), action)
}

pub fn tooltip_text(response: egui::Response, text: &str, action: Action) -> egui::Response {
    response.on_hover_ui(|ui| {
        ui.label(with_key(ui.style(), text, &key_hint(action)));
    })
}

/// `text (key)` with the key styled as in the shortcuts dialog.
pub fn with_key(style: &egui::Style, text: &str, key: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontSelection::Default;
    let align = egui::Align::Center;
    egui::RichText::new(format!("{text} (")).append_to(&mut job, style, font.clone(), align);
    egui::RichText::new(key)
        .monospace()
        .strong()
        .append_to(&mut job, style, font.clone(), align);
    egui::RichText::new(")").append_to(&mut job, style, font, align);
    job
}

/// Consumes this frame's presses of any of the action's keys.
pub fn pressed(ctx: &egui::Context, action: Action) -> bool {
    !consume_pressed(ctx, |b| b.action == action).is_empty()
}

/// The actions of this frame's key presses that `wanted` accepts, once each. Their keys
/// are consumed.
fn consume_pressed(ctx: &egui::Context, wanted: impl Fn(&Binding) -> bool) -> Vec<Action> {
    ctx.input_mut(|i| {
        let mut actions = Vec::new();
        let any_key =
            (i.events.iter()).any(|e| matches!(e, egui::Event::Key { pressed: true, .. }));
        if !any_key {
            return actions;
        }
        // A key without modifiers also matches with Shift held, so those go last.
        let (plain, modified): (Vec<_>, Vec<_>) = bindings()
            .filter(|b| wanted(b))
            .partition(|b| b.modifiers.is_none());
        for b in modified.into_iter().chain(plain) {
            if i.consume_key(b.modifiers, b.key) && !actions.contains(&b.action) {
                actions.push(b.action);
            }
        }
        actions
    })
}

impl App {
    /// Runs each pressed action once, even if several of its keys came in this frame.
    /// Opening the help or the settings drops the frame's other keys.
    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.confirm_delete.is_some()
            || self.show_help
            || self.show_settings
            || self.show_header
            || ctx.text_edit_focused()
        {
            return;
        }
        let mut actions = consume_pressed(ctx, |_| true);
        if let Some(&dialog) = actions.iter().find(|a| matches!(a, Help | Settings)) {
            actions = vec![dialog];
        }
        for action in actions {
            self.run(ctx, action);
        }
    }

    pub(super) fn run(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Step(delta) => self.step(delta),
            First => self.select(0),
            Last => self.select(usize::MAX),
            Delete => self.confirm_delete = self.current().cloned().map(Deletion::File),
            Reject => {
                if let Some(path) = self.current().cloned() {
                    self.toggle_rejected(&path);
                }
            }
            HandleRejected => self.handle_rejected(ctx),
            // Zoom doesn't apply to the mosaic, as on the toolbar.
            ZoomFit | ZoomActual if self.mosaic => {}
            ZoomFit => self.zoom_to_fit(),
            ZoomActual => self.toggle_actual_size(),
            AutoStretch => self.toggle_stretch(),
            Boost(delta) => self.change_boost(delta),
            ClipFlash => self.toggle_clip_flash(),
            Stars => self.toggle_star_view(),
            Tilt => self.toggle_tilt_view(),
            Sky => self.toggle_sky_view(),
            Mosaic => self.toggle_mosaic(),
            Align => self.toggle_align(),
            FullRes => self.toggle_full_res(),
            Neutralize => {
                self.params.neutralize = !self.params.neutralize;
                self.reload_all();
            }
            Info => self.show_info = !self.show_info,
            Header => {
                if self.current().is_some() {
                    self.open_header();
                }
            }
            Debug => self.show_debug = !self.show_debug,
            Export => self.export_current(ctx),
            Filter => self.focus_filter(ctx),
            Open => self.pick_folder(),
            Rescan => self.refresh(),
            Settings => self.show_settings = true,
            Help => self.show_help = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readme_lists_every_shortcut() {
        let readme = include_str!("../../README.md");
        let table = readme
            .split("## Keys")
            .nth(1)
            .and_then(|s| s.split("\n## ").next())
            .expect("README has a Keys section");
        let descriptions: Vec<&str> = table
            .lines()
            .filter(|l| l.starts_with("| ") && !l.starts_with("| Key") && !l.starts_with("| ---"))
            .filter_map(|l| l.trim_end_matches('|').rsplit('|').next())
            .map(str::trim)
            .collect();
        let expected: Vec<&str> = SHORTCUTS.iter().map(|s| s.description).collect();
        assert_eq!(descriptions, expected);
    }

    #[test]
    fn no_key_is_bound_twice() {
        let bindings: Vec<&Binding> = bindings().collect();
        for (i, a) in bindings.iter().enumerate() {
            for b in &bindings[i + 1..] {
                assert!(
                    (a.modifiers, a.key) != (b.modifiers, b.key),
                    "{:?} is bound to {:?} and {:?}",
                    a.key,
                    a.action,
                    b.action
                );
            }
        }
    }

    #[test]
    fn help_rows_list_their_keys() {
        for shortcut in SHORTCUTS {
            let keys = platform_keys(shortcut.keys);
            let listed: Vec<&str> = keys
                .split(", ")
                .flat_map(|k| k.split(" / "))
                .flat_map(str::split_whitespace)
                .collect();
            for b in shortcut.bindings {
                let label = b.label();
                assert!(listed.contains(&label.as_str()), "{label} not in {keys:?}");
            }
        }
    }

    #[test]
    fn shift_x_handles_the_rejected_files_instead_of_rejecting() {
        let ctx = egui::Context::default();
        let press = |modifiers| egui::Event::Key {
            key: Key::X,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let pressed = |modifiers| {
            let events = vec![press(modifiers)];
            ctx.begin_pass(egui::RawInput {
                events,
                ..Default::default()
            });
            let actions = consume_pressed(&ctx, |_| true);
            ctx.end_pass().textures_delta.clear();
            actions
        };
        assert_eq!(pressed(Modifiers::SHIFT), [HandleRejected]);
        assert_eq!(pressed(Modifiers::NONE), [Reject]);
    }

    #[test]
    fn hints_name_the_first_key() {
        assert_eq!(key_hint(Step(1)), "→");
        assert_eq!(key_hint(Delete), "Del");
        assert_eq!(key_hint(HandleRejected), "Shift+X");
        let command = if IS_MAC { "Cmd" } else { "Ctrl" };
        assert_eq!(key_hint(Settings), format!("{command}+,"));
        assert_eq!(keys(Export), format!("E, {command}+S"));
        assert_eq!(keys(Boost(-1)), "+ / -");
    }
}

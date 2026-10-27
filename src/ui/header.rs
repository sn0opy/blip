//! The dialog listing every header entry of the current image, sortable by column.

use super::dialogs::{DIALOG_WIDTH, with_key};
use super::{App, file_name};
use blip_core::image::Card;
use eframe::egui::{self, Key, Modifiers};
use egui_extras::{Column, TableBuilder};
use std::cmp::Ordering;

/// The heading, file name, Close button, frame and a gap to the window's edges.
const FIXED_HEIGHT: f32 = 180.0;
const WIDTH: f32 = DIALOG_WIDTH * 4.0 / 3.0;
const NO_COMMENT: &str = "no comment";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SortColumn {
    Keyword,
    Value,
    Comment,
}

impl SortColumn {
    const ALL: [Self; 3] = [Self::Keyword, Self::Value, Self::Comment];

    fn label(self) -> &'static str {
        match self {
            Self::Keyword => "Keyword",
            Self::Value => "Value",
            Self::Comment => "Comment",
        }
    }

    fn text(self, card: &Card) -> &str {
        match self {
            Self::Keyword => &card.key,
            Self::Value => card.value.as_deref().unwrap_or_default(),
            Self::Comment if card.comment.eq_ignore_ascii_case(NO_COMMENT) => "",
            Self::Comment => &card.comment,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Sort {
    column: SortColumn,
    descending: bool,
}

fn next_sort(sort: Option<Sort>, column: SortColumn) -> Option<Sort> {
    match sort {
        Some(s) if s.column == column && !s.descending => Some(Sort {
            column,
            descending: true,
        }),
        Some(s) if s.column == column => None,
        _ => Some(Sort {
            column,
            descending: false,
        }),
    }
}

fn compare(a: &str, b: &str) -> Ordering {
    let number = |s: &str| s.trim().parse::<f64>().ok().filter(|x| x.is_finite());
    match (number(a), number(b)) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

fn sorted(cards: &[Card], sort: Option<Sort>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..cards.len()).collect();
    if let Some(Sort { column, descending }) = sort {
        order.sort_by(|&a, &b| {
            let (a, b) = (column.text(&cards[a]), column.text(&cards[b]));
            match (a.is_empty(), b.is_empty()) {
                (false, false) if descending => compare(b, a),
                (false, false) => compare(a, b),
                (a, b) => a.cmp(&b),
            }
        });
    }
    order
}

impl App {
    pub(super) fn header_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_header {
            return;
        }
        let Some(path) = self.current().cloned() else {
            self.show_header = false;
            return;
        };
        let stale = (self.stale.as_ref()).filter(|(p, _)| *p == path);
        let cards = (self.cache.get(&path).or(stale.map(|(_, e)| e))).map(|e| &e.loaded.cards);
        let error = self.errors.get(&path);
        let mut sort = self.header_sort;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("header")).show(ctx, |ui| {
            let screen = ctx.content_rect();
            ui.set_width(WIDTH);
            let style = ui.style().clone();
            ui.heading("Header");
            ui.add_space(8.0);
            ui.label(egui::RichText::new(file_name(&path)).strong());
            ui.add_space(8.0);
            match (cards, error) {
                (Some(cards), _) if cards.is_empty() => {
                    ui.weak("The file has no header entries.");
                }
                (Some(cards), _) => {
                    let max_height = screen.height() - FIXED_HEIGHT;
                    egui::ScrollArea::both()
                        .max_height(max_height)
                        .show(ui, |ui| table(ui, cards, &mut sort));
                }
                (None, Some(e)) => {
                    ui.weak(format!("Cannot read the file: {e}"));
                }
                (None, None) => {
                    ui.weak("Loading…");
                }
            }
            ui.add_space(10.0);
            if ui.button(with_key(&style, "Close", "Esc")).clicked() {
                close = true;
            }
        });
        self.header_sort = sort;
        let toggled = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::H));
        if close || toggled || modal.should_close() {
            self.show_header = false;
        }
    }

    pub(super) fn open_header(&mut self) {
        self.show_header = true;
        self.header_sort = None;
    }
}

fn table(ui: &mut egui::Ui, cards: &[Card], sort: &mut Option<Sort>) {
    let order = sorted(cards, *sort);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    let row_height =
        ui.text_style_height(&egui::TextStyle::Body) + 2.0 * ui.spacing().item_spacing.y;
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .vscroll(false)
        .columns(Column::auto().at_least(80.0), 3)
        .header(row_height, |mut header| {
            for column in SortColumn::ALL {
                header.col(|ui| {
                    let text = egui::RichText::new(column.label()).strong();
                    let response = ui.add(egui::Button::new(text).frame(false));
                    if let Some(s) = sort.filter(|s| s.column == column) {
                        sort_arrow(ui, response.rect, s.descending);
                    }
                    if response.clicked() {
                        *sort = next_sort(*sort, column);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(row_height, order.len(), |mut row| {
                let card = &cards[order[row.index()]];
                row.col(|ui| {
                    ui.label(egui::RichText::new(&card.key).monospace());
                });
                for column in [SortColumn::Value, SortColumn::Comment] {
                    row.col(|ui| {
                        ui.label(column.text(card));
                    });
                }
            });
        });
}

/// The bundled fonts have no arrow glyphs.
fn sort_arrow(ui: &egui::Ui, label: egui::Rect, descending: bool) {
    let size = ui.text_style_height(&egui::TextStyle::Body) * 0.3;
    let center = egui::pos2(label.right() + size * 1.5, label.center().y);
    let (tip, base) = if descending {
        (size, -size)
    } else {
        (-size, size)
    };
    let points = vec![
        center + egui::vec2(0.0, tip),
        center + egui::vec2(-size, base),
        center + egui::vec2(size, base),
    ];
    let color = ui.visuals().strong_text_color();
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(key: &str, value: Option<&str>, comment: &str) -> Card {
        Card {
            key: key.into(),
            value: value.map(Into::into),
            comment: comment.into(),
        }
    }

    #[test]
    fn sort_cycles_back_to_file_order() {
        let asc = next_sort(None, SortColumn::Value);
        assert_eq!(
            asc,
            Some(Sort {
                column: SortColumn::Value,
                descending: false
            })
        );
        let desc = next_sort(asc, SortColumn::Value);
        assert!(desc.is_some_and(|s| s.descending));
        assert_eq!(next_sort(desc, SortColumn::Value), None);
        assert!(!next_sort(desc, SortColumn::Keyword).unwrap().descending);
    }

    #[test]
    fn sorts_numbers_by_value_and_empty_cells_last() {
        let cards = [
            card("EXPTIME", Some("120."), "[s]"),
            card("COMMENT", None, "made by N.I.N.A."),
            card("CD1_1", Some("1E-4"), "no comment"),
            card("CCD-TEMP", Some("-10.5"), ""),
            card("object", Some("M 31"), "target"),
            card("GAIN", Some("9"), ""),
            card("FILTER", Some("Ha"), "filter"),
        ];
        let by = |column, descending| sorted(&cards, Some(Sort { column, descending }));
        assert_eq!(sorted(&cards, None), [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(by(SortColumn::Keyword, false), [3, 2, 1, 0, 6, 5, 4]);
        assert_eq!(by(SortColumn::Value, false), [3, 2, 5, 0, 6, 4, 1]);
        assert_eq!(by(SortColumn::Value, true), [4, 6, 0, 5, 2, 3, 1]);
        assert_eq!(by(SortColumn::Comment, true), [4, 1, 6, 0, 2, 3, 5]);
    }
}

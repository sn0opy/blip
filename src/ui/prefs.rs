//! Preferences from the settings dialog, kept in eframe's storage.

use super::MAX_SIDE;
use super::loader::Binning;
use blip_core::process::MAX_BOOST;
use eframe::Storage;
use std::collections::HashMap;

pub const MAX_PREFETCH: usize = 8;
pub const MAX_SUBFOLDER_DEPTH: usize = 6;

/// A setting picked from a list, saved under its key.
pub trait Choice: Copy + PartialEq + 'static {
    const ALL: &'static [Self];
    fn label(self) -> &'static str;
    fn key(self) -> &'static str;
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum BinningPref {
    #[default]
    Auto,
    Bin1,
    Bin2,
    Bin3,
    Bin4,
}

impl BinningPref {
    pub fn binning(self) -> Binning {
        match self {
            Self::Auto => Binning::MaxSide(MAX_SIDE),
            Self::Bin1 => Binning::Full,
            Self::Bin2 => Binning::Factor(2),
            Self::Bin3 => Binning::Factor(3),
            Self::Bin4 => Binning::Factor(4),
        }
    }
}

impl Choice for BinningPref {
    const ALL: &'static [Self] = &[Self::Auto, Self::Bin1, Self::Bin2, Self::Bin3, Self::Bin4];

    fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Bin1 => "1x1",
            Self::Bin2 => "2x2",
            Self::Bin3 => "3x3",
            Self::Bin4 => "4x4",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Bin1 => "1",
            Self::Bin2 => "2",
            Self::Bin3 => "3",
            Self::Bin4 => "4",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SubjectOrder {
    #[default]
    Size,
    Name,
}

impl Choice for SubjectOrder {
    const ALL: &'static [Self] = &[Self::Size, Self::Name];

    fn label(self) -> &'static str {
        match self {
            Self::Size => "Apparent size",
            Self::Name => "Name",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Name => "name",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum RejectHandling {
    #[default]
    MoveToFolder,
    MoveToTrash,
    Delete,
}

impl Choice for RejectHandling {
    const ALL: &'static [Self] = &[Self::MoveToFolder, Self::MoveToTrash, Self::Delete];

    fn label(self) -> &'static str {
        match self {
            Self::MoveToFolder => "Move to rejected folder",
            Self::MoveToTrash => "Move to trash",
            Self::Delete => "Delete permanently",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::MoveToFolder => "folder",
            Self::MoveToTrash => "trash",
            Self::Delete => "delete",
        }
    }
}

/// The starting state: keys change the session, not these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preferences {
    /// 0 lists only the opened folder's images.
    pub subfolder_depth: usize,
    pub group_by_folder: bool,
    pub header_search: bool,
    pub scan_headers: bool,
    pub binning: BinningPref,
    /// Counted in the browsing direction.
    pub ahead: usize,
    pub behind: usize,
    pub subject_order: SubjectOrder,
    pub reject_handling: RejectHandling,
    pub boost: u32,
    pub show_info: bool,
    pub show_debug: bool,
    pub actual_size: bool,
    pub neutralize: bool,
    pub auto_stretch: bool,
    pub flash_clip: bool,
    pub star_view: bool,
    pub tilt_view: bool,
    pub sky_view: bool,
    pub show_toolbar: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            subfolder_depth: 0,
            group_by_folder: true,
            header_search: false,
            scan_headers: false,
            binning: BinningPref::Auto,
            ahead: 3,
            behind: 1,
            subject_order: SubjectOrder::Size,
            reject_handling: RejectHandling::MoveToFolder,
            boost: 0,
            show_info: true,
            show_debug: false,
            actual_size: false,
            neutralize: true,
            auto_stretch: true,
            flash_clip: false,
            star_view: false,
            tilt_view: false,
            sky_view: false,
            show_toolbar: true,
        }
    }
}

impl Preferences {
    pub fn load(storage: Option<&dyn Storage>) -> Self {
        let mut prefs = Self::default();
        let Some(storage) = storage else {
            return prefs;
        };
        load_choice(storage, "binning", &mut prefs.binning);
        load_choice(storage, "subject_order", &mut prefs.subject_order);
        load_choice(storage, "reject_handling", &mut prefs.reject_handling);
        let number = |key: &str, max: usize| {
            (storage.get_string(key)?.parse::<usize>().ok()).filter(|&n| n <= max)
        };
        if let Some(n) = number("subfolder_depth", MAX_SUBFOLDER_DEPTH) {
            prefs.subfolder_depth = n;
        }
        if let Some(n) = number("ahead", MAX_PREFETCH) {
            prefs.ahead = n;
        }
        if let Some(n) = number("behind", MAX_PREFETCH) {
            prefs.behind = n;
        }
        if let Some(n) = number("boost", MAX_BOOST as usize) {
            prefs.boost = n as u32;
        }
        for (key, flag) in prefs.flags() {
            match storage.get_string(key).as_deref() {
                Some("true") => *flag = true,
                Some("false") => *flag = false,
                _ => {}
            }
        }
        prefs
    }

    /// Reads eframe's storage file directly, for use before eframe has opened it.
    pub fn load_saved(app_id: &str) -> Self {
        let saved = eframe::storage_dir(app_id)
            .and_then(|dir| std::fs::read_to_string(dir.join("app.ron")).ok())
            .and_then(|text| ron::from_str(&text).ok())
            .map(MapStorage);
        Self::load(saved.as_ref().map(|s| s as &dyn Storage))
    }

    pub fn save(mut self, storage: &mut dyn Storage) {
        storage.set_string("subfolder_depth", self.subfolder_depth.to_string());
        storage.set_string("binning", self.binning.key().into());
        storage.set_string("ahead", self.ahead.to_string());
        storage.set_string("behind", self.behind.to_string());
        storage.set_string("subject_order", self.subject_order.key().into());
        storage.set_string("reject_handling", self.reject_handling.key().into());
        storage.set_string("boost", self.boost.to_string());
        for (key, flag) in self.flags() {
            storage.set_string(key, flag.to_string());
        }
    }

    fn flags(&mut self) -> [(&'static str, &mut bool); 13] {
        [
            ("group_by_folder", &mut self.group_by_folder),
            ("header_search", &mut self.header_search),
            ("scan_headers", &mut self.scan_headers),
            ("show_info", &mut self.show_info),
            ("show_debug", &mut self.show_debug),
            ("actual_size", &mut self.actual_size),
            ("neutralize", &mut self.neutralize),
            ("auto_stretch", &mut self.auto_stretch),
            ("flash_clip", &mut self.flash_clip),
            ("star_view", &mut self.star_view),
            ("tilt_view", &mut self.tilt_view),
            ("sky_view", &mut self.sky_view),
            ("show_toolbar", &mut self.show_toolbar),
        ]
    }
}

/// Leaves `value` as it is if nothing valid was saved.
fn load_choice<T: Choice>(storage: &dyn Storage, key: &str, value: &mut T) {
    let saved = storage.get_string(key);
    if let Some(&choice) = T::ALL.iter().find(|c| Some(c.key()) == saved.as_deref()) {
        *value = choice;
    }
}

#[derive(Default)]
struct MapStorage(HashMap<String, String>);

impl Storage for MapStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn flush(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(entries: &[(&str, &str)]) -> Preferences {
        let mut storage = MapStorage::default();
        for (key, value) in entries {
            storage.set_string(key, value.to_string());
        }
        Preferences::load(Some(&storage))
    }

    #[test]
    fn saved_preferences_load_back() {
        let prefs = Preferences {
            subfolder_depth: MAX_SUBFOLDER_DEPTH,
            group_by_folder: false,
            header_search: true,
            scan_headers: true,
            binning: BinningPref::Bin3,
            ahead: MAX_PREFETCH,
            behind: 0,
            subject_order: SubjectOrder::Name,
            reject_handling: RejectHandling::MoveToTrash,
            boost: MAX_BOOST,
            show_info: false,
            show_debug: true,
            actual_size: true,
            neutralize: false,
            auto_stretch: false,
            flash_clip: true,
            star_view: true,
            tilt_view: true,
            sky_view: true,
            show_toolbar: false,
        };
        let mut storage = MapStorage::default();
        prefs.save(&mut storage);
        assert_eq!(Preferences::load(Some(&storage)), prefs);
    }

    #[test]
    fn invalid_values_fall_back_to_the_defaults() {
        let defaults = Preferences::default();
        assert_eq!(load(&[]), defaults);
        let max_prefetch = (MAX_PREFETCH + 1).to_string();
        let max_boost = (MAX_BOOST + 1).to_string();
        let max_subfolder_depth = (MAX_SUBFOLDER_DEPTH + 1).to_string();
        let invalid = [
            ("subfolder_depth", max_subfolder_depth.as_str()),
            ("binning", "5"),
            ("ahead", max_prefetch.as_str()),
            ("behind", "-1"),
            ("subject_order", "Name"),
            ("reject_handling", "Trash"),
            ("boost", max_boost.as_str()),
            ("show_info", "no"),
            ("show_debug", "1"),
            ("actual_size", "on"),
            ("neutralize", ""),
            ("auto_stretch", "TRUE"),
            ("flash_clip", "yes"),
        ];
        assert_eq!(load(&invalid), defaults);
        assert_eq!(load(&[("ahead", "2.5"), ("behind", "lots")]), defaults);
    }
}

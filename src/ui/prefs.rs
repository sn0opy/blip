//! Preferences from the settings dialog, kept in eframe's storage.

use super::MAX_SIDE;
use super::loader::Binning;
use blip_core::process::MAX_BOOST;
use eframe::Storage;
use std::collections::HashMap;

pub const MAX_PREFETCH: usize = 8;
pub const MAX_SUBFOLDER_DEPTH: usize = 6;

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
    pub const ALL: [Self; 5] = [Self::Auto, Self::Bin1, Self::Bin2, Self::Bin3, Self::Bin4];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Bin1 => "1x1",
            Self::Bin2 => "2x2",
            Self::Bin3 => "3x3",
            Self::Bin4 => "4x4",
        }
    }

    pub fn binning(self) -> Binning {
        match self {
            Self::Auto => Binning::MaxSide(MAX_SIDE),
            Self::Bin1 => Binning::Full,
            Self::Bin2 => Binning::Factor(2),
            Self::Bin3 => Binning::Factor(3),
            Self::Bin4 => Binning::Factor(4),
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

/// The starting state: keys change the session, not these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preferences {
    /// 0 lists only the opened folder's images.
    pub subfolder_depth: usize,
    pub binning: BinningPref,
    /// Counted in the browsing direction.
    pub ahead: usize,
    pub behind: usize,
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
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            subfolder_depth: 0,
            binning: BinningPref::Auto,
            ahead: 3,
            behind: 1,
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
        }
    }
}

impl Preferences {
    pub fn load(storage: Option<&dyn Storage>) -> Self {
        let mut prefs = Self::default();
        let Some(storage) = storage else {
            return prefs;
        };
        let binning = storage.get_string("binning");
        if let Some(b) = BinningPref::ALL
            .into_iter()
            .find(|b| Some(b.key()) == binning.as_deref())
        {
            prefs.binning = b;
        }
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
        storage.set_string("boost", self.boost.to_string());
        for (key, flag) in self.flags() {
            storage.set_string(key, flag.to_string());
        }
    }

    fn flags(&mut self) -> [(&'static str, &mut bool); 9] {
        [
            ("show_info", &mut self.show_info),
            ("show_debug", &mut self.show_debug),
            ("actual_size", &mut self.actual_size),
            ("neutralize", &mut self.neutralize),
            ("auto_stretch", &mut self.auto_stretch),
            ("flash_clip", &mut self.flash_clip),
            ("star_view", &mut self.star_view),
            ("tilt_view", &mut self.tilt_view),
            ("sky_view", &mut self.sky_view),
        ]
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
            binning: BinningPref::Bin3,
            ahead: MAX_PREFETCH,
            behind: 0,
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

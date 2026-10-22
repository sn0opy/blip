//! Links to AstroBin's image search. AstroBin keeps the search in the `p` URL parameter:
//! a MessagePack string, zlib compressed, then Base64 and percent encoded. It isn't a
//! documented API, so AstroBin may change it.

use crate::wcs::Wcs;
use flate2::{Compression, write::ZlibEncoder};
use std::io::Write;

const SEARCH_URL: &str = "https://app.astrobin.com/search?p=";
const ORDERING: &str = "-likes";
const NO_TEXT: &str = r#"{"value":"","matchType":"ALL","onlySearchInTitlesAndDescriptions":false}"#;
/// Matches focal lengths from f / 1.25 to f × 1.25.
const FOCAL_TOLERANCE: f64 = 0.25;

/// In degrees: the RA extent at the center's declination, and the Dec extent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldOfView {
    pub center: (f64, f64),
    pub width: f64,
    pub height: f64,
}

impl FieldOfView {
    /// Measured through the corners and edge midpoints, so a rotated frame is covered too.
    pub fn of(wcs: &Wcs, size: [usize; 2]) -> Self {
        let [w, h] = size.map(|s| s as f64);
        let (ra, dec) = wcs.pixel_to_sky(w / 2.0, h / 2.0);
        let (mut half_ra, mut half_dec) = (0.0_f64, 0.0_f64);
        for x in [0.0, w / 2.0, w] {
            for y in [0.0, h / 2.0, h] {
                let (r, d) = wcs.pixel_to_sky(x, y);
                half_ra = half_ra.max(((r - ra + 540.0).rem_euclid(360.0) - 180.0).abs());
                half_dec = half_dec.max((d - dec).abs());
            }
        }
        Self {
            center: (ra, dec),
            width: 2.0 * half_ra * dec.to_radians().cos(),
            height: 2.0 * half_dec,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Search {
    pub field: Option<FieldOfView>,
    pub focal_length_mm: Option<f64>,
    /// An object as AstroBin names it in its plate solves, like "M 42".
    pub subject: Option<String>,
}

/// `None` if the field of view can't be searched.
pub fn search_url(search: &Search) -> Option<String> {
    let packed = message_pack_string(&search_state(search)?);
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    let compressed = (encoder.write_all(&packed))
        .and_then(|()| encoder.finish())
        .expect("writing to a Vec doesn't fail");
    Some(format!(
        "{SEARCH_URL}{}",
        percent_encode(&base64(&compressed))
    ))
}

/// The search as a query string, before encoding.
fn search_state(search: &Search) -> Option<String> {
    let mut state = format!("text={}&ordering={ORDERING}", percent_encode(NO_TEXT));
    if let Some(subject) = &search.subject {
        let subjects = format!(r#"{{"value":[{}],"matchType":null}}"#, json_string(subject));
        state += &format!("&subjects={}", percent_encode(&subjects));
    }
    if let Some(field) = &search.field {
        state += &format!("&coords={}", percent_encode(&coordinates(field)?));
    }
    if let Some(focal) = search.focal_length_mm.filter(|f| *f > 0.0 && f.is_finite()) {
        let factor = 1.0 + FOCAL_TOLERANCE;
        // Whole millimeters, rounded as AstroBin's own Python code does.
        let focal_range = range(
            (focal / factor).round_ties_even(),
            (focal * factor).round_ties_even(),
        );
        state += &format!("&telescope_focal_length={}", percent_encode(&focal_range));
    }
    Some(state + "&page=1&pageSize=100")
}

/// AstroBin filters right ascension in minutes of time, 0 to 1440.
fn coordinates(field: &FieldOfView) -> Option<String> {
    let FieldOfView {
        center: (ra, dec),
        width,
        height,
    } = *field;
    let valid = (0.0..360.0).contains(&ra)
        && (-90.0..=90.0).contains(&dec)
        && width > 0.0
        && width <= 180.0
        && height > 0.0
        && height <= 180.0;
    if !valid {
        return None;
    }
    let cos_dec = dec.to_radians().cos().abs();
    let half_minutes = if cos_dec < 1e-6 {
        720.0
    } else {
        width * 2.0 / cos_dec
    };
    let mut ra_range = (ra * 4.0 - half_minutes, ra * 4.0 + half_minutes);
    // A range across 0h can't be expressed, so search all right ascensions.
    if ra_range.0 < 0.0 || ra_range.1 > 1440.0 {
        ra_range = (0.0, 1440.0);
    }
    let dec_range = (
        (dec - height / 2.0).max(-90.0),
        (dec + height / 2.0).min(90.0),
    );
    Some(format!(
        r#"{{"ra":{},"dec":{}}}"#,
        range(ra_range.0, ra_range.1),
        range(dec_range.0, dec_range.1)
    ))
}

fn range(min: f64, max: f64) -> String {
    format!(r#"{{"min":{},"max":{}}}"#, number(min), number(max))
}

/// Rounded to six decimals and without a fraction when whole, like AstroBin's own JSON.
fn number(value: f64) -> String {
    let rounded = (value * 1e6).round() / 1e6;
    if rounded == rounded.trunc() {
        (rounded as i64).to_string()
    } else {
        rounded.to_string()
    }
}

/// Everything but RFC 3986 unreserved characters.
fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded += &format!("%{b:02X}");
        }
    }
    encoded
}

fn json_string(text: &str) -> String {
    let mut json = String::from('"');
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                json.push('\\');
                json.push(c);
            }
            c if (c as u32) < 0x20 => json += &format!("\\u{:04x}", c as u32),
            c => json.push(c),
        }
    }
    json + "\""
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = (0..3).fold(0_u32, |g, i| g << 8 | *chunk.get(i).unwrap_or(&0) as u32);
        for i in 0..4 {
            encoded.push(if i <= chunk.len() {
                ALPHABET[(group >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    encoded
}

fn message_pack_string(text: &str) -> Vec<u8> {
    let len = text.len();
    let mut packed = match len {
        0..=31 => vec![0xA0 | len as u8],
        32..=0xFF => vec![0xD9, len as u8],
        0x100..=0xFFFF => [&[0xDA][..], &(len as u16).to_be_bytes()].concat(),
        _ => [&[0xDB][..], &(len as u32).to_be_bytes()].concat(),
    };
    packed.extend_from_slice(text.as_bytes());
    packed
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    // Expected states come from the Python implementation in slt-project-browser.
    const TEXT: &str = "text=%7B%22value%22%3A%22%22%2C%22matchType%22%3A%22ALL%22%2C%22onlySearchInTitlesAndDescriptions%22%3Afalse%7D";

    fn field_search(center: (f64, f64), size: (f64, f64), focal: Option<f64>) -> Search {
        Search {
            field: Some(FieldOfView {
                center,
                width: size.0,
                height: size.1,
            }),
            focal_length_mm: focal,
            ..Search::default()
        }
    }

    fn likes(rest: &str) -> String {
        format!("{TEXT}&ordering=-likes&coords={rest}&page=1&pageSize=100")
    }

    #[test]
    fn matches_the_python_implementation() {
        let state = search_state(&field_search((83.82, -5.39), (1.2, 0.8), Some(400.0)));
        assert_eq!(
            state.unwrap(),
            likes(
                "%7B%22ra%22%3A%7B%22min%22%3A332.869341%2C%22max%22%3A337.690659%7D%2C%22dec%22%3A%7B%22min%22%3A-5.79%2C%22max%22%3A-4.99%7D%7D\
                 &telescope_focal_length=%7B%22min%22%3A320%2C%22max%22%3A500%7D"
            )
        );
        let state = search_state(&field_search((201.365, -43.019), (0.5, 0.33), Some(1000.0)));
        assert_eq!(
            state.unwrap(),
            likes(
                "%7B%22ra%22%3A%7B%22min%22%3A804.09225%2C%22max%22%3A806.82775%7D%2C%22dec%22%3A%7B%22min%22%3A-43.184%2C%22max%22%3A-42.854%7D%7D\
                 &telescope_focal_length=%7B%22min%22%3A800%2C%22max%22%3A1250%7D"
            )
        );
    }

    #[test]
    fn searches_all_right_ascensions_across_0h() {
        let state = search_state(&field_search((1.5, 20.0), (3.0, 2.0), None));
        assert_eq!(
            state.unwrap(),
            likes(
                "%7B%22ra%22%3A%7B%22min%22%3A0%2C%22max%22%3A1440%7D%2C%22dec%22%3A%7B%22min%22%3A19%2C%22max%22%3A21%7D%7D"
            )
        );
    }

    #[test]
    fn clamps_at_the_pole_and_omits_a_zero_focal_length() {
        let state = search_state(&field_search((10.0, 89.9), (1.0, 1.0), Some(0.0))).unwrap();
        assert!(
            state.contains("%22min%22%3A89.4%2C%22max%22%3A90%7D"),
            "{state}"
        );
        assert!(!state.contains("telescope_focal_length"), "{state}");
    }

    #[test]
    fn searches_a_subject_like_astrobin_does() {
        let search = Search {
            subject: Some("M 42".into()),
            ..Search::default()
        };
        assert_eq!(
            search_state(&search).unwrap(),
            format!(
                "{TEXT}&ordering=-likes\
                 &subjects=%7B%22value%22%3A%5B%22M%2042%22%5D%2C%22matchType%22%3Anull%7D\
                 &page=1&pageSize=100"
            )
        );
    }

    #[test]
    fn rejects_invalid_fields() {
        for (center, size) in [
            ((360.0, 0.0), (1.0, 1.0)),
            ((10.0, 91.0), (1.0, 1.0)),
            ((10.0, 0.0), (0.0, 1.0)),
            ((10.0, 0.0), (1.0, 181.0)),
            ((f64::NAN, 0.0), (1.0, 1.0)),
        ] {
            assert_eq!(search_url(&field_search(center, size, None)), None);
        }
    }

    fn decode_base64(text: &str) -> Vec<u8> {
        let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let (mut group, mut bits, mut decoded) = (0_u32, 0, Vec::new());
        for c in text.chars().take_while(|c| *c != '=') {
            group = group << 6 | alphabet.find(c).unwrap() as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                decoded.push((group >> bits) as u8);
            }
        }
        decoded
    }

    fn decode_percent(text: &str) -> String {
        let mut decoded = String::new();
        let mut rest = text;
        while let Some(i) = rest.find('%') {
            decoded += &rest[..i];
            decoded.push(u8::from_str_radix(&rest[i + 1..i + 3], 16).unwrap() as char);
            rest = &rest[i + 3..];
        }
        decoded + rest
    }

    #[test]
    fn url_decodes_to_the_search_state() {
        let search = field_search((83.82, -5.39), (1.2, 0.8), Some(400.0));
        let url = search_url(&search).unwrap();
        let parameter = url.strip_prefix(SEARCH_URL).unwrap();
        assert!(
            !parameter.contains(['+', '/', '=', '&', '?']),
            "{parameter}"
        );
        let compressed = decode_base64(&decode_percent(parameter));
        let mut packed = Vec::new();
        ZlibDecoder::new(&compressed[..])
            .read_to_end(&mut packed)
            .unwrap();
        assert_eq!(packed, message_pack_string(&search_state(&search).unwrap()));
    }

    #[test]
    fn encodes_like_the_specifications() {
        assert_eq!(json_string("a\"b\\c\n"), r#""a\"b\\c\u000a""#);
        assert_eq!(
            ["", "f", "fo", "foo"].map(|s| base64(s.as_bytes())),
            ["", "Zg==", "Zm8=", "Zm9v"]
        );
        assert_eq!(base64(&[0xFF, 0xFE, 0xFD]), "//79");
        assert_eq!(
            percent_encode("aZ09-._~ /+=é"),
            "aZ09-._~%20%2F%2B%3D%C3%A9"
        );
        assert_eq!(message_pack_string("abc"), b"\xA3abc");
        assert_eq!(message_pack_string(&"x".repeat(32))[..2], [0xD9, 0x20]);
        assert_eq!(
            message_pack_string(&"x".repeat(256))[..3],
            [0xDA, 0x01, 0x00]
        );
    }

    #[test]
    fn measures_a_rotated_field() {
        let s = 1.0 / 3600.0;
        let size = [3600, 1800];
        let crpix = [1800.0, 900.0];
        let upright = Wcs {
            crval: [83.8, 0.0],
            crpix,
            cd: [[-s, 0.0], [0.0, -s]],
        };
        let sideways = Wcs {
            cd: [[0.0, -s], [-s, 0.0]],
            ..upright
        };
        let close = |a: f64, b: f64| (a - b).abs() < 1e-3;
        let field = FieldOfView::of(&upright, size);
        assert!(
            close(field.width, 1.0) && close(field.height, 0.5),
            "{field:?}"
        );
        let field = FieldOfView::of(&sideways, size);
        assert!(
            close(field.width, 0.5) && close(field.height, 1.0),
            "{field:?}"
        );
        assert!(close(field.center.0, 83.8) && close(field.center.1, 0.0));
    }
}

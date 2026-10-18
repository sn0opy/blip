//! Builds `data/dso.tsv` from the OpenNGC database (`NGC.csv`, `addendum.csv`).
//!
//! cargo run -p blip-core --features solve --example build_dso -- <out.tsv> <csv>...
//!
//! One object per line: label, RA and Dec (degrees), major and minor axis
//! (arcminutes), position angle (degrees), OpenNGC type, common name. Faint galaxies
//! without a common name are left out.

use blip_core::wcs::parse_sexagesimal;
use std::io::Write;

const SKIPPED_TYPES: [&str; 6] = ["*", "**", "Dup", "NonEx", "Other", "Nova"];
const MAX_UNNAMED_GALAXY_B_MAG: f64 = 14.0;
const MIN_UNNAMED_GALAXY_ARCMIN: f64 = 2.0;

/// "IC0434" → "IC 434"
fn label(name: &str) -> String {
    let split = name
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(name.len());
    let (prefix, number) = name.split_at(split);
    if number.is_empty() || name.contains('-') || !number.chars().all(|c| c.is_ascii_digit()) {
        return name.to_string();
    }
    format!("{prefix} {}", number.trim_start_matches('0'))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [out, inputs @ ..] = args.as_slice() else {
        eprintln!("usage: build_dso <out.tsv> <openngc.csv>...");
        std::process::exit(2);
    };
    let mut lines = Vec::new();
    for path in inputs {
        let text = std::fs::read_to_string(path).expect(path);
        for row in text.lines().skip(1) {
            let f: Vec<&str> = row.split(';').collect();
            let get = |i: usize| f.get(i).copied().unwrap_or("").trim();
            let num = |i: usize| get(i).parse::<f64>().ok();
            let (name, kind) = (get(0), get(1));
            if SKIPPED_TYPES.contains(&kind) {
                continue;
            }
            let (Some(ra), Some(dec)) = (parse_sexagesimal(get(2)), parse_sexagesimal(get(3)))
            else {
                continue;
            };
            let messier = get(23).trim_start_matches('0');
            let common = get(28).split(',').next().unwrap_or("").trim();
            let common = common.strip_prefix("the ").unwrap_or(common);
            let (major, minor) = (num(5).unwrap_or(0.0), num(6));
            if kind.starts_with('G')
                && messier.is_empty()
                && common.is_empty()
                && num(8).is_none_or(|b| b > MAX_UNNAMED_GALAXY_B_MAG)
                && major < MIN_UNNAMED_GALAXY_ARCMIN
            {
                continue;
            }
            let label = if messier.is_empty() {
                label(name)
            } else {
                format!("M {messier} ({})", label(name))
            };
            lines.push(format!(
                "{label}\t{:.4}\t{:.4}\t{major}\t{}\t{}\t{kind}\t{common}",
                ra * 15.0,
                dec,
                minor.unwrap_or(major),
                num(7).unwrap_or(0.0),
            ));
        }
    }
    let mut file = std::fs::File::create(out).expect(out);
    for l in &lines {
        writeln!(file, "{l}").expect("write");
    }
    println!("wrote {out}: {} objects", lines.len());
}

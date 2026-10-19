//! Builds `data/stars.bin` from Gaia DR3 CSV exports (see `scripts/build-catalog.sh`).
//!
//! cargo run --release -p blip-core --features solve --example build_catalog -- \
//!     <stars> <out.bin> <gaia.csv>...
//!
//! Keeps the brightest stars of every cell, with the per-cell quota chosen so the
//! catalog holds about `<stars>` stars.

use blip_core::catalog::{self, EPOCH_YEAR, MAGIC, ZONES};
use std::io::{BufRead, BufReader, Write};

const GAIA_EPOCH_YEAR: f64 = 2016.0;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [target, out, inputs @ ..] = args.as_slice() else {
        eprintln!("usage: build_catalog <stars> <out.bin> <gaia.csv>...");
        std::process::exit(2);
    };
    let target: usize = target.parse().expect("star count");
    let starts = catalog::zone_starts();
    let mut cells: Vec<Vec<(f32, u32)>> = vec![Vec::new(); starts[ZONES]];
    let dt = EPOCH_YEAR - GAIA_EPOCH_YEAR;
    let mut rows = 0usize;
    for path in inputs {
        let file = std::fs::File::open(path).expect(path);
        for line in BufReader::new(file).lines().skip(1) {
            let line = line.expect("read");
            let f: Vec<&str> = line.split(',').collect();
            let num = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok());
            let (Some(ra), Some(dec), Some(mag)) = (num(0), num(1), num(2)) else {
                continue;
            };
            const MAS_PER_DEG: f64 = 3.6e6;
            let pmra_cos_dec_mas_per_year = num(3).unwrap_or(0.0);
            let pmdec_mas_per_year = num(4).unwrap_or(0.0);
            let dec = (dec + pmdec_mas_per_year * dt / MAS_PER_DEG).clamp(-90.0, 90.0);
            let cos_dec = dec.to_radians().cos().max(1e-6);
            let ra =
                (ra + pmra_cos_dec_mas_per_year * dt / MAS_PER_DEG / cos_dec).rem_euclid(360.0);
            let (z, i) = catalog::cell_of(ra, dec);
            cells[starts[z] + i].push((mag as f32, catalog::encode(z, i, ra, dec, mag)));
            rows += 1;
        }
    }
    for c in &mut cells {
        c.sort_by(|a, b| a.0.total_cmp(&b.0));
    }

    let kept = |quota: usize| cells.iter().map(|c| c.len().min(quota)).sum::<usize>();
    let quota = (1..=255).find(|&q| kept(q) >= target).unwrap_or(255);
    let total = kept(quota);
    let short = cells.iter().filter(|c| c.len() < quota).count();
    let faintest = cells
        .iter()
        .filter_map(|c| c.get(quota.min(c.len()).checked_sub(1)?))
        .map(|s| s.0)
        .fold(f32::MIN, f32::max);
    println!(
        "{rows} input stars, {} cells, quota {quota}: {total} stars, {short} cells below quota, faintest kept G {faintest:.2}",
        cells.len()
    );

    let mut bytes = MAGIC.to_vec();
    bytes.extend(cells.iter().map(|c| c.len().min(quota) as u8));
    for c in &cells {
        for &(_, v) in c.iter().take(quota) {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    std::fs::File::create(out)
        .and_then(|mut f| f.write_all(&bytes))
        .expect(out);
    println!("wrote {out}: {} bytes", bytes.len());
}

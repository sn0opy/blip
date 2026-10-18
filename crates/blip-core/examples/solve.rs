//! Plate solve files from the command line and compare with their header WCS.
//!
//! cargo run --release -p blip-core --features solve --example solve -- <file>...

use blip_core::solve::{self, Hint};
use blip_core::wcs::{self, Wcs};
use blip_core::{image, process, stars};
use std::time::Instant;

fn main() {
    for path in std::env::args().skip(1) {
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let img = match image::read_file(path.as_ref()) {
            Ok(img) => img,
            Err(e) => {
                println!("{name}: {e}");
                continue;
            }
        };
        let t = Instant::now();
        let clip = process::clip_stats(&img);
        let st = stars::detect(&img, &clip);
        let detect_ms = t.elapsed().as_secs_f64() * 1000.0;
        let hint = Hint::from_header(&img);
        let (cx, cy) = (img.width as f64 / 2.0, img.height as f64 / 2.0);
        match solve::solve(&st.points_brightest_first, img.width, img.height, &hint) {
            Ok(sol) => {
                let (ra, dec) = sol.wcs.pixel_to_sky(cx, cy);
                let mut line = format!(
                    "{name}: RA {ra:.4} Dec {dec:.4} · {:.3}\"/px · north {:.1}°{} · {} matched · rms {:.2}\" · hint off {:.2}° · {} tiles · solve {:.0} ms (detect {detect_ms:.0} ms, {} points)",
                    sol.wcs.arcsec_per_px(),
                    sol.wcs.north_clockwise_from_up(),
                    if sol.wcs.mirrored() { " mirrored" } else { "" },
                    sol.matched_stars,
                    sol.rms_arcsec,
                    sol.hint_offset_deg,
                    sol.tiles_tried,
                    sol.ms,
                    st.points_brightest_first.len(),
                );
                if let Some(h) = Wcs::from_header(&img) {
                    let w = img.width as f64 - 1.0;
                    let hgt = img.height as f64 - 1.0;
                    let worst_of_corners_and_center =
                        [(0.0, 0.0), (w, 0.0), (0.0, hgt), (w, hgt), (cx, cy)]
                            .iter()
                            .map(|&(x, y)| {
                                let (a, b) = sol.wcs.pixel_to_sky(x, y);
                                let (c, d) = h.pixel_to_sky(x, y);
                                wcs::separation(a, b, c, d) * 3600.0
                            })
                            .fold(0.0, f64::max);
                    line += &format!(
                        " · vs header WCS: max {worst_of_corners_and_center:.1}\" ({:.3}\"/px, north {:.1}°)",
                        h.arcsec_per_px(),
                        h.north_clockwise_from_up()
                    );
                }
                println!("{line}");
            }
            Err(e) => println!(
                "{name}: FAILED: {e} (hint {:?}, {} points, detect {detect_ms:.0} ms)",
                hint,
                st.points_brightest_first.len()
            ),
        }
    }
}

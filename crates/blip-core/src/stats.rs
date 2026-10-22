use rayon::prelude::*;

/// MAD to standard deviation of a normal distribution.
pub(crate) const MAD_TO_SIGMA: f64 = 1.4826;

/// Median and MAD, normalized to 0..1.
pub(crate) fn median_mad(data: &[u16]) -> (f64, f64) {
    hist_median_mad(&histogram(data))
}

fn histogram(data: &[u16]) -> Vec<u32> {
    // Interleaved, so repeated background values don't serialize on one counter.
    data.par_chunks(1 << 20)
        .map(|chunk| {
            let mut h = vec![0u32; 4 << 16];
            let (quads, rest) = chunk.as_chunks::<4>();
            for q in quads {
                for (i, &v) in q.iter().enumerate() {
                    h[(i << 16) | v as usize] += 1;
                }
            }
            for &v in rest {
                h[v as usize] += 1;
            }
            let (first, others) = h.split_at_mut(1 << 16);
            for other in others.as_chunks::<65536>().0 {
                first.iter_mut().zip(other).for_each(|(a, b)| *a += b);
            }
            h.truncate(1 << 16);
            h
        })
        .reduce(|| vec![0; 1 << 16], sum_histograms)
}

pub(crate) fn hist_median_mad(hist: &[u32]) -> (f64, f64) {
    let n = hist.iter().map(|&c| c as u64).sum::<u64>();
    if n == 0 {
        return (0.0, 0.0);
    }
    let half = n.div_ceil(2);
    let med = hist_median(hist);

    let mut cum = hist[med] as u64;
    let mut mad = 0usize;
    if cum < half {
        for d in 1..65536usize {
            if med + d < 65536 {
                cum += hist[med + d] as u64;
            }
            if d <= med {
                cum += hist[med - d] as u64;
            }
            if cum >= half {
                mad = d;
                break;
            }
        }
    }
    (med as f64 / 65535.0, mad as f64 / 65535.0)
}

/// Lower median.
pub(crate) fn hist_median(hist: &[u32]) -> usize {
    let half = hist.iter().map(|&c| c as u64).sum::<u64>().div_ceil(2);
    let mut cum = 0u64;
    hist.iter()
        .position(|&c| {
            cum += c as u64;
            cum >= half.max(1)
        })
        .unwrap_or(0)
}

pub(crate) fn rgb_histograms(
    rows: &[usize],
    tasks: usize,
    fill: impl Fn(&[usize], &mut [u32]) + Sync,
) -> Vec<u32> {
    let per_task = rows.len().div_ceil(tasks).max(1);
    rows.par_chunks(per_task)
        .map(|ys| {
            let mut hist = vec![0u32; 3 << 16];
            fill(ys, &mut hist);
            hist
        })
        .reduce_with(sum_histograms)
        .unwrap_or_else(|| vec![0; 3 << 16])
}

pub(crate) fn sum_histograms(mut a: Vec<u32>, b: Vec<u32>) -> Vec<u32> {
    a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
    a
}

/// Exact `k`-th smallest of `f(v)` over every `step`-th value, by radix select.
pub(crate) fn select_nth(
    vals: &[f32],
    step: usize,
    mut k: usize,
    f: impl Fn(f32) -> f32 + Sync,
) -> f32 {
    // Orders like `f32::total_cmp`.
    let key = |v: f32| {
        let b = f(v).to_bits();
        b ^ ((((b as i32) >> 31) as u32) | 0x8000_0000)
    };
    // A multiple of `step` keeps the stride phase across chunks.
    let chunk = vals
        .len()
        .div_ceil(rayon::current_num_threads())
        .max(1 << 16)
        .next_multiple_of(step);
    let mut prefix = 0u32;
    for (shift, bits) in [(20, 12), (8, 12), (0, 8)] {
        let top = shift + bits;
        let mask = (1u32 << bits) - 1;
        let hist = vals
            .par_chunks(chunk)
            .map(|c| {
                let mut hist = vec![0u32; 1 << bits];
                for &v in c.iter().step_by(step) {
                    let key = key(v);
                    if (key as u64 >> top) as u32 == prefix {
                        hist[((key >> shift) & mask) as usize] += 1;
                    }
                }
                hist
            })
            .reduce_with(sum_histograms)
            .unwrap_or_default();
        let mut digit = 0;
        for (i, &c) in hist.iter().enumerate() {
            if c as usize > k {
                digit = i as u32;
                break;
            }
            k -= c as usize;
        }
        prefix = (prefix << bits) | digit;
    }
    f32::from_bits(if prefix >> 31 == 1 {
        prefix ^ 0x8000_0000
    } else {
        !prefix
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::testutil::noise;

    #[test]
    fn median_mad_exact() {
        assert_eq!(median_mad(&[]), (0.0, 0.0));
        assert_eq!(median_mad(&[7; 9]), (7.0 / 65535.0, 0.0));
        // Median 3; deviations 2, 1, 0, 1, 97 -> MAD 1.
        assert_eq!(
            median_mad(&[1, 2, 3, 4, 100]),
            (3.0 / 65535.0, 1.0 / 65535.0)
        );
        assert_eq!(median_mad(&[0, 65535, 65535]), (1.0, 0.0));
    }

    #[test]
    fn select_nth_matches_sorting() {
        let vals: Vec<f32> = (0..10_001)
            .map(|i| (noise(i, 20_000) as f32 - 5_000.0) / 7.0)
            .collect();
        for step in [1, 5] {
            let mut s: Vec<f32> = vals.iter().step_by(step).copied().collect();
            s.sort_by(f32::total_cmp);
            for k in [0, 1, s.len() / 3, s.len() / 2, s.len() - 1] {
                assert_eq!(select_nth(&vals, step, k, |v| v), s[k]);
            }
            let med = s[s.len() / 2];
            let mut dev: Vec<f32> = s.iter().map(|v| (v - med).abs()).collect();
            dev.sort_by(f32::total_cmp);
            let k = dev.len() / 2;
            assert_eq!(select_nth(&vals, step, k, |v| (v - med).abs()), dev[k]);
        }
    }
}

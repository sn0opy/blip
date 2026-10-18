#!/usr/bin/env bash
# Rebuild the plate solving data in crates/blip-core/data from the source catalogs:
# Gaia DR3 stars brighter than G 12.5 (~4.8M, ~400 MB of CSV) and OpenNGC.
#
#   scripts/build-catalog.sh [work-dir] [stars]
set -euo pipefail

work=${1:-target/catalog}
stars=${2:-1200000}
tap=https://gea.esac.esa.int/tap-server/tap
openngc=https://raw.githubusercontent.com/mattiaverga/OpenNGC/master/database_files
data=crates/blip-core/data
mkdir -p "$work"

# Gaia: one async TAP job per declination band (each well below the row limit).
bands=("-90 -60" "-60 -30" "-30 0" "0 30" "30 60" "60 91")
jobs=()
for band in "${bands[@]}"; do
    read -r lo hi <<<"$band"
    query="SELECT ra,dec,phot_g_mean_mag,pmra,pmdec FROM gaiadr3.gaia_source WHERE phot_g_mean_mag<12.5 AND dec>=$lo AND dec<$hi"
    job=$(curl -s -i -X POST "$tap/async" \
        --data-urlencode REQUEST=doQuery --data-urlencode LANG=ADQL \
        --data-urlencode FORMAT=csv --data-urlencode PHASE=RUN \
        --data-urlencode MAXREC=3000000 --data-urlencode "QUERY=$query" |
        tr -d '\r' | awk 'tolower($1) == "location:" { print $2; exit }')
    echo "dec [$lo, $hi): $job"
    jobs+=("$job")
done
csvs=()
for i in "${!jobs[@]}"; do
    job=${jobs[$i]}
    until [ "$(curl -s "$job/phase")" = COMPLETED ]; do
        phase=$(curl -s "$job/phase")
        case $phase in ERROR | ABORTED) echo "$job: $phase" >&2; exit 1 ;; esac
        sleep 10
    done
    csvs+=("$work/gaia-$i.csv")
    curl -s -L --compressed -o "$work/gaia-$i.csv" "$job/results/result"
done

curl -s -L -o "$work/NGC.csv" "$openngc/NGC.csv"
curl -s -L -o "$work/addendum.csv" "$openngc/addendum.csv"

cargo run --release -p blip-core --features solve --example build_catalog -- \
    "$stars" "$data/stars.bin" "${csvs[@]}"
cargo run --release -p blip-core --features solve --example build_dso -- \
    "$data/dso.tsv" "$work/NGC.csv" "$work/addendum.csv"

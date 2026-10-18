# Plate solving data

Embedded by the `solve` feature of `blip-core`. Rebuild both files with
`scripts/build-catalog.sh`.

## `stars.bin`

The 30 brightest stars of every ~1 deg² cell of the sky, positions
propagated to 2026.0, from [Gaia DR3](https://www.cosmos.esa.int/web/gaia/dr3)
(stars brighter than G 12.5). The format is described in `src/catalog.rs`.

This work has made use of data from the European Space Agency (ESA) mission Gaia
(https://www.cosmos.esa.int/gaia), processed by the Gaia Data Processing and Analysis
Consortium (DPAC, https://www.cosmos.esa.int/web/gaia/dpac/consortium). Funding for the
DPAC has been provided by national institutions, in particular the institutions
participating in the Gaia Multilateral Agreement.

## `dso.tsv`

Deep-sky objects (Messier, NGC, IC and the addendum; faint galaxies without a common
name left out) from [OpenNGC](https://github.com/mattiaverga/OpenNGC) by Mattia
Verga, licensed under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/).

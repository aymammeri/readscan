# readscan

Reads a scanned PDF of two-sided documents (page 1: name form, page 2: check with RIB),
OCRs every page and checks each document against a spreadsheet of `full name` / `rib`.

Pure Rust: [hayro](https://crates.io/crates/hayro) renders PDF pages, [ocrs](https://crates.io/crates/ocrs) does OCR,
[calamine](https://crates.io/crates/calamine) reads `.xlsx/.xls/.ods` (`.csv` also accepted), [rayon](https://crates.io/crates/rayon) runs documents in parallel.

## Setup

```sh
mkdir -p models
curl -L https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten   -o models/text-detection.rten
curl -L https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.rten -o models/text-recognition.rten
cargo build --release
```

Models are looked up in `$READSCAN_MODELS`, `./models`, `models/` next to the binary, then `~/.cache/ocrs`.

## Usage

```sh
./target/release/readscan
Spreadsheet path: clients.xlsx
Scanned PDF path: scans.pdf
```

Pages are paired in order: (1,2), (3,4), ... Both sides of a document are OCR'd at once,
and many documents are processed concurrently on every logical CPU of the machine
(override with `RTEN_NUM_THREADS=4`).

## Windows

Every push to `master` builds `readscan-windows-x64` (exe + models) in the repo's Actions tab;
pushing a `v*` tag also attaches it as a zip to a GitHub release. Unzip, keep `models\` next to
`readscan.exe`, and double-click it or run it from a terminal. On Windows, paste paths with
"Copy as path" (the quotes are fine). Peak memory is roughly 200 MB per thread.

## Results

- **green**: name found and the check shows one of that person's RIBs
- **red**: name found but no RIB of that person is on the check
- **yellow**: page unreadable, name not found / ambiguous, or no RIB readable on the check
- **dim**: spreadsheet row with no document in the PDF

Results are also written into the spreadsheet:

- first sheet: every row colored, plus `Scan result`, `Scan pages`, `Scan RIB read`, `Scan details` columns
- `Scan summary` sheet: run info, totals per color, notes
- `Scan documents` sheet: every scanned document, including ones whose name matched no row

`.xlsx` files are updated in place and the previous version is kept as `<name>.backup.xlsx`.
`.csv`, `.xls`, `.ods`, `.xlsm` and `.xlsb` are left untouched and results go to a new `<name>_results.xlsx`.
If the file can't be overwritten (e.g. open in Excel), results also go to `<name>_results.xlsx`.
Re-running on the same file reuses the result columns and replaces the two scan sheets.

The spreadsheet's first sheet is used. Columns are found by header (`name`/`nom`, `rib`/`rip`);
otherwise column A is the name and column B the RIB. Keep the RIB column formatted as **Text**
in Excel, numbers longer than 15 digits lose precision.

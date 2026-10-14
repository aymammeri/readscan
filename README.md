# readscan

Reads a scanned PDF of two-sided documents (page 1: name form, page 2: check with RIB),
OCRs every page and checks each document against a spreadsheet of `full name` / `rib`.

Pure Rust: [hayro](https://crates.io/crates/hayro) renders PDF pages, [ocrs](https://crates.io/crates/ocrs) does OCR,
[calamine](https://crates.io/crates/calamine) reads `.xlsx/.xls/.ods` (`.csv` also accepted), [rayon](https://crates.io/crates/rayon) runs documents in parallel.

## Build from source

The OCR models are built into the program, so download them into `models/` first:

```sh
mkdir -p models
curl -L https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten   -o models/text-detection.rten
curl -L https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.rten -o models/text-recognition.rten
cargo build --release
```

## Usage

```sh
./target/release/readscan
Spreadsheet path: clients.xlsx
Scanned PDF path: scans.pdf
```

Pages are paired in order: (1,2), (3,4), ... Both sides of a document are OCR'd at once,
and many documents are processed concurrently on every logical CPU of the machine
(override with `RTEN_NUM_THREADS=4`).

## Download

Ready-to-run programs are on the [Releases](https://github.com/aymammeri/readscan/releases) page,
one file per system, with the OCR models built in:

- Windows: `readscan-windows-x64.exe`, double-click it (SmartScreen: *More info* > *Run anyway*).
- macOS (Apple Silicon and Intel): `readscan-macos-universal`; once, run
  `chmod +x readscan-macos-universal && xattr -d com.apple.quarantine readscan-macos-universal`,
  then `./readscan-macos-universal`.

Pushing a `v*` tag builds both and publishes the release.

## How a scan is checked (rules, in order)

**1. Spreadsheet.** First sheet; name and RIB columns found by header (`name`/`nom`, `rib`/`rip`),
otherwise A and B. A RIB is 20 digits: letters cut it and only the digits after the last letters
count (`025DZD004...` keeps `004...`). Rows whose RIB is not 20 digits are warned about.
Rows with the same name are one person with several accounts.

**2. Reading pages.** Pages are OCR'd in page order (1, 2, 3, ...), each free thread taking the
next page. On every page, OCR lines on the same printed row are joined left to right, so a RIB
printed in segments with big gaps reads as one row.

**3. Pairing pages.** Name pages carry the title **QUITTANCE DE REGLEMENT**; checks never do.
Pages pair as (1,2), (3,4), ... until a title page shows up where a check should be:

- title page then title page: **check missing**, the first is kept alone (PAGE ORDER)
- check, its title page, next title page: **sides swapped**, re-paired and checked normally
- check before a title page that has its own check: **extra page / name page missing**, set aside
  (PAGE ORDER) and identified by its RIB when at least 15 digits match

Pairing then resumes at the next title page. A name page whose title OCR misses never shifts the
pairing. Every issue is listed with its pages in the terminal and on the `Scan summary` sheet.

**4. Who the document is for.** The name page is looked up in the spreadsheet (accents, word order
and small OCR typos tolerated). If the page is unreadable, or the name matches nobody or several
people, the check's RIB decides when all 20 digits, or at least 15, match one of them. Such a
document is never better than PARTIAL MATCH ("check the name manually"). Otherwise it is UNREADABLE.

**5. The RIB on the check**, against that person's RIBs only:

1. each row is cut into digit pieces: spaces and `- . / :` are ignored; letters and garbage symbols
   (`#`, `~`, `*`...) cut the number; a lone look-alike letter touching digits (`O`, `I`, `S`, `B`...)
   is read as a digit
2. **MATCH**: all 20 digits found, in one piece, in a row's pieces joined, or over two rows joined
3. **PARTIAL MATCH**: otherwise each row's pieces are laid on the RIB where they share the longest
   run of digits (from the left, the right or the middle); **4 or more matching digits, at any
   position**, is a partial match, flagged for a manual check
4. **MISMATCH**: otherwise, a clean 20-digit number was read and fewer than 4 of its digits match
5. **UNREADABLE**: otherwise; whatever digits were read are shown for a manual check

**6. Rotated pages.** Only a document that fails gets this step, and only for the page that
failed. Pages that are clearly upright are never re-read: a name page with a readable title, or a
check with check wording ("Payez", "chèque", "banque") or RIB digits matching in place.

- tilted more than 10° (measured from the word boxes): flagged, not re-read (turning can't fix it)
- on its side (words run top to bottom): read turned left and right
- otherwise: read upside down once

A turn is kept only for a full match, a found name, or 10+ matching digits. Then the result stays
and the page is flagged ("Page 2 upside down. Read turned. Rescan it."). If the document still
fails on a rotated or tilted page, it is **ROTATED** ("Page 8 tilted 45°. Rescan it.").

**7. Spreadsheet rows.** A row takes the best document that matched its RIB (MATCH over PARTIAL
MATCH), and mentions it when several documents matched it (duplicate scan?). A row with no match
takes the worst result among that person's other documents; with none it is NOT IN PDF.

**8. Digits, bulls and cows.** The RIB cell, `Scan RIB read` and the terminal color every digit:
**green** right digit in the right place, **orange** a digit of the RIB read in another place,
**red** a digit not in the RIB, **grey** not readable (`?`). `Scan details` ends with the counts,
e.g. "Digits: 13 right, 0 misplaced, 0 wrong, 7 not read".

## Results

- **green (MATCH)**: the check shows all 20 digits of one of that person's RIBs
- **blue (PARTIAL MATCH)**: 4 or more digits match, or the person was found through the check; check manually
- **red (MISMATCH)**: a clean 20-digit RIB on the check that is not the person's
- **purple (ROTATED)**: a page is upside down, on its side or tilted; rescan it
- **orange (PAGE ORDER)**: pages out of physical order, document incomplete
- **yellow (UNREADABLE)**: page or RIB not readable, or name not found; check manually
- **grey (NOT IN PDF)**: spreadsheet row with no document in the PDF

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

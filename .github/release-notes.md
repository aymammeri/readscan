Checks scanned QUITTANCE + check pages against a spreadsheet of names and RIBs.

## Download

- **Windows**: `readscan-windows-x64.zip`
- **macOS** (Apple Silicon and Intel): `readscan-macos-universal.zip`

Each zip has the program, its `models` folder and the README. Keep `models` next to the program.

## Run

**Windows**: unzip, double-click `readscan.exe`. If SmartScreen warns, click *More info* > *Run anyway*.

**macOS**: unzip, then in Terminal, in the unzipped folder:

```sh
xattr -dr com.apple.quarantine .
./readscan
```

The first line is needed once: macOS blocks downloaded programs that are not from the App Store.

The program asks for the spreadsheet, then the PDF, and writes the results into the spreadsheet
(a backup of the previous version is kept).

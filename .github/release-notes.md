Checks scanned QUITTANCE + check pages against a spreadsheet of names and RIBs.
One standalone file per machine, nothing to install.

| Machine | File |
|---|---|
| Windows (Intel / AMD, 64-bit) | `readscan-windows-x64.exe` |
| Mac with Apple Silicon (M1, M2, M3...) | `readscan-macos-apple-silicon` |
| Mac with Intel processor | `readscan-macos-intel` |

**Windows**: double-click the `.exe`. If SmartScreen warns: *More info* > *Run anyway*.

**macOS**: in Terminal, once (macOS blocks downloaded programs):

```sh
cd ~/Downloads
chmod +x readscan-macos-*
xattr -d com.apple.quarantine readscan-macos-*
```

Then run `./readscan-macos-apple-silicon` or `./readscan-macos-intel`.
Not sure which Mac you have: Apple menu > *About This Mac* > *Chip* (Apple M...) or *Processor* (Intel).

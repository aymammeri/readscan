Checks scanned QUITTANCE + check pages against a spreadsheet of names and RIBs.
One file per system, nothing to install.

**Windows**: download `readscan-windows-x64.exe`, double-click it.
If SmartScreen warns: *More info* > *Run anyway*.

**macOS** (Apple Silicon and Intel): download `readscan-macos-universal`, then in Terminal:

```sh
cd ~/Downloads
chmod +x readscan-macos-universal
xattr -d com.apple.quarantine readscan-macos-universal
./readscan-macos-universal
```

The first two commands are needed once (macOS blocks downloaded programs).

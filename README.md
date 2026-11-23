# Task Manager Remake

A cleaner, more overseeable replacement for the Windows 11 Task Manager.

- **Overview**: CPU, memory, GPU and network at a glance, the apps using the most memory, drives.
- **Apps**: processes grouped by app (a browser's 29 processes are one row), sortable by CPU,
  memory, GPU, disk and network, with search, end task, end process tree and open file location.
- **Performance**: live graphs for the CPU (overall or per thread), memory, GPU, network and every
  drive. Temperatures sit right in each part's card.
- **Startup**: turn startup apps on or off, with the boot impact Windows measured.
- **Services**: start and stop Windows services.
- **Settings**: open this app instead of Windows Task Manager (Ctrl+Shift+Esc).

## Install

Download the installer from the latest [release](../../releases). The app always runs as
administrator (ending system processes, services and per-app network need it).

The installer also offers the free, signed [PawnIO](https://pawnio.eu) driver, which Windows needs
to read an AMD Ryzen CPU temperature. GPU and NVMe drive temperatures work without it.

## Build

Requires Rust (stable), Node.js and the WebView2 runtime.

```
npm ci
bash tools/fetch-pawnio.sh
cd src-tauri && cargo test --lib && cd ..
npx tauri build
```

`TM_DEV_AS_INVOKER=1 cargo build` makes a non-elevated build for UI testing; never ship it.

## Licenses

`src-tauri/resources/pawnio/AMDFamily17.bin` is from PawnIO.Modules (LGPL-2.1-or-later, see
`COPYING` next to it). The Geist font is under the SIL Open Font License (`src/fonts/OFL.txt`).

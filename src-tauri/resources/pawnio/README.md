# PawnIO

`AMDFamily17.bin` and `COPYING` come from PawnIO.Modules 0.2.12
(https://github.com/namazso/PawnIO.Modules, LGPL-2.1-or-later, source: `AMDFamily17.p`).
The app loads this signed module into the PawnIO driver to read the AMD CPU temperature.

`PawnIO_setup.exe` (the official signed driver installer; its author allows redistributing it)
is not checked in. `tools/fetch-pawnio.sh` downloads it with a pinned checksum before a build.

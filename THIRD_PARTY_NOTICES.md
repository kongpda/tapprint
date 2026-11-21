# Third-party notices

TapPrint's own code is under the [MIT licence](LICENSE). Each build also
contains open source code written by others. This file lists what you need to
know when you share TapPrint or a build of it.

The full licence text of every Rust crate in a build is in
`THIRD-PARTY-LICENSES.html`, attached to each
[release](https://github.com/kongpda/tapprint/releases). To make it yourself:

```shell
cargo install cargo-about --features cli
cargo about generate about.hbs -o THIRD-PARTY-LICENSES.html
```

`cargo deny check licenses` checks every dependency against the list in
[`deny.toml`](deny.toml). CI runs it on every push.

## libusb (LGPL-2.1-or-later)

TapPrint talks to USB printers through [libusb](https://libusb.info) 1.0.27.
The `libusb1-sys` crate builds it from source and links it into the TapPrint
program, so the printer works without installing anything else.

libusb is free software under the GNU Lesser General Public License,
version 2.1 or later. The licence text is in
[`licenses/libusb-LGPL-2.1.txt`](licenses/libusb-LGPL-2.1.txt), and copies of
TapPrint carry it in the download.

- Copyright © 2001 Johannes Erdfelt, © 2007-2009 Daniel Drake,
  © 2010-2012 Peter Stuge, © 2008-2016 Nathan Hjelm, © 2009-2013 Pete
  Batard, © 2009-2013 Ludovic Rousseau, © 2010-2012 Michael Plante,
  © 2011-2013 Hans de Goede, © 2012-2013 Martin Pieuchot, © 2012-2013 Toby
  Gray, © 2013-2018 Chris Dickens, and the other authors in libusb's
  `AUTHORS` file, copied to
  [`licenses/libusb-AUTHORS.txt`](licenses/libusb-AUTHORS.txt).
- Each GitHub release provides the exact `libusb1-sys` crate used to build it
  in `libusb1-sys-source.tar.gz`. The crate includes the libusb source in
  `libusb/`. TapPrint's source and lockfile are in the source archive for
  the same release tag.
- To rebuild with a modified libusb, download both archives, extract them,
  and edit the library under `libusb/`. Add this to the workspace root
  `Cargo.toml`, using the path to the extracted crate:

  ```toml
  [patch.crates-io]
  libusb1-sys = { path = "../libusb1-sys-0.7.0" }
  ```

  Run `cargo update -p libusb1-sys --offline` after fetching the original
  dependencies, then follow [Building from source](docs/building.md).

If you share a TapPrint build, share these notices with it, keep this source
available, and do not forbid people from changing or reverse engineering the
program for their own use.

## Other licences in a build

Most crates are under MIT or Apache-2.0. These are the others, all
compatible with shipping TapPrint under MIT. None requires TapPrint's own
code to change licence.

| Licence | Crates (examples) | What it asks |
| --- | --- | --- |
| BSD-2-Clause, BSD-3-Clause, ISC, Zlib | `rav1e`, `exr`, `libloading`, `foldhash`, `ring` (with Apache-2.0) | Keep the copyright notice and licence text. |
| MPL-2.0 | `dwrote` (Windows text), `option-ext` | Unchanged, used as is. If you change these files, share those files' source. |
| Unicode-3.0 | `icu_*`, `litemap`, `zerovec` and others | Keep the notice. |
| CDLA-Permissive-2.0 | `webpki-roots` (root certificates for update checks) | Keep the notice. |
| CC0-1.0, Unlicense | `hexf-parse`, `memchr` and others | Nothing. |

`THIRD-PARTY-LICENSES.html` has every text in full.

## Trademarks

EPSON and ESC/POS are registered trademarks of Seiko Epson Corporation.
TapPrint uses the name ESC/POS only to say which printer command language it
speaks. TapPrint is not made, endorsed or supported by Seiko Epson.

macOS and Safari are trademarks of Apple Inc. Windows is a trademark of the
Microsoft group of companies. GitHub is a trademark of GitHub, Inc. Other
names are trademarks of their owners. Naming them does not mean they endorse
TapPrint.

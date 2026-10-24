# Third-party notices and rights

## Tool code

The KANADE voice dataset builder's own source code has no license grant in this repository. Do not infer a source-code license from the dependency notices below.

## Direct dependencies

The Cargo package uses third-party crates under their respective licenses. Cargo.lock records the resolved versions. In particular:

| Component | Version | License / notice |
| --- | --- | --- |
| `ds_parser` | 0.4.0 | BlueOak-1.0.0; see the upstream [Blue Oak Model License](https://blueoakcouncil.org/license/1.0.0) and crate distribution. |
| `emote-psb` | 0.8.0 | MIT; upstream copyright and license notice reproduced below. |

The other direct dependencies are `clap`, `csv`, `serde`, `serde_json`, `sha2`, `smol_str`, `thiserror`, `unicode-normalization`, and `walkdir`; test-only dependencies are `base64` and `serde_yaml`. Their own package licenses and notices continue to apply. This file does not relicense any dependency.

### `emote-psb` MIT notice

Copyright (c) 2026 storycraft

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

The synthetic Finder fixture at `tests/fixtures/finder-macos.ds_store.b64` comes from `dmgbuild/ds_store` and is MIT licensed. Its exact provenance, copyright, and full MIT notice are preserved in [tests/fixtures/README.md](tests/fixtures/README.md).

## Game content

KANADE game scripts, scenarios, audio, other assets, and any derived dataset are separate from this tool and its dependencies. None are included here. Their rights remain with their respective owners; no game-content rights are granted by this repository.

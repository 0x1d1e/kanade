# Third-party notices

Kanade's own source is under the [MIT License](LICENSE). It vendors no third-party code, fonts or artwork: the icons in `src/icons/` are drawn for Kanade. The crates below are fetched by Cargo, each under its own license, and are linked into the `kanade` binary. Anyone distributing that binary must include their copyright and license notices, which each crate ships in its source.

## Amane

Kanade is built on [Amane](https://github.com/MystiaFin/amane), pinned in `Cargo.toml`.

```
MIT License

Copyright (c) 2026 MystiaFin

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
```

## Crates

Every crate the binary links on Linux, as of `Cargo.lock`. After a dependency change, list them again with:

```sh
cargo tree -e normal --target x86_64-unknown-linux-gnu --prefix none -f '{p}|{l}|{r}' --locked | sort -u
```

| Crate | Version | License | Source |
|---|---|---|---|
| adler2 | v2.0.1 | 0BSD OR MIT OR Apache-2.0 | https://github.com/oyvindln/adler2 |
| allocator-api2 | v0.2.21 | MIT OR Apache-2.0 | https://github.com/zakarumych/allocator-api2 |
| amane | v0.1.1 | MIT | https://github.com/MystiaFin/amane |
| arrayref | v0.3.9 | BSD-2-Clause | https://github.com/droundy/arrayref |
| arrayvec | v0.7.8 | MIT OR Apache-2.0 | https://github.com/bluss/arrayvec |
| ash | v0.38.0+1.3.281 | MIT OR Apache-2.0 | https://github.com/ash-rs/ash |
| async-broadcast | v0.7.2 | MIT OR Apache-2.0 | https://github.com/smol-rs/async-broadcast |
| async-channel | v2.5.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/async-channel |
| async-executor | v1.14.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/async-executor |
| async-io | v2.6.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/async-io |
| async-lock | v3.4.2 | Apache-2.0 OR MIT | https://github.com/smol-rs/async-lock |
| async-process | v2.5.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/async-process |
| async-recursion | v1.2.0 | MIT OR Apache-2.0 | https://github.com/dcchut/async-recursion |
| async-signal | v0.2.14 | Apache-2.0 OR MIT | https://github.com/smol-rs/async-signal |
| async-task | v4.7.1 | Apache-2.0 OR MIT | https://github.com/smol-rs/async-task |
| async-trait | v0.1.92 | MIT OR Apache-2.0 | https://github.com/dtolnay/async-trait |
| atomic-waker | v1.1.2 | Apache-2.0 OR MIT | https://github.com/smol-rs/atomic-waker |
| bitflags | v2.13.2 | MIT OR Apache-2.0 | https://github.com/bitflags/bitflags |
| bit-set | v0.9.1 | Apache-2.0 OR MIT | https://github.com/contain-rs/bit-set |
| bit-vec | v0.9.1 | Apache-2.0 OR MIT | https://github.com/contain-rs/bit-vec |
| blocking | v1.7.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/blocking |
| bytemuck_derive | v1.12.1 | Zlib OR Apache-2.0 OR MIT | https://github.com/Lokathor/bytemuck |
| bytemuck | v1.25.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/Lokathor/bytemuck |
| calloop | v0.14.5 | MIT | https://github.com/Smithay/calloop |
| calloop-wayland-source | v0.4.1 | MIT | https://github.com/smithay/calloop-wayland-source |
| cfg-if | v1.0.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/cfg-if |
| codespan-reporting | v0.13.1 | Apache-2.0 | https://github.com/brendanzab/codespan |
| color | v0.3.3 | Apache-2.0 OR MIT | https://github.com/linebender/color |
| concurrent-queue | v2.5.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/concurrent-queue |
| crc32fast | v1.5.2 | MIT OR Apache-2.0 | https://github.com/srijs/rust-crc32fast |
| crossbeam-utils | v0.8.23 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| cursor-icon | v1.2.0 | MIT OR Apache-2.0 OR Zlib | https://github.com/rust-windowing/cursor-icon |
| data-url | v0.3.2 | MIT OR Apache-2.0 | https://github.com/servo/rust-url |
| dlib | v0.5.3 | MIT | https://github.com/elinorbgr/dlib |
| document-features | v0.2.12 | MIT OR Apache-2.0 | https://github.com/slint-ui/document-features |
| downcast-rs | v1.2.1 | MIT/Apache-2.0 | https://github.com/marcianx/downcast-rs |
| endi | v1.1.1 | MIT | https://github.com/zeenix/endi |
| enumflags2_derive | v0.7.12 | MIT OR Apache-2.0 | https://github.com/meithecatte/enumflags2 |
| enumflags2 | v0.7.12 | MIT OR Apache-2.0 | https://github.com/meithecatte/enumflags2 |
| equivalent | v1.0.2 | Apache-2.0 OR MIT | https://github.com/indexmap-rs/equivalent |
| errno | v0.3.14 | MIT OR Apache-2.0 | https://github.com/lambda-fairy/rust-errno |
| euclid | v0.22.14 | MIT OR Apache-2.0 | https://github.com/servo/euclid |
| event-listener-strategy | v0.5.4 | Apache-2.0 OR MIT | https://github.com/smol-rs/event-listener-strategy |
| event-listener | v5.4.2 | Apache-2.0 OR MIT | https://github.com/smol-rs/event-listener |
| fastrand | v2.5.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/fastrand |
| fdeflate | v0.3.7 | MIT OR Apache-2.0 | https://github.com/image-rs/fdeflate |
| flate2 | v1.1.10 | MIT OR Apache-2.0 | https://github.com/rust-lang/flate2-rs |
| float-cmp | v0.9.0 | MIT | https://github.com/mikedilger/float-cmp |
| foldhash | v0.1.5 | Zlib | https://github.com/orlp/foldhash |
| foldhash | v0.2.0 | Zlib | https://github.com/orlp/foldhash |
| fontconfig | v0.11.0 | MIT | https://github.com/yeslogic/fontconfig-rs |
| font-types | v0.12.6 | MIT OR Apache-2.0 | https://github.com/googlefonts/fontations |
| futures-core | v0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-intrusive | v0.5.0 | MIT OR Apache-2.0 | https://github.com/Matthias247/futures-intrusive |
| futures-io | v0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-lite | v2.6.1 | Apache-2.0 OR MIT | https://github.com/smol-rs/futures-lite |
| glow | v0.17.0 | MIT OR Apache-2.0 OR Zlib | https://github.com/grovesNL/glow |
| gpu-allocator | v0.28.0 | MIT OR Apache-2.0 | https://github.com/Traverse-Research/gpu-allocator |
| gpu-descriptor-types | v0.2.0 | MIT OR Apache-2.0 | https://github.com/zakarumych/gpu-descriptor |
| gpu-descriptor | v0.3.2 | MIT OR Apache-2.0 | https://github.com/zakarumych/gpu-descriptor |
| guillotiere | v0.7.0 | MIT/Apache-2.0 | https://github.com/nical/guillotiere |
| half | v2.7.1 | MIT OR Apache-2.0 | https://github.com/VoidStarKat/half-rs |
| hashbrown | v0.15.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| hashbrown | v0.16.1 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| hashbrown | v0.17.1 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| hexf-parse | v0.2.1 | CC0-1.0 | https://github.com/lifthrasiir/hexf |
| hex | v0.4.3 | MIT OR Apache-2.0 | https://github.com/KokaKiwi/rust-hex |
| imagesize | v0.15.0 | MIT | https://github.com/Roughsketch/imagesize |
| indexmap | v2.14.2 | Apache-2.0 OR MIT | https://github.com/indexmap-rs/indexmap |
| inotify-sys | v0.1.8 | ISC | https://github.com/hannobraun/inotify-sys |
| inotify | v0.11.5 | ISC | https://github.com/hannobraun/inotify-rs |
| itoa | v1.0.18 | MIT OR Apache-2.0 | https://github.com/dtolnay/itoa |
| khronos-egl | v6.0.0 | MIT/Apache-2.0 | https://github.com/timothee-haudebourg/khronos-egl |
| kurbo | v0.13.1 | Apache-2.0 OR MIT | https://github.com/linebender/kurbo |
| libc | v0.2.190 | MIT OR Apache-2.0 | https://github.com/rust-lang/libc |
| libloading | v0.8.9 | ISC | https://github.com/nagisa/rust_libloading/ |
| libm | v0.2.16 | MIT | https://github.com/rust-lang/compiler-builtins |
| libpulse-binding | v2.30.1 | MIT OR Apache-2.0 | https://github.com/jnqnfe/pulse-binding-rust |
| libpulse-sys | v1.23.0 | MIT OR Apache-2.0 | https://github.com/jnqnfe/pulse-binding-rust |
| linebender_resource_handle | v0.1.1 | Apache-2.0 OR MIT | https://github.com/linebender/raw_resource_handle |
| linux-raw-sys | v0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/sunfishcode/linux-raw-sys |
| litrs | v1.0.0 | MIT OR Apache-2.0 | https://github.com/LukasKalbertodt/litrs |
| lock_api | v0.4.14 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |
| log | v0.4.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/log |
| memchr | v2.8.3 | Unlicense OR MIT | https://github.com/BurntSushi/memchr |
| memmap2 | v0.9.11 | MIT OR Apache-2.0 | https://github.com/RazrFalcon/memmap2-rs |
| miniz_oxide | v0.8.9 | MIT OR Zlib OR Apache-2.0 | https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide |
| miniz_oxide | v0.9.1 | MIT OR Zlib OR Apache-2.0 | https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide |
| naga | v29.0.4 | MIT OR Apache-2.0 | https://github.com/gfx-rs/wgpu |
| num-derive | v0.4.2 | MIT OR Apache-2.0 | https://github.com/rust-num/num-derive |
| num-traits | v0.2.19 | MIT OR Apache-2.0 | https://github.com/rust-num/num-traits |
| once_cell | v1.21.4 | MIT OR Apache-2.0 | https://github.com/matklad/once_cell |
| ordered-float | v5.5.0 | MIT | https://github.com/reem/rust-ordered-float |
| ordered-stream | v0.2.0 | MIT OR Apache-2.0 | https://github.com/danieldg/ordered-stream |
| parking_lot_core | v0.9.12 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |
| parking_lot | v0.12.5 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |
| parking | v2.2.1 | Apache-2.0 OR MIT | https://github.com/smol-rs/parking |
| peniko | v0.6.1 | Apache-2.0 OR MIT | https://github.com/linebender/peniko |
| pico-args | v0.5.0 | MIT | https://github.com/RazrFalcon/pico-args |
| pin-project-lite | v0.2.17 | Apache-2.0 OR MIT | https://github.com/taiki-e/pin-project-lite |
| piper | v0.2.5 | MIT OR Apache-2.0 | https://github.com/smol-rs/piper |
| png | v0.18.1 | MIT OR Apache-2.0 | https://github.com/image-rs/image-png |
| polling | v3.11.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/polling |
| polycool | v0.4.0 | MIT OR Apache-2.0 | https://github.com/linebender/kurbo |
| pp-rs | v0.2.1 | BSD-3-Clause | https://github.com/Kangz/glslpp-rs |
| presser | v0.3.1 | MIT OR Apache-2.0 | https://github.com/EmbarkStudios/presser |
| proc-macro2 | v1.0.107 | MIT OR Apache-2.0 | https://github.com/dtolnay/proc-macro2 |
| proc-macro-crate | v3.5.0 | MIT OR Apache-2.0 | https://github.com/bkchr/proc-macro-crate |
| profiling | v1.0.18 | MIT OR Apache-2.0 | https://github.com/aclysma/profiling |
| quick-xml | v0.41.0 | MIT | https://github.com/tafia/quick-xml |
| quote | v1.0.47 | MIT OR Apache-2.0 | https://github.com/dtolnay/quote |
| raw-window-handle | v0.6.2 | MIT OR Apache-2.0 OR Zlib | https://github.com/rust-windowing/raw-window-handle |
| read-fonts | v0.41.0 | MIT OR Apache-2.0 | https://github.com/googlefonts/fontations |
| renderdoc-sys | v1.1.0 | MIT OR Apache-2.0 | https://github.com/ebkalderon/renderdoc-rs |
| resvg | v0.48.1 | Apache-2.0 OR MIT | https://github.com/linebender/resvg |
| rgb | v0.8.53 | MIT | https://github.com/kornelski/rust-rgb |
| roxmltree | v0.21.1 | MIT OR Apache-2.0 | https://github.com/RazrFalcon/roxmltree |
| rustc-hash | v1.1.0 | Apache-2.0/MIT | https://github.com/rust-lang-nursery/rustc-hash |
| rustix | v1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/bytecodealliance/rustix |
| scoped-tls | v1.0.1 | MIT/Apache-2.0 | https://github.com/alexcrichton/scoped-tls |
| scopeguard | v1.2.0 | MIT OR Apache-2.0 | https://github.com/bluss/scopeguard |
| serde_core | v1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_derive | v1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_json | v1.0.151 | MIT OR Apache-2.0 | https://github.com/serde-rs/json |
| serde_repr | v0.1.21 | MIT OR Apache-2.0 | https://github.com/dtolnay/serde-repr |
| serde_spanned | v1.1.1 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| serde | v1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| signal-hook-registry | v1.4.8 | MIT OR Apache-2.0 | https://github.com/vorner/signal-hook |
| simd-adler32 | v0.3.10 | MIT | https://github.com/mcountryman/simd-adler32 |
| simplecss | v0.2.2 | Apache-2.0 OR MIT | https://github.com/linebender/simplecss |
| siphasher | v1.0.4 | MIT OR Apache-2.0 | https://github.com/jedisct1/rust-siphash |
| skrifa | v0.44.0 | MIT OR Apache-2.0 | https://github.com/googlefonts/fontations |
| slab | v0.4.12 | MIT | https://github.com/tokio-rs/slab |
| smallvec | v1.16.2 | MIT OR Apache-2.0 | https://github.com/servo/rust-smallvec |
| smithay-client-toolkit | v0.21.1 | MIT | https://github.com/smithay/client-toolkit |
| spirv | v0.4.0+sdk-1.4.341.0 | Apache-2.0 | https://github.com/gfx-rs/rspirv |
| static_assertions | v1.1.0 | MIT OR Apache-2.0 | https://github.com/nvzqz/static-assertions-rs |
| strict-num | v0.1.1 | MIT | https://github.com/RazrFalcon/strict-num |
| svg_fmt | v0.4.5 | MIT/Apache-2.0 | https://github.com/nical/rust_debug |
| svgtypes | v0.16.1 | Apache-2.0 OR MIT | https://github.com/linebender/svgtypes |
| syn | v2.0.119 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| syn | v3.0.6 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| termcolor | v1.4.1 | Unlicense OR MIT | https://github.com/BurntSushi/termcolor |
| thiserror-impl | v2.0.21 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| thiserror | v2.0.21 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| tiny-skia-path | v0.12.0 | BSD-3-Clause | https://github.com/linebender/tiny-skia/tree/master/path |
| tiny-skia | v0.12.0 | BSD-3-Clause | https://github.com/linebender/tiny-skia |
| toml_datetime | v1.1.1+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_edit | v0.25.15+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_parser | v1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml | v1.1.6+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_writer | v1.1.2+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| tracing-attributes | v0.1.31 | MIT | https://github.com/tokio-rs/tracing |
| tracing-core | v0.1.36 | MIT | https://github.com/tokio-rs/tracing |
| tracing | v0.1.44 | MIT | https://github.com/tokio-rs/tracing |
| ttf-parser | v0.25.1 | MIT OR Apache-2.0 | https://github.com/harfbuzz/ttf-parser |
| unicode-ident | v1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 | https://github.com/dtolnay/unicode-ident |
| unicode-width | v0.2.2 | MIT OR Apache-2.0 | https://github.com/unicode-rs/unicode-width |
| unicode-xid | v0.2.6 | MIT OR Apache-2.0 | https://github.com/unicode-rs/unicode-xid |
| usvg | v0.48.1 | Apache-2.0 OR MIT | https://github.com/linebender/resvg |
| uuid | v1.27.0 | Apache-2.0 OR MIT | https://github.com/uuid-rs/uuid |
| vello_encoding | v0.10.0 | Apache-2.0 OR MIT | https://github.com/linebender/vello |
| vello_shaders | v0.10.0 | Apache-2.0 OR MIT | https://github.com/linebender/vello |
| vello | v0.10.0 | Apache-2.0 OR MIT | https://github.com/linebender/vello |
| wayland-backend | v0.3.17 | MIT | https://github.com/smithay/wayland-rs |
| wayland-client | v0.31.15 | MIT | https://github.com/smithay/wayland-rs |
| wayland-csd-frame | v0.3.0 | MIT | https://github.com/rust-windowing/wayland-csd-frame |
| wayland-cursor | v0.31.14 | MIT | https://github.com/smithay/wayland-rs |
| wayland-protocols-experimental | v20251230.0.3 | MIT | https://github.com/smithay/wayland-rs |
| wayland-protocols-misc | v0.3.12 | MIT | https://github.com/smithay/wayland-rs |
| wayland-protocols | v0.32.13 | MIT | https://github.com/smithay/wayland-rs |
| wayland-protocols-wlr | v0.3.12 | MIT | https://github.com/smithay/wayland-rs |
| wayland-scanner | v0.31.11 | MIT | https://github.com/smithay/wayland-rs |
| wayland-sys | v0.31.11 | MIT | https://github.com/smithay/wayland-rs |
| wgpu-core-deps-windows-linux-android | v29.0.4 | MIT OR Apache-2.0 | https://github.com/gfx-rs/wgpu |
| wgpu-core | v29.0.4 | MIT OR Apache-2.0 | https://github.com/gfx-rs/wgpu |
| wgpu-hal | v29.0.4 | MIT OR Apache-2.0 | https://github.com/gfx-rs/wgpu |
| wgpu-naga-bridge | v29.0.4 | MIT OR Apache-2.0 | https://github.com/gfx-rs/wgpu |
| wgpu-types | v29.0.4 | MIT OR Apache-2.0 | https://github.com/gfx-rs/wgpu |
| wgpu | v29.0.4 | MIT OR Apache-2.0 | https://github.com/gfx-rs/wgpu |
| winnow | v1.0.4 | MIT | https://github.com/winnow-rs/winnow |
| xcursor | v0.3.11 | MIT | https://github.com/esposm03/xcursor-rs |
| xkbcommon | v0.8.0 | MIT | https://github.com/rust-x-bindings/xkbcommon-rs |
| xkeysym | v0.2.1 | MIT OR Apache-2.0 OR Zlib | https://github.com/notgull/xkeysym |
| yeslogic-fontconfig-sys | v6.0.1 | MIT | https://github.com/yeslogic/fontconfig-rs |
| zbus_macros | v5.19.0 | MIT | https://github.com/z-galaxy/zbus/ |
| zbus_names | v4.3.4 | MIT | https://github.com/z-galaxy/zbus/ |
| zbus | v5.19.0 | MIT | https://github.com/z-galaxy/zbus/ |
| zcheapstr | v1.1.0 | MIT | https://github.com/z-galaxy/zcheapstr/ |
| zerocopy-derive | v0.8.59 | BSD-2-Clause OR Apache-2.0 OR MIT | https://github.com/google/zerocopy |
| zerocopy | v0.8.59 | BSD-2-Clause OR Apache-2.0 OR MIT | https://github.com/google/zerocopy |
| zmij | v1.0.23 | MIT | https://github.com/dtolnay/zmij |
| zune-core | v0.5.3 | MIT OR Apache-2.0 OR Zlib | https://github.com/etemesi254/zune-image |
| zune-jpeg | v0.5.15 | MIT OR Apache-2.0 OR Zlib | https://github.com/etemesi254/zune-image/tree/dev/crates/zune-jpeg |
| zvariant_derive | v5.15.0 | MIT | https://github.com/z-galaxy/zbus/ |
| zvariant_utils | v4.2.0 | MIT | https://github.com/z-galaxy/zbus/ |
| zvariant | v5.15.0 | MIT | https://github.com/z-galaxy/zbus/ |

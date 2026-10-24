# Image preprocessing benchmark

Runs the production `src/image_processing.rs` pipeline (one-million-pixel cap, Lanczos3, JPEG quality 92) on deterministic detailed RGBA images. No capture, UI, network or credentials are used. Each invocation warms up once and reports the median of 11 samples.

```powershell
cargo run --manifest-path tools/image-bench/Cargo.toml --release
cargo run --manifest-path tools/image-bench/Cargo.toml --profile image-speed
```

Both profiles use `opt-level = "z"`, LTO and one codegen unit. `image-speed` changes only the `image` crate to `opt-level = 3`. This isolates the package override; it is not an end-to-end application benchmark or a binary-size measurement. The harness enables JPEG only, while the application retains its full set of image formats.

Measured on 2026-10-24 with an Intel Core i5-13600K and rustc 1.99.1 (58cf8f9c3), three process runs per profile (median milliseconds per run):

| Input | All crates z | image crate 3 |
| --- | --- | --- |
| 3840×2160 | 236.079 / 232.552 / 232.103 | 213.928 / 210.025 / 210.508 |
| 1920×80 | 11.051 / 10.955 / 11.079 | 10.678 / 10.537 / 10.445 |
| 320×160 | 3.648 / 3.614 / 3.658 | 3.479 / 3.468 / 3.455 |

The median across runs improves the 4K case from 232.552 ms to 210.508 ms (about 9.5%). The repeated improvement supports the image-only override. These results do not establish a better optimization level for the app's own pixel-difference or scroll-stitching loops, which remain unchanged.

<h1 align="center">
  🦅 glyphon 🦁
</h1>
<div align="center">
  Fast, simple 2D text rendering for <a href="https://github.com/gfx-rs/wgpu/"><code>wgpu</code></a>
</div>
<br />
<div align="center">
  <a href="https://crates.io/crates/glyphon"><img src="https://img.shields.io/crates/v/glyphon.svg?label=glyphon" alt="crates.io"></a>
  <a href="https://docs.rs/glyphon"><img src="https://docs.rs/glyphon/badge.svg" alt="docs.rs"></a>
  <img src="https://img.shields.io/badge/min%20rust-1.60-green.svg" alt="Minimum Rust Version">
  <a href="https://github.com/grovesNL/glyphon/actions"><img src="https://github.com/grovesNL/glyphon/workflows/CI/badge.svg?branch=main" alt="Build Status" /></a>
</div>

## What is this?

This crate provides a simple way to render 2D text with [`wgpu`](https://github.com/gfx-rs/wgpu/) by:

- shaping/calculating layout/rasterizing glyphs (with [`cosmic-text`](https://github.com/pop-os/cosmic-text/))
- packing the glyphs into texture atlas (with [`etagere`](https://github.com/nical/etagere/))
- sampling from the texture atlas to render text (with [`wgpu`](https://github.com/gfx-rs/wgpu/))

To avoid extra render passes, rendering uses existing render passes (following the middleware pattern described in [`wgpu`'s Encapsulating Graphics Work wiki page](https://github.com/gfx-rs/wgpu/wiki/Encapsulating-Graphics-Work).

## License

This project is licensed under either [Apache License, Version 2.0](LICENSE-APACHE), [zlib License](LICENSE-ZLIB), or [MIT License](LICENSE-MIT), at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache 2.0 license, shall be triple licensed as above, without any additional terms or conditions.

## Feedback

Glyphon prioritises simplicity of API over performance.

### _Automatic deallocation cost_

Most of the prepare time is attributable to choices that allow the consumer to not think about allocations and deallocations to the texture atlas. Unfortunately, this means that on every frame we have to perform O(glyph) updates to our LRUs and inserts to our `glyphs_in_use` hashmap. This represents 40% of the time.

- Mitigations: Manual allocation + deallocation

### _Preparation is inevitable_

A common operation in an editor text rendering pipeline is updating a global transform that Glyphon does not support. Instead, in Glyphon, we require iterating and potentially re-preparing each glyph. We'd prefer to update a global uniform and re-render quickly.

- Mitigations:
  - Allowing the setting of translation and scale globals
  - Allowing scaling out existing glyphs (and have them alias), but, also allowing their regeneration at some point in time.

### _Texture atlas constraints_

Glyphon assumes that all glyphs can fit into a single texture. It's likely that an Editor could enter a state with too much text to fit into a single texture.

- Mitigations: When a texture becomes full, start allocating into a separate texture up to `MAX_TEXTURES` count.

### _Unnecessary Subpixel Rendering_

Devices with large DPI don't require subpixel rendering.

### _Custom Hash Function_

We use FxHash, but, we could use a simple hash function of the glyph ID using a RawTable.

### _Culling_

We don't cull text areas prior to rendering. They could be off screen.

## Design

- Prepare function is a builder (allows ease of evolution like disabling subpixel rendering)
- Allocations and deallocations are explicit.
- Allow fast paths for translation and scaling by passing a global.
- Custom hash function?

## Data Structures

### Prepare

```rs

```

- All mutations must occur within prepare because we may mutate the atlas.
- Perhaps text areas have an ID attached / special struct?

## V2 TODO

- [x] Remove LRUs
- [ ] Deallocate
- [x] Pass global translation / scale
- [ ] Culling
- [ ] Remove bounds checks against viewport in `prepare_glyph` and the like
- [ ] Move subpixel rendering to prepare

Even if I figure out de-allocation, will a font atlas provide sufficient performance for us? We could consider strategies like pre-caching the glyphs at different scales in textures, but this will suffer from a memory/runtime hit.

I think even quality isn't that great. Consider what happens on rotating a glyph using a shader - it's horrible.

It's not that bad re-creating a single text boxes buffer.

## TODO:

- [ ] Can we alias during zoom if there is too much on screen?

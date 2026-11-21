# Compatibility patches

Pinned upstream versions are retained. No Vulkan SDK, loader or driver is shipped.

- `glutin-winit 0.5.0`: remove caption, resize-frame and extended edge styles from undecorated WGL windows before the display/device context is created. winit implements frameless windows through `WM_NCCALCSIZE` while retaining `WS_CAPTION`; older WGL presentation can still apply those frame insets to layered windows. Reword also preserves the genuinely borderless popup styles when updating the overlay's native flags.

- `wgpu-hal 30.0.2`: cap the requested Vulkan instance API and effective device API at 1.1 (or 1.0 when that is all the loader offers). Existing extension-based older-device paths remain in use.
- `wgpu-hal 30.0.2`: backport the nonempty descriptor-pool workaround from [gfx-rs/wgpu#10124](https://github.com/gfx-rs/wgpu/pull/10124). Older Intel HD 520 Vulkan drivers can crash when an empty layout results in `poolSizeCount == 0`.
- `i-slint-renderer-femtovg 1.19.0`: reject transparent surfaces that offer neither premultiplied nor postmultiplied composition. The supervising process starts OpenGL instead of displaying an opaque rectangle.

The renderer supervisor runs Vulkan and OpenGL in separate processes. Native driver access violations cannot be caught by Rust `catch_unwind`; an unsuccessful child exit or startup timeout triggers the OpenGL attempt. A normal user exit does not trigger a fallback.

Upstream license and copyright files are retained beside the patched sources.

# Architecture

Every pixel on screen is drawn by this program. There is a window, a GPU device, a terminal core, and a shader chain between them. Nothing else is on the surface.

## The shape

One process holds one `wgpu` device and a `winit` window. Inside it:

- A terminal core keeps the grid. It is `rio-vt`, pinned to an exact version, built from a fork branch that carries emulation fixes for left-right margins, origin mode, reverse wrap, and cursor-at-margin behaviour. Those are core grid semantics, so they belong in the core rather than in a compensation layer around it, and the branch is both what this workspace builds and the series offered upstream.
- The application owns its own PTY read loop. Every byte read goes first to a DCS tap that watches for the tmux control-mode handshake, and the core's parser gets back everything that is not inside the envelope. Owning the loop is what makes the tap possible.
- Glyphs are rasterised through `cosmic-text` and `swash` into an atlas this workspace owns, uploaded as a texture, and drawn by one WGSL shader into an offscreen grid texture. The atlas thresholds coverage to binary for the bitmap faces and scales them by integer arithmetic, which is what keeps pixel type exact at any window scale or device pixel ratio.
- That grid texture is the input to a CRT pass chain, expressed as slang shaders run by `librashader`. The chain carries the picture's identity: curvature, phosphor persistence, bloom, scan line, chroma, rasterisation modes.
- Phosphor persistence is a feedback pass. An alias hands the pass its own previous frame, which it samples against the live source, so the decay is state the chain holds rather than something the application recomputes.
- The chassis composites over the chain's output, in one native pass of its own: the casting, every piece of furniture standing on it, and the transient badges over the glass, as instances of a single draw in the plan's own order. It is flat, square, and outside the curvature, because a cabinet that bent with the tube would read as painted onto the glass rather than standing around it.

## Crates

| Crate | Owns |
|---|---|
| `config` | The settings schema, its defaults, the built-in presets, TOML reading and writing, the file watch, and the profile model. |
| `term` | Everything that is a terminal and nothing that is a window: the session, the PTY loop, the grid read-back seam, the DCS tap, the glyph atlas, selection, hotspots, and the pointer distortion math. |
| `crt-render` | The pass graph, the persistence feedback pass inside it, and materialising the preset and shader bodies into the cache directory. |
| `gpu` | The wgpu device concerns every crate shares: the feature set a device is created with, the offscreen target and its readback, and, for tests only, the machine-wide device lock. |
| `chassis` | The cabinet: bank geometry, shells, channel displays, the procedural metal, and the furniture drawn over it as descriptions. It holds no device and draws nothing. |
| `tmux-cc` | The control-mode protocol codec, and nothing else. Session and window policy live with the gateway that uses it. |
| `app` | The process: command line, window shell, single-instance arbitration, input, crash logger, and the tmux gateway. |
| `xtask` | The evaluation harness and the packaging targets. |
| `shader-oracle` | CPU reimplementations of the shader math, as a development dependency. Tests render a pixel on the GPU and compare it against the same closed form computed here. |

Four boundaries carry weight. The chain stops at the glass, so `chassis` computes geometry and colour without depending on the render chain at all, shipped or dev. What keeps the cabinet out of the curvature is not the chain's own machinery but the order of the frame: the chassis is drawn by one native `wgpu` pass in `app`, after the chain has finished, scissored to the bank column and composited with `LoadOp::Load`. Its shader bodies are WGSL, which no preset and no cache directory stand between. The one pass of the cabinet's that *is* slang is the bezel, because the bezel sits inside the curvature; the chain mounts it, and measures it. The protocol codec holds no policy, so it can be driven from recorded transcripts. The configuration plumbing is schema-agnostic, operating on a TOML document and on any deserializable type, so the schema and the file mechanics can change independently. The oracle is a development dependency, so no reference implementation ships in a binary.

## No widget toolkit

No GTK, no Qt, no toolkit of any kind is linked in. This follows from what the product is rather than from thrift.

The surface is a curved tube. A widget drawn by a toolkit would have to be either inside the curvature, which no toolkit can do because it composites rectangles into a flat surface, or floating above it as an untextured rectangle that announces itself as not part of the appliance. The chassis, the channel bank, and the strip displays are drawn by the same GPU passes as the picture, from the same settings, in the same colour space. A pointer event travels back through the curvature before it means anything, so hit testing belongs to whoever owns that transform.

Settings therefore have no dialog. The configuration file is the interface: edit it in any editor, and a file watch reloads it live. A change that only moves a uniform is pushed to the running chain; a change that alters the chain's shape rebuilds it. The file is a diff against built-in defaults, so an absent key means its default and a two-line file is a valid one.

## The field

Any terminal with this picture needs four things: a VT core, a window and input layer, a GPU pipeline that can run a multi-pass filter chain, and a text pipeline that can put exact pixels on a curved surface. The paths differ in how many of those come pre-built, and in what they cost when they do.

| Path | Reach | What it costs |
|---|---|---|
| **`rio-vt` + `winit` + `wgpu` + `librashader`** | Linux, macOS, Windows at launch. ConPTY is in the core. | The core is young, so its API moves and the version pins exactly. `librashader` pins `wgpu` exactly in turn, so the GPU stack upgrades on its schedule. The core carries a mandatory C++ dependency, so macOS and Windows build on native runners rather than cross-compiling. |
| `alacritty_terminal` + `winit` + `wgpu` | The same three platforms. | Every component has a production existence proof, and the core is pure Rust, which cross-compiles cleanly. Nothing above the core exists: the render-pass graph, the filter chain, the glyph pipeline, and the control-mode hooks are all hand-built. This is the standing fallback, and the layers it shares with the path taken (settings, chassis, protocol codec, harness) are not tied to the core. |
| A fork of Rio | The same three platforms, working on day one. | Its filter pass runs full-window and in place on the swapchain, with no offscreen grid texture before it, so cabinet chrome drawn through its renderer bends with the curvature along with the type. Keeping the cabinet undistorted means rewriting a sealed frame loop and carrying that as a permanent diff against a fast-moving upstream. Its filters also do not run on the native Vulkan and Metal backends it defaults to, so the picture would live behind a non-default flag. The day-one terminal is exactly the render path that would be rewritten. |
| A fork of WezTerm | Broad. | The only codebase where tmux control mode is already implemented. It has no shader hook, so the entire visual identity becomes surgery on a bespoke renderer, carried as a permanent diff against a single-maintainer upstream. It serves better as a reference implementation for the protocol than as a base. |
| `libghostty` with an own renderer | Post-1.0 for Windows, undated. | The VT core is excellent. The embedding API is explicitly unstable, the GPU layer is planned rather than shipped, and the surrounding ecosystem supplies no window-layer equivalent, so it is the hand-built renderer path with less around it. |
| `xterm.js` + WebGL in Electron | Widest, and the most forgiving to build. | The CRT pass list including persistence has working prior art, and control mode needs no fork. Input latency and keyboard-protocol fidelity have a ceiling that no later work removes, and the runtime weighs about 150 MB. A terminal that behaves like hardware cannot spend its budget there. |
| A Tcl/Tk application over a C VT library | Uncertain. | Tk cannot composite a GPU child, so the entire picture would have to live in a GPU surface hosted inside a Tk widget through private platform internals, which no shipping application demonstrates. The core choice underneath does not change that. |

The picture and the cabinet are the product, so the paths that hand over a working terminal in exchange for owning someone else's render path cost more than they give. The paths that hand over nothing above a VT core leave the filter chain, the feedback pass, and the preset machinery to be written by hand, all of which exist as maintained crates on the path taken.

## Reach

macOS is a first-class target, so nothing that cannot ship a polished macOS application is eligible. Windows is reached at launch because ConPTY support sits in the terminal core rather than in a platform shim above it. Linux runs on Wayland and X11 through the window layer.

Because the core carries a C++ dependency, each platform's binaries are built on that platform. Cross-compiling from one host is not part of the build.

## How it is held true

Correctness here is mostly visual, so the tests are arithmetic rather than judgement.

- Shader math has two implementations. The GPU renders a pixel; the oracle crate computes the same closed form on the CPU; the test compares them. A shared helper file carries the surface math the metal passes have in common, so there is one place to change and one reference to check it against.
- The evaluation harness drives a real binary under a scratch environment, screenshots it, masks the regions a comparison should ignore, and reports RMSE against a reference image. It is parameterised on the executable path, so it can measure any binary that honours the window and command-line contract it prints.
- Emulation is measured against esctest, with the failing families fixed in the core.
- The control-mode gateway is exercised against a live tmux server: transport, per-page windows, collapse on exit, gateway death, and two simultaneous sockets.
- Settings duplication is measured, not judged. `cargo run -p xtask -- fanout <setting>` counts how many non-test places mention a setting, read beside a setting the change under measure never touched, so a rise from editing churn shows in both. The structural check is to add one throwaway setting end to end, or delete a real one, and count the files a maintainer had to edit.

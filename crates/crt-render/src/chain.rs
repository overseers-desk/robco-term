//! The filter chain: one preset, loaded once, driven by uniforms.
//!
//! [`Chain`] owns the librashader `FilterChain` and the only decision the
//! settings layer forces on it: whether a change is a uniform push or a
//! rebuild. The chain decides that for itself, on every settings change, by
//! deriving a [`Structure`] from the old and new `Config` and rebuilding iff
//! they differ. `app::settings::classify()` names the same split at the
//! config layer, but only for logging: it is informational, not load-bearing,
//! and the two encodings are independent rather than agreeing by
//! construction. What ties them together is that `Structure::from_config`
//! reads a subset of the fields `config::structural::STRUCTURAL` names, and a
//! test in `crates/app` holds that subset relation.
//!
//! The chain's input is `gpu::Target`'s texture, which carries
//! `TEXTURE_BINDING` for exactly this. Its output is the finished glass image
//! and nothing else: chassis chrome composites over this result rather than
//! through it. `docs/arch.md` explains why that makes the renderer this
//! chain rather than a fork of Rio.
//!
//! One pass in the chain has state outside its own uniforms, and [`Chain`]
//! carries the host half of it: the burn-in accumulator at pass 0.
//! [`crate::burn_in`]'s docs state what a host graph owes it, and all of it is
//! here rather than at the call site, so nothing above this crate can forget a
//! frame of it:
//!
//! | The contract | Where |
//! |---|---|
//! | `push()` every frame, before `FilterChain::frame` | [`Chain::frame`], first thing it does |
//! | `set_burn_in()` when the setting moves | [`Chain::set_params`], from the same `Params` the uniforms come from |
//! | `restart()` when the accumulator is discontinued | [`Chain::apply_settings`], on the rebuild path only |

use std::path::{Path, PathBuf};

use config::Config;
use librashader::presets::ShaderFeatures;
use librashader::runtime::wgpu::{FilterChain, FilterChainOptions, WgpuOutputView};
use librashader::runtime::{FilterChainParameters, Size, Viewport};

use crate::burn_in::BurnInPass;

use crate::pacing::FrameTime;
use crate::params::Params;
use crate::preset::{self, Structure};

/// What applying a settings change did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Uniforms only. The chain was not touched, so the burn-in ghost, the
    /// feedback framebuffers and every compiled pipeline survive.
    Parameters,
    /// A structural key moved, so the preset was rewritten and the chain
    /// reloaded, an accepted cost that resets the burn-in ghost.
    Rebuilt,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Load(String),
    Frame(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "materialising the preset: {e}"),
            Error::Load(e) => write!(f, "loading the filter chain: {e}"),
            Error::Frame(e) => write!(f, "rendering a chain frame: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub struct Chain {
    inner: FilterChain,
    /// The host half of pass 0. It owns no GPU state of its own -- the ghost
    /// lives in the chain's feedback framebuffer -- only the wall clock the
    /// decay is measured on.
    burn_in: BurnInPass,
    dir: PathBuf,
    preset_path: PathBuf,
    structure: Structure,
    /// How many times the `FilterChain` has been built, the first load
    /// included. A test that means "no reload happened" asserts on this, and
    /// so does anything measuring the ~10 ms a reload costs against a uniform
    /// write.
    loads: u64,
}

impl Chain {
    /// Load the chain for `structure`, writing the preset into `dir`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dir: &Path,
        structure: Structure,
    ) -> Result<Self, Error> {
        let preset_path = preset::materialize(dir, &structure)?;
        let inner = load(device, queue, &preset_path)?;
        Ok(Self {
            inner,
            // Zero until the first `set_params`, which is the safe end: a decay
            // of 1.0 leaves the accumulator equal to the live image, so a chain
            // that is never given parameters shows no ghost rather than one
            // that never fades.
            burn_in: BurnInPass::new(0.0),
            dir: dir.to_path_buf(),
            preset_path,
            structure,
            loads: 1,
        })
    }

    /// Load the chain a `Config` asks for.
    pub fn from_config(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dir: &Path,
        cfg: &Config,
    ) -> Result<Self, Error> {
        Self::new(device, queue, dir, Structure::from_config(cfg))
    }

    /// Take a new `Config`, rebuilding only if it moved a structural key.
    ///
    /// This does not push parameters. Parameters are pushed every frame by
    /// [`Chain::set_params`] whether the settings changed or not, because the
    /// time and degauss uniforms move on their own; making a settings change
    /// also push them would be a second path to the same write.
    pub fn apply_settings(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cfg: &Config,
    ) -> Result<Applied, Error> {
        let wanted = Structure::from_config(cfg);
        if wanted == self.structure {
            return Ok(Applied::Parameters);
        }
        self.preset_path = preset::materialize(&self.dir, &wanted)?;
        self.inner = load(device, queue, &self.preset_path)?;
        self.structure = wanted;
        self.loads += 1;
        // The rebuilt chain has fresh feedback framebuffers, so the ghost is
        // gone and there is nothing for the next frame to decay. Without this
        // the next frame subtracts the whole stall since the last one from an
        // accumulator that is already black.
        self.burn_in.restart();
        Ok(Applied::Rebuilt)
    }

    /// Push this frame's uniforms. Cheap by design: a uniform write costs
    /// ~83 ns, against ~10 ms for the reload the one-preset shape avoids.
    ///
    /// The accumulator's rate arrives with them. It is not one of the uniforms
    /// -- the decay the shader subtracts is computed per frame from the real
    /// delta, in [`Chain::frame`] -- but `screen.burn_in` sets the rate that
    /// computation uses, and this is where the settings reach the chain.
    /// Setting it does not restart the clock, so the ghost on screen keeps
    /// fading from where it is, at the new rate: the parameter-level path
    /// that avoids a chain rebuild.
    pub fn set_params(&mut self, params: &Params) {
        let sink = self.inner.parameters();
        for (name, value) in params.iter() {
            sink.set_parameter_value(name, *value);
        }
        self.burn_in.set_burn_in(params.burn_in());
    }

    /// Draw one frame: `input` through the chain, into `output`.
    ///
    /// `input` is `gpu::Target::texture`. `output` is whatever the
    /// chassis layer wants the glass on, a swapchain view in the app and an
    /// offscreen view under test; the chain does not care which.
    ///
    /// `time` is the frame's clock, and pass 0 is why it is a whole
    /// [`FrameTime`] rather than an index: the accumulator's decay is a
    /// wall-clock quantity, so this is where `BurnInPass::push` is called,
    /// unconditionally and before the chain runs. Doing it here rather than
    /// leaving it to the caller is the point -- a frame drawn on a stale delta
    /// fades by the wrong amount, and there is no way to notice from the
    /// outside.
    pub fn frame(
        &mut self,
        input: &wgpu::Texture,
        output: &wgpu::TextureView,
        output_size: (u32, u32),
        output_format: wgpu::TextureFormat,
        encoder: &mut wgpu::CommandEncoder,
        time: FrameTime,
    ) -> Result<(), Error> {
        self.frame_at(
            input,
            output,
            (0, 0),
            output_size,
            output_format,
            encoder,
            time,
        )
    }

    /// [`Chain::frame`] into a rectangle of `output` rather than all of it.
    ///
    /// The rectangle is the *screen well*, which is the window less the bank
    /// column when a chassis stands (`chassis::WindowLayout`). Everything
    /// the chain measures is already the well's rather than the window's:
    /// `Params::build` reads `normalizedScreenScale` off the geometry it is
    /// handed, and that geometry comes from the offscreen target, which the
    /// host sizes to the well, so this only moves where the finished glass
    /// lands.
    ///
    /// What it does *not* do is leave the rest of the image alone: librashader
    /// begins its last render pass with a clear over the whole attachment and
    /// sets its scissor afterwards, so the column's ground is cleared to
    /// transparent black by this call whatever the origin is. That is why
    /// `app::chrome` composites over the frame after this returns rather than
    /// before it.
    #[allow(clippy::too_many_arguments)]
    pub fn frame_at(
        &mut self,
        input: &wgpu::Texture,
        output: &wgpu::TextureView,
        output_origin: (u32, u32),
        output_size: (u32, u32),
        output_format: wgpu::TextureFormat,
        encoder: &mut wgpu::CommandEncoder,
        time: FrameTime,
    ) -> Result<(), Error> {
        self.burn_in.push(self.inner.parameters(), time.now);

        let size = Size::new(output_size.0, output_size.1);
        let viewport = Viewport {
            x: output_origin.0 as f32,
            y: output_origin.1 as f32,
            mvp: None,
            output: WgpuOutputView::new_from_raw(output, size, output_format),
            size,
        };
        self.inner
            .frame(input, &viewport, encoder, time.index, None)
            .map_err(|e| Error::Frame(e.to_string()))
    }

    /// The accumulator, for a caller with a reason the settings cannot express.
    ///
    /// The application has one: `restart()` after a resize or a font change,
    /// which discontinue the ghost without moving a settings key, so
    /// [`Chain::apply_settings`] never sees them. Tests have another, the mask
    /// switch.
    pub fn burn_in(&mut self) -> &mut BurnInPass {
        &mut self.burn_in
    }

    /// The decay pushed on the last frame, for logging and tests.
    pub fn last_burn_in_decay(&self) -> Option<f32> {
        self.burn_in.last_decay()
    }

    pub fn loads(&self) -> u64 {
        self.loads
    }

    pub fn structure(&self) -> Structure {
        self.structure
    }

    pub fn preset_path(&self) -> &Path {
        &self.preset_path
    }
}

fn load(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    preset_path: &Path,
) -> Result<FilterChain, Error> {
    let options = FilterChainOptions {
        force_no_mipmaps: false,
        // The shader cache is off until someone measures that it pays. Turning
        // it on is not free of conditions: librashader calls
        // `create_pipeline_cache` without checking the device supports it, so
        // `wgpu::Features::PIPELINE_CACHE` has to be requested at device
        // creation or this panics on a rayon worker rather than returning an
        // error.
        enable_cache: false,
        adapter_info: None,
    };
    FilterChain::load_from_path(
        preset_path,
        ShaderFeatures::NONE,
        device,
        queue,
        Some(&options),
    )
    .map_err(|e| Error::Load(format!("{}: {e}", preset_path.display())))
}

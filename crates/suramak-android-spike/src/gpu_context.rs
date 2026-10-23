//! The one place an `egui::Context` is built — and the ownership rule behind it.
//!
//! `egui::Context` owns texture *bookkeeping*: which ids exist, their sizes and
//! options, and which parts of them are dirty. The GPU copy of those textures
//! lives in `egui_wgpu::Renderer`. The two are one pair. A context that outlives
//! its renderer keeps believing every id is still resident, so it sends
//! `ImageDelta { pos: Some(..) }` — a patch of an *existing* texture — and
//! `egui-wgpu-0.33.3/src/renderer.rs:657` panics:
//!
//! ```text
//! Tried to update a texture that has not been allocated yet.
//! ```
//!
//! That is exactly the v2 field crash (`docs/WORKORDER-M13B3.md`): the context
//! was built once in `Handler` and outlived `self.gpu = None` on suspend, so the
//! first frame after every successful resume patched ids the freshly built
//! renderer had never allocated.
//!
//! The fix is structural, not an API chase: [`crate::app`] calls
//! [`new_context`] *inside* `Gpu::new`, so the context is owned by the same
//! value as the surface, device, queue and renderer. Dropping the `Gpu` on
//! `suspended` drops the context with it, and the next generation starts from
//! an empty registry on both sides — every id re-registers through a real
//! (full-image) create on its first frame.
//!
//! The cost of that choice is egui's *per-context* UI state: the text
//! caret/selection, scroll offsets and hover reset on every rebuild. The
//! spike's own state ([`crate::ui::SpikeState`] — text buffer, probe, counters)
//! is app-owned and survives, which is what check 2 measures.
//!
//! The tests below pin the mechanism on the host: a reduced model of the
//! renderer's registry replays real `egui` texture deltas, first against a kept
//! context (the field panic, reproduced) and then against a rebuilt one (every
//! id re-registers through a full-image create). See
//! `docs/reports/M13b3-spike-v2.1.md` §2 for why this is the strongest evidence
//! available off-device.

/// Build the spike's egui context: dark visuals and phone-sized type.
///
/// **One context per GPU generation.** Call this from the code that owns the
/// surface (see [`crate::app`]'s `Gpu::new`) and store the result next to the
/// renderer; never keep one alive across a renderer rebuild.
pub fn new_context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::dark());
    // Phone-sized defaults: the spike is read at arm's length.
    ctx.style_mut(|style| {
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(17.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(16.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(24.0));
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 10.0);
    });
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::ImageDelta;
    use egui::{TextureId, TexturesDelta};
    use std::collections::HashMap;

    /// A model of `egui_wgpu::Renderer`'s texture registry, reduced to the one
    /// rule that panicked in the field: `ImageDelta::pos == Some` means "patch a
    /// texture that is already on the GPU", so an id that was never allocated is
    /// a hard failure (`egui-wgpu-0.33.3/src/renderer.rs:657`,
    /// `.expect("Tried to update a texture that has not been allocated yet.")`).
    /// `pos == None` allocates (or replaces) the whole texture.
    ///
    /// Deliberately minimal: a real renderer also validates the patch bounds and
    /// rebuilds bind groups. Only the allocation rule is modelled, because that
    /// is the rule the field crash broke.
    #[derive(Default)]
    struct Registry {
        textures: HashMap<TextureId, [usize; 2]>,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum RegistryError {
        NotAllocated(TextureId),
    }

    impl Registry {
        fn update(&mut self, id: TextureId, delta: &ImageDelta) -> Result<(), RegistryError> {
            match delta.pos {
                Some(_patch_origin) => {
                    if !self.textures.contains_key(&id) {
                        return Err(RegistryError::NotAllocated(id));
                    }
                    Ok(())
                }
                None => {
                    self.textures
                        .insert(id, [delta.image.width(), delta.image.height()]);
                    Ok(())
                }
            }
        }

        /// What `Gpu::frame` does with `full_output.textures_delta.set`, then
        /// `.free`.
        fn apply(&mut self, deltas: &TexturesDelta) -> Result<(), RegistryError> {
            for (id, delta) in &deltas.set {
                self.update(*id, delta)?;
            }
            for id in &deltas.free {
                self.textures.remove(id);
            }
            Ok(())
        }
    }

    fn swatch(size: usize) -> egui::ColorImage {
        egui::ColorImage::filled([size, size], egui::Color32::WHITE)
    }

    /// One real `egui` frame (layout + texture deltas, no GPU), replayed into
    /// `registry` the way `Gpu::frame` replays it into the renderer.
    fn run_frame(ctx: &egui::Context, registry: &mut Registry) -> Result<(), RegistryError> {
        run_frame_deltas(ctx, registry).map(|_| ())
    }

    /// The same frame, handing back what the context decided to send — a full
    /// image (create) or a patch of an existing texture — so a test can assert
    /// on it.
    fn run_frame_deltas(
        ctx: &egui::Context,
        registry: &mut Registry,
    ) -> Result<TexturesDelta, RegistryError> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 360.0),
            )),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            // Something that tessellates, so the font atlas is exercised too.
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("texture lifecycle probe");
            });
        });
        registry.apply(&output.textures_delta)?;
        Ok(output.textures_delta)
    }

    /// The v2 field failure, reproduced structurally: keep the context, drop the
    /// renderer's registry (what `self.gpu = None` did on suspend), then let the
    /// kept context patch a texture it still believes is resident. The fresh
    /// registry has never heard of that id — in `egui_wgpu` this is the
    /// `renderer.rs:657` panic, on the first frame after a successful resume.
    #[test]
    fn a_kept_context_against_a_fresh_registry_reproduces_the_field_panic() {
        let ctx = new_context();
        let mut registry = Registry::default();

        let mut texture = ctx.load_texture("swatch", swatch(8), egui::TextureOptions::default());
        run_frame(&ctx, &mut registry).expect("the first frame only creates textures");
        let id = texture.id();
        assert!(
            registry.textures.contains_key(&id),
            "the swatch was not allocated"
        );

        // Suspend: the renderer and its registry go, the context stays (v2 shape).
        drop(registry);
        let mut rebuilt = Registry::default();

        // Resume: the context patches the texture it believes is still there.
        texture.set_partial([0, 0], swatch(2), egui::TextureOptions::default());
        let err = run_frame(&ctx, &mut rebuilt).expect_err("this is the field panic");
        assert_eq!(err, RegistryError::NotAllocated(id));
        assert!(
            !rebuilt.textures.contains_key(&id),
            "the rebuilt registry must not have allocated the patched id implicitly"
        );

        // The same delta against a registry that *did* see the create is fine —
        // the failure is the generation mismatch, not the patch itself.
        let mut same_generation = Registry::default();
        same_generation.textures.insert(id, [8, 8]);
        same_generation
            .update(
                id,
                &ImageDelta::partial([0, 0], swatch(2), egui::TextureOptions::default()),
            )
            .expect("a patch of an allocated texture is legal");
    }

    /// The v2.1 shape: the context is dropped and rebuilt with the renderer, so
    /// a generation's first frame carries a *full* image for every id it uses
    /// and the registry never sees a patch for something it has not allocated.
    #[test]
    fn a_rebuilt_context_re_registers_every_texture_with_full_images() {
        // Generation 1.
        let ctx_a = new_context();
        let mut registry_a = Registry::default();
        let texture_a = ctx_a.load_texture("swatch", swatch(8), egui::TextureOptions::default());
        run_frame(&ctx_a, &mut registry_a).expect("generation 1 first frame");
        let id_a = texture_a.id();
        // A second frame with a patch, to show the id was live in generation 1.
        let ctx_a_data = ctx_a.clone();
        ctx_a_data.data_mut(|data| data.insert_temp(egui::Id::new("probe"), 7u32));

        // Suspend: everything, context included, goes away.
        drop(texture_a);
        drop(ctx_a_data);
        drop(ctx_a);
        drop(registry_a);

        // Generation 2.
        let ctx_b = new_context();
        let mut registry_b = Registry::default();
        let texture_b = ctx_b.load_texture("swatch", swatch(8), egui::TextureOptions::default());
        let first = run_frame_deltas(&ctx_b, &mut registry_b).expect("generation 2 first frame");
        let id_b = texture_b.id();

        assert_eq!(
            id_a, id_b,
            "egui re-uses the same numeric ids in a fresh context — which is why the \
             renderer must not be reused across a context rebuild"
        );
        assert!(
            !first.set.is_empty(),
            "a fresh context creates its textures"
        );
        assert!(
            first.set.iter().all(|(_, delta)| delta.pos.is_none()),
            "every delta in a rebuilt context's first frame must be a full image, \
             never a patch: {:?}",
            first
                .set
                .iter()
                .map(|(id, d)| (*id, d.pos))
                .collect::<Vec<_>>()
        );
        assert!(
            first.set.iter().any(|(id, _)| *id == id_b),
            "the swatch must be allocated in this generation"
        );
        assert!(registry_b.textures.contains_key(&id_b));

        // And the per-context UI state really did reset — the documented cost of
        // a full rebuild (the spike's own `SpikeState` does not live here).
        assert_eq!(
            ctx_b.data(|data| data.get_temp::<u32>(egui::Id::new("probe"))),
            None,
            "a rebuilt context must not inherit the previous generation's UI state"
        );

        // The new generation can patch freely: its registry saw the create.
        let mut texture_b = texture_b;
        texture_b.set_partial([0, 0], swatch(2), egui::TextureOptions::default());
        run_frame(&ctx_b, &mut registry_b).expect("a patch inside one generation is legal");
    }

    /// `new_context` must hand out a *new* context every call: a cached one would
    /// reintroduce exactly the bug (ids patched against a registry that no longer
    /// exists).
    #[test]
    fn every_call_builds_a_distinct_context() {
        let first = new_context();
        let second = new_context();
        first.data_mut(|data| data.insert_temp(egui::Id::new("marker"), 1u32));
        assert_eq!(
            second.data(|data| data.get_temp::<u32>(egui::Id::new("marker"))),
            None
        );
        // Same styling policy on both.
        assert_eq!(
            first.style().text_styles.len(),
            second.style().text_styles.len()
        );
    }
}

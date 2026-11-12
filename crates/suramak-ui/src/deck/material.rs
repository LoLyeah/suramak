//! M20B — the frosted material: one capability, one recipe, two honest answers.
//!
//! Android has no system backdrop blur for a single GL surface, so "frosted"
//! is painted **in-surface**: a vertical veil (`Mesh`, no texture), a 1 px
//! perimeter hairline, a 1 px specular arc over the top edge, and a 64×64
//! deterministic micro-grain on the floating planes. macOS has the real thing
//! (`NSGlassEffectView` / `NSVisualEffectView` behind the render surface), so
//! [`MaterialBackend::SystemGlass`] zeroes every frost alpha and the same
//! primitives paint no veil, no rim and no grain — never a painted imitation
//! of Liquid Glass over the real optical material.
//!
//! The shadow budget is part of the material, not the screens: cards sit in the
//! scroll plane with **no** drop shadow; sheets and the floating dock are the
//! only two elevated planes (`12 pt` blur, `+4 pt` Y, `22 %` black).
//!
//! **Not adopted by the screens in this slice.** These are the primitives the
//! workorder ships for slices 3–5; nothing in `compact::` calls them yet.
//!
//! Signatures:
//!
//! ```text
//! frosted_panel(ui, tokens, rect, radius: impl Into<CornerRadius>, plane: Plane)
//! scrim(ui, rect, tokens)
//! dock_scrim(ui, rect, tokens)
//! card(ui, tokens, rect, divider_after: Option<f32>) -> Rect   // content rect
//! veil_mesh(rect, radius, top: Color32, bottom: Color32) -> Mesh
//! grain_image(alpha: u8) -> ColorImage
//! grain_texture(ctx, alpha: u8) -> Option<TextureHandle>
//! ```

use egui::epaint::{Mesh, Shadow, Vertex, WHITE_UV};
use egui::{
    Color32, Context, CornerRadius, Pos2, Rect, Stroke, StrokeKind, TextureHandle, TextureOptions,
    TextureWrapMode, Ui,
};

use crate::deck::theme::Tokens;

/// The standard internal padding of a card (plan §D: the 8 pt rhythm, 12 pt
/// inside a card).
pub const CARD_PADDING: f32 = 12.0;

/// The micro-grain tile's side, in texels. One 4 KB texture, tiled by UV.
pub const GRAIN_TILE: usize = 64;

/// The one float-plane shadow: `12 pt` blur, `+4 pt` Y, `22 %` black. Cards
/// get [`Shadow::NONE`] — a scrolling card is not an elevated plane.
pub const FLOAT_SHADOW: Shadow = Shadow {
    offset: [0, 4],
    blur: 12,
    spread: 0,
    color: Color32::from_black_alpha(56), // 22 % of 255, rounded
};

/// Which platform material the UI draws against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialBackend {
    /// The OS owns the material: real AppKit blur behind a transparent window
    /// (macOS). The painted frost is suppressed so nothing sits over it.
    SystemGlass,
    /// No system backdrop exists (Android, Windows, Linux): the in-surface
    /// veil + rim + grain recipe is the material.
    PaintedFrost,
}

impl MaterialBackend {
    /// The capability for this process. macOS always has a real material path
    /// (`NSGlassEffectView` on 26+, `NSVisualEffectView` below it); every other
    /// target answers `PaintedFrost` without probing anything.
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::from_system_glass(suramak_glass::system_glass_available())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::PaintedFrost
        }
    }

    /// The pure rule [`detect`](Self::detect) feeds: a system material exists
    /// or it does not. Kept separate from the platform probe so the macOS
    /// branch (and the zeroing it triggers) is testable on any host.
    pub const fn from_system_glass(available: bool) -> Self {
        if available {
            Self::SystemGlass
        } else {
            Self::PaintedFrost
        }
    }

    /// Whether the painted recipe runs. `SystemGlass` paints no frost at all.
    pub const fn paints_frost(self) -> bool {
        matches!(self, Self::PaintedFrost)
    }

    /// Zeroes the seven frost tokens when the OS supplies the material. The
    /// legacy ladder (window/panel/card/sheet) is untouched: M20B adds a frost
    /// layer, it does not restyle screens. On macOS the existing window is
    /// already transparent, so what remains is the real AppKit material.
    pub fn apply(self, tokens: &mut Tokens) {
        if self.paints_frost() {
            return;
        }
        tokens.frost_veil_top = Color32::TRANSPARENT;
        tokens.frost_veil_bottom = Color32::TRANSPARENT;
        tokens.rim_specular = Color32::TRANSPARENT;
        tokens.rim_hairline = Color32::TRANSPARENT;
        tokens.frost_grain_alpha = 0.0;
        tokens.dock_scrim_top = Color32::TRANSPARENT;
        tokens.dock_scrim_bottom = Color32::TRANSPARENT;
    }
}

/// The three material planes. `Card` is in the scroll plane (no shadow, no
/// grain); `Sheet` and `Dock` float (one shadow, grain).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    Card,
    Sheet,
    Dock,
}

impl Plane {
    /// The shadow budget: cards get none; sheets and the dock share the one
    /// float shadow.
    pub const fn shadow(self) -> Shadow {
        match self {
            Self::Card => Shadow::NONE,
            Self::Sheet | Self::Dock => FLOAT_SHADOW,
        }
    }

    /// Whether the plane takes the micro-grain (Sheet/Dock only — a card gets
    /// the veil, not grain).
    pub const fn takes_grain(self) -> bool {
        !matches!(self, Self::Card)
    }

    /// The token the plane is painted from.
    pub const fn base(self, tokens: &Tokens) -> Color32 {
        match self {
            Self::Card => tokens.card,
            Self::Sheet => tokens.sheet,
            Self::Dock => tokens.panel,
        }
    }
}

/// Paints one frosted panel: base fill, veil gradient, grain (Sheet/Dock),
/// hairline rim, top specular arc, and the plane's shadow.
///
/// `radius` accepts a `u8` (the token ladder's radii) or a `CornerRadius` for
/// sheets whose top corners round and bottom corners do not.
///
/// Under [`MaterialBackend::SystemGlass`] the tokens carry zero alphas, so the
/// veil, grain, rim and specular all draw nothing and only the base token fill
/// remains (translucent on macOS — the AppKit material is the surface).
pub fn frosted_panel(
    ui: &Ui,
    tokens: &Tokens,
    rect: Rect,
    radius: impl Into<CornerRadius>,
    plane: Plane,
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let radius = radius.into();
    let painter = ui.painter();

    // 1. The plane's shadow (none for cards).
    let shadow = plane.shadow();
    if shadow != Shadow::NONE {
        painter.add(shadow.as_shape(rect, radius));
    }

    // 2. The base token fill.
    painter.rect_filled(rect, radius, plane.base(tokens));

    // 3. The veil: one vertical-gradient mesh, no texture binds.
    if tokens.frost_veil_top.a() > 0 || tokens.frost_veil_bottom.a() > 0 {
        painter.add(egui::Shape::mesh(veil_mesh(
            rect,
            radius,
            tokens.frost_veil_top,
            tokens.frost_veil_bottom,
        )));
    }

    // 4. The micro-grain: sheets and the floating dock only.
    if plane.takes_grain() {
        let alpha = grain_alpha(tokens);
        if let Some(texture) = grain_texture(ui.ctx(), alpha) {
            painter.add(egui::Shape::mesh(grain_mesh(rect, radius, &texture)));
        }
    }

    // 5. The perimeter hairline.
    if tokens.rim_hairline.a() > 0 {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0_f32, tokens.rim_hairline),
            StrokeKind::Inside,
        );
    }

    // 6. The top specular: the straight top edge plus the two upper corner
    //    tangents, 1 px, never a full-perimeter highlight.
    if tokens.rim_specular.a() > 0 {
        let mut points = upper_perimeter(rect, radius);
        if points.len() >= 2 {
            painter.add(egui::Shape::line(
                std::mem::take(&mut points),
                Stroke::new(1.0_f32, tokens.rim_specular),
            ));
        }
    }
}

/// The sheet scrim: the page behind a modal is dimmed so the sheet reads above
/// it. (egui has no backdrop blur; a dim is the honest ceiling.)
pub fn scrim(ui: &Ui, rect: Rect, tokens: &Tokens) {
    if tokens.scrim.a() == 0 {
        return;
    }
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, tokens.scrim);
}

/// The dock's backdrop fade: clear at the top, the window base at 92 % at the
/// bottom, so content scrolls under the floating dock without a hard rule.
pub fn dock_scrim(ui: &Ui, rect: Rect, tokens: &Tokens) {
    if tokens.dock_scrim_bottom.a() == 0 {
        return;
    }
    let mesh = veil_mesh(
        rect,
        CornerRadius::ZERO,
        tokens.dock_scrim_top,
        tokens.dock_scrim_bottom,
    );
    ui.painter().add(egui::Shape::mesh(mesh));
}

/// The card anatomy: frosted panel + the standard [`CARD_PADDING`] + an
/// optional 1 px hairline divider `divider_after` points below the card top.
/// Returns the content rect (the card inset by the padding) for the caller's
/// title/body rows.
///
/// This is the helper slices 3–5 adopt; the compact screens still use
/// `compact::chrome::card` in this slice.
pub fn card(ui: &Ui, tokens: &Tokens, rect: Rect, divider_after: Option<f32>) -> Rect {
    frosted_panel(ui, tokens, rect, tokens.radius.card, Plane::Card);
    let content = rect.shrink(CARD_PADDING);
    if let Some(offset) = divider_after {
        let y = rect.top() + offset;
        if y > rect.top() && y < rect.bottom() {
            ui.painter().line_segment(
                [Pos2::new(content.left(), y), Pos2::new(content.right(), y)],
                Stroke::new(1.0_f32, tokens.hairline),
            );
        }
    }
    content
}

/// The veil as a vertical gradient mesh with no texture (`WHITE_UV`).
///
/// A square panel is the plan's four-vertex quad (two triangles). A rounded
/// panel tessellates the same gradient around its corner arcs: a four-vertex
/// quad would paint past a 12 pt radius and square off the silhouette, which
/// is visible at the light ladder's 62 % top stop.
pub fn veil_mesh(rect: Rect, radius: CornerRadius, top: Color32, bottom: Color32) -> Mesh {
    gradient_mesh(rect, radius, top, bottom)
}

/// The one-time 64×64 micro-grain. Deterministic hash noise: each texel is
/// white or black at `alpha` (a zero-mean sparkle that breaks gradient banding
/// both ways instead of only lightening it). Same alpha ⇒ same bytes, on every
/// run and every platform.
pub fn grain_image(alpha: u8) -> egui::ColorImage {
    let mut pixels = Vec::with_capacity(GRAIN_TILE * GRAIN_TILE);
    for y in 0..GRAIN_TILE as u32 {
        for x in 0..GRAIN_TILE as u32 {
            let hash = hash_noise(x, y);
            // A slight per-texel ramp keeps the noise from reading as a fixed
            // stencil; the ceiling is the token alpha.
            let strength = 128 + (hash >> 24) % 128; // 128..=255
            let a = ((u32::from(alpha) * strength) / 255) as u8;
            pixels.push(if hash & 1 == 0 {
                Color32::from_white_alpha(a)
            } else {
                Color32::from_black_alpha(a)
            });
        }
    }
    egui::ColorImage::new([GRAIN_TILE, GRAIN_TILE], pixels)
}

/// The grain texture for this alpha, created once per alpha and cached in
/// `ctx.data`. `None` when the token is zero (SystemGlass) — no texture is
/// ever uploaded on macOS.
pub fn grain_texture(ctx: &Context, alpha: u8) -> Option<TextureHandle> {
    if alpha == 0 {
        return None;
    }
    let id = egui::Id::new(("suramak-material-grain", alpha));
    if let Some(texture) = ctx.data(|data| data.get_temp::<TextureHandle>(id)) {
        return Some(texture);
    }
    let texture = ctx.load_texture(
        format!("suramak-material-grain-{alpha}"),
        grain_image(alpha),
        TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification: egui::TextureFilter::Linear,
            wrap_mode: TextureWrapMode::Repeat,
            mipmap_mode: None,
        },
    );
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    Some(texture)
}

/// The token alpha as a texture byte (1.8 % → 5, 1.2 % → 3).
fn grain_alpha(tokens: &Tokens) -> u8 {
    (tokens.frost_grain_alpha.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn gradient_mesh(rect: Rect, radius: CornerRadius, top: Color32, bottom: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return mesh;
    }
    let at = |y: f32| gradient_color(rect, top, bottom, y);
    if radius == CornerRadius::ZERO {
        mesh.vertices = vec![
            Vertex {
                pos: rect.left_top(),
                uv: WHITE_UV,
                color: top,
            },
            Vertex {
                pos: rect.right_top(),
                uv: WHITE_UV,
                color: top,
            },
            Vertex {
                pos: rect.right_bottom(),
                uv: WHITE_UV,
                color: bottom,
            },
            Vertex {
                pos: rect.left_bottom(),
                uv: WHITE_UV,
                color: bottom,
            },
        ];
        mesh.indices = vec![0, 1, 2, 2, 3, 0];
        return mesh;
    }
    let points = rounded_perimeter(rect, radius);
    mesh.vertices.push(Vertex {
        pos: rect.center(),
        uv: WHITE_UV,
        color: at(rect.center().y),
    });
    for point in &points {
        mesh.vertices.push(Vertex {
            pos: *point,
            uv: WHITE_UV,
            color: at(point.y),
        });
    }
    for index in 0..points.len() {
        let a = 1 + index as u32;
        let b = 1 + ((index + 1) % points.len()) as u32;
        mesh.indices.extend_from_slice(&[0, a, b]);
    }
    mesh
}

fn grain_mesh(rect: Rect, radius: CornerRadius, texture: &TextureHandle) -> Mesh {
    let mut mesh = Mesh::with_texture(texture.id());
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return mesh;
    }
    let uv = |point: Pos2| {
        Pos2::new(
            (point.x - rect.left()) / GRAIN_TILE as f32,
            (point.y - rect.top()) / GRAIN_TILE as f32,
        )
    };
    if radius == CornerRadius::ZERO {
        mesh.vertices = vec![
            Vertex {
                pos: rect.left_top(),
                uv: uv(rect.left_top()),
                color: Color32::WHITE,
            },
            Vertex {
                pos: rect.right_top(),
                uv: uv(rect.right_top()),
                color: Color32::WHITE,
            },
            Vertex {
                pos: rect.right_bottom(),
                uv: uv(rect.right_bottom()),
                color: Color32::WHITE,
            },
            Vertex {
                pos: rect.left_bottom(),
                uv: uv(rect.left_bottom()),
                color: Color32::WHITE,
            },
        ];
        mesh.indices = vec![0, 1, 2, 2, 3, 0];
        return mesh;
    }
    let points = rounded_perimeter(rect, radius);
    mesh.vertices.push(Vertex {
        pos: rect.center(),
        uv: uv(rect.center()),
        color: Color32::WHITE,
    });
    for point in &points {
        mesh.vertices.push(Vertex {
            pos: *point,
            uv: uv(*point),
            color: Color32::WHITE,
        });
    }
    for index in 0..points.len() {
        let a = 1 + index as u32;
        let b = 1 + ((index + 1) % points.len()) as u32;
        mesh.indices.extend_from_slice(&[0, a, b]);
    }
    mesh
}

fn gradient_color(rect: Rect, top: Color32, bottom: Color32, y: f32) -> Color32 {
    let t = ((y - rect.top()) / rect.height()).clamp(0.0, 1.0);
    // `Color32` is premultiplied, so the gradient must interpolate the stored
    // channels as-is. (`motion::lerp_color` rebuilds via
    // `from_rgba_unmultiplied`, which would premultiply a second time and turn
    // the 4.5 % dark veil into an opaque grey — measured, not guessed.)
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        mix(top.r(), bottom.r()),
        mix(top.g(), bottom.g()),
        mix(top.b(), bottom.b()),
        mix(top.a(), bottom.a()),
    )
}

/// The clockwise perimeter of a rounded rect, starting at the NW arc's left
/// tangent. One point per corner-arc step plus the straight edges; a corner
/// with radius 0 contributes its square corner so mixed radii (a sheet with
/// only its top corners rounded) stay a closed rectilinear silhouette.
fn rounded_perimeter(rect: Rect, radius: CornerRadius) -> Vec<Pos2> {
    let limit = (rect.width().min(rect.height()) / 2.0).max(0.0);
    let nw = (radius.nw as f32).min(limit);
    let ne = (radius.ne as f32).min(limit);
    let se = (radius.se as f32).min(limit);
    let sw = (radius.sw as f32).min(limit);
    let mut points = Vec::new();
    let corner =
        |points: &mut Vec<Pos2>, center: Pos2, r: f32, start: f32, end: f32, square: Pos2| {
            if r <= 0.01 {
                if points
                    .last()
                    .is_none_or(|last: &Pos2| last.distance(square) >= 0.01)
                {
                    points.push(square);
                }
                return;
            }
            let steps = ((r * 1.2).ceil() as usize).clamp(4, 12);
            for step in 0..=steps {
                let t = step as f32 / steps as f32;
                let angle = start + (end - start) * t;
                let point = Pos2::new(center.x + r * angle.cos(), center.y + r * angle.sin());
                let duplicate = points
                    .last()
                    .is_some_and(|last: &Pos2| last.distance(point) < 0.01);
                if !duplicate {
                    points.push(point);
                }
            }
        };
    use std::f32::consts::PI;
    // NW: 180° → 270° (left edge up to the top edge).
    corner(
        &mut points,
        Pos2::new(rect.left() + nw, rect.top() + nw),
        nw,
        PI,
        1.5 * PI,
        rect.left_top(),
    );
    // NE: 270° → 360° (top edge across to the right edge).
    corner(
        &mut points,
        Pos2::new(rect.right() - ne, rect.top() + ne),
        ne,
        1.5 * PI,
        2.0 * PI,
        rect.right_top(),
    );
    // SE: 0° → 90°.
    corner(
        &mut points,
        Pos2::new(rect.right() - se, rect.bottom() - se),
        se,
        0.0,
        0.5 * PI,
        rect.right_bottom(),
    );
    // SW: 90° → 180°.
    corner(
        &mut points,
        Pos2::new(rect.left() + sw, rect.bottom() - sw),
        sw,
        0.5 * PI,
        PI,
        rect.left_bottom(),
    );
    points
}

/// The top straight edge plus the two upper corner tangents — the specular
/// arc's ink. Sheet corners (top-only radius) work: the lower arcs simply do
/// not exist.
fn upper_perimeter(rect: Rect, radius: CornerRadius) -> Vec<Pos2> {
    let limit = (rect.width().min(rect.height()) / 2.0).max(0.0);
    let nw = (radius.nw as f32).min(limit);
    let ne = (radius.ne as f32).min(limit);
    let mut points = Vec::new();
    let arc = |center: Pos2, radius: f32, start: f32, end: f32, points: &mut Vec<Pos2>| {
        if radius <= 0.01 {
            return;
        }
        // The specular is 1 px of ink — six segments per corner is plenty.
        let steps = 6;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let angle = start + (end - start) * t;
            let point = Pos2::new(
                center.x + radius * angle.cos(),
                center.y + radius * angle.sin(),
            );
            let duplicate = points
                .last()
                .is_some_and(|last: &Pos2| last.distance(point) < 0.01);
            if !duplicate {
                points.push(point);
            }
        }
    };
    use std::f32::consts::PI;
    // Inset by half a point so the 1 px stroke lands inside the panel.
    let y = rect.top() + 0.5;
    arc(
        Pos2::new(rect.left() + nw, rect.top() + nw),
        nw - 0.5,
        PI,
        1.5 * PI,
        &mut points,
    );
    push_unique(&mut points, Pos2::new(rect.left() + nw, y));
    push_unique(&mut points, Pos2::new(rect.right() - ne, y));
    arc(
        Pos2::new(rect.right() - ne, rect.top() + ne),
        ne - 0.5,
        1.5 * PI,
        2.0 * PI,
        &mut points,
    );
    points
}

/// Appends `point` unless it duplicates the last one (arcs share their
/// endpoints with the straight edges).
fn push_unique(points: &mut Vec<Pos2>, point: Pos2) {
    if points
        .last()
        .is_none_or(|last| last.distance(point) >= 0.01)
    {
        points.push(point);
    }
}

/// SplitMix64-flavoured integer hash: deterministic, dependency-free, and
/// stable across platforms (the grain's "same bytes on two runs" contract).
fn hash_noise(x: u32, y: u32) -> u32 {
    let mut hash = x
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add(y.wrapping_mul(0x85EB_CA77))
        .wrapping_add(0xC2B2_AE3D);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    hash ^= hash >> 12;
    hash = hash.wrapping_mul(0x297A_2D39);
    hash ^= hash >> 15;
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::theme::{self, AppTheme};

    #[test]
    fn system_glass_zeroes_every_frost_alpha() {
        for theme in [AppTheme::Light, AppTheme::Dark] {
            let painted = theme::tokens_for(theme, MaterialBackend::PaintedFrost);
            assert!(painted.frost_veil_top.a() > 0);
            assert!(painted.frost_grain_alpha > 0.0);

            let glass = theme::tokens_for(theme, MaterialBackend::SystemGlass);
            assert_eq!(glass.frost_veil_top, Color32::TRANSPARENT);
            assert_eq!(glass.frost_veil_bottom, Color32::TRANSPARENT);
            assert_eq!(glass.rim_specular, Color32::TRANSPARENT);
            assert_eq!(glass.rim_hairline, Color32::TRANSPARENT);
            assert_eq!(glass.frost_grain_alpha, 0.0);
            assert_eq!(glass.dock_scrim_top, Color32::TRANSPARENT);
            assert_eq!(glass.dock_scrim_bottom, Color32::TRANSPARENT);
            assert!(grain_texture(&Context::default(), 0).is_none());
            // The legacy ladder is untouched by the gate (no screen restyle).
            assert_eq!(glass.ink, painted.ink);
            assert_eq!(glass.card, painted.card);
            assert_eq!(glass.sheet, painted.sheet);
        }
    }

    #[test]
    fn the_backend_rule_is_the_probe() {
        assert_eq!(
            MaterialBackend::from_system_glass(true),
            MaterialBackend::SystemGlass
        );
        assert_eq!(
            MaterialBackend::from_system_glass(false),
            MaterialBackend::PaintedFrost
        );
        assert!(!MaterialBackend::SystemGlass.paints_frost());
        assert!(MaterialBackend::PaintedFrost.paints_frost());
        // The live probe: only macOS has a system material to pass through.
        #[cfg(target_os = "macos")]
        assert_eq!(MaterialBackend::detect(), MaterialBackend::SystemGlass);
        #[cfg(not(target_os = "macos"))]
        assert_eq!(MaterialBackend::detect(), MaterialBackend::PaintedFrost);
    }

    #[test]
    fn the_shadow_budget_is_cards_zero_and_float_planes_one() {
        assert_eq!(Plane::Card.shadow(), Shadow::NONE);
        for plane in [Plane::Sheet, Plane::Dock] {
            let shadow = plane.shadow();
            assert_eq!(shadow.blur, 12);
            assert_eq!(shadow.offset, [0, 4]);
            assert_eq!(shadow.color.a(), 56, "22 % of black");
            assert!(plane.takes_grain());
        }
        assert!(!Plane::Card.takes_grain());
    }

    #[test]
    fn the_veil_is_a_four_vertex_gradient_on_a_square_panel() {
        let mesh = veil_mesh(
            Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 60.0)),
            CornerRadius::ZERO,
            Color32::from_white_alpha(158),
            Color32::from_white_alpha(97),
        );
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.indices.len(), 6);
        assert_eq!(mesh.vertices[0].color.a(), 158);
        assert_eq!(mesh.vertices[3].color.a(), 97);
        assert_eq!(mesh.vertices[0].uv, WHITE_UV);
        assert!(mesh.texture_id == egui::TextureId::default());
    }

    #[test]
    fn a_rounded_veil_follows_the_arcs_instead_of_squaring_the_corners() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 60.0));
        let mesh = veil_mesh(
            rect,
            CornerRadius::same(12),
            Color32::from_white_alpha(158),
            Color32::from_white_alpha(97),
        );
        assert!(mesh.vertices.len() > 4);
        // Every perimeter vertex is inside the rect and none is the square
        // corner (the arc pulls them in by ~3.5 pt at 45°).
        for vertex in &mesh.vertices {
            assert!(rect.expand(0.01).contains(vertex.pos));
        }
        let corner = Pos2::new(0.0, 0.0);
        let nearest = mesh
            .vertices
            .iter()
            .map(|vertex| vertex.pos.distance(corner))
            .fold(f32::INFINITY, f32::min);
        assert!(
            nearest > 3.0,
            "arc point {nearest} pt from the square corner"
        );
    }

    /// FNV-1a over the premultiplied bytes — the across-run pin.
    fn grain_digest(image: &egui::ColorImage) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for pixel in &image.pixels {
            for byte in [pixel.r(), pixel.g(), pixel.b(), pixel.a()] {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
    }

    #[test]
    fn the_grain_is_deterministic_and_alpha_scaled() {
        let first = grain_image(5);
        let second = grain_image(5);
        assert_eq!(first.size, [64, 64]);
        assert_eq!(first.pixels, second.pixels, "same alpha, same bytes");
        println!("grain(5) digest = 0x{:016x}", grain_digest(&first));
        assert_eq!(
            grain_digest(&first),
            0x8510_5b98_62ee_29e9,
            "the 64×64 grain changed — the golden and every run depend on these bytes"
        );
        // Both tones exist (white *and* black texels), and every texel is at
        // or under the token alpha. `Color32` is premultiplied, so a white
        // texel carries `r == a` and a black one `r == 0`.
        assert!(first.pixels.iter().any(|pixel| pixel.r() > 0));
        assert!(first.pixels.iter().any(|pixel| pixel.r() == 0));
        assert!(first.pixels.iter().all(|pixel| pixel.a() <= 5));
        let lighter = grain_image(8);
        assert_ne!(first.pixels, lighter.pixels);
        assert!(lighter.pixels.iter().all(|pixel| pixel.a() <= 8));
    }

    #[test]
    fn card_padding_is_the_standard_twelve_points() {
        let ctx = Context::default();
        let tokens = theme::tokens_for(AppTheme::Dark, MaterialBackend::PaintedFrost);
        let mut content = Rect::NOTHING;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = Rect::from_min_size(Pos2::new(10.0, 10.0), egui::vec2(200.0, 120.0));
                content = card(ui, &tokens, rect, Some(48.0));
            });
        });
        assert_eq!(content.left(), 22.0);
        assert_eq!(content.top(), 22.0);
        assert_eq!(content.right(), 198.0);
        assert_eq!(content.bottom(), 118.0);
    }
}

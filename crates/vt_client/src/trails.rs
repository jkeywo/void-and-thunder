//! Trail ribbons: the glowing wake a drive plume or a torpedo leaves behind it.
//!
//! A ribbon is a breadcrumb history. Each frame the emitter drops a [`Crumb`] at
//! its current world position (or slides the existing head crumb, if it hasn't
//! travelled far enough to earn a new one), every crumb ages, and the whole
//! deque is rebuilt into a flat triangle strip. The strip lies in the XY plane —
//! the sim's plane, with +Z up — so it reads properly under the game's
//! looking-down camera.
//!
//! Ported from `project-phoenix-v2`'s `server/pfx.rs` engine trails, but without
//! its four-texture WGSL material: all colour here comes from **vertex colours**
//! on a single shared additive [`StandardMaterial`], so there is no new shader,
//! no new texture asset, and nothing extra for the wasm build to load.
//!
//! The ribbon core ([`commit_crumb`], [`age_crumbs`], [`rebuild_mesh`]) is pure and
//! knows nothing about ships; the drivers on top of it point it at a ship's two
//! sterns ([`attach_engine_trails`]) or a torpedo's tail
//! ([`attach_torpedo_trails`]).

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use std::collections::VecDeque;
use vt_sim::prelude::*;

use crate::data::feel::RibbonFeel;
use crate::data::FeelTuning;
use crate::interpolate::SmoothingSet;

// Engine and torpedo trail tuning lives in `FeelTuning::trails` (see
// `data/feel.rs`), so a plume can be reshaped with the game running.

/// Mounts the trail ribbons: the shared material plus the attach/update systems.
pub struct TrailPlugin;

impl Plugin for TrailPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TrailAssets>().add_systems(
            Update,
            (
                attach_engine_trails,
                attach_torpedo_trails,
                update_trails.after(attach_engine_trails),
            )
                // Emitters are read off `Transform`, so run after the fixed-step
                // smoothing or the plume would be laid down in 64 Hz hops while
                // the hull it comes out of glides between them.
                .after(SmoothingSet),
        );
    }
}

/// The one material every ribbon shares. It is deliberately featureless —
/// white, unlit, additive — because the mesh's vertex colours override
/// `base_color` outright in Bevy's PBR fragment shader. One material for every
/// trail in the scene means no per-ship material churn.
#[derive(Resource)]
pub struct TrailAssets {
    material: Handle<StandardMaterial>,
}

impl FromWorld for TrailAssets {
    fn from_world(world: &mut World) -> Self {
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        Self {
            material: materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                alpha_mode: AlphaMode::Add,
                // The ribbon is a flat strip; keep it visible from either face.
                cull_mode: None,
                double_sided: true,
                ..default()
            }),
        }
    }
}

/// One point in a ribbon's history.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Crumb {
    pos: Vec3,
    /// Full width of the ribbon here, before the age taper.
    width: f32,
    /// Seconds since this crumb was laid down.
    age: f32,
    /// How hard the emitter was burning when it laid this crumb (0..=1). Drives
    /// the crumb's brightness, so easing off the throttle dims the plume from
    /// the nozzle backwards rather than cutting it dead.
    glow: f32,
}

/// The shape of one kind of ribbon, as the renderer wants it.
///
/// Built from the authored [`RibbonFeel`] rather than being the authored type
/// itself: colours are `LinearRgba` here and plain arrays in the file, because a
/// designer editing a colour wants three numbers, not a colour space.
#[derive(Clone, Copy, Debug)]
struct RibbonCfg {
    /// Colour at the head of the ribbon.
    hot: LinearRgba,
    /// Colour it cools to at the tail.
    cool: LinearRgba,
    /// Base full width at the head, in world units.
    width: f32,
    /// Seconds a crumb survives.
    lifetime: f32,
    /// Hard cap on history length.
    max_crumbs: usize,
    /// How far the emitter must travel before a new crumb is laid rather than
    /// the head one being slid along.
    min_step: f32,
    /// How much of its width a crumb loses by the end of its life, on top of the
    /// alpha fade — so the ribbon tapers to a point rather than a blunt edge.
    width_falloff: f32,
}

impl RibbonCfg {
    fn from_feel(feel: &RibbonFeel, width_falloff: f32) -> Self {
        Self {
            hot: LinearRgba::rgb(feel.hot[0], feel.hot[1], feel.hot[2]),
            cool: LinearRgba::rgb(feel.cool[0], feel.cool[1], feel.cool[2]),
            width: feel.width,
            lifetime: feel.lifetime,
            max_crumbs: feel.max_crumbs as usize,
            min_step: feel.min_step,
            width_falloff,
        }
    }
}

/// A ribbon streaming from `source`. Lives on its **own** entity with an
/// identity transform, because its geometry is built in world space — it must
/// not move with the ship that is paying it out.
#[derive(Component)]
pub struct Ribbon {
    /// The entity this ribbon streams from. When it goes, so does the ribbon.
    source: Entity,
    /// Where on the source, in the source's local frame, the ribbon emits from.
    offset: Vec3,
    /// Newest crumb first, oldest last.
    crumbs: VecDeque<Crumb>,
    mesh: Handle<Mesh>,
    cfg: RibbonCfg,
}

/// Marks a ship/torpedo whose ribbons have already been spawned, so they are
/// only attached once.
#[derive(Component)]
pub struct TrailsAttached;

// ================================ ribbon core ================================

/// Commit a crumb at `pos`, if the emitter has pulled [`RibbonCfg::min_step`]
/// clear of the last committed one. Returns nothing — the caller pins the
/// ribbon's *visible* head to the emitter separately (see [`rebuild_mesh`]).
///
/// Committed crumbs never move. An earlier version slid the head crumb along to
/// the emitter each frame instead of committing a new one, which looked
/// equivalent and was not: the next frame then measured its distance from the
/// already-slid head, so the test only ever saw a *single frame* of travel
/// (~2 units at cruise) against a 5-unit step. A second crumb was never laid,
/// the ribbon stayed one point long, and it drew nothing except on frames long
/// enough to cover the whole step at once — an engine trail that flickered on
/// only when the frame rate hitched.
fn commit_crumb(crumbs: &mut VecDeque<Crumb>, pos: Vec3, width: f32, glow: f32, cfg: &RibbonCfg) {
    let far_enough = crumbs
        .front()
        .is_none_or(|last| last.pos.distance(pos) >= cfg.min_step);
    if !far_enough {
        return;
    }

    crumbs.push_front(Crumb {
        pos,
        width,
        age: 0.0,
        glow,
    });
    while crumbs.len() > cfg.max_crumbs {
        crumbs.pop_back();
    }
}

/// Age every crumb by `dt` and retire the expired ones off the tail.
fn age_crumbs(crumbs: &mut VecDeque<Crumb>, dt: f32, cfg: &RibbonCfg) {
    for crumb in crumbs.iter_mut() {
        crumb.age += dt;
    }
    while crumbs.back().is_some_and(|c| c.age >= cfg.lifetime) {
        crumbs.pop_back();
    }
}

/// Rebuild the ribbon geometry in place: two vertices per crumb, one quad per
/// gap. Colour runs `hot` → `cool` head-to-tail and alpha fades with age, so the
/// whole thing dissolves behind the emitter.
///
/// `head` is the emitter's live position while it is burning. It is drawn in
/// front of the committed history but never stored, which is what keeps the
/// ribbon welded to the nozzle between commits — and what lets a ribbon draw
/// from its very first committed crumb rather than waiting for a second.
fn rebuild_mesh(mesh: &mut Mesh, crumbs: &VecDeque<Crumb>, head: Option<Crumb>, cfg: &RibbonCfg) {
    // The head only earns a place if it is actually clear of the newest
    // committed crumb; sitting on top of it would give a zero-length tangent.
    let head = head.filter(|h| {
        crumbs
            .front()
            .is_none_or(|last| last.pos.distance(h.pos) > 1e-3)
    });

    let points: Vec<&Crumb> = head.iter().chain(crumbs.iter()).collect();
    let n = points.len();
    if n < 2 {
        write_blank(mesh);
        return;
    }

    let mut positions = Vec::with_capacity(n * 2);
    let mut normals = Vec::with_capacity(n * 2);
    let mut uvs = Vec::with_capacity(n * 2);
    let mut colors = Vec::with_capacity(n * 2);
    let mut indices = Vec::with_capacity((n - 1) * 6);

    for (i, crumb) in points.iter().enumerate() {
        // Central-difference tangent, pointing toward the newer crumb.
        let tangent = match i {
            0 => points[0].pos - points[1].pos,
            i if i == n - 1 => points[n - 2].pos - points[n - 1].pos,
            i => points[i - 1].pos - points[i + 1].pos,
        };
        // The ribbon lies flat in the sim's XY plane. A torpedo climbing
        // straight up has a tangent parallel to Z and no in-plane perpendicular,
        // so fall back to a fixed axis rather than collapsing to zero width.
        let perp = tangent.cross(Vec3::Z).try_normalize().unwrap_or(Vec3::X);

        let t = i as f32 / (n - 1) as f32;
        let age_frac = (crumb.age / cfg.lifetime.max(1e-4)).clamp(0.0, 1.0);
        let half = crumb.width * (1.0 - age_frac * cfg.width_falloff) * 0.5;

        positions.push((crumb.pos - perp * half).to_array());
        positions.push((crumb.pos + perp * half).to_array());
        normals.push([0.0, 0.0, 1.0]);
        normals.push([0.0, 0.0, 1.0]);
        uvs.push([t, 0.0]);
        uvs.push([t, 1.0]);

        let rgb = lerp_rgb(cfg.hot, cfg.cool, t);
        let alpha = (1.0 - age_frac) * crumb.glow;
        let color = [rgb.red, rgb.green, rgb.blue, alpha];
        colors.push(color);
        colors.push(color);

        // Two triangles per gap, wound counter-clockwise seen from +Z (where
        // the camera always is).
        if i < n - 1 {
            let v = (i * 2) as u32;
            indices.extend_from_slice(&[v, v + 2, v + 3, v, v + 3, v + 1]);
        }
    }

    write_mesh(mesh, positions, normals, uvs, colors, indices);
}

/// Replace every attribute on `mesh` in one go, so the empty and populated
/// paths can't drift apart.
fn write_mesh(
    mesh: &mut Mesh,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
) {
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
}

/// Linear blend between two ribbon colours.
fn lerp_rgb(from: LinearRgba, to: LinearRgba, t: f32) -> LinearRgba {
    LinearRgba::rgb(
        from.red + (to.red - from.red) * t,
        from.green + (to.green - from.green) * t,
        from.blue + (to.blue - from.blue) * t,
    )
}

/// Blank the ribbon out — a single zero-area, zero-alpha triangle rather than a
/// genuinely empty mesh.
///
/// A mesh with no vertices gets no allocation in Bevy's render-side slab, but
/// the upload still tries to copy into one, which spams
/// `slab_allocator: Use-after-free` every frame for every idle ribbon. A
/// degenerate triangle keeps the allocation alive and rasterizes nothing.
fn write_blank(mesh: &mut Mesh) {
    write_mesh(
        mesh,
        vec![[0.0, 0.0, 0.0]; 3],
        vec![[0.0, 0.0, 1.0]; 3],
        vec![[0.0, 0.0]; 3],
        vec![[0.0, 0.0, 0.0, 0.0]; 3],
        vec![0, 1, 2],
    );
}

/// A blank ribbon mesh, ready to be filled in each frame.
fn empty_ribbon_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    write_blank(&mut mesh);
    mesh
}

/// Spawn one ribbon entity streaming from `source` at a local `offset`.
fn spawn_ribbon(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    assets: &TrailAssets,
    source: Entity,
    offset: Vec3,
    cfg: RibbonCfg,
) {
    let mesh = meshes.add(empty_ribbon_mesh());
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(assets.material.clone()),
        Transform::default(),
        // The geometry is world-space under an identity transform, so Bevy's
        // frustum test would cull it against the wrong bounds.
        NoFrustumCulling,
        Ribbon {
            source,
            offset,
            crumbs: VecDeque::new(),
            mesh,
            cfg,
        },
    ));
}

// ================================== drivers ==================================

/// Give every new ship its drive plumes.
///
/// Anchors come from the model's rig sidecar (`<model>.model.ron`) when it
/// has one — each hull streams from its own authored points — and fall back
/// to the global feel-tuning nacelle pair otherwise, which is the exact
/// pre-sidecar behaviour.
pub fn attach_engine_trails(
    mut commands: Commands,
    assets: Res<TrailAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    ships: Query<(Entity, &Faction), (With<Ship>, Without<TrailsAttached>)>,
    feel: Res<FeelTuning>,
    rigs: Res<crate::data::ModelRigs>,
) {
    let trails = feel.trails;
    let fallback = [
        Vec3::new(trails.stern_x, trails.nacelle_y, 0.0),
        Vec3::new(trails.stern_x, -trails.nacelle_y, 0.0),
    ];
    for (ship, faction) in &ships {
        let (model, _) = crate::ship_model(faction);
        let anchors = rigs.trail_anchors(model).unwrap_or(&fallback);
        for anchor in anchors {
            spawn_ribbon(
                &mut commands,
                &mut meshes,
                &assets,
                ship,
                *anchor,
                RibbonCfg::from_feel(&trails.engine, trails.width_falloff),
            );
        }
        commands.entity(ship).insert(TrailsAttached);
    }
}

/// Give every torpedo a burn trail off its tail.
pub fn attach_torpedo_trails(
    mut commands: Commands,
    assets: Res<TrailAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    torpedoes: Query<Entity, (With<Torpedo>, Without<TrailsAttached>)>,
    feel: Res<FeelTuning>,
) {
    let trails = feel.trails;
    for torpedo in &torpedoes {
        spawn_ribbon(
            &mut commands,
            &mut meshes,
            &assets,
            torpedo,
            Vec3::new(0.0, 0.0, trails.torpedo_tail_z),
            RibbonCfg::from_feel(&trails.torpedo, trails.width_falloff),
        );
        commands.entity(torpedo).insert(TrailsAttached);
    }
}

/// Advance every ribbon: age its history, lay a fresh crumb at its emitter if
/// that emitter is burning, and rebuild the strip.
///
/// A ribbon whose source has despawned is despawned with it — which is also what
/// cleans trails up on restart, since `session::restart` only despawns ships.
pub fn update_trails(
    time: Res<Time>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut ribbons: Query<(Entity, &mut Ribbon)>,
    sources: Query<(
        &Transform,
        Option<&Helm>,
        Option<&Velocity>,
        Option<&ShipStats>,
        Option<&BoostDrive>,
        Has<Disabled>,
    )>,
    feel: Res<FeelTuning>,
) {
    let trails = feel.trails;
    let dt = time.delta_secs();
    for (entity, mut ribbon) in &mut ribbons {
        let Ok((transform, helm, velocity, stats, boost, disabled)) = sources.get(ribbon.source)
        else {
            commands.entity(entity).despawn();
            continue;
        };

        let cfg = ribbon.cfg;
        age_crumbs(&mut ribbon.crumbs, dt, &cfg);

        // While the emitter burns, its live position is both committed to the
        // history (every `min_step`) and drawn as the ribbon's head every frame.
        let head = burn(
            &cfg,
            helm,
            velocity,
            stats,
            boost,
            disabled,
            trails.throttle_deadzone,
            trails.boost_width,
        )
        .map(|(width, glow)| {
            let pos = transform.transform_point(ribbon.offset);
            commit_crumb(&mut ribbon.crumbs, pos, width, glow, &cfg);
            Crumb {
                pos,
                width,
                age: 0.0,
                glow,
            }
        });

        if let Some(mut mesh) = meshes.get_mut(&ribbon.mesh) {
            rebuild_mesh(&mut mesh, &ribbon.crumbs, head, &cfg);
        }
    }
}

/// How hard this emitter is burning right now, as `(width, glow)` — or `None`
/// when it is cold and should lay no crumb at all.
///
/// A ship burns on forward throttle only (backing sails throw no plume) and
/// never as a crippled hulk, since a cripple is a ship with a dead drive. A
/// torpedo — anything with no [`Helm`] — burns flat out for its whole flight.
#[allow(clippy::too_many_arguments)]
fn burn(
    cfg: &RibbonCfg,
    helm: Option<&Helm>,
    velocity: Option<&Velocity>,
    stats: Option<&ShipStats>,
    boost: Option<&BoostDrive>,
    disabled: bool,
    deadzone: f32,
    boost_width: f32,
) -> Option<(f32, f32)> {
    let Some(helm) = helm else {
        return Some((cfg.width, 1.0));
    };

    let throttle = helm.throttle.clamp(0.0, 1.0);
    if disabled || throttle <= deadzone {
        return None;
    }

    // Blend throttle with how much of its top speed the ship is actually
    // making, so a plume builds as the hull picks up rather than snapping to
    // full the instant the key goes down.
    let speed_frac = match (velocity, stats) {
        (Some(v), Some(s)) if s.max_speed > 0.0 => (v.0.length() / s.max_speed).clamp(0.0, 1.0),
        _ => 1.0,
    };
    let drive = throttle * (0.45 + 0.55 * speed_frac);

    let boosting = boost.is_some_and(BoostDrive::engaged);
    let width = cfg.width * (0.5 + 0.5 * drive) * if boosting { boost_width } else { 1.0 };
    let glow = if boosting {
        1.0
    } else {
        (drive * 0.85).min(1.0)
    };
    Some((width, glow))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feel() -> crate::data::feel::TrailFeel {
        crate::data::feel::TrailFeel::default()
    }

    fn cfg() -> RibbonCfg {
        let f = feel();
        RibbonCfg::from_feel(&f.engine, f.width_falloff)
    }

    /// A crumb that hasn't cleared `min_step` isn't committed — and, crucially,
    /// the one already there does **not** move to meet it. Committed crumbs are
    /// fixed points in the world; the live emitter is drawn separately.
    #[test]
    fn a_short_step_commits_nothing_and_moves_nothing() {
        let cfg = cfg();
        let mut crumbs = VecDeque::new();
        commit_crumb(&mut crumbs, Vec3::ZERO, 5.0, 1.0, &cfg);
        commit_crumb(
            &mut crumbs,
            Vec3::new(cfg.min_step * 0.5, 0.0, 0.0),
            5.0,
            1.0,
            &cfg,
        );

        assert_eq!(crumbs.len(), 1, "a sub-step move should not commit a crumb");
        assert_eq!(crumbs[0].pos.x, 0.0, "committed crumbs never move");
    }

    #[test]
    fn a_full_step_commits_a_new_crumb_at_the_front() {
        let cfg = cfg();
        let mut crumbs = VecDeque::new();
        commit_crumb(&mut crumbs, Vec3::ZERO, 5.0, 1.0, &cfg);
        commit_crumb(
            &mut crumbs,
            Vec3::new(cfg.min_step * 1.5, 0.0, 0.0),
            5.0,
            1.0,
            &cfg,
        );

        assert_eq!(crumbs.len(), 2);
        assert_eq!(crumbs[0].pos.x, cfg.min_step * 1.5, "newest crumb is first");
        assert_eq!(crumbs[1].pos.x, 0.0, "the older crumb falls back");
    }

    /// The regression that matters: fly a ship at cruise speed for a few seconds
    /// at a normal frame rate and the plume must be drawable on **every** frame
    /// after the first.
    ///
    /// The original bug slid the head crumb to the emitter each frame, so the
    /// distance test only ever saw one frame of travel (~2 units) against a
    /// 5-unit step. A second crumb was never committed, the ribbon stayed one
    /// point long, and it drew nothing but for the odd long frame — an engine
    /// trail that only flickered in when the frame rate hitched.
    #[test]
    fn a_ship_at_cruise_trails_continuously() {
        let cfg = cfg();
        let dt = 1.0 / 60.0;
        let speed = 127.5; // ShipStats::default().max_speed
        let step = speed * dt; // ~2.1 units per frame, well under min_step

        let mut crumbs = VecDeque::new();
        let mut pos = Vec3::ZERO;
        let mut blank_frames = 0;

        for frame in 0..180 {
            age_crumbs(&mut crumbs, dt, &cfg);
            pos.x += step;
            commit_crumb(&mut crumbs, pos, cfg.width, 1.0, &cfg);
            let head = Crumb {
                pos,
                width: cfg.width,
                age: 0.0,
                glow: 1.0,
            };

            let mut mesh = empty_ribbon_mesh();
            rebuild_mesh(&mut mesh, &crumbs, Some(head), &cfg);
            // Frame 0 legitimately has only the head and nothing behind it.
            if frame > 0 && mesh.count_vertices() <= 3 {
                blank_frames += 1;
            }
        }

        assert_eq!(
            blank_frames, 0,
            "the plume went blank on {blank_frames} frames of steady cruise"
        );
    }

    /// The head is drawn but never stored, so the history stays a coarse trail
    /// of `min_step` waypoints however fast the frames tick by.
    #[test]
    fn the_live_head_is_drawn_without_being_committed() {
        let cfg = cfg();
        let mut crumbs = VecDeque::new();
        commit_crumb(&mut crumbs, Vec3::ZERO, 5.0, 1.0, &cfg);

        // One committed crumb plus a live head is already a drawable ribbon.
        let head = Crumb {
            pos: Vec3::new(cfg.min_step * 0.5, 0.0, 0.0),
            width: 5.0,
            age: 0.0,
            glow: 1.0,
        };
        let mut mesh = empty_ribbon_mesh();
        rebuild_mesh(&mut mesh, &crumbs, Some(head), &cfg);

        assert_eq!(mesh.count_vertices(), 4, "head + one crumb => one quad");
        assert_eq!(crumbs.len(), 1, "drawing the head must not commit it");
    }

    #[test]
    fn history_is_capped() {
        let cfg = cfg();
        let mut crumbs = VecDeque::new();
        for i in 0..(cfg.max_crumbs * 2) {
            let x = i as f32 * cfg.min_step * 2.0;
            commit_crumb(&mut crumbs, Vec3::new(x, 0.0, 0.0), 5.0, 1.0, &cfg);
        }
        assert_eq!(crumbs.len(), cfg.max_crumbs);
    }

    #[test]
    fn expired_crumbs_retire_off_the_tail() {
        let cfg = cfg();
        let mut crumbs = VecDeque::new();
        commit_crumb(&mut crumbs, Vec3::ZERO, 5.0, 1.0, &cfg);
        commit_crumb(
            &mut crumbs,
            Vec3::new(cfg.min_step * 2.0, 0.0, 0.0),
            5.0,
            1.0,
            &cfg,
        );

        age_crumbs(&mut crumbs, cfg.lifetime + 0.01, &cfg);
        assert!(crumbs.is_empty(), "everything should have aged out");
    }

    #[test]
    fn a_ribbon_is_two_verts_per_crumb_and_two_triangles_per_gap() {
        let cfg = cfg();
        let mut crumbs = VecDeque::new();
        for i in 0..4 {
            commit_crumb(
                &mut crumbs,
                Vec3::new(i as f32 * cfg.min_step * 2.0, 0.0, 0.0),
                5.0,
                1.0,
                &cfg,
            );
        }

        let mut mesh = empty_ribbon_mesh();
        rebuild_mesh(&mut mesh, &crumbs, None, &cfg);

        assert_eq!(mesh.count_vertices(), 8, "4 crumbs => 8 vertices");
        assert_eq!(
            mesh.indices().map(|i| i.len()),
            Some(3 * 6),
            "3 gaps => 6 indices each"
        );
    }

    /// A ribbon needs two points to have a direction, so one crumb draws
    /// nothing — but as a *collapsed* triangle, never a vertex-less mesh, which
    /// would make Bevy's slab allocator scream once per frame per idle ribbon.
    #[test]
    fn a_single_crumb_draws_a_collapsed_triangle_not_an_empty_mesh() {
        let cfg = cfg();
        let mut crumbs = VecDeque::new();
        commit_crumb(&mut crumbs, Vec3::ZERO, 5.0, 1.0, &cfg);

        let mut mesh = empty_ribbon_mesh();
        rebuild_mesh(&mut mesh, &crumbs, None, &cfg);

        assert_eq!(mesh.count_vertices(), 3, "must never be a vertex-less mesh");
        assert_eq!(mesh.indices().map(|i| i.len()), Some(3));
    }

    /// The plume is the drive burning — a crippled hulk's drive is dead, and
    /// backing sails throw nothing.
    #[test]
    fn a_cold_drive_lays_no_crumb() {
        let cfg = cfg();
        let f = feel();
        let idle = Helm {
            throttle: 0.0,
            turn: 0.0,
        };
        let ahead = Helm {
            throttle: 1.0,
            turn: 0.0,
        };
        let astern = Helm {
            throttle: -1.0,
            turn: 0.0,
        };

        assert!(burn(
            &cfg,
            Some(&idle),
            None,
            None,
            None,
            false,
            f.throttle_deadzone,
            f.boost_width
        )
        .is_none());
        assert!(burn(
            &cfg,
            Some(&astern),
            None,
            None,
            None,
            false,
            f.throttle_deadzone,
            f.boost_width
        )
        .is_none());
        assert!(
            burn(
                &cfg,
                Some(&ahead),
                None,
                None,
                None,
                true,
                f.throttle_deadzone,
                f.boost_width
            )
            .is_none(),
            "a crippled hulk should not burn"
        );
        assert!(burn(
            &cfg,
            Some(&ahead),
            None,
            None,
            None,
            false,
            f.throttle_deadzone,
            f.boost_width
        )
        .is_some());
    }

    /// A torpedo has no helm — it burns flat out until it hits something.
    #[test]
    fn a_torpedo_always_burns() {
        let f = feel();
        let torpedo = RibbonCfg::from_feel(&f.torpedo, f.width_falloff);
        let (width, glow) = burn(
            &torpedo,
            None,
            None,
            None,
            None,
            false,
            f.throttle_deadzone,
            f.boost_width,
        )
        .expect("a torpedo should always lay a trail");
        assert_eq!(width, torpedo.width);
        assert_eq!(glow, 1.0);
    }
}

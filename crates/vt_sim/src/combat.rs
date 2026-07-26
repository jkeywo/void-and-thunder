//! Combat: broadside firing, projectile flight, collision and damage.
//!
//! As with movement, the geometry and damage rules are pure functions so they
//! can be tested headlessly; the Bevy systems wrap them and touch the `World`.

use bevy_ecs::prelude::*;
use bevy_math::{Vec2, Vec3};
use bevy_reflect::Reflect;
use bevy_time::Time;
use bevy_transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::components::{
    Brace, Collider, Faction, FireOrders, Heading, Hull, Invulnerable, Projectile, Ttl, Velocity,
};
use crate::events::{ShipDestroyed, ShipHit};
use crate::shield::{shield_arc, DamageReport, Shield, ShieldArc};
use crate::tuning::SimTuning;
use crate::util::wrap_angle;

// Defaults for the combat rules. Each is mirrored by a [`SimTuning`] field that
// systems actually read, so a designer can move them at runtime; these stay as
// the documented starting values and the fallback when no data file is loaded.

/// Fraction of damage a braced ship still takes (Black-Flag brace).
pub const BRACE_DAMAGE_FACTOR: f32 = 0.35;

/// How long a cannonball lives before falling into the void (seconds).
pub const PROJECTILE_TTL: f32 = 2.5;
/// Cannonball collision radius.
pub const PROJECTILE_RADIUS: f32 = 5.0;
/// Length of hull the guns of a volley are spread along.
pub const HULL_LENGTH: f32 = 40.0;
/// How far ahead of the ship a volley's muzzles sit.
pub const MUZZLE_STANDOFF: f32 = 22.0;

/// Per-side reload + telegraph state for one broadside bank. Port and starboard
/// each carry their own, so the two sides reload and fire independently.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct BankState {
    /// Remaining reload until this side can fire again (0 = ready).
    pub timer: f32,
    /// Remaining wind-up if this side is currently charging (0 = idle). Read by
    /// the client to draw the enemy telegraph.
    pub charging: f32,
    /// World direction captured when this side's charge began.
    pub charge_dir: Vec2,
}

/// A pair of side-mounted railgun banks — the core aimed weapon. The player may
/// steer each volley within `arc` of the beam; an enemy telegraphs a
/// `charge_time` wind-up before firing. The two sides ([`BankState`]) reload on
/// independent timers.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Broadside {
    /// Seconds between volleys (per side).
    pub cooldown: f32,
    /// Damage per shot.
    pub damage: f32,
    /// Muzzle speed of each shot (units/s), added to the ship's velocity.
    pub muzzle_speed: f32,
    /// Number of guns per side (shots per volley, spread along the hull).
    pub guns: u32,
    /// Half-angle the volley may be steered from the beam (radians). The full
    /// firing arc is `2 * arc`, centred straight out the beam — equal reach fore
    /// and aft, never biased toward the bow.
    pub arc: f32,
    /// Wind-up before firing (seconds). 0 for the player; ~0.5 for a telegraphed
    /// enemy so the shot is dodgeable.
    pub charge_time: f32,
    /// Port bank (the ship's left, +90° from the bow). Live reload/charge
    /// state — never authored, so a saved class can't carry a half-spent timer.
    #[serde(skip)]
    pub port: BankState,
    /// Starboard bank (the ship's right, -90°). Live state, as `port`.
    #[serde(skip)]
    pub starboard: BankState,
}

impl Default for Broadside {
    fn default() -> Self {
        Self {
            cooldown: 1.5,
            damage: 12.0,
            // Flat shots that arrive quickly — with [`PROJECTILE_TTL`] this also
            // sets the bank's reach (speed × time to live).
            muzzle_speed: 325.0,
            guns: 3,
            // 67.5° either way — a 135° arc centred straight out the beam.
            arc: std::f32::consts::FRAC_PI_2 * 0.75,
            charge_time: 0.0,
            port: BankState::default(),
            starboard: BankState::default(),
        }
    }
}

impl Broadside {
    /// The reload/charge state for one side.
    pub fn side(&self, port: bool) -> &BankState {
        if port {
            &self.port
        } else {
            &self.starboard
        }
    }

    /// Mutable reload/charge state for one side.
    pub fn side_mut(&mut self, port: bool) -> &mut BankState {
        if port {
            &mut self.port
        } else {
            &mut self.starboard
        }
    }

    /// True when a side has reloaded and isn't mid wind-up — i.e. it can fire now.
    pub fn ready(&self, port: bool) -> bool {
        let s = self.side(port);
        s.timer <= 0.0 && s.charging <= 0.0
    }
}

/// One cannonball's spawn state: where it appears and how fast it travels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectileSpawn {
    pub position: Vec2,
    pub velocity: Vec2,
}

/// The world direction a broadside on `heading` throws along, given the pilot's
/// desired `aim` (or `None` to fire straight out the beam), clamped to `±arc` of
/// the beam. `port` is the ship's left beam (+90° from bow), else starboard.
pub fn broadside_direction(heading: f32, port: bool, aim: Option<Vec2>, arc: f32) -> Vec2 {
    use std::f32::consts::FRAC_PI_2;
    let beam = if port {
        heading + FRAC_PI_2
    } else {
        heading - FRAC_PI_2
    };
    let angle = match aim {
        Some(dir) if dir.length_squared() > 1e-6 => {
            let off = wrap_angle(dir.to_angle() - beam).clamp(-arc, arc);
            beam + off
        }
        _ => beam,
    };
    Vec2::from_angle(angle)
}

/// Compute the volley of shots a broadside throws along `fire_dir` (a unit
/// world direction). Guns are spread along the hull (perpendicular to the fire
/// direction) so the volley leaves a line, not a point. Muzzle velocity is added
/// to the ship's own velocity — you inherit the ship's momentum.
pub fn broadside_volley(
    ship_pos: Vec2,
    ship_vel: Vec2,
    fire_dir: Vec2,
    bank: &Broadside,
    hull_length: f32,
    muzzle_standoff: f32,
) -> Vec<ProjectileSpawn> {
    let dir = fire_dir.normalize_or(Vec2::X);
    let along = dir.perp(); // spread the guns across the hull
    let guns = bank.guns.max(1);

    let mut out = Vec::with_capacity(guns as usize);
    for i in 0..guns {
        let t = if guns == 1 {
            0.0
        } else {
            (i as f32 / (guns - 1) as f32) - 0.5
        };
        let muzzle = ship_pos + along * (t * hull_length) + dir * muzzle_standoff;
        let velocity = ship_vel + dir * bank.muzzle_speed;
        out.push(ProjectileSpawn {
            position: muzzle,
            velocity,
        });
    }
    out
}

/// Where a moving target will be when a shot fired now would reach it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lead {
    /// World point the target will occupy at impact — what to put the volley
    /// through.
    pub point: Vec2,
    /// Seconds of flight until then.
    pub time: f32,
}

/// Solve the firing lead against a target moving at constant velocity.
///
/// Shots inherit the ship's momentum ([`broadside_volley`]), so the shooter's
/// own velocity cancels out of the geometry: relative to the ship the shot
/// always leaves at `muzzle_speed`, and the target closes at
/// `target_vel - shooter_vel`. Writing that out, a shot fired at time 0 meets
/// the target when
///
/// ```text
///   (|v|² - s²)·t² + 2(d·v)·t + |d|² = 0
/// ```
///
/// for `d` the offset to the target, `v` the relative velocity and `s` the
/// muzzle speed — the earliest positive root is the flight time.
///
/// Returns `None` when nothing can catch the target: it is outrunning the shot,
/// or is already on top of the muzzle. The returned `point` is the target's true
/// world position at impact, so it can be aimed at directly — the drawn aim
/// beams already carry the ship's momentum, so lining a beam up with this point
/// *is* the firing solution.
pub fn intercept_lead(
    muzzle: Vec2,
    muzzle_speed: f32,
    shooter_vel: Vec2,
    target: Vec2,
    target_vel: Vec2,
) -> Option<Lead> {
    let d = target - muzzle;
    let v = target_vel - shooter_vel;
    let a = v.length_squared() - muzzle_speed * muzzle_speed;
    let b = 2.0 * d.dot(v);
    let c = d.length_squared();

    // Degenerate: the target is receding at exactly the muzzle speed, so the
    // quadratic collapses to a line.
    let time = if a.abs() < 1e-4 {
        if b.abs() < 1e-6 {
            return None;
        }
        -c / b
    } else {
        let discriminant = b * b - 4.0 * a * c;
        if discriminant < 0.0 {
            return None; // the shot can never reach it
        }
        let root = discriminant.sqrt();
        let (t0, t1) = ((-b - root) / (2.0 * a), (-b + root) / (2.0 * a));
        // The earliest root that is actually in the future.
        let earliest = t0.min(t1);
        let latest = t0.max(t1);
        if earliest > 0.0 {
            earliest
        } else {
            latest
        }
    };

    if time <= 0.0 || !time.is_finite() {
        return None;
    }
    Some(Lead {
        point: target + target_vel * time,
        time,
    })
}

/// Circle-vs-circle overlap test used for cannonball hits.
pub fn circles_overlap(a: Vec2, ra: f32, b: Vec2, rb: f32) -> bool {
    a.distance_squared(b) <= (ra + rb) * (ra + rb)
}

/// Sphere-vs-sphere overlap test — the 3D sibling of [`circles_overlap`], used
/// where a weapon's travel isn't confined to the sim's flat plane (torpedoes).
pub fn spheres_overlap(a: Vec3, ra: f32, b: Vec3, rb: f32) -> bool {
    a.distance_squared(b) <= (ra + rb) * (ra + rb)
}

/// Hull damage after brace mitigation — shared by every weapon's hit system so
/// "how much a brace cuts damage by" has exactly one home.
pub fn braced_damage(base_damage: f32, braced: bool, brace_factor: f32) -> f32 {
    base_damage * if braced { brace_factor } else { 1.0 }
}

/// Resolve one blow against a ship. **The only place in the sim a hull goes
/// down.**
///
/// Every weapon funnels through here, so brace mitigation, shields and
/// invulnerability each have exactly one home and compose in a fixed order:
///
/// 1. **Invulnerable** stops everything, including shield drain — a test-range
///    target should read as untouched, not as one slowly losing its shields.
/// 2. **Brace** reduces the blow first, so bracing makes a shield go further
///    rather than only mattering once the shield is gone.
/// 3. **The struck arc's shield** soaks what it can.
/// 4. Whatever is left reaches the hull.
///
/// Invulnerability short-circuits rather than being handled in
/// `destruction_system`: letting the hull go negative and refusing to despawn
/// would quietly break every reader of the hull *fraction* — the HUD's low-hull
/// vignette, the cripple threshold, and the AI's decision to flee.
///
/// Returns how the blow was split, so the client can flare a shield differently
/// from a hull breach.
pub fn apply_hull_damage(
    hull: &mut Hull,
    shield: Option<&mut Shield>,
    arc: ShieldArc,
    base_damage: f32,
    braced: bool,
    invulnerable: bool,
    brace_factor: f32,
) -> DamageReport {
    if invulnerable {
        return DamageReport::default();
    }
    let incoming = braced_damage(base_damage, braced, brace_factor);
    let through = match shield {
        Some(shield) => shield.absorb(arc, incoming),
        None => incoming,
    };
    hull.current -= through;
    DamageReport {
        to_shield: (incoming - through).max(0.0),
        to_hull: through,
    }
}

/// Bevy system: fire aimed broadsides, with a per-bank charge/telegraph.
///
/// [`FireOrders`] is a *request*, consumed here — raise a side and (if the bank
/// has reloaded) it begins a `charge_time` wind-up along the clamped aim
/// direction, then fires. `charge_time` 0 (the player) fires immediately; an
/// enemy's ~0.5s wind-up is drawn by the client so the shot is dodgeable. The AI
/// re-requests every tick, so it keeps firing on cooldown.
pub fn weapons_system(
    time: Res<Time>,
    tuning: Res<SimTuning>,
    mut commands: Commands,
    mut ships: Query<(
        &Transform,
        &Heading,
        &Velocity,
        &Faction,
        &mut Broadside,
        &mut FireOrders,
    )>,
) {
    let dt = time.delta_secs();
    for (transform, heading, velocity, faction, mut bank, mut orders) in &mut ships {
        let pos = transform.translation.truncate();
        // A config snapshot for the volley geometry, so we can freely mutate the
        // two banks' reload/charge state below without aliasing.
        let cfg = *bank;

        // Tick each side's reload independently.
        bank.port.timer = (bank.port.timer - dt).max(0.0);
        bank.starboard.timer = (bank.starboard.timer - dt).max(0.0);

        // Consume this step's request (a shot asked for mid-reload is dropped,
        // not queued). Both sides fire this step if both are ready and requested.
        let (want_port, want_starboard) = (orders.port, orders.starboard);
        let aim = orders.aim;
        orders.port = false;
        orders.starboard = false;
        orders.aim = None;

        for (port, want) in [(true, want_port), (false, want_starboard)] {
            // Resolve an in-progress charge on this side first.
            if bank.side(port).charging > 0.0 {
                let done = {
                    let s = bank.side_mut(port);
                    s.charging = (s.charging - dt).max(0.0);
                    s.charging <= 0.0
                };
                if done {
                    let dir = bank.side(port).charge_dir;
                    fire_broadside(&mut commands, pos, velocity.0, dir, &cfg, *faction, &tuning);
                    bank.side_mut(port).timer = cfg.cooldown;
                }
                continue; // a charging side ignores fresh requests
            }

            if !want || bank.side(port).timer > 0.0 {
                continue;
            }
            let dir = broadside_direction(heading.0, port, aim, cfg.arc);
            if cfg.charge_time > 0.0 {
                let s = bank.side_mut(port);
                s.charging = cfg.charge_time;
                s.charge_dir = dir;
            } else {
                fire_broadside(&mut commands, pos, velocity.0, dir, &cfg, *faction, &tuning);
                bank.side_mut(port).timer = cfg.cooldown;
            }
        }
    }
}

/// Spawn one broadside volley along `dir`.
fn fire_broadside(
    commands: &mut Commands,
    pos: Vec2,
    ship_vel: Vec2,
    dir: Vec2,
    bank: &Broadside,
    faction: Faction,
    tuning: &SimTuning,
) {
    for shot in broadside_volley(
        pos,
        ship_vel,
        dir,
        bank,
        tuning.hull_length,
        tuning.muzzle_standoff,
    ) {
        commands.spawn((
            Projectile {
                damage: bank.damage,
                faction,
                radius: tuning.projectile_radius,
            },
            Velocity(shot.velocity),
            Transform::from_translation(shot.position.extend(0.0)),
            Ttl(tuning.projectile_ttl),
        ));
    }
}

/// Bevy system: fly projectiles forward and expire them.
pub fn projectile_system(
    time: Res<Time>,
    mut commands: Commands,
    mut projectiles: Query<(Entity, &mut Transform, &Velocity, &mut Ttl), With<Projectile>>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, velocity, mut ttl) in &mut projectiles {
        transform.translation += (velocity.0 * dt).extend(0.0);
        ttl.0 -= dt;
        if ttl.0 <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// Bevy system: resolve cannonball hits against ships and apply damage,
/// announcing each hit for the presentation layer.
pub fn collision_system(
    mut commands: Commands,
    tuning: Res<SimTuning>,
    mut hits: MessageWriter<ShipHit>,
    projectiles: Query<(Entity, &Transform, &Velocity, &Projectile)>,
    mut ships: Query<(
        Entity,
        &Transform,
        &Heading,
        &Collider,
        &Faction,
        &mut Hull,
        Option<&mut Shield>,
        Option<&Brace>,
        Has<Invulnerable>,
    )>,
) {
    for (proj_entity, proj_tf, proj_vel, projectile) in &projectiles {
        let proj_pos = proj_tf.translation.truncate();
        for (
            ship_entity,
            ship_tf,
            ship_heading,
            collider,
            faction,
            mut hull,
            shield,
            brace,
            invulnerable,
        ) in &mut ships
        {
            if !projectile.faction.hostile_to(*faction) {
                continue;
            }
            let ship_pos = ship_tf.translation.truncate();
            if circles_overlap(proj_pos, projectile.radius, ship_pos, collider.radius) {
                let arc = shield_arc(ship_heading.0, ship_pos, proj_pos);
                let report = apply_hull_damage(
                    &mut hull,
                    shield.map(Mut::into_inner),
                    arc,
                    projectile.damage,
                    brace.is_some_and(|b| b.active),
                    invulnerable,
                    tuning.brace_damage_factor,
                );
                // Announced (and the ball spent) even on an invulnerable target:
                // the sparks and shake are how you see a shot land at all.
                hits.write(ShipHit {
                    position: proj_pos,
                    ship: ship_entity,
                    faction: *faction,
                    damage: projectile.damage,
                    // The ball's own heading — the truest impact direction there
                    // is, and it makes a raking shot spray differently from a
                    // square one.
                    direction: proj_vel.0.normalize_or_zero(),
                    report,
                });
                commands.entity(proj_entity).despawn();
                break; // one ball, one hit
            }
        }
    }
}

/// Bevy system: remove ships whose hull has been reduced to zero, announcing
/// each destruction.
pub fn destruction_system(
    mut commands: Commands,
    mut destroyed: MessageWriter<ShipDestroyed>,
    ships: Query<(Entity, &Transform, &Faction, &Hull, &Heading, &Velocity)>,
) {
    for (entity, transform, faction, hull, heading, velocity) in &ships {
        if hull.current <= 0.0 {
            destroyed.write(ShipDestroyed {
                position: transform.translation.truncate(),
                ship: entity,
                faction: *faction,
                heading: heading.0,
                velocity: velocity.0,
            });
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volley_has_one_spawn_per_gun() {
        let bank = Broadside {
            guns: 3,
            ..Default::default()
        };
        let volley = broadside_volley(
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::Y,
            &bank,
            HULL_LENGTH,
            MUZZLE_STANDOFF,
        );
        assert_eq!(volley.len(), 3);
    }

    #[test]
    fn beam_direction_defaults_to_the_side() {
        // Facing +X (heading 0): port beam is +Y, starboard is -Y.
        assert!(broadside_direction(0.0, true, None, 0.6).y > 0.9);
        assert!(broadside_direction(0.0, false, None, 0.6).y < -0.9);
    }

    #[test]
    fn aim_is_clamped_to_the_arc() {
        // Facing +X, port beam +Y (90°); asking to fire forward (+X, 0°) is 90°
        // off the beam but the arc is only ~34°, so it clamps toward +Y.
        let dir = broadside_direction(0.0, true, Some(Vec2::X), 0.6);
        let angle = dir.to_angle();
        assert!(
            angle >= std::f32::consts::FRAC_PI_2 - 0.6 - 1e-3,
            "clamped angle {angle} left the arc"
        );
    }

    /// The arc must sit *centred on the beam* — the same reach toward the bow as
    /// toward the stern. A forward bias here is what makes aiming feel like it
    /// only swings one way.
    #[test]
    fn the_arc_is_centred_on_the_beam() {
        use std::f32::consts::FRAC_PI_2;
        let arc = Broadside::default().arc;
        for (port, beam) in [(true, FRAC_PI_2), (false, -FRAC_PI_2)] {
            // Ask for a direction far past each edge; both must clamp to exactly
            // `arc` off the beam, in opposite directions.
            let fore = broadside_direction(0.0, port, Some(Vec2::X), arc);
            let aft = broadside_direction(0.0, port, Some(Vec2::NEG_X), arc);
            let fore_off = wrap_angle(fore.to_angle() - beam);
            let aft_off = wrap_angle(aft.to_angle() - beam);
            assert!(
                (fore_off.abs() - arc).abs() < 1e-4 && (aft_off.abs() - arc).abs() < 1e-4,
                "port={port}: edges should sit exactly `arc` off the beam, got {fore_off} / {aft_off}"
            );
            assert!(
                (fore_off + aft_off).abs() < 1e-4,
                "port={port}: arc is lopsided — fore {fore_off} vs aft {aft_off}"
            );
        }
    }

    /// The default bank covers a 135° arc (67.5° either side of the beam).
    #[test]
    fn the_default_arc_is_135_degrees_wide() {
        let total = Broadside::default().arc.to_degrees() * 2.0;
        assert!((total - 135.0).abs() < 1e-3, "arc was {total}°");
    }

    #[test]
    fn shots_inherit_ship_momentum() {
        let bank = Broadside {
            guns: 1,
            ..Default::default()
        };
        // Fire straight +Y; the ship's +X momentum carries into the shot.
        let ship_vel = Vec2::new(50.0, 0.0);
        let shot = broadside_volley(
            Vec2::ZERO,
            ship_vel,
            Vec2::Y,
            &bank,
            HULL_LENGTH,
            MUZZLE_STANDOFF,
        )[0];
        assert!(
            (shot.velocity.x - 50.0).abs() < 1e-4,
            "vx was {}",
            shot.velocity.x
        );
    }

    #[test]
    fn overlap_detects_a_hit() {
        assert!(circles_overlap(Vec2::ZERO, 5.0, Vec2::new(8.0, 0.0), 5.0));
        assert!(!circles_overlap(Vec2::ZERO, 5.0, Vec2::new(20.0, 0.0), 5.0));
    }

    #[test]
    fn spheres_overlap_detects_a_hit() {
        assert!(spheres_overlap(
            Vec3::ZERO,
            5.0,
            Vec3::new(8.0, 0.0, 0.0),
            5.0
        ));
    }

    #[test]
    fn spheres_overlap_ignores_distant_spheres() {
        assert!(!spheres_overlap(
            Vec3::ZERO,
            5.0,
            Vec3::new(20.0, 0.0, 0.0),
            5.0
        ));
    }

    #[test]
    fn braced_damage_applies_the_brace_factor() {
        let dmg = braced_damage(100.0, true, BRACE_DAMAGE_FACTOR);
        assert!((dmg - 100.0 * BRACE_DAMAGE_FACTOR).abs() < 1e-4);
    }

    #[test]
    fn braced_damage_is_full_when_not_braced() {
        assert!((braced_damage(100.0, false, BRACE_DAMAGE_FACTOR) - 100.0).abs() < 1e-4);
    }

    #[test]
    fn friendly_fire_is_ignored() {
        assert!(!Faction::Corsairs.hostile_to(Faction::Corsairs));
        assert!(Faction::Corsairs.hostile_to(Faction::Houses));
        assert!(Faction::Houses.hostile_to(Faction::Freebooters));
    }

    #[test]
    fn a_stationary_target_needs_no_lead() {
        let lead = intercept_lead(
            Vec2::ZERO,
            100.0,
            Vec2::ZERO,
            Vec2::new(100.0, 0.0),
            Vec2::ZERO,
        )
        .expect("a sitting duck is always reachable");
        assert!((lead.time - 1.0).abs() < 1e-3, "time was {}", lead.time);
        assert!(
            lead.point.distance(Vec2::new(100.0, 0.0)) < 1e-3,
            "the mark should sit on the target, was {:?}",
            lead.point
        );
    }

    /// The whole point: the mark leads a crossing target, and the shot fired at
    /// it actually arrives there at the same moment the target does.
    #[test]
    fn the_lead_mark_is_where_the_shot_and_the_target_meet() {
        let muzzle = Vec2::ZERO;
        let speed = 300.0;
        let target = Vec2::new(400.0, 0.0);
        let target_vel = Vec2::new(0.0, 120.0); // crossing left to right

        let lead = intercept_lead(muzzle, speed, Vec2::ZERO, target, target_vel).unwrap();
        assert!(
            lead.point.y > 0.0,
            "the mark should lead the target's travel"
        );

        // Fly a shot at the mark for `time` seconds and see where it lands.
        let shot_dir = (lead.point - muzzle).normalize();
        let shot_at_impact = muzzle + shot_dir * speed * lead.time;
        let target_at_impact = target + target_vel * lead.time;
        assert!(
            shot_at_impact.distance(target_at_impact) < 1e-2,
            "shot landed at {shot_at_impact:?}, target was at {target_at_impact:?}"
        );
    }

    /// Shots inherit the ship's momentum, so a shooter's own velocity must cancel
    /// out — only motion *relative* to the shooter needs leading. Two ships
    /// travelling together are stationary with respect to each other.
    #[test]
    fn matching_velocities_need_no_lead() {
        let drift = Vec2::new(90.0, -40.0);
        let target = Vec2::new(300.0, 0.0);
        let lead = intercept_lead(Vec2::ZERO, 300.0, drift, target, drift).unwrap();
        // The mark still travels with the target, but the *relative* solution is
        // the same as a standing shot: distance over muzzle speed.
        assert!((lead.time - 1.0).abs() < 1e-3, "time was {}", lead.time);
        assert!(lead.point.distance(target + drift * lead.time) < 1e-3);
    }

    #[test]
    fn a_target_outrunning_the_shot_has_no_solution() {
        // Fleeing straight down-range faster than the shot can fly.
        assert!(intercept_lead(
            Vec2::ZERO,
            100.0,
            Vec2::ZERO,
            Vec2::new(200.0, 0.0),
            Vec2::new(150.0, 0.0),
        )
        .is_none());

        // And the exact-muzzle-speed case, where the quadratic degenerates.
        assert!(intercept_lead(
            Vec2::ZERO,
            100.0,
            Vec2::ZERO,
            Vec2::new(200.0, 0.0),
            Vec2::new(100.0, 0.0),
        )
        .is_none());
    }

    /// A target closing head-on is reached sooner than a standing one.
    #[test]
    fn a_closing_target_shortens_the_flight() {
        let standing = intercept_lead(
            Vec2::ZERO,
            100.0,
            Vec2::ZERO,
            Vec2::new(100.0, 0.0),
            Vec2::ZERO,
        )
        .unwrap();
        let closing = intercept_lead(
            Vec2::ZERO,
            100.0,
            Vec2::ZERO,
            Vec2::new(100.0, 0.0),
            Vec2::new(-50.0, 0.0),
        )
        .unwrap();
        assert!(
            closing.time < standing.time,
            "closing {} should beat standing {}",
            closing.time,
            standing.time
        );
    }

    /// A hit must name the hull it struck and the blow's weight — that identity
    /// is what lets the client shake the camera for the *player's* damage only,
    /// and the damage is what scales the shake with the hit.
    #[test]
    fn a_hit_names_the_ship_and_the_damage() {
        let mut world = World::new();
        world.init_resource::<Messages<ShipHit>>();
        world.init_resource::<SimTuning>();

        let ship = world
            .spawn((
                Transform::default(),
                // Bow along +X. Damage resolution needs the heading to work out
                // which shield arc a blow landed on; this hull carries no
                // shields, so it is only the arc bookkeeping that uses it.
                Heading(0.0),
                Collider { radius: 10.0 },
                Faction::Houses,
                Hull::new(100.0),
            ))
            .id();
        world.spawn((
            Transform::default(),
            // Travelling along +X: every real shot has a velocity (the volley
            // gives it one), and the hit carries its heading so the client can
            // spray sparks back the way it came.
            Velocity(Vec2::new(300.0, 0.0)),
            Projectile {
                damage: 17.0,
                faction: Faction::Corsairs,
                radius: 5.0,
            },
        ));

        let mut schedule = Schedule::default();
        schedule.add_systems(collision_system);
        schedule.run(&mut world);

        let messages = world.resource::<Messages<ShipHit>>();
        let hit = messages
            .iter_current_update_messages()
            .next()
            .copied()
            .expect("the overlapping shot should have announced a hit");
        assert_eq!(hit.ship, ship, "the hit should name the ship it struck");
        assert!(
            (hit.damage - 17.0).abs() < 1e-4,
            "the hit should carry the shot's damage, was {}",
            hit.damage
        );
        assert!(
            (hit.direction - Vec2::X).length() < 1e-4,
            "the hit should carry the shot's heading as a unit vector, was {}",
            hit.direction
        );
    }
}

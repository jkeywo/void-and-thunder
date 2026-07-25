//! Smoothing the fixed-rate simulation onto the display's refresh rate.
//!
//! The sim steps at a fixed 64 Hz (Bevy's default `Time<Fixed>`) while the
//! window redraws at whatever the monitor runs at. Left alone the two beat
//! against each other — some frames advance the world one step, some two, some
//! none — and everything visibly stutters. It is worst at high refresh rates,
//! where most frames show no movement at all and then one jumps, and it is made
//! plainer still by the camera: [`camera_orbit`](crate::camera::camera_orbit)
//! eases on *real* time, so a perfectly smooth camera glides against a world
//! that is hopping.
//!
//! The fix is to render each entity *between* its last two simulated poses,
//! rather than at the most recent one. [`SimPose`] keeps those two poses;
//! [`interpolate_sim_pose`] writes the blend into `Transform` for the renderer,
//! and [`restore_sim_pose`] puts the authoritative pose back before every fixed
//! step so the simulation never integrates from a smoothed value.
//!
//! Presentation systems that read `Transform` — the camera, gizmos, trails —
//! must run after [`SmoothingSet`], so they see the smoothed pose the player is
//! actually looking at.

use bevy::prelude::*;
use bevy::time::Fixed;
use vt_sim::prelude::*;

/// Everything that renders from a sim pose runs after this.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SmoothingSet;

/// A jump larger than this is a teleport, not travel, and is snapped to rather
/// than interpolated across.
///
/// The fastest thing in the game covers ~5 units in one 64 Hz step (a cannonball
/// at 325 u/s), while the shortest microwarp is hundreds. Anything in between is
/// comfortably on the right side of both.
const TELEPORT_STEP: f32 = 60.0;

/// An entity's authoritative pose at the end of the previous and the latest
/// fixed step. `Transform` holds the blend of the two that is actually drawn.
#[derive(Component, Debug, Clone, Copy)]
pub struct SimPose {
    prev_pos: Vec3,
    pos: Vec3,
    /// Facing at those same two steps. Zero for things that carry no [`Heading`]
    /// (shot, bolts, torpedoes — they are oriented from their own velocity).
    prev_heading: f32,
    heading: f32,
}

impl SimPose {
    fn new(pos: Vec3, heading: f32) -> Self {
        Self {
            prev_pos: pos,
            pos,
            prev_heading: heading,
            heading,
        }
    }

    /// Facing `alpha` of the way through the current step, taking the short way
    /// round so a heading wrapping past ±π doesn't spin the hull the long way.
    pub fn heading_at(&self, alpha: f32) -> f32 {
        self.prev_heading + wrap_angle(self.heading - self.prev_heading) * alpha
    }

    /// Position `alpha` of the way through the current step.
    fn pos_at(&self, alpha: f32) -> Vec3 {
        self.prev_pos.lerp(self.pos, alpha)
    }
}

/// How far through the current fixed step this frame is drawing.
pub fn overstep(fixed: &Time<Fixed>) -> f32 {
    fixed.overstep_fraction().clamp(0.0, 1.0)
}

/// Give every sim-moved entity a pose to interpolate, starting from where it
/// already is so a newly spawned ship doesn't slide in from somewhere else.
pub fn attach_sim_pose(
    mut commands: Commands,
    moving: Query<
        (Entity, &Transform, Option<&Heading>),
        (
            Without<SimPose>,
            Or<(With<Ship>, With<Projectile>, With<EmpBolt>, With<Torpedo>)>,
        ),
    >,
) {
    for (entity, transform, heading) in &moving {
        commands.entity(entity).insert(SimPose::new(
            transform.translation,
            heading.map_or(0.0, |h| h.0),
        ));
    }
}

/// Put the authoritative pose back before the simulation steps.
///
/// This is what makes rendering a *blend* safe: the sim integrates from
/// `Transform`, so without this it would step forward from the smoothed value
/// and the world would drift a fraction of a step behind itself every frame.
pub fn restore_sim_pose(mut posed: Query<(&SimPose, &mut Transform)>) {
    for (pose, mut transform) in &mut posed {
        transform.translation = pose.pos;
    }
}

/// Record the pose the simulation just produced, retiring the one before it.
pub fn record_sim_pose(mut posed: Query<(&mut SimPose, &Transform, Option<&Heading>)>) {
    for (mut pose, transform, heading) in &mut posed {
        pose.prev_pos = pose.pos;
        pose.prev_heading = pose.heading;
        pose.pos = transform.translation;
        pose.heading = heading.map_or(pose.heading, |h| h.0);

        // A microwarp is a jump, not travel — interpolating across it would drag
        // the ship over every metre it skipped.
        if pose.prev_pos.distance(pose.pos) > TELEPORT_STEP {
            pose.prev_pos = pose.pos;
            pose.prev_heading = pose.heading;
        }
    }
}

/// Draw every sim-moved entity between its last two poses.
///
/// Rotation is deliberately left alone: hulls get theirs from
/// [`bank_ships`](crate::visuals::bank_ships), which heels them into their turns
/// off the same interpolated heading, and shot and torpedoes are oriented from
/// their own velocity.
pub fn interpolate_sim_pose(fixed: Res<Time<Fixed>>, mut posed: Query<(&SimPose, &mut Transform)>) {
    let alpha = overstep(&fixed);
    for (pose, mut transform) in &mut posed {
        transform.translation = pose.pos_at(alpha);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(prev: Vec3, curr: Vec3) -> SimPose {
        SimPose {
            prev_pos: prev,
            pos: curr,
            prev_heading: 0.0,
            heading: 0.0,
        }
    }

    #[test]
    fn a_pose_blends_between_the_last_two_steps() {
        let p = pose(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(p.pos_at(0.0), Vec3::ZERO);
        assert_eq!(p.pos_at(0.5), Vec3::new(5.0, 0.0, 0.0));
        assert_eq!(p.pos_at(1.0), Vec3::new(10.0, 0.0, 0.0));
    }

    /// A heading wrapping past ±π must take the short way round, or a ship
    /// crossing due-west visibly spins all the way back through north.
    #[test]
    fn heading_interpolation_takes_the_short_way_round() {
        use std::f32::consts::PI;
        let p = SimPose {
            prev_pos: Vec3::ZERO,
            pos: Vec3::ZERO,
            prev_heading: PI - 0.1,
            heading: -PI + 0.1,
        };
        // Half way across the wrap should sit *past* π, not back near zero.
        let mid = wrap_angle(p.heading_at(0.5));
        assert!(
            mid.abs() > PI - 0.02,
            "expected the short way across the wrap, got {mid}"
        );
    }

    #[test]
    fn a_fresh_pose_does_not_slide() {
        let p = SimPose::new(Vec3::new(3.0, 4.0, 0.0), 1.0);
        assert_eq!(p.pos_at(0.0), p.pos_at(1.0), "a new entity must not drift");
    }
}

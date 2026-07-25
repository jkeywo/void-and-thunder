//! Off-screen enemy call-outs: a pool of UI markers pinned to the window edge
//! in each off-screen enemy's on-screen direction.

use bevy::prelude::*;
use vt_sim::prelude::*;

use crate::camera::MainCamera;
use crate::Player;

/// Size of the pool of off-screen enemy markers (max shown at once).
pub const EDGE_MARKER_COUNT: usize = 24;
/// Pixel size of an edge marker.
pub const EDGE_MARKER_SIZE: f32 = 16.0;
/// Inset of the edge markers from the window border, in pixels.
const EDGE_MARGIN: f32 = 42.0;

/// One of the pooled UI markers that point at off-screen enemies.
#[derive(Component)]
pub struct EdgeMarker;

/// Base display colour for a faction's ships (before damage tinting).
fn faction_color(faction: &Faction) -> Color {
    match faction {
        Faction::Corsairs => Color::srgb(0.35, 0.85, 0.55),
        Faction::Houses => Color::srgb(0.85, 0.30, 0.30),
        Faction::Janissariat => Color::srgb(0.85, 0.65, 0.20),
        Faction::Guild => Color::srgb(0.45, 0.60, 0.90),
        Faction::Freebooters => Color::srgb(0.75, 0.45, 0.85),
    }
}

/// Point a pooled marker at each off-screen enemy, pinned to the window edge in
/// the enemy's on-screen direction. Uses the camera's own basis, so it stays
/// correct as the camera yaws.
pub fn update_offscreen_markers(
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    player_q: Query<&Transform, With<Player>>,
    enemies: Query<(&Transform, &Faction, Option<&Disabled>), (With<Ship>, Without<Player>)>,
    mut markers: Query<(&mut Node, &mut BackgroundColor), With<EdgeMarker>>,
) {
    let hide_all = |markers: &mut Query<(&mut Node, &mut BackgroundColor), With<EdgeMarker>>| {
        for (mut node, _) in markers.iter_mut() {
            node.display = Display::None;
        }
    };

    let (Ok(window), Ok((camera, cam_gt)), Ok(player)) =
        (windows.single(), camera_q.single(), player_q.single())
    else {
        hide_all(&mut markers);
        return;
    };

    let (w, h) = (window.width(), window.height());
    let center = Vec2::new(w * 0.5, h * 0.5);
    let ship = player.translation.truncate();
    // World directions that map to screen right / screen up.
    let screen_right = cam_gt.right().truncate().normalize_or_zero();
    let screen_up = cam_gt.up().truncate().normalize_or_zero();

    let mut pool = markers.iter_mut();
    for (enemy, faction, disabled) in &enemies {
        let on_screen = match camera.world_to_viewport(cam_gt, enemy.translation) {
            Ok(vp) => vp.x >= 0.0 && vp.x <= w && vp.y >= 0.0 && vp.y <= h,
            Err(_) => false, // behind the camera
        };
        if on_screen {
            continue;
        }

        let Some((mut node, mut color)) = pool.next() else {
            break; // pool exhausted; rare
        };

        let d = enemy.translation.truncate() - ship;
        // Screen-space direction (y is down in UI space).
        let dir = Vec2::new(d.dot(screen_right), -d.dot(screen_up)).normalize_or_zero();
        if dir == Vec2::ZERO {
            node.display = Display::None;
            continue;
        }
        // Intersect the ray from centre with the inset window rectangle.
        let (hw, hh) = (w * 0.5 - EDGE_MARGIN, h * 0.5 - EDGE_MARGIN);
        let tx = if dir.x.abs() > 1e-3 {
            hw / dir.x.abs()
        } else {
            f32::INFINITY
        };
        let ty = if dir.y.abs() > 1e-3 {
            hh / dir.y.abs()
        } else {
            f32::INFINITY
        };
        let pos = center + dir * tx.min(ty);

        node.display = Display::Flex;
        node.left = Val::Px(pos.x - EDGE_MARKER_SIZE * 0.5);
        node.top = Val::Px(pos.y - EDGE_MARKER_SIZE * 0.5);
        *color = BackgroundColor(if disabled.is_some() {
            Color::srgb(0.55, 0.58, 0.65) // crippled: boardable
        } else {
            faction_color(faction)
        });
    }

    // Hide any markers left unused this frame.
    for (mut node, _) in pool {
        node.display = Display::None;
    }
}

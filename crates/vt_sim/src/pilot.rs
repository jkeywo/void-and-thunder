//! The pilot brain — the utility AI that flies a ship carrying the full player
//! kit (broadsides, EMP, torpedoes, microwarp, boost, and a hull to ram with).
//!
//! The enemy AI in [`ai`](crate::ai) does one thing: present a beam and shoot.
//! A ship with the whole kit cannot be written that way, because its tools want
//! contradictory things — torpedoes want standoff, the broadside wants the beam
//! at three hundred units, the EMP wants the bow on, boarding wants a dead stop
//! alongside a hulk. A priority list resolves that badly: whichever branch sits
//! at the top wins every frame it is legal, so the ship ends up flying one
//! weapon and carrying the rest.
//!
//! So each action scores itself instead.
//!
//! 1. **Every action costs itself out** ([`score`]) against the same
//!    [`Situation`] — range, readiness, ammunition, hull, how crowded it is.
//! 2. **The best score takes the helm.** Exactly one action steers the ship
//!    ([`govern`]), and it is the only one allowed to move her.
//! 3. **Every other action still offers what it could do** ([`opportunities`])
//!    — but only from where the ship already is. A bank whose beam happens to
//!    bear has a firing solution; the emitter has a shot if the target has
//!    wandered into its arc; tubes can lock whatever the cursor is already
//!    resting on. Nothing in this pass may touch the helm.
//! 4. **Two hands decide what actually gets pressed** ([`apply_hands`]). The
//!    ship is flown from a gamepad, and a gamepad cannot do everything at once:
//!    one aim between the four shoulder inputs, one thumb across the four face
//!    buttons, and a real gap to cross the pad between two of them. Whatever the
//!    first three passes wanted, this is what a pair of hands could deliver.
//!
//! That split is the whole design. It is what makes the EMP opportunistic
//! rather than obsessive, and it means adding a weapon later is a scoring
//! function plus a trigger, not a new branch in a priority chain. The hands are
//! the reason it stays honest: the brain may want six things, and it will get
//! at most two, chosen the way a player would choose them.
//!
//! The decision is the pure function [`plan`], so the whole brain is testable
//! without a `World`; [`pilot_system`] only gathers the situation and applies
//! the result.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_time::Time;
use bevy_transform::components::Transform;
use std::f32::consts::FRAC_PI_2;

use crate::ai::desired_helm;
use crate::combat::{intercept_lead, Broadside};
use crate::components::{
    AiController, Brace, Disabled, EmpDefense, Faction, FireOrders, Heading, Helm, Hull,
    PilotIntent, Ship, Velocity,
};
use crate::drive::{BoostDrive, MicrowarpDrive};
use crate::emp::EmpWeapon;
use crate::piracy::BoardIntent;
use crate::shield::{Shield, ShieldArc};
use crate::torpedo::{TorpedoBay, TorpedoLock};
use crate::tuning::{AiTuning, PilotTuning, SimTuning};
use crate::util::wrap_angle as wrap;

/// One thing the pilot can decide to do with the ship.
///
/// These are *stances*, not button presses: the action that wins owns the helm
/// until something outscores it. Firing a gun is not an action — that happens
/// opportunistically whichever stance is being flown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Action {
    /// Nothing to fight and nothing to take: hold station.
    #[default]
    Hold,
    /// Work the beam — the main battery, and the default answer to a fight.
    Broadside,
    /// Stand off and build a torpedo volley.
    Torpedo,
    /// Hold the bow on to keep a target under the emitter.
    Emp,
    /// Put the bow through them under boost.
    Ram,
    /// Come alongside a crippled ship and take it.
    Board,
    /// Jump — clear of a scrum, or across the field to the fight.
    Microwarp,
    /// Break off and run.
    Disengage,
}

impl Action {
    /// Every action, in the order ties are broken.
    pub const ALL: [Action; 7] = [
        Action::Broadside,
        Action::Torpedo,
        Action::Board,
        Action::Ram,
        Action::Microwarp,
        Action::Emp,
        Action::Disengage,
    ];

    /// Short name, for HUD and debugging.
    pub fn name(self) -> &'static str {
        match self {
            Action::Hold => "hold",
            Action::Broadside => "broadside",
            Action::Torpedo => "torpedo",
            Action::Emp => "emp",
            Action::Ram => "ram",
            Action::Board => "board",
            Action::Microwarp => "microwarp",
            Action::Disengage => "disengage",
        }
    }
}

/// The four inputs that take the aim: two triggers and two bumpers. A player
/// has fingers for all of them, but only one aim between them — the cursor, the
/// steered volley, the jump preview are the same act of pointing — so the pilot
/// gets exactly one per step, same as a pair of hands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shoulder {
    /// LT — aim and throw the port volley.
    Port,
    /// RT — the starboard volley.
    Starboard,
    /// LB — hold to build a torpedo lock.
    Torpedo,
    /// RB — hold to prime the jump.
    Microwarp,
}

/// The four face buttons. One thumb, so one at a time, and crossing from one to
/// another costs [`PilotTuning::thumb_travel`] — the pilot is not pressing
/// anything while its thumb is in the air.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thumb {
    /// X — the EMP trigger.
    Emp,
    /// Y — brace.
    Brace,
    /// A — boost.
    Boost,
    /// B — board.
    Board,
}

/// Live brain state that has to survive between steps: what the ship is doing
/// now (so it can be given the benefit of the doubt next step), how far along a
/// microwarp has been primed, and where the thumb is.
///
/// A component rather than a field on [`AiController`] because none of it is
/// authored — a saved ship class describes a *fit*, never a decision in flight.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PilotBrain {
    /// The action currently flying the ship.
    pub action: Action,
    /// Seconds a microwarp has been held down to prime it.
    pub warp_prime: f32,
    /// The face button the thumb is on, or heading for.
    pub thumb: Option<Thumb>,
    /// Seconds left before the thumb arrives and can press. Non-zero only while
    /// it is crossing the pad.
    pub thumb_travel: f32,
}

/// A ship the pilot can see.
#[derive(Clone, Copy, Debug)]
pub struct Contact {
    pub pos: Vec2,
    pub vel: Vec2,
    /// Hull as a fraction of its maximum.
    pub hull_frac: f32,
    /// EMP load as a fraction of what the drive can soak. At `1.0` there is
    /// nothing left to gain by shooting it again.
    pub emp_frac: f32,
    /// Distance from the pilot's ship, cached — every score wants it.
    pub dist: f32,
}

/// What the ship is carrying and whether it can be used *this step*. Gathered
/// from the kit components so the scores never have to know which component a
/// number came from.
#[derive(Clone, Copy, Debug)]
pub struct Kit {
    /// How far out the guns actually throw (muzzle speed × shot lifetime).
    pub gun_reach: f32,
    /// Half-angle either side of the beam a volley can be steered into.
    pub gun_arc: f32,
    pub muzzle_speed: f32,
    pub port_ready: bool,
    pub starboard_ready: bool,
    pub emp_range: f32,
    /// Full width of the emitter's arc.
    pub emp_arc: f32,
    pub torpedo_range: f32,
    pub lock_radius: f32,
    /// Tubes loaded and ready to lock.
    pub tubes: f32,
    /// Everything aboard — tubes plus magazine — as a fraction of a full ship.
    /// This is what makes torpedoes feel finite: the scarcer they are, the less
    /// eager the brain is to spend them.
    pub stock_frac: f32,
    /// Locks accrued on the volley being built right now.
    pub locks: u32,
    pub warp_range: f32,
    pub warp_ready: bool,
    pub board_range: f32,
    /// Boost battery remaining, as a fraction.
    pub boost_frac: f32,
    /// Charge in the bow arc as a fraction of its own capacity. Zero on a hull
    /// with no shields fitted, which is what makes every shield term below
    /// vanish for such a ship rather than needing a special case.
    pub fore_frac: f32,
    /// Charge in the stern arc, likewise.
    pub aft_frac: f32,
    /// Whether the bow arc is suppressed — it took a hit recently and will not
    /// regenerate until it stops being shot at. The sim's own record of
    /// *incoming fire*, per side, which is exactly the thing worth turning away
    /// from.
    pub fore_suppressed: bool,
    /// Whether the stern arc is suppressed, likewise.
    pub aft_suppressed: bool,
}

/// Everything the brain knows this step.
pub struct Situation {
    pub pos: Vec2,
    pub heading: f32,
    pub vel: Vec2,
    /// Hull alone, as a fraction of its maximum. The right number for anything
    /// only the hull answers for: boarding repairs plate, and a ram is paid for
    /// in it too.
    pub hull_frac: f32,
    /// How much punishment is left in the ship — hull *and* shields, as a
    /// fraction of a fresh one. Five percent hull behind full banks is not the
    /// emergency that five percent hull alone is, and this is where that is
    /// written down. Shields count for less than their face value
    /// ([`PilotTuning::shield_worth`]) because they are directional: a blow on
    /// the flat side skips them entirely. On an unshielded hull this is exactly
    /// `hull_frac`.
    pub integrity: f32,
    /// Active enemies, nearest first.
    pub hostiles: Vec<Contact>,
    /// Crippled hulls worth taking, nearest first.
    pub prizes: Vec<Contact>,
    pub kit: Kit,
    /// The controller's own ranges and thresholds (engage range, flee point).
    pub ai: AiController,
    /// How crowded it is: every hostile inside the surround radius counts for
    /// its closeness, so two ships in your lap read as `~2` and one on the
    /// horizon reads as `~0`. This is the *boxed-in* measure — what makes a jump
    /// worth spending and a boarding a bad idea.
    pub threat: f32,
    /// How much shooting is actually pointed at the ship, measured over gun
    /// reach rather than the surround radius. A separate number from `threat`
    /// because it answers a different question: not "am I crowded" but "is
    /// anyone in a position to hurt me", which is the only thing that makes
    /// running away worth doing — and, crucially, the thing that stops being
    /// true once the ship has actually got clear.
    pub danger: f32,
}

impl Situation {
    /// Assemble a situation, sorting contacts and computing the derived numbers
    /// the scores share.
    pub fn new(
        pos: Vec2,
        heading: f32,
        vel: Vec2,
        hull_frac: f32,
        integrity: f32,
        mut hostiles: Vec<Contact>,
        mut prizes: Vec<Contact>,
        kit: Kit,
        ai: AiController,
        surround_radius: f32,
    ) -> Self {
        hostiles.sort_by(|a, b| a.dist.total_cmp(&b.dist));
        prizes.sort_by(|a, b| a.dist.total_cmp(&b.dist));
        let crowding = |radius: f32| -> f32 {
            hostiles
                .iter()
                .map(|h| (1.0 - h.dist / radius.max(1.0)).clamp(0.0, 1.0))
                .sum()
        };
        let threat = crowding(surround_radius);
        let danger = crowding(kit.gun_reach);
        Self {
            pos,
            heading,
            vel,
            hull_frac,
            integrity,
            hostiles,
            prizes,
            kit,
            ai,
            threat,
            danger,
        }
    }

    /// The enemy being fought: the nearest active hostile.
    pub fn target(&self) -> Option<Contact> {
        self.hostiles.first().copied()
    }

    /// The hulk being hunted: the nearest crippled ship.
    pub fn prize(&self) -> Option<Contact> {
        self.prizes.first().copied()
    }

    /// Unit vector the bow points along.
    pub fn forward(&self) -> Vec2 {
        Vec2::from_angle(self.heading)
    }

    /// Whether the ship has an answer ready right now — a loaded bank with the
    /// target inside gun reach, or a torpedo in a tube. Several scores hang off
    /// this: EMP and ramming are both things you reach for with your hands
    /// otherwise empty.
    pub fn armed(&self) -> bool {
        let guns = (self.kit.port_ready || self.kit.starboard_ready)
            && self.target().is_some_and(|c| c.dist <= self.kit.gun_reach);
        guns || self.kit.tubes >= 1.0
    }
}

/// A bank's firing solution against the best target it currently bears on.
#[derive(Clone, Copy, Debug)]
pub struct Solution {
    /// How far off the lead point is — how the hands pick which of two bearing
    /// banks is worth the one aim they have between them.
    pub dist: f32,
    /// World direction to throw the volley along.
    pub dir: Vec2,
}

/// What the brain decided: one stance flying the ship, everything the ship's
/// current pose makes possible, and — after [`apply_hands`] — the subset of that
/// a player could actually be pressing.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    /// The action that won the helm.
    pub action: Action,
    pub helm: Helm,
    /// Aim cursor. Shared by the emitter (picks its target near it), the torpedo
    /// bay (locks what it rests on) and the microwarp (jumps to it), so the
    /// action holding the helm sets it and the opportunists work with what they
    /// are given.
    pub aim_point: Vec2,
    pub emp_fire: bool,
    pub torpedo_hold: bool,
    pub microwarp_hold: bool,
    pub warp_prime: f32,
    pub board: bool,
    pub boost: bool,
    pub brace: bool,
    pub fire: FireOrders,
    /// The solutions each bank has this step, before the hands choose between
    /// them. `None` means that side bears on nothing it could hit.
    pub port_solution: Option<Solution>,
    pub starboard_solution: Option<Solution>,
    /// Which aim input the pilot spent this step, and which face button it is on
    /// or heading for — the state that makes the input model legible from
    /// outside, and testable.
    pub shoulder: Option<Shoulder>,
    pub thumb: Option<Thumb>,
    pub thumb_travel: f32,
    /// Every action's score this step, in [`Action::ALL`] order — the reason the
    /// ship is doing what it is doing, for tests and debugging.
    pub scores: [f32; Action::ALL.len()],
}

impl Plan {
    fn new(action: Action, aim_point: Vec2) -> Self {
        Self {
            action,
            helm: Helm::default(),
            aim_point,
            emp_fire: false,
            torpedo_hold: false,
            microwarp_hold: false,
            warp_prime: 0.0,
            board: false,
            boost: false,
            brace: false,
            fire: FireOrders::default(),
            port_solution: None,
            starboard_solution: None,
            shoulder: None,
            thumb: None,
            thumb_travel: 0.0,
            scores: [0.0; Action::ALL.len()],
        }
    }

    /// This step's score for one action.
    pub fn score_of(&self, action: Action) -> f32 {
        Action::ALL
            .iter()
            .position(|a| *a == action)
            .map(|i| self.scores[i])
            .unwrap_or(0.0)
    }
}

// ---------------------------------------------------------------------------
// Cost functions
// ---------------------------------------------------------------------------

/// `1.0` at `full`, sliding linearly to `0.0` at `zero`, clamped outside both.
/// Works in either direction, so a score can read "worth less the further away"
/// or "worth more the further away" without changing helper.
fn ramp(x: f32, full: f32, zero: f32) -> f32 {
    if (zero - full).abs() < 1e-6 {
        return if x <= full { 1.0 } else { 0.0 };
    }
    ((zero - x) / (zero - full)).clamp(0.0, 1.0)
}

/// How much a stance is worth for the side it turns to the enemy.
///
/// The shields are two independent 180° banks, so a stance that fixes the ship's
/// facing also picks which bank does the soaking. Running shows the stern;
/// ramming and working the EMP show the bow. This weighs the bank being offered
/// against the one being hidden, and — because a suppressed bank cannot come
/// back while it stays pointed at the guns that flattened it — counts a flat,
/// still-being-hit arc as worse than merely empty. Turning the other side to
/// them buys regeneration as well as protection.
///
/// Scaled by `danger`, so presentation is free when nobody is shooting and
/// decisive when everybody is. On an unshielded hull both fractions are zero and
/// this is exactly `1.0`.
///
/// The broadside is deliberately not weighed this way: beam-on puts the enemy on
/// the boundary between the arcs, so blows land across both and the ship spends
/// its whole capacity instead of one bank's. That is already the right answer
/// when both are healthy, which is most of the time.
fn presentation(s: &Situation, showing_bow: bool, t: &PilotTuning) -> f32 {
    let (mine, mine_hot, theirs, theirs_hot) = if showing_bow {
        (
            s.kit.fore_frac,
            s.kit.fore_suppressed,
            s.kit.aft_frac,
            s.kit.aft_suppressed,
        )
    } else {
        (
            s.kit.aft_frac,
            s.kit.aft_suppressed,
            s.kit.fore_frac,
            s.kit.fore_suppressed,
        )
    };
    // Being pinned only argues for anything as a *difference* between the two
    // sides. Taken on both, it cancels — which is what a pooled shield wants,
    // since there is no other side to turn to.
    let pinned = |hot: bool, frac: f32| if hot && frac <= 0.0 { 0.5 } else { 0.0 };
    let edge =
        ((mine - theirs) - (pinned(mine_hot, mine) - pinned(theirs_hot, theirs))) * t.shield_bias;
    (1.0 + edge * s.danger.clamp(0.0, 1.0)).max(t.presentation_floor)
}

/// Score one action against the situation. Higher wins; `0.0` means "not
/// possible right now", which is how an action rules itself out (no target, no
/// ammunition, drive still cooling) rather than being gated by a caller.
pub fn score(action: Action, s: &Situation, t: &PilotTuning) -> f32 {
    match action {
        Action::Hold => 0.0,
        Action::Broadside => score_broadside(s, t),
        Action::Torpedo => score_torpedo(s, t),
        Action::Emp => score_emp(s, t),
        Action::Ram => score_ram(s, t),
        Action::Board => score_board(s, t),
        Action::Microwarp => score_microwarp(s, t),
        Action::Disengage => score_disengage(s, t),
    }
}

/// The default answer to an enemy: get on the beam and work the guns. Never
/// scores zero while a hostile lives, so there is always something to fall back
/// to — an unarmed ship still wants to be presenting a side when the banks come
/// back up, not drifting.
fn score_broadside(s: &Situation, t: &PilotTuning) -> f32 {
    let Some(c) = s.target() else {
        return 0.0;
    };
    let ready = if s.kit.port_ready || s.kit.starboard_ready {
        1.0
    } else {
        t.reloading_interest
    };
    // Measured against the station-keeping range, not the guns' outer reach: a
    // broadside fight is fought *at* three hundred units, and the further off
    // that the enemy is the more this stance is really just a long trudge.
    let band = ramp(c.dist, s.ai.engage_range, s.ai.engage_range * 4.0);
    t.w_broadside * ready * (0.3 + 0.7 * band)
}

/// Torpedoes are the standoff weapon and a spendable one. Worth more the
/// further out the target is (inside the bay's reach), worth less the emptier
/// the ship is — the last two torpedoes in the magazine should be waited with,
/// not thrown at the first hull that presents itself.
fn score_torpedo(s: &Situation, t: &PilotTuning) -> f32 {
    let Some(c) = s.target() else {
        return 0.0;
    };
    if s.kit.tubes < 1.0 || c.dist > s.kit.torpedo_range {
        return 0.0; // nothing loaded, or nothing lockable
    }
    let reach = 0.5 + 0.5 * (c.dist / s.kit.torpedo_range).clamp(0.0, 1.0);
    let stock = t.scarcity_floor + (1.0 - t.scarcity_floor) * s.kit.stock_frac;
    t.w_torpedo * reach * stock
}

/// The EMP is a weapon of opportunity, and this is where that is written down:
/// it scores low against a target the guns or tubes can already answer, and only
/// argues for the helm when the ship's hands are otherwise empty. Worthless
/// against a drive already swamped — the bar is full, more bolts add nothing.
fn score_emp(s: &Situation, t: &PilotTuning) -> f32 {
    let Some(c) = s.target() else {
        return 0.0;
    };
    if c.dist > s.kit.emp_range {
        return 0.0;
    }
    let headroom = (1.0 - c.emp_frac).clamp(0.0, 1.0);
    let idle = if s.armed() { 0.0 } else { 1.0 };
    let closeness = ramp(c.dist, s.kit.emp_range * 0.5, s.kit.emp_range);
    t.w_emp
        * headroom
        * closeness
        * (t.emp_busy_interest + (1.0 - t.emp_busy_interest) * idle)
        * presentation(s, true, t)
}

/// Ramming is what a dry ship does. It needs the battery to be worth anything
/// (the bonus is for spending the drive, not for arriving), a bow already
/// pointing the right way, and something left to absorb the contact — the damage
/// is symmetric, so a ship with nothing behind its bow is losing that trade.
/// A ram lands on the bow, so a charged fore bank is most of what pays for it.
fn score_ram(s: &Situation, t: &PilotTuning) -> f32 {
    let Some(c) = s.target() else {
        return 0.0;
    };
    let close = ramp(c.dist, s.ai.engage_range * 0.5, s.ai.engage_range * 1.5);
    if close <= 0.0 {
        return 0.0;
    }
    let aligned = s
        .forward()
        .dot((c.pos - s.pos).normalize_or_zero())
        .max(0.0);
    let nerve = ramp(s.integrity, 1.0, t.ram_hull_floor);
    let need = if s.armed() { 0.0 } else { 1.0 };
    t.w_ram
        * close
        * (0.3 + 0.7 * aligned)
        * s.kit.boost_frac
        * nerve
        * (0.3 + 0.7 * need)
        * presentation(s, true, t)
}

/// Taking a hulk is how the ship sustains itself — hull back, torpedoes back —
/// so the hungrier it is, the further it will go for one. Held back by how busy
/// the field is: a boarding is three seconds sitting still, which is not
/// something to do with a live enemy in your lap.
fn score_board(s: &Situation, t: &PilotTuning) -> f32 {
    let Some(p) = s.prize() else {
        return 0.0;
    };
    let hunger = (t.board_base
        + t.board_repair_pull * (1.0 - s.hull_frac)
        + t.board_resupply_pull * (1.0 - s.kit.stock_frac))
        .min(1.25);
    let reach = ramp(p.dist, s.kit.board_range, s.kit.torpedo_range);
    let safety = ramp(s.threat, 0.0, t.board_safe_threat);
    t.w_board * hunger * (0.3 + 0.7 * reach) * safety
}

/// The jump does two jobs, and it is scored for whichever is worth more: getting
/// out from under a crowd (worse the more hurt the ship is) or crossing dead
/// space to reach the fight. Zero while the drive is cooling down.
fn score_microwarp(s: &Situation, t: &PilotTuning) -> f32 {
    if !s.kit.warp_ready {
        return 0.0;
    }
    let escape = {
        let pressure = (s.threat / t.warp_escape_threat.max(0.01)).clamp(0.0, 1.5);
        pressure * (0.6 + 0.4 * (1.0 - s.integrity))
    };
    let reposition = {
        let objective = s
            .target()
            .or_else(|| s.prize())
            .map(|c| c.dist)
            .unwrap_or(0.0);
        // Worth a jump once the gap approaches what the drive can cross in one;
        // anything a short run would close is not worth spending it on.
        let far = ramp(objective, s.kit.warp_range * 1.5, s.ai.engage_range * 2.0);
        far * (1.0 - s.threat.clamp(0.0, 1.0)) * t.warp_reposition_interest
    };
    t.w_microwarp * escape.max(reposition)
}

/// Running is the answer of last resort, and it has to be able to outbid a
/// perfectly good broadside — a ship that fights on to the last plate never gets
/// to spend its hull on anything.
fn score_disengage(s: &Situation, t: &PilotTuning) -> f32 {
    // Integrity, not bare hull: a ship down to its last plate but carrying full
    // banks still has punishment in it, and running from a fight it can still
    // take is how a good position gets thrown away.
    if s.hostiles.is_empty() || s.integrity >= s.ai.flee_hull_frac {
        return 0.0;
    }
    // Crossing the controller's own flee threshold is what starts the argument;
    // how far below it decides how loudly.
    let hurt = 0.6 + 0.4 * ramp(s.integrity, 0.0, s.ai.flee_hull_frac);
    // Scaled by danger with no floor under it, which is what makes running a
    // manoeuvre rather than a mood: the moment the ship is actually clear this
    // collapses to nothing and something else takes the helm. Without that a
    // crippled ship simply leaves — it outruns an enemy that cannot catch it and
    // the encounter never resolves, which is a stalemate, not a survival.
    //
    // And running is a stance too: it shows the stern, so it is worth more with
    // a stern bank still up than with a flat one being shot through.
    t.w_disengage * hurt * s.danger.clamp(0.0, 1.0) * presentation(s, false, t)
}

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// Score every action, hand the helm to the winner, let every other action offer
/// what the resulting pose allows, then cut it down to what two hands can press.
///
/// `brain` is last step's state. The action it was flying gets
/// [`PilotTuning::commit_bonus`] added to its score so the pilot commits to a
/// manoeuvre instead of dithering between two actions that happen to be scoring
/// within a whisker of each other. The bonus never resurrects an action that has
/// scored itself out, so a jump with the drive cooling or a volley with an empty
/// magazine still drops immediately.
pub fn plan(s: &Situation, t: &AiTuning, brain: &PilotBrain, dt: f32) -> Plan {
    let incumbent = brain.action;
    let mut scores = [0.0f32; Action::ALL.len()];
    let mut best = Action::Hold;
    let mut best_score = 0.0;
    for (i, action) in Action::ALL.iter().enumerate() {
        let raw = score(*action, s, &t.pilot);
        scores[i] = raw;
        let effective = if raw > 0.0 && *action == incumbent {
            raw + t.pilot.commit_bonus
        } else {
            raw
        };
        if effective > best_score {
            best_score = effective;
            best = *action;
        }
    }

    let mut plan = govern(best, s, t, brain.warp_prime, dt);
    plan.scores = scores;
    opportunities(s, t, &mut plan);
    apply_hands(s, &t.pilot, &mut plan, brain, dt);
    plan
}

/// A helm that turns the bow toward `aim` at the given throttle.
fn face(s: &Situation, aim: Vec2, throttle: f32) -> Helm {
    let err = wrap((aim - s.pos).to_angle() - s.heading);
    Helm {
        throttle,
        turn: (err * 2.5).clamp(-1.0, 1.0),
    }
}

/// Fly the winning action: set the helm, the aim cursor, and whatever that
/// action alone is entitled to do (prime a jump, hold a boarding, spend the
/// boost battery). This is the *only* place the helm is written.
fn govern(action: Action, s: &Situation, t: &AiTuning, warp_prime: f32, dt: f32) -> Plan {
    // Default cursor: on the enemy if there is one, else on the prize, else
    // straight ahead. Whatever else happens, the opportunists downstream get a
    // cursor that is somewhere useful.
    let default_aim = s
        .target()
        .or_else(|| s.prize())
        .map(|c| c.pos)
        .unwrap_or(s.pos + s.forward() * s.ai.engage_range);
    let mut plan = Plan::new(action, default_aim);

    match action {
        Action::Hold => {}

        // Present a beam. The station-keeping geometry is the enemy AI's — one
        // implementation of "put them on the beam" for every ship in the game.
        // Hull is passed as full deliberately: whether to run is the Disengage
        // action's decision, made by score, not a branch buried in the helm.
        Action::Broadside => {
            if let Some(c) = s.target() {
                let (helm, _) = desired_helm(s.pos, s.heading, 1.0, c.pos, &s.ai, t);
                plan.helm = helm;
            }
        }

        // Hold the range the tubes want and keep the cursor on the target so
        // locks accrue, then let go once the volley is worth firing.
        Action::Torpedo => {
            if let Some(c) = s.target() {
                let standoff = s.kit.torpedo_range * t.pilot.torpedo_standoff;
                // Close if outside the band, ease off inside it: a torpedo boat
                // that keeps charging ends up in a knife fight it did not want.
                let throttle = ramp(c.dist, standoff * 1.6, standoff * 0.7);
                plan.helm = face(s, c.pos, throttle);
                plan.torpedo_hold = s.kit.locks < volley_size(s, t);
            }
        }

        // The emitter is frontal, so this is the one stance that fights bow-on.
        Action::Emp => {
            if let Some(c) = s.target() {
                plan.helm = face(s, c.pos, t.pilot.emp_throttle);
                plan.emp_fire = true;
            }
        }

        // Everything into the bow: full throttle, boost lit, braced for the
        // contact (the hull takes its share of a ram however well aimed).
        Action::Ram => {
            if let Some(c) = s.target() {
                let aim = intercept_lead(s.pos, s.kit.muzzle_speed.max(1.0), s.vel, c.pos, c.vel)
                    .map(|l| l.point)
                    .unwrap_or(c.pos);
                plan.helm = face(s, aim, 1.0);
                plan.boost = true;
                plan.brace = true;
            }
        }

        // Come alongside and stop. The dwell does the rest.
        Action::Board => {
            if let Some(p) = s.prize() {
                plan.aim_point = p.pos;
                let throttle = if p.dist <= s.kit.board_range {
                    0.0
                } else {
                    ramp(p.dist, s.kit.board_range * 4.0, s.kit.board_range).max(0.15)
                };
                plan.helm = face(s, p.pos, throttle);
                plan.board = p.dist <= s.kit.board_range;
                plan.brace = plan.board; // sitting still alongside a hulk, take the hits
            }
        }

        // Pick a destination, point at it, hold the drive down long enough to
        // prime it. The warp fires on the *falling* edge in `microwarp_system`,
        // so letting go is the jump.
        Action::Microwarp => {
            plan.aim_point = warp_destination(s);
            plan.helm = face(s, plan.aim_point, 1.0);
            let primed = warp_prime + dt;
            if primed < t.warp_prime {
                plan.microwarp_hold = true;
                plan.warp_prime = primed;
            }
        }

        // Bow away and everything to the engines. No brace: a braced run is
        // still a run, and the point is to be somewhere else.
        Action::Disengage => {
            if let Some(c) = s.target() {
                let away = s.pos + (s.pos - c.pos).normalize_or(s.forward()) * s.kit.warp_range;
                plan.helm = face(s, away, 1.0);
                plan.helm.throttle = 1.0; // running is worth a wide turn
                plan.boost = true;
            }
        }
    }

    plan
}

/// How many tubes to build into a volley: what the tuning asks for, capped by
/// what is actually loaded, never zero (a one-tube volley beats no volley).
fn volley_size(s: &Situation, t: &AiTuning) -> u32 {
    t.torpedo_min_volley.min(s.kit.tubes as u32).max(1)
}

/// Where a jump should put the ship: out from under a crowd when there is one,
/// otherwise onto the objective — stopping at broadside range rather than on top
/// of it, since arriving inside someone's hull is a ram, not an approach.
fn warp_destination(s: &Situation) -> Vec2 {
    let crowd: Vec<Vec2> = s
        .hostiles
        .iter()
        .filter(|h| h.dist < s.kit.warp_range * 0.5)
        .map(|h| h.pos)
        .collect();
    let objective = s.target().or_else(|| s.prize());

    let far_enough = objective.is_some_and(|c| c.dist > s.ai.engage_range * 2.0);
    if !crowd.is_empty() && (!far_enough || s.hull_frac < s.ai.flee_hull_frac) {
        let centroid = crowd.iter().copied().fold(Vec2::ZERO, |a, b| a + b) / crowd.len() as f32;
        let away = (s.pos - centroid).normalize_or(s.forward());
        return s.pos + away * s.kit.warp_range;
    }
    match objective {
        Some(c) => {
            let toward = (c.pos - s.pos).normalize_or(s.forward());
            let stop = s.ai.engage_range.min(c.dist);
            crate::drive::clamp_to_range(s.pos, c.pos - toward * stop, s.kit.warp_range)
        }
        None => s.pos + s.forward() * s.kit.warp_range,
    }
}

/// The second pass: every action that did *not* win the helm gets to fire if the
/// pose the governor chose already suits it. Nothing here may steer.
fn opportunities(s: &Situation, t: &AiTuning, plan: &mut Plan) {
    // --- Broadsides. Any bank whose arc happens to cover a hostile offers a
    // solution, no matter what the ship thinks it is doing: this is why ramming
    // past someone rakes them on the way through.
    (plan.port_solution, plan.starboard_solution) = firing_solutions(s);

    // --- EMP. Hold the trigger whenever a hostile is already inside the
    // emitter's cone. The emitter re-checks range and arc itself and only fires
    // on cooldown, so this costs nothing when nothing lines up.
    if !plan.emp_fire {
        plan.emp_fire = s.hostiles.iter().any(|h| {
            h.dist <= s.kit.emp_range
                && wrap((h.pos - s.pos).to_angle() - s.heading).abs() <= s.kit.emp_arc * 0.5
                && h.emp_frac < 1.0
        });
    }

    // --- Torpedoes. Locks are taken from whatever the cursor is resting on, so
    // tubes can only be worked opportunistically when the governor's cursor
    // happens to be on a hostile in range. A jump owns the cursor outright (it
    // is the destination), so no volley builds during one.
    if !plan.torpedo_hold && plan.action != Action::Microwarp && s.kit.tubes >= 1.0 {
        let under_cursor = s.hostiles.iter().any(|h| {
            h.dist <= s.kit.torpedo_range && h.pos.distance(plan.aim_point) <= s.kit.lock_radius
        });
        plan.torpedo_hold = under_cursor && s.kit.locks < volley_size(s, t);
    }

    // --- Boarding. The dwell is positional by definition: if the ship is
    // already alongside a hulk, it is boarding it whether that was the plan or
    // not, so raise the intent and let the dwell run.
    if let Some(p) = s.prize() {
        plan.board |= p.dist <= s.kit.board_range;
    }

    // --- Brace. Free defence, so the only question is when it costs nothing:
    // between volleys, with an enemy close enough to be hitting back.
    let nothing_to_shoot = plan.port_solution.is_none() && plan.starboard_solution.is_none();
    let under_fire = s
        .target()
        .is_some_and(|c| c.dist <= s.kit.gun_reach * t.pilot.brace_range_frac);
    plan.brace |= nothing_to_shoot && under_fire;
}

/// The firing solution each bank has against the best target it bears on.
///
/// Pure geometry against the ship's current pose — this never asks the ship to
/// turn, and it does not pull a trigger either. Both banks may well have a
/// solution; deciding which one the pilot can actually take is [`apply_hands`].
fn firing_solutions(s: &Situation) -> (Option<Solution>, Option<Solution>) {
    let bank = |is_ready: bool, beam_off: f32| -> Option<Solution> {
        if !is_ready {
            return None;
        }
        // The nearest hostile whose lead point falls inside the bank's arc and
        // reach — hostiles are already sorted nearest-first.
        for c in &s.hostiles {
            let Some(lead) = intercept_lead(s.pos, s.kit.muzzle_speed, s.vel, c.pos, c.vel) else {
                continue;
            };
            let to_lead = lead.point - s.pos;
            let dist = to_lead.length();
            if dist > s.kit.gun_reach {
                continue;
            }
            let beam = s.heading + beam_off;
            if wrap(to_lead.to_angle() - beam).abs() > s.kit.gun_arc {
                continue;
            }
            return Some(Solution {
                dist,
                dir: to_lead.normalize_or(s.forward()),
            });
        }
        None
    };
    (
        bank(s.kit.port_ready, FRAC_PI_2),
        bank(s.kit.starboard_ready, -FRAC_PI_2),
    )
}

// ---------------------------------------------------------------------------
// The hands
// ---------------------------------------------------------------------------

/// Cut the plan down to what a player could actually be doing.
///
/// The ship is flown from a gamepad, and that is a real constraint rather than a
/// presentation detail: an AI that holds both triggers, both bumpers and every
/// face button at once is not flying the same ship the player is, and every
/// number tuned against it is tuned against a fiction.
///
/// Two limits, both taken straight off the pad:
///
/// - **One aim.** LT, RT, LB and RB all point at something — a steered volley, a
///   torpedo lock, a jump preview — and there is only one cursor. The pilot gets
///   one of the four.
/// - **One thumb.** X, Y, A and B are one thumb's worth of buttons, and crossing
///   from one to another takes [`PilotTuning::thumb_travel`], during which
///   nothing at all is pressed.
///
/// Both are resolved by priority, and the priorities are the ones a player would
/// use: whatever the stance flying the ship needs comes first, because that is
/// the thing being *done*, and the opportunists take what is left.
pub fn apply_hands(s: &Situation, t: &PilotTuning, plan: &mut Plan, brain: &PilotBrain, dt: f32) {
    // --- The aim.
    //
    // A jump outranks everything: it is already half-primed and letting go early
    // throws the ship somewhere it did not choose. A volley part-built comes
    // next — a player who has started sweeping locks finishes the sweep rather
    // than dribbling tubes out one at a time, which is what releasing early
    // does. Then the guns, the nearer solution first, since the further one is
    // the likelier to have moved out of the arc by the time it lands.
    let committed_volley = plan.torpedo_hold && (plan.action == Action::Torpedo || s.kit.locks > 0);
    let better_gun = match (plan.port_solution, plan.starboard_solution) {
        (Some(p), Some(sb)) => Some(if p.dist <= sb.dist {
            Shoulder::Port
        } else {
            Shoulder::Starboard
        }),
        (Some(_), None) => Some(Shoulder::Port),
        (None, Some(_)) => Some(Shoulder::Starboard),
        (None, None) => None,
    };
    let shoulder = if plan.microwarp_hold {
        Some(Shoulder::Microwarp)
    } else if committed_volley {
        Some(Shoulder::Torpedo)
    } else if let Some(gun) = better_gun {
        Some(gun)
    } else if plan.torpedo_hold {
        Some(Shoulder::Torpedo)
    } else {
        None
    };

    plan.fire = FireOrders::default();
    plan.microwarp_hold = shoulder == Some(Shoulder::Microwarp);
    plan.torpedo_hold = shoulder == Some(Shoulder::Torpedo);
    match shoulder {
        Some(Shoulder::Port) => {
            plan.fire.port = true;
            plan.fire.aim = plan.port_solution.map(|sol| sol.dir);
        }
        Some(Shoulder::Starboard) => {
            plan.fire.starboard = true;
            plan.fire.aim = plan.starboard_solution.map(|sol| sol.dir);
        }
        _ => {}
    }
    plan.shoulder = shoulder;
    // A jump that lost its own hold has not primed anything, so the prime must
    // not carry forward either — it would jump the moment the hold came back.
    if !plan.microwarp_hold {
        plan.warp_prime = 0.0;
    }

    // --- The thumb.
    //
    // Boost first: it is only ever asked for by a stance that is committing the
    // ship somewhere (a ram, a run), and those fail outright without it. Then the
    // EMP, which is the whole point of the opportunistic pass. Brace and the
    // boarding press are what a spare thumb does.
    let wanted = if plan.boost {
        Some(Thumb::Boost)
    } else if plan.emp_fire {
        Some(Thumb::Emp)
    } else if plan.brace {
        Some(Thumb::Brace)
    } else if plan.board {
        Some(Thumb::Board)
    } else {
        None
    };

    // Lifting off a button is free; reaching a different one is not.
    let mut travel = (brain.thumb_travel - dt).max(0.0);
    if wanted.is_some() && wanted != brain.thumb {
        travel = t.thumb_travel;
    }
    let pressed = if travel > 0.0 { None } else { wanted };

    plan.boost = pressed == Some(Thumb::Boost);
    plan.emp_fire = pressed == Some(Thumb::Emp);
    plan.brace = pressed == Some(Thumb::Brace);
    plan.board = pressed == Some(Thumb::Board);
    plan.thumb = wanted;
    plan.thumb_travel = travel;
}

// ---------------------------------------------------------------------------
// The system
// ---------------------------------------------------------------------------

/// Bevy system: run the pilot brain for every ship whose controller has
/// `use_abilities` set, and apply the plan to that ship's helm, fire orders,
/// intent, boost and brace.
///
/// Ships without it (the ordinary enemy) are left to
/// [`ai_system`](crate::ai::ai_system) — the two never write the same ship.
#[allow(clippy::type_complexity)]
pub fn pilot_system(
    time: Res<Time>,
    tuning: Res<SimTuning>,
    mut board_intent: ResMut<BoardIntent>,
    mut ships: Query<
        (
            &Transform,
            &Heading,
            &Velocity,
            &Hull,
            &Faction,
            &AiController,
            &mut PilotBrain,
            (
                &Broadside,
                &EmpWeapon,
                &TorpedoBay,
                &TorpedoLock,
                &MicrowarpDrive,
                &Shield,
            ),
            (
                &mut BoostDrive,
                &mut Brace,
                &mut PilotIntent,
                &mut Helm,
                &mut FireOrders,
            ),
        ),
        (With<Ship>, Without<Disabled>),
    >,
    targets: Query<
        (
            &Transform,
            &Velocity,
            &Hull,
            &Faction,
            &EmpDefense,
            Has<Disabled>,
        ),
        With<Ship>,
    >,
) {
    let dt = time.delta_secs();

    // Snapshot the field once: the mutable pass below cannot also read it.
    let all: Vec<(Vec2, Vec2, f32, f32, Faction, bool)> = targets
        .iter()
        .map(|(tf, vel, hull, faction, emp, disabled)| {
            (
                tf.translation.truncate(),
                vel.0,
                (hull.current / hull.max.max(1.0)).clamp(0.0, 1.0),
                (emp.damage / emp.resist.max(1.0)).clamp(0.0, 1.0),
                *faction,
                disabled,
            )
        })
        .collect();

    for (
        transform,
        heading,
        velocity,
        hull,
        faction,
        ai,
        mut brain,
        (bank, emp, bay, lock, warp, shield),
        (mut boost, mut brace, mut intent, mut helm, mut orders),
    ) in &mut ships
    {
        if !ai.use_abilities {
            continue;
        }
        let pos = transform.translation.truncate();

        let mut hostiles = Vec::new();
        let mut prizes = Vec::new();
        for (t_pos, t_vel, t_hull, t_emp, t_faction, disabled) in &all {
            if !faction.hostile_to(*t_faction) {
                continue;
            }
            let contact = Contact {
                pos: *t_pos,
                vel: *t_vel,
                hull_frac: *t_hull,
                emp_frac: *t_emp,
                dist: t_pos.distance(pos),
            };
            if *disabled {
                prizes.push(contact);
            } else {
                hostiles.push(contact);
            }
        }

        let kit = Kit {
            gun_reach: bank.muzzle_speed * tuning.projectile_ttl,
            gun_arc: bank.arc,
            muzzle_speed: bank.muzzle_speed,
            port_ready: bank.ready(true),
            starboard_ready: bank.ready(false),
            emp_range: emp.range,
            emp_arc: emp.arc,
            torpedo_range: bay.range,
            lock_radius: bay.lock_radius,
            tubes: bay.loaded,
            stock_frac: ((bay.loaded + bay.magazine as f32)
                / (bay.tubes_max as f32 + bay.magazine_max as f32).max(1.0))
            .clamp(0.0, 1.0),
            locks: lock.locks,
            warp_range: warp.range,
            warp_ready: warp.timer <= 0.0,
            board_range: tuning.board_range,
            boost_frac: (boost.battery / boost.battery_max.max(0.01)).clamp(0.0, 1.0),
            fore_frac: shield.fraction(ShieldArc::Fore),
            aft_frac: shield.fraction(ShieldArc::Aft),
            // A cooling bank is one that was hit inside the regen delay — the
            // sim's own record of which side is currently being shot at. Asked
            // per arc rather than read off the banks, so a pooled fit answers
            // the same for both and the pilot never goes looking for the good
            // side of a shield that does not have one.
            fore_suppressed: shield.suppressed(ShieldArc::Fore),
            aft_suppressed: shield.suppressed(ShieldArc::Aft),
        };

        // Hull and shields are both pools of punishment, so they add — but a
        // shield point is discounted, because it only protects the side it
        // happens to be facing. An unshielded hull lands exactly on `hull_frac`.
        let hull_frac = (hull.current / hull.max.max(1.0)).clamp(0.0, 1.0);
        let worth = tuning.ai.pilot.shield_worth;
        let banked = worth * (shield.fore.charge + shield.aft.charge);
        let banked_max = worth * (shield.fore_max + shield.aft_max);
        let integrity =
            ((hull.current + banked) / (hull.max + banked_max).max(1.0)).clamp(0.0, 1.0);

        let situation = Situation::new(
            pos,
            heading.0,
            velocity.0,
            hull_frac,
            integrity,
            hostiles,
            prizes,
            kit,
            *ai,
            tuning.ai.surround_radius,
        );

        let decided = plan(&situation, &tuning.ai, &brain, dt);

        brain.action = decided.action;
        brain.warp_prime = decided.warp_prime;
        brain.thumb = decided.thumb;
        brain.thumb_travel = decided.thumb_travel;
        *helm = decided.helm;
        *orders = decided.fire;
        intent.aim_point = decided.aim_point;
        intent.emp_fire = decided.emp_fire;
        intent.torpedo_hold = decided.torpedo_hold;
        intent.microwarp_hold = decided.microwarp_hold;
        boost.active = decided.boost;
        brace.active = decided.brace;
        if decided.board {
            board_intent.active = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kit() -> Kit {
        Kit {
            gun_reach: 812.0,
            gun_arc: 1.178,
            muzzle_speed: 325.0,
            port_ready: true,
            starboard_ready: true,
            emp_range: 620.0,
            emp_arc: FRAC_PI_2,
            torpedo_range: 506.0,
            lock_radius: 112.5,
            tubes: 6.0,
            stock_frac: 1.0,
            locks: 0,
            warp_range: 506.0,
            warp_ready: true,
            board_range: 95.0,
            boost_frac: 1.0,
            fore_frac: 0.0,
            aft_frac: 0.0,
            fore_suppressed: false,
            aft_suppressed: false,
        }
    }

    fn contact(pos: Vec2) -> Contact {
        Contact {
            pos,
            vel: Vec2::ZERO,
            hull_frac: 1.0,
            emp_frac: 0.0,
            dist: pos.length(),
        }
    }

    /// A situation from the origin, facing +X, at full hull with a full kit.
    /// The default hull is unshielded, so hull and integrity agree.
    fn at(hostiles: Vec<Contact>, prizes: Vec<Contact>, kit: Kit) -> Situation {
        Situation::new(
            Vec2::ZERO,
            0.0,
            Vec2::ZERO,
            1.0,
            1.0,
            hostiles,
            prizes,
            kit,
            AiController::piloting(),
            400.0,
        )
    }

    /// Hurt an unshielded hull: with no banks fitted, plate is all the ship has,
    /// so both survivability numbers move together.
    fn wound(s: &mut Situation, frac: f32) {
        s.hull_frac = frac;
        s.integrity = frac;
    }

    /// One step, from a cold start — hands empty, thumb nowhere.
    fn decide(s: &Situation) -> Plan {
        plan(s, &AiTuning::default(), &PilotBrain::default(), 1.0 / 60.0)
    }

    /// Let the pilot fly the same situation for `seconds`, feeding its own state
    /// forward. Anything on a face button needs this: a thumb has to physically
    /// reach the button before it can press it, so the first step of a fight
    /// never has the EMP down.
    fn settle(s: &Situation, seconds: f32) -> Plan {
        let t = AiTuning::default();
        let step = 1.0 / 60.0;
        let mut brain = PilotBrain::default();
        let mut out = plan(s, &t, &brain, step);
        for _ in 0..(seconds / step) as u32 {
            brain.action = out.action;
            brain.warp_prime = out.warp_prime;
            brain.thumb = out.thumb;
            brain.thumb_travel = out.thumb_travel;
            out = plan(s, &t, &brain, step);
        }
        out
    }

    #[test]
    fn a_healthy_ship_with_an_enemy_on_the_beam_fights_the_broadside() {
        let s = at(vec![contact(Vec2::new(0.0, 250.0))], vec![], kit());
        let p = decide(&s);
        assert_eq!(p.action, Action::Broadside, "scores were {:?}", p.scores);
        assert!(p.fire.port, "and rakes them on the way past");
    }

    /// The bug this whole architecture exists to prevent: the EMP has more than
    /// twice the reach of the broadside, so a priority chain that checks it
    /// first flies every engagement bow-on and never fires a gun.
    #[test]
    fn emp_never_outbids_a_working_broadside() {
        let s = at(vec![contact(Vec2::new(250.0, 0.0))], vec![], kit());
        let p = decide(&s);
        assert_ne!(p.action, Action::Emp);
        assert!(
            p.score_of(Action::Emp) < p.score_of(Action::Broadside),
            "emp {} vs broadside {}",
            p.score_of(Action::Emp),
            p.score_of(Action::Broadside)
        );
    }

    /// ...but it still fires, unasked, whenever the bow happens to sweep across
    /// someone. That is the opportunistic half of the design.
    #[test]
    fn emp_fires_opportunistically_when_the_bow_crosses_a_target() {
        // Dead ahead and well inside emitter range, but the ship is fighting
        // its broadside, not jockeying for the shot.
        let s = at(vec![contact(Vec2::new(400.0, 0.0))], vec![], kit());
        let p = settle(&s, 0.5);
        assert!(p.emp_fire, "the emitter takes what the arc offers");

        // Directly astern: outside the emitter's cone, so the trigger stays up.
        let behind = at(vec![contact(Vec2::new(-400.0, 0.0))], vec![], kit());
        let q = settle(&behind, 0.5);
        assert!(!q.emp_fire, "nothing in the arc, nothing to hold down for");
    }

    /// EMP is the answer when there is no other answer: banks reloading, tubes
    /// dry. Then it is worth turning the bow for.
    #[test]
    fn emp_takes_the_helm_when_the_ship_has_nothing_else() {
        let mut k = kit();
        k.port_ready = false;
        k.starboard_ready = false;
        k.tubes = 0.0;
        k.stock_frac = 0.0;
        k.boost_frac = 0.0; // no battery, so ramming is not the answer either
        let s = at(vec![contact(Vec2::new(300.0, 0.0))], vec![], k);
        let p = decide(&s);
        assert_eq!(p.action, Action::Emp, "scores were {:?}", p.scores);
    }

    /// A dry magazine is worth less than a full one, so the brain gets
    /// progressively less willing to open a volley as stores run down.
    #[test]
    fn torpedoes_are_worth_less_as_the_magazine_empties() {
        let full = at(vec![contact(Vec2::new(450.0, 0.0))], vec![], kit());
        let mut low = kit();
        low.stock_frac = 0.1;
        let scarce = at(vec![contact(Vec2::new(450.0, 0.0))], vec![], low);
        let t = PilotTuning::default();
        assert!(
            score_torpedo(&full, &t) > score_torpedo(&scarce, &t),
            "scarcity must cost: {} vs {}",
            score_torpedo(&full, &t),
            score_torpedo(&scarce, &t)
        );
    }

    #[test]
    fn an_empty_bay_never_argues_for_torpedoes() {
        let mut k = kit();
        k.tubes = 0.0;
        k.stock_frac = 0.0;
        let s = at(vec![contact(Vec2::new(450.0, 0.0))], vec![], k);
        let p = decide(&s);
        assert_eq!(p.score_of(Action::Torpedo), 0.0);
        assert!(!p.torpedo_hold, "and never holds tubes it does not have");
    }

    /// With the banks reloading and the tubes empty, a ship with battery left
    /// puts its bow through them instead of drifting.
    #[test]
    fn a_dry_ship_rams() {
        let mut k = kit();
        k.port_ready = false;
        k.starboard_ready = false;
        k.tubes = 0.0;
        k.stock_frac = 0.0;
        let s = at(vec![contact(Vec2::new(180.0, 0.0))], vec![], k);
        let p = settle(&s, 0.5);
        assert_eq!(p.action, Action::Ram, "scores were {:?}", p.scores);
        assert!(p.boost, "a ram is worth the battery");
        assert!(p.helm.throttle > 0.9, "and everything the engines have");
    }

    /// Ramming is a mutual exchange, so a nearly dead ship must not choose it.
    #[test]
    fn a_wounded_ship_will_not_trade_rams() {
        let mut k = kit();
        k.port_ready = false;
        k.starboard_ready = false;
        k.tubes = 0.0;
        k.stock_frac = 0.0;
        let mut s = at(vec![contact(Vec2::new(180.0, 0.0))], vec![], k);
        wound(&mut s, 0.2);
        let p = decide(&s);
        assert_ne!(p.action, Action::Ram, "scores were {:?}", p.scores);
    }

    #[test]
    fn a_hurt_ship_breaks_off() {
        let mut s = at(vec![contact(Vec2::new(200.0, 0.0))], vec![], kit());
        wound(&mut s, 0.05);
        let p = decide(&s);
        assert!(
            matches!(p.action, Action::Disengage | Action::Microwarp),
            "should run or jump, chose {:?} from {:?}",
            p.action,
            p.scores
        );
    }

    /// A prize with the field clear is the sustain loop — hull and torpedoes
    /// both come back — so it should outbid chasing nothing in particular.
    #[test]
    fn a_quiet_field_with_a_hulk_in_it_goes_looting() {
        let s = at(vec![], vec![contact(Vec2::new(300.0, 0.0))], kit());
        let p = decide(&s);
        assert_eq!(p.action, Action::Board, "scores were {:?}", p.scores);
    }

    /// ...but not with a live enemy at knife range. Boarding is three seconds
    /// sitting still.
    #[test]
    fn boarding_waits_until_the_shooting_stops() {
        let s = at(
            vec![contact(Vec2::new(120.0, 0.0))],
            vec![contact(Vec2::new(150.0, 0.0))],
            kit(),
        );
        let p = decide(&s);
        assert_ne!(p.action, Action::Board, "scores were {:?}", p.scores);
    }

    /// A hungry ship — hurt, and nearly out of torpedoes — should want a prize
    /// more than a healthy, fully stocked one does.
    #[test]
    fn hunger_pulls_harder_toward_a_prize() {
        let t = PilotTuning::default();
        let fed = at(vec![], vec![contact(Vec2::new(400.0, 0.0))], kit());
        let mut lean_kit = kit();
        lean_kit.stock_frac = 0.05;
        let mut lean = at(vec![], vec![contact(Vec2::new(400.0, 0.0))], lean_kit);
        wound(&mut lean, 0.4);
        assert!(score_board(&lean, &t) > score_board(&fed, &t));
    }

    /// Sitting alongside a hulk claims it whether or not boarding was the plan —
    /// the dwell is positional. Which is just as well: mid-fight the pilot's one
    /// thumb is on the EMP, so it is not pressing the boarding button at all, and
    /// the prize comes in anyway.
    #[test]
    fn a_hulk_alongside_is_claimed_without_a_thumb_to_spare() {
        let s = at(
            vec![contact(Vec2::new(150.0, 0.0))],
            vec![contact(Vec2::new(40.0, 0.0))],
            kit(),
        );
        let p = settle(&s, 0.5);
        assert_ne!(p.action, Action::Board, "the fight still owns the helm");
        assert_eq!(
            p.thumb,
            Some(Thumb::Emp),
            "and the one thumb there is, is on the emitter"
        );
        assert!(!p.board, "so nobody is pressing B");
    }

    #[test]
    fn surrounded_and_hurt_the_ship_jumps_clear() {
        let mut s = at(
            vec![
                contact(Vec2::new(150.0, 30.0)),
                contact(Vec2::new(160.0, -40.0)),
                contact(Vec2::new(120.0, 10.0)),
            ],
            vec![],
            kit(),
        );
        wound(&mut s, 0.5);
        let p = decide(&s);
        assert_eq!(p.action, Action::Microwarp, "scores were {:?}", p.scores);
        assert!(p.aim_point.x < 0.0, "away from the crowd, not into it");
    }

    /// A jump on cooldown is not a plan, whatever it would be worth.
    #[test]
    fn a_cooling_drive_never_wins() {
        let mut k = kit();
        k.warp_ready = false;
        let mut s = at(
            vec![
                contact(Vec2::new(150.0, 30.0)),
                contact(Vec2::new(160.0, -40.0)),
                contact(Vec2::new(120.0, 10.0)),
            ],
            vec![],
            k,
        );
        wound(&mut s, 0.5);
        let p = decide(&s);
        assert_ne!(p.action, Action::Microwarp);
        assert_eq!(p.score_of(Action::Microwarp), 0.0);
    }

    /// The jump also closes distance: an enemy right across the field is worth
    /// crossing to, and the destination stops short of them rather than landing
    /// in their lap.
    #[test]
    fn a_distant_enemy_is_worth_jumping_toward() {
        let s = at(vec![contact(Vec2::new(900.0, 0.0))], vec![], kit());
        let p = decide(&s);
        assert_eq!(p.action, Action::Microwarp, "scores were {:?}", p.scores);
        assert!(p.aim_point.x > 0.0, "toward them");
        assert!(
            p.aim_point.x < 900.0 - s.ai.engage_range * 0.5,
            "but stopping short: {}",
            p.aim_point.x
        );
    }

    /// The commitment bonus keeps the ship from twitching between two actions
    /// scoring within a whisker of each other — but must never revive one that
    /// has ruled itself out.
    #[test]
    fn commitment_steadies_the_helm_without_reviving_the_impossible() {
        let s = at(vec![contact(Vec2::new(450.0, 0.0))], vec![], kit());
        let t = AiTuning::default();
        let torpedoing = PilotBrain {
            action: Action::Torpedo,
            ..Default::default()
        };
        let held = plan(&s, &t, &torpedoing, 1.0 / 60.0);
        assert_eq!(held.action, Action::Torpedo, "incumbency should hold it");

        let mut dry = kit();
        dry.tubes = 0.0;
        dry.stock_frac = 0.0;
        let empty = at(vec![contact(Vec2::new(450.0, 0.0))], vec![], dry);
        let dropped = plan(&empty, &t, &torpedoing, 1.0 / 60.0);
        assert_ne!(dropped.action, Action::Torpedo, "an empty bay is no plan");
    }

    /// Nothing on the field: hold station rather than steering somewhere
    /// arbitrary.
    #[test]
    fn an_empty_field_holds_station() {
        let p = decide(&at(vec![], vec![], kit()));
        assert_eq!(p.action, Action::Hold);
        assert_eq!(p.helm.throttle, 0.0);
        assert!(!p.emp_fire && !p.torpedo_hold && !p.microwarp_hold);
    }

    /// Locks are taken from whatever the cursor rests on, and a jump owns the
    /// cursor — so no volley may build during one.
    #[test]
    fn a_jump_does_not_try_to_build_a_volley() {
        let mut s = at(
            vec![
                contact(Vec2::new(150.0, 30.0)),
                contact(Vec2::new(160.0, -40.0)),
                contact(Vec2::new(120.0, 10.0)),
            ],
            vec![],
            kit(),
        );
        wound(&mut s, 0.5);
        let p = decide(&s);
        assert_eq!(p.action, Action::Microwarp);
        assert!(!p.torpedo_hold);
    }

    /// Guns fire on the solution, not at where the target used to be.
    #[test]
    fn the_volley_is_laid_on_the_firing_solution() {
        let mut c = contact(Vec2::new(0.0, 250.0));
        c.vel = Vec2::new(120.0, 0.0); // crossing fast, to starboard-ahead
        let s = at(vec![c], vec![], kit());
        let p = decide(&s);
        assert!(p.fire.port);
        let aim = p.fire.aim.expect("a volley should carry its solution");
        assert!(aim.x > 0.0, "must lead ahead of the target: {aim:?}");
    }

    // -----------------------------------------------------------------------
    // Two hands
    // -----------------------------------------------------------------------

    /// One aim between four inputs. With a hostile off each beam both banks have
    /// a solution, and a player could still only throw one volley — the nearer,
    /// because the further target is likelier to have left the arc by the time
    /// the shot arrives.
    #[test]
    fn only_one_volley_leaves_at_a_time() {
        let s = at(
            vec![
                contact(Vec2::new(0.0, 200.0)),
                contact(Vec2::new(0.0, -260.0)),
            ],
            vec![],
            kit(),
        );
        let p = decide(&s);
        assert!(
            p.port_solution.is_some() && p.starboard_solution.is_some(),
            "both banks should bear"
        );
        assert!(
            p.fire.port ^ p.fire.starboard,
            "but only one trigger can be pulled"
        );
        assert!(p.fire.port, "and it is the nearer solution");
        assert_eq!(p.shoulder, Some(Shoulder::Port));
    }

    /// A volley half-built keeps the aim. Letting go early is not free — it
    /// launches what is locked — so a pilot that surrendered the bumper every
    /// time a bank came to bear would dribble its tubes out one at a time.
    #[test]
    fn a_half_built_volley_keeps_the_aim_from_the_guns() {
        let mut k = kit();
        k.locks = 2; // mid-sweep
        let s = at(vec![contact(Vec2::new(0.0, 200.0))], vec![], k);
        let p = decide(&s);
        assert!(p.port_solution.is_some(), "the bank does bear");
        assert!(!p.fire.port, "but the bumper is spoken for");
        assert!(p.torpedo_hold);
        assert_eq!(p.shoulder, Some(Shoulder::Torpedo));
    }

    /// A jump outranks everything with an aim on it: releasing the bumper early
    /// throws the ship at whatever the cursor was resting on.
    #[test]
    fn priming_a_jump_takes_the_aim_off_the_guns() {
        let mut s = at(
            vec![
                contact(Vec2::new(150.0, 30.0)),
                contact(Vec2::new(160.0, -40.0)),
                contact(Vec2::new(120.0, 10.0)),
            ],
            vec![],
            kit(),
        );
        wound(&mut s, 0.5);
        let p = decide(&s);
        assert_eq!(p.action, Action::Microwarp);
        assert_eq!(p.shoulder, Some(Shoulder::Microwarp));
        assert!(!p.fire.port && !p.fire.starboard);
    }

    /// One thumb. A ram wants boost *and* brace and can only have one of them —
    /// boost, because a ram without it is just a collision.
    #[test]
    fn the_thumb_cannot_boost_and_brace_at_once() {
        let mut k = kit();
        k.port_ready = false;
        k.starboard_ready = false;
        k.tubes = 0.0;
        k.stock_frac = 0.0;
        let s = at(vec![contact(Vec2::new(180.0, 0.0))], vec![], k);
        let p = settle(&s, 0.5);
        assert_eq!(p.action, Action::Ram);
        assert!(p.boost, "the drive is what a ram is made of");
        assert!(!p.brace, "and the same thumb cannot also be on Y");
    }

    /// Crossing the pad takes time, and nothing is pressed on the way. The first
    /// step of an engagement never has a face button down.
    #[test]
    fn a_thumb_has_to_reach_the_button_first() {
        let s = at(vec![contact(Vec2::new(400.0, 0.0))], vec![], kit());
        let cold = decide(&s);
        assert_eq!(cold.thumb, Some(Thumb::Emp), "on its way to X");
        assert!(!cold.emp_fire, "but not there yet");
        assert!(cold.thumb_travel > 0.0);

        // Well short of the traversal: still in the air.
        assert!(!settle(&s, 0.1).emp_fire);
        // Past it: pressed, and staying pressed.
        assert!(settle(&s, 0.3).emp_fire);
    }

    /// Changing your mind costs the traversal again — which is the whole reason
    /// the pilot cannot flicker between EMP and brace every other step.
    #[test]
    fn changing_buttons_pays_the_crossing_again() {
        let s = at(vec![contact(Vec2::new(400.0, 0.0))], vec![], kit());
        let t = AiTuning::default();
        // Settled on the emitter...
        let settled = settle(&s, 0.5);
        assert!(settled.emp_fire);

        // ...now a stance that wants the boost instead. The step it switches, the
        // thumb is in the air and neither button is down.
        let mut dry = kit();
        dry.port_ready = false;
        dry.starboard_ready = false;
        dry.tubes = 0.0;
        dry.stock_frac = 0.0;
        let ramming = at(vec![contact(Vec2::new(180.0, 0.0))], vec![], dry);
        let brain = PilotBrain {
            action: Action::Ram,
            thumb: settled.thumb,
            thumb_travel: 0.0,
            warp_prime: 0.0,
        };
        let switching = plan(&ramming, &t, &brain, 1.0 / 60.0);
        assert_eq!(switching.thumb, Some(Thumb::Boost));
        assert!(!switching.boost && !switching.emp_fire, "mid-crossing");
        assert!((switching.thumb_travel - t.pilot.thumb_travel).abs() < 1e-6);
    }

    // -----------------------------------------------------------------------
    // Shields
    // -----------------------------------------------------------------------

    /// A shielded hull to fight in: both banks full, nothing suppressed.
    fn shielded() -> Kit {
        Kit {
            fore_frac: 1.0,
            aft_frac: 1.0,
            ..kit()
        }
    }

    /// Five percent hull is an emergency on a bare hull and merely bad news
    /// behind full banks — there is still punishment left in the ship, and
    /// running from a fight it can still take throws the position away.
    #[test]
    fn full_shields_buy_time_before_running() {
        let bare = {
            let mut s = at(vec![contact(Vec2::new(200.0, 0.0))], vec![], kit());
            wound(&mut s, 0.05);
            s
        };
        let behind_banks = {
            let mut s = at(vec![contact(Vec2::new(200.0, 0.0))], vec![], shielded());
            s.hull_frac = 0.05;
            s.integrity = 0.6; // most of the ship's remaining punishment is shield
            s
        };
        assert_eq!(decide(&bare).action, Action::Disengage);
        assert_ne!(
            decide(&behind_banks).action,
            Action::Disengage,
            "shields are survivability: keep fighting while they hold"
        );

        // ...and the moment they collapse, the same hull runs.
        let mut collapsed = behind_banks;
        collapsed.kit.fore_frac = 0.0;
        collapsed.kit.aft_frac = 0.0;
        collapsed.integrity = 0.05;
        assert_eq!(decide(&collapsed).action, Action::Disengage);
    }

    /// The arcs are two independent banks, so which way the ship points decides
    /// which one soaks. A flat bow that is still being shot at is the worst of
    /// both — it stops nothing and cannot regenerate while it stays facing them —
    /// so a stance that shows the stern is worth more than one that shows the bow.
    #[test]
    fn a_pinned_bow_bank_favours_showing_the_stern() {
        let mut k = shielded();
        k.fore_frac = 0.0;
        k.fore_suppressed = true; // being raked on the bow right now
        let s = at(vec![contact(Vec2::new(200.0, 0.0))], vec![], k);
        let t = PilotTuning::default();
        assert!(
            presentation(&s, false, &t) > presentation(&s, true, &t),
            "stern {} should beat bow {}",
            presentation(&s, false, &t),
            presentation(&s, true, &t)
        );

        // Flip the damage to the stern and the argument flips with it: now the
        // ship wants to turn and face them.
        let mut turned = s;
        turned.kit = Kit {
            fore_frac: 1.0,
            fore_suppressed: false,
            aft_frac: 0.0,
            aft_suppressed: true,
            ..turned.kit
        };
        assert!(presentation(&turned, true, &t) > presentation(&turned, false, &t));
    }

    /// A pooled shield has no better side, so it must not bias the facing at
    /// all — even flat and suppressed, where a directional fit would be
    /// screaming to turn about.
    #[test]
    fn a_pooled_shield_never_argues_about_facing() {
        // What `Shield::fraction` and `suppressed` report for a pooled fit:
        // the same answer for both arcs.
        let mut k = shielded();
        k.fore_frac = 0.0;
        k.aft_frac = 0.0;
        k.fore_suppressed = true;
        k.aft_suppressed = true;
        let s = at(vec![contact(Vec2::new(200.0, 0.0))], vec![], k);
        let t = PilotTuning::default();
        assert_eq!(presentation(&s, true, &t), 1.0);
        assert_eq!(presentation(&s, false, &t), 1.0);
    }

    /// Facing only matters while someone is shooting: with the field clear the
    /// shields must not bias the decision at all.
    #[test]
    fn presentation_costs_nothing_with_nobody_in_range() {
        let mut k = shielded();
        k.fore_frac = 0.0;
        k.fore_suppressed = true;
        // The only hostile is far beyond gun reach, so `danger` is zero.
        let s = at(vec![contact(Vec2::new(4000.0, 0.0))], vec![], k);
        let t = PilotTuning::default();
        assert_eq!(s.danger, 0.0);
        assert_eq!(presentation(&s, true, &t), 1.0);
        assert_eq!(presentation(&s, false, &t), 1.0);
    }

    /// An unshielded hull must behave exactly as it did before shields entered
    /// the arithmetic — both fractions are zero, so the term is identically 1.
    #[test]
    fn an_unshielded_hull_is_untouched_by_any_of_this() {
        let s = at(vec![contact(Vec2::new(150.0, 0.0))], vec![], kit());
        let t = PilotTuning::default();
        assert_eq!(presentation(&s, true, &t), 1.0);
        assert_eq!(presentation(&s, false, &t), 1.0);
    }

    /// A ram is paid for out of the bow. With the fore bank up it is a far
    /// better bargain than with the bow already stove in.
    #[test]
    fn a_charged_bow_makes_a_ram_worth_more() {
        let dry = |mut k: Kit| {
            k.port_ready = false;
            k.starboard_ready = false;
            k.tubes = 0.0;
            k.stock_frac = 0.0;
            k
        };
        let t = PilotTuning::default();

        let mut strong = at(
            vec![contact(Vec2::new(180.0, 0.0))],
            vec![],
            dry(shielded()),
        );
        strong.hull_frac = 0.4;
        strong.integrity = 0.75; // banks carrying most of it

        let mut stove_in = at(
            vec![contact(Vec2::new(180.0, 0.0))],
            vec![],
            dry(Kit {
                fore_frac: 0.0,
                fore_suppressed: true,
                ..shielded()
            }),
        );
        stove_in.hull_frac = 0.4;
        stove_in.integrity = 0.55;

        assert!(
            score_ram(&strong, &t) > score_ram(&stove_in, &t),
            "{} vs {}",
            score_ram(&strong, &t),
            score_ram(&stove_in, &t)
        );
    }

    /// The whole system, through a real ECS schedule: an AI-piloted ship must
    /// drive its own helm and orders, and must not touch a ship whose controller
    /// leaves `use_abilities` off.
    #[test]
    fn pilot_system_flies_only_the_ships_that_asked_for_it() {
        use crate::components::{Collider, ShipStats, SpeedScale};
        use crate::shield::Shield;
        use crate::spawn::{ship_bundle, ShipLoadout};
        use bevy_app::prelude::*;

        let mut app = App::new();
        app.init_resource::<SimTuning>()
            .init_resource::<BoardIntent>()
            .init_resource::<Time>()
            .add_systems(Update, pilot_system);

        let loadout = ShipLoadout::default();
        let pilot = app
            .world_mut()
            .spawn((
                ship_bundle(
                    Faction::Corsairs,
                    ShipStats::default(),
                    100.0,
                    Vec2::ZERO,
                    0.0,
                    loadout,
                ),
                AiController::piloting(),
            ))
            .id();
        // An ordinary enemy: same hull, but its controller does not use the kit.
        let enemy = app
            .world_mut()
            .spawn((
                ship_bundle(
                    Faction::Houses,
                    ShipStats::default(),
                    100.0,
                    Vec2::new(0.0, 250.0),
                    0.0,
                    loadout,
                ),
                AiController::default(),
            ))
            .id();

        app.update();

        let world = app.world();
        let helm = world.get::<Helm>(pilot).unwrap();
        assert!(
            helm.throttle != 0.0 || helm.turn != 0.0,
            "the pilot should be flying the ship"
        );
        assert_eq!(
            world.get::<PilotBrain>(pilot).unwrap().action,
            Action::Broadside
        );
        let enemy_helm = world.get::<Helm>(enemy).unwrap();
        assert_eq!(enemy_helm.throttle, 0.0, "an enemy is not the pilot's ship");
        assert_eq!(world.get::<PilotBrain>(enemy).unwrap().action, Action::Hold);

        // Keep the unused-import warnings honest: these ride along on the bundle.
        assert!(world.get::<Shield>(pilot).is_some());
        assert!(world.get::<Collider>(pilot).is_some());
        assert!(world.get::<SpeedScale>(pilot).is_some());
    }
}

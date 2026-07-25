//! Pushing an edited ship class onto the ships already flying.
//!
//! The panel edits the *class*, because that is the thing that can be saved and
//! that survives a restart. This is the other half: whenever the class table
//! changes — from the panel or from a hot reload — copy the authored fields onto
//! every live ship wearing that class.
//!
//! The care is all in "authored fields only". A `Broadside` carries two
//! `BankState`s holding live reload timers, and a `TorpedoBay` carries the
//! number of tubes currently loaded. Copying the class wholesale would reset a
//! reload mid-cycle and refill the magazine on every slider tick, which reads as
//! the game cheating rather than as tuning. So the merge walks both values in
//! lockstep and copies only what [`FieldKind::Config`] covers.

use bevy::prelude::*;
use bevy::reflect::{PartialReflect, ReflectMut, ReflectRef};
use vt_sim::prelude::{
    AiController, Broadside, ClassId, Collider, EmpDefense, EmpWeapon, Hull, MicrowarpDrive,
    ShipStats, TorpedoBay,
};

use crate::data::ShipTable;

use super::fields::{owner_of, spec_for, FieldKind};

/// Copy `from`'s authored fields onto `into`, leaving live state alone.
///
/// Both must be the same type. Fields the descriptor table marks `Live` are
/// skipped whole, including their subtrees.
pub fn merge_config(into: &mut dyn PartialReflect, from: &dyn PartialReflect) {
    let owner = owner_of(&*into).to_string();
    let (ReflectMut::Struct(dst), ReflectRef::Struct(src)) =
        (into.reflect_mut(), from.reflect_ref())
    else {
        // A leaf: no live/config distinction to make, so take the new value.
        into.apply(from);
        return;
    };

    for i in 0..dst.field_len() {
        let Some(name) = dst.name_at(i).map(str::to_string) else {
            continue;
        };
        if spec_for(&owner, &name, 0.0).kind == FieldKind::Live {
            continue; // belongs to the running ship, not to its class
        }
        let (Some(dst_field), Some(src_field)) = (dst.field_at_mut(i), src.field(&name)) else {
            continue;
        };
        merge_config(dst_field, src_field);
    }
}

/// Push class edits onto every live ship of that class.
///
/// Runs only when the table actually changed, so the ordinary frame cost is one
/// change-detection check.
#[allow(clippy::type_complexity)]
pub fn apply_class_edits(
    table: Res<ShipTable>,
    mut ships: Query<(
        &ClassId,
        &mut ShipStats,
        &mut Hull,
        &mut Collider,
        &mut EmpDefense,
        &mut Broadside,
        &mut EmpWeapon,
        &mut TorpedoBay,
        &mut MicrowarpDrive,
        Option<&mut AiController>,
    )>,
) {
    if !table.is_changed() {
        return;
    }

    for (
        class_id,
        mut stats,
        mut hull,
        mut collider,
        mut emp_def,
        mut bank,
        mut emp,
        mut bay,
        mut warp,
        ai,
    ) in &mut ships
    {
        let Some(class) = table.get(*class_id) else {
            // The class was removed by a reload. Leaving the ship exactly as it
            // is beats guessing at a replacement.
            continue;
        };

        *stats = class.stats;
        *collider = class.collider;
        merge_config(emp_def.as_mut(), &class.emp_defense);
        merge_config(bank.as_mut(), &class.loadout.broadside);
        merge_config(emp.as_mut(), &class.loadout.emp);
        merge_config(bay.as_mut(), &class.loadout.torpedoes);
        merge_config(warp.as_mut(), &class.loadout.microwarp);
        if let Some(mut ai) = ai {
            merge_config(ai.as_mut(), &class.ai);
        }

        // Hull needs a rule of its own, because `max` and `current` mean
        // different things: raising the maximum must not heal a damaged ship,
        // and lowering it must not kill one that was already below the new cap.
        hull.max = class.hull;
        hull.current = hull.current.min(hull.max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vt_sim::prelude::BankState;

    /// The property the whole merge exists for: an edit to a class's damage must
    /// not disturb the reload timer of a bank that is mid-cycle.
    #[test]
    fn merging_a_broadside_leaves_the_reload_timers_alone() {
        let mut live = Broadside {
            damage: 12.0,
            port: BankState {
                timer: 0.8,
                charging: 0.3,
                ..BankState::default()
            },
            ..Broadside::default()
        };
        let authored = Broadside {
            damage: 40.0,
            ..Broadside::default() // its banks are fresh/zeroed
        };

        merge_config(&mut live, &authored);

        assert_eq!(live.damage, 40.0, "the authored change must land");
        assert_eq!(live.port.timer, 0.8, "a live reload must survive the edit");
        assert_eq!(live.port.charging, 0.3, "as must a live telegraph");
    }

    /// Same story for a magazine: tuning torpedo damage must not reload the ship.
    #[test]
    fn merging_a_torpedo_bay_does_not_refill_it() {
        let mut live = TorpedoBay {
            loaded: 1.0,
            damage: 22.0,
            ..TorpedoBay::default()
        };
        let authored = TorpedoBay {
            damage: 50.0,
            ..TorpedoBay::default() // loaded: 6.0
        };

        merge_config(&mut live, &authored);

        assert_eq!(live.damage, 50.0);
        assert_eq!(live.loaded, 1.0, "a spent magazine must stay spent");
    }

    /// EMP soak is live state too — retuning resistance must not clear it.
    #[test]
    fn merging_emp_defense_keeps_the_soaked_damage() {
        let mut live = EmpDefense {
            damage: 60.0,
            ..EmpDefense::default()
        };
        let authored = EmpDefense {
            resist: 250.0,
            ..EmpDefense::default()
        };

        merge_config(&mut live, &authored);

        assert_eq!(live.resist, 250.0);
        assert_eq!(live.damage, 60.0, "the ship is still EMP'd");
    }
}

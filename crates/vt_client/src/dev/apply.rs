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
    AiController, Battery, BoostDrive, Broadside, ClassId, Collider, EmpDefense, EmpWeapon,
    FireBarrelRack, Hull, MicrowarpDrive, PointDefense, Shield, ShipStats, TorpedoBay,
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
        //
        // `try_apply` rather than `apply`, which unwraps: a kind mismatch here
        // means a caller paired two different types, and the schema moving
        // under the panel should cost one merge rather than the whole session.
        if let Err(e) = into.try_apply(from) {
            warn!("design panel: could not merge {owner}: {e:?}");
        }
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

/// Merge an authored loadout slot onto the component fitted for it.
///
/// A slot the class leaves `None` and a component the ship does not carry say
/// the same thing — there is nothing to merge — and neither is an error. What a
/// hull *carries* is decided once, at spawn, by its fit; a slider may retune a
/// device but must never bolt one on or tear one off mid-flight.
///
/// This pairing is also what keeps an `Option` away from [`merge_config`]: an
/// `Option` reflects as an enum, and handing one to a walker expecting a struct
/// is what used to take the game down on the first edit.
fn merge_slot<T: Component + PartialReflect>(live: Option<Mut<T>>, authored: &Option<T>) {
    if let (Some(mut live), Some(authored)) = (live, authored.as_ref()) {
        merge_config(live.as_mut(), authored);
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
        // The hull every ship has, nested to stay inside Bevy's tuple limit now
        // that the whole fit is queried alongside it.
        (
            &mut ShipStats,
            &mut Hull,
            &mut Collider,
            &mut EmpDefense,
            &mut Broadside,
            &mut Shield,
        ),
        // Every device is a fit the loadout may leave off; a non-optional query
        // here silently dropped the whole ship out of the panel's reach the
        // moment one was missing.
        (
            Option<&mut Battery>,
            Option<&mut BoostDrive>,
            Option<&mut EmpWeapon>,
            Option<&mut PointDefense>,
        ),
        (
            Option<&mut TorpedoBay>,
            Option<&mut MicrowarpDrive>,
            Option<&mut FireBarrelRack>,
            Option<&mut AiController>,
        ),
    )>,
) {
    if !table.is_changed() {
        return;
    }

    for (
        class_id,
        (mut stats, mut hull, mut collider, mut emp_def, mut bank, mut shield),
        (battery, boost, emp, point_defense),
        (bay, warp, barrels, ai),
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
        // The two every hull carries.
        merge_config(bank.as_mut(), &class.loadout.broadside);
        merge_config(shield.as_mut(), &class.loadout.shield);
        // The fitted half.
        merge_slot(battery, &class.loadout.battery);
        merge_slot(boost, &class.loadout.boost);
        merge_slot(emp, &class.loadout.emp);
        merge_slot(point_defense, &class.loadout.point_defense);
        merge_slot(bay, &class.loadout.torpedoes);
        merge_slot(warp, &class.loadout.microwarp);
        merge_slot(barrels, &class.loadout.barrels);
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
    use crate::data::ships::ShipClass;
    use vt_sim::prelude::{BankState, ShipLoadout};

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

    /// A ship is a fit, not a fixed kit. A hull that carries no tubes must still
    /// track its class: the query used to demand every device, so one missing
    /// component quietly took the whole ship out of the panel's reach and edits
    /// to its hull, stats and guns stopped landing.
    #[test]
    fn a_ship_without_the_full_kit_still_tracks_its_class() {
        use crate::data::ships::{NamedClass, ShipTable};
        use vt_sim::prelude::{spawn_ship_in, Faction};

        let mut app = App::new();
        app.insert_resource(ShipTable {
            classes: vec![NamedClass {
                name: "stripped".into(),
                class: ShipClass {
                    hull: 250.0,
                    ..ShipClass::default()
                },
            }],
        })
        .add_systems(Update, apply_class_edits);

        // An all-`None` fit: a hull with guns and nothing else.
        let ship = spawn_ship_in(
            app.world_mut(),
            Faction::Corsairs,
            ShipStats::default(),
            100.0,
            Vec2::ZERO,
            0.0,
            Default::default(),
        )
        .insert(ClassId(0))
        .id();
        assert!(app.world().get::<TorpedoBay>(ship).is_none());

        app.update();

        assert_eq!(
            app.world().get::<Hull>(ship).unwrap().max,
            250.0,
            "the class edit must reach a ship that is missing devices"
        );
    }

    /// A fitted ship, which is the case the all-`None` test above cannot reach.
    ///
    /// The loadout's slots are `Option`s, and an `Option` reflects as an *enum*.
    /// Handing one straight to the struct walker took the game down on the first
    /// slider drag — `MismatchedKinds { from_kind: Enum, to_kind: Struct }` —
    /// for every ship that actually carried a device, which is to say the
    /// player's, immediately.
    #[test]
    fn a_fully_fitted_ship_survives_a_class_edit() {
        let (mut app, ship) = fitted_app(ShipClass {
            hull: 250.0,
            loadout: ShipLoadout::player(),
            ..ShipClass::default()
        });

        app.update(); // must not panic

        assert_eq!(
            app.world().get::<Hull>(ship).unwrap().max,
            250.0,
            "the edit must land on a ship carrying its whole kit"
        );
    }

    /// A device the class no longer describes is left alone rather than reset.
    ///
    /// What a hull carries is settled at spawn by its fit; the panel edits a
    /// class, and a class saying `None` is not an instruction to strip a flying
    /// ship of its emitter.
    #[test]
    fn a_slot_the_class_leaves_empty_does_not_disturb_a_fitted_device() {
        // A class with no disruptor authored, on a ship that carries one.
        let (mut app, ship) = fitted_app(ShipClass::default());
        app.world_mut().get_mut::<EmpWeapon>(ship).unwrap().range = 1234.0;

        app.update();

        assert_eq!(
            app.world().get::<EmpWeapon>(ship).unwrap().range,
            1234.0,
            "an unauthored slot must leave the fitted device untouched"
        );
    }

    /// Shields are merged now, so they need the same live/config care as guns:
    /// raising a bank's capacity must not hand back the charge a fight took off
    /// it.
    #[test]
    fn merging_a_shield_does_not_recharge_a_flattened_bank() {
        let (mut app, ship) = fitted_app(ShipClass {
            loadout: ShipLoadout {
                shield: Shield {
                    fore_max: 90.0,
                    ..ShipLoadout::player().shield
                },
                ..ShipLoadout::player()
            },
            ..ShipClass::default()
        });
        app.world_mut().get_mut::<Shield>(ship).unwrap().fore.charge = 0.0;

        app.update();

        let shield = app.world().get::<Shield>(ship).unwrap();
        assert_eq!(shield.fore_max, 90.0, "the authored change must land");
        assert_eq!(
            shield.fore.charge, 0.0,
            "a bank beaten flat must stay flat through a retune"
        );
    }

    /// A world holding one fully-fitted ship of class 0, and that class.
    fn fitted_app(class: ShipClass) -> (App, Entity) {
        use crate::data::ships::{NamedClass, ShipTable};
        use vt_sim::prelude::{spawn_ship_in, Faction};

        let mut app = App::new();
        app.insert_resource(ShipTable {
            classes: vec![NamedClass {
                name: "fitted".into(),
                class,
            }],
        })
        .add_systems(Update, apply_class_edits);

        let ship = spawn_ship_in(
            app.world_mut(),
            Faction::Corsairs,
            ShipStats::default(),
            100.0,
            Vec2::ZERO,
            0.0,
            ShipLoadout::player(),
        )
        .insert(ClassId(0))
        .id();

        (app, ship)
    }
}

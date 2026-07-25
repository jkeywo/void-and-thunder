//! Rendering an arbitrary struct as editable widgets, by reflection.
//!
//! One function handles every tuning struct in the game. It walks a value's
//! fields with `bevy_reflect`, renders each leaf as a slider paired with a text
//! box, and recurses into nested structs. Nothing here knows what a `Broadside`
//! is — which is exactly why adding a field to one needs no change here.
//!
//! Leaf types are the ones a designer actually tunes: `f32`, `u32`, `usize` and
//! `bool`. Anything else is shown read-only rather than skipped, so an
//! unsupported field is visible-but-inert instead of silently absent.

use bevy::reflect::{PartialReflect, ReflectMut, ReflectRef};
use bevy_egui::egui;

use super::fields::{owner_of, spec_for, FieldKind, FieldSpec};

/// Draw `value`'s fields as editable widgets. Returns true if anything changed.
///
/// `editable` is false for a live-state subtree, which greys the whole thing out
/// rather than letting a class edit stamp a reload timer onto a running ship.
pub fn edit_value(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut dyn PartialReflect,
    editable: bool,
) -> bool {
    // The top of a walk has no parent struct to key the descriptor table
    // against, so a leaf here falls back to the heuristic range. Everything the
    // panel shows at the top level is a struct, so in practice this never bites.
    edit_field(ui, name, value, editable, None)
}

/// One field. `spec` comes from the *parent* struct's entry in the descriptor
/// table, which is what lets `Broadside.damage` and `EmpDefense.damage` be told
/// apart — the first is authored, the second is EMP a ship has soaked.
fn edit_field(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut dyn PartialReflect,
    editable: bool,
    spec: Option<FieldSpec>,
) -> bool {
    // Leaves first: most fields are numbers, and checking them here keeps the
    // struct recursion below from having to know about primitives at all.
    if let Some(v) = value.try_downcast_mut::<f32>() {
        return edit_f32(ui, name, v, editable, spec);
    }
    if let Some(v) = value.try_downcast_mut::<u32>() {
        let mut f = *v as f32;
        if edit_f32(ui, name, &mut f, editable, spec) {
            *v = f.round().max(0.0) as u32;
            return true;
        }
        return false;
    }
    if let Some(v) = value.try_downcast_mut::<usize>() {
        let mut f = *v as f32;
        if edit_f32(ui, name, &mut f, editable, spec) {
            *v = f.round().max(0.0) as usize;
            return true;
        }
        return false;
    }
    if let Some(v) = value.try_downcast_mut::<bool>() {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(editable, |ui| {
                changed = ui.checkbox(v, name).changed();
            });
        });
        return changed;
    }

    // Not a leaf: recurse into a struct's fields under a collapsing header.
    let is_struct = matches!(value.reflect_ref(), ReflectRef::Struct(_));
    if !is_struct {
        ui.horizontal(|ui| {
            ui.label(name);
            ui.weak("(not editable)");
        });
        return false;
    }

    let mut changed = false;
    egui::CollapsingHeader::new(name)
        .default_open(true)
        .show(ui, |ui| {
            let owner = owner_of(&*value).to_string();
            let ReflectMut::Struct(s) = value.reflect_mut() else {
                return;
            };
            for i in 0..s.field_len() {
                // `name_at` and `field_at_mut` cannot be held at once (one
                // borrows immutably, the other mutably), so take the name first.
                let field_name = s.name_at(i).unwrap_or("?").to_string();
                let field_spec = spec_for(&owner, &field_name, 0.0);
                // A whole subtree marked Live is greyed out, not hidden: seeing a
                // reload timer tick is useful, writing it is not.
                let live = field_spec.kind == FieldKind::Live;
                let Some(field) = s.field_at_mut(i) else {
                    continue;
                };
                changed |= edit_field(ui, &field_name, field, editable && !live, Some(field_spec));
            }
        });
    changed
}

/// One number: a slider for feel, a drag box for precision. Both edit the same
/// value, so you can sweep to find the shape and then type the exact figure.
fn edit_f32(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut f32,
    editable: bool,
    spec: Option<FieldSpec>,
) -> bool {
    let spec = spec.unwrap_or_else(|| spec_for("", name, *value));
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(editable, |ui| {
            ui.label(name);
            // The slider is clamped to the spec's range; the drag box is not, so
            // a value outside the table's guess is still reachable by typing.
            changed |= ui
                .add(
                    egui::Slider::new(value, spec.min..=spec.max)
                        .show_value(false)
                        .clamping(egui::SliderClamping::Never),
                )
                .changed();
            changed |= ui
                .add(egui::DragValue::new(value).speed(drag_speed(spec.min, spec.max)))
                .changed();
        });
    });
    changed
}

/// Drag granularity scaled to the field's range, so a 0..1 fraction moves in
/// hundredths while a 0..3000 thrust moves in whole units.
fn drag_speed(min: f32, max: f32) -> f64 {
    ((max - min) as f64 / 300.0).max(0.001)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vt_sim::prelude::{Broadside, ShipStats};

    /// The walker's whole justification: it must find every field of a struct it
    /// has never heard of. If reflection stopped exposing these, the panel would
    /// silently show a shorter list rather than fail.
    #[test]
    fn reflection_exposes_every_tuning_field() {
        let mut stats = ShipStats::default();
        let ReflectMut::Struct(s) = stats.reflect_mut() else {
            panic!("ShipStats should reflect as a struct");
        };
        let names: Vec<&str> = (0..s.field_len()).filter_map(|i| s.name_at(i)).collect();
        assert_eq!(names, ["thrust", "turn_rate", "max_speed", "linear_drag"]);
    }

    /// Live subtrees must be identifiable from the field name alone, since that
    /// is all the walker has when it decides whether to grey one out.
    #[test]
    fn a_broadsides_bank_state_is_reachable_and_marked_live() {
        let mut bank = Broadside::default();
        let ReflectMut::Struct(s) = bank.reflect_mut() else {
            panic!("Broadside should reflect as a struct");
        };
        let names: Vec<String> = (0..s.field_len())
            .filter_map(|i| s.name_at(i))
            .map(str::to_string)
            .collect();
        assert!(names.contains(&"port".to_string()));
        assert!(names.contains(&"damage".to_string()));
        assert_eq!(spec_for("Broadside", "port", 0.0).kind, FieldKind::Live);
        assert_eq!(spec_for("Broadside", "damage", 0.0).kind, FieldKind::Config);
    }

    /// Numbers must round-trip through the walker's downcast, or edits would be
    /// silently dropped for integer fields.
    #[test]
    fn integer_fields_downcast() {
        let mut bank = Broadside::default();
        let ReflectMut::Struct(s) = bank.reflect_mut() else {
            panic!()
        };
        let idx = (0..s.field_len())
            .find(|i| s.name_at(*i) == Some("guns"))
            .expect("guns field");
        let field = s.field_at_mut(idx).unwrap();
        assert!(
            field.try_downcast_mut::<u32>().is_some(),
            "guns must be reachable as a u32 or the panel cannot edit it"
        );
    }
}

//! One menu look, imported everywhere: **frosted glass**.
//!
//! This module is the single definition of the menu surface — fill, edge, radius,
//! scrim, and the selected/hover states — so changing the look is one edit instead
//! of a sweep across every screen, and no two menus can disagree.
//!
//! The rules, in one place:
//!
//! - A menu is **translucent** — you can always see the world it is suspended over.
//!   That is what makes it read as glass rather than as a new screen.
//! - A **modal** menu sits on a [`scrim`], so it is legible over anything behind it
//!   and obviously dismissable. A **HUD** panel ([`hud`]) has no scrim: it lives
//!   alongside play rather than interrupting it.
//! - Nested boxes go one step *deeper*, never lighter, so depth reads as depth.
//! - Interactive rows use [`chip`], and its states come from here too.
//!
//! Usage is `commands.spawn(glass::panel(Val::Px(480.0)))` and
//! `commands.spawn(glass::scrim()).with_children(|c| { c.spawn(glass::panel(..)); })`.

use bevy::prelude::*;

/// The glass itself: a cold, dark blue at 88% — opaque enough to read small text
/// over a bright biome, translucent enough to see the world through.
pub const GLASS: Color = Color::srgba(0.055, 0.075, 0.17, 0.88);
/// One step deeper, for a box nested inside a panel (a hero cell, a shop row).
pub const GLASS_DEEP: Color = Color::srgba(0.03, 0.045, 0.11, 0.9);
/// A HUD panel: thinner glass, because it shares the screen with play rather than
/// interrupting it. This is the battle HUD's fill — Bevy UI has no true backdrop
/// blur, so a low alpha over a busy 3D scene is what carries the frosted effect.
pub const GLASS_THIN: Color = Color::srgba(0.06, 0.09, 0.17, 0.5);
/// The dim behind a modal menu.
pub const SCRIM: Color = Color::srgba(0.0, 0.0, 0.0, 0.5);

/// The gold edge every menu shares.
pub const EDGE: Color = Color::srgb(0.98, 0.86, 0.42);
/// A quieter edge for nested boxes and unfocused rows: a hairline of light, which
/// is what makes a translucent panel read as a pane of glass rather than a hole.
pub const EDGE_SOFT: Color = Color::srgba(0.78, 0.86, 1.0, 0.32);

/// Headings and anything the player is meant to read first.
pub const TITLE: Color = Color::srgb(0.98, 0.86, 0.42);
/// Body text.
pub const TEXT: Color = Color::srgb(0.9, 0.95, 1.0);
/// Secondary text: hints, footers, what-this-costs.
pub const DIM: Color = Color::srgb(0.72, 0.78, 0.9);
/// A gain, a heal, an unlock earned.
pub const GOOD: Color = Color::srgb(0.55, 0.98, 0.62);
/// A cost, a risk, a locked row.
pub const WARN: Color = Color::srgb(0.95, 0.72, 0.42);

/// The active/selected wash, and the bright edge that goes with it.
pub const ACTIVE: Color = Color::srgba(0.85, 0.68, 0.28, 0.5);
pub const ACTIVE_EDGE: Color = Color::srgba(1.0, 0.9, 0.5, 0.8);

/// Corner rounding and edge weight, shared so panels look like siblings.
const RADIUS: f32 = 8.0;
const BORDER: f32 = 2.0;

/// A full-screen dimmed container that centres its child. Spawn a [`panel`] into
/// it for a modal menu.
pub fn scrim() -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        },
        BackgroundColor(SCRIM),
    )
}

/// The standard menu panel: frosted, gold-edged, rounded, laid out as a column.
/// `width` is the only thing a caller normally varies.
pub fn panel(width: Val) -> impl Bundle {
    (
        Node {
            width,
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            padding: UiRect::all(Val::Px(18.0)),
            border: UiRect::all(Val::Px(BORDER)),
            ..default()
        },
        BackgroundColor(GLASS),
        BorderColor(EDGE),
        BorderRadius::all(Val::Px(RADIUS)),
    )
}

/// The same panel laid out as a **row**, for a banner with an image beside its
/// text. Same fill, edge and radius — only the axis differs.
pub fn panel_row(width: Val) -> impl Bundle {
    (
        Node {
            width,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(16.0),
            padding: UiRect::all(Val::Px(24.0)),
            border: UiRect::all(Val::Px(BORDER)),
            ..default()
        },
        BackgroundColor(GLASS),
        BorderColor(EDGE),
        BorderRadius::all(Val::Px(RADIUS)),
    )
}

/// A HUD panel: same family, no scrim, thinner glass and a quiet edge — it belongs
/// beside play, not on top of it.
pub fn hud(width: Val) -> impl Bundle {
    (
        Node {
            width,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            row_gap: Val::Px(3.0),
            padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(GLASS_THIN),
        BorderColor(EDGE_SOFT),
        BorderRadius::all(Val::Px(6.0)),
    )
}

/// A box nested inside a panel — a hero cell, a shop row, a stat block.
pub fn inset() -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            padding: UiRect::all(Val::Px(6.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(GLASS_DEEP),
        BorderColor(EDGE_SOFT),
        BorderRadius::all(Val::Px(5.0)),
    )
}

/// A selectable pill: a tab, a menu row, a toggle. `on` is the selected state.
pub fn chip(on: bool) -> impl Bundle {
    (
        Node {
            padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(if on { CHIP_ON } else { CHIP_OFF }),
        BorderColor(if on { EDGE } else { EDGE_SOFT }),
        BorderRadius::all(Val::Px(5.0)),
    )
}

/// Chip fills. Public because interaction systems repaint them in place (mutating
/// `BackgroundColor` on hover) rather than respawning the chip.
///
/// Selection is a translucent **gold wash** rather than a blue one, so a selected
/// tab, a selected menu row and the active hero all say "selected" the same way.
pub const CHIP_ON: Color = ACTIVE;
pub const CHIP_OFF: Color = GLASS_THIN;
/// Hover fill for any interactive row, so hover means one thing everywhere.
pub const CHIP_HOVER: Color = Color::srgba(0.2, 0.26, 0.46, 0.8);
/// Hover fill for a row whose action is destructive (unequip, discard).
pub const CHIP_HOVER_WARN: Color = Color::srgba(0.4, 0.2, 0.22, 0.75);

/// A horizontal rule between sections of a panel.
pub fn divider() -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(1.0),
            margin: UiRect::vertical(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(EDGE_SOFT),
    )
}

/// A text line, so font size and colour come from the same place as the panel.
pub fn text(content: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(content.into()),
        TextFont { font_size: size, ..default() },
        TextColor(color),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The look has rules, and a rule nobody checks is a rule that drifts. These
    /// are the ones that make the family read as a family.
    #[test]
    fn every_menu_surface_is_glass_not_a_wall() {
        for (name, c) in [("GLASS", GLASS), ("GLASS_DEEP", GLASS_DEEP), ("GLASS_THIN", GLASS_THIN)] {
            let a = c.alpha();
            assert!(a < 1.0, "{name} is opaque — you cannot see the world through it");
            assert!(a >= 0.5, "{name} is too thin to read small text over a bright biome");
        }
        // Nested goes DEEPER, never lighter: depth has to read as depth.
        assert!(
            GLASS_DEEP.to_linear().red < GLASS.to_linear().red,
            "an inset box is lighter than the panel holding it"
        );
        // A HUD panel is thinner than a modal one, because it shares the screen.
        assert!(GLASS_THIN.alpha() < GLASS.alpha());
        // The scrim dims without blacking out.
        assert!(SCRIM.alpha() > 0.2 && SCRIM.alpha() < 0.7);
    }

    #[test]
    fn selected_reads_stronger_than_unselected() {
        // Selection is a gold wash, so it must be warmer than the cold glass it sits
        // on — alpha alone would not distinguish it.
        let (on, off) = (CHIP_ON.to_linear(), CHIP_OFF.to_linear());
        assert!(on.red > off.red && on.blue < on.red, "the selected chip is not a gold wash");
        assert!(CHIP_HOVER.alpha() > CHIP_OFF.alpha());
        assert_eq!(CHIP_ON, ACTIVE, "two definitions of 'selected'");
    }
}

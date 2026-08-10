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
            // Never taller than the window. Measured in VIEWPORT height, not percent:
            // a percentage resolves against the parent, and the parent here is an
            // auto-height row whose own height is this panel — which resolves to
            // something arbitrary and clips the last row off.
            max_height: Val::Vh(92.0),
            overflow: Overflow::scroll_y(),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            padding: UiRect::all(Val::Px(22.0)),
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

/// A box nested inside a panel — a hero cell, a shop row, a stat block. `focused`
/// brightens its edge; it is a parameter rather than a component the caller adds on
/// top, because a second `BorderColor` in the same bundle is a runtime panic.
pub fn inset(focused: bool) -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(3.0),
            padding: UiRect::all(Val::Px(9.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(GLASS_DEEP),
        BorderColor(if focused { EDGE } else { EDGE_SOFT }),
        BorderRadius::all(Val::Px(5.0)),
    )
}

/// A selectable pill: a tab, a menu row, a toggle. `on` is the selected state.
pub fn chip(on: bool) -> impl Bundle {
    chip_sized(on, Val::Auto)
}

/// A chip with a floor on its width, for a label that must not wrap onto two lines
/// (`Row: Front` in a hero cell, where a second line costs real vertical space).
pub fn chip_sized(on: bool, min_width: Val) -> impl Bundle {
    (
        Node {
            min_width,
            justify_content: JustifyContent::Center,
            padding: UiRect::axes(Val::Px(12.0), Val::Px(7.0)),
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

// ---------------------------------------------------------------- columns --
//
// THE THREE-COLUMN CONVENTION. Every cascade screen (the menu, equipment, the town
// counters) is nav | main | detail, at fixed FRACTIONS of the window: 1/6, 1/2, 1/3, which
// sum to exactly 1.
//
// Fractions, not content-sizing, because content-sized columns move. The menu's row had no
// width at all and was centred by the scrim, so every time a third column appeared the whole
// thing shifted sideways under the cursor — clicking a nav item moved the nav item you just
// clicked. A column that is 1/6 of the window is 1/6 of the window whether or not its
// neighbours have anything in them.
//
// The detail column's SLOT is always spawned, empty if there is nothing to say, for the same
// reason: a panel that appears must not push its siblings.

/// Nav: the section list. Small, and never the place content grows.
pub const COL_NAV: f32 = 100.0 / 6.0;
/// Main: the column the screen is actually about. Always the biggest, and it absorbs any
/// slack left by the other two's minimum widths.
pub const COL_MAIN: f32 = 50.0;
/// Detail: the fine print for whatever is focused in `main`.
pub const COL_DETAIL: f32 = 100.0 / 3.0;

// The three columns must tile the window exactly. Checked at COMPILE time: a split that
// does not add up leaves a gap the row re-centres into, which is the drift this whole
// convention exists to stop — so it should fail to build, not fail a test.
const _: () = assert!(COL_NAV + COL_MAIN + COL_DETAIL == 100.0);
const _: () = assert!(COL_MAIN > COL_DETAIL);
const _: () = assert!(COL_DETAIL > COL_NAV);

/// Floors, so a narrow window degrades instead of crushing. At 800px wide the detail column
/// is only ~18 monospace characters, which wraps the longest ability description to nine
/// lines — legible, but the floors keep the nav readable and let `main` give up width first.
const COL_NAV_MIN: f32 = 172.0;
const COL_DETAIL_MIN: f32 = 240.0;

/// The full-width row the three columns live in. Full width is the point: the row cannot
/// resize with its contents, so nothing inside it can shift sideways.
pub fn columns() -> impl Bundle {
    Node {
        // VIEWPORT width, not percent: as a flex item of the scrim a percentage width still
        // got shrunk to fit its own content, so the columns came out at about half the share
        // they claim. `Vw` is measured against the window and cannot be negotiated away.
        width: Val::Vw(100.0),
        flex_shrink: 0.0,
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        justify_content: JustifyContent::Center,
        column_gap: Val::Px(14.0),
        padding: UiRect::axes(Val::Px(18.0), Val::Px(0.0)),
        ..default()
    }
}

/// One column of the convention, as a frosted panel. `frac` is one of the `COL_*` constants.
pub fn column(frac: f32) -> impl Bundle {
    // Nothing shrinks. A column that can shrink does, the moment its neighbour has more
    // content than it — which is the drift this convention exists to stop. Main is the only
    // one that GROWS, so it absorbs whatever the floors leave over.
    let (min, grow) = if frac == COL_MAIN {
        (Val::Px(0.0), 1.0)
    } else if frac == COL_NAV {
        (Val::Px(COL_NAV_MIN), 0.0)
    } else {
        (Val::Px(COL_DETAIL_MIN), 0.0)
    };
    let shrink = 0.0;
    (
        Node {
            width: Val::Percent(frac),
            min_width: min,
            flex_grow: grow,
            flex_shrink: shrink,
            max_height: Val::Vh(92.0),
            overflow: Overflow::scroll_y(),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            padding: UiRect::all(Val::Px(18.0)),
            border: UiRect::all(Val::Px(BORDER)),
            ..default()
        },
        BackgroundColor(GLASS),
        BorderColor(EDGE),
        BorderRadius::all(Val::Px(RADIUS)),
    )
}

/// A column slot that holds nothing — same width as `column`, no frosting. Keeps the
/// geometry when a detail panel has nothing to show, so its neighbours never move.
pub fn column_empty(frac: f32) -> impl Bundle {
    Node {
        width: Val::Percent(frac),
        min_width: if frac == COL_DETAIL { Val::Px(COL_DETAIL_MIN) } else { Val::Px(0.0) },
        flex_shrink: 0.0,
        ..default()
    }
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

    /// And the floors have to fit inside the fractions they belong to at the smallest window
    /// we care about, or a column stops being the size it claims.
    #[test]
    fn the_column_floors_leave_main_something_to_work_with() {
        // 1280 logical px is the reference window.
        let w = 1280.0;
        let nav = (w * COL_NAV / 100.0).max(COL_NAV_MIN);
        let detail = (w * COL_DETAIL / 100.0).max(COL_DETAIL_MIN);
        let main = w - nav - detail;
        assert!(main > detail, "main should still be the widest at 1280: {main} vs {detail}");
        assert!(nav >= COL_NAV_MIN && detail >= COL_DETAIL_MIN);

        // And at a cramped 800 the floors bite without eating main entirely.
        let w = 800.0;
        let nav = (w * COL_NAV / 100.0).max(COL_NAV_MIN);
        let detail = (w * COL_DETAIL / 100.0).max(COL_DETAIL_MIN);
        assert!(w - nav - detail > 200.0, "main must survive a narrow window");
    }
}

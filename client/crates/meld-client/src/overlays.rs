//! Overworld overlays: the tabbed inventory/equip/status screen, gear tooltip,
//! loot report, and the level-up (stat-gain) screen.
//! Extracted from `main.rs` during the module reorg.


use bevy::prelude::*;

use meld_client::net::{ClientCmd, GearLine};
use meld_proto::enums::Insurance;

use super::*;

// -------------------------------------------------- overworld overlays -----

/// Open/close the inventory (I) and level-up (L) screens; fetch fresh data on
/// open. ESC closes whichever is up.
#[allow(clippy::too_many_arguments)]
pub(crate) fn overlay_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    mut overlay: ResMut<Overlay>,
    mut tab: ResMut<OverlayTab>,
    mut inv: ResMut<InventoryData>,
    mut picker: ResMut<EquipPicker>,
    mut cursor: ResMut<OverlayCursor>,
    roster: Res<PartyRoster>,
    mut rename: ResMut<HeroRename>,
    state: Res<State<Screen>>,
) {
    // While renaming a hero on the party screen, capture text and swallow the
    // other overlay hotkeys (so typing a name doesn't toggle screens).
    if let Some(slot) = rename.slot {
        if keys.just_pressed(KeyCode::Escape) {
            rename.slot = None;
            rename.buffer.clear();
            return;
        }
        if keys.just_pressed(KeyCode::Enter) {
            let name = rename.buffer.trim().to_string();
            if !name.is_empty() {
                net.0.send(ClientCmd::RenameHero { slot: slot as i32, name });
            }
            rename.slot = None;
            rename.buffer.clear();
            return;
        }
        if keys.just_pressed(KeyCode::Backspace) {
            rename.buffer.pop();
            return;
        }
        if keys.just_pressed(KeyCode::Space) && rename.buffer.len() < 24 {
            rename.buffer.push(' ');
        }
        for key in keys.get_just_pressed() {
            if let Some(c) = key_to_code_char(*key) {
                if rename.buffer.len() < 24 {
                    rename.buffer.push(c);
                }
            }
        }
        return;
    }

    // C or I toggles the inventory overlay (also openable by tapping your
    // character — see `overworld_click_menu`). Fresh opens land on Items.
    // Overworld-only: in the City, `C` is already co-op ( `city_input`) and the
    // storage chest opens the same overlay via its own `V`/`E` interact instead.
    if *state.get() == Screen::Overworld
        && (keys.just_pressed(KeyCode::KeyC) || keys.just_pressed(KeyCode::KeyI))
    {
        if overlay.kind == Some(OverlayKind::Inventory) {
            overlay.kind = None;
        } else {
            overlay.kind = Some(OverlayKind::Inventory);
            *tab = OverlayTab::Items;
            inv.loaded = false;
            net.0.fetch_inventory();
        }
        picker.category = None;
    }
    // ESC backs out of the Equip picker screen first (if it's open), only
    // closing the whole menu once that's already closed.
    if keys.just_pressed(KeyCode::Escape) {
        if picker.category.is_some() {
            picker.category = None;
            cursor.index = 0;
        } else {
            overlay.kind = None;
        }
    }
    // On the Status tab, digits 1-4 start renaming that hero slot.
    if overlay.kind == Some(OverlayKind::Inventory) && *tab == OverlayTab::Status {
        let slots = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4];
        for (i, k) in slots.iter().enumerate() {
            if keys.just_pressed(*k) {
                if let Some(h) = roster.heroes.get(i) {
                    rename.slot = Some(i);
                    rename.buffer = h.name.clone();
                }
            }
        }
    }
}

/// Arrow-key navigation + Space-to-activate for the inventory overlay.
/// Up/Down move the cursor through the active screen's navigable rows (down
/// is down, up is up); Left/Right switch tabs (and reset the cursor). Space
/// activates whatever the cursor is on — mirrors what a click on that same
/// row would do. The Equip tab has two screens (see `EquipPicker`): the main
/// per-hero summary, and — once a category is activated — a picker of every
/// candidate for it, so nothing ever equips just by being found or browsed
/// past.
pub(crate) fn overlay_nav_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    overlay: Res<Overlay>,
    mut tab: ResMut<OverlayTab>,
    mut cursor: ResMut<OverlayCursor>,
    mut equip_sel: ResMut<EquipSelection>,
    mut picker: ResMut<EquipPicker>,
    mut rename: ResMut<HeroRename>,
    inv: Res<InventoryData>,
    run_gear: Res<RunGearData>,
    roster: Res<PartyRoster>,
    hero_names: Res<AccountHeroNames>,
) {
    if overlay.kind != Some(OverlayKind::Inventory) || rename.slot.is_some() {
        return;
    }
    // Left/Right cycles tabs. Blocked while the Equip picker is open —
    // switching tabs mid-pick would be disorienting; Escape (handled in
    // `overlay_input`) backs out of the picker instead.
    if picker.category.is_none()
        && (keys.just_pressed(KeyCode::ArrowLeft) || keys.just_pressed(KeyCode::ArrowRight))
    {
        let order = [OverlayTab::Items, OverlayTab::Equip, OverlayTab::Status];
        let i = order.iter().position(|t| *t == *tab).unwrap_or(0);
        let next = if keys.just_pressed(KeyCode::ArrowRight) {
            (i + 1) % order.len()
        } else {
            (i + order.len() - 1) % order.len()
        };
        *tab = order[next];
        cursor.index = 0;
        return;
    }
    let row_count = match *tab {
        OverlayTab::Items => 0,
        OverlayTab::Equip => match picker.category {
            Some(cat) => equip_picker_rows(
                &inv,
                &run_gear,
                cat,
                equip_sel.hero_slot,
                hero_class_at(&roster, &hero_names, equip_sel.hero_slot),
            )
            .len(),
            None => equip_main_rows(hero_count(&roster, &hero_names)).len(),
        },
        OverlayTab::Status => hero_count(&roster, &hero_names),
    };
    // Up/Down moves the row cursor — down is down, up is up.
    if row_count > 0 {
        if keys.just_pressed(KeyCode::ArrowUp) {
            cursor.index = (cursor.index + row_count - 1) % row_count;
        } else if keys.just_pressed(KeyCode::ArrowDown) {
            cursor.index = (cursor.index + 1) % row_count;
        }
    }
    if keys.just_pressed(KeyCode::Space) {
        match *tab {
            OverlayTab::Equip => {
                if let Some(cat) = picker.category {
                    let rows = equip_picker_rows(
                        &inv,
                        &run_gear,
                        cat,
                        equip_sel.hero_slot,
                        hero_class_at(&roster, &hero_names, equip_sel.hero_slot),
                    );
                    match rows.get(cursor.index) {
                        Some(PickerRow::Unequip) => {
                            unequip_category(&net.0, &inv, &run_gear, cat, equip_sel.hero_slot);
                            picker.category = None;
                            cursor.index = 0;
                        }
                        Some(PickerRow::Gear { gear_id, source }) => {
                            let target = Some(equip_sel.hero_slot);
                            // Same two rules as the mouse path (GR-5): a row this
                            // hero's class can't wear does nothing, and a
                            // two-hander frees the off-hand on its way in.
                            let line = inv
                                .gear
                                .iter()
                                .chain(run_gear.gear.iter())
                                .find(|g| &g.gear_id == gear_id);
                            let hero_class =
                                hero_class_at(&roster, &hero_names, equip_sel.hero_slot);
                            if line
                                .map(|l| gear_block_reason(l, hero_class).is_some())
                                .unwrap_or(false)
                            {
                                return;
                            }
                            let free_first = line
                                .and_then(|l| off_hand_in_the_way(&inv.gear, l, equip_sel.hero_slot));
                            match source {
                                GearSource::Vault => match free_first {
                                    Some(off) => net.0.equip_gear_freeing(
                                        off,
                                        gear_id.clone(),
                                        equip_sel.hero_slot,
                                    ),
                                    None => net.0.equip_gear(gear_id.clone(), target),
                                },
                                GearSource::RunLoot => net.0.send(ClientCmd::EquipLoot {
                                    gear_id: gear_id.clone(),
                                    hero_slot: target.map(|s| s as i32),
                                }),
                            }
                            picker.category = None;
                            cursor.index = 0;
                        }
                        None => {}
                    }
                } else {
                    let rows = equip_main_rows(hero_count(&roster, &hero_names));
                    match rows.get(cursor.index) {
                        Some(EquipRow::Hero(i)) => equip_sel.hero_slot = *i,
                        Some(EquipRow::Category(cat)) => {
                            picker.category = Some(cat);
                            cursor.index = 0;
                        }
                        None => {}
                    }
                }
            }
            OverlayTab::Status => {
                if let Some(name) = hero_name_at(&roster, &hero_names, cursor.index) {
                    rename.slot = Some(cursor.index);
                    rename.buffer = name;
                }
            }
            OverlayTab::Items => {}
        }
    }
}

/// Immediate-mode: draw the open overlay (inventory or level-up) as a centered
/// window over a dimmed overworld. The inventory panel has three vertical
/// tabs — Items (materials), Equip (per-hero gear loadout), Status (per-hero
/// stats) — switched via `OverlayTab`.
pub(crate) fn render_overlay(
    mut commands: Commands,
    overlay: Res<Overlay>,
    tab: Res<OverlayTab>,
    equip_sel: Res<EquipSelection>,
    picker: Res<EquipPicker>,
    cursor: Res<OverlayCursor>,
    inv: Res<InventoryData>,
    run_gear: Res<RunGearData>,
    prog: Res<ProgressData>,
    roster: Res<PartyRoster>,
    rename: Res<HeroRename>,
    hero_names: Res<AccountHeroNames>,
    stats: Res<RunStats>,
    backpack: Res<RunBackpack>,
    wa: Option<Res<WorldAssets>>,
    existing: Query<Entity, With<OverlayRoot>>,
) {
    // Rebuild only when something the overlay shows changed. The gear rows are
    // real buttons; they must persist across frames for click detection, so we
    // must NOT despawn+respawn them every frame.
    if !(overlay.is_changed()
        || tab.is_changed()
        || equip_sel.is_changed()
        || picker.is_changed()
        || cursor.is_changed()
        || inv.is_changed()
        || run_gear.is_changed()
        || prog.is_changed()
        || roster.is_changed()
        || rename.is_changed()
        || hero_names.is_changed()
        || stats.is_changed()
        || backpack.is_changed())
    {
        return;
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    let Some(kind) = overlay.kind else {
        return;
    };
    let label = |p: &mut ChildSpawnerCommands, text: String, size: f32, color: Color| {
        p.spawn((
            Text::new(text),
            TextFont { font_size: size, ..default() },
            TextColor(color),
        ));
    };
    let gold = Color::srgb(0.95, 0.85, 0.5);
    let dim = Color::srgb(0.72, 0.78, 0.9);
    let green = Color::srgb(0.5, 0.9, 0.55);
    let red = Color::srgb(0.9, 0.45, 0.45);
    // Keyboard cursor highlight: a bright border on whatever row Up/Down is on.
    let focus_border = Color::srgb(1.0, 0.9, 0.5);
    let no_border = Color::NONE;
    // A small item icon: the material's harvest-node sprite reused from the world,
    // so the vault/backpack lists read like DQ3's item panel. Falls back to a
    // nerdfont glyph when a material has no node art (e.g. consumables).
    let mat_icon = |row: &mut ChildSpawnerCommands, kind: &str| {
        if let Some(tex) = wa
            .as_ref()
            .and_then(|w| w.prop_sprites.get(&format!("resource_{kind}")))
        {
            row.spawn((
                ImageNode::new(tex.clone()),
                Node {
                    width: Val::Px(28.0),
                    height: Val::Px(28.0),
                    ..default()
                },
            ));
        } else {
            let glyph = match kind {
                "town_portal" => "\u{f0f10}", // portal
                "salve" | "elixir" => "\u{f0f04}", // flask
                "chits" => "\u{f0114}",       // coin
                _ => "\u{f0a7d}",              // generic cube
            };
            row.spawn(Node {
                width: Val::Px(28.0),
                justify_content: JustifyContent::Center,
                ..default()
            })
            .with_children(|b| {
                b.spawn((
                    Text::new(glyph),
                    TextFont { font_size: 18.0, ..default() },
                    TextColor(dim),
                ));
            });
        }
    };
    commands
        .spawn((
            OverlayRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Px(660.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(10.0),
                    padding: UiRect::all(Val::Px(18.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    ..default()
                },
                BorderColor(Color::srgb(0.45, 0.55, 0.85)),
                BackgroundColor(Color::srgb(0.055, 0.075, 0.17)),
                BorderRadius::all(Val::Px(8.0)),
            ))
            .with_children(|p| match kind {
                OverlayKind::Inventory => {
                    label(p, "INVENTORY".into(), 24.0, gold);
                    p.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(16.0),
                        ..default()
                    })
                    .with_children(|row| {
                        // --- vertical tab strip ---
                        row.spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(9.0),
                            width: Val::Px(120.0),
                            flex_shrink: 0.0,
                            ..default()
                        })
                        .with_children(|tabs| {
                            for (t, text) in [
                                (OverlayTab::Items, "Items"),
                                (OverlayTab::Equip, "Equip"),
                                (OverlayTab::Status, "Status"),
                            ] {
                                let active = *tab == t;
                                tabs.spawn((
                                    Button,
                                    TabButton(t),
                                    Node {
                                        width: Val::Percent(100.0),
                                        padding: UiRect::all(Val::Px(8.0)),
                                        ..default()
                                    },
                                    BackgroundColor(if active {
                                        Color::srgba(0.18, 0.28, 0.5, 0.85)
                                    } else {
                                        Color::NONE
                                    }),
                                    BorderRadius::all(Val::Px(4.0)),
                                ))
                                .with_children(|b| {
                                    b.spawn((
                                        Text::new(text),
                                        TextFont { font_size: 16.0, ..default() },
                                        TextColor(if active { gold } else { dim }),
                                    ));
                                });
                            }
                        });
                        // --- tab content ---
                        row.spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(9.0),
                            width: Val::Px(480.0),
                            ..default()
                        })
                        .with_children(|content| match *tab {
                            OverlayTab::Items => {
                                if !inv.loaded {
                                    label(content, "loading...".into(), 16.0, dim);
                                    return;
                                }
                                label(
                                    content,
                                    format!("Chits: {}", inv.chits),
                                    18.0,
                                    Color::srgb(0.95, 0.85, 0.5),
                                );
                                label(
                                    content,
                                    "- Materials -  (Withdraw stages 1 for your next run)".into(),
                                    15.0,
                                    gold,
                                );
                                if inv.materials.is_empty() {
                                    label(content, "  (none)".into(), 14.0, dim);
                                }
                                for (kind, qty) in &inv.materials {
                                    content
                                        .spawn(Node {
                                            flex_direction: FlexDirection::Row,
                                            align_items: AlignItems::Center,
                                            column_gap: Val::Px(8.0),
                                            ..default()
                                        })
                                        .with_children(|row| {
                                            mat_icon(row, kind);
                                            row.spawn((
                                                Text::new(format!("{} x{}", kind.replace('_', " "), qty)),
                                                TextFont { font_size: 15.0, ..default() },
                                                TextColor(dim),
                                            ));
                                            row.spawn((
                                                Button,
                                                WithdrawButton { item_kind: kind.clone() },
                                                Node {
                                                    padding: UiRect::axes(Val::Px(6.0), Val::Px(1.0)),
                                                    ..default()
                                                },
                                                BackgroundColor(Color::srgba(0.18, 0.28, 0.5, 0.6)),
                                                BorderRadius::all(Val::Px(4.0)),
                                            ))
                                            .with_children(|b| {
                                                b.spawn((
                                                    Text::new("Withdraw"),
                                                    TextFont { font_size: 12.0, ..default() },
                                                    TextColor(gold),
                                                ));
                                            });
                                        });
                                }
                                if !inv.pending.is_empty() {
                                    label(content, "- Queued for next run -".into(), 15.0, gold);
                                    for (kind, qty) in &inv.pending {
                                        content
                                            .spawn(Node {
                                                flex_direction: FlexDirection::Row,
                                                align_items: AlignItems::Center,
                                                column_gap: Val::Px(8.0),
                                                ..default()
                                            })
                                            .with_children(|row| {
                                                mat_icon(row, kind);
                                                row.spawn((
                                                    Text::new(format!("{} x{}", kind.replace('_', " "), qty)),
                                                    TextFont { font_size: 14.0, ..default() },
                                                    TextColor(Color::srgb(0.6, 0.95, 0.7)),
                                                ));
                                            });
                                    }
                                }
                            }
                            OverlayTab::Status => {
                                // --- Run readouts (moved off the always-on HUD) ---
                                // Distance / tier / biome and the run backpack now live
                                // here in the menu, so the overworld view stays clean.
                                // Only meaningful during a live dive (distance > 0 or a
                                // non-empty backpack); in the City it's just zeroes.
                                if !roster.heroes.is_empty() {
                                    label(
                                        content,
                                        format!(
                                            "Distance {}   |   Tier {}   |   {}",
                                            stats.distance, stats.tier, stats.biome
                                        ),
                                        17.0,
                                        gold,
                                    );
                                    let tp = backpack.count("town_portal");
                                    let mut bag = format!("Town Portals: {tp}   |   Chits: {}", backpack.chits);
                                    if !backpack.gear.is_empty() {
                                        bag.push_str(&format!("   |   Loot x{}", backpack.gear.len()));
                                    }
                                    label(content, bag, 14.0, dim);
                                    // Backpack materials as icon rows (matches the Items tab),
                                    // so harvested loot reads with its node sprite at a glance.
                                    let mats: Vec<(&String, &i32)> = backpack
                                        .items
                                        .iter()
                                        .filter(|(k, _)| k != "town_portal")
                                        .map(|(k, q)| (k, q))
                                        .collect();
                                    label(content, "Materials:".into(), 14.0, dim);
                                    if mats.is_empty() {
                                        label(content, "  (none)".into(), 13.0, dim);
                                    }
                                    for (kind, qty) in mats {
                                        content
                                            .spawn(Node {
                                                flex_direction: FlexDirection::Row,
                                                align_items: AlignItems::Center,
                                                column_gap: Val::Px(8.0),
                                                ..default()
                                            })
                                            .with_children(|row| {
                                                mat_icon(row, kind);
                                                row.spawn((
                                                    Text::new(format!("{} x{}", nice_name(kind), qty)),
                                                    TextFont { font_size: 14.0, ..default() },
                                                    TextColor(dim),
                                                ));
                                            });
                                    }
                                    // Return-home actions, also reachable from the overworld
                                    // bar / T,E keys — tagged TouchActionButton so the existing
                                    // `touch_action_buttons` handler drives them. Town Portal
                                    // works anywhere (needs one); Deep Portal only at the portal.
                                    content
                                        .spawn(Node {
                                            flex_direction: FlexDirection::Row,
                                            column_gap: Val::Px(10.0),
                                            margin: UiRect::vertical(Val::Px(5.0)),
                                            ..default()
                                        })
                                        .with_children(|row| {
                                            let mut act_btn = |row: &mut ChildSpawnerCommands, act: OverworldAct, text: String, on: bool| {
                                                row.spawn((
                                                    Button,
                                                    TouchActionButton(act),
                                                    Node {
                                                        padding: UiRect::axes(Val::Px(12.0), Val::Px(6.0)),
                                                        border: UiRect::all(Val::Px(1.5)),
                                                        ..default()
                                                    },
                                                    BorderColor(if on { Color::srgb(0.5, 0.6, 0.85) } else { Color::srgb(0.34, 0.37, 0.48) }),
                                                    BorderRadius::all(Val::Px(6.0)),
                                                    BackgroundColor(Color::srgba(0.12, 0.16, 0.3, if on { 0.9 } else { 0.4 })),
                                                ))
                                                .with_children(|b| {
                                                    b.spawn((
                                                        Text::new(text),
                                                        TextFont { font_size: 14.0, ..default() },
                                                        TextColor(if on { gold } else { dim }),
                                                    ));
                                                });
                                            };
                                            // Join an ally's in-progress fight (no-ops
                                            // unless you're near one — the handler guards
                                            // it). Kept here so the overworld shows only
                                            // the Menu button (#6).
                                            act_btn(row, OverworldAct::Join, "\u{f0fd2} Join Fight".into(), true);
                                            act_btn(row, OverworldAct::TownPortal, format!("\u{f0f10} Town Portal ({tp})"), tp > 0);
                                            act_btn(row, OverworldAct::Extract, "\u{f0817} Deep Portal".into(), true);
                                        });
                                    label(content, "- Heroes -".into(), 15.0, gold);
                                }
                                // Every hero's name, class, level and stats — this is
                                // where attributes live (not the battle HUD). Outside
                                // an active run (e.g. opened from the City's storage
                                // chest) `roster.heroes` is empty, so fall back to
                                // just the account's hero names — class/level/stats/
                                // formation aren't known without a live run.
                                if roster.heroes.is_empty() {
                                    if hero_names.names.is_empty() {
                                        label(content, "(enter a run to see your heroes)".into(), 13.0, dim);
                                    } else {
                                        for (i, name) in hero_names.names.iter().enumerate() {
                                            let focused = cursor.index == i;
                                            content
                                                .spawn((
                                                    Node {
                                                        flex_direction: FlexDirection::Column,
                                                        border: UiRect::all(Val::Px(2.0)),
                                                        padding: UiRect::all(Val::Px(2.0)),
                                                        ..default()
                                                    },
                                                    BorderColor(if focused { focus_border } else { no_border }),
                                                    BorderRadius::all(Val::Px(4.0)),
                                                ))
                                                .with_children(|hero_box| {
                                                    label(
                                                        hero_box,
                                                        format!("[{}] {}", i + 1, name),
                                                        16.0,
                                                        Color::srgb(0.85, 0.92, 1.0),
                                                    );
                                                });
                                        }
                                        label(
                                            content,
                                            "(enter a run to see class/level/stats)".into(),
                                            12.0,
                                            dim,
                                        );
                                    }
                                }
                                for (i, h) in roster.heroes.iter().enumerate() {
                                    let focused = cursor.index == i;
                                    let name_line = if rename.slot == Some(i) {
                                        format!("[{}] {}_   (typing...)", i + 1, rename.buffer)
                                    } else {
                                        format!(
                                            "[{}] {}   {} | Lv {}",
                                            i + 1,
                                            h.name,
                                            class_display(&h.class_key),
                                            h.level
                                        )
                                    };
                                    let name_col = if rename.slot == Some(i) {
                                        Color::srgb(0.98, 0.85, 0.4)
                                    } else {
                                        Color::srgb(0.85, 0.92, 1.0)
                                    };
                                    // The hero's front-facing (south / idle[0]) class
                                    // sprite as a portrait, so the status screen shows
                                    // WHO each hero is, not just their stats (#7).
                                    let portrait =
                                        wa.as_ref().map(|w| w.class_frames(&h.class_key).idle[0].clone());
                                    content
                                        .spawn((
                                            Node {
                                                flex_direction: FlexDirection::Row,
                                                align_items: AlignItems::Center,
                                                column_gap: Val::Px(8.0),
                                                border: UiRect::all(Val::Px(2.0)),
                                                padding: UiRect::all(Val::Px(2.0)),
                                                ..default()
                                            },
                                            BorderColor(if focused { focus_border } else { no_border }),
                                            BorderRadius::all(Val::Px(4.0)),
                                        ))
                                        .with_children(|cell| {
                                            if let Some(tex) = portrait {
                                                cell.spawn((
                                                    ImageNode::new(tex),
                                                    Node {
                                                        width: Val::Px(44.0),
                                                        height: Val::Px(60.0),
                                                        ..default()
                                                    },
                                                ));
                                            }
                                            cell.spawn(Node {
                                                flex_direction: FlexDirection::Column,
                                                flex_grow: 1.0,
                                                ..default()
                                            })
                                        .with_children(|hero_box| {
                                            label(hero_box, name_line, 16.0, name_col);
                                            label(
                                                hero_box,
                                                format!(
                                                    "   HP {}/{}   STR {}  MND {}  DEX {}  WLL {}",
                                                    h.hp, h.max_hp, h.str_, h.mnd, h.dex, h.wll
                                                ),
                                                13.0,
                                                dim,
                                            );
                                            let to_next = (h.xp_to_next - h.xp).max(0);
                                            label(
                                                hero_box,
                                                format!(
                                                    "   EXP {} / {}   ({} to next level)",
                                                    h.xp, h.xp_to_next, to_next
                                                ),
                                                13.0,
                                                dim,
                                            );
                                            // Clickable front/back-row toggle (handled by `formation_click`).
                                            let (row_text, row_col) = if h.back_row {
                                                ("   [ Back row ]  (half dmg, targeted less) - click to move up", Color::srgb(0.7, 0.8, 1.0))
                                            } else {
                                                ("   [ Front row ]  - click to move to the back", Color::srgb(0.95, 0.8, 0.55))
                                            };
                                            hero_box
                                                .spawn((
                                                    Button,
                                                    FormationButton { slot: i as i32, back_row: h.back_row },
                                                    Node {
                                                        padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                                                        ..default()
                                                    },
                                                    BackgroundColor(Color::NONE),
                                                    BorderRadius::all(Val::Px(4.0)),
                                                ))
                                                .with_children(|b| {
                                                    b.spawn((
                                                        Text::new(row_text),
                                                        TextFont { font_size: 13.0, ..default() },
                                                        TextColor(row_col),
                                                    ));
                                                });
                                            });
                                        });
                                }
                                // AD-2: what this comp ENABLES — the build feedback
                                // loop. Without this the synergies and combos are
                                // invisible mechanics nobody plans around.
                                if !roster.synergies.is_empty() {
                                    label(content, "Party synergies (active)".into(), 15.0, gold);
                                    for s in &roster.synergies {
                                        label(
                                            content,
                                            format!("  {} - {}", s.name, s.detail),
                                            13.0,
                                            Color::srgb(0.6, 0.95, 0.7),
                                        );
                                    }
                                }
                                if !roster.combos.is_empty() {
                                    label(content, "Combos this party can run".into(), 15.0, gold);
                                    for c in &roster.combos {
                                        label(
                                            content,
                                            format!("  {} : {}  (+{}% on the payoff)", c.name, c.detail, c.bonus_pct),
                                            13.0,
                                            Color::srgb(0.75, 0.85, 1.0),
                                        );
                                    }
                                }
                                if !roster.heroes.is_empty() {
                                    label(
                                        content,
                                        "[Up/Down] select | [Space]/[1-4] rename | [Enter] save | [Esc] cancel | click a row to swap formation".into(),
                                        12.0,
                                        dim,
                                    );
                                }
                            }
                            OverlayTab::Equip => {
                                if !inv.loaded {
                                    label(content, "loading...".into(), 16.0, dim);
                                    return;
                                }
                                let n_heroes = hero_count(&roster, &hero_names);
                                if n_heroes == 0 {
                                    label(content, "(enter a run to see your heroes)".into(), 13.0, dim);
                                    return;
                                }
                                let selected = equip_sel.hero_slot;
                                let hero_label = hero_name_at(&roster, &hero_names, selected)
                                    .unwrap_or_else(|| format!("Hero {}", selected + 1));

                                if let Some(category) = picker.category {
                                    // Picker screen: every candidate for this one
                                    // category + hero. Nothing here equips until
                                    // it's actually clicked/Space'd.
                                    label(
                                        content,
                                        format!(
                                            "Equip {} - {}",
                                            class_display(category),
                                            hero_label
                                        ),
                                        16.0,
                                        gold,
                                    );
                                    let mut idx = 0usize;
                                    {
                                        let focused = cursor.index == idx;
                                        idx += 1;
                                        content
                                            .spawn((
                                                Button,
                                                PickerBackButton,
                                                Node {
                                                    padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                                                    border: UiRect::all(Val::Px(2.0)),
                                                    ..default()
                                                },
                                                BorderColor(if focused { focus_border } else { no_border }),
                                                BackgroundColor(Color::NONE),
                                                BorderRadius::all(Val::Px(4.0)),
                                            ))
                                            .with_children(|b| {
                                                label(b, "< Back".into(), 14.0, dim);
                                            });
                                    }
                                    let baseline =
                                        effective_baseline_stat(&inv, &run_gear, category, selected);
                                    {
                                        let focused = cursor.index == idx;
                                        idx += 1;
                                        content
                                            .spawn((
                                                Button,
                                                PickerUnequipButton { category },
                                                Node {
                                                    width: Val::Percent(100.0),
                                                    padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                                                    border: UiRect::all(Val::Px(2.0)),
                                                    ..default()
                                                },
                                                BorderColor(if focused { focus_border } else { no_border }),
                                                BackgroundColor(Color::NONE),
                                                BorderRadius::all(Val::Px(4.0)),
                                            ))
                                            .with_children(|b| {
                                                label(
                                                    b,
                                                    "  ( remove - leave this slot empty )".into(),
                                                    15.0,
                                                    Color::srgb(0.9, 0.6, 0.55),
                                                );
                                            });
                                    }
                                    let hero_class = hero_class_at(&roster, &hero_names, selected);
                                    let rows: Vec<(&GearLine, GearSource)> = category_gear(
                                        &inv.gear, category, selected, hero_class,
                                    )
                                    .into_iter()
                                    .map(|g| (g, GearSource::Vault))
                                    .chain(
                                        category_gear(&run_gear.gear, category, selected, hero_class)
                                            .into_iter()
                                            .map(|g| (g, GearSource::RunLoot)),
                                    )
                                    .collect();
                                    if rows.is_empty() {
                                        label(content, "  (nothing else available)".into(), 14.0, dim);
                                    }
                                    for (g, source) in rows {
                                        let broken = g.max_durability == 0;
                                        let worn_here = g.equipped_hero_slot == Some(selected);
                                        let focused = cursor.index == idx;
                                        idx += 1;
                                        let stat = gear_slot_stat(g);
                                        // Plain ASCII — the game font has no glyph
                                        // for the Unicode triangle arrows.
                                        let arrow = if worn_here || stat == baseline {
                                            ""
                                        } else if stat > baseline {
                                            " ^"
                                        } else {
                                            " v"
                                        };
                                        let tag = if worn_here {
                                            match source {
                                                GearSource::Vault => "  [equipped]",
                                                GearSource::RunLoot => "  [worn this run]",
                                            }
                                        } else if broken {
                                            "  [broken]"
                                        } else {
                                            match source {
                                                GearSource::RunLoot => "  (found this run)",
                                                GearSource::Vault => "",
                                            }
                                        };
                                        // The tier is spelled out, never colour-coded
                                        // alone: a player must be able to read that
                                        // an item is temporary (GR-6).
                                        let ins = match insurance_of(&g.insurance) {
                                            Insurance::Ephemeral => " ephemeral",
                                            Insurance::Insured => "",
                                        };
                                        let class_tag = if g.class_key.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" {}", class_display(&g.class_key))
                                        };
                                        // GR-5: an item this hero may not wear stays
                                        // in the list, labelled with the reason —
                                        // hiding it would teach the player nothing.
                                        let blocked = gear_block_reason(g, hero_class);
                                        let affix_mark = if g.affixes.is_empty() {
                                            String::new()
                                        } else {
                                            format!("  *{}", g.affixes.len())
                                        };
                                        let text = match &blocked {
                                            Some(why) => format!(
                                                "  {} {}  [{}{}{} t{}]  -- {}",
                                                gear_slot_icon(category), g.name, category, ins, class_tag, g.tier, why
                                            ),
                                            None => format!(
                                                "  {} {}  [{}{}{} t{}]  +{}{}{}{}",
                                                gear_slot_icon(category), g.name, category, ins, class_tag, g.tier, stat, affix_mark, arrow, tag
                                            ),
                                        };
                                        let mut col = if worn_here {
                                            Color::srgb(0.6, 0.95, 0.7)
                                        } else if broken {
                                            Color::srgb(0.85, 0.45, 0.45)
                                        } else {
                                            Color::srgb(0.85, 0.92, 1.0)
                                        };
                                        if !worn_here && arrow == " ^" {
                                            col = green;
                                        } else if !worn_here && arrow == " v" {
                                            col = red;
                                        }
                                        if blocked.is_some() {
                                            col = Color::srgb(0.48, 0.5, 0.58);
                                        }
                                        content
                                            .spawn((
                                                Button,
                                                GearButton {
                                                    gear_id: g.gear_id.clone(),
                                                    source,
                                                    target_hero_slot: selected,
                                                    worn: worn_here,
                                                    blocked: blocked.is_some(),
                                                    free_first: off_hand_in_the_way(
                                                        &inv.gear, g, selected,
                                                    ),
                                                },
                                                Node {
                                                    width: Val::Percent(100.0),
                                                    padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                                                    border: UiRect::all(Val::Px(2.0)),
                                                    ..default()
                                                },
                                                BorderColor(if focused { focus_border } else { no_border }),
                                                BackgroundColor(if worn_here {
                                                    Color::srgba(0.12, 0.3, 0.16, 0.6)
                                                } else {
                                                    Color::NONE
                                                }),
                                                BorderRadius::all(Val::Px(4.0)),
                                            ))
                                            .with_children(|b| {
                                                b.spawn((
                                                    Text::new(text),
                                                    TextFont { font_size: 15.0, ..default() },
                                                    TextColor(col),
                                                ));
                                            });
                                    }
                                } else {
                                    // Main screen: hero switcher, then one row per
                                    // equipment category showing whatever's
                                    // currently worn (from the Vault or this run's
                                    // loot — see `effective_worn_item`). Selecting
                                    // a category opens the picker for it; nothing
                                    // equips from this screen directly.
                                    let mut idx = 0usize;
                                    content
                                        .spawn(Node {
                                            flex_direction: FlexDirection::Row,
                                            column_gap: Val::Px(4.0),
                                            ..default()
                                        })
                                        .with_children(|hs| {
                                            for i in 0..n_heroes {
                                                let name = hero_name_at(&roster, &hero_names, i)
                                                    .unwrap_or_else(|| format!("Hero {}", i + 1));
                                                let active = selected == i;
                                                let focused = cursor.index == idx;
                                                idx += 1;
                                                hs.spawn((
                                                    Button,
                                                    HeroSwitchButton(i),
                                                    Node {
                                                        padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                                                        border: UiRect::all(Val::Px(2.0)),
                                                        ..default()
                                                    },
                                                    BorderColor(if focused { focus_border } else { no_border }),
                                                    BackgroundColor(if active {
                                                        Color::srgba(0.18, 0.28, 0.5, 0.85)
                                                    } else {
                                                        Color::NONE
                                                    }),
                                                    BorderRadius::all(Val::Px(4.0)),
                                                ))
                                                .with_children(|b| {
                                                    b.spawn((
                                                        Text::new(name),
                                                        TextFont { font_size: 14.0, ..default() },
                                                        TextColor(if active { gold } else { dim }),
                                                    ));
                                                });
                                            }
                                        });
                                    for category in crate::GEAR_CATEGORIES {
                                        let focused = cursor.index == idx;
                                        idx += 1;
                                        let worn = effective_worn_item(&inv, &run_gear, category, selected);
                                        let icon = gear_slot_icon(category);
                                        let text = match worn {
                                            Some(g) => {
                                                let class_tag = if g.class_key.is_empty() {
                                                    String::new()
                                                } else {
                                                    format!(" ({})", class_display(&g.class_key))
                                                };
                                                format!(
                                                    "- {} -   {icon} {}   +{}{}",
                                                    class_display(category),
                                                    g.name,
                                                    gear_slot_stat(g),
                                                    class_tag
                                                )
                                            }
                                            None => {
                                                format!("- {} -   (empty)", class_display(category))
                                            }
                                        };
                                        content
                                            .spawn((
                                                Button,
                                                CategoryButton { category },
                                                Node {
                                                    width: Val::Percent(100.0),
                                                    padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                                                    border: UiRect::all(Val::Px(2.0)),
                                                    ..default()
                                                },
                                                BorderColor(if focused { focus_border } else { no_border }),
                                                BackgroundColor(Color::NONE),
                                                BorderRadius::all(Val::Px(4.0)),
                                            ))
                                            .with_children(|b| {
                                                label(
                                                    b,
                                                    text,
                                                    15.0,
                                                    if worn.is_some() { gold } else { dim },
                                                );
                                            });
                                    }
                                }
                            }
                        });
                    });
                    let hint = if *tab == OverlayTab::Equip && picker.category.is_some() {
                        "[Up/Down] select | [Space] equip | [ESC] back"
                    } else {
                        "[Left/Right] tabs | [Up/Down] select | [Space] act | [ESC] close"
                    };
                    label(p, hint.into(), 13.0, dim);
                }
                OverlayKind::LevelUp => {
                    label(p, "LEVEL UP".into(), 24.0, gold);
                    if !prog.loaded {
                        label(p, "loading...".into(), 16.0, dim);
                        label(p, "[ESC] close".into(), 13.0, dim);
                        return;
                    }
                    label(p, "- Meld Skills -".into(), 15.0, gold);
                    for s in &prog.skills {
                        label(
                            p,
                            format!("  {:<12} Lv {}   ({} XP)", s.kind.replace('_', " "), s.level, s.xp),
                            16.0,
                            Color::srgb(0.7, 0.85, 1.0),
                        );
                    }
                    label(p, "- Classes Unlocked -".into(), 15.0, gold);
                    let classes = if prog.classes.is_empty() {
                        "  (none)".to_string()
                    } else {
                        format!("  {}", prog.classes.join(", "))
                    };
                    label(p, classes, 15.0, dim);
                    label(p, "[ESC] close   [I] inventory".into(), 13.0, dim);
                }
            });
        });
}

/// The single combat stat a gear item's own category cares about (main hand →
/// atk, accessory → spd, every protective piece → def — see
/// `meld_world::roll_creature_loot`).
pub(crate) fn gear_slot_stat(g: &GearLine) -> i32 {
    match g.slot.as_str() {
        "main_hand" => g.atk_bonus,
        "accessory" => g.spd_bonus,
        _ => g.def_bonus,
    }
}

/// A small Nerd Font glyph for a gear slot (sword/armor/gem), prefixed onto
/// item names so they're recognizable at a glance in the Equip screens.
/// Shares the same `TextFont` as the surrounding label text (see
/// `netglue::apply_ui_font`), so it always renders at the same size.
fn gear_slot_icon(slot: &str) -> &'static str {
    match slot {
        "main_hand" => "\u{f04e5}",              // nf-md-sword
        "off_hand" | "chest" => "\u{f132}",      // nf-fa-shield
        "head" => "\u{f02fc}",                   // nf-md-hard_hat
        "legs" => "\u{f0552}",                   // nf-md-shoe_print
        _ => "\u{f3a5}",                         // nf-fa-gem (accessory/jewelry)
    }
}

/// A floating box near the cursor with an item's full stat breakdown while
/// hovering it in the Equip picker — the row text alone only shows the one
/// stat that slot cares about; this spells out tier/durability/insurance too.
/// Immediate-mode like the overlay itself: rebuilt every frame, cheap since
/// it's at most a handful of text nodes.
#[derive(Component)]
pub(crate) struct GearTooltipRoot;

pub(crate) fn render_gear_tooltip(
    mut commands: Commands,
    windows: Query<&Window>,
    hovered: Query<(&Interaction, &GearButton)>,
    inv: Res<InventoryData>,
    run_gear: Res<RunGearData>,
    roster: Res<PartyRoster>,
    hero_names: Res<AccountHeroNames>,
    existing: Query<Entity, With<GearTooltipRoot>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    let Some((_, g)) = hovered.iter().find(|(i, _)| **i == Interaction::Hovered) else {
        return;
    };
    let list = match g.source {
        GearSource::Vault => &inv.gear,
        GearSource::RunLoot => &run_gear.gear,
    };
    let Some(item) = list.iter().find(|it| it.gear_id == g.gear_id) else {
        return;
    };
    let Some(pos) = windows.iter().next().and_then(|w| w.cursor_position()) else {
        return;
    };

    let gold = Color::srgb(0.95, 0.85, 0.5);
    let dim = Color::srgb(0.72, 0.78, 0.9);
    let good = Color::srgb(0.6, 0.95, 0.7);
    let stat_label = match item.slot.as_str() {
        "main_hand" => "Attack",
        "accessory" => "Speed",
        _ => "Defense",
    };
    let ins = insurance_of(&item.insurance);
    let ins_label = format!("{} - {}", ins.label(), ins.tooltip());
    let dur = if item.max_durability <= 0 {
        "BROKEN".to_string()
    } else {
        format!("{}/{}", item.max_durability, item.base_max_durability)
    };
    let class_label = if item.class_key.is_empty() {
        "Any class".to_string()
    } else {
        class_display(&item.class_key)
    };
    // Ephemeral gets its own amber line rather than a word buried in a header:
    // a player must never lose an item without having been told it was temporary.
    let ins_color = match ins {
        Insurance::Ephemeral => Color::srgb(0.98, 0.62, 0.35),
        Insurance::Insured => dim,
    };
    let mut lines: Vec<(String, Color)> = vec![
        (item.name.clone(), gold),
        (format!("{} | Tier {}", class_display(&item.slot), item.tier), dim),
        (ins_label, ins_color),
        (format!("Class: {class_label}"), dim),
    ];
    let note = wearable_note(item);
    if !note.is_empty() {
        lines.push((note, dim));
    }
    // AD-1: the affixes ARE the item. One line each, in their own colour, so a
    // drop is read for what it does rather than for its stat number.
    let affix_color = Color::srgb(0.72, 0.86, 1.0);
    for a in &item.affixes {
        lines.push((format!("  {}", a.describe()), affix_color));
    }
    // AD-1 chase tiers: a unique's cost is shown in warning red right under its
    // upside, because a unique is a trade and both halves have to be visible.
    if let Some(u) = meld_proto::uniques::unique(&item.unique_key) {
        lines.push((format!("  {}", u.drawback.describe()), Color::srgb(0.95, 0.45, 0.45)));
        lines.push((format!("  \"{}\"", u.flavour), Color::srgb(0.7, 0.66, 0.58)));
    }
    if let Some(set) = meld_proto::uniques::set(&item.set_key) {
        lines.push((
            format!("  {} set ({} pieces)", set.name, set.pieces_required),
            Color::srgb(0.85, 0.75, 0.95),
        ));
    }
    lines.push((format!("{stat_label}: +{}", gear_slot_stat(item)), good));
    lines.push((format!("Durability: {dur}"), dim));
    if let Some(slot) = item.equipped_hero_slot {
        let who = hero_name_at(&roster, &hero_names, slot).unwrap_or_else(|| format!("Hero {}", slot + 1));
        lines.push((format!("Equipped - {who}"), good));
    }

    commands
        .spawn((
            GearTooltipRoot,
            GlobalZIndex(1000),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(pos.x + 18.0),
                top: Val::Px(pos.y + 18.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(8.0)),
                row_gap: Val::Px(2.0),
                max_width: Val::Px(260.0),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.05, 0.06, 0.12, 0.95)),
            BorderColor(Color::srgb(0.45, 0.55, 0.85)),
            BorderRadius::all(Val::Px(6.0)),
        ))
        .with_children(|t| {
            for (i, (text, color)) in lines.into_iter().enumerate() {
                t.spawn((
                    Text::new(text),
                    TextFont { font_size: if i == 0 { 15.0 } else { 12.0 }, ..default() },
                    TextColor(color),
                ));
            }
        });
}

/// Click / hover on the Equip tab picker's gear rows. A press equips the item
/// to the hero the picker was opened for and closes back to the main screen;
/// the server enforces the rules (one per hero+category, not broken), and a
/// rejected change (409) is a silent no-op — the refresh re-shows the truth.
/// `Changed<Interaction>` makes each click fire exactly once.
pub(crate) fn gear_click(
    net: NonSend<NetRes>,
    mut picker: ResMut<EquipPicker>,
    mut cursor: ResMut<OverlayCursor>,
    mut rows: Query<(&Interaction, &GearButton, &mut BackgroundColor), Changed<Interaction>>,
) {
    for (interaction, g, mut bg) in &mut rows {
        match *interaction {
            Interaction::Pressed => {
                if !g.gear_id.is_empty() && !g.blocked {
                    let target = Some(g.target_hero_slot);
                    match g.source {
                        GearSource::Vault => match &g.free_first {
                            // Both hands: put the off-hand away first, rather than
                            // bouncing the player off a 409 (GR-5).
                            Some(off) => net.0.equip_gear_freeing(
                                off.clone(),
                                g.gear_id.clone(),
                                g.target_hero_slot,
                            ),
                            None => net.0.equip_gear(g.gear_id.clone(), target),
                        },
                        GearSource::RunLoot => net.0.send(ClientCmd::EquipLoot {
                            gear_id: g.gear_id.clone(),
                            hero_slot: target.map(|s| s as i32),
                        }),
                    }
                    picker.category = None;
                    cursor.index = 0;
                }
            }
            Interaction::Hovered => {
                *bg = BackgroundColor(Color::srgba(0.2, 0.24, 0.4, 0.7));
            }
            Interaction::None => {
                *bg = BackgroundColor(if g.worn {
                    Color::srgba(0.12, 0.3, 0.16, 0.6)
                } else {
                    Color::NONE
                });
            }
        }
    }
}

/// Click a category row on the Equip tab's main screen: opens the picker
/// screen for that category (nothing equips just by opening it).
pub(crate) fn category_button_click(
    mut picker: ResMut<EquipPicker>,
    mut cursor: ResMut<OverlayCursor>,
    mut rows: Query<(&Interaction, &CategoryButton, &mut BackgroundColor), Changed<Interaction>>,
) {
    for (interaction, c, mut bg) in &mut rows {
        match *interaction {
            Interaction::Pressed => {
                picker.category = Some(c.category);
                cursor.index = 0;
            }
            Interaction::Hovered => *bg = BackgroundColor(Color::srgba(0.2, 0.24, 0.4, 0.7)),
            Interaction::None => *bg = BackgroundColor(Color::NONE),
        }
    }
}

/// Click the picker screen's "remove" row: clears the hero's equipped item(s)
/// in that category (Vault and/or run-loot) and closes back to the main screen.
pub(crate) fn picker_unequip_click(
    net: NonSend<NetRes>,
    inv: Res<InventoryData>,
    run_gear: Res<RunGearData>,
    equip_sel: Res<EquipSelection>,
    mut picker: ResMut<EquipPicker>,
    mut cursor: ResMut<OverlayCursor>,
    mut rows: Query<(&Interaction, &PickerUnequipButton, &mut BackgroundColor), Changed<Interaction>>,
) {
    for (interaction, u, mut bg) in &mut rows {
        match *interaction {
            Interaction::Pressed => {
                unequip_category(&net.0, &inv, &run_gear, u.category, equip_sel.hero_slot);
                picker.category = None;
                cursor.index = 0;
            }
            Interaction::Hovered => *bg = BackgroundColor(Color::srgba(0.4, 0.2, 0.22, 0.7)),
            Interaction::None => *bg = BackgroundColor(Color::NONE),
        }
    }
}

/// Click the picker screen's "back" row: closes it without changing anything.
pub(crate) fn picker_back_click(
    mut picker: ResMut<EquipPicker>,
    mut cursor: ResMut<OverlayCursor>,
    mut rows: Query<(&Interaction, &mut BackgroundColor), (Changed<Interaction>, With<PickerBackButton>)>,
) {
    for (interaction, mut bg) in &mut rows {
        match *interaction {
            Interaction::Pressed => {
                picker.category = None;
                cursor.index = 0;
            }
            Interaction::Hovered => *bg = BackgroundColor(Color::srgba(0.2, 0.24, 0.4, 0.7)),
            Interaction::None => *bg = BackgroundColor(Color::NONE),
        }
    }
}

/// Party-screen front/back-row toggle: clicking a hero's row flips its formation
/// (server persists it + re-sends the roster, which re-renders the button).
pub(crate) fn formation_click(
    net: NonSend<NetRes>,
    mut rows: Query<(&Interaction, &FormationButton, &mut BackgroundColor), Changed<Interaction>>,
) {
    for (interaction, f, mut bg) in &mut rows {
        match *interaction {
            Interaction::Pressed => {
                net.0.send(ClientCmd::SetFormation { slot: f.slot, back_row: !f.back_row });
            }
            Interaction::Hovered => {
                *bg = BackgroundColor(Color::srgba(0.2, 0.24, 0.4, 0.7));
            }
            Interaction::None => {
                *bg = BackgroundColor(Color::NONE);
            }
        }
    }
}

/// Click on an inventory tab button (Items/Equip/Status).
pub(crate) fn overlay_tab_click(
    mut tab: ResMut<OverlayTab>,
    rows: Query<(&Interaction, &TabButton), Changed<Interaction>>,
) {
    for (interaction, t) in &rows {
        if *interaction == Interaction::Pressed {
            *tab = t.0;
        }
    }
}

/// Click on a hero-switcher button on the Equip tab.
pub(crate) fn equip_hero_switch_click(
    mut sel: ResMut<EquipSelection>,
    rows: Query<(&Interaction, &HeroSwitchButton), Changed<Interaction>>,
) {
    for (interaction, h) in &rows {
        if *interaction == Interaction::Pressed {
            sel.hero_slot = h.0;
        }
    }
}

/// Click a material's "Withdraw" button on the Items tab: takes 1 unit out of
/// the Vault into the pending-backpack queue (a 409 — not enough left — is a
/// silent no-op; the refresh re-shows the truth).
pub(crate) fn withdraw_click(
    net: NonSend<NetRes>,
    rows: Query<(&Interaction, &WithdrawButton), Changed<Interaction>>,
) {
    for (interaction, w) in &rows {
        if *interaction == Interaction::Pressed {
            net.0.withdraw_material(w.item_kind.clone(), 1);
        }
    }
}

/// Marker for the loot report banner's root.
#[derive(Component)]
pub(crate) struct LootReportRoot;

/// Seconds the loot report stays up before auto-dismissing.
pub(crate) const LOOT_REPORT_DURATION: f32 = 4.0;

/// Immediate-mode: a glass banner top-center showing XP/chits/items gained
/// from a battle victory or a treasure chest, auto-dismissing after a few
/// seconds (or immediately on Escape). A pure display — no buttons to
/// preserve across frames — so it's cheap to rebuild every tick while active.
/// (Dismissing on any click was considered but dropped: a click here would
/// also fall through to overworld movement/avatar-click handling with no way
/// to consume it.)
/// Colour a gear name by its rarity prefix (Rare/Epic/Legendary), else the neutral
/// common colour. Rarity rides the name ("Legendary Frostforged Greatblade"), so
/// this works for both fresh drops and banked Vault gear (FS gear-rarity).
pub(crate) fn rarity_color(name: &str) -> Color {
    match name.split_whitespace().next() {
        Some("Legendary") => Color::srgb(1.0, 0.8, 0.3),
        Some("Epic") => Color::srgb(0.78, 0.5, 1.0),
        Some("Rare") => Color::srgb(0.45, 0.72, 1.0),
        _ => Color::srgb(0.6, 0.95, 0.7),
    }
}

pub(crate) fn render_loot_report(
    mut commands: Commands,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut report: ResMut<LootReport>,
    existing: Query<Entity, With<LootReportRoot>>,
) {
    if report.active {
        report.elapsed += time.delta_secs();
        if report.elapsed > LOOT_REPORT_DURATION || keys.just_pressed(KeyCode::Escape) {
            report.active = false;
        }
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    if !report.active {
        return;
    }
    let dim = Color::srgb(0.72, 0.78, 0.9);
    commands
        .spawn((
            LootReportRoot,
            // Root UI nodes have no reliable stacking order otherwise (Bevy
            // doesn't guarantee draw order between separate UI roots) — pin
            // this above the level-up screen so a level-up right after a
            // battle never covers the XP/loot you just got from it.
            GlobalZIndex(100),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                // Below the overworld HUD's distance/biome/hints text block
                // (~3 lines) so the two don't overlap.
                top: Val::Px(110.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::FlexStart,
                flex_direction: FlexDirection::Column,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(6.0),
                    align_items: AlignItems::Center,
                    padding: UiRect::axes(Val::Px(32.0), Val::Px(20.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    ..default()
                },
                BorderColor(glass_edge()),
                BackgroundColor(glass_fill()),
                BorderRadius::all(Val::Px(10.0)),
            ))
            .with_children(|p| {
                p.spawn((
                    Text::new(report.title.clone()),
                    TextFont { font_size: 28.0, ..default() },
                    TextColor(Color::srgb(0.95, 0.85, 0.5)),
                ));
                let totals = match report.xp {
                    Some(xp) => format!("+{xp} XP    +{} chits", report.chits),
                    None => format!("+{} chits", report.chits),
                };
                p.spawn((
                    Text::new(totals),
                    TextFont { font_size: 20.0, ..default() },
                    TextColor(Color::srgb(0.85, 0.92, 1.0)),
                ));
                for (kind, qty) in &report.items {
                    p.spawn((
                        Text::new(format!("+{} {}", qty, kind.replace('_', " "))),
                        TextFont { font_size: 16.0, ..default() },
                        TextColor(dim),
                    ));
                }
                for name in &report.gear {
                    p.spawn((
                        Text::new(format!("+1 {name}")),
                        TextFont { font_size: 16.0, ..default() },
                        TextColor(rarity_color(name)),
                    ));
                }
            });
        });
}

/// Seconds between each stat line revealing, and the hold before auto-advancing.
pub(crate) const LU_REVEAL_STEP: f32 = 0.34;
pub(crate) const LU_HOLD_AFTER: f32 = 1.6;

/// Old-school "LEVEL UP!" stat screen: plays one hero at a time, revealing each
/// stat gain line-by-line (the classic scroll), then auto-advances (or [Space]/
/// [Enter] to hurry). Immediate-mode: the panel is rebuilt each frame while a
/// hero is showing, and torn down when the queue drains.
pub(crate) fn level_up_screen(
    mut commands: Commands,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut lu: ResMut<LevelUpQueue>,
    existing: Query<Entity, With<LevelUpRoot>>,
) {
    // Pull the next hero when none is on screen.
    if lu.current.is_none() {
        match lu.pending.pop_front() {
            Some(h) => {
                lu.current = Some(h);
                lu.elapsed = 0.0;
            }
            None => {
                for e in &existing {
                    commands.entity(e).despawn();
                }
                return;
            }
        }
    }
    lu.elapsed += time.delta_secs();

    const TOTAL_LINES: usize = 5; // HP, STR, MND, DEX, WLL
    let reveal_full_at = LU_REVEAL_STEP * TOTAL_LINES as f32;
    let hurry = keys.just_pressed(KeyCode::Space) || keys.just_pressed(KeyCode::Enter);
    if hurry {
        if lu.elapsed < reveal_full_at {
            lu.elapsed = reveal_full_at; // reveal everything at once
        } else {
            lu.current = None; // advance to next hero (or finish)
            for e in &existing {
                commands.entity(e).despawn();
            }
            return;
        }
    } else if !lu.hold && lu.elapsed >= reveal_full_at + LU_HOLD_AFTER {
        lu.current = None; // auto-advance (skipped in held demo mode)
        for e in &existing {
            commands.entity(e).despawn();
        }
        return;
    }

    let reveal = ((lu.elapsed / LU_REVEAL_STEP).floor() as usize + 1).min(TOTAL_LINES);
    let h = lu.current.clone().unwrap();
    let more = !lu.pending.is_empty();

    // The five stat lines, in classic order.
    let lines: [(&str, (i32, i32)); TOTAL_LINES] = [
        ("HP ", h.max_hp),
        ("STR", h.str_),
        ("MND", h.mnd),
        ("DEX", h.dex),
        ("WLL", h.wll),
    ];

    for e in &existing {
        commands.entity(e).despawn();
    }
    let gold = Color::srgb(0.98, 0.86, 0.42);
    let dim = Color::srgb(0.72, 0.78, 0.9);
    let gain_col = Color::srgb(0.55, 0.98, 0.62);
    commands
        .spawn((
            LevelUpRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Px(420.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(6.0),
                    padding: UiRect::all(Val::Px(20.0)),
                    border: UiRect::all(Val::Px(3.0)),
                    ..default()
                },
                BorderColor(gold),
                BackgroundColor(Color::srgb(0.06, 0.05, 0.14)),
                BorderRadius::all(Val::Px(6.0)),
            ))
            .with_children(|p| {
                let label = |p: &mut ChildSpawnerCommands, text: String, size: f32, color: Color| {
                    p.spawn((
                        Text::new(text),
                        TextFont { font_size: size, ..default() },
                        TextColor(color),
                    ));
                };
                label(p, "*  LEVEL UP!  *".into(), 26.0, gold);
                label(p, h.name.clone(), 22.0, Color::srgb(0.9, 0.95, 1.0));
                label(
                    p,
                    format!("{}  -  now Level {}", class_display(&h.class_key), h.level),
                    14.0,
                    dim,
                );
                // Divider.
                p.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(2.0),
                        margin: UiRect::vertical(Val::Px(4.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.4, 0.34, 0.14)),
                ));
                for (i, (name, (before, after))) in lines.iter().enumerate() {
                    if i >= reveal {
                        break;
                    }
                    let gain = after - before;
                    let (arrow, gcol) = if gain > 0 {
                        (format!("+{gain}"), gain_col)
                    } else {
                        ("--".to_string(), dim)
                    };
                    // One row: STAT   before -> after        +gain
                    p.spawn((Node {
                        width: Val::Percent(100.0),
                        justify_content: JustifyContent::SpaceBetween,
                        ..default()
                    },))
                        .with_children(|row| {
                            row.spawn((
                                Text::new(format!("{name}   {before} -> {after}")),
                                TextFont { font_size: 18.0, ..default() },
                                TextColor(if gain > 0 { Color::srgb(0.9, 0.95, 1.0) } else { dim }),
                            ));
                            row.spawn((
                                Text::new(arrow),
                                TextFont { font_size: 18.0, ..default() },
                                TextColor(gcol),
                            ));
                        });
                }
                let footer = if more { "[Space] next hero" } else { "[Space] continue" };
                label(p, footer.into(), 12.0, dim);
            });
        });
}

/// Parse a gear row's insurance word, tolerating the stored chest colours.
/// Anything unrecognized reads as Ephemeral — the cautious default, since the
/// cost of wrongly believing an item is safe is losing it (GR-6).
fn insurance_of(word: &str) -> Insurance {
    Insurance::from_wire(word).unwrap_or(Insurance::Ephemeral)
}

/// The one-line "who can wear this, and how many hands" note for the detail card
/// (GR-5). Empty when the item carries no restriction.
pub(crate) fn wearable_note(item: &GearLine) -> String {
    use meld_proto::equipment as eq;
    let mut parts: Vec<String> = Vec::new();
    if let Some(f) = eq::ItemFamily::from_wire(&item.family) {
        parts.push(if f.hands() == 2 {
            format!("{} (two-handed)", f.wire())
        } else {
            f.wire().to_string()
        });
    }
    if let Some(w) = eq::ArmorWeight::from_wire(&item.armor_weight) {
        parts.push(format!("{} armor", w.wire()));
    }
    if parts.is_empty() {
        return String::new();
    }
    parts.join(" | ")
}

#[cfg(test)]
mod gear_label_tests {
    use super::*;

    fn line(insurance: &str, family: &str, weight: &str) -> GearLine {
        GearLine {
            gear_id: "g".into(),
            name: "Test".into(),
            slot: "main_hand".into(),
            class_key: String::new(),
            insurance: insurance.into(),
            family: family.into(),
            armor_weight: weight.into(),
            tier: 1,
            equipped_hero_slot: None,
            max_durability: 10,
            base_max_durability: 10,
            atk_bonus: 1,
            def_bonus: 0,
            spd_bonus: 0,
            affixes: Vec::new(),
            unique_key: String::new(),
            set_key: String::new(),
        }
    }

    #[test]
    fn ephemeral_says_what_it_does_in_words() {
        let e = insurance_of("ephemeral");
        assert_eq!(e.label(), "Ephemeral");
        assert!(e.tooltip().contains("Vanishes when the run ends"));
        // The stored chest colours still parse to the right tier.
        assert_eq!(insurance_of("red"), Insurance::Ephemeral);
        assert_eq!(insurance_of("blue"), Insurance::Insured);
        // An unreadable word is treated as ephemeral: wrongly believing an item is
        // safe costs the player the item.
        assert_eq!(insurance_of("???"), Insurance::Ephemeral);
    }

    #[test]
    fn wearable_note_calls_out_two_handed_and_weight() {
        assert_eq!(wearable_note(&line("insured", "staff", "")), "staff (two-handed)");
        assert_eq!(wearable_note(&line("insured", "dagger", "")), "dagger");
        assert_eq!(wearable_note(&line("insured", "", "heavy")), "heavy armor");
        // A plain stat stick claims nothing.
        assert!(wearable_note(&line("insured", "", "")).is_empty());
    }
}

#[cfg(test)]
mod equip_ux_tests {
    use super::*;

    fn item(name: &str, slot: &str, family: &str, weight: &str) -> GearLine {
        GearLine {
            gear_id: name.into(),
            name: name.into(),
            slot: slot.into(),
            class_key: String::new(),
            insurance: "insured".into(),
            family: family.into(),
            armor_weight: weight.into(),
            tier: 2,
            equipped_hero_slot: None,
            max_durability: 40,
            base_max_durability: 40,
            atk_bonus: 3,
            def_bonus: 3,
            spd_bonus: 0,
            affixes: Vec::new(),
            unique_key: String::new(),
            set_key: String::new(),
        }
    }

    #[test]
    fn blocked_rows_state_the_rule_in_the_player_s_words() {
        let spear = item("Warpike", "main_hand", "spear", "");
        let plate = item("Battleplate", "chest", "", "heavy");
        assert_eq!(gear_block_reason(&spear, Some("resonant")).as_deref(), Some("cannot wield"));
        assert_eq!(gear_block_reason(&plate, Some("psyker")).as_deref(), Some("too heavy"));
        // Legal kit is not labelled.
        assert!(gear_block_reason(&spear, Some("explorer")).is_none());
        assert!(gear_block_reason(&plate, Some("iron_hull")).is_none());
        // An unknown class never blocks: the server would allow it, so the UI must
        // not claim otherwise.
        assert!(gear_block_reason(&spear, None).is_none());
        assert!(gear_block_reason(&spear, Some("")).is_none());
    }

    #[test]
    fn a_two_hander_names_the_off_hand_it_displaces() {
        let mut shield = item("Targe", "off_hand", "shield", "");
        shield.equipped_hero_slot = Some(1);
        let spear = item("Warpike", "main_hand", "spear", "");
        let sword = item("Warblade", "main_hand", "sword", "");
        let gear = vec![shield.clone()];
        // Two-handed + a filled off-hand → free it first.
        assert_eq!(off_hand_in_the_way(&gear, &spear, 1).as_deref(), Some("Targe"));
        // One-handed displaces nothing, and neither does another hero's off-hand.
        assert!(off_hand_in_the_way(&gear, &sword, 1).is_none());
        assert!(off_hand_in_the_way(&gear, &spear, 0).is_none());
        // Nothing in the way at all.
        assert!(off_hand_in_the_way(&[], &spear, 1).is_none());
    }
}

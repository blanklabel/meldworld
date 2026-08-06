//! The main menu: **three columns that cascade left**, in the manner of the Dragon
//! Quest remakes.
//!
//! Column one is the nav — *Items, Materials, Party, Map*. Choosing one opens column
//! two to its right; from the Party column, a hero's *Equipment* or *Abilities*
//! button opens column three. The nav never disappears, so a player can always see
//! where they are and step back out one column at a time.
//!
//! Each column is a flat list with exactly one job, which is what lets the deep
//! content — a hero's gear, a hero's abilities — have a whole panel to itself
//! instead of being crammed under a shared header.
//!
//! **Nothing here shows how to unlock anything.** No locked ability rows, no
//! "reach level N", no trigger hints — finding out is the game. Only what a hero
//! actually has is listed.

use bevy::prelude::*;

use meld_client::glass;

use super::*;

/// Which nav row is open (column two's content).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MenuSection {
    Items,
    Materials,
    Party,
    Map,
}

impl MenuSection {
    pub(crate) const ALL: [MenuSection; 4] =
        [MenuSection::Items, MenuSection::Materials, MenuSection::Party, MenuSection::Map];

    pub(crate) fn label(self) -> &'static str {
        match self {
            MenuSection::Items => "Items",
            MenuSection::Materials => "Materials",
            MenuSection::Party => "Party",
            MenuSection::Map => "Map",
        }
    }
}

/// Which per-hero pane column three is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MenuPane {
    Equipment,
    Abilities,
}

/// The cascade's state. `section` opens column two, `pane` opens column three for
/// hero `member`; `cursor` walks the deepest open column.
#[derive(Resource, Default)]
pub(crate) struct MainMenu {
    pub(crate) section: Option<MenuSection>,
    pub(crate) member: usize,
    pub(crate) pane: Option<MenuPane>,
    pub(crate) cursor: usize,
}

impl MainMenu {
    /// How deep the cascade is open — which column the cursor and Back apply to.
    pub(crate) fn depth(&self) -> u8 {
        match (self.section, self.pane) {
            (_, Some(_)) => 2,
            (Some(_), _) => 1,
            _ => 0,
        }
    }

    /// Step back one column. Returns false when there is nothing left to close, so
    /// the caller can shut the whole menu instead.
    pub(crate) fn back(&mut self) -> bool {
        self.cursor = 0;
        if self.pane.is_some() {
            self.pane = None;
            true
        } else if self.section.is_some() {
            self.section = None;
            true
        } else {
            false
        }
    }
}

/// A nav row in column one.
#[derive(Component)]
pub(crate) struct NavButton(pub(crate) MenuSection);

/// An Equipment/Abilities button under a hero in the Party column.
#[derive(Component)]
pub(crate) struct PaneButton {
    pub(crate) member: usize,
    pub(crate) pane: MenuPane,
}

/// Marker for the cascade's root, so it can be torn down as one.
#[derive(Component)]
pub(crate) struct MainMenuRoot;

/// The hero rows column two's Party pane shows: the live run's roster when there is
/// one, else the account's saved names (browsing from town between dives).
fn party_lines(roster: &PartyRoster, names: &AccountHeroNames) -> Vec<meld_client::net::HeroLine> {
    if !roster.heroes.is_empty() {
        return roster.heroes.clone();
    }
    names
        .names
        .iter()
        .enumerate()
        .map(|(i, n)| meld_client::net::HeroLine {
            name: n.clone(),
            class_key: names.classes.get(i).cloned().unwrap_or_default(),
            // Browsing from town: there is no run, so nothing but the name and class
            // is known yet. Level 1 is what the roster screen shows for a slot that
            // has never dived.
            level: 1,
            str_: 0,
            mnd: 0,
            dex: 0,
            wll: 0,
            max_hp: 0,
            hp: 0,
            xp: 0,
            xp_to_next: 0,
            back_row: false,
        })
        .collect()
}

/// `Explorer - Pioneer`: the class, plus the rank the hero holds in its order. The
/// rank gates nothing; it is there because a career reads better than a number.
fn class_and_rank(class_key: &str, level: i32) -> String {
    let class = class_display(class_key);
    match meld_proto::skills::rank_title(class_key, level) {
        Some(rank) => format!("{class} - {rank}"),
        None => class,
    }
}

/// How many rows the deepest open column has, so the cursor can wrap.
pub(crate) fn column_len(
    menu: &MainMenu,
    roster: &PartyRoster,
    names: &AccountHeroNames,
    inv: &InventoryData,
    backpack: &RunBackpack,
    picker: &EquipPicker,
) -> usize {
    match (menu.section, menu.pane) {
        (Some(MenuSection::Party), Some(MenuPane::Abilities)) => {
            let heroes = party_lines(roster, names);
            heroes
                .get(menu.member)
                .map(|h| {
                    meld_proto::skills::skills_for_class_at(&h.class_key, h.level).len()
                })
                .unwrap_or(0)
        }
        (Some(MenuSection::Party), Some(MenuPane::Equipment)) => match picker.category {
            // The picker's own rows are counted by the equip flow that owns them.
            Some(_) => 0,
            None => GEAR_CATEGORIES.len(),
        },
        (Some(MenuSection::Party), None) => party_lines(roster, names).len(),
        (Some(MenuSection::Items), None) => backpack.items.len().max(1),
        (Some(MenuSection::Materials), None) => inv.materials.len().max(1),
        (Some(MenuSection::Map), None) => 1,
        (None, _) => MenuSection::ALL.len(),
        // Only the Party column opens a third; anything else has nothing deeper.
        (Some(_), Some(_)) => 0,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_main_menu(
    mut commands: Commands,
    overlay: Res<Overlay>,
    menu: Res<MainMenu>,
    equip_sel: Res<EquipSelection>,
    picker: Res<EquipPicker>,
    inv: Res<InventoryData>,
    run_gear: Res<RunGearData>,
    roster: Res<PartyRoster>,
    hero_names: Res<AccountHeroNames>,
    stats: Res<RunStats>,
    backpack: Res<RunBackpack>,
    wa: Option<Res<WorldAssets>>,
    existing: Query<Entity, With<MainMenuRoot>>,
) {
    if !(overlay.is_changed()
        || menu.is_changed()
        || equip_sel.is_changed()
        || picker.is_changed()
        || inv.is_changed()
        || run_gear.is_changed()
        || roster.is_changed()
        || hero_names.is_changed()
        || stats.is_changed()
        || backpack.is_changed())
    {
        return;
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    if overlay.kind != Some(OverlayKind::Inventory) {
        return;
    }
    let heroes = party_lines(&roster, &hero_names);
    let depth = menu.depth();

    commands
        .spawn((MainMenuRoot, glass::scrim()))
        .with_children(|root| {
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                column_gap: Val::Px(14.0),
                ..default()
            })
            .with_children(|cols| {
                // ---- column one: the nav. Always present, so you can always see
                // where you are and step back out.
                cols.spawn(glass::panel(Val::Px(240.0))).with_children(|nav| {
                    nav.spawn(glass::text("MENU", 26.0, glass::TITLE));
                    nav.spawn(glass::divider());
                    for (i, s) in MenuSection::ALL.iter().enumerate() {
                        let open = menu.section == Some(*s);
                        let focused = depth == 0 && menu.cursor == i;
                        nav.spawn((Button, NavButton(*s), glass::chip(open || focused)))
                            .with_children(|b| {
                                b.spawn(glass::text(
                                    s.label(),
                                    22.0,
                                    if open || focused { glass::TITLE } else { glass::TEXT },
                                ));
                            });
                    }
                    nav.spawn(glass::text("[Esc] close", 14.0, glass::DIM));
                });

                // ---- column two: whatever the nav opened.
                let Some(section) = menu.section else { return };
                cols.spawn(glass::panel(Val::Px(520.0))).with_children(|col| {
                    col.spawn(glass::text(section.label().to_uppercase(), 26.0, glass::TITLE));
                    col.spawn(glass::divider());
                    match section {
                        MenuSection::Items => {
                            if backpack.items.is_empty() {
                                col.spawn(glass::text("(carrying nothing)", 16.0, glass::DIM));
                            }
                            for (kind, qty) in held_potions(&backpack) {
                                let name = meld_proto::consumables::consumable(&kind)
                                    .map(|c| c.name.to_string())
                                    .unwrap_or_else(|| kind.clone());
                                col.spawn(glass::text(
                                    format!("{name}  x{qty}"),
                                    18.0,
                                    glass::TEXT,
                                ));
                                if let Some(def) = meld_proto::consumables::consumable(&kind) {
                                    col.spawn(glass::text(
                                        format!("   {}", def.description),
                                        14.0,
                                        glass::DIM,
                                    ));
                                }
                            }
                        }
                        MenuSection::Materials => {
                            if inv.materials.is_empty() {
                                col.spawn(glass::text("(nothing banked)", 16.0, glass::DIM));
                            }
                            for (kind, n) in &inv.materials {
                                col.spawn((
                                    Button,
                                    WithdrawButton { item_kind: kind.clone() },
                                    glass::chip(false),
                                ))
                                .with_children(|b| {
                                    b.spawn(glass::text(
                                        format!("{kind}  x{n}"),
                                        18.0,
                                        glass::TEXT,
                                    ));
                                });
                            }
                            col.spawn(glass::text(
                                "click a material to take it on your next dive",
                                14.0,
                                glass::DIM,
                            ));
                            col.spawn(glass::divider());
                            col.spawn(glass::text(
                                format!("{} chits", inv.chits),
                                18.0,
                                glass::WARN,
                            ));
                        }
                        MenuSection::Map => {
                            for line in [
                                format!("Distance   {}", stats.distance),
                                format!("Tier       {}", stats.tier),
                                format!("Biome      {}", stats.biome),
                                format!(
                                    "Town portals  {}",
                                    backpack
                                        .items
                                        .iter()
                                        .filter(|(kind, _)| kind == "town_portal")
                                        .map(|(_, q)| *q)
                                        .sum::<i32>()
                                ),
                            ] {
                                col.spawn(glass::text(line, 19.0, glass::TEXT));
                            }
                        }
                        MenuSection::Party => {
                            for (i, h) in heroes.iter().enumerate() {
                                let focused = depth == 1 && menu.cursor == i;
                                let selected = menu.member == i;
                                col.spawn(glass::inset(focused || selected))
                                .with_children(|cell| {
                                    // Sprite on the left half, everything else on the
                                    // right: sharing the vertical space is what keeps
                                    // four heroes on one screen.
                                    cell.spawn(Node {
                                        flex_direction: FlexDirection::Row,
                                        column_gap: Val::Px(10.0),
                                        align_items: AlignItems::Center,
                                        ..default()
                                    })
                                    .with_children(|row| {
                                        // The portrait takes the left of the cell. Kept
                                        // square, because the class sheets are square
                                        // and a rectangle would stretch the figure.
                                        if let Some(w) = wa.as_ref() {
                                            // The class sheets are 188² canvases with a
                                            // lot of transparent margin, so the
                                            // portrait draws a SUB-RECT of the source
                                            // rather than the whole square: the figure
                                            // fills its half of the cell instead of
                                            // floating in a box of nothing, and the
                                            // cell's height stays the text's business.
                                            row.spawn((
                                                ImageNode {
                                                    image: w
                                                        .class_frames(&h.class_key)
                                                        .idle[0]
                                                        .clone(),
                                                    rect: Some(Rect::new(
                                                        44.0, 16.0, 144.0, 172.0,
                                                    )),
                                                    ..default()
                                                },
                                                Node {
                                                    width: Val::Px(96.0),
                                                    height: Val::Px(150.0),
                                                    flex_shrink: 0.0,
                                                    ..default()
                                                },
                                            ));
                                        }
                                        row.spawn(Node {
                                            flex_direction: FlexDirection::Column,
                                            row_gap: Val::Px(2.0),
                                            flex_grow: 1.0,
                                            ..default()
                                        })
                                        .with_children(|txt| {
                                            txt.spawn(glass::text(
                                                format!("{}   Lv {}", h.name, h.level),
                                                21.0,
                                                glass::TEXT,
                                            ));
                                            txt.spawn(glass::text(
                                                class_and_rank(&h.class_key, h.level),
                                                15.0,
                                                glass::TITLE,
                                            ));
                                            txt.spawn(glass::text(
                                                format!("HP  {}/{}", h.hp, h.max_hp),
                                                17.0,
                                                glass::TEXT,
                                            ));
                                            if let Some(res) = hero_resource(&h.class_key) {
                                                txt.spawn(glass::text(res, 15.0, glass::DIM));
                                            }
                                            txt.spawn(glass::text(
                                                format!("EXP {} / {}", h.xp, h.xp_to_next.max(1)),
                                                15.0,
                                                glass::DIM,
                                            ));
                                            txt.spawn(glass::text(
                                                format!(
                                                    "STR {}  MND {}  DEX {}  WLL {}",
                                                    h.str_, h.mnd, h.dex, h.wll
                                                ),
                                                15.0,
                                                glass::DIM,
                                            ));
                                            // Row, Equipment and Abilities share one
                                            // line — three buttons, one row of height.
                                            txt.spawn(Node {
                                                flex_direction: FlexDirection::Row,
                                                column_gap: Val::Px(5.0),
                                                margin: UiRect::top(Val::Px(3.0)),
                                                ..default()
                                            })
                                            .with_children(|btns| {
                                                btns.spawn((
                                                    Button,
                                                    FormationButton {
                                                        slot: i as i32,
                                                        back_row: h.back_row,
                                                    },
                                                    glass::chip_sized(
                                                        h.back_row,
                                                        Val::Px(108.0),
                                                    ),
                                                ))
                                                .with_children(|b| {
                                                    b.spawn(glass::text(
                                                        if h.back_row {
                                                            "Row: Back"
                                                        } else {
                                                            "Row: Front"
                                                        },
                                                        15.0,
                                                        glass::TEXT,
                                                    ));
                                                });
                                                for (pane, label) in [
                                                    (MenuPane::Equipment, "Equipment"),
                                                    (MenuPane::Abilities, "Abilities"),
                                                ] {
                                                    let on = selected && menu.pane == Some(pane);
                                                    btns.spawn((
                                                        Button,
                                                        PaneButton { member: i, pane },
                                                        glass::chip(on),
                                                    ))
                                                    .with_children(|b| {
                                                        b.spawn(glass::text(
                                                            label,
                                                            15.0,
                                                            if on {
                                                                glass::TITLE
                                                            } else {
                                                                glass::TEXT
                                                            },
                                                        ));
                                                    });
                                                }
                                            });
                                        });
                                    });
                                });
                            }
                        }
                    }
                });

                // ---- column three: a hero's gear, or a hero's abilities.
                let Some(pane) = menu.pane else { return };
                let Some(hero) = heroes.get(menu.member) else { return };
                cols.spawn(glass::panel(Val::Px(520.0))).with_children(|col| match pane {
                    MenuPane::Abilities => {
                        col.spawn(glass::text("ABILITIES", 26.0, glass::TITLE));
                        col.spawn(glass::text(
                            class_and_rank(&hero.class_key, hero.level),
                            16.0,
                            glass::DIM,
                        ));
                        col.spawn(glass::divider());
                        // ONLY what the hero has. What comes later, and what it takes,
                        // is theirs to discover.
                        let owned = meld_proto::skills::skills_for_class_at(
                            &hero.class_key,
                            hero.level,
                        );
                        if owned.is_empty() {
                            col.spawn(glass::text("(none yet)", 16.0, glass::DIM));
                        }
                        for (i, def) in owned.iter().enumerate() {
                            let focused = depth == 2 && menu.cursor == i;
                            col.spawn(glass::text(
                                def.name,
                                21.0,
                                if focused { glass::TITLE } else { glass::TEXT },
                            ));
                            col.spawn(glass::text(
                                format!("   {}", def.description),
                                15.0,
                                glass::DIM,
                            ));
                        }
                    }
                    MenuPane::Equipment => {
                        col.spawn(glass::text("EQUIPMENT", 26.0, glass::TITLE));
                        col.spawn(glass::text(hero.name.clone(), 16.0, glass::DIM));
                        col.spawn(glass::divider());
                        equipment_pane(
                            col,
                            menu.member,
                            &hero.class_key,
                            &inv,
                            &run_gear,
                            &picker,
                            depth,
                            menu.cursor,
                        );
                    }
                });
            });
        });
}

/// The class's own combat resource, shown where MP would sit. The ATB adaptation
/// has no cast pool, so a literal "MP 0/0" would be a lie on every hero; each class
/// gets the bar it actually spends instead.
fn hero_resource(class_key: &str) -> Option<String> {
    match class_key {
        // One line each: a wrapped label costs a whole row of cell height, and four
        // cells have to fit one screen.
        "hunter" => Some("Adrenaline".to_string()),
        "psyker" => Some("Focus slots".to_string()),
        "resonant" => Some("Pays in its own HP".to_string()),
        _ => None,
    }
}

/// Column three's Equipment body: the six categories with what is worn, or — once a
/// category is opened — the candidates for it. Both reuse the existing equip flow's
/// buttons, so clicking still routes through the same equip/unequip commands.
#[allow(clippy::too_many_arguments)]
fn equipment_pane(
    col: &mut ChildSpawnerCommands,
    member: usize,
    class_key: &str,
    inv: &InventoryData,
    run_gear: &RunGearData,
    picker: &EquipPicker,
    depth: u8,
    cursor: usize,
) {
    let class = (!class_key.is_empty()).then_some(class_key);
    match picker.category {
        None => {
            for (i, cat) in GEAR_CATEGORIES.iter().enumerate() {
                let worn = category_gear(&inv.gear, cat, member, class)
                    .into_iter()
                    .find(|g| g.equipped_hero_slot == Some(member))
                    .or_else(|| {
                        category_gear(&run_gear.gear, cat, member, class)
                            .into_iter()
                            .find(|g| g.equipped_hero_slot == Some(member))
                    });
                let focused = depth == 2 && cursor == i;
                col.spawn((Button, CategoryButton { category: cat }, glass::chip(focused)))
                    .with_children(|b| {
                        b.spawn(glass::text(
                            format!(
                                "{}   {}",
                                gear_category_label(cat),
                                worn.map(|g| g.name.clone()).unwrap_or_else(|| "-".into())
                            ),
                            18.0,
                            if focused { glass::TITLE } else { glass::TEXT },
                        ));
                    });
            }
            col.spawn(glass::text("[Enter] change  [Esc] back", 14.0, glass::DIM));
        }
        Some(cat) => {
            col.spawn(glass::text(gear_category_label(cat), 19.0, glass::WARN));
            col.spawn((Button, PickerUnequipButton { category: cat }, glass::chip(false)))
                .with_children(|b| {
                    b.spawn(glass::text("Remove", 18.0, glass::TEXT));
                });
            for g in category_gear(&inv.gear, cat, member, class) {
                gear_row(col, g, member, GearSource::Vault, class);
            }
            for g in category_gear(&run_gear.gear, cat, member, class) {
                gear_row(col, g, member, GearSource::RunLoot, class);
            }
            col.spawn((Button, PickerBackButton, glass::chip(false))).with_children(|b| {
                b.spawn(glass::text("Back", 18.0, glass::DIM));
            });
        }
    }
}

/// One candidate row in the equip picker. A piece the class cannot wear renders dim
/// with the reason, rather than being hidden or handed to the server to refuse.
fn gear_row(
    col: &mut ChildSpawnerCommands,
    g: &GearLine,
    member: usize,
    source: GearSource,
    class: Option<&str>,
) {
    let blocked = gear_block_reason(g, class);
    let worn = g.equipped_hero_slot == Some(member);
    col.spawn((
        Button,
        GearButton {
            gear_id: g.gear_id.clone(),
            source,
            target_hero_slot: member,
            worn,
            blocked: blocked.is_some(),
            free_first: None,
        },
        glass::chip(worn),
    ))
    .with_children(|b| {
        b.spawn(glass::text(
            format!("{}  +{}", g.name, gear_slot_stat(g)),
            18.0,
            if blocked.is_some() {
                Color::srgb(0.55, 0.5, 0.5)
            } else {
                rarity_color(&g.name)
            },
        ));
        if let Some(why) = blocked {
            b.spawn(glass::text(format!("   {why}"), 14.0, glass::WARN));
        }
    });
}

/// A gear category's display name.
fn gear_category_label(cat: &str) -> &'static str {
    match cat {
        "main_hand" => "Main hand",
        "off_hand" => "Off hand",
        "head" => "Head",
        "chest" => "Chest",
        "legs" => "Legs",
        _ => "Accessory",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cascade_closes_one_column_at_a_time() {
        let mut m = MainMenu {
            section: Some(MenuSection::Party),
            member: 2,
            pane: Some(MenuPane::Abilities),
            cursor: 4,
        };
        assert_eq!(m.depth(), 2);
        assert!(m.back());
        assert_eq!(m.depth(), 1, "Back should close the third column, not both");
        assert_eq!(m.cursor, 0, "the cursor belongs to the column, not the menu");
        assert_eq!(m.member, 2, "stepping back must not forget which hero");
        assert!(m.back());
        assert_eq!(m.depth(), 0);
        // Nothing left to close: the caller shuts the whole menu.
        assert!(!m.back());
    }

    #[test]
    fn a_rank_rides_beside_the_class_and_a_class_without_an_order_shows_none() {
        assert_eq!(class_and_rank("phoenix_guard", 1), "Phoenix Guard - Initiate");
        assert_eq!(class_and_rank("phoenix_guard", 255), "Phoenix Guard - Apotheosis");
        // The Resonant has no order, so it is just a class — no empty separator.
        assert_eq!(class_and_rank("resonant", 40), class_display("resonant"));
    }
}

/// Keyboard for the cascade: Up/Down walk the deepest open column, Right/Enter step
/// into it, Left/Esc step back out one column (and close the menu from the nav).
pub(crate) fn main_menu_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut overlay: ResMut<Overlay>,
    mut menu: ResMut<MainMenu>,
    mut picker: ResMut<EquipPicker>,
    mut equip_sel: ResMut<EquipSelection>,
    roster: Res<PartyRoster>,
    hero_names: Res<AccountHeroNames>,
    inv: Res<InventoryData>,
    backpack: Res<RunBackpack>,
    mut rename: ResMut<HeroRename>,
) {
    if overlay.kind != Some(OverlayKind::Inventory) || rename.slot.is_some() {
        return;
    }
    let len = column_len(&menu, &roster, &hero_names, &inv, &backpack, &picker).max(1);
    if keys.just_pressed(KeyCode::ArrowDown) {
        menu.cursor = (menu.cursor + 1) % len;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        menu.cursor = (menu.cursor + len - 1) % len;
    }
    if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::ArrowLeft) {
        // A category picker is a step of its own: close it before the column.
        if picker.category.is_some() {
            picker.category = None;
        } else if !menu.back() {
            overlay.kind = None;
        }
        return;
    }
    if keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::ArrowRight)
        || keys.just_pressed(KeyCode::Space)
    {
        match menu.depth() {
            0 => {
                menu.section = MenuSection::ALL.get(menu.cursor).copied();
                menu.cursor = 0;
            }
            1 if menu.section == Some(MenuSection::Party) => {
                // Stepping into a hero opens its gear; Abilities is a click or a
                // second press away.
                menu.member = menu.cursor;
                equip_sel.hero_slot = menu.cursor;
                menu.pane = Some(MenuPane::Equipment);
                menu.cursor = 0;
            }
            2 if menu.pane == Some(MenuPane::Equipment) && picker.category.is_none() => {
                picker.category = GEAR_CATEGORIES.get(menu.cursor).copied();
            }
            _ => {}
        }
    }
    // [R] renames the focused hero: typing is handled by `hero_rename_input`, which
    // owns the buffer.
    if keys.just_pressed(KeyCode::KeyR)
        && menu.section == Some(MenuSection::Party)
        && menu.depth() == 1
    {
        rename.slot = Some(menu.cursor);
        rename.buffer.clear();
    }
    // [A] jumps straight to the focused hero's abilities — the thing a player opens
    // the menu to read.
    if keys.just_pressed(KeyCode::KeyA) && menu.section == Some(MenuSection::Party) {
        menu.member = if menu.depth() == 1 { menu.cursor } else { menu.member };
        menu.pane = Some(MenuPane::Abilities);
        menu.cursor = 0;
    }
}

/// Clicks on the nav rows and the per-hero Equipment/Abilities buttons. The gear
/// rows themselves are handled by the equip flow's own click systems.
pub(crate) fn main_menu_click(
    nav: Query<(&Interaction, &NavButton), Changed<Interaction>>,
    panes: Query<(&Interaction, &PaneButton), Changed<Interaction>>,
    mut menu: ResMut<MainMenu>,
    mut picker: ResMut<EquipPicker>,
    mut equip_sel: ResMut<EquipSelection>,
) {
    for (interaction, NavButton(section)) in &nav {
        if *interaction == Interaction::Pressed {
            // Clicking the open section again folds it away, so the nav doubles as
            // the way back out.
            if menu.section == Some(*section) {
                menu.section = None;
                menu.pane = None;
            } else {
                menu.section = Some(*section);
                menu.pane = None;
            }
            menu.cursor = 0;
            picker.category = None;
        }
    }
    for (interaction, btn) in &panes {
        if *interaction == Interaction::Pressed {
            menu.member = btn.member;
            equip_sel.hero_slot = btn.member;
            menu.pane = Some(btn.pane);
            menu.cursor = 0;
            picker.category = None;
        }
    }
}

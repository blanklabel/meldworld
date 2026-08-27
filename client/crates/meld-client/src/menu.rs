//! The main menu: **three columns that cascade left**, in the manner of the Dragon
//! Quest remakes.
//!
//! Column one is the nav — *Items, Materials, Party, Map, Guide*. Choosing one opens column
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

use meld_client::glass;

use super::*;

/// Which nav row is open (column two's content).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MenuSection {
    Items,
    Materials,
    Party,
    Map,
    Quests,
    Guide,
}

impl MenuSection {
    pub(crate) const ALL: [MenuSection; 6] = [
        MenuSection::Items,
        MenuSection::Materials,
        MenuSection::Party,
        MenuSection::Map,
        MenuSection::Quests,
        MenuSection::Guide,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            MenuSection::Items => "Party Inventory",
            MenuSection::Materials => "Materials",
            MenuSection::Party => "Party",
            MenuSection::Map => "Map",
            MenuSection::Quests => "Quests",
            MenuSection::Guide => "Guide",
        }
    }

    /// The account unlock this section waits on, or `None` for the ones everyone has.
    ///
    /// **Quests** is the Den's board, so it appears with the Hunter (CL-1) rather than
    /// sitting there greyed out — nothing in this menu advertises what you have not
    /// earned, which is the rule the whole panel is built on.
    pub(crate) fn requires(self) -> Option<&'static str> {
        match self {
            MenuSection::Quests => Some("class_hunter"),
            _ => None,
        }
    }
}

/// The nav rows this account can actually see, in order.
pub(crate) fn visible_sections(owned: &[String]) -> Vec<MenuSection> {
    MenuSection::ALL
        .iter()
        .copied()
        .filter(|s| match s.requires() {
            Some(key) => owned.iter().any(|k| k == key),
            None => true,
        })
        .collect()
}

/// The controls, as `(heading, rows)`. This is where they live now: a permanent
/// on-screen control list is noise a player stops reading on the second dive, but it
/// still has to be findable somewhere that is not the README.
const GUIDE: [(&str, &[(&str, &str)]); 3] = [
    (
        "Moving",
        &[
            ("WASD / arrows", "walk"),
            ("drag", "thumbstick"),
            ("tap the ground", "go there"),
        ],
    ),
    (
        "Acting",
        &[
            ("[E] / Interact", "gather, open, descend, extract, join a fight"),
            ("[E] again", "stop channelling"),
            ("[V]", "WATCH a fight nearby without joining it"),
            ("[N]", "ask the bench in reach for its boon"),
            ("[C] / [I] / tap yourself", "this menu"),
            ("[P]", "show the party behind you"),
        ],
    ),
    (
        "In this menu",
        &[
            ("up / down", "move"),
            ("right / Enter", "open"),
            ("left / Esc", "back out"),
            ("[A]", "the focused hero's abilities"),
            ("[R]", "rename the focused hero"),
        ],
    ),
];

/// Which per-hero pane column three is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MenuPane {
    Equipment,
    Abilities,
    /// "Who drinks it?" — the hero picker the Items column opens once a potion is
    /// chosen. The only third column that hangs off something other than a hero.
    UseOn,
}

/// The cascade's state. `section` opens column two, `pane` opens column three for
/// hero `member`; `cursor` walks the deepest open column.
#[derive(Resource, Default)]
pub(crate) struct MainMenu {
    pub(crate) section: Option<MenuSection>,
    pub(crate) member: usize,
    pub(crate) pane: Option<MenuPane>,
    pub(crate) cursor: usize,
    /// Which potion the [`MenuPane::UseOn`] picker is about to spend.
    pub(crate) item_kind: Option<String>,
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
            self.item_kind = None;
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

/// The Map column's "Return to town" row — spends a Town Portal item.
#[derive(Component)]
pub(crate) struct ReturnToTownButton;

/// Tapping this raises a field station of `kind` where you stand (MS-1).
#[derive(Component)]
pub(crate) struct BuildStationButton {
    pub kind: &'static str,
}

/// A "raise a structure" row in the Map column — one component for every function,
/// because there is one primitive (CANON D21/§W3).
#[derive(Component, Clone, Copy)]
pub(crate) struct BuildStructureButton {
    pub function: &'static str,
}

/// A potion row in the Items column — clicking it opens the hero picker.
#[derive(Component)]
pub(crate) struct UseItemButton {
    pub(crate) item_kind: String,
}

/// A hero row in the [`MenuPane::UseOn`] picker: the one who drinks it.
#[derive(Component)]
pub(crate) struct UseOnHeroButton {
    pub(crate) slot: usize,
}

/// Hand the staged item to this hero's pouch, so they can reach it in a fight.
#[derive(Component)]
pub(crate) struct GiveToHeroButton {
    pub(crate) slot: usize,
}

/// Put one of a hero's pouch items back into the Party Inventory.
#[derive(Component)]
pub(crate) struct TakeBackButton {
    pub(crate) slot: usize,
    pub(crate) item_kind: String,
}

/// The potions sitting in the PARTY INVENTORY (not in anyone's pouch).
///
/// The battle screen's [`held_potions`] is the pouch-side twin of this: the two lists
/// are disjoint on purpose, because an item is in exactly one container and which one
/// decides whether a hero can reach it mid-fight.
pub(crate) fn inventory_potions(backpack: &RunBackpack) -> Vec<(String, i32)> {
    backpack
        .items
        .iter()
        .filter(|(kind, qty)| *qty > 0 && meld_proto::consumables::is_consumable(kind))
        .cloned()
        .collect()
}

/// Whether a potion does anything OUTSIDE a fight. Barrier/Regen/Evasion/Adrenaline
/// are combat state that would be gone by the next encounter, so the server refuses
/// them out here (`run.use_item`). It gates the **DRINK NOW** rows only — every potion
/// can still be GIVEN to a hero, since a fight-only potion is exactly the kind that
/// needs to be in a pouch. Kept in step with the server's own match.
fn usable_in_field(item_kind: &str) -> bool {
    use meld_proto::consumables::ConsumableEffect as E;
    matches!(
        meld_proto::consumables::consumable(item_kind).map(|c| c.effect),
        Some(E::Heal | E::FullHeal | E::Revive | E::Experience)
    )
}

/// Tapping this asks the SERVER to dress this hero from the spare gear (GR-5).
#[derive(Component, Clone, Copy)]
pub(crate) struct EquipBestButton {
    pub member: usize,
}

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
            afflictions: Vec::new(),
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

/// The Quests column: the Den's standing contracts, then everything already settled.
///
/// Reading only. A contract is paid at the Bounty Board in town, so a finished one says so
/// rather than offering a button that would hand you power in the middle of a run.
fn quest_column(col: &mut ChildSpawnerCommands, bounties: &BountyData) {
    if !bounties.loaded {
        col.spawn(glass::text("The Den is checking its ledger...", 16.0, glass::DIM));
        return;
    }
    col.spawn(glass::text(
        format!("Hunter rank {} - {}", bounties.rank, bounties.rank_title),
        17.0,
        glass::TITLE,
    ));
    col.spawn(glass::text(
        format!("{} XP to the next rank", bounties.rank_xp_to_next),
        14.0,
        glass::DIM,
    ));
    col.spawn(glass::divider());

    col.spawn(glass::text("Standing", 17.0, glass::TITLE));
    if bounties.active.is_empty() {
        col.spawn(glass::text("Nothing posted for you.", 15.0, glass::DIM));
    }
    for b in &bounties.active {
        let done = b.state == "completed";
        col.spawn(glass::text(
            format!("{}{}", b.mark_name, if done { "  - felled" } else { "" }),
            16.0,
            if done { glass::TITLE } else { glass::TEXT },
        ));
        col.spawn(glass::text(format!("   {}", b.where_to_look), 14.0, glass::DIM));
        let reward = match (b.reward_material_qty > 0, b.reward_gear) {
            (true, true) => format!(
                "   Pays {}c, {} {} and a piece of gear",
                b.reward_chits,
                b.reward_material_qty,
                crate::icons::display_name(&b.reward_material)
            ),
            (true, false) => format!(
                "   Pays {}c and {} {}",
                b.reward_chits,
                b.reward_material_qty,
                crate::icons::display_name(&b.reward_material)
            ),
            _ => format!("   Pays {}c", b.reward_chits),
        };
        col.spawn(glass::text(reward, 14.0, glass::DIM));
        col.spawn(glass::text(
            if done {
                "   Claim it at the Bounty Board.".to_string()
            } else {
                format!(
                    "   {}x a standard creature at that depth   -   {}",
                    format_power(b.power),
                    remaining(b.expires_in_secs)
                )
            },
            14.0,
            glass::DIM,
        ));
    }
    col.spawn(glass::divider());
    col.spawn(glass::text("Settled", 17.0, glass::TITLE));
    if bounties.history.is_empty() {
        col.spawn(glass::text("Nothing yet.", 15.0, glass::DIM));
    }
    for b in bounties.history.iter().take(12) {
        let tail = if b.state == "claimed" { "paid" } else { "withdrawn" };
        col.spawn(glass::text(format!("{} - {tail}", b.mark_name), 15.0, glass::DIM));
    }
}

/// A power multiplier as a player reads it: one decimal, and no trailing `.0`.
fn format_power(power: f64) -> String {
    let s = format!("{power:.1}");
    s.trim_end_matches(".0").to_string()
}

/// How long a contract has left, in the coarsest unit that is still true.
fn remaining(secs: i64) -> String {
    if secs <= 0 {
        return "withdrawn".to_string();
    }
    let hours = secs / 3600;
    if hours >= 1 {
        format!("{hours}h left")
    } else {
        format!("{}m left", (secs / 60).max(1))
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
    owned: &[String],
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
            // The six slots plus the "Equip best" row under them.
            None => GEAR_CATEGORIES.len() + 1,
        },
        // The pane lists every hero twice for a field-usable potion (give, then drink)
        // and once for a fight-only one, which has nothing to drink.
        (Some(MenuSection::Items), Some(MenuPane::UseOn)) => {
            let heroes = party_lines(roster, names).len();
            let field = menu.item_kind.as_deref().is_some_and(usable_in_field);
            if field { heroes * 2 } else { heroes }
        }
        (Some(MenuSection::Party), None) => party_lines(roster, names).len(),
        (Some(MenuSection::Items), None) => inventory_potions(backpack).len().max(1),
        (Some(MenuSection::Materials), None) => inv.materials.len().max(1),
        // Return to town, the two field stations, then ONE PER STRUCTURE from the registry.
        // This was a literal `3`, so the keyboard could never reach a "Raise a ..." row —
        // every buildable in `meld_proto::structures` was mouse-only, silently. A count
        // written by hand is a count a new row gets left out of.
        (Some(MenuSection::Map), None) => 3 + meld_proto::structures::STRUCTURES.len(),
        // Reading only — the cursor has nothing to land on.
        (Some(MenuSection::Guide), None) => 0,
        // Reading only, both columns: the board is a log, and the reward is taken at the
        // Bounty Board in town rather than from a menu mid-run.
        (Some(MenuSection::Quests), None) => 0,
        (None, _) => visible_sections(owned).len(),
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
    // Grouped as one tuple param to stay inside Bevy's 16-param system limit — the same
    // reason `netglue`'s world resources travel together.
    reads: (
        Res<PerksRes>,
        Res<crate::overworld::ExploredMap>,
        Res<Notice>,
        Res<UnlocksRes>,
        Res<BountyData>,
    ),
    wa: Option<Res<WorldAssets>>,
    ground: Option<Res<crate::minimap::MinimapTiles>>,
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
        || backpack.is_changed()
        || reads.3.is_changed()
        || reads.4.is_changed()
        || reads.2.is_changed())
    {
        return;
    }
    let (perks, explored, notice, unlocks, bounties) =
        (&*reads.0, &*reads.1, &*reads.2, &*reads.3, &*reads.4);
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
            root.spawn(glass::columns()).with_children(|cols| {
                // ---- column one: the nav. Always present, so you can always see
                // where you are and step back out.
                cols.spawn(glass::column(glass::COL_NAV)).with_children(|nav| {
                    nav.spawn(glass::text("MENU", 26.0, glass::TITLE));
                    nav.spawn(glass::divider());
                    for (i, s) in visible_sections(&unlocks.owned).iter().enumerate() {
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

                // ---- column two: whatever the nav opened. Its SLOT is spawned either
                // way — an empty column keeps the geometry, so opening a section does not
                // shove the nav sideways under the cursor that just clicked it.
                let Some(section) = menu.section else {
                    cols.spawn(glass::column_empty(glass::COL_MAIN));
                    cols.spawn(glass::column_empty(glass::COL_DETAIL));
                    return;
                };
                cols.spawn(glass::column(glass::COL_MAIN)).with_children(|col| {
                    col.spawn(glass::text(section.label().to_uppercase(), 26.0, glass::TITLE));
                    col.spawn(glass::divider());
                    match section {
                        MenuSection::Items => {
                            col.spawn(glass::text(
                                "shared — a hero cannot reach this in a fight",
                                14.0,
                                glass::DIM,
                            ));
                            let shared = inventory_potions(&backpack);
                            if shared.is_empty() {
                                col.spawn(glass::text("(carrying nothing)", 16.0, glass::DIM));
                            }
                            for (i, (kind, qty)) in shared.iter().enumerate() {
                                let name = meld_proto::consumables::consumable(kind)
                                    .map(|c| c.name.to_string())
                                    .unwrap_or_else(|| kind.clone());
                                let focused = depth == 1 && menu.cursor == i;
                                let chosen = menu.item_kind.as_deref() == Some(kind.as_str());
                                col.spawn((
                                    Button,
                                    UseItemButton { item_kind: kind.clone() },
                                    glass::inset(focused || chosen),
                                ))
                                .with_children(|row| {
                                    crate::icons::spawn_icon(row, wa.as_deref(), kind, 24.0);
                                    row.spawn(glass::text(
                                        format!("{name}  x{qty}"),
                                        18.0,
                                        glass::TEXT,
                                    ));
                                });
                            }
                            if !shared.is_empty() {
                                col.spawn(glass::text(
                                    "pick one, then give it to a hero or have them drink it",
                                    14.0,
                                    glass::DIM,
                                ));
                            }
                            // The pouches, read-only here: this is the answer to "who is
                            // carrying the heals", which is the question the split exists
                            // to make you ask before you walk into something.
                            col.spawn(glass::divider());
                            col.spawn(glass::text("POUCHES", 18.0, glass::TITLE));
                            let cap = backpack.pouch_capacity;
                            for (slot, h) in party_lines(&roster, &hero_names).iter().enumerate() {
                                let pouch = backpack.pouch(slot);
                                col.spawn(glass::text(
                                    format!("{}   {}/{cap}", h.name, pouch.len()),
                                    17.0,
                                    glass::TEXT,
                                ));
                                if pouch.is_empty() {
                                    col.spawn(glass::text("   (empty)", 14.0, glass::DIM));
                                }
                                for (kind, qty) in pouch {
                                    let name = meld_proto::consumables::consumable(kind)
                                        .map(|c| c.name.to_string())
                                        .unwrap_or_else(|| kind.clone());
                                    col.spawn((
                                        Button,
                                        TakeBackButton { slot, item_kind: kind.clone() },
                                        glass::row_chip(false),
                                    ))
                                    .with_children(|row| {
                                        crate::icons::spawn_icon(row, wa.as_deref(), kind, 22.0);
                                        row.spawn(glass::text(
                                            format!("{name} x{qty}   - take back"),
                                            15.0,
                                            glass::DIM,
                                        ));
                                    });
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
                                    glass::row_chip(false),
                                ))
                                .with_children(|b| {
                                    crate::icons::spawn_icon(b, wa.as_deref(), kind, 26.0);
                                    b.spawn(glass::text(
                                        format!(
                                            "{}  x{n}",
                                            crate::icons::display_name(kind)
                                        ),
                                        18.0,
                                        glass::TEXT,
                                    ));
                                });
                            }
                            col.spawn(glass::text(
                                "click a material to take it on your next run",
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
                            // The centre column is the MAP. It used to sit under a stack of
                            // action rows — a portal, two benches and one row per buildable —
                            // and got whatever vertical space was left, which was not enough
                            // to read a route off. The rows it shared with are choices ABOUT
                            // the place, so they belong in the detail column beside it, the
                            // way a hero's equipment sits beside the hero.
                            for line in [
                                format!("Distance   {}", stats.distance),
                                format!("Tier       {}", stats.tier),
                                format!("Biome      {}", stats.biome),
                            ] {
                                col.spawn(glass::text(line, 19.0, glass::TEXT));
                            }
                            // **WHICH WORLD THIS IS** (CANON D19: the overworld is a
                            // *player-seeded* World, and §W5 stores this number instead of a
                            // map because the baseline is a pure function of it). It belongs
                            // on the Map column rather than as a seventh nav row: it is a
                            // fact about the place the map is OF.
                            //
                            // Read off the world, never off what we asked for — a client
                            // showing a requested seed instead of the live one is the bug
                            // `run.started.tutorial` exists to prevent.
                            //
                            // Shown in FULL: it is the name a player says out loud to bring
                            // somebody else here, so a truncated or prettified form would be
                            // a name that does not work.
                            let seed = crate::world_render::world_seed();
                            if seed != 0 {
                                col.spawn(glass::text(
                                    format!("World      {seed}"),
                                    19.0,
                                    glass::TEXT,
                                ));
                            }
                            col.spawn(glass::divider());
                            explored_map(col, perks, explored, ground.as_deref());
                        }
                        MenuSection::Quests => {
                            quest_column(col, bounties);
                        }
                        MenuSection::Guide => {
                            for (heading, rows) in GUIDE {
                                col.spawn(glass::text(heading, 17.0, glass::TITLE));
                                for (key, what) in rows {
                                    // Two cells, not one padded string: a description
                                    // long enough to wrap has to wrap under ITSELF,
                                    // not back under the key column.
                                    col.spawn(Node {
                                        flex_direction: FlexDirection::Row,
                                        column_gap: Val::Px(12.0),
                                        padding: UiRect::left(Val::Px(10.0)),
                                        ..default()
                                    })
                                    .with_children(|row| {
                                        row.spawn((
                                            Node { width: Val::Px(190.0), flex_shrink: 0.0, ..default() },
                                            glass::text(*key, 16.0, glass::TEXT),
                                        ));
                                        row.spawn((
                                            Node { width: Val::Px(260.0), ..default() },
                                            glass::text(*what, 16.0, glass::DIM),
                                        ));
                                    });
                                }
                                col.spawn(glass::divider());
                            }
                            // The one control a player cannot discover by pressing
                            // things, because it deliberately has no key: a Town
                            // Portal is an item, so leaving is a choice on the Map
                            // column rather than a hotkey.
                            col.spawn(glass::text(
                                "Going home is not a key \u{2014} it costs a Town Portal, on the Map column.",
                                15.0,
                                glass::DIM,
                            ));
                            col.spawn(glass::text(
                                "Walking west into the city is the free way back.",
                                15.0,
                                glass::DIM,
                            ));
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
                                            // Being DOWN, or carrying something that will
                                            // not wear off, belongs on the same line as the
                                            // name — a hero at 0 HP read as an ordinary
                                            // row, and an affliction (which never expires
                                            // out of combat) was invisible everywhere
                                            // outside the battle screen.
                                            let state = h.condition_label();
                                            txt.spawn(glass::text(
                                                if state.is_empty() {
                                                    format!("{}   Lv {}", h.name, h.level)
                                                } else {
                                                    format!(
                                                        "{}   Lv {}   {}",
                                                        h.name, h.level, state
                                                    )
                                                },
                                                21.0,
                                                if state.is_empty() {
                                                    glass::TEXT
                                                } else {
                                                    glass::WARN
                                                },
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

                // ---- column three. The Map's own choices live here, beside the map they
                // act on; everything else uses it for a hero's gear or abilities.
                if menu.section == Some(MenuSection::Map) {
                    cols.spawn(glass::column(glass::COL_DETAIL)).with_children(|col| {
                        map_actions(col, &menu, &backpack, &roster, depth, wa.as_deref());
                    });
                    return;
                }
                // ---- column three: a hero's gear, or a hero's abilities. Also always a
                // slot, for the same reason.
                let (Some(pane), Some(hero)) = (menu.pane, heroes.get(menu.member)) else {
                    cols.spawn(glass::column_empty(glass::COL_DETAIL));
                    return;
                };
                cols.spawn(glass::column(glass::COL_DETAIL)).with_children(|col| match pane {
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
                            // The prose says what KIND of thing it is; this says how
                            // much. Without it the ladder is unreadable — Power Strike
                            // and Frenzy differ only in numbers nobody could see.
                            let fx = roster.effect(def.key);
                            if !fx.is_empty() {
                                col.spawn(glass::text(format!("   {fx}"), 14.0, glass::TITLE));
                            }
                        }
                    }
                    MenuPane::UseOn => {
                        let kind = menu.item_kind.clone().unwrap_or_default();
                        let kind_ref = (!kind.is_empty()).then_some(kind.as_str());
                        let field_usable = usable_in_field(&kind);
                        let def = meld_proto::consumables::consumable(&kind);
                        col.spawn(glass::text("WHO CARRIES IT?", 26.0, glass::TITLE));
                        col.spawn(glass::text(
                            def.map(|c| c.name.to_string()).unwrap_or_else(|| kind.clone()),
                            16.0,
                            glass::DIM,
                        ));
                        col.spawn(glass::divider());
                        // A revive aims at the FALLEN and every other potion at the
                        // living, so the two lists are disjoint — showing the wrong
                        // half as clickable would only earn a rejection.
                        let reviving = def
                            .map(|c| {
                                c.effect == meld_proto::consumables::ConsumableEffect::Revive
                            })
                            .unwrap_or(false);
                        // GIVE comes first and is offered for EVERY potion, including the
                        // fight-only ones: handing a Bulwark Tonic to whoever will need it
                        // is the whole point of a pouch, and it is the only thing you can
                        // do with a potion that does nothing out here.
                        col.spawn(glass::text("GIVE TO", 16.0, glass::TITLE));
                        let cap = backpack.pouch_capacity;
                        for (i, h) in heroes.iter().enumerate() {
                            let pouch = backpack.pouch(i);
                            let carried = pouch
                                .iter()
                                .find(|(k, _)| Some(k.as_str()) == kind_ref)
                                .map_or(0, |(_, q)| *q);
                            let room = pouch.len() < cap as usize || carried > 0;
                            let focused = depth == 2 && menu.cursor == i;
                            col.spawn((
                                Button,
                                GiveToHeroButton { slot: i },
                                glass::inset(focused),
                            ))
                            .with_children(|row| {
                                let carrying =
                                    if carried > 0 { format!("  (has {carried})") } else { String::new() };
                                row.spawn(glass::text(
                                    format!("{}   {}/{cap}{carrying}", h.name, pouch.len()),
                                    18.0,
                                    if room { glass::TEXT } else { glass::DIM },
                                ));
                            });
                        }
                        if !field_usable {
                            col.spawn(glass::text(
                                "only works in a fight - give it to whoever will need it",
                                14.0,
                                glass::DIM,
                            ));
                        }
                        if field_usable {
                            col.spawn(glass::divider());
                            col.spawn(glass::text("DRINK NOW", 16.0, glass::TITLE));
                            for (i, h) in heroes.iter().enumerate() {
                                let down = h.hp <= 0;
                                let full = h.hp >= h.max_hp && h.max_hp > 0;
                                let ok = if reviving { down } else { !down && !full };
                                let focused = depth == 2 && menu.cursor == heroes.len() + i;
                                col.spawn((
                                    Button,
                                    UseOnHeroButton { slot: i },
                                    glass::inset(focused),
                                ))
                                .with_children(|row| {
                                    row.spawn(glass::text(
                                        format!("{}   {}/{}", h.name, h.hp.max(0), h.max_hp),
                                        18.0,
                                        if ok { glass::TEXT } else { glass::DIM },
                                    ));
                                });
                            }
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
                            notice,
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

/// The Map column's CHOICES: go home, raise a bench, raise a structure — one per row, in
/// the detail column beside the map.
///
/// Split out of the centre column when the map moved into it. Every row here spends
/// something you are carrying, which is why none of them is a hotkey: the primary way out of
/// a dive belongs somewhere a player can find it.
/// **One row of the Map column, as data.** What it does, what it says, and whether it is
/// live — decided without a `Commands` in sight.
///
/// The repo already does this for the town counters (`CounterView`): rows as data is what
/// lets each be its own tappable chip. Here it buys something else as well — the row logic
/// becomes TESTABLE. It needed to be: the build rows asked for ORE on every structure for a
/// whole release, and nothing could catch it because the decision lived inside a
/// `with_children` closure that only a running game exercises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MapRowKind {
    ReturnToTown,
    /// A field bench (MS-1). Class-gated: a forge is a Smithwright's, a still a Keeper's.
    Station(&'static str),
    /// A player-built structure (BD-2). **Not** class-gated — see
    /// `raising_a_structure_is_not_locked_behind_a_profession_class`.
    Structure(&'static str),
}

#[derive(Debug, Clone)]
pub(crate) struct MapRow {
    pub(crate) kind: MapRowKind,
    pub(crate) label: String,
    /// Live (bright) vs unavailable (dim). The button is spawned either way — a row you
    /// cannot use still has to say WHY, which is the whole point of the label.
    pub(crate) live: bool,
    /// A second line under the row, where the registry has one.
    pub(crate) detail: Option<&'static str>,
}

/// Every row the Map column shows, in order, for this backpack and party.
///
/// Takes the party's CLASS KEYS rather than the whole roster: the only question it asks of
/// the party is "is there a Smithwright / a Keeper in it", and narrowing the argument to
/// that is what lets it be tested without constructing twenty fields of hero.
pub(crate) fn map_rows(backpack: &RunBackpack, party_classes: &[&str]) -> Vec<MapRow> {
    let mut rows = Vec::new();

    let portals = backpack.count("town_portal");
    rows.push(MapRow {
        kind: MapRowKind::ReturnToTown,
        label: if portals > 0 {
            format!("Return to town   ({portals})")
        } else {
            "Return to town   (none held)".to_string()
        },
        live: portals > 0,
        detail: None,
    });

    // A forge is a Smithwright's bench and a still is a Keeper's, so the row says who is
    // MISSING rather than offering work nobody in this party can do.
    for kind in ["smith", "alembic"] {
        let what = if kind == "smith" {
            ("smith station", "ore", "Smithwright", "smithwright")
        } else {
            ("Keeper's still", "reagents", "Keeper", "keeper")
        };
        let have_builder = party_classes.contains(&what.3);
        let stock = carried_for(backpack, kind);
        let (label, live) = if !have_builder {
            (format!("Set up a {}   (needs a {} in the party)", what.0, what.2), false)
        } else {
            match stock {
                Some((k, qty)) => (format!("Set up a {}   ({qty} {k})", what.0), true),
                None => (format!("Set up a {}   (no {} carried)", what.0, what.1), false),
            }
        };
        rows.push(MapRow { kind: MapRowKind::Station(kind), label, live, detail: None });
    }

    // The structures, from the REGISTRY rather than a list here: a new function is a row in
    // `meld_proto::structures`, and a hand-written list is a list a function gets left off.
    for def in meld_proto::structures::STRUCTURES {
        let (label, tint) = build_row(def, backpack);
        rows.push(MapRow {
            kind: MapRowKind::Structure(def.key),
            label,
            live: tint == glass::TEXT,
            detail: Some(def.description),
        });
    }
    rows
}

fn map_actions(
    col: &mut ChildSpawnerCommands,
    menu: &MainMenu,
    backpack: &RunBackpack,
    roster: &PartyRoster,
    depth: u8,
    wa: Option<&WorldAssets>,
) {
    let _ = wa;
    col.spawn(glass::text("HERE".to_string(), 26.0, glass::TITLE));
    col.spawn(glass::text(
        "what you can do where you stand".to_string(),
        13.0,
        glass::DIM,
    ));
    col.spawn(glass::divider());
    // Rendering only: every decision above, in `map_rows`.
    let classes: Vec<&str> = roster.heroes.iter().map(|h| h.class_key.as_str()).collect();
    for (i, row) in map_rows(backpack, &classes).into_iter().enumerate() {
        let focused = depth == 1 && menu.cursor == i;
        let tint = if row.live { glass::TEXT } else { glass::DIM };
        let mut ent = col.spawn((Button, glass::inset(focused)));
        match row.kind {
            MapRowKind::ReturnToTown => {
                ent.insert(ReturnToTownButton);
            }
            MapRowKind::Station(kind) => {
                ent.insert(BuildStationButton { kind });
            }
            MapRowKind::Structure(function) => {
                ent.insert(BuildStructureButton { function });
            }
        }
        ent.with_children(|r| {
            r.spawn(glass::text(row.label, 19.0, tint));
        });
        if let Some(d) = row.detail {
            col.spawn(glass::text(d, 14.0, glass::DIM));
        }
    }
}

/// The Map column's map: everywhere this dive has been, and the landmarks it saw on
/// the way. It is the EXPLORER's — the order whose creed is "a world known" carries
/// the map, so without one in the party the column keeps its readouts and says why
/// there is nothing to look at.
fn explored_map(
    col: &mut ChildSpawnerCommands,
    perks: &PerksRes,
    explored: &crate::overworld::ExploredMap,
    ground: Option<&crate::minimap::MinimapTiles>,
) {
    use crate::overworld::{landmark_color, map_bounds, map_to_px, MAP_CELL};

    if perks.0.explorer_map == 0 {
        col.spawn(glass::text(
            "No map. An Explorer in the party keeps one.",
            16.0,
            glass::DIM,
        ));
        return;
    }
    if !explored.walked {
        col.spawn(glass::text("Nothing walked yet.", 16.0, glass::DIM));
        return;
    }
    const W: f32 = 460.0;
    const H: f32 = 260.0;
    let bounds = map_bounds(explored);
    // One cell's footprint in pixels, floored to a visible minimum: early in a dive
    // the walked rectangle is tiny and the scale enormous, and late in a dive it is
    // the other way round — a fixed dot size would be a smear at one end and
    // invisible at the other.
    let step = {
        let (sx, sy) = (bounds.2 - bounds.0, bounds.3 - bounds.1);
        let scale = (W / sx.max(MAP_CELL)).min(H / sy.max(MAP_CELL));
        (MAP_CELL * scale).clamp(2.0, 14.0)
    };
    let plot = |p: &mut ChildSpawnerCommands, x: f32, y: f32, size: f32, color: Color| {
        let (px, py) = map_to_px(x, y, bounds, W, H);
        p.spawn((
            Node {
                border_radius: BorderRadius::all(Val::Px(size / 2.0)),
                position_type: PositionType::Absolute,
                left: Val::Px(px - size / 2.0),
                top: Val::Px(py - size / 2.0),
                width: Val::Px(size),
                height: Val::Px(size),
                ..default()
            },
            BackgroundColor(color),
        ));
    };
    col.spawn((
        Node {
            width: Val::Px(W),
            height: Val::Px(H),
            position_type: PositionType::Relative,
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(Color::srgba(0.05, 0.08, 0.12, 0.55)),
    ))
    .with_children(|panel| {
        // The GROUND, under everything: biome, coast, water and terrace height, drawn by
        // `minimap::repaint` into its own texture through its own camera. It covers the
        // whole panel at the same scale `map_to_px` fits the walk to, so a tile and the
        // dot on top of it name the same place.
        if let Some(g) = ground {
            panel.spawn((
                ImageNode::new(g.image.clone()),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ));
        }
        for (cx, cy) in &explored.visited {
            let (x, y) = (*cx as f32 * MAP_CELL, *cy as f32 * MAP_CELL);
            // Walked ground is a light WASH now rather than the map itself — the terrain
            // beneath it is the picture, and this only says "you have been here".
            plot(panel, x, y, step, Color::srgba(0.75, 0.88, 1.0, 0.16));
        }
        for ((cx, cy), what) in &explored.seen {
            let (x, y) = (*cx as f32 * MAP_CELL, *cy as f32 * MAP_CELL);
            plot(panel, x, y, (step * 1.6).max(5.0), landmark_color(*what));
        }
        let (hx, hy) = explored.here;
        plot(panel, hx, hy, (step * 1.8).max(7.0), Color::WHITE);
    });
    col.spawn(glass::text(map_legend(perks, explored), 15.0, glass::DIM));
}

/// What the map is showing, in words: how much ground it covers and which landmark
/// classes this party's perks let it plot at all.
pub(crate) fn map_legend(
    perks: &PerksRes,
    explored: &crate::overworld::ExploredMap,
) -> String {
    let mut plots = vec!["portal"];
    if perks.0.explorer_map >= 2 {
        plots.push("chests");
    }
    if perks.0.explorer_map >= 3 {
        plots.push("nodes");
    }
    if perks.0.shifter_dungeon_radius > 0.0 {
        plots.push("doors");
    }
    format!(
        "{} cells walked, {} landmark(s) - plots {}",
        explored.visited.len(),
        explored.seen.len(),
        plots.join(", ")
    )
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
    notice: &Notice,
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
            // One press to dress the hero from the SPARE gear. The server picks (it owns
            // every legality rule already), so this row only has to ask.
            col.spawn((Button, EquipBestButton { member }, glass::chip(depth == 2 && cursor == GEAR_CATEGORIES.len())))
                .with_children(|b| {
                    b.spawn(glass::text("Equip best   [B]", 18.0, glass::WARN));
                });
            col.spawn(glass::text("[Enter] change  [B] equip best  [Esc] back", 14.0, glass::DIM));
            // Whatever the last Vault write said, at the press's own elbow. A refusal that
            // only reaches the overworld HUD behind this panel is a refusal nobody reads,
            // and "nothing happened" is indistinguishable from a dead button.
            if !notice.text.is_empty() {
                col.spawn(glass::text(notice.text.clone(), 14.0, glass::WARN));
            }
        }
        Some(cat) => {
            col.spawn(glass::text(gear_category_label(cat), 19.0, glass::WARN));
            col.spawn((Button, PickerUnequipButton { category: cat }, glass::chip(false)))
                .with_children(|b| {
                    b.spawn(glass::text("Remove", 18.0, glass::TEXT));
                });
            for g in category_gear(&inv.gear, cat, member, class) {
                gear_row(col, g, member, GearSource::Vault, class, &inv.gear);
            }
            for g in category_gear(&run_gear.gear, cat, member, class) {
                gear_row(col, g, member, GearSource::RunLoot, class, &run_gear.gear);
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
    // Everything this hero could be wearing, so a two-hander knows what it displaces.
    worn_pool: &[GearLine],
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
            // GR-5: a two-hander puts the off-hand away first rather than bouncing the
            // player off a 409. This was hardcoded `None` here, so the rule only existed
            // in a helper nothing called and a test nobody noticed was the sole caller.
            free_first: off_hand_in_the_way(worn_pool, g, member),
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
    unlocks: Res<UnlocksRes>,
    mut rename: ResMut<HeroRename>,
    net: NonSend<NetRes>,
) {
    if overlay.kind != Some(OverlayKind::Inventory) || rename.slot.is_some() {
        return;
    }
    let len =
        column_len(&menu, &roster, &hero_names, &inv, &backpack, &picker, &unlocks.owned).max(1);
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
                menu.section = visible_sections(&unlocks.owned).get(menu.cursor).copied();
                menu.cursor = 0;
            }
            1 if menu.section == Some(MenuSection::Map) && menu.cursor >= 1 => {
                // The server checks the skill level and takes the stock; the client only
                // declines to ask when there is plainly nothing to build from.
                let kind = if menu.cursor == 1 { "smith" } else { "alembic" };
                if carried_for(&backpack, kind).is_some() {
                    net.0.send(ClientCmd::BuildStation { kind: kind.into() });
                    overlay.kind = None;
                }
            }
            1 if menu.section == Some(MenuSection::Map) && menu.cursor == 0
                // Explicit, and only when you actually hold one.
                && backpack.count("town_portal") > 0 => {
                    net.0.send(ClientCmd::TownPortal);
                    overlay.kind = None;
                }
            1 if menu.section == Some(MenuSection::Items) => {
                let held = inventory_potions(&backpack);
                if let Some((kind, _)) = held.get(menu.cursor) {
                    menu.item_kind = Some(kind.clone());
                    menu.pane = Some(MenuPane::UseOn);
                    menu.cursor = 0;
                }
            }
            2 if menu.pane == Some(MenuPane::UseOn) => {
                // The pane is GIVE rows then DRINK rows, so the cursor's half decides
                // which it is — the same split the renderer lays out.
                let heroes = roster.heroes.len().max(1);
                let kind = menu.item_kind.clone();
                match (kind, menu.cursor < heroes) {
                    (Some(kind), true) => {
                        net.0.send(ClientCmd::MoveItem {
                            item_kind: kind,
                            hero_slot: menu.cursor as i32,
                            to_pouch: true,
                        });
                        menu.cursor = 0;
                    }
                    (Some(kind), false) => {
                        net.0.send(ClientCmd::UseItem {
                            item_kind: kind,
                            hero_slot: (menu.cursor - heroes) as i32,
                        });
                        menu.pane = None;
                        menu.item_kind = None;
                        menu.cursor = 0;
                    }
                    (None, _) => {
                        menu.pane = None;
                        menu.cursor = 0;
                    }
                }
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
                match GEAR_CATEGORIES.get(menu.cursor).copied() {
                    Some(cat) => picker.category = Some(cat),
                    // The row under the six slots: ask the server to dress this hero.
                    None => net.0.equip_best(menu.member),
                }
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
    // [B] dresses the focused hero from the spare gear, from anywhere in its Equipment
    // pane — the row is there to be found, the key is there once you know it.
    if keys.just_pressed(KeyCode::KeyB)
        && menu.pane == Some(MenuPane::Equipment)
        && picker.category.is_none()
    {
        net.0.equip_best(menu.member);
    }
    // [A] jumps straight to the focused hero's abilities — the thing a player opens
    // the menu to read.
    if keys.just_pressed(KeyCode::KeyA) && menu.section == Some(MenuSection::Party) {
        menu.member = if menu.depth() == 1 { menu.cursor } else { menu.member };
        menu.pane = Some(MenuPane::Abilities);
        menu.cursor = 0;
    }
}

/// The deepest stack of the stock this station is built from, as `(kind, quantity)`: ore
/// for a smith's forge, reagents for a Keeper's still. Deepest first, so someone who
/// hauled good stock out of a deep section is not left spending it last.
pub(crate) fn carried_for(backpack: &RunBackpack, station: &str) -> Option<(String, i32)> {
    let class = if station == "alembic" {
        meld_proto::materials::MaterialClass::Reagent
    } else {
        meld_proto::materials::MaterialClass::Ore
    };
    carried_of_class(backpack, class)
}

/// The deepest carried stack of one material class, as `(kind, quantity)`.
///
/// ⚠️ THE BUILD MENU USED TO ASK FOR ORE, WHATEVER IT WAS OFFERING TO BUILD. It called
/// `carried_for(.., "smith")` for every structure in the registry, which was true only while
/// everything was built out of ore — and BD-1 ended that. Afterwards the menu was wrong in
/// BOTH directions: a player carrying six stone saw "Raise an Anchor (no ore carried)",
/// greyed out, on a build the server would have accepted; and a player carrying ore saw it
/// lit up and got refused. Ask the REGISTRY what a structure is made of
/// (`StructureDef::material`), the way the server does.
pub(crate) fn carried_of_class(
    backpack: &RunBackpack,
    class: meld_proto::materials::MaterialClass,
) -> Option<(String, i32)> {
    backpack
        .items
        .iter()
        .filter(|(kind, qty)| *qty > 0 && meld_proto::materials::is_class(kind, class))
        .max_by_key(|(kind, _)| {
            meld_proto::materials::material(kind).map(|m| m.tier).unwrap_or(0)
        })
        .map(|(kind, qty)| (kind.clone(), *qty))
}

/// One build row's label and tint: what it raises, and what you are carrying toward it.
///
/// Pulled out of the UI closure so it can be TESTED. The bug it was written to close is not
/// hypothetical — the row used to ask `carried_for(.., "smith")`, i.e. ORE, for every
/// structure in the registry, which was true only while everything was built out of ore.
/// After BD-1 it was wrong in both directions at once, and nothing could catch that because
/// the logic lived inside a `with_children` closure that only a running game exercises.
pub(crate) fn build_row(
    def: &meld_proto::structures::StructureDef,
    backpack: &RunBackpack,
) -> (String, Color) {
    match carried_of_class(backpack, def.material) {
        Some((kind, qty)) => (format!("Raise a {}   ({qty} {kind})", def.name), glass::TEXT),
        // Name the material it WANTS. "No ore carried" on a timber palisade sent a player
        // looking for entirely the wrong thing.
        None => (
            format!("Raise a {}   (no {} carried)", def.name, def.material.wire()),
            glass::DIM,
        ),
    }
}

/// Tapping the Map column's "Set up a smith station" row raises one — the touch twin of
/// pressing Enter on it.
pub(crate) fn build_station_click(
    rows: Query<(&Interaction, &BuildStationButton), Changed<Interaction>>,
    mut overlay: ResMut<Overlay>,
    backpack: Res<RunBackpack>,
    net: NonSend<NetRes>,
) {
    for (interaction, btn) in &rows {
        if *interaction == Interaction::Pressed && carried_for(&backpack, btn.kind).is_some() {
            net.0.send(ClientCmd::BuildStation { kind: btn.kind.into() });
            overlay.kind = None;
        }
    }
}

/// Tapping a "Raise a …" row builds it where you stand — the touch twin of pressing
/// Enter on it. One handler for every function; the registry decides what the rows are.
pub(crate) fn build_structure_click(
    rows: Query<(&Interaction, &BuildStructureButton), Changed<Interaction>>,
    mut overlay: ResMut<Overlay>,
    backpack: Res<RunBackpack>,
    mut build: ResMut<crate::builder::BuildMode>,
) {
    for (interaction, btn) in &rows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        // ⚠️ THIS GATE ASKED FOR ORE, AND THAT IS WHY NOTHING WAS CLICKABLE. The row's
        // LABEL was fixed to ask the registry and this handler was not — so a player
        // carrying eight timber saw "Raise a Wall (8 Heartoak Log)" lit up, clicked it, and
        // nothing happened at all. One rule in two places, in the same file as the comment
        // warning about it.
        //
        // The affordability question belongs to `map_rows` (which decides `live`), so ask
        // the same question the same way: the structure's OWN material.
        let Some(def) = meld_proto::structures::structure(btn.function) else {
            continue;
        };
        if carried_of_class(&backpack, def.material).is_some() {
            // BD-9: clicking a row ARMS the tool rather than dropping a structure at your
            // feet. You then aim it, turn it with `R`, and drag to lay a run — which is what
            // "click and stretch" needs, and what a single click-to-place could never be.
            build.arm(btn.function);
            overlay.kind = None;
        }
    }
}

/// Tapping the Map column's "Return to town" row spends a Town Portal — the same
/// explicit action as pressing Enter on it, for touch.
pub(crate) fn return_to_town_click(
    rows: Query<&Interaction, (Changed<Interaction>, With<ReturnToTownButton>)>,
    mut overlay: ResMut<Overlay>,
    backpack: Res<RunBackpack>,
    net: NonSend<NetRes>,
) {
    for interaction in &rows {
        if *interaction == Interaction::Pressed && backpack.count("town_portal") > 0 {
            net.0.send(ClientCmd::TownPortal);
            overlay.kind = None;
        }
    }
}

/// Clicks in the Items column: a potion opens the hero picker, a hero drinks it.
/// The server decides whether it lands — this only avoids offering the obviously
/// hopeless (a fight-only potion, a full hero, a revive on someone standing).
pub(crate) fn use_item_click(
    potions: Query<(&Interaction, &UseItemButton), Changed<Interaction>>,
    targets: Query<(&Interaction, &UseOnHeroButton), Changed<Interaction>>,
    gives: Query<(&Interaction, &GiveToHeroButton), Changed<Interaction>>,
    takes: Query<(&Interaction, &TakeBackButton), Changed<Interaction>>,
    mut menu: ResMut<MainMenu>,
    net: NonSend<NetRes>,
) {
    for (interaction, btn) in &potions {
        // Staging is NOT gated on `usable_in_field` any more: a fight-only potion still
        // has to be handed to a hero, and refusing to open the pane for it made exactly
        // the potions that need a pouch the ones you could not put in one.
        if *interaction == Interaction::Pressed {
            menu.item_kind = Some(btn.item_kind.clone());
            menu.pane = Some(MenuPane::UseOn);
            menu.cursor = 0;
        }
    }
    for (interaction, btn) in &gives {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(kind) = menu.item_kind.clone() else { continue };
        net.0.send(ClientCmd::MoveItem {
            item_kind: kind,
            hero_slot: btn.slot as i32,
            to_pouch: true,
        });
        // The pane stays OPEN so handing out three salves is three clicks rather than
        // three trips back through the inventory list.
        menu.cursor = 0;
    }
    for (interaction, btn) in &takes {
        if *interaction != Interaction::Pressed {
            continue;
        }
        net.0.send(ClientCmd::MoveItem {
            item_kind: btn.item_kind.clone(),
            hero_slot: btn.slot as i32,
            to_pouch: false,
        });
    }
    for (interaction, btn) in &targets {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(kind) = menu.item_kind.clone() else { continue };
        net.0.send(ClientCmd::UseItem { item_kind: kind, hero_slot: btn.slot as i32 });
        menu.pane = None;
        menu.item_kind = None;
        menu.cursor = 0;
    }
}

/// Clicks on the nav rows and the per-hero Equipment/Abilities buttons. The gear
/// rows themselves are handled by the equip flow's own click systems.
/// Tapping the Equipment pane's "Equip best" row — the touch twin of [B].
pub(crate) fn equip_best_click(
    rows: Query<(&Interaction, &EquipBestButton), Changed<Interaction>>,
    net: NonSend<NetRes>,
) {
    for (interaction, btn) in &rows {
        if *interaction == Interaction::Pressed {
            net.0.equip_best(btn.member);
        }
    }
}

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

#[cfg(test)]
mod tests {

    /// GR-5's free-the-off-hand-first rule reaches the rows the player actually clicks.
    /// It was computed by a helper whose only caller was its own unit test, while the
    /// live menu hardcoded `free_first: None` — so picking up a two-hander with a shield
    /// on took a 409 instead of putting the shield away.
    #[test]
    fn a_two_hander_row_knows_which_off_hand_it_displaces() {
        let piece = |id: &str, slot: &str, family: &str, worn: Option<usize>| GearLine {
            gear_id: id.into(),
            name: id.into(),
            slot: slot.into(),
            class_key: String::new(),
            insurance: "insured".into(),
            family: family.into(),
            armor_weight: String::new(),
            tier: 1,
            equipped_hero_slot: worn,
            max_durability: 20,
            base_max_durability: 20,
            atk_bonus: 2,
            def_bonus: 2,
            spd_bonus: 0,
            affixes: Vec::new(),
            unique_key: String::new(),
            set_key: String::new(),
            reroll_cost: 3,
        };
        let shield = piece("shield-1", "off_hand", "shield", Some(1));
        let spear = piece("spear-1", "main_hand", "spear", None);
        let sword = piece("sword-1", "main_hand", "sword", None);
        let pool = vec![shield.clone()];

        assert_eq!(
            off_hand_in_the_way(&pool, &spear, 1).as_deref(),
            Some("shield-1"),
            "a two-hander must name the off-hand it displaces"
        );
        assert!(off_hand_in_the_way(&pool, &sword, 1).is_none(), "one-handers displace nothing");
        assert!(
            off_hand_in_the_way(&pool, &spear, 0).is_none(),
            "another hero's off-hand is not in the way"
        );
    }

    use super::*;

    #[test]
    fn the_cascade_closes_one_column_at_a_time() {
        let mut m = MainMenu {
            section: Some(MenuSection::Party),
            member: 2,
            pane: Some(MenuPane::Abilities),
            cursor: 4,
            item_kind: None,
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

    #[test]
    fn every_section_is_reachable_from_the_nav() {
        // The nav renders `ALL` and opening a row indexes into it, so a variant
        // missing from `ALL` is a column no player can ever reach.
        for s in [
            MenuSection::Items,
            MenuSection::Materials,
            MenuSection::Party,
            MenuSection::Map,
            MenuSection::Guide,
        ] {
            assert!(MenuSection::ALL.contains(&s), "{s:?} is off the nav");
            assert!(!s.label().is_empty());
        }
    }

    #[test]
    fn the_guide_names_a_key_and_what_it_does_on_every_row() {
        assert!(!GUIDE.is_empty());
        for (heading, rows) in GUIDE {
            assert!(!heading.is_empty());
            assert!(!rows.is_empty(), "{heading} lists nothing");
            for (key, what) in rows {
                assert!(!key.is_empty() && !what.is_empty(), "{heading} has a half-row");
            }
        }
    }

    /// The Den's board appears when the Den does. Nothing in this menu advertises what
    /// has not been earned, so a locked Quests row would break the panel's own rule.
    #[test]
    fn quests_appears_with_the_hunter_and_not_before() {
        let none: Vec<String> = Vec::new();
        assert!(!visible_sections(&none).contains(&MenuSection::Quests));
        assert_eq!(visible_sections(&none).len(), MenuSection::ALL.len() - 1);

        let owned = vec!["class_explorer".to_string(), "class_hunter".to_string()];
        let with = visible_sections(&owned);
        assert!(with.contains(&MenuSection::Quests));
        assert_eq!(with.len(), MenuSection::ALL.len());
        // Order is the nav's own order either way, so a row never moves under the cursor.
        assert_eq!(with, MenuSection::ALL.to_vec());
        let expected: Vec<MenuSection> =
            MenuSection::ALL.iter().copied().filter(|s| *s != MenuSection::Quests).collect();
        assert_eq!(visible_sections(&none), expected);
    }

    #[test]
    fn the_guide_column_is_reading_only() {
        // Nothing to select, so `column_len` is 0 — the cursor arithmetic in
        // `main_menu_input` divides by it, and only its `.max(1)` keeps that safe.
        let menu = MainMenu { section: Some(MenuSection::Guide), ..default() };
        let len = column_len(
            &menu,
            &PartyRoster::default(),
            &AccountHeroNames::default(),
            &InventoryData::default(),
            &RunBackpack::default(),
            &EquipPicker::default(),
            &[],
        );
        assert_eq!(len, 0);
        assert_eq!(len.max(1), 1, "the guard the cursor relies on");
    }
}

#[cfg(test)]
mod build_row_tests {
    use super::*;

    fn bag(items: &[(&str, i32)]) -> RunBackpack {
        RunBackpack {
            items: items.iter().map(|(k, q)| ((*k).to_string(), *q)).collect(),
            ..Default::default()
        }
    }

    /// **Each row asks about its OWN material.** A bag of masonry lights up the anchor and
    /// dims the palisade, and a bag of timber does the opposite. The old code asked for ore
    /// on every row, so after BD-1 it was wrong in BOTH directions: stone in the bag showed
    /// "no ore carried" on a build the server would have accepted, and ore in the bag lit up
    /// a row the server would refuse.
    #[test]
    fn a_build_row_asks_for_the_material_it_is_made_of() {
        let stone_only = bag(&[("river_granite", 9)]);
        let wood_only = bag(&[("heartoak_log", 9)]);
        for def in meld_proto::structures::STRUCTURES {
            let (with_stone, tint_stone) = build_row(def, &stone_only);
            let (with_wood, tint_wood) = build_row(def, &wood_only);
            match def.material {
                meld_proto::materials::MaterialClass::Stone => {
                    assert!(with_stone.contains("river_granite"), "{}: {with_stone}", def.key);
                    assert_eq!(tint_stone, glass::TEXT, "{} should be live on stone", def.key);
                    assert!(with_wood.contains("no stone carried"), "{}: {with_wood}", def.key);
                    assert_eq!(tint_wood, glass::DIM, "{} should be dim on wood", def.key);
                }
                meld_proto::materials::MaterialClass::Wood => {
                    assert!(with_wood.contains("heartoak_log"), "{}: {with_wood}", def.key);
                    assert_eq!(tint_wood, glass::TEXT, "{} should be live on wood", def.key);
                    assert!(with_stone.contains("no wood carried"), "{}: {with_stone}", def.key);
                    assert_eq!(tint_stone, glass::DIM, "{} should be dim on stone", def.key);
                }
                other => panic!("{} is built from {other:?}, which is not structural", def.key),
            }
        }
    }

    /// ORE never pays for a building any more, and the row must say so. This is the exact
    /// regression: `carried_for(.., "smith")` would have found this bag and lit every row.
    #[test]
    fn a_bag_of_ore_does_not_light_up_a_single_build_row() {
        let ore = bag(&[("heartoak_bark", 40), ("dune_iron", 40)]);
        for def in meld_proto::structures::STRUCTURES {
            let (label, tint) = build_row(def, &ore);
            assert_eq!(tint, glass::DIM, "{} lit up for a bag of ore: {label}", def.key);
            assert!(label.contains("carried"), "{}: {label}", def.key);
        }
    }

    /// The DEEPEST stack is what a row reports, matching what the server will actually
    /// spend (`building::affordable_kind`). A menu naming the shallow stock while the
    /// server spends the deep stock is a menu that lies about your bag.
    #[test]
    fn a_row_names_the_stock_the_server_will_spend() {
        let both = bag(&[("heartoak_log", 9), ("bog_root_timber", 9)]);
        let wall = meld_proto::structures::structure("wall").unwrap();
        let (label, _) = build_row(wall, &both);
        assert!(label.contains("bog_root_timber"), "should name the deeper stock: {label}");
    }
}

#[cfg(test)]
mod map_row_tests {
    use super::*;

    fn bag(items: &[(&str, i32)]) -> RunBackpack {
        RunBackpack {
            items: items.iter().map(|(k, q)| ((*k).to_string(), *q)).collect(),
            ..Default::default()
        }
    }

    fn row<'a>(rows: &'a [MapRow], kind: &MapRowKind) -> &'a MapRow {
        rows.iter().find(|r| &r.kind == kind).expect("that row should exist")
    }

    /// **BUILDING IS NOT LOCKED BEHIND A PROFESSION CLASS, AND STATIONS ARE.** The two sit
    /// next to each other in the same column, so it is an easy thing to be wrong about in
    /// either direction — and the server agrees with this: `building::raise` has no class
    /// check, while the station path maps `"smith" => CharacterClass::Smithwright`.
    ///
    /// It is deliberate rather than an oversight. An anchor is the co-op permanence verb —
    /// it is specifically exempt from the no-build-near-player rule so a party can plant one
    /// together, and anyone may repair one. Gating it on a class would mean a party without
    /// a Smithwright could not hold ground at all.
    #[test]
    fn raising_a_structure_is_not_locked_behind_a_profession_class() {
        // A party with NO Smithwright and NO Keeper, carrying both structural materials.
        let rows = map_rows(
            &bag(&[("heartoak_log", 9), ("river_granite", 9)]),
            &(["explorer", "psyker", "resonant", "hunter"]),
        );
        for def in meld_proto::structures::STRUCTURES {
            let r = row(&rows, &MapRowKind::Structure(def.key));
            assert!(
                r.live,
                "`{}` is dim for a party with no crafter, but the server would allow it: {}",
                def.key, r.label
            );
        }
        // …while the benches say who is missing.
        for (kind, who) in [("smith", "Smithwright"), ("alembic", "Keeper")] {
            let r = row(&rows, &MapRowKind::Station(kind));
            assert!(!r.live, "the {kind} bench should be dim with no {who}: {}", r.label);
            assert!(r.label.contains(who), "it should name the class it needs: {}", r.label);
        }
    }

    /// With the right crafter AND the right stock, the bench lights up — so the test above
    /// is proving a GATE rather than a row that is simply always dim.
    #[test]
    fn a_bench_lights_up_for_the_crafter_who_owns_it() {
        let rows = map_rows(
            &bag(&[("heartoak_bark", 9), ("bloom_herb", 9)]),
            &(["smithwright", "keeper"]),
        );
        assert!(row(&rows, &MapRowKind::Station("smith")).live, "a Smithwright with ore");
        assert!(row(&rows, &MapRowKind::Station("alembic")).live, "a Keeper with reagents");
    }

    /// Every structure in the registry gets a row, and each is lit only by ITS OWN material.
    /// This is the regression that shipped: the rows all asked for ore.
    #[test]
    fn each_structure_row_is_lit_only_by_its_own_material() {
        let crafters = ["explorer"];
        for def in meld_proto::structures::STRUCTURES {
            let own = meld_proto::materials::MATERIALS
                .iter()
                .find(|m| m.class == def.material)
                .expect("its material exists")
                .key;
            let other = meld_proto::materials::MATERIALS
                .iter()
                .find(|m| m.class.is_structural() && m.class != def.material)
                .expect("the other structural class exists")
                .key;

            let with_own = map_rows(&bag(&[(own, 9)]), &crafters);
            assert!(row(&with_own, &MapRowKind::Structure(def.key)).live, "{} on {own}", def.key);

            let with_other = map_rows(&bag(&[(other, 9)]), &crafters);
            assert!(
                !row(&with_other, &MapRowKind::Structure(def.key)).live,
                "{} lit up on {other}, which cannot pay for it",
                def.key
            );

            // And ore pays for NOTHING structural any more.
            let with_ore = map_rows(&bag(&[("heartoak_bark", 40)]), &crafters);
            assert!(
                !row(&with_ore, &MapRowKind::Structure(def.key)).live,
                "{} lit up for a bag of ore",
                def.key
            );
        }
    }

    /// Going home needs the ITEM, and the row says so when you have none — the primary way
    /// out of a dive must be findable, including when you cannot use it.
    #[test]
    fn the_way_home_is_dim_without_a_portal() {
        let rows = map_rows(&bag(&[]), &(["explorer"]));
        let r = row(&rows, &MapRowKind::ReturnToTown);
        assert!(!r.live);
        assert!(r.label.contains("none held"), "{}", r.label);
        let rows = map_rows(&bag(&[("town_portal", 2)]), &(["explorer"]));
        assert!(row(&rows, &MapRowKind::ReturnToTown).live);
    }

    /// The column's rows and the cursor must agree about how many there are. `map_actions`
    /// indexes the cursor by row position, so a row added anywhere but the end used to shift
    /// what every key below it pressed.
    #[test]
    fn the_column_has_a_row_for_everything_it_can_do() {
        let rows = map_rows(&bag(&[]), &(["explorer"]));
        assert_eq!(rows.len(), 1 + 2 + meld_proto::structures::STRUCTURES.len());
    }
}

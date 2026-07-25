//! Simple full-screen screens: Join (party builder), co-op Lobby, and the
//! Ended (extract/death) summary.
//! Extracted from `main.rs` during the module reorg.


use bevy::prelude::*;

use meld_client::net::ClientCmd;

use super::*;

// ---------------------------------------------------------------- join -----

/// Picker metadata for a class (client-side; the authoritative stats live in
/// `balance.toml`, but the Join screen is pre-connection so it shows an at-a-glance
/// role + relative 0..5 ratings + kit). Ratings are qualitative, tuned to read the
/// class taxonomy (CLAUDE.md) at a glance, not the exact balance numbers.
pub(crate) struct ClassInfo {
    pub key: &'static str,
    pub name: &'static str,
    pub role: &'static str,
    pub hp: u8,
    pub atk: u8,
    pub spd: u8,
    pub mag: u8,
    pub def: u8,
    pub kit: &'static str,
}

pub(crate) const CLASS_INFO: [ClassInfo; 5] = [
    ClassInfo { key: "hunter", name: "Hunter", role: "Front-line bruiser. Basic attacks bank Adrenaline; every skill spends it.", hp: 4, atk: 4, spd: 3, mag: 1, def: 3, kit: "Power Strike \u{b7} Second Wind \u{b7} Snare \u{b7} Frenzy" },
    ClassInfo { key: "psyker", name: "Psyker", role: "Psychic channeler. Weaves persistent Foci from the back row.", hp: 2, atk: 1, spd: 3, mag: 5, def: 2, kit: "Gravity Well \u{b7} Kinetic Aegis \u{b7} Mind Spike \u{b7} Temporal Anchor" },
    ClassInfo { key: "resonant", name: "Resonant", role: "Healer. Innate Regen keeps the party standing.", hp: 3, atk: 2, spd: 3, mag: 4, def: 2, kit: "Transfuse \u{b7} Regen Boon \u{b7} Ward" },
    ClassInfo { key: "shifter", name: "Shifter", role: "Rogue skirmisher. Fast, fragile, the only innate dodge.", hp: 2, atk: 4, spd: 5, mag: 1, def: 1, kit: "Backstab \u{b7} Flicker \u{b7} Ransack" },
    ClassInfo { key: "iron_hull", name: "Iron Hull", role: "Order of the Iron Hull monk \u{2014} the tankiest, slowest wall.", hp: 5, atk: 3, spd: 1, mag: 1, def: 5, kit: "Swell Strike \u{b7} Root \u{b7} Kinetic Shock \u{b7} Toll of the Deep" },
];

pub(crate) fn class_info(key: &str) -> &'static ClassInfo {
    CLASS_INFO.iter().find(|c| c.key == key).unwrap_or(&CLASS_INFO[0])
}

/// The class whose details fill the panel (last hovered/selected). Init to the lead.
#[derive(Resource)]
pub(crate) struct JoinFocus(pub String);
impl Default for JoinFocus {
    fn default() -> Self {
        JoinFocus("hunter".into())
    }
}

// Join-screen element markers.
#[derive(Component)]
pub(crate) struct JoinSlot(pub usize); // clickable party-slot card
#[derive(Component)]
pub(crate) struct JoinSlotSprite(pub usize);
#[derive(Component)]
pub(crate) struct JoinSlotName(pub usize);
#[derive(Component)]
pub(crate) struct JoinClassCard(pub &'static str); // palette card (hover=details, click=assign)
#[derive(Component)]
pub(crate) struct JoinClassSprite(pub &'static str); // palette card sprite (refreshed once art loads)
#[derive(Component)]
pub(crate) struct JoinDetailSprite;
#[derive(Component)]
pub(crate) struct JoinDetailName;
#[derive(Component)]
pub(crate) struct JoinDetailRole;
#[derive(Component)]
pub(crate) struct JoinDetailKit;
#[derive(Component)]
pub(crate) struct JoinStatFill {
    pub stat: u8, // 0..5 which stat row
    pub seg: u8,  // 0..5 which segment
}

fn glass(a: f32) -> Color {
    Color::srgba(0.10, 0.13, 0.22, a)
}

/// One class card (used for both the 4 party slots and the 5-class palette): a
/// framed sprite over a label. `sprite` is the class's front idle frame.
fn class_card(
    parent: &mut ChildSpawnerCommands,
    sprite: Handle<Image>,
    label: &str,
    sub: &str,
    w: f32,
    tags: impl Bundle,
    sprite_tag: impl Bundle,
    name_tag: impl Bundle,
) {
    parent
        .spawn((
            Button,
            tags,
            Node {
                width: Val::Px(w),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(4.0),
                padding: UiRect::all(Val::Px(8.0)),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BorderColor(glass(0.9)),
            BackgroundColor(glass(0.5)),
            BorderRadius::all(Val::Px(10.0)),
        ))
        .with_children(|c| {
            if !sub.is_empty() {
                c.spawn((
                    Text::new(sub.to_string()),
                    TextFont { font_size: 12.0, ..default() },
                    TextColor(Color::srgb(0.55, 0.6, 0.75)),
                ));
            }
            c.spawn((
                ImageNode::new(sprite),
                sprite_tag,
                Node { width: Val::Px(w * 0.7), height: Val::Px(w * 0.7), ..default() },
            ));
            c.spawn((
                Text::new(label.to_string()),
                name_tag,
                TextFont { font_size: 15.0, ..default() },
                TextColor(Color::srgb(0.9, 0.93, 1.0)),
            ));
        });
}

pub(crate) fn join_ui(mut commands: Commands, wa: Option<Res<WorldAssets>>, session: Res<Session>) {
    let sprite = |key: &str| -> Handle<Image> {
        wa.as_ref().map(|w| w.class_frames(key).idle[0].clone()).unwrap_or_default()
    };
    commands
        .spawn((
            JoinRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(9.0),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("MELDWORLD"),
                TextFont { font_size: 36.0, ..default() },
                TextColor(Color::srgb(0.85, 0.9, 1.0)),
            ));
            p.spawn((
                Text::new("Your party of 4 \u{2014} click a slot, then a class. Hover any class for details."),
                TextFont { font_size: 16.0, ..default() },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));

            // The party: 4 slot cards.
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(10.0),
                ..default()
            })
            .with_children(|row| {
                for i in 0..4 {
                    let key = session.party.get(i).cloned().unwrap_or_else(|| "hunter".into());
                    class_card(
                        row,
                        sprite(&key),
                        class_info(&key).name,
                        &format!("Slot {}", i + 1),
                        108.0,
                        JoinSlot(i),
                        JoinSlotSprite(i),
                        JoinSlotName(i),
                    );
                }
            });

            // The class palette.
            p.spawn((
                Text::new("Choose a class"),
                TextFont { font_size: 14.0, ..default() },
                TextColor(Color::srgb(0.55, 0.6, 0.75)),
                Node { margin: UiRect::top(Val::Px(4.0)), ..default() },
            ));
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                ..default()
            })
            .with_children(|row| {
                for ci in &CLASS_INFO {
                    class_card(row, sprite(ci.key), ci.name, "", 92.0, JoinClassCard(ci.key), JoinClassSprite(ci.key), ());
                }
            });

            // The detail panel (filled by `join_refresh` from `JoinFocus`).
            let lead = class_info(&session.party.first().cloned().unwrap_or_else(|| "hunter".into()));
            p.spawn((
                Node {
                    width: Val::Px(560.0),
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(16.0),
                    padding: UiRect::all(Val::Px(12.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    margin: UiRect::top(Val::Px(2.0)),
                    ..default()
                },
                BorderColor(glass(0.9)),
                BackgroundColor(glass(0.55)),
                BorderRadius::all(Val::Px(12.0)),
            ))
            .with_children(|d| {
                d.spawn((
                    ImageNode::new(sprite(lead.key)),
                    JoinDetailSprite,
                    Node { width: Val::Px(96.0), height: Val::Px(96.0), ..default() },
                ));
                d.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(6.0),
                    flex_grow: 1.0,
                    ..default()
                })
                .with_children(|col| {
                    col.spawn((
                        Text::new(lead.name.to_string()),
                        JoinDetailName,
                        TextFont { font_size: 24.0, ..default() },
                        TextColor(Color::srgb(1.0, 0.85, 0.45)),
                    ));
                    col.spawn((
                        Text::new(lead.role.to_string()),
                        JoinDetailRole,
                        TextFont { font_size: 14.0, ..default() },
                        TextColor(Color::srgb(0.78, 0.82, 0.95)),
                    ));
                    // Stat bars.
                    for (si, name) in ["HP", "ATK", "SPD", "MAG", "DEF"].iter().enumerate() {
                        col.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(6.0),
                            ..default()
                        })
                        .with_children(|r| {
                            r.spawn((
                                Text::new(name.to_string()),
                                TextFont { font_size: 12.0, ..default() },
                                TextColor(Color::srgb(0.6, 0.65, 0.8)),
                                Node { width: Val::Px(34.0), ..default() },
                            ));
                            for seg in 0..5u8 {
                                r.spawn((
                                    JoinStatFill { stat: si as u8, seg },
                                    Node { width: Val::Px(22.0), height: Val::Px(9.0), ..default() },
                                    BackgroundColor(Color::srgb(0.2, 0.22, 0.3)),
                                    BorderRadius::all(Val::Px(2.0)),
                                ));
                            }
                        });
                    }
                    col.spawn((
                        Text::new(format!("Kit:  {}", lead.kit)),
                        JoinDetailKit,
                        TextFont { font_size: 13.0, ..default() },
                        TextColor(Color::srgb(0.7, 0.85, 0.7)),
                        Node { margin: UiRect::top(Val::Px(2.0)), ..default() },
                    ));
                });
            });

            p.spawn((
                Text::new("ENTER: dive solo     C: co-op"),
                TextFont { font_size: 15.0, ..default() },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
                Node { margin: UiRect::top(Val::Px(6.0)), ..default() },
            ));
            p.spawn((
                StatusText,
                Text::new(""),
                TextFont { font_size: 15.0, ..default() },
                TextColor(Color::srgb(0.9, 0.6, 0.6)),
            ));
        });
}

/// Keyboard: 1-4 select the slot to edit, ←/→ cycle the selected slot's class,
/// ENTER/C start. (Mouse: click a slot then a class; hover any class for details —
/// see `join_interact`.)
pub(crate) fn join_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    mut session: ResMut<Session>,
    mut focus: ResMut<JoinFocus>,
    mut status_q: Query<&mut Text, With<StatusText>>,
) {
    if !session.connecting {
        // 1-4 select which slot the palette / arrows edit.
        for (slot, key) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4]
            .iter()
            .enumerate()
        {
            if keys.just_pressed(*key) && slot < session.party.len() {
                session.party_cursor = slot;
                focus.0 = session.party[slot].clone();
            }
        }
        // ←/→ cycle the selected slot's class through the roster.
        let dir = if keys.just_pressed(KeyCode::ArrowRight) {
            1
        } else if keys.just_pressed(KeyCode::ArrowLeft) {
            -1
        } else {
            0
        };
        if dir != 0 {
            let slot = session.party_cursor.min(session.party.len().saturating_sub(1));
            if let Some(cur) = session.party.get(slot).cloned() {
                let n = PARTY_CLASSES.len() as i32;
                let i = PARTY_CLASSES.iter().position(|c| *c == cur).unwrap_or(0) as i32;
                let key = PARTY_CLASSES[(((i + dir) % n + n) % n) as usize].to_string();
                session.party[slot] = key.clone();
                focus.0 = key;
            }
        }
    }

    // ENTER (or autoplay) = solo dive. C = co-op → the lobby after connecting.
    let solo = keys.just_pressed(KeyCode::Enter) || autoplay.0;
    let coop = keys.just_pressed(KeyCode::KeyC);
    if (solo || coop) && !session.connecting {
        session.connecting = true;
        session.coop = coop;
        let name = std::env::var("MELD_NAME").unwrap_or_else(|_| {
            format!("guest{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
        });
        session.status = "connecting...".to_string();
        net.0.send(ClientCmd::Connect { username: name });
    }

    if let Ok(mut t) = status_q.single_mut() {
        **t = session.status.clone();
    }
}

/// Mouse/touch on the party slots + class palette: hover a class card to preview it
/// in the detail panel; click a slot to select it, click a class to assign it to the
/// selected slot (and preview it).
#[allow(clippy::type_complexity)]
pub(crate) fn join_interact(
    mut session: ResMut<Session>,
    mut focus: ResMut<JoinFocus>,
    slots: Query<(&Interaction, &JoinSlot), Changed<Interaction>>,
    cards: Query<(&Interaction, &JoinClassCard), Changed<Interaction>>,
) {
    if session.connecting {
        return;
    }
    for (interaction, slot) in &slots {
        match interaction {
            Interaction::Pressed => {
                session.party_cursor = slot.0;
                if let Some(k) = session.party.get(slot.0) {
                    focus.0 = k.clone();
                }
            }
            Interaction::Hovered => {
                if let Some(k) = session.party.get(slot.0) {
                    focus.0 = k.clone();
                }
            }
            Interaction::None => {}
        }
    }
    for (interaction, card) in &cards {
        match interaction {
            Interaction::Pressed => {
                let slot = session.party_cursor.min(session.party.len().saturating_sub(1));
                if slot < session.party.len() {
                    session.party[slot] = card.0.to_string();
                }
                focus.0 = card.0.to_string();
            }
            Interaction::Hovered => focus.0 = card.0.to_string(),
            Interaction::None => {}
        }
    }
}

/// Keep the Join visuals in sync with the party + focused class: slot sprites/names,
/// the selected-slot highlight, and the detail panel (sprite, name, role, kit, stat
/// bars). Immediate-mode-ish, but cheap (a handful of nodes).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn join_refresh(
    session: Res<Session>,
    focus: Res<JoinFocus>,
    wa: Option<Res<WorldAssets>>,
    mut slot_sprites: Query<(&JoinSlotSprite, &mut ImageNode), (Without<JoinDetailSprite>, Without<JoinClassSprite>)>,
    mut class_sprites: Query<(&JoinClassSprite, &mut ImageNode), (Without<JoinDetailSprite>, Without<JoinSlotSprite>)>,
    mut slot_names: Query<(&JoinSlotName, &mut Text), (Without<JoinDetailName>, Without<JoinDetailRole>, Without<JoinDetailKit>)>,
    mut slot_borders: Query<(&JoinSlot, &mut BorderColor)>,
    mut det_sprite: Query<&mut ImageNode, (With<JoinDetailSprite>, Without<JoinSlotSprite>, Without<JoinClassSprite>)>,
    mut det_name: Query<&mut Text, (With<JoinDetailName>, Without<JoinSlotName>, Without<JoinDetailRole>, Without<JoinDetailKit>)>,
    mut det_role: Query<&mut Text, (With<JoinDetailRole>, Without<JoinSlotName>, Without<JoinDetailName>, Without<JoinDetailKit>)>,
    mut det_kit: Query<&mut Text, (With<JoinDetailKit>, Without<JoinSlotName>, Without<JoinDetailName>, Without<JoinDetailRole>)>,
    mut stat_fills: Query<(&JoinStatFill, &mut BackgroundColor)>,
) {
    let Some(wa) = wa else { return };
    let img = |key: &str| wa.class_frames(key).idle[0].clone();
    // Party slots.
    for (s, mut node) in &mut slot_sprites {
        if let Some(k) = session.party.get(s.0) {
            node.image = img(k);
        }
    }
    // Palette cards (fixed class, but re-assign so the sprite appears once its art
    // finishes loading — the Join screen spawns before assets are ready).
    for (c, mut node) in &mut class_sprites {
        node.image = img(c.0);
    }
    for (s, mut t) in &mut slot_names {
        if let Some(k) = session.party.get(s.0) {
            **t = class_info(k).name.to_string();
        }
    }
    for (s, mut bc) in &mut slot_borders {
        *bc = BorderColor(if s.0 == session.party_cursor {
            Color::srgb(1.0, 0.85, 0.45) // gold: the slot the palette/arrows edit
        } else {
            glass(0.9)
        });
    }
    // Detail panel.
    let ci = class_info(&focus.0);
    if let Ok(mut n) = det_sprite.single_mut() {
        n.image = img(ci.key);
    }
    if let Ok(mut t) = det_name.single_mut() {
        **t = ci.name.to_string();
    }
    if let Ok(mut t) = det_role.single_mut() {
        **t = ci.role.to_string();
    }
    if let Ok(mut t) = det_kit.single_mut() {
        **t = format!("Kit:  {}", ci.kit);
    }
    let vals = [ci.hp, ci.atk, ci.spd, ci.mag, ci.def];
    let cols = [
        Color::srgb(0.4, 0.75, 0.45),
        Color::srgb(0.9, 0.5, 0.4),
        Color::srgb(0.5, 0.8, 0.9),
        Color::srgb(0.7, 0.55, 1.0),
        Color::srgb(0.6, 0.65, 0.85),
    ];
    for (f, mut bc) in &mut stat_fills {
        let on = f.seg < vals[f.stat as usize];
        *bc = BackgroundColor(if on { cols[f.stat as usize] } else { Color::srgb(0.2, 0.22, 0.3) });
    }
}

// ---------------------------------------------------------------- lobby ----

pub(crate) fn lobby_ui(mut commands: Commands) {
    commands
        .spawn((
            LobbyRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(14.0),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("CO-OP LOBBY"),
                TextFont { font_size: 40.0, ..default() },
                TextColor(Color::srgb(0.85, 0.9, 1.0)),
            ));
            p.spawn((
                LobbyText,
                Text::new(""),
                TextFont { font_size: 20.0, ..default() },
                TextColor(Color::srgb(0.8, 0.88, 1.0)),
            ));
        });
}

/// Map a just-pressed key to a lobby-code character (A–Z, 0–9).
pub(crate) fn key_to_code_char(key: KeyCode) -> Option<char> {
    use KeyCode::*;
    let c = match key {
        KeyA => 'A', KeyB => 'B', KeyC => 'C', KeyD => 'D', KeyE => 'E', KeyF => 'F',
        KeyG => 'G', KeyH => 'H', KeyI => 'I', KeyJ => 'J', KeyK => 'K', KeyL => 'L',
        KeyM => 'M', KeyN => 'N', KeyO => 'O', KeyP => 'P', KeyQ => 'Q', KeyR => 'R',
        KeyS => 'S', KeyT => 'T', KeyU => 'U', KeyV => 'V', KeyW => 'W', KeyX => 'X',
        KeyY => 'Y', KeyZ => 'Z',
        Digit0 => '0', Digit1 => '1', Digit2 => '2', Digit3 => '3', Digit4 => '4',
        Digit5 => '5', Digit6 => '6', Digit7 => '7', Digit8 => '8', Digit9 => '9',
        _ => return None,
    };
    Some(c)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lobby_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    session: Res<Session>,
    mut lobby: ResMut<LobbyData>,
    mut next: ResMut<NextState<Screen>>,
) {
    if !lobby.in_lobby {
        // Not in a lobby yet: create one, or type a code and join.
        if keys.just_pressed(KeyCode::Enter) {
            // ENTER with no code = create; with a code = join.
            if lobby.code_input.is_empty() {
                net.0.send(ClientCmd::LobbyCreate { party: session.party.clone() });
            } else {
                net.0.send(ClientCmd::LobbyJoin {
                    code: lobby.code_input.clone(),
                    party: session.party.clone(),
                });
            }
            return;
        }
        if keys.just_pressed(KeyCode::Backspace) {
            lobby.code_input.pop();
        }
        for key in keys.get_just_pressed() {
            if lobby.code_input.len() < 6 {
                if let Some(c) = key_to_code_char(*key) {
                    lobby.code_input.push(c);
                }
            }
        }
        return;
    }

    // In a lobby: ready up, start (host), or leave.
    if keys.just_pressed(KeyCode::KeyR) {
        let want = !lobby.my_ready;
        lobby.my_ready = want;
        net.0.send(ClientCmd::LobbyReady { ready: want });
    }
    if keys.just_pressed(KeyCode::Enter) && lobby.host == session.player_id {
        net.0.send(ClientCmd::LobbyStart);
    }
    if keys.just_pressed(KeyCode::Escape) {
        net.0.send(ClientCmd::LobbyLeave);
        lobby.in_lobby = false;
        lobby.code_input.clear();
        next.set(Screen::City);
    }
}

pub(crate) fn render_lobby(
    lobby: Res<LobbyData>,
    session: Res<Session>,
    mut q: Query<&mut Text, With<LobbyText>>,
) {
    let Ok(mut t) = q.single_mut() else { return };
    if !lobby.in_lobby {
        **t = format!(
            "Join code: {}_\n\ntype a code + ENTER to join,\nor ENTER (empty) to create a new lobby",
            lobby.code_input
        );
        return;
    }
    let host_is_me = lobby.host == session.player_id;
    let mut lines = vec![format!("Code: {}", lobby.code), String::new()];
    for (id, username, ready) in &lobby.members {
        let you = if id == &session.player_id { " (you)" } else { "" };
        let host = if id == &lobby.host { " [host]" } else { "" };
        let tag = if *ready { "READY" } else { "..." };
        lines.push(format!("  {username}{you}{host}  -  {tag}"));
    }
    lines.push(String::new());
    let all_ready = !lobby.members.is_empty() && lobby.members.iter().all(|(_, _, r)| *r);
    let start = if host_is_me {
        if all_ready { "ENTER: start the dive" } else { "ENTER: start (need everyone READY)" }
    } else {
        "waiting for the host to start..."
    };
    lines.push(format!("R: toggle ready    {start}    ESC: leave"));
    **t = lines.join("\n");
}

// ----------------------------------------------------------------- ended ---

pub(crate) fn ended_ui(mut commands: Commands, end: Res<EndInfo>) {
    let (title, color): (String, Color) = match end.outcome.as_str() {
        "victory" => ("VICTORY - the creature is slain!".into(), Color::srgb(0.5, 0.95, 0.6)),
        "extracted" => {
            let mut msg = format!(
                "EXTRACTED - banked {} item(s) + {} chits to your Vault",
                end.banked, end.chits
            );
            if end.gear > 0 {
                msg.push_str(&format!(" - {} red-chest gear", end.gear));
            }
            (msg, Color::srgb(0.4, 0.9, 0.95))
        }
        "defeat" | "died" => {
            let msg = if end.chits > 0 {
                format!("DEFEAT - your hero has fallen. Lost {} chits.", end.chits)
            } else {
                "DEFEAT - your hero has fallen.".into()
            };
            (msg, Color::srgb(0.95, 0.4, 0.4))
        }
        // WG-4: walked back west across the border into Last City (run abandoned).
        "abandoned" => (
            "RETURNED to Last City - you slipped back west through the wall.".into(),
            Color::srgb(0.75, 0.85, 0.95),
        ),
        _ => ("The run is over.".into(), Color::srgb(0.8, 0.8, 0.8)),
    };
    commands
        .spawn((
            EndedRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(14.0),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new(title),
                TextFont {
                    font_size: 40.0,
                    ..default()
                },
                TextColor(color),
            ));
            p.spawn((
                Text::new("Press ENTER to return to The Weld    -    ESC to quit"),
                TextFont {
                    font_size: 18.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));
        });
}

pub(crate) fn ended_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut session: ResMut<Session>,
    mut next: ResMut<NextState<Screen>>,
    mut exit: EventWriter<AppExit>,
) {
    // Return to the hub — banked loot (or, on death, your insured blue gear) is
    // waiting there. `city_ui` re-fetches the Vault and re-arms the next dive.
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        session.channeling = false;
        session.status.clear();
        next.set(Screen::City);
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
}

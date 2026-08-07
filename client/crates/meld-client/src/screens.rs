//! Simple full-screen screens: Join (party builder), co-op Lobby, and the
//! Ended (extract/death) summary.
//! Extracted from `main.rs` during the module reorg.


use bevy::prelude::*;

use meld_client::glass;

use meld_client::net::{self, ClientCmd};

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
    /// The class kit as `(skill name, what it does)` — shown in the detail panel.
    pub kit: &'static [(&'static str, &'static str)],
}

pub(crate) const CLASS_INFO: [ClassInfo; 6] = [
    ClassInfo { key: "explorer", name: "Explorer", role: "The order that maps and anchors the world \u{2014} tempo and stability, not burst.", hp: 4, atk: 3, spd: 3, mag: 2, def: 3, kit: &[
        ("Trailblaze", "a solid strike that costs nothing"),
        ("Field Dressing", "patch up an ally or yourself (Lv2)"),
        ("Read the Ground", "damage + steal the foe's tempo (Lv5)"),
        ("Set Anchor", "Barrier for the WHOLE party (Lv9)"),
    ] },
    ClassInfo { key: "hunter", name: "Hunter", role: "The guild that disposes of dangerous creatures \u{2014} the martial baseline.", hp: 4, atk: 4, spd: 3, mag: 1, def: 3, kit: &[
        ("Power Strike", "a heavy blow; spends Adrenaline"),
        ("Second Wind", "heal yourself for Adrenaline (Lv2)"),
        ("Snare", "damage + steal the foe's turn (Lv2)"),
        ("Frenzy", "your biggest hit, biggest cost (Lv5)"),
    ] },
    ClassInfo { key: "psyker", name: "Psyker", role: "Psychic channeler. Weaves persistent Foci from the back row.", hp: 2, atk: 1, spd: 3, mag: 5, def: 2, kit: &[
        ("Gravity Well", "armour-ignoring damage every turn"),
        ("Kinetic Aegis", "shield an ally (Barrier)"),
        ("Mind Spike", "a stronger damage Focus (Lv3)"),
        ("Temporal Anchor", "drain the enemy's ATB gauge (Lv5)"),
    ] },
    ClassInfo { key: "resonant", name: "Resonant", role: "Healer. Innate Regen keeps the party standing.", hp: 3, atk: 2, spd: 3, mag: 4, def: 2, kit: &[
        ("Transfuse", "heal an ally, paid from your own HP"),
        ("Regen Boon", "grant an ally Regen (Lv2)"),
        ("Ward", "shield an ally (Barrier) (Lv3)"),
    ] },
    ClassInfo { key: "shifter", name: "Shifter", role: "Rogue skirmisher. Fast, fragile, the only innate dodge.", hp: 2, atk: 4, spd: 5, mag: 1, def: 1, kit: &[
        ("Backstab", "a heavy strike that pierces armour"),
        ("Flicker", "blink for self Evasion (Lv2)"),
        ("Ransack", "hit + drain the enemy's ATB (Lv3)"),
    ] },
    ClassInfo { key: "phoenix_guard", name: "Phoenix Guard", role: "The Last City's anti-undead order \u{2014} a wall that hits the risen hardest.", hp: 5, atk: 3, spd: 1, mag: 1, def: 5, kit: &[
        ("Silvered Strike", "a staggering blow; far worse for undead"),
        ("Rite of Rest", "a self Barrier stance (Lv2)"),
        ("Holy Censure", "heavy blow, zeroes the foe's gauge (Lv5)"),
        ("Purging Light", "light on ALL enemies (Lv9)"),
    ] },
];

pub(crate) fn class_info(key: &str) -> &'static ClassInfo {
    CLASS_INFO.iter().find(|c| c.key == key).unwrap_or(&CLASS_INFO[0])
}

/// The kit as a multi-line "Skills\n  Name — what it does" block for the detail panel.
fn kit_text(ci: &ClassInfo) -> String {
    let mut s = String::from("Skills");
    for (name, desc) in ci.kit {
        s.push_str(&format!("\n  {name} \u{2014} {desc}"));
    }
    s
}


/// Which account-login field is being typed into: 0 = username, 1 = password, None =
/// no field (so 1-4 / arrows still drive the class picker).
#[derive(Resource, Default)]
pub(crate) struct LoginFocus(pub Option<u8>);

#[derive(Component)]
pub(crate) struct JoinUserField; // clickable username box
#[derive(Component)]
pub(crate) struct JoinPassField; // clickable password box
#[derive(Component)]
pub(crate) struct JoinUserText;
#[derive(Component)]
pub(crate) struct JoinPassText;

/// KeyCode → character for typing an account username/password: lowercase letters
/// (uppercase with Shift), digits, and a couple of safe symbols.
pub(crate) fn typed_char(key: KeyCode, shift: bool) -> Option<char> {
    use KeyCode::*;
    let base = match key {
        KeyA => 'a', KeyB => 'b', KeyC => 'c', KeyD => 'd', KeyE => 'e', KeyF => 'f',
        KeyG => 'g', KeyH => 'h', KeyI => 'i', KeyJ => 'j', KeyK => 'k', KeyL => 'l',
        KeyM => 'm', KeyN => 'n', KeyO => 'o', KeyP => 'p', KeyQ => 'q', KeyR => 'r',
        KeyS => 's', KeyT => 't', KeyU => 'u', KeyV => 'v', KeyW => 'w', KeyX => 'x',
        KeyY => 'y', KeyZ => 'z',
        Digit0 => '0', Digit1 => '1', Digit2 => '2', Digit3 => '3', Digit4 => '4',
        Digit5 => '5', Digit6 => '6', Digit7 => '7', Digit8 => '8', Digit9 => '9',
        Minus => '-', Period => '.',
        _ => return None,
    };
    Some(if shift && base.is_ascii_alphabetic() { base.to_ascii_uppercase() } else { base })
}

/// One labelled, clickable text field (username / password).
fn field_box(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    field_tag: impl Bundle,
    text_tag: impl Bundle,
) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(6.0),
            ..default()
        })
        .with_children(|r| {
            r.spawn((
                Text::new(label.to_string()),
                TextFont { font_size: 14.0, ..default() },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));
            r.spawn((
                Button,
                field_tag,
                Node {
                    width: Val::Px(170.0),
                    height: Val::Px(28.0),
                    align_items: AlignItems::Center,
                    padding: UiRect::horizontal(Val::Px(8.0)),
                    border: UiRect::all(Val::Px(1.5)),
                    ..default()
                },
                BorderColor(glass(0.9)),
                BackgroundColor(glass(0.5)),
                BorderRadius::all(Val::Px(6.0)),
            ))
            .with_children(|b| {
                b.spawn((
                    Text::new(String::new()),
                    text_tag,
                    TextFont { font_size: 15.0, ..default() },
                    TextColor(Color::srgb(0.92, 0.94, 1.0)),
                ));
            });
        });
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
                    TextFont { font_size: 15.0, ..default() },
                    TextColor(Color::srgb(0.55, 0.6, 0.75)),
                ));
            }
            c.spawn((
                ImageNode::new(sprite),
                sprite_tag,
                Node { width: Val::Px(w * 0.9), height: Val::Px(w * 0.9), ..default() },
            ));
            c.spawn((
                Text::new(label.to_string()),
                name_tag,
                TextFont { font_size: 19.0, ..default() },
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
                row_gap: Val::Px(7.0),
                ..default()
            },
            // A dark scrim over the (bright) 3D overworld behind the menu so the text
            // and cards read clearly instead of washing out against the grass.
            BackgroundColor(glass::SCRIM),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("MELDWORLD"),
                TextFont { font_size: 44.0, ..default() },
                TextColor(Color::srgb(0.85, 0.9, 1.0)),
            ));
            p.spawn((
                Text::new("Log in \u{2014} then muster your party in the Last City."),
                TextFont { font_size: 19.0, ..default() },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));

            // Account login (real, persistent accounts): click a field and type;
            // TAB switches fields. New name → account is created on first login.
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(18.0),
                ..default()
            })
            .with_children(|row| {
                field_box(row, "Username", JoinUserField, JoinUserText);
                field_box(row, "Password", JoinPassField, JoinPassText);
            });
            p.spawn((
                Text::new("Click a field and type, then ENTER \u{2014} first login creates your account.  (username 3\u{2013}20 \u{b7} password 8+ chars)"),
                TextFont { font_size: 12.0, ..default() },
                TextColor(Color::srgb(0.5, 0.55, 0.7)),
            ));

            // The season's Vanguard Board. The party used to be built here, but this
            // screen runs BEFORE login: it cannot know which classes the account owns,
            // so it could only offer all six and let the server clamp the answer. The
            // party is mustered in town (the Drill Yard), where the unlock set is
            // known. What belongs on a login screen is a reason to log in.
            p.spawn((
                Text::new("The Vanguard \u{2014} deepest of the season"),
                TextFont { font_size: 16.0, ..default() },
                TextColor(Color::srgb(0.85, 0.78, 0.5)),
                Node { margin: UiRect::top(Val::Px(10.0)), ..default() },
            ));
            p.spawn((
                Node {
                    width: Val::Px(460.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(3.0),
                    padding: UiRect::all(Val::Px(14.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BorderColor(glass(0.9)),
                BackgroundColor(glass(0.55)),
                BorderRadius::all(Val::Px(12.0)),
            ))
            .with_children(|d| {
                d.spawn((
                    Text::new("reading the board..."),
                    JoinBoardText,
                    TextFont { font_size: 14.0, ..default() },
                    TextColor(Color::srgb(0.72, 0.76, 0.88)),
                ));
            });


            p.spawn((
                Text::new("ENTER: log in"),
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

/// The Vanguard board line on the login screen.
#[derive(Component)]
pub(crate) struct JoinBoardText;

/// Fill the login screen's Vanguard board once the fetch lands.
pub(crate) fn join_board_refresh(
    board: Res<VanguardBoardData>,
    mut q: Query<&mut Text, With<JoinBoardText>>,
) {
    let Ok(mut t) = q.single_mut() else { return };
    if !board.loaded {
        return;
    }
    if board.entries.is_empty() {
        **t = "No one has come back deep enough yet.".to_string();
        return;
    }
    **t = board
        .entries
        .iter()
        .take(8)
        .map(|e| format!("{:>2}.  {:<20}  {} m", e.rank, e.username, e.max_distance))
        .collect::<Vec<_>>()
        .join("\n");
}

/// Join-screen keyboard. Autoplay auto-connects as a guest. Otherwise: when a login
/// field is focused (click it), typing edits it and TAB switches fields. ENTER logs in
/// with the typed account (creating it on first use). The party is mustered in town and
/// co-op starts there too, so neither is reachable from here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn join_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    mut session: ResMut<Session>,
    mut login: ResMut<LoginFocus>,
    mut status_q: Query<&mut Text, With<StatusText>>,
) {
    // Autoplay / headless: skip the login UI, connect as a throwaway guest.
    if autoplay.0 && !session.connecting {
        session.connecting = true;
        session.username = std::env::var("MELD_NAME")
            .unwrap_or_else(|_| format!("guest{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        session.password = net::GUEST_PASSWORD.to_string();
        session.status = "connecting...".to_string();
        net.0.send(ClientCmd::Connect {
            username: session.username.clone(),
            password: session.password.clone(),
        });
    }

    if !session.connecting {
        if let Some(f) = login.0 {
            // Typing into a login field.
            let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
            if keys.just_pressed(KeyCode::Tab) {
                login.0 = Some(1 - f);
            } else if keys.just_pressed(KeyCode::Backspace) {
                if f == 0 {
                    session.username.pop();
                } else {
                    session.password.pop();
                }
            } else {
                for key in keys.get_just_pressed() {
                    if let Some(c) = typed_char(*key, shift) {
                        let field = if f == 0 { &mut session.username } else { &mut session.password };
                        if field.chars().count() < 24 {
                            field.push(c);
                        }
                    }
                }
            }
        }

        // ENTER = log in & play. Co-op is NOT startable here: a lobby wants a party,
        // and the party is assembled in town, so starting one from the login screen
        // means picking teammates before you have picked heroes. The city's [C] is the
        // one way in.
        if keys.just_pressed(KeyCode::Enter) {
            let user = session.username.trim().to_string();
            if user.is_empty() {
                session.status = "Enter a username to log in.".to_string();
            } else if session.password.is_empty() {
                session.status = "Enter a password.".to_string();
                login.0 = Some(1);
            } else {
                session.connecting = true;
                session.coop = false;
                session.status = "logging in...".to_string();
                let password = session.password.clone();
                net.0.send(ClientCmd::Connect { username: user, password });
            }
        }
    }

    if let Ok(mut t) = status_q.single_mut() {
        **t = session.status.clone();
    }
}



/// Render the account login fields: username as typed, password masked, a caret on
/// the focused field, and a gold border on it.
#[allow(clippy::type_complexity)]
pub(crate) fn join_login_refresh(
    session: Res<Session>,
    login: Res<LoginFocus>,
    mut user_text: Query<&mut Text, (With<JoinUserText>, Without<JoinPassText>)>,
    mut pass_text: Query<&mut Text, (With<JoinPassText>, Without<JoinUserText>)>,
    mut user_border: Query<&mut BorderColor, (With<JoinUserField>, Without<JoinPassField>)>,
    mut pass_border: Query<&mut BorderColor, (With<JoinPassField>, Without<JoinUserField>)>,
) {
    let gold = Color::srgb(1.0, 0.85, 0.45);
    if let Ok(mut t) = user_text.single_mut() {
        let caret = if login.0 == Some(0) { "_" } else { "" };
        **t = format!("{}{caret}", session.username);
    }
    if let Ok(mut t) = pass_text.single_mut() {
        let masked: String = "\u{2022}".repeat(session.password.chars().count());
        let caret = if login.0 == Some(1) { "_" } else { "" };
        **t = format!("{masked}{caret}");
    }
    if let Ok(mut b) = user_border.single_mut() {
        *b = BorderColor(if login.0 == Some(0) { gold } else { glass(0.9) });
    }
    if let Ok(mut b) = pass_border.single_mut() {
        *b = BorderColor(if login.0 == Some(1) { gold } else { glass(0.9) });
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
            // Tap actions. Which are visible depends on lobby state (`render_lobby`
            // toggles them): Create before you're in a lobby; Ready/Start/Leave once
            // you are. Typing/joining by CODE stays on the keyboard (text entry).
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(12.0),
                margin: UiRect::top(Val::Px(8.0)),
                ..default()
            })
            .with_children(|row| {
                for (act, label) in [
                    (LobbyAct::Create, "Create Lobby"),
                    (LobbyAct::Ready, "Ready"),
                    (LobbyAct::Start, "Start"),
                    (LobbyAct::Leave, "Leave"),
                ] {
                    lobby_button(row, act, label);
                }
            });
        });
}

/// A tap action in the co-op lobby (mirrors the keyboard, except code entry).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum LobbyAct {
    Create,
    Ready,
    Start,
    Leave,
}

/// Marks a tappable lobby button; `render_lobby` shows/hides it per lobby state.
#[derive(Component)]
pub(crate) struct LobbyButton(pub(crate) LobbyAct);

/// Spawn one lobby button (starts hidden; `render_lobby` reveals the relevant ones).
fn lobby_button(parent: &mut ChildSpawnerCommands, act: LobbyAct, label: &str) {
    parent
        .spawn((
            Button,
            LobbyButton(act),
            Node {
                display: Display::None,
                padding: UiRect::axes(Val::Px(18.0), Val::Px(10.0)),
                border: UiRect::all(Val::Px(1.5)),
                ..default()
            },
            BorderColor(glass::EDGE_SOFT),
            BorderRadius::all(Val::Px(8.0)),
            BackgroundColor(glass::GLASS),
        ))
        .with_children(|b| {
            b.spawn((
                Text::new(label.to_string()),
                TextFont { font_size: 18.0, ..default() },
                TextColor(Color::srgb(0.9, 0.94, 1.0)),
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

#[allow(clippy::type_complexity)]
pub(crate) fn render_lobby(
    lobby: Res<LobbyData>,
    session: Res<Session>,
    mut q: Query<&mut Text, With<LobbyText>>,
    mut btns: Query<(&LobbyButton, &mut Node), Without<LobbyText>>,
) {
    // Reveal only the buttons that apply to the current lobby state: Create before
    // you're in a lobby; Ready/Leave once in; Start only for the host.
    let host_is_me = lobby.host == session.player_id;
    for (btn, mut node) in &mut btns {
        let show = match btn.0 {
            LobbyAct::Create => !lobby.in_lobby,
            LobbyAct::Ready | LobbyAct::Leave => lobby.in_lobby,
            LobbyAct::Start => lobby.in_lobby && host_is_me,
        };
        node.display = if show { Display::Flex } else { Display::None };
    }
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
        if all_ready { "ENTER: start the run" } else { "ENTER: start (need everyone READY)" }
    } else {
        "waiting for the host to start..."
    };
    lines.push(format!("R: toggle ready    {start}    ESC: leave"));
    **t = lines.join("\n");
}

/// Tap handler for the lobby buttons — same effects as [`lobby_input`] (except code
/// entry, which stays on the keyboard).
#[allow(clippy::too_many_arguments)]
pub(crate) fn lobby_buttons(
    q: Query<(&Interaction, &LobbyButton), Changed<Interaction>>,
    net: NonSend<NetRes>,
    session: Res<Session>,
    mut lobby: ResMut<LobbyData>,
    mut next: ResMut<NextState<Screen>>,
) {
    for (interaction, btn) in &q {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match btn.0 {
            LobbyAct::Create => {
                net.0.send(ClientCmd::LobbyCreate { party: session.party.clone() });
            }
            LobbyAct::Ready => {
                let want = !lobby.my_ready;
                lobby.my_ready = want;
                net.0.send(ClientCmd::LobbyReady { ready: want });
            }
            LobbyAct::Start => {
                if lobby.host == session.player_id {
                    net.0.send(ClientCmd::LobbyStart);
                }
            }
            LobbyAct::Leave => {
                net.0.send(ClientCmd::LobbyLeave);
                lobby.in_lobby = false;
                lobby.code_input.clear();
                next.set(Screen::City);
            }
        }
    }
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
                Text::new("Tap a button below, or press ENTER to return / ESC to quit"),
                TextFont {
                    font_size: 18.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));
            // Tap equivalents of Enter / Esc, so the summary is click/tap driven too.
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(14.0),
                margin: UiRect::top(Val::Px(6.0)),
                ..default()
            })
            .with_children(|row| {
                ended_button(row, EndedAct::Continue, "Return to The Last City", Color::srgb(0.35, 0.55, 0.85));
                ended_button(row, EndedAct::Quit, "Quit", Color::srgb(0.55, 0.3, 0.3));
            });
        });
}

/// A tap action on the run-summary screen (mirrors ENTER / ESC).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum EndedAct {
    Continue,
    Quit,
}

/// Marks a tappable button on the Ended screen.
#[derive(Component)]
pub(crate) struct EndedButton(pub(crate) EndedAct);

/// Spawn one Ended-screen button.
fn ended_button(parent: &mut ChildSpawnerCommands, act: EndedAct, label: &str, bg: Color) {
    parent
        .spawn((
            Button,
            EndedButton(act),
            Node {
                padding: UiRect::axes(Val::Px(20.0), Val::Px(12.0)),
                border: UiRect::all(Val::Px(1.5)),
                ..default()
            },
            BorderColor(Color::srgb(0.5, 0.6, 0.85)),
            BorderRadius::all(Val::Px(8.0)),
            BackgroundColor(bg),
        ))
        .with_children(|b| {
            b.spawn((
                Text::new(label.to_string()),
                TextFont { font_size: 18.0, ..default() },
                TextColor(Color::srgb(0.95, 0.97, 1.0)),
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

/// Tap handler for the Ended screen buttons — same effects as [`ended_input`].
pub(crate) fn ended_buttons(
    q: Query<(&Interaction, &EndedButton), Changed<Interaction>>,
    mut session: ResMut<Session>,
    mut next: ResMut<NextState<Screen>>,
    mut exit: EventWriter<AppExit>,
) {
    for (interaction, btn) in &q {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match btn.0 {
            EndedAct::Continue => {
                session.channeling = false;
                session.status.clear();
                next.set(Screen::City);
            }
            EndedAct::Quit => {
                exit.write(AppExit::Success);
            }
        }
    }
}

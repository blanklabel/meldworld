//! Simple full-screen screens: Join (party builder), co-op Lobby, and the
//! Ended (extract/death) summary.
//! Extracted from `main.rs` during the module reorg.


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
}

pub(crate) const CLASS_INFO: [ClassInfo; 8] = [
    ClassInfo { key: "explorer", name: "Explorer", role: "The order that maps and anchors the world \u{2014} tempo and stability, not burst.", hp: 4, atk: 3, spd: 3, mag: 2, def: 3 },
    ClassInfo { key: "hunter", name: "Hunter", role: "The guild that disposes of dangerous creatures \u{2014} the martial baseline.", hp: 4, atk: 4, spd: 3, mag: 1, def: 3 },
    ClassInfo { key: "psyker", name: "Psyker", role: "Psychic channeler. Weaves persistent Foci from the back row.", hp: 2, atk: 1, spd: 3, mag: 5, def: 2 },
    ClassInfo { key: "resonant", name: "Resonant", role: "Healer. Innate Regen keeps the party standing.", hp: 3, atk: 2, spd: 3, mag: 4, def: 2 },
    ClassInfo { key: "shifter", name: "Shifter", role: "Rogue skirmisher. Fast, fragile, the only innate dodge.", hp: 2, atk: 4, spd: 5, mag: 1, def: 1 },
    ClassInfo { key: "phoenix_guard", name: "Phoenix Guard", role: "The Last City's anti-undead order \u{2014} a wall that hits the risen hardest.", hp: 5, atk: 3, spd: 1, mag: 1, def: 5 },
    ClassInfo { key: "smithwright", name: "Smithwright", role: "The Foundry's builder \u{2014} raises the field forge, and buys the party time.", hp: 4, atk: 4, spd: 2, mag: 1, def: 4 },
    ClassInfo { key: "keeper", name: "Keeper", role: "Open Flower grower \u{2014} sets up the still, and keeps everyone standing.", hp: 2, atk: 1, spd: 4, mag: 5, def: 2 },
];

pub(crate) fn class_info(key: &str) -> &'static ClassInfo {
    CLASS_INFO.iter().find(|c| c.key == key).unwrap_or(&CLASS_INFO[0])
}

/// The kit as a multi-line "Skills\n  Name — what it does" block for the detail panel,
/// read from [`meld_proto::skills`] — the one registry the server gates on and the battle
/// menu builds from. It used to be a hand-copied list on each `ClassInfo`, which had
/// already drifted: the Explorer's card still named "Set Anchor" and put its rungs at
/// Lv2/5/9 when the registry says 4/9/16.
pub(crate) fn kit_text(ci: &ClassInfo) -> String {
    let mut s = String::from("Skills");
    for def in meld_proto::skills::skills_for_class(ci.key).iter().take(KIT_ROWS) {
        let at = if def.unlock > 1 {
            format!(" (Lv{})", def.unlock)
        } else {
            String::new()
        };
        s.push_str(&format!("\n  {} \u{2014} {}{at}", def.name, def.description));
    }
    // Say what is behind the cut. Every ladder now runs to level 100, so a card that
    // silently showed four of eight read as "this is the whole class".
    let rest = meld_proto::skills::skills_for_class(ci.key).len().saturating_sub(KIT_ROWS);
    if rest > 0 {
        let top = meld_proto::skills::ladder_top(meld_proto::skills::archetype(ci.key));
        s.push_str(&format!("\n  ...and {rest} more, out to level {top}"));
    }
    s
}

/// Kit rows the Join screen's card shows before it runs out of room.
const KIT_ROWS: usize = 4;


/// Which account-login field is being typed into: 0 = username, 1 = password, None =
/// no field (so 1-4 / arrows still drive the class picker).
#[derive(Resource, Default)]
pub(crate) struct LoginFocus(pub Option<u8>);

/// The login screen's looping backdrop. Bevy plays no video, so the source clip is
/// baked into a WebP frame sequence (`assets/loginscreens/`) and stepped here. The
/// clip is a slow push-in, which would jump on a plain loop, so it plays **ping-pong**
/// — forwards, then backwards — and joins itself seamlessly.
///
/// The handles live here only while the Join screen is up, so the frame textures are
/// handed back to the GPU on log-in.
#[derive(Resource, Default)]
pub(crate) struct LoginBg {
    frames: Vec<Handle<Image>>,
    idx: usize,
    forward: bool,
    t: f32,
}

const LOGIN_BG_FRAMES: usize = 120;
const LOGIN_BG_FPS: f32 = 12.0;
const LOGIN_BG_ASPECT: f32 = 16.0 / 9.0;

#[derive(Component)]
pub(crate) struct LoginBgImage;

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
                TextColor(glass::DIM),
            ));
            r.spawn((
                Button,
                field_tag,
                Node {
                    width: Val::Px(180.0),
                    height: Val::Px(30.0),
                    align_items: AlignItems::Center,
                    padding: UiRect::horizontal(Val::Px(8.0)),
                    border: UiRect::all(Val::Px(1.5)),
                    ..default()
                },
                BorderColor(glass::EDGE_SOFT),
                BackgroundColor(glass::GLASS_DEEP),
                BorderRadius::all(Val::Px(6.0)),
            ))
            .with_children(|b| {
                b.spawn((
                    Text::new(String::new()),
                    text_tag,
                    TextFont { font_size: 15.0, ..default() },
                    TextColor(glass::TEXT),
                ));
            });
        });
}

#[allow(clippy::type_complexity)]
pub(crate) fn join_ui(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut bg: ResMut<LoginBg>,
    mut login: ResMut<LoginFocus>,
) {
    bg.frames = (0..LOGIN_BG_FRAMES)
        .map(|i| assets.load(format!("loginscreens/gears_and_forest/frame{i:03}.webp")))
        .collect();
    bg.idx = 0;
    bg.forward = true;
    bg.t = 0.0;

    // A field is focused up front, so the first thing a player types lands in the
    // username instead of being swallowed (they should not have to find the click).
    login.0 = Some(0);

    commands
        .spawn((
            JoinRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                LoginBgImage,
                ImageNode::new(bg.frames[0].clone()).with_color(BG_TINT),
                Node { position_type: PositionType::Absolute, ..default() },
                BackgroundColor(Color::BLACK),
                ZIndex(0),
            ));
            // The backdrop is a busy painting, so the panel needs a real scrim under
            // it — glass alone leaves 12 px hint text fighting bark and gear teeth.
            p.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(glass::SCRIM),
                ZIndex(1),
            ));

            p.spawn((
                Node {
                    width: Val::Px(560.0),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(7.0),
                    padding: UiRect::axes(Val::Px(28.0), Val::Px(24.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(glass::GLASS),
                BorderColor(glass::EDGE_SOFT),
                BorderRadius::all(Val::Px(14.0)),
                ZIndex(2),
            ))
            .with_children(|p| {
                p.spawn((
                    Text::new("MELDWORLD"),
                    TextFont { font_size: 44.0, ..default() },
                    TextColor(glass::TITLE),
                ));
                p.spawn((
                    Text::new("Log in \u{2014} then muster your party in the Last City."),
                    TextFont { font_size: 17.0, ..default() },
                    TextColor(glass::DIM),
                    Node { margin: UiRect::bottom(Val::Px(6.0)), ..default() },
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
                    Text::new("Type, TAB to switch field, ENTER to log in \u{2014} first login creates your account."),
                    TextFont { font_size: 13.0, ..default() },
                    TextColor(glass::DIM),
                ));
                p.spawn((
                    Text::new("username 3\u{2013}20 \u{b7} password 8+ characters"),
                    TextFont { font_size: 12.0, ..default() },
                    TextColor(Color::srgba(0.72, 0.78, 0.9, 0.6)),
                ));

                // The season's Vanguard Board. The party used to be built here, but this
                // screen runs BEFORE login: it cannot know which classes the account owns,
                // so it could only offer all six and let the server clamp the answer. The
                // party is mustered in town (the Drill Yard), where the unlock set is
                // known. What belongs on a login screen is a reason to log in.
                p.spawn((
                    Text::new("The Vanguard \u{2014} deepest of the season"),
                    TextFont { font_size: 16.0, ..default() },
                    TextColor(glass::TITLE),
                    Node { margin: UiRect::top(Val::Px(12.0)), ..default() },
                ));
                p.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(3.0),
                        padding: UiRect::all(Val::Px(14.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor(glass::EDGE_SOFT),
                    BackgroundColor(glass::GLASS_DEEP),
                    BorderRadius::all(Val::Px(12.0)),
                ))
                .with_children(|d| {
                    d.spawn((
                        Text::new("reading the board..."),
                        JoinBoardText,
                        TextFont { font_size: 14.0, ..default() },
                        TextColor(glass::TEXT),
                    ));
                });

                p.spawn((
                    StatusText,
                    Text::new(""),
                    TextFont { font_size: 15.0, ..default() },
                    TextColor(glass::WARN),
                    Node { margin: UiRect::top(Val::Px(8.0)), ..default() },
                ));
            });
        });
}

/// How far the backdrop is knocked down before any UI sits on it.
const BG_TINT: Color = Color::srgb(0.62, 0.62, 0.68);

/// Ping-pong the baked frame sequence behind the login panel.
pub(crate) fn login_bg_play(
    time: Res<Time>,
    images: Res<Assets<Image>>,
    mut bg: ResMut<LoginBg>,
    mut q: Query<&mut ImageNode, With<LoginBgImage>>,
) {
    let Ok(mut img) = q.single_mut() else { return };
    if bg.frames.is_empty() {
        return;
    }
    bg.t += time.delta_secs();
    while bg.t >= 1.0 / LOGIN_BG_FPS {
        bg.t -= 1.0 / LOGIN_BG_FPS;
        if bg.forward {
            if bg.idx + 1 >= bg.frames.len() {
                bg.forward = false;
            } else {
                bg.idx += 1;
            }
        } else if bg.idx == 0 {
            bg.forward = true;
        } else {
            bg.idx -= 1;
        }
    }
    // Hold the frame on screen until its successor has actually decoded — the 120
    // loads land over the first moment or two, and swapping to a pending handle
    // draws nothing at all (a black flash) rather than an old frame.
    let frame = bg.frames[bg.idx].clone();
    if img.image != frame && images.contains(&frame) {
        img.image = frame;
    }
}

/// Cover-fit the backdrop to the window: fill it in both axes and centre the
/// overflow, so the clip never stretches to the window's aspect.
pub(crate) fn login_bg_fit(
    window: Query<&Window>,
    mut q: Query<&mut Node, With<LoginBgImage>>,
) {
    let (Ok(win), Ok(mut node)) = (window.single(), q.single_mut()) else { return };
    let (ww, wh) = (win.width(), win.height());
    let (w, h) = if ww / wh > LOGIN_BG_ASPECT {
        (ww, ww / LOGIN_BG_ASPECT)
    } else {
        (wh * LOGIN_BG_ASPECT, wh)
    };
    node.width = Val::Px(w);
    node.height = Val::Px(h);
    node.left = Val::Px((ww - w) * 0.5);
    node.top = Val::Px((wh - h) * 0.5);
}

/// Drop the frame handles on leaving the login screen so their textures are freed.
pub(crate) fn login_bg_unload(mut bg: ResMut<LoginBg>) {
    bg.frames.clear();
}

/// This frame's login keyboard, as plain data.
pub(crate) struct LoginKeys<'a> {
    pub tab: bool,
    pub backspace: bool,
    pub shift: bool,
    pub typed: &'a [KeyCode],
}

/// The longest an account name or password may be typed to.
const LOGIN_FIELD_MAX: usize = 24;

/// Apply one frame of the login keyboard to the focused field. Split out of
/// [`join_input`] so the typing rules can be tested without a window.
pub(crate) fn edit_login_field(
    focus: &mut Option<u8>,
    keys: LoginKeys,
    username: &mut String,
    password: &mut String,
) {
    // TAB is the keyboard's only way INTO the fields, so it has to work from an
    // unfocused screen too — otherwise the login is unusable without a mouse.
    if keys.tab {
        *focus = Some(1 - focus.unwrap_or(1));
        return;
    }
    let Some(f) = *focus else { return };
    let field = if f == 0 { username } else { password };
    if keys.backspace {
        field.pop();
        return;
    }
    for key in keys.typed {
        if let Some(c) = typed_char(*key, keys.shift) {
            if field.chars().count() < LOGIN_FIELD_MAX {
                field.push(c);
            }
        }
    }
}

/// Click-to-focus for the login fields. Nothing else ever sets [`LoginFocus`], so
/// without this every keystroke on the login screen is discarded.
#[allow(clippy::type_complexity)]
pub(crate) fn join_field_click(
    user: Query<&Interaction, (Changed<Interaction>, With<JoinUserField>)>,
    pass: Query<&Interaction, (Changed<Interaction>, With<JoinPassField>)>,
    mut login: ResMut<LoginFocus>,
) {
    if user.iter().any(|i| *i == Interaction::Pressed) {
        login.0 = Some(0);
    }
    if pass.iter().any(|i| *i == Interaction::Pressed) {
        login.0 = Some(1);
    }
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
        let s = &mut *session;
        edit_login_field(
            &mut login.0,
            LoginKeys {
                tab: keys.just_pressed(KeyCode::Tab),
                backspace: keys.just_pressed(KeyCode::Backspace),
                shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
                typed: &keys.get_just_pressed().copied().collect::<Vec<_>>(),
            },
            &mut s.username,
            &mut s.password,
        );

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
    let gold = glass::EDGE;
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
        *b = BorderColor(if login.0 == Some(0) { gold } else { glass::EDGE_SOFT });
    }
    if let Ok(mut b) = pass_border.single_mut() {
        *b = BorderColor(if login.0 == Some(1) { gold } else { glass::EDGE_SOFT });
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
            // The wipe's bill, itemised per hero. A TPK is the largest durability charge
            // in the game and the one a player is least able to infer, since every hero
            // in the party paid it at once.
            for (hero, points) in &end.worn {
                p.spawn((
                    Text::new(format!("{hero} fell: kit worn -{points} durability")),
                    TextFont {
                        font_size: 18.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.95, 0.75, 0.35)),
                ));
            }
            if !end.worn.is_empty() {
                p.spawn((
                    Text::new("Repair at the Forge before your next dive.".to_string()),
                    TextFont {
                        font_size: 16.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.7, 0.75, 0.85)),
                ));
            }
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

#[cfg(test)]
mod tests {

    /// Every class card's kit comes from the ONE registry, so a card can never again name
    /// an ability that was renamed or put its unlocks at the wrong level — the Explorer's
    /// card still said "Set Anchor" at Lv9 long after the registry said otherwise.
    #[test]
    fn every_class_card_reads_its_kit_from_the_registry() {
        for ci in CLASS_INFO.iter() {
            let defs = meld_proto::skills::skills_for_class(ci.key);
            assert!(!defs.is_empty(), "{} has no abilities in the registry", ci.key);
            let text = kit_text(ci);
            for def in defs.iter().take(KIT_ROWS) {
                assert!(
                    text.contains(def.name),
                    "{}'s card omits {} - the card and the registry have drifted",
                    ci.key,
                    def.name
                );
            }
        }
    }

    /// And the ability that used to be called "Set Anchor" is gone from the setting: an
    /// Anchor takes three orders to make and only an Explorer of Serin may set one, so a
    /// routine party Barrier must not borrow the name (docs/lore/factions.md).
    #[test]
    fn no_class_claims_to_set_an_anchor() {
        for ci in CLASS_INFO.iter() {
            let text = kit_text(ci);
            assert!(!text.contains("Set Anchor"), "{} still claims to set Anchors", ci.key);
        }
        assert!(
            kit_text(class_info("explorer")).contains("Stable Ground"),
            "the Explorer should offer Stable Ground instead"
        );
    }

    use super::*;

    fn keys(typed: &[KeyCode]) -> LoginKeys<'_> {
        LoginKeys { tab: false, backspace: false, shift: false, typed }
    }

    #[test]
    fn tab_reaches_the_fields_from_an_unfocused_screen() {
        let mut focus = None;
        let (mut u, mut p) = (String::new(), String::new());
        edit_login_field(&mut focus, LoginKeys { tab: true, ..keys(&[]) }, &mut u, &mut p);
        assert_eq!(focus, Some(0), "TAB with nothing focused must land on the username");
        edit_login_field(&mut focus, LoginKeys { tab: true, ..keys(&[]) }, &mut u, &mut p);
        assert_eq!(focus, Some(1), "TAB again crosses to the password");
        edit_login_field(&mut focus, LoginKeys { tab: true, ..keys(&[]) }, &mut u, &mut p);
        assert_eq!(focus, Some(0), "and back — the two fields cycle");
    }

    #[test]
    fn typing_lands_in_the_focused_field_only() {
        let mut focus = Some(0);
        let (mut u, mut p) = (String::new(), String::new());
        edit_login_field(&mut focus, keys(&[KeyCode::KeyA, KeyCode::KeyB]), &mut u, &mut p);
        assert_eq!((u.as_str(), p.as_str()), ("ab", ""));

        focus = Some(1);
        edit_login_field(&mut focus, keys(&[KeyCode::Digit7]), &mut u, &mut p);
        assert_eq!((u.as_str(), p.as_str()), ("ab", "7"));
    }

    #[test]
    fn shift_uppercases_and_backspace_deletes_one() {
        let mut focus = Some(0);
        let (mut u, mut p) = (String::new(), String::new());
        edit_login_field(
            &mut focus,
            LoginKeys { shift: true, ..keys(&[KeyCode::KeyD]) },
            &mut u,
            &mut p,
        );
        edit_login_field(&mut focus, keys(&[KeyCode::KeyO]), &mut u, &mut p);
        assert_eq!(u, "Do");
        edit_login_field(&mut focus, LoginKeys { backspace: true, ..keys(&[]) }, &mut u, &mut p);
        assert_eq!(u, "D");
        assert!(p.is_empty(), "backspace must not reach across fields");
    }

    #[test]
    fn a_field_stops_growing_at_its_cap() {
        let mut focus = Some(0);
        let (mut u, mut p) = ("x".repeat(LOGIN_FIELD_MAX), String::new());
        edit_login_field(&mut focus, keys(&[KeyCode::KeyA]), &mut u, &mut p);
        assert_eq!(u.chars().count(), LOGIN_FIELD_MAX);
    }

    #[test]
    fn an_unfocused_screen_swallows_typing() {
        let mut focus = None;
        let (mut u, mut p) = (String::new(), String::new());
        edit_login_field(&mut focus, keys(&[KeyCode::KeyA]), &mut u, &mut p);
        assert!(u.is_empty() && p.is_empty());
        assert_eq!(focus, None, "which is why join_ui focuses a field up front");
    }
}

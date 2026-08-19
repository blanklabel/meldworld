//! Launch-time flags: server URL + `MELD_*`/`?query` toggles (native vs wasm).
//! Extracted from `main.rs` during the module reorg.

/// Where the API + realtime socket live. Native: `MELD_SERVER` env (default
/// localhost). Browser: the page origin (trunk proxies `/v1` to the server).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn server_base() -> String {
    std::env::var("MELD_SERVER").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn server_base() -> String {
    let win = web_sys::window();
    let search = win
        .as_ref()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    // `?server=<url>` override (for the local demo); else the page origin.
    if let Ok(params) = web_sys::UrlSearchParams::new_with_str(&search) {
        if let Some(s) = params.get("server") {
            if !s.is_empty() {
                return s;
            }
        }
    }
    win.and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
}

/// Autopilot self-drives the loop (connect → walk → attack) for demos and
/// headless screenshots. Native: `MELD_AUTOPLAY` env. Browser: `?autoplay` in
/// the URL. Real players use the keyboard as normal.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn autoplay_flag() -> bool {
    std::env::var("MELD_AUTOPLAY").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn autoplay_flag() -> bool {
    query_has("autoplay")
}
/// With autoplay, enter the maze but **idle** at the hub instead of walking east —
/// a stable overworld frame for screenshotting the world art. `MELD_IDLE` / `?idle`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn world_idle_flag() -> bool {
    std::env::var("MELD_IDLE").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn world_idle_flag() -> bool {
    query_has("idle")
}

/// Connect, then **park in The Last City** (the hub city) instead of diving — a stable
/// City frame for screenshotting / iterating on the hub. Reuses the autoplay
/// connect path but gates the auto-dive. Native: `MELD_CITY`. Browser: `?city`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn city_idle_flag() -> bool {
    std::env::var("MELD_CITY").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn city_idle_flag() -> bool {
    query_has("city")
}

/// Open the Apothecary's shelf on arrival (with `MELD_CITY`) — a stable frame for
/// screenshotting the shop without walking to the district. Native: `MELD_SHOP`.
/// Browser: `?shop`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn shop_preview_flag() -> bool {
    std::env::var("MELD_SHOP").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn shop_preview_flag() -> bool {
    query_has("shop")
}

/// Open the Forge & Alembic's recipe book on arrival (with `MELD_CITY`) — a stable
/// frame for screenshotting the crafting panel without walking to the district.
/// Native: `MELD_FORGE`. Browser: `?forge`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn forge_preview_flag() -> bool {
    std::env::var("MELD_FORGE").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn forge_preview_flag() -> bool {
    query_has("forge")
}

/// Seed a fake open HEAT on arrival (with `MELD_CITY`) so the smithing bar can be
/// screenshotted without diving, gathering and building a station first. Native:
/// `MELD_HEAT`. Browser: `?heat`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn heat_preview_flag() -> bool {
    std::env::var("MELD_HEAT").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn heat_preview_flag() -> bool {
    query_has("heat")
}

/// Seed a fake extraction TALLY on arrival so the haul screen (icons + counts) can be
/// screenshotted without completing a dive. Native: `MELD_TALLY`. Browser: `?tally`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn tally_preview_flag() -> bool {
    std::env::var("MELD_TALLY").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn tally_preview_flag() -> bool {
    query_has("tally")
}

/// Light the Vanguard Wall on arrival (with `MELD_CITY`) — a stable frame for
/// screenshotting the seasonal board without having to walk over and press [E].
/// Native: `MELD_WALL`. Browser: `?wall`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn wall_preview_flag() -> bool {
    std::env::var("MELD_WALL").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn wall_preview_flag() -> bool {
    query_has("wall")
}

/// PICK a counter row on arrival, so the detail column's description + amount + commit
/// buttons are on screen for a screenshot. The buy flow is two steps by design (a row picks,
/// the third column commits), and the second step is exactly the half a capture cannot reach
/// without a click. `MELD_PICK=<row>` / `?pick=<row>`, 0-based.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn pick_preview_flag() -> Option<usize> {
    std::env::var("MELD_PICK").ok().and_then(|v| v.parse().ok())
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn pick_preview_flag() -> Option<usize> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("pick").and_then(|v| v.parse().ok())
}

/// Open the Bounty Board's hunts on arrival in Last City (AD-4) — a stable frame for
/// screenshots. Native: `MELD_HUNTS=1`. Browser: `?hunts`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn hunts_preview_flag() -> bool {
    std::env::var("MELD_HUNTS").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn hunts_preview_flag() -> bool {
    query_has("hunts")
}

/// Preview a boss/elite sprite in The Last City plaza (with `MELD_CITY`) — a stable
/// frame for eyeballing the encounter art. Native: `MELD_BOSS=ironmaw`. Browser:
/// `?boss=ironmaw`. See `world_render::BOSS_KEYS` for valid ids.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn boss_preview() -> Option<String> {
    std::env::var("MELD_BOSS").ok().filter(|s| !s.is_empty())
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn boss_preview() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("boss").filter(|s| !s.is_empty())
}

/// Offline render demo: no networking; scripted canned data drives the real
/// rendering so the Overworld/Battle screens can be shown without a server.
/// Native: `MELD_DEMO` env. Browser: `?demo`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn demo_flag() -> bool {
    std::env::var("MELD_DEMO").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn demo_flag() -> bool {
    query_has("demo")
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn query_has(key: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|s| s.contains(key))
        .unwrap_or(false)
}

/// Pre-select a class without the Join screen (handy for demos/headless runs and
/// with `?autoplay`). Native: `MELD_CLASS` env. Browser: `?class=psyker`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn class_flag() -> Option<String> {
    std::env::var("MELD_CLASS").ok().filter(|s| !s.is_empty())
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn class_flag() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("class").filter(|s| !s.is_empty())
}

/// Pre-build the whole party (comma-separated class keys) without the builder.
/// Native: `MELD_PARTY=explorer,psyker,resonant,explorer`. Browser: `?party=…`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn party_flag() -> Option<String> {
    std::env::var("MELD_PARTY").ok().filter(|s| !s.is_empty())
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn party_flag() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("party").filter(|s| !s.is_empty())
}

/// Offline battle-screen mockup: jump straight into the Battle screen with canned
/// combatants and the command window open, so the subscreen can be inspected
/// without a server or walking there. Native: `MELD_BATTLE` env. Browser: `?battle`.
///
/// `MELD_BATTLE=coop` adds joined ally parties (the surround layout); `=skills` opens
/// the Skill page, which is the only one that draws a tooltip — an ability's prose and
/// its magnitudes are otherwise unreachable in a screenshot without driving a fight.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn battle_mockup_flag() -> bool {
    std::env::var("MELD_BATTLE").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn battle_mockup_flag() -> bool {
    query_has("battle")
}

/// Offline mockups for the overworld overlays (`?inventory` / `?levelup`, or
/// `MELD_INVENTORY` / `MELD_LEVELUP`).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn inventory_mockup_flag() -> bool {
    std::env::var("MELD_INVENTORY").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn inventory_mockup_flag() -> bool {
    query_has("inventory")
}
/// Which tab (and, for Equip, which category picker) the `MELD_INVENTORY` mockup
/// opens on — so a gear-UI change can be screenshot-verified without a server or
/// a mouse. `MELD_INVENTORY_TAB=equip|items|status` / `?inventory_tab=`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn inventory_tab_flag() -> Option<String> {
    std::env::var("MELD_INVENTORY_TAB").ok().filter(|s| !s.is_empty())
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn inventory_tab_flag() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("inventory_tab").filter(|s| !s.is_empty())
}

/// `MELD_UNLOCK=<key>` / `?unlock=<key>` — queue that unlock's banner (and the
/// locked-roster rows) with no server, so the CL-1 presentation can be
/// screenshot-verified. Any key from `meld_proto::unlocks::UNLOCKS`.
/// `MELD_MENU=party|party.abilities|party.equipment|items|materials|map` /
/// `?menu=` — open the three-column cascade at that column with no server, so each
/// depth can be screenshot-verified.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn menu_flag() -> Option<String> {
    std::env::var("MELD_MENU").ok().filter(|s| !s.is_empty())
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn menu_flag() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("menu").filter(|s| !s.is_empty())
}

/// `MELD_HERO_LEVEL=<n>` / `?hero_level=` — the level the mock roster's heroes are at.
/// Defaults to 1. The deep ability rungs land at 49 and 100, so without this the
/// mocked Abilities pane can only ever show a level-1 kit and the rows added out
/// there cannot be screenshot-verified at all.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn hero_level_flag() -> i32 {
    std::env::var("MELD_HERO_LEVEL").ok().and_then(|v| v.parse().ok()).unwrap_or(1)
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn hero_level_flag() -> i32 {
    (|| {
        let search = web_sys::window()?.location().search().ok()?;
        let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
        params.get("hero_level")?.parse().ok()
    })()
    .unwrap_or(1)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn unlock_mockup_flag() -> Option<String> {
    std::env::var("MELD_UNLOCK").ok().filter(|s| !s.is_empty())
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn unlock_mockup_flag() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("unlock").filter(|s| !s.is_empty())
}

/// `MELD_FEEL="lunge_ttl=0.5,number_rise=70"` / `?feel=` — override any
/// [`crate::BattleFeel`] knob at launch, so the combat feel can be dialed in against a
/// running fight instead of a recompile per guess.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn feel_flag() -> Option<String> {
    std::env::var("MELD_FEEL").ok().filter(|s| !s.is_empty())
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn feel_flag() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("feel").filter(|s| !s.is_empty())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn world_feel_flag() -> Option<String> {
    std::env::var("MELD_WORLD_FEEL").ok().filter(|s| !s.is_empty())
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn world_feel_flag() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("worldfeel").filter(|s| !s.is_empty())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn levelup_mockup_flag() -> bool {
    std::env::var("MELD_LEVELUP").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn levelup_mockup_flag() -> bool {
    query_has("levelup")
}
/// Offline mockup for the animated "LEVEL UP!" stat screen (`?levelup_anim` /
/// `MELD_LEVELUP_ANIM`) — seeds a canned level-up so the sequence can be
/// screenshotted without a battle.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn levelup_anim_mockup_flag() -> bool {
    std::env::var("MELD_LEVELUP_ANIM").is_ok()
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn levelup_anim_mockup_flag() -> bool {
    query_has("levelup_anim")
}

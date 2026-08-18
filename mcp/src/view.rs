//! The game as text, because that is the screen a tool has.
//!
//! Every view answers "what can I do next" as directly as the HUD does: a thing in the
//! world is printed with the id a tool takes, and a hero is printed with the slot a tool
//! takes. A view that reads well but cannot be acted on is a screenshot, not an interface.

use crate::session::{Battle, Side, State};

/// Bearing as a compass point, which is far easier to hold in mind than a signed pair.
fn bearing(dx: f64, dy: f64) -> &'static str {
    let a = dy.atan2(dx).to_degrees();
    match a {
        a if !(-157.5..157.5).contains(&a) => "W",
        a if a < -112.5 => "SW",
        a if a < -67.5 => "S",
        a if a < -22.5 => "SE",
        a if a < 22.5 => "E",
        a if a < 67.5 => "NE",
        a if a < 112.5 => "N",
        _ => "NW",
    }
}

/// Where you are, what is around you, and what your party is carrying.
pub fn look(s: &State) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "d{} at ({:.0}, {:.0}){}\n",
        s.distance(),
        s.pos.0,
        s.pos.1,
        if s.steering.is_empty() {
            String::new()
        } else {
            format!(" — {}", s.steering)
        }
    ));
    if let Some(r) = &s.run_result {
        out.push_str(&format!("RUN OVER: {r}\n"));
    }
    out.push_str(&party_line(s));

    let mut near: Vec<_> = s
        .entities
        .iter()
        .filter(|e| e.id != s.player_id)
        .map(|e| (e.dist_to(s.pos), e))
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    if near.is_empty() {
        out.push_str("nothing in sight.\n");
    } else {
        out.push_str("nearby:\n");
        for (d, e) in near.iter().take(16) {
            let what = if e.is_mob() {
                format!(
                    "{} L{}{}",
                    e.kind(),
                    e.level,
                    match e.encounter_class.as_str() {
                        "standard" => String::new(),
                        c => format!(" [{c}]"),
                    }
                )
            } else {
                e.state.clone()
            };
            out.push_str(&format!(
                "  {:>5.0} {:<2}  {:<34} {}{}\n",
                d,
                bearing(e.x - s.pos.0, e.y - s.pos.1),
                what,
                e.id,
                // A terrace is not a decoration: nothing can be touched, harvested or
                // fought across an elevation change, and a creature 6 units away on the
                // level above reads as "right there" until this says otherwise.
                match e.elevation {
                    0 => String::new(),
                    l => format!("  (level {l})"),
                }
            ));
        }
        if near.len() > 16 {
            out.push_str(&format!("  … and {} more\n", near.len() - 16));
        }
        // Anything that is not an ordinary creature is listed WHATEVER its distance. A
        // champion, a Gatekeeper or the end fight is the most important thing in the
        // snapshot and also, being deliberately rare, the most likely to fall past a
        // nearest-16 cut — which reads as "it was never placed".
        let notable: Vec<&(f64, &crate::session::Ent)> = near
            .iter()
            .filter(|(_, e)| e.is_mob() && e.encounter_class != "standard")
            .collect();
        if !notable.is_empty() {
            out.push_str("of note:\n");
            for (d, e) in notable {
                out.push_str(&format!(
                    "  {:>5.0} {:<2}  {} L{} [{}]  {}\n",
                    d,
                    bearing(e.x - s.pos.0, e.y - s.pos.1),
                    e.kind(),
                    e.level,
                    e.encounter_class,
                    e.id
                ));
            }
        }
    }

    if !s.backpack.is_empty() {
        let mut items: Vec<_> = s.backpack.iter().collect();
        items.sort();
        out.push_str(&format!(
            "backpack: {}\n",
            items
                .iter()
                .map(|(k, n)| format!("{n}x {k}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out
}

/// One line per hero: who they are and how they are doing.
pub fn party_line(s: &State) -> String {
    if s.heroes.is_empty() {
        return String::new();
    }
    let mut out = String::from("party:\n");
    for (i, h) in s.heroes.iter().enumerate() {
        out.push_str(&format!(
            "  [{i}] {:<14} {:<14} L{:<4} {}/{} hp   str{} mnd{} dex{} wll{}\n",
            h["name"].as_str().unwrap_or("?"),
            h["class_key"].as_str().unwrap_or("?"),
            h["level"].as_i64().unwrap_or(0),
            h["hp"].as_i64().unwrap_or(0),
            h["max_hp"].as_i64().unwrap_or(0),
            h["str_"].as_i64().or_else(|| h["str"].as_i64()).unwrap_or(0),
            h["mnd"].as_i64().unwrap_or(0),
            h["dex"].as_i64().unwrap_or(0),
            h["wll"].as_i64().unwrap_or(0),
        ));
    }
    out
}

/// The arena: both sides, gauges, conditions, and whose window is open.
pub fn battle(b: &Battle) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "battle [{}] — {} turns taken, party at {}/{} hp\n",
        b.encounter_class,
        b.my_turns,
        b.party_hp(),
        b.opening_hp
    ));
    let row = |c: &crate::session::Comb, slot: Option<usize>| {
        let tag = match slot {
            Some(i) => format!("[{i}]"),
            None => "   ".to_string(),
        };
        let conds: Vec<&str> = c
            .statuses
            .iter()
            .filter(|s| {
                !s.starts_with("name:")
                    && !s.starts_with("class:")
                    && !s.starts_with("str:")
                    && !s.starts_with("mnd:")
                    && !s.starts_with("dex:")
                    && !s.starts_with("wll:")
            })
            .map(|s| s.as_str())
            .collect();
        format!(
            "  {tag} {:<16} {:<12} L{:<4} {:>6}/{:<6} gauge {:>4.0}%{}{}\n",
            c.name,
            c.class,
            c.level,
            c.hp.max(0),
            c.max_hp,
            c.gauge * 100.0,
            if c.alive() { "" } else { "  DOWN" },
            if conds.is_empty() {
                String::new()
            } else {
                format!("  {}", conds.join(" "))
            }
        )
    };
    out.push_str("yours:\n");
    let mut slot = 0;
    for c in b.combatants.iter().filter(|c| c.side == Side::Ally) {
        if c.mine {
            out.push_str(&row(c, Some(slot)));
            slot += 1;
        } else {
            out.push_str(&row(c, None));
        }
    }
    out.push_str("enemies:\n");
    for (i, c) in b.enemies().iter().enumerate() {
        out.push_str(&row(c, Some(i)));
    }
    if let Some(e) = &b.ended {
        out.push_str(&format!("ENDED: {e}\n"));
        if !b.loot.is_empty() {
            out.push_str(&format!("loot: {}\n", b.loot.join(", ")));
        }
    } else if b.ready.is_empty() {
        out.push_str("waiting: no hero's gauge is full yet.\n");
    } else {
        let who: Vec<String> = b
            .ready
            .iter()
            .filter_map(|id| b.get(id).map(|c| c.name.clone()))
            .collect();
        out.push_str(&format!("awaiting orders: {}\n", who.join(", ")));
    }
    out
}

/// What each hero can actually press right now, straight off the registry at their level.
pub fn abilities(s: &State) -> String {
    let mut out = String::new();
    for (i, h) in s.heroes.iter().enumerate() {
        let class = h["class_key"].as_str().unwrap_or("");
        let level = h["level"].as_i64().unwrap_or(1) as i32;
        out.push_str(&format!(
            "[{i}] {} — {class} L{level}\n",
            h["name"].as_str().unwrap_or("?")
        ));
        out.push_str("     attack, defend, flee\n");
        for sk in meld_proto::skills::skills_for_class_at(class, level) {
            out.push_str(&format!(
                "     {:<22} {:?}  {}\n",
                sk.key, sk.target, sk.description
            ));
            for asp in meld_proto::skills::aspects_of(sk.key) {
                if level >= asp.unlock {
                    out.push_str(&format!(
                        "       └ {:<20} {:?}  {}\n",
                        asp.key, asp.target, asp.description
                    ));
                }
            }
        }
    }
    out
}

//! Shared helpers for the headless bot tests.

use serde_json::Value;

/// Where a bot is, and what it should walk at — rebuilt from every `world.snapshot`.
///
/// A bot must STEER at prey rather than march due east. The CR-6 encounter ramp
/// deliberately thins the shallow ring, so a straight line out of the hub walks
/// *past* the sparse band and the bot times out having never touched a creature —
/// and it does so only sometimes, depending on where creatures happened to spawn,
/// which is a coin toss wearing a conformance test's clothes.
///
/// It must also steer at prey it can BEAT. Elites and Gatekeepers stand in the same
/// world as ordinary creatures and are sometimes the nearest thing to the hub; a
/// starting party that walks into one loses, and a test asserting `victory` fails on
/// the world seed rather than on the behaviour it is checking.
///
/// [`Nav::observe`] reads a snapshot; [`Nav::heading`] gives the direction to send as
/// `move_dir`.
#[derive(Debug, Default, Clone)]
pub struct Nav {
    /// This bot's own avatar position.
    pub pos: (f64, f64),
    /// Ordinary creatures currently on screen, weakest-then-nearest first. Elites and
    /// Gatekeepers are excluded — see [`Nav::tough`] for those.
    pub creatures: Vec<Target>,
    /// The elites and Gatekeepers that were filtered out, same ordering.
    pub tough: Vec<Target>,
}

/// A creature the bot can see: `(entity_id, x, y, level)`.
pub type Target = (String, f64, f64, i64);

impl Nav {
    /// Update from a `world.snapshot` payload. `my_entity_id` is this bot's player id —
    /// pass it, because with two bots in one instance "whichever entity is active"
    /// silently becomes the *other* bot and the steering inverts.
    pub fn observe(&mut self, payload: &Value, my_entity_id: &str) {
        let empty = Vec::new();
        let ents = payload["entities"].as_array().unwrap_or(&empty);
        for e in ents {
            if e["entity_id"].as_str() == Some(my_entity_id) && e["position"]["x"].is_number() {
                self.pos = (
                    e["position"]["x"].as_f64().unwrap_or(self.pos.0),
                    e["position"]["y"].as_f64().unwrap_or(self.pos.1),
                );
            }
        }
        let (mut found, mut tough) = (Vec::new(), Vec::new());
        for e in ents {
            if !e["avatar_state"]
                .as_str()
                .is_some_and(|s| s.starts_with("mob:"))
            {
                continue;
            }
            let at = (
                e["entity_id"].as_str().unwrap_or_default().to_string(),
                e["position"]["x"].as_f64().unwrap_or(0.0),
                e["position"]["y"].as_f64().unwrap_or(0.0),
                e["mob_level"].as_i64().unwrap_or(1),
            );
            // A missing class is treated as ordinary: absent intel should not empty
            // the target list and strand the bot walking east forever.
            match e["encounter_class"].as_str() {
                Some("elite") | Some("gatekeeper") => tough.push(at),
                _ => found.push(at),
            }
        }
        let here = self.pos;
        // Threat and travel, traded off against each other rather than ranked. Pure
        // proximity walks a starting party into the level-2 creature area 0 puts NEARER
        // the hub than the level-1 one it means you to meet first (a fight a solo hero
        // loses two times in three). But letting level dominate is worse: the bot then
        // chases the weakest creature on the map, never arrives, and the run times out.
        // A level is worth `LEVEL_COST` world units of walking, so a slightly tougher
        // creature underfoot still beats a pushover far away.
        const LEVEL_COST: f64 = 40.0;
        let cost = |t: &Target| {
            let d = ((t.1 - here.0).powi(2) + (t.2 - here.1).powi(2)).sqrt();
            d + LEVEL_COST * t.3 as f64
        };
        // Entity id as the tie-break so two bots reading the same world agree on the
        // ordering and can pick different targets from it.
        let by_threat =
            |a: &Target, b: &Target| cost(a).total_cmp(&cost(b)).then(a.0.cmp(&b.0));
        found.sort_by(&by_threat);
        tough.sort_by(&by_threat);
        self.creatures = found;
        self.tough = tough;
    }

    /// Direction to the `nth` best target (0 = the one to fight first), or east when
    /// none is in sight — east still being the way out of the hub and into the world.
    pub fn heading(&self, nth: usize) -> (f64, f64) {
        let Some((_, tx, ty, _)) = self
            .creatures
            .get(nth)
            .or_else(|| self.creatures.first())
        else {
            return (1.0, 0.0);
        };
        let (dx, dy) = (tx - self.pos.0, ty - self.pos.1);
        let d = (dx * dx + dy * dy).sqrt();
        if d < 1e-6 {
            return (1.0, 0.0);
        }
        (dx / d, dy / d)
    }

    /// Whether any creature is in sight.
    pub fn has_prey(&self) -> bool {
        !self.creatures.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap(me: (f64, f64), mobs: &[(&str, f64, f64)]) -> Value {
        let mut ents = vec![json!({
            "entity_id": "me",
            "avatar_state": "active",
            "position": {"x": me.0, "y": me.1}
        })];
        for (id, x, y) in mobs {
            ents.push(json!({
                "entity_id": id,
                "avatar_state": "mob:thing:beast",
                "encounter_class": "standard",
                "mob_level": 1,
                "position": {"x": x, "y": y}
            }));
        }
        json!({ "entities": ents })
    }

    #[test]
    fn a_bot_steers_at_the_nearest_creature_and_east_when_there_is_none() {
        let mut nav = Nav::default();
        nav.observe(&snap((0.0, 0.0), &[]), "me");
        assert!(!nav.has_prey());
        assert_eq!(nav.heading(0), (1.0, 0.0), "no prey should walk east");

        // Due north, so the heading is a unit vector straight up.
        nav.observe(&snap((0.0, 0.0), &[("a", 0.0, 10.0)]), "me");
        assert!(nav.has_prey());
        let (dx, dy) = nav.heading(0);
        assert!(dx.abs() < 1e-9 && (dy - 1.0).abs() < 1e-9, "{dx},{dy}");
    }

    #[test]
    fn two_bots_reading_one_world_can_pick_different_targets() {
        // The concurrent-battles case: both bots see the same creatures, and if both
        // walk at the nearest they collapse into a single battle. Same ordering on
        // both sides means "you take the nearest, I take the next" needs no chatter.
        let mut nav = Nav::default();
        nav.observe(&snap((0.0, 0.0), &[("far", 0.0, 30.0), ("near", 5.0, 0.0)]), "me");
        assert_eq!(nav.creatures[0].0, "near");
        assert_eq!(nav.creatures[1].0, "far");
        let first = nav.heading(0);
        let second = nav.heading(1);
        assert_ne!(first, second, "the two bots would walk at the same creature");

        // Asking past the end falls back to the nearest rather than standing still.
        assert_eq!(nav.heading(99), first);
    }

    #[test]
    fn a_bot_tracks_its_own_avatar_not_whichever_one_came_last() {
        // Two players in one instance: reading "whichever entity is active" picks up
        // the other bot's position and inverts the steering.
        let mut nav = Nav::default();
        let payload = json!({"entities": [
            {"entity_id": "me", "avatar_state": "active", "position": {"x": 0.0, "y": 0.0}},
            {"entity_id": "them", "avatar_state": "active", "position": {"x": 100.0, "y": 0.0}},
            {"entity_id": "m", "avatar_state": "mob:thing:beast", "position": {"x": 10.0, "y": 0.0}},
        ]});
        nav.observe(&payload, "me");
        assert_eq!(nav.pos, (0.0, 0.0));
        assert_eq!(nav.heading(0), (1.0, 0.0), "should walk east toward the mob at +10");
    }

    #[test]
    fn a_bot_walks_past_an_elite_to_reach_something_it_can_beat() {
        // An elite parked between the hub and the ordinary creatures is the failure
        // this filter exists for: the bot picks the fight it loses, and a test that
        // asserts `victory` fails on where the world put its elites.
        let mut nav = Nav::default();
        nav.observe(
            &json!({"entities": [
                {"entity_id": "me", "avatar_state": "active", "position": {"x": 0.0, "y": 0.0}},
                {"entity_id": "champ", "avatar_state": "mob:wolf:hostile",
                 "encounter_class": "elite", "position": {"x": 5.0, "y": 0.0}},
                {"entity_id": "keeper", "avatar_state": "mob:wolf:hostile",
                 "encounter_class": "gatekeeper", "position": {"x": 8.0, "y": 0.0}},
                {"entity_id": "rat", "avatar_state": "mob:rat:hostile",
                 "encounter_class": "standard", "position": {"x": 0.0, "y": 40.0}},
            ]}),
            "me",
        );
        assert_eq!(nav.creatures.len(), 1, "only the rat is fair game");
        assert_eq!(nav.creatures[0].0, "rat");
        assert_eq!(nav.tough.len(), 2, "the elite and the gatekeeper are still visible");
        let (dx, dy) = nav.heading(0);
        assert!(dy > 0.9, "should head north at the rat, not east at the elite: {dx},{dy}");
    }

    #[test]
    fn a_bot_takes_the_weaker_fight_even_when_it_is_further_away() {
        // Area 0's layout in miniature: the level-2 creature is nearer the hub than the
        // level-1 one the tutorial means you to meet first. Measured over 25 seeds, a
        // solo level-1 party beats the level-1 creature 25 times and the level-2 one
        // 9 times — so proximity alone loses the run about two thirds of the time.
        let mut nav = Nav::default();
        nav.observe(
            &json!({"entities": [
                {"entity_id": "me", "avatar_state": "active", "position": {"x": 0.0, "y": 0.0}},
                {"entity_id": "boar", "avatar_state": "mob:boar:hostile",
                 "encounter_class": "standard", "mob_level": 2, "position": {"x": 2.0, "y": 0.0}},
                {"entity_id": "stalker", "avatar_state": "mob:stalker:hostile",
                 "encounter_class": "standard", "mob_level": 1, "position": {"x": 8.0, "y": 0.0}},
            ]}),
            "me",
        );
        assert_eq!(nav.creatures[0].0, "stalker", "should walk past the level-2 boar");
        assert_eq!(nav.creatures[1].0, "boar");
    }

    #[test]
    fn a_bot_does_not_cross_the_map_for_a_slightly_weaker_creature() {
        // The other half of the trade-off, and the more expensive failure: when level
        // outranks distance outright the bot chases the weakest thing on the map,
        // never arrives, and the run times out instead of merely losing a fight.
        let mut nav = Nav::default();
        nav.observe(
            &json!({"entities": [
                {"entity_id": "me", "avatar_state": "active", "position": {"x": 0.0, "y": 0.0}},
                {"entity_id": "underfoot", "avatar_state": "mob:boar:hostile",
                 "encounter_class": "standard", "mob_level": 2, "position": {"x": 3.0, "y": 0.0}},
                {"entity_id": "miles_away", "avatar_state": "mob:rat:hostile",
                 "encounter_class": "standard", "mob_level": 1, "position": {"x": 900.0, "y": 0.0}},
            ]}),
            "me",
        );
        assert_eq!(
            nav.creatures[0].0, "underfoot",
            "a level-1 creature 900 units away is not worth the walk"
        );
    }

    #[test]
    fn a_creature_with_no_intel_is_still_a_target() {
        // `encounter_class` is perk-gated intel. If a snapshot ever omits it, treating
        // the creature as unbeatable would empty the list and strand the bot.
        let mut nav = Nav::default();
        nav.observe(
            &json!({"entities": [
                {"entity_id": "me", "avatar_state": "active", "position": {"x": 0.0, "y": 0.0}},
                {"entity_id": "m", "avatar_state": "mob:rat:hostile", "position": {"x": 10.0, "y": 0.0}},
            ]}),
            "me",
        );
        assert_eq!(nav.creatures.len(), 1);
        assert!(nav.has_prey());
    }
}

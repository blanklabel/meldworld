# MELDWORLD: Game Design Document

> Canonical source document. Spec files cite sections of this document as `GDD.md §N`.
> Where this document is ambiguous or contains gaps, `CANON.md` resolves them; CANON.md wins on conflict.

## §1. High-Level Concept

Meldworld is an instanced, asynchronous MMO roguelite. Players explore a real-time, procedurally generated, distance-based maze, but combat takes place in isolated, dynamic "Active Time Battle" (ATB) JRPG subscreens. It blends the tension of extraction shooters, the tactical depth of classic 16-bit JRPGs, and a player-driven MMO economy.

Target Tech Stack (rev. 2 — all-in on Bevy):

* Game Engine + UI: Bevy (Handles the 2D top-down overworld, collisions, movement, AND all UI: ATB combat menus, hub screens, player shop UIs).
* Art Direction: indie-style HD-2D (pixel-art sprites and tiles in a 3D-lit scene: depth-of-field, dynamic lighting, particle effects).
* Backend/Networking: Custom Rust Server (Using Naia for networking, Tokio for WebSocket state sync, authoritative combat math, multi-threading spatial queries, and Postgres persistence).
* Auth: username + password (bcrypt-hashed) stored in Postgres.
* Currency: Chits (replaces all "Gold"/"G" references below).

## §2. The Core Gameplay Loop (Extract or Die)

The game is divided into two strict states: Persistent and Ephemeral.

### §2.1 The Hub (Persistent State)

* Safety: No combat. Players interact, trade, and organize.
* The Vault: Where permanent currency (Chits), class unlocks, and extracted crafting materials are stored.
* Blue Chest Gear: Players equip their permanent, insured items. If they die in the Maze, this gear is safely returned to the Hub (though it may lose max durability).

### §2.2 The Maze (Ephemeral State)

* The Reset: Upon entering the Maze, the player's combat "Run Level" resets to a specific Base Level.
* The Backpack: All items found during the run (potions, raw materials, ultra-powerful "Red Chest" gear) go into a temporary inventory.
* The Choice:
  * Extract: Find a portal or use an escape item to bank the Backpack contents into the Vault.
  * Die: All Backpack contents and accumulated combat Run Levels are permanently deleted. Blue Chest gear is returned to the Hub.

## §3. World Generation: The Distance Model

Rather than a traditional descending dungeon, the world is an infinite radial plane expanding outward from the Center Hub.

* Radial Scaling: Difficulty, monster types, and loot rarity scale based on distance from the Center Hub (formula defined in CANON.md §Balance).
* Biomes & Chokepoints: The server dynamically loads chunks based on distance (e.g., Distance 0-100 is Forest; Distance 100-300 is Desert). Procedural generation forces geographic "chokepoints" (like bridges) to naturally push players together.
* Gatekeeper Bosses: At the exact border of a new biome/hub distance (e.g., Distance 499), the procedural generation spawns a massive, unavoidable Boss Arena. These act as ultimate progression blockers and multiplayer rallying points.
* Persistent Milestones: After defeating a Gatekeeper Boss at deep distances (e.g., Distance 500), players can access and rebuild ruined camps to unlock them as Outer Hubs.

## §4. Hubs & Meta-Progression

Unlocking Outer Hubs and progressing within the Hub completely changes the roguelite loop.

* Gatekeeper Drops (Class Unlocks): Gatekeeper Bosses drop unique, persistent account unlocks. For example, defeating the Distance 500 Desert Gatekeeper drops the "Emblem of the Dragoon," permanently unlocking that character class for the player to recruit.
* Base Level Scaling: Starting a run from the Center Hub sets the party's combat level to Run Level 1. Starting from the Distance 500 Hub instantly sets the party to Run Level 40.
* The Training Ground: A UI subscreen in Outer Hubs allows players to instantly allocate massive blocks of combat skill points using custom "Build Templates" before stepping into the maze.
* Resource Stratification: High-tier hubs drop rare materials, but low-tier materials (needed for base crafting components) only spawn near the Center Hub, keeping the entire world economically relevant.

### §4.1 Persistent Non-Combat Skills (The Meld Path)

While combat stats wipe upon entering the Maze, Meld Skills are permanent and level up exclusively in the Hubs or through extraction success.

* Crafting/Forging: Levels up by successfully combining extracted raw materials. High-level Crafters can restore more max durability to Blue Chest gear and craft items with better stat variance.
* Mercantile/Trading: Levels up by successfully completing player contracts or selling items at stalls. Higher levels unlock larger stall sizes, reduced Hub taxes, and the ability to place stalls in deeper Outer Hubs.
* Alchemy/Synthesis: Levels up by extracting rare plants and monster parts. High-level Alchemists can craft permanent "Materia/Gems" that slot into Blue Chest gear.

## §5. Combat & Instances: Instanced ATB Subscreens

* The 4-Player Instance: Players matchmake into the Maze in groups of up to 4. This ensures intimate, cooperative play without overcrowding the world map or making encounters chaotic.
* Active Time Battle (ATB): Combat does not happen on the overworld canvas. Touching an enemy pulls the player into a full-screen battle UI overlay. Timers constantly fill up. If a player walks away, the enemy keeps attacking.
* The Expandable Party (Raid Mechanics): If Player B touches an enemy (or Gatekeeper Boss) that Player A is already fighting, Player B seamlessly joins the fight. The battle UI dynamically expands to accommodate both active parties.
* The Disconnect & Sleep Mechanics: If a player loses cell service or crashes, the Rust server intercepts the connection drop and applies situational rules:
  * Standard Encounters (Auto-Flee): For normal mobs, the server forces the disconnected party to successfully "Flee" the subscreen.
  * Critical Encounters (Progression Saved): Gatekeeper Bosses and elite encounters do not force an auto-flee. Instead, the disconnected party enters an "Auto-Defend" state to prevent wiping out a boss attempt.
  * The "Sleeping" State: Once out of battle, the disconnected avatar is left "Sleeping" on the world map. This is not a safe state. If a roaming monster touches a sleeping avatar, it can attack them, putting the run at risk.
  * Protective Items: Active players can protect sleeping allies by deploying consumable items over them on the map, such as "Warding Tents" or "Sanctuary Campfires," which make the sleeping avatar invisible to monster pathfinding.

## §6. Asynchronous MMO Interaction

Players who are not in combat can still actively shape the world and help (or hinder) those who are.

* Real-Time Influence: Player B (on the world map) can drop a health potion on Player A's active battle sprite. The server intercepts this and instantly heals Player A inside their ATB subscreen.
* Backpack Dropping: Players can freely drop items from their temporary Backpack onto the overworld map for other players to pick up, fostering organic cooperation (or paying bodyguards).

## §7. The Player-Driven Economy

NPCs do not sell the best gear. The community runs the world.

* Player Stalls: In any Hub, a player can deploy a "Stall." Their avatar turns into a shop sprite. Other players tap it to open an in-game shop UI to buy items. The stall remains active even if the owner logs off.
* Bounty Board (Contracts): Casual players can grab gathering contracts (e.g., "Need 50 Iron Ore for 500c") posted by offline crafters, giving direction to short 15-minute play sessions.
* The Durability Sink: Permanent Blue Chest gear degrades max durability upon death in the Maze. High-level Crafters are constantly required to repair max durability, ensuring low-level and high-level economies stay intertwined.

## §8. Endgame & Leaderboards (Infinite Scaling)

Because the world generation formula is theoretically infinite, the game accommodates a permanent, highly competitive endgame.

* The Vanguard Board: A global, real-time leaderboard tracks the highest distance achieved by a 4-player instance.
* Infinite Scaling: Past the final curated Outer Hub (e.g., Distance 5000), the Rust server continues to procedurally generate the world. Monsters gain exponential stat multipliers and drop highly coveted "Prestige" cosmetic aura items that prove how far a player has pushed.
* Seasonal Wipes: To keep the Vanguard Board fresh, the infinite scaling leaderboards operate on a seasonal schedule (e.g., every 3 months). When a season ends, the board is immortalized, and top players receive unique cosmetic titles in the Hub.

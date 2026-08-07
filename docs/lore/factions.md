# The orders — faction canon

> The organisations a hero belongs to. This is the **source of truth for names,
> rank ladders, and what each order is *for*** — CANON (§G) defers to it on faction
> naming, and the ability registry
> ([`meld_proto::skills`](../../shared/meld-proto/src/skills.rs)) is generated against
> the rank ladders below.
>
> Every order runs a **six-rank ladder**, and the later ranks are gated on character
> level at **5 / 9 / 13 / 17**. That is not a coincidence to be smoothed away: it is
> the game's ability ladder. A hero's abilities arrive *as promotions*, so levelling
> reads as standing in an organisation rather than as a bigger number.

| Order | What it does | Class | Ranks 1–6 |
|---|---|---|---|
| **Hunters** | Disposal of dangerous non-civilian creatures | `hunter` | Wisker · Stalker · Shikari · Predator · Master Hunter · Apex |
| **Explorers** | Mapping and reclaiming the unstable world; Anchors | `explorer` | Walker · Traveler · Scout · Pioneer · Discoverer · Globemaster |
| **Phoenix Guard** | Eradicating undead inside the Last City | `phoenix_guard` | Initiate · Purifier · Exemplar · Luminary · Redeemer · Apotheosis |
| **Shifters / Runners** | Salvaging The Lost from the Shifting Lands | `shifter` | Flicker Foot · Shift Rat · Runner · Shifter · Void-Dancer · The Named |
| **The Trace** | Tracking and containing anomalies — including Psykers | `psyker` | Initiate · Tracer · Field Marshal · Lead Investigator · Bureau Chief · Director |
| **Order of the Iron Hull** | Blunt-force deterrent against leviathans | *future* | Bilge-Scraper · Wake-Striker · Anchor-Priest · Iron-Bound · Deck-Master · Grandmaster |
| **Leviathan's Slumber** | Druids of the coast and the colossal deep | *future* | Tidewatcher · Wave Weaver · Stormspeaker · Deepwalker · Seafather/Seamother · Leviathan's Voice |
| **The Open Flower** | Druids of agriculture and balance | *future* (`keeper`) | Sprout · Seedling · Budling · Flowerling · Cultivator · Terra |
| **The Order** | Secret anti-corruption coteries | *future, hidden* | Pacifier · Placater · Peacemaker · Conciliator · Arbiter · Judge |
| **The Foundry** | Structural iron and magitech metal for the city's infrastructure | *future* (`smithwright`) | Indentured Extractor · Smelter Apprentice · Journeyman Smithwright / Extractor Foreman · Smithwright · Master Smithwright · Master of the Foundry |

The **Resonant** is the one class with no order yet — a healer who pays in its own
blood fits none of the above cleanly, and inventing one to fill the table would be
worse than leaving the gap named.

The city's **non-class institutions** — government, the Sentinels, the Archivists,
Artificing, the Messengers, the Wall Defense Force, the Healery, and the criminal
syndicates — live in [`city-institutions.md`](city-institutions.md).

---

## Hunters

**Vision** All known non-civilian threats in known civilization killed or captured.
**Mission** Capture, kill, or otherwise remove dangerous non-civilian creatures from
the Last City and the surrounding Stable Lands.

Varied backgrounds — motley crews and adrenaline junkies. Guildmaster reports into
legal democracy; hall leaders report to the Guildmaster; hunters pull work from
halls. **"It's a victory when the hunt is over."**

Halls: **Den** (Commons — a 60ft posting hallway into a ring of five fighting pits),
**Civs End** (Dreary Draught), **Spiderducts**. Notable: **Veth** (runs The Hall),
**Vessa** (reward distributor), **Usk** (runs Civs End), **Serissa** (Spiderducts),
**Slyvia "The Great Destroyer"** (most kills in the city).

**Hunt etiquette** is a whole mechanic waiting to be built: postings colour-coded by
armband rank and tagged by day marker (dawn → sunrise → noon → dusk → midnight →
solstice); hunters hang a **callsign** on a poster to claim a slot, with a rung
limiting how many may join; withdrawing is shameless but habitual withdrawal earns
mockery ("praying to hunt instead of hunting prey"); a callsign hung *over* another's
is a **duel** challenge, and the bands rattle as the two draw closer; hunts pay only
on evidence of the kill; a dead hunter's callsign may be kept, marked with a
horizontal line; wearing a *living* hunter's callsign is a debt.

**Why it is the martial class.** The Hunter's mission is the game's core loop, so the
Hunter carries the martial baseline: basic attacks bank **Adrenaline** and every
skill spends it. Adrenaline junkies, in the lore's own words.

Rank levels: Wisker 1 · Stalker 2 · Shikari **5** · Predator **9** · Master Hunter
**13** · Apex **17**.

## Explorers

**Vision** A world known. **Mission** See and map the world for the good of The Last
City.

Government-funded, reporting to the council. Members follow Serin, Mestiva, or
nature; **only those of Serin may set Anchors**. Led by **Medford** (half-elf), with
**Narn** (Tabaxi) as second, **Heartwood** (treant) leading **the Guides**, and
**Barmala** the beast master.

**Anchors** are the world's load-bearing idea: the Serin power to fix stable land out
of the chaos, and the foundation civilisation is built on. **The Guides** move people
safely between stable lands — navigation, diplomacy, conflict resolution.

Beliefs: exploration leads to understanding; nobody accomplishes it alone; the
natural world must be protected.

**Why its kit is tempo and stability, not damage.** An order defined by *safe
passage* and *anchoring* fights by keeping its party moving and standing, which is
mechanically distinct from the Hunter's burst and the Resonant's healing.

Ranks: Recruit (0) · Walker 1 · Traveler 2 · Scout **5** · Pioneer **9** ·
Discoverer **13** · Globemaster **17**.

## Phoenix Guard

**Vision** All dead remain resting. **Mission** Purge the Last City of undead and
prevent their resurgence.

Formed **539 AM**, after strange happenings left the city overrun each night. A newly
formed, highly elite section of the Last City's government. Hand-picked for combat
prowess, resilience and commitment; the majority are **constructs or Earth Genasi**,
which appear resistant to being turned. Trained in anti-undead technique and in
necromantic lore and identification.

Notable: **Alloy**, a veteran divine golem and one of the oldest creatures in the
Last City, leads the Guard; **Lapis** (Earth Genasi) is training officer and field
commander; **Silent Sister Lilith the Haunted Healer** (green dragonborn, no tongue)
investigates the fungal uprising and keeps the organics organic.

Beliefs: undeath gained to harm the living is an abomination; eradication is a sacred
duty; **mercy is a luxury that cannot be afforded**; knowledge of necromancy is
necessary but corrupting. Etiquette: *no one gets turned*; every strike is completed
to the point of eradication; nobody is left behind; weaknesses are shared freely;
there are no secrets.

**Why its kit is anti-undead.** Silvered and holy tools, rites that keep the line
intact, and light that burns a whole pack — with a standing bonus against undead. It
is unlocked by surviving an undead rite, so the class arrives already pointed at what
it is for.

Ranks: Initiate 1 · Purifier 2 · Exemplar **5** · Luminary **9** · Redeemer **13** ·
Apotheosis **17**.

## Shifters / Runners

*Also:* Scrapers; **Shift-rats** (derogatory). An informal collective — no vision, no
mission, no leadership. **Reputation is the only currency.** Hundreds of them, in
independent crews of 2–5.

Where the Explorers map and stabilise, Shifters enter the *most* unstable regions to
retrieve **The Lost**: artifacts and scrap that hold value. They are defined by being
able to **read the reality-glitches** of the Shifting Lands and survive where logic
fails. Notable: **Marrow** (calm during a Stutter), **Skuff** (badgerfolk, finds
Heavies where others won't go).

Beliefs: the world is a gift, not a given — anything not bagged is claimed by the
Shift; **finders, keepers, sellers**; every artifact is paid for in years or nerves.

Etiquette: **The First Sip** goes to whoever spotted the Lost. **The Bag Rule** — if a
teammate is Going Void you grab their bag; you don't leave The Lost behind, even if
you have to leave the person. **The Heavy Protocol** — never pull a Heavy until the
crew is ready to sprint, because lifting it starts the Flicker.

**Cant.** *The Lost / Scrap* — what you came for. *Flicker / Stutter* — the land going
unstable, usually because a Heavy just left it. *It has a Heavy in it* — a zone
suspiciously stable, so something powerful is anchoring it. *That's Heavy* — slang for
impressive. *Shift Sick* — the trauma of nearly Going Void; the nerve doesn't come
back. *Going Void* — erased from reality. *Check the Weight* — is it stabilising, or
just loot?

**Relationships.** The Explorers: borderline hostile in both directions — bureaucratic
cowards vs. rats who destabilise the world for profit. The Collectors: anonymous
buyers who don't ask. The public: tragic gamblers, respected and feared.

**Why it senses items and dungeons.** Shift-sense is the class fantasy, so the Shifter
is the one who knows where the doors are and which loot is permanent — see
[`progression-and-unlocks.md`](../proposals/progression-and-unlocks.md). Live as of
`CL-2`: `shifter_dungeon_radius` reveals entrances, `shifter_item_sense` reads
permanence. The *map* is the Explorers' and the *prey-sense* is the Hunters'; a Runner
contributes the doors.

Ranks: Flicker Foot 1 · Shift Rat 2 · Runner **5** · Shifter **9** · Void-Dancer
**13** · The Named **17**.

## Order of the Iron Hull — *a future class*

A heavily regimented ascetic monastic order confined to a single massive rusting
vessel patrolling near the Glass Desert and the Last City. A brutal, pragmatic
counter-balance to the druids of the Leviathan's Slumber, and a physical deterrent
against the colossal marine predators of the deep. Their martial art is built on
kinetic momentum, isometric strength and the raw physics of the ocean's swell.

**Vision** To be the unyielding iron bulwark against the deep, holding equilibrium
through density, discipline and blunt-force trauma. **Mission** Protect the coast
through acoustic disruption and brute force — and keep the Leviathan's Slumber druids
from letting the ocean overgrow its bounds. *They do not revere nature; they survive
it.*

Humans, Goliaths, half-orcs and amphibious Kuo-toa: gaunt, dense with isometric
muscle, in ashen sailcloth robes pitch-stained at the hem, hands and shins bound in
canvas dyed with rust scraped off the ship's rivets. Weapons are heavy ship's oars
banded in copper gone verdigris. **Grandmaster Kaelen "Iron-Spine"** has hull rivets
pierced through his spine, shoulders and knuckles and fights with a ship's bell on a
chain; **Hull-Listener Gloop** (Kuo-toa) hangs over the side pressing his face to the
wood to feel what is swimming beneath.

Beliefs: *the vessel is a tool*; the **Doctrine of Equilibrium** (every action demands
a counter-balance); **root over reach** — power comes from rooting to the deck and
transferring the momentum of the world through your bones. Etiquette: perfect spacing
across the joists to hold buoyancy; **hull resonance** instead of shouting (encoded
vibration through the ship's skeleton); the **Resonant Wake**, a synchronised hum and
oar-slam that deafens and deters sea beasts; vertical hammocks lashed to the masts.

**Its kit is already authored** — the rank perks are the ability ladder, and they are
reserved for it: **Swell-Step** (Anchor-Priest, L5), **Structural Rooting** (Iron-Bound,
L9), **Kinetic Shock** (Deck-Master, L13), **Toll of the Deep** (Grandmaster, L17),
with Sea-Legs and Oar-Fighter at the lower ranks. The `iron_hull` class key is
therefore **reserved, not recycled** — nothing else may claim it.

Ranks: Bilge-Scraper 1 · Wake-Striker 2 · Anchor-Priest **5** · Iron-Bound **9** ·
Deck-Master **13** · Grandmaster of the Hull **17**.

## The Trace — *Target Recon, Abnormalities Containment and Eradication*

A newly formalised official agency: the Republic's logistical marshals and anomaly
hunters. As the city pushes its boundaries and new powers emerge in the populace, The
Trace tracks, investigates and stabilises what nobody understands yet — securing
infrastructure beyond the walls, documenting unregistered powers, and making sure the
city survives its own expansion.

**Vision** All unknowns, known and risk assessed. **Mission** Investigate anomalies
and register risks, so the city can integrate new phenomena without a catastrophic
shift.

Field teams pair a **Tracer** (an investigator) with a **Contingency** (a heavily
grounded physical operative who provides protection). Notable: **Rayne**, a junior
operative and **Psyker**, currently assigned to investigate others like himself;
**Draught**, his large mute brother and Contingency, who has been beyond the wall
setting a tether and has fought alongside the Phoenix Guard; **Director Silas Vane**,
a veteran marshal who decides by risk-assessment algorithm which anomalies are studied
and which are purged.

Beliefs: **unregistered power is a threat to the city's infrastructure until it is
quantified, stabilized, or purged**; *hope is hard work* — documentation and
stabilisation are the shields. Etiquette: city-sanctioned deployment orders, strict
logging, and partners watching each other's physical *and mental* state, ready to
apply manual suppression if a Tracer starts losing their grip. It operates out of the
**Stabilization Bureau**, which also contains the Explorers, and coordinates with the
Wall Defense Force and the Phoenix Guard.

**Why the Psyker belongs to it.** Psykers are the anomaly the city is currently
registering, and the ones it sends to do the registering. The class's Foci — held,
maintained, revoked, one slot at a time — read as containment rather than sorcery,
which is exactly what The Trace does to everything else.

**Its manifestations are the class doc's, scaled.** The canonical Psyker gates
Manifestation tiers at D&D levels 1 / 5 / 9 / 13 / 17; on our ladder those become
Gravity Well and Kinetic Aegis (1), Mind Spike (9), Temporal Anchor (16), **Kinetic
Wave** (25), **Thermal Flux** (36), **Matter Dissolution** (49), **Phase Shift** (64),
**Dominate Mind** (81) and **Reality Collapse** (100). Focus slots grow 2 → 5 across
the same span.

Still to bring across from the doc: **Psi Points** as a real cost (Foci are currently
limited only by slots), the **Psychic Strain** save that threatens a Focus when the
Psyker takes damage, and the per-Manifestation *aspects* with their prerequisites
(Pressure → Gravity → Anchor).

Ranks: Initiate 1 · Tracer 2 · Field Marshal **5** · Lead Investigator **9** ·
Bureau Chief **13** · Director **17**.

## Leviathan's Slumber — *a future class*

A circle of druids on floating islands of interwoven trees and earth, drifting the
ocean off the Last City. They keep the marine ecosystem healthy, guide fish toward the
city's fleets, and divert or calm the colossal creatures that threaten its shores —
the counterpart the Iron Hull exists to counterbalance.

**Vision** Harmony between the city and the ocean through balanced coexistence.
**Mission** Stand between land and sea: protect against the deep, and keep the
relationship with the ocean's resources sustainable.

Notable: **Coralia Tidebinder**, elder and spiritual leader, who speaks with whales;
**Aegir Stormcaller**, who raises protective storms around the islands; **Nereid
Deepwalker**, a shapechanger who scouts the depths. Beliefs: the ocean is a living
entity owed reverence; balance between city and ecosystem; cooperation with marine
life is mutual survival; the power of nature is immense and wants caution. Decisions
are consensus, with Coralia holding the final say; the floating islands are sacred and
maintained by everyone.

Ranks: Tidewatcher 1 · Wave Weaver 2 · Stormspeaker **5** · Deepwalker **9** ·
Seafather/Seamother **13** · Leviathan's Voice **17**.

## The Open Flower — *a future class*

The government-sanctioned order responsible for agriculture in the Last City and the
stable lands. Mostly lawful-neutral druids and rangers. **Vision** A society in
harmony with Gaia. **Mission** Advance civilisation without disrupting nature's
balance.

A **holacracy** — no reporting structure, no titles, and famously no front desk or
formal entrance to their business. Notable: **Elder Elara Meadowlight**, the
"non-leader" whose botany and connection to the land make her wisdom sought by all;
**Ranger Bryn Stonehand**, a perfectly black Harengon tracker and animal handler who
manages where wildlife meets cultivated land; the potion makers **Tri-Keen Drik** (a
purple four-armed mantis) and **Blue Bill Dances** (a Kenku in a green leather apron
and thick glass goggles); and **Twilight Rose**, grimy and enormous on a straining
mushroom-laden chair, who cannot leave her little babies but will tell you what she
doesn't know.

Beliefs: nature must be preserved; civilisation must balance with the land to sustain
it; sustainable practice is the long game; the wisdom of nature guides action.

**Already touching the game:** the potion makers are the alchemy ladder, and the
Apothecary's shelf is the city-facing end of it.

**The class is the `keeper`** — an Open Flower hero in the field. Keepers are
**Alchemy's** gathering hands: reagents, farms, groves, planting. Stacking them in one
party buys **tempo** on every one of those timed actions rather than more material — four
Keepers harvest, plant and tend markedly faster, which is what makes a mono-Keeper party
a fast, fragile gathering raid instead of a wasted roster
([`proposals/crafting-and-professions.md`](../proposals/crafting-and-professions.md)
§2.3a).

Ranks: Sprout 1 · Seedling 2 · Budling **5** · Flowerling **9** · Cultivator **13** ·
Terra **17**.

## The Foundry — *a future class*

Not a guild — an **industry**. A heavily subsidised, strictly audited branch of the city
government, and the backbone the rest of the Last City is bolted to. **Vision** The
absolute survival of the Last City through relentless efficiency, industrial production
and the strict fulfilment of city-mandated quotas. **Mission** Supply the structural iron
and magitech metals that keep the mechanical **Stabilizing Towers**, the **Great Ivory
Wall** and the **Power Grid** standing.

Three castes, and they are a production line rather than a hierarchy of prestige. The
**Extractors** rip resources out of the Shifting Lands and the Slag-Fields; the city
fills those ranks with citizens in debt and minor criminals, keeping labour cheap and
expendable, and the mortality rate is what you would expect. The **Smelters** boil the
corruption and magical volatility out of raw ore to stabilise it — the bridge between
raw material and anything usable. The **Smithwrights** build: complex magitech
components, heavy riveted plating for the Wall, structural armor.

Run as a bureaucracy under the **First Bloc**, out of the **Crystal Tower**, which hands
down daily quotas to the Smithwrights and Smelters, who in turn spend the Extractors.
Transactions are strictly bureaucratic — the Foundry makes gear for the military and the
state, so an outsider cannot simply walk in and buy, only navigate red tape or pay a
bribe. Its internal hierarchy is modelled deliberately on the automatons that run the
First Tower.

Notable: **Thora Iron-Bind** (dwarf, Master Smithwright), a zealot of industrial
progress who takes the Forge-God cult literally, reads engineering blueprints as
infallible holy text, has voluntarily replaced her left arm and jaw with magitech and
riveted plate to better resemble the First Tower's automatons, and refuses anyone who
wastes her time with inefficient emotions; **Silas "Scrap" Copper-Cough** (gnome,
Requisition Officer), who controls military-gear distribution from a smog-filled office
in the lower Crystal Tower, filters off-the-books material to the highest bidder, and
coughs black metallic phlegm; **Brannek Deep-Shift** (dwarf, Extractor Foreman), who uses
old Dwarven Ethereal Way technique to feel a resource vein about to dimensionally
collapse and is clashing with Crystal Tower officials over a quota he knows will kill his
crew; **Fidget Wrench-Warp** (gnome, Smelter Apprentice), who finds stable material
boring and is hoarding the most corrupted slag in the lower furnaces to build an illegal
golem powered by raw concentrated magic; **Jaxen "Lucky" Vane** (human, Indentured
Extractor), drafted to pay off a gambling debt, fourteen months alive in the Slag-Fields
against all odds, scarred by magical volatility and convinced the shifting earth is
personally hunting him.

Beliefs: industrial efficiency is a literal form of worship; blueprints are holy texts;
the quota is the measure of a life's worth. The order keeps a state-mandated
pseudo-religion dedicated to **"One", the Forge God** — which is the Foundry's **heresy
of Terim**, the god of crafting and building: a cult that industrialised a god of
*making* into a god of *throughput*. Terim's own followers, who maintain the stabilisers
and staff the Healery, do not accept the renaming.

**Already touching the game:** the Foundry is **Forging's home order**, and its three
castes are the Forging pipeline end to end — Extractors are ore/wood harvest, Smelters
are the raw→refined step the game does not have yet, Smithwrights are the Forge itself.
See [`proposals/crafting-and-professions.md`](../proposals/crafting-and-professions.md)
§2.4.

**The class is the `smithwright`**, named for the caste that builds rather than the order
that employs them — as the Psyker is named for what it is, not for The Trace. A
Smithwright is **Forging's** hands: extraction, smelting, forging, repair, and
(eventually) raising walls and structures. Stacking them buys **tempo** on all of it —
four Smithwrights build, repair, smelt and forge markedly faster, never producing *more*
material than the vein held (§2.3a). Their eventual apex is co-authoring an **Anchor**:
the Foundry forges the body, Artificing fits it out, and only an Explorer of Serin can
set it — the load-bearing artifact of the setting is a thing no single order can make.

Ranks: Indentured Extractor 1 · Smelter Apprentice 2 · Journeyman Smithwright /
Extractor Foreman **5** · Smithwright **9** · Master Smithwright **13** · Master of the
Foundry **17**.

> **Open naming questions**, carried from the design notes rather than silently
> resolved: **Thora Iron-Bind** sits close to the Iron Hull's rank-4 title
> **Iron-Bound**; and the **First Bloc** may or may not be the same body as the
> **council** the Explorers report to. *(The Foundry's Extractor Foreman was renamed
> from Kaelen to **Brannek** Deep-Shift to clear the collision with the Iron Hull's
> Grandmaster Kaelen "Iron-Spine".)*

## The Order — *a future class, and a hidden one*

Highly secretive, female-only, operating in **coteries of five**. Their mission is to
keep Last City society free of corruption, for as long as it exists. Members tend to be
rogues and sages who watch the city from where information collects — brothel and bar
owners, and any other profession with an ear to the ground.

**Vision** A city in balance. **Mission** Keep the Last City free of corruption
throughout its existence.

Beliefs: **the outcome is more important than the people**; the Order is more important
than hired help; **secrecy is sacred**; act by proxy and anonymously.

The structure is unknown *even to its members*: each coterie is isolated from every
other, deliberately, so that no centre of power can form. Coteries can cluster for a
large operation and still stay anonymous to one another. When a member dies the coterie
finds a replacement, drawn first from the family or friends of active members; those
who fail training are disposed of, or their memory is wiped.

**A design note, not a caveat:** a class whose whole premise is that nobody knows who
is in it wants a different unlock shape than the others — earned quietly, and probably
never announced with the banner every other unlock uses.

Ranks: Pacifier 1 · Placater 2 · Peacemaker **5** · Conciliator **9** · Arbiter **13** ·
Judge **17**.

# Factorio block generator — v2 plan

## Goal

Generate **placeable production blocks**: a rectangle with machines, belts, pipes, inserters and poles inside,
and a published interface on its edges (which item or fluid enters or leaves where, at what rate, on which
lane). Routing between blocks stays with the player. Output: an importable 2.0 blueprint plus a port manifest.

The v1 engine (free placement, simulated annealing, PathFinder) is the base. It already produces tight valid
layouts for blocks of about ten machines in seconds; v2 makes it complete (more machine types, pipes,
by-products, several belts per item, splitters).

**Correctness over speed.** A layout is only reported if the validator proves it works; run time is spent
freely on more seeds, longer annealing and longer negotiation. Block size is limited by what routes, not by
how long it takes.

## Lessons from v1 that shape this plan

1. **The six-science failure was space, not belt throughput.** v1 already gave heavy items several belts
   (copper cable at 115/s got 5) and split busy nets, 72 chains for 29 items, and no chain ran over capacity.
   What failed was room: at 118 × 118 the belts' total length grew from 4,100 to 8,700 tiles during
   negotiation, and at 160 × 160 it stalled with 60–90 belts crossing the same tiles, mostly in the gaps beside
   machine inserter slots. Each chain had to snake past every consumer of its item across the whole factory.
   More belts per item fix throughput but add tiles; they do not create room.
2. **Chains should be trees, and that is what should make big builds possible.** A tree with splitters
   reaches scattered consumers with far less belt than one chain visiting them in turn, and lets each branch
   stay local. Together with a placer that groups each item's producers and consumers, this attacks the real
   cause above. Consumers along a belt just pick from it, so branches are needed only where consumers lie in
   different directions. Whether it is enough for a 161-machine single block is an experiment (milestone D6);
   splitting into blocks remains the fallback.
3. **Direct insertion matters more than anything else** for inserter-heavy recipes. Keep v1's placer bonus,
   flow settling and validator model.
4. **Flow is a max-flow problem.** The validator's max-flow split of shared producers generalises to belt
   trees with splitters, merges and lanes: a layout is valid when a feasible flow meets every demand.
5. **Inserter research changes everything.** Without hand-size bonus a circuit machine needs 5 cable
   inserters from a belt. Research level must be an input.
6. **Facts first.** Lane rules, splitter lanes and fluid connection points must be verified before code relies
   on them.

## Scope

In:
- Base-game machines: assembling machines 1–3, stone, steel and electric furnaces, chemical plants, oil
  refineries, centrifuges. Footprints from 2×2 to 5×5, rotation and mirroring.
- Belts: several belts per item, splitters for branching and merging, sideload merges, undergrounds, two
  belt tiers for weaving, lane sharing for low-rate items.
- Fluids: base-game pipes and pipe-to-ground (pumps where needed), fluid ports on the block edge, no mixing.
- Recipes with several products (oil processing, cracking, Kovarex, uranium processing). Every product gets a
  destination.
- Burner machines: fuel as an extra input item.
- Power by poles; test harness with infinity chests and pipes.

Out (v3, with hooks now): Space Age machines and their fluid connections, beacons and modules, quality,
spoilage, heat pipes, belt stacking, surface conditions, space platforms, bots, circuits.

## Data: base-game fluid connections

`recipes.json` has machine sizes, speeds and categories but no fluid box positions. For the base game they are
few and well known, so v2 starts from a hand-written table in `GameConstants.kt` (machine facing north,
offsets from the machine centre, pipe tile just outside the machine), each line checked in game:

| Machine | Size | Inputs | Outputs | Notes |
|---|---|---|---|---|
| Assembling machine 2/3 | 3×3 | middle of north side | middle of south side | only active for fluid recipes |
| Chemical plant | 3×3 | north side, left and right of centre | south side, left and right of centre | always active |
| Oil refinery | 5×5 | south side, 1 left and 1 right of centre | north side, 2 left, centre, 2 right | which output carries which fluid comes from the recipe's `fluidbox_index` |
| Furnaces, centrifuge | 2×2 / 3×3 | — | — | no fluids |

Also to verify: pipe-to-ground span (10 tiles in 1.1), mirroring of the refinery and chemical plant, and
which refinery connections stay connectable when a recipe leaves them unused. A data-export script for every
prototype (needed for Space Age and mods) moves to v3.

## Pipeline

| Stage | What | From v1 |
|---|---|---|
| 1 Plan | Recipe rates by linear program, machine counts, item and fluid nets, ports | rewritten |
| 2 Split | If the job exceeds the block cap, propose blocks along the recipe graph | new |
| 3 Place | Simulated annealing on mixed footprints with rotation and mirroring | extended |
| 4 Route | Belt trees and pipe networks, negotiated congestion | extended |
| 5 Power, harness | Poles; infinity chests and pipes on ports in test mode | kept |
| 6 Validate | Max-flow over belts with lanes and splitters; fluid networks; power | extended |
| 7 Search | Box size, seeds, anytime outputs, calibration | kept |
| 8 Emit | Blueprint 2.0 and port manifest | extended |

### Stage 1: planning with by-products

Replace the recursive expansion with a linear program (OR-Tools GLOP is already a dependency):

- Variables: crafts per second per recipe (allowing several recipes per product, e.g. basic vs advanced oil
  processing, cracking).
- Constraints: for every item and fluid, production − consumption = target output (final products), ≥ 0 or
  = 0 for intermediates, and = −input for items supplied by ports. Raw inputs are items with no allowed recipe
  or the ones the user lists as inputs.
- Objective: minimise machines (or a user weight per input), so cracking and recipe choice come out right.
- **By-products**: any surplus becomes an output port of the block (or, if the user allows, a void/recycler
  sink in test mode). A machine whose by-product has nowhere to go stops in game, so the validator treats an
  unrouted product as an error.
- Machine choice per recipe category from config (e.g. `crafting → assembling-machine-3`,
  `smelting → electric-furnace`, `oil-processing → oil-refinery`), crafting speed from data, research block for
  inserter hand sizes.

Output: machine instances (with footprint, fluid boxes, energy source), item nets, fluid nets, and the
block's ports with rates.

### Stage 2: block split

First try the whole job as one block. Only if no seed validates at any size within the budget, partition along
the recipe graph so cut items are few and high-rate (they become clean port belts): min-cut style
partitioning weighted by the number of belts a cut item needs. The user can also name the blocks directly
("circuits", "red+green", …). Each block is then generated on its own.

### Stage 3: placement

v1's annealer, generalised:

- Any rectangle footprint; rotation (90°) for all machines, mirroring where the data allows; fluid box tiles
  move with rotation and mirroring.
- Pin access per side from the real footprint; fluid connection tiles count as reserved pins.
- Direct-insertion bonus for every producer–consumer pair whose geometry fits (gap 1, or 3 with long-handed).
- New cost term for fluids: HPWL of each fluid net, plus a penalty when connection tiles of different fluids
  are adjacent (they would merge).
- Ports: their sides are fixed by config or chosen by the placer (then reported in the manifest).
- Congestion: RUDY failed for nets spanning the whole factory, because their demand is spread flat. With trees,
  estimate each item's demand as a rectilinear Steiner tree (or MST) over its terminals and spread each edge's
  demand along its own bounding box, so moving a consumer next to its producers visibly lowers congestion.
- Locality: a term that pulls each consumer towards its nearest producer of each ingredient (the MST edges
  above), so items form local clusters instead of factory-wide spreads.

### Stage 4: routing belts as trees

Each item has one or more **belt networks** (several belts when flow exceeds 0.9 of a belt, or when splitting
makes routing easier). A network is a directed tree-or-DAG from its sources (producer inserters, input ports)
to its sinks (consumer inserters, output ports).

- Growth, VPR style: start from a source; repeatedly A* from **every tile of the existing network** to the
  nearest unserved terminal. Joining mid-belt requires a splitter (branch) on the existing path; joining onto
  a belt from the side is a sideload merge (one lane); two belts into one is a splitter merge.
- Splitters are 2-wide entities placed across the belt; the router adds them as moves when a branch leaves
  the tree, with their own tile resources.
- Lanes: track per-lane items and flow. Lane sharing (two items, one per lane) is a network-pairing option
  using the verified drop-lane rule; off by default until verified.
- Flow check during routing: after each net, a max-flow from sources to sinks over the network (belt and lane
  capacities) must meet every sink's demand; if not, add a branch from another source or a second belt.
- Keep from v1: negotiated congestion with history, underground channels per axis and tier, lazy path repair,
  leg backtracking, dead-end rules, inserter options I1–I3, direct insertion.

### Stage 4b: routing pipes

- Fluid networks are routed with the same negotiated-congestion core: pipes, and pipe-to-ground pairs that
  connect only along their axis and pass under anything, with a channel resource per axis as for undergrounds.
- **No mixing**: every pipe joins every adjacent pipe and every adjacent machine connection point, so a pipe
  of fluid A may not sit next to a pipe or connection point of fluid B. Modelled as a hard adjacency resource;
  pipe-to-ground ends are exempt on their sides.
- A pipe touching a machine anywhere except a connection point does not connect, so pipes may run along
  machine sides; but a pipe on an unused connection point of another machine does connect (a validator check).
- Each fluid network is a tree from its sources (refinery outputs, input ports) to its consumers; branching is
  free (pipes have no direction).
- Throughput: base-game blocks are short enough that pipe flow is not a limit, except refineries and cracking
  feeding many plants; the validator flags networks longer than the verified segment limit, and a pump splits
  them.

### Stage 5–8

Power: v1 set cover and MST; burner machines get fuel through an inserter like any ingredient. Test harness:
infinity chests on item ports, infinity pipes on fluid ports, a sink chest or void on output ports.

Validator: v1 checks plus splitters and lanes (max-flow per lane), fluid networks (one fluid per connected pipe
component, every fluid box connected to the right one, no foreign adjacency), every product routed.

Manifest (`ports.json`): for every port, side, tile, item or fluid, rate, belt tier, lanes used. This is what
you route against between blocks.

## Extension points for Space Age (v3)

Designed in now, implemented later:
- **Surface conditions** on recipes and machines from the data export; a job names its surface and the planner
  filters recipes.
- **Spoilage**: item spoil time in data; a per-item freshness budget turns into a maximum belt length between
  producer and consumer (a routing constraint).
- **Energy sources**: electric, burner, heat (Aquilo heat pipes are another network type, like pipes).
- **Belt stacking** and stack inserters: capacity per lane times stack size; inserter rates from research.
- **Modules, beacons, quality**: machine speed and productivity multipliers in Stage 1; beacons as placed
  entities with an effect radius term in the placer.

## Milestones

1. **D0 Facts.** Base-game fluid connection table; verify kit run in game (inserter direction, drop lanes,
   splitter lanes, sideloads, underground and pipe-to-ground pairing and spans, fluid connections per machine and
   rotation, refinery output order). *Test:* checklist complete; the table matches the game.
2. **D1 Planner.** LP with recipe choice and by-products; machine map per category; research block.
   *Tests:* advanced oil processing plus cracking to petroleum balances; Kovarex keeps its U-235 loop;
   T1–T4 counts unchanged from v1.
3. **D2 Mixed footprints.** Geometry, rotation and mirroring for any machine; placer and validator on 2×2 to
   5×5. *Test:* a hand placement of a refinery, chemical plants and furnaces validates its footprints and fluid
   tiles.
4. **D3 Pipes.** Fluid routing and fluid validator. *Tests:* plastic block (refinery → chemical plants) routes
   with no mixing; a fixture with two fluids side by side fails validation.
5. **D4 Belt trees.** Splitters, merges, several belts per item, max-flow validator with lanes. *Tests:* a
   fixture needing a branch routes and validates; an item at 1.5 belts uses two belts; 200 random blocks never
   throw and every success validates.
6. **D5 Ports and manifest.** Item and fluid ports, output ports for products and by-products, test harness.
   *Test:* every product of every machine reaches a consumer or an output port.
7. **D6 Big blocks and split.** Steiner-based congestion and locality in the placer; six science at 1/s as one
   block with a long budget (an hour is fine). If it validates, record size and time; if not, record what
   blocks it (conflict map) and fall back to the split. *Test:* six science produces valid blocks either way.
8. **D7 Benchmark.** Red/green, oil-to-plastic, processing units, a smelting column; in game in test mode, each
   reaches 90% of target rate in 10 minutes.

## Decisions for you

- Split into blocks only when one block fails, or always split at item boundaries you choose?
- Port sides: fixed by you per item, or chosen by the generator and reported?
- Default machine per category (e.g. electric vs stone furnaces) and default research level.
- What surplus by-products should do: always an output port, or allowed to be voided in test mode?

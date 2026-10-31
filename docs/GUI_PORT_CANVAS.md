# Selecting input and output cells on a canvas

Status: **open**. Can the GUI let the user pick, on a grid of cells, where inputs enter and outputs leave, and have the generator calculate the factory from that?

## Answer

Yes. A port is already either a fixed tile or a stretch of an edge, so a cell canvas is a way to fill `sources` and `outputs`. It does not need a new planner. It needs a grid that writes those fields, and a locked box so a clicked cell stays the same tile while the search runs.

The canvas edits the block's interface. Targets and source names still drive the rate solver: which recipes run, how many machines, and the items per second on each port. A painted cell says where that flow may cross the box. The search still places the machines and the belts. The result picture stays the preview that already exists; the picker is a separate empty grid.

`docs/GUI_PLAN.md` leaves `sources`, `outputs`, `reserved` and `forbidden_tiles` in the JSON tab until someone asks for widgets. This is that request. A grid is the widget that matches these fields.

## What a click writes

A source or an output is a `ConfigSource` (`src/config/run_configuration.rs`): an item name, plus either an edge or one tile.

| Form | Fields | How the router reads it |
|------|--------|-------------------------|
| Edge | `edge` in `north` / `east` / `south` / `west` / `any`, optional `from`..`to` | `port_candidates` (`src/place/placement.rs`) lays the segment on that side of whatever box the search is trying |
| Tile | `x`, `y`, and `dir` (`N` / `E` / `S` / `W`) when the belt heading is fixed | the same function returns that one tile. `x` and `y` may sit one tile outside the box |

Several rows with the same name are one port with several candidate cells. `port_tiles_of` (`src/route/chains.rs`) keeps a candidate that lies in the box or on the one-tile ring. A fixed tile inside the box is also a static obstacle (`static_obstacles` in `placement.rs`), so the placer leaves it empty.

An item with no row is already allowed anywhere on the perimeter. `ports` (`src/plan/plan.rs`) fills a missing source or output with `edge: "any"` and warns when a named source is not an input of the job.

## What the canvas does not decide

The rate solver (`src/plan/lp.rs`) reads targets and the names on `sources`. Naming `iron-plate` as a source is what tells it not to craft plates inside the block. A cell has no item of its own. After the targets are set, the plan already knows the lists to offer: supplied items in, and products plus surplus by-products out. Fluids always leave through a port. Solid products leave through a port when `sinks.mode` is `port`.

## The box has to stay still for exact cells

Tile coordinates are absolute inside the box. `Search::box_size` (`src/search/search.rs`) then tries many sizes. `fixed_width` alone still lets the height grow with the size parameter, and `fixed_height` alone lets the width grow. When both are set, `box_size` returns on the width and ignores the height. A click on the south edge of a 20×16 grid is a different tile once the box becomes 20×24.

Painting exact cells means both sides are fixed and the size search is skipped for that job. That is a small change in `box_size` and `Search::run`: when `fixed_width` and `fixed_height` are both set, the box is that rectangle and the grow / binary-search loop does not run.

Edge segments do not need that change. `from` and `to` run along the side, and `port_candidates` remaps them onto each size the search tries. A factory of blocks already requires this form: `run_factory` (`src/blocks/factory.rs`) refuses a port given by `x` and `y`.

## Two steps

1. **Sides.** A rectangle sized by the box fields. Click a side, or drag a run of its cells, and assign an item from the plan's input and output lists. This writes edge segments, and the search may still grow and shrink the box. It is the step that also works for a factory of blocks.
2. **Exact cells.** Lock width and height, paint perimeter cells and the one-tile ring, and set `dir` from the way the belt must face. Interior cells are legal and much harder to route. The natural place for a port is the perimeter.

The grid is drawn as cells, with the zoom and pan the preview already uses. It is not the result raster: that picture has a margin, a title and a legend, and it appears only after a run.

Reserved holes and forbidden tiles are the same grid later. They share the coordinate problem, and they are separate from ports.

## What a painted port can still fail

One cell is one belt. A fast item needs several cells, or the router branches with splitters onto the cells it was given. `Search::say_plan` already notes when the inserter visits exceed `max_chain_visits` times the tile count. Touching fluid cells join into one pipe, which is why a pinned fluid port is entered from just outside the box and stays on the edge (`docs/STATUS.md`, job 9). A pin that is too narrow or badly placed is a normal failed search, the same as a bad JSON job.

---
name: solidcraft-pcb
description: Use when laying out a circuit board in SolidCraft through its MCP server - outline, stack-up, fab profile, net classes, design intent, placing footprints, routing tracks and vias, pours, planes and stitching, design rule and quality checks, silkscreen and designators, getting a board ready to order - or when a board check reports findings to fix. Read reference/workflow.md next.
---

# SolidCraft PCB layout

A board is done when every connection is routed and `ecad.board.check {"stage":"release"}` shows no blocking item: no unwaived DRC error or critical quality finding, and the intent the checks need declared. Work in stages, end each with `ecad.board.check`, and fix what it lists, worst first.

## Before the first call
1. Read `reference/workflow.md`: `skill_reference {"skill":"solidcraft-pcb","file":"reference/workflow.md"}`. Topic files load the same way.
2. Run `ecad.inspect`, then `ecad.board.check`.
3. Parts and footprints come from the library skill, `solidcraft-electronics`.

## Stages
1. Plan: fab profile, outline, stack-up, net classes, intent (`ecad.intent.inspect` proposes, `ecad.intent.set` confirms).
2. Floorplan: connectors, holes, antenna, buttons where the enclosure puts them; then each block's anchor IC.
3. Place: each anchor's satellites at their pins (decoupling, crystal, TVS, switcher loop), then the rest.
4. Route: RF, switcher loops, crystals, clocks, pairs; then power; then the rest.
5. Pour and stitch.
6. Check and fix until the gate passes.
7. Labels and release.

References, under `reference/`:
- `placement.md`: stages 2-3
- `routing-dfm.md`: stage 4, fab minimums and style
- `power-ground.md`: stack-up, decoupling, switchers, planes
- `signal-integrity.md`: clocks, crystals, pairs, buses
- `rf-emc.md`: radios, antennas, ESD, cables
- `silkscreen-assembly.md`: stage 7
- `manufacturing.md`: profiles and release
- `review.md`: the reviewer's rubric
- `process.md`: blind starts, scoring, reviews, audit

## Reading a finding
`ecad.drc.inspect` and `ecad.quality.inspect` give each finding:
- a code and a class: critical, practical or cosmetic
- `measured` against a `limit`, with units and `margin`
- the `objects` it is about and where they are
- a `fix`: commands and what they do, when there is one
- a `section` to read: `power-ground#decoupling` is `reference/power-ground.md`, heading "Decoupling"

`not_applicable` means a check could not run, usually because intent is missing. It is not a pass. `ecad.quality.rules` lists every code with what it finds, why, and its parameters. Numbers in the references are defaults with their source; checks use the board's values.

## Order when rules pull apart
1. Hard constraints are never traded: connectivity, fab minimums, voltage spacing.
2. Critical loops and return paths: decoupling, switcher loops, crystals, RF, ESD.
3. Manufacturing and assembly margins.
4. SI and EMC refinements.
5. Cosmetics.

For regulators and radios, copy the vendor reference layout; deviate only with a stated reason.

## Traps
- Change copper only with `ecad.board.*` commands, a few items per batch, and read each reply's `findings_delta` before going on.
- Lower findings by fixing, never by unrouting or deleting copper: the gate checks completion first and flags drops.
- Approve a finding only with a note that says why it is fine; critical approvals wait for the user.
- A 90° track corner is a style point. An acute corner or narrow pocket is a manufacturing problem.
- Splitting a ground plane is almost never the fix; partition the placement on one ground.

# PCB layout: workflow

Contents: [Stages](#stages) · [Intent](#intent) · [The fix loop](#the-fix-loop) · [Replies and batches](#replies-and-batches) · [Approvals](#approvals) · [Done](#done) · [A worked loop](#a-worked-loop) · [Checks that point here](#checks-that-point-here)

Each stage ends with `ecad.board.check {"stage": …}` (`plan`, `place`, `route`, `release`; `route` when left out). It reports gates in a fixed order: completeness, connectivity, fab, critical, intent, practical, cosmetic. It also lists the next actions (`next`, each with a command and the section to `read`) and what it could not check (`not_verified`). When you tick an item below, write down the number the reply gave, not "OK".

## Stages

**1. Plan** (`manufacturing.md#profiles`, `power-ground.md#stack-up`)
- [ ] Choose the fab profile with `ecad.board.profile {"fab":"standard"}` (`standard`, `fine` or `hdi`; see `manufacturing.md#choosing-one`). Note the least track and gap, the least drill and the ring convention it reports.
- [ ] Draw the outline with `ecad.board.outline` and add mounting holes with `ecad.board.hole`. Take both from the mechanical drawing, never from guesses.
- [ ] Set up the stack-up with `ecad.board.stack`. Put each layer that carries fast or switching copper next to a plane. The decoupling rules depend on each plane pair's spacing (close below 0.3 mm, wide above 0.5 mm), and the decoupling findings say which regime they used.
- [ ] Add net classes with `ecad.board.netclass` (width, clearance, via, `diff_pair`, `current`). Take impedance widths from `ecad.board.impedance.inspect {"layer":"top","kind":"microstrip","target":50}` and current widths from `ecad.board.current.inspect {"current":2,"formula":"ipc2152"}`.
- [ ] Declare intent: see [Intent](#intent).
- [ ] Run `ecad.board.check {"stage":"plan"}`. It checks the footprints, the profile and the intent's blocking roles.

**2. Floorplan** (`placement.md#floorplan`)
- [ ] Place the parts that the enclosure positions (connectors, holes, switches, LEDs, display, antenna) at their drawing coordinates with `ecad.board.move {"items":["J1"],"to":[x,y],"rotation":90}`. There is no lock yet: list them in your notes and leave them where they are.
- [ ] Place the power entry and its protection and filter chain at its connector.
- [ ] Place each block's anchor IC (processor, radio, regulators). Keep blocks contiguous and the signal flow one way.

**3. Place** (`placement.md#grouping`, `power-ground.md#decoupling`)
- [ ] Place each anchor's satellites at their pins: decoupling, crystal and load caps, the switcher's input cap, inductor and output caps, TVS parts, series resistors at their drivers. Read the pin positions from `ecad.board.inspect` (each footprint's `pads`) and move each part with `ecad.board.move`.
- [ ] Run `ecad.board.loop.inspect {"kind":"decap"}` and write down the worst loop length and its estimated nH.
- [ ] Place the remaining parts, then fiducials, test points and the debug footprint.
- [ ] Run `ecad.board.check {"stage":"place"}` and expect no placement, decoupling, crystal or ESD-order blocks.

**4. Route** (`routing-dfm.md#widths-and-clearances`, then the topic files)
- [ ] Route in this order:
  1. RF (`rf-emc.md#rf-lines`)
  2. switcher hot loops (`power-ground.md#switchers`)
  3. crystals (`signal-integrity.md#crystals`)
  4. clocks (`signal-integrity.md#clocks`)
  5. pairs (`signal-integrity.md#pairs`)
  6. power, with its current (`power-ground.md#current`)
  7. everything else
- [ ] Route one connection per call with `ecad.board.track {"layer":"top","points":[[x0,y0],[x1,y1]]}`. A layer change is `via` at the last point, and the next run is one of `legs`, all in one undo step. Width and via come from the net's class when you leave them out.
- [ ] Round or chamfer corners afterwards with `ecad.board.track.miter`.
- [ ] Run `ecad.board.check {"stage":"route"}`.

**5. Pour and stitch** (`power-ground.md#pours`, `power-ground.md#stitching`)
- [ ] Make planes and ground pours with `ecad.board.pour` (priority sets which pour fills first where they overlap). Read the fills with `ecad.board.pour.inspect`.
- [ ] Stitch with `ecad.board.stitch {"net":"GND","pattern":"grid"}`, fence RF lines with `"pattern":"fence"`, and give fast layer changes a return via with `"pattern":"layer_change"`. Try `"dry_run":true` first.
- [ ] Read the return-path and stitching findings. A pour can cut a return path or leave a floating island.

**6. Check and fix**: see [The fix loop](#the-fix-loop).

**7. Labels and release** (`silkscreen-assembly.md#sizes`, `manufacturing.md#release`)
- [ ] Set the name height with `ecad.board.names {"height":1.0}` and move single names with `ecad.board.edit` (`name_at`, `show_name`). Then check that `ecad.quality.inspect {"families":["silk"]}` has no polarity, clipping or ambiguous-name findings left.
- [ ] Run `ecad.board.check {"stage":"release"}`, then ask a fresh reviewer to run `review.md`.

## Intent

The checks need to know what each net and part is for. Run `ecad.intent.inspect` first. It proposes roles with evidence and a confidence, and `{"blocking":true}` lists only those that need a look. Nothing is kept until it is accepted. Then:

1. **Accept the low-risk proposals in bulk** with `ecad.intent.set {"accept_low_risk":true,"note":"low-risk roles as proposed"}`.
2. **Confirm the blocking roles with evidence** (a datasheet section), with `accept` for a proposal that is right, or a record of your own:

   | Role | What to give |
   |---|---|
   | `power` | `voltage` and `current_a` (the regulator's or charger's setting, the load) |
   | `ground` | The ground net; the stitching and RF checks need it |
   | `clock`, `fast` | `driver` (`standard`, `high`, `lvds`, `serdes`, `clock`) or `edge_ns` |
   | `rf` | `f_mhz` and `z_target` |
   | `hv` | The peak `voltage` between nets |
   | `connector` part | `cable`: whether a cable plugs in |
   | Pairs | `p`, `n` and `interface` (`usb2_fs`, `usb2_hs`, …): `usb2_fs` and `usb2_hs` also set both nets' speed tier |
   | `decoupling` part | `of`: the IC power pins it serves, such as `U1.VDD` |
   | `crystal`, `tvs` parts | `of`: the oscillator pins or the protected connector pins |

3. **Ask the user** for facts the files cannot give, and record them with a note saying the user said so:
   - currents not stated anywhere
   - which connectors carry cables, and the enclosure
   - the radio bands and region (`board.bands`)
   - the product class: prototype or production
4. Never guess an edge rate from a clock frequency. Use the driver class: about 3 ns for standard MCU drive and 1 ns for high drive (`signal-integrity.md#edge-rates`). A net an IC drives with nothing declared is assumed standard drive, and its findings say "assumed".

A check that needs a missing field reports `not_applicable` with `needs`, for example `intent: current_a of VBUS`. Fill the field, do not approve the finding.

## The fix loop

1. Run `ecad.board.check` and take the first blocking item, critical before practical.
2. Read only the section that the finding names.
3. Write down the value you expect after the fix, for example "C12 to U1.VDD 0.6 mm, 1 via".
4. Apply the `fix` command if the finding gives one, otherwise the smallest edit that removes the cause.
5. Read the reply's `findings_delta` (`introduced`, `resolved`) and compare it with your expected value. A fix that introduces new findings is not finished. Board edits give it unless the call says `findings_delta: false` (a batch of moves you will check at the end); `truncated` in it means the quick check ran out of budget, so run `ecad.board.check`.
6. Repeat. After 3 rounds in which neither the blocking count fell nor completion rose, stop. Report what blocks to the user, with the finding keys.

Never resend a refused command unchanged. The refusal names what to change. A finding that keeps coming back after a fix usually means the intent is wrong: check the role before you move copper again.

## Replies and batches

- **Ask for summaries first:** `ecad.quality.inspect {"summary":true}` gives counts by class and code and the worst 5. Narrow a list with `codes`, `families`, `nets`, `parts`, `region`, `layer`, `status`, `limit` and `offset`. The reply says how many it left out (`more`).
- **Prefer the narrow inspects.** On an 84-part board, `ecad.board.inspect` returns about 100 KB. Use `ecad.board.net.inspect` (length, vias, bends, references, coupling, a pair's skew), `ecad.board.loop.inspect` (loop length, area, estimated nH) or `ecad.board.pour.inspect`.
- **Keep batches small**, about 5–20 edits, so a rollback costs little. Name parts by designator and pads as `U1.A20`, and copper by the ids replies give, never by guessed ids.
- **Read facts as data, never from a picture.** A picture (`ecad.export.pages` with `"format":"png"`) is for judgement only: label clutter, tidiness.

## Approvals

Approve a finding only when it is correct but acceptable, for example a datasheet layout the check does not know about:

`ecad.quality.approve {"keys":["pi.decap_distance|p3.4|p18.2"],"note":"Datasheet fig. 12 places C8 on the bottom under the pad"}`

- The note is required.
- An agent's approval of a critical finding waits for the user (`pending_user`) and still counts until the user confirms it.
- Approvals of DRC findings work the same way, with `ecad.drc.approve`.
- A threshold that does not fit the board is changed with a reason, not approved away: `ecad.quality.rules {"params":{"pi.layer_change":{"reach":3}},"note":"…"}`. Only the user lowers a critical code's level; the board check's critical gate names the parameters an agent set on critical codes.

## Done

The release gate passes when all of these hold:
- every connection is routed and every footprint is on the board
- 0 unwaived DRC errors
- 0 unwaived critical quality findings, and every critical waiver is confirmed by the user
- the intent's blocking roles are all settled
- practical findings are fixed or each has a note

Report the scorecard numbers to the user (routed, DRC errors, critical, intent coverage, vias, length), and add the `not_verified` list: what the checks could not cover.

## A worked loop

Excerpts from a routed 6-layer radio board (nRF52840 and a LoRa module), trimmed:

```
ecad.board.check {}
→ completeness pass, connectivity pass, fab fail (186), critical fail (105), intent fail (36), practical fail (204)
  next: fix the 105 critical findings: ecad.quality.inspect {"codes":[…]}, read power-ground#return-paths
ecad.quality.inspect {"codes":["si.xtal_loop","esd.path"],"limit":6}
→ si.xtal_loop  error  crystal net HFXO_XC2 goes through 2 vias (129, 131): route it on the crystal's layer with none
                fix: ecad.board.net.inspect {"nets":["HFXO_XC2"],"metrics":["layers","vias"]}, section signal-integrity#crystals
  esd.path      error  J1.A4 reaches R101.1 (VBUS) without passing U7.5: the TVS hangs off the line on a 2.54 mm branch
                measured 2.54 mm, limit 0.5 mm (esd.path.stub), section rf-emc#esd
```

Then fix the crystal: write down "HFXO_XC2: 0 vias, under 5 mm", unroute its run (`ecad.board.unroute` with the track's id), route it again on the crystal's layer with `ecad.board.track`, and read `findings_delta`: the `si.xtal_loop` key should be in `resolved`. Then fix the ESD order: route VBUS from J1 over U7's pad and only then on to R101. Check again.

## Checks that point here

The board check's completeness and intent gates, and the DRC code `unrouted`.

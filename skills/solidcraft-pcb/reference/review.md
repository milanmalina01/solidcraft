# PCB layout: review rubric

For a reviewer that did not lay out the board: a fresh agent, or a person. The checks cover what a program can decide. This rubric covers the rest, and it confirms that the checks were given the right intent.

## How to review

- **Review one dimension per pass**, in the order below.
- **Answer every item `yes`, `no` or `unknown`.**
  - A `no` names the objects and says what you would change.
  - `unknown` is a valid answer: do not invent a problem to have something to report.
- **Use data commands for facts:** `ecad.board.check`, `ecad.quality.inspect`, `ecad.board.net.inspect`, `ecad.board.loop.inspect`.
- **Use pictures only for judgement.** Print them with `ecad.export.pages` as PNG files and look at them:
  - `ecad.export.pages {"pages":["board_top","board_bottom","inner_layers"],"format":"png","path":"review/board"}` for copper and silk, layer by layer
  - `ecad.export.pages {"pages":["assembly_top","assembly_bottom"],"format":"png","path":"review/assembly"}` for the assembly drawing
  - with the app open, `screenshot` shows the board as the user sees it
- **Do not approve waivers** that the layout author proposed. List them for the user.

## 1. Intent

- Is every external connector's cable status confirmed by the user, not guessed?
- Do the power currents match the regulator, charger and load datasheets?
- Is every IC power pin's decoupling assignment right? Spot-check the three largest ICs against their datasheets.
- Are the radio bands and region declared?
- Is every `not_applicable` finding explained by a field the board genuinely does not need?

## 2. Critical loops

- Is every switcher's hot loop compact, on one layer, with no via, as the vendor layout shows it?
- Does each crystal sit within a few millimetres of its pins, with no vias and no foreign copper under it?
- Does each external connector's ESD path run connector → TVS → IC, with a short TVS ground?
- Is every RF line over unbroken ground, with its match in the reference order?

## 3. Flow and grouping

- Does each functional block sit in one area, with signals flowing one way?
- Are cable connectors on one edge, or is the area between them free of clocks and switchers?
- Are noisy blocks away from analog and from the I/O?

## 4. Tidiness (pictures)

- Do buses run as parallel groups that bend together?
- Does each layer have a clear preferred direction, with adjacent signal layers crossing at right angles?
- Are there no stair-steps, no needless detours and no acute corners?
- Do tracks enter pads from their ends, centred? Decoupling capacitors with side-entry vias are fine.
- Are vias aligned and uniform rather than scattered?

## 5. Labels (silk and assembly pictures)

- Can every printed designator be read at the board's real size?
- Is each designator unambiguous to its part?
- Is no designator under a part, and none clipped?
- Does every polarised part have a mark that stays visible after placement?
- Does the assembly drawing show every part, at a size you can read?
- Are the board name, revision and date code present?

## 6. Manufacturing

- Is the chosen fab profile the one the board will be ordered from?
- Does the class report name only features that need to be at the limit?
- Does every via in a pad have a filled-and-capped protection?

## Calibration

Before a release decision rests on this rubric, compare its answers with the user's verdicts on a handful of boards. Reword or drop any item where they disagree more than about 3 times in 10.

---
name: solidcraft-schematic
description: Use when designing or checking a circuit from requirements and a verified library - two blind designers, a code comparison of their netlists, ratings against real loads, a reviewer and an auditor. Read reference.md for the design format, the briefs and the script.
---

# From requirements to a checked circuit

The circuit is designed twice, compared by code, then attacked. Claim only what the evidence shows.

## Before you start
- A signed-off or flagged `requirements.json` (`skills/solidcraft-requirements/SKILL.md`).
- A library with evidence for each chosen part (`skills/solidcraft-datasheet/SKILL.md`). A part with no evidence is named as such in the report.

## The process
1. **Designer A and Designer B** are fresh agents, in separate folders, briefed word for word from reference.md. Each gets the requirements, the verified parts' evidence and datasheets, and may choose generic passives. Neither sees the other, any earlier design of this board, or your reasoning.
2. **Code check each.** `design_check.py check` on each design: every pin on a net or open with a reason, no one-pin net, a rating row with a page for every part, no load over a rating. Send failures back to its own designer.
3. **Compare.** `design_check.py diff A B` pairs the designs by what they connect, so names don't matter. Each difference goes on a list.
4. **Settle each difference** from a printed page or a calculation, by a third look from a fresh agent where they disagree on a fact. It names the cause. A majority isn't a cause. A choice only the person can make goes to them.
5. **Merge** into one `design.json`, noting each difference's outcome.
6. **Reviewer** is a fresh agent judging the engineering of `design.json`, briefed from reference.md. It fixes nothing. The author checks each finding on its page and answers it.
7. **Draw** it with `ecad.*` (`skills/solidcraft-electronics/SKILL.md`). Export with `ecad.netlist.export`, turn it with `design_check.py netlist`, and `diff` against `design.json`: no differences. Then `ecad.erc.inspect`: each finding fixed or explained.
8. **Auditor** is a fresh agent checking the evidence, and attacks approvals especially. It re-runs the code, checks a sample of citations on the pages, and returns pass or fail. A fail goes back, then to a NEW auditor.

Only an auditor pass with nothing open earns `status: checked`. A later change to the circuit voids it.

## Things that agreement can hide
The reviewer checks each of these whether or not the designers agreed:
- Every state: USB only, cell only, both, switch off, cell reversed, firmware as shipped, erased, crashed.
- Every I/O pin's voltage against its own supply in each of those states, including rails that are off.
- Every part's ratings against its real load, peak as well as average.
- What each output does from reset until firmware runs.
- Every note printed on the chosen parts' pages, on each part's `board_rules`.

## Seeded faults
Plant faults in a copy of the finished circuit for a new reviewer, as reference.md says. A miss is a gap in the brief or the code: fix that and report the score.

## Every escape becomes a check
A mistake found later goes into the code, or into the brief if code can't see it.

## When you can't do a step
No fresh agent: say so and report `not_checked`. Never stand in for a designer, reviewer or auditor yourself.

## Report
Short: differences and how each was settled, requirement evidence by status, ratings closest to their limit, findings, seeded score, status. Then what the person must decide.

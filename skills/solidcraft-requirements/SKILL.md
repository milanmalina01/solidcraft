---
name: solidcraft-requirements
description: Use before designing a board, to turn what the person said into numbered requirements that each later stage must show evidence against, with what nobody stated flagged.
---

# Requirements for a board

The first stage of a design. Every later stage answers to this list, so a thing left out here gets missed everywhere after it, and a thing made up here gets built.

## The process
1. **Collect the person's own words.** Pull them from the conversation or notes into one file, as quotes. Earlier proposals they accepted count, and are marked as such.
2. **Author** writes `<project>/requirements.json` from those words and the datasheets of parts already chosen. Each requirement is one checkable sentence.
3. **Reviewer** is a fresh agent with the person's words and the list, and nothing of the author's reasoning. It's strict: it looks for what's missing, untestable or invented, and returns `pass` or `revise` with numbered findings. It fixes nothing.
4. **Author** answers each finding: changes the list, or says why not. Repeat with a new reviewer when the list changed in substance.
5. **Ask the person** only what changes what they get. Everything assumed goes on the sign-off list, read to them as a short list. They can answer later: an open answer stays `assumed` and blocks release, not the work.

## The file
```json
{"project": "...", "version": 1, "status": "draft",
 "sign_off": ["35 x 37 mm outline", "..."],
 "requirements": [
  {"id": "R1", "text": "All parts on the top.",
   "source": {"kind": "user", "quote": "one-sided"},
   "check": {"stage": "layout", "how": "the placement lists no bottom part"}}
 ]}
```
`source.kind`:
- `user`: their words, quoted exactly.
- `decision`: they accepted a proposal.
- `datasheet`: a verified part's page, cited.
- `reviewer`: an engineering point the reviewer raised and the author took.
- `assumed`: nobody stated it. Always flagged.

`check.stage` is the first stage that shows evidence: `library`, `schematic`, `layout`, `release` or `person`. `check.how` says what that evidence is, specific enough that someone else could fail it.

## What a list must cover
Function and interfaces; power in every state (USB only, cell only, both, switched off, cell reversed); runtime as a sum; size, height, mass; the environment and what it's used on; connectors and what mates with them; antennas; what firmware needs from the board; assembly, fab, cost and quantity; what's explicitly out.

A number nobody gave is `assumed`, however reasonable. Things the person ruled out go on the list too, so nobody adds them back.

## Later stages
Each stage's report lists every requirement it owns with its evidence: met, not met or open. A requirement that changes goes up a version, and the stages that relied on it say whether they still hold. A change of mind from the person outranks the list: update the list first.

## When you can't do a step
No fresh agent for the review: say so and mark the list `not_reviewed`. Never stand in for the reviewer yourself.

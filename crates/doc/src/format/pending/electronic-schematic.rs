//! A design's electronics can hold a schematic (`ecad.schematic`): `parts` (each an `id`, the electronic `component` it places,
//! an optional `variant` of it by name (the component's first without one), a unique `designator`, an optional own `value`, a
//! `placement` with `at`, `rotation` and `mirror`, optional `designator_at` and `value_at` text offsets and `dnp`), `wires`
//! (`from` and `to` anchors, each a part's pin `{"part", "pin"}` or a point `[x, y]`, and `via` bends), `labels` (a net `name`,
//! a `kind` of net, rail or ground, an anchor `at` and a `rotation`), `markers` (no_connect or power_flag on an anchor), the
//! electrical rules check's `approvals` (`key`, `note`, `by`: user or agent, left out on approvals made before it was kept: not
//! known, so a critical error's waits for the user's confirmation) and `rules` (a level per finding code), and the id counter `next_id`. A
//! part's pins are those of the symbol its component uses. Older designs need no transformation: without `schematic` the sheet
//! is empty, and an empty sheet is not saved. A build without this change refuses a design that has one, and the last release
//! refuses the newer format number.

//! A design's electronics can hold a library of schematic symbols (`ecad.symbols`: each with an `id`, a `name` unique among the
//! symbols, an optional `description` and `pins`, each with a `number`, an optional `name` (and `name_hidden: true` when the
//! name is kept but not drawn, as the body's shape says it: a diode's `A` and `K`) and `electrical` type, the point `at`
//! where a wire connects, the `direction` toward the body and a `length`) and footprints (`ecad.footprints`: each with an `id`, a
//! unique `name`, an optional `description` and `pads`, each with a `number`, a centre `at`, a `size`, an optional `rotation` in
//! degrees and a `side`). An electronic component uses one symbol (`symbol`, its id) and has `variants`, each a `name` unique in
//! the component and an optional `footprint` id; many components can share one symbol or footprint. A pin connects to the pad
//! with its number. A pin's name (and a schematic label's) may mark letters for a bar over them with `~{…}` (`~{CE}`, an
//! active-low pin): the name is saved as written, matched and searched as written, and drawn with the bar. Older designs need
//! no transformation: they have no library and their electronic components have no symbol or variants. A build without this
//! change refuses a design that has them, and the last release refuses the newer format number.

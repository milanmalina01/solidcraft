//! An electronic component can have a schematic symbol (`symbol.pins`: each with a `number`, an optional `name` and `electrical`
//! type, the point `at` where a wire connects, the `direction` toward the body and a `length`) and a footprint (`footprint.pads`:
//! rectangular surface-mount pads, each with a `number`, a centre `at`, a `size`, an optional `rotation` in degrees and a `side`).
//! A pin connects to the pad with its number. Older designs need no transformation: their electronic components have no pins
//! or pads. A build without this change refuses a design that has them, and the last release refuses the newer format number.

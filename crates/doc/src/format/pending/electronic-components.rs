//! A design's electronics can hold electronic components (`ecad.components`: each with an `id`, a unique `name`, a designator
//! `prefix` and optional `value`, `mpn`, `manufacturer` and `description`) and the id counter `ecad.next_id`, one id space for
//! the components and the library's symbols, footprints and 3D packages. Older designs need no transformation: without them a
//! design has no electronic components. A build without this change refuses a design that has them, as it refuses any key
//! inside `ecad` it doesn't know, and the last release refuses the newer format number.

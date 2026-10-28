//! A design can hold electronics (`ecad`, the Electronics workspace's data): its electronic components, the schematic and the
//! circuit board, which later changes add one by one, each with its own format change. Empty electronics are not saved (no `ecad`
//! key), and older designs need no transformation: without `ecad` a design has no electronics. Keys inside `ecad` that a build
//! doesn't know are refused, not dropped. The last release (0.25, which reads formats up to 41) refuses the newer format number,
//! as it does for every design this build saves.

//! An electronic component can have a 3D package (`package`: the SolidCraft component `model` by id, an optional `translate`
//! `[x, y, z]` from the footprint's origin and a `rotation` about z in degrees). Deleting the model component clears it. Older
//! designs need no transformation: their electronic components have no 3D package. A build without this change refuses a
//! design that has one, and the last release refuses the newer format number.

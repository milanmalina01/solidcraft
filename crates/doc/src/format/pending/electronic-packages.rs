//! A design's electronics can hold a library of 3D packages (`ecad.packages`: each with an `id`, a unique `name`, the SolidCraft
//! component `model` by id, an optional `translate` `[x, y, z]` from the footprint's origin and a `rotation` about z in degrees),
//! and an electronic component's variant can use one (`package`, its id). Deleting the model component deletes the 3D package
//! and clears the variants' references to it. Older designs need no transformation: they have no 3D packages. A build without
//! this change refuses a design that has one, and the last release refuses the newer format number.

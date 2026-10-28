//! A footprint's pads can be round, oval or rounded rectangles (`shape`, with `corner` for a rounded rectangle's corner radius as
//! a share of its shorter side), plated through-hole pads or unplated holes (`kind`), with a `drill` (`[w, h]`, equal for a round
//! hole). Older designs need no transformation: their pads are rectangular surface-mount pads, which is what a pad without
//! these keys is. A build without this change refuses a design that has them, and the last release refuses the newer format
//! number.

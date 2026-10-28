//! An electronic component can map its pins to pads (`pin_map`: pin number to the pad numbers it connects to, such as a tab
//! that shares a pin), in place of connecting each pin to the pad with its number. Older designs need no transformation:
//! without `pin_map` each pin connects to the pad with its number, as before. A build without this change refuses a design
//! that has one, and the last release refuses the newer format number.

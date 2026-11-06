//! A library footprint's surface-mount pad can have a `paste` opening `[w, h]`, for a maker's stencil drawing that asks for less
//! than the whole pad. Older designs need no transformation: their paste stays the whole pad. A build without this change
//! refuses a design that has one, and the last release refuses the newer format number.

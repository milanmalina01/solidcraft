//! A symbol can have a body (`symbol.graphics`) and a footprint silkscreen, fabrication and courtyard graphics
//! (`footprint.graphics`, each with a `layer` and a `side`): lines, rectangles, circles, arcs, polygons and text, each a `type`
//! and the keys it needs, with a line `width` and an optional `fill`. Older designs need no transformation: they have no
//! graphics. A build without this change refuses a design that has them, and the last release refuses the newer format
//! number.

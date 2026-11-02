//! An electronic component can keep its LCSC part number (`ecad.components[].lcsc`, and the same in
//! an electronics library), and a board its assembly placement list's rotation offset for each
//! library footprint (`ecad.board.cpl_rotations`, by footprint name, in degrees), for the
//! assembly parts and placement lists `ecad.board.export.jlc` writes. Older designs need no
//! transformation: without them every part has no LCSC number and no footprint an offset. A build
//! without this change refuses a design with either, and the last release refuses the newer
//! format number.

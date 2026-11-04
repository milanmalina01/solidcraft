//! An electronic component can keep its LCSC part number (`ecad.components[].lcsc`, and the same in
//! an electronics library), and a library footprint its assembly placement list's rotation offset
//! (`ecad.footprints[].assembly_rotation`, in degrees, -360 to 360), for the assembly parts and
//! placement lists `ecad.board.export.jlc` writes. Older designs need no transformation: without
//! them every part has no LCSC number and no footprint an offset. A build without this change
//! refuses a design with either, and the last release refuses the newer format number.

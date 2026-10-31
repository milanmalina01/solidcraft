//! A document can say what it is (`kind`): `electronics_library` (symbols, footprints, 3D packages and the electronic components
//! that use them, in `ecad`, with bodies for the packages' models) or `electronics_design` (the schematic, later the board, and the
//! electronic components it uses). A 3D design saves no `kind`, so older designs need no transformation: without one a document is
//! a 3D design. A build without this change would read an electronics document as a 3D design and drop the key, so the last release
//! refuses the newer format number instead.

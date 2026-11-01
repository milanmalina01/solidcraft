//! A board's stitching via can keep the pattern `needed` (`ecad.board.vias[].pattern`): placed by needed stitching
//! (`ecad.board.stitch {"pattern":"needed"}`), one at a time where the net's planes were most weakly tied, so it is
//! found and removed with the others of that pattern. Older designs need no transformation: their stitching vias keep
//! the patterns they have (`grid`, `fence`, `layer_change`, or none). A build without this change refuses a design
//! with a via of this pattern, and the last release refuses the newer format number.

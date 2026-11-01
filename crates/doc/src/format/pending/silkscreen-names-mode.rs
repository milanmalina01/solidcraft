//! A board can keep the silkscreen names mode its names were last laid out by (`ecad.board.names_mode`: `full`,
//! `fit`, `minimal` or `none`, from `ecad.board.labels`), and a footprint whether its name's spot and visibility were
//! set by that mode rather than by hand (`ecad.board.footprints[].name_placed`, left out while false), so a later
//! layout moves only the names it placed. Older designs need no transformation: without `names_mode` the names are as
//! each footprint has them, and without `name_placed` every spot counts as set by hand. A build without this change
//! refuses a design that has either, and the last release refuses the newer format number.
